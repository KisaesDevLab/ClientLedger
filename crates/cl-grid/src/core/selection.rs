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
    /// Keys the current Shift extension added (not selected before it), so
    /// the next extension from the same anchor can replace the range.
    range: BTreeSet<RowKey>,
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

    /// Set the Shift anchor when there is none (the cursor row before a
    /// Shift move), so the range starts where the owner was.
    pub fn anchor_if_none(&mut self, key: RowKey) {
        if self.anchor.is_none() {
            self.anchor = Some(key);
            self.range.clear();
        }
    }

    /// Clear; true when something was selected.
    pub fn clear(&mut self) -> bool {
        self.anchor = None;
        self.range.clear();
        let had = !self.keys.is_empty();
        self.keys.clear();
        had
    }

    /// Select only this row (a plain click in Single mode).
    pub fn set_single(&mut self, key: RowKey) {
        self.keys.clear();
        self.keys.insert(key);
        self.anchor = Some(key);
        self.range.clear();
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
                self.range.clear();
            }
        }
    }

    /// Shift+click or Shift+arrow: select from the anchor to `to`, through
    /// the model's current order. The range the previous extension from the
    /// same anchor added is replaced, so it grows and shrinks. The range
    /// stops at the first row that is not loaded. Without an anchor (or in
    /// Single mode) it selects `to` and makes it the anchor; it never
    /// deselects rows selected before the extension.
    pub fn extend_to(&mut self, model: &dyn RowModel, to: RowKey, mode: SelectionMode) {
        if mode != SelectionMode::Multi {
            if mode == SelectionMode::Single {
                self.set_single(to);
            }
            return;
        }
        let Some(end) = model.index_of(to) else {
            return;
        };
        let start = self.anchor.and_then(|a| model.index_of(a));
        let Some(start) = start else {
            self.anchor = Some(to);
            self.range.clear();
            if self.keys.insert(to) {
                self.range.insert(to);
            }
            return;
        };
        for k in std::mem::take(&mut self.range) {
            self.keys.remove(&k);
        }
        let step: isize = if end >= start { 1 } else { -1 };
        let mut i = start;
        // A row that is not loaded ends the range.
        while let Some(k) = model.row_key(i) {
            if self.keys.insert(k) {
                self.range.insert(k);
            }
            if i == end {
                break;
            }
            match i.checked_add_signed(step) {
                Some(n) => i = n,
                None => break,
            }
        }
    }

    /// Select exactly `keys` (Select all); the anchor stays when it is
    /// among them. True when the selection changed.
    pub fn select_only(&mut self, keys: impl IntoIterator<Item = RowKey>) -> bool {
        let keys: BTreeSet<RowKey> = keys.into_iter().collect();
        self.range.clear();
        if self.anchor.is_some_and(|a| !keys.contains(&a)) {
            self.anchor = None;
        }
        let changed = keys != self.keys;
        self.keys = keys;
        changed
    }

    /// Unselect one row (the blank ENTRY row a range ran onto); true when
    /// it was selected.
    pub fn remove(&mut self, key: RowKey) -> bool {
        self.range.remove(&key);
        self.keys.remove(&key)
    }

    /// Drop keys the model no longer has; true when the selection changed
    /// (the grid then emits `SelectionChanged`).
    pub fn retain_existing(&mut self, model: &dyn RowModel) -> bool {
        let before = self.keys.len();
        self.keys.retain(|k| model.index_of(*k).is_some());
        self.range.retain(|k| model.index_of(*k).is_some());
        if self.anchor.is_some_and(|a| model.index_of(a).is_none()) {
            self.anchor = None;
        }
        self.keys.len() != before
    }
}
