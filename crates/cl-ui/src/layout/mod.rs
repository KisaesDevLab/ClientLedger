//! The shared screen layout (Q-265): the title, toolbar, filters and column
//! headers stay at the top, the actions (Clear, Save & New, Save & Close,
//! totals) stay at the bottom, and only the content between them scrolls.
//!
//! A screen draws in three calls on a [`FixedFrame`], in focus order:
//!
//! ```ignore
//! let mut frame = FixedFrame::begin(ui, "write_check", FrameOpts::default());
//! frame.top(|ui| self.toolbar(ui));
//! frame.body(Body::Scroll, |ui| self.form(ui));
//! frame.bottom(|ui| self.actions(ui));
//! self.layout = frame.end();
//! ```
//!
//! Each call borrows the screen on its own, so the parts can all change it.
//! The bottom bar is placed from its height last frame (kept in egui memory
//! under the frame's id); a change lays the frame out again at once.
//!
//! The minimum window at 200% zoom (480 × 320 points): when the two bars
//! together would take more than `max_fixed_share` of the height, the top bar
//! scrolls on its own, capped so the body keeps `min_body_rows` rows and the
//! actions stay fully on screen. A `Body::Fill` body shorter than its rows
//! gives its content that height and scrolls it. A bottom bar too tall for
//! the window's least top, two body rows and itself (the register's entry
//! row at 200%) scrolls too, its end (the actions) in view.
//!
//! The shell draws a screen whose [`crate::windows::Screen::fixed_layout`]
//! is true straight into the window's rectangle; any other screen scrolls
//! whole, as before.

use std::hash::Hash;

use eframe::egui::{self, Id, Rect, Ui, UiBuilder};
use serde::Serialize;

/// How the content between the bars is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Forms and lists of sections: the content scrolls (both ways, as the
    /// window did before).
    Scroll,
    /// Grid screens: the content gets a fixed rectangle (the grid takes it
    /// with `GridHeight::Fill`, its header row at the top, never scrolled
    /// away).
    Fill,
}

/// The frame's rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOpts {
    /// The body always keeps room for this many rows.
    pub min_body_rows: u16,
    /// The height of one body row (points).
    pub body_row_height: f32,
    /// Above this share of the height, the top bar scrolls.
    pub max_fixed_share: f32,
}

impl Default for FrameOpts {
    fn default() -> Self {
        FrameOpts {
            min_body_rows: 3,
            body_row_height: 30.0,
            max_fixed_share: 0.5,
        }
    }
}

/// List screens: the body keeps the column headings, three rows and the
/// count line under them.
pub const LIST_FRAME: FrameOpts = FrameOpts {
    min_body_rows: 5,
    body_row_height: 30.0,
    max_fixed_share: 0.5,
};

/// The sizes the frame used this frame, for the control channel
/// (`state.screen.layout`; sizes only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct FrameReport {
    /// The top bar's height on screen.
    pub top_h: f32,
    /// The bottom bar's height.
    pub bottom_h: f32,
    /// The body's height.
    pub body_h: f32,
    /// The top bar is taller than its room and scrolls on its own.
    pub top_scrolls: bool,
    /// The bottom bar is taller than a very short window allows and scrolls
    /// on its own, its end (the actions) in view.
    pub bottom_scrolls: bool,
}

impl FrameReport {
    /// The report as JSON (whole points).
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        // Whole points, so the state never carries a decimal point.
        #[allow(clippy::cast_possible_truncation)] // screen sizes fit easily
        let points = |v: f32| v.round().clamp(0.0, 1.0e6) as i64;
        serde_json::json!({
            "top_h": points(self.top_h),
            "bottom_h": points(self.bottom_h),
            "body_h": points(self.body_h),
            "top_scrolls": self.top_scrolls,
            "bottom_scrolls": self.bottom_scrolls,
        })
    }
}

/// Space between the bars and the body.
const GAP: f32 = 6.0;

/// The bottom bar's height before it was ever measured (an action row and a
/// totals line).
const FIRST_BOTTOM: f32 = 64.0;

/// The top bar's room never goes below one row.
const MIN_TOP: f32 = 30.0;

/// The bottom bar's room never goes below one action row.
const MIN_BOTTOM: f32 = 48.0;

/// Heights measured last frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Measured {
    /// The top bar's full (unscrolled) height.
    top: Option<f32>,
    /// The bottom bar's height (0 when the screen has none).
    bottom: Option<f32>,
}

/// One frame of a screen with fixed bars. See the module docs.
pub struct FixedFrame<'u> {
    ui: &'u mut Ui,
    id: Id,
    opts: FrameOpts,
    full: Rect,
    last: Measured,
    now: Measured,
    /// Where the next part starts.
    cursor_y: f32,
    report: FrameReport,
    bottom_drawn: bool,
}

impl<'u> FixedFrame<'u> {
    /// Starts a frame filling what is left of `ui`.
    pub fn begin(
        ui: &'u mut Ui,
        id_salt: impl Hash + std::fmt::Debug,
        opts: FrameOpts,
    ) -> FixedFrame<'u> {
        let id = ui.scope_id().with(("cl.fixed_frame", id_salt));
        let full = ui.available_rect_before_wrap();
        let last: Measured = ui.data(|d| d.get_temp(id)).unwrap_or_default();
        FixedFrame {
            ui,
            id,
            opts,
            full,
            last,
            now: Measured::default(),
            cursor_y: full.top(),
            report: FrameReport::default(),
            bottom_drawn: false,
        }
    }

    /// The bottom bar's height as placed this frame (last frame's, capped).
    fn bottom_shown(&self) -> f32 {
        let h = self.last.bottom.unwrap_or(FIRST_BOTTOM);
        if h > 0.0 {
            h.min(self.bottom_cap())
        } else {
            0.0
        }
    }

    /// The bottom bar with its gap.
    fn bottom_room(&self) -> f32 {
        let h = self.bottom_shown();
        if h > 0.0 { h + GAP } else { 0.0 }
    }

    /// The most the bottom bar may take: a window too short for the top's
    /// least, two body rows and the whole bar (the register's entry row at
    /// 200% zoom) scrolls the bar instead, its end (the actions) in view.
    fn bottom_cap(&self) -> f32 {
        let two_rows = 2.0 * self.opts.body_row_height;
        (self.full.height() - MIN_TOP - GAP - two_rows - GAP).max(MIN_BOTTOM)
    }

    fn min_body(&self) -> f32 {
        f32::from(self.opts.min_body_rows) * self.opts.body_row_height
    }

    /// The most the top bar may take, or `None` when it shows whole.
    fn top_cap(&self) -> Option<f32> {
        let avail = self.full.height();
        let bottom = self.bottom_room();
        let top = self.last.top?;
        if top + bottom <= self.opts.max_fixed_share * avail {
            return None;
        }
        let cap = (avail - bottom - self.min_body() - GAP).max(MIN_TOP);
        (top > cap).then_some(cap)
    }

    fn child(&mut self, rect: Rect, salt: &'static str) -> Ui {
        let mut child = self.ui.new_child(
            UiBuilder::new()
                .id_salt((self.id, salt))
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.shrink_clip_rect(rect);
        child
    }

    /// The top bar: title, toolbar, filters. Wraps; scrolls only when it
    /// would take too much of a short window.
    pub fn top<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let cap = self.top_cap();
        let room = Rect::from_min_max(
            egui::pos2(self.full.left(), self.cursor_y),
            egui::pos2(
                self.full.right(),
                cap.map_or(self.full.bottom(), |c| self.cursor_y + c),
            ),
        );
        let mut child = self.child(room, "top");
        // A row too wide for the window (a filter row that doesn't wrap)
        // scrolls sideways rather than being cut off.
        let (inner, natural, shown) = if let Some(cap) = cap {
            let out = egui::ScrollArea::both()
                .id_salt((self.id, "top-scroll"))
                .auto_shrink([false, true])
                .max_height(cap)
                .min_scrolled_height(cap)
                .show(&mut child, add);
            (out.inner, out.content_size.y, out.inner_rect.height())
        } else {
            let out = egui::ScrollArea::horizontal()
                .id_salt((self.id, "top-scroll"))
                .auto_shrink([false, true])
                .show(&mut child, add);
            let h = child.min_rect().height();
            (out.inner, h, h)
        };
        self.now.top = Some(natural);
        self.report.top_h = shown;
        self.report.top_scrolls = cap.is_some();
        self.cursor_y += shown + GAP;
        inner
    }

    /// The content between the bars.
    pub fn body<R>(&mut self, body: Body, add: impl FnOnce(&mut Ui) -> R) -> R {
        let bottom = (self.full.bottom() - self.bottom_room()).max(self.cursor_y);
        let rect = Rect::from_min_max(
            egui::pos2(self.full.left(), self.cursor_y),
            egui::pos2(self.full.right(), bottom),
        );
        self.report.body_h = rect.height();
        let mut child = self.child(rect, "body");
        let inner = match body {
            Body::Scroll => {
                egui::ScrollArea::both()
                    .id_salt((self.id, "body-scroll"))
                    .auto_shrink([false, false])
                    .max_height(rect.height())
                    .min_scrolled_height(rect.height())
                    .show(&mut child, add)
                    .inner
            }
            // Too short for its rows (200% zoom on the minimum window): the
            // content gets its minimum height and scrolls in the room.
            Body::Fill if rect.height() + 0.5 < self.min_body() => {
                let min = self.min_body();
                egui::ScrollArea::vertical()
                    .id_salt((self.id, "body-fill-scroll"))
                    .auto_shrink([false, false])
                    .max_height(rect.height())
                    .min_scrolled_height(rect.height())
                    .show(&mut child, |ui| {
                        let r = ui.available_rect_before_wrap();
                        let room = Rect::from_min_size(r.min, egui::vec2(r.width(), min));
                        let out = ui.scope_builder(UiBuilder::new().max_rect(room), add);
                        ui.advance_cursor_after_rect(room);
                        out.inner
                    })
                    .inner
            }
            Body::Fill => add(&mut child),
        };
        self.cursor_y = bottom;
        inner
    }

    /// The bottom bar: always fully on screen.
    pub fn bottom<R>(&mut self, add: impl FnOnce(&mut Ui) -> R) -> R {
        let placed = self.bottom_shown();
        let cap = self.bottom_cap();
        let scrolls = self.last.bottom.is_some_and(|h| h > cap + 0.5);
        let top = (self.full.bottom() - placed).max(self.cursor_y.min(self.full.bottom()));
        let rect = Rect::from_min_max(
            egui::pos2(self.full.left(), top),
            egui::pos2(self.full.right(), self.full.bottom().max(top)),
        );
        // Not clipped to `rect`: a bar taller than last frame shows whole
        // while the frame is laid out again.
        let mut child = self.ui.new_child(
            UiBuilder::new()
                .id_salt((self.id, "bottom"))
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        let (inner, natural, shown) = if scrolls {
            child.shrink_clip_rect(rect);
            let out = egui::ScrollArea::vertical()
                .id_salt((self.id, "bottom-scroll"))
                .auto_shrink([false, true])
                .max_height(cap)
                .min_scrolled_height(cap)
                .stick_to_bottom(true)
                .show(&mut child, add);
            (out.inner, out.content_size.y, out.inner_rect.height())
        } else {
            let inner = add(&mut child);
            let h = child.min_rect().height();
            (inner, h, h)
        };
        self.now.bottom = Some(natural);
        self.report.bottom_h = shown;
        self.report.bottom_scrolls = scrolls;
        self.bottom_drawn = true;
        inner
    }

    /// Ends the frame: remembers the heights (a change lays the frame out
    /// again) and takes the whole rectangle in the parent.
    #[must_use]
    pub fn end(mut self) -> FrameReport {
        if !self.bottom_drawn {
            self.now.bottom = Some(0.0);
        }
        let changed = |a: Option<f32>, b: Option<f32>| match (a, b) {
            (Some(a), Some(b)) => (a - b).abs() > 0.5,
            (None, None) => false,
            _ => true,
        };
        let moved = changed(self.last.bottom, self.now.bottom)
            || (self.now.top.is_some() && changed(self.last.top, self.now.top));
        if moved {
            let id = self.id;
            let now = Measured {
                top: self.now.top.or(self.last.top),
                bottom: self.now.bottom,
            };
            self.ui.data_mut(|d| d.insert_temp(id, now));
            self.ui.ctx().request_discard("fixed frame measured");
            self.ui.ctx().request_repaint();
        }
        let full = self.full;
        self.ui.advance_cursor_after_rect(full);
        self.report
    }
}

#[cfg(test)]
mod tests;
