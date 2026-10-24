//! Write Check (P13, `vendors.write_check`, Ctrl+W): the check card on top,
//! the Expenses lines below, the totals line and Clear / Save & New /
//! Save & Close.
//!
//! The form is a [`CheckForm`] (cl-model): the header fields live in a
//! `FormState<CheckForm>` here and the lines in the shared
//! [`LinesGrid`]; [`WriteCheckScreen::compose`] puts them together. The
//! one exact gate is `cl_model::forms::check::check` (the lines must equal
//! the check amount to the cent; nothing is sent otherwise), and again
//! `BankPosting::check_with` in cl-api (plan §4). The save maps the checked
//! check field by field onto the engine draft (`save::to_draft`), with no
//! arithmetic.
//!
//! The ending balance is the engine's account balance, read again after
//! every save; the screen never derives one (non-negotiable 7). The amount
//! in words comes from `cl_model::forms::amount_words`.
//!
//! WP7 adds payee prefill ([`prefill`], Q-191/Q-192), the payee panel
//! ([`panel`]: open balance and the open-bill note, recent checks), the
//! attachments ([`attach`]) and account suggestions ([`suggest`]: the
//! owner's rules offline, the AI only behind every AI switch).
//!
//! Every engine call runs on the request queue and is polled each frame;
//! `--demo` answers from `company::demo::checks`.

mod attach;
mod form;
mod panel;
pub(crate) mod prefill;
mod save;
mod suggest;
mod template;

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Capabilities, CompanySettings, Contact, ContactFilter,
    IdempotencyKey, PrintStatus, RecordVersion, RequestGroup, Tag, TagFilter,
};
use cl_core::ids::{AccountId, TagId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money};
use cl_model::forms::FormState;
use cl_model::forms::check::{CheckForm, CheckProblem, NumberField, propose_number};
use cl_model::forms::lines::FormLines;
use cl_model::lists::tags::{TagEntry, TagList};
use cl_model::register::AccountClass;
use cl_model::rowkey::RowKeyGen;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::forms::closing_prompt::ClosingPrompt;
use crate::forms::lines_grid::{L_BILLABLE, L_CUSTOMER, LinesGrid, LinesSpec};
use crate::i18n::{t, t_with};
use crate::screens::company::form::DateInput;
use crate::screens::register::lists::RegisterLists;
use crate::windows::Screen;

pub use save::to_draft;

/// The command and window kind.
pub const KIND: &str = "vendors.write_check";

/// The Expenses lines (every column, Q-247: no Items tab).
pub const LINES: LinesSpec = LinesSpec::all("write_check/lines", "check.lines.label");

/// The Write Check window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(WriteCheckScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// Header fields (undo merges typing within one field).
mod field {
    pub const BANK: u64 = 1;
    pub const NUMBER: u64 = 2;
    pub const DATE: u64 = 3;
    pub const PAYEE: u64 = 4;
    pub const AMOUNT: u64 = 5;
    pub const ADDRESS: u64 = 6;
    pub const MEMO: u64 = 7;
}

/// A header field the keyboard can be put in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Bank,
    Number,
    Date,
    Payee,
    Amount,
    Address,
    Memo,
}

/// The company name and address on the check card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CompanyCard {
    pub name: String,
    pub address: String,
}

/// The text of the header inputs as typed (the form holds the values).
#[derive(Clone, Debug)]
pub(crate) struct Inputs {
    pub number: String,
    pub date: DateInput,
    pub payee: String,
    pub amount: String,
    /// Why the typed amount is not accepted (i18n key).
    pub amount_problem: Option<&'static str>,
    /// The typed payee is not a name from the list.
    pub payee_problem: bool,
    /// The header revision the texts were last taken from.
    pub rev: u64,
}

/// A saved check being changed.
#[derive(Clone, Debug)]
pub(crate) struct Editing {
    pub id: TransactionId,
    /// The check's version (`PUT /checks/{id}`, `check-edit-v1`).
    pub version: RecordVersion,
    /// The ledger record's version (`PUT /transactions/{id}`, the path
    /// without `check-edit-v1`): its fingerprint leaves out the check
    /// fields.
    pub txn_version: RecordVersion,
    /// The bank account the check is drawn on (its bank line).
    pub bank: AccountId,
    pub old_date: CalendarDate,
    pub print_status: PrintStatus,
    /// Shown, never changed here: void, a check that pays bills, lines
    /// this form can't show exactly, or line customers the engine can't
    /// take yet.
    pub read_only: bool,
    /// Why it is read-only (i18n key).
    pub read_only_note: &'static str,
}

/// What the closing-date password is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// The save (with what follows it).
    Save(cl_model::forms::SaveIntent),
    /// Void with this reason.
    Void(String),
}

/// The last save's outcome, for the control channel (no amounts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LastSaved {
    pub number: Option<u32>,
    pub print_status: String,
}

/// Saving and its outcome.
#[derive(Default)]
pub(crate) struct SaveState {
    pub task: Option<Task<save::Saved>>,
    pub intent: Option<cl_model::forms::SaveIntent>,
    /// The idempotency key of the request being retried.
    pub key: Option<(save::SaveIdentity, IdempotencyKey)>,
    pub problems: Vec<CheckProblem>,
    pub error: Option<ApiError>,
    /// The number the engine refused as used, and the one offered instead.
    pub duplicate: Option<(u32, u32)>,
    /// The check changed somewhere else since it was read.
    pub stale: bool,
    pub count: u32,
    pub last: Option<LastSaved>,
    /// `void`, `save`, `clear`, `load`.
    pub last_action: Option<&'static str>,
    /// A plain note under the toolbar (i18n key).
    pub notice: Option<&'static str>,
    /// The owner's answer for a printed check whose printed face changes
    /// (sent with the next save of this check).
    pub printed_choice: Option<cl_api::PrintedCheckChange>,
}

/// "This check was already printed": Reprint / Keep number / Cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrintedPrompt {
    /// The printed check's number.
    pub number: Option<u32>,
    /// What the save was for.
    pub intent: cl_model::forms::SaveIntent,
}

/// Prev/Next over the bank account's checks (newest first).
#[derive(Default)]
pub(crate) struct Nav {
    pub list: Vec<cl_api::CheckSummary>,
    pub task: Option<Task<Vec<cl_api::CheckSummary>>>,
    /// The list is complete (the last page was short).
    pub done: bool,
    /// The bank account the list is for.
    pub bank: Option<AccountId>,
    /// Where the form is in the list (`None`: a new check).
    pub at: Option<usize>,
    /// A step waiting for the next page: true is older (Prev).
    pub want_older: Option<bool>,
}

/// Void (Edit menu): the reason being typed.
#[derive(Default)]
pub(crate) struct VoidPrompt {
    pub open: bool,
    pub reason: String,
    pub task: Option<Task<()>>,
}

/// History of a saved check.
#[derive(Default)]
pub(crate) struct HistoryView {
    pub open: bool,
    pub rows: Vec<String>,
    pub task: Option<Task<Vec<String>>>,
    pub error: bool,
}

/// The Write Check window.
pub struct WriteCheckScreen {
    pub(crate) link: Arc<CompanyLink>,
    pub(crate) tenant: Option<(TenantId, String)>,
    pub(crate) group: Option<RequestGroup>,
    pub(crate) today: CalendarDate,
    pub(crate) accounts: Vec<Account>,
    accounts_task: Option<Task<Vec<Account>>>,
    pub(crate) accounts_loaded: bool,
    pub(crate) contacts: Vec<Contact>,
    contacts_task: Option<Task<Vec<Contact>>>,
    pub(crate) tags: Vec<Tag>,
    tags_task: Option<Task<Vec<Tag>>>,
    pub(crate) tag_list: TagList,
    pub(crate) lists: Arc<RegisterLists>,
    settings_task: Option<Task<CompanySettings>>,
    pub(crate) closing: Option<CalendarDate>,
    company_task: Option<Task<CompanyCard>>,
    pub(crate) company: CompanyCard,
    /// The header (its `lines` stay empty: the grid holds them).
    pub(crate) header: FormState<CheckForm>,
    pub(crate) lines: LinesGrid<RegisterLists>,
    pub(crate) inputs: Inputs,
    /// The engine's next number and the highest check number in the
    /// account, for the proposal (Q-P13m).
    numbers_task: Option<Task<(Option<u32>, Option<u32>)>>,
    pub(crate) proposal: Option<u32>,
    balance_task: Option<Task<Option<Money>>>,
    pub(crate) balance: Option<Money>,
    pub(crate) editing: Option<Editing>,
    pub(crate) load_task: Option<Task<cl_api::Versioned<cl_api::CheckRecord>>>,
    /// The bank account the check being read is drawn on.
    pub(crate) load_bank: Option<AccountId>,
    pub(crate) save: SaveState,
    pub(crate) closing_prompt: ClosingPrompt<Pending>,
    pub(crate) nav: Nav,
    pub(crate) void: VoidPrompt,
    pub(crate) history: HistoryView,
    /// Put the keyboard here next frame.
    pub(crate) focus: Option<Field>,
    /// The header field that had the keyboard last frame.
    pub(crate) focused: Option<Field>,
    /// The payee name list is open (typing or Ctrl+L).
    pub(crate) payee_list: bool,
    /// The lines grid had the keyboard last frame.
    in_grid: bool,
    /// A command to run once the frame's fields are drawn (Enter in a
    /// header field is Save & New, Q-190).
    pub(crate) after_frame: Option<CommandId>,
    /// The fixed bars' sizes last frame (Q-265).
    pub(crate) layout: crate::layout::FrameReport,
    /// The owner's money display style (from the shell each frame).
    pub(crate) money_style: cl_core::MoneyStyle,
    /// Payee prefill (WP7).
    pub(crate) prefill: prefill::PrefillState,
    /// The payee panel's cards.
    pub(crate) panel: panel::PanelState,
    /// Attachments.
    pub(crate) attach: attach::AttachState,
    /// Account suggestions.
    pub(crate) suggest: suggest::SuggestState,
    /// Prev (true) or Next asked on unsaved changes: Save / Discard /
    /// Keep editing.
    pub(crate) nav_prompt: Option<bool>,
    /// Prev/Next to take once the prompt's save is done.
    pub(crate) after_save_nav: Option<bool>,
    /// The printed-check question after a 409 `CHECK_PRINTED`.
    pub(crate) printed: Option<PrintedPrompt>,
    /// Saves in other windows (Q-262).
    pub(crate) bus: crate::data_bus::Subscription,
    /// The names, accounts or tags changed elsewhere while the form had
    /// unsaved edits: read them again once it is clean.
    pub(crate) lists_stale: bool,
    /// Save as Template and the template this check came from (P15).
    pub(crate) template: crate::screens::templates::SaveFlow,
}

/// The tag list `check` needs for the header-tag defaults.
fn tag_list(tags: &[Tag]) -> TagList {
    TagList::new(
        Vec::new(),
        tags.iter()
            .map(|t| TagEntry {
                id: t.id,
                name: t.fields.name.clone(),
                group: t.fields.group_id,
                color: t.fields.color.clone(),
                description: t.fields.description.clone(),
                active: t.is_active,
                usage: t.usage_count,
            })
            .collect(),
    )
}

/// A blank check form (no lines: the grid holds them).
pub(crate) fn blank(bank: Option<AccountId>, date: CalendarDate, number: NumberField) -> CheckForm {
    let mut f = CheckForm::new(bank, Some(date), number, &mut RowKeyGen::new());
    f.lines = FormLines::default();
    f
}

impl WriteCheckScreen {
    /// A new check in the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> WriteCheckScreen {
        let group = link.group().and_then(Result::ok);
        let lists = Arc::new(RegisterLists::default());
        let lines = LinesGrid::new(&LINES, Arc::clone(&lists), Vec::new());
        let mut s = WriteCheckScreen {
            link,
            tenant,
            group,
            today,
            accounts: Vec::new(),
            accounts_task: None,
            accounts_loaded: false,
            contacts: Vec::new(),
            contacts_task: None,
            tags: Vec::new(),
            tags_task: None,
            tag_list: TagList::default(),
            lists,
            settings_task: None,
            closing: None,
            company_task: None,
            company: CompanyCard::default(),
            header: FormState::new(blank(None, today, NumberField::Typed(String::new()))),
            lines,
            inputs: Inputs {
                number: String::new(),
                date: DateInput::new(today),
                payee: String::new(),
                amount: String::new(),
                amount_problem: None,
                payee_problem: false,
                rev: u64::MAX,
            },
            numbers_task: None,
            proposal: None,
            balance_task: None,
            balance: None,
            editing: None,
            load_task: None,
            load_bank: None,
            save: SaveState::default(),
            closing_prompt: ClosingPrompt::default(),
            nav: Nav::default(),
            void: VoidPrompt::default(),
            history: HistoryView::default(),
            focus: Some(Field::Payee),
            focused: None,
            payee_list: false,
            in_grid: false,
            after_frame: None,
            layout: crate::layout::FrameReport::default(),
            money_style: cl_core::MoneyStyle::default(),
            prefill: prefill::PrefillState::default(),
            panel: panel::PanelState::default(),
            attach: attach::AttachState::default(),
            suggest: suggest::SuggestState::default(),
            nav_prompt: None,
            after_save_nav: None,
            printed: None,
            bus: crate::data_bus::Subscription::detached(),
            lists_stale: false,
            template: crate::screens::templates::SaveFlow::default(),
        };
        s.bus = s.link.bus().subscribe();
        s.start_load();
        // The demo answers at once: the form is complete before the first
        // frame.
        s.poll();
        s
    }

    /// The mockup's unsaved draft for `--demo` (Ozark Feed & Seed, No.
    /// 4418, 1,286.40 on two lines, Print later off).
    #[must_use]
    pub fn demo_draft(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> WriteCheckScreen {
        let mut s = WriteCheckScreen::new(link, tenant, today);
        s.seed_mockup();
        s
    }

    fn seed_mockup(&mut self) {
        let by_number = |n: &str| {
            self.accounts
                .iter()
                .find(|a| a.fields.account_number.as_deref() == Some(n))
                .map(|a| a.id)
        };
        let contact = |name: &str| {
            self.contacts
                .iter()
                .find(|c| c.fields.display_name == name)
                .map(|c| c.id)
        };
        let tag = |name: &str| -> Option<TagId> {
            self.tags
                .iter()
                .find(|t| t.fields.name == name)
                .map(|t| t.id)
        };
        let money = |s: &str| Money::parse_wire(s).ok();
        let (plants, mulch) = (by_number("5100"), by_number("5110"));
        let payee = contact("Ozark Feed & Seed");
        let customer = contact("Hollister Family Trust");
        let install = tag("Install");
        self.header.edit_step(|h| {
            h.number = NumberField::Typed("4418".into());
            h.payee = payee;
            h.payee_name = "Ozark Feed & Seed".into();
            h.address = "Ozark Feed & Seed\n1200 Commerce Dr\nAurora, MO 65605".into();
            h.amount = money("1286.40");
            h.memo = "Acct 7712 \u{b7} perennials and mulch".into();
        });
        let keys = &mut self.lines.form_mut().adapter_mut().keys;
        let mut seeded = vec![
            cl_model::forms::lines::FormLine {
                account: plants,
                amount: money("842.15"),
                memo: "Perennials".into(),
                customer,
                billable: customer.is_some(),
                tag: install,
                ..cl_model::forms::lines::FormLine::blank(keys.next_key())
            },
            cl_model::forms::lines::FormLine {
                account: mulch,
                amount: money("444.25"),
                memo: "Hardwood mulch".into(),
                tag: install,
                ..cl_model::forms::lines::FormLine::blank(keys.next_key())
            },
        ];
        seeded.push(cl_model::forms::lines::FormLine::blank(keys.next_key()));
        self.lines.form_mut().edit_step(|v| v.lines = seeded);
        self.sync_inputs();
    }

    /// The whole check: the header with the grid's lines.
    #[must_use]
    pub(crate) fn compose(&self) -> CheckForm {
        let mut f = self.header.current().clone();
        f.lines = self.lines.value().clone();
        f
    }

    /// The engine takes customers and billable marks on check lines.
    pub(crate) fn line_billable(&self) -> bool {
        // The demo engine behaves like `line-billable-v1` unless a test
        // turned it off.
        match self.link.demo_state() {
            Some(d) => !d.checks.line_billable_off,
            None => self.link.capabilities().has(Capabilities::LINE_BILLABLE),
        }
    }

    /// The engine edits a check's own fields (`check-edit-v1`).
    pub(crate) fn check_edit(&self) -> bool {
        match self.link.demo_state() {
            Some(d) => !d.checks.check_edit_off,
            None => self.link.capabilities().has(Capabilities::CHECK_EDIT),
        }
    }

    /// The engine takes the closing-date password.
    pub(crate) fn closing_supported(&self) -> bool {
        self.link.is_demo() || self.link.capabilities().has(Capabilities::CLOSING_PASSWORD)
    }

    /// The bank account shown.
    pub(crate) fn bank(&self) -> Option<AccountId> {
        self.header.current().bank
    }

    /// The active bank accounts a check can be written on.
    pub(crate) fn bank_accounts(&self) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| {
                a.is_active && crate::screens::register::class_of(a) == Some(AccountClass::Bank)
            })
            .collect()
    }

    fn start_load(&mut self) {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
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
            self.closing = closing;
            self.accounts_task = Some(Task::ready(chart));
            self.contacts_task = Some(Task::ready(contacts));
            self.tags_task = Some(Task::ready(tags));
            self.company_task = Some(Task::ready(CompanyCard {
                name: "Ozark Ridge Landscaping LLC".into(),
                address: "410 Ridge Rd \u{b7} Monett, MO 65708".into(),
            }));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.accounts_loaded = true;
            return;
        };
        if let Ok(p) = group.read(&session, |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            self.accounts_task = Some(Task::Pending(p));
        } else {
            self.accounts_loaded = true;
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.contacts().list_all(&ContactFilter::default()).await
        }) {
            self.contacts_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.tags().list_all(&TagFilter::default()).await
        }) {
            self.tags_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move { s.company_settings().await }) {
            self.settings_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, |s| async move {
            let p = s.company_profile().await?.record.profile;
            let city = [
                p.city,
                p.state.map(|st| match p.zip {
                    Some(z) => format!("{st} {z}"),
                    None => st,
                }),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ");
            let address = [p.address_line1, p.address_line2, Some(city)]
                .into_iter()
                .flatten()
                .filter(|l| !l.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" \u{b7} ");
            Ok(CompanyCard {
                name: p.business_name,
                address,
            })
        }) {
            self.company_task = Some(Task::Pending(p));
        }
    }

    /// Reads the next number and the ending balance for the bank account.
    pub(crate) fn fetch_bank_facts(&mut self) {
        let Some(bank) = self.bank() else { return };
        if let Some(mut demo) = self.link.demo_state() {
            let numbers = crate::company::demo::checks::numbers(&mut demo, bank);
            let balance = crate::company::demo::checks::balance(&mut demo, bank);
            drop(demo);
            self.numbers_task = Some(Task::ready(numbers));
            self.balance_task = Some(Task::ready(balance));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let next = s
                .checks()
                .settings()
                .await
                .ok()
                .and_then(|c| c.next_for(bank));
            // The highest number on a check: written here (an expense) or
            // paying bills (a bill payment).
            let mut highest: Option<u32> = None;
            for kind in [cl_api::TxnType::Expense, cl_api::TxnType::BillPayment] {
                let mut q = cl_api::RegisterQuery::new(CalendarDate::min(), CalendarDate::max());
                q.txn_type = Some(kind);
                q.sort = cl_api::RegisterSort::Number;
                q.dir = cl_api::SortDir::Desc;
                let top = s
                    .register()
                    .page(bank, &q, 1, 1)
                    .await
                    .ok()
                    .and_then(|page| page.lines.first().and_then(|l| l.check_number))
                    .and_then(|n| u32::try_from(n).ok());
                highest = highest.max(top);
            }
            Ok((next, highest))
        }) {
            self.numbers_task = Some(Task::Pending(p));
        }
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok(s.accounts().get(bank).await?.record.balance)
        }) {
            self.balance_task = Some(Task::Pending(p));
        }
    }

    fn apply_lists(&mut self) {
        let mut lists = RegisterLists::default();
        lists.set_accounts(&self.accounts);
        lists.set_contacts(&self.contacts);
        lists.set_tags(&self.tags);
        self.lists = Arc::new(lists);
        self.lines.set_lists(Arc::clone(&self.lists));
        self.tag_list = tag_list(&self.tags);
        if !self.line_billable() {
            self.lines.set_locked(vec![L_CUSTOMER, L_BILLABLE]);
        }
        self.inputs.rev = u64::MAX;
    }

    /// The bank account to start on: the last one used in this run, else
    /// the first active bank account.
    fn default_bank(&self, ctx: Option<&egui::Context>) -> Option<AccountId> {
        let banks = self.bank_accounts();
        let remembered = ctx.and_then(|c| c.data(|d| d.get_temp::<AccountId>(last_bank_id())));
        remembered
            .filter(|id| banks.iter().any(|a| a.id == *id))
            .or_else(|| banks.first().map(|a| a.id))
    }

    pub(crate) fn poll(&mut self) {
        let mut lists_changed = false;
        if let Some(r) = self.accounts_task.as_mut().and_then(Task::try_take) {
            self.accounts_task = None;
            self.accounts_loaded = true;
            if let Ok(a) = r {
                self.accounts = a;
                lists_changed = true;
                if self.bank().is_none()
                    && let Some(bank) = self.default_bank(None)
                {
                    // Not an edit: the starting account is part of the blank form.
                    let mut f = self.header.current().clone();
                    f.bank = Some(bank);
                    self.header.load(f);
                    self.fetch_bank_facts();
                }
            }
        }
        if let Some(r) = self.contacts_task.as_mut().and_then(Task::try_take) {
            self.contacts_task = None;
            if let Ok(c) = r {
                self.contacts = c;
                lists_changed = true;
            }
        }
        if let Some(r) = self.tags_task.as_mut().and_then(Task::try_take) {
            self.tags_task = None;
            if let Ok(tags) = r {
                self.tags = tags;
                lists_changed = true;
            }
        }
        if lists_changed {
            self.apply_lists();
        }
        if let Some(r) = self.settings_task.as_mut().and_then(Task::try_take) {
            self.settings_task = None;
            if let Ok(s) = r {
                self.closing = s.lock_date;
            }
        }
        if let Some(r) = self.company_task.as_mut().and_then(Task::try_take) {
            self.company_task = None;
            if let Ok(c) = r {
                self.company = c;
            }
        }
        if let Some(r) = self.balance_task.as_mut().and_then(Task::try_take) {
            self.balance_task = None;
            self.balance = r.ok().flatten();
        }
        if let Some(r) = self.numbers_task.as_mut().and_then(Task::try_take) {
            self.numbers_task = None;
            if let Ok((next, highest)) = r {
                self.numbers_arrived(propose_number(next, highest));
            }
        }
        self.poll_save();
        self.poll_prefill();
        self.poll_panel();
        self.poll_attach();
        self.poll_suggest();
    }

    /// Saves in other windows of this app (Q-262). The read-only parts come
    /// again from the engine: the ending balance and the proposed number
    /// (a typed number is kept), and the payee panel (its cards stay until
    /// the new ones arrive). The name, account and tag lists are read
    /// again only while the form has no unsaved edits.
    pub(crate) fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if changes.lists() {
            self.lists_stale = true;
        }
        if self.lists_stale
            && !self.is_dirty()
            && self.accounts_task.is_none()
            && self.contacts_task.is_none()
            && self.tags_task.is_none()
        {
            self.lists_stale = false;
            self.start_load();
        }
        if let Some(bank) = self.bank()
            && changes.ledger_touches(bank)
        {
            self.fetch_bank_facts();
        }
        if changes.ledger_any() || changes.contacts {
            self.refresh_panel();
        }
    }

    /// Tells the other windows this one changed the ledger (Q-262).
    pub(crate) fn publish_ledger(&self) {
        // A check touches its bank account, its lines' accounts and the
        // payee; an edit may also leave an old bank account.
        self.link
            .bus()
            .publish_from(&self.bus, crate::data_bus::Change::Ledger(None));
    }

    /// The proposed next number arrived (Q-242, Q-P13m). A clean new form
    /// takes it as its starting value; a changed one only replaces an
    /// earlier proposal. A duplicate refusal offers the larger of it and
    /// the refused number + 1.
    fn numbers_arrived(&mut self, n: u32) {
        self.proposal = Some(n);
        if let Some((refused, offer)) = self.save.duplicate {
            self.save.duplicate = Some((refused, offer.max(n)));
        }
        if self.editing.is_some() {
            return;
        }
        let current = self.header.current().number.clone();
        let fresh = matches!(&current, NumberField::Proposed(_))
            || matches!(&current, NumberField::Typed(s) if s.is_empty());
        if !fresh {
            return;
        }
        if self.header.is_dirty() {
            self.header
                .edit_step(|h| h.number = NumberField::Proposed(n));
        } else {
            let mut f = self.header.current().clone();
            f.number = NumberField::Proposed(n);
            self.header.load(f);
        }
        self.inputs.rev = u64::MAX;
    }

    /// The header texts from the form (after a load, clear, undo or a
    /// change the screen made).
    pub(crate) fn sync_inputs(&mut self) {
        let f = self.header.current();
        let style = self.money_style;
        self.inputs.number = f.number.text().unwrap_or_default();
        if let Some(d) = f.date {
            self.inputs.date = DateInput::new(d);
        }
        self.inputs.payee = f
            .payee
            .and_then(|p| self.lists.name_text(p))
            .map_or_else(|| f.payee_name.clone(), str::to_string);
        self.inputs.amount = f.amount.map(|m| m.format(style)).unwrap_or_default();
        self.inputs.amount_problem = None;
        self.inputs.payee_problem = false;
        self.inputs.rev = self.header.revision();
    }

    /// The form has unsaved edits.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.header.is_dirty() || self.lines.dirty()
    }

    /// The owner left the lines grid (Q-P13r b): a blank check amount
    /// takes the lines total; an amount from the lines follows them.
    pub(crate) fn left_lines(&mut self) {
        let mut f = self.compose();
        f.leave_lines();
        let (amount, from) = (f.amount, f.amount_from_lines);
        let h = self.header.current();
        if h.amount != amount || h.amount_from_lines != from {
            self.header.edit_step(|h| {
                h.amount = amount;
                h.amount_from_lines = from;
            });
            self.sync_inputs();
        }
    }

    /// The owner typed the check amount (Q-P13r a): one used line with no
    /// amount (or the old amount) follows it.
    pub(crate) fn amount_typed(&mut self, amount: Option<Money>) {
        let mut f = self.compose();
        f.set_amount(amount);
        let lines = f.lines;
        self.header.edit(field::AMOUNT, |h| {
            h.amount = amount;
            h.amount_from_lines = false;
        });
        if &lines != self.lines.value() {
            self.lines.form_mut().edit_step(|v| *v = lines);
        }
        // The field shows the amount taken (a save commits what it shows).
        self.inputs.amount = amount
            .map(|m| m.format(self.money_style))
            .unwrap_or_default();
        self.inputs.rev = self.header.revision();
    }

    /// Picks the bank account.
    pub(crate) fn set_bank(&mut self, ctx: &egui::Context, bank: AccountId) {
        if self.bank() == Some(bank) {
            return;
        }
        ctx.data_mut(|d| d.insert_temp(last_bank_id(), bank));
        self.header.edit(field::BANK, |h| h.bank = Some(bank));
        self.balance = None;
        self.nav = Nav::default();
        self.fetch_bank_facts();
    }

    /// The tab title: "Write Check · #4418", "· To print" or plain.
    fn title_text(&self) -> String {
        let f = self.header.current();
        if f.print_later {
            return t("check.title.to_print");
        }
        match f.number.text().filter(|n| !n.trim().is_empty()) {
            Some(n) => t_with("check.title.number", &[("number", n.trim())]),
            None => t("cmd.vendors.write_check"),
        }
    }
}

fn last_bank_id() -> egui::Id {
    egui::Id::unique("write_check/last_bank")
}

/// `lines_disagree` and the like (control channel; no amounts).
#[must_use]
pub(crate) fn problem_name(p: CheckProblem) -> &'static str {
    match p {
        CheckProblem::BankMissing => "bank_missing",
        CheckProblem::DateMissing => "date_missing",
        CheckProblem::PayeeMissing => "payee_missing",
        CheckProblem::AmountMissing => "amount_missing",
        CheckProblem::AmountNotPositive => "amount_not_positive",
        CheckProblem::LinesDisagree { .. } => "lines_disagree",
        CheckProblem::LineAccountMissing(_) => "line_account_missing",
        CheckProblem::LineAmountNotPositive(_) => "line_amount_not_positive",
        CheckProblem::BillableNeedsCustomer(_) => "billable_needs_customer",
        CheckProblem::NumberInvalid => "number_invalid",
        CheckProblem::NumberWithPrintLater => "number_with_print_later",
        CheckProblem::AddressTooLong => "address_too_long",
        CheckProblem::AmountTooLarge => "amount_too_large",
        CheckProblem::TooManyLines => "too_many_lines",
    }
}

impl Screen for WriteCheckScreen {
    fn title(&self) -> String {
        self.title_text()
    }

    fn ui(&mut self, ui: &mut Ui) {
        let style = crate::grid_env::shell_frame(ui.ctx()).money_style;
        if style != self.money_style {
            self.money_style = style;
            self.inputs.rev = u64::MAX;
        }
        self.poll();
        self.hear_changes();
        if self.bank().is_none()
            && self.accounts_loaded
            && let Some(bank) = self.default_bank(Some(ui.ctx()))
        {
            let mut f = self.header.current().clone();
            f.bank = Some(bank);
            self.header.load(f);
            self.fetch_bank_facts();
        }
        if self.inputs.rev != self.header.revision() {
            self.sync_inputs();
        }
        self.draw(ui);
        // Leaving the grid (Q-P13r b).
        let in_grid = self.lines.has_focus();
        if self.in_grid && !in_grid {
            self.left_lines();
        }
        self.in_grid = in_grid;
        // A refused save's problems follow the owner's fixes.
        self.recheck_problems();
        // Lines left with a memo or amount and no account (WP7.3).
        self.maybe_suggest();
        self.poll_suggest();
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

    fn state(&self) -> Value {
        self.state_json()
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    fn open_template(&mut self, t: crate::forms::template::TemplateOpen) -> bool {
        self.fill_template(&t)
    }
}

impl WriteCheckScreen {
    fn busy(&self) -> bool {
        self.accounts_task.is_some()
            || self.contacts_task.is_some()
            || self.numbers_task.is_some()
            || self.balance_task.is_some()
            || self.save.task.is_some()
            || self.load_task.is_some()
            || self.nav.task.is_some()
            || self.void.task.is_some()
            || self.history.task.is_some()
            || self.prefill.task.is_some()
            || self.panel.busy()
            || self.attach.busy()
            || self.suggest.busy()
    }

    /// The control-channel state: flags, counts and check numbers only
    /// (never an amount, a name, an address or a memo).
    pub(crate) fn state_json(&self) -> Value {
        let f = self.header.current();
        let number = match &f.number {
            NumberField::Proposed(n) => Some(*n),
            NumberField::Typed(s) => s.trim().parse::<u32>().ok(),
            NumberField::ToPrint { .. } => None,
        };
        let number_text = if f.print_later {
            t("check.number.to_print")
        } else {
            f.number.text().unwrap_or_default()
        };
        let form = self.compose();
        let totals = form.totals_status().map(|s| match s {
            cl_model::forms::check::TotalsStatus::Balanced => "balanced",
            cl_model::forms::check::TotalsStatus::Remaining(_) => "remaining",
            cl_model::forms::check::TotalsStatus::Over(_) => "over",
        });
        let mut v = json!({
            "screen": "write_check",
            "company_open": self.tenant.is_some(),
            "loaded": self.accounts_loaded,
            "mode": if self.editing.is_some() { "edit" } else { "new" },
            "read_only": self.editing.as_ref().is_some_and(|e| e.read_only),
            "read_only_note": self.editing.as_ref().filter(|e| e.read_only).map(|e| e.read_only_note),
            "edit_print_status": self.editing.as_ref().map(|e| e.print_status.as_wire().to_string()),
            "locked": self.editing_locked(),
            "bank_id": f.bank.map(|b| b.to_string()),
            "ending_balance_shown": self.balance.is_some(),
            "number": number,
            "number_text": number_text,
            "number_proposed": matches!(f.number, NumberField::Proposed(_)),
            "print_later": f.print_later,
            "date": f.date.map(CalendarDate::to_wire),
            "has_payee": f.payee.is_some(),
            "amount_set": f.amount.is_some(),
            "amount_from_lines": f.amount_from_lines,
            "words": f.amount.and_then(cl_model::forms::amount_words::amount_in_words).is_some(),
            "lines": form.lines.used().count(),
            "totals": totals,
            "dirty": self.is_dirty(),
            "saving": self.save.task.is_some(),
            "saved_count": self.save.count,
            "save": {
                "refused": self.save.problems.first().map(|p| problem_name(*p)),
                "problems": self.save.problems.len(),
                "error": self.save.error.is_some(),
                "duplicate": self.save.duplicate.map(|(_, offer)| offer),
                "stale": self.save.stale,
            },
            "last_saved": self.save.last.as_ref().map(|l| json!({
                "number": l.number,
                "print_status": l.print_status,
            })),
            "last_action": self.save.last_action,
            "notice": self.save.notice,
            "line_billable": self.line_billable(),
            "check_edit": self.check_edit(),
            "void_open": self.void.open,
            "history_open": self.history.open,
            "nav_at": self.nav.at,
            "payee_list": self.payee_list,
            "focused": self.focused.map(|f| format!("{f:?}").to_lowercase()),
            "grid": {
                "focused": self.lines.has_focus(),
            },
            "layout": self.layout.json(),
        });
        let wp7 = json!({
            "prefill": {
                "applied": self.prefill.applied,
                "source": self.prefill.source,
                "dropped": self.prefill.dropped,
                "pending": self.prefill.task.is_some(),
            },
            "panel": self.panel_json(),
            "attachments": {
                "count": self.attach.list.len(),
                "draft": f.draft_attachment.is_some(),
                "uploading": self.attach.upload.is_some(),
                "problem": self.attach.problem,
                "saved_copy": self.attach.saved_to.is_some(),
            },
            "ai": self.suggest_json(),
            "nav_prompt": self.nav_prompt.is_some(),
            "printed_prompt": self.printed.is_some(),
            "printed_choice": self.save.printed_choice.map(cl_api::PrintedCheckChange::as_wire),
            "template": self.template.state_json(),
        });
        if let (Value::Object(m), Value::Object(w)) = (&mut v, wp7) {
            m.extend(w);
        }
        if let (Value::Object(m), Value::Object(c)) = (
            &mut v,
            self.closing_prompt.state_json(self.closing_supported()),
        ) {
            m.extend(c);
        }
        v
    }
}

#[cfg(test)]
mod tests;
