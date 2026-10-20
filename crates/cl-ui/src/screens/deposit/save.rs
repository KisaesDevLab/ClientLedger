//! Saving a deposit (create or edit), the closing-date password, a payment
//! already deposited elsewhere (409), Void and Prev/Next.
//!
//! Money (plan §4): `check()` is the exact gate; [`to_draft`] copies the
//! checked values onto the engine draft one field at a time, with no
//! arithmetic; `BankPosting::deposit_with` refuses once more before
//! anything leaves.

use std::sync::Arc;

use cl_api::{
    ApiError, BankPosting, CashBack, ClosedPeriodAuthorization, ConflictReason, DepositDraft,
    DepositRecord, DepositSource, GroupedPayment, IdempotencyKey, LocalValidation, OrphanLabel,
    PageRequest, PostingOptions, RecordVersion, SplitLine, Transaction, TransactionFilter,
    TxnStatus, TxnType, Versioned, is_already_deposited,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Secret};
use cl_model::closing::{is_locked, needs_password};
use cl_model::forms::deposit::{
    CashBackForm, CheckedDeposit, DepositField, DepositForm, DepositProblem, PickedPayment, check,
};
use cl_model::forms::lines::{FormLine, FormLines, PostingLine};
use cl_model::forms::{SaveFailure, SaveIntent, SaveStart};

use super::{DepositRow, DepositScreen, Editing, Field, Nav, Pending, Saved, VoidPrompt, blank};
use crate::company::{CompanyEvent, Task};
use crate::forms::lines_grid::{L_ACCOUNT, L_AMOUNT};

/// Deposits are listed 50 at a time for Prev/Next.
const NAV_PAGE: u16 = 50;

/// What a save sends, for its idempotency key (never the password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub posting: BankPosting,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

fn split(l: &PostingLine) -> SplitLine {
    SplitLine {
        account: l.account,
        amount: l.amount,
        memo: l.memo.clone(),
        contact: l.contact,
        billable: false,
        tag: l.tag,
    }
}

/// The checked deposit as the engine's draft, field by field (no
/// arithmetic): each picked payment is a whole source line on Payments
/// Clearing at the engine's amount, the lines and fees keep the owner's
/// typed amounts (fees positive; cl-api sends them negative).
///
/// # Errors
/// `Local(InvalidValue)` when payments are picked and the tenant has no
/// Payments Clearing account.
pub fn to_draft(c: &CheckedDeposit, clearing: Option<AccountId>) -> Result<DepositDraft, ApiError> {
    let payments = if c.payments.is_empty() {
        Vec::new()
    } else {
        let clearing = clearing.ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
        c.payments
            .iter()
            .map(|p| GroupedPayment {
                source: p.source,
                kind: DepositSource::Payment,
                account: clearing,
                amount: p.amount,
                contact: p.contact,
                memo: None,
            })
            .collect()
    };
    Ok(DepositDraft {
        account: c.account,
        date: c.date,
        memo: c.memo.clone(),
        payments,
        lines: c.lines.iter().map(split).collect(),
        deductions: c.deductions.iter().map(split).collect(),
        cash_back: c.cash_back.as_ref().map(|b| CashBack {
            account: b.account,
            amount: b.amount,
            memo: b.memo.clone(),
        }),
    })
}

/// The accounts a deposit touches (Q-262): the bank, Payments Clearing
/// when payments were taken, every line, fee and cash-back account.
pub(crate) fn touched(d: &DepositDraft) -> Vec<AccountId> {
    let mut v = vec![d.account];
    v.extend(d.payments.iter().map(|p| p.account));
    v.extend(d.lines.iter().chain(&d.deductions).map(|l| l.account));
    v.extend(d.cash_back.as_ref().map(|c| c.account));
    v.sort();
    v.dedup();
    v
}

/// The accounts a saved deposit touches (for a void).
fn touched_by(r: &DepositRecord) -> Vec<AccountId> {
    let mut v: Vec<AccountId> = r.txn.lines.iter().map(|l| l.account_id).collect();
    v.push(r.account);
    v.sort();
    v.dedup();
    v
}

/// The form for a saved deposit: the header (payments as picked, cash
/// back) and the other lines (fees negative), all the engine's amounts.
pub(crate) fn form_of(
    r: &DepositRecord,
    keys: &mut cl_model::rowkey::RowKeyGen,
) -> (DepositForm, FormLines) {
    let mut f = blank(Some(r.account), r.txn.txn_date);
    f.memo = r.txn.memo.clone().unwrap_or_default();
    f.picked = r
        .payments
        .iter()
        .map(|p| PickedPayment {
            source: p.source,
            amount: p.amount,
            contact: p.contact,
        })
        .collect();
    if let Some(c) = &r.cash_back {
        f.cash_back = CashBackForm {
            account: Some(c.account),
            amount: Some(c.amount),
            memo: c.memo.clone().unwrap_or_default(),
        };
    }
    let seeds = r
        .lines
        .iter()
        .map(|l| FormLine {
            account: Some(l.account),
            amount: Some(l.amount),
            memo: l.memo.clone().unwrap_or_default(),
            customer: l.contact,
            tag: l.tag,
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

impl DepositScreen {
    fn focus_problem(&mut self, p: DepositProblem) {
        let lists = Arc::clone(&self.lists);
        let cell = |row, col| cl_grid::CellPos { row, col };
        match p.field() {
            DepositField::Account => self.focus = Some(Field::Account),
            DepositField::Date => self.focus = Some(Field::Date),
            DepositField::Payments => self.focus = Some(Field::Payments),
            DepositField::Lines => self.lines.focus_first(),
            DepositField::LineAccount(k) => {
                self.lines.form_mut().focus(cell(k, L_ACCOUNT), &*lists);
            }
            DepositField::LineAmount(k) => {
                self.lines.form_mut().focus(cell(k, L_AMOUNT), &*lists);
            }
            DepositField::CashBackAccount => self.focus = Some(Field::CashAccount),
            DepositField::CashBackAmount => self.focus = Some(Field::CashAmount),
        }
    }

    fn refuse(&mut self, problems: Vec<DepositProblem>) {
        if let Some(first) = problems.first().copied() {
            self.focus_problem(first);
        }
        self.save.problems = problems;
    }

    /// A refused save's problems, checked again against the form as it is
    /// now: the ones the owner fixed go away.
    pub(crate) fn recheck_problems(&mut self) {
        if self.save.problems.is_empty() {
            return;
        }
        let control = self.control_accounts();
        let now = check(&self.compose(), &self.context(&control))
            .err()
            .unwrap_or_default();
        let shown: Vec<_> = self
            .save
            .problems
            .iter()
            .map(std::mem::discriminant)
            .collect();
        self.save.problems = now
            .into_iter()
            .filter(|p| shown.contains(&std::mem::discriminant(p)))
            .collect();
    }

    /// Commits the cash-back amount still being typed. False (with the
    /// keyboard put there) when it is not an amount.
    pub(crate) fn commit_header(&mut self) -> bool {
        if self.inputs.rev == self.header.revision() {
            self.cash_amount_left();
        }
        if self.inputs.cash_problem.is_some() {
            self.focus = Some(Field::CashAmount);
            return false;
        }
        true
    }

    /// Checks the form and starts the save. Nothing is sent when the check
    /// doesn't pass. True when there was nothing to save or it saved at
    /// once.
    pub(crate) fn start_save(&mut self, intent: SaveIntent, password: Option<Secret>) -> bool {
        if self.save.task.is_some() {
            return false;
        }
        if let Some(e) = self.editing.as_ref().filter(|e| e.read_only) {
            self.save.notice = Some(e.note);
            return false;
        }
        if !self.commit_header() || !self.lines.commit() {
            return false;
        }
        self.save.problems.clear();
        self.save.error = None;
        self.save.stale = false;
        self.save.notice = None;
        self.save.clearing_warning = false;
        if !self.is_dirty() {
            self.follow_up(intent);
            return true;
        }
        let control = self.control_accounts();
        let checked = match check(&self.compose(), &self.context(&control)) {
            Ok(c) => c,
            Err(p) => {
                self.refuse(p);
                return false;
            }
        };
        let draft = match to_draft(&checked, self.clearing()) {
            Ok(d) => d,
            Err(e) => {
                self.save.error = Some(e);
                return false;
            }
        };
        let posting = match BankPosting::deposit_with(draft.clone(), self.caps()) {
            Ok(p) => p,
            Err(LocalValidation::NonPositiveAmount) => {
                // check() refuses this first; kept as the second gate.
                self.refuse(vec![DepositProblem::TotalNotPositive]);
                return false;
            }
            Err(v) => {
                self.save.error = Some(ApiError::Local(v));
                return false;
            }
        };
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
        let lists = Arc::clone(&self.lists);
        let _ = self.lines.form_mut().begin_save(intent, &*lists);
        if let SaveStart::Busy = self.header.begin_save(intent) {
            return false;
        }
        self.save.intent = Some(intent);
        self.save.published = touched(&draft);
        let edit = self.editing.as_ref().map(|e| (e.id, e.version));
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            let r = local.save(&link, &draft, edit, password.as_ref());
            return match r {
                Ok(s) => {
                    self.saved(&s);
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
            what: "orphan.deposit_save",
            key: Some(key.clone()),
        };
        let date = draft.date;
        let written = group.write(&session, label, move |s| async move {
            let opts = options(password);
            match edit {
                None => {
                    let d = s.payments().create_deposit(&posting, &opts, key).await?;
                    Ok(Saved {
                        id: d.saved.record.id,
                        version: d.saved.version,
                        date,
                        clearing_negative: d.clearing_would_go_negative,
                    })
                }
                Some((id, version)) => {
                    let v = s
                        .transactions()
                        .update_bank_posting(id, &version, &posting, &opts, key)
                        .await?;
                    Ok(Saved {
                        id,
                        version: v.version,
                        date,
                        clearing_negative: false,
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

    /// Polls the save, the record being read, the list, Void.
    pub(crate) fn poll_save(&mut self) {
        if let Some(r) = self.save.task.as_mut().and_then(Task::try_take) {
            self.save.task = None;
            match r {
                Ok(s) => self.saved(&s),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            match r {
                Ok(v) => self.show_txn(&v),
                Err(e) => self.save.error = Some(e),
            }
        }
        if let Some(r) = self.nav.task.as_mut().and_then(Task::try_take) {
            self.nav.task = None;
            match r {
                Ok(rows) => {
                    self.nav.loaded = true;
                    self.nav.list = rows;
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
    }

    pub(crate) fn closing_failed(&mut self, e: &ApiError, pending: Pending) -> bool {
        let supported = self.closing_supported();
        self.closing_prompt
            .failed(e, pending, supported, self.closing)
    }

    /// The engine took the save.
    fn saved(&mut self, s: &Saved) {
        crate::forms::publish_saved(&self.link, &self.bus, &self.save.published);
        self.closing_prompt.saved();
        let intent = self.save.intent.take().unwrap_or(SaveIntent::Save);
        self.save.count += 1;
        self.save.last_action = Some("save");
        self.save.key = None;
        self.save.problems.clear();
        self.save.error = None;
        self.save.clearing_warning = s.clearing_negative;
        let lists = Arc::clone(&self.lists);
        let header = self.header.current().clone();
        self.header.save_ok(header);
        let lines = self.lines.value().clone();
        self.lines.form_mut().save_ok(lines, &*lists);
        self.nav = Nav::default();
        if let Some(e) = self.editing.as_mut().filter(|e| e.id == s.id) {
            e.version = s.version;
            e.old_date = s.date;
        }
        // A warning keeps the deposit in front so it can be changed or
        // voided (PR #159).
        let intent = if s.clearing_negative {
            SaveIntent::Save
        } else {
            intent
        };
        self.follow_up(intent);
        self.refresh_pending();
        self.refresh_facts();
    }

    /// After a save: a new deposit, close, or stay.
    fn follow_up(&mut self, intent: SaveIntent) {
        match intent {
            SaveIntent::SaveAndNew => self.new_deposit(true),
            SaveIntent::SaveAndClose => {
                self.link
                    .push(CompanyEvent::CloseWindow(cl_model::CommandId::new(
                        super::KIND,
                    )));
            }
            SaveIntent::Save => {}
        }
    }

    /// A blank deposit. Save & New keeps the account and the date; Clear
    /// starts from today.
    pub(crate) fn new_deposit(&mut self, keep_date: bool) {
        let current = self.header.current();
        let account = current.account;
        let date = if keep_date {
            current.date.unwrap_or(self.today)
        } else {
            self.today
        };
        let lists = Arc::clone(&self.lists);
        self.header.load(blank(account, date));
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().load(fresh, &*lists);
        self.editing = None;
        self.record = None;
        self.show_summary = false;
        self.nav.at = None;
        self.void = VoidPrompt::default();
        self.save.problems.clear();
        self.save.stale = false;
        self.panel.chosen = None;
        self.panel.no_grouping = false;
        self.focus = Some(Field::Account);
        self.sync_inputs();
    }

    /// Clear: throw the unsaved edits away and start a new deposit.
    pub(crate) fn clear(&mut self) {
        if self.save.task.is_some() {
            return;
        }
        self.save.error = None;
        self.save.notice = None;
        self.save.clearing_warning = false;
        self.save.last_action = Some("clear");
        self.new_deposit(false);
    }

    pub(crate) fn save_failed(&mut self, e: ApiError) {
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
        if self.closing_failed(&e, Pending::Save(intent)) {
            return;
        }
        if is_already_deposited(&e) {
            // Nothing posted: the list is read again and the stale picks
            // are dropped when it arrives.
            self.save.notice = Some("deposit.already_deposited");
            self.save.key = None;
            self.refresh_pending();
            return;
        }
        match &e {
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            } => self.save.stale = true,
            _ => self.save.error = Some(e),
        }
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
            Pending::Refund => {
                self.send_refund(Some(secret));
                true
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
    // Saved deposits: Prev/Next, loading, Void.

    /// Prev (older) or Next (newer) over the account's deposits.
    pub(crate) fn navigate(&mut self, older: bool) {
        if self.save.task.is_some() || self.load_task.is_some() {
            return;
        }
        if self.is_dirty() {
            self.save.notice = Some("deposit.nav.unsaved");
            return;
        }
        if self.nav.account != self.account() {
            self.nav = Nav {
                account: self.account(),
                ..Nav::default()
            };
        }
        if !self.nav.loaded {
            self.nav.want_older = Some(older);
            self.fetch_list();
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
            None => self.new_deposit(true),
            Some(i) if i < self.nav.list.len() => {
                let Some(account) = self.nav.account else {
                    return;
                };
                self.nav.at = Some(i);
                let id = self.nav.list[i].id;
                self.load_deposit(id, account);
            }
            Some(_) => self.save.notice = Some("deposit.nav.oldest"),
        }
    }

    /// The account's deposits, newest first (the first 50 pages' worth for
    /// Prev/Next and the recent list).
    pub(crate) fn list_task(&mut self, account: AccountId) -> Option<Task<Vec<DepositRow>>> {
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            return Some(Task::ready(local.list(&link, account)));
        }
        let (session, group) = (self.link.session()?, self.group.as_ref()?);
        group
            .read(&session, move |s| async move {
                let filter = TransactionFilter {
                    txn_type: Some(TxnType::Deposit),
                    account_id: Some(account),
                    sort: Some((cl_api::TxnSort::Date, cl_api::SortDir::Desc)),
                    ..TransactionFilter::default()
                };
                let page = s
                    .transactions()
                    .list(&filter, PageRequest::new(NAV_PAGE, 0)?)
                    .await?;
                Ok(page
                    .data
                    .into_iter()
                    .map(|r| DepositRow {
                        id: r.id,
                        date: r.txn_date,
                        total: r.total,
                        void: r.status == TxnStatus::Void,
                    })
                    .collect())
            })
            .ok()
            .map(Task::Pending)
    }

    fn fetch_list(&mut self) {
        let Some(account) = self.account() else {
            return;
        };
        self.nav.task = self.list_task(account);
        self.poll_save();
    }

    /// Reads a saved deposit into the form (`account`: the one it is
    /// expected in; any bank account of the record otherwise).
    pub(crate) fn load_deposit(&mut self, id: TransactionId, account: AccountId) {
        self.nav.account.get_or_insert(account);
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.get(&link, id) {
                Ok(v) => self.show_txn(&v),
                Err(e) => self.save.error = Some(e),
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

    /// The bank account a deposit went into: the form's own account if the
    /// record has its bank debit there, else the first deposit account
    /// with one.
    fn record_of(&self, txn: &Transaction) -> Result<DepositRecord, ApiError> {
        let mut candidates: Vec<AccountId> = self.account().into_iter().collect();
        candidates.extend(self.deposit_accounts().iter().map(|a| a.id));
        for a in candidates {
            if let Ok(r) = DepositRecord::from_transaction(txn.clone(), a) {
                return Ok(r);
            }
        }
        Err(ApiError::Decode { what: "deposit" })
    }

    pub(crate) fn show_txn(&mut self, v: &Versioned<Transaction>) {
        let r = match self.record_of(&v.record) {
            Ok(r) => r,
            Err(e) => {
                self.save.error = Some(e);
                return;
            }
        };
        let lists = Arc::clone(&self.lists);
        let (form, lines) = form_of(&r, &mut self.lines.form_mut().adapter_mut().keys);
        let note = if r.txn.status == TxnStatus::Void {
            Some("deposit.read_only.void")
        } else if r.is_grouped() {
            Some("deposit.read_only.grouped")
        } else if !r.exact {
            Some("deposit.read_only.shape")
        } else {
            None
        };
        if form.account != self.account() {
            self.panel.balance = None;
        }
        self.header.load(form);
        self.lines.form_mut().load(lines, &*lists);
        self.editing = Some(Editing {
            id: r.txn.id,
            version: v.version,
            account: r.account,
            old_date: r.txn.txn_date,
            grouped: r.is_grouped(),
            read_only: note.is_some(),
            note: note.unwrap_or("deposit.read_only.void"),
        });
        self.record = Some(r);
        self.save.problems.clear();
        self.save.error = None;
        self.save.notice = None;
        self.save.stale = false;
        self.save.last_action = Some("load");
        self.void = VoidPrompt::default();
        self.panel.chosen = None;
        self.sync_inputs();
        self.refresh_facts();
    }

    /// The edited deposit was changed elsewhere: read it again.
    pub(crate) fn reload(&mut self) {
        if let Some((id, account)) = self.editing.as_ref().map(|e| (e.id, e.account)) {
            self.save.stale = false;
            self.load_deposit(id, account);
        }
    }

    /// The saved deposit is dated on or before the closing date.
    pub(crate) fn editing_locked(&self) -> bool {
        self.editing
            .as_ref()
            .is_some_and(|e| is_locked(e.old_date, self.closing))
    }

    /// Edit > Void Transaction: only for a saved deposit that isn't void.
    pub(crate) fn start_void(&mut self) -> bool {
        let void = self
            .record
            .as_ref()
            .is_some_and(|r| r.txn.status == TxnStatus::Void);
        match &self.editing {
            Some(_) if !void => {
                self.void.open = true;
                self.void.reason.clear();
            }
            Some(_) => self.save.notice = Some("deposit.void.already"),
            None => self.save.notice = Some("deposit.void.saved_only"),
        }
        true
    }

    /// Void with the typed reason (and the closing-date password when the
    /// deposit is in the closed period). Voiding a grouped deposit returns
    /// its payments to the waiting list (E2).
    pub(crate) fn send_void(&mut self, reason: &str, password: Option<Secret>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let reason = reason.trim().to_string();
        if reason.is_empty() {
            self.save.notice = Some("deposit.void.reason_needed");
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
        self.save.published = self.record.as_ref().map(touched_by).unwrap_or_default();
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            match local.void(&link, e.id, password.as_ref()) {
                Ok(()) => self.void.task = Some(Task::ready(())),
                Err(err) => {
                    if !self.closing_failed(&err, Pending::Void(reason)) {
                        self.save.error = Some(err);
                    }
                    return;
                }
            }
            self.poll_save();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.deposit_void",
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
        crate::forms::publish_saved(&self.link, &self.bus, &self.save.published);
        self.closing_prompt.saved();
        self.void = VoidPrompt::default();
        self.save.last_action = Some("void");
        self.nav = Nav::default();
        self.new_deposit(true);
        self.refresh_pending();
        self.refresh_facts();
    }

    /// The date shown for a deposit row.
    pub(crate) fn row_date(d: CalendarDate) -> String {
        crate::shell::us_date(d)
    }
}
