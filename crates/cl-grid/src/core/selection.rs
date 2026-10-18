//! Row selection by [`RowKey`] (P10 plan Rev 2 §2.5, B5, Q-P10n).
//!
//! Selection never stores indexes, so inserts, deletes and reloads above or
//! below never move it onto other rows. Shift ranges are resolved through the
//! model when they are made, and stop at the first row that is not loaded.

use std::collections::BTreeSet;

use super::model::RowModel;
use super::rowkey::RowKey;

/// What a grid allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// No selection (line grids).
    #[default]
    None,
    /// At most one row.
    Single,
    /// Any rows (Space, Ctrl+click, Shift ranges).
    Multi,
}

/// The selected rows and the Shift anchor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection {
    keys: BTreeSet<RowKey>,
    anchor: Option<RowKey>,
}

impl Selection {
    /// Nothing selected.
    #[must_use]
    pub fn new() -> Selection {
        Selection::default()
    }

    /// Is the row selected?
    #[must_use]
    pub fn contains(&self, key: RowKey) -> bool {
        self.keys.contains(&key)
    }

    /// How many rows are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Nothing selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The selected keys, in key order.
    pub fn iter(&self) -> impl Iterator<Item = RowKey> + '_ {
        self.keys.iter().copied()
    }

    /// The Shift anchor.
    #[must_use]
    pub fn anchor(&self) -> Option<RowKey> {
        self.anchor
    }

    /// Clear; true when something was selected.
    pub fn clear(&mut self) -> bool {
        self.anchor = None;
        let had = !self.keys.is_empty();
        self.keys.clear();
        had
    }

    /// Select only this row (a plain click in Single mode).
    pub fn set_single(&mut self, key: RowKey) {
        self.keys.clear();
        self.keys.insert(key);
        self.anchor = Some(key);
    }

    /// Space or Ctrl+click: toggle one row and make it the anchor.
    pub fn toggle(&mut self, key: RowKey, mode: SelectionMode) {
        match mode {
            SelectionMode::None => {}
            SelectionMode::Single => {
                if self.keys.contains(&key) {
                    self.clear();
                } else {
                    self.set_single(key);
                }
            }
            SelectionMode::Multi => {
                if !self.keys.remove(&key) {
                    self.keys.insert(key);
                }
                self.anchor = Some(key);
            }
        }
    }

    /// Shift+click or Shift+arrow: select from the anchor to `to`, through
    /// the model's current order. The range stops at the first row that is
    /// not loaded. Without an anchor (or in Single mode) it selects `to`.
    pub fn extend_to(&mut self, model: &dyn RowModel, to: RowKey, mode: SelectionMode) {
        if mode != SelectionMode::Multi {
            if mode == SelectionMode::Single {
                self.set_single(to);
            }
            return;
        }
        let (Some(anchor), Some(end)) = (self.anchor, model.index_of(to)) else {
            self.toggle(to, mode);
            return;
        };
        let Some(start) = model.index_of(anchor) else {
            self.toggle(to, mode);
            return;
        };
        let step: isize = if end >= start { 1 } else { -1 };
        let mut i = start;
        // A row that is not loaded ends the range.
        while let Some(k) = model.row_key(i) {
            self.keys.insert(k);
            if i == end {
                break;
            }
            match i.checked_add_signed(step) {
                Some(n) => i = n,
                None => break,
            }
        }
    }

    /// Drop keys the model no longer has; true when the selection changed
    /// (the grid then emits `SelectionChanged`).
    pub fn retain_existing(&mut self, model: &dyn RowModel) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| model.index_of(*k).is_some());
        if self.anchor.is_some_and(|a| model.index_of(a).is_none()) {
            self.anchor = None;
        }
        self.keys.len() != before
    }
}
