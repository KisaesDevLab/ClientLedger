//! Saving, voiding, deleting and moving between saved journal entries.
//!
//! The gate is `cl_model::forms::journal::check` (balanced exactly, the
//! A/R-A/P rule, the dates), then `NewJournalEntry::new_with` checks again
//! before anything is sent. The checked value maps field by field onto the
//! engine's entry; nothing is added up here.

use std::sync::Arc;

use cl_api::{
    ApiError, Basis, ClosedPeriodAuthorization, ConflictReason, DeleteOptions, IdempotencyKey,
    JE_AR_AP_NAME_REQUIRED, JE_AR_AP_ONE_PER_ENTRY, JeLine, JeOptions, LocalValidation,
    NewJournalEntry, OrphanLabel, PageRequest, PostingOptions, REVERSAL_EDITED, RecordVersion,
    Side, SortDir, TransactionFilter, TxnSort, TxnType, has_code,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::SaveIntent;
use cl_model::forms::journal::{CheckedJournal, JeSide, JournalField, JournalProblem, check};

use super::{Editing, JournalScreen, KIND, Pending, SaveIdentity, field};
use crate::company::{CompanyEvent, Task};
use crate::screens::company::form::DateInput;
use crate::windows::PromptAnswer;

/// How many entries Prev/Next looks through.
const LIST_PAGE: u16 = 200;

fn options(password: Option<Secret>) -> PostingOptions {
    PostingOptions {
        closed_period: password.map(ClosedPeriodAuthorization::with_password),
    }
}

/// A delete's answer. On the `retry` of a delete whose reply was lost,
/// "not found" (or "already deleted") means the first try took: done.
pub(crate) fn delete_answer(r: Result<(), ApiError>, retry: bool) -> Result<(), ApiError> {
    match r {
        Err(
            ApiError::NotFound { .. }
            | ApiError::Conflict {
                reason: cl_api::ConflictReason::AlreadyDeleted,
                ..
            },
        ) if retry => Ok(()),
        r => r,
    }
}

impl JournalScreen {
    /// The checked form as the engine's entry, field by field.
    ///
    /// # Errors
    /// What `NewJournalEntry::new_with` refuses (nothing is sent).
    pub(crate) fn entry_of(
        &self,
        c: &CheckedJournal,
        basis: Basis,
    ) -> Result<NewJournalEntry, LocalValidation> {
        let lines = c
            .lines
            .iter()
            .map(|l| JeLine {
                account: l.account,
                side: match l.side {
                    JeSide::Debit(m) => Side::Debit(m),
                    JeSide::Credit(m) => Side::Credit(m),
                },
                description: l.memo.clone(),
                tag: l.tag,
                contact: l.contact,
            })
            .collect();
        NewJournalEntry::new_with(
            c.date,
            c.memo.clone(),
            basis,
            lines,
            JeOptions {
                number: c.number.clone(),
                adjusting: c.adjusting,
                reverse_on: c.reverse_on,
            },
            &self.roles(),
        )
    }

    /// Commits the dates still being typed. False when a typed date is not
    /// a date.
    fn commit_header(&mut self) -> bool {
        let today = self.today;
        let date_text = self
            .header
            .current()
            .date
            .map(|d| DateInput::new(d).text)
            .unwrap_or_default();
        if self.inputs.date.text.trim() != date_text {
            self.inputs.date.reparse(today);
            let d = self.inputs.date.value;
            if d.is_none() {
                return false;
            }
            self.header.edit(field::DATE, |h| h.date = d);
        }
        let reverse_text = self
            .header
            .current()
            .reverse_on
            .map(|d| DateInput::new(d).text)
            .unwrap_or_default();
        if self.inputs.reverse.text.trim() != reverse_text {
            self.inputs.reverse.reparse(today);
            let d = self.inputs.reverse.value;
            if d.is_none() && !self.inputs.reverse.text.trim().is_empty() {
                return false;
            }
            self.header.edit(field::REVERSE, |h| h.reverse_on = d);
        }
        self.inputs.rev = self.header.revision();
        true
    }

    /// Checks the form and starts the save. Nothing is sent when the check
    /// doesn't pass (above all when debits and credits differ by any
    /// amount). True when it saved at once or had nothing to save.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.busy() || (self.closing_prompt.dialog_open() && password.is_none()) {
            return false;
        }
        // The last save's outcome is unknown: nothing is sent until the
        // recheck answers or the owner confirms (B2).
        if self.header.is_unconfirmed() {
            return false;
        }
        if let Some(note) = self.read_only() {
            self.notice = Some(note);
            return false;
        }
        if !self.commit_header() || !self.lines.commit() {
            return false;
        }
        self.problems.clear();
        self.error = None;
        self.notice = None;
        self.stale = false;
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let checked = match check(&self.compose(), &self.context()) {
            Ok(c) => c,
            Err(p) => {
                self.refuse(p);
                return false;
            }
        };
        let basis = self.editing.as_ref().map_or(self.new_basis, |e| e.basis);
        let entry = match self.entry_of(&checked, basis) {
            Ok(e) => e,
            Err(v) => {
                self.local_refusal(v);
                return false;
            }
        };
        let closing = self.books.closing;
        let old = self.editing.as_ref().map(|e| e.old_date);
        let old_reversal = self
            .editing
            .as_ref()
            .and_then(|e| e.reversal)
            .map(|(_, d)| d);
        let locked = needs_password(old, checked.date, closing)
            || [checked.reverse_on, old_reversal]
                .into_iter()
                .flatten()
                .any(|d| is_locked(d, closing));
        if password.is_none() && locked {
            let supported = self.closing_supported();
            if supported {
                self.closing_prompt
                    .ask(Pending::Save(intent), supported, closing);
            } else {
                self.closing_prompt.unavailable(supported, closing);
            }
            return false;
        }
        let lists = Arc::clone(&self.books.lists);
        let _ = self.lines.form_mut().begin_save(intent, &*lists);
        let _ = self.header.begin_save(intent);
        self.intent = Some(intent);
        let mut touched: Vec<AccountId> = checked.lines.iter().map(|l| l.account).collect();
        if let Some(e) = &self.editing {
            touched.extend(e.accounts.iter().copied());
        }
        self.published = touched;
        let edit = self.editing.as_ref().map(|e| (e.id, e.version));
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            return match local.save(&link, &entry, edit, password.as_ref()) {
                Ok((id, v)) => {
                    self.saved(id, v);
                    true
                }
                Err(e) => {
                    self.save_failed(e);
                    false
                }
            };
        }
        let key = self.key_for(&SaveIdentity {
            entry: entry.clone(),
            edit,
            password: password.is_some(),
        });
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.save_failed(ApiError::NoCompanyOpen);
            return false;
        };
        let label = OrphanLabel {
            what: "orphan.journal_save",
            key: Some(key.clone()),
        };
        match group.write(&session, label, move |s| async move {
            let opts = options(password);
            let v = match edit {
                None => {
                    s.transactions()
                        .create_journal_entry(&entry, &opts, key)
                        .await?
                }
                Some((id, version)) => {
                    s.transactions()
                        .update_journal_entry(id, &version, &entry, &opts, key)
                        .await?
                }
            };
            Ok((v.record.id, v.version))
        }) {
            Ok(p) => self.save_task = Some(Task::Pending(p)),
            Err(_) => self.save_failed(ApiError::Local(LocalValidation::InvalidValue)),
        }
        false
    }

    /// The problems of a refused save; the first one's field gets the
    /// cursor, and an unbalanced entry says so in the status bar too.
    pub(crate) fn refuse(&mut self, problems: Vec<JournalProblem>) {
        if problems
            .iter()
            .any(|p| matches!(p, JournalProblem::Unbalanced { .. }))
        {
            self.link
                .push(CompanyEvent::Toast("journal.problem.unbalanced"));
        }
        if let Some(first) = problems.first() {
            let lists = Arc::clone(&self.books.lists);
            let cell = |row, col| cl_grid::CellPos { row, col };
            match first.field() {
                JournalField::LineAccount(k) => self
                    .lines
                    .form_mut()
                    .focus(cell(k, crate::forms::lines_grid::L_ACCOUNT), &*lists),
                JournalField::LineAmount(k) => self
                    .lines
                    .form_mut()
                    .focus(cell(k, crate::forms::lines_grid::L_AMOUNT), &*lists),
                JournalField::LineName(k) => self
                    .lines
                    .form_mut()
                    .focus(cell(k, crate::forms::lines_grid::L_CUSTOMER), &*lists),
                JournalField::Date
                | JournalField::Number
                | JournalField::ReverseOn
                | JournalField::Adjusting
                | JournalField::Lines => {}
            }
        }
        self.problems = problems;
    }

    /// A refusal from `NewJournalEntry::new_with` or the demo engine, in
    /// the form's own words.
    fn local_refusal(&mut self, v: LocalValidation) {
        let p = match v {
            LocalValidation::Unbalanced => {
                let difference = cl_model::forms::journal::totals(self.lines.value())
                    .map_or(cl_core::Money::ZERO, |t| t.difference);
                Some(JournalProblem::Unbalanced { difference })
            }
            LocalValidation::TooFewLines => Some(JournalProblem::TooFewLines),
            LocalValidation::TooManyLines => Some(JournalProblem::TooManyLines),
            LocalValidation::ReverseNotAfter => Some(JournalProblem::ReverseNotAfter),
            LocalValidation::ArApNameMissing => {
                self.notice = Some("journal.problem.ar_ap_name_missing");
                None
            }
            other => {
                self.error = Some(ApiError::Local(other));
                None
            }
        };
        if let Some(p) = p {
            self.refuse(vec![p]);
        }
    }

    /// The key for this save: the stored one when the same save (same
    /// entry, same record version, same password-or-not) goes again after
    /// an unknown outcome, a fresh one otherwise.
    pub(crate) fn key_for(&mut self, id: &SaveIdentity) -> IdempotencyKey {
        match &self.key {
            Some((d, k)) if d == id => k.clone(),
            _ => {
                let k = IdempotencyKey::new();
                self.key = Some((id.clone(), k.clone()));
                k
            }
        }
    }

    fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        let closing = self.books.closing;
        let verb = pending.verb();
        let took = self.closing_prompt.failed(e, pending, supported, closing);
        if took {
            self.closing_prompt.set_verb(verb);
        }
        took
    }

    /// The engine took the save (Q-262: every window showing these
    /// accounts reads them again).
    /// The recheck or the owner answered an unknown save (B2). Found: the
    /// entry is in the books, so the form shows it.
    pub(crate) fn lost_event(&mut self, ev: crate::forms::lost_save::LostEvent) {
        use crate::forms::lost_save::LostEvent;
        if let LostEvent::Saved(found) = ev {
            crate::forms::publish_saved(&self.link, &self.bus, &self.published);
            self.key = None;
            self.saved_count += 1;
            self.last_action = Some("save");
            let lists = Arc::clone(&self.books.lists);
            let header = self.header.current().clone();
            self.header.load(header);
            let lines = self.lines.value().clone();
            self.lines.load(lines, &*lists);
            if let Some(id) = found {
                self.template
                    .form_saved(&self.link, self.group.as_ref(), id);
                self.last_saved = Some(id);
                self.nav_at = None;
                self.listed = false;
                self.load(id);
            }
        }
    }

    pub(crate) fn saved(&mut self, id: TransactionId, version: RecordVersion) {
        self.lost.saved();
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        // An entry opened from a Templates Due date marks it entered.
        self.template
            .form_saved(&self.link, self.group.as_ref(), id);
        self.closing_prompt.saved();
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        self.saved_count += 1;
        self.last_action = Some("save");
        self.last_saved = Some(id);
        self.key = None;
        self.problems.clear();
        let lists = Arc::clone(&self.books.lists);
        let header = self.header.current().clone();
        if self.header.is_saving() {
            self.header.save_ok(header);
        } else {
            self.header.load(header);
        }
        let lines = self.lines.value().clone();
        self.lines.form_mut().save_ok(lines.clone(), &*lists);
        self.lines.load(lines, &*lists);
        self.seen_lines = self.lines.value().clone();
        let accounts = self.published.clone();
        match self.editing.as_mut() {
            Some(e) => {
                e.version = version;
                e.accounts = accounts;
                e.old_date = self.header.current().date.unwrap_or(e.old_date);
            }
            None if intent == SaveIntent::Save => {
                let date = self.header.current().date.unwrap_or(self.today);
                self.editing = Some(Editing {
                    id,
                    version,
                    old_date: date,
                    basis: self.new_basis,
                    read_only: None,
                    void: false,
                    accounts,
                    reversal: None,
                    reverses: None,
                });
            }
            None => {}
        }
        self.listed = false;
        self.follow_up(intent);
        if intent == SaveIntent::Save {
            // Read it back: the reversal link and the engine's version.
            self.reading_back = true;
            self.load(id);
            if self.load_task.is_none() {
                self.reading_back = false;
            }
        }
        if let Some(older) = self.after_save_nav.take() {
            self.navigate(older);
        }
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
        let intent = self.intent.take().unwrap_or(SaveIntent::Save);
        self.after_save_nav = None;
        if !crate::screens::register::keeps_key(&e) {
            self.key = None;
        }
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        if self.lost.lost(&e) {
            self.header.save_unknown(failure.clone());
            self.lines.form_mut().save_failed(failure);
            return;
        }
        self.header.save_failed(failure.clone());
        self.lines.form_mut().save_failed(failure);
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        self.engine_refusal(e);
    }

    /// An engine refusal in plain words: stale → Reload; the A/R-A/P
    /// codes and the reversal rule as notes; anything else as an error.
    fn engine_refusal(&mut self, e: ApiError) {
        match e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.stale = true,
            ApiError::Local(v) => self.local_refusal(v),
            e if has_code(&e, JE_AR_AP_NAME_REQUIRED) => {
                self.notice = Some("journal.problem.ar_ap_name_missing");
            }
            e if has_code(&e, JE_AR_AP_ONE_PER_ENTRY) => {
                self.refuse(vec![JournalProblem::ArApTwoLines]);
            }
            e if has_code(&e, REVERSAL_EDITED) => self.notice = Some("journal.reversal_edited"),
            e => self.error = Some(e),
        }
    }

    pub(crate) fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_form(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
            }
            SaveIntent::Save => {}
        }
    }

    pub(crate) fn submit_closing(&mut self) {
        let Some((pending, secret)) = self.closing_prompt.submit() else {
            return;
        };
        match pending {
            Pending::Save(intent) => {
                self.start_save(intent, Some(secret));
                if self.save_task.is_none() && self.closing_prompt.dialog_open() {
                    self.closing_prompt.nothing_sent();
                }
            }
            Pending::Void(reason) => {
                self.void_reason = reason;
                self.send_void(Some(secret));
            }
            Pending::Delete => self.send_delete(Some(secret)),
        }
    }

    // ------------------------------------------------------- void, delete

    /// The dates a void or delete of the open entry touches.
    fn action_dates(e: &Editing) -> Vec<CalendarDate> {
        let mut dates = vec![e.old_date];
        dates.extend(e.reversal.map(|(_, d)| d));
        dates
    }

    /// Edit > Void Transaction.
    pub(crate) fn start_void(&mut self) -> bool {
        match &self.editing {
            Some(e) if e.void => self.notice = Some("journal.void.already"),
            Some(_) => {
                self.void_open = true;
                self.delete_open = false;
            }
            None => self.notice = Some("journal.void.saved_only"),
        }
        true
    }

    /// Delete Transaction (Ctrl+D).
    pub(crate) fn start_delete(&mut self) -> bool {
        if self.editing.is_none() {
            self.notice = Some("journal.delete.saved_only");
        } else if !self.has(cl_api::Capabilities::TXN_DELETE) {
            self.notice = Some("journal.delete.unavailable");
        } else {
            self.delete_open = true;
            self.void_open = false;
        }
        true
    }

    /// Asks for the password when the entry (or its reversal) is in the
    /// closed period; false when it asked (nothing sent).
    fn action_allowed(&mut self, e: &Editing, pending: &Pending, password: bool) -> bool {
        let closing = self.books.closing;
        if password || !Self::action_dates(e).iter().any(|d| is_locked(*d, closing)) {
            return true;
        }
        let supported = self.closing_supported();
        if supported {
            self.closing_prompt.ask(pending.clone(), supported, closing);
            self.closing_prompt.set_verb(pending.verb());
        } else {
            self.closing_prompt.unavailable(supported, closing);
        }
        false
    }

    pub(crate) fn send_void(&mut self, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            self.notice = Some("journal.void.saved_only");
            return;
        };
        let reason = self.void_reason.trim().to_string();
        if reason.is_empty() {
            self.notice = Some("journal.void.reason_needed");
            return;
        }
        let pending = Pending::Void(reason.clone());
        if !self.action_allowed(&e, &pending, password.is_some()) {
            return;
        }
        self.published.clone_from(&e.accounts);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.void(&link, e.id, password.as_ref()) {
                Ok(()) => self.action_done(&pending),
                Err(err) => self.action_failed(&err, pending),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        // A fresh key per try is safe: void is idempotent at the engine.
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.journal_void",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.transactions()
                .void(e.id, &reason, &options(password), key)
                .await?;
            Ok(())
        }) {
            self.action_task = Some((pending, Task::Pending(p)));
        }
    }

    pub(crate) fn send_delete(&mut self, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            self.notice = Some("journal.delete.saved_only");
            return;
        };
        if !self.action_allowed(&e, &Pending::Delete, password.is_some()) {
            return;
        }
        self.published.clone_from(&e.accounts);
        let link = Arc::clone(&self.link);
        let retry = self.delete_unknown == Some(e.id);
        if let Some(local) = self.local.as_mut() {
            match delete_answer(local.delete(&link, e.id, password.as_ref()), retry) {
                Ok(()) => self.action_done(&Pending::Delete),
                Err(err) => self.action_failed(&err, Pending::Delete),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.journal_delete",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            let opts = DeleteOptions {
                reason: None,
                closing_password: password,
            };
            let r = s.transactions().delete(e.id, &opts, key).await.map(|_| ());
            delete_answer(r, retry)
        }) {
            self.action_task = Some((Pending::Delete, Task::Pending(p)));
        }
    }

    /// The void or delete took: the entry leaves the form.
    pub(crate) fn action_done(&mut self, pending: &Pending) {
        self.delete_unknown = None;
        crate::forms::publish_saved(&self.link, &self.bus, &self.published);
        self.closing_prompt.saved();
        self.void_open = false;
        self.delete_open = false;
        self.void_reason.clear();
        self.last_action = Some(match pending {
            Pending::Delete => "delete",
            Pending::Void(_) | Pending::Save(_) => "void",
        });
        self.listed = false;
        self.new_form(true);
    }

    pub(crate) fn action_failed(&mut self, e: &ApiError, pending: Pending) {
        // A delete with an unknown outcome may have taken: remember it, so
        // the retry's "not found" counts as done.
        self.delete_unknown = match (&pending, &self.editing) {
            (Pending::Delete, Some(ed)) if crate::screens::register::keeps_key(e) => Some(ed.id),
            _ => None,
        };
        if self.closing_failed(e, pending) {
            return;
        }
        self.engine_refusal(e.clone());
    }

    // ------------------------------------------------------------ records

    /// Prev (`older`) or Next through the saved entries, newest first.
    pub(crate) fn navigate(&mut self, older: bool) {
        if self.busy() || self.load_task.is_some() || self.nav_task.is_some() {
            return;
        }
        if self.is_dirty() {
            // The standard question: Save / Discard changes / Keep editing.
            self.nav_prompt = Some(older);
            return;
        }
        if !self.listed {
            self.want_older = Some(older);
            self.fetch_list();
            return;
        }
        self.step(older);
    }

    /// Prev/Next on unsaved changes: the answer to Save / Discard changes /
    /// Keep editing.
    pub(crate) fn answer_nav(&mut self, answer: PromptAnswer) {
        let Some(older) = self.nav_prompt.take() else {
            return;
        };
        match answer {
            PromptAnswer::Keep => {}
            PromptAnswer::Discard => {
                let lists = Arc::clone(&self.books.lists);
                self.header.revert_form();
                self.lines.revert_form(&*lists);
                self.seen_lines = self.lines.value().clone();
                self.problems.clear();
                self.sync_inputs();
                self.navigate(older);
            }
            PromptAnswer::Save => {
                self.after_save_nav = Some(older);
                let done = self.start_save(SaveIntent::Save, None);
                if !done && self.save_task.is_none() {
                    self.after_save_nav = None;
                }
            }
        }
    }

    pub(crate) fn step(&mut self, older: bool) {
        // Where the open entry is, when it came from elsewhere.
        if self.nav_at.is_none()
            && let Some(e) = &self.editing
        {
            self.nav_at = self.nav.iter().position(|x| *x == e.id);
        }
        let target = match (self.nav_at, older) {
            (None, true) => Some(0),
            (None, false) => return,
            (Some(i), true) => Some(i + 1),
            (Some(0), false) => None,
            (Some(i), false) => Some(i - 1),
        };
        match target {
            None => self.new_form(true),
            Some(i) if i < self.nav.len() => {
                self.nav_at = Some(i);
                let id = self.nav[i];
                self.load(id);
            }
            Some(_) => self.notice = Some("journal.nav.oldest"),
        }
    }

    fn fetch_list(&mut self) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            self.nav_task = Some(Task::ready(local.list(&link)));
            self.poll();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let filter = TransactionFilter {
                txn_type: Some(TxnType::JournalEntry),
                sort: Some((TxnSort::Date, SortDir::Desc)),
                ..TransactionFilter::default()
            };
            let page = s
                .transactions()
                .list(&filter, PageRequest::new(LIST_PAGE, 0)?)
                .await?;
            Ok(page.data.into_iter().map(|r| r.id).collect())
        }) {
            self.nav_task = Some(Task::Pending(p));
        }
    }

    pub(crate) fn load(&mut self, id: TransactionId) {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.get(&link, id) {
                Ok(v) => self.show(&v),
                Err(e) => self.error = Some(e),
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(
            &session,
            move |s| async move { s.transactions().get(id).await },
        ) {
            self.load_task = Some(Task::Pending(p));
        }
    }

    /// Reads the open entry again (it changed elsewhere).
    pub(crate) fn reload(&mut self) {
        if let Some(id) = self.editing.as_ref().map(|e| e.id) {
            self.load(id);
        }
    }

    /// Opens the reversal (or the entry it reverses) when the form is clean.
    pub(crate) fn open_linked(&mut self, id: TransactionId) {
        if self.is_dirty() {
            self.notice = Some("helper.unsaved");
            return;
        }
        self.nav_at = None;
        self.load(id);
    }

    // ------------------------------------------------------------ commands

    /// Esc: an open question or card closes, then the focused field goes
    /// back to its saved value; with nothing to revert the shell closes
    /// the window (asking first when the form is dirty).
    pub(crate) fn revert_field(&mut self) -> bool {
        if self.nav_prompt.is_some() {
            self.answer_nav(PromptAnswer::Keep);
            return true;
        }
        if self.void_open || self.delete_open {
            self.void_open = false;
            self.delete_open = false;
            return true;
        }
        let Some(f) = self.focused else { return false };
        let saved = self.header.original().clone();
        let now = self.header.current().clone();
        let differs = match f {
            field::DATE => now.date != saved.date,
            field::NUMBER => now.number != saved.number,
            field::MEMO => now.memo != saved.memo,
            field::ADJUSTING => now.adjusting != saved.adjusting,
            field::REVERSE => now.reverse_on != saved.reverse_on,
            _ => false,
        };
        if differs {
            self.header.edit_step(|h| match f {
                field::DATE => h.date = saved.date,
                field::NUMBER => h.number.clone_from(&saved.number),
                field::MEMO => h.memo.clone_from(&saved.memo),
                field::ADJUSTING => h.adjusting = saved.adjusting,
                _ => h.reverse_on = saved.reverse_on,
            });
            self.sync_inputs();
            return true;
        }
        false
    }

    /// Find (Ctrl+F), as in Write Check: the register of the entry's first
    /// account, where Go To finds an entry; with no account yet, why not.
    pub(crate) fn find(&mut self) {
        match self.lines.value().used().find_map(|l| l.account) {
            Some(a) => self.link.push(CompanyEvent::OpenRegister(a)),
            None => self.notice = Some("journal.find.no_account"),
        }
    }

    /// Keymap commands the grid and the closing dialog did not take.
    pub(crate) fn form_command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            KIND => true,
            "form.save_new" => {
                self.start_save(SaveIntent::SaveAndNew, None);
                true
            }
            "form.save_close" => {
                self.start_save(SaveIntent::SaveAndClose, None);
                true
            }
            "form.clear" => {
                self.clear();
                true
            }
            "form.prev_record" => {
                self.navigate(true);
                true
            }
            "form.next_record" => {
                self.navigate(false);
                true
            }
            "form.save_as_template" => self.save_as_template(),
            "edit.find" => {
                self.find();
                true
            }
            "edit.void_transaction" => self.start_void(),
            "form.delete_item" => self.start_delete(),
            "edit.undo" => {
                let done = self.header.undo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "edit.redo" => {
                let done = self.header.redo();
                if done {
                    self.sync_inputs();
                }
                done
            }
            "form.revert" => self.revert_field(),
            _ => false,
        }
    }
}
