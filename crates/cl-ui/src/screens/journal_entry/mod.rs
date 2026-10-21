//! Make Journal Entry (P15 lane J, `company.journal_entry`, Ctrl+J), on the
//! Write Check pattern (Q-261, Q-P15t; no mockup): the shared toolbar with
//! Prev/Next, Save as Template, Void and Delete; the header card (Date,
//! Entry No. suggested from the engine's next number, Reverse on, the
//! Adjusting flag, Memo); the shared lines grid in debit/credit mode
//! (Account · Debit · Credit · Memo · Name · Tags, one tag per line,
//! Q-233); the totals line; Clear / Save & New / Save & Close.
//!
//! Money: the owner's typed amounts are exact `Money` (cents, Q-232), kept
//! signed in the grid (debit positive) only as `cl_model::forms::journal`
//! defines. The totals and the balance helper are that module's exact sums
//! of the owner's own figures, never a ledger balance (non-negotiable 7).
//! **An unbalanced entry cannot save**: Save & New and Save & Close are off
//! with the reason in their tooltip, Alt+S / Alt+N refuse it with the
//! reason in the status bar, `check()` refuses it, and
//! `NewJournalEntry::new_with` refuses it again; nothing is sent.
//!
//! The A/R-A/P rule (Q-206) is refused locally in plain words; a closed
//! period (either date, and the reversal's) asks for the closing-date
//! password; an edit is guarded by the record's version (409 → Reload).
//! Every save, void and delete publishes the accounts it touched (Q-262).

mod engine;
mod form;
mod save;

use std::sync::Arc;

use cl_api::{
    AccountRoles, ApiError, Basis, Capabilities, DraftWarning, IdempotencyKey, NewJournalEntry,
    RecordVersion, RequestGroup, TemplateDraft, Transaction, TxnType, Versioned,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_model::forms::journal::{
    JournalContext, JournalForm, JournalProblem, NameKind, check, line_from_side, side_of,
};
use cl_model::forms::lines::FormLines;
use cl_model::forms::{FormState, SaveIntent};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::forms::lines_grid::{LinesGrid, LinesSpec};
use crate::forms::template::{SaveAsTemplate, TemplateOpen, TemplateSource};
use crate::i18n::t;
use crate::screens::card_charge::kit::Books;
use crate::screens::company::form::DateInput;
use crate::screens::register::lists::RegisterLists;
use crate::windows::Screen;

pub(crate) use engine::{DemoJournalEngine, JournalEngine};

/// The command and window kind.
pub const KIND: &str = "company.journal_entry";

/// The Make Journal Entry window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(JournalScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
mod field {
    pub const DATE: u64 = 1;
    pub const NUMBER: u64 = 2;
    pub const MEMO: u64 = 3;
    pub const ADJUSTING: u64 = 4;
    pub const REVERSE: u64 = 5;
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(SaveIntent),
    /// Void the open entry with this reason.
    Void(String),
    /// Delete the open entry.
    Delete,
}

/// A saved entry open in the form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub version: RecordVersion,
    /// Its date in the books (the closed-period rule looks at both dates).
    pub old_date: CalendarDate,
    /// Its report basis (kept on an edit; the form has no basis field).
    pub basis: Basis,
    /// Shown, never changed here: why (i18n key).
    pub read_only: Option<&'static str>,
    /// It is void.
    pub void: bool,
    /// The accounts it touches in the books (an edit, a void and a delete
    /// tell them, Q-262).
    pub accounts: Vec<AccountId>,
    /// Its live reversing entry and that entry's date.
    pub reversal: Option<(TransactionId, CalendarDate)>,
    /// The entry it reverses (it is a reversing entry).
    pub reverses: Option<TransactionId>,
}

/// What a save sends, for its idempotency key (never the password).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SaveIdentity {
    pub entry: NewJournalEntry,
    pub edit: Option<(TransactionId, RecordVersion)>,
    pub password: bool,
}

/// The header texts being typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub reverse: DateInput,
    pub rev: u64,
}

/// The Make Journal Entry window.
pub struct JournalScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    /// The demo engine or a test double (`None`: the live engine).
    pub(crate) local: Option<Box<dyn JournalEngine>>,
    pub(crate) books: Books,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<JournalForm>,
    pub(crate) lines: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    /// The Name column is shown (`je-names-v1`).
    pub(crate) names_column: bool,
    pub(crate) editing: Option<Editing>,
    pub(crate) load_task: Option<Task<Versioned<Transaction>>>,
    pub(crate) save_task: Option<Task<(TransactionId, RecordVersion)>>,
    pub(crate) action_task: Option<(Pending, Task<()>)>,
    /// A delete whose reply was lost (the entry may be gone): its retry
    /// takes "not found" as done.
    pub(crate) delete_unknown: Option<TransactionId>,
    pub(crate) number_task: Option<Task<String>>,
    pub(crate) intent: Option<SaveIntent>,
    pub(crate) key: Option<(SaveIdentity, IdempotencyKey)>,
    pub(crate) problems: Vec<JournalProblem>,
    pub(crate) error: Option<ApiError>,
    pub(crate) notice: Option<&'static str>,
    pub(crate) stale: bool,
    pub(crate) saved_count: u32,
    pub(crate) last_action: Option<&'static str>,
    pub(crate) last_saved: Option<TransactionId>,
    pub(crate) published: Vec<AccountId>,
    pub(crate) nav: Vec<TransactionId>,
    pub(crate) listed: bool,
    pub(crate) nav_task: Option<Task<Vec<TransactionId>>>,
    pub(crate) nav_at: Option<usize>,
    pub(crate) want_older: Option<bool>,
    pub(crate) void_open: bool,
    pub(crate) void_reason: String,
    pub(crate) delete_open: bool,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) money_style: MoneyStyle,
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) lists_stale: bool,
    pub(crate) focused: Option<u64>,
    pub(crate) nav_prompt: Option<bool>,
    pub(crate) after_save_nav: Option<bool>,
    pub(crate) after_frame: Option<CommandId>,
    /// The lines as last seen: a line that just got its account and has no
    /// amount takes the balancing amount (Q-P15n).
    pub(crate) seen_lines: FormLines,
    /// Save as Template (the Template dialog, Replace for a form opened
    /// from a template, Q-P15l), the template's warnings and the Templates
    /// Due date this entry fills in.
    pub(crate) template: crate::screens::templates::SaveFlow,
    /// The record being read is this window's own save (Save and stay).
    pub(crate) reading_back: bool,
}

impl JournalScreen {
    /// A new journal entry in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> JournalScreen {
        let local: Option<Box<dyn JournalEngine>> = link
            .is_demo()
            .then(|| Box::new(DemoJournalEngine) as Box<dyn JournalEngine>);
        JournalScreen::with_local(link, tenant, today, local)
    }

    pub(crate) fn with_local(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        local: Option<Box<dyn JournalEngine>>,
    ) -> JournalScreen {
        let group = link.group().and_then(Result::ok);
        let names_column = link.is_demo() || link.capabilities().has(Capabilities::JE_NAMES);
        let spec = if names_column {
            LinesSpec::JOURNAL
        } else {
            LinesSpec {
                customer: false,
                ..LinesSpec::JOURNAL
            }
        };
        let books = Books::default();
        let lines = LinesGrid::new(&spec, Arc::clone(&books.lists), Vec::new());
        let seen_lines = lines.value().clone();
        let mut s = JournalScreen {
            bus: link.bus().subscribe(),
            link,
            tenant,
            group,
            today,
            local,
            books,
            header: FormState::new(blank(today)),
            lines,
            inputs: Inputs {
                date: DateInput::new(today),
                reverse: empty_date(),
                rev: u64::MAX,
            },
            names_column,
            editing: None,
            load_task: None,
            save_task: None,
            action_task: None,
            delete_unknown: None,
            number_task: None,
            intent: None,
            key: None,
            problems: Vec::new(),
            error: None,
            notice: None,
            stale: false,
            saved_count: 0,
            last_action: None,
            last_saved: None,
            published: Vec::new(),
            nav: Vec::new(),
            listed: false,
            nav_task: None,
            nav_at: None,
            want_older: None,
            void_open: false,
            void_reason: String::new(),
            delete_open: false,
            closing_prompt: ClosingPrompt::default(),
            money_style: MoneyStyle::default(),
            lists_stale: false,
            focused: None,
            nav_prompt: None,
            after_save_nav: None,
            after_frame: None,
            seen_lines,
            template: crate::screens::templates::SaveFlow::default(),
            reading_back: false,
        };
        s.books.start(&s.link, s.group.as_ref());
        s.fetch_number();
        s.poll();
        s
    }

    /// The engine (or the demo) has `cap`.
    pub(crate) fn has(&self, cap: &str) -> bool {
        self.local.is_some() || self.link.capabilities().has(cap)
    }

    /// The engine takes the closing-date password.
    pub(crate) fn closing_supported(&self) -> bool {
        self.has(Capabilities::CLOSING_PASSWORD)
    }

    /// The chart's A/R and A/P accounts.
    pub(crate) fn roles(&self) -> AccountRoles {
        let mut r = AccountRoles::default();
        for a in &self.books.accounts {
            match crate::screens::register::class_of(a) {
                Some(AccountClass::Receivable) => {
                    r.receivable.insert(a.id);
                }
                Some(AccountClass::Payable) => {
                    r.payable.insert(a.id);
                }
                _ => {}
            }
        }
        r
    }

    /// What `check` needs: the A/R-A/P accounts, each name's kind and the
    /// engine's P15 capabilities.
    pub(crate) fn context(&self) -> JournalContext {
        let roles = self.roles();
        let names = self
            .books
            .contacts
            .iter()
            .map(|c| {
                let kind = match c.fields.contact_type {
                    cl_api::ContactType::Customer => NameKind {
                        customer: true,
                        vendor: false,
                    },
                    cl_api::ContactType::Vendor => NameKind {
                        customer: false,
                        vendor: true,
                    },
                    cl_api::ContactType::Both => NameKind {
                        customer: true,
                        vendor: true,
                    },
                };
                (c.id, kind)
            })
            .collect();
        JournalContext {
            receivable: roles.receivable,
            payable: roles.payable,
            names,
            names_supported: self.has(Capabilities::JE_NAMES),
            number_supported: self.has(Capabilities::JE_NUMBER),
            adjusting_supported: self.has(Capabilities::JE_ADJUSTING),
            reversing_supported: self.has(Capabilities::JE_REVERSING),
        }
    }

    /// The whole form: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> JournalForm {
        let mut f = self.header.current().clone();
        f.lines = self.lines.value().clone();
        f
    }

    /// Unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.lines.dirty()
    }

    /// The open entry can't be changed here: why.
    pub(crate) fn read_only(&self) -> Option<&'static str> {
        self.editing.as_ref().and_then(|e| e.read_only)
    }

    pub(crate) fn busy(&self) -> bool {
        self.save_task.is_some() || self.action_task.is_some()
    }

    /// Nothing typed may change while this is false.
    pub(crate) fn idle(&self) -> bool {
        !self.busy() && !self.closing_prompt.dialog_open()
    }

    pub(crate) fn poll(&mut self) {
        if self.books.poll() {
            self.lines.set_lists(Arc::clone(&self.books.lists));
            self.inputs.rev = u64::MAX;
        }
        if let Some(r) = self.number_task.as_mut().and_then(Task::try_take) {
            self.number_task = None;
            if let Ok(n) = r {
                self.suggest_number(n);
            }
        }
        if let Some(r) = self.save_task.as_mut().and_then(Task::try_take) {
            self.save_task = None;
            match r {
                Ok((id, v)) => self.saved(id, v),
                Err(e) => self.save_failed(e),
            }
        }
        if let Some((pending, mut task)) = self.action_task.take() {
            match task.try_take() {
                None => self.action_task = Some((pending, task)),
                Some(Ok(())) => self.action_done(&pending),
                Some(Err(e)) => self.action_failed(&e, pending),
            }
        }
        if let Some(r) = self.load_task.as_mut().and_then(Task::try_take) {
            self.load_task = None;
            match r {
                Ok(v) => self.show(&v),
                Err(e) => {
                    self.reading_back = false;
                    self.error = Some(e);
                }
            }
        }
        if let Some(r) = self.nav_task.as_mut().and_then(Task::try_take) {
            self.nav_task = None;
            self.nav = r.unwrap_or_default();
            self.listed = true;
            if let Some(older) = self.want_older.take() {
                self.step(older);
            }
        }
    }

    /// Saves in other windows (Q-262): a changed list of accounts, names
    /// or tags is read again once the form is clean.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && !self.books.busy() {
            self.lists_stale = false;
            self.books.start(&self.link, self.group.as_ref());
        }
        if changes.ledger_any() {
            // Another window's save may add or remove entries.
            self.listed = false;
        }
    }

    /// Asks the engine for the next entry number (`je-number-v1`).
    pub(crate) fn fetch_number(&mut self) {
        if !self.has(Capabilities::JE_NUMBER) {
            return;
        }
        let link = Arc::clone(&self.link);
        if let Some(local) = self.local.as_mut() {
            if let Some(n) = local.next_number(&link) {
                self.number_task = Some(Task::ready(n));
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move { s.journal().next_number().await }) {
            self.number_task = Some(Task::Pending(p));
        }
    }

    /// The suggested number goes into a new entry's empty Entry No. (part
    /// of the blank form, so it doesn't make the form dirty).
    fn suggest_number(&mut self, n: String) {
        if self.editing.is_some() || !self.header.current().number.is_empty() {
            return;
        }
        if self.header.is_dirty() {
            self.header.edit(field::NUMBER, |h| h.number = n);
        } else {
            let mut f = self.header.current().clone();
            f.number = n;
            self.header.load(f);
        }
        self.inputs.rev = u64::MAX;
    }

    /// The header texts from the form.
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.reverse = f.reverse_on.map_or_else(empty_date, DateInput::new);
        self.inputs.rev = self.header.revision();
    }

    /// The saved entry `v` as this form: each line one positive side.
    /// Anything else (or an adjusting entry from the Trial Balance module)
    /// is shown read-only (`false`). Amounts are the engine's, copied as
    /// they are.
    pub(crate) fn form_of(&mut self, v: &Versioned<Transaction>) -> (JournalForm, FormLines, bool) {
        let txn = &v.record;
        let mut lines: Vec<&cl_api::TransactionLine> = txn.lines.iter().collect();
        lines.sort_by_key(|l| l.line_order.unwrap_or(0));
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        let mut fits = txn.txn_type == TxnType::JournalEntry;
        let mut seeds = Vec::with_capacity(lines.len());
        for l in lines {
            let side = match (l.debit.is_positive(), l.credit.is_positive()) {
                (true, false) if l.credit.is_zero() => side_of(Some(l.debit)),
                (false, true) if l.debit.is_zero() => side_of(l.credit.checked_neg().ok()),
                _ => None,
            };
            let Some(side) = side else {
                fits = false;
                continue;
            };
            seeds.push(line_from_side(
                keys.next_key(),
                l.account_id,
                side,
                l.description.as_deref(),
                l.contact_id,
                l.tag_id,
            ));
        }
        let form = JournalForm {
            date: Some(txn.txn_date),
            number: txn.txn_number.clone().unwrap_or_default(),
            memo: txn.memo.clone().unwrap_or_default(),
            adjusting: txn.is_adjusting,
            reverse_on: txn.reverse_on,
            lines: FormLines::default(),
        };
        (form, FormLines::from_lines(seeds, keys), fits)
    }

    /// Shows a saved entry (Prev/Next, the register, Reload, after Save).
    pub(crate) fn show(&mut self, v: &Versioned<Transaction>) {
        let (form, lines, fits) = self.form_of(v);
        let lists = Arc::clone(&self.books.lists);
        self.header.load(form);
        self.lines.form_mut().load(lines, &*lists);
        self.seen_lines = self.lines.value().clone();
        let void = v.record.status == cl_api::TxnStatus::Void;
        let mut accounts: Vec<AccountId> = v.record.lines.iter().map(|l| l.account_id).collect();
        accounts.sort();
        accounts.dedup();
        self.editing = Some(Editing {
            id: v.record.id,
            version: v.version,
            old_date: v.record.txn_date,
            basis: v.record.basis,
            read_only: if void {
                Some("journal.read_only.void")
            } else if fits {
                None
            } else {
                Some("journal.read_only.shape")
            },
            void,
            accounts,
            reversal: v.record.reversal_transaction_id.zip(v.record.reverse_on),
            reverses: v.record.reverses_transaction_id,
        });
        self.focused = None;
        self.stale = false;
        self.key = None;
        self.problems.clear();
        self.error = None;
        self.void_open = false;
        self.delete_open = false;
        self.template.forget();
        if !std::mem::take(&mut self.reading_back) {
            self.last_action = Some("load");
        }
        self.sync_inputs();
    }

    /// A blank entry; Save & New keeps the date.
    pub(crate) fn new_form(&mut self, keep_date: bool) {
        let date = if keep_date {
            self.header.current().date.unwrap_or(self.today)
        } else {
            self.today
        };
        self.header.load(blank(date));
        let lists = Arc::clone(&self.books.lists);
        let fresh =
            FormLines::from_lines(Vec::new(), &mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().load(fresh, &*lists);
        self.seen_lines = self.lines.value().clone();
        self.editing = None;
        self.nav_at = None;
        self.nav_prompt = None;
        self.focused = None;
        self.void_open = false;
        self.delete_open = false;
        self.problems.clear();
        self.stale = false;
        self.template.forget();
        self.sync_inputs();
        self.fetch_number();
        self.poll_number();
    }

    fn poll_number(&mut self) {
        if let Some(r) = self.number_task.as_mut().and_then(Task::try_take) {
            self.number_task = None;
            if let Ok(n) = r {
                self.suggest_number(n);
            }
        }
    }

    /// Clear: throw the unsaved edits away.
    pub(crate) fn clear(&mut self) {
        if self.busy() {
            return;
        }
        self.error = None;
        self.notice = None;
        self.last_action = Some("clear");
        self.new_form(false);
    }

    /// A line that just got its account and has no amount takes the
    /// balancing amount (Q-P15n): the owner's own figures, overwritten by
    /// typing; Tab keeps it.
    pub(crate) fn prefill_balance(&mut self) {
        let now = self.lines.value().clone();
        if now == self.seen_lines {
            return;
        }
        let fresh: Vec<_> = now
            .lines
            .iter()
            .filter(|l| l.account.is_some() && l.amount.is_none())
            .filter(|l| {
                self.seen_lines
                    .lines
                    .iter()
                    .find(|o| o.key == l.key)
                    .is_none_or(|o| o.account.is_none())
            })
            .map(|l| l.key)
            .collect();
        if let Some(key) = fresh.first().copied()
            && self.idle()
            && self.read_only().is_none()
        {
            self.balance_line(key);
        }
        self.seen_lines = self.lines.value().clone();
    }

    /// Puts the balancing amount on line `key` (one undo step); the grid
    /// keeps a blank entry line last. False when already balanced.
    pub(crate) fn balance_line(&mut self, key: cl_grid::RowKey) -> bool {
        let mut v = self.lines.value().clone();
        if !cl_model::forms::journal::apply_balance(&mut v, key) {
            return false;
        }
        v.ensure_entry(&mut self.lines.form_mut().adapter_mut().keys);
        self.lines.form_mut().edit_step(|x| *x = v);
        true
    }

    /// "Balance on this line": the cursor's line, else the blank entry line.
    pub(crate) fn balance_here(&mut self) {
        let key = self
            .lines
            .cursor()
            .map(|c| c.row)
            .or_else(|| self.lines.value().lines.last().map(|l| l.key));
        let done = key.is_some_and(|k| self.balance_line(k));
        if !done {
            self.notice = Some("journal.balance_here.none");
        }
        self.seen_lines = self.lines.value().clone();
    }

    /// The Save as Template button.
    pub(crate) fn template_state(&self) -> SaveAsTemplate {
        if self.has(Capabilities::TEMPLATES) {
            self.template.button(&self.link, None)
        } else {
            SaveAsTemplate::Disabled(t("template.save.unavailable"))
        }
    }

    /// Save as Template (Ctrl+M): the same check as Save (the date, Entry
    /// No. and Reverse on aside, Q-P15f/g), then the Template dialog every
    /// form shares (or Replace / Save as new for an entry opened from a
    /// template).
    pub(crate) fn save_as_template(&mut self) -> bool {
        if self.template_state() != SaveAsTemplate::Enabled {
            self.notice = Some("template.save.unavailable");
            return true;
        }
        if !self.lines.commit() {
            return true;
        }
        match self.template_draft() {
            Ok(d) => {
                self.problems.clear();
                self.last_action = Some("template");
                let name = self.header.current().memo.trim().to_string();
                let link = Arc::clone(&self.link);
                self.template.start(
                    &link,
                    self.group.as_ref(),
                    d,
                    cl_api::client_form::JOURNAL_ENTRY,
                    name,
                    self.today,
                );
            }
            Err(p) => self.problems = p,
        }
        true
    }

    /// The control-channel state (flags, ids, dates and the owner's typed
    /// totals; never a name or a memo).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.compose();
        let totals = cl_model::forms::journal::totals(&f.lines);
        let fmt = |m: cl_core::Money| m.format(MoneyStyle::default());
        let blocker = cl_model::forms::journal::save_blocker(&f);
        let mut v = json!({
            "screen": "journal_entry",
            "company_open": self.tenant.is_some(),
            "loaded": self.books.loaded,
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.read_only(),
            "date": f.date.map(CalendarDate::to_wire),
            "number": f.number,
            "adjusting": f.adjusting,
            "reverse_on": f.reverse_on.map(CalendarDate::to_wire),
            "lines": f.lines.used().count(),
            "debits": totals.map(|t| fmt(t.debits)),
            "credits": totals.map(|t| fmt(t.credits)),
            "difference": totals.map(|t| fmt(t.difference.abs())),
            "balanced": totals.is_some_and(cl_model::forms::journal::JeTotals::balanced),
            "can_save": blocker.is_none(),
            "blocked_by": blocker.map(JournalProblem::name),
            "names_column": self.names_column,
            "dirty": self.is_dirty(),
            "saving": self.save_task.is_some(),
            "saved_count": self.saved_count,
            "txn_id": self.last_saved.map(|id| id.to_string()),
            "editing_id": self.editing.as_ref().map(|e| e.id.to_string()),
            "reversal_id": self.editing.as_ref().and_then(|e| e.reversal).map(|(r, _)| r.to_string()),
            "reverses": self.editing.as_ref().is_some_and(|e| e.reverses.is_some()),
            "save": {
                "refused": self.problems.first().map(|p| p.name()),
                "problems": self.problems.len(),
                "error": self.error.is_some(),
                "stale": self.stale,
            },
            "published": self.published.len(),
            "last_action": self.last_action,
            "notice": self.notice,
            "void_open": self.void_open,
            "delete_open": self.delete_open,
            "nav_at": self.nav_at,
            "nav_prompt": self.nav_prompt.is_some(),
            "template": self.template_json(),
            "grid": { "focused": self.lines.has_focus() },
        });
        if let (Value::Object(m), Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(self.closing_supported()),
        ) {
            m.extend(c);
        }
        v
    }

    /// Save as Template's state: the button, then the shared flow's (the
    /// dialog, Replace, the template it came from, its warnings).
    fn template_json(&self) -> Value {
        let mut v = self.template.state_json();
        if let Value::Object(m) = &mut v {
            m.insert(
                "button".into(),
                json!(match self.template_state() {
                    SaveAsTemplate::Hidden => "hidden",
                    SaveAsTemplate::Disabled(_) => "disabled",
                    SaveAsTemplate::Enabled => "enabled",
                }),
            );
        }
        v
    }

    /// A refused save's problems, checked again against the form as it is
    /// now (a fixed problem goes away at once, P13).
    pub(crate) fn recheck_problems(&mut self) {
        if self.problems.is_empty() {
            return;
        }
        let now = check(&self.compose(), &self.context())
            .err()
            .unwrap_or_default();
        crate::forms::recheck(&mut self.problems, now);
    }
}

/// A blank entry dated `date`.
fn blank(date: CalendarDate) -> JournalForm {
    JournalForm {
        date: Some(date),
        ..JournalForm::default()
    }
}

/// An empty date field (Reverse on).
fn empty_date() -> DateInput {
    DateInput {
        text: String::new(),
        value: None,
    }
}

impl TemplateSource for JournalScreen {
    type Problem = JournalProblem;

    fn template_draft(&self) -> Result<TemplateDraft, Vec<JournalProblem>> {
        let mut f = self.compose();
        // A template keeps neither the date nor the number nor Reverse on
        // (Q-P15f, Q-P15g); the form's own check decides the rest.
        f.date = Some(f.date.unwrap_or(self.today));
        f.number.clear();
        f.reverse_on = None;
        let c = check(&f, &self.context())?;
        self.entry_of(&c, Basis::Both)
            .map(TemplateDraft::Journal)
            .map_err(|_| vec![JournalProblem::TooLarge])
    }

    fn fill_from_template(&mut self, draft: TemplateDraft, warnings: &[DraftWarning]) {
        let TemplateDraft::Journal(e) = draft else {
            return;
        };
        self.new_form(false);
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        let seeds = e
            .lines()
            .iter()
            .map(|l| {
                let side = match l.side {
                    cl_api::Side::Debit(m) => cl_model::forms::journal::JeSide::Debit(m),
                    cl_api::Side::Credit(m) => cl_model::forms::journal::JeSide::Credit(m),
                };
                let mut line = line_from_side(
                    keys.next_key(),
                    l.account,
                    side,
                    l.description.as_deref(),
                    l.contact,
                    l.tag,
                );
                // An account the engine dropped (`MISSING_ACCOUNT`) shows
                // empty, as on every other form.
                line.account = crate::screens::templates::real_account(l.account);
                line
            })
            .collect();
        let lines = FormLines::from_lines(seeds, keys);
        self.lines.form_mut().edit_step(|v| *v = lines);
        self.seen_lines = self.lines.value().clone();
        let memo = e.memo().unwrap_or_default().to_string();
        let adjusting = e.adjusting();
        self.header.edit_step(|h| {
            h.memo = memo;
            h.adjusting = adjusting;
        });
        self.template.warnings = warnings.to_vec();
        self.sync_inputs();
    }
}

impl Screen for JournalScreen {
    fn title(&self) -> String {
        t("cmd.company.journal_entry")
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        if style != self.money_style {
            self.money_style = style;
            self.inputs.rev = u64::MAX;
        }
        self.poll();
        self.hear_changes();
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
        self.draw(ui);
        self.prefill_balance();
        self.recheck_problems();
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        let busy = self.books.busy()
            || self.busy()
            || self.load_task.is_some()
            || self.nav_task.is_some()
            || self.number_task.is_some();
        if busy {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.is_dirty()
    }

    fn save(&mut self) -> bool {
        self.start_save(SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(h) = self.closing_prompt.command(id) {
            return h;
        }
        if let Some(h) = self.lines.command(id) {
            return h;
        }
        self.form_command(id)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        self.load(txn);
        self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_task.is_some()
    }

    fn open_template(&mut self, template: TemplateOpen) -> bool {
        // Only a journal entry's draft fills this form.
        if template.kind.as_str() != KIND || !matches!(template.draft, TemplateDraft::Journal(_)) {
            return false;
        }
        let date = template.occurrence;
        self.fill_from_template(template.draft.clone(), &template.warnings);
        if let Some(d) = date {
            self.header.edit(field::DATE, |h| h.date = Some(d));
            self.sync_inputs();
        }
        // Remembered for Replace, and the Templates Due date it fills in.
        self.template.opened(&template);
        true
    }

    fn state(&self) -> Value {
        self.state_json()
    }
}

#[cfg(test)]
mod tests;
