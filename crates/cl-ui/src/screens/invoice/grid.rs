//! The invoice lines grid (P20 WP-B step 1), on the `forms/lines_grid.rs`
//! pattern: cl-grid's `line_entry` preset over the lines of
//! `cl_model::forms::invoice::InvoiceForm`.
//!
//! Columns: Item · Description (several lines) · Service date · Qty · Rate ·
//! Amount · Tax · Income account. **No money arithmetic here** (Q-P20n):
//! Amount is display-only and shows the engine's preview figure, or
//! "Updating…" while a newer preview is pending, or "Can't calculate"; a
//! blank Rate shows the engine's rate. Qty and Rate of a billable cost are
//! the source's and read-only (Q-P20p).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use cl_core::ids::ItemId;
use cl_core::{Money, UserInput};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridHeight, GridOutput, GridSpec, Line, ListId, Lookups, RecordOutcome, RowKey,
    RowKeyGen, RowModel, RowState, Source, TextKey, WidthSpec,
};
use cl_model::CommandId;
use cl_model::amount_entry::{AmountKind, AmountRules, Sign};
use cl_model::forms::FormState;
use cl_model::forms::invoice::{InvoiceForm, InvoiceLine, LineView, PreviewTracker, RateSource};
use eframe::egui::Ui;

use crate::forms::item_lines_grid::{ITEMS, ItemLists};
use crate::forms::lines_grid::LineLists;
use crate::grid_form::{GridForm, LinesAdapter, Routed};
use crate::i18n::t;
use crate::screens::register::lists::ACCOUNTS;

/// Item.
pub const V_ITEM: ColumnId = ColumnId(31);
/// Description.
pub const V_DESC: ColumnId = ColumnId(32);
/// Service date.
pub const V_DATE: ColumnId = ColumnId(33);
/// Quantity.
pub const V_QTY: ColumnId = ColumnId(34);
/// Rate.
pub const V_RATE: ColumnId = ColumnId(35);
/// Amount (the engine's).
pub const V_AMOUNT: ColumnId = ColumnId(36);
/// Taxable.
pub const V_TAX: ColumnId = ColumnId(37);
/// Income account (blank: the item's).
pub const V_ACCOUNT: ColumnId = ColumnId(38);

const PICK_FROM_LIST: TextKey = TextKey("register.entry.pick_from_list");
const BAD_VALUE: TextKey = TextKey("register.entry.bad_value");
const SAVING: TextKey = TextKey("grid.form.saving");
const LOCKED: TextKey = TextKey("invoice.grid.billable_locked");
const NEED_ITEM: TextKey = TextKey("invoice.grid.need_item");

/// Quantities: four places, either sign.
const QTY: AmountRules = AmountRules {
    kind: AmountKind::Quantity,
    input: UserInput::FULL,
    sign: Sign::Any,
};
/// Rates: money to four places, either sign (a credit line).
const RATE: AmountRules = AmountRules {
    kind: AmountKind::Money,
    input: UserInput::FULL,
    sign: Sign::Any,
};

/// What a line's Rate and Amount cells show (copied from the
/// [`PreviewTracker`] before each frame).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// Not priced.
    Nothing,
    /// A newer preview is pending.
    Updating,
    /// The engine couldn't compute it.
    Cant,
    /// The engine's figures.
    Ready {
        /// The rate it used.
        rate: Option<Money>,
        /// Where it came from.
        source: RateSource,
        /// The line amount.
        amount: Money,
    },
}

impl Shown {
    fn of(v: LineView<'_>) -> Shown {
        match v {
            LineView::Nothing => Shown::Nothing,
            LineView::Updating => Shown::Updating,
            LineView::CantCalculate => Shown::Cant,
            LineView::Ready(f) => Shown::Ready {
                rate: f.unit_price,
                source: f.source,
                amount: f.amount,
            },
        }
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

/// The invoice grid's columns.
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
            placeholder: Some(TextKey("invoice.col.item")),
            ..column(V_ITEM, "invoice.col.item", picker(ITEMS), (110, 150, 1))
        },
        column(
            V_DESC,
            "invoice.col.description",
            CellKind::Memo {
                multiline: true,
                max_chars: 4000,
            },
            (140, 260, 3),
        ),
        Column {
            mono: true,
            ..column(
                V_DATE,
                "invoice.col.service_date",
                CellKind::Date,
                (90, 100, 0),
            )
        },
        number(column(
            V_QTY,
            "invoice.col.qty",
            CellKind::Amount(QTY),
            (50, 60, 0),
        )),
        number(column(
            V_RATE,
            "invoice.col.rate",
            CellKind::Amount(RATE),
            (80, 90, 0),
        )),
        Column {
            editable: false,
            ..number(column(
                V_AMOUNT,
                "invoice.col.amount",
                CellKind::Display,
                (90, 110, 0),
            ))
        },
        Column {
            align: Align::Center,
            ..column(V_TAX, "invoice.col.tax", CellKind::Checkbox, (44, 50, 0))
        },
        Column {
            empty: Some(TextKey("invoice.col.account.empty")),
            ..column(
                V_ACCOUNT,
                "invoice.col.account",
                picker(ACCOUNTS),
                (100, 140, 1),
            )
        },
    ])
}

/// Row keys, the lists, each item's taxable flag and the shown figures.
pub struct InvState {
    /// Key generator.
    pub keys: RowKeyGen,
    /// Picker lists.
    pub lists: Arc<ItemLists>,
    /// Each item's taxable flag (a line follows it until set).
    pub taxable: HashMap<ItemId, bool>,
    /// What each line's Rate and Amount show.
    pub shown: HashMap<RowKey, Shown>,
    updating: String,
    cant: String,
    snapshot: Option<InvoiceLine>,
}

struct View<'a> {
    form: &'a mut FormState<InvoiceForm>,
    a: &'a mut InvState,
}

impl LinesAdapter<InvoiceForm> for InvState {
    fn model<'a>(
        &'a mut self,
        form: &'a mut FormState<InvoiceForm>,
    ) -> Box<dyn EditableModel + 'a> {
        Box::new(View { form, a: self })
    }
}

impl View<'_> {
    fn lines(&self) -> &[InvoiceLine] {
        &self.form.current().lines
    }
}

fn picked<L: Lookups + ?Sized>(lists: &L, list: ListId, id: Option<u64>) -> CellValue<'_> {
    id.map_or(CellValue::Empty, |id| CellValue::Ref {
        id,
        display: lists.display(list, id).unwrap_or(Cow::Borrowed("")),
    })
}

/// The taxable flag a line shows: its own, else its item's.
#[must_use]
pub fn taxable_of(l: &InvoiceLine, taxable: &HashMap<ItemId, bool>) -> bool {
    l.taxable.unwrap_or_else(|| {
        l.item
            .and_then(|i| taxable.get(&i).copied())
            .unwrap_or(false)
    })
}

impl RowModel for View<'_> {
    fn revision(&self) -> u64 {
        // The shown figures change without an edit: fold them in.
        let mut h = self.form.revision();
        for (k, s) in &self.a.shown {
            let tag: u64 = match s {
                Shown::Nothing => 1,
                Shown::Updating => 2,
                Shown::Cant => 3,
                Shown::Ready { .. } => 5,
            };
            h = h
                .wrapping_mul(31)
                .wrapping_add(tag.wrapping_mul(k.0.wrapping_add(7)));
        }
        h
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
        let shown = self.a.shown.get(&l.key).copied().unwrap_or(Shown::Nothing);
        match col {
            V_ITEM => picked(lists, ITEMS, l.item.and_then(|i| lists.item_key(i))),
            V_DESC => CellValue::Text(Cow::Borrowed(&l.description)),
            V_DATE => l.service_date.map_or(CellValue::Empty, CellValue::Date),
            V_QTY => l.quantity.map_or(CellValue::Empty, CellValue::Quantity),
            V_RATE => match (l.rate, shown) {
                (Some(r), _) if l.billable.is_none() => CellValue::Money(r),
                (_, Shown::Ready { rate: Some(r), .. }) | (Some(r), _) => CellValue::Money(r),
                _ => CellValue::Empty,
            },
            V_AMOUNT => match shown {
                Shown::Nothing => CellValue::Empty,
                Shown::Updating => CellValue::Text(Cow::Borrowed(&self.a.updating)),
                Shown::Cant => CellValue::Text(Cow::Borrowed(&self.a.cant)),
                Shown::Ready { amount, .. } => CellValue::Money(amount),
            },
            V_TAX => {
                if l.is_blank() {
                    CellValue::Empty
                } else {
                    CellValue::Bool(taxable_of(l, &self.a.taxable))
                }
            }
            V_ACCOUNT => picked(
                lists,
                ACCOUNTS,
                l.account
                    .and_then(|a| LineLists::account_key(&*lists.base(), a)),
            ),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

/// Puts one committed cell into an invoice line (no arithmetic).
pub(crate) fn apply(
    l: &mut InvoiceLine,
    col: ColumnId,
    value: CellInput,
    lists: &ItemLists,
) -> Option<TextKey> {
    if l.amounts_locked() && matches!(col, V_QTY | V_RATE) {
        return Some(LOCKED);
    }
    match (col, value) {
        (V_ITEM, CellInput::Ref(id)) => match lists.item(id) {
            Some(item) => {
                if l.item != Some(item) {
                    l.taxable = None;
                }
                l.item = Some(item);
                if l.description.trim().is_empty() {
                    l.description = lists.description(item).unwrap_or_default().to_string();
                }
            }
            None => return Some(PICK_FROM_LIST),
        },
        (V_ACCOUNT, CellInput::Ref(id)) => match LineLists::account(&*lists.base(), id) {
            Some(a) => l.account = Some(a),
            None => return Some(PICK_FROM_LIST),
        },
        (V_ITEM | V_ACCOUNT, CellInput::Unresolved(_)) => return Some(PICK_FROM_LIST),
        (V_ITEM, CellInput::Clear) => l.item = None,
        (V_ACCOUNT, CellInput::Clear) => l.account = None,
        (V_DESC, CellInput::Text(v)) => l.description = v,
        (V_DESC, CellInput::Clear) => l.description.clear(),
        (V_DATE, CellInput::Date(d)) => l.service_date = Some(d),
        (V_DATE, CellInput::Clear) => l.service_date = None,
        (V_QTY, CellInput::Quantity(q)) => l.quantity = Some(q),
        (V_QTY, CellInput::Clear) => l.quantity = None,
        (V_RATE, CellInput::Money(m)) => l.rate = Some(m),
        (V_RATE, CellInput::Clear) => l.rate = None,
        (V_TAX, CellInput::Bool(b)) => l.taxable = Some(b),
        (V_TAX, CellInput::Clear) => l.taxable = Some(false),
        _ => return Some(BAD_VALUE),
    }
    None
}

impl EditableModel for View<'_> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        let Some(l) = self.lines().get(index) else {
            return false;
        };
        match col {
            V_AMOUNT => false,
            V_QTY | V_RATE => !l.amounts_locked(),
            _ => true,
        }
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
        let lists = Arc::clone(&self.a.lists);
        let keys = &mut self.a.keys;
        let mut problem = None;
        let applied = self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == pos.row) {
                problem = apply(l, pos.col, value, &lists);
            }
            v.ensure_entry_line(keys);
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
            v.ensure_entry_line(keys);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.a.keys.next_key();
        self.form
            .edit_step(|v| {
                let at = before
                    .and_then(|b| v.lines.iter().position(|l| l.key == b))
                    .unwrap_or(v.lines.len());
                v.lines.insert(at, InvoiceLine::blank(key));
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
            v.ensure_entry_line(keys);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        let l = &self.lines()[i];
        if l.item.is_none() && l.account.is_none() {
            return RecordOutcome::Invalid(vec![CellProblem::new(NEED_ITEM).at(V_ITEM)]);
        }
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

const GRID_COMMANDS: &[&str] = &[
    "form.revert",
    "edit.undo",
    "edit.redo",
    "edit.open_list",
    "form.insert_line",
    "form.delete_line",
];

/// The invoice's lines on the grid.
pub struct InvoiceGrid {
    form: GridForm<InvoiceForm, InvState>,
}

impl InvoiceGrid {
    /// An empty grid (one blank entry line).
    ///
    /// # Panics
    /// Never: the spec is fixed and covered by tests.
    #[must_use]
    pub fn new(lists: Arc<ItemLists>) -> InvoiceGrid {
        let mut keys = RowKeyGen::new();
        let value = InvoiceForm::new(None, &mut keys);
        // The spec is fixed; `the_invoice_spec_is_valid` covers it.
        let grid = Grid::new("invoice/lines", grid_spec()).expect("the invoice spec is valid");
        InvoiceGrid {
            form: GridForm::new(
                FormState::new(value),
                InvState {
                    keys,
                    lists,
                    taxable: HashMap::new(),
                    shown: HashMap::new(),
                    updating: t("invoice.updating"),
                    cant: t("invoice.cant_calculate"),
                    snapshot: None,
                },
                grid,
                t("invoice.lines.label"),
                GridHeight::Rows { min: 4, max: 14 },
            ),
        }
    }

    /// The grid has the keyboard.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.form.grid().has_focus()
    }

    /// The lines (in an otherwise blank form).
    #[must_use]
    pub fn value(&self) -> &InvoiceForm {
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

    /// The pickers and the items' taxable flags changed.
    pub fn set_lists(&mut self, lists: Arc<ItemLists>, taxable: HashMap<ItemId, bool>) {
        let a = self.form.adapter_mut();
        a.lists = lists;
        a.taxable = taxable;
    }

    /// The items' taxable flags.
    #[must_use]
    pub fn taxable(&self) -> &HashMap<ItemId, bool> {
        &self.form.adapter().taxable
    }

    /// Copies what each line shows from the tracker.
    pub fn show(&mut self, tracker: &PreviewTracker) {
        let shown: HashMap<RowKey, Shown> = self
            .value()
            .lines
            .iter()
            .map(|l| (l.key, Shown::of(tracker.line(l.key))))
            .collect();
        self.form.adapter_mut().shown = shown;
    }

    /// What line `key` shows.
    #[must_use]
    pub fn shown(&self, key: RowKey) -> Shown {
        self.form
            .adapter()
            .shown
            .get(&key)
            .copied()
            .unwrap_or(Shown::Nothing)
    }

    /// The row keys.
    pub fn keys(&mut self) -> &mut RowKeyGen {
        &mut self.form.adapter_mut().keys
    }

    /// Replaces the lines (a load, Clear): not an edit.
    pub fn load(&mut self, lines: Vec<InvoiceLine>) {
        let lists = Arc::clone(&self.form.adapter().lists);
        let mut v = InvoiceForm {
            lines,
            ..InvoiceForm::default()
        };
        v.ensure_entry_line(&mut self.form.adapter_mut().keys);
        self.form.load(v, &*lists);
    }

    /// One undoable edit of the lines (Add billable costs).
    pub fn edit(&mut self, f: impl FnOnce(&mut InvoiceForm, &mut RowKeyGen)) -> bool {
        let lists = Arc::clone(&self.form.adapter().lists);
        let mut v = self.value().clone();
        f(&mut v, &mut self.form.adapter_mut().keys);
        let done = self.form.edit_step(|x| *x = v);
        self.form.rebind_view(&*lists);
        done
    }

    /// Puts the cursor on `col` of line `key`.
    pub fn focus(&mut self, key: RowKey, col: ColumnId) {
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.focus(CellPos { row: key, col }, &*lists);
    }

    /// A keymap command while the grid has the keyboard.
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
    use cl_core::Quantity;
    use cl_model::rowkey::RowKeyGen as Keys;

    #[test]
    fn the_invoice_spec_is_valid() {
        let spec = grid_spec();
        assert_eq!(spec.validate(), Ok(()));
        for c in &spec.columns {
            assert!(crate::i18n::has(c.header.0), "{}", c.header.0);
        }
        for k in [LOCKED, NEED_ITEM] {
            assert!(crate::i18n::has(k.0), "{}", k.0);
        }
        assert!(crate::i18n::has("invoice.lines.label"));
        assert!(crate::i18n::has("invoice.col.account.empty"));
    }

    #[test]
    fn a_billable_line_keeps_its_quantity_and_rate() {
        let lists = ItemLists::default();
        let mut l = InvoiceLine::blank(Keys::new().next_key());
        let q = Quantity::parse_wire("2").expect("q");
        assert_eq!(apply(&mut l, V_QTY, CellInput::Quantity(q), &lists), None);
        l.billable = Some(cl_model::forms::invoice::BillableRef {
            source_line: cl_core::ids::JournalLineId::parse("d4000000-0000-4000-8000-000000000001")
                .expect("id"),
            markup_percent: None,
        });
        assert_eq!(
            apply(&mut l, V_RATE, CellInput::Money(Money::ZERO), &lists),
            Some(LOCKED)
        );
        assert_eq!(apply(&mut l, V_TAX, CellInput::Bool(false), &lists), None);
        assert_eq!(l.taxable, Some(false));
    }

    #[test]
    fn amounts_show_the_engines_figure_or_updating_never_a_local_sum() {
        let g = InvoiceGrid::new(Arc::new(ItemLists::default()));
        let key = g.value().lines[0].key;
        assert_eq!(g.shown(key), Shown::Nothing);
        let tracker = PreviewTracker::new();
        let mut g = g;
        g.show(&tracker);
        assert_eq!(g.shown(key), Shown::Nothing);
    }
}
