//! The grid's data contract (P10 plan Rev 2 §2.2).
//!
//! Screens own the data and implement [`RowModel`] (read-only grids) or
//! [`EditableModel`] (line grids) over their `FormState` lines or a paged
//! cache. The grid asks for cells by index for drawing and names rows by
//! [`RowKey`] for everything that must survive a change. It never sums,
//! rounds or derives balances.

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;

use cl_core::{CalendarDate, Money, Quantity, Rate};
use cl_model::TextKey;
use cl_model::amount_entry::AmountRules;
use cl_model::tags::TagChip;

use super::rowkey::RowKey;

/// A column's id, unique within one grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnId(pub u16);

/// One cell: a row by key and a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellPos {
    /// The row.
    pub row: RowKey,
    /// The column.
    pub col: ColumnId,
}

/// A list the screen supplies for pickers and tags (opaque to the grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListId(pub u16);

/// Placement in a two-line row (register: Number above Type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Line {
    /// The first line (or the only one).
    #[default]
    First,
    /// The second line of a two-line row.
    Second,
}

/// Text alignment in a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Left (text).
    #[default]
    Left,
    /// Right (amounts).
    Right,
    /// Centered (marks, checkboxes).
    Center,
}

/// One choice of a dropdown column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OptionSpec {
    /// The value stored in the model.
    pub id: u64,
    /// The text shown.
    pub label: TextKey,
}

/// What a column holds and how it is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    /// Single-line text.
    Text {
        /// Longest accepted text.
        max_chars: u32,
    },
    /// A memo or description (Q-200: multi-line in P20).
    Memo {
        /// Shift+Enter inserts a line break; the row grows.
        multiline: bool,
        /// Longest accepted text.
        max_chars: u32,
    },
    /// An exact amount with the calculator (Q-12).
    Amount(AmountRules),
    /// A date with the date keys.
    Date,
    /// An account, name, item or Client:Job picker.
    Picker {
        /// Which list.
        list: ListId,
        /// Offer "Add …" when nothing matches (quick-add, P11).
        add_new: bool,
    },
    /// Tag chips, one per group (Q-179).
    Tags {
        /// Which tag list.
        list: ListId,
    },
    /// A checkbox.
    Checkbox,
    /// A fixed list of choices.
    Dropdown {
        /// The choices.
        options: &'static [OptionSpec],
    },
    /// The cleared/reconciled mark column (drawn, not a glyph).
    Mark,
    /// Read-only text (balance, type).
    Display,
}

impl CellKind {
    /// Kinds whose value can be a [`CellValue::Suggested`] (Q-P10l, NN4:
    /// never amounts).
    #[must_use]
    pub fn accepts_suggestions(self) -> bool {
        matches!(self, CellKind::Picker { .. } | CellKind::Tags { .. })
    }
}

/// Column width in logical points (zoom-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidthSpec {
    /// Never narrower than this.
    pub min: u16,
    /// Starting width.
    pub preferred: u16,
    /// Share of extra width (0 = fixed).
    pub flex: u8,
}

/// One column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    /// Unique id within the grid.
    pub id: ColumnId,
    /// Header text.
    pub header: TextKey,
    /// Placeholder shown in the empty entry line ("Account").
    pub placeholder: Option<TextKey>,
    /// A short explanation shown as the header's (and cells') tooltip and
    /// spoken as their description ("R = reconciled, ✓ = cleared").
    pub help: Option<TextKey>,
    /// What the cells hold.
    pub kind: CellKind,
    /// Which line of a two-line row.
    pub line: Line,
    /// Alignment.
    pub align: Align,
    /// Width rules.
    pub width: WidthSpec,
    /// Cells may be edited (still subject to row locks).
    pub editable: bool,
    /// Monospaced digits (amounts, dates, numbers).
    pub mono: bool,
}

/// How tall rows are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowHeight {
    /// Every row the same height: one line (30 pt) or two (44 pt).
    Uniform {
        /// Two lines per row (register).
        two_lines: bool,
    },
    /// Rows measured from their wrapped content (line grids only).
    Measured {
        /// The most rows such a grid may hold (at most 5,000, Q-P10m).
        max_rows: u32,
    },
}

/// The most rows a measured grid may hold (Q-P10m).
pub const MEASURED_MAX_ROWS: u32 = 5_000;

/// Row flags supplied by the screen. Locking comes from the screen, which
/// knows the closing date; the grid never decides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct RowState(u16);

impl RowState {
    /// Dated on or before the closing date: no editor opens (NN8).
    pub const LOCKED: RowState = RowState(1);
    /// Cleared at the bank.
    pub const CLEARED: RowState = RowState(1 << 1);
    /// Reconciled.
    pub const RECONCILED: RowState = RowState(1 << 2);
    /// Unsaved changes.
    pub const DIRTY: RowState = RowState(1 << 3);
    /// A problem to fix (failed save, invalid line).
    pub const ERROR: RowState = RowState(1 << 4);
    /// Needs the owner's review (bank feed).
    pub const NEEDS_REVIEW: RowState = RowState(1 << 5);
    /// The blank entry line at the end of a line grid.
    pub const ENTRY: RowState = RowState(1 << 6);
    /// Holds an AI or rule suggestion.
    pub const SUGGESTED: RowState = RowState(1 << 7);
    /// A save of this row is in flight.
    pub const PENDING: RowState = RowState(1 << 8);
    /// Shown muted (a void transaction): secondary text colour.
    pub const MUTED: RowState = RowState(1 << 9);

    /// No flags.
    #[must_use]
    pub const fn empty() -> RowState {
        RowState(0)
    }

    /// All flags of `other` are set.
    #[must_use]
    pub const fn contains(self, other: RowState) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of flags.
    #[must_use]
    pub const fn union(self, other: RowState) -> RowState {
        RowState(self.0 | other.0)
    }

    /// Without `other`'s flags.
    #[must_use]
    pub const fn without(self, other: RowState) -> RowState {
        RowState(self.0 & !other.0)
    }

    /// The raw bits.
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl std::ops::BitOr for RowState {
    type Output = RowState;
    fn bitor(self, rhs: RowState) -> RowState {
        self.union(rhs)
    }
}

/// The cleared/reconciled mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkKind {
    /// Not cleared.
    #[default]
    None,
    /// Cleared (a drawn check).
    Cleared,
    /// Reconciled (a mono `R`).
    Reconciled,
}

/// What a cell shows.
#[derive(Clone, PartialEq, Eq)]
pub enum CellValue<'a> {
    /// Nothing.
    Empty,
    /// Not loaded yet (paged models): drawn as a skeleton bar.
    Loading,
    /// Text.
    Text(Cow<'a, str>),
    /// Money.
    Money(Money),
    /// A quantity.
    Quantity(Quantity),
    /// A rate.
    Rate(Rate),
    /// A date.
    Date(CalendarDate),
    /// A checkbox value.
    Bool(bool),
    /// A picked list entry; the display text comes from the screen.
    Ref {
        /// The entry's id.
        id: u64,
        /// What to show.
        display: Cow<'a, str>,
    },
    /// A dropdown choice ([`OptionSpec::id`]).
    Choice(u64),
    /// Tag chips.
    Tags(&'a [TagChip]),
    /// The cleared/reconciled mark.
    Mark(MarkKind),
    /// An AI or rule suggestion for an empty picker or tag cell. Carries no
    /// money (NN4): an amount column given one draws it as empty.
    Suggested {
        /// The suggested entry's id.
        id: u64,
        /// What to show (placeholder style).
        display: Cow<'a, str>,
    },
}

/// Only the variant: cell values are owner data and never belong in logs.
impl fmt::Debug for CellValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CellValue::Empty => "Empty",
            CellValue::Loading => "Loading",
            CellValue::Text(_) => "Text(..)",
            CellValue::Money(_) => "Money(..)",
            CellValue::Quantity(_) => "Quantity(..)",
            CellValue::Rate(_) => "Rate(..)",
            CellValue::Date(_) => "Date(..)",
            CellValue::Bool(_) => "Bool(..)",
            CellValue::Ref { .. } => "Ref(..)",
            CellValue::Choice(_) => "Choice(..)",
            CellValue::Tags(_) => "Tags(..)",
            CellValue::Mark(_) => "Mark(..)",
            CellValue::Suggested { .. } => "Suggested(..)",
        })
    }
}

/// A value the grid commits into the model.
#[derive(Clone, PartialEq, Eq)]
pub enum CellInput {
    /// The cell was emptied.
    Clear,
    /// Text (text and memo cells).
    Text(String),
    /// An exact amount (typed or calculated); never a string or float.
    Money(Money),
    /// A quantity.
    Quantity(Quantity),
    /// A rate.
    Rate(Rate),
    /// A date.
    Date(CalendarDate),
    /// A checkbox.
    Bool(bool),
    /// A picked list entry.
    Ref(u64),
    /// A dropdown choice.
    Choice(u64),
    /// The cell's tags.
    Tags(Vec<TagChip>),
    /// Picker text that matched no entry (flagged; blocks the save at
    /// validation until resolved or added).
    Unresolved(String),
}

/// Only the variant, never the value (S5): safe in any log.
impl fmt::Debug for CellInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CellInput::Clear => "Clear",
            CellInput::Text(_) => "Text(..)",
            CellInput::Money(_) => "Money(..)",
            CellInput::Quantity(_) => "Quantity(..)",
            CellInput::Rate(_) => "Rate(..)",
            CellInput::Date(_) => "Date(..)",
            CellInput::Bool(_) => "Bool(..)",
            CellInput::Ref(_) => "Ref(..)",
            CellInput::Choice(_) => "Choice(..)",
            CellInput::Tags(_) => "Tags(..)",
            CellInput::Unresolved(_) => "Unresolved(..)",
        })
    }
}

/// Where a committed value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Typed.
    Typed,
    /// Picked from a list.
    Picked,
    /// An AI or rule suggestion the owner accepted with Tab (Q-P10w).
    SuggestionAccepted,
    /// Pasted.
    Pasted,
    /// The result of a calculation (Q-12).
    Calculator,
}

/// A note under a row (measured grids).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowNote<'a> {
    /// Plain caption text.
    Caption(Cow<'a, str>),
    /// The suggestion caption ("Suggested from 4 earlier Ozark checks").
    Suggestion(Cow<'a, str>),
    /// A problem.
    Error(TextKey),
}

/// A problem with one cell. Carries keys, never values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellProblem {
    /// The owner message.
    pub key: TextKey,
    /// The suggested fix.
    pub fix: Option<TextKey>,
    /// The cell's column, when known.
    pub col: Option<ColumnId>,
}

impl CellProblem {
    /// A problem with just a message.
    #[must_use]
    pub const fn new(key: TextKey) -> CellProblem {
        CellProblem {
            key,
            fix: None,
            col: None,
        }
    }

    /// The same problem, placed on a column.
    #[must_use]
    pub const fn at(mut self, col: ColumnId) -> CellProblem {
        self.col = Some(col);
        self
    }
}

/// A problem with the whole grid ("lines total does not match the check").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridProblem {
    /// The owner message.
    pub key: TextKey,
    /// The suggested fix.
    pub fix: Option<TextKey>,
    /// The row, when one is at fault.
    pub row: Option<RowKey>,
    /// The column, when one is at fault.
    pub col: Option<ColumnId>,
}

/// What [`EditableModel::record_row`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordOutcome {
    /// The row was recorded; `next` is the row to continue on (usually the
    /// entry line).
    Recorded {
        /// Where the cursor goes.
        next: Option<RowKey>,
    },
    /// The row is not valid; the cursor goes to the first problem.
    Invalid(Vec<CellProblem>),
    /// An asynchronous save started (`GridSpec::async_record`); the row is
    /// marked pending and the cursor moves on.
    Pending,
}

/// Read access the grid needs to draw and navigate.
pub trait RowModel {
    /// Changes whenever any row changes (`FormState::revision` for form-backed
    /// models). The open editor reloads when it moves (B3).
    fn revision(&self) -> u64;
    /// Number of rows, the entry line included.
    fn row_count(&self) -> usize;
    /// The key of the row at `index`; `None` for a row not loaded yet.
    fn row_key(&self, index: usize) -> Option<RowKey>;
    /// Where a key is now; `None` when the row is gone.
    fn index_of(&self, key: RowKey) -> Option<usize>;
    /// The row's flags.
    fn row_state(&self, index: usize) -> RowState;
    /// One cell's value.
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_>;
    /// A note under the row (measured grids only).
    fn row_note(&self, _index: usize) -> Option<RowNote<'_>> {
        None
    }
    /// The row's accessible label ("10/02/2026, Ozark Feed & Seed"); the
    /// grid appends "row n of N". Line grids may return an empty string and
    /// the grid says "line n".
    fn row_label(&self, index: usize) -> String;
    /// The rows drawn this frame (paging and prefetch hint, P12).
    fn visible_range(&mut self, _range: Range<usize>) {}
}

/// Write access for editable grids. Each `set_cell` and `revert_row` is one
/// non-merging undo step (`FormState::edit_step`).
pub trait EditableModel: RowModel {
    /// Can this cell be edited now (column editable and row not locked)?
    fn cell_editable(&self, index: usize, col: ColumnId) -> bool;
    /// The cursor entered this row: take the Esc snapshot. Called again after
    /// an undo or redo that touched the row.
    fn row_entered(&mut self, key: RowKey);
    /// Commit a value.
    ///
    /// A model over a `FormState` must return an error (for example
    /// `grid.form.saving`) when `edit_step` refuses because a save is in
    /// flight: an `Ok` with nothing stored would silently lose the typed
    /// text, while an error keeps it in the editor. `revert_row`,
    /// `insert_line` and `delete_line` likewise report "nothing changed"
    /// (`false` / `None`) then.
    ///
    /// # Errors
    /// The value is not accepted for this cell, or the record is saving.
    fn set_cell(
        &mut self,
        pos: CellPos,
        value: CellInput,
        source: Source,
    ) -> Result<(), CellProblem>;
    /// Back to the snapshot taken by `row_entered`; false when nothing
    /// changed.
    fn revert_row(&mut self, key: RowKey) -> bool;
    /// Insert a blank line before `before` (or at the end); its new key.
    fn insert_line(&mut self, before: Option<RowKey>) -> Option<RowKey>;
    /// Delete a line; false when refused.
    fn delete_line(&mut self, key: RowKey) -> bool;
    /// Enter on a row: check it and move on.
    fn record_row(&mut self, key: RowKey) -> RecordOutcome;
    /// Whole-grid rules (lines total = check amount).
    fn validate_grid(&self) -> Vec<GridProblem> {
        Vec::new()
    }
}

/// The model a grid is shown or commanded with.
pub enum ModelRef<'a> {
    /// A read-only model (Browse grids).
    Browse(&'a mut dyn RowModel),
    /// An editable model (line and fixed-row grids).
    Edit(&'a mut dyn EditableModel),
}

impl ModelRef<'_> {
    /// Read access either way.
    #[must_use]
    pub fn rows(&self) -> &dyn RowModel {
        match self {
            ModelRef::Browse(m) => &**m,
            ModelRef::Edit(m) => upcast(&**m),
        }
    }

    /// Mutable read access either way.
    pub fn rows_mut(&mut self) -> &mut dyn RowModel {
        match self {
            ModelRef::Browse(m) => &mut **m,
            ModelRef::Edit(m) => upcast_mut(&mut **m),
        }
    }

    /// Write access, when the model is editable.
    pub fn edit(&mut self) -> Option<&mut dyn EditableModel> {
        match self {
            ModelRef::Browse(_) => None,
            ModelRef::Edit(m) => Some(&mut **m),
        }
    }
}

fn upcast(m: &dyn EditableModel) -> &dyn RowModel {
    m
}

fn upcast_mut(m: &mut dyn EditableModel) -> &mut dyn RowModel {
    m
}

/// One entry a picker or tag cell can offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The entry's id.
    pub id: u64,
    /// What to show ("5100 · Materials:Plants").
    pub label: String,
    /// The tag group, for tag lists.
    pub group: Option<u64>,
}

/// A lookup answer; lists may still be loading (never blocks a frame).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LookupResult {
    /// The ranked entries.
    Ready(Vec<Suggestion>),
    /// The list is not loaded yet.
    Loading,
}

/// In-memory lists the screen supplies for pickers and tags.
pub trait Lookups {
    /// The best `limit` entries for `query` (`cl_model::lookup` ranking).
    fn suggest(&self, list: ListId, query: &str, limit: usize) -> LookupResult;
    /// The single entry whose name or number is exactly `query`.
    fn exact(&self, list: ListId, query: &str) -> Option<Suggestion>;
    /// The display text of an entry.
    fn display(&self, list: ListId, id: u64) -> Option<Cow<'_, str>>;
    /// Every active entry (Ctrl+L full list).
    fn all(&self, list: ListId) -> LookupResult;
}

/// No lists at all (Browse grids, tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoLookups;

impl Lookups for NoLookups {
    fn suggest(&self, _list: ListId, _query: &str, _limit: usize) -> LookupResult {
        LookupResult::Ready(Vec::new())
    }
    fn exact(&self, _list: ListId, _query: &str) -> Option<Suggestion> {
        None
    }
    fn display(&self, _list: ListId, _id: u64) -> Option<Cow<'_, str>> {
        None
    }
    fn all(&self, _list: ListId) -> LookupResult {
        LookupResult::Ready(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_state_flags() {
        let s = RowState::LOCKED | RowState::CLEARED;
        assert!(s.contains(RowState::LOCKED));
        assert!(!s.contains(RowState::ERROR));
        assert_eq!(s.without(RowState::LOCKED), RowState::CLEARED);
        assert_eq!(RowState::empty().bits(), 0);
    }

    #[test]
    fn debug_never_prints_values() {
        let m = Money::parse_wire("842.15").unwrap();
        let inputs = [
            CellInput::Money(m),
            CellInput::Text("Perennials 842".into()),
            CellInput::Unresolved("51".into()),
            CellInput::Ref(5100),
            CellInput::Choice(3),
        ];
        for i in inputs {
            let d = format!("{i:?}");
            assert!(!d.contains(|c: char| c.is_ascii_digit()), "{d}");
        }
        let v = CellValue::Ref {
            id: 5100,
            display: "5100 · Materials".into(),
        };
        assert_eq!(format!("{v:?}"), "Ref(..)");
        assert_eq!(format!("{:?}", CellValue::Money(m)), "Money(..)");
    }

    #[test]
    fn only_pickers_and_tags_take_suggestions() {
        assert!(
            CellKind::Picker {
                list: ListId(1),
                add_new: true
            }
            .accepts_suggestions()
        );
        assert!(CellKind::Tags { list: ListId(2) }.accepts_suggestions());
        assert!(!CellKind::Amount(AmountRules::MONEY).accepts_suggestions());
        assert!(!CellKind::Date.accepts_suggestions());
    }

    #[test]
    fn no_lookups_is_empty() {
        let l = NoLookups;
        assert_eq!(
            l.suggest(ListId(0), "x", 5),
            LookupResult::Ready(Vec::new())
        );
        assert_eq!(l.exact(ListId(0), "x"), None);
        assert_eq!(l.display(ListId(0), 1), None);
        assert_eq!(l.all(ListId(0)), LookupResult::Ready(Vec::new()));
        assert_eq!(
            CellProblem::new(TextKey("k")).at(ColumnId(2)).col,
            Some(ColumnId(2))
        );
    }
}
