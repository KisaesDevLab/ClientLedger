//! The compare tools' geometry (A04 D4): every float of the compare tools
//! lives here (`cargo xtask nofloat` exempts this file and refuses any
//! money type in it). It maps page boxes (ten-thousandths of the original
//! page) to the drawn page through the zoom and the view-only turn, and
//! back; hit-tests clicks against the page words and the targets' boxes;
//! turns a drag into a [`PageBox`]; draws the highlights, the picked words,
//! the keyboard's word and the box being dragged; and cuts check-list
//! snippets out of textures the viewer already drew (no new decoding).
//!
//! It hands the rest of the tools word indices and page boxes only, never
//! a float.

use std::sync::Arc;

use eframe::egui::{
    self, Align, Color32, Pos2, Rect, Response, Role, Sense, Shape, Stroke, StrokeKind, TextureId,
    Ui, Vec2, WidgetInfo,
};

use super::{
    CompareEvent, CompareNote, CompareView, HighlightState, PageBox, TargetKey, TargetLevel,
    key_name,
};
use crate::i18n::t_with;
use crate::theme::Tokens;
use crate::widgets::doc_worker::Rotation;

/// A click within this many ten-thousandths of a word still picks it.
const WORD_SLOP: u16 = 30;

fn unit(v: u16) -> f32 {
    f32::from(v.min(PageBox::SIDE)) / f32::from(PageBox::SIDE)
}

// Clamped to 0..=1 first, so the cast stays in range.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn units(v: f32) -> u16 {
    (v.clamp(0.0, 1.0) * f32::from(PageBox::SIDE)).round() as u16
}

/// A point of the original page (0..1 each way) where the turned page
/// shows it (0..1 each way).
#[must_use]
pub fn to_display(rotation: Rotation, x: f32, y: f32) -> (f32, f32) {
    match rotation {
        Rotation::None => (x, y),
        Rotation::Right => (1.0 - y, x),
        Rotation::Half => (1.0 - x, 1.0 - y),
        Rotation::Left => (y, 1.0 - x),
    }
}

/// The inverse of [`to_display`].
#[must_use]
pub fn to_original(rotation: Rotation, u: f32, v: f32) -> (f32, f32) {
    match rotation {
        Rotation::None => (u, v),
        Rotation::Right => (v, 1.0 - u),
        Rotation::Half => (1.0 - u, 1.0 - v),
        Rotation::Left => (1.0 - v, u),
    }
}

/// Where `b` shows on a page drawn in `page` turned by `rotation`.
#[must_use]
pub fn box_rect(page: Rect, rotation: Rotation, b: PageBox) -> Rect {
    let (x0, y0) = to_display(rotation, unit(b.l), unit(b.t));
    let (x1, y1) = to_display(rotation, unit(b.r), unit(b.b));
    let at = |x: f32, y: f32| {
        Pos2::new(
            page.min.x + x * page.width(),
            page.min.y + y * page.height(),
        )
    };
    Rect::from_two_pos(at(x0, y0), at(x1, y1))
}

/// The original-page point (box units) under `pos`, or `None` off the
/// page.
#[must_use]
pub fn point_on_page(page: Rect, rotation: Rotation, pos: Pos2) -> Option<(u16, u16)> {
    if !page.contains(pos) || page.width() <= 0.0 || page.height() <= 0.0 {
        return None;
    }
    let u = (pos.x - page.min.x) / page.width();
    let v = (pos.y - page.min.y) / page.height();
    let (x, y) = to_original(rotation, u, v);
    Some((units(x), units(y)))
}

/// The page box between two points of a drag (each kept on the page).
#[must_use]
pub fn drag_box(
    page: Rect,
    rotation: Rotation,
    number: u16,
    from: Pos2,
    to: Pos2,
) -> Option<PageBox> {
    if page.width() <= 0.0 || page.height() <= 0.0 {
        return None;
    }
    let on_page = |pos: Pos2| {
        let pos = page.clamp(pos);
        let across = (pos.x - page.min.x) / page.width();
        let down = (pos.y - page.min.y) / page.height();
        let (x, y) = to_original(rotation, across, down);
        (units(x), units(y))
    };
    let (ax, ay) = on_page(from);
    let (bx, by) = on_page(to);
    PageBox::new(number, ax.min(bx), ay.min(by), ax.max(bx), ay.max(by))
}

/// The word of `words` under the page point `at`, by index in reading
/// order (integer test, a little slop round each word).
#[must_use]
pub fn hit_word(words: &[cl_docread::model::Word], at: (u16, u16)) -> Option<u32> {
    let (x, y) = at;
    words
        .iter()
        .position(|w| {
            let b = w.bbox;
            x.saturating_add(WORD_SLOP) >= b.left()
                && x <= b.right().saturating_add(WORD_SLOP)
                && y.saturating_add(WORD_SLOP) >= b.top()
                && y <= b.bottom().saturating_add(WORD_SLOP)
        })
        .and_then(|i| u32::try_from(i).ok())
}

/// The target whose box holds the page point `at` on page `n` (the
/// smallest when boxes overlap).
#[must_use]
pub fn hit_target(view: &CompareView, n: u16, at: (u16, u16)) -> Option<TargetKey> {
    let (x, y) = at;
    view.feed
        .targets
        .iter()
        .filter_map(|t| t.spot.map(|s| (t.key, s)))
        .filter(|(_, s)| s.page == n && x >= s.l && x <= s.r && y >= s.t && y <= s.b)
        .min_by_key(|(_, s)| u32::from(s.width()) * u32::from(s.height()))
        .map(|(k, _)| k)
}

/// The words picked last (a range on one page; Shift+click extends it
/// from `anchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pick {
    page: u16,
    anchor: u32,
    first: u32,
    last: u32,
}

/// A texture the viewer already drew, for check-list snippets.
#[derive(Debug, Clone, Copy)]
pub struct SnippetSource {
    /// The texture.
    pub texture: TextureId,
    /// Its size in pixels.
    pub size: Vec2,
    /// The turn it was drawn at (thumbnails are upright).
    pub rotation: Rotation,
}

/// One viewer's compare state: the view it draws, the last pick, the
/// keyboard's word, a drag in progress, the events for its owner.
#[derive(Default)]
pub struct Overlay {
    view: Option<CompareView>,
    pick: Option<Pick>,
    cursor: Option<(u16, u32)>,
    seen_focus: Option<TargetKey>,
    drag_from: Option<Pos2>,
    drag_to: Option<Pos2>,
    checklist: bool,
    events: Vec<CompareEvent>,
    scroll_to: Option<PageBox>,
    shown_before: bool,
    /// Where the keyboard's word was when a refusal note first showed
    /// (UI check item 6: moving it hides the note).
    note_seen: bool,
    note_cursor: Option<(u16, u32)>,
}

impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay")
            .field("view", &self.view)
            .field("checklist", &self.checklist)
            .field("events", &self.events.len())
            .finish_non_exhaustive()
    }
}

impl Overlay {
    /// The view to draw (`None`: no compare tools on this viewer).
    pub fn set_view(&mut self, view: Option<CompareView>) {
        if view.is_none() {
            self.pick = None;
            self.cursor = None;
            self.drag_from = None;
            self.drag_to = None;
            self.checklist = false;
            self.seen_focus = None;
        }
        // Other words (a re-read replaced them): a pick's indices belong to
        // the old ones, so it goes before it can name another word.
        let old = self.view.as_ref().and_then(|v| v.words.as_ref());
        let new = view.as_ref().and_then(|v| v.words.as_ref());
        let same = match (old, new) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.pick = None;
            self.cursor = None;
        }
        self.view = view;
    }

    /// Another document opened: the picks, the keyboard's word and a drag
    /// belong to the old one. The focused field's page is turned to again.
    pub fn forget_picks(&mut self) {
        self.pick = None;
        self.cursor = None;
        self.drag_from = None;
        self.drag_to = None;
        self.seen_focus = None;
        self.scroll_to = None;
    }

    /// The view drawn.
    #[must_use]
    pub fn view(&self) -> Option<&CompareView> {
        self.view.as_ref()
    }

    /// The note to show under the toolbar: the view's, except a refusal
    /// ("That isn't an amount") once the keyboard's word moved on from
    /// where it was when the refusal came (UI check item 6). A new pick
    /// shows the next answer again.
    pub fn shown_note(&mut self) -> Option<CompareNote> {
        let note = self.view.as_ref().and_then(|v| v.note);
        if !matches!(note, Some(CompareNote::Refused(_))) {
            self.note_seen = false;
            return note;
        }
        if !self.note_seen {
            self.note_seen = true;
            self.note_cursor = self.cursor;
        }
        if self.note_cursor == self.cursor {
            note
        } else {
            None
        }
    }

    /// True when the tools are on this viewer.
    #[must_use]
    pub fn active(&self) -> bool {
        self.view.is_some()
    }

    /// True when a click on a word fills a field (a drag then draws a box
    /// instead of panning the page).
    #[must_use]
    pub fn armed(&self) -> bool {
        self.view.as_ref().is_some_and(|v| v.feed.armed.is_some())
    }

    /// The check list is open.
    #[must_use]
    pub fn checklist_open(&self) -> bool {
        self.checklist
    }

    /// Open or close the check list.
    pub fn set_checklist(&mut self, open: bool) {
        self.checklist = open && self.view.is_some();
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<CompareEvent> {
        // A new pick: its answer shows wherever the keyboard's word is.
        if self
            .events
            .iter()
            .any(|e| matches!(e, CompareEvent::PageClick { .. } | CompareEvent::PageBox(_)))
        {
            self.note_seen = false;
        }
        std::mem::take(&mut self.events)
    }

    /// Queue an event for the owner.
    pub fn push(&mut self, event: CompareEvent) {
        self.events.push(event);
    }

    /// The page to turn to when the form's focused field changed and its
    /// spot is on a page (its box is then scrolled into view).
    pub fn turn_to(&mut self) -> Option<u16> {
        let view = self.view.as_ref()?;
        let focused = view.feed.focused;
        if focused == self.seen_focus {
            return None;
        }
        self.seen_focus = focused;
        let spot = view.focused_target()?.spot?;
        self.scroll_to = Some(spot);
        Some(spot.page)
    }

    /// "Find on page" in the check list: show `key`'s box.
    pub fn find(&mut self, key: TargetKey) -> Option<u16> {
        let spot = self
            .view
            .as_ref()?
            .feed
            .targets
            .iter()
            .find(|t| t.key == key)?
            .spot?;
        self.scroll_to = Some(spot);
        self.checklist = false;
        Some(spot.page)
    }

    fn page_words(&self, page: u16) -> Option<&[cl_docread::model::Word]> {
        let words = self.view.as_ref()?.words.as_ref()?;
        Some(words.page(page)?.words.as_slice())
    }

    /// `viewer.next_word`, `viewer.prev_word`, `viewer.fill_word`,
    /// `viewer.toggle_highlights` on page `page`. False when the command
    /// isn't one of these, or has nothing to act on (Enter then stays
    /// with a focused button).
    pub fn command(&mut self, id: &str, page: u16) -> bool {
        if self.view.is_none() {
            return false;
        }
        match id {
            "viewer.toggle_highlights" => {
                self.events.push(CompareEvent::ToggleHighlights);
                true
            }
            "viewer.next_word" | "viewer.prev_word" => {
                let Some(words) = self.page_words(page) else {
                    return false;
                };
                let n = u32::try_from(words.len()).unwrap_or(u32::MAX);
                if n == 0 {
                    return false;
                }
                let at = match (self.cursor, id) {
                    (Some((p, i)), "viewer.next_word") if p == page => (i + 1).min(n - 1),
                    (Some((p, i)), _) if p == page => i.saturating_sub(1),
                    (_, "viewer.next_word") => 0,
                    _ => n - 1,
                };
                let spot = words
                    .get(usize::try_from(at).unwrap_or(usize::MAX))
                    .and_then(|w| {
                        PageBox::new(
                            page,
                            w.bbox.left(),
                            w.bbox.top(),
                            w.bbox.right(),
                            w.bbox.bottom(),
                        )
                    });
                self.cursor = Some((page, at));
                self.scroll_to = spot;
                true
            }
            "viewer.fill_word" => match self.cursor {
                Some((p, i)) if p == page && self.armed() => {
                    self.pick = Some(Pick {
                        page,
                        anchor: i,
                        first: i,
                        last: i,
                    });
                    self.events.push(CompareEvent::PageClick {
                        page,
                        first: i,
                        last: i,
                    });
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// A click at the page point `at` on page `n` (`None`: a click without
    /// a pointer, such as the control channel's accessibility click on a
    /// box, which focuses that box's field).
    fn click(&mut self, n: u16, at: Option<(u16, u16)>, shift: bool, on_box: Option<TargetKey>) {
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let Some(at) = at else {
            if let Some(k) = on_box {
                self.events.push(CompareEvent::TargetClicked(k));
            }
            return;
        };
        let armed = view.feed.armed;
        let target = on_box.or_else(|| {
            view.feed
                .highlights
                .then(|| hit_target(view, n, at))
                .flatten()
        });
        let word = self.page_words(n).and_then(|w| hit_word(w, at));
        if let (true, Some(w), Some(p), Some(_)) = (shift, word, self.pick, armed)
            && p.page == n
        {
            let (first, last) = (p.anchor.min(w), p.anchor.max(w));
            self.pick = Some(Pick {
                page: n,
                anchor: p.anchor,
                first,
                last,
            });
            self.cursor = Some((n, w));
            self.events.push(CompareEvent::PageClick {
                page: n,
                first,
                last,
            });
            return;
        }
        match (target, word, armed) {
            // Another field's box: that field gets the keyboard.
            (Some(k), _, armed) if armed != Some(k) => {
                self.events.push(CompareEvent::TargetClicked(k));
            }
            (_, Some(w), Some(_)) => {
                self.pick = Some(Pick {
                    page: n,
                    anchor: w,
                    first: w,
                    last: w,
                });
                self.cursor = Some((n, w));
                self.events.push(CompareEvent::PageClick {
                    page: n,
                    first: w,
                    last: w,
                });
            }
            (Some(k), _, _) => self.events.push(CompareEvent::TargetClicked(k)),
            _ => {}
        }
    }

    /// Draws the tools over the page drawn in `page_rect` (page `n`, turned
    /// by `rotation`) and takes the page's clicks and drags (`response`).
    pub fn paint(
        &mut self,
        ui: &Ui,
        page_rect: Rect,
        rotation: Rotation,
        n: u16,
        response: &Response,
        tokens: &Tokens,
    ) {
        let Some(view) = self.view.clone() else {
            return;
        };
        let painter = ui.painter_at(page_rect.expand(2.0));
        let shift = ui.input(|i| i.modifiers.shift);
        // A drag draws a box while a field is armed (the page then pans
        // with the scroll bars, the wheel and the arrow keys).
        if view.feed.armed.is_some() {
            if response.drag_started() {
                // From where the button went down, not where the drag was
                // recognised.
                self.drag_from = ui
                    .input(|i| i.pointer.press_origin())
                    .or_else(|| response.interact_pointer_pos());
                self.drag_to = self.drag_from;
            }
            if response.dragged()
                && let Some(p) = response.interact_pointer_pos()
            {
                self.drag_to = Some(p);
            }
            if response.drag_stopped()
                && let (Some(a), Some(b)) = (self.drag_from.take(), self.drag_to.take())
                && let Some(area) = drag_box(page_rect, rotation, n, a, b)
            {
                self.events.push(CompareEvent::PageBox(area));
            }
        } else {
            self.drag_from = None;
            self.drag_to = None;
        }
        if response.clicked() {
            let at = response
                .interact_pointer_pos()
                .and_then(|p| point_on_page(page_rect, rotation, p));
            self.click(n, at, shift, None);
        }
        // The targets' boxes: drawn, and clickable (each is its own
        // accessible button, "Amount due on the page").
        if view.feed.highlights {
            for t in &view.feed.targets {
                let Some(spot) = t.spot.filter(|s| s.page == n) else {
                    continue;
                };
                let r = box_rect(page_rect, rotation, spot).expand(2.0);
                let id = ui.make_persistent_id(("compare-box", key_name(t.key)));
                let resp = ui.interact(r, id, Sense::click());
                let label = t_with("compare.box", &[("field", &t.label)]);
                resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
                if resp.clicked() {
                    let at = resp
                        .interact_pointer_pos()
                        .and_then(|p| point_on_page(page_rect, rotation, p));
                    self.click(n, at, shift, at.is_none().then_some(t.key));
                }
                let focused = view.feed.focused == Some(t.key);
                let width = if focused { 2.5 } else { 1.5 };
                match t.level {
                    TargetLevel::Sure => {
                        painter.rect_stroke(
                            r,
                            2.0,
                            Stroke::new(width, tokens.accent),
                            StrokeKind::Outside,
                        );
                    }
                    TargetLevel::CheckThis => {
                        painter.rect_stroke(
                            r,
                            2.0,
                            Stroke::new(width, tokens.warn),
                            StrokeKind::Outside,
                        );
                        // S15: not by colour alone. The mark sits just left
                        // of the box, level with it: on its top-right
                        // corner it covered the next box on the line (UI
                        // check item 9). Paired fill and text colours.
                        let c = Pos2::new(r.min.x - 8.0, r.center().y);
                        painter.circle_filled(c, 6.0, tokens.warn_badge);
                        painter.text(
                            c,
                            egui::Align2::CENTER_CENTER,
                            "?",
                            egui::FontId::proportional(10.0),
                            tokens.on_warn,
                        );
                    }
                    TargetLevel::NotFound => {
                        dashed_rect(&painter, r, Stroke::new(width, tokens.border_strong));
                    }
                }
                if focused {
                    painter.rect_filled(r, 2.0, tokens.accent.gamma_multiply(0.12));
                }
            }
            // The field with the keyboard was not found on the page: a
            // dashed frame round the page says so.
            if let Some(t) = view.focused_target()
                && t.spot.is_none()
            {
                dashed_rect(
                    &painter,
                    page_rect.shrink(3.0),
                    Stroke::new(2.0, tokens.border_strong),
                );
            }
        }
        // The words picked and the keyboard's word.
        if let Some(words) = self.page_words(n) {
            if let Some(p) = self.pick.filter(|p| p.page == n) {
                for i in p.first..=p.last {
                    if let Some(w) = words.get(usize::try_from(i).unwrap_or(usize::MAX)) {
                        let b = word_box(n, w);
                        let r = box_rect(page_rect, rotation, b).expand(1.0);
                        painter.rect_filled(r, 1.0, tokens.accent.gamma_multiply(0.25));
                    }
                }
            }
            if let Some((p, i)) = self.cursor.filter(|(p, _)| *p == n)
                && let Some(w) = words.get(usize::try_from(i).unwrap_or(usize::MAX))
            {
                let r = box_rect(page_rect, rotation, word_box(p, w)).expand(2.0);
                painter.rect_stroke(r, 1.0, Stroke::new(2.0, tokens.accent), StrokeKind::Outside);
            }
        }
        // The box being dragged.
        if let (Some(a), Some(b)) = (self.drag_from, self.drag_to) {
            let r = Rect::from_two_pos(page_rect.clamp(a), page_rect.clamp(b));
            painter.rect_filled(r, 0.0, tokens.accent.gamma_multiply(0.15));
            painter.rect_stroke(r, 0.0, Stroke::new(1.5, tokens.accent), StrokeKind::Inside);
        }
        if let Some(s) = self.scroll_to.take() {
            if s.page == n {
                ui.scroll_to_rect(
                    box_rect(page_rect, rotation, s).expand(24.0),
                    Some(Align::Center),
                );
            } else {
                self.scroll_to = Some(s);
            }
        }
    }

    /// What the control channel sees (`state.doc_viewer.highlight`): names,
    /// levels and counts.
    #[must_use]
    pub fn state(&self, page: u16) -> HighlightState {
        let Some(view) = &self.view else {
            return HighlightState::default();
        };
        let f = &view.feed;
        let focused = view.focused_target();
        HighlightState {
            on: f.highlights,
            drawn: if f.highlights {
                f.targets
                    .iter()
                    .filter(|t| t.spot.is_some_and(|s| s.page == page))
                    .count()
            } else {
                0
            },
            focused: f.focused.map(key_name),
            page: Some(page),
            level: focused.map(|t| t.level.name()),
            armed: f.armed.map(key_name),
            words: self.page_words(page).map_or(0, <[_]>::len),
            cursor: self.cursor.filter(|(p, _)| *p == page).map(|(_, i)| i),
            picked: self
                .pick
                .filter(|p| p.page == page)
                .map_or(0, |p| usize::try_from(p.last - p.first + 1).unwrap_or(0)),
            note: view.note.map(super::CompareNote::name).or_else(|| {
                (f.armed.is_some() && view.words.is_none())
                    .then_some(super::CompareNote::WordsLoading.name())
            }),
            checklist: self.checklist,
        }
    }

    /// True once after the tools left this viewer (it then publishes the
    /// default state once).
    pub fn left(&mut self) -> bool {
        let now = self.view.is_some();
        let left = self.shown_before && !now;
        self.shown_before = now;
        left
    }
}

fn word_box(n: u16, w: &cl_docread::model::Word) -> PageBox {
    PageBox {
        page: n,
        l: w.bbox.left(),
        t: w.bbox.top(),
        r: w.bbox.right(),
        b: w.bbox.bottom(),
    }
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let path = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(Shape::dashed_line(&path, stroke, 5.0, 4.0));
}

/// The check list's snippet: the part of `source` that shows `spot`, a
/// little room round it, 36 points high (at most 180 wide). No new
/// decoding: the texture is the viewer's.
pub fn snippet(ui: &mut Ui, source: &SnippetSource, spot: PageBox, label: &str) -> Response {
    let unit_rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    let uv = box_rect(unit_rect, source.rotation, spot)
        .expand(0.01)
        .intersect(unit_rect);
    let px = Vec2::new(uv.width() * source.size.x, uv.height() * source.size.y);
    let h = 36.0;
    let w = if px.y > 0.0 {
        (h * px.x / px.y).clamp(24.0, 180.0)
    } else {
        h
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::WHITE);
    painter.image(source.texture, rect, uv, Color32::WHITE);
    resp.widget_info(|| WidgetInfo::labeled(Role::Image, true, label));
    resp
}

/// The room a snippet takes when there is none to show, so rows line up.
pub fn snippet_space(ui: &mut Ui) {
    ui.allocate_exact_size(Vec2::new(36.0, 36.0), Sense::hover());
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::widgets::compare::{CompareFeed, CompareTarget, DocWords, FillKind};
    use cl_docread::model::{Bbox, Confidence, PageWords, Word, WordSource};
    use eframe::egui::{Context, RawInput, pos2, vec2};

    const ALL: [Rotation; 4] = [
        Rotation::None,
        Rotation::Right,
        Rotation::Half,
        Rotation::Left,
    ];

    /// A box at the original page's top left lands at the corner the turn
    /// takes it to, and a point maps back where it came from.
    #[test]
    fn boxes_follow_each_turn_and_points_map_back() {
        let page = Rect::from_min_size(pos2(100.0, 50.0), vec2(200.0, 400.0));
        let corner = PageBox::new(0, 0, 0, 1000, 1000).unwrap();
        let near = |r: Rect, x: f32, y: f32| {
            (r.center().x - x).abs() < 0.5 && (r.center().y - y).abs() < 0.5
        };
        // Top left, top right, bottom right, bottom left of the drawn page.
        assert!(near(box_rect(page, Rotation::None, corner), 110.0, 70.0));
        assert!(near(box_rect(page, Rotation::Right, corner), 290.0, 70.0));
        assert!(near(box_rect(page, Rotation::Half, corner), 290.0, 430.0));
        assert!(near(box_rect(page, Rotation::Left, corner), 110.0, 430.0));
        for rot in ALL {
            for (x, y) in [(0, 0), (2500, 7500), (10_000, 10_000), (9_999, 1)] {
                let (u, v) = to_display(rot, unit(x), unit(y));
                let p = pos2(
                    page.min.x + u * page.width(),
                    page.min.y + v * page.height(),
                );
                let back = point_on_page(page.expand(0.01), rot, p).expect("on the page");
                assert!(
                    back.0.abs_diff(x) <= 2 && back.1.abs_diff(y) <= 2,
                    "{rot:?} {x},{y} -> {back:?}"
                );
            }
        }
        assert_eq!(point_on_page(page, Rotation::None, pos2(0.0, 0.0)), None);
    }

    #[test]
    fn a_drag_becomes_a_box_on_the_original_page() {
        let page = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 100.0));
        // Dragged from the drawn top right corner area, page turned right.
        let b = drag_box(page, Rotation::Right, 1, pos2(90.0, 0.0), pos2(100.0, 10.0)).unwrap();
        assert_eq!(b, PageBox::new(1, 0, 0, 1000, 1000).unwrap());
        // Off the page is clamped.
        let b = drag_box(
            page,
            Rotation::None,
            0,
            pos2(-50.0, 50.0),
            pos2(500.0, 60.0),
        )
        .unwrap();
        assert_eq!((b.l, b.r, b.t, b.b), (0, 10_000, 5000, 6000));
    }

    fn word(text: &str, l: u16, t: u16, r: u16, b: u16) -> Word {
        Word {
            text: text.into(),
            bbox: Bbox::new(l, t, r, b).unwrap(),
            confidence: Confidence::CERTAIN,
        }
    }

    fn view(armed: bool) -> CompareView {
        let total = TargetKey::field("amount_due");
        let date = TargetKey::field("bill_date");
        CompareView {
            feed: CompareFeed {
                targets: vec![
                    CompareTarget {
                        key: total,
                        label: "Amount due".into(),
                        spot: PageBox::new(0, 7000, 8000, 9000, 8500),
                        level: TargetLevel::CheckThis,
                        // Any fill kind: the overlay never reads values.
                        fill: Some(FillKind::Text),
                    },
                    CompareTarget {
                        key: date,
                        label: "Date".into(),
                        spot: PageBox::new(0, 1000, 1000, 3000, 1500),
                        level: TargetLevel::Sure,
                        fill: Some(FillKind::Date),
                    },
                ],
                armed: armed.then_some(total),
                focused: Some(total),
                highlights: true,
                generation: 1,
            },
            values: vec!["$12.50".into(), "3/14/2026".into()],
            words: Some(Arc::new(DocWords {
                pages: vec![PageWords {
                    page: 0,
                    aspect: 12_941,
                    source: WordSource::TextLayer,
                    words: vec![
                        word("$", 7000, 8000, 7400, 8500),
                        word("12.50", 7500, 8000, 9000, 8500),
                        word("3/14/2026", 1000, 1000, 3000, 1500),
                    ],
                }],
            })),
            note: None,
        }
    }

    #[test]
    fn clicks_pick_words_shift_extends_and_other_boxes_focus() {
        let mut o = Overlay::default();
        o.set_view(Some(view(true)));
        // A word inside the armed field's own box is picked.
        o.click(0, Some((8000, 8200)), false, None);
        assert_eq!(
            o.take_events(),
            vec![CompareEvent::PageClick {
                page: 0,
                first: 1,
                last: 1
            }]
        );
        // Shift+click on "$" adds it.
        o.click(0, Some((7100, 8200)), true, None);
        assert_eq!(
            o.take_events(),
            vec![CompareEvent::PageClick {
                page: 0,
                first: 0,
                last: 1
            }]
        );
        // Another field's box focuses that field.
        o.click(0, Some((2000, 1200)), false, None);
        assert_eq!(
            o.take_events(),
            vec![CompareEvent::TargetClicked(TargetKey::field("bill_date"))]
        );
        // An accessibility click (no pointer) on a box focuses its field.
        o.click(0, None, false, Some(TargetKey::field("amount_due")));
        assert_eq!(
            o.take_events(),
            vec![CompareEvent::TargetClicked(TargetKey::field("amount_due"))]
        );
        // Nothing armed: a word click outside any box does nothing.
        o.set_view(Some(view(false)));
        o.click(0, Some((5000, 5000)), false, None);
        assert_eq!(o.take_events(), Vec::new());
        let s = o.state(0);
        assert_eq!(
            (s.drawn, s.words, s.picked),
            (2, 3, 0),
            "another words set (a new view here) drops the pick"
        );
        assert_eq!(s.focused.as_deref(), Some("amount_due"));
        assert_eq!(s.level, Some("check_this"));
    }

    #[test]
    fn the_keyboard_steps_words_and_enter_fills() {
        let mut o = Overlay::default();
        assert!(!o.command("viewer.next_word", 0), "no tools, no keys");
        o.set_view(Some(view(true)));
        assert!(
            !o.command("viewer.fill_word", 0),
            "no word yet: Enter stays"
        );
        assert!(o.command("viewer.next_word", 0));
        assert!(o.command("viewer.next_word", 0));
        assert_eq!(o.state(0).cursor, Some(1));
        assert!(o.command("viewer.prev_word", 0));
        assert!(o.command("viewer.prev_word", 0));
        assert_eq!(o.state(0).cursor, Some(0), "stays at the first word");
        assert!(o.command("viewer.next_word", 0));
        assert!(o.command("viewer.fill_word", 0));
        assert_eq!(
            o.take_events(),
            vec![CompareEvent::PageClick {
                page: 0,
                first: 1,
                last: 1
            }]
        );
        assert!(o.command("viewer.toggle_highlights", 0));
        assert_eq!(o.take_events(), vec![CompareEvent::ToggleHighlights]);
        // New words (a re-read): the old indices go.
        assert_eq!(o.state(0).cursor, Some(1));
        o.set_view(Some(view(true)));
        assert_eq!((o.state(0).cursor, o.state(0).picked), (None, 0));
        assert!(!o.command("viewer.next_word", 3), "a page without words");
        o.set_view(Some(view(false)));
        assert!(!o.command("viewer.fill_word", 0), "nothing armed");
        assert!(!o.command("viewer.zoom_in", 0));
    }

    #[test]
    fn a_new_focus_turns_to_its_page_once() {
        let mut o = Overlay::default();
        let mut v = view(true);
        if let Some(t) = v.feed.targets.get_mut(0) {
            t.spot = PageBox::new(2, 0, 0, 100, 100);
        }
        o.set_view(Some(v.clone()));
        assert_eq!(o.turn_to(), Some(2));
        assert_eq!(o.turn_to(), None, "only when the focus changes");
        v.feed.focused = Some(TargetKey::field("bill_date"));
        o.set_view(Some(v));
        assert_eq!(o.turn_to(), Some(0));
        assert_eq!(o.find(TargetKey::field("amount_due")), Some(2));
        assert!(!o.left());
        assert!(o.active());
        o.set_view(None);
        assert!(o.left());
        assert!(!o.left(), "once");
        assert_eq!(o.state(0), HighlightState::default());
    }

    /// Drawn on a real egui frame: the boxes are accessible buttons named
    /// for their field; a drag on an armed page makes a `PageBox`.
    #[test]
    fn boxes_are_named_and_a_drag_makes_a_box() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let tokens = Tokens::of(&ctx);
        let mut o = Overlay::default();
        o.set_view(Some(view(true)));
        let page = Rect::from_min_size(pos2(20.0, 20.0), vec2(400.0, 500.0));
        let frame = |o: &mut Overlay, events: Vec<egui::Event>| {
            let raw = RawInput {
                events,
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(raw, |ui| {
                let r = ui.interact(page, egui::Id::unique("test-page"), Sense::click_and_drag());
                o.paint(ui, page, Rotation::None, 0, &r, &tokens);
            });
            out.textures_delta.clear();
            out.platform_output
                .accesskit_update
                .map(|u| {
                    u.nodes
                        .iter()
                        .filter_map(|(_, n)| n.label().map(str::to_owned))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let names = frame(&mut o, Vec::new());
        assert!(
            names.iter().any(|n| n == "Amount due on the page"),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n == "Date on the page"), "{names:?}");
        let press = |p: Pos2, pressed: bool| egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let a = pos2(60.0, 300.0);
        let b = pos2(260.0, 400.0);
        frame(&mut o, vec![egui::Event::PointerMoved(a), press(a, true)]);
        frame(&mut o, vec![egui::Event::PointerMoved(pos2(150.0, 350.0))]);
        frame(&mut o, vec![egui::Event::PointerMoved(b)]);
        frame(&mut o, vec![press(b, false)]);
        let events = o.take_events();
        let boxed: Vec<PageBox> = events
            .iter()
            .filter_map(|e| match e {
                CompareEvent::PageBox(b) => Some(*b),
                _ => None,
            })
            .collect();
        assert_eq!(boxed.len(), 1, "{events:?}");
        assert_eq!(
            (boxed[0].l, boxed[0].t, boxed[0].r, boxed[0].b),
            (1000, 5600, 6000, 7600)
        );
    }

    #[test]
    fn a_snippet_is_cut_from_the_drawn_texture() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let source = SnippetSource {
            texture: TextureId::Managed(0),
            size: vec2(1000.0, 1300.0),
            rotation: Rotation::None,
        };
        let mut size = Vec2::ZERO;
        let mut out = ctx.run_ui(RawInput::default(), |ui| {
            let spot = PageBox::new(0, 7000, 8000, 9000, 8500).unwrap();
            size = snippet(ui, &source, spot, "Amount due").rect.size();
        });
        out.textures_delta.clear();
        assert!((size.y - 36.0).abs() < 0.01);
        assert!(size.x > 24.0 && size.x <= 180.0, "{size:?}");
    }
}
