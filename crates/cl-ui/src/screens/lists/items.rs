//! Item List (P11 step 4): every product and service on the ledger grid,
//! an add/edit panel, and make inactive / active again.
//!
//! The engine keeps service items only (name, description, sales price,
//! income account, taxable). The other kinds (non-inventory, other charge,
//! discount, subtotal), the purchase side of two-sided items (Q-99) and the
//! unit-of-measure label (Q-96) wait for U-39; price levels (Q-58) wait for
//! U-40. The panel says so instead of offering fields that would be
//! dropped. Prices are exact: typed text is parsed to `Money` and shown back
//! from the engine's decimal string.
//!
//! Keyboard: Ctrl+N new, Enter or Ctrl+E edit, Ctrl+Shift+I make inactive
//! or active again, Ctrl+Shift+H include inactive, Ctrl+F find; in the
//! panel Alt+N Save & New, Alt+S Save & Close, Esc reverts, then closes.

use std::sync::Arc;

use cl_api::{
    Account, AccountFilter, AccountType, ApiError, Capabilities, ConflictReason, EditableFields,
    IdempotencyKey, Item, ItemFields, ItemFilter as ApiItemFilter, ItemUpdate, OrphanLabel,
    RecordVersion, RequestGroup, Versioned,
};
use cl_core::ids::{AccountId, ItemId, TenantId};
use cl_grid::{CellPos, Grid, GridEvent, GridHeight, GridSnapshot, ModelRef, NoLookups, RowKey};
use cl_model::forms::{AfterSave, FormState, SaveIntent, SaveStart};
use cl_model::lists::items::{
    ItemDraft, ItemEntry, ItemField, ItemFilter, ItemKind, ItemList, ItemProblem, check, price_text,
};
use cl_model::{CommandId, Scope, commands};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::{CompanyLink, Task};
use crate::grid_env::EnvParts;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::screens::lists::detail_pane as pane;
use crate::screens::lists::rows::{TextRow, TextRows, column, figure_column, list_spec};
use crate::theme::Tokens;
use crate::windows::Screen;

/// The command and window kind.
pub const KIND: &str = "lists.items";

const COL_NAME: cl_grid::ColumnId = cl_grid::ColumnId(1);
const COL_DESCRIPTION: cl_grid::ColumnId = cl_grid::ColumnId(2);
const COL_KIND: cl_grid::ColumnId = cl_grid::ColumnId(3);
const COL_PRICE: cl_grid::ColumnId = cl_grid::ColumnId(4);
const COL_ACCOUNT: cl_grid::ColumnId = cl_grid::ColumnId(5);
const COL_TAXABLE: cl_grid::ColumnId = cl_grid::ColumnId(6);
const COL_STATUS: cl_grid::ColumnId = cl_grid::ColumnId(7);

// Field ids for undo merging.
const F_NAME: u64 = 1;
const F_DESCRIPTION: u64 = 2;
const F_PRICE: u64 = 3;
const F_ACCOUNT: u64 = 4;
const F_TAXABLE: u64 = 5;
const F_KIND: u64 = 6;

/// Width of the add/edit panel.
const PANEL_WIDTH: f32 = 420.0;
/// Typed letters within this many seconds build one find prefix.
const TYPE_AHEAD_SECS: f64 = 1.0;

fn field_id(f: ItemField) -> egui::Id {
    egui::Id::unique(("items-form", f as u8))
}

fn search_id() -> egui::Id {
    egui::Id::unique("items-search")
}

fn shortcut_hint(id: &str) -> String {
    commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

fn cols(compact: bool) -> Vec<cl_grid::Column> {
    if compact {
        vec![
            column(COL_NAME, "items.col.name", 220, 2),
            figure_column(COL_PRICE, "items.col.price", 110),
            column(COL_STATUS, "items.col.status", 90, 0),
        ]
    } else {
        vec![
            column(COL_NAME, "items.col.name", 220, 2),
            column(COL_DESCRIPTION, "items.col.description", 240, 2),
            column(COL_KIND, "items.col.kind", 110, 0),
            figure_column(COL_PRICE, "items.col.price", 110),
            column(COL_ACCOUNT, "items.col.account", 170, 1),
            column(COL_TAXABLE, "items.col.taxable", 80, 0),
            column(COL_STATUS, "items.col.status", 90, 0),
        ]
    }
}

fn new_grid(compact: bool) -> (Grid, TextRows<ItemId>) {
    let spec_cols = cols(compact);
    let ids = spec_cols.iter().map(|c| c.id).collect();
    let salt = if compact {
        "items/list-compact"
    } else {
        "items/list"
    };
    // The specs are fixed and covered by tests; a failure is a programming
    // error in this file.
    let grid = Grid::new(salt, list_spec(spec_cols)).expect("the item list grid spec is valid");
    (grid, TextRows::new(ids))
}

/// The add/edit panel.
struct FormPanel {
    editing: Option<ItemId>,
    form: FormState<ItemDraft>,
    problems: Vec<ItemProblem>,
    error: Option<ApiError>,
    focus: Option<ItemField>,
    task: Option<Task<Versioned<Item>>>,
}

impl FormPanel {
    fn new(editing: Option<ItemId>, draft: ItemDraft) -> FormPanel {
        FormPanel {
            editing,
            form: FormState::new(draft),
            problems: Vec::new(),
            error: None,
            focus: Some(ItemField::Name),
            task: None,
        }
    }

    fn save_failed(&mut self, e: ApiError) {
        self.form.save_failed(cl_model::forms::SaveFailure {
            message: cl_core::OwnerFacing::owner_message(&e),
            detail: cl_core::OwnerFacing::scrubbed_detail(&e),
        });
        // Another window took the name first: the same plain words as the
        // local check, on the name field.
        if matches!(
            &e,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists | ConflictReason::Unspecified,
                ..
            }
        ) {
            self.problems = vec![ItemProblem::NameTaken];
            self.focus = Some(ItemField::Name);
        } else {
            self.error = Some(e);
        }
    }
}

/// The Item List window.
pub struct ItemsScreen {
    link: Arc<CompanyLink>,
    group: Option<RequestGroup>,
    items: Vec<Item>,
    accounts: Vec<Account>,
    list: ItemList,
    filter: ItemFilter,
    model: TextRows<ItemId>,
    grid: Grid,
    load: Option<Task<(Vec<Item>, Vec<Account>)>>,
    loaded: bool,
    load_error: Option<ApiError>,
    toggle: Option<(ItemId, Task<Versioned<Item>>)>,
    toggle_error: Option<ApiError>,
    panel: Option<Box<FormPanel>>,
    note: Option<&'static str>,
    warn: Option<&'static str>,
    parts: EnvParts,
    snap: GridSnapshot,
    focus_search: bool,
    want_cursor: Option<ItemId>,
    typed: String,
    typed_at: f64,
    compact: bool,
    just_hidden: Option<ItemId>,
    item_types: bool,
    price_levels: bool,
}

impl ItemsScreen {
    /// The window over the open company.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, _tenant: Option<(TenantId, String)>) -> ItemsScreen {
        let group = link.group().and_then(Result::ok);
        let caps = link.client().map(cl_api::Client::capabilities);
        let has = |c: &str| caps.as_ref().is_some_and(|k| k.has(c));
        let (grid, model) = new_grid(false);
        let mut s = ItemsScreen {
            item_types: has(Capabilities::ITEM_TYPES),
            price_levels: has(Capabilities::PRICE_LEVELS),
            link,
            group,
            items: Vec::new(),
            accounts: Vec::new(),
            list: ItemList::default(),
            filter: ItemFilter::default(),
            model,
            grid,
            load: None,
            loaded: false,
            load_error: None,
            toggle: None,
            toggle_error: None,
            panel: None,
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

    fn start_load(&mut self) {
        self.load_error = None;
        if let Some(mut d) = self.link.demo_state() {
            let items = d.items_mut().clone();
            let chart = d
                .chart
                .get_or_insert_with(crate::company::demo::chart)
                .clone();
            self.load = Some(Task::ready((items, chart)));
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.loaded = true;
            return;
        };
        match group.read(&session, |s| async move {
            let items = s.items().list_all(&ApiItemFilter::default()).await?;
            let accounts = s.accounts().list_all(&AccountFilter::default()).await?;
            Ok((items, accounts))
        }) {
            Ok(p) => self.load = Some(Task::Pending(p)),
            Err(_) => self.loaded = true,
        }
    }

    fn account_name(&self, id: AccountId) -> String {
        self.accounts
            .iter()
            .find(|a| a.id == id)
            .map(|a| match &a.fields.account_number {
                Some(n) if !n.is_empty() => format!("{n} {}", a.fields.name),
                _ => a.fields.name.clone(),
            })
            .unwrap_or_default()
    }

    /// Accounts an item's sale may post to: active income accounts, plus
    /// the item's own account when it is something else.
    fn income_choices(&self, keep: Option<AccountId>) -> Vec<AccountId> {
        let mut ids: Vec<AccountId> = self
            .accounts
            .iter()
            .filter(|a| {
                a.is_active
                    && matches!(
                        a.fields.account_type,
                        AccountType::Revenue | AccountType::OtherRevenue
                    )
            })
            .map(|a| a.id)
            .collect();
        if let Some(k) = keep
            && !ids.contains(&k)
        {
            ids.push(k);
        }
        ids
    }

    fn is_income(&self, id: AccountId) -> bool {
        self.accounts.iter().any(|a| {
            a.id == id
                && matches!(
                    a.fields.account_type,
                    AccountType::Revenue | AccountType::OtherRevenue
                )
        })
    }

    fn apply(&mut self) {
        let keep = self.want_cursor.take().or_else(|| self.selected());
        let entries = self
            .items
            .iter()
            .map(|i| ItemEntry {
                id: i.id,
                name: i.fields.name.clone(),
                description: i.fields.description.clone(),
                price: i.fields.unit_price,
                income_account: i.fields.income_account_id,
                taxable: i.fields.is_taxable,
                active: i.is_active,
            })
            .collect();
        self.list = ItemList::new(entries);
        self.refilter();
        self.loaded = true;
        self.place_cursor(keep);
    }

    fn refilter(&mut self) {
        let (service, yes, no, inactive) = (
            t(ItemKind::Service.label().as_str()),
            t("items.taxable.yes"),
            t("items.taxable.no"),
            t("items.status.inactive"),
        );
        let rows = self
            .list
            .visible(&self.filter)
            .into_iter()
            .map(|i| {
                let e = &self.list.entries()[i];
                let price = e.price.map(price_text).unwrap_or_default();
                let status = if e.active {
                    String::new()
                } else {
                    inactive.clone()
                };
                let mut label = e.name.clone();
                if !price.is_empty() {
                    label = format!("{label}, {price}");
                }
                if !e.active {
                    label = format!("{label}, {inactive}");
                }
                let cells = if self.compact {
                    vec![e.name.clone(), price, status]
                } else {
                    vec![
                        e.name.clone(),
                        e.description.clone().unwrap_or_default(),
                        service.clone(),
                        price,
                        self.account_name(e.income_account),
                        if e.taxable { yes.clone() } else { no.clone() },
                        status,
                    ]
                };
                TextRow {
                    id: e.id,
                    cells,
                    label,
                }
            })
            .collect();
        self.model.set(rows);
    }

    fn place_cursor(&mut self, id: Option<ItemId>) {
        let key = id
            .and_then(|i| self.model.key_of(i))
            .or_else(|| cl_grid::RowModel::row_key(&self.model, 0));
        if let Some(key) = key {
            self.focus_row(key, !self.compact);
        }
    }

    fn focus_row(&mut self, key: RowKey, take_focus: bool) {
        let label = String::new();
        let env = self
            .parts
            .env(&NoLookups, &label, GridHeight::Rows { min: 1, max: 1 });
        let pos = CellPos {
            row: key,
            col: COL_NAME,
        };
        if take_focus {
            self.grid
                .focus(pos, ModelRef::Browse(&mut self.model), &env);
        } else {
            self.grid
                .set_cursor(pos, ModelRef::Browse(&mut self.model), &env);
        }
        self.grid.scroll_to(
            key,
            cl_grid::core::viewport::RowAlign::Nearest,
            &ModelRef::Browse(&mut self.model),
        );
    }

    fn sync_compact(&mut self) {
        let want = self.panel.is_some();
        if want == self.compact {
            return;
        }
        let keep = self.selected();
        self.compact = want;
        (self.grid, self.model) = new_grid(want);
        self.refilter();
        if let Some(key) = keep.and_then(|id| self.model.key_of(id)) {
            self.focus_row(key, !want);
        } else if !want {
            self.place_cursor(None);
        }
    }

    /// The item under the cursor.
    #[must_use]
    pub fn selected(&self) -> Option<ItemId> {
        self.grid
            .core()
            .cursor()
            .and_then(|c| self.model.id_at(c.row))
    }

    fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    fn form_dirty(&self) -> bool {
        self.panel.as_ref().is_some_and(|p| p.form.is_dirty())
    }

    fn default_income(&self) -> Option<AccountId> {
        self.selected()
            .and_then(|id| self.item(id))
            .map(|i| i.fields.income_account_id)
            .or_else(|| self.income_choices(None).first().copied())
    }

    /// Ctrl+N.
    pub fn open_new(&mut self) {
        if self.form_dirty() {
            self.warn = Some("items.finish_first");
            return;
        }
        self.warn = None;
        self.panel = Some(Box::new(FormPanel::new(
            None,
            ItemDraft::new(self.default_income()),
        )));
    }

    /// Enter or Ctrl+E on a row.
    pub fn open_edit(&mut self, id: Option<ItemId>) {
        let Some(id) = id else {
            self.warn = Some("items.select_first");
            return;
        };
        if self.form_dirty() {
            self.warn = Some("items.finish_first");
            return;
        }
        let Some(e) = self.list.entry(id) else {
            return;
        };
        self.warn = None;
        self.panel = Some(Box::new(FormPanel::new(Some(id), ItemDraft::from_entry(e))));
    }

    /// Ctrl+Shift+I.
    pub fn toggle_active(&mut self) {
        let Some(id) = self.selected() else {
            self.warn = Some("items.select_first");
            return;
        };
        if self.toggle.is_some() {
            return;
        }
        let Some(item) = self.item(id).cloned() else {
            return;
        };
        self.warn = None;
        self.note = None;
        self.toggle_error = None;
        let task = if let Some(mut d) = self.link.demo_state() {
            let mut changed = item.clone();
            changed.is_active = !item.is_active;
            if let Some(i) = d.items_mut().iter_mut().find(|i| i.id == id) {
                *i = changed.clone();
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
                what: "orphan.item_save",
                key: Some(key.clone()),
            };
            let pending = if item.is_active {
                group.write(&session, label, move |s| async move {
                    s.items().deactivate(id, key).await?;
                    s.items().get(id).await
                })
            } else {
                let version = RecordVersion::Fingerprint(item.fingerprint());
                let update = ItemUpdate {
                    fields: item.fields.clone(),
                    is_active: true,
                };
                group.write(&session, label, move |s| async move {
                    s.items().update(id, &version, &update, key).await
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
        self.refilter();
        self.place_cursor(keep);
    }

    /// Alt+N / Alt+S. Returns true when the save finished at once.
    pub fn save_form(&mut self, intent: SaveIntent) -> bool {
        let item_types = self.item_types;
        let checked = {
            let Some(panel) = self.panel.as_ref() else {
                return false;
            };
            if panel.task.is_some() {
                return false;
            }
            check(
                panel.form.current(),
                &self.list,
                panel.editing,
                item_types,
                |a| self.is_income(a),
            )
        };
        let Some(panel) = self.panel.as_mut() else {
            return false;
        };
        let checked = match checked {
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
            SaveStart::Busy => return false,
        }
        let fields = ItemFields {
            name: checked.name,
            description: checked.description,
            unit_price: checked.price,
            income_account_id: checked.income_account,
            is_taxable: checked.taxable,
        };
        let editing = panel.editing;
        let existing = editing.and_then(|id| self.items.iter().find(|i| i.id == id).cloned());
        if self.link.is_demo() {
            let saved = demo_save(&self.link, existing, fields);
            if let Some(panel) = self.panel.as_mut() {
                panel.task = Some(Task::ready(saved));
            }
            return self.poll_save();
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return false;
        };
        let key = IdempotencyKey::new();
        let label = OrphanLabel {
            what: "orphan.item_save",
            key: Some(key.clone()),
        };
        let pending = match existing {
            Some(i) => {
                let id = i.id;
                let version = RecordVersion::Fingerprint(i.fingerprint());
                let update = ItemUpdate {
                    fields,
                    is_active: i.is_active,
                };
                group.write(&session, label, move |s| async move {
                    s.items().update(id, &version, &update, key).await
                })
            }
            None => group.write(&session, label, move |s| async move {
                s.items().create(&fields, key).await
            }),
        };
        if let (Ok(p), Some(panel)) = (pending, self.panel.as_mut()) {
            panel.task = Some(Task::Pending(p));
        }
        false
    }

    fn poll_save(&mut self) -> bool {
        let Some(panel) = self.panel.as_mut() else {
            return false;
        };
        let Some(result) = panel.task.as_mut().and_then(Task::try_take) else {
            return false;
        };
        panel.task = None;
        match result {
            Ok(saved) => {
                let item = saved.record;
                let after = panel.form.save_ok(panel.form.current().clone());
                match self.items.iter_mut().find(|i| i.id == item.id) {
                    Some(i) => *i = item.clone(),
                    None => self.items.push(item.clone()),
                }
                self.want_cursor = Some(item.id);
                self.apply();
                self.note = Some("items.saved");
                self.after_save(after, Some(item.id));
                true
            }
            Err(e) => {
                panel.save_failed(e);
                false
            }
        }
    }

    fn after_save(&mut self, after: AfterSave, saved: Option<ItemId>) {
        match after {
            AfterSave::Stay => {
                if let Some(id) = saved
                    && let Some(e) = self.list.entry(id)
                {
                    let mut p = FormPanel::new(Some(id), ItemDraft::from_entry(e));
                    p.focus = None;
                    self.panel = Some(Box::new(p));
                }
            }
            AfterSave::NewForm => {
                let income = self
                    .panel
                    .as_ref()
                    .and_then(|p| p.form.current().income_account);
                self.panel = Some(Box::new(FormPanel::new(None, ItemDraft::new(income))));
            }
            AfterSave::CloseWindow => {
                self.panel = None;
                self.sync_compact();
                self.place_cursor(saved.or_else(|| self.selected()));
            }
        }
    }

    fn poll(&mut self) {
        if let Some(result) = self.load.as_mut().and_then(Task::try_take) {
            self.load = None;
            match result {
                Ok((items, accounts)) => {
                    self.items = items;
                    self.accounts = accounts;
                    self.apply();
                }
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
                    if let Some(i) = self.items.iter_mut().find(|i| i.id == id) {
                        *i = saved.record;
                    }
                    self.want_cursor = Some(id);
                    self.apply();
                    self.note = Some(if active {
                        "items.made_active"
                    } else {
                        "items.made_inactive"
                    });
                }
                Err(e) => self.toggle_error = Some(e),
            }
        }
        if self.panel.as_ref().is_some_and(|p| p.task.is_some()) {
            self.poll_save();
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let selected_active = self
            .selected()
            .and_then(|id| self.list.entry(id))
            .map(|e| e.active);
        ui.horizontal_wrapped(|ui| {
            let b = |ui: &mut Ui, key: &str, cmd: &str| {
                ui.button(t(key))
                    .on_hover_text(format!("{} ({})", t(key), shortcut_hint(cmd)))
            };
            if b(ui, "items.new", "list.new").clicked() {
                self.open_new();
            }
            if b(ui, "items.edit", "edit.edit_transaction").clicked() {
                self.open_edit(self.selected());
            }
            let toggle_key = if selected_active == Some(false) {
                "items.make_active"
            } else {
                "items.make_inactive"
            };
            if b(ui, toggle_key, "list.toggle_active").clicked() {
                self.toggle_active();
            }
            ui.separator();
            let l = ui.add(egui::Label::new(t("items.find")).selectable(false));
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
                self.refilter();
            }
            let mut inactive = self.filter.show_inactive;
            if ui
                .checkbox(&mut inactive, t("items.include_inactive"))
                .on_hover_text(shortcut_hint("list.show_inactive"))
                .changed()
            {
                self.set_show_inactive(inactive);
            }
        });
    }

    fn messages(&self, ui: &mut Ui, tokens: &Tokens) {
        for e in [&self.load_error, &self.toggle_error].into_iter().flatten() {
            form::inline_error(ui, tokens, e);
        }
        if let Some(w) = self.warn {
            ui.label(RichText::new(t(w)).color(tokens.warn_text));
        } else if let Some(n) = self.note {
            ui.label(RichText::new(t(n)).color(tokens.text_secondary));
        }
    }

    fn list_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let label = t("items.grid_label");
        let env = self.parts.env(
            &NoLookups,
            &label,
            GridHeight::Fill {
                reserve_below: 52.0,
                min_rows: 6,
            },
        );
        let out = self.grid.show(ui, ModelRef::Browse(&mut self.model), &env);
        self.snap = self.grid.snapshot(&ModelRef::Browse(&mut self.model));
        for e in out.events {
            match e {
                GridEvent::Activate { key } => {
                    let id = self.model.id_at(key);
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
                        self.focus_row(key, true);
                    }
                }
                _ => {}
            }
        }
        ui.label(
            RichText::new(t_with(
                "items.count",
                &[
                    ("shown", &self.model.len().to_string()),
                    ("total", &self.list.len().to_string()),
                ],
            ))
            .color(tokens.text_secondary),
        );
        if !self.price_levels {
            pane::pending(ui, tokens, &t("items.price_levels_later"));
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        let item_types = self.item_types;
        let income_ids = {
            let keep = self
                .panel
                .as_ref()
                .and_then(|p| p.form.current().income_account);
            self.income_choices(keep)
        };
        let income_labels: Vec<String> = income_ids.iter().map(|a| self.account_name(*a)).collect();
        let Some(panel) = self.panel.as_mut() else {
            return;
        };
        let mut save: Option<SaveIntent> = None;
        let mut cancel = false;
        let heading = if panel.editing.is_some() {
            t("items.form.edit_title")
        } else {
            t("items.form.new_title")
        };
        form::heading(ui, tokens, &heading);
        ui.add_space(6.0);
        let saving = panel.task.is_some();
        ui.add_enabled_ui(!saving, |ui| {
            let d = panel.form.current().clone();
            let focus = panel.focus.take();
            let want = |f: ItemField| focus == Some(f);
            // Kind: services only until U-39.
            let kinds = ItemKind::available(item_types);
            let labels: Vec<String> = kinds.iter().map(|k| t(k.label().as_str())).collect();
            let cur = kinds.iter().position(|k| *k == d.kind).unwrap_or(0);
            if let Some(i) = pane::picker(
                ui,
                field_id(ItemField::Kind),
                &t("items.form.kind"),
                &labels,
                cur,
                want(ItemField::Kind),
            ) {
                let k = kinds[i];
                panel.form.edit(F_KIND, |x| x.kind = k);
            }
            if !item_types {
                pane::pending(ui, tokens, &t("items.form.kinds_later"));
            }
            let mut name = d.name.clone();
            let r = pane::text_row(
                ui,
                field_id(ItemField::Name),
                &t("items.form.name"),
                &mut name,
                want(ItemField::Name),
            );
            if r.changed() {
                panel.form.edit(F_NAME, |x| x.name = name);
            }
            if enter_left(ui, &r) {
                save = Some(SaveIntent::SaveAndNew);
            }
            let mut description = d.description.clone();
            let r = pane::text_row(
                ui,
                field_id(ItemField::Description),
                &t("items.form.description"),
                &mut description,
                want(ItemField::Description),
            );
            if r.changed() {
                panel
                    .form
                    .edit(F_DESCRIPTION, |x| x.description = description);
            }
            let mut price = d.price.clone();
            let r = pane::text_row(
                ui,
                field_id(ItemField::Price),
                &t("items.form.price"),
                &mut price,
                want(ItemField::Price),
            );
            if r.changed() {
                panel.form.edit(F_PRICE, |x| x.price = price);
            }
            if enter_left(ui, &r) {
                save = Some(SaveIntent::SaveAndNew);
            }
            let cur = d
                .income_account
                .and_then(|a| income_ids.iter().position(|x| *x == a))
                .unwrap_or(0);
            if let Some(i) = pane::picker(
                ui,
                field_id(ItemField::IncomeAccount),
                &t("items.form.income_account"),
                &income_labels,
                cur,
                want(ItemField::IncomeAccount),
            ) {
                let a = income_ids[i];
                panel.form.edit(F_ACCOUNT, |x| x.income_account = Some(a));
            }
            let mut taxable = d.taxable;
            let r = ui.checkbox(&mut taxable, t("items.form.taxable"));
            if want(ItemField::Taxable) {
                r.request_focus();
            }
            if r.changed() {
                panel.form.edit(F_TAXABLE, |x| x.taxable = taxable);
            }
            ui.add_space(4.0);
            if !item_types {
                pane::pending(ui, tokens, &t("items.form.purchase_later"));
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
            form::progress_line(ui, tokens, &t("items.form.saving"));
        }
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if crate::widgets::primary_button(ui, &t("items.form.save_close"))
                .on_hover_text(shortcut_hint("form.save_close"))
                .clicked()
            {
                save = Some(SaveIntent::SaveAndClose);
            }
            if ui
                .button(t("items.form.save_new"))
                .on_hover_text(shortcut_hint("form.save_new"))
                .clicked()
            {
                save = Some(SaveIntent::SaveAndNew);
            }
            if ui.button(t("items.form.cancel")).clicked() {
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

    fn close_panel(&mut self) {
        let keep = self.selected();
        self.panel = None;
        self.sync_compact();
        self.place_cursor(keep);
    }
}

/// Saves into the demo items at once (no engine).
pub(crate) fn demo_save(
    link: &CompanyLink,
    existing: Option<Item>,
    fields: ItemFields,
) -> Versioned<Item> {
    let mut d = link.demo_state();
    let item = if let Some(mut i) = existing {
        i.fields = fields;
        i
    } else {
        let n = d.as_mut().map_or(0, |d| d.items_mut().len());
        Item {
            id: crate::company::demo::lists::item_id(900_000 + u32::try_from(n).unwrap_or(0)),
            fields,
            is_active: true,
            default_tag_id: None,
            created_at: Some(crate::company::demo::DemoState::now()),
            updated_at: Some(crate::company::demo::DemoState::now()),
        }
    };
    if let Some(d) = d.as_mut() {
        let items = d.items_mut();
        match items.iter_mut().find(|i| i.id == item.id) {
            Some(i) => *i = item.clone(),
            None => items.push(item.clone()),
        }
    }
    Versioned {
        version: RecordVersion::Fingerprint(item.fingerprint()),
        record: item,
    }
}

/// True when Enter was pressed to leave `r`.
fn enter_left(ui: &Ui, r: &egui::Response) -> bool {
    r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

impl Screen for ItemsScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        self.parts = EnvParts::capture(ui.ctx());
        self.poll();
        self.sync_compact();
        let tokens = Tokens::of(ui.ctx());
        form::heading(ui, &tokens, &t("cmd.lists.items"));
        ui.add_space(4.0);
        self.toolbar(ui);
        self.messages(ui, &tokens);
        ui.add_space(4.0);
        if !self.loaded {
            form::progress_line(ui, &tokens, &t("items.loading"));
            return;
        }
        if self.panel.is_some() {
            ui.horizontal_top(|ui| {
                let left = (ui.available_width() - PANEL_WIDTH - 16.0).max(300.0);
                ui.vertical(|ui| {
                    ui.set_width(left);
                    self.list_ui(ui, &tokens);
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(PANEL_WIDTH);
                    egui::ScrollArea::vertical()
                        .id_salt("items-form-scroll")
                        .show(ui, |ui| self.form_ui(ui, &tokens));
                });
            });
        } else {
            self.list_ui(ui, &tokens);
        }
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
            "edit.find" => self.focus_search = true,
            "form.save_new" | "form.save_close" => {
                if self.panel.is_none() {
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
                let Some(p) = self.panel.as_mut() else {
                    return false;
                };
                if id.as_str() == "edit.undo" {
                    p.form.undo();
                } else {
                    p.form.redo();
                }
            }
            "form.revert" => match self.panel.as_mut() {
                Some(p) if p.form.is_dirty() && p.task.is_none() => {
                    p.form.revert_form();
                    p.problems.clear();
                    p.error = None;
                    p.focus = Some(ItemField::Name);
                }
                Some(_) => self.close_panel(),
                None if !self.filter.search.is_empty() => {
                    self.filter.search.clear();
                    let keep = self.selected();
                    self.refilter();
                    self.place_cursor(keep);
                }
                None => return false,
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
        self.save_form(SaveIntent::Save)
    }

    fn state(&self) -> Value {
        let selected = self.selected().and_then(|id| self.list.entry(id));
        let (dirty, problems, error, saving) = match &self.panel {
            Some(p) => (
                p.form.is_dirty(),
                p.problems
                    .iter()
                    .map(|p| p.key().as_str().rsplit('.').next().unwrap_or_default())
                    .collect::<Vec<_>>(),
                p.error.is_some(),
                p.task.is_some(),
            ),
            None => (false, Vec::new(), false, false),
        };
        let panel = match &self.panel {
            None => "none",
            Some(p) if p.editing.is_some() => "edit",
            Some(_) => "new",
        };
        json!({
            "screen": "items",
            "loaded": self.loaded,
            "load_error": self.load_error.is_some(),
            "rows": self.model.len(),
            "total": self.list.len(),
            "show_inactive": self.filter.show_inactive,
            "searching": !self.filter.search.is_empty(),
            "cursor_row": self.snap.cursor_row,
            "focused": self.snap.focused,
            "selected_active": selected.map(|e| e.active),
            "selected_price": selected.and_then(|e| e.price).map(cl_core::Money::to_wire),
            "panel": panel,
            "dirty": dirty,
            "problem": problems.first(),
            "problems": problems,
            "error": error,
            "saving": saving,
            "toggling": self.toggle.is_some(),
            "warning": self.warn.map(|w| w.rsplit('.').next().unwrap_or_default()),
            "note": self.note.map(|n| n.rsplit('.').next().unwrap_or_default()),
            "kinds_available": ItemKind::available(self.item_types).len(),
            "price_levels_available": self.price_levels,
        })
    }
}

#[cfg(test)]
mod tests;
