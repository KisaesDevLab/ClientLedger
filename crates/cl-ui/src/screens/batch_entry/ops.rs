//! Bulk Entry's row actions (P16 steps 0f and 0h; Q-272, Q-274): the grid's
//! right-click menu (also Shift+F10 or the Menu key), Copy down and Copy to
//! rows below (Ctrl+D), the row selection, and Delete rows.
//!
//! Every action is one `edit_step` over the section, so Ctrl+Z takes it
//! back whole. Changed rows lose their engine check on their own (answers
//! are keyed by the row's hash) and are checked again half a second later.
//! Amounts, debits, credits and check numbers are never copied or set
//! (`cl_model::forms::batch::ops::copyable`, non-negotiable 6).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cl_grid::{CellPos, ContextItem, ContextMenuSource, ContextTarget, GridCommand, RowKey};
use cl_model::forms::batch::{ColumnChoice, CopyScope, Field, Section, SectionKind, ops};

use super::grid::{ensure_entry, field_of, header_key};
use super::{BulkEntryScreen, columns::needed_key};
use crate::i18n::{t, t_count};

/// The menu entries' ids (reported back by `GridEvent::ContextPicked`).
pub(crate) mod item {
    /// Copy down.
    pub const COPY_DOWN: u16 = 1;
    /// Copy to rows below.
    pub const COPY_BELOW: u16 = 2;
    /// Select all rows.
    pub const SELECT_ALL: u16 = 3;
    /// Clear selection.
    pub const CLEAR_SELECTION: u16 = 4;
    /// Delete the selected rows (asks first).
    pub const DELETE_ROWS: u16 = 5;
    /// Columns… (the column chooser).
    pub const COLUMNS: u16 = 6;
    /// Hide the column (header menu).
    pub const HIDE_COLUMN: u16 = 7;
}

/// The shortcut a command shows in menus ("Ctrl+D"), if it has one.
fn shortcut_of(id: &str) -> Option<String> {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
}

/// A menu entry labelled and keyed like its command.
fn entry(id: u16, command: &str) -> ContextItem {
    let mut i = ContextItem::new(id, t(&format!("cmd.{command}")));
    i.shortcut = shortcut_of(command);
    i
}

/// What the grid's context menu offers, worked out before the grid draws
/// (the grid holds the batch while it draws). Keys and flags only.
pub(crate) struct BulkMenu {
    kind: SectionKind,
    /// Each row of the shown section in grid order: (key, blank, dated).
    rows: Vec<(RowKey, bool, bool)>,
    /// How many rows are selected.
    selected: usize,
    choice: ColumnChoice,
}

impl BulkMenu {
    /// True when Copy down of `f` from `from` reaches a row: the same rule
    /// as `ops::can_copy_down` (a date to copy, another non-blank row in
    /// reach), over the flags.
    fn can_copy(&self, from: RowKey, f: Field, scope: CopyScope) -> bool {
        if !ops::copyable(self.kind, f) {
            return false;
        }
        let Some(at) = self.rows.iter().position(|r| r.0 == from) else {
            return false;
        };
        if f == Field::Date && !self.rows[at].2 {
            return false;
        }
        self.rows
            .iter()
            .enumerate()
            .any(|(i, r)| i != at && !r.1 && (scope == CopyScope::AllDisplayed || i > at))
    }

    fn cell_items(&self, pos: CellPos) -> Vec<ContextItem> {
        let field = field_of(pos.col);
        let why_not = match field {
            Some(f) if !ops::copyable(self.kind, f) => t("batch_entry.copy.protected"),
            _ => t("batch_entry.copy.nothing"),
        };
        let copy = |id: u16, command: &str, scope: CopyScope| {
            let on = field.is_some_and(|f| self.can_copy(pos.row, f, scope));
            let i = entry(id, command).enabled(on);
            if on { i } else { i.tooltip(why_not.clone()) }
        };
        let any_rows = self.rows.iter().any(|r| !r.1);
        vec![
            copy(item::COPY_DOWN, "grid.copy_down", CopyScope::AllDisplayed),
            copy(item::COPY_BELOW, "grid.copy_down_below", CopyScope::Below),
            entry(item::SELECT_ALL, "grid.select_all").enabled(any_rows),
            entry(item::CLEAR_SELECTION, "grid.clear_selection").enabled(self.selected > 0),
            entry(item::DELETE_ROWS, "batch.delete_rows").enabled(self.selected > 0),
            entry(item::COLUMNS, "batch.columns"),
        ]
    }

    fn header_items(&self, f: Option<Field>) -> Vec<ContextItem> {
        let hide = ContextItem::new(item::HIDE_COLUMN, t("batch_entry.columns.hide"));
        let hide = match f {
            Some(f) if cl_model::forms::batch::is_required(self.kind, f) => {
                hide.enabled(false).tooltip(t(needed_key(self.kind)))
            }
            Some(f) if !self.choice.is_hidden(f) => hide,
            _ => hide.enabled(false),
        };
        vec![entry(item::COLUMNS, "batch.columns"), hide]
    }
}

impl ContextMenuSource for BulkMenu {
    fn items(&self, target: ContextTarget) -> Vec<ContextItem> {
        match target {
            ContextTarget::Cell(pos) => self.cell_items(pos),
            ContextTarget::Header(col) => self.header_items(field_of(col)),
        }
    }
}

impl BulkEntryScreen {
    /// The context menu's flags for this frame.
    pub(crate) fn menu(&self) -> BulkMenu {
        let rows = self
            .batch()
            .sections
            .get(self.selected())
            .map(|s| {
                s.rows
                    .iter()
                    .map(|r| (r.key, r.is_blank(), r.date.is_some()))
                    .collect()
            })
            .unwrap_or_default();
        BulkMenu {
            kind: self.shown_kind,
            rows,
            selected: self.selected_rows().len(),
            choice: self.choice(self.shown_kind),
        }
    }

    /// The selected rows of the shown section (never the blank entry row).
    #[must_use]
    pub(crate) fn selected_rows(&self) -> BTreeSet<RowKey> {
        let picked: BTreeSet<RowKey> = self.grid.grid().selected_keys().collect();
        if picked.is_empty() {
            return picked;
        }
        self.batch()
            .sections
            .get(self.selected())
            .map(|s| {
                s.rows
                    .iter()
                    .filter(|r| !r.is_blank() && picked.contains(&r.key))
                    .map(|r| r.key)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A pick on the grid's menu.
    pub(crate) fn menu_picked(&mut self, target: ContextTarget, id: u16) {
        match (id, target) {
            (item::COPY_DOWN, ContextTarget::Cell(pos)) => {
                self.copy_down_at(pos, CopyScope::AllDisplayed);
            }
            (item::COPY_BELOW, ContextTarget::Cell(pos)) => {
                self.copy_down_at(pos, CopyScope::Below);
            }
            (item::SELECT_ALL, _) => self.select_all(),
            (item::CLEAR_SELECTION, _) => self.clear_selection(),
            (item::DELETE_ROWS, _) => self.ask_delete(),
            (item::COLUMNS, _) => self.open_columns(),
            (item::HIDE_COLUMN, ContextTarget::Header(col)) => {
                if let Some(f) = field_of(col) {
                    let _ = self.hide_column(f, true);
                }
            }
            _ => {}
        }
    }

    /// The open cell goes into the row first (an action must not lose or
    /// overwrite typing); false when it is not valid (the cursor stays on
    /// it with its problem).
    fn commit_first(&mut self) -> bool {
        let lists = Arc::clone(&self.books.lists);
        self.grid.commit_pending(&*lists).is_ok()
    }

    /// One action on the shown section as one undo step: `f` changes a
    /// copy and says how many rows it changed; nothing happens when none
    /// did. The changed rows show their problems from now on (they are
    /// checked again after the usual half second). Returns the count.
    pub(crate) fn change_section(&mut self, f: impl FnOnce(&mut Section) -> usize) -> usize {
        let i = self.selected();
        let Some(before) = self.batch().sections.get(i).cloned() else {
            return 0;
        };
        let mut after = before.clone();
        let n = f(&mut after);
        if n == 0 || after == before {
            return 0;
        }
        let old: BTreeMap<RowKey, &cl_model::forms::batch::RowDraft> =
            before.rows.iter().map(|r| (r.key, r)).collect();
        let changed: Vec<RowKey> = after
            .rows
            .iter()
            .filter(|r| old.get(&r.key).is_some_and(|b| *b != *r))
            .map(|r| r.key)
            .collect();
        let today = self.today;
        let highest = self.grid.adapter().highest.clone();
        let mut keys = std::mem::take(&mut self.grid.adapter_mut().keys);
        let applied = self.grid.edit_step(|b| {
            if let Some(s) = b.sections.get_mut(i) {
                *s = after;
            }
            ensure_entry(b, i, &mut keys, today, &highest);
        });
        self.grid.adapter_mut().keys = keys;
        if !applied {
            return 0;
        }
        self.touched.extend(changed);
        self.error = None;
        n
    }

    /// Ctrl+D (Copy down) or Copy to rows below on the cursor cell.
    pub(crate) fn copy_down(&mut self, scope: CopyScope) -> usize {
        match self.grid.grid().core().cursor() {
            Some(pos) => self.copy_down_at(pos, scope),
            None => 0,
        }
    }

    /// Copy the cell at `pos` to the other rows of the section (every one,
    /// or only those below). Never an amount or a number; never into a
    /// blank row.
    pub(crate) fn copy_down_at(&mut self, pos: CellPos, scope: CopyScope) -> usize {
        if self.busy() || !self.commit_first() {
            return 0;
        }
        let kind = self.shown_kind;
        let Some(f) = field_of(pos.col) else {
            return 0;
        };
        if !ops::copyable(kind, f) {
            self.bulk_note = Some(t("batch_entry.copy.protected"));
            self.rows_changed = 0;
            return 0;
        }
        let n = self.change_section(|s| ops::copy_down(s, pos.row, f, scope));
        self.rows_changed = n;
        self.last_action = Some(match scope {
            CopyScope::AllDisplayed => "copy_down",
            CopyScope::Below => "copy_below",
        });
        self.bulk_note = Some(if n == 0 {
            t("batch_entry.copy.nothing")
        } else {
            t_count(
                "batch_entry.copy.done",
                n,
                &[("column", &t(header_key(kind, f)))],
            )
        });
        n
    }

    /// Select every row of the section (Ctrl+A in the grid; not the blank
    /// entry row).
    pub(crate) fn select_all(&mut self) {
        let lists = Arc::clone(&self.books.lists);
        self.grid.grid_command(GridCommand::SelectAll, &*lists);
        self.last_action = Some("select_all");
    }

    /// Clear the selection (the bar goes away).
    pub(crate) fn clear_selection(&mut self) {
        let lists = Arc::clone(&self.books.lists);
        self.grid.grid_command(GridCommand::ClearSelection, &*lists);
        self.picker = None;
        self.confirm_delete = false;
        self.last_action = Some("clear_selection");
    }

    /// Delete rows: asks first.
    pub(crate) fn ask_delete(&mut self) {
        if self.busy() || self.selected_rows().is_empty() {
            return;
        }
        self.picker = None;
        self.confirm_delete = true;
    }

    /// Delete rows, confirmed: the selected rows go (unsaved rows only;
    /// nothing is sent to the engine). One undo step.
    pub(crate) fn delete_selected(&mut self) -> usize {
        self.confirm_delete = false;
        let rows = self.selected_rows();
        if rows.is_empty() || self.busy() || !self.commit_first() {
            return 0;
        }
        let n = self.change_section(|s| ops::delete_rows(s, &rows));
        let lists = Arc::clone(&self.books.lists);
        self.grid.grid_command(GridCommand::ClearSelection, &*lists);
        self.checks.prune(self.grid.form().current());
        self.rows_changed = n;
        self.last_action = Some("delete_rows");
        self.bulk_note = Some(t_count("batch_entry.delete.done", n, &[]));
        n
    }

    /// "Set on 3 rows." plus, for a hidden column, that it is hidden.
    pub(crate) fn set_note(&self, f: Field, n: usize) -> String {
        let kind = self.shown_kind;
        let column = t(header_key(kind, f));
        if self.choice(kind).is_hidden(f) {
            t_count("batch_entry.set.done_hidden", n, &[("column", &column)])
        } else {
            t_count("batch_entry.set.done", n, &[("column", &column)])
        }
    }
}

#[cfg(test)]
mod tests {
    use cl_grid::RowKeyGen;
    use cl_model::forms::batch::RowDraft;

    use super::*;

    /// The menu's flags agree with `ops::can_copy_down` on every row,
    /// field and scope.
    #[test]
    fn the_menu_offers_copy_down_exactly_when_ops_would_copy() {
        let mut keys = RowKeyGen::new();
        let mut s = Section::new(SectionKind::Checks, None);
        for (date, memo) in [(true, "a"), (false, "b"), (true, ""), (false, "")] {
            let mut r = RowDraft::new(keys.next_key());
            if date {
                r.date = cl_core::CalendarDate::from_ymd(2026, 11, 2).ok();
            }
            r.memo = memo.into();
            s.rows.push(r);
        }
        let menu = BulkMenu {
            kind: s.kind,
            rows: s
                .rows
                .iter()
                .map(|r| (r.key, r.is_blank(), r.date.is_some()))
                .collect(),
            selected: 0,
            choice: ColumnChoice::default_for(s.kind),
        };
        for r in &s.rows {
            for f in s.kind.columns() {
                for scope in [CopyScope::AllDisplayed, CopyScope::Below] {
                    assert_eq!(
                        menu.can_copy(r.key, *f, scope),
                        ops::can_copy_down(&s, r.key, *f, scope),
                        "{f:?} {scope:?}"
                    );
                }
            }
        }
        // Amount is offered, greyed, with the reason.
        let items = menu.items(ContextTarget::Cell(CellPos {
            row: s.rows[0].key,
            col: super::super::grid::column_of(Field::Amount),
        }));
        assert!(!items[0].enabled && items[0].tooltip.is_some());
        assert_eq!(items[0].shortcut, shortcut_of("grid.copy_down"));
        assert!(items[0].shortcut.is_some(), "Ctrl+D shows");
        // The header menu: Amount can't be hidden, Memo can.
        let head = |f| menu.items(ContextTarget::Header(super::super::grid::column_of(f)));
        assert!(!head(Field::Amount)[1].enabled);
        assert!(head(Field::Memo)[1].enabled);
    }
}
