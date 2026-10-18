//! The row-anchored integer viewport (P10 plan §2.8, Rev 2 §2.9; spike §12).
//!
//! The scroll position is a top row plus an offset into it, in 1/64 point
//! units held in `u64`, so positions are exact at any row count (egui's
//! `f32` content offsets drift by whole points beyond 2^23). Uniform grids
//! use O(1) index maths; measured grids (at most 5,000 rows) a prefix table.
//! The top row is also remembered by [`RowKey`], so rows inserted or deleted
//! above never shift the view (B5).

use std::ops::Range;

use super::model::RowModel;
use super::rowkey::RowKey;

/// Viewport units per logical point.
pub const UNITS_PER_PT: u64 = 64;

/// Row heights in units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Heights {
    /// Every row `row` units tall.
    Uniform {
        /// One row's height.
        row: u64,
        /// Number of rows.
        rows: usize,
    },
    /// `prefix[i]` is the top of row `i`; `prefix[rows]` the total.
    Measured {
        /// Running sums, one longer than the row count.
        prefix: Vec<u64>,
    },
}

impl Heights {
    /// `rows` rows of `row` units each (a zero height counts as one unit).
    #[must_use]
    pub fn uniform(row: u64, rows: usize) -> Heights {
        Heights::Uniform {
            row: row.max(1),
            rows,
        }
    }

    /// Measured rows (each at least one unit).
    #[must_use]
    pub fn measured(heights: &[u64]) -> Heights {
        let mut prefix = Vec::with_capacity(heights.len() + 1);
        let mut sum = 0u64;
        prefix.push(0);
        for h in heights {
            sum = sum.saturating_add((*h).max(1));
            prefix.push(sum);
        }
        Heights::Measured { prefix }
    }

    /// Number of rows.
    #[must_use]
    pub fn rows(&self) -> usize {
        match self {
            Heights::Uniform { rows, .. } => *rows,
            Heights::Measured { prefix } => prefix.len().saturating_sub(1),
        }
    }

    /// Total content height.
    #[must_use]
    pub fn total(&self) -> u64 {
        match self {
            Heights::Uniform { row, rows } => row.saturating_mul(*rows as u64),
            Heights::Measured { prefix } => prefix.last().copied().unwrap_or(0),
        }
    }

    /// Top of row `i` (the total for `i == rows`).
    #[must_use]
    pub fn top_of(&self, i: usize) -> u64 {
        match self {
            Heights::Uniform { row, rows } => row.saturating_mul(i.min(*rows) as u64),
            Heights::Measured { prefix } => prefix.get(i).or(prefix.last()).copied().unwrap_or(0),
        }
    }

    /// Height of row `i` (0 past the end).
    #[must_use]
    pub fn height_of(&self, i: usize) -> u64 {
        if i >= self.rows() {
            return 0;
        }
        self.top_of(i + 1) - self.top_of(i)
    }

    /// The row containing content position `pos` (the last row past the
    /// end; 0 when empty).
    #[must_use]
    pub fn row_at(&self, pos: u64) -> usize {
        let n = self.rows();
        if n == 0 {
            return 0;
        }
        let i = match self {
            Heights::Uniform { row, .. } => usize::try_from(pos / row).unwrap_or(usize::MAX),
            // The last prefix entry <= pos.
            Heights::Measured { prefix } => prefix.partition_point(|t| *t <= pos).saturating_sub(1),
        };
        i.min(n - 1)
    }
}

/// Where the viewport shows a row asked for by [`Viewport::scroll_to`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAlign {
    /// Scroll as little as possible.
    Nearest,
    /// Put the row at the top.
    Top,
    /// Put the row at the bottom.
    Bottom,
}

/// The scroll state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Viewport {
    top_key: Option<RowKey>,
    top: usize,
    offset: u64,
    height: u64,
}

impl Viewport {
    /// A viewport `height` units tall at the start.
    #[must_use]
    pub fn new(height: u64) -> Viewport {
        Viewport {
            height,
            ..Viewport::default()
        }
    }

    /// The first visible row.
    #[must_use]
    pub fn top(&self) -> usize {
        self.top
    }

    /// The first visible row's key.
    #[must_use]
    pub fn top_key(&self) -> Option<RowKey> {
        self.top_key
    }

    /// How far the first visible row is scrolled off the top, in units.
    #[must_use]
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The viewport height in units.
    #[must_use]
    pub fn height(&self) -> u64 {
        self.height
    }

    /// The absolute content position of the viewport's top edge.
    #[must_use]
    pub fn pos(&self, h: &Heights) -> u64 {
        h.top_of(self.top).saturating_add(self.offset)
    }

    /// The largest scroll position.
    #[must_use]
    pub fn max_pos(&self, h: &Heights) -> u64 {
        h.total().saturating_sub(self.height)
    }

    /// Change the height (window resize, zoom) and keep the top row.
    pub fn set_height(&mut self, height: u64, h: &Heights, model: &dyn RowModel) {
        self.height = height;
        let p = self.pos(h);
        self.set_pos(p, h, model);
    }

    /// Scroll to an absolute position (clamped).
    pub fn set_pos(&mut self, pos: u64, h: &Heights, model: &dyn RowModel) {
        let pos = pos.min(self.max_pos(h));
        self.top = h.row_at(pos);
        self.offset = pos - h.top_of(self.top).min(pos);
        self.top_key = model.row_key(self.top);
    }

    /// Scroll by a signed amount (wheel, drag).
    pub fn scroll_by(&mut self, delta: i64, h: &Heights, model: &dyn RowModel) {
        let p = self.pos(h).saturating_add_signed(delta);
        self.set_pos(p, h, model);
    }

    /// Whole rows that fit in the viewport (at least one).
    #[must_use]
    pub fn page_rows(&self, h: &Heights) -> usize {
        match h {
            Heights::Uniform { row, .. } => {
                usize::try_from((self.height / row).max(1)).unwrap_or(usize::MAX)
            }
            Heights::Measured { .. } => {
                let first = self.top;
                let mut n = 0usize;
                let mut used = 0u64;
                while first + n < h.rows() {
                    used = used.saturating_add(h.height_of(first + n));
                    if used > self.height {
                        break;
                    }
                    n += 1;
                }
                n.max(1)
            }
        }
    }

    /// Page Up (`forward == false`) or Page Down: by whole rows for uniform
    /// heights, by the viewport height for measured ones (N3).
    pub fn page(&mut self, forward: bool, h: &Heights, model: &dyn RowModel) {
        let by = match h {
            Heights::Uniform { row, .. } => row.saturating_mul(self.page_rows(h) as u64),
            Heights::Measured { .. } => self.height.max(1),
        };
        let p = self.pos(h);
        let p = if forward {
            p.saturating_add(by)
        } else {
            p.saturating_sub(by)
        };
        self.set_pos(p, h, model);
    }

    /// Ctrl+Home.
    pub fn to_start(&mut self, h: &Heights, model: &dyn RowModel) {
        self.set_pos(0, h, model);
    }

    /// Ctrl+End: the last row's bottom exactly at the viewport's bottom.
    pub fn to_end(&mut self, h: &Heights, model: &dyn RowModel) {
        let p = self.max_pos(h);
        self.set_pos(p, h, model);
    }

    /// Make row `i` fully visible (a row taller than the viewport is shown
    /// from its top).
    pub fn ensure_visible(&mut self, i: usize, h: &Heights, model: &dyn RowModel) {
        self.scroll_to(i, RowAlign::Nearest, h, model);
    }

    /// Scroll so row `i` is placed as asked.
    pub fn scroll_to(&mut self, i: usize, align: RowAlign, h: &Heights, model: &dyn RowModel) {
        if h.rows() == 0 {
            self.set_pos(0, h, model);
            return;
        }
        let i = i.min(h.rows() - 1);
        let top = h.top_of(i);
        let bottom = top.saturating_add(h.height_of(i));
        let pos = self.pos(h);
        let fits = h.height_of(i) <= self.height;
        let target = match align {
            RowAlign::Bottom if fits => bottom - self.height,
            RowAlign::Top | RowAlign::Bottom => top,
            RowAlign::Nearest => {
                if top < pos || !fits {
                    top
                } else if bottom > pos.saturating_add(self.height) {
                    bottom - self.height
                } else {
                    pos
                }
            }
        };
        self.set_pos(target, h, model);
    }

    /// The rows at least partly visible.
    #[must_use]
    pub fn visible_range(&self, h: &Heights) -> Range<usize> {
        let n = h.rows();
        if n == 0 {
            return 0..0;
        }
        let pos = self.pos(h);
        let end_pos = pos.saturating_add(self.height);
        let mut end = h.row_at(end_pos.saturating_sub(1)) + 1;
        if self.height == 0 {
            end = self.top;
        }
        self.top.min(n)..end.min(n)
    }

    /// Where row `i`'s top is relative to the viewport top (negative when
    /// partly scrolled off), in units. Small enough to convert to points.
    #[must_use]
    pub fn local_top(&self, i: usize, h: &Heights) -> i64 {
        let pos = self.pos(h);
        let top = h.top_of(i);
        if top >= pos {
            i64::try_from(top - pos).unwrap_or(i64::MAX)
        } else {
            -i64::try_from(pos - top).unwrap_or(i64::MAX)
        }
    }

    /// Keep the view on the same row after the model changed: when the top
    /// row's key moved, follow it; when it is gone, keep the index (clamped).
    pub fn resync(&mut self, h: &Heights, model: &dyn RowModel) {
        if let Some(key) = self.top_key
            && model.row_key(self.top) != Some(key)
            && let Some(i) = model.index_of(key)
        {
            self.top = i;
        }
        let n = h.rows();
        if n == 0 {
            self.top = 0;
            self.offset = 0;
        } else if self.top >= n {
            self.top = n - 1;
            self.offset = 0;
        }
        self.offset = self.offset.min(h.height_of(self.top).saturating_sub(1));
        // Re-clamp (the content may have shrunk below the viewport).
        let p = self.pos(h);
        self.set_pos(p, h, model);
    }

    /// The scrollbar thumb for a track `track` units long: `(start, len)`,
    /// at least `min_len` long, from integer ratios only.
    #[must_use]
    pub fn thumb(&self, track: u64, min_len: u64, h: &Heights) -> (u64, u64) {
        let total = h.total();
        if total <= self.height || track == 0 {
            return (0, track);
        }
        let len = (u128::from(track) * u128::from(self.height) / u128::from(total))
            .max(u128::from(min_len.min(track)));
        let len = u64::try_from(len).unwrap_or(track).min(track);
        let free = track - len;
        let max = self.max_pos(h);
        let start = u128::from(free) * u128::from(self.pos(h)) / u128::from(max.max(1));
        (u64::try_from(start).unwrap_or(free).min(free), len)
    }

    /// The scroll position for a thumb dragged to `start` on a track of
    /// `track` units with a thumb `len` long (the inverse of [`Viewport::thumb`]).
    #[must_use]
    pub fn pos_for_thumb(&self, start: u64, track: u64, len: u64, h: &Heights) -> u64 {
        let free = track.saturating_sub(len);
        if free == 0 {
            return 0;
        }
        let max = self.max_pos(h);
        let p = u128::from(start.min(free)) * u128::from(max) / u128::from(free);
        u64::try_from(p).unwrap_or(max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_lookup() {
        let h = Heights::measured(&[10, 20, 30]);
        assert_eq!(h.total(), 60);
        assert_eq!(h.rows(), 3);
        assert_eq!(
            (
                h.row_at(0),
                h.row_at(9),
                h.row_at(10),
                h.row_at(59),
                h.row_at(99)
            ),
            (0, 0, 1, 2, 2)
        );
        assert_eq!(h.height_of(1), 20);
        assert_eq!(h.height_of(3), 0);
        assert_eq!(Heights::measured(&[]).row_at(5), 0);
        assert_eq!(
            Heights::uniform(0, 3).total(),
            3,
            "zero heights count as one unit"
        );
    }
}
