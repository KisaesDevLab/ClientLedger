//! The page window over the engine's register (P12 plan WP4, §5 "Large
//! data").
//!
//! The engine pages the register by OFFSET, 200 lines a page. The window
//! keeps at most [`MAX_PAGES`] of them, asks for the pages the owner can see
//! plus one page either side, and turns every row it does not hold into
//! [`RowView::Loading`]. It never makes a row up: a `Ready` row is exactly
//! what the engine sent for the current generation.
//!
//! Every request carries a ticket. A reply whose ticket is no longer current
//! (the page scrolled away, the ledger changed, the query changed) is
//! dropped, so a late reply can never overwrite newer rows.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

/// Lines per engine page (`per_page`).
pub const PAGE: u64 = 200;
/// Pages kept in memory (12,800 lines).
pub const MAX_PAGES: usize = 64;

/// A zero-based page number. The engine counts from one
/// ([`PageNo::engine_page`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageNo(pub u64);

impl PageNo {
    /// The page holding row `row`.
    #[must_use]
    pub const fn of_row(row: u64) -> PageNo {
        PageNo(row / PAGE)
    }

    /// The engine's `page` parameter.
    #[must_use]
    pub const fn engine_page(self) -> u64 {
        self.0 + 1
    }

    /// Index of this page's first row.
    #[must_use]
    pub const fn first_row(self) -> u64 {
        self.0 * PAGE
    }
}

/// One page to fetch. Hand it back with the reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageRequest {
    /// The page.
    pub page: PageNo,
    /// The window generation it was asked for in.
    pub generation: u64,
    serial: u64,
}

/// What became of a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reply {
    /// Stored (or, for a failure, recorded).
    Taken,
    /// The request was no longer current; nothing changed.
    Dropped,
}

/// What a row shows.
#[derive(Debug, PartialEq, Eq)]
pub enum RowView<'a, R> {
    /// The engine's row for the current generation.
    Ready(&'a R),
    /// An older generation's row, kept on screen in the loading style until
    /// the new one arrives (no flash to empty). Not editable.
    Stale(&'a R),
    /// Not fetched yet.
    Loading,
    /// The page failed; "Couldn't load — Try again" ([`WindowCache::retry`]).
    Failed,
}

#[derive(Debug)]
struct CachedPage<R> {
    generation: u64,
    rows: Vec<R>,
    used: u64,
}

/// The page window. `R` is the screen's row type.
#[derive(Debug)]
pub struct WindowCache<R> {
    query_fp: u64,
    generation: u64,
    total_rows: Option<u64>,
    total_fresh: bool,
    pages: BTreeMap<u64, CachedPage<R>>,
    in_flight: BTreeMap<u64, (u64, u64)>,
    failed: BTreeSet<u64>,
    next_serial: u64,
    tick: u64,
}

impl<R> WindowCache<R> {
    /// An empty window for the query with fingerprint `query_fp` (range,
    /// filters and sort, computed by the screen).
    #[must_use]
    pub fn new(query_fp: u64) -> WindowCache<R> {
        WindowCache {
            query_fp,
            generation: 0,
            total_rows: None,
            total_fresh: false,
            pages: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            failed: BTreeSet::new(),
            next_serial: 0,
            tick: 0,
        }
    }

    /// The current query fingerprint.
    #[must_use]
    pub fn query_fp(&self) -> u64 {
        self.query_fp
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Lines in the register, once a page has told us.
    #[must_use]
    pub fn total_rows(&self) -> Option<u64> {
        self.total_rows
    }

    /// Pages in the register, once known.
    #[must_use]
    pub fn total_pages(&self) -> Option<u64> {
        self.total_rows.map(|t| t.div_ceil(PAGE))
    }

    /// The last page, once known and not empty.
    #[must_use]
    pub fn last_page(&self) -> Option<PageNo> {
        self.total_pages()
            .and_then(|n| n.checked_sub(1))
            .map(PageNo)
    }

    /// Pages held in memory.
    #[must_use]
    pub fn cached_pages(&self) -> usize {
        self.pages.len()
    }

    /// Requests not yet answered.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Switches to another query (range, filters or sort changed). Every
    /// row and request of the old query is dropped, because its row numbers
    /// mean other lines. Returns false when the fingerprint is the same.
    pub fn set_query(&mut self, query_fp: u64) -> bool {
        if query_fp == self.query_fp {
            return false;
        }
        self.query_fp = query_fp;
        self.generation += 1;
        self.total_rows = None;
        self.total_fresh = false;
        self.pages.clear();
        self.in_flight.clear();
        self.failed.clear();
        true
    }

    /// The ledger changed (stamp moved, own write, reconnect). Every row
    /// becomes [`RowView::Stale`] until its page is fetched again, and every
    /// request in flight is dropped on arrival.
    pub fn invalidate(&mut self) {
        self.generation += 1;
        self.total_fresh = false;
        self.in_flight.clear();
        self.failed.clear();
    }

    /// The pages to fetch now for the rows `visible` (row indices, end
    /// exclusive): the visible pages first, then one page either side.
    /// Pages already held for this generation, in flight or failed are not
    /// asked again, and nothing beyond the known end is asked. Requests for
    /// pages that left this set are dropped (their replies will be too).
    ///
    /// Before the size of the register is known only the first page is
    /// asked, which tells it.
    pub fn wanted(&mut self, visible: Range<u64>) -> Vec<PageRequest> {
        self.tick += 1;
        let targets = self.targets(&visible);
        for p in &targets[..targets.len().min(self.visible_count(&visible))] {
            if let Some(page) = self.pages.get_mut(p) {
                page.used = self.tick;
            }
        }
        self.in_flight.retain(|p, _| targets.contains(p));
        let mut out = Vec::new();
        for p in targets {
            let held = self
                .pages
                .get(&p)
                .is_some_and(|c| c.generation == self.generation);
            if held || self.in_flight.contains_key(&p) || self.failed.contains(&p) {
                continue;
            }
            let serial = self.next_serial;
            self.next_serial += 1;
            self.in_flight.insert(p, (serial, self.generation));
            out.push(PageRequest {
                page: PageNo(p),
                generation: self.generation,
                serial,
            });
        }
        out
    }

    fn visible_pages(&self, visible: &Range<u64>) -> Vec<u64> {
        let Some(total_pages) = self.total_pages() else {
            return Vec::new();
        };
        if visible.is_empty() || total_pages == 0 {
            return Vec::new();
        }
        let first = visible.start / PAGE;
        let last = ((visible.end - 1) / PAGE).min(total_pages - 1);
        (first..=last).collect()
    }

    fn visible_count(&self, visible: &Range<u64>) -> usize {
        self.visible_pages(visible).len()
    }

    fn targets(&self, visible: &Range<u64>) -> Vec<u64> {
        let Some(total_pages) = self.total_pages() else {
            return vec![0];
        };
        let mut targets = self.visible_pages(visible);
        if let (Some(&first), Some(&last)) = (targets.first(), targets.last()) {
            if first > 0 {
                targets.push(first - 1);
            }
            if last + 1 < total_pages {
                targets.push(last + 1);
            }
        }
        if targets.is_empty() && !self.total_fresh {
            // Nothing visible (an empty register, or a view past a shrunken
            // end): page one still tells the new size.
            targets.push(0);
        }
        targets
    }

    /// True while `req` is still wanted; the request queue skips a request
    /// that is no longer current instead of sending it.
    #[must_use]
    pub fn is_current(&self, req: &PageRequest) -> bool {
        self.in_flight.get(&req.page.0) == Some(&(req.serial, req.generation))
    }

    /// Stores the engine's reply to `req`: the register's `total_rows` and
    /// the page's rows, exactly as sent. A reply that is no longer current
    /// is dropped. Pages past a smaller total are forgotten; the least
    /// recently used page goes when more than [`MAX_PAGES`] are held.
    pub fn accept(&mut self, req: PageRequest, total_rows: u64, mut rows: Vec<R>) -> Reply {
        if !self.is_current(&req) {
            return Reply::Dropped;
        }
        self.in_flight.remove(&req.page.0);
        self.total_rows = Some(total_rows);
        self.total_fresh = true;
        let total_pages = total_rows.div_ceil(PAGE);
        self.pages.retain(|p, _| *p < total_pages);
        self.failed.retain(|p| *p < total_pages);
        self.in_flight.retain(|p, _| *p < total_pages);
        // The engine sends at most `per_page` rows; more are not ours.
        rows.truncate(usize::try_from(PAGE).unwrap_or(usize::MAX));
        if req.page.0 < total_pages {
            self.pages.insert(
                req.page.0,
                CachedPage {
                    generation: self.generation,
                    rows,
                    used: self.tick,
                },
            );
        }
        self.evict(req.page.0);
        Reply::Taken
    }

    /// Records that `req` failed; its rows show [`RowView::Failed`] until
    /// [`WindowCache::retry`].
    pub fn fail(&mut self, req: PageRequest) -> Reply {
        if !self.is_current(&req) {
            return Reply::Dropped;
        }
        self.in_flight.remove(&req.page.0);
        self.failed.insert(req.page.0);
        Reply::Taken
    }

    /// "Try again" on a failed page: it is asked again by the next
    /// [`WindowCache::wanted`].
    pub fn retry(&mut self, page: PageNo) {
        self.failed.remove(&page.0);
    }

    fn evict(&mut self, keep: u64) {
        while self.pages.len() > MAX_PAGES {
            // Older generations go first, then the least recently used.
            let victim = self
                .pages
                .iter()
                .filter(|(p, _)| **p != keep)
                .min_by_key(|(_, c)| (c.generation == self.generation, c.used))
                .map(|(p, _)| *p);
            match victim {
                Some(p) => {
                    self.pages.remove(&p);
                }
                None => break,
            }
        }
    }

    /// What row `row` shows; `None` past the known end.
    #[must_use]
    pub fn row(&self, row: u64) -> Option<RowView<'_, R>> {
        if self.total_rows.is_some_and(|t| row >= t) {
            return None;
        }
        let page = row / PAGE;
        let at = usize::try_from(row % PAGE).ok()?;
        if let Some(c) = self.pages.get(&page)
            && let Some(r) = c.rows.get(at)
        {
            return Some(if c.generation == self.generation {
                RowView::Ready(r)
            } else {
                RowView::Stale(r)
            });
        }
        if self.failed.contains(&page) {
            return Some(RowView::Failed);
        }
        Some(RowView::Loading)
    }

    /// `row` moved inside the register after its size changed (the anchor
    /// row of the viewport stays on a real line).
    #[must_use]
    pub fn clamp_row(&self, row: u64) -> u64 {
        match self.total_rows {
            Some(t) if t > 0 => row.min(t - 1),
            _ => 0,
        }
    }
}
