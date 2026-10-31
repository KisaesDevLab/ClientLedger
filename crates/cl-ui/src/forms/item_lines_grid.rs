//! The item lines grid (P17, plan §3 WP4): Enter Bill's Items tab (shown
//! only with `bill-lines-v1`, Q-P17c). It lives beside the shared
//! [`super::lines_grid`] so the screens using it save through
//! `GridForm::begin_save` like every grid form.
//!
//! cl-grid's `line_entry` preset with Item · Description · Qty · Cost ·
//! Amount · Account · Customer / Project · Billable · Tags over
//! `cl_model::forms::item_lines` (WP3). Amount is computed and read-only
//! when Qty and Cost are both set (D6): `cost × qty` rounded half away from
//! zero to cents, through `ItemLine::set_qty` / `set_rate`; the grid never
//! does the arithmetic itself. Items have no purchase account yet (U-39), so
//! a picked item's line takes the vendor's default account when it has none
//! (D5); the owner can change it.

use std::borrow::Cow;
use std::sync::Arc;

use cl_api::Item;
use cl_core::UserInput;
use cl_core::ids::{AccountId, ItemId};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridHeight, GridOutput, GridSpec, Line, ListId, LookupResult, Lookups, RecordOutcome,
    RowKey, RowKeyGen, RowModel, RowState, Source, Suggestion, TagChip, TextKey, WidthSpec,
};
use cl_model::CommandId;
use cl_model::amount_entry::{AmountKind, AmountRules, Sign};
use cl_model::forms::FormState;
use cl_model::forms::item_lines::{ItemLine, ItemLines};
use cl_model::lookup::{LookupEntry, LookupIndex};
use eframe::egui::Ui;

use crate::forms::lines_grid::LineLists;
use crate::grid_form::{GridForm, LinesAdapter, Routed};
use crate::i18n::t;
use crate::screens::register::lists::{ACCOUNTS, NAMES, RegisterLists, TAGS};

/// Item.
pub const I_ITEM: ColumnId = ColumnId(11);
/// Description.
pub const I_DESC: ColumnId = ColumnId(12);
/// Quantity.
pub const I_QTY: ColumnId = ColumnId(13);
/// Cost (the rate).
pub const I_COST: ColumnId = ColumnId(14);
/// Amount.
pub const I_AMOUNT: ColumnId = ColumnId(15);
/// Account (D5).
pub const I_ACCOUNT: ColumnId = ColumnId(16);
/// Customer / Project.
pub const I_CUSTOMER: ColumnId = ColumnId(17);
/// Billable.
pub const I_BILLABLE: ColumnId = ColumnId(18);
/// Tags (one per line, Q-233).
pub const I_TAG: ColumnId = ColumnId(19);

/// The items list.
pub const ITEMS: ListId = ListId(4);

const PICK_FROM_LIST: TextKey = TextKey("register.entry.pick_from_list");
const BAD_VALUE: TextKey = TextKey("register.entry.bad_value");
const SAVING: TextKey = TextKey("grid.form.saving");
const COMPUTED: TextKey = TextKey("bill.items.amount_computed");

/// Quantities: up to four decimals, zero or more as typed (the form refuses
/// zero).
const QTY: AmountRules = AmountRules {
    kind: AmountKind::Quantity,
    input: UserInput::FULL,
    sign: Sign::NonNegative,
};
/// Cost per unit: money with up to four decimals, zero or more.
const COST: AmountRules = AmountRules {
    kind: AmountKind::Money,
    input: UserInput::FULL,
    sign: Sign::NonNegative,
};

/// The Items grid's pickers: the register's lists plus the items.
#[derive(Default)]
pub struct ItemLists {
    base: Arc<RegisterLists>,
    ids: Vec<ItemId>,
    labels: Vec<String>,
    descriptions: Vec<Option<String>>,
    index: Option<LookupIndex>,
}

impl ItemLists {
    /// The register's lists and the company's items.
    #[must_use]
    pub fn new(base: Arc<RegisterLists>, items: &[Item]) -> ItemLists {
        let mut ids = Vec::new();
        let mut labels = Vec::new();
        let mut descriptions = Vec::new();
        let mut entries = Vec::new();
        for (i, it) in items.iter().enumerate() {
            ids.push(it.id);
            labels.push(it.fields.name.clone());
            descriptions.push(it.fields.description.clone());
            entries.push(LookupEntry {
                id: i as u64 + 1,
                number: None,
                full_name: it.fields.name.clone(),
                active: it.is_active,
            });
        }
        ItemLists {
            base,
            ids,
            labels,
            descriptions,
            index: Some(LookupIndex::new(entries)),
        }
    }

    /// The register's lists under the items (accounts, names, tags).
    #[must_use]
    pub fn base(&self) -> Arc<RegisterLists> {
        Arc::clone(&self.base)
    }

    /// The item behind a picker id.
    #[must_use]
    pub fn item(&self, key: u64) -> Option<ItemId> {
        let i = usize::try_from(key.checked_sub(1)?).ok()?;
        self.ids.get(i).copied()
    }

    /// The picker id of an item.
    #[must_use]
    pub fn item_key(&self, id: ItemId) -> Option<u64> {
        self.ids.iter().position(|x| *x == id).map(|i| i as u64 + 1)
    }

    /// An item's own description (the line's default).
    #[must_use]
    pub fn description(&self, id: ItemId) -> Option<&str> {
        let i = self.ids.iter().position(|x| *x == id)?;
        self.descriptions.get(i)?.as_deref()
    }

    fn label(&self, key: u64) -> Option<&str> {
        let i = usize::try_from(key.checked_sub(1)?).ok()?;
        self.labels.get(i).map(String::as_str)
    }
}

impl Lookups for ItemLists {
    fn suggest(&self, list: ListId, query: &str, limit: usize) -> LookupResult {
        if list != ITEMS {
            return self.base.suggest(list, query, limit);
        }
        let Some(index) = &self.index else {
            return LookupResult::Ready(Vec::new());
        };
        LookupResult::Ready(
            index
                .rank(query, limit)
                .into_iter()
                .filter_map(|r| {
                    self.label(r.id).map(|label| Suggestion {
                        id: r.id,
                        label: label.to_string(),
                        group: None,
                    })
                })
                .collect(),
        )
    }

    fn exact(&self, list: ListId, query: &str) -> Option<Suggestion> {
        if list != ITEMS {
            return self.base.exact(list, query);
        }
        let e = self.index.as_ref()?.exact(query)?;
        self.label(e.id).map(|label| Suggestion {
            id: e.id,
            label: label.to_string(),
            group: None,
        })
    }

    fn display(&self, list: ListId, id: u64) -> Option<Cow<'_, str>> {
        if list != ITEMS {
            return self.base.display(list, id);
        }
        self.label(id).map(Cow::Borrowed)
    }

    fn all(&self, list: ListId) -> LookupResult {
        self.suggest(list, "", 200)
    }
}

fn column(id: ColumnId, header: &'static str, kind: CellKind, width: (u16, u16, u8)) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        empty: None,
        help: None,
        kind,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: width.0,
            preferred: width.1,
            flex: width.2,
        },
        editable: true,
        mono: false,
    }
}

/// The Items grid's columns (Q-P17c).
#[must_use]
pub fn grid_spec() -> GridSpec {
    let picker = |list| CellKind::Picker {
        list,
        add_new: false,
    };
    let number = |c: Column| Column {
        align: Align::Right,
        mono: true,
        ..c
    };
    GridSpec::line_entry(vec![
        Column {
            placeholder: Some(TextKey("bill.items.col.item")),
            ..column(I_ITEM, "bill.items.col.item", picker(ITEMS), (110, 160, 1))
        },
        column(
            I_DESC,
            "bill.items.col.description",
            CellKind::Text { max_chars: 500 },
            (110, 180, 2),
        ),
        number(column(
            I_QTY,
            "bill.items.col.qty",
            CellKind::Amount(QTY),
            (60, 70, 0),
        )),
        number(column(
            I_COST,
            "bill.items.col.cost",
            CellKind::Amount(COST),
            (80, 90, 0),
        )),
        number(column(
            I_AMOUNT,
            "lines.col.amount",
            CellKind::Amount(AmountRules::MONEY),
            (90, 100, 0),
        )),
        column(
            I_ACCOUNT,
            "lines.col.account",
            picker(ACCOUNTS),
            (120, 180, 1),
        ),
        Column {
            empty: Some(TextKey("lines.empty")),
            ..column(
                I_CUSTOMER,
                "lines.col.customer_project",
                picker(NAMES),
                (110, 150, 1),
            )
        },
        Column {
            align: Align::Center,
            ..column(
                I_BILLABLE,
                "lines.col.billable",
                CellKind::Checkbox,
                (60, 70, 0),
            )
        },
        column(
            I_TAG,
            "lines.col.tags",
            CellKind::Tags { list: TAGS },
            (90, 120, 1),
        ),
    ])
}

/// Row keys, the lists, the locked columns, the vendor's default account
/// and the Esc snapshot of the line being edited.
pub struct ItemsState {
    /// Key generator.
    pub keys: RowKeyGen,
    /// Picker lists.
    pub lists: Arc<ItemLists>,
    /// Columns the owner can't change right now.
    pub locked: Vec<ColumnId>,
    /// The account a picked item's line takes when it has none (D5).
    pub default_account: Option<AccountId>,
    snapshot: Option<ItemLine>,
}

struct View<'a> {
    form: &'a mut FormState<ItemLines>,
    a: &'a mut ItemsState,
    chips: Vec<Option<TagChip>>,
}

impl LinesAdapter<ItemLines> for ItemsState {
    fn model<'a>(&'a mut self, form: &'a mut FormState<ItemLines>) -> Box<dyn EditableModel + 'a> {
        let lists = &self.lists.base;
        let chips = form
            .current()
            .lines
            .iter()
            .map(|l| {
                l.tag
                    .and_then(|g| lists.tag_key(g))
                    .and_then(|k| LineLists::tag_chip(&**lists, k))
            })
            .collect();
        Box::new(View {
            form,
            a: self,
            chips,
        })
    }
}

impl View<'_> {
    fn lines(&self) -> &[ItemLine] {
        &self.form.current().lines
    }
}

fn picked<L: Lookups + ?Sized>(lists: &L, list: ListId, id: Option<u64>) -> CellValue<'_> {
    id.map_or(CellValue::Empty, |id| CellValue::Ref {
        id,
        display: lists.display(list, id).unwrap_or(Cow::Borrowed("")),
    })
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        self.form.revision()
    }
    fn row_count(&self) -> usize {
        self.lines().len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.lines().get(index).map(|l| l.key)
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.lines().iter().position(|l| l.key == key)
    }
    fn row_state(&self, index: usize) -> RowState {
        match self.lines().get(index) {
            Some(l) if index + 1 == self.lines().len() && l.is_blank() => RowState::ENTRY,
            _ => RowState::empty(),
        }
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(l) = self.lines().get(index) else {
            return CellValue::Empty;
        };
        let lists = &*self.a.lists;
        match col {
            I_ITEM => picked(lists, ITEMS, l.item.and_then(|i| lists.item_key(i))),
            I_DESC => CellValue::Text(Cow::Borrowed(&l.description)),
            I_QTY => l.qty.map_or(CellValue::Empty, CellValue::Quantity),
            I_COST => l.rate.map_or(CellValue::Empty, CellValue::Money),
            I_AMOUNT => l.amount.map_or(CellValue::Empty, CellValue::Money),
            I_ACCOUNT => picked(
                lists,
                ACCOUNTS,
                l.account.and_then(|a| lists.base.account_key(a)),
            ),
            I_CUSTOMER => picked(
                lists,
                NAMES,
                l.customer.and_then(|c| lists.base.name_key(c)),
            ),
            I_BILLABLE => CellValue::Bool(l.billable),
            I_TAG => match self.chips.get(index) {
                Some(Some(chip)) => CellValue::Tags(std::slice::from_ref(chip)),
                _ => CellValue::Tags(&[]),
            },
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

/// Puts one committed cell into an item line. Qty and Cost go through the
/// line's setters, so a computed Amount follows them (D6).
pub(crate) fn apply(
    l: &mut ItemLine,
    col: ColumnId,
    value: CellInput,
    lists: &ItemLists,
    default_account: Option<AccountId>,
) -> Option<TextKey> {
    match (col, value) {
        (I_ITEM, CellInput::Ref(id)) => match lists.item(id) {
            Some(item) => {
                l.item = Some(item);
                if l.description.trim().is_empty() {
                    l.description = lists.description(item).unwrap_or_default().to_string();
                }
                if l.account.is_none() {
                    l.account = default_account;
                }
            }
            None => return Some(PICK_FROM_LIST),
        },
        (I_ACCOUNT, CellInput::Ref(id)) => match lists.base.account(id) {
            Some(a) => l.account = Some(a),
            None => return Some(PICK_FROM_LIST),
        },
        (I_CUSTOMER, CellInput::Ref(id)) => match lists.base.name(id) {
            Some(c) => l.customer = Some(c),
            None => return Some(PICK_FROM_LIST),
        },
        (I_TAG, CellInput::Tags(chips)) => match chips.last() {
            None => l.tag = None,
            Some(chip) => match lists.base.tag(chip.id) {
                Some(g) => l.tag = Some(g),
                None => return Some(PICK_FROM_LIST),
            },
        },
        (I_ITEM | I_ACCOUNT | I_CUSTOMER | I_TAG, CellInput::Unresolved(_)) => {
            return Some(PICK_FROM_LIST);
        }
        (I_ITEM, CellInput::Clear) => l.item = None,
        (I_ACCOUNT, CellInput::Clear) => l.account = None,
        (I_CUSTOMER, CellInput::Clear) => l.customer = None,
        (I_TAG, CellInput::Clear) => l.tag = None,
        (I_DESC, CellInput::Text(v)) => l.description = v,
        (I_DESC, CellInput::Clear) => l.description.clear(),
        (I_QTY, CellInput::Quantity(q)) => l.set_qty(Some(q)),
        (I_QTY, CellInput::Clear) => l.set_qty(None),
        (I_COST, CellInput::Money(m)) => l.set_rate(Some(m)),
        (I_COST, CellInput::Clear) => l.set_rate(None),
        (I_AMOUNT, CellInput::Money(m)) => {
            if !l.set_amount(Some(m)) {
                return Some(COMPUTED);
            }
        }
        (I_AMOUNT, CellInput::Clear) => {
            if !l.set_amount(None) {
                return Some(COMPUTED);
            }
        }
        (I_BILLABLE, CellInput::Bool(b)) => l.billable = b,
        (I_BILLABLE, CellInput::Clear) => l.billable = false,
        _ => return Some(BAD_VALUE),
    }
    None
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        let Some(l) = self.lines().get(index) else {
            return false;
        };
        !self.a.locked.contains(&col) && (col != I_AMOUNT || !l.amount_is_computed())
    }
    fn row_entered(&mut self, key: RowKey) {
        self.a.snapshot = self.lines().iter().find(|l| l.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        _source: Source,
    ) -> Result<(), CellProblem> {
        if self.a.locked.contains(&pos.col) {
            return Err(CellProblem::new(BAD_VALUE).at(pos.col));
        }
        let lists = Arc::clone(&self.a.lists);
        let default_account = self.a.default_account;
        let keys = &mut self.a.keys;
        let mut problem = None;
        let applied = self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == pos.row) {
                problem = apply(l, pos.col, value, &lists, default_account);
            }
            v.ensure_entry(keys);
        });
        if !applied {
            return Err(CellProblem::new(SAVING).at(pos.col));
        }
        problem.map_or(Ok(()), |k| Err(CellProblem::new(k).at(pos.col)))
    }
    fn revert_row(&mut self, key: RowKey) -> bool {
        let Some(snap) = self.a.snapshot.clone().filter(|s| s.key == key) else {
            return false;
        };
        if self.lines().contains(&snap) {
            return false;
        }
        let keys = &mut self.a.keys;
        self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == key) {
                *l = snap;
            }
            v.ensure_entry(keys);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.a.keys.next_key();
        self.form
            .edit_step(|v| {
                let at = before
                    .and_then(|b| v.lines.iter().position(|l| l.key == b))
                    .unwrap_or(v.lines.len());
                v.lines.insert(at, ItemLine::blank(key));
            })
            .then_some(key)
    }
    fn delete_line(&mut self, key: RowKey) -> bool {
        if self.index_of(key).is_none() {
            return false;
        }
        let keys = &mut self.a.keys;
        self.form.edit_step(|v| {
            v.lines.retain(|l| l.key != key);
            v.ensure_entry(keys);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        if self.lines()[i].account.is_none() {
            return RecordOutcome::Invalid(vec![
                CellProblem::new(TextKey("lines.need_account")).at(I_ACCOUNT),
            ]);
        }
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

/// The keymap commands the Items grid takes while it has the keyboard.
const GRID_COMMANDS: &[&str] = &[
    "form.revert",
    "edit.undo",
    "edit.redo",
    "edit.open_list",
    "form.insert_line",
    "form.delete_line",
];

/// The bill's item lines on the grid.
pub struct ItemsGrid {
    form: GridForm<ItemLines, ItemsState>,
}

impl ItemsGrid {
    /// An empty grid (one blank entry line).
    ///
    /// # Panics
    /// Never: the spec is fixed and covered by tests.
    #[must_use]
    pub fn new(lists: Arc<ItemLists>) -> ItemsGrid {
        let mut keys = RowKeyGen::new();
        let value = ItemLines::from_lines(Vec::new(), &mut keys);
        // The spec is fixed; `the_items_spec_is_valid` covers it.
        let grid = Grid::new("enter_bill/items", grid_spec()).expect("the items spec is valid");
        ItemsGrid {
            form: GridForm::new(
                FormState::new(value),
                ItemsState {
                    keys,
                    lists,
                    locked: Vec::new(),
                    default_account: None,
                    snapshot: None,
                },
                grid,
                t("bill.items.label"),
                GridHeight::Rows { min: 4, max: 10 },
            ),
        }
    }

    /// The grid form (saves, loads and header-driven edits).
    pub fn form_mut(&mut self) -> &mut GridForm<ItemLines, ItemsState> {
        &mut self.form
    }

    /// The grid has the keyboard.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.form.grid().has_focus()
    }

    /// The lines.
    #[must_use]
    pub fn value(&self) -> &ItemLines {
        self.form.form().current()
    }

    /// Unsaved edits.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.form.dirty()
    }

    /// Commits the open cell. False when it is not valid.
    pub fn commit(&mut self) -> bool {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.commit_pending(&*lists).is_ok()
    }

    /// The screen replaced the lines (see `GridForm::rebind_view`).
    pub fn lines_replaced(&mut self) {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.rebind_view(&*lists);
    }

    /// The pickers changed.
    pub fn set_lists(&mut self, lists: Arc<ItemLists>) {
        self.form.adapter_mut().lists = lists;
    }

    /// The pickers.
    #[must_use]
    pub fn lists(&self) -> Arc<ItemLists> {
        Arc::clone(&self.form.adapter().lists)
    }

    /// Makes `cols` read-only (all others editable).
    pub fn set_locked(&mut self, cols: Vec<ColumnId>) {
        self.form.adapter_mut().locked = cols;
    }

    /// The account a picked item's line takes when it has none (D5).
    pub fn set_default_account(&mut self, account: Option<AccountId>) {
        self.form.adapter_mut().default_account = account;
    }

    /// The row keys (fresh lines for a load).
    pub fn keys(&mut self) -> &mut RowKeyGen {
        &mut self.form.adapter_mut().keys
    }

    /// Replaces the lines (a load, Clear): not an edit.
    pub fn load(&mut self, lines: ItemLines) {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.load(lines, &*lists);
    }

    /// Puts the cursor on the first line's Item cell.
    pub fn focus_first(&mut self) {
        let Some(key) = self.value().lines.first().map(|l| l.key) else {
            return;
        };
        self.focus(key, I_ITEM);
    }

    /// Puts the cursor on `col` of line `key`.
    pub fn focus(&mut self, key: RowKey, col: ColumnId) {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.focus(CellPos { row: key, col }, &*lists);
    }

    /// A keymap command while the grid has the keyboard: `Some(true)` when
    /// the grid took it.
    pub fn command(&mut self, id: CommandId) -> Option<bool> {
        if !self.has_focus() || !GRID_COMMANDS.contains(&id.as_str()) {
            return None;
        }
        let lists = Arc::clone(&self.form.adapter().lists);
        match self.form.command(id, &*lists) {
            Routed::Handled => Some(true),
            _ => None,
        }
    }

    /// Draws the grid.
    pub fn ui(&mut self, ui: &mut Ui) -> GridOutput {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.ui(ui, &*lists)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::{Money, Quantity};

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    #[test]
    fn the_items_spec_is_valid() {
        let spec = grid_spec();
        assert_eq!(spec.validate(), Ok(()));
        let ids: Vec<ColumnId> = spec.columns.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                I_ITEM, I_DESC, I_QTY, I_COST, I_AMOUNT, I_ACCOUNT, I_CUSTOMER, I_BILLABLE, I_TAG
            ]
        );
        for c in &spec.columns {
            assert!(crate::i18n::has(c.header.0), "{}", c.header.0);
        }
        assert!(crate::i18n::has(COMPUTED.0));
        assert!(crate::i18n::has("bill.items.label"));
    }

    #[test]
    fn qty_and_cost_compute_the_amount_and_lock_it() {
        let lists = ItemLists::default();
        let mut l = ItemLine::blank(RowKeyGen::new().next_key());
        let q = Quantity::parse_wire("2").expect("q");
        assert_eq!(
            apply(&mut l, I_QTY, CellInput::Quantity(q), &lists, None),
            None
        );
        assert_eq!(
            apply(
                &mut l,
                I_COST,
                CellInput::Money(money("665.0049")),
                &lists,
                None
            ),
            None
        );
        // 2 × 665.0049 = 1,330.0098 → 1,330.01 (half away from zero).
        assert_eq!(l.amount, Some(money("1330.01")));
        assert_eq!(
            apply(&mut l, I_AMOUNT, CellInput::Money(money("1")), &lists, None),
            Some(COMPUTED),
            "read-only while computed"
        );
        assert_eq!(apply(&mut l, I_COST, CellInput::Clear, &lists, None), None);
        assert_eq!(
            apply(
                &mut l,
                I_AMOUNT,
                CellInput::Money(money("150")),
                &lists,
                None
            ),
            None
        );
        assert_eq!(l.amount, Some(money("150")));
        assert_eq!(
            apply(&mut l, I_ITEM, CellInput::Ref(9), &lists, None),
            Some(PICK_FROM_LIST)
        );
        assert_eq!(
            apply(
                &mut l,
                I_BILLABLE,
                CellInput::Money(money("1")),
                &lists,
                None
            ),
            Some(BAD_VALUE)
        );
    }

    #[test]
    fn a_picked_item_takes_its_description_and_the_default_account() {
        let items: Vec<Item> = serde_json::from_value(serde_json::json!([{
            "id": "d0000000-0000-4000-8000-f00000000001",
            "name": "Excavator day",
            "description": "Mini excavator, per day",
            "incomeAccountId": "d0000000-0000-4000-8000-a00000006210",
            "isActive": true,
        }]))
        .expect("items");
        let lists = ItemLists::new(Arc::new(RegisterLists::default()), &items);
        assert!(matches!(lists.suggest(ITEMS, "exc", 5), LookupResult::Ready(v) if v.len() == 1));
        let key = lists.item_key(items[0].id).expect("key");
        let account = crate::company::demo::account_id(6210);
        let mut l = ItemLine::blank(RowKeyGen::new().next_key());
        assert_eq!(
            apply(&mut l, I_ITEM, CellInput::Ref(key), &lists, Some(account)),
            None
        );
        assert_eq!(l.item, Some(items[0].id));
        assert_eq!(l.description, "Mini excavator, per day");
        assert_eq!(l.account, Some(account));
        let g = ItemsGrid::new(Arc::new(lists));
        assert_eq!(g.value().lines.len(), 1, "one blank entry line");
        assert!(!g.dirty());
    }
}
