//! Posting a day: `check()` (the exact gate) → a draft (created, or the
//! kept draft updated) → the engine's preview → post only when the
//! preview's over/short equals the form's to the cent and nothing is
//! unmapped (Q-P14w). The closing-date password (`daily-sales-lock-v1`),
//! Void, Clear, and the Q-262 notice after a post or a void.
//!
//! A lost reply on the draft runs the outcome probe (the drafts of that
//! layout and day) before anything is sent again; a failed post keeps the
//! draft, which the next save posts (Q-P14u).

use cl_api::{
    ApiError, DailyEntry, DailyEntryDraft, DailyEntryQuery, DailyValue, EntryStatus, IdempotencyKey,
};
use cl_core::ids::{AccountId, DailyEntryId};
use cl_core::{Money, Secret};
use cl_model::closing::{ClosingStage, is_locked, needs_password};
use cl_model::forms::daily_sales::{CheckedDaily, DailySalesForm, check};
use cl_model::forms::{SaveFailure, SaveIntent};

use super::{
    Call, DailyApi, DailySalesScreen, Field, Pending, Reply, Saved, VoidState, system_account,
};
use crate::company::{CompanyEvent, Task};

/// One save: the draft to write and the over/short its preview must have.
pub(crate) struct Plan {
    /// The kept draft (a failed post or a mismatch), or `None` for a new one.
    pub draft: Option<DailyEntryId>,
    pub body: DailyEntryDraft,
    /// The form's exact over/short (debits − credits).
    pub expected: Money,
    pub password: Option<Secret>,
    pub key: IdempotencyKey,
    /// Drafts the form already knew of (the outcome probe skips them).
    pub known: Vec<DailyEntryId>,
}

/// How a save ended.
pub(crate) enum Outcome {
    /// Posted.
    Posted(DailyEntry),
    /// The engine's preview disagreed with the form (the layout changed
    /// elsewhere) or a line is unmapped: nothing posted; the draft is kept.
    Mismatch { draft: DailyEntryId },
    /// The draft is saved but the post failed: kept for "Try again".
    NotPosted {
        draft: DailyEntryId,
        error: ApiError,
    },
}

/// What a save sends, for its idempotency key (never the password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub body: DailyEntryDraft,
    pub draft: Option<DailyEntryId>,
    pub password: bool,
}

/// The save, over the engine or the books.
pub(crate) async fn flow<A: DailyApi>(api: &A, plan: Plan) -> Result<Outcome, ApiError> {
    let entry = match plan.draft {
        Some(id) => match api.update_draft(id, &plan.body, plan.key).await {
            Ok(e) => e,
            // Posted elsewhere meanwhile: read it again; done.
            Err(ApiError::Validation(v)) => {
                let e = api.entry(id).await?;
                if e.status == EntryStatus::Posted {
                    return Ok(Outcome::Posted(e));
                }
                return Err(ApiError::Validation(v));
            }
            Err(e) => return Err(e),
        },
        None => match api.create_draft(&plan.body, plan.key).await {
            Ok(e) => e,
            Err(lost @ ApiError::OutcomeUnknown { .. }) => {
                // The outcome probe: a draft of this layout and day the
                // form did not know is the one just written.
                let q = DailyEntryQuery {
                    status: Some(EntryStatus::Draft),
                    template: Some(plan.body.template),
                    date: Some(plan.body.date),
                    limit: Some(50),
                };
                let found = api
                    .list(&q)
                    .await?
                    .into_iter()
                    .find(|d| !plan.known.contains(&d.id));
                match found {
                    Some(d) => {
                        api.update_draft(d.id, &plan.body, IdempotencyKey::new())
                            .await?
                    }
                    None => return Err(lost),
                }
            }
            Err(e) => return Err(e),
        },
    };
    let preview = match api.preview(plan.body.template, &plan.body.values).await {
        Ok(p) => p,
        Err(error) => {
            return Ok(Outcome::NotPosted {
                draft: entry.id,
                error,
            });
        }
    };
    if preview.over_short != plan.expected || !preview.unmapped.is_empty() {
        return Ok(Outcome::Mismatch { draft: entry.id });
    }
    match api.post(entry.id, plan.password).await {
        Ok(posted) => Ok(Outcome::Posted(posted)),
        Err(error) => Ok(Outcome::NotPosted {
            draft: entry.id,
            error,
        }),
    }
}

/// The checked day as the engine's draft, field by field (no arithmetic).
pub(crate) fn to_draft(c: &CheckedDaily) -> DailyEntryDraft {
    DailyEntryDraft {
        template: c.template,
        date: c.date,
        tag: c.tag,
        notes: c.notes.clone(),
        values: c
            .values
            .iter()
            .map(|(line, amount)| DailyValue {
                line: *line,
                amount: *amount,
                tag: None,
            })
            .collect(),
    }
}

impl DailySalesScreen {
    /// Save & New / Save & Close / Save: check, then post (see the module
    /// docs). True when the form is done (nothing was waiting).
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.save.task.is_some() || self.setup.is_some() {
            return false;
        }
        self.save.error = None;
        self.save.notice = None;
        if self.read_only() {
            self.follow_up(intent);
            return true;
        }
        let draft = self.saved.as_ref().map(|s| s.id);
        if !self.is_dirty() && draft.is_none() && self.header.current().values.is_empty() {
            self.follow_up(intent);
            return true;
        }
        let form = self.header.current().clone();
        let checked = match check(&form, &self.layout) {
            Ok(c) => c,
            Err(problems) => {
                self.save.problems = problems;
                return false;
            }
        };
        self.save.problems.clear();
        // A day in the closed period needs the closing-date password (NN8):
        // asked before anything is sent; refused plainly without the
        // engine's lock (Q-P14v).
        let old = self.saved.as_ref().map(|s| s.date);
        if password.is_none() && needs_password(old, checked.date, self.closing) {
            let supported = self.lock_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return false;
        }
        let body = to_draft(&checked);
        let key = self.key_for(&SaveIdentity {
            body: body.clone(),
            draft,
            password: password.is_some(),
        });
        let known = self.panel.drafts.iter().map(|d| d.id).collect();
        let plan = super::save::Plan {
            draft,
            body,
            expected: checked.expected_over_short,
            password,
            key,
            known,
        };
        self.save.intent = Some(intent);
        self.save.task = self.run(Call::Save(plan), true);
        if self.save.task.is_none() {
            self.save_failed(ApiError::NoCompanyOpen);
            return false;
        }
        // The books answer at once.
        self.poll_save();
        false
    }

    fn key_for(&mut self, id: &SaveIdentity) -> IdempotencyKey {
        match &self.save.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.save.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    pub(crate) fn poll_save(&mut self) {
        if let Some(r) = self.save.task.as_mut().and_then(Task::poll_result) {
            self.save.task = None;
            match r {
                Ok(Reply::Outcome(Outcome::Posted(e))) => self.posted(&e),
                Ok(Reply::Outcome(Outcome::Mismatch { draft })) => self.mismatch(draft),
                Ok(Reply::Outcome(Outcome::NotPosted { draft, error })) => {
                    self.keep_draft(draft);
                    self.save.last_action = Some("not_posted");
                    self.save_failed(error);
                    if self.save.error.is_some() {
                        self.save.notice = Some("daily.not_posted");
                    }
                }
                Ok(_) => {}
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(r) = self.void.task.as_mut().and_then(Task::poll_result) {
            self.void.task = None;
            match r {
                Ok(Reply::Entry(e)) => self.voided(&e),
                Ok(_) => {}
                Err(e) => {
                    let supported = self.lock_supported();
                    if !self
                        .closing_prompt
                        .failed(&e, Pending::Void, supported, self.closing)
                    {
                        self.save.error = Some(e);
                    }
                }
            }
        }
    }

    /// The draft the engine now has for this day (kept for the next save).
    fn keep_draft(&mut self, draft: DailyEntryId) {
        let date = self.header.current().date.unwrap_or(self.today);
        self.saved = Some(Saved {
            id: draft,
            status: EntryStatus::Draft,
            transaction: None,
            date,
        });
        self.load_panel();
    }

    /// The preview disagreed: nothing posted; the layout is read again.
    fn mismatch(&mut self, draft: DailyEntryId) {
        self.keep_draft(draft);
        self.save.intent = None;
        self.save.last_action = Some("mismatch");
        self.save.notice = Some("daily.layout_changed");
        if let Some(id) = self.header.current().template {
            self.load_template(id);
        }
    }

    /// The accounts a post or a void touched (Q-262): every mapped line
    /// with an amount and the engine's Cash Over/Short when the day is not
    /// even. Empty (any account) when that account is not known here.
    pub(crate) fn touched(&self, form: &DailySalesForm) -> Vec<AccountId> {
        let mut out: Vec<AccountId> = self
            .layout
            .lines
            .iter()
            .filter(|l| {
                l.active
                    && l.id
                        .and_then(|id| form.values.get(&id).copied().flatten())
                        .is_some_and(|m| !m.is_zero())
            })
            .filter_map(|l| l.account)
            .collect();
        let uneven = cl_model::forms::daily_sales::totals(form, &self.layout)
            .is_ok_and(|t| !t.signed_over_short.is_zero());
        if uneven {
            match system_account(&self.accounts, "cash_over_short") {
                Some(a) => out.push(a),
                None => return Vec::new(),
            }
        }
        out
    }

    /// The engine posted the day.
    fn posted(&mut self, e: &DailyEntry) {
        let form = self.header.current().clone();
        crate::forms::publish_saved(&self.link, &self.bus, &self.touched(&form));
        self.closing_prompt.saved();
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        self.save.count += 1;
        self.save.key = None;
        self.save.problems.clear();
        self.save.error = None;
        self.save.notice = None;
        self.save.last_action = Some("post");
        self.header.save_ok(form);
        self.saved = Some(Saved {
            id: e.id,
            status: e.status.clone(),
            transaction: e.transaction,
            date: e.date,
        });
        self.load_panel();
        self.follow_up(intent);
    }

    /// After a save (or with nothing to save): the next day, close, or stay.
    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => {
                let date = self.header.current().date.unwrap_or(self.today);
                let next = date.add_days(1).unwrap_or(date);
                self.new_day(next);
            }
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(cl_model::CommandId::new(
                        super::KIND,
                    )));
            }
            SaveIntent::Save => {}
        }
    }

    /// A blank day on the same layout.
    pub(crate) fn new_day(&mut self, date: cl_core::CalendarDate) {
        let template = self.header.current().template;
        let tag = self.header.current().tag;
        self.header.load(DailySalesForm {
            template,
            date: Some(date),
            tag,
            ..DailySalesForm::default()
        });
        self.saved = None;
        self.void = VoidState::default();
        self.save.problems.clear();
        self.save.key = None;
        self.focus = Some(Field::Date);
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away and start a new day (today).
    pub(crate) fn clear(&mut self) {
        if self.save.task.is_some() {
            return;
        }
        self.save.error = None;
        self.save.notice = None;
        self.save.last_action = Some("clear");
        self.new_day(self.today);
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        if !crate::screens::register::keeps_key(&e) {
            self.save.key = None;
        }
        self.header.save_failed(SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        let supported = self.lock_supported();
        if self
            .closing_prompt
            .failed(&e, Pending::Save(intent), supported, self.closing)
        {
            return;
        }
        self.save.error = Some(e);
    }

    /// Void (Edit menu): asks first; only a posted day.
    pub(crate) fn start_void(&mut self) -> bool {
        let posted = self
            .saved
            .as_ref()
            .is_some_and(|s| s.status == EntryStatus::Posted);
        if !posted {
            self.save.notice = Some("daily.void.posted_only");
            return true;
        }
        self.void.open = true;
        true
    }

    /// Void confirmed (with the closing-date password when the day is in
    /// the closed period).
    pub(crate) fn send_void(&mut self, password: Option<Secret>) {
        let Some(s) = self.saved.clone() else { return };
        if password.is_none() && is_locked(s.date, self.closing) {
            let supported = self.lock_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Void, supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return;
        }
        self.void.task = self.run(Call::Void(s.id, password), true);
        if self.void.task.is_none() {
            self.save.error = Some(ApiError::NoCompanyOpen);
        }
        self.poll_save();
    }

    fn voided(&mut self, e: &DailyEntry) {
        let form = self.header.current().clone();
        crate::forms::publish_saved(&self.link, &self.bus, &self.touched(&form));
        self.closing_prompt.saved();
        self.void = VoidState::default();
        self.save.last_action = Some("void");
        if let Some(s) = self.saved.as_mut() {
            s.status = e.status.clone();
        }
        self.load_panel();
    }

    /// The closing-date dialog's Change: the pending change with the
    /// password.
    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        let sent = match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                self.save.task.is_some() || self.closing_prompt.stage() != ClosingStage::Sending
            }
            Pending::Void => {
                self.send_void(Some(secret));
                true
            }
        };
        if !sent {
            self.closing_prompt.nothing_sent();
        }
    }
}
