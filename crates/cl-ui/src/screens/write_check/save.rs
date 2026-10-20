//! Saving a check (create or edit), the closing-date password, a used
//! number (409), Void, Prev/Next over the saved checks and History.
//!
//! Money (plan §4): `check()` is the exact gate (the lines equal the check
//! amount to the cent, or nothing is sent); [`to_draft`] then copies the
//! checked values onto the engine draft one field at a time, with no
//! arithmetic; `BankPosting::check_with` refuses lines that don't add up
//! once more before anything leaves.

use std::sync::Arc;

use cl_api::{
    ApiError, AuditAction, AuditEntity, BankPosting, CheckDraft, CheckRecord,
    ClosedPeriodAuthorization, ConflictReason, DraftKey, IdempotencyKey, LocalValidation,
    OrphanLabel, PageRequest, PostingOptions, PrintStatus, RecordVersion, SplitLine, TxnStatus,
    TxnType, Versioned,
};
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Secret};
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::check::{
    CheckContext, CheckField, CheckForm, CheckProblem, CheckedCheck, NumberField, check,
};
use cl_model::forms::lines::{FormLine, FormLines};
use cl_model::forms::{SaveFailure, SaveIntent, SaveStart};

use super::{Editing, Field, LastSaved, Pending, WriteCheckScreen, blank};
use crate::company::{CompanyEvent, Task};
use crate::forms::lines_grid::{L_ACCOUNT, L_AMOUNT, L_CUSTOMER};

/// Checks are listed 50 at a time for Prev/Next.
const NAV_PAGE: u16 = 50;

/// What a save sends, for its idempotency key: the posting, the record and
/// version it edits, and whether a closing-date password goes with it
/// (never the password itself).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub posting: BankPosting,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

/// A save the engine took.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Saved {
    pub id: TransactionId,
    pub number: Option<u32>,
    pub print_status: String,
}

/// The checked check as the engine's draft, field by field (no
/// arithmetic): the amount and every line amount are the owner's typed
/// `Money`, the memo is also the printed memo (Q-P13n), and an empty name
/// on the check takes the payee's display name.
///
/// # Errors
/// `Local(InvalidValue)` when a draft attachment's id is not one the
/// engine gave.
pub fn to_draft(c: &CheckedCheck, payee_display: &str) -> Result<CheckDraft, ApiError> {
    let draft_attachment = match &c.draft_attachment {
        None => None,
        Some(a) => Some(
            DraftKey::parse(&a.id).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
        ),
    };
    Ok(CheckDraft {
        bank: c.bank,
        date: c.date,
        payee: Some(c.payee),
        payee_name: if c.payee_name.is_empty() {
            payee_display.to_string()
        } else {
            c.payee_name.clone()
        },
        check_number: c.number,
        print_later: c.print_later,
        payee_address: c.address.clone(),
        memo: c.memo.clone(),
        printed_memo: c.memo.clone(),
        tag_ids: c.header_tags.clone(),
        draft_attachment,
        amount: c.amount,
        lines: c
            .lines
            .iter()
            .map(|l| SplitLine {
                account: l.account,
                amount: l.amount,
                memo: l.memo.clone(),
                contact: l.contact,
                billable: l.billable,
                tag: l.tag,
            })
            .collect(),
    })
}

/// The form for a saved check read from the engine (exact amounts from
/// `GET /transactions/{id}`, never from a list), and whether it is shown
/// read-only.
pub(crate) fn form_of(
    r: &CheckRecord,
    keys: &mut cl_model::rowkey::RowKeyGen,
) -> (CheckForm, FormLines) {
    let bank_line = r
        .txn
        .lines
        .iter()
        .position(|l| l.credit.is_positive() && l.debit.is_zero());
    let bank = bank_line.map(|i| r.txn.lines[i].account_id);
    let amount = bank_line.map(|i| r.txn.lines[i].credit);
    let queued = r.print_status == PrintStatus::Queue;
    let number = match (queued, r.check_number) {
        (true, _) => NumberField::ToPrint {
            kept: String::new(),
            proposed: false,
        },
        (false, Some(n)) => NumberField::Typed(n.to_string()),
        (false, None) => NumberField::Typed(String::new()),
    };
    let mut f = blank(bank, r.txn.txn_date, number);
    f.print_later = queued;
    f.payee = r.txn.contact_id;
    f.payee_name.clone_from(&r.payee_name);
    f.address = r.payee_address.clone().unwrap_or_default();
    f.amount = amount;
    f.memo = r.txn.memo.clone().unwrap_or_default();
    f.header_tags.clone_from(&r.tag_ids);
    let seeds = r
        .txn
        .lines
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != bank_line)
        .map(|(_, l)| FormLine {
            account: Some(l.account_id),
            amount: Some(l.debit),
            memo: l.description.clone().unwrap_or_default(),
            customer: l.contact_id,
            billable: l.is_billable,
            tag: l.tag_id,
            ..FormLine::blank(keys.next_key())
        })
        .collect();
    (f, FormLines::from_lines(seeds, keys))
}

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

impl WriteCheckScreen {
    /// Puts the keyboard on the field a problem belongs to.
    fn focus_problem(&mut self, p: CheckProblem) {
        let lists = Arc::clone(&self.lists);
        let cell = |row, col| cl_grid::CellPos { row, col };
        match p.field() {
            CheckField::Bank => self.focus = Some(Field::Bank),
            CheckField::Number => self.focus = Some(Field::Number),
            CheckField::Date => self.focus = Some(Field::Date),
            CheckField::Payee => self.focus = Some(Field::Payee),
            CheckField::Amount => self.focus = Some(Field::Amount),
            CheckField::Address => self.focus = Some(Field::Address),
            CheckField::Lines => self.lines.focus_first(),
            CheckField::LineAccount(k) => self.lines.form_mut().focus(cell(k, L_ACCOUNT), &*lists),
            CheckField::LineAmount(k) => self.lines.form_mut().focus(cell(k, L_AMOUNT), &*lists),
            CheckField::LineCustomer(k) => {
                self.lines.form_mut().focus(cell(k, L_CUSTOMER), &*lists);
            }
        }
    }

    fn refuse(&mut self, problems: Vec<CheckProblem>) {
        if let Some(first) = problems.first().copied() {
            self.focus_problem(first);
        }
        self.save.problems = problems;
    }

    /// Checks the form and starts the save (Save & New, Save & Close, the
    /// close prompt's Save). Nothing is sent when the check doesn't pass,
    /// above all when the lines and the amount disagree by any amount.
    /// Returns true when there was nothing to save or it saved at once.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.save.task.is_some() {
            return false;
        }
        if self.editing.as_ref().is_some_and(|e| e.read_only) {
            self.save.notice = Some("check.read_only");
            return false;
        }
        // The open cell first (B4), then the header follows the lines.
        if !self.lines.commit() {
            return false;
        }
        self.left_lines();
        self.save.problems.clear();
        self.save.error = None;
        self.save.duplicate = None;
        self.save.stale = false;
        self.save.notice = None;
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let form = self.compose();
        let ctx = CheckContext {
            tags: &self.tag_list,
        };
        let checked = match check(&form, &ctx) {
            Ok(c) => c,
            Err(problems) => {
                self.refuse(problems);
                return false;
            }
        };
        let display = self
            .lists
            .name_text(checked.payee)
            .unwrap_or_default()
            .to_string();
        let draft = match to_draft(&checked, &display) {
            Ok(d) => d,
            Err(e) => {
                self.save.error = Some(e);
                return false;
            }
        };
        let posting = match BankPosting::check_with(draft.clone(), self.line_billable()) {
            Ok(p) => p,
            Err(LocalValidation::TotalMismatch) => {
                // `check()` already refuses this; kept as the second gate.
                self.refuse(vec![CheckProblem::LinesDisagree {
                    difference: cl_core::Money::ZERO,
                }]);
                return false;
            }
            Err(v) => {
                self.save.error = Some(ApiError::Local(v));
                return false;
            }
        };
        // A change touching the closed period needs the closing-date
        // password (NN8): ask before anything is sent.
        let old = self.editing.as_ref().map(|e| e.old_date);
        if password.is_none() && needs_password(old, checked.date, self.closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return false;
        }
        // Lock the form while the save runs (the grid commits again).
        let lists = Arc::clone(&self.lists);
        let _ = self.lines.form_mut().begin_save(intent, &*lists);
        if let SaveStart::Busy = self.header.begin_save(intent) {
            return false;
        }
        self.save.intent = Some(intent);
        let edit = self.editing.as_ref().map(|e| (e.id, e.version));
        if self.link.is_demo() {
            let saved = self.demo_save(&draft, edit.as_ref().map(|(id, _)| *id), password.as_ref());
            return match saved {
                Ok(s) => {
                    self.saved(s);
                    true
                }
                Err(e) => {
                    self.save_failed(e);
                    false
                }
            };
        }
        let key = self.key_for(&SaveIdentity {
            posting: posting.clone(),
            edit,
            password: password.is_some(),
        });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save_failed(ApiError::NoCompanyOpen);
            return false;
        };
        let label = OrphanLabel {
            what: "orphan.check_save",
            key: Some(key.clone()),
        };
        let check_edit = self.check_edit();
        let print_later = draft.print_later;
        let written = group.write(&session, label, move |s| async move {
            let opts = options(password);
            let queued = |q: bool| if q { "queue" } else { "hand_written" }.to_string();
            match edit {
                None => {
                    let v = s
                        .transactions()
                        .create_bank_posting(&posting, &opts, key)
                        .await?;
                    Ok(Saved {
                        id: v.record.id,
                        number: v.record.check_number.and_then(|n| u32::try_from(n).ok()),
                        print_status: queued(print_later),
                    })
                }
                Some((id, version)) if check_edit => {
                    let v = s
                        .checks()
                        .update_check(id, &version, &posting, &opts, key)
                        .await?;
                    Ok(Saved {
                        id,
                        number: v.record.check_number,
                        print_status: v.record.print_status.as_wire().to_string(),
                    })
                }
                Some((id, version)) => {
                    let v = s
                        .transactions()
                        .update_bank_posting(id, &version, &posting, &opts, key)
                        .await?;
                    Ok(Saved {
                        id,
                        number: v.record.check_number.and_then(|n| u32::try_from(n).ok()),
                        print_status: queued(print_later),
                    })
                }
            }
        });
        match written {
            Ok(p) => self.save.task = Some(Task::Pending(p)),
            Err(_) => self.save_failed(ApiError::Local(LocalValidation::InvalidValue)),
        }
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

    fn demo_save(
        &self,
        draft: &CheckDraft,
        existing: Option<TransactionId>,
        password: Option<&Secret>,
    ) -> Result<Saved, ApiError> {
        let mut demo = self
            .link
            .demo_state()
            .ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        let id = match existing {
            None => crate::company::demo::checks::create(&mut demo, draft, password)?,
            Some(id) => crate::company::demo::checks::update(&mut demo, id, draft, password)?,
        };
        let got = crate::company::demo::checks::get(&mut demo, id)
            .ok_or(ApiError::NotFound { code: None })?
            .record;
        Ok(Saved {
            id,
            number: got.check_number,
            print_status: got.print_status.as_wire().to_string(),
        })
    }

    /// Polls the save, the record being read, the checks list, Void and
    /// History.
    pub(crate) fn poll_save(&mut self) {
        if let Some(r) = self.save.task.as_mut().and_then(Task::try_take) {
            self.save.task = None;
            match r {
                Ok(s) => self.saved(s),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            match r {
                Ok(v) => self.show_record(&v),
                Err(e) => self.save.error = Some(e),
            }
        }
        if let Some(r) = self.nav.task.as_mut().and_then(Task::try_take) {
            self.nav.task = None;
            match r {
                Ok(rows) => {
                    self.nav.done = rows.len() < usize::from(NAV_PAGE);
                    self.nav.list.extend(rows);
                    if let Some(older) = self.nav.want_older.take() {
                        self.step(older);
                    }
                }
                Err(e) => {
                    self.nav.want_older = None;
                    self.save.error = Some(e);
                }
            }
        }
        if let Some(r) = self.void.task.as_mut().and_then(Task::try_take) {
            self.void.task = None;
            match r {
                Ok(()) => self.voided(),
                Err(e) => {
                    let reason = self.void.reason.clone();
                    if !self.closing_failed(&e, Pending::Void(reason)) {
                        self.save.error = Some(e);
                    }
                }
            }
        }
        if let Some(r) = self.history.task.as_mut().and_then(Task::try_take) {
            self.history.task = None;
            match r {
                Ok(rows) => self.history.rows = rows,
                Err(_) => self.history.error = true,
            }
        }
    }

    fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        self.closing_prompt
            .failed(e, pending, supported, self.closing)
    }

    /// The engine took the save.
    fn saved(&mut self, s: Saved) {
        self.closing_prompt.saved();
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        self.save.count += 1;
        self.save.last = Some(LastSaved {
            number: s.number,
            print_status: s.print_status,
        });
        self.save.last_action = Some("save");
        self.save.key = None;
        self.save.problems.clear();
        self.save.error = None;
        let lists = Arc::clone(&self.lists);
        let header = self.header.current().clone();
        self.header.save_ok(header);
        let lines = self.lines.value().clone();
        self.lines.form_mut().save_ok(lines, &*lists);
        self.nav = super::Nav::default();
        self.follow_up(intent);
        // The engine's balance and next number after the save.
        self.fetch_bank_facts();
    }

    /// After a save (or with nothing to save): a new check, close, or stay.
    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_check(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(cl_model::CommandId::new(
                        super::KIND,
                    )));
            }
            SaveIntent::Save => {}
        }
    }

    /// A blank check. Save & New keeps the bank account and the date and
    /// proposes the next number; Clear starts from today.
    pub(crate) fn new_check(&mut self, keep_date: bool) {
        let current = self.header.current();
        let bank = current.bank;
        let date = if keep_date {
            current.date.unwrap_or(self.today)
        } else {
            self.today
        };
        let number = self
            .proposal
            .map_or_else(|| NumberField::Typed(String::new()), NumberField::Proposed);
        let lists = Arc::clone(&self.lists);
        self.header.load(blank(bank, date, number));
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().load(fresh, &*lists);
        self.editing = None;
        self.nav.at = None;
        self.void = super::VoidPrompt::default();
        self.history = super::HistoryView::default();
        self.save.problems.clear();
        self.save.duplicate = None;
        self.save.stale = false;
        self.focus = Some(Field::Payee);
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away and start a new check.
    pub(crate) fn clear(&mut self) {
        if self.save.task.is_some() {
            return;
        }
        self.save.error = None;
        self.save.notice = None;
        self.save.last_action = Some("clear");
        self.new_check(false);
    }

    fn save_failed(&mut self, e: ApiError) {
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        if !crate::screens::register::keeps_key(&e) {
            self.save.key = None;
        }
        let failure = SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        self.header.save_failed(failure.clone());
        self.lines.form_mut().save_failed(failure);
        // A closed-period refusal or a wrong password shows in the
        // closing-date dialog, which can send the save again.
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        match &e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.save.stale = true,
            ApiError::Conflict { .. } => {
                if let Some(n) = self.number_sent() {
                    // A re-proposal for the owner to take, never a silent
                    // change (plan WP6.2).
                    self.save.duplicate = Some((n, n.saturating_add(1)));
                    self.focus = Some(Field::Number);
                    self.fetch_bank_facts();
                } else {
                    self.save.error = Some(e);
                }
            }
            _ => self.save.error = Some(e),
        }
    }

    /// The number this check would be saved with.
    fn number_sent(&self) -> Option<u32> {
        let f = self.header.current();
        if f.print_later {
            return None;
        }
        match &f.number {
            NumberField::Proposed(n) => Some(*n),
            NumberField::Typed(s) => s.trim().parse().ok(),
            NumberField::ToPrint { .. } => None,
        }
    }

    /// "Use 4419" after a used number.
    pub(crate) fn use_offered_number(&mut self) {
        let Some((_, offer)) = self.save.duplicate.take() else {
            return;
        };
        self.header
            .edit_step(|h| h.number = NumberField::Typed(offer.to_string()));
        self.sync_inputs();
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
                self.save.task.is_some() || !self.closing_prompt.dialog_open()
            }
            Pending::Void(reason) => {
                self.send_void(&reason, Some(secret));
                true
            }
        };
        if !sent {
            self.closing_prompt.nothing_sent();
        }
    }

    // --------------------------------------------------------------------
    // Editing a saved check: Prev/Next, loading, Void, History.

    /// Prev (older) or Next (newer) over the bank account's checks.
    pub(crate) fn navigate(&mut self, older: bool) {
        if self.save.task.is_some() || self.load_task.is_some() {
            return;
        }
        if self.is_dirty() {
            self.save.notice = Some("check.nav.unsaved");
            return;
        }
        if self.nav.bank != self.bank() {
            self.nav = super::Nav {
                bank: self.bank(),
                ..super::Nav::default()
            };
        }
        if self.nav.list.is_empty() && !self.nav.done {
            self.nav.want_older = Some(older);
            self.fetch_checks();
            return;
        }
        self.step(older);
    }

    fn step(&mut self, older: bool) {
        let target = match (self.nav.at, older) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => self.new_check(true),
            Some(i) if i < self.nav.list.len() => {
                self.nav.at = Some(i);
                let id = self.nav.list[i].id;
                self.load_check(id);
            }
            Some(_) if !self.nav.done => {
                self.nav.want_older = Some(older);
                self.fetch_checks();
            }
            Some(_) => self.save.notice = Some("check.nav.oldest"),
        }
    }

    fn fetch_checks(&mut self) {
        let Some(bank) = self.bank() else { return };
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let all = crate::company::demo::checks::list(&mut demo, bank);
            drop(demo);
            let rows: Vec<_> = all
                .into_iter()
                .skip(self.nav.list.len())
                .take(usize::from(NAV_PAGE))
                .collect();
            self.nav.task = Some(Task::ready(rows));
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let offset = self.nav.list.len() as u64;
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = PageRequest::new(NAV_PAGE, offset)?;
            s.checks().list(Some(bank), None, None, page).await
        }) {
            self.nav.task = Some(Task::Pending(p));
        }
    }

    /// Reads a saved check into the form.
    pub(crate) fn load_check(&mut self, id: TransactionId) {
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let got = crate::company::demo::checks::get(&mut demo, id);
            drop(demo);
            match got {
                Some(v) => self.show_record(&v),
                None => self.save.error = Some(ApiError::NotFound { code: None }),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move { s.checks().get(id).await }) {
            self.load_task = Some(Task::Pending(p));
        }
    }

    fn show_record(&mut self, v: &Versioned<CheckRecord>) {
        let r = &v.record;
        let lists = Arc::clone(&self.lists);
        let (form, lines) = form_of(r, &mut self.lines.form_mut().adapter_mut().keys);
        self.header.load(form);
        self.lines.form_mut().load(lines, &*lists);
        self.editing = Some(Editing {
            id: r.txn.id,
            version: v.version,
            old_date: r.txn.txn_date,
            print_status: r.print_status.clone(),
            read_only: r.txn.status == TxnStatus::Void || r.txn.txn_type == TxnType::BillPayment,
        });
        self.save.problems.clear();
        self.save.error = None;
        self.save.stale = false;
        self.save.last_action = Some("load");
        self.history = super::HistoryView::default();
        self.void = super::VoidPrompt::default();
        self.sync_inputs();
    }

    /// The edited check was changed elsewhere: read it again.
    pub(crate) fn reload(&mut self) {
        if let Some(id) = self.editing.as_ref().map(|e| e.id) {
            self.save.stale = false;
            self.load_check(id);
        }
    }

    /// The saved check is dated on or before the closing date.
    pub(crate) fn editing_locked(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| is_locked(e.old_date, self.closing))
    }

    /// Edit > Void Transaction: only for a saved check.
    pub(crate) fn start_void(&mut self) -> bool {
        match &self.editing {
            Some(e) if !e.read_only => {
                self.void.open = true;
                self.void.reason.clear();
            }
            Some(_) => self.save.notice = Some("check.void.already"),
            None => self.save.notice = Some("check.void.saved_only"),
        }
        true
    }

    /// Void with the typed reason (and the closing-date password when the
    /// check is in the closed period).
    pub(crate) fn send_void(&mut self, reason: &str, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            self.save.notice = Some("check.void.reason_needed");
            return;
        }
        self.void.reason.clone_from(&reason);
        if password.is_none() && is_locked(e.old_date, self.closing) {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Void(reason), supported, self.closing);
            } else {
                self.closing_prompt.unavailable(supported, self.closing);
            }
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(mut demo) = link.demo_state() {
            let r = crate::company::demo::checks::void(&mut demo, e.id, password.as_ref());
            drop(demo);
            self.void.task = Some(match r {
                Ok(()) => Task::ready(()),
                Err(err) => {
                    if !self.closing_failed(&err, Pending::Void(reason)) {
                        self.save.error = Some(err);
                    }
                    return;
                }
            });
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.check_void",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.transactions()
                .void(e.id, &reason, &options(password), key)
                .await?;
            Ok(())
        }) {
            self.void.task = Some(Task::Pending(p));
        }
    }

    fn voided(&mut self) {
        self.closing_prompt.saved();
        self.void = super::VoidPrompt::default();
        self.save.last_action = Some("void");
        self.nav = super::Nav::default();
        self.new_check(true);
        self.fetch_bank_facts();
    }

    /// History of the saved check (when it was created, changed, voided).
    pub(crate) fn toggle_history(&mut self) {
        if self.history.open {
            self.history = super::HistoryView::default();
            return;
        }
        let Some(id) = self.editing.as_ref().map(|e| e.id) else {
            self.save.notice = Some("check.history.saved_only");
            return;
        };
        self.history.open = true;
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let rows = crate::company::demo::checks::history(&demo, id)
                .into_iter()
                .map(|h| crate::i18n::t(action_key_of(h.action)))
                .collect();
            drop(demo);
            self.history.task = Some(Task::ready(rows));
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let uuid = *id.as_uuid();
        if let Ok(p) = group.read(&session, move |s| async move {
            let rows = s.audit().for_record(AuditEntity::Transaction, uuid).await?;
            Ok(rows
                .into_iter()
                .map(|r| {
                    let day = CalendarDate::from_naive(r.at.as_utc().date_naive())
                        .map(crate::shell::us_date)
                        .unwrap_or_default();
                    format!("{day} \u{b7} {}", crate::i18n::t(action_key(&r.action)))
                })
                .collect())
        }) {
            self.history.task = Some(Task::Pending(p));
        }
    }
}

fn action_key(a: &AuditAction) -> &'static str {
    match a {
        AuditAction::Create => "check.history.create",
        AuditAction::Update => "check.history.update",
        AuditAction::Void => "check.history.void",
        AuditAction::Delete => "check.history.delete",
        AuditAction::Override => "check.history.override",
        AuditAction::Other(_) => "check.history.other",
    }
}

fn action_key_of(a: &str) -> &'static str {
    match a {
        "create" => "check.history.create",
        "update" => "check.history.update",
        "void" => "check.history.void",
        _ => "check.history.other",
    }
}
