//! The run state of a Report Builder report window (A07 plan §4), pure: no
//! egui, no I/O, no clock. The window feeds it events and the current time
//! in milliseconds, and performs the [`Effect`]s it returns.
//!
//! ```text
//! Empty -> Running{gen, open} -> Shown | Failed | TimedOut | Unavailable(cap)
//! Shown|Running --expand(p)/collapse(p)--> pending open updated;
//!     Running{gen+1, cumulative open} (debounced 150 ms)
//!   collapse also hides p's rows at once and cancels any Paging under p
//! Running(gen) superseded --> request cancelled (one in-flight run)
//! Shown --More(p)--> Paging{gen, p, cursor} -> rows replace the More row;
//!   dropped if p closed or gen changed
//!   409 RUN_CHANGED -> banner (books) | rerun (params)
//! Running failed/timed out after expand --> keep last table, revert open,
//!   "Try again"
//! Rerun with same stamp --> re-request loaded pages per path
//! definitionId 409 STALE_VERSION --> reload definition, rerun; 404 --> deleted
//! Expand all --> openAll; Collapse all --> open = []
//! ```
//!
//! The client's open set is authoritative: rows the engine sends under a
//! path the set doesn't hold are hidden ([`BuilderRunState::hidden`]).

use std::collections::{BTreeMap, BTreeSet};

use cl_core::report::{Cell, RowPath};

/// The debounce between an expand or collapse and the run it causes.
pub const DEBOUNCE_MS: u64 = 150;

/// Why a run or page failed, as the window maps it from the engine's
/// answer (cl-model can't see cl-api's errors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The books changed since the run (409 `RUN_CHANGED {reason:'books'}`).
    BooksChanged,
    /// The run's parameters changed (409 `RUN_CHANGED {reason:'params'}`).
    ParamsChanged,
    /// Over 30 s (503 `REPORT_TIMEOUT`).
    Timeout,
    /// Over 100,000 group rows (422 `REPORT_TOO_LARGE`).
    TooLarge,
    /// The memorized definition changed (409 `STALE_VERSION`).
    StaleDefinition,
    /// The memorized definition was deleted (404).
    Deleted,
    /// The engine can't run builder reports yet (Q-220).
    NeedsEngineUpdate,
    /// Anything else (offline engine, a shape error...).
    Other,
}

/// Why the report can't show at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    /// Too many group rows: "Add a filter or fewer group-bys".
    TooLarge,
    /// The engine needs an update (Q-220).
    NeedsEngineUpdate,
}

/// Where the window stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing asked yet.
    Empty,
    /// A run is in flight.
    Running,
    /// The table is current.
    Shown,
    /// The first run failed (no table to keep).
    Failed,
    /// The first run took too long.
    TimedOut,
    /// It can't run.
    Unavailable(Cap),
    /// "This memorized report was deleted".
    Deleted,
}

impl Phase {
    /// The control channel's name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Phase::Empty => "empty",
            Phase::Running => "running",
            Phase::Shown => "shown",
            Phase::Failed => "failed",
            Phase::TimedOut => "timed_out",
            Phase::Unavailable(Cap::TooLarge) => "too_large",
            Phase::Unavailable(Cap::NeedsEngineUpdate) => "unavailable",
            Phase::Deleted => "deleted",
        }
    }
}

/// A message over the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Banner {
    /// "The books changed since this report ran. Refresh".
    BooksChanged,
    /// The last expand, collapse or page failed; the table is the last
    /// good one ("Try again").
    TryAgain,
    /// It took too long ("Try fewer months or add a filter").
    TookTooLong,
}

/// What the window must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Run generation `generation` with these open paths (`None`: the sections'
    /// defaults, first run only) or everything open.
    Run {
        /// The generation the answer must carry.
        generation: u64,
        /// Open paths.
        open: Option<Vec<RowPath>>,
        /// Expand all.
        open_all: bool,
    },
    /// Cancel the request of generation `generation` (one in flight per window).
    Cancel {
        /// The superseded generation.
        generation: u64,
    },
    /// Fetch the page after `cursor` under `path` for generation `generation`.
    Page {
        /// The generation.
        generation: u64,
        /// The section.
        node_id: String,
        /// The path.
        path: RowPath,
        /// The cursor.
        cursor: String,
    },
    /// After a rerun on the same stamp: load `pages` pages under `path`
    /// again, so "Load more" isn't lost.
    Repage {
        /// The generation.
        generation: u64,
        /// The path.
        path: RowPath,
        /// How many pages were loaded.
        pages: usize,
    },
    /// Read the memorized definition again (409 `STALE_VERSION`), then run.
    ReloadDefinition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Paging {
    generation: u64,
    path: RowPath,
}

/// The builder window's run state.
#[derive(Clone, Debug)]
pub struct BuilderRunState {
    phase: Phase,
    generation: u64,
    in_flight: Option<u64>,
    /// The open set of the table shown (authoritative once shown).
    open: BTreeSet<RowPath>,
    /// The open set asked for and not yet shown: shown at once (collapse
    /// hides rows immediately), sent when the debounce ends.
    pending: Option<BTreeSet<RowPath>>,
    /// The open set of the run in flight (kept on success, reverted on
    /// failure).
    asked: Option<BTreeSet<RowPath>>,
    due_at: Option<u64>,
    open_all: bool,
    first_run: bool,
    has_table: bool,
    stamp: Option<String>,
    paging: Option<Paging>,
    pages: BTreeMap<RowPath, usize>,
    banner: Option<Banner>,
}

impl Default for BuilderRunState {
    fn default() -> Self {
        BuilderRunState::new()
    }
}

impl BuilderRunState {
    /// Nothing run yet.
    #[must_use]
    pub fn new() -> BuilderRunState {
        BuilderRunState {
            phase: Phase::Empty,
            generation: 0,
            in_flight: None,
            open: BTreeSet::new(),
            pending: None,
            asked: None,
            due_at: None,
            open_all: false,
            first_run: true,
            has_table: false,
            stamp: None,
            paging: None,
            pages: BTreeMap::new(),
            banner: None,
        }
    }

    /// Where it stands.
    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    /// The current generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The message over the table, if any.
    #[must_use]
    pub const fn banner(&self) -> Option<Banner> {
        self.banner
    }

    /// The open paths the screen follows (pending ones first).
    #[must_use]
    pub fn open_paths(&self) -> &BTreeSet<RowPath> {
        self.pending.as_ref().unwrap_or(&self.open)
    }

    /// Everything is open.
    #[must_use]
    pub const fn is_open_all(&self) -> bool {
        self.open_all
    }

    /// True when `path` is open.
    #[must_use]
    pub fn is_open(&self, path: &RowPath) -> bool {
        self.open_all || self.open_paths().contains(path)
    }

    /// A page is being fetched under `path`.
    #[must_use]
    pub fn is_paging(&self, path: &RowPath) -> bool {
        self.paging.as_ref().is_some_and(|p| &p.path == path)
    }

    /// Which rows to hide: each row whose path lies under an expandable
    /// row's path that isn't open. `rows` is (path, expandable) in table
    /// order.
    #[must_use]
    pub fn hidden(&self, rows: &[(RowPath, bool)]) -> Vec<bool> {
        if self.open_all {
            return vec![false; rows.len()];
        }
        let closed: Vec<&RowPath> = rows
            .iter()
            .filter(|(p, expandable)| *expandable && !self.is_open(p))
            .map(|(p, _)| p)
            .collect();
        rows.iter()
            .map(|(p, _)| closed.iter().any(|c| p.is_under(c)))
            .collect()
    }

    fn next_run(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        if let Some(old) = self.in_flight.take() {
            fx.push(Effect::Cancel { generation: old });
        }
        self.generation += 1;
        self.in_flight = Some(self.generation);
        self.paging = None;
        self.due_at = None;
        let open = if self.first_run {
            None
        } else {
            let set = self.pending.clone().unwrap_or_else(|| self.open.clone());
            let list = set.iter().cloned().collect();
            self.asked = Some(set);
            Some(list)
        };
        self.phase = Phase::Running;
        fx.push(Effect::Run {
            generation: self.generation,
            open,
            open_all: self.open_all,
        });
        fx
    }

    /// The first run, or Refresh (F5): runs now.
    pub fn refresh(&mut self) -> Vec<Effect> {
        self.banner = None;
        self.next_run()
    }

    fn toggle(&mut self, path: &RowPath, open: bool, now_ms: u64) -> Vec<Effect> {
        let mut set = if self.open_all {
            // Leaving "expand all": every path shown open stays open.
            self.open_all = false;
            self.open.clone()
        } else {
            self.open_paths().clone()
        };
        if open {
            set.insert(path.clone());
        } else {
            set.retain(|p| p != path && !p.is_under(path));
            if self
                .paging
                .as_ref()
                .is_some_and(|pg| &pg.path == path || pg.path.is_under(path))
            {
                self.paging = None;
            }
            self.pages.retain(|p, _| p != path && !p.is_under(path));
        }
        self.pending = Some(set);
        self.due_at = Some(now_ms + DEBOUNCE_MS);
        self.banner = None;
        Vec::new()
    }

    /// Opens `path` (the run follows after the debounce).
    pub fn expand(&mut self, path: &RowPath, now_ms: u64) -> Vec<Effect> {
        self.toggle(path, true, now_ms)
    }

    /// Closes `path`: its rows hide at once, a page under it is dropped,
    /// and the run follows after the debounce.
    pub fn collapse(&mut self, path: &RowPath, now_ms: u64) -> Vec<Effect> {
        self.toggle(path, false, now_ms)
    }

    /// Expand all: one run with `openAll` (bounded by the engine's caps).
    pub fn expand_all(&mut self) -> Vec<Effect> {
        self.open_all = true;
        self.first_run = false;
        self.pending = None;
        self.next_run()
    }

    /// Collapse all: `open = []`.
    pub fn collapse_all(&mut self) -> Vec<Effect> {
        self.open_all = false;
        self.first_run = false;
        self.pages.clear();
        self.pending = Some(BTreeSet::new());
        self.next_run()
    }

    /// Call each frame: sends the debounced run when it is due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.due_at {
            Some(t) if now_ms >= t => {
                self.first_run = false;
                self.next_run()
            }
            _ => Vec::new(),
        }
    }

    /// The run of `generation` answered with `stamp` and the open paths the
    /// engine expanded on a first run (`defaultExpanded`).
    pub fn run_ok(&mut self, generation: u64, stamp: &str, opened: &[RowPath]) -> Vec<Effect> {
        if generation != self.generation {
            return Vec::new(); // a superseded answer
        }
        self.in_flight = None;
        if self.first_run {
            self.open = opened.iter().cloned().collect();
            self.first_run = false;
        } else if let Some(asked) = self.asked.take() {
            self.open = asked;
        }
        // An expand made while this ran stays pending (its run is due).
        if self.due_at.is_none() {
            self.pending = None;
        }
        self.phase = Phase::Shown;
        self.has_table = true;
        let same_books = self.stamp.as_deref() == Some(stamp);
        self.stamp = Some(stamp.to_string());
        if !same_books {
            self.pages.clear();
            return Vec::new();
        }
        // Collapse already dropped the pages under a closed path.
        self.pages
            .iter()
            .map(|(path, pages)| Effect::Repage {
                generation,
                path: path.clone(),
                pages: *pages,
            })
            .collect()
    }

    /// The run of `generation` failed.
    pub fn run_failed(&mut self, generation: u64, why: Failure) -> Vec<Effect> {
        if generation != self.generation {
            return Vec::new();
        }
        self.in_flight = None;
        self.asked = None;
        if self.due_at.is_none() {
            self.pending = None; // revert the open set to the shown one
        }
        match why {
            Failure::StaleDefinition => {
                self.phase = Phase::Running;
                return vec![Effect::ReloadDefinition];
            }
            Failure::Deleted => self.phase = Phase::Deleted,
            Failure::TooLarge => self.phase = Phase::Unavailable(Cap::TooLarge),
            Failure::NeedsEngineUpdate => {
                self.phase = Phase::Unavailable(Cap::NeedsEngineUpdate);
            }
            Failure::Timeout if self.has_table => {
                self.phase = Phase::Shown;
                self.banner = Some(Banner::TookTooLong);
            }
            Failure::Timeout => self.phase = Phase::TimedOut,
            _ if self.has_table => {
                self.phase = Phase::Shown;
                self.banner = Some(Banner::TryAgain);
            }
            _ => self.phase = Phase::Failed,
        }
        Vec::new()
    }

    /// The definition was read again after `STALE_VERSION`: run it.
    pub fn definition_reloaded(&mut self) -> Vec<Effect> {
        self.next_run()
    }

    /// "Load more" on a `more` row of `node_id` at `path`.
    pub fn more(&mut self, node_id: &str, path: &RowPath, cursor: &str) -> Vec<Effect> {
        if self.phase != Phase::Shown || self.paging.is_some() {
            return Vec::new();
        }
        self.paging = Some(Paging {
            generation: self.generation,
            path: path.clone(),
        });
        vec![Effect::Page {
            generation: self.generation,
            node_id: node_id.to_string(),
            path: path.clone(),
            cursor: cursor.to_string(),
        }]
    }

    /// A page for `path` of `generation` arrived: `true` when the window should
    /// put its rows in place of the `more` row (dropped when the path
    /// closed or a newer run started).
    pub fn page_ok(&mut self, generation: u64, path: &RowPath) -> bool {
        let wanted = Paging {
            generation,
            path: path.clone(),
        };
        if self.paging.as_ref() != Some(&wanted) || generation != self.generation {
            return false;
        }
        self.paging = None;
        *self.pages.entry(path.clone()).or_insert(0) += 1;
        true
    }

    /// A page failed. Books changed: the banner; params changed: a rerun;
    /// anything else keeps the rows shown and offers "Try again".
    pub fn page_failed(&mut self, generation: u64, path: &RowPath, why: Failure) -> Vec<Effect> {
        if generation != self.generation || !self.is_paging(path) {
            return Vec::new();
        }
        self.paging = None;
        match why {
            Failure::BooksChanged => {
                self.banner = Some(Banner::BooksChanged);
                Vec::new()
            }
            Failure::ParamsChanged => self.next_run(),
            _ => {
                self.banner = Some(Banner::TryAgain);
                Vec::new()
            }
        }
    }
}

/// A subtotal's drill tab: pages of 500, "Load more", and the engine's
/// echoed subtotal as the footer. Every page must echo the same subtotal;
/// a different one means the books changed (an equality check, never a
/// sum).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DetailPaging {
    /// The footer.
    pub subtotal: Option<Vec<Cell>>,
    /// The next page's cursor (`None` after the last page).
    pub cursor: Option<String>,
    /// Pages loaded.
    pub pages: usize,
    /// A page echoed a different subtotal.
    pub books_changed: bool,
}

impl DetailPaging {
    /// Takes a page's cursor and echoed subtotal.
    pub fn page(&mut self, cursor: Option<String>, subtotal: Vec<Cell>) {
        match &self.subtotal {
            Some(s) if *s != subtotal => self.books_changed = true,
            Some(_) => {}
            None => self.subtotal = Some(subtotal),
        }
        self.cursor = cursor;
        self.pages += 1;
    }

    /// More pages wait.
    #[must_use]
    pub const fn has_more(&self) -> bool {
        self.cursor.is_some()
    }
}

#[cfg(test)]
#[path = "run_state_tests.rs"]
mod tests;
