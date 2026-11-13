//! The run state of a Report Builder report window (A07 plan §4), pure: no
//! egui, no I/O, no clock. The window feeds it events and the current time
//! in milliseconds, and performs the [`Effect`]s it returns.
//!
//! ```text
//! Empty -> Running{gen} -> Shown | Failed | TimedOut | Unavailable(cap)
//! Shown|Running --expand(p)/collapse(p)--> shown set updated at once;
//!     open/collapsed lists updated; Running{gen+1} (debounced 150 ms)
//!   collapse also hides p's rows at once and cancels any Paging under p
//! Running(gen) superseded --> request cancelled (one in-flight run)
//! Shown --More(p)--> Paging{gen, p, cursor} -> rows replace the More row;
//!   dropped if p closed or gen changed
//!   409 RUN_CHANGED -> banner (books) | rerun (params)
//! Running failed/timed out after expand --> keep last table, revert the
//!   lists and the shown set, "Try again"
//! Rerun --> pages loaded by "Load more" come back as the run's `more` rows
//! 409 STALE_VERSION --> "Try again", never a rerun (Q-A07u); 404 --> deleted
//! Expand all --> expandAll; Collapse all --> open = [], collapsed = shown
//! ```
//!
//! The engine expands a row when `expandAll` is set, its path is in `open`,
//! or its section has `defaultExpanded` and the path is not in `collapsed`
//! (engine PR-C, Q-A07r). The window keeps the two lists it sends and takes
//! the engine's `expanded` rows as the shown open set after every run.

use std::collections::BTreeSet;

use cl_core::report::{Cell, RowPath};

/// Paths each run-request list (`open`, `collapsed`) may carry (the
/// engine's cap).
pub const MAX_OPEN_PATHS: usize = 1_000;

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
    /// The role can't see this section (403).
    Denied,
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
    /// More than 1,000 parts were opened or closed by hand; some were left
    /// as the report's default (Q-A07r).
    TooManyOpen,
    /// The last expand would show over 100,000 rows; the table is the last
    /// good one ("Add a filter or fewer group-bys").
    TooLarge,
}

/// What the window must do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Run generation `generation` with these lists.
    Run {
        /// The generation the answer must carry.
        generation: u64,
        /// Paths opened by hand (≤ 1,000).
        open: Vec<RowPath>,
        /// Paths closed under a `defaultExpanded` section (≤ 1,000).
        collapsed: Vec<RowPath>,
        /// Expand all.
        expand_all: bool,
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
        /// The cursor (`None`: from the start).
        cursor: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Paging {
    generation: u64,
    path: RowPath,
}

/// What a run sends about the open rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Lists {
    open: BTreeSet<RowPath>,
    collapsed: BTreeSet<RowPath>,
    expand_all: bool,
}

/// The builder window's run state.
#[derive(Clone, Debug)]
pub struct BuilderRunState {
    phase: Phase,
    generation: u64,
    in_flight: Option<u64>,
    /// The open set of the table shown (the engine's `expanded` rows).
    shown: BTreeSet<RowPath>,
    /// The open set asked for and not yet shown: shown at once (collapse
    /// hides rows immediately), sent when the debounce ends.
    pending: Option<BTreeSet<RowPath>>,
    /// The lists the next run sends.
    lists: Lists,
    /// The lists of the last run shown (restored when a run fails).
    committed: Lists,
    /// The lists of the run in flight.
    asked: Option<Lists>,
    due_at: Option<u64>,
    has_table: bool,
    stamp: Option<u64>,
    paging: Option<Paging>,
    banner: Option<Banner>,
    /// Moves whenever the rows shown could change without a new table
    /// (an expand, a collapse, a rollback): the window's visible-row cache
    /// key (S-L6).
    revision: u64,
}

impl Default for BuilderRunState {
    fn default() -> Self {
        BuilderRunState::new()
    }
}

/// Keeps the first `MAX_OPEN_PATHS` paths in path order: `true` when some
/// were cut.
fn cap(set: &mut BTreeSet<RowPath>) -> bool {
    if set.len() <= MAX_OPEN_PATHS {
        return false;
    }
    *set = std::mem::take(set)
        .into_iter()
        .take(MAX_OPEN_PATHS)
        .collect();
    true
}

impl BuilderRunState {
    /// Nothing run yet.
    #[must_use]
    pub fn new() -> BuilderRunState {
        BuilderRunState {
            phase: Phase::Empty,
            generation: 0,
            in_flight: None,
            shown: BTreeSet::new(),
            pending: None,
            lists: Lists::default(),
            committed: Lists::default(),
            asked: None,
            due_at: None,
            has_table: false,
            stamp: None,
            paging: None,
            banner: None,
            revision: 0,
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

    /// Moves whenever [`BuilderRunState::hidden`] could answer differently
    /// for the same rows.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// When the debounced run is due (milliseconds on the window's clock):
    /// the window must ask for a frame then, or the run never goes (#7).
    #[must_use]
    pub const fn due_at(&self) -> Option<u64> {
        self.due_at
    }

    /// The ledger stamp of the table shown.
    #[must_use]
    pub const fn stamp(&self) -> Option<u64> {
        self.stamp
    }

    /// The open and collapsed lists and Expand all that made the table
    /// shown: an export run asks for the same (A09, Q-A09h).
    #[must_use]
    pub fn shown_lists(&self) -> (Vec<RowPath>, Vec<RowPath>, bool) {
        (
            self.committed.open.iter().cloned().collect(),
            self.committed.collapsed.iter().cloned().collect(),
            self.committed.expand_all,
        )
    }

    /// The open paths the screen follows (pending ones first).
    #[must_use]
    pub fn open_paths(&self) -> &BTreeSet<RowPath> {
        self.pending.as_ref().unwrap_or(&self.shown)
    }

    /// Everything is open.
    #[must_use]
    pub const fn is_open_all(&self) -> bool {
        self.lists.expand_all
    }

    /// True when `path` is open.
    #[must_use]
    pub fn is_open(&self, path: &RowPath) -> bool {
        self.lists.expand_all || self.open_paths().contains(path)
    }

    /// A page is being fetched under `path`.
    #[must_use]
    pub fn is_paging(&self, path: &RowPath) -> bool {
        self.paging.as_ref().is_some_and(|p| &p.path == path)
    }

    /// Which rows to hide. `rows` is (path, expandable, leaf) in table
    /// order, where a leaf is a detail, opening or `more` row: the engine
    /// gives those their line's path. A row hides when an expandable row
    /// whose path is a proper prefix of its own is closed, and a leaf also
    /// when its own line is closed.
    #[must_use]
    pub fn hidden(&self, rows: &[(RowPath, bool, bool)]) -> Vec<bool> {
        if self.lists.expand_all {
            return vec![false; rows.len()];
        }
        // One set lookup per prefix: linear in the rows (up to 100,000
        // group rows, most of them closed at first).
        let closed: std::collections::HashSet<&[String]> = rows
            .iter()
            .filter(|(p, expandable, _)| *expandable && !self.is_open(p))
            .map(|(p, _, _)| p.0.as_slice())
            .collect();
        rows.iter()
            .map(|(p, _, leaf)| {
                let end = if *leaf { p.0.len() + 1 } else { p.0.len() };
                (1..end).any(|k| closed.contains(&p.0[..k]))
            })
            .collect()
    }

    fn next_run(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.revision += 1;
        if let Some(old) = self.in_flight.take() {
            fx.push(Effect::Cancel { generation: old });
        }
        self.generation += 1;
        self.in_flight = Some(self.generation);
        self.paging = None;
        self.due_at = None;
        // Each list carries at most 1,000 paths (Q-A07r): the first ones in
        // path order are sent, the rest follow the report's default.
        let cut_open = cap(&mut self.lists.open);
        let cut_closed = cap(&mut self.lists.collapsed);
        if cut_open || cut_closed {
            self.banner = Some(Banner::TooManyOpen);
        }
        self.asked = Some(self.lists.clone());
        self.phase = Phase::Running;
        fx.push(Effect::Run {
            generation: self.generation,
            open: self.lists.open.iter().cloned().collect(),
            collapsed: self.lists.collapsed.iter().cloned().collect(),
            expand_all: self.lists.expand_all,
        });
        fx
    }

    /// The run in flight no longer matters (the report changed under it):
    /// its answer will be dropped and nothing waits for it. The table shown
    /// stays.
    pub fn cancel(&mut self) {
        if self.in_flight.take().is_none() {
            return;
        }
        self.generation += 1;
        self.revision += 1;
        self.asked = None;
        if self.phase == Phase::Running {
            self.phase = if self.has_table {
                Phase::Shown
            } else {
                Phase::Empty
            };
        }
    }

    /// The first run, or Refresh (F5): runs now.
    pub fn refresh(&mut self) -> Vec<Effect> {
        self.banner = None;
        self.next_run()
    }

    fn toggle(&mut self, path: &RowPath, open: bool, now_ms: u64) -> Vec<Effect> {
        if self.lists.expand_all {
            // Leaving "expand all" (the engine ignores `collapsed` under
            // it): every path shown open is listed as opened by hand.
            self.lists.expand_all = false;
            self.lists.open = self.shown.clone();
            self.lists.collapsed.clear();
        }
        let mut set = self.open_paths().clone();
        if open {
            set.insert(path.clone());
            self.lists.open.insert(path.clone());
            self.lists.collapsed.remove(path);
        } else {
            let gone = |p: &RowPath| p == path || p.is_under(path);
            set.retain(|p| !gone(p));
            self.lists.open.retain(|p| !gone(p));
            // `open` wins over `collapsed`, so a default-open path needs
            // both changes.
            self.lists.collapsed.insert(path.clone());
            if self
                .paging
                .as_ref()
                .is_some_and(|pg| &pg.path == path || pg.path.is_under(path))
            {
                self.paging = None;
            }
        }
        self.revision += 1;
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

    /// Expand all: one run with `expandAll` (bounded by the engine's caps).
    pub fn expand_all(&mut self) -> Vec<Effect> {
        self.lists.expand_all = true;
        self.pending = None;
        self.next_run()
    }

    /// Collapse all: nothing opened by hand, every path shown open closed
    /// (default-open sections too).
    pub fn collapse_all(&mut self) -> Vec<Effect> {
        let shown = self.open_paths().clone();
        self.lists.expand_all = false;
        self.lists.open.clear();
        self.lists.collapsed.extend(shown);
        self.pending = Some(BTreeSet::new());
        self.next_run()
    }

    /// Call each frame: sends the debounced run when it is due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        match self.due_at {
            Some(t) if now_ms >= t => self.next_run(),
            _ => Vec::new(),
        }
    }

    /// The run of `generation` answered with `stamp` and the paths the
    /// engine shows expanded: they are the open set from now on. Pages
    /// loaded with "Load more" before come back as the run's `more` rows
    /// (Q-A07u). False for a superseded answer.
    pub fn run_ok(&mut self, generation: u64, stamp: u64, expanded: &[RowPath]) -> bool {
        if generation != self.generation {
            return false; // a superseded answer
        }
        self.revision += 1;
        self.in_flight = None;
        if let Some(asked) = self.asked.take() {
            self.committed = asked;
        }
        self.shown = expanded.iter().cloned().collect();
        // An expand made while this ran stays pending (its run is due).
        if self.due_at.is_none() {
            self.pending = None;
        }
        self.phase = Phase::Shown;
        self.has_table = true;
        self.stamp = Some(stamp);
        true
    }

    /// The run of `generation` failed. Nothing is rerun from here: a
    /// refusal that would come again (a stale memorized version, S-M1)
    /// waits for the owner's Refresh.
    pub fn run_failed(&mut self, generation: u64, why: Failure) {
        if generation != self.generation {
            return;
        }
        self.revision += 1;
        self.in_flight = None;
        self.asked = None;
        if self.due_at.is_none() {
            // Revert to the table shown and the lists that made it.
            self.pending = None;
            self.lists = self.committed.clone();
        }
        match why {
            Failure::Deleted => self.phase = Phase::Deleted,
            // Expand all is what usually trips the cap: keep the table.
            Failure::TooLarge if self.has_table => {
                self.phase = Phase::Shown;
                self.banner = Some(Banner::TooLarge);
            }
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
    }

    /// "Load more" on a `more` row of `node_id` at `path` (`cursor`
    /// `None`: from the start).
    pub fn more(&mut self, node_id: &str, path: &RowPath, cursor: Option<&str>) -> Vec<Effect> {
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
            cursor: cursor.map(str::to_string),
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
