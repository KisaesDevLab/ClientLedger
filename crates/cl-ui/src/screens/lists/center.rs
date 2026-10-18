//! The Client center ("Clients & Jobs") and the Vendor center (P11 steps
//! 2-3): one window per role over the same contact records. A contact
//! with both roles (Q-77) is listed in both. Each center is a list on the
//! ledger grid beside a detail pane (details, open balance, recent
//! transactions, jobs, notes) or the add/edit panel, plus make inactive /
//! active again and merge.
//!
//! The engine owns the contacts. A contact's open balance is the engine's
//! figure from its client balance or vendor aging report, never added up
//! here (non-negotiable 7; a per-contact field is U-38). Jobs (U-35),
//! several ship-to addresses and contact people (U-36), dated notes and
//! to-dos (U-37) and recording a paper W-9 (U-44) wait for the engine and
//! say so in plain words. Attaching files to a contact is not offered yet.
//! There are no credit limits (Q-131).
//!
//! Keyboard: Ctrl+N new, Enter or Ctrl+E edit, Ctrl+Shift+I make inactive
//! or active again, Ctrl+Shift+H include inactive, Ctrl+Shift+Q the
//! contact's transactions, Ctrl+F find; in the panel Alt+N Save & New,
//! Alt+S Save & Close, Esc reverts, then closes the panel.
//!
//! Every engine call runs on the request queue and is polled each frame.

mod form;
pub mod model;

use std::collections::HashMap;
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, Contact, ContactFilter as ApiContactFilter, ContactUpdate,
    EditableFields, IdempotencyKey, OrphanLabel, Page, PageRequest, RecordVersion, RequestGroup,
    TransactionSummary, Vendor1099Profile, Versioned,
};
use cl_core::CalendarDate;
use cl_core::ids::{ContactId, TenantId};
use cl_grid::{CellPos, Grid, GridEvent, GridHeight, GridSnapshot, ModelRef, NoLookups, RowKey};
use cl_model::lists::contacts::{ContactFilter, Role, Roles, masked_tax_id};
use cl_model::{CommandId, Scope, commands};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use self::form::FormPanel;
use self::model::{
    Balance, COL_NAME, HistoryModel, ListModel, history_spec, list_spec, report_is_zero,
};
use crate::company::{CompanyLink, Task};
use crate::grid_env::EnvParts;
use crate::i18n::{t, t_with};
use crate::screens::company::form as cform;
use crate::screens::lists::detail_pane as pane;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The Client center's command and window kind.
pub const CLIENTS_KIND: &str = "clients.center";
/// The Vendor center's command and window kind.
pub const VENDORS_KIND: &str = "vendors.center";

/// Width of the detail pane and the add/edit panel.
const PANEL_WIDTH: f32 = 500.0;
/// Typed letters within this many seconds build one find prefix.
const TYPE_AHEAD_SECS: f64 = 1.0;
/// Transactions the pane shows at first.
const RECENT: u16 = 10;
/// Transactions the QuickReport view loads.
const ALL: u16 = 500;

/// The detail pane's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Details,
    Transactions,
    Jobs,
    Notes,
}

impl Tab {
    fn of(role: Role) -> &'static [Tab] {
        match role {
            Role::Client => &[Tab::Details, Tab::Transactions, Tab::Jobs, Tab::Notes],
            Role::Vendor => &[Tab::Details, Tab::Transactions, Tab::Notes],
        }
    }

    const fn key(self) -> &'static str {
        match self {
            Tab::Details => "center.tab.details",
            Tab::Transactions => "center.tab.transactions",
            Tab::Jobs => "center.tab.jobs",
            Tab::Notes => "center.tab.notes",
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Tab::Details => "details",
            Tab::Transactions => "transactions",
            Tab::Jobs => "jobs",
            Tab::Notes => "notes",
        }
    }
}

/// What the detail pane loaded for the selected contact.
struct Detail {
    id: ContactId,
    all: bool,
    history: Option<Task<Page<TransactionSummary>>>,
    model: HistoryModel,
    grid: Grid,
    total: Option<u64>,
    history_error: Option<ApiError>,
    profile: Option<Task<Vendor1099Profile>>,
    profile_value: Option<Vendor1099Profile>,
    profile_error: Option<ApiError>,
}

/// The merge confirmation.
struct MergePanel {
    source: ContactId,
    source_name: String,
    /// The source is tracked for 1099s (its payments move with the merge).
    source_1099: bool,
    targets: Vec<(ContactId, String)>,
    choice: usize,
    focus: bool,
    task: Option<Task<Versioned<Contact>>>,
    error: Option<ApiError>,
}

enum Panel {
    None,
    Form(Box<FormPanel>),
    Merge(Box<MergePanel>),
}

impl Panel {
    fn name(&self) -> &'static str {
        match self {
            Panel::None => "none",
            Panel::Form(f) if f.editing.is_some() => "edit",
            Panel::Form(_) => "new",
            Panel::Merge(_) => "merge",
        }
    }
}

/// Balances as the report gave them: contact and display amount.
type Balances = HashMap<ContactId, cl_api::ReportAmount>;

/// A Client or Vendor center window.
pub struct CenterScreen {
    role: Role,
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    contacts: Vec<Contact>,
    accounts: Vec<Account>,
    filter: ContactFilter,
    model: ListModel,
    grid: Grid,
    load: Option<Task<Vec<Contact>>>,
    accounts_load: Option<Task<Vec<Account>>>,
    balances_load: Option<Task<Balances>>,
    loaded: bool,
    load_error: Option<ApiError>,
    balances_error: Option<ApiError>,
    toggle: Option<(ContactId, Task<Versioned<Contact>>)>,
    toggle_error: Option<ApiError>,
    panel: Panel,
    detail: Option<Detail>,
    tab: Tab,
    /// A plain note (i18n key) under the toolbar.
    note: Option<&'static str>,
    /// A plain warning (i18n key) under the toolbar.
    warn: Option<&'static str>,
    parts: EnvParts,
    snap: GridSnapshot,
    focus_search: bool,
    want_cursor: Option<ContactId>,
    typed: String,
    typed_at: f64,
    compact: bool,
    just_hidden: Option<ContactId>,
}

fn new_grid(salt: &str, spec: cl_grid::GridSpec) -> Grid {
    // Both specs are fixed and covered by tests; a failure is a programming
    // error in this file.
    Grid::new(salt, spec).expect("the center grid specs are valid")
}

fn shortcut_hint(id: &str) -> String {
    commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

fn search_id(role: Role) -> egui::Id {
    egui::Id::unique(("center-search", role == Role::Client))
}

const fn salt(role: Role, compact: bool) -> &'static str {
    match (role, compact) {
        (Role::Client, false) => "center/clients",
        (Role::Client, true) => "center/clients-compact",
        (Role::Vendor, false) => "center/vendors",
        (Role::Vendor, true) => "center/vendors-compact",
    }
}

impl CenterScreen {
    /// The center of `role` over the open company.
    #[must_use]
    pub fn new(
        role: Role,
        link: Arc<CompanyLink>,
        _tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> CenterScreen {
        let group = link.group().and_then(Result::ok);
        let mut s = CenterScreen {
            role,
            link,
            group,
            today,
            contacts: Vec::new(),
            accounts: Vec::new(),
            filter: ContactFilter::default(),
            model: ListModel::new(role),
            grid: new_grid(salt(role, false), list_spec(role, false)),
            load: None,
            accounts_load: None,
            balances_load: None,
            loaded: false,
            load_error: None,
            balances_error: None,
            toggle: None,
            toggle_error: None,
            panel: Panel::None,
            detail: None,
            tab: Tab::Details,
            note: None,
            warn: None,
            parts: EnvParts::default(),
            snap: GridSnapshot::default(),
            focus_search: false,
            want_cursor: None,
            typed: String::new(),
            typed_at: f64::NEG_INFINITY,
            compact: false,
            just_hidden: None,
        };
        s.start_load();
        s
    }

    /// The role this center shows.
    #[must_use]
    pub fn role(&self) -> Role {
        self.role
    }

    fn kind(&self) -> &'static str {
        match self.role {
            Role::Client => CLIENTS_KIND,
            Role::Vendor => VENDORS_KIND,
        }
    }

    fn start_load(&mut self) {
        self.load_error = None;
        if self.link.is_demo() {
            let (contacts, accounts) =
                self.link
                    .demo_state()
                    .map_or_else(Default::default, |mut d| {
                        let cs = d.contacts_mut().clone();
                        let chart = d
                            .chart
                            .get_or_insert_with(crate::company::demo::chart)
                            .clone();
                        (cs, chart)
                    });
            self.load = Some(Task::ready(contacts));
            self.accounts_load = Some(Task::ready(accounts));
            self.start_balances();
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            s.contacts().list_all(&ApiContactFilter::default()).await
        }) {
            Ok(p) => self.load = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
        if let Ok(p) = group.read(&session, |s| async move {
            s.accounts().list_all(&AccountFilter::default()).await
        }) {
            self.accounts_load = Some(Task::Pending(p));
        }
        self.start_balances();
    }

    /// Asks for the open balances: the client balance summary or the
    /// vendor aging summary as of today.
    fn start_balances(&mut self) {
        self.balances_error = None;
        let role = self.role;
        if self.link.is_demo() {
            let balances = self.link.demo_state().map(|d| match role {
                Role::Client => crate::company::demo::contacts::customer_balances(&d)
                    .data
                    .iter()
                    .map(|r| (r.id, r.balance.clone()))
                    .collect(),
                Role::Vendor => crate::company::demo::contacts::ap_aging(&d)
                    .vendors
                    .iter()
                    .filter_map(|r| Some((r.contact_id?, r.total.clone())))
                    .collect(),
            });
            self.balances_load = Some(Task::ready(balances.unwrap_or_default()));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let today = self.today;
        let pending = group.read(&session, move |s| async move {
            Ok(match role {
                Role::Client => s
                    .reports()
                    .customer_balance_summary()
                    .await?
                    .data
                    .iter()
                    .map(|r| (r.id, r.balance.clone()))
                    .collect(),
                Role::Vendor => s
                    .reports()
                    .ap_aging_summary(today)
                    .await?
                    .vendors
                    .iter()
                    .filter_map(|r| Some((r.contact_id?, r.total.clone())))
                    .collect(),
            })
        });
        if let Ok(p) = pending {
            self.balances_load = Some(Task::Pending(p));
        }
    }

    fn apply_contacts(&mut self, contacts: Vec<Contact>) {
        let keep = self.want_cursor.take().or_else(|| self.selected());
        self.contacts = contacts;
        self.model.set_contacts(&self.contacts, &self.filter);
        self.loaded = true;
        self.place_cursor(keep);
    }

    /// Puts the cursor on `id` (or the first row) and focuses the grid.
    fn place_cursor(&mut self, id: Option<ContactId>) {
        let key = id
            .and_then(|i| self.model.key_of(i))
            .or_else(|| cl_grid::RowModel::row_key(&self.model, 0));
        if let Some(key) = key {
            self.focus_row(key);
        }
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

    /// Swaps to the compact list while a side panel is open (and back),
    /// keeping the cursor on the same contact.
    fn sync_compact(&mut self) {
        let want = !matches!(self.panel, Panel::None);
        if want == self.compact {
            return;
        }
        let keep = self.selected();
        self.compact = want;
        self.grid = new_grid(salt(self.role, want), list_spec(self.role, want));
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

    /// The contact under the cursor.
    #[must_use]
    pub fn selected(&self) -> Option<ContactId> {
        self.grid
            .core()
            .cursor()
            .and_then(|c| self.model.contact_at(c.row))
            .map(|e| e.id)
    }

    fn contact(&self, id: ContactId) -> Option<&Contact> {
        self.contacts.iter().find(|c| c.id == id)
    }

    fn form_dirty(&self) -> bool {
        matches!(&self.panel, Panel::Form(f) if f.form.is_dirty())
    }

    fn busy_panel(&mut self) -> bool {
        if self.form_dirty() {
            self.warn = Some("center.finish_first");
            return true;
        }
        if matches!(&self.panel, Panel::Merge(m) if m.task.is_some()) {
            return true;
        }
        false
    }

    /// Ctrl+N.
    pub fn open_new(&mut self) {
        if self.busy_panel() {
            return;
        }
        self.warn = None;
        self.panel = Panel::Form(Box::new(FormPanel::new_contact(self.role)));
    }

    /// Enter or Ctrl+E on a row.
    pub fn open_edit(&mut self, id: Option<ContactId>) {
        let Some(id) = id else {
            self.warn = Some("center.select_first");
            return;
        };
        if self.busy_panel() {
            return;
        }
        let Some(c) = self.contact(id).cloned() else {
            return;
        };
        self.warn = None;
        let profile = self
            .detail
            .as_ref()
            .filter(|d| d.id == id)
            .and_then(|d| d.profile_value.clone());
        self.panel = Panel::Form(Box::new(FormPanel::edit(&c, profile.as_ref())));
    }

    /// Ctrl+Shift+I: make the selected contact inactive, or active again.
    pub fn toggle_active(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("center.select_first");
            return;
        };
        if self.toggle.is_some() || self.busy_panel() {
            return;
        }
        let Some(contact) = self.contact(id).cloned() else {
            return;
        };
        self.warn = None;
        self.note = None;
        self.toggle_error = None;
        let task = if self.link.is_demo() {
            let mut changed = contact.clone();
            changed.is_active = !contact.is_active;
            if let Some(mut d) = self.link.demo_state()
                && let Some(c) = d.contacts_mut().iter_mut().find(|c| c.id == id)
            {
                *c = changed.clone();
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
                what: "orphan.contact_save",
                key: Some(key.clone()),
            };
            let pending = if contact.is_active {
                group.write(&session, label, move |s| async move {
                    s.contacts().deactivate(id, key).await
                })
            } else {
                let version = RecordVersion::Fingerprint(contact.fingerprint());
                let update = ContactUpdate {
                    fields: contact.fields.clone(),
                    is_active: true,
                };
                group.write(&session, label, move |s| async move {
                    s.contacts().update(id, &version, &update, key).await
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

    /// Ctrl+Shift+Q: every transaction of the selected contact.
    pub fn open_report(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("center.select_first");
            return;
        };
        if self.busy_panel() {
            return;
        }
        self.warn = None;
        self.panel = Panel::None;
        self.tab = Tab::Transactions;
        let reload = self.detail.as_ref().is_none_or(|d| d.id != id || !d.all);
        if reload {
            self.load_detail(id, true);
        }
    }

    /// Opens the merge confirmation for the selected contact.
    pub fn open_merge(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("center.select_first");
            return;
        };
        if self.busy_panel() {
            return;
        }
        let Some(source) = self.model.list().entry(id).cloned() else {
            return;
        };
        let targets: Vec<(ContactId, String)> = self
            .model
            .list()
            .merge_targets(self.role, id)
            .iter()
            .map(|e| (e.id, e.name.clone()))
            .collect();
        if targets.is_empty() {
            self.warn = Some("center.merge.nothing");
            return;
        }
        self.warn = None;
        let source_1099 = self
            .contacts
            .iter()
            .any(|c| c.id == id && c.fields.is1099_eligible);
        self.panel = Panel::Merge(Box::new(MergePanel {
            source: id,
            source_name: source.name,
            source_1099,
            targets,
            choice: 0,
            focus: true,
            task: None,
            error: None,
        }));
    }

    /// Merges the confirmation's source into its chosen target.
    fn confirm_merge(&mut self) {
        let Panel::Merge(m) = &mut self.panel else {
            return;
        };
        if m.task.is_some() {
            return;
        }
        let Some(&(target, _)) = m.targets.get(m.choice) else {
            return;
        };
        let source = m.source;
        m.error = None;
        if self.link.is_demo() {
            let kept = self.link.demo_state().and_then(|mut d| {
                if let Some(c) = d.contacts_mut().iter_mut().find(|c| c.id == source) {
                    c.is_active = false;
                }
                d.merged.push((source, target));
                d.contacts_mut().iter().find(|c| c.id == target).cloned()
            });
            if let Some(kept) = kept {
                m.task = Some(Task::ready(Versioned {
                    version: RecordVersion::Fingerprint(kept.fingerprint()),
                    record: kept,
                }));
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.contact_merge",
            key: Some(key.clone()),
        };
        if let Ok(p) = group.write(&session, label, move |s| async move {
            s.contacts().merge(source, target, key).await
        }) {
            m.task = Some(Task::Pending(p));
        }
    }

    /// Loads the pane for `id`: its transactions (recent or all) and, for a
    /// vendor, its 1099 profile.
    fn load_detail(&mut self, id: ContactId, all: bool) {
        let limit = if all { ALL } else { RECENT };
        let vendor = self
            .model
            .list()
            .entry(id)
            .is_some_and(|e| e.roles.has(Role::Vendor));
        let mut d = Detail {
            id,
            all,
            history: None,
            model: HistoryModel::default(),
            grid: new_grid("center/history", history_spec()),
            total: None,
            history_error: None,
            profile: None,
            profile_value: None,
            profile_error: None,
        };
        if let Some(old) = self.detail.take().filter(|o| o.id == id) {
            d.profile_value = old.profile_value;
        }
        let want_profile = vendor && self.role == Role::Vendor && d.profile_value.is_none();
        if self.link.is_demo() {
            if let Some(state) = self.link.demo_state() {
                let (rows, total) =
                    crate::company::demo::contacts::history(&state, id, usize::from(limit));
                d.history = Some(Task::ready(Page { data: rows, total }));
                if want_profile {
                    d.profile = Some(Task::ready(crate::company::demo::contacts::profile(
                        &state, id,
                    )));
                }
            }
            self.detail = Some(d);
            return;
        }
        if let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) {
            if let Ok(page) = PageRequest::new(limit, 0)
                && let Ok(p) = group.read(&session, move |s| async move {
                    s.contacts().transactions(id, page).await
                })
            {
                d.history = Some(Task::Pending(p));
            }
            if want_profile
                && let Ok(p) =
                    group.read(
                        &session,
                        move |s| async move { s.form1099().profile(id).await },
                    )
            {
                d.profile = Some(Task::Pending(p));
            }
        }
        self.detail = Some(d);
    }

    /// Keeps the pane on the selected contact.
    fn follow_selection(&mut self) {
        let sel = self.selected();
        match (sel, self.detail.as_ref().map(|d| d.id)) {
            (Some(id), Some(cur)) if id == cur => {}
            (Some(id), _) => self.load_detail(id, false),
            (None, _) => self.detail = None,
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one poll per running task, each short"
    )]
    fn poll(&mut self) {
        if let Some(task) = self.load.as_mut()
            && let Some(result) = task.try_take()
        {
            self.load = None;
            match result {
                Ok(contacts) => self.apply_contacts(contacts),
                Err(e) => {
                    self.loaded = true;
                    self.load_error = Some(e);
                }
            }
        }
        if let Some(task) = self.accounts_load.as_mut()
            && let Some(result) = task.try_take()
        {
            self.accounts_load = None;
            // Without the chart the form simply offers no expense accounts.
            if let Ok(a) = result {
                self.accounts = a;
            }
        }
        if let Some(task) = self.balances_load.as_mut()
            && let Some(result) = task.try_take()
        {
            self.balances_load = None;
            match result {
                Ok(b) => self.model.set_balances(b),
                Err(e) => self.balances_error = Some(e),
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
                    if let Some(c) = self.contacts.iter_mut().find(|c| c.id == id) {
                        *c = saved.record;
                    }
                    self.want_cursor = Some(id);
                    let contacts = std::mem::take(&mut self.contacts);
                    self.apply_contacts(contacts);
                    self.start_balances();
                    self.note = Some(if active {
                        "center.made_active"
                    } else {
                        "center.made_inactive"
                    });
                }
                Err(e) => self.toggle_error = Some(e),
            }
        }
        if let Panel::Form(_) = &self.panel {
            self.poll_save();
        }
        if let Panel::Merge(m) = &mut self.panel
            && let Some(task) = m.task.as_mut()
            && let Some(result) = task.try_take()
        {
            m.task = None;
            match result {
                Ok(kept) => {
                    self.panel = Panel::None;
                    self.detail = None;
                    self.want_cursor = Some(kept.record.id);
                    self.note = Some("center.merge.done");
                    self.start_load();
                }
                Err(e) => m.error = Some(e),
            }
        }
        if let Some(d) = self.detail.as_mut() {
            if let Some(task) = d.history.as_mut()
                && let Some(result) = task.try_take()
            {
                d.history = None;
                match result {
                    Ok(page) => {
                        d.total = Some(page.total);
                        d.model.set(page.data);
                    }
                    Err(e) => d.history_error = Some(e),
                }
            }
            if let Some(task) = d.profile.as_mut()
                && let Some(result) = task.try_take()
            {
                d.profile = None;
                match result {
                    Ok(p) => d.profile_value = Some(p),
                    Err(e) => d.profile_error = Some(e),
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let selected_active = self
            .selected()
            .and_then(|id| self.model.list().entry(id))
            .map(|e| e.active);
        let new_key = match self.role {
            Role::Client => "center.new_client",
            Role::Vendor => "center.new_vendor",
        };
        ui.horizontal_wrapped(|ui| {
            let b = |ui: &mut Ui, key: &str, cmd: &str| {
                let hint = shortcut_hint(cmd);
                let tip = if hint.is_empty() {
                    t(key)
                } else {
                    format!("{} ({hint})", t(key))
                };
                ui.button(t(key)).on_hover_text(tip)
            };
            if b(ui, new_key, "list.new").clicked() {
                self.open_new();
            }
            if b(ui, "center.edit", "edit.edit_transaction").clicked() {
                self.open_edit(self.selected());
            }
            let toggle_key = if selected_active == Some(false) {
                "center.make_active"
            } else {
                "center.make_inactive"
            };
            if b(ui, toggle_key, "list.toggle_active").clicked() {
                self.toggle_active();
            }
            if b(ui, "center.quick_report", "list.quick_report").clicked() {
                self.open_report();
            }
            if b(ui, "center.merge", "list.merge").clicked() {
                self.open_merge();
            }
            ui.separator();
            let find_key = match self.role {
                Role::Client => "center.find_client",
                Role::Vendor => "center.find_vendor",
            };
            let l = ui.add(egui::Label::new(t(find_key)).selectable(false));
            let search = ui
                .add_sized(
                    [200.0, 24.0],
                    egui::TextEdit::singleline(&mut self.filter.search).id(search_id(self.role)),
                )
                .labelled_by(l.id)
                .on_hover_text(shortcut_hint("edit.find"));
            if self.focus_search {
                search.request_focus();
                self.focus_search = false;
            }
            if search.changed() {
                self.model.apply_filter(&self.filter);
            }
            let mut inactive = self.filter.show_inactive;
            if ui
                .checkbox(&mut inactive, t("center.include_inactive"))
                .on_hover_text(shortcut_hint("list.show_inactive"))
                .changed()
            {
                self.set_show_inactive(inactive);
            }
        });
    }

    fn messages(&self, ui: &mut Ui, tokens: &Tokens) {
        for e in [&self.load_error, &self.toggle_error].into_iter().flatten() {
            cform::inline_error(ui, tokens, e);
        }
        if let Some(w) = self.warn {
            ui.label(RichText::new(t(w)).color(tokens.warn_text));
        } else if let Some(n) = self.note {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
    }

    fn list_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        self.model.set_style(self.parts.frame.money_style);
        let label = t(match self.role {
            Role::Client => "center.grid_label_clients",
            Role::Vendor => "center.grid_label_vendors",
        });
        let env = self.parts.env(
            &NoLookups,
            &label,
            GridHeight::Fill {
                reserve_below: 28.0,
                min_rows: 6,
            },
        );
        let out = self.grid.show(ui, ModelRef::Browse(&mut self.model), &env);
        self.snap = self.grid.snapshot(&ModelRef::Browse(&mut self.model));
        for e in out.events {
            match e {
                GridEvent::Activate { key } => {
                    let id = self.model.contact_at(key).map(|e| e.id);
                    self.open_edit(id);
                }
                GridEvent::TypedInBrowse(text) => {
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
        let count = t_with(
            "center.count",
            &[
                ("shown", &self.model.shown().to_string()),
                ("total", &self.model.list().in_role(self.role).to_string()),
            ],
        );
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(count).color(tokens.text_secondary));
            if self.balances_error.is_some() {
                ui.label(RichText::new(t("center.balance.unavailable")).color(tokens.warn_text));
            }
        });
    }

    fn detail_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let Some(id) = self.detail.as_ref().map(|d| d.id) else {
            pane::pending(ui, tokens, &t("center.pane.none"));
            return;
        };
        let Some(c) = self.contact(id).cloned() else {
            return;
        };
        cform::heading(ui, tokens, &c.fields.display_name);
        let roles = model::roles_of(c.fields.contact_type);
        let mut sub = t(roles.label().as_str());
        if !c.is_active {
            sub.push_str(" \u{b7} ");
            sub.push_str(&t("center.status.inactive"));
        }
        ui.label(RichText::new(sub).color(tokens.text_secondary));
        let tabs = Tab::of(self.role);
        let labels: Vec<String> = tabs.iter().map(|t_| t(t_.key())).collect();
        let cur = tabs.iter().position(|x| *x == self.tab).unwrap_or(0);
        if let Some(i) = pane::tabs(ui, &labels, cur) {
            self.tab = tabs[i];
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("center-pane-scroll")
            .show(ui, |ui| match self.tab {
                Tab::Details => self.details_tab(ui, tokens, &c),
                Tab::Transactions => self.transactions_tab(ui, tokens),
                Tab::Jobs => {
                    pane::section(ui, tokens, &t("center.tab.jobs"));
                    pane::pending(ui, tokens, &t("center.jobs.later"));
                }
                Tab::Notes => {
                    pane::section(ui, tokens, &t("center.notes.title"));
                    let notes = c.fields.notes.clone().unwrap_or_default();
                    if notes.trim().is_empty() {
                        pane::pending(ui, tokens, &t("center.notes.empty"));
                    } else {
                        ui.add(egui::Label::new(notes).wrap());
                    }
                    pane::section(ui, tokens, &t("center.todos.title"));
                    pane::pending(ui, tokens, &t("center.todos.later"));
                    pane::section(ui, tokens, &t("center.attachments.title"));
                    pane::pending(ui, tokens, &t("center.attachments.later"));
                }
            });
    }

    fn details_tab(&self, ui: &mut Ui, tokens: &Tokens, c: &Contact) {
        let f = &c.fields;
        // The engine's open balance (report figure, never summed here).
        let (title, source) = match self.role {
            Role::Client => ("center.balance.client", "center.balance.client_source"),
            Role::Vendor => ("center.balance.vendor", "center.balance.vendor_source"),
        };
        pane::section(ui, tokens, &t(title));
        let shown = if self.role == Role::Client && !c.is_active {
            t("center.balance.inactive")
        } else if let Some(text) = self.model.balance_text(c.id) {
            text
        } else if matches!(self.model.balance(c.id), Balance::NotInReport) {
            t("center.balance.none")
        } else if self.balances_error.is_some() {
            t("center.balance.unavailable")
        } else {
            t("center.balance.loading")
        };
        ui.label(RichText::new(shown).monospace().size(18.0));
        pane::pending(ui, tokens, &t(source));
        if self.role == Role::Vendor {
            self.vendor_details(ui, tokens, c);
        }

        pane::section(ui, tokens, &t("center.details.contact"));
        let person = [f.first_name.as_deref(), f.last_name.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ");
        pane::row(
            ui,
            tokens,
            &t("center.field.company"),
            f.company_name.as_deref().unwrap_or_default(),
        );
        pane::row(ui, tokens, &t("center.field.person"), &person);
        pane::row(
            ui,
            tokens,
            &t("center.field.email"),
            f.email.as_deref().unwrap_or_default(),
        );
        pane::row(
            ui,
            tokens,
            &t("center.field.phone"),
            f.phone.as_deref().unwrap_or_default(),
        );
        let terms =
            cl_model::lists::contacts::TermsChoice::from_stored(f.default_payment_terms.as_deref());
        pane::row(
            ui,
            tokens,
            &t("center.field.terms"),
            &form::terms_text(&terms),
        );

        pane::section(ui, tokens, &t("center.details.addresses"));
        let bill = address_text(
            [&f.billing_line1, &f.billing_line2],
            [&f.billing_city, &f.billing_state, &f.billing_zip],
        );
        let ship = address_text(
            [&f.shipping_line1, &f.shipping_line2],
            [&f.shipping_city, &f.shipping_state, &f.shipping_zip],
        );
        pane::row(ui, tokens, &t("center.field.bill_to"), &bill);
        pane::row(ui, tokens, &t("center.field.ship_to"), &ship);
        pane::pending(ui, tokens, &t("center.ship_to.later"));
        pane::section(ui, tokens, &t("center.details.people"));
        pane::pending(ui, tokens, &t("center.people.later"));
    }

    /// A vendor's own details and 1099 tracking, shown right after what
    /// you owe so they stay in view without scrolling.
    fn vendor_details(&self, ui: &mut Ui, tokens: &Tokens, c: &Contact) {
        let f = &c.fields;
        pane::section(ui, tokens, &t("center.details.vendor"));
        let account = f
            .default_expense_account_id
            .map(|a| form::account_label(&self.accounts, a))
            .unwrap_or_default();
        pane::row(ui, tokens, &t("center.field.expense_account"), &account);
        pane::row(
            ui,
            tokens,
            &t("center.field.vendor_account"),
            f.vendor_account_number.as_deref().unwrap_or_default(),
        );
        pane::section(ui, tokens, &t("center.details.1099"));
        let yes_no = |b: bool| t(if b { "center.yes" } else { "center.no" });
        pane::row(
            ui,
            tokens,
            &t("center.field.track_1099"),
            &yes_no(f.is1099_eligible),
        );
        let tin = self
            .detail
            .as_ref()
            .and_then(|d| d.profile_value.as_ref())
            .and_then(|p| p.tin_type);
        let tax = f
            .tax_id
            .as_deref()
            .and_then(|s| masked_tax_id(s, form::kind_of(tin)))
            .unwrap_or_default();
        pane::row(ui, tokens, &t("center.field.tax_id"), &tax);
        let d = self.detail.as_ref();
        match (
            d.and_then(|d| d.profile_value.as_ref()),
            d.and_then(|d| d.profile_error.as_ref()),
        ) {
            (Some(p), _) => {
                let w9 = if p.w9_on_file {
                    t("center.yes")
                } else {
                    t("center.no")
                };
                pane::row(ui, tokens, &t("center.field.w9"), &w9);
                let kind = match p.tin_type {
                    Some(cl_api::TinType::Ssn) => t("center.tin.ssn"),
                    Some(cl_api::TinType::Ein) => t("center.tin.ein"),
                    None => String::new(),
                };
                pane::row(ui, tokens, &t("center.field.tin_type"), &kind);
                pane::row(
                    ui,
                    tokens,
                    &t("center.field.backup"),
                    &yes_no(p.backup_withholding),
                );
            }
            (None, Some(e)) => cform::inline_error(ui, tokens, e),
            (None, None) => pane::pending(ui, tokens, &t("center.profile.loading")),
        }
    }

    fn transactions_tab(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let mut show_all = false;
        let Some(d) = self.detail.as_mut() else {
            return;
        };
        let title = if d.all {
            t("center.history.all_title")
        } else {
            t("center.history.recent_title")
        };
        pane::section(ui, tokens, &title);
        if let Some(e) = &d.history_error {
            cform::inline_error(ui, tokens, e);
            return;
        }
        let Some(total) = d.total else {
            cform::progress_line(ui, tokens, &t("center.history.loading"));
            return;
        };
        if d.model.is_empty() {
            pane::pending(ui, tokens, &t("center.history.empty"));
            return;
        }
        let label = t("center.history.grid_label");
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 3, max: 14 });
        d.grid.show(ui, ModelRef::Browse(&mut d.model), &env);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(t_with(
                    "center.history.count",
                    &[
                        ("shown", &d.model.len().to_string()),
                        ("total", &total.to_string()),
                    ],
                ))
                .color(tokens.text_secondary),
            );
            let more = u64::try_from(d.model.len()).unwrap_or(u64::MAX) < total;
            if !d.all
                && more
                && ui
                    .button(t("center.history.show_all"))
                    .on_hover_text(shortcut_hint("list.quick_report"))
                    .clicked()
            {
                show_all = true;
            }
        });
        if show_all {
            let id = d.id;
            self.load_detail(id, true);
        }
    }

    fn merge_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let role_word = t(match self.role {
            Role::Client => "center.merge.client",
            Role::Vendor => "center.merge.vendor",
        });
        let mut confirm = false;
        let mut cancel = false;
        let Panel::Merge(m) = &mut self.panel else {
            return;
        };
        cform::heading(
            ui,
            tokens,
            &t_with("center.merge.title", &[("name", &m.source_name)]),
        );
        ui.add_space(6.0);
        let busy = m.task.is_some();
        ui.add_enabled_ui(!busy, |ui| {
            let names: Vec<String> = m.targets.iter().map(|(_, n)| n.clone()).collect();
            let focus = std::mem::take(&mut m.focus);
            if let Some(i) = pane::picker(
                ui,
                egui::Id::unique("center-merge-target"),
                &t_with("center.merge.keep", &[("role", &role_word)]),
                &names,
                m.choice,
                focus,
            ) {
                m.choice = i;
            }
            let target = names.get(m.choice).cloned().unwrap_or_default();
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(
                    RichText::new(t_with(
                        "center.merge.explain",
                        &[("source", &m.source_name), ("target", &target)],
                    ))
                    .color(tokens.warn_text),
                )
                .wrap(),
            );
            if m.source_1099 {
                ui.add(
                    egui::Label::new(
                        RichText::new(t_with(
                            "center.merge.explain_1099",
                            &[("source", &m.source_name), ("target", &target)],
                        ))
                        .color(tokens.warn_text),
                    )
                    .wrap(),
                );
            }
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                if crate::widgets::primary_button(ui, &t("center.merge.confirm")).clicked() {
                    confirm = true;
                }
                if ui.button(t("center.merge.cancel")).clicked() {
                    cancel = true;
                }
            });
        });
        if let Some(e) = &m.error {
            cform::inline_error(ui, tokens, e);
        }
        if busy {
            cform::progress_line(ui, tokens, &t("center.merge.working"));
        }
        if confirm {
            self.confirm_merge();
        }
        if cancel {
            self.close_panel();
        }
    }

    fn close_panel(&mut self) {
        let keep = self.selected();
        self.panel = Panel::None;
        self.place_cursor(keep);
    }
}

fn address_text(lines: [&Option<String>; 2], place: [&Option<String>; 3]) -> String {
    let mut parts: Vec<String> = lines.iter().filter_map(|l| (*l).clone()).collect();
    let [city, state, zip] = place;
    let mut last = city.clone().unwrap_or_default();
    if let Some(s) = state.as_deref().filter(|s| !s.is_empty()) {
        if !last.is_empty() {
            last.push_str(", ");
        }
        last.push_str(s);
    }
    if let Some(z) = zip.as_deref().filter(|z| !z.is_empty()) {
        if !last.is_empty() {
            last.push(' ');
        }
        last.push_str(z);
    }
    if !last.is_empty() {
        parts.push(last);
    }
    parts.join("\n")
}

impl Screen for CenterScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{}", self.kind()))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        self.poll();
        self.sync_compact();
        let tokens = Tokens::of(ui.ctx());
        cform::heading(ui, &tokens, &self.title());
        ui.add_space(4.0);
        self.toolbar(ui);
        self.messages(ui, &tokens);
        ui.add_space(4.0);
        if !self.loaded {
            cform::progress_line(ui, &tokens, &t("center.loading"));
            return;
        }
        self.follow_selection();
        ui.horizontal_top(|ui| {
            let left = (ui.available_width() - PANEL_WIDTH - 16.0).max(320.0);
            ui.vertical(|ui| {
                ui.set_width(left);
                self.list_ui(ui, &tokens);
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.set_width(PANEL_WIDTH);
                match self.panel {
                    Panel::Form(_) => {
                        egui::ScrollArea::vertical()
                            .id_salt("center-form-scroll")
                            .show(ui, |ui| self.form_ui(ui, &tokens));
                    }
                    Panel::Merge(_) => self.merge_ui(ui, &tokens),
                    Panel::None => self.detail_ui(ui, &tokens),
                }
            });
        });
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
            "list.merge" => self.open_merge(),
            "edit.find" => self.focus_search = true,
            "form.save_new" | "form.save_close" => {
                if !matches!(self.panel, Panel::Form(_)) {
                    return false;
                }
                let intent = if id.as_str() == "form.save_new" {
                    cl_model::forms::SaveIntent::SaveAndNew
                } else {
                    cl_model::forms::SaveIntent::SaveAndClose
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
            "form.revert" => match &mut self.panel {
                Panel::Form(p) if p.form.is_dirty() && p.task.is_none() => p.revert(),
                Panel::Merge(m) if m.task.is_some() => {}
                Panel::Form(_) | Panel::Merge(_) => self.close_panel(),
                Panel::None if self.tab != Tab::Details => self.tab = Tab::Details,
                Panel::None if !self.filter.search.is_empty() => {
                    self.filter.search.clear();
                    let keep = self.selected();
                    self.model.apply_filter(&self.filter);
                    self.place_cursor(keep);
                }
                Panel::None => return false,
            },
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
        self.save_form(cl_model::forms::SaveIntent::Save)
    }

    fn state(&self) -> Value {
        let selected = self.selected().and_then(|id| self.model.list().entry(id));
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
        let d = self.detail.as_ref();
        json!({
            "screen": match self.role {
                Role::Client => "client_center",
                Role::Vendor => "vendor_center",
            },
            "loaded": self.loaded,
            "load_error": self.load_error.is_some(),
            "rows": self.model.shown(),
            "total": self.model.list().in_role(self.role),
            "show_inactive": self.filter.show_inactive,
            "searching": !self.filter.search.is_empty(),
            "cursor_row": self.snap.cursor_row,
            "focused": self.snap.focused,
            "selected_active": selected.map(|e| e.active),
            "selected_both": selected.map(|e| e.roles == Roles::Both),
            "panel": self.panel.name(),
            "tab": self.tab.name(),
            "dirty": form_dirty,
            "problem": problems.first(),
            "problems": problems,
            "error": error,
            "saving": saving,
            "toggling": self.toggle.is_some(),
            "warning": self.warn.map(|w| w.rsplit('.').next().unwrap_or_default()),
            "note": self.note.map(|n| n.rsplit('.').next().unwrap_or_default()),
            "balances_loaded": self.model.balances_loaded(),
            "selected_has_balance": selected.and_then(|e| match self.model.balance(e.id) {
                Balance::Loading => None,
                Balance::NotInReport => Some(false),
                Balance::Report(a) => Some(!report_is_zero(a)),
            }),
            "detail_loaded": d.is_some_and(|d| d.total.is_some()),
            "history_rows": d.map_or(0, |d| d.model.len()),
            "history_total": d.and_then(|d| d.total),
            "history_all": d.is_some_and(|d| d.all),
            "profile_loaded": d.is_some_and(|d| d.profile_value.is_some()),
            "w9_on_file": d.and_then(|d| d.profile_value.as_ref()).map(|p| p.w9_on_file),
            "merge_available": true,
            "merging": matches!(&self.panel, Panel::Merge(m) if m.task.is_some()),
            "merge_source_1099": matches!(&self.panel, Panel::Merge(m) if m.source_1099),
            "jobs_available": false,
            "attachments_available": false,
        })
    }
}

#[cfg(test)]
mod tests;
