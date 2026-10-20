//! The account register (P12, `banking.register`): one tab per account,
//! the engine's register on the ledger grid, newest at the bottom.
//!
//! The engine is the ledger. Every row, amount and balance is what the
//! engine sent for the shown dates (`GET /accounts/{id}/register`, paged
//! 200 lines at a time through [`cl_model::register::WindowCache`]); the
//! screen never adds anything up (non-negotiable 7, Q-187). On an engine
//! without `register-exact-v1` the amounts are display-only text and a
//! small note says so; edits (WP6) always start from the transaction
//! itself, never from register figures.
//!
//! Keyboard: Ctrl+G opens the register (the selected account in the Chart
//! of Accounts, else the last one used, else the first bank account);
//! Page Up/Down move a page; arrows move between rows. WP6-WP8 add the
//! entry row, editing, Go To, filters and the closed-period flow; their
//! commands are routed to the handlers at the end of this file.
//!
//! Every engine call runs on the request queue and is polled each frame.

mod dialogs;
mod edit;
pub mod entry_row;
pub mod filters;
pub mod footer;
pub mod goto;
pub mod lists;
mod quick_report;
mod record;
pub(crate) use record::keeps_key;
mod splits;
pub mod view;

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, ApiError, CompanySettings, EntryType, LedgerStamp, RegisterPage,
    RegisterQuery, RegisterSort, RequestGroup, SortDir,
};
use cl_core::CalendarDate;
use cl_core::fiscal::FiscalYearSpec;
use cl_core::ids::{AccountId, TenantId};
use cl_grid::core::viewport::RowAlign;
use cl_grid::{CellPos, Grid, GridEvent, GridHeight, GridSnapshot, ModelRef, NoLookups};
use cl_model::register::{
    AccountClass, BalanceMode, Filters, PAGE, PageRequest, RegisterPrefs, SortChoice,
};
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use self::view::{COL_DATE, Mapping, RegisterModel};
use crate::company::{CompanyEvent, CompanyLink, Task};
use crate::grid_env::EnvParts;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::lists::accounts::model::kind_of;
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "banking.register";

/// The register window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn crate::windows::Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(RegisterScreen::new(Arc::clone(link), open, today, None)) as Box<_>)
}

/// The register class of an account (`None` for income and expense
/// accounts, which have no register).
#[must_use]
pub fn class_of(a: &Account) -> Option<AccountClass> {
    let c = AccountClass::of(
        kind_of(a.fields.account_type),
        a.fields.detail_type.as_deref(),
    );
    c.has_register().then_some(c)
}

/// "1010 · Operating Checking", or the name alone.
#[must_use]
pub fn account_text(a: &Account) -> String {
    match a.fields.account_number.as_deref().filter(|n| !n.is_empty()) {
        Some(n) => format!("{n} \u{b7} {}", a.fields.name),
        None => a.fields.name.clone(),
    }
}

/// The engine sort and direction for a "Sort by" choice.
#[must_use]
pub fn sort_of(c: SortChoice) -> (RegisterSort, SortDir) {
    let sort = match c {
        SortChoice::DateOldest | SortChoice::DateNewest => RegisterSort::Date,
        SortChoice::Number => RegisterSort::Number,
        SortChoice::Amount => RegisterSort::Amount,
        SortChoice::Type => RegisterSort::Type,
    };
    let dir = if c.descending() {
        SortDir::Desc
    } else {
        SortDir::Asc
    };
    (sort, dir)
}

/// Engine page size as the API takes it.
fn per_page() -> u16 {
    u16::try_from(PAGE).unwrap_or(cl_api::REGISTER_MAX_PER_PAGE)
}

fn new_grid(spec: cl_grid::GridSpec) -> Grid {
    // The register specs are fixed and covered by tests; a failure is a
    // programming error in `view::spec`.
    Grid::new("register/grid", spec).expect("the register grid specs are valid")
}

/// A custom "Show" range being typed.
#[derive(Debug, Default)]
struct CustomRange {
    open: bool,
    from: String,
    to: String,
    problem: Option<&'static str>,
}

/// The register window.
pub struct RegisterScreen {
    link: Arc<CompanyLink>,
    tenant: Option<(TenantId, String)>,
    group: Option<RequestGroup>,
    today: CalendarDate,
    fiscal: FiscalYearSpec,
    accounts: Vec<Account>,
    accounts_task: Option<Task<Vec<Account>>>,
    accounts_loaded: bool,
    settings_task: Option<Task<CompanySettings>>,
    /// The account asked for when the window opened.
    want_account: Option<AccountId>,
    account: Option<AccountId>,
    class: AccountClass,
    mapping: Mapping,
    prefs: RegisterPrefs,
    filters: Filters,
    query: Option<RegisterQuery>,
    model: RegisterModel,
    grid: Grid,
    grid_shape: (bool, AccountClass, BalanceMode),
    pages: Vec<(PageRequest, Task<RegisterPage>)>,
    allowed: Vec<EntryType>,
    /// Scroll to the newest line once it arrives (date order, oldest first).
    jump_to_end: bool,
    load_error: Option<ApiError>,
    custom: CustomRange,
    parts: EnvParts,
    snap: GridSnapshot,
    /// The account whose remembered choices were applied.
    prefs_for: Option<AccountId>,
    /// The choices changed and are not handed to the shell yet.
    prefs_dirty: bool,
    stamp: StampWatch,
    footer: footer::Footer,
    /// The entry row and Splits pickers.
    lists: Arc<lists::RegisterLists>,
    /// The entry row pinned under the grid.
    entry: record::EntryState,
    /// The Splits panel, when open.
    splits: Option<splits::SplitsPanel>,
    /// The edited transaction's other-side lines as read (a single line's
    /// memo and tag are kept).
    edit_lines: Vec<splits::SplitRow>,
    /// A transaction being read for an edit.
    edit_load: Option<edit::EditLoad>,
    /// A transaction the register can't change, shown read-only.
    read_only: Option<Vec<edit::ReadLine>>,
    /// The quick filters as typed.
    quick: filters::QuickInput,
    /// Go To (Ctrl+F).
    goto: goto::GotoBar,
    /// QuickReport (Ctrl+Shift+Q), when shown.
    report: Option<quick_report::QuickReport>,
    /// The grid's height last frame (it changes when the entry row grows
    /// for Splits, or a bar or QuickReport opens above).
    grid_room: f32,
    /// The closing-date password flow, Void and Delete (WP8).
    actions: dialogs::RowActions,
}

/// Watches the ledger stamp (Q-P12n): read on window focus, every
/// [`STAMP_EVERY`] seconds while the window is drawn, and after each own
/// write; a change reloads the shown pages.
#[derive(Debug, Default)]
struct StampWatch {
    last: Option<LedgerStamp>,
    task: Option<Task<LedgerStamp>>,
    read_at: Option<f64>,
    focused: bool,
    /// Read again at once (after an own write); the answer only resets the
    /// baseline because the register already reloaded.
    now: bool,
    quiet: bool,
    /// Own writes so far, and the count when the read in flight started:
    /// a read started before an own write is ignored (it would make the
    /// write look like someone else's, or eat the quiet re-read).
    writes: u64,
    task_writes: u64,
}

/// Takes one stamp reading. `fresh` is false for a read started before the
/// latest own write: it is ignored. Returns true when the ledger moved
/// under someone else's change (the register must reload).
fn stamp_arrived<S: PartialEq + Copy>(
    last: &mut Option<S>,
    quiet: &mut bool,
    fresh: bool,
    s: S,
) -> bool {
    if !fresh {
        return false;
    }
    let moved = last.is_some_and(|old| old != s);
    *last = Some(s);
    moved && !std::mem::take(quiet)
}

/// Seconds between ledger-stamp reads while the register is visible.
const STAMP_EVERY: f64 = 5.0;

impl RegisterScreen {
    /// The register of `account` (or the default account) in the open
    /// company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
        account: Option<AccountId>,
    ) -> RegisterScreen {
        let group = link.group().and_then(Result::ok);
        let class = AccountClass::Bank;
        let balance = BalanceMode::Engine;
        let shared = Arc::new(lists::RegisterLists::default());
        let mut s = RegisterScreen {
            link,
            tenant,
            group,
            today,
            fiscal: FiscalYearSpec::CALENDAR_YEAR,
            accounts: Vec::new(),
            accounts_task: None,
            accounts_loaded: false,
            settings_task: None,
            want_account: account,
            account: None,
            class,
            mapping: Mapping::default(),
            prefs: RegisterPrefs::default(),
            filters: Filters::default(),
            query: None,
            model: RegisterModel::new(0),
            grid: new_grid(view::spec(false, class, balance)),
            grid_shape: (false, class, balance),
            pages: Vec::new(),
            allowed: Vec::new(),
            jump_to_end: false,
            load_error: None,
            custom: CustomRange::default(),
            parts: EnvParts::default(),
            snap: GridSnapshot::default(),
            prefs_for: None,
            prefs_dirty: false,
            stamp: StampWatch::default(),
            footer: footer::Footer::default(),
            entry: record::EntryState::new(Arc::clone(&shared), today),
            lists: shared,
            splits: None,
            edit_lines: Vec::new(),
            edit_load: None,
            read_only: None,
            quick: filters::QuickInput::default(),
            goto: goto::GotoBar::default(),
            report: None,
            grid_room: 0.0,
            actions: dialogs::RowActions::default(),
        };
        s.start_load();
        s.load_lists();
        // The demo answers at once: the tab title is right before the
        // window is first drawn.
        s.poll();
        s
    }

    /// The account shown.
    #[must_use]
    pub fn account(&self) -> Option<AccountId> {
        self.account
    }

    fn account_record(&self) -> Option<&Account> {
        let id = self.account?;
        self.accounts.iter().find(|a| a.id == id)
    }

    /// True when the engine reports balances that ignore filters (R1).
    fn register_exact(&self) -> bool {
        // The demo engine behaves like `register-exact-v1`.
        self.link.is_demo()
            || self
                .link
                .capabilities()
                .has(cl_api::Capabilities::REGISTER_EXACT)
    }

    fn balance_mode(&self) -> BalanceMode {
        BalanceMode::choose(self.filters.is_active(), self.register_exact())
    }

    fn start_load(&mut self) {
        self.load_error = None;
        if self.link.is_demo() {
            let accounts = self.link.demo_state().map(|mut d| {
                d.chart
                    .get_or_insert_with(crate::company::demo::chart)
                    .clone()
            });
            let closing = self.link.demo_state().and_then(|d| {
                let open = d.open?;
                d.companies
                    .iter()
                    .find(|c| c.tenant_id == open)
                    .and_then(|c| c.closed_through)
            });
            self.model.closing = closing;
            self.accounts_task = Some(Task::ready(accounts.unwrap_or_default()));
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
        if let Ok(p) = group.read(&session, |s| async move { s.company_settings().await }) {
            self.settings_task = Some(Task::Pending(p));
        }
    }

    /// The account to show when none was asked for: the first active bank
    /// account, else the first account with a register.
    fn default_account(&self) -> Option<AccountId> {
        let with_register = || {
            self.accounts
                .iter()
                .filter(|a| a.is_active && class_of(a).is_some())
        };
        with_register()
            .find(|a| class_of(a) == Some(AccountClass::Bank))
            .or_else(|| with_register().next())
            .map(|a| a.id)
    }

    fn apply_accounts(&mut self, accounts: Vec<Account>) {
        self.accounts = accounts;
        self.accounts_loaded = true;
        self.mapping.names = self
            .accounts
            .iter()
            .map(|a| (a.id, account_text(a)))
            .collect();
        self.rebuild_lists();
        let wanted = self
            .want_account
            .or(self.account)
            .filter(|id| {
                self.accounts
                    .iter()
                    .any(|a| a.id == *id && class_of(a).is_some())
            })
            .or_else(|| self.default_account());
        if let Some(id) = wanted {
            self.show_account(id);
        }
    }

    /// Shows `id` in this window.
    pub(crate) fn show_account(&mut self, id: AccountId) {
        let Some(class) = self.accounts.iter().find(|a| a.id == id).and_then(class_of) else {
            return;
        };
        self.account = Some(id);
        self.class = class;
        self.mapping.class = Some(class);
        self.allowed.clear();
        self.reset_entry();
        self.apply_query();
    }

    /// Rebuilds the engine query from the range, sort and filters, and
    /// starts over when it changed.
    pub(crate) fn apply_query(&mut self) {
        let Some(account) = self.account else { return };
        let range = self.prefs.range.resolve(self.today, &self.fiscal);
        let mut q = RegisterQuery::new(range.start, range.end);
        (q.sort, q.dir) = sort_of(self.prefs.sort);
        q.payee = self.filters.payee_text().map(str::to_string);
        q.min = self.filters.min;
        q.max = self.filters.max;
        if self.register_exact() {
            q.tag = self.filters.tag;
            q.cleared = self
                .filters
                .uncleared
                .then_some(cl_api::ClearedFilter::Uncleared);
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        account.to_string().hash(&mut h);
        format!("{q:?}").hash(&mut h);
        (q.payee.as_deref(), q.min, q.max, q.tag).hash(&mut h);
        let fp = h.finish();
        if self.query.as_ref() != Some(&q) {
            self.goto.cancel();
        }
        self.query = Some(q);
        if self.model.set_query(fp) {
            self.jump_to_end = self.prefs.sort == SortChoice::DateOldest;
        }
        self.sync_grid();
    }

    /// Rebuilds the grid when its columns change (one line, labels,
    /// balance help).
    fn sync_grid(&mut self) {
        let shape = (self.prefs.one_line, self.class, self.balance_mode());
        self.model.one_line = shape.0;
        self.model.balance = shape.2;
        self.model.touch();
        if shape != self.grid_shape {
            let keep = self.grid.core().cursor().map(|c| c.row);
            self.grid = new_grid(view::spec(shape.0, shape.1, shape.2));
            self.grid_shape = shape;
            // The cursor stays on its line in the new layout.
            if let Some(key) = keep
                && cl_grid::RowModel::index_of(&self.model, key).is_some()
            {
                let label = String::new();
                let env = self
                    .parts
                    .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
                self.grid
                    .scroll_to(key, RowAlign::Nearest, &ModelRef::Browse(&mut self.model));
                self.grid.focus(
                    CellPos {
                        row: key,
                        col: COL_DATE,
                    },
                    ModelRef::Browse(&mut self.model),
                    &env,
                );
            }
        }
    }

    /// Starts the page fetches the grid needs.
    fn request_pages(&mut self) {
        let Some(account) = self.account else { return };
        let Some(query) = self.query.clone() else {
            return;
        };
        let rows = if self.jump_to_end {
            self.model.total().filter(|t| *t > 0).map(|t| (t - 1)..t)
        } else {
            self.goto.jump.map(|(i, _)| i..i + 1)
        };
        for req in self.model.wanted(rows) {
            match self.fetch_page(account, &query, req) {
                Some(task) => self.pages.push((req, task)),
                None => {
                    self.model.fail(req);
                }
            }
        }
    }

    fn fetch_page(
        &self,
        account: AccountId,
        query: &RegisterQuery,
        req: PageRequest,
    ) -> Option<Task<RegisterPage>> {
        let page = u32::try_from(req.page.engine_page()).ok()?;
        self.fetch_raw(account, query, page, per_page())
    }

    /// Reads page `page` (from 1) of `query` with `per` rows a page.
    fn fetch_raw(
        &self,
        account: AccountId,
        query: &RegisterQuery,
        page: u32,
        per: u16,
    ) -> Option<Task<RegisterPage>> {
        if let Some(mut demo) = self.link.demo_state() {
            let chart = demo
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            return crate::company::demo::register::page(
                &demo.ledger,
                &chart,
                account,
                query,
                page,
                per,
            )
            .map(Task::ready);
        }
        let session = self.link.session()?;
        let group = self.group.as_ref()?;
        let q = query.clone();
        group
            .read(&session, move |s| async move {
                s.register().page(account, &q, page, per).await
            })
            .ok()
            .map(Task::Pending)
    }

    fn poll(&mut self) {
        self.poll_entry();
        self.poll_edit();
        self.poll_actions();
        self.drive_goto();
        self.poll_report();
        if let Some(task) = self.accounts_task.as_mut()
            && let Some(result) = task.try_take()
        {
            self.accounts_task = None;
            match result {
                Ok(accounts) => self.apply_accounts(accounts),
                Err(e) => {
                    self.accounts_loaded = true;
                    self.load_error = Some(e);
                }
            }
        }
        if let Some(task) = self.settings_task.as_mut()
            && let Some(result) = task.try_take()
        {
            self.settings_task = None;
            if let Ok(s) = result {
                self.model.closing = s.lock_date;
                if let Some(m) = s.fiscal_year_start_month {
                    self.fiscal = FiscalYearSpec::Calendar { start_month: m };
                }
                self.model.touch();
                // Quarters and years may have moved with the fiscal start.
                self.apply_query();
            }
        }
        let mut i = 0;
        while i < self.pages.len() {
            let Some(result) = self.pages[i].1.try_take() else {
                i += 1;
                continue;
            };
            let (req, _) = self.pages.swap_remove(i);
            match result {
                Ok(page) => {
                    if self.model.is_current(&req) {
                        self.allowed.clone_from(&page.allowed);
                        self.footer.page_arrived(&page);
                        self.load_error = None;
                    }
                    self.model.accept(req, &page, &self.mapping);
                }
                Err(e) => {
                    if self.model.is_current(&req) && self.model.total().is_none() {
                        self.load_error = Some(e);
                    }
                    self.model.fail(req);
                }
            }
        }
    }

    /// Once the newest line has arrived, scroll to it and put the cursor
    /// there. The keyboard moves to the register only when nobody is typing
    /// in a field outside it (a quick filter reloads the rows while the
    /// owner may already be in the next filter field).
    fn finish_jump(&mut self, ctx: &egui::Context) {
        // Wait for the pages being read (after an own write the old rows
        // stay on screen until the new ones arrive).
        if !self.jump_to_end || !self.pages.is_empty() || self.model.window().in_flight() > 0 {
            return;
        }
        let Some(total) = self.model.total() else {
            return;
        };
        if total == 0 {
            self.jump_to_end = false;
            return;
        }
        let last = usize::try_from(total - 1).unwrap_or(usize::MAX);
        let Some(key) = cl_grid::RowModel::row_key(&self.model, last) else {
            return;
        };
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        self.grid
            .scroll_to(key, RowAlign::Bottom, &ModelRef::Browse(&mut self.model));
        self.jump_to_end = false;
        if self.typing_elsewhere(ctx) {
            return;
        }
        self.grid.focus(
            CellPos {
                row: key,
                col: COL_DATE,
            },
            ModelRef::Browse(&mut self.model),
            &env,
        );
        // The keyboard goes to the entry row, as on paper (plan WP5).
        if !self.view_only() && self.entry.editing.is_none() {
            self.entry.focus = Some(entry_row::E_DATE);
        }
    }

    /// A text field outside the register, its entry row and Splits has
    /// the keyboard (a quick filter, Go To's Find, a reason field).
    fn typing_elsewhere(&self, ctx: &egui::Context) -> bool {
        let Some(id) = ctx.memory(egui::Memory::focused) else {
            return false;
        };
        let ours = id == self.grid.focus_id()
            || id == self.entry.form.grid().focus_id()
            || self
                .splits
                .as_ref()
                .is_some_and(splits::SplitsPanel::has_focus);
        !ours && ctx.text_edit_focused()
    }

    fn grid_ui(&mut self, ui: &mut Ui, reserve: f32) {
        let room = ui.available_height() - reserve;
        let label = t("register.grid_label");
        let env = self.parts.env(
            &NoLookups,
            &label,
            GridHeight::Fill {
                reserve_below: reserve,
                min_rows: 2,
            },
        );
        let out = self.grid.show(ui, ModelRef::Browse(&mut self.model), &env);
        // The grid got shorter or taller (Splits opened or closed under it,
        // Go To or QuickReport above it): keep the cursor row in view.
        if (room - self.grid_room).abs() > 0.5 {
            self.grid_room = room;
            if let Some(key) = self.grid.core().cursor().map(|c| c.row) {
                self.grid
                    .scroll_to(key, RowAlign::Nearest, &ModelRef::Browse(&mut self.model));
                ui.ctx().request_repaint();
            }
        }
        self.snap = self.grid.snapshot(&ModelRef::Browse(&mut self.model));
        for e in out.events {
            match e {
                GridEvent::Activate { key } => {
                    self.on_activate(key);
                }
                GridEvent::LockedRowEditAttempt { key } => {
                    self.on_locked_edit(key);
                }
                GridEvent::TypedInBrowse(text) => {
                    self.on_typed(&text);
                }
                _ => {}
            }
        }
    }

    /// The key of this register's remembered choices (company + account).
    fn prefs_key(&self) -> Option<String> {
        let (tenant, _) = self.tenant.as_ref()?;
        Some(crate::prefs::RegisterPrefsBook::key(
            &tenant.to_string(),
            &self.account?.to_string(),
        ))
    }

    /// Applies the choices remembered for a newly shown account, and hands
    /// changed choices to the shell (which writes them, never during a
    /// controlled run).
    fn sync_prefs(&mut self, ctx: &egui::Context) {
        if self.account.is_none() {
            return;
        }
        if self.prefs_for != self.account {
            self.prefs_for = self.account;
            if let Some(p) = self
                .prefs_key()
                .and_then(|k| crate::grid_env::load_register_prefs(ctx, &k))
                && p != self.prefs
            {
                self.prefs = p;
                self.prefs_dirty = false;
                self.apply_query();
            }
        } else if self.prefs_dirty {
            self.prefs_dirty = false;
            if let Some(k) = self.prefs_key() {
                crate::grid_env::store_register_prefs(ctx, &k, self.prefs);
            }
        }
    }

    /// After this window saved a change: reload now and re-read the stamp
    /// so the change is not taken for someone else's (WP6-WP8 call it).
    pub(crate) fn after_own_write(&mut self) {
        self.model.invalidate();
        self.footer.invalidate();
        self.stamp.now = true;
        self.stamp.quiet = true;
        self.stamp.writes += 1;
    }

    /// The ledger moved: the shown pages reload (rows stay on screen until
    /// their pages return).
    fn ledger_changed(&mut self) {
        self.model.invalidate();
        self.footer.invalidate();
    }

    /// Polls and starts ledger-stamp reads (Q-P12n).
    fn watch_stamp(&mut self, ctx: &egui::Context) {
        if let Some(task) = self.stamp.task.as_mut()
            && let Some(result) = task.try_take()
        {
            self.stamp.task = None;
            let fresh = self.stamp.task_writes == self.stamp.writes;
            if let Ok(s) = result {
                let w = &mut self.stamp;
                if stamp_arrived(&mut w.last, &mut w.quiet, fresh, s) {
                    self.ledger_changed();
                }
                if fresh {
                    self.stamp.quiet = false;
                }
            }
        }
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        let gained = focused && !self.stamp.focused;
        self.stamp.focused = focused;
        let now = ctx.input(|i| i.time);
        let due =
            gained || self.stamp.now || self.stamp.read_at.is_none_or(|t| now - t >= STAMP_EVERY);
        if due && self.stamp.task.is_none() && self.account.is_some() {
            self.stamp.now = false;
            self.stamp.read_at = Some(now);
            self.stamp.task_writes = self.stamp.writes;
            self.stamp.task = self.read_stamp();
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(STAMP_EVERY));
    }

    fn read_stamp(&self) -> Option<Task<LedgerStamp>> {
        if self.link.is_demo() {
            // The demo ledger only changes through this window.
            return None;
        }
        let session = self.link.session()?;
        let group = self.group.as_ref()?;
        group
            .read(&session, |s| async move { s.ledger_version().await })
            .ok()
            .map(Task::Pending)
    }

    fn heading(&self, ui: &mut Ui, tokens: &Tokens) {
        let Some(a) = self.account_record() else {
            form::heading(ui, tokens, &t("cmd.banking.register"));
            return;
        };
        // Mockup Register.html: the account in the 22/600 page heading, its
        // kind beside it in secondary text. The masked bank number the mockup
        // shows after the kind waits on the engine (U-34).
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            ui.add(
                egui::Label::new(
                    RichText::new(account_text(a))
                        .text_style(egui::TextStyle::Heading)
                        .color(tokens.text_primary),
                )
                .selectable(false),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(t(filters::class_key(self.class))).color(tokens.text_secondary),
                )
                .selectable(false),
            );
        });
    }

    fn messages(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(e) = &self.load_error {
            form::inline_error(ui, tokens, e);
            if ui.button(t("register.try_again")).clicked() {
                self.retry();
            }
        }
        if self.model.display_only() || self.footer.display_only() {
            ui.add(
                egui::Label::new(
                    RichText::new(t("register.display_only"))
                        .size(12.0)
                        .color(tokens.text_secondary),
                )
                .wrap(),
            );
        }
        if self.model.failed_pages() > 0
            && self.load_error.is_none()
            && ui.button(t("register.try_again")).clicked()
        {
            self.model.retry_failed();
        }
    }

    fn retry(&mut self) {
        self.load_error = None;
        if self.accounts_loaded && self.accounts.is_empty() {
            self.accounts_loaded = false;
            self.start_load();
        } else {
            self.model.retry_failed();
        }
    }

    fn empty_ui(&mut self, ui: &mut Ui, tokens: &Tokens) -> bool {
        if self.account.is_none() {
            if self.load_error.is_some() {
                return true;
            }
            ui.label(RichText::new(t("register.no_accounts")).color(tokens.text_secondary));
            if ui.button(t("register.open_chart")).clicked() {
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    crate::screens::lists::accounts::KIND,
                )));
            }
            return true;
        }
        if self.model.total() == Some(0) {
            let Some(q) = &self.query else { return true };
            let key = if self.filters.is_active() {
                "register.empty_filtered"
            } else {
                "register.empty_range"
            };
            ui.label(
                RichText::new(t_with(
                    key,
                    &[
                        ("from", &crate::shell::us_date(q.start)),
                        ("to", &crate::shell::us_date(q.end)),
                    ],
                ))
                .color(tokens.text_secondary),
            );
            if !self.filters.is_active()
                && self.prefs.range != cl_model::register::RangePreset::AllDates
                && ui.link(t("register.show_all_dates")).clicked()
            {
                self.prefs.range = cl_model::register::RangePreset::AllDates;
                self.apply_query();
            }
            return true;
        }
        false
    }
}

// ----------------------------------------------------------------------
// Later lanes (WP6-WP8) fill these in; WP5 routes the events here so the
// lanes do not collide in this file.
#[allow(
    clippy::unused_self,
    reason = "event handlers that WP6-WP8 fill in; WP5 only routes to them"
)]
impl RegisterScreen {
    /// Enter or a double-click on a row: change it in the pinned row.
    fn on_activate(&mut self, key: cl_grid::RowKey) {
        self.begin_edit(key, false);
    }

    /// Typing on a locked row: with an engine that takes the closing-date
    /// password the edit opens (the password is asked when it is recorded);
    /// otherwise a message explains that closed rows can't be changed yet.
    fn on_locked_edit(&mut self, key: cl_grid::RowKey) {
        if self.closing_supported() {
            self.begin_edit(key, false);
        } else {
            self.closing_unavailable();
        }
    }

    /// Typing on a row while browsing starts an edit of it.
    fn on_typed(&mut self, _text: &str) {
        if let Some(key) = self.grid.core().cursor().map(|c| c.row) {
            self.begin_edit(key, false);
        }
    }

    /// Ctrl+F: Go To.
    fn on_find(&mut self) -> bool {
        self.open_goto();
        true
    }

    /// Ctrl+E: Splits for the row being entered, else Edit Transaction on
    /// the register's row (with Splits for checks and card charges).
    fn on_edit_transaction(&mut self) -> bool {
        let in_entry = self.entry.form.grid().has_focus()
            || self
                .splits
                .as_ref()
                .is_some_and(splits::SplitsPanel::has_focus);
        if in_entry || self.entry.editing.is_some() {
            self.toggle_splits();
        } else if let Some(key) = self.grid.core().cursor().map(|c| c.row) {
            self.begin_edit(key, true);
        }
        true
    }

    /// Ctrl+D: delete the row's transaction, or offer Void where delete
    /// can't be used.
    fn on_delete(&mut self) -> bool {
        self.start_delete()
    }

    /// Void Transaction on the row under the cursor.
    fn on_void(&mut self) -> bool {
        self.start_void()
    }

    /// Ctrl+Shift+Q: QuickReport for this account and the shown dates.
    fn on_quick_report(&mut self) -> bool {
        self.toggle_quick_report();
        true
    }

    /// Alt+N / Alt+S on the entry row (WP6): record the row. The register
    /// stays open either way.
    fn on_record(&mut self, _id: CommandId) -> bool {
        self.start_record();
        true
    }
}

impl RegisterScreen {
    /// Esc closes the Go To bar, then the QuickReport, before the tab:
    /// `None` when neither is open (or for any other command).
    fn panel_command(&mut self, id: CommandId) -> Option<bool> {
        if id.as_str() != "form.revert" {
            return None;
        }
        if self.goto.open {
            self.close_goto();
            return Some(true);
        }
        if self.report.is_some() {
            self.report = None;
            return Some(true);
        }
        None
    }
}

impl Screen for RegisterScreen {
    fn title(&self) -> String {
        self.account_record().map_or_else(
            || t("cmd.banking.register"),
            |a| t_with("register.title", &[("account", &a.fields.name)]),
        )
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        if self.model.style != self.parts.frame.money_style {
            self.model.style = self.parts.frame.money_style;
            self.model.touch();
        }
        self.poll();
        self.sync_prefs(ui.ctx());
        self.watch_stamp(ui.ctx());
        let tokens = Tokens::of(ui.ctx());
        self.heading(ui, &tokens);
        ui.add_space(4.0);
        if !self.accounts_loaded {
            form::progress_line(ui, &tokens, &t("register.loading"));
            if self.accounts_task.is_some() {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            return;
        }
        self.toolbar(ui, &tokens);
        self.goto_ui(ui, &tokens);
        self.quick_report_ui(ui, &tokens);
        self.messages(ui, &tokens);
        self.actions_ui(ui, &tokens);
        ui.add_space(4.0);
        if self.empty_ui(ui, &tokens) {
            self.request_pages();
            return;
        }
        if self.model.total().is_none() {
            form::progress_line(ui, &tokens, &t("register.loading"));
        } else {
            self.grid_ui(ui, footer::HEIGHT + self.entry.height);
            self.entry_ui(ui, &tokens);
            self.footer_ui(ui, &tokens);
        }
        self.request_pages();
        self.footer_load();
        self.finish_jump(ui.ctx());
        self.finish_goto();
        if !self.pages.is_empty() || self.footer.loading() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn scope(&self) -> Scope {
        // A grid screen: Esc arrives as `form.revert` (Q-P10d).
        Scope::Form
    }

    fn dirty(&self) -> bool {
        self.entry_dirty()
    }

    fn save(&mut self) -> bool {
        self.start_record()
    }

    fn command(&mut self, id: CommandId) -> bool {
        if let Some(handled) = self.actions_command(id) {
            return handled;
        }
        if let Some(handled) = self.entry_command(id) {
            return handled;
        }
        if let Some(handled) = self.panel_command(id) {
            return handled;
        }
        match id.as_str() {
            // Ctrl+G inside a register stays on it.
            KIND => true,
            "register.one_line" => {
                self.set_one_line(!self.prefs.one_line);
                true
            }
            "edit.find" => self.on_find(),
            "edit.edit_transaction" => self.on_edit_transaction(),
            "form.delete_item" => self.on_delete(),
            "edit.void_transaction" => self.on_void(),
            "list.quick_report" => self.on_quick_report(),
            "form.save_new" | "form.save_close" => self.on_record(id),
            "form.prev_record" | "form.next_record" => {
                let cmd = if id.as_str() == "form.prev_record" {
                    cl_grid::GridCommand::PageUp
                } else {
                    cl_grid::GridCommand::PageDown
                };
                let label = String::new();
                let env = self
                    .parts
                    .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
                self.grid
                    .command(cmd, ModelRef::Browse(&mut self.model), &env)
                    .0
                    .yes()
            }
            _ => false,
        }
    }

    fn state(&self) -> Value {
        let total = self.model.total();
        json!({
            "screen": "register",
            "company_open": self.tenant.is_some(),
            "account_id": self.account.map(|a| a.to_string()),
            "account_class": self.account.map(|_| filters::class_name(self.class)),
            "loaded": self.accounts_loaded && total.is_some(),
            "load_error": self.load_error.is_some(),
            "no_accounts": self.accounts_loaded && self.account.is_none(),
            "rows": total,
            "range": self.prefs.range.to_pref(),
            "range_start": self.query.as_ref().map(|q| q.start.to_wire()),
            "range_end": self.query.as_ref().map(|q| q.end.to_wire()),
            "sort": format!("{:?}", self.prefs.sort),
            "one_line": self.prefs.one_line,
            "balance_mode": match self.balance_mode() {
                BalanceMode::Engine => "engine",
                BalanceMode::HiddenFiltered => "hidden_filtered",
            },
            "display_only": self.model.display_only(),
            "filters_active": self.filters.is_active(),
            "filters": {
                "uncleared": self.filters.uncleared,
                "payee": self.filters.payee_text().is_some(),
                "tag": self.filters.tag.is_some(),
                "amount": self.filters.min.is_some() || self.filters.max.is_some(),
                "problem": self.quick.problem.is_some(),
            },
            "cursor_row": self.snap.cursor_row,
            "top_row": self.snap.top_row,
            "focused": self.snap.focused,
            "pages_cached": self.model.window().cached_pages(),
            "in_flight": self.model.window().in_flight(),
            "failed_pages": self.model.failed_pages(),
            "entry_types": self.allowed.len(),
            "footer": self.footer_state(),
            "entry": self.entry_state(),
            "goto": self.goto.state(),
            "quick_report": self.report.as_ref().map(quick_report::QuickReport::state),
            "row_action": self.actions_state(),
        })
    }
}

#[cfg(test)]
mod tests;
