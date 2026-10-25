//! Make Deposit (P14 lane A, `banking.deposit`, Ctrl+D), on the Write Check
//! pattern (Q-261): the deposit card on top (Deposit to with the engine's
//! balance, Date, Memo), "Payments to deposit" (the payments waiting in
//! Payments Clearing, ticked whole), "Other deposits" (the shared lines
//! grid, `LinesSpec::DEPOSIT`: a negative amount is a fee, Q-84), the cash
//! back row, the totals line and Clear / Save & New / Save & Close; the
//! side panel shows bank-file deposits waiting (a click ticks the engine's
//! suggested grouping, Q-P14e), the engine's Payments Clearing balance and
//! recent deposits.
//!
//! Money (plan §4): the one exact gate is `cl_model::forms::deposit::check`
//! (total = payments + lines − cash back, greater than zero; a picked
//! payment is always the engine's exact amount), then
//! `BankPosting::deposit_with` refuses again before anything is sent. The
//! screen maps the checked deposit onto the draft field by field
//! ([`save::to_draft`]) and adds nothing up itself; balances are the
//! engine's. Every save and void publishes on the data bus (Q-262).
//!
//! Engine calls run on the request queue and are polled each frame;
//! `--demo` (and tests) answer through [`engine::LocalDeposits`].

mod engine;
mod form;
mod payments;
mod refund;
mod save;
mod summary;
mod template;

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, Contact, ContactFilter, DepositCaps,
    DepositRecord, FeedDeposit, FeedGroupings, IdempotencyKey, PendingDeposits, RecordVersion,
    RequestGroup, Tag, TagFilter,
};
use cl_core::ids::{AccountId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money};
use cl_model::forms::FormState;
use cl_model::forms::deposit::{
    BankMatch, DepositContext, DepositForm, DepositProblem, bank_match,
};
use cl_model::register::AccountClass;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::forms::lines_grid::{LinesGrid, LinesSpec};
use crate::i18n::t;
use crate::screens::company::form::DateInput;
use crate::screens::register::lists::RegisterLists;
use crate::windows::Screen;

pub(crate) use engine::{DemoDeposits, DepositRow, LocalDeposits, Saved, VendorCredit};
pub use save::to_draft;

/// The command and window kind.
pub const KIND: &str = "banking.deposit";

/// The Other deposits lines.
pub const LINES: LinesSpec = LinesSpec::DEPOSIT;

/// The Make Deposit window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(DepositScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
pub(crate) mod field {
    pub const ACCOUNT: u64 = 1;
    pub const DATE: u64 = 2;
    pub const MEMO: u64 = 3;
    pub const CASH_ACCOUNT: u64 = 4;
    pub const CASH_AMOUNT: u64 = 5;
    pub const CASH_MEMO: u64 = 6;
}

/// A header field the keyboard can be put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Account,
    Date,
    Memo,
    Payments,
    CashAccount,
    CashAmount,
}

/// The texts of the inputs as typed.
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub date: DateInput,
    pub cash_amount: String,
    /// Why the typed cash back is not accepted (i18n key).
    pub cash_problem: Option<&'static str>,
    /// The header revision the texts were last taken from.
    pub rev: u64,
}

/// A saved deposit being looked at or changed.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    pub version: RecordVersion,
    pub account: AccountId,
    pub old_date: CalendarDate,
    /// It took payments whole: read-only except Void (Q-P14g).
    pub grouped: bool,
    pub read_only: bool,
    pub note: &'static str,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(cl_model::forms::SaveIntent),
    /// A vendor refund (option 2).
    Refund,
    /// Void with this reason.
    Void(String),
}

/// Saving and its outcome.
#[derive(Default)]
pub(crate) struct SaveState {
    pub task: Option<Task<Saved>>,
    pub intent: Option<cl_model::forms::SaveIntent>,
    /// The idempotency key of the request being retried.
    pub key: Option<(save::SaveIdentity, IdempotencyKey)>,
    pub problems: Vec<DepositProblem>,
    pub error: Option<ApiError>,
    /// The deposit changed somewhere else since it was read.
    pub stale: bool,
    pub count: u32,
    /// `save`, `void`, `clear`, `load`, `refund`.
    pub last_action: Option<&'static str>,
    /// A plain note under the toolbar (i18n key).
    pub notice: Option<&'static str>,
    /// The engine warned that Payments Clearing went negative.
    pub clearing_warning: bool,
    /// The accounts the last save or void published (Q-262).
    pub published: Vec<AccountId>,
    /// A save whose outcome is unknown (B2).
    pub lost: crate::forms::lost_save::LostSave,
}

/// Prev/Next over the account's deposits (newest first).
#[derive(Default)]
pub(crate) struct Nav {
    pub list: Vec<DepositRow>,
    pub task: Option<Task<Vec<DepositRow>>>,
    pub loaded: bool,
    pub account: Option<AccountId>,
    pub at: Option<usize>,
    pub want_older: Option<bool>,
}

/// Void: the reason being typed.
#[derive(Default)]
pub(crate) struct VoidPrompt {
    pub open: bool,
    pub reason: String,
    pub task: Option<Task<()>>,
}

/// The lists the form picks from.
pub(crate) struct Loaded {
    pub accounts: Vec<Account>,
    pub contacts: Vec<Contact>,
    pub tags: Vec<Tag>,
    pub closing: Option<CalendarDate>,
}

/// The side panel's readers.
#[derive(Default)]
pub(crate) struct Panel {
    pub pending: Option<PendingDeposits>,
    pub pending_task: Option<Task<PendingDeposits>>,
    pub pending_failed: bool,
    pub waiting: Vec<FeedDeposit>,
    pub waiting_task: Option<Task<Vec<FeedDeposit>>>,
    /// The bank-file deposit the owner clicked.
    pub chosen: Option<FeedDeposit>,
    pub groupings_task: Option<Task<FeedGroupings>>,
    /// The engine had no grouping for the chosen deposit.
    pub no_grouping: bool,
    pub recent: Vec<DepositRow>,
    pub recent_task: Option<Task<Vec<DepositRow>>>,
    pub balance: Option<Money>,
    pub balance_task: Option<Task<Option<Money>>>,
}

/// The Make Deposit window.
pub struct DepositScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    /// The demo or a test double; `None`: the live engine.
    pub(crate) local: Option<Box<dyn LocalDeposits>>,
    pub(crate) accounts: Vec<Account>,
    pub(crate) contacts: Vec<Contact>,
    pub(crate) tags: Vec<Tag>,
    pub(crate) lists: Arc<RegisterLists>,
    lists_task: Option<Task<Loaded>>,
    pub(crate) loaded: bool,
    pub(crate) closing: Option<CalendarDate>,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<DepositForm>,
    pub(crate) lines: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    pub(crate) panel: Panel,
    pub(crate) editing: Option<Editing>,
    /// The saved deposit being shown (Summary).
    pub(crate) record: Option<DepositRecord>,
    pub(crate) load_task: Option<Task<cl_api::Versioned<cl_api::Transaction>>>,
    pub(crate) save: SaveState,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) nav: Nav,
    pub(crate) void: VoidPrompt,
    pub(crate) refund: refund::RefundState,
    /// The Summary view is shown instead of the form.
    pub(crate) show_summary: bool,
    pub(crate) focus: Option<Field>,
    pub(crate) focused: Option<Field>,
    pub(crate) after_frame: Option<CommandId>,
    /// The fixed bars' sizes last frame (Q-265).
    pub(crate) layout: crate::layout::FrameReport,
    pub(crate) money_style: cl_core::MoneyStyle,
    /// Saves in other windows (Q-262).
    pub(crate) bus: crate::data_bus::Subscription,
    pub(crate) lists_stale: bool,
    /// Prev (true) or Next asked on unsaved changes: Save / Discard /
    /// Keep editing.
    pub(crate) nav_prompt: Option<bool>,
    /// Prev/Next to take once the prompt's save is done.
    pub(crate) after_save_nav: Option<bool>,
    /// Save as Template and the template this deposit came from (P15).
    pub(crate) template: crate::screens::templates::SaveFlow,
}

/// A blank deposit (no lines: the grid holds them).
pub(crate) fn blank(account: Option<AccountId>, date: CalendarDate) -> DepositForm {
    DepositForm {
        account,
        date: Some(date),
        ..DepositForm::default()
    }
}

impl DepositScreen {
    /// A new deposit in the open company (`--demo` answers from the demo
    /// engine).
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> DepositScreen {
        let local: Option<Box<dyn LocalDeposits>> = link
            .is_demo()
            .then(|| Box::new(DemoDeposits) as Box<dyn LocalDeposits>);
        DepositScreen::with_local(link, tenant, today, local)
    }

    /// A new deposit answered by `local` (tests) or the live engine.
    #[must_use]
    pub(crate) fn with_local(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        local: Option<Box<dyn LocalDeposits>>,
    ) -> DepositScreen {
        let group = link.group().and_then(Result::ok);
        let lists = Arc::new(RegisterLists::default());
        let lines = LinesGrid::new(&LINES, Arc::clone(&lists), Vec::new());
        let mut s = DepositScreen {
            link,
            tenant,
            group,
            today,
            local,
            accounts: Vec::new(),
            contacts: Vec::new(),
            tags: Vec::new(),
            lists,
            lists_task: None,
            loaded: false,
            closing: None,
            header: FormState::new(blank(None, today)),
            lines,
            inputs: Inputs {
                date: DateInput::new(today),
                cash_amount: String::new(),
                cash_problem: None,
                rev: u64::MAX,
            },
            panel: Panel::default(),
            editing: None,
            record: None,
            load_task: None,
            save: SaveState::default(),
            closing_prompt: ClosingPrompt::default(),
            nav: Nav::default(),
            void: VoidPrompt::default(),
            refund: refund::RefundState::default(),
            show_summary: false,
            focus: Some(Field::Account),
            focused: None,
            after_frame: None,
            layout: crate::layout::FrameReport::default(),
            money_style: cl_core::MoneyStyle::default(),
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            nav_prompt: None,
            after_save_nav: None,
            template: crate::screens::templates::SaveFlow::default(),
        };
        s.bus = s.link.bus().subscribe();
        s.start_load();
        s.poll();
        s
    }

    /// The whole form: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> DepositForm {
        let mut f = self.header.current().clone();
        f.lines = self.lines.value().clone();
        f
    }

    /// The deposit parts the engine takes.
    pub(crate) fn caps(&self) -> DepositCaps {
        match &self.local {
            Some(l) => l.caps(),
            None => DepositCaps::from_caps(&self.link.capabilities()),
        }
    }

    /// The engine suggests groupings for bank-file deposits.
    pub(crate) fn feed_groupings(&self) -> bool {
        match &self.local {
            Some(l) => l.feed_groupings(),
            None => self
                .link
                .capabilities()
                .has(Capabilities::FEED_DEPOSIT_GROUPINGS),
        }
    }

    /// The engine takes the closing-date password.
    pub(crate) fn closing_supported(&self) -> bool {
        self.local.is_some() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    /// The account shown.
    pub(crate) fn account(&self) -> Option<AccountId> {
        self.header.current().account
    }

    /// The bank accounts money can be deposited into.
    pub(crate) fn deposit_accounts(&self) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| {
                a.is_active && crate::screens::register::class_of(a) == Some(AccountClass::Bank)
            })
            .collect()
    }

    /// The tenant's Payments Clearing account.
    pub(crate) fn clearing(&self) -> Option<AccountId> {
        self.accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("payments_clearing"))
            .map(|a| a.id)
    }

    /// The tenant's Accounts Payable account.
    pub(crate) fn payables(&self) -> Option<AccountId> {
        self.accounts
            .iter()
            .find(|a| {
                a.system_tag.as_deref() == Some("accounts_payable")
                    || crate::screens::register::class_of(a) == Some(AccountClass::Payable)
            })
            .map(|a| a.id)
    }

    /// Payments Clearing, A/R, A/P and Sales Tax Payable: cash back can't
    /// go there (PR #159).
    pub(crate) fn control_accounts(&self) -> Vec<AccountId> {
        self.accounts
            .iter()
            .filter(|a| {
                matches!(
                    a.system_tag.as_deref(),
                    Some(
                        "payments_clearing"
                            | "accounts_receivable"
                            | "accounts_payable"
                            | "sales_tax_payable"
                    )
                ) || matches!(
                    crate::screens::register::class_of(a),
                    Some(AccountClass::Receivable | AccountClass::Payable)
                )
            })
            .map(|a| a.id)
            .collect()
    }

    /// What `check` needs.
    pub(crate) fn context<'a>(&self, control: &'a [AccountId]) -> DepositContext<'a> {
        let caps = self.caps();
        DepositContext {
            grouping: caps.grouping,
            cashback: caps.cashback,
            control_accounts: control,
        }
    }

    fn start_load(&mut self) {
        if let Some(mut demo) = self.link.demo_state() {
            let accounts = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            let contacts = demo.contacts_mut().clone();
            let tags = demo.tags_mut().clone();
            let closing = demo.open.and_then(|open| {
                demo.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
            drop(demo);
            self.lists_task = Some(Task::ready(Loaded {
                accounts,
                contacts,
                tags,
                closing,
            }));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            let contacts = s
                .contacts()
                .list_all(&ContactFilter::default())
                .await
                .unwrap_or_default();
            let tags = s
                .tags()
                .list_all(&TagFilter::default())
                .await
                .unwrap_or_default();
            let closing = s.company_settings().await.ok().and_then(|c| c.lock_date);
            Ok(Loaded {
                accounts,
                contacts,
                tags,
                closing,
            })
        }) {
            Ok(p) => self.lists_task = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    fn apply_lists(&mut self) {
        let mut lists = RegisterLists::default();
        lists.set_accounts(&self.accounts);
        lists.set_contacts(&self.contacts);
        lists.set_tags(&self.tags);
        self.lists = Arc::new(lists);
        self.lines.set_lists(Arc::clone(&self.lists));
        self.inputs.rev = u64::MAX;
    }

    /// The account to start on: the first active bank account.
    fn default_account(&self) -> Option<AccountId> {
        self.deposit_accounts().first().map(|a| a.id)
    }

    pub(crate) fn poll(&mut self) {
        if let Some(r) = self.lists_task.as_mut().and_then(Task::try_take) {
            self.lists_task = None;
            self.loaded = true;
            if let Ok(l) = r {
                self.accounts = l.accounts;
                self.contacts = l.contacts;
                self.tags = l.tags;
                self.closing = l.closing;
                self.apply_lists();
                if self.account().is_none()
                    && let Some(a) = self.default_account()
                {
                    // Not an edit: the starting account is part of the blank form.
                    let mut f = self.header.current().clone();
                    f.account = Some(a);
                    self.header.load(f);
                }
                self.refresh_facts();
                self.refresh_pending();
            }
        }
        self.poll_panel();
        self.poll_save();
        self.poll_refund();
    }

    /// Saves in other windows (Q-262): the waiting payments, the balances
    /// and the recent deposits come again from the engine; the lists only
    /// while the form is clean.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale && !self.is_dirty() && self.lists_task.is_none() {
            self.lists_stale = false;
            self.start_load();
        }
        let clearing = self.clearing();
        if changes.ledger_any()
            && (clearing.is_none_or(|c| changes.ledger_touches(c))
                || self.account().is_some_and(|a| changes.ledger_touches(a)))
        {
            self.refresh_pending();
            self.refresh_facts();
        }
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.lines.dirty()
    }

    /// The texts from the form (after a load, clear, undo).
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.cash_amount = f
            .cash_back
            .amount
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        self.inputs.cash_problem = None;
        self.inputs.rev = self.header.revision();
    }

    /// Picks the deposit account.
    pub(crate) fn set_account(&mut self, account: AccountId) {
        if self.account() == Some(account) {
            return;
        }
        self.header
            .edit(field::ACCOUNT, |h| h.account = Some(account));
        self.panel.balance = None;
        self.panel.chosen = None;
        self.nav = Nav::default();
        self.refresh_facts();
    }

    /// The bank deposit the totals are compared with.
    pub(crate) fn bank_match(&self) -> Option<BankMatch> {
        let chosen = self.panel.chosen.as_ref()?;
        let total = self.compose().totals()?.total;
        bank_match(total, chosen.amount)
    }

    fn busy(&self) -> bool {
        self.lists_task.is_some()
            || self.save.task.is_some()
            || self.load_task.is_some()
            || self.nav.task.is_some()
            || self.void.task.is_some()
            || self.panel.pending_task.is_some()
            || self.panel.waiting_task.is_some()
            || self.panel.groupings_task.is_some()
            || self.panel.recent_task.is_some()
            || self.panel.balance_task.is_some()
            || self.refund.busy()
    }

    /// The control-channel state: flags, counts and the deposit total the
    /// scenarios check (plan §6.4); no names or memos.
    pub(crate) fn state_json(&self) -> Value {
        let form = self.compose();
        let totals = form.totals();
        let pending = self.panel.pending.as_ref();
        let mut v = json!({
            "screen": "deposit",
            "layout": self.layout.json(),
            "company_open": self.tenant.is_some(),
            "loaded": self.loaded,
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.editing.as_ref().is_some_and(|e| e.read_only),
            "grouped": self.editing.as_ref().is_some_and(|e| e.grouped),
            "account_id": form.account.map(|a| a.to_string()),
            "date": form.date.map(CalendarDate::to_wire),
            "balance_shown": self.panel.balance.is_some(),
            "pending": {
                "count": pending.map(|p| p.items.len()),
                "exact": pending.map(|p| p.exact),
                "failed": self.panel.pending_failed,
                "clearing_balance": pending.and_then(|p| p.clearing_balance.display(false)),
            },
            "picked": form.picked.len(),
            "lines": form.lines.used().count(),
            "cash_back": !form.cash_back.is_blank(),
            "totals": totals.map(|t| json!({
                "payments": t.payments.format(cl_core::MoneyStyle::default()),
                "other": t.other.format(cl_core::MoneyStyle::default()),
                "total": t.total.format(cl_core::MoneyStyle::default()),
            })),
            "bank_match": self.bank_match().map(|m| match m {
                BankMatch::Matches => "matches",
                BankMatch::Differs(_) => "differs",
            }),
            "waiting": self.panel.waiting.len(),
            "chosen": self.panel.chosen.is_some(),
            "no_grouping": self.panel.no_grouping,
            "dirty": self.is_dirty(),
            "saving": self.save.task.is_some(),
            "saved_count": self.save.count,
            "save": {
                "refused": self.save.problems.first().map(|p| p.name()),
                "problems": self.save.problems.len(),
                "error": self.save.error.is_some(),
                "stale": self.save.stale,
                "clearing_warning": self.save.clearing_warning,
            },
            "published": self.save.published.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "last_action": self.save.last_action,
            "notice": self.save.notice,
            "caps": {
                "grouping": self.caps().grouping,
                "cashback": self.caps().cashback,
                "vendor_refund": self.caps().vendor_refund,
                "feed_groupings": self.feed_groupings(),
            },
            "summary_open": self.show_summary,
            "void_open": self.void.open,
            "refund": self.refund.state_json(),
            "nav_at": self.nav.at,
            "nav_prompt": self.nav_prompt.is_some(),
            "template": self.template.state_json(),

            "focused": self.focused.map(|f| format!("{f:?}").to_lowercase()),
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
}

impl Screen for DepositScreen {
    fn title(&self) -> String {
        t("cmd.banking.deposit")
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
        self.recheck_problems();
        if let Some(cmd) = self.after_frame.take() {
            self.command(cmd);
        }
        if self.busy() {
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
        self.start_save(cl_model::forms::SaveIntent::Save, None)
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.closing_prompt.command(id) {
            return handled;
        }
        if let Some(handled) = self.lines.command(id) {
            return handled;
        }
        self.form_command(id)
    }

    fn open_template(&mut self, t: crate::forms::template::TemplateOpen) -> bool {
        self.fill_template(&t)
    }

    fn open_record(&mut self, txn: TransactionId) -> bool {
        let Some(account) = self.account().or_else(|| self.default_account()) else {
            return false;
        };
        self.load_deposit(txn, account);
        // Shown, or on its way from the engine; an unknown record opens
        // nothing new (the shell brings the window up as it is).
        let shown = self.editing.as_ref().is_some_and(|e| e.id == txn) || self.load_task.is_some();
        if !shown {
            self.save.error = None;
        }
        shown
    }

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
