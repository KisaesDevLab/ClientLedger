//! The compare tools over the document viewer (A04 D4, Q-291; A06 reuses
//! them): highlights both ways between a form's fields and their spots,
//! click to fill, drag a box, the check list. `overlay.rs` owns all float
//! geometry and hit-testing, `fill.rs` the one-amount rule, `checklist.rs`
//! the list; the model types are [`cl_model::doc_draft`]'s.
//!
//! # How a form uses them (WP-F2; A06 the same way with its own targets)
//!
//! The form owns one [`CompareTools`]. Each frame it:
//! 1. gives it its targets and the values it shows
//!    ([`CompareTools::set_targets`]), the field with the keyboard
//!    ([`CompareTools::focus`]) and, once known, the page words
//!    ([`CompareTools::set_words`]: the session's [`crate::reading::service::WordCache`]
//!    or a word re-read);
//! 2. docked: hands the view to its viewer before drawing it
//!    (`viewer.set_compare(Some(tools.view()))`, then `DocPane::show`) and
//!    takes the viewer's events after (`viewer.take_compare_events()`);
//!    popped out: answers [`crate::windows::Screen::compare`] with
//!    `tools.view()` and gets the pop-out's events through
//!    [`crate::windows::Screen::on_viewer_event`] (the shell's outbox);
//! 3. runs each event through [`CompareTools::on_event`] and each frame
//!    [`CompareTools::poll`] (a box re-read's answer), and applies the
//!    [`CompareAction`]s: focus a field, fill it (the field becomes
//!    `Edited`, its spot the words'), mark it looked at;
//! 4. runs `viewer.toggle_highlights` through
//!    [`CompareTools::toggle_highlights`].
//!
//! The control channel's view: whoever draws the highlights (the docked
//! pane or the pop-out) publishes [`HighlightState`] each frame it draws,
//! and `state.doc_viewer.highlight` reports [`highlight`]. Field names,
//! levels and counts only: never a value, a word or a file name.
//!
//! No floats here or in `fill.rs` / `checklist.rs` (`cargo xtask nofloat`).

pub mod checklist;
pub mod fill;
pub mod overlay;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_docread::model::{PageWords, Word, WordSource};
use eframe::egui;
use serde::Serialize;

pub use cl_model::doc_draft::{
    CompareFeed, CompareTarget, FillArm, FillKind, PageBox, TargetKey, TargetLevel,
};
pub use fill::{FillRefusal, FillValue};

use crate::reading::service::{ReadingService, RegionJob, RegionOutcome, RegionRequest};

/// What the viewer's highlights show now (`state.doc_viewer.highlight`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HighlightState {
    /// Highlights are on (`viewer.toggle_highlights`).
    pub on: bool,
    /// Targets drawn on the page shown.
    pub drawn: usize,
    /// The outlined field (the form's focused target), by stable name.
    pub focused: Option<String>,
    /// The page shown, from 0.
    pub page: Option<u16>,
    /// The outlined box's level name (`sure`, `check_this`, `not_found`).
    pub level: Option<&'static str>,
    /// The armed field (what a click on a word fills), by stable name.
    pub armed: Option<String>,
    /// WP-C: words known on the page shown (0 until the words are ready).
    pub words: usize,
    /// WP-C: the keyboard's word (Alt+Left / Alt+Right), by index on the
    /// page in reading order.
    pub cursor: Option<u32>,
    /// WP-C: words picked by the last click (Shift+click adds).
    pub picked: usize,
    /// WP-C: the note shown under the toolbar, by stable name
    /// ([`CompareNote::name`]).
    pub note: Option<&'static str>,
    /// WP-C: the check list is open.
    pub checklist: bool,
}

impl HighlightState {
    /// The control channel's JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

fn highlight_id() -> egui::Id {
    egui::Id::unique("a04/compare/highlight")
}

/// Records what the highlights show (the viewer that draws them, each
/// frame it draws).
pub fn publish_highlight(ctx: &egui::Context, state: HighlightState) {
    ctx.data_mut(|d| d.insert_temp(highlight_id(), state));
}

/// What the highlights showed last; the default (off, nothing drawn) until
/// a viewer published.
#[must_use]
pub fn highlight(ctx: &egui::Context) -> HighlightState {
    ctx.data(|d| d.get_temp::<HighlightState>(highlight_id()))
        .unwrap_or_default()
}

/// A target's stable name for the control channel: the field name, and
/// for a line's field its row number (`line_amount:3`). Never a value.
#[must_use]
pub fn key_name(key: TargetKey) -> String {
    match key.row {
        Some(r) => format!("{}:{}", key.field, r.0),
        None => key.field.to_string(),
    }
}

/// What the compare tools say under the viewer's toolbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareNote {
    /// A pick filled nothing.
    Refused(FillRefusal),
    /// The page's words are still being read ("Getting the page ready…").
    WordsLoading,
    /// No field is armed: click a field first.
    NoField,
    /// The dragged box is smaller than 1% of the page side.
    BoxTooSmall,
    /// A box is being read in the worker.
    ReadingBox,
    /// The box couldn't be read.
    BoxNotRead,
    /// Box re-reads aren't available on this computer (no reading worker,
    /// or one without region reading).
    BoxUnavailable,
}

impl CompareNote {
    /// Stable name (control channel).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            CompareNote::Refused(r) => r.name(),
            CompareNote::WordsLoading => "words_loading",
            CompareNote::NoField => "no_field",
            CompareNote::BoxTooSmall => "box_too_small",
            CompareNote::ReadingBox => "reading_box",
            CompareNote::BoxNotRead => "box_not_read",
            CompareNote::BoxUnavailable => "box_unavailable",
        }
    }

    /// The i18n key of the words shown.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            CompareNote::Refused(r) => r.key(),
            CompareNote::WordsLoading => "compare.note.words_loading",
            CompareNote::NoField => "compare.note.no_field",
            CompareNote::BoxTooSmall => "compare.note.box_too_small",
            CompareNote::ReadingBox => "compare.note.reading_box",
            CompareNote::BoxNotRead => "compare.note.box_not_read",
            CompareNote::BoxUnavailable => "compare.note.box_unavailable",
        }
    }
}

/// The page words a form compares against: every read page's words in
/// reading order. Shared (`Arc`), never copied per frame. `Debug` shows
/// counts only.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DocWords {
    /// One entry per page read.
    pub pages: Vec<PageWords>,
}

impl std::fmt::Debug for DocWords {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocWords")
            .field("pages", &self.pages.len())
            .finish()
    }
}

impl DocWords {
    /// The words of page `page`, if it was read.
    #[must_use]
    pub fn page(&self, page: u16) -> Option<&PageWords> {
        self.pages.iter().find(|p| p.page == page)
    }
}

/// What a viewer draws the compare tools from (docked: the form hands it
/// over; popped out: [`crate::windows::Screen::compare`]). `values` are
/// the values the form shows, one per target in `feed.targets` order, for
/// the check list only (owner data: never logged, `Debug` shows counts).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CompareView {
    /// Targets, armed and focused fields, Highlights.
    pub feed: CompareFeed,
    /// The form's shown value of each target ("$25.00"), same order.
    pub values: Vec<String>,
    /// The page words, once known.
    pub words: Option<Arc<DocWords>>,
    /// The note under the toolbar.
    pub note: Option<CompareNote>,
}

impl std::fmt::Debug for CompareView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompareView")
            .field("targets", &self.feed.targets.len())
            .field("generation", &self.feed.generation)
            .field("words", &self.words.as_ref().map(|w| w.pages.len()))
            .field("note", &self.note)
            .finish_non_exhaustive()
    }
}

impl CompareView {
    /// The label of the armed field ("Amount due").
    #[must_use]
    pub fn armed_label(&self) -> Option<&str> {
        let key = self.feed.armed?;
        self.feed
            .targets
            .iter()
            .find(|t| t.key == key)
            .map(|t| t.label.as_str())
    }

    /// The focused target.
    #[must_use]
    pub fn focused_target(&self) -> Option<&CompareTarget> {
        let key = self.feed.focused?;
        self.feed.targets.iter().find(|t| t.key == key)
    }
}

/// What a viewer's compare tools asked for this frame (the docked viewer:
/// `ViewerState::take_compare_events`; the pop-out: the shell's outbox).
/// Positions are page boxes and word indices, never text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareEvent {
    /// Words `first..=last` (reading order) of `page` were picked: a click
    /// on a word, Shift+click to add the words up to another one, or Enter
    /// on the keyboard's word.
    PageClick {
        /// The page, from 0.
        page: u16,
        /// The first word picked.
        first: u32,
        /// The last word picked.
        last: u32,
    },
    /// A box was dragged on the page.
    PageBox(PageBox),
    /// A target's box was clicked, or "Find on page" in the check list:
    /// the form focuses that field.
    TargetClicked(TargetKey),
    /// "Looks right" in the check list.
    LooksRight(TargetKey),
    /// The Highlights button or `viewer.toggle_highlights`.
    ToggleHighlights,
}

/// What the form does after an event (see the module comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareAction {
    /// Put the keyboard in this field.
    Focus(TargetKey),
    /// Fill this field with the picked value: it becomes `Edited` and its
    /// spot becomes `spot`.
    Fill {
        /// The field.
        key: TargetKey,
        /// The value (exact; `Debug` redacted).
        value: FillValue,
        /// Where it was picked.
        spot: PageBox,
    },
    /// The owner looked at this field and it is right.
    LooksRight(TargetKey),
}

/// What [`CompareTools::on_event`] may need besides the event.
pub struct CompareEnv<'a> {
    /// The owner's today (dates without a year).
    pub today: CalendarDate,
    /// The reading service, for a box re-read (`None`: not available).
    pub service: Option<&'a ReadingService>,
    /// The document's bytes, for a box re-read.
    pub bytes: Option<Arc<[u8]>>,
}

/// A box re-read in flight: what it fills when it answers.
struct PendingRegion {
    key: TargetKey,
    kind: FillKind,
    area: PageBox,
    job: RegionJob,
}

/// One form's compare tools (see the module comment). Plain state; the
/// form drives it. `Debug` shows counts only.
pub struct CompareTools {
    feed: CompareFeed,
    values: Vec<String>,
    arm: FillArm,
    words: Option<Arc<DocWords>>,
    note: Option<CompareNote>,
    region: Option<PendingRegion>,
}

impl std::fmt::Debug for CompareTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompareTools")
            .field("targets", &self.feed.targets.len())
            .field("generation", &self.feed.generation)
            .field("note", &self.note)
            .field("reading_box", &self.region.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for CompareTools {
    fn default() -> Self {
        CompareTools::new()
    }
}

impl CompareTools {
    /// No targets yet; Highlights on.
    #[must_use]
    pub fn new() -> CompareTools {
        CompareTools {
            feed: CompareFeed {
                highlights: true,
                ..CompareFeed::default()
            },
            values: Vec::new(),
            arm: FillArm::new(),
            words: None,
            note: None,
            region: None,
        }
    }

    fn bump(&mut self) {
        self.feed.generation = self.feed.generation.wrapping_add(1);
    }

    /// The form's targets and the values it shows (same order). A changed
    /// list bumps the generation; an armed field that is gone disarms.
    pub fn set_targets(&mut self, targets: Vec<CompareTarget>, values: Vec<String>) {
        if targets == self.feed.targets && values == self.values {
            return;
        }
        self.arm.retain(&targets);
        self.feed.targets = targets;
        self.values = values;
        self.feed.armed = self.arm.armed().map(|(k, _)| k);
        if self
            .feed
            .focused
            .is_some_and(|k| !self.feed.targets.iter().any(|t| t.key == k))
        {
            self.feed.focused = None;
        }
        self.bump();
    }

    /// The field with the keyboard (`None`: the viewer or a widget that
    /// isn't a target). A target outlines and turns the viewer to its
    /// page; a fillable one arms. `None` keeps both.
    pub fn focus(&mut self, key: Option<TargetKey>) {
        let Some(key) = key.filter(|k| self.feed.targets.iter().any(|t| t.key == *k)) else {
            return;
        };
        self.arm.focus(Some(key), &self.feed.targets);
        let armed = self.arm.armed().map(|(k, _)| k);
        if self.feed.focused != Some(key) || self.feed.armed != armed {
            self.feed.focused = Some(key);
            self.feed.armed = armed;
            if matches!(self.note, Some(CompareNote::NoField)) {
                self.note = None;
            }
            self.bump();
        }
    }

    /// The page words (`None`: not known yet; picks then say "Getting the
    /// page ready…").
    pub fn set_words(&mut self, words: Option<Arc<DocWords>>) {
        let same = match (&self.words, &words) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.words = words;
            if matches!(self.note, Some(CompareNote::WordsLoading)) && self.words.is_some() {
                self.note = None;
            }
            self.bump();
        }
    }

    /// The page words known.
    #[must_use]
    pub fn words(&self) -> Option<&Arc<DocWords>> {
        self.words.as_ref()
    }

    /// Highlights on or off (`viewer.toggle_highlights`).
    pub fn toggle_highlights(&mut self) {
        self.feed.highlights = !self.feed.highlights;
        self.bump();
    }

    /// Disarm and forget the note ("Clear and type it myself", another
    /// draft). A box re-read in flight is cancelled.
    pub fn clear(&mut self) {
        self.arm.clear();
        self.feed.armed = None;
        self.note = None;
        if let Some(p) = self.region.take() {
            p.job.cancel();
        }
        self.bump();
    }

    /// The feed (what [`CompareView::feed`] carries).
    #[must_use]
    pub fn feed(&self) -> &CompareFeed {
        &self.feed
    }

    /// The note shown under the toolbar.
    #[must_use]
    pub fn note(&self) -> Option<CompareNote> {
        self.note
    }

    /// True while a box re-read is in flight.
    #[must_use]
    pub fn reading_box(&self) -> bool {
        self.region.is_some()
    }

    /// What a viewer draws (cheap: the words are shared).
    #[must_use]
    pub fn view(&self) -> CompareView {
        CompareView {
            feed: self.feed.clone(),
            values: self.values.clone(),
            words: self.words.clone(),
            note: self.note,
        }
    }

    fn set_note(&mut self, note: Option<CompareNote>) {
        if self.note != note {
            self.note = note;
            self.bump();
        }
    }

    /// The armed field, or the note that asks for one.
    fn armed(&mut self) -> Option<(TargetKey, FillKind)> {
        let armed = self.arm.armed();
        if armed.is_none() {
            self.set_note(Some(CompareNote::NoField));
        }
        armed
    }

    /// Fills the armed field from `picked` words of `page`.
    fn fill_from(
        &mut self,
        key: TargetKey,
        kind: FillKind,
        page: u16,
        picked: &[&Word],
        today: CalendarDate,
    ) -> Option<CompareAction> {
        let texts: Vec<&str> = picked.iter().map(|w| w.text.as_str()).collect();
        match fill::fill(kind, &texts, today) {
            Ok(value) => {
                self.set_note(None);
                let spot = fill::spot_of(page, picked)?;
                Some(CompareAction::Fill { key, value, spot })
            }
            Err(r) => {
                self.set_note(Some(CompareNote::Refused(r)));
                None
            }
        }
    }

    /// Runs one viewer event (see [`CompareAction`]).
    pub fn on_event(&mut self, event: CompareEvent, env: &CompareEnv<'_>) -> Option<CompareAction> {
        match event {
            CompareEvent::TargetClicked(key) => {
                self.focus(Some(key));
                Some(CompareAction::Focus(key))
            }
            CompareEvent::LooksRight(key) => Some(CompareAction::LooksRight(key)),
            CompareEvent::ToggleHighlights => {
                self.toggle_highlights();
                None
            }
            CompareEvent::PageClick { page, first, last } => {
                let (key, kind) = self.armed()?;
                self.cancel_region();
                let Some(words) = self.words.clone() else {
                    self.set_note(Some(CompareNote::WordsLoading));
                    return None;
                };
                let (lo, hi) = (first.min(last), first.max(last));
                let picked: Vec<&Word> = words
                    .page(page)
                    .map(|p| {
                        p.words
                            .iter()
                            .skip(usize::try_from(lo).unwrap_or(usize::MAX))
                            .take(usize::try_from(hi - lo).unwrap_or(0).saturating_add(1))
                            .collect()
                    })
                    .unwrap_or_default();
                self.fill_from(key, kind, page, &picked, env.today)
            }
            CompareEvent::PageBox(area) => {
                let (key, kind) = self.armed()?;
                self.cancel_region();
                if area.too_small() {
                    self.set_note(Some(CompareNote::BoxTooSmall));
                    return None;
                }
                // A text page's own words, when the box holds some.
                if let Some(words) = self.words.clone()
                    && let Some(p) = words.page(area.page)
                    && p.source == WordSource::TextLayer
                {
                    let inside = fill::words_in(area, &p.words);
                    if !inside.is_empty() {
                        let picked: Vec<&Word> =
                            inside.iter().filter_map(|i| p.words.get(*i)).collect();
                        return self.fill_from(key, kind, area.page, &picked, env.today);
                    }
                }
                // A scan, or no words there: the worker reads the box.
                let (Some(service), Some(bytes)) = (env.service, env.bytes.clone()) else {
                    self.set_note(Some(CompareNote::BoxUnavailable));
                    return None;
                };
                let job = service.submit_region(RegionRequest {
                    bytes,
                    area,
                    today: env.today,
                });
                self.region = Some(PendingRegion {
                    key,
                    kind,
                    area,
                    job,
                });
                self.set_note(Some(CompareNote::ReadingBox));
                None
            }
        }
    }

    fn cancel_region(&mut self) {
        if let Some(p) = self.region.take() {
            p.job.cancel();
        }
    }

    /// A box re-read's answer, when it came (call each frame; the M4 rule
    /// applies to the words it returns).
    pub fn poll(&mut self, today: CalendarDate) -> Option<CompareAction> {
        let answer = self.region.as_ref()?.job.answer.try_recv();
        let outcome = match answer {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                RegionOutcome::Failed(crate::reading::service::ReadFailure::GaveUp)
            }
        };
        let p = self.region.take()?;
        match outcome {
            RegionOutcome::Words(words) => {
                // Only words the worker placed inside the box count.
                let picked: Vec<&Word> = words
                    .iter()
                    .filter(|w| {
                        PageBox::new(
                            p.area.page,
                            w.bbox.left(),
                            w.bbox.top(),
                            w.bbox.right(),
                            w.bbox.bottom(),
                        )
                        .is_some_and(|b| {
                            b.l < p.area.r && b.r > p.area.l && b.t < p.area.b && b.b > p.area.t
                        })
                    })
                    .collect();
                self.fill_from(p.key, p.kind, p.area.page, &picked, today)
            }
            RegionOutcome::NotSupported
            | RegionOutcome::Failed(crate::reading::service::ReadFailure::Unavailable) => {
                self.set_note(Some(CompareNote::BoxUnavailable));
                None
            }
            RegionOutcome::Failed(f) => {
                tracing::info!(class = f.class(), "box re-read failed");
                self.set_note(Some(CompareNote::BoxNotRead));
                None
            }
        }
    }
}

#[cfg(test)]
mod tests;
