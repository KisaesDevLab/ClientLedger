//! Chart of Accounts (P11 step 1): every account in hierarchy order on the
//! ledger grid, numbers shown by preference (Q-79), the engine's own
//! balance of each account, an add/edit panel, make inactive / active
//! again, and the account's quick report.
//!
//! The engine owns the accounts. Numbers must be unique (Q-209): the panel
//! checks the loaded chart first and shows the engine's 409
//! `ACCOUNT_NUMBER_EXISTS` in the same plain words. A parent's balance is
//! never added up from its sub-accounts (non-negotiable 7; the engine's
//! subtree total is U-32). Merge stays hidden until the engine moves the
//! journal lines (U-31). The sensitive mark is kept on the company record
//! until the engine stores it (U-33); hiding sensitive accounts from other
//! roles arrives with P29. The full bank or card number field is shown
//! switched off until the engine can keep it masked (U-34, Q-154).
//!
//! Keyboard: Ctrl+A opens the window; Ctrl+N new, Enter or Ctrl+E edit,
//! Ctrl+Shift+I make inactive or active again, Ctrl+Shift+Q quick report,
//! Ctrl+Shift+H include inactive, Ctrl+F find; in the panel Alt+N Save &
//! New, Alt+S Save & Close, Esc reverts, then closes the panel.
//!
//! A03: a credit card account gets "Card ends in" (its last four digits,
//! so reading a receipt can pick the card) when the engine stores them
//! (`document-drafts-v1`). They are saved with the account in one request
//! (a new account: right after it is created) and never logged or put in
//! the control-channel state.
//!
//! Every engine call runs on the request queue and is polled each frame.

pub mod model;

use std::collections::BTreeSet;
use std::sync::Arc;

use cl_api::{
    Account, AccountFields, AccountFilter, AccountReport, AccountUpdate, ApiError, CardLast4,
    ConflictReason, EditableFields, IdempotencyKey, OrphanLabel, RecordVersion, RequestGroup,
    Versioned,
};
use cl_core::CalendarDate;
use cl_core::ids::{AccountId, TenantId};
use cl_grid::{CellPos, Grid, GridEvent, GridHeight, GridSnapshot, ModelRef, NoLookups, RowKey};
use cl_model::forms::{AfterSave, FormState, SaveIntent, SaveStart};
use cl_model::lists::accounts::{
    AccountDraft, AccountKind, CardEnding, ChartFilter, DraftField, DraftProblem, check,
    holds_bank_or_card_number, takes_card_digits,
};
use cl_model::{CommandId, Scope, commands};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use self::model::{
    COL_NAME, ChartModel, ReportModel, api_type, chart_spec, detail_text, report_spec,
};
use crate::company::{CompanyLink, Task};
use crate::grid_env::EnvParts;
use crate::i18n::{t, t_with};
use crate::layout::{Body, FixedFrame, LIST_FRAME};
use crate::screens::company::form;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.chart_of_accounts";

/// Form field ids (undo merging).
const F_NAME: u64 = 1;
const F_NUMBER: u64 = 2;
const F_KIND: u64 = 3;
const F_DETAIL: u64 = 4;
const F_DESCRIPTION: u64 = 5;
const F_PARENT: u64 = 6;
const F_SENSITIVE: u64 = 7;
const F_CARD: u64 = 8;

/// Width of the add/edit panel.
const PANEL_WIDTH: f32 = 400.0;
/// Typed letters within this many seconds build one find prefix.
const TYPE_AHEAD_SECS: f64 = 1.0;
/// Room the quick report takes under the chart.
const REPORT_HEIGHT: f32 = 330.0;

fn field_id(f: DraftField) -> egui::Id {
    egui::Id::unique(("coa-form", f as u8))
}

fn search_id() -> egui::Id {
    egui::Id::unique("coa-search")
}

fn shortcut_hint(id: &str) -> String {
    commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

/// The add/edit panel.
struct FormPanel {
    /// The account being changed; `None` for a new one.
    editing: Option<AccountId>,
    form: FormState<AccountDraft>,
    problems: Vec<DraftProblem>,
    error: Option<ApiError>,
    focus: Option<DraftField>,
    task: Option<Task<Versioned<Account>>>,
}

impl FormPanel {
    fn new(editing: Option<AccountId>, draft: AccountDraft) -> FormPanel {
        FormPanel {
            editing,
            form: FormState::new(draft),
            problems: Vec::new(),
            error: None,
            focus: Some(DraftField::Kind),
            task: None,
        }
    }

    /// Takes the engine's refusal of a save.
    fn save_failed(&mut self, e: ApiError) {
        let failure = cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        };
        self.form.save_failed(failure);
        // Q-209: the engine's 409 `ACCOUNT_NUMBER_EXISTS` reads the same as
        // the local check, on the number field.
        let number_taken = matches!(
            &e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists,
                ..
            }
        ) && !self.form.current().number.trim().is_empty();
        if number_taken {
            self.problems = vec![DraftProblem::NumberTaken];
            self.focus = Some(DraftField::Number);
        } else {
            self.error = Some(e);
        }
    }
}

/// The quick report under the chart.
struct ReportPanel {
    account: AccountId,
    title: String,
    all_dates: bool,
    task: Option<Task<AccountReport>>,
    grid: Grid,
    model: ReportModel,
    loaded: bool,
    error: Option<ApiError>,
}

enum Panel {
    None,
    Form(Box<FormPanel>),
    Report(Box<ReportPanel>),
}

impl Panel {
    fn name(&self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Form(f) if f.editing.is_some() => "edit",
            Panel::Form(_) => "new",
            Panel::Report(_) => "report",
        }
    }
}

/// The Chart of Accounts window.
pub struct ChartScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    accounts: Vec<Account>,
    sensitive: BTreeSet<AccountId>,
    numbers: bool,
    filter: ChartFilter,
    model: ChartModel,
    grid: Grid,
    load: Option<Task<Vec<Account>>>,
    loaded: bool,
    load_error: Option<ApiError>,
    toggle: Option<(AccountId, Task<Versioned<Account>>)>,
    toggle_error: Option<ApiError>,
    panel: Panel,
    /// A plain note (i18n key) under the toolbar.
    note: Option<&'static str>,
    /// A plain warning (i18n key) under the toolbar.
    warn: Option<&'static str>,
    parts: EnvParts,
    /// The fixed bars' sizes last frame (Q-265).
    layout: crate::layout::FrameReport,
    snap: GridSnapshot,
    focus_search: bool,
    want_cursor: Option<AccountId>,
    /// Type-ahead prefix and when its last letter arrived (egui time).
    typed: String,
    typed_at: f64,
    /// The chart shows its compact columns (the add/edit panel is open).
    compact: bool,
    /// The account just made inactive (hidden by the filter): turning on
    /// Include inactive puts the cursor back on it.
    just_hidden: Option<AccountId>,
    /// Saves in other windows (Q-262).
    bus: crate::data_bus::Subscription,
    /// The chart being read again after a save elsewhere: the cursor
    /// stays where it was and the keyboard is not moved.
    refreshing: bool,
}

fn new_grid(salt: &str, spec: cl_grid::GridSpec) -> Grid {
    // Both specs are fixed and covered by tests; a failure is a programming
    // error in this file.
    Grid::new(salt, spec).expect("the chart of accounts grid specs are valid")
}

impl ChartScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> ChartScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = ChartScreen {
            link,
            tenant,
            group,
            today,
            accounts: Vec::new(),
            sensitive: BTreeSet::new(),
            numbers: true,
            filter: ChartFilter::default(),
            model: ChartModel::default(),
            grid: new_grid("coa/chart", chart_spec(true, false)),
            load: None,
            loaded: false,
            load_error: None,
            toggle: None,
            toggle_error: None,
            panel: Panel::None,
            note: None,
            warn: None,
            parts: EnvParts::default(),
            layout: crate::layout::FrameReport::default(),
            snap: GridSnapshot::default(),
            focus_search: false,
            want_cursor: None,
            typed: String::new(),
            typed_at: f64::NEG_INFINITY,
            compact: false,
            just_hidden: None,
            bus: crate::data_bus::Subscription::detached(),
            refreshing: false,
        };
        s.bus = s.link.bus().subscribe();
        s.read_prefs();
        s.grid = new_grid("coa/chart", chart_spec(s.numbers, false));
        s.start_load();
        s
    }

    /// The account-number preference and the sensitive marks (Q-79, Q-157).
    fn read_prefs(&mut self) {
        if let Some(demo) = self.link.demo_state() {
            self.sensitive = demo.sensitive.clone();
        }
        let Some((id, _)) = &self.tenant else { return };
        if let Some(rec) = self
            .link
            .store()
            .load()
            .ok()
            .and_then(|l| l.get(*id).cloned())
        {
            self.numbers = rec.prefs.account_numbers;
            if !self.link.is_demo() {
                self.sensitive.clone_from(&rec.prefs.sensitive_accounts);
            }
        }
    }

    fn start_load(&mut self) {
        self.load_error = None;
        if self.link.is_demo() {
            let accounts = self.link.demo_state().map(|mut d| {
                let mut accounts = d
                    .chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone();
                // The demo engine keeps Payments Clearing's balance in its
                // ledger (deposits move money out of it, P14); the chart
                // shows that stored balance, as the engine's list does.
                let clearing = crate::company::demo::deposits::clearing_balance(&mut d);
                for a in &mut accounts {
                    if a.system_tag.as_deref() == Some("payments_clearing") {
                        a.balance = Some(clearing);
                    }
                }
                accounts
            });
            self.load = Some(Task::ready(accounts.unwrap_or_default()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            Ok(p) => self.load = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    fn apply_accounts(&mut self, accounts: Vec<Account>) {
        let keep = self.want_cursor.take().or_else(|| self.selected());
        self.accounts = accounts;
        self.model
            .set_accounts(&self.accounts, &self.sensitive, self.numbers, &self.filter);
        self.loaded = true;
        if std::mem::take(&mut self.refreshing) {
            self.keep_cursor(keep);
        } else {
            self.place_cursor(keep);
        }
    }

    /// Saves in other windows of this app (Q-262): the chart and its
    /// balances come again from the engine; the cursor stays on its
    /// account and an open form keeps its edits.
    fn hear_changes(&mut self) {
        let changes = self.link.bus().take(&mut self.bus);
        if (changes.accounts || changes.ledger_any()) && self.load.is_none() {
            self.refreshing = true;
            self.start_load();
            if self.load.is_none() {
                self.refreshing = false;
            }
        }
    }

    /// Tells the other windows this one changed the chart (an opening
    /// balance is a transaction).
    fn publish(&self, ledger: bool) {
        let bus = self.link.bus();
        bus.publish_from(&self.bus, crate::data_bus::Change::Accounts);
        if ledger {
            bus.publish_from(&self.bus, crate::data_bus::Change::Ledger(None));
        }
    }

    /// Puts the cursor back on `id` without taking the keyboard.
    fn keep_cursor(&mut self, id: Option<AccountId>) {
        let Some(key) = id.and_then(|i| self.model.key_of(i)) else {
            return;
        };
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        self.grid.set_cursor(
            CellPos {
                row: key,
                col: COL_NAME,
            },
            ModelRef::Browse(&mut self.model),
            &env,
        );
    }

    /// Puts the cursor on `id` (or the first row) and focuses the grid.
    fn place_cursor(&mut self, id: Option<AccountId>) {
        let key = id
            .and_then(|i| self.model.key_of(i))
            .or_else(|| cl_grid::RowModel::row_key(&self.model, 0));
        if let Some(key) = key {
            self.focus_row(key);
        }
    }

    /// Swaps to the compact chart while the add/edit panel is open (and
    /// back), keeping the cursor on the same account. The compact chart
    /// leaves the focus with the form; the full one takes it back.
    fn sync_compact(&mut self) {
        let want = matches!(self.panel, Panel::Form(_));
        if want == self.compact {
            return;
        }
        let keep = self.selected();
        self.compact = want;
        let salt = if want {
            "coa/chart-compact"
        } else {
            "coa/chart"
        };
        self.grid = new_grid(salt, chart_spec(self.numbers, want));
        if !want {
            self.place_cursor(keep);
            return;
        }
        let Some(key) = keep.and_then(|id| self.model.key_of(id)) else {
            return;
        };
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        self.grid.set_cursor(
            CellPos {
                row: key,
                col: COL_NAME,
            },
            ModelRef::Browse(&mut self.model),
            &env,
        );
        self.grid.scroll_to(
            key,
            cl_grid::core::viewport::RowAlign::Nearest,
            &ModelRef::Browse(&mut self.model),
        );
    }

    fn focus_row(&mut self, key: RowKey) {
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        self.grid.focus(
            CellPos {
                row: key,
                col: COL_NAME,
            },
            ModelRef::Browse(&mut self.model),
            &env,
        );
        self.grid.scroll_to(
            key,
            cl_grid::core::viewport::RowAlign::Nearest,
            &ModelRef::Browse(&mut self.model),
        );
    }

    /// The account under the cursor.
    #[must_use]
    pub fn selected(&self) -> Option<AccountId> {
        self.grid
            .core()
            .cursor()
            .and_then(|c| self.model.account_at(c.row))
            .map(|e| e.id)
    }

    /// True when the selected account has a register (balance sheet).
    fn selected_has_register(&self) -> bool {
        self.selected()
            .and_then(|id| self.account(id))
            .and_then(crate::screens::register::class_of)
            .is_some()
    }

    /// Ctrl+G or "Open Register": the selected account's register in its
    /// own tab. False (the shell opens the default register) when the
    /// selected account has none.
    pub fn open_register(&mut self) -> bool {
        let Some(id) = self.selected().filter(|_| self.selected_has_register()) else {
            return false;
        };
        self.link
            .push(crate::company::CompanyEvent::OpenRegister(id));
        true
    }

    fn account(&self, id: AccountId) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// The engine stores "Card ends in" (`document-drafts-v1`; in the demo,
    /// while `dev.demo_card_digits` has it on).
    fn card_digits_on(&self) -> bool {
        if self.link.is_demo() {
            return self.link.demo_state().is_some_and(|d| d.card_digits);
        }
        self.link
            .capabilities()
            .has(cl_api::Capabilities::DOCUMENT_DRAFTS)
    }

    fn form_dirty(&self) -> bool {
        matches!(&self.panel, Panel::Form(f) if f.form.is_dirty())
    }

    /// Ctrl+N.
    pub fn open_new(&mut self) {
        if self.form_dirty() {
            self.warn = Some("lists.accounts.finish_first");
            return;
        }
        let kind = self
            .selected()
            .and_then(|id| self.model.chart().entry(id))
            .map_or(AccountKind::Expense, |e| e.kind);
        self.warn = None;
        self.panel = Panel::Form(Box::new(FormPanel::new(None, AccountDraft::new(kind))));
    }

    /// Enter or Ctrl+E on a row.
    pub fn open_edit(&mut self, id: Option<AccountId>) {
        let Some(id) = id else {
            self.warn = Some("lists.accounts.select_first");
            return;
        };
        if self.form_dirty() {
            self.warn = Some("lists.accounts.finish_first");
            return;
        }
        let Some(entry) = self.model.chart().entry(id) else {
            return;
        };
        self.warn = None;
        let mut panel = FormPanel::new(Some(id), AccountDraft::from_entry(entry));
        panel.focus = Some(DraftField::Name);
        self.panel = Panel::Form(Box::new(panel));
    }

    /// Ctrl+Shift+I: make the selected account inactive, or active again.
    pub fn toggle_active(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("lists.accounts.select_first");
            return;
        };
        if self.toggle.is_some() {
            return;
        }
        let Some(account) = self.account(id).cloned() else {
            return;
        };
        self.warn = None;
        self.note = None;
        self.toggle_error = None;
        if account.is_active {
            if account.is_system {
                self.warn = Some("lists.accounts.cant_deactivate_system");
                return;
            }
            // The engine's figure, read, never computed (NN7).
            if account.balance.is_some_and(|b| !b.is_zero()) {
                self.warn = Some("lists.accounts.cant_deactivate_balance");
                return;
            }
        }
        let task = if self.link.is_demo() {
            let mut changed = account.clone();
            changed.is_active = !account.is_active;
            if let Some(mut d) = self.link.demo_state()
                && let Some(chart) = d.chart.as_mut()
                && let Some(a) = chart.iter_mut().find(|a| a.id == id)
            {
                *a = changed.clone();
            }
            Task::ready(Versioned {
                version: RecordVersion::Fingerprint(changed.fingerprint()),
                record: changed,
            })
        } else {
            let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
                return;
            };
            let key = IdempotencyKey::new();
            let label = OrphanLabel {
                what: "orphan.account_save",
                key: Some(key.clone()),
            };
            let pending = if account.is_active {
                group.write(&session, label, move |s| async move {
                    s.accounts().deactivate(id, key).await
                })
            } else {
                let version = RecordVersion::Fingerprint(account.fingerprint());
                let update = AccountUpdate {
                    fields: account.fields.clone(),
                    is_active: true,
                };
                group.write(&session, label, move |s| async move {
                    s.accounts().update(id, &version, &update, key).await
                })
            };
            match pending {
                Ok(p) => Task::Pending(p),
                Err(_) => return,
            }
        };
        self.toggle = Some((id, task));
    }

    /// Ctrl+Shift+H.
    pub fn set_show_inactive(&mut self, on: bool) {
        self.filter.show_inactive = on;
        let hidden = self.just_hidden.take().filter(|_| on);
        let keep = hidden.or_else(|| self.selected());
        self.model.apply_filter(&self.filter);
        self.place_cursor(keep);
    }

    /// Ctrl+Shift+Q: the selected account's quick report.
    pub fn open_report(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("lists.accounts.select_first");
            return;
        };
        if self.form_dirty() {
            self.warn = Some("lists.accounts.finish_first");
            return;
        }
        let title = self.model.chart().display_name(id).unwrap_or_default();
        self.warn = None;
        self.panel = Panel::Report(Box::new(ReportPanel {
            account: id,
            title,
            all_dates: false,
            task: None,
            grid: new_grid("coa/report", report_spec()),
            model: ReportModel::default(),
            loaded: false,
            error: None,
        }));
        self.run_report();
    }

    fn run_report(&mut self) {
        let today = self.today;
        let Panel::Report(r) = &mut self.panel else {
            return;
        };
        let (start, end) = if r.all_dates {
            (None, None)
        } else {
            (Some(today.start_of_year()), Some(today))
        };
        r.loaded = false;
        r.error = None;
        r.model.clear();
        let id = r.account;
        if self.link.is_demo() {
            let Some(account) = self.accounts.iter().find(|a| a.id == id) else {
                return;
            };
            let mut report = crate::company::demo::account_report(account);
            report.data.retain(|l| {
                let day = model::report_day(l.txn_date);
                start.is_none_or(|s| day.is_some_and(|d| d >= s))
                    && end.is_none_or(|e| day.is_some_and(|d| d <= e))
            });
            r.task = Some(Task::ready(report));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            s.reports().account_report(id, start, end).await
        }) {
            r.task = Some(Task::Pending(p));
        }
    }

    /// Alt+N / Alt+S / Enter in a field. Returns true when the save
    /// finished at once (demo) or nothing needed saving.
    pub fn save_form(&mut self, intent: SaveIntent) -> bool {
        let Panel::Form(panel) = &mut self.panel else {
            return false;
        };
        if panel.task.is_some() {
            return false;
        }
        let checked = match check(panel.form.current(), self.model.chart(), panel.editing) {
            Ok(c) => c,
            Err(problems) => {
                panel.focus = problems.first().map(|p| p.field());
                panel.problems = problems;
                return false;
            }
        };
        panel.problems.clear();
        panel.error = None;
        match panel.form.begin_save(intent) {
            SaveStart::Send(_) => {}
            SaveStart::Skip(after) => {
                self.after_save(after, None);
                return true;
            }
            SaveStart::Busy | SaveStart::Unconfirmed => return false,
        }
        let editing = panel.editing;
        let existing = editing.and_then(|id| self.accounts.iter().find(|a| a.id == id).cloned());
        // The chart shows the owner-facing name (Q-202); saving it unchanged
        // keeps the name the engine stores.
        let name = existing.as_ref().map_or_else(
            || checked.name.clone(),
            |a| crate::account_names::stored_name(a, &checked.name),
        );
        let fields = AccountFields {
            name,
            account_number: checked.number.clone(),
            account_type: api_type(checked.kind),
            detail_type: checked.detail.clone(),
            description: checked.description.clone(),
            parent_id: checked.parent,
        };
        let sensitive = checked.sensitive;
        // "Card ends in" (A03): only when the engine stores it and the
        // account is a credit card; `None` clears it.
        let card_on =
            self.card_digits_on() && checked.detail.as_deref().is_some_and(takes_card_digits);
        let digits = checked
            .card_last4
            .as_ref()
            .and_then(|c| CardLast4::parse(c.as_str()));
        if self.link.is_demo() {
            let mut saved = demo_save(&self.link, editing, existing, fields);
            if card_on {
                demo_set_card(&self.link, &mut saved, digits);
            }
            let Panel::Form(panel) = &mut self.panel else {
                return false;
            };
            panel.task = Some(Task::ready(saved));
            return self.poll_save(sensitive);
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.account_save",
            key: Some(key.clone()),
        };
        let pending = match (editing, existing) {
            // The digits changed: one PUT carries the form and the digits,
            // guarded by the version the form was opened on.
            (Some(_), Some(a)) if card_on && a.card_last4 != digits => {
                let current = Versioned {
                    version: RecordVersion::Fingerprint(a.fingerprint()),
                    record: Account { fields, ..a },
                };
                group.write(&session, label, move |s| async move {
                    s.accounts().set_card_last4(&current, digits, key).await
                })
            }
            (Some(id), Some(a)) => {
                let version = RecordVersion::Fingerprint(a.fingerprint());
                let update = AccountUpdate {
                    fields,
                    is_active: a.is_active,
                };
                group.write(&session, label, move |s| async move {
                    s.accounts().update(id, &version, &update, key).await
                })
            }
            // A new account: the engine takes the digits on an edit only,
            // so they follow the create. If that second step fails the
            // account stays saved and the window says the digits were not.
            _ => group.write(&session, label, move |s| async move {
                let created = s.accounts().create(&fields, key).await?;
                if !card_on || digits.is_none() {
                    return Ok(created);
                }
                let with_card = s
                    .accounts()
                    .set_card_last4(&created, digits, IdempotencyKey::new())
                    .await;
                match with_card {
                    Ok(a) => Ok(a),
                    // The digits may have landed: what the engine holds is
                    // read back, so the window says the truth (review nit).
                    Err(ApiError::OutcomeUnknown { .. }) => Ok(s
                        .accounts()
                        .get(created.record.id)
                        .await
                        .unwrap_or_else(|_| {
                            tracing::warn!("card digits unknown after creating an account");
                            created
                        })),
                    Err(_) => {
                        tracing::warn!("card digits not saved after creating an account");
                        Ok(created)
                    }
                }
            }),
        };
        if let (Ok(p), Panel::Form(panel)) = (pending, &mut self.panel) {
            panel.task = Some(Task::Pending(p));
        }
        false
    }

    /// Takes a finished save. Returns true when it saved.
    fn poll_save(&mut self, sensitive: bool) -> bool {
        let card_on = self.card_digits_on();
        let Panel::Form(panel) = &mut self.panel else {
            return false;
        };
        let Some(task) = panel.task.as_mut() else {
            return false;
        };
        let Some(result) = task.try_take() else {
            return false;
        };
        panel.task = None;
        match result {
            Ok(saved) => {
                let account = saved.record;
                // A03: the digits asked for did not stick (a new account's
                // second step failed): say so; the account itself saved.
                let wanted = card_wanted(panel.form.current());
                let card_missed = card_on
                    && takes_card_digits(&panel.form.current().detail)
                    && account.card_last4.as_ref().map(CardLast4::as_str) != wanted.as_deref();
                let after = panel.form.save_ok(panel.form.current().clone());
                self.store_sensitive(account.id, sensitive);
                match self.accounts.iter_mut().find(|a| a.id == account.id) {
                    Some(a) => *a = account.clone(),
                    None => self.accounts.push(account.clone()),
                }
                self.want_cursor = Some(account.id);
                let accounts = std::mem::take(&mut self.accounts);
                self.apply_accounts(accounts);
                self.note = Some("lists.accounts.saved");
                if card_missed {
                    self.warn = Some("lists.accounts.card_not_saved");
                }
                self.publish(true);
                self.after_save(after, Some(account.id));
                true
            }
            Err(e) => {
                panel.save_failed(e);
                false
            }
        }
    }

    fn after_save(&mut self, after: AfterSave, saved: Option<AccountId>) {
        match after {
            AfterSave::Stay => {
                if let Some(id) = saved
                    && let Some(entry) = self.model.chart().entry(id)
                {
                    let mut panel = FormPanel::new(Some(id), AccountDraft::from_entry(entry));
                    panel.focus = None;
                    self.panel = Panel::Form(Box::new(panel));
                }
            }
            AfterSave::NewForm => {
                let kind = match &self.panel {
                    Panel::Form(f) => f.form.current().kind,
                    _ => AccountKind::Expense,
                };
                self.panel = Panel::Form(Box::new(FormPanel::new(None, AccountDraft::new(kind))));
            }
            AfterSave::CloseWindow => {
                // Save & Close closes the panel; the window stays.
                self.panel = Panel::None;
                self.place_cursor(saved.or_else(|| self.selected()));
            }
        }
    }

    fn store_sensitive(&mut self, id: AccountId, on: bool) {
        if self.sensitive.contains(&id) == on {
            return;
        }
        if on {
            self.sensitive.insert(id);
        } else {
            self.sensitive.remove(&id);
        }
        if let Some(mut d) = self.link.demo_state() {
            d.sensitive.clone_from(&self.sensitive);
            return;
        }
        if let Some((tenant, _)) = &self.tenant {
            let set = self.sensitive.clone();
            let tenant = *tenant;
            if self
                .link
                .store()
                .update(|l| {
                    if let Some(rec) = l.get_mut(tenant) {
                        rec.prefs.sensitive_accounts = set;
                    }
                })
                .is_err()
            {
                self.warn = Some("lists.accounts.sensitive_not_saved");
            }
        }
    }

    fn poll(&mut self) {
        if let Some(task) = self.load.as_mut()
            && let Some(result) = task.try_take()
        {
            self.load = None;
            match result {
                Ok(accounts) => self.apply_accounts(accounts),
                Err(e) => {
                    self.loaded = true;
                    self.load_error = Some(e);
                }
            }
        }
        if let Some((id, task)) = self.toggle.as_mut()
            && let Some(result) = task.try_take()
        {
            let id = *id;
            self.toggle = None;
            match result {
                Ok(saved) => {
                    let active = saved.record.is_active;
                    self.just_hidden = (!active && !self.filter.show_inactive).then_some(id);
                    if let Some(a) = self.accounts.iter_mut().find(|a| a.id == id) {
                        *a = saved.record;
                    }
                    self.want_cursor = Some(id);
                    let accounts = std::mem::take(&mut self.accounts);
                    self.apply_accounts(accounts);
                    self.publish(false);
                    self.note = Some(if active {
                        "lists.accounts.made_active"
                    } else {
                        "lists.accounts.made_inactive"
                    });
                }
                Err(ApiError::Validation(_)) => {
                    self.warn = Some("lists.accounts.deactivate_refused");
                }
                Err(e) => self.toggle_error = Some(e),
            }
        }
        if let Panel::Form(panel) = &self.panel
            && panel.task.is_some()
        {
            let sensitive = panel.form.current().sensitive;
            self.poll_save(sensitive);
        }
        if let Panel::Report(r) = &mut self.panel
            && let Some(task) = r.task.as_mut()
            && let Some(result) = task.try_take()
        {
            r.task = None;
            r.loaded = true;
            match result {
                Ok(report) => r.model.set(report),
                Err(e) => r.error = Some(e),
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let selected_active = self
            .selected()
            .and_then(|id| self.model.chart().entry(id))
            .map(|e| e.active);
        ui.horizontal_wrapped(|ui| {
            let b = |ui: &mut Ui, key: &str, cmd: &str| {
                ui.button(t(key))
                    .on_hover_text(format!("{} ({})", t(key), shortcut_hint(cmd)))
            };
            if b(ui, "lists.accounts.new", "list.new").clicked() {
                self.open_new();
            }
            if b(ui, "lists.accounts.edit", "edit.edit_transaction").clicked() {
                self.open_edit(self.selected());
            }
            let toggle_key = if selected_active == Some(false) {
                "lists.accounts.make_active"
            } else {
                "lists.accounts.make_inactive"
            };
            if b(ui, toggle_key, "list.toggle_active").clicked() {
                self.toggle_active();
            }
            if b(ui, "lists.accounts.quick_report", "list.quick_report").clicked() {
                self.open_report();
            }
            if self.selected_has_register()
                && b(ui, "lists.accounts.open_register", "banking.register").clicked()
            {
                self.open_register();
            }
            ui.separator();
            let l = ui.add(egui::Label::new(t("lists.accounts.find")).selectable(false));
            let search = ui
                .add_sized(
                    [220.0, 24.0],
                    egui::TextEdit::singleline(&mut self.filter.search).id(search_id()),
                )
                .labelled_by(l.id)
                .on_hover_text(shortcut_hint("edit.find"));
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if search.changed() {
                // Typing stays in the box; the grid keeps or moves its own
                // cursor when rows go away.
                self.model.apply_filter(&self.filter);
            }
            let mut inactive = self.filter.show_inactive;
            if ui
                .checkbox(&mut inactive, t("lists.accounts.include_inactive"))
                .on_hover_text(shortcut_hint("list.show_inactive"))
                .changed()
            {
                self.set_show_inactive(inactive);
            }
        });
    }

    fn messages(&self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(e) = &self.load_error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(e) = &self.toggle_error {
            form::inline_error(ui, tokens, e);
        }
        if let Some(w) = self.warn {
            ui.label(RichText::new(t(w)).color(tokens.warn_text));
        } else if let Some(n) = self.note {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
    }

    fn chart_ui(&mut self, ui: &mut Ui, reserve: f32) {
        let label = t("lists.accounts.grid_label");
        let env = self.parts.env(
            &NoLookups,
            &label,
            GridHeight::Fill {
                reserve_below: reserve,
                min_rows: 3,
            },
        );
        let out = self.grid.show(ui, ModelRef::Browse(&mut self.model), &env);
        self.snap = self.grid.snapshot(&ModelRef::Browse(&mut self.model));
        for e in out.events {
            match e {
                GridEvent::Activate { key } => {
                    let id = self.model.account_at(key).map(|e| e.id);
                    self.open_edit(id);
                }
                GridEvent::TypedInBrowse(text) => {
                    // Letters typed close together build one prefix ("67"
                    // finds 6700); a pause starts a new one.
                    let now = ui.input(|i| i.time);
                    if now - self.typed_at > TYPE_AHEAD_SECS {
                        self.typed.clear();
                    }
                    self.typed_at = now;
                    self.typed.push_str(&text);
                    let mut key = self.model.find_prefix(&self.typed);
                    if key.is_none() {
                        self.typed.clone_from(&text);
                        key = self.model.find_prefix(&text);
                    }
                    if let Some(key) = key {
                        self.focus_row(key);
                    }
                }
                _ => {}
            }
        }
    }

    fn footer(&self, ui: &mut Ui, tokens: &Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t_with(
                    "lists.accounts.count",
                    &[
                        ("shown", &self.model.shown().to_string()),
                        ("total", &self.model.chart().len().to_string()),
                    ],
                ))
                .color(tokens.text_secondary),
            );
            ui.label(
                RichText::new(t("lists.accounts.own_balance_note")).color(tokens.text_tertiary),
            );
        });
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let numbers = self.numbers;
        let card_on = self.card_digits_on();
        let chart = self.model.chart();
        let Panel::Form(panel) = &mut self.panel else {
            return;
        };
        let mut save: Option<SaveIntent> = None;
        let mut cancel = false;
        let heading = if panel.editing.is_some() {
            t("lists.accounts.form.edit_title")
        } else {
            t("lists.accounts.form.new_title")
        };
        form::heading(ui, tokens, &heading);
        ui.add_space(6.0);
        let saving = panel.task.is_some();
        ui.add_enabled_ui(!saving, |ui| {
            let mut d = panel.form.current().clone();
            let focus = panel.focus.take();
            let want = |f: DraftField| focus == Some(f);
            // Type.
            let kinds: Vec<String> = AccountKind::ALL
                .iter()
                .map(|k| t(k.label().as_str()))
                .collect();
            let cur = AccountKind::ALL
                .iter()
                .position(|k| *k == d.kind)
                .unwrap_or(0);
            if let Some(i) = picker(
                ui,
                DraftField::Kind,
                &t("lists.accounts.form.kind"),
                &kinds,
                cur,
                want(DraftField::Kind),
            ) {
                let kind = AccountKind::ALL[i];
                panel.form.edit(F_KIND, |v| v.set_kind(kind));
                d = panel.form.current().clone();
            }
            // Detail type: the built-ins, plus the account's own value.
            let mut details: Vec<String> = d
                .kind
                .detail_types()
                .iter()
                .map(|s| (*s).to_string())
                .collect();
            if !d.detail.is_empty() && !details.contains(&d.detail) {
                details.push(d.detail.clone());
            }
            let labels: Vec<String> = details.iter().map(|s| detail_text(Some(s))).collect();
            let cur = details.iter().position(|s| *s == d.detail).unwrap_or(0);
            if let Some(i) = picker(
                ui,
                DraftField::Detail,
                &t("lists.accounts.form.detail"),
                &labels,
                cur,
                want(DraftField::Detail),
            ) {
                let v = details[i].clone();
                panel.form.edit(F_DETAIL, |x| x.detail = v);
            }
            // Name and number.
            let mut name = d.name.clone();
            let r = text_row(
                ui,
                DraftField::Name,
                &t("lists.accounts.form.name"),
                &mut name,
                want(DraftField::Name),
            );
            if r.changed() {
                panel.form.edit(F_NAME, |x| x.name = name);
            }
            if enter_left(ui, &r) {
                save = Some(SaveIntent::SaveAndNew);
            }
            if numbers {
                let mut number = d.number.clone();
                let r = text_row(
                    ui,
                    DraftField::Number,
                    &t("lists.accounts.form.number"),
                    &mut number,
                    want(DraftField::Number),
                );
                if r.changed() {
                    panel.form.edit(F_NUMBER, |x| x.number = number);
                }
                if enter_left(ui, &r) {
                    save = Some(SaveIntent::SaveAndNew);
                }
            }
            // Sub-account of.
            let choices = chart.parent_choices(d.kind, panel.editing);
            let mut ids: Vec<Option<AccountId>> = vec![None];
            let mut labels = vec![t("lists.accounts.form.no_parent")];
            for r in &choices {
                if let Some(e) = chart.entry_at(*r) {
                    ids.push(Some(e.id));
                    labels.push(chart.display_name(e.id).unwrap_or_default());
                }
            }
            if let Some(p) = d.parent
                && !ids.contains(&Some(p))
            {
                ids.push(Some(p));
                labels.push(
                    chart
                        .display_name(p)
                        .unwrap_or_else(|| t("lists.accounts.form.parent_gone")),
                );
            }
            let cur = ids.iter().position(|i| *i == d.parent).unwrap_or(0);
            if let Some(i) = picker(
                ui,
                DraftField::Parent,
                &t("lists.accounts.form.parent"),
                &labels,
                cur,
                want(DraftField::Parent),
            ) {
                let p = ids[i];
                panel.form.edit(F_PARENT, |x| x.parent = p);
            }
            let mut description = d.description.clone();
            let r = ui
                .horizontal(|ui| {
                    let l = ui.add(
                        egui::Label::new(t("lists.accounts.form.description")).selectable(false),
                    );
                    ui.add_sized(
                        [ui.available_width().min(260.0), 24.0],
                        egui::TextEdit::singleline(&mut description),
                    )
                    .labelled_by(l.id)
                })
                .inner;
            if r.changed() {
                panel
                    .form
                    .edit(F_DESCRIPTION, |x| x.description = description);
            }
            // Sensitive (Q-157): kept now, enforced with user roles (P29).
            let mut sensitive = d.sensitive;
            if ui
                .checkbox(&mut sensitive, t("lists.accounts.form.sensitive"))
                .changed()
            {
                panel.form.edit(F_SENSITIVE, |x| x.sensitive = sensitive);
            }
            form::hint(ui, tokens, &t("lists.accounts.form.sensitive_hint"));
            // Bank or card number (Q-154): not available until U-34.
            if holds_bank_or_card_number(d.kind, Some(d.detail.as_str())) {
                ui.add_space(4.0);
                let mut none = String::new();
                ui.horizontal(|ui| {
                    let l = ui.add(
                        egui::Label::new(t("lists.accounts.form.bank_number")).selectable(false),
                    );
                    ui.add_enabled(
                        false,
                        egui::TextEdit::singleline(&mut none)
                            .hint_text("\u{2022}\u{2022}\u{2022}\u{2022}")
                            .desired_width(160.0),
                    )
                    .labelled_by(l.id);
                });
                form::hint(ui, tokens, &t("lists.accounts.form.bank_number_later"));
            }
            // Card ends in (A03): the card's last four digits, when the
            // engine stores them.
            if card_on && takes_card_digits(&d.detail) {
                ui.add_space(4.0);
                let mut digits = d.card_last4.0.clone();
                let tip = t("lists.accounts.form.card_last4_tip");
                let r = ui
                    .horizontal(|ui| {
                        let l = ui
                            .add(
                                egui::Label::new(t("lists.accounts.form.card_last4"))
                                    .selectable(false),
                            )
                            .on_hover_text(&tip);
                        ui.add(
                            egui::TextEdit::singleline(&mut digits)
                                .id(field_id(DraftField::Card))
                                .char_limit(4)
                                .desired_width(60.0),
                        )
                        .labelled_by(l.id)
                        .on_hover_text(&tip)
                    })
                    .inner;
                if want(DraftField::Card) {
                    r.request_focus();
                }
                if r.changed() {
                    panel
                        .form
                        .edit(F_CARD, |x| x.card_last4 = CardEnding(digits));
                }
                if enter_left(ui, &r) {
                    save = Some(SaveIntent::SaveAndNew);
                }
            }
        });
        ui.add_space(6.0);
        for p in &panel.problems {
            ui.label(RichText::new(t(p.key().as_str())).color(tokens.warn_text));
        }
        if let Some(e) = &panel.error {
            form::inline_error(ui, tokens, e);
        }
        if saving {
            form::progress_line(ui, tokens, &t("lists.accounts.form.saving"));
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let sc = format!(
                "{} ({})",
                t("cmd.form.save_close"),
                shortcut_hint("form.save_close")
            );
            if crate::widgets::primary_button(ui, &t("lists.accounts.form.save_close"))
                .on_hover_text(sc)
                .clicked()
            {
                save = Some(SaveIntent::SaveAndClose);
            }
            if ui
                .button(t("lists.accounts.form.save_new"))
                .on_hover_text(shortcut_hint("form.save_new"))
                .clicked()
            {
                save = Some(SaveIntent::SaveAndNew);
            }
            if ui.button(t("lists.accounts.form.cancel")).clicked() {
                cancel = true;
            }
        });
        if let Some(intent) = save {
            self.save_form(intent);
        }
        if cancel {
            self.close_panel();
        }
    }

    fn report_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let balance = match &self.panel {
            Panel::Report(r) => self.model.chart().entry(r.account).and_then(|e| e.balance),
            _ => None,
        };
        let style = self.parts.frame.money_style;
        let mut rerun = false;
        let mut close = false;
        let Panel::Report(r) = &mut self.panel else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            form::heading(
                ui,
                tokens,
                &t_with("lists.qr.title", &[("account", &r.title)]),
            );
            ui.separator();
            if ui.radio(!r.all_dates, t("lists.qr.this_year")).clicked() && r.all_dates {
                r.all_dates = false;
                rerun = true;
            }
            if ui.radio(r.all_dates, t("lists.qr.all_dates")).clicked() && !r.all_dates {
                r.all_dates = true;
                rerun = true;
            }
            if ui.button(t("lists.qr.close")).clicked() {
                close = true;
            }
        });
        if let Some(e) = &r.error {
            form::inline_error(ui, tokens, e);
        } else if !r.loaded {
            form::progress_line(ui, tokens, &t("lists.qr.loading"));
        } else if r.model.is_empty() {
            ui.label(RichText::new(t("lists.qr.empty")).color(tokens.text_secondary));
        } else {
            let label = t("lists.qr.grid_label");
            let env = self
                .parts
                .env(&NoLookups, &label, GridHeight::Rows { min: 4, max: 9 });
            r.grid.show(ui, ModelRef::Browse(&mut r.model), &env);
        }
        // The engine's balance of this account; the lines are never summed.
        let text = balance.map_or_else(
            || t("lists.qr.no_balance"),
            |b| t_with("lists.qr.balance", &[("amount", &b.format(style))]),
        );
        ui.label(RichText::new(text).color(tokens.text_secondary));
        if rerun {
            self.run_report();
        }
        if close {
            self.close_panel();
        }
    }

    fn close_panel(&mut self) {
        let keep = self.selected();
        self.panel = Panel::None;
        self.place_cursor(keep);
    }
}

/// Saves into the demo chart at once (no engine).
pub(crate) fn demo_save(
    link: &CompanyLink,
    editing: Option<AccountId>,
    existing: Option<Account>,
    fields: AccountFields,
) -> Versioned<Account> {
    let mut d = link.demo_state();
    let chart = d.as_mut().and_then(|d| d.chart.as_mut());
    let account = if let (Some(_), Some(mut a)) = (editing, existing) {
        a.fields = fields;
        // As the engine: an account that stops being a credit card loses
        // its card digits (A03).
        if !a
            .fields
            .detail_type
            .as_deref()
            .is_some_and(takes_card_digits)
        {
            a.card_last4 = None;
        }
        a
    } else {
        let n = chart.as_ref().map_or(0, |c| c.len());
        Account {
            id: crate::company::demo::account_id(900_000 + u32::try_from(n).unwrap_or(0)),
            fields,
            is_active: true,
            is_system: false,
            system_tag: None,
            balance: Some(cl_core::Money::ZERO),
            created_at: Some(crate::company::demo::DemoState::now()),
            updated_at: Some(crate::company::demo::DemoState::now()),
            card_last4: None,
        }
    };
    if let Some(chart) = chart {
        match chart.iter_mut().find(|a| a.id == account.id) {
            Some(a) => *a = account.clone(),
            None => chart.push(account.clone()),
        }
    }
    Versioned {
        version: RecordVersion::Fingerprint(account.fingerprint()),
        record: account,
    }
}

/// Sets (or clears) a credit card account's digits in the demo chart, as
/// the engine's `PUT` with `cardLast4` does (A03).
fn demo_set_card(link: &CompanyLink, saved: &mut Versioned<Account>, digits: Option<CardLast4>) {
    if !saved
        .record
        .fields
        .detail_type
        .as_deref()
        .is_some_and(takes_card_digits)
    {
        return;
    }
    saved.record.card_last4 = digits;
    saved.version = RecordVersion::Fingerprint(saved.record.fingerprint());
    if let Some(mut d) = link.demo_state()
        && let Some(chart) = d.chart.as_mut()
        && let Some(a) = chart.iter_mut().find(|a| a.id == saved.record.id)
    {
        a.card_last4 = digits;
    }
}

/// The digits a form asks for: four digits, or `None` (empty or not valid,
/// which the checks refuse before any save).
fn card_wanted(d: &AccountDraft) -> Option<String> {
    d.card_last4
        .is_valid()
        .then(|| d.card_last4.as_str().to_string())
}

/// Enter left `r` and means Save & New (Q-190); with "Enter moves between
/// fields" on it moved the focus instead (Q-264).
fn enter_left(_ui: &Ui, r: &egui::Response) -> bool {
    crate::forms::enter::entered(r).saves()
}

/// A labelled single-line field with its own id.
fn text_row(
    ui: &mut Ui,
    f: DraftField,
    label: &str,
    value: &mut String,
    focus: bool,
) -> egui::Response {
    let r = ui
        .horizontal(|ui| {
            let l = ui.add(egui::Label::new(label).selectable(false));
            ui.add_sized(
                [ui.available_width().min(260.0), 24.0],
                egui::TextEdit::singleline(value).id(field_id(f)),
            )
            .labelled_by(l.id)
        })
        .inner;
    if focus {
        r.request_focus();
    }
    r
}

/// A labelled choice that works from the keyboard: Up/Down (or a letter)
/// change it while it has focus; a click opens the list. Returns the new
/// index when it changed.
fn picker(
    ui: &mut Ui,
    field: DraftField,
    label: &str,
    options: &[String],
    current: usize,
    focus: bool,
) -> Option<usize> {
    let mut picked = None;
    ui.horizontal(|ui| {
        let caption = ui.add(egui::Label::new(label).selectable(false));
        let shown = options.get(current).cloned().unwrap_or_default();
        let inner = egui::ComboBox::from_id_salt(field_id(field))
            .selected_text(shown)
            .width(ui.available_width().min(260.0))
            .show_ui(ui, |ui| {
                for (i, o) in options.iter().enumerate() {
                    if ui.selectable_label(i == current, o).clicked() {
                        picked = Some(i);
                    }
                }
            });
        let resp = inner.response.labelled_by(caption.id);
        if focus {
            resp.request_focus();
        }
        if resp.has_focus() {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    resp.id,
                    egui::EventFilter {
                        vertical_arrows: true,
                        ..egui::EventFilter::default()
                    },
                );
            });
            let count = options.len();
            let (down, up, typed) = ui.input(|i| {
                let typed = i.events.iter().find_map(|ev| match ev {
                    egui::Event::Text(s) => s.chars().next(),
                    _ => None,
                });
                (
                    i.key_pressed(egui::Key::ArrowDown),
                    i.key_pressed(egui::Key::ArrowUp),
                    typed,
                )
            });
            if down && current + 1 < count {
                picked = Some(current + 1);
            }
            if up && current > 0 {
                picked = Some(current - 1);
            }
            if let Some(ch) = typed {
                let ch = ch.to_lowercase().to_string();
                let start = current + 1;
                picked = (0..count)
                    .map(|k| (start + k) % count)
                    .find(|i| options[*i].to_lowercase().starts_with(&ch))
                    .or(picked);
            }
        }
    });
    picked.filter(|p| *p != current)
}

impl Screen for ChartScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        self.poll();
        self.hear_changes();
        self.sync_compact();
        let tokens = Tokens::of(ui.ctx());
        // The title, toolbar and messages stay put; the chart keeps its
        // column headers at the top of its own area (Q-265).
        let mut frame = FixedFrame::begin(ui, KIND, LIST_FRAME);
        frame.top(|ui| {
            form::heading(ui, &tokens, &t("cmd.lists.chart_of_accounts"));
            ui.add_space(4.0);
            self.toolbar(ui);
            self.messages(ui, &tokens);
        });
        frame.body(Body::Fill, |ui| {
            if !self.loaded {
                form::progress_line(ui, &tokens, &t("lists.accounts.loading"));
                return;
            }
            match self.panel {
                Panel::Form(_) => {
                    ui.horizontal_top(|ui| {
                        let left = (ui.available_width() - PANEL_WIDTH - 16.0).max(320.0);
                        ui.vertical(|ui| {
                            ui.set_width(left);
                            self.chart_ui(ui, 28.0);
                            self.footer(ui, &tokens);
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.set_width(PANEL_WIDTH);
                            egui::ScrollArea::vertical()
                                .id_salt("coa-form-scroll")
                                .show(ui, |ui| self.form_ui(ui, &tokens));
                        });
                    });
                }
                Panel::Report(_) => {
                    self.chart_ui(ui, REPORT_HEIGHT);
                    self.footer(ui, &tokens);
                    ui.separator();
                    self.report_ui(ui, &tokens);
                }
                Panel::None => {
                    self.chart_ui(ui, 28.0);
                    self.footer(ui, &tokens);
                }
            }
        });
        self.layout = frame.end();
    }

    fn fixed_layout(&self) -> bool {
        true
    }

    fn dirty(&self) -> bool {
        self.form_dirty()
    }

    fn scope(&self) -> Scope {
        // A grid screen: Esc arrives as `form.revert` (Q-P10d).
        Scope::Form
    }

    fn command(&mut self, id: CommandId) -> bool {
        match id.as_str() {
            "list.new" => self.open_new(),
            "edit.edit_transaction" => self.open_edit(self.selected()),
            "list.toggle_active" => self.toggle_active(),
            "list.show_inactive" => self.set_show_inactive(!self.filter.show_inactive),
            "list.quick_report" => self.open_report(),
            crate::screens::register::KIND => return self.open_register(),
            "edit.find" => self.focus_search = true,
            // Demo only (A03): the demo engine starts or stops storing
            // "Card ends in", standing in for `document-drafts-v1`.
            "dev.demo_card_digits" => {
                let Some(mut d) = self.link.demo_state() else {
                    return false;
                };
                d.card_digits = !d.card_digits;
            }
            "form.save_new" | "form.save_close" => {
                if !matches!(self.panel, Panel::Form(_)) {
                    return false;
                }
                let intent = if id.as_str() == "form.save_new" {
                    SaveIntent::SaveAndNew
                } else {
                    SaveIntent::SaveAndClose
                };
                self.save_form(intent);
            }
            "edit.undo" | "edit.redo" => {
                let Panel::Form(p) = &mut self.panel else {
                    return false;
                };
                if id.as_str() == "edit.undo" {
                    p.form.undo();
                } else {
                    p.form.redo();
                }
            }
            "form.revert" => {
                // Esc: revert the form, then close the panel, then clear the
                // search; with nothing left the window closes (the shell).
                match &mut self.panel {
                    Panel::Form(p) if p.form.is_dirty() && p.task.is_none() => {
                        p.form.revert_form();
                        p.problems.clear();
                        p.error = None;
                        // egui drops the field's focus on Esc; keep the
                        // owner in the form.
                        p.focus = Some(if p.editing.is_some() {
                            DraftField::Name
                        } else {
                            DraftField::Kind
                        });
                    }
                    Panel::Form(_) | Panel::Report(_) => self.close_panel(),
                    Panel::None if !self.filter.search.is_empty() => {
                        self.filter.search.clear();
                        let keep = self.selected();
                        self.model.apply_filter(&self.filter);
                        self.place_cursor(keep);
                    }
                    Panel::None => return false,
                }
            }
            other => {
                let cmd = match other {
                    "form.prev_record" => cl_grid::GridCommand::PageUp,
                    "form.next_record" => cl_grid::GridCommand::PageDown,
                    _ => return false,
                };
                let label = String::new();
                let env = self
                    .parts
                    .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
                return self
                    .grid
                    .command(cmd, ModelRef::Browse(&mut self.model), &env)
                    .0
                    .yes();
            }
        }
        true
    }

    fn save(&mut self) -> bool {
        // The close prompt: a demo save finishes at once; an engine save
        // keeps the window open until it is done.
        self.save_form(SaveIntent::Save)
    }

    fn state(&self) -> Value {
        let selected = self.selected().and_then(|id| self.model.chart().entry(id));
        let depth = self
            .selected()
            .and_then(|id| self.model.chart().row_of(id))
            .map(|r| self.model.chart().rows()[r].depth);
        let (form_dirty, problems, error, saving) = match &self.panel {
            Panel::Form(p) => (
                p.form.is_dirty(),
                p.problems
                    .iter()
                    .map(|p| p.key().as_str().rsplit('.').next().unwrap_or_default())
                    .collect::<Vec<_>>(),
                p.error.is_some(),
                p.task.is_some(),
            ),
            _ => (false, Vec::new(), false, false),
        };
        // Whether "Card ends in" shows and whether the selected account has
        // digits; never the digits themselves.
        let card_on = self.card_digits_on();
        let card_field = matches!(&self.panel, Panel::Form(p)
            if card_on && takes_card_digits(&p.form.current().detail));
        let (report_lines, report_loaded) = match &self.panel {
            Panel::Report(r) => (r.model.len(), r.loaded),
            _ => (0, false),
        };
        json!({
            "screen": "chart_of_accounts",
            "layout": self.layout.json(),
            "loaded": self.loaded,
            "load_error": self.load_error.is_some(),
            "rows": self.model.shown(),
            "total": self.model.chart().len(),
            "numbers": self.numbers,
            "show_inactive": self.filter.show_inactive,
            "searching": !self.filter.search.is_empty(),
            "cursor_row": self.snap.cursor_row,
            "focused": self.snap.focused,
            "selected_active": selected.map(|e| e.active),
            "selected_sensitive": selected.map(|e| e.sensitive),
            "selected_card_set": selected.map(|e| !e.card_last4.is_empty()),
            "card_digits_on": card_on,
            "card_field": card_field,
            "selected_depth": depth,
            "panel": self.panel.name(),
            "dirty": form_dirty,
            "problem": problems.first(),
            "problems": problems,
            "error": error,
            "saving": saving,
            "toggling": self.toggle.is_some(),
            "warning": self.warn.map(|w| w.rsplit('.').next().unwrap_or_default()),
            "note": self.note.map(|n| n.rsplit('.').next().unwrap_or_default()),
            "report_lines": report_lines,
            "report_loaded": report_loaded,
            "merge_available": false,
        })
    }
}

#[cfg(test)]
mod tests;
