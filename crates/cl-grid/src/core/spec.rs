//! What a grid is (columns, presets) and what it tells its screen (P10 plan
//! Rev 2 §2.2-2.3).

use std::fmt;
use std::ops::Range;

use super::model::{CellKind, CellPos, Column, ColumnId, MEASURED_MAX_ROWS, RowHeight, Source};
use super::rowkey::RowKey;
use super::selection::SelectionMode;

/// A grid's shape and behaviour. Use a preset and adjust.
// The plan names these flags (Q-P10r); presets set them together.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridSpec {
    /// The columns, left to right (line 1 then line 2 for two-line rows).
    pub columns: Vec<Column>,
    /// Row height rule.
    pub height: RowHeight,
    /// Cells can be edited.
    pub editable: bool,
    /// The model keeps a blank ENTRY row at the end.
    pub entry_line: bool,
    /// Ctrl+Insert / Ctrl+Delete are allowed.
    pub insert_delete: bool,
    /// Row selection.
    pub selection: SelectionMode,
    /// `record_row` may answer `Pending` (P12's entry row).
    pub async_record: bool,
}

/// Why a spec is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpecError {
    /// No columns at all.
    #[error("a grid needs at least one column")]
    NoColumns,
    /// Two columns share an id.
    #[error("two columns share an id")]
    DuplicateColumn,
    /// An editable grid with no editable column.
    #[error("an editable grid needs an editable column")]
    NoEditableColumn,
    /// A measured grid allowed more than 5,000 rows (Q-P10m).
    #[error("measured rows are limited to 5,000")]
    MeasuredTooLarge,
    /// A read-only column kind marked editable.
    #[error("a display or mark column cannot be editable")]
    ReadOnlyKindEditable,
    /// An entry line or insert/delete on a grid that cannot edit.
    #[error("entry lines and insert/delete need an editable grid")]
    EditFlagsWithoutEditing,
}

impl GridSpec {
    /// Split, journal, bill and invoice lines: editable, a blank entry line,
    /// insert/delete, no selection, synchronous record.
    #[must_use]
    pub fn line_entry(columns: Vec<Column>) -> GridSpec {
        GridSpec {
            columns,
            height: RowHeight::Uniform { two_lines: false },
            editable: true,
            entry_line: true,
            insert_delete: true,
            selection: SelectionMode::None,
            async_record: false,
        }
    }

    /// Registers, lists, bank feed: read-only, multi-row selection.
    #[must_use]
    pub fn browse(columns: Vec<Column>) -> GridSpec {
        GridSpec {
            columns,
            height: RowHeight::Uniform { two_lines: true },
            editable: false,
            entry_line: false,
            insert_delete: false,
            selection: SelectionMode::Multi,
            async_record: false,
        }
    }

    /// Pay Bills, Receive Payment, Reconcile: editable cells on a fixed set
    /// of rows (no entry line, no insert/delete), multi-row selection.
    #[must_use]
    pub fn fixed_edit(columns: Vec<Column>) -> GridSpec {
        GridSpec {
            columns,
            height: RowHeight::Uniform { two_lines: false },
            editable: true,
            entry_line: false,
            insert_delete: false,
            selection: SelectionMode::Multi,
            async_record: false,
        }
    }

    /// Check the spec.
    ///
    /// # Errors
    /// See [`SpecError`].
    pub fn validate(&self) -> Result<(), SpecError> {
        if self.columns.is_empty() {
            return Err(SpecError::NoColumns);
        }
        for (i, c) in self.columns.iter().enumerate() {
            if self.columns[..i].iter().any(|p| p.id == c.id) {
                return Err(SpecError::DuplicateColumn);
            }
            if c.editable && matches!(c.kind, CellKind::Display | CellKind::Mark) {
                return Err(SpecError::ReadOnlyKindEditable);
            }
        }
        if self.editable && !self.columns.iter().any(|c| c.editable) {
            return Err(SpecError::NoEditableColumn);
        }
        if !self.editable && (self.entry_line || self.insert_delete || self.async_record) {
            return Err(SpecError::EditFlagsWithoutEditing);
        }
        if let RowHeight::Measured { max_rows } = self.height
            && max_rows > MEASURED_MAX_ROWS
        {
            return Err(SpecError::MeasuredTooLarge);
        }
        Ok(())
    }

    /// The column with this id.
    #[must_use]
    pub fn column(&self, id: ColumnId) -> Option<&Column> {
        self.columns.iter().find(|c| c.id == id)
    }

    /// Position of a column in [`GridSpec::columns`].
    #[must_use]
    pub fn column_index(&self, id: ColumnId) -> Option<usize> {
        self.columns.iter().position(|c| c.id == id)
    }
}

/// A command the screen forwards from the keymap (F1) or a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GridCommand {
    /// Commit the open cell (before a save).
    Commit,
    /// Esc: close the popup, revert the cell, then the row.
    Revert,
    /// Ctrl+Insert.
    InsertLine,
    /// Ctrl+Delete.
    DeleteLine,
    /// Ctrl+L: the full list.
    OpenList,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Ctrl+Z.
    Undo,
    /// Ctrl+Y.
    Redo,
    /// Ctrl+C in Browse.
    Copy,
}

/// Whether a command did anything. `NotHandled` lets the shell run its
/// default (Esc closes or prompts; Ctrl+Z is the form's undo).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handled {
    /// The grid acted.
    Handled,
    /// The grid had nothing to do.
    NotHandled,
}

impl Handled {
    /// True for [`Handled::Handled`].
    #[must_use]
    pub fn yes(self) -> bool {
        self == Handled::Handled
    }
}

/// What happened in the grid this frame. `Debug` prints the variant only:
/// typed text and pasted names never reach a log (S5).
#[derive(Clone, PartialEq, Eq)]
pub enum GridEvent {
    /// A value was committed into the model.
    CellCommitted {
        /// Which cell.
        pos: CellPos,
        /// How it was entered.
        source: Source,
    },
    /// Enter recorded a row.
    RowRecorded {
        /// Which row.
        key: RowKey,
    },
    /// A row could not be recorded or vanished (`grid.row.gone`).
    RecordRefused {
        /// Which row.
        key: RowKey,
    },
    /// Enter or double-click in a Browse grid.
    Activate {
        /// Which row.
        key: RowKey,
    },
    /// The selection changed.
    SelectionChanged,
    /// "Add …" chosen in a picker: the screen opens quick-add (P11) and
    /// calls `resolve_pending`.
    AddNew {
        /// Which cell.
        pos: CellPos,
        /// What was typed.
        text: String,
    },
    /// An edit, insert or delete was tried on a locked row: the screen runs
    /// the closing-date password flow (NN8).
    LockedRowEditAttempt {
        /// Which row.
        key: RowKey,
    },
    /// Enter on the pristine entry line: the screen treats it as Save & New
    /// (Q-P10v, Q-190).
    EnterOnEmptyEntry,
    /// A raw Esc the grid could not use (defensive; grid screens use Form
    /// scope so Esc arrives as a command).
    EscapeUnhandled,
    /// Tab or Shift+Tab left the grid.
    LeftGrid {
        /// Tab (true) or Shift+Tab (false).
        forward: bool,
    },
    /// Printable text typed in a Browse grid (P12 starts an entry, P26 binds
    /// keys).
    TypedInBrowse(String),
    /// The rows drawn this frame.
    VisibleRange(Range<usize>),
    /// A column was resized (the screen persists widths).
    ColumnsResized,
}

impl fmt::Debug for GridEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            GridEvent::CellCommitted { .. } => "CellCommitted",
            GridEvent::RowRecorded { .. } => "RowRecorded",
            GridEvent::RecordRefused { .. } => "RecordRefused",
            GridEvent::Activate { .. } => "Activate",
            GridEvent::SelectionChanged => "SelectionChanged",
            GridEvent::AddNew { .. } => "AddNew",
            GridEvent::LockedRowEditAttempt { .. } => "LockedRowEditAttempt",
            GridEvent::EnterOnEmptyEntry => "EnterOnEmptyEntry",
            GridEvent::EscapeUnhandled => "EscapeUnhandled",
            GridEvent::LeftGrid { .. } => "LeftGrid",
            GridEvent::TypedInBrowse(_) => "TypedInBrowse",
            GridEvent::VisibleRange(_) => "VisibleRange",
            GridEvent::ColumnsResized => "ColumnsResized",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Align, Line, WidthSpec};
    use cl_model::TextKey;
    use cl_model::amount_entry::AmountRules;

    fn col(id: u16, kind: CellKind, editable: bool) -> Column {
        Column {
            id: ColumnId(id),
            header: TextKey("h"),
            placeholder: None,
            kind,
            line: Line::First,
            align: Align::Left,
            width: WidthSpec {
                min: 40,
                preferred: 80,
                flex: 1,
            },
            editable,
            mono: false,
        }
    }

    #[test]
    fn presets_validate() {
        let cols = vec![col(1, CellKind::Amount(AmountRules::MONEY), true)];
        for spec in [
            GridSpec::line_entry(cols.clone()),
            GridSpec::fixed_edit(cols.clone()),
            GridSpec::browse(vec![col(1, CellKind::Display, false)]),
        ] {
            assert_eq!(spec.validate(), Ok(()));
        }
        let f = GridSpec::fixed_edit(cols.clone());
        assert!(!f.entry_line && !f.insert_delete && f.editable);
        assert_eq!(f.selection, SelectionMode::Multi);
        assert_eq!(GridSpec::line_entry(cols).selection, SelectionMode::None);
    }

    #[test]
    fn bad_specs_are_refused() {
        assert_eq!(
            GridSpec::browse(vec![]).validate(),
            Err(SpecError::NoColumns)
        );
        let dup = vec![
            col(1, CellKind::Display, false),
            col(1, CellKind::Date, false),
        ];
        assert_eq!(
            GridSpec::browse(dup).validate(),
            Err(SpecError::DuplicateColumn)
        );
        let ro = vec![col(1, CellKind::Date, false)];
        assert_eq!(
            GridSpec::line_entry(ro.clone()).validate(),
            Err(SpecError::NoEditableColumn)
        );
        let mut b = GridSpec::browse(ro.clone());
        b.entry_line = true;
        assert_eq!(b.validate(), Err(SpecError::EditFlagsWithoutEditing));
        let mut m = GridSpec::line_entry(vec![col(1, CellKind::Date, true)]);
        m.height = RowHeight::Measured { max_rows: 5_001 };
        assert_eq!(m.validate(), Err(SpecError::MeasuredTooLarge));
        m.height = RowHeight::Measured { max_rows: 5_000 };
        assert_eq!(m.validate(), Ok(()));
        let disp = vec![col(1, CellKind::Display, true)];
        assert_eq!(
            GridSpec::line_entry(disp).validate(),
            Err(SpecError::ReadOnlyKindEditable)
        );
    }

    #[test]
    fn lookups_by_column() {
        let s = GridSpec::browse(vec![
            col(4, CellKind::Display, false),
            col(9, CellKind::Mark, false),
        ]);
        assert_eq!(s.column_index(ColumnId(9)), Some(1));
        assert!(s.column(ColumnId(4)).is_some());
        assert!(s.column(ColumnId(5)).is_none());
        assert!(Handled::Handled.yes());
        assert!(!Handled::NotHandled.yes());
    }

    #[test]
    fn event_debug_prints_no_values() {
        let pos = CellPos {
            row: RowKey(12),
            col: ColumnId(3),
        };
        for e in [
            GridEvent::AddNew {
                pos,
                text: "Ozark 842".into(),
            },
            GridEvent::TypedInBrowse("5".into()),
            GridEvent::CellCommitted {
                pos,
                source: Source::Typed,
            },
            GridEvent::VisibleRange(10..20),
        ] {
            let d = format!("{e:?}");
            assert!(!d.contains(|c: char| c.is_ascii_digit()), "{d}");
            assert!(!d.contains("Ozark"));
        }
    }
}
