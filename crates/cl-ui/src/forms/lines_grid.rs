//! The lines grid every transaction form shares (P13 WP5.1): the register's
//! Splits panel and Write Check's Expenses tab edit the same
//! [`FormLines`] value on the `line_entry` grid preset.
//!
//! The columns are Account · Amount · Memo · Customer:Job · Billable · Tags;
//! a [`LinesSpec`] says which of the optional ones a screen shows and what
//! they are called. The register keeps its P12 columns (Account · Amount ·
//! Memo · Customer · Tag, one tag per line through a picker, Q-233) through
//! [`LinesSpec::REGISTER_SPLITS`].
//!
//! The grid only stores what the owner typed: it never adds anything up
//! (the remaining/total lines come from `cl_model::forms::lines`).

use std::borrow::Cow;
use std::sync::Arc;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_grid::{
    Align, CellInput, CellKind, CellPos, CellProblem, CellValue, Column, ColumnId, EditableModel,
    Grid, GridHeight, GridOutput, GridSpec, Line, ListId, Lookups, RecordOutcome, RowKey,
    RowKeyGen, RowModel, RowState, Source, TagChip, TextKey, WidthSpec,
};
use cl_model::CommandId;
use cl_model::amount_entry::AmountRules;
use cl_model::forms::FormState;
use cl_model::forms::lines::{FormLine, FormLines, ensure_entry};
use eframe::egui::Ui;

use crate::grid_form::{GridForm, LinesAdapter, Routed};
use crate::i18n::t;

/// Account.
pub const L_ACCOUNT: ColumnId = ColumnId(1);
/// Amount.
pub const L_AMOUNT: ColumnId = ColumnId(2);
/// Memo.
pub const L_MEMO: ColumnId = ColumnId(3);
/// Customer (or customer job).
pub const L_CUSTOMER: ColumnId = ColumnId(4);
/// Tag (one per line, Q-233).
pub const L_TAG: ColumnId = ColumnId(5);
/// Billable to the line's customer.
pub const L_BILLABLE: ColumnId = ColumnId(6);

const PICK_FROM_LIST: TextKey = TextKey("register.entry.pick_from_list");
const BAD_VALUE: TextKey = TextKey("register.entry.bad_value");
const SAVING: TextKey = TextKey("grid.form.saving");

/// The pickers behind a lines grid. A picker id is only meaningful inside
/// its window; these turn it back into the engine id (and back).
pub trait LineLists: Lookups {
    /// The accounts list.
    const ACCOUNTS: ListId;
    /// The customers (and jobs) list.
    const CUSTOMERS: ListId;
    /// The tags list.
    const TAGS: ListId;

    /// The account behind a picker id.
    fn account(&self, key: u64) -> Option<AccountId>;
    /// The picker id of an account.
    fn account_key(&self, id: AccountId) -> Option<u64>;
    /// The customer behind a picker id.
    fn customer(&self, key: u64) -> Option<ContactId>;
    /// The picker id of a customer.
    fn customer_key(&self, id: ContactId) -> Option<u64>;
    /// The tag behind a picker id.
    fn tag(&self, key: u64) -> Option<TagId>;
    /// The picker id of a tag.
    fn tag_key(&self, id: TagId) -> Option<u64>;

    /// The chip a tag shows as in a chips column (its group decides which
    /// chip a new pick replaces). Ungrouped by default.
    fn tag_chip(&self, key: u64) -> Option<TagChip> {
        self.display(Self::TAGS, key).map(|label| TagChip {
            id: key,
            label: label.into_owned(),
            group: None,
        })
    }
}

/// The column headers (i18n keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineHeaders {
    /// Account (also the entry line's placeholder).
    pub account: TextKey,
    /// Amount.
    pub amount: TextKey,
    /// Memo.
    pub memo: TextKey,
    /// Customer:Job.
    pub customer: TextKey,
    /// Billable.
    pub billable: TextKey,
    /// Tag(s).
    pub tags: TextKey,
}

/// Which optional columns a screen shows, and what the grid is called.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinesSpec {
    /// The grid's id (stable: the control channel finds it by it).
    pub grid_id: &'static str,
    /// The grid's accessible label (i18n key).
    pub label: &'static str,
    /// The Customer:Job column.
    pub customer: bool,
    /// The Billable column (a checkbox).
    pub billable: bool,
    /// The Tags column.
    pub tags: bool,
    /// The Tags column takes a chip (`CellKind::Tags`, one per line) rather
    /// than a picker.
    pub tag_chips: bool,
    /// Column headers.
    pub headers: LineHeaders,
    /// Shown when Enter records a line without an account.
    pub need_account: TextKey,
    /// How tall the grid is.
    pub height: GridHeight,
}

impl LinesSpec {
    /// The register's Splits panel (P12): Account · Amount · Memo ·
    /// Customer · Tag (a picker).
    pub const REGISTER_SPLITS: LinesSpec = LinesSpec {
        grid_id: "register/splits",
        label: "register.splits.label",
        customer: true,
        billable: false,
        tags: true,
        tag_chips: false,
        headers: LineHeaders {
            account: TextKey("register.entry.col.account"),
            amount: TextKey("register.splits.col.amount"),
            memo: TextKey("register.entry.col.memo"),
            customer: TextKey("register.splits.col.customer"),
            billable: TextKey("lines.col.billable"),
            tags: TextKey("register.splits.col.tag"),
        },
        need_account: TextKey("register.splits.need_account"),
        height: GridHeight::Rows { min: 3, max: 6 },
    };

    /// Every column (Write Check's Expenses tab): Account · Amount · Memo ·
    /// Customer:Job · Billable · Tags (one chip per line).
    #[must_use]
    pub const fn all(grid_id: &'static str, label: &'static str) -> LinesSpec {
        LinesSpec {
            grid_id,
            label,
            customer: true,
            billable: true,
            tags: true,
            tag_chips: true,
            headers: LineHeaders {
                account: TextKey("lines.col.account"),
                amount: TextKey("lines.col.amount"),
                memo: TextKey("lines.col.memo"),
                customer: TextKey("lines.col.customer_job"),
                billable: TextKey("lines.col.billable"),
                tags: TextKey("lines.col.tags"),
            },
            need_account: TextKey("lines.need_account"),
            height: GridHeight::Rows { min: 4, max: 10 },
        }
    }

    /// The grid spec for `L`'s lists.
    #[must_use]
    pub fn grid_spec<L: LineLists>(&self) -> GridSpec {
        let h = self.headers;
        let picker = |list| CellKind::Picker {
            list,
            add_new: false,
        };
        let mut columns = vec![
            Column {
                placeholder: Some(h.account),
                ..column(L_ACCOUNT, h.account, picker(L::ACCOUNTS), (120, 220, 2))
            },
            Column {
                align: Align::Right,
                mono: true,
                ..column(
                    L_AMOUNT,
                    h.amount,
                    CellKind::Amount(AmountRules::MONEY),
                    (90, 110, 0),
                )
            },
            column(
                L_MEMO,
                h.memo,
                CellKind::Text { max_chars: 500 },
                (100, 200, 2),
            ),
        ];
        if self.customer {
            columns.push(column(
                L_CUSTOMER,
                h.customer,
                picker(L::CUSTOMERS),
                (110, 170, 1),
            ));
        }
        if self.billable {
            columns.push(Column {
                align: Align::Center,
                ..column(L_BILLABLE, h.billable, CellKind::Checkbox, (60, 70, 0))
            });
        }
        if self.tags {
            let kind = if self.tag_chips {
                CellKind::Tags { list: L::TAGS }
            } else {
                picker(L::TAGS)
            };
            columns.push(column(L_TAG, h.tags, kind, (90, 130, 1)));
        }
        GridSpec::line_entry(columns)
    }
}

fn column(id: ColumnId, header: TextKey, kind: CellKind, width: (u16, u16, u8)) -> Column {
    Column {
        id,
        header,
        placeholder: None,
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

/// Row keys, the lists, the columns that are read-only and the Esc snapshot
/// of the line being edited.
pub struct LinesState<L> {
    /// Key generator.
    pub keys: RowKeyGen,
    /// Picker lists.
    pub lists: Arc<L>,
    /// Columns the owner can't change right now (e.g. Customer:Job and
    /// Billable without `line-billable-v1`).
    pub locked: Vec<ColumnId>,
    tag_chips: bool,
    need_account: TextKey,
    snapshot: Option<FormLine>,
    /// A suggested account per line (rule or AI): shown as a ghost value in
    /// an empty Account cell, on a `warn.tint` row; Tab or Accept sets it.
    /// Never an amount (non-negotiable 4).
    suggested: std::collections::BTreeMap<RowKey, AccountId>,
    /// Suggestions the owner accepted with Tab since the screen last asked.
    accepted: Vec<(RowKey, AccountId)>,
}

struct View<'a, L> {
    form: &'a mut FormState<FormLines>,
    a: &'a mut LinesState<L>,
    /// Each line's tag as a chip (chips columns only).
    chips: Vec<Option<TagChip>>,
}

impl<L: LineLists> LinesAdapter<FormLines> for LinesState<L> {
    fn model<'a>(&'a mut self, form: &'a mut FormState<FormLines>) -> Box<dyn EditableModel + 'a> {
        let chips = if self.tag_chips {
            let lists = &*self.lists;
            form.current()
                .lines
                .iter()
                .map(|l| {
                    l.tag
                        .and_then(|g| lists.tag_key(g))
                        .and_then(|k| lists.tag_chip(k))
                })
                .collect()
        } else {
            Vec::new()
        };
        Box::new(View {
            form,
            a: self,
            chips,
        })
    }
}

impl<L> View<'_, L> {
    fn lines(&self) -> &[FormLine] {
        &self.form.current().lines
    }
}

fn picked<L: LineLists>(lists: &L, list: ListId, id: Option<u64>) -> CellValue<'_> {
    id.map_or(CellValue::Empty, |id| CellValue::Ref {
        id,
        display: lists.display(list, id).unwrap_or(Cow::Borrowed("")),
    })
}

impl<L: LineLists> RowModel for View<'_, L> {
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
            Some(l) if l.account.is_none() && self.a.suggested.contains_key(&l.key) => {
                RowState::SUGGESTED
            }
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
            L_ACCOUNT if l.account.is_none() => self
                .a
                .suggested
                .get(&l.key)
                .and_then(|a| lists.account_key(*a))
                .map_or(CellValue::Empty, |id| CellValue::Suggested {
                    id,
                    display: lists.display(L::ACCOUNTS, id).unwrap_or(Cow::Borrowed("")),
                }),
            L_ACCOUNT => picked(
                lists,
                L::ACCOUNTS,
                l.account.and_then(|a| lists.account_key(a)),
            ),
            L_AMOUNT => l.amount.map_or(CellValue::Empty, CellValue::Money),
            L_MEMO => CellValue::Text(Cow::Borrowed(&l.memo)),
            L_CUSTOMER => picked(
                lists,
                L::CUSTOMERS,
                l.customer.and_then(|c| lists.customer_key(c)),
            ),
            L_BILLABLE => CellValue::Bool(l.billable),
            L_TAG if self.a.tag_chips => match self.chips.get(index) {
                Some(Some(chip)) => CellValue::Tags(std::slice::from_ref(chip)),
                _ => CellValue::Tags(&[]),
            },
            L_TAG => picked(lists, L::TAGS, l.tag.and_then(|g| lists.tag_key(g))),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, _index: usize) -> String {
        String::new()
    }
}

/// Puts one committed cell into a line.
pub(crate) fn apply<L: LineLists>(
    l: &mut FormLine,
    col: ColumnId,
    value: CellInput,
    lists: &L,
) -> Option<TextKey> {
    match (col, value) {
        (L_ACCOUNT, CellInput::Ref(id)) => match lists.account(id) {
            Some(a) => l.account = Some(a),
            None => return Some(PICK_FROM_LIST),
        },
        (L_CUSTOMER, CellInput::Ref(id)) => match lists.customer(id) {
            Some(c) => l.customer = Some(c),
            None => return Some(PICK_FROM_LIST),
        },
        (L_TAG, CellInput::Ref(id)) => match lists.tag(id) {
            Some(g) => l.tag = Some(g),
            None => return Some(PICK_FROM_LIST),
        },
        // One tag per line (Q-233): the newest chip wins.
        (L_TAG, CellInput::Tags(chips)) => match chips.last() {
            None => l.tag = None,
            Some(chip) => match lists.tag(chip.id) {
                Some(g) => l.tag = Some(g),
                None => return Some(PICK_FROM_LIST),
            },
        },
        (L_ACCOUNT | L_CUSTOMER | L_TAG, CellInput::Unresolved(_)) => return Some(PICK_FROM_LIST),
        (L_ACCOUNT, CellInput::Clear) => l.account = None,
        (L_CUSTOMER, CellInput::Clear) => l.customer = None,
        (L_TAG, CellInput::Clear) => l.tag = None,
        (L_AMOUNT, CellInput::Money(m)) => l.amount = Some(m),
        (L_AMOUNT, CellInput::Clear) => l.amount = None,
        (L_MEMO, CellInput::Text(v)) => l.memo = v,
        (L_MEMO, CellInput::Clear) => l.memo.clear(),
        (L_BILLABLE, CellInput::Bool(b)) => l.billable = b,
        (L_BILLABLE, CellInput::Clear) => l.billable = false,
        _ => return Some(BAD_VALUE),
    }
    None
}

impl<L: LineLists> EditableModel for View<'_, L> {
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool {
        index < self.lines().len() && !self.a.locked.contains(&col)
    }
    fn row_entered(&mut self, key: RowKey) {
        self.a.snapshot = self.lines().iter().find(|l| l.key == key).cloned();
    }
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        source: Source,
    ) -> Result<(), CellProblem> {
        if self.a.locked.contains(&pos.col) {
            return Err(CellProblem::new(BAD_VALUE).at(pos.col));
        }
        let lists = Arc::clone(&self.a.lists);
        if source == Source::SuggestionAccepted
            && pos.col == L_ACCOUNT
            && let CellInput::Ref(id) = &value
            && let Some(account) = lists.account(*id)
        {
            self.a.accepted.push((pos.row, account));
        }
        let keys = &mut self.a.keys;
        let mut problem = None;
        let applied = self.form.edit_step(|v| {
            if let Some(l) = v.lines.iter_mut().find(|l| l.key == pos.row) {
                problem = apply(l, pos.col, value, &*lists);
            }
            ensure_entry(&mut v.lines, keys);
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
            ensure_entry(&mut v.lines, keys);
        })
    }
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey> {
        let key = self.a.keys.next_key();
        self.form
            .edit_step(|v| {
                let at = before
                    .and_then(|b| v.lines.iter().position(|l| l.key == b))
                    .unwrap_or(v.lines.len());
                v.lines.insert(at, FormLine::blank(key));
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
            ensure_entry(&mut v.lines, keys);
        })
    }
    fn record_row(&mut self, key: RowKey) -> RecordOutcome {
        let Some(i) = self.index_of(key) else {
            return RecordOutcome::Invalid(Vec::new());
        };
        if self.lines()[i].account.is_none() {
            return RecordOutcome::Invalid(vec![
                CellProblem::new(self.a.need_account).at(L_ACCOUNT),
            ]);
        }
        RecordOutcome::Recorded {
            next: self.row_key(i + 1),
        }
    }
}

/// The keymap commands a lines grid takes while it has the keyboard.
const GRID_COMMANDS: &[&str] = &[
    "form.revert",
    "edit.undo",
    "edit.redo",
    "edit.open_list",
    "form.insert_line",
    "form.delete_line",
];

/// A form's lines on the shared grid.
pub struct LinesGrid<L> {
    form: GridForm<FormLines, LinesState<L>>,
}

impl<L: LineLists + 'static> LinesGrid<L> {
    /// A grid with `seeds` (each gets a fresh key; a blank entry line is
    /// added).
    ///
    /// # Panics
    /// Never: the specs are fixed and covered by tests.
    #[must_use]
    pub fn new(spec: &LinesSpec, lists: Arc<L>, seeds: Vec<FormLine>) -> LinesGrid<L> {
        let mut keys = RowKeyGen::new();
        let value = FormLines::from_lines(seeds, &mut keys);
        // The specs are fixed; `lines_specs_are_valid` covers every shape.
        let grid = Grid::new(spec.grid_id, spec.grid_spec::<L>()).expect("the lines spec is valid");
        LinesGrid {
            form: GridForm::new(
                FormState::new(value),
                LinesState {
                    keys,
                    lists,
                    locked: Vec::new(),
                    tag_chips: spec.tags && spec.tag_chips,
                    need_account: spec.need_account,
                    snapshot: None,
                    suggested: std::collections::BTreeMap::new(),
                    accepted: Vec::new(),
                },
                grid,
                t(spec.label),
                spec.height,
            ),
        }
    }

    /// The grid form (saves, loads and header-driven edits).
    pub fn form_mut(&mut self) -> &mut GridForm<FormLines, LinesState<L>> {
        &mut self.form
    }

    /// The grid form, read only.
    #[must_use]
    pub fn form(&self) -> &GridForm<FormLines, LinesState<L>> {
        &self.form
    }

    /// The grid has the keyboard.
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.form.grid().has_focus()
    }

    /// The lines.
    #[must_use]
    pub fn value(&self) -> &FormLines {
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

    /// The pickers changed.
    pub fn set_lists(&mut self, lists: Arc<L>) {
        self.form.adapter_mut().lists = lists;
    }

    /// Makes `cols` read-only (all others editable).
    pub fn set_locked(&mut self, cols: Vec<ColumnId>) {
        self.form.adapter_mut().locked = cols;
    }

    /// Shows `account` as the suggestion for line `key` (`None` removes it).
    /// Only an empty Account cell shows it; the line keeps no account until
    /// the owner accepts.
    pub fn set_suggestion(&mut self, key: RowKey, account: Option<AccountId>) {
        let s = &mut self.form.adapter_mut().suggested;
        match account {
            Some(a) => {
                s.insert(key, a);
            }
            None => {
                s.remove(&key);
            }
        }
    }

    /// Removes every suggestion.
    pub fn clear_suggestions(&mut self) {
        self.form.adapter_mut().suggested.clear();
        self.form.adapter_mut().accepted.clear();
    }

    /// The suggestions still waiting for the owner: lines without an
    /// account that have one.
    #[must_use]
    pub fn open_suggestions(&self) -> Vec<(RowKey, AccountId)> {
        let a = self.form.adapter();
        self.value()
            .lines
            .iter()
            .filter(|l| l.account.is_none())
            .filter_map(|l| a.suggested.get(&l.key).map(|s| (l.key, *s)))
            .collect()
    }

    /// The owner clicked Accept: the suggestion becomes the line's account
    /// (one undo step). False when there is none or the line has an account.
    pub fn accept_suggestion(&mut self, key: RowKey) -> bool {
        let Some(account) = self.form.adapter().suggested.get(&key).copied() else {
            return false;
        };
        let applied = self.form.edit_step(|v| {
            if let Some(l) = v
                .lines
                .iter_mut()
                .find(|l| l.key == key && l.account.is_none())
            {
                l.account = Some(account);
            }
        });
        if applied {
            self.form.adapter_mut().accepted.push((key, account));
        }
        applied
    }

    /// The suggestions accepted (Tab or Accept) since the last call.
    pub fn take_accepted(&mut self) -> Vec<(RowKey, AccountId)> {
        std::mem::take(&mut self.form.adapter_mut().accepted)
    }

    /// The cursor's line and column, when the grid has one.
    #[must_use]
    pub fn cursor(&self) -> Option<CellPos> {
        self.form.grid().core().cursor()
    }

    /// Puts the cursor on the first line's Account cell.
    pub fn focus_first(&mut self) {
        let Some(key) = self.value().lines.first().map(|l| l.key) else {
            return;
        };
        let lists = Arc::clone(&self.form.adapter().lists);
        self.form.focus(
            CellPos {
                row: key,
                col: L_ACCOUNT,
            },
            &*lists,
        );
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
    use crate::screens::register::lists::RegisterLists;
    use cl_core::Money;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    #[test]
    fn lines_specs_are_valid() {
        let register = LinesSpec::REGISTER_SPLITS.grid_spec::<RegisterLists>();
        assert_eq!(register.validate(), Ok(()));
        let ids: Vec<ColumnId> = register.columns.iter().map(|c| c.id).collect();
        assert_eq!(ids, [L_ACCOUNT, L_AMOUNT, L_MEMO, L_CUSTOMER, L_TAG]);
        let all = LinesSpec::all("t/lines", "lines.label").grid_spec::<RegisterLists>();
        assert_eq!(all.validate(), Ok(()));
        let ids: Vec<ColumnId> = all.columns.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [L_ACCOUNT, L_AMOUNT, L_MEMO, L_CUSTOMER, L_BILLABLE, L_TAG]
        );
        assert_eq!(all.columns[4].kind, CellKind::Checkbox);
        assert!(matches!(all.columns[5].kind, CellKind::Tags { .. }));
        for c in &all.columns {
            assert!(crate::i18n::has(c.header.0), "{}", c.header.0);
        }
        assert!(crate::i18n::has(
            LinesSpec::all("x", "lines.label").need_account.0
        ));
        let bare = LinesSpec {
            customer: false,
            billable: false,
            tags: false,
            ..LinesSpec::REGISTER_SPLITS
        }
        .grid_spec::<RegisterLists>();
        assert_eq!(bare.columns.len(), 3);
    }

    #[test]
    fn a_line_takes_one_tag_and_picks_must_match() {
        let lists = RegisterLists::default();
        let mut l = FormLine::blank(RowKeyGen::new().next_key());
        assert_eq!(
            apply(&mut l, L_TAG, CellInput::Ref(1), &lists),
            Some(PICK_FROM_LIST)
        );
        assert_eq!(
            apply(&mut l, L_AMOUNT, CellInput::Money(money("2")), &lists),
            None
        );
        assert_eq!(
            apply(&mut l, L_MEMO, CellInput::Text("x".into()), &lists),
            None
        );
        assert!(!l.is_blank());
        assert_eq!(apply(&mut l, L_TAG, CellInput::Clear, &lists), None);
        assert_eq!(l.tag, None);
        let chip = TagChip {
            id: 9,
            label: "x".into(),
            group: None,
        };
        assert_eq!(
            apply(&mut l, L_TAG, CellInput::Tags(vec![chip]), &lists),
            Some(PICK_FROM_LIST)
        );
        assert_eq!(
            apply(&mut l, L_TAG, CellInput::Tags(Vec::new()), &lists),
            None
        );
        assert_eq!(l.tag, None);
    }

    #[test]
    fn billable_is_a_checkbox_and_never_an_amount() {
        let lists = RegisterLists::default();
        let mut l = FormLine::blank(RowKeyGen::new().next_key());
        assert_eq!(
            apply(&mut l, L_BILLABLE, CellInput::Bool(true), &lists),
            None
        );
        assert!(l.billable);
        assert_eq!(apply(&mut l, L_BILLABLE, CellInput::Clear, &lists), None);
        assert!(!l.billable);
        assert_eq!(
            apply(&mut l, L_BILLABLE, CellInput::Money(money("1")), &lists),
            Some(BAD_VALUE)
        );
    }

    #[test]
    fn the_grid_keeps_a_blank_entry_line_and_honours_locks() {
        let chart = crate::company::demo::chart();
        let mut lists = RegisterLists::default();
        lists.set_accounts(&chart);
        let fuel = chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("6100"))
            .expect("6100")
            .id;
        let seed = FormLine {
            account: Some(fuel),
            amount: Some(money("12.50")),
            ..FormLine::blank(RowKeyGen::new().next_key())
        };
        let mut g = LinesGrid::new(
            &LinesSpec::all("t/lines", "lines.label"),
            Arc::new(lists),
            vec![seed],
        );
        assert_eq!(g.value().lines.len(), 2, "one blank entry line");
        assert!(!g.dirty());
        g.set_locked(vec![L_CUSTOMER, L_BILLABLE]);
        let key = g.value().lines[0].key;
        let state = g.form_mut().adapter_mut();
        assert_eq!(state.locked, [L_CUSTOMER, L_BILLABLE]);
        let mut form = FormState::new(g.value().clone());
        let mut model = g.form_mut().adapter_mut().model(&mut form);
        assert!(!model.cell_editable(0, L_BILLABLE));
        assert!(model.cell_editable(0, L_MEMO));
        assert!(
            model
                .set_cell(
                    CellPos {
                        row: key,
                        col: L_BILLABLE
                    },
                    CellInput::Bool(true),
                    Source::Typed,
                )
                .is_err()
        );
        assert!(matches!(model.cell(0, L_TAG), CellValue::Tags(t) if t.is_empty()));
        assert_eq!(model.cell(0, L_BILLABLE), CellValue::Bool(false));
        assert!(
            model
                .set_cell(
                    CellPos {
                        row: key,
                        col: L_MEMO
                    },
                    CellInput::Text("bags".into()),
                    Source::Typed,
                )
                .is_ok()
        );
        drop(model);
        assert_eq!(form.current().lines[0].memo, "bags");
    }
}
