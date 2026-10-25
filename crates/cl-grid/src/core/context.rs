//! The shared grid context menu (P16 D6, Q-272, Q-274): what the screen
//! offers for a cell or a column header, and the menu's open state.
//!
//! The grid keeps no screen logic. A screen implements
//! [`ContextMenuSource`]; the grid asks it for items when the owner
//! right-clicks a cell or header (or presses Shift+F10 or the Menu key on
//! the cursor cell), shows them, and reports the pick as
//! [`super::spec::GridEvent::ContextPicked`].

use super::model::{CellPos, ColumnId};

/// What a context menu is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContextTarget {
    /// A cell (the cursor moves there before the menu opens).
    Cell(CellPos),
    /// A column header.
    Header(ColumnId),
}

/// One menu entry, with its text already resolved by the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextItem {
    /// The screen's own id for the entry (reported back when picked).
    pub id: u16,
    /// The label ("Copy down").
    pub label: String,
    /// A disabled entry is shown greyed out and cannot be picked.
    pub enabled: bool,
    /// The shortcut shown at the right ("Ctrl+D").
    pub shortcut: Option<String>,
    /// Why it is disabled, or what it does (hover text and the AccessKit
    /// description).
    pub tooltip: Option<String>,
}

impl ContextItem {
    /// An enabled entry with no shortcut or tooltip.
    #[must_use]
    pub fn new(id: u16, label: impl Into<String>) -> ContextItem {
        ContextItem {
            id,
            label: label.into(),
            enabled: true,
            shortcut: None,
            tooltip: None,
        }
    }

    /// Enabled or not.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> ContextItem {
        self.enabled = enabled;
        self
    }

    /// With a shortcut shown at the right.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> ContextItem {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// With hover text.
    #[must_use]
    pub fn tooltip(mut self, tooltip: impl Into<String>) -> ContextItem {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// What a screen offers in a grid's context menu.
pub trait ContextMenuSource {
    /// The entries for `target`, top to bottom. An empty list opens no
    /// menu.
    fn items(&self, target: ContextTarget) -> Vec<ContextItem>;
}

/// An open context menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenu {
    /// What it is about.
    pub target: ContextTarget,
    /// The entries.
    pub items: Vec<ContextItem>,
    /// The highlighted entry (always an enabled one, or none).
    pub highlight: Option<usize>,
}

impl ContextMenu {
    /// A menu with the first enabled entry highlighted.
    #[must_use]
    pub fn new(target: ContextTarget, items: Vec<ContextItem>) -> ContextMenu {
        let highlight = items.iter().position(|i| i.enabled);
        ContextMenu {
            target,
            items,
            highlight,
        }
    }

    /// Move the highlight to the next (`forward`) or previous enabled
    /// entry, wrapping around.
    pub fn step(&mut self, forward: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let start = self.highlight.unwrap_or(if forward { n - 1 } else { 0 });
        let mut i = start;
        for _ in 0..n {
            i = if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
            if self.items[i].enabled {
                self.highlight = Some(i);
                return;
            }
        }
    }

    /// The first (`last == false`) or last enabled entry.
    pub fn jump(&mut self, last: bool) {
        let found = if last {
            self.items.iter().rposition(|i| i.enabled)
        } else {
            self.items.iter().position(|i| i.enabled)
        };
        if found.is_some() {
            self.highlight = found;
        }
    }

    /// The entry at `index` when it can be picked.
    #[must_use]
    pub fn pickable(&self, index: usize) -> Option<&ContextItem> {
        self.items.get(index).filter(|i| i.enabled)
    }
}
