//! Reconcile (P27, `banking.reconcile`; Q-60, Q-100, Q-P27a–n).
//!
//! - **Begin** ([`begin`]): the account, statement date, ending balance,
//!   service charge and interest (a card: one finance charge); an
//!   unfinished reconciliation offers Continue or Start over.
//! - **The window** ([`window`]): two panes, bank-file lines pre-checked
//!   by the engine, the engine's totals and difference, Reconcile now only
//!   at exactly 0.00 with every tick answered (Q-P27a, Q-P27f).
//! - **Finish** ([`finish`]): the closing password when a pending charge is
//!   locked, the done dialog, and for a card the pay step (Q-60, Q-P27g).
//! - **Reports and Undo** ([`reports`], [`undo`]).
//!
//! The engine owns every balance: ticks go in batches (`view=summary`) and
//! each answer replaces the totals. Nothing here adds amounts.

mod begin;
mod finish;
mod reports;
#[cfg(test)]
mod tests;
mod undo;
mod window;

use std::sync::Arc;

use cl_api::{
    Account, ApiError, DiscrepancyReport, ReconHistoryRow, ReconLine, ReconStart, ReconStatus,
    ReconcileRefusal, Reconciliation, ReconciliationId, RequestGroup, UndoMode, reconcile_refusal,
};
use cl_core::ids::{AccountId, JournalLineId, TenantId};
use cl_core::{CalendarDate, Money, Secret};
use cl_model::reconcile::{BeginForm, BeginProblem, Pane, ReconKind, TickQueue, can_finish};
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::t;
use crate::screens::company::form::DateInput;
use crate::screens::invoice::engine::{self, Job};
use crate::screens::preferences::intake_folders::source as intake_source;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "banking.reconcile";

/// What the card pay step hands to Write Check, Enter Bill or Transfer
/// Funds (Q-60, Q-P27g): the printed statement balance and the card.
#[derive(Clone, PartialEq, Eq)]
pub struct CardPay {
    /// The card account (the expense line's account, or Transfer's To).
    pub card: AccountId,
    /// The printed statement balance (the owner's own figure).
    pub amount: Money,
    /// The form's date (the statement date).
    pub date: CalendarDate,
}

/// No amounts.
impl std::fmt::Debug for CardPay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardPay")
            .field("card", &self.card)
            .finish_non_exhaustive()
    }
}

/// A timestamp's day as `MM/DD/YYYY` (UTC).
pub(crate) fn day_text(ts: cl_core::Timestamp) -> String {
    let iso = ts.to_string();
    iso.get(..10)
        .and_then(|d| CalendarDate::parse_wire(d).ok())
        .map(crate::shell::us_date)
        .unwrap_or_default()
}

/// The Reconcile window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(ReconcileScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Which step shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// The Begin form (also "Change statement info").
    Begin,
    /// The two panes.
    Window,
}

/// A dialog over the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// Finish needs the closing-date password.
    ClosingPassword,
    /// Finished: Summary, Detail, Close (a card goes on to the pay step).
    Done,
    /// A card's pay step (Q-60).
    CardPay,
    /// Undo the last reconciliation.
    Undo,
    /// Previous reports.
    Previous,
    /// The discrepancy report.
    Discrepancy,
    /// Start over: discard the reconciliation in progress.
    StartOver,
    /// Begin: a service charge or interest like this one is already posted
    /// in the account (Q-P27u).
    ChargeExists,
}

/// The Reconcile window.
pub struct ReconcileScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) today: CalendarDate,
    pub(crate) step: Step,
    /// Bank and card accounts, banks first.
    pub(crate) accounts: Vec<Account>,
    /// Every active account (the charge accounts' choices).
    pub(crate) chart: Vec<Account>,
    accounts_job: Option<Job<Vec<Account>>>,
    pub(crate) accounts_loaded: bool,
    pub(crate) form: BeginForm,
    pub(crate) date_in: DateInput,
    pub(crate) svc_date: DateInput,
    pub(crate) int_date: DateInput,
    pub(crate) problems: Vec<BeginProblem>,
    /// The account's history (the unfinished one, the last finished one).
    pub(crate) history: Vec<ReconHistoryRow>,
    history_job: Option<Job<Vec<ReconHistoryRow>>>,
    history_for: Option<AccountId>,
    /// Begin reopened as "Change statement info".
    pub(crate) editing: bool,
    /// The worksheet as the engine last reported it.
    pub(crate) rec: Option<Reconciliation>,
    /// Its lines (a summary answer keeps them; only the ticks change).
    pub(crate) lines: Vec<ReconLine>,
    rec_job: Option<Job<Reconciliation>>,
    /// The epoch `rec_job` was asked in (review D4).
    rec_job_epoch: u64,
    /// Moves on each tick batch sent or answered and on leaving the
    /// worksheet: a worksheet read asked in an older epoch may predate
    /// ticks the engine has since taken, so its answer is read again.
    rec_epoch: u64,
    tick_job: Option<Job<Reconciliation>>,
    act_job: Option<(&'static str, Job<()>)>,
    pub(crate) queue: TickQueue,
    pub(crate) hide_later: bool,
    pub(crate) pane: Pane,
    pub(crate) cursor: Option<JournalLineId>,
    /// The keyboard moved the cursor: its row's box takes the keyboard
    /// (and the screen reader's) focus when next drawn (P27 QA-9). The
    /// pane and the cursor are the one truth; egui's focus follows them.
    pub(crate) focus_cursor: bool,
    /// The keyboard is on a control outside the panes (Hide later, the
    /// buttons): Tab, Space and the arrows are egui's there, not the
    /// panes' (P27 keyboard fix). Read from egui's focus each frame.
    pub(crate) off_panes: bool,
    /// Tab past the last pane (`Some(true)`) or Shift+Tab before the first
    /// (`Some(false)`): the next control takes the focus when drawn.
    pub(crate) leave_panes: Option<bool>,
    /// A row's box had the focus in the frame being drawn.
    pub(crate) row_focused: bool,
    pub(crate) dialog: Option<Dialog>,
    pub(crate) password: String,
    /// The engine refused the closing password Finish sent (review D1).
    pub(crate) password_rejected: bool,
    /// A card's pay step, from the worksheet as it finished (review D2).
    pub(crate) card_pay: Option<CardPay>,
    /// Leave for now (or the close prompt) waits for the ticks to reach
    /// the engine, then closes (review D5).
    pub(crate) leaving: bool,
    pub(crate) undo_mode: UndoMode,
    pub(crate) discrepancy: Option<DiscrepancyReport>,
    disc_job: Option<Job<DiscrepancyReport>>,
    /// Accepted bank lines whose entry was voided (Q-P27j; P26 counts).
    pub(crate) entry_voided: Option<u64>,
    voided_job: Option<Job<u64>>,
    /// The reconciliation the done dialog and the reports are about.
    pub(crate) finished: Option<ReconciliationId>,
    pub(crate) error: Option<ApiError>,
    /// A plain notice (i18n key).
    pub(crate) notice: Option<&'static str>,
    /// What the last action did (control channel).
    pub(crate) last: Option<&'static str>,
    /// The engine refused Undo for this user (not an Admin).
    pub(crate) not_admin: bool,
    /// The user's engine role in this company (`None` until read, or when
    /// it can't be): only an Admin (`owner`) sees Undo (Q-P27s).
    pub(crate) role: Option<String>,
    role_job: Option<Job<Option<String>>>,
    /// Begin's look in the register for the statement's charges (Q-P27u).
    dup_job: Option<Job<bool>>,
    /// The owner said "Add it again": the next Begin goes on (Q-P27u).
    pub(crate) dup_confirmed: bool,
    pub(crate) reports: reports::Reports,
    pub(crate) bus: crate::data_bus::Subscription,
}

impl std::fmt::Debug for ReconcileScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReconcileScreen")
            .field("step", &self.step)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl ReconcileScreen {
    /// Begin on today.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> ReconcileScreen {
        let group = link.group().and_then(Result::ok);
        let bus = link.bus().subscribe();
        let mut s = ReconcileScreen {
            group,
            tenant,
            today,
            step: Step::Begin,
            accounts: Vec::new(),
            chart: Vec::new(),
            accounts_job: None,
            accounts_loaded: false,
            form: BeginForm::new(today),
            date_in: DateInput::new(today),
            svc_date: DateInput {
                text: String::new(),
                value: None,
            },
            int_date: DateInput {
                text: String::new(),
                value: None,
            },
            problems: Vec::new(),
            history: Vec::new(),
            history_job: None,
            history_for: None,
            editing: false,
            rec: None,
            lines: Vec::new(),
            rec_job: None,
            rec_job_epoch: 0,
            rec_epoch: 0,
            tick_job: None,
            act_job: None,
            queue: TickQueue::default(),
            hide_later: true,
            pane: Pane::MoneyIn,
            cursor: None,
            focus_cursor: false,
            off_panes: false,
            leave_panes: None,
            row_focused: false,
            dialog: None,
            password: String::new(),
            password_rejected: false,
            card_pay: None,
            leaving: false,
            undo_mode: UndoMode::KeepChecks,
            discrepancy: None,
            disc_job: None,
            entry_voided: None,
            voided_job: None,
            finished: None,
            error: None,
            notice: None,
            last: None,
            not_admin: false,
            role: None,
            role_job: None,
            dup_job: None,
            dup_confirmed: false,
            reports: reports::Reports::new(&link),
            bus,
            link,
        };
        s.load_accounts();
        s.load_role();
        s.poll();
        s
    }

    /// Reads the user's role in the open company (the demo's own role).
    fn load_role(&mut self) {
        let tenant = self.tenant.as_ref().map(|(id, _)| *id);
        self.role_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(Some(d.intake.role())),
            move |s| async move {
                Ok(match tenant {
                    Some(t) => intake_source::engine_role(&s, t).await,
                    None => None,
                })
            },
        ));
    }

    /// The user is an Admin (engine role `owner`, Q-A01k): Undo is shown
    /// only then (Q-P27s; the engine still refuses anyone else).
    pub(crate) fn is_admin(&self) -> bool {
        self.role.as_deref() == Some(intake_source::ADMIN_ROLE)
    }

    /// The engine has what the window needs (the demo always does).
    pub(crate) fn supported(&self) -> bool {
        self.link.is_demo() || cl_api::reconcile_screen_supported(&self.link.capabilities())
    }

    /// The discrepancy report is there (`reconcile-discrepancy-v1`).
    pub(crate) fn discrepancy_supported(&self) -> bool {
        self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::Capabilities::RECONCILE_DISCREPANCY)
    }

    /// Bank or card.
    pub(crate) fn kind(&self) -> ReconKind {
        self.form.kind
    }

    pub(crate) fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    pub(crate) fn account_text(&self, id: AccountId) -> String {
        self.account(id)
            .map(crate::screens::register::account_text)
            .unwrap_or_default()
    }

    fn load_accounts(&mut self) {
        self.accounts_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::home::accounts(d)),
            |s| async move {
                s.accounts()
                    .list_all(&cl_api::AccountFilter::default())
                    .await
            },
        ));
    }

    /// The kind of `a`, when it can be reconciled.
    pub(crate) fn kind_of(a: &Account) -> Option<ReconKind> {
        use cl_model::register::AccountClass;
        match crate::screens::register::class_of(a)? {
            AccountClass::Bank => Some(ReconKind::Bank),
            AccountClass::CreditCard => Some(ReconKind::Card),
            _ => None,
        }
    }

    /// Bank fees and interest income, by name, until the owner picks.
    fn default_charge_accounts(&mut self) {
        let find = |words: &[&str]| {
            self.chart
                .iter()
                .find(|a| {
                    let n = a.fields.name.to_lowercase();
                    words.iter().all(|w| n.contains(w))
                })
                .map(|a| a.id)
        };
        let fees = find(&["bank", "fee"]).or_else(|| find(&["service charge"]));
        let interest = find(&["interest", "income"]);
        if self.form.service_charge.account.is_none() {
            self.form.service_charge.account = fees;
        }
        if self.form.interest.account.is_none() {
            self.form.interest.account = interest;
        }
    }

    /// A new statement: the ending balance, the charges and their dates
    /// cleared, the date back on today (QA-6, QA-7). A finished or undone
    /// statement's charge would otherwise be posted again next month, or
    /// on another account. The charge accounts go back to the defaults.
    pub(crate) fn reset_statement(&mut self) {
        let account = self.form.account;
        let kind = self.form.kind;
        self.form = BeginForm::new(self.today);
        self.form.account = account;
        self.form.kind = kind;
        self.date_in = DateInput::new(self.today);
        self.svc_date = DateInput {
            text: String::new(),
            value: None,
        };
        self.int_date = DateInput {
            text: String::new(),
            value: None,
        };
        self.problems.clear();
        self.default_charge_accounts();
    }

    /// Picks the account (Begin): another account starts from its own
    /// figures and notices (QA-7).
    pub(crate) fn pick_account(&mut self, id: AccountId) {
        if self.form.account != Some(id) {
            self.form.account = Some(id);
            self.reset_statement();
            self.notice = None;
            self.error = None;
            self.not_admin = false;
        }
        self.form.account = Some(id);
        self.form.kind = self
            .account(id)
            .and_then(Self::kind_of)
            .unwrap_or(ReconKind::Bank);
        self.pane = self.form.kind.left();
        self.history_for = None;
        self.problems.clear();
    }

    /// The in-progress reconciliation of the account picked, if any.
    pub(crate) fn unfinished(&self) -> Option<&ReconHistoryRow> {
        self.history
            .iter()
            .find(|r| r.status == ReconStatus::InProgress)
    }

    /// "Undo last reconciliation" is on: a finished one, an Admin, not in
    /// "Change statement info", and none started (the engine refuses Undo
    /// while one is, QA minor).
    pub(crate) fn undo_offered(&self) -> bool {
        self.last_finished().is_some()
            && self.is_admin()
            && !self.not_admin
            && !self.editing
            && self.unfinished().is_none()
    }

    /// The finished reconciliation with the latest statement date.
    pub(crate) fn last_finished(&self) -> Option<&ReconHistoryRow> {
        self.history
            .iter()
            .filter(|r| r.status == ReconStatus::Complete)
            .max_by_key(|r| r.statement_date)
    }

    // ------------------------------------------------------------ polling

    /// Takes every answer that arrived; reads what is due.
    pub(crate) fn poll(&mut self) {
        if !self.link.bus().take(&mut self.bus).is_empty() {
            self.history_for = None;
            if self.step == Step::Window
                && let Some(r) = &self.rec
            {
                let id = r.id;
                self.refresh_sheet(id);
            }
        }
        if let Some(r) = engine::poll(&mut self.accounts_job) {
            self.accounts_loaded = true;
            match r {
                Ok(all) => {
                    self.chart = all.iter().filter(|a| a.is_active).cloned().collect();
                    self.default_charge_accounts();
                    let mut v: Vec<Account> = all
                        .into_iter()
                        .filter(|a| a.is_active && Self::kind_of(a).is_some())
                        .collect();
                    v.sort_by_key(|a| {
                        (
                            Self::kind_of(a) != Some(ReconKind::Bank),
                            a.fields.account_number.clone().unwrap_or_default(),
                        )
                    });
                    if self.form.account.is_none()
                        && let Some(first) = v.first().map(|a| a.id)
                    {
                        self.accounts = v;
                        self.pick_account(first);
                    } else {
                        self.accounts = v;
                    }
                }
                Err(e) => self.error = Some(e),
            }
        }
        if let Some(r) = engine::poll(&mut self.history_job) {
            match r {
                Ok(h) => self.history = h,
                Err(e) => self.error = Some(e),
            }
        }
        if self.history_job.is_none()
            && self.form.account.is_some()
            && self.form.account != self.history_for
            && self.supported()
        {
            self.history_for = self.form.account;
            if let Some(a) = self.form.account {
                self.history = Vec::new();
                self.history_job = Some(engine::read(
                    &self.link,
                    self.group.as_ref(),
                    move |d| crate::company::demo::reconcile::history(d, a),
                    move |s| async move { s.reconciliations().history(a).await },
                ));
            }
        }
        // Before the worksheet's answer: going on starts it (the demo
        // answers at once).
        if let Some(r) = engine::poll(&mut self.dup_job) {
            if let Ok(true) = r {
                self.dialog = Some(Dialog::ChargeExists);
            } else {
                // None there, or the register couldn't be read: the
                // warning is a courtesy, Begin goes on.
                self.dup_confirmed = true;
                self.begin();
            }
        }
        if let Some(r) = engine::poll(&mut self.rec_job) {
            if self.rec_job_epoch == self.rec_epoch {
                match r {
                    Ok(rec) => self.took(rec),
                    Err(e) => self.refused(e),
                }
            } else if self.step == Step::Window
                && let Some(id) = self.rec.as_ref().map(|r| r.id)
            {
                // Asked before ticks the engine has since taken (or while
                // they were on their way): read the worksheet again.
                self.read_sheet(id);
            }
        }
        // Send the next batch first: the demo answers at once.
        if self.tick_job.is_none()
            && let Some(id) = self.rec.as_ref().map(|r| r.id)
            && let Some(batch) = self.queue.take_batch()
        {
            self.rec_epoch = self.rec_epoch.wrapping_add(1);
            self.tick_job = Some(engine::write(
                &self.link,
                self.group.as_ref(),
                cl_api::OrphanLabel {
                    what: "orphan.reconcile_ticks",
                    key: None,
                },
                {
                    let b = batch.clone();
                    move |d| crate::company::demo::reconcile::tick(d, id, &b)
                },
                move |s| async move { s.reconciliations().tick(id, &batch).await },
            ));
        }
        if let Some(r) = engine::poll(&mut self.tick_job) {
            self.rec_epoch = self.rec_epoch.wrapping_add(1);
            match r {
                Ok(rec) => {
                    self.queue.sent();
                    if self.rec.is_some() {
                        self.took(rec);
                    }
                }
                Err(e) => {
                    self.queue.failed();
                    // Stay so the owner sees what failed.
                    self.leaving = false;
                    self.refused(e);
                }
            }
        }
        if let Some((what, job)) = self.act_job.as_mut()
            && let Some(r) = job.take()
        {
            let what = *what;
            self.act_job = None;
            self.acted(what, r);
        }
        if let Some(r) = engine::poll(&mut self.role_job) {
            // A failed read leaves the role unknown: no Undo shown.
            self.role = r.ok().flatten();
        }
        if let Some(r) = engine::poll(&mut self.voided_job) {
            // Only a note: a failed read shows none.
            self.entry_voided = r.ok().filter(|n| *n > 0);
        }
        if let Some(r) = engine::poll(&mut self.disc_job) {
            match r {
                Ok(d) => self.discrepancy = Some(d),
                Err(e) => self.error = Some(e),
            }
        }
        self.reports.poll();
        if self.leaving && self.queue.idle() && self.tick_job.is_none() {
            self.leaving = false;
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
        }
    }

    /// The engine's answer: totals replace ours; lines when it sent them.
    fn took(&mut self, rec: Reconciliation) {
        if self.rec.as_ref().is_some_and(|r| r.id != rec.id) {
            // An answer about another worksheet than the one shown.
            return;
        }
        if let Some(lines) = &rec.lines {
            self.lines.clone_from(lines);
            self.queue
                .load(self.lines.iter().map(|l| (l.journal_line, l.cleared)));
        }
        if rec.status == ReconStatus::Complete && self.step == Step::Window {
            // Finished somewhere else: read it again from Begin.
            self.notice = Some("reconcile.notice.finished_elsewhere");
            self.back_to_begin();
            return;
        }
        if self.step != Step::Window {
            self.read_entry_voided();
        }
        self.step = Step::Window;
        self.editing = false;
        self.form.statement_date = Some(rec.statement_date);
        self.rec = Some(rec);
        self.error = None;
    }

    /// A refusal: a reconciliation finished or discarded elsewhere is a
    /// notice and a reload, not an error.
    fn refused(&mut self, e: ApiError) {
        match reconcile_refusal(&e) {
            Some(ReconcileRefusal::NotInProgress) => {
                self.notice = Some("reconcile.notice.finished_elsewhere");
                self.back_to_begin();
            }
            Some(ReconcileRefusal::InProgress { .. }) => {
                self.notice = Some("reconcile.notice.in_progress");
                self.history_for = None;
            }
            Some(ReconcileRefusal::AdminRequired) => {
                self.not_admin = true;
                self.notice = Some("reconcile.undo.admin_only");
            }
            Some(ReconcileRefusal::ChargePosted) => {
                // Stays on "Change statement info" (editing is kept until
                // the engine takes the patch), with Back to the
                // reconciliation (Q-P27o minor).
                self.notice = Some("reconcile.notice.charge_posted");
            }
            Some(ReconcileRefusal::ChargeAccountInvalid) => {
                self.notice = Some("reconcile.problem.charge_account_invalid");
            }
            Some(ReconcileRefusal::StatementDateOrder) => {
                // The same words as Begin's own check; the history is read
                // again so the date it names is the engine's.
                self.problems = vec![BeginProblem::DateNotAfterLast];
                self.history_for = None;
                if self.rec.is_some() && self.step == Step::Begin {
                    self.editing = true;
                }
            }
            _ => {
                if matches!(
                    e,
                    ApiError::ClosedPeriod(_)
                        | ApiError::ClosingPasswordRequired
                        | ApiError::ClosingPasswordRejected
                ) {
                    self.notice = Some("reconcile.closing.needed");
                }
                self.error = Some(e);
            }
        }
    }

    pub(crate) fn back_to_begin(&mut self) {
        // Reads still on their way are about the worksheet left behind.
        self.rec_epoch = self.rec_epoch.wrapping_add(1);
        self.leaving = false;
        self.step = Step::Begin;
        self.editing = false;
        self.rec = None;
        self.lines.clear();
        self.queue = TickQueue::default();
        self.history_for = None;
    }

    fn acted(&mut self, what: &'static str, r: Result<(), ApiError>) {
        match r {
            Ok(()) => {
                self.last = Some(what);
                self.error = None;
                match what {
                    "finished" => {
                        self.password.clear();
                        self.password_rejected = false;
                        self.card_pay = self.card_pay_due();
                        self.finished = self.rec.as_ref().map(|r| r.id);
                        self.dialog = Some(Dialog::Done);
                        if let Some(a) = self.form.account {
                            self.link
                                .bus()
                                .publish(crate::data_bus::Change::ledger([a]));
                        }
                        self.back_to_begin();
                        self.reset_statement();
                    }
                    "undone" | "canceled" => {
                        self.password.clear();
                        self.dialog = None;
                        self.history_for = None;
                        if what == "undone" {
                            // Undo keeps the reconciliation's figures in the
                            // engine; Begin starts a new statement (QA-6).
                            self.reset_statement();
                            self.notice = None;
                        }
                        if let Some(a) = self.form.account {
                            self.link
                                .bus()
                                .publish(crate::data_bus::Change::ledger([a]));
                        }
                    }
                    _ => {}
                }
            }
            Err(e) => {
                if what == "finished"
                    && matches!(
                        e,
                        ApiError::ClosedPeriod(_) | ApiError::ClosingPasswordRequired
                    )
                {
                    self.dialog = Some(Dialog::ClosingPassword);
                    return;
                }
                if what == "finished" && matches!(e, ApiError::ClosingPasswordRejected) {
                    // Ask again in the dialog, the wrong password gone.
                    self.password.clear();
                    self.password_rejected = true;
                    self.dialog = Some(Dialog::ClosingPassword);
                    return;
                }
                if what == "finished"
                    && let Some(ReconcileRefusal::NotInProgress) = reconcile_refusal(&e)
                {
                    // A retried finish whose first try went through.
                    if let Some(id) = self.rec.as_ref().map(|r| r.id) {
                        self.password.clear();
                        self.password_rejected = false;
                        self.card_pay = self.card_pay_due();
                        self.finished = Some(id);
                        self.dialog = Some(Dialog::Done);
                        self.back_to_begin();
                        self.reset_statement();
                        return;
                    }
                }
                self.refused(e);
            }
        }
    }

    // ------------------------------------------------------------ actions

    /// Begin: start the worksheet (or PATCH it in "Change statement info").
    pub(crate) fn begin(&mut self) {
        self.form.statement_date = self.date_in.value;
        self.form.service_charge.date = self.svc_date.value;
        self.form.interest.date = self.int_date.value;
        // The engine never refuses the worksheet's own date sent again
        // (one started before the order rule may be out of order).
        let own_date =
            self.editing && self.rec.as_ref().map(|r| r.statement_date) == self.form.statement_date;
        self.form.last_reconciled = if own_date {
            None
        } else {
            self.last_finished().map(|r| r.statement_date)
        };
        let v = match self.form.validate() {
            Ok(v) => v,
            Err(p) => {
                self.problems = p;
                return;
            }
        };
        self.problems.clear();
        self.notice = None;
        let charge = |c: Option<cl_model::reconcile::ChargeValue>| {
            c.map(|c| cl_api::StatementCharge {
                amount: c.amount,
                date: c.date,
                account: c.account,
            })
        };
        if self.editing
            && let Some((id, held_svc, held_int)) = self
                .rec
                .as_ref()
                .map(|r| (r.id, r.service_charge, r.interest))
        {
            // A charge goes only when it differs from the one held: a
            // posted one unchanged is never sent; changed, the engine takes
            // it once its entry was voided or deleted (U-193).
            let changed = |next: Option<cl_api::StatementCharge>,
                           held: Option<cl_api::HeldCharge>| {
                (next != held.map(|h| h.charge)).then_some(next)
            };
            let p = cl_api::ReconPatch {
                statement_date: Some(v.statement_date),
                statement_ending_balance: Some(v.ending),
                service_charge: changed(charge(v.service_charge), held_svc),
                interest: if self.kind() == ReconKind::Card {
                    None
                } else {
                    changed(charge(v.interest), held_int)
                },
            };
            self.set_rec_job(engine::write(
                &self.link,
                self.group.as_ref(),
                cl_api::OrphanLabel {
                    what: "orphan.reconcile_patch",
                    key: None,
                },
                {
                    let p = p.clone();
                    move |d| crate::company::demo::reconcile::patch(d, id, &p)
                },
                move |s| async move { s.reconciliations().patch(id, &p).await },
            ));
            // `took` leaves "Change statement info"; a refusal stays in it.
            return;
        }
        let req = ReconStart {
            account: v.account,
            statement_date: v.statement_date,
            statement_ending_balance: v.ending,
            service_charge: charge(v.service_charge),
            interest: charge(v.interest),
        };
        // Q-P27u: a charge already in the books (an Undo › Discard keeps it)
        // is asked about before it is added again.
        let charges: Vec<cl_api::StatementCharge> = [req.service_charge, req.interest]
            .into_iter()
            .flatten()
            .collect();
        if !std::mem::take(&mut self.dup_confirmed) && !charges.is_empty() {
            if self.dup_job.is_none() {
                self.dup_job = Some(charges_in_books_job(
                    &self.link,
                    self.group.as_ref(),
                    v.account,
                    charges,
                ));
            }
            return;
        }
        self.set_rec_job(engine::write(
            &self.link,
            self.group.as_ref(),
            cl_api::OrphanLabel {
                what: "orphan.reconcile_start",
                key: None,
            },
            {
                let r = req.clone();
                move |d| crate::company::demo::reconcile::start(d, &r)
            },
            move |s| async move {
                // Start answers the summary view (U-188): read the lines.
                let r = s.reconciliations();
                let started = r.start(&req).await?;
                r.get(started.id).await
            },
        ));
        self.last = Some("started");
    }

    /// "Add it again" (Q-P27u): Begin goes on with the charges as entered.
    pub(crate) fn add_charge_again(&mut self) {
        self.dialog = None;
        self.dup_confirmed = true;
        self.begin();
    }

    /// Continue the unfinished reconciliation.
    pub(crate) fn continue_open(&mut self) {
        if let Some(id) = self.unfinished().map(|r| r.id) {
            self.read_sheet(id);
            self.last = Some("continued");
        }
    }

    /// A worksheet read or write, stamped with the current epoch.
    fn set_rec_job(&mut self, job: Job<Reconciliation>) {
        self.rec_job = Some(job);
        self.rec_job_epoch = self.rec_epoch;
    }

    /// A card's pay step for the worksheet shown: the printed statement
    /// balance as the engine holds it, when the owner owes something.
    fn card_pay_due(&self) -> Option<CardPay> {
        let rec = self.rec.as_ref()?;
        if self.kind() != ReconKind::Card {
            return None;
        }
        let amount = self.kind().shown(rec.statement_ending_balance);
        amount.is_positive().then_some(CardPay {
            card: rec.account,
            amount,
            date: rec.statement_date,
        })
    }

    fn read_sheet(&mut self, id: ReconciliationId) {
        self.set_rec_job(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::reconcile::get(d, id),
            move |s| async move { s.reconciliations().get(id).await },
        ));
    }

    /// The Bank Feed's count of accepted lines whose entry was voided.
    fn read_entry_voided(&mut self) {
        if !self.link.is_demo()
            && !self
                .link
                .capabilities()
                .has(cl_api::Capabilities::FEED_REVIEW)
        {
            return;
        }
        self.voided_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            |d| Ok(crate::company::demo::feed_review::counts(d, None).entry_voided),
            |s| async move { Ok(s.feed().counts(None).await?.entry_voided) },
        ));
    }

    /// Retry after a failed answer: read the worksheet again.
    pub(crate) fn refresh_after_error(&mut self, id: ReconciliationId) {
        self.read_sheet(id);
    }

    /// Lines entered since Begin (another window): refresh, then read.
    fn refresh_sheet(&mut self, id: ReconciliationId) {
        if self.link.is_demo() {
            self.read_sheet(id);
            return;
        }
        self.set_rec_job(engine::write(
            &self.link,
            self.group.as_ref(),
            cl_api::OrphanLabel {
                what: "orphan.reconcile_refresh",
                key: None,
            },
            move |d| crate::company::demo::reconcile::get(d, id),
            move |s| async move {
                let r = s.reconciliations();
                r.refresh(id).await?;
                r.get(id).await
            },
        ));
    }

    /// Start over: discard the unfinished one.
    pub(crate) fn start_over(&mut self) {
        let Some(id) = self.unfinished().map(|r| r.id) else {
            return;
        };
        self.dialog = None;
        self.act(
            "canceled",
            move |d| crate::company::demo::reconcile::cancel(d, id),
            move |s| async move { s.reconciliations().cancel(id).await },
        );
    }

    fn act<F, Fut>(
        &mut self,
        what: &'static str,
        demo: impl FnOnce(&mut crate::company::demo::DemoState) -> Result<(), ApiError>,
        live: F,
    ) where
        F: FnOnce(cl_api::Session) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), ApiError>> + Send + 'static,
    {
        self.act_job = Some((
            what,
            engine::write(
                &self.link,
                self.group.as_ref(),
                cl_api::OrphanLabel {
                    what: "orphan.reconcile",
                    key: None,
                },
                demo,
                live,
            ),
        ));
    }

    /// Reconcile now (only at the engine's exact zero, nothing pending).
    pub(crate) fn finish(&mut self) {
        let Some(rec) = &self.rec else {
            return;
        };
        // Nothing on its way: a worksheet read landing after Finish would
        // bring the finished worksheet back (review D4).
        if !can_finish(rec.difference, &self.queue)
            || self.act_job.is_some()
            || self.rec_job.is_some()
            || self.tick_job.is_some()
        {
            return;
        }
        if rec.posting_locked && self.password.is_empty() {
            self.dialog = Some(Dialog::ClosingPassword);
            return;
        }
        let id = rec.id;
        let pw = (!self.password.is_empty()).then(|| Secret::new(self.password.clone()));
        let pw2 = pw.clone();
        self.dialog = None;
        self.act(
            "finished",
            move |d| crate::company::demo::reconcile::complete(d, id, pw2.as_ref()),
            move |s| async move {
                let auth = pw.map(cl_api::ClosedPeriodAuthorization::with_password);
                s.reconciliations().complete(id, auth.as_ref()).await
            },
        );
    }

    /// Undo the last finished reconciliation.
    pub(crate) fn undo_last(&mut self) {
        let Some(id) = self.last_finished().map(|r| r.id) else {
            return;
        };
        let mode = self.undo_mode;
        let pw = (!self.password.is_empty()).then(|| Secret::new(self.password.clone()));
        let pw2 = pw.clone();
        self.act(
            "undone",
            move |d| crate::company::demo::reconcile::undo(d, id, mode, pw2.as_ref()),
            move |s| async move {
                let auth = pw.map(cl_api::ClosedPeriodAuthorization::with_password);
                s.reconciliations().undo(id, mode, auth.as_ref()).await
            },
        );
    }

    /// The discrepancy report of the account.
    pub(crate) fn open_discrepancy(&mut self) {
        let Some(a) = self.form.account else {
            return;
        };
        self.dialog = Some(Dialog::Discrepancy);
        self.discrepancy = None;
        self.disc_job = Some(engine::read(
            &self.link,
            self.group.as_ref(),
            move |d| crate::company::demo::reconcile::discrepancy(d, a),
            move |s| async move { s.reconciliations().discrepancy(a).await },
        ));
    }

    /// "Change statement info": Begin again with the figures shown.
    pub(crate) fn change_info(&mut self) {
        let Some(rec) = &self.rec else {
            return;
        };
        let kind = self.kind();
        self.form.ending = cl_model::reconcile::display(kind.shown(rec.statement_ending_balance));
        self.date_in.set(rec.statement_date);
        for (held, input, date) in [
            (
                rec.service_charge,
                &mut self.form.service_charge,
                &mut self.svc_date,
            ),
            (rec.interest, &mut self.form.interest, &mut self.int_date),
        ] {
            *input = cl_model::reconcile::ChargeInput::default();
            if let Some(h) = held {
                input.amount = cl_model::reconcile::display(h.charge.amount);
                input.account = Some(h.charge.account);
                input.posted = h.posted.is_some();
                date.set(h.charge.date);
            }
        }
        // A charge not held yet gets the same default account as on
        // Begin (P27 QA-8).
        self.default_charge_accounts();
        self.editing = true;
        self.step = Step::Begin;
    }

    /// Leave for now: the work stays in the engine. Ticks not yet there
    /// go first; the window closes once the engine has them.
    pub(crate) fn leave(&mut self) {
        if self.queue.idle() && self.tick_job.is_none() {
            self.link
                .push(CompanyEvent::CloseWindow(CommandId::new(KIND)));
        } else {
            self.leaving = true;
        }
    }

    /// Ticks the engine doesn't have yet.
    pub(crate) fn ticks_pending(&self) -> bool {
        self.step == Step::Window && (!self.queue.idle() || self.tick_job.is_some())
    }

    // ------------------------------------------------------------ panes

    /// The lines shown (later lines hidden while the box is on).
    pub(crate) fn shown(&self) -> impl Iterator<Item = &ReconLine> {
        let hide = self.hide_later;
        self.lines
            .iter()
            .filter(move |l| !(hide && l.after_statement_date))
    }

    /// The lines shown in pane `p`.
    pub(crate) fn pane_lines(&self, p: Pane) -> Vec<&ReconLine> {
        self.shown()
            .filter(|l| cl_model::reconcile::pane_of(l.amount) == p)
            .collect()
    }

    pub(crate) fn toggle(&mut self, id: JournalLineId) {
        if self.rec.is_some() {
            self.queue.toggle(id);
            self.cursor = Some(id);
            self.focus_cursor = true;
        }
    }

    /// Mark all / Unmark all: the lines shown only (Q-P27c).
    pub(crate) fn mark_all(&mut self, on: bool) {
        let ids: Vec<JournalLineId> = self.shown().map(|l| l.journal_line).collect();
        self.queue.set(ids, on);
    }

    fn move_cursor(&mut self, down: bool) {
        let ids: Vec<JournalLineId> = self
            .pane_lines(self.pane)
            .iter()
            .map(|l| l.journal_line)
            .collect();
        let at = self.cursor.and_then(|c| ids.iter().position(|i| *i == c));
        let next = match (at, down) {
            (None, _) => ids.first(),
            (Some(i), true) => ids.get((i + 1).min(ids.len().saturating_sub(1))),
            (Some(i), false) => ids.get(i.saturating_sub(1)),
        };
        self.cursor = next.copied();
        self.focus_cursor = true;
    }

    /// Tab (`forward`) or Shift+Tab while the keyboard is in the panes:
    /// into the current pane when no row has the cursor, to the other pane
    /// (left to right forward, right to left back), else out of the panes
    /// to the controls after (or before) them. The cursor stays one truth
    /// with the focus (QA-9).
    pub(crate) fn tab(&mut self, forward: bool) {
        let kind = self.kind();
        let in_pane = self.cursor.is_some_and(|c| {
            self.pane_lines(self.pane)
                .iter()
                .any(|l| l.journal_line == c)
        });
        let (from, to) = if forward {
            (kind.left(), kind.right())
        } else {
            (kind.right(), kind.left())
        };
        if forward && !in_pane && !self.pane_lines(self.pane).is_empty() {
            self.cursor = None;
            self.move_cursor(true);
        } else if in_pane && self.pane == from && !self.pane_lines(to).is_empty() {
            self.pane = to;
            self.cursor = None;
            self.move_cursor(true);
        } else {
            self.focus_cursor = false;
            self.off_panes = true;
            self.leave_panes = Some(forward);
        }
    }

    /// Ctrl+E: the cursor's transaction in its form.
    fn open_cursor(&mut self) {
        let Some(l) = self
            .cursor
            .and_then(|c| self.lines.iter().find(|l| l.journal_line == c))
        else {
            return;
        };
        let txn = l.transaction;
        let form = match l.txn_type.as_str() {
            "check" => Some("vendors.write_check"),
            "deposit" => Some("banking.deposit"),
            "transfer" => Some("banking.transfer"),
            "journal_entry" => Some("company.journal_entry"),
            "expense" | "card_charge" | "credit_card_charge" => Some("vendors.card_charge"),
            "bill_payment" => Some("vendors.bill_payment"),
            _ => None,
        };
        match form {
            Some(kind) => self
                .link
                .push(CompanyEvent::OpenRecord(CommandId::new(kind), txn)),
            None => {
                if let Some(a) = self.form.account {
                    self.link.push(CompanyEvent::OpenRegister(a));
                }
            }
        }
        self.last = Some("opened");
    }

    /// The finished reconciliation the reports are about.
    pub(crate) fn report_target(&self) -> Option<ReconciliationId> {
        self.finished.or_else(|| self.last_finished().map(|r| r.id))
    }

    // ------------------------------------------------------------ dev

    fn dev_command(&mut self, s: &str) -> bool {
        let Some(mut d) = self.link.demo_state() else {
            return false;
        };
        match s {
            "dev.demo_reconcile_reset" => {
                crate::company::demo::reconcile::reset(&mut d);
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = CalendarDate::from_ymd(2026, 9, 30).ok();
                }
                drop(d);
                let accounts = std::mem::take(&mut self.accounts);
                let link = Arc::clone(&self.link);
                *self = ReconcileScreen::new(link, self.tenant.clone(), self.today);
                if !accounts.is_empty() {
                    self.accounts = accounts;
                }
                true
            }
            "dev.demo_reconcile_edit" => {
                crate::company::demo::reconcile::recategorize_reconciled(&mut d);
                true
            }
            "dev.demo_reconcile_close_books" => {
                if let Some(open) = d.open
                    && let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open)
                {
                    c.closed_through = CalendarDate::from_ymd(2026, 11, 1).ok();
                }
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ state

    pub(crate) fn state_json(&self) -> Value {
        let rec = self.rec.as_ref();
        let kind = self.kind();
        let money = |m: Money| cl_model::reconcile::display(kind.shown(m));
        let pane = |p: Pane| {
            let lines = self.pane_lines(p);
            json!({
                "rows": lines.len(),
                "checked": lines.iter().filter(|l| self.queue.ticked(l.journal_line)).count(),
            })
        };
        json!({
            "screen": "reconcile",
            "step": match self.step { Step::Begin => "begin", Step::Window => "window" },
            "supported": self.supported(),
            "kind": match kind { ReconKind::Bank => "bank", ReconKind::Card => "card" },
            "accounts": self.accounts.len(),
            "editing": self.editing,
            "unfinished": self.unfinished().is_some(),
            "last_reconciled": self.last_finished().map(|r| crate::shell::us_date(r.statement_date)),
            "problems": self.problems.iter().map(|p| p.key()).collect::<Vec<_>>(),
            "beginning": rec.map(|r| money(r.beginning_balance)),
            "ending": rec.map(|r| money(r.statement_ending_balance)),
            "cleared": rec.map(|r| money(r.cleared_balance)),
            "difference": rec.map(|r| money(r.difference)),
            "deposits": rec.map(|r| json!({"count": r.cleared_deposits.count, "total": cl_model::reconcile::display(r.cleared_deposits.total.abs())})),
            "payments": rec.map(|r| json!({"count": r.cleared_payments.count, "total": cl_model::reconcile::display(r.cleared_payments.total.abs())})),
            "pane": match self.pane { Pane::MoneyIn => "in", Pane::MoneyOut => "out" },
            "cursor": self.cursor.and_then(|c| self.lines.iter().find(|l| l.journal_line == c)).map(window::row_label),
            "left": pane(kind.left()),
            "right": pane(kind.right()),
            "hide_later": self.hide_later,
            "banner": window::banner_shown(self).0,
            "pending": !self.queue.idle(),
            "leaving": self.leaving,
            "password_rejected": self.password_rejected,
            "card_pay": self.card_pay.as_ref().map(|p| cl_model::reconcile::display(p.amount)),
            "can_finish": rec.is_some_and(|r| can_finish(r.difference, &self.queue)),
            "hint": window::hint_text(self),
            "finish_help": rec
                .filter(|r| !can_finish(r.difference, &self.queue))
                .map(|r| t(window::finish_help_key(r.difference))),
            "posting_locked": rec.is_some_and(|r| r.posting_locked),
            "entry_voided": self.entry_voided,
            "admin": self.is_admin(),
            "dialog": self.dialog.as_ref().map(|d| format!("{d:?}").to_lowercase()),
            "discrepancy_rows": self.discrepancy.as_ref().map(|d| d.rows.len()),
            "notice": self.notice,
            "last": self.last,
            "error": self.error.as_ref().map(|e| format!("{e}")),
            "reports": self.reports.state(),
        })
    }
}

/// One of `charges` is already in the books (Q-P27u): a line of the
/// account's register on its date, not void, for exactly its amount, whose
/// other side is the charge's account. Display-only amounts never match.
pub(crate) fn charge_in_books(
    lines: &[cl_api::RegisterLine],
    charge: &cl_api::StatementCharge,
) -> bool {
    lines.iter().any(|l| {
        l.date == charge.date
            && l.status != cl_api::TxnStatus::Void
            && matches!(l.category, cl_api::Category::Account { id, .. } if id == charge.account)
            && [l.payment.as_ref(), l.deposit.as_ref()]
                .into_iter()
                .flatten()
                .any(|a| a.exact().is_some_and(|m| m.abs() == charge.amount.abs()))
    })
}

/// Reads the account's register on each charge's date and answers whether
/// any charge is already there.
fn charges_in_books_job(
    link: &Arc<CompanyLink>,
    group: Option<&cl_api::RequestGroup>,
    account: AccountId,
    charges: Vec<cl_api::StatementCharge>,
) -> Job<bool> {
    let query = |c: &cl_api::StatementCharge| {
        let mut q = cl_api::RegisterQuery::new(c.date, c.date);
        q.include_void = false;
        q
    };
    let demo_charges = charges.clone();
    engine::read(
        link,
        group,
        move |d| {
            let chart = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            Ok(demo_charges.iter().any(|c| {
                crate::company::demo::register::page(&d.ledger, &chart, account, &query(c), 1, 200)
                    .is_some_and(|p| charge_in_books(&p.lines, c))
            }))
        },
        move |s| async move {
            for c in &charges {
                let page = s.register().page(account, &query(c), 1, 200).await?;
                if charge_in_books(&page.lines, c) {
                    return Ok(true);
                }
            }
            Ok(false)
        },
    )
}

impl Screen for ReconcileScreen {
    fn title(&self) -> String {
        match (self.step, self.form.account) {
            (Step::Window, Some(a)) => {
                let name = self
                    .account(a)
                    .map(crate::account_names::shown_name)
                    .unwrap_or_default();
                crate::i18n::t_with("reconcile.tab", &[("account", &name)])
            }
            _ => t(&format!("cmd.{KIND}")),
        }
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.poll();
        let busy = self.rec_job.is_some()
            || self.tick_job.is_some()
            || self.act_job.is_some()
            || self.accounts_job.is_some()
            || self.history_job.is_some()
            || self.disc_job.is_some()
            || self.dup_job.is_some()
            || self.role_job.is_some()
            || self.reports.busy();
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        match self.step {
            Step::Begin => begin::draw(self, ui),
            Step::Window => window::draw(self, ui),
        }
        finish::dialogs(self, ui);
    }

    fn dirty(&self) -> bool {
        self.ticks_pending()
    }

    fn can_save(&self) -> bool {
        // "Save" is waiting for the ticks to reach the engine.
        true
    }

    fn save(&mut self) -> bool {
        if !self.ticks_pending() {
            return true;
        }
        self.leaving = true;
        false
    }

    fn scope(&self) -> Scope {
        match self.step {
            // Off the panes Tab, Space and the arrows are the controls'
            // (every action reachable by keyboard).
            Step::Window if self.dialog.is_none() && !self.off_panes => Scope::Reconcile,
            _ => Scope::Window,
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        let s = id.as_str();
        if s.starts_with("dev.") {
            return self.dev_command(s);
        }
        if self.step != Step::Window {
            return false;
        }
        match s {
            "reconcile.tick" => {
                if let Some(c) = self.cursor {
                    self.toggle(c);
                }
            }
            // Tab: into the panes, to the other pane, then on to the
            // controls after them; Shift+Tab back (QA-9, keyboard fix).
            "reconcile.switch_pane" => self.tab(true),
            "reconcile.switch_pane_back" => self.tab(false),
            "reconcile.next" => self.move_cursor(true),
            "reconcile.prev" => self.move_cursor(false),
            "reconcile.mark_all" => self.mark_all(true),
            "reconcile.unmark_all" => self.mark_all(false),
            "reconcile.finish" => self.finish(),
            "reconcile.leave" => self.leave(),
            "edit.edit_transaction" => self.open_cursor(),
            _ => return false,
        }
        true
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}
