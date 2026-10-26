//! The document viewer (P17, plan §3 WP1): one widget for the Enter Bill
//! source-document card, its full-size tab (`screens::document_view`), the
//! receipt inbox preview and Write Check's attachment preview.
//!
//! # Handoff (WP0 skeleton, WP1 filled it)
//!
//! Fixed by WP0, called by WP4, WP5 and the full-size tab (later WPs may add,
//! not change):
//! - [`DocViewer::show`]`(ui, &mut ViewerState, &Tokens) -> ViewerEvent`,
//!   drawn every frame the preview is visible. It fills the width it is
//!   given and the height the caller leaves (give the card a fixed height,
//!   e.g. `ui.allocate_ui(vec2(w, 360.0), ..)`).
//! - [`ViewerState`] (`Default` = nothing open): [`ViewerState::open`],
//!   [`ViewerState::clear`], [`ViewerState::source`], [`ViewerState::page`]
//!   (from 0), [`ViewerState::page_count`], [`ViewerState::zoom`],
//!   [`ViewerState::has_focus`] (the screen then reports
//!   `Scope::Viewer`, so the `viewer.*` keys reach it),
//!   [`ViewerState::set_full_size_button`], [`ViewerState::command`] (runs
//!   `viewer.zoom_in`, `viewer.zoom_out`, `viewer.fit`, `viewer.page_next`,
//!   `viewer.page_prev`; returns true when handled) and
//!   [`ViewerState::state_json`] (no file names). WP1 added
//!   [`ViewerState::focus`] (put the keyboard in the viewer next frame) and
//!   [`ViewerState::status`].
//! - [`ViewerEvent`]: `OpenFullSize` (the screen pushes
//!   `CompanyEvent::OpenWith(nav.document_view, OpenPayload::Document(..))`)
//!   and `SaveCopy` (the screen offers Save a copy, P13).
//! - [`DocSource`]: the file name (owner data: never logged, `Debug`
//!   redacted) and its bytes.
//!
//! A02 WP-A added (A02 plan D1): [`ViewerState::open_keep_zoom`],
//! [`ViewerState::release_pixels`], [`ViewerState::set_pop_out_button`]
//! ("Pop out" answers [`ViewerEvent::PopOut`]; `Dock` is drawn by
//! `doc_pane`), [`ViewerState::set_zoom`], [`ViewerState::rotation`]; the
//! `viewer.rotate_cw` / `viewer.rotate_ccw` commands (per page, view only,
//! reset by a new document); a zoom menu (Fit page, Fit width, 50–400 %);
//! dragging or the arrow keys pan the page; multi-page TIFF; a page the
//! worker can't draw says so with Save a copy while the others show.
//! `state_json` gained `rotation` and `page_unreadable`.
//!
//! The pages come from [`super::doc_worker`] (a background thread): the
//! viewer asks for the page at the pixel width it shows, keeps one texture
//! (freed when the page, the document or the state goes) and polls replies
//! each frame, so drawing never waits. Zoom is UI geometry (`f32`), not
//! money. Without PDFium a PDF shows "Preview isn't available on this
//! computer" with Save a copy; pictures still preview.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use cl_model::CommandId;
use eframe::egui::{
    self, Color32, ColorImage, Rect, RichText, Role, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, Vec2, WidgetInfo,
};
use serde_json::{Value, json};

use super::doc_worker::{DocReply, DocRequest, DocWorker, RenderedPage, Rotation};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// A document to show.
#[derive(Clone, PartialEq, Eq)]
pub struct DocSource {
    /// The file name as the owner knows it (never logged).
    pub name: String,
    /// The file's bytes.
    pub bytes: Arc<[u8]>,
}

/// The size only: the name is owner data.
impl std::fmt::Debug for DocSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocSource")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// How the page is sized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zoom {
    /// The page fills the width.
    #[default]
    FitWidth,
    /// The whole page shows.
    FitPage,
    /// A fixed size, 25–400 %.
    Percent(u16),
}

/// The zoom steps, in percent.
pub const ZOOM_STEPS: [u16; 15] = [
    25, 33, 50, 67, 75, 90, 100, 110, 125, 150, 175, 200, 250, 300, 400,
];

/// The step after `current` percent (Zoom in), at most 400.
#[must_use]
pub fn zoom_step_in(current: f32) -> u16 {
    ZOOM_STEPS
        .iter()
        .copied()
        .find(|s| f32::from(*s) > current + 0.5)
        .unwrap_or(400)
}

/// The step before `current` percent (Zoom out), at least 25.
#[must_use]
pub fn zoom_step_out(current: f32) -> u16 {
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|s| f32::from(*s) < current - 0.5)
        .unwrap_or(25)
}

/// The page's shown width in points for `zoom`, in an area `avail`, for a
/// page `natural` (points at 100 %) in size.
#[must_use]
pub fn display_width(zoom: Zoom, avail: Vec2, natural: Vec2) -> f32 {
    let avail_w = avail.x.max(1.0);
    match zoom {
        Zoom::FitWidth => avail_w,
        Zoom::FitPage => {
            if natural.x > 0.0 && natural.y > 0.0 {
                avail_w.min(avail.y.max(1.0) * natural.x / natural.y)
            } else {
                avail_w
            }
        }
        Zoom::Percent(p) => natural.x.max(1.0) * f32::from(p) / 100.0,
    }
}

/// What the viewer asks its screen to do this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewerEvent {
    /// Nothing.
    #[default]
    None,
    /// "Open full size": show the document in its own tab.
    OpenFullSize,
    /// "Save a copy" (the preview is unavailable or the PDF is protected).
    SaveCopy,
    /// "Pop out" (A02): show the document in the shell's viewer window.
    /// Offered when [`ViewerState::set_pop_out_button`] is on.
    PopOut,
    /// "Dock" (A02): bring the document back into the form. Drawn by
    /// `widgets::doc_pane` on the popped-out placeholder, not by the viewer.
    Dock,
}

/// Where the open document is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing open.
    #[default]
    Empty,
    /// The worker is reading it.
    Loading,
    /// Pages can be shown.
    Ready,
    /// A password-protected PDF.
    Protected,
    /// Not a readable PDF or image.
    Unreadable,
    /// A PDF, and PDFium is not on this computer.
    Unavailable,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Empty => "empty",
            Status::Loading => "loading",
            Status::Ready => "ready",
            Status::Protected => "protected",
            Status::Unreadable => "unreadable",
            Status::Unavailable => "unavailable",
        }
    }
}

/// The page on screen.
struct Shown {
    page: u32,
    width: u32,
    rotation: Rotation,
    texture: TextureHandle,
    natural: Vec2,
}

/// A02: one thumbnail.
struct Thumb {
    texture: TextureHandle,
    /// Its size in pixels.
    size: Vec2,
    /// The frame it was last drawn.
    used: u64,
}

/// Thumbnail textures kept per viewer (96 px wide: about 3 MB in all).
pub const MAX_THUMBS: usize = 64;
/// A thumbnail's width asked of the worker, in pixels.
const THUMB_PX: u32 = 96;
/// The strip shows when the viewer is at least this wide (Q-A02l).
pub const THUMB_STRIP_MIN_WIDTH: f32 = 520.0;
/// The strip's width and one row's height, in points.
const STRIP_W: f32 = 96.0;
const THUMB_ROW_H: f32 = 118.0;
/// New thumbnails asked for per frame, so a page render never queues
/// behind a burst of them.
const THUMB_ASKS_PER_FRAME: usize = 4;

/// One viewer's state.
#[derive(Default)]
pub struct ViewerState {
    source: Option<DocSource>,
    page: u32,
    pages: u32,
    zoom: Zoom,
    focused: bool,
    full_size_button: bool,
    pop_out_button: bool,
    status: Status,
    truncated: bool,
    worker: Option<DocWorker>,
    /// The worker's number for the open document (new on every open).
    doc: u64,
    /// The last (page, px width, turn) asked for.
    asked: Option<(u32, u32, Rotation)>,
    shown: Option<Shown>,
    /// Each page's turn (A02: view only, never saved; Q-A02b).
    rotations: HashMap<u32, Rotation>,
    /// A page the worker could not draw (the rest of the document shows).
    failed_page: Option<u32>,
    /// A02: thumbnails by page (at most [`MAX_THUMBS`], the least recently
    /// drawn go first).
    thumbs: HashMap<u32, Thumb>,
    /// Thumbnails asked for and not yet answered or dropped.
    thumb_asked: HashSet<u32>,
    /// Pages whose thumbnail can't be drawn (shown blank, not asked again).
    thumb_failed: HashSet<u32>,
    /// Frames drawn, for the thumbnails' least-recently-used order.
    frame: u64,
    /// The strip showed last frame.
    strip_shown: bool,
    /// The page the strip last scrolled to.
    strip_page: Option<u32>,
    /// The strip's scroll offset last frame, in points.
    strip_top: f32,
    /// The page's size at 100 % (known after the first render).
    natural: Option<Vec2>,
    /// The page area last frame (for zoom steps from a fit).
    last_area: Vec2,
    want_focus: bool,
}

/// No file name, no pixels.
impl std::fmt::Debug for ViewerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewerState")
            .field("open", &self.source.is_some())
            .field("status", &self.status)
            .field("page", &self.page)
            .field("pages", &self.pages)
            .field("zoom", &self.zoom)
            .finish_non_exhaustive()
    }
}

impl ViewerState {
    /// Show `source` from its first page.
    pub fn open(&mut self, source: DocSource) {
        self.reset();
        self.status = Status::Loading;
        let worker = self.worker.get_or_insert_with(DocWorker::start);
        worker.send(DocRequest::Open {
            doc: self.doc,
            bytes: Arc::clone(&source.bytes),
        });
        self.source = Some(source);
    }

    /// Show `source` from its first page, keeping the zoom setting (A02: the
    /// pop-out window following a new form keeps the owner's zoom).
    pub fn open_keep_zoom(&mut self, source: DocSource) {
        let zoom = self.zoom;
        self.open(source);
        self.zoom = zoom;
    }

    /// Show nothing.
    pub fn clear(&mut self) {
        self.reset();
    }

    /// Drop the drawn page's texture (A02: the docked form while its
    /// document shows in the pop-out window). The document stays open; the
    /// page is asked for again the next time the viewer is drawn.
    pub fn release_pixels(&mut self) {
        self.shown = None;
        self.asked = None;
        self.drop_thumbs();
    }

    /// Forget the open document (the worker too), keeping the worker, the
    /// button choice and the document numbering.
    fn reset(&mut self) {
        if self.source.is_some()
            && let Some(w) = &self.worker
        {
            w.send(DocRequest::Close { doc: self.doc });
        }
        self.doc += 1;
        self.source = None;
        self.page = 0;
        self.pages = 0;
        self.zoom = Zoom::default();
        self.status = Status::Empty;
        self.truncated = false;
        self.asked = None;
        self.shown = None;
        self.natural = None;
        self.rotations.clear();
        self.failed_page = None;
        self.drop_thumbs();
        self.strip_page = None;
    }

    /// Forget the thumbnails (their textures free as they drop).
    fn drop_thumbs(&mut self) {
        self.thumbs.clear();
        self.thumb_asked.clear();
        self.thumb_failed.clear();
    }

    /// The document shown.
    #[must_use]
    pub fn source(&self) -> Option<&DocSource> {
        self.source.as_ref()
    }

    /// The page shown, from 0.
    #[must_use]
    pub fn page(&self) -> u32 {
        self.page
    }

    /// Pages known (0 until the worker opened the document).
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.pages
    }

    /// The zoom setting.
    #[must_use]
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// Set the zoom (A02: the zoom menu; a percent is kept within 25–400).
    pub fn set_zoom(&mut self, zoom: Zoom) {
        self.zoom = match zoom {
            Zoom::Percent(p) => Zoom::Percent(p.clamp(25, 400)),
            z => z,
        };
    }

    /// The shown page's turn (view only).
    #[must_use]
    pub fn rotation(&self) -> Rotation {
        self.rotations.get(&self.page).copied().unwrap_or_default()
    }

    /// Turn the shown page a quarter turn. The page size at 100 % swaps
    /// at once so the layout doesn't wait for the worker.
    fn turn(&mut self, to: Rotation) {
        let from = self.rotation();
        if from == to {
            return;
        }
        if from.swaps() != to.swaps()
            && let Some(n) = &mut self.natural
        {
            *n = Vec2::new(n.y, n.x);
        }
        if to == Rotation::None {
            self.rotations.remove(&self.page);
        } else {
            self.rotations.insert(self.page, to);
        }
    }

    /// True when the page on screen is the shown page at its turn.
    fn page_drawn(&self) -> bool {
        self.shown
            .as_ref()
            .is_some_and(|s| s.page == self.page && s.rotation == self.rotation())
    }

    /// Where the document is (loading, ready, protected …).
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    /// True while the keyboard is in the viewer (last frame).
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.focused
    }

    /// Put the keyboard in the viewer on the next frame (the full-size tab
    /// does on open, so the zoom and page keys work at once).
    pub fn focus(&mut self) {
        self.want_focus = true;
    }

    /// Offer "Open full size" (the card does; the full-size tab does not).
    pub fn set_full_size_button(&mut self, on: bool) {
        self.full_size_button = on;
    }

    /// Offer "Pop out" (A02: the docked viewer does; the pop-out window and
    /// the full-size tab do not). A click answers [`ViewerEvent::PopOut`].
    pub fn set_pop_out_button(&mut self, on: bool) {
        self.pop_out_button = on;
    }

    /// The zoom in effect, in percent of the page's own size (100 before
    /// the first page is drawn).
    fn effective_percent(&self) -> f32 {
        match (self.zoom, self.natural) {
            (Zoom::Percent(p), _) => f32::from(p),
            (z, Some(n)) if n.x > 0.0 => display_width(z, self.last_area, n) / n.x * 100.0,
            _ => 100.0,
        }
    }

    /// Run a `viewer.*` command. Returns true when handled.
    pub fn command(&mut self, id: CommandId) -> bool {
        if self.source.is_none() {
            return false;
        }
        match id.as_str() {
            "viewer.zoom_in" => {
                self.zoom = Zoom::Percent(zoom_step_in(self.effective_percent()));
            }
            "viewer.zoom_out" => {
                self.zoom = Zoom::Percent(zoom_step_out(self.effective_percent()));
            }
            "viewer.fit" => {
                self.zoom = if self.zoom == Zoom::FitWidth {
                    Zoom::FitPage
                } else {
                    Zoom::FitWidth
                };
            }
            "viewer.page_next" => {
                if self.page + 1 < self.pages {
                    self.page += 1;
                }
            }
            "viewer.page_prev" => {
                self.page = self.page.saturating_sub(1);
            }
            // A02: view only, no shortcut (Q-A02m); the toolbar buttons.
            "viewer.rotate_cw" => self.turn(self.rotation().cw()),
            "viewer.rotate_ccw" => self.turn(self.rotation().ccw()),
            _ => return false,
        }
        true
    }

    /// The control channel's view (no file names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        let zoom = match self.zoom {
            Zoom::FitWidth => json!("fit_width"),
            Zoom::FitPage => json!("fit_page"),
            Zoom::Percent(p) => json!(p),
        };
        // Whole percent for the control channel; the zoom is geometry.
        #[allow(clippy::cast_possible_truncation)]
        let percent = self.effective_percent().round() as i64;
        json!({
            "open": self.source.is_some(),
            "status": self.status.as_str(),
            "page": self.page,
            "pages": self.pages,
            "truncated": self.truncated,
            "zoom": zoom,
            "zoom_kind": if matches!(self.zoom, Zoom::Percent(_)) { "percent" } else { "fit" },
            "zoom_percent": percent,
            "rendered": self.page_drawn(),
            "focused": self.focused,
            "rotation": self.rotation().degrees(),
            "page_unreadable": self.failed_page == Some(self.page),
            "thumbnails": {"shown": self.strip_shown, "loaded": self.thumbs.len()},
        })
    }

    /// Take the worker's replies for the open document.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        let mut rendered = None;
        let mut thumbs = Vec::new();
        while let Some(r) = worker.try_recv() {
            match r {
                DocReply::Opened {
                    doc,
                    pages,
                    truncated,
                } if doc == self.doc => {
                    self.status = Status::Ready;
                    self.pages = pages;
                    self.truncated = truncated;
                    self.page = self.page.min(pages.saturating_sub(1));
                }
                DocReply::Protected { doc } if doc == self.doc => self.status = Status::Protected,
                DocReply::Unreadable { doc } if doc == self.doc => {
                    if self.status == Status::Ready && (self.shown.is_some() || self.pages > 1) {
                        // One page of a readable document: that page says
                        // so, the rest still show (A02, a TIFF page in a
                        // colour this can't read).
                        self.failed_page = self.asked.map(|(page, ..)| page);
                    } else if self.shown.is_none() {
                        self.status = Status::Unreadable;
                    }
                }
                DocReply::Unavailable { doc } if doc == self.doc => {
                    self.status = Status::Unavailable;
                }
                DocReply::Rendered(p) if p.doc == self.doc => rendered = Some(p),
                DocReply::Thumb(p) if p.doc == self.doc => thumbs.push(p),
                DocReply::NoThumb { doc, page } if doc == self.doc => {
                    self.thumb_asked.remove(&page);
                    self.thumb_failed.insert(page);
                }
                // An earlier document's answer.
                _ => {}
            }
        }
        if let Some(p) = rendered {
            self.take_page(ctx, &p);
        }
        for p in thumbs {
            self.take_thumb(ctx, &p);
        }
    }

    /// Keep a thumbnail's texture, dropping the least recently drawn over
    /// [`MAX_THUMBS`].
    fn take_thumb(&mut self, ctx: &egui::Context, p: &RenderedPage) {
        self.thumb_asked.remove(&p.page);
        let (Ok(w), Ok(h)) = (usize::try_from(p.width), usize::try_from(p.height)) else {
            return;
        };
        let max = ctx.input(|i| i.max_texture_side);
        if p.rgba.len() != w * h * 4 || w > max || h > max {
            return;
        }
        // Numbers only in the name, never the file name.
        let texture = ctx.load_texture(
            format!("doc-thumb-{}-{}", self.doc, p.page),
            ColorImage::from_rgba_unmultiplied([w, h], &p.rgba),
            TextureOptions::LINEAR,
        );
        #[allow(clippy::cast_precision_loss)] // At most 4096.
        let size = Vec2::new(p.width as f32, p.height as f32);
        self.thumbs.insert(
            p.page,
            Thumb {
                texture,
                size,
                used: self.frame,
            },
        );
        while self.thumbs.len() > MAX_THUMBS {
            let Some(oldest) = self
                .thumbs
                .iter()
                .min_by_key(|(page, t)| (t.used, **page))
                .map(|(page, _)| *page)
            else {
                break;
            };
            self.thumbs.remove(&oldest);
        }
    }

    fn take_page(&mut self, ctx: &egui::Context, p: &RenderedPage) {
        let (Ok(w), Ok(h)) = (usize::try_from(p.width), usize::try_from(p.height)) else {
            return;
        };
        if p.rgba.len() != w * h * 4 {
            return;
        }
        // The page may be drawn larger than this graphics card takes in one
        // texture (egui refuses it with a panic): scale it down to fit.
        let max = ctx.input(|i| i.max_texture_side).max(1);
        let image = if w > max || h > max {
            let long = w.max(h);
            let (fw, fh) = ((w * max / long).max(1), (h * max / long).max(1));
            let (Ok(sw), Ok(sh), Ok(tw), Ok(th)) = (
                u32::try_from(w),
                u32::try_from(h),
                u32::try_from(fw),
                u32::try_from(fh),
            ) else {
                return;
            };
            let Some(full) = image::RgbaImage::from_raw(sw, sh, p.rgba.clone()) else {
                return;
            };
            let small = image::imageops::thumbnail(&full, tw, th);
            ColorImage::from_rgba_unmultiplied([fw, fh], small.as_raw())
        } else {
            ColorImage::from_rgba_unmultiplied([w, h], &p.rgba)
        };
        // The texture name carries numbers only, never the file name.
        let texture = ctx.load_texture(
            format!("doc-viewer-{}-{}", self.doc, p.page),
            image,
            TextureOptions::LINEAR,
        );
        #[allow(clippy::cast_precision_loss)] // Page sizes are small.
        let natural = Vec2::new(p.natural_width as f32, p.natural_height as f32);
        // A page drawn before its latest turn: the layout uses the turned
        // size.
        let now = self.rotations.get(&p.page).copied().unwrap_or_default();
        self.natural = Some(if now.swaps() == p.rotation.swaps() {
            natural
        } else {
            Vec2::new(natural.y, natural.x)
        });
        if self.failed_page == Some(p.page) {
            self.failed_page = None;
        }
        // The old texture drops here, which frees it.
        self.shown = Some(Shown {
            page: p.page,
            width: p.width,
            rotation: p.rotation,
            texture,
            natural,
        });
    }

    /// Ask for the current page at `px` pixels wide unless that is what
    /// shows or was asked for (within 3 %).
    fn ask(&mut self, px: u32) {
        let close = |a: u32, b: u32| a.abs_diff(b) <= (b / 33).max(2);
        let rotation = self.rotation();
        if let Some(s) = &self.shown
            && s.page == self.page
            && s.rotation == rotation
            && close(px, s.width)
        {
            return;
        }
        if let Some((page, w, r)) = self.asked
            && page == self.page
            && r == rotation
            && close(px, w)
        {
            return;
        }
        if let Some(worker) = &self.worker {
            worker.send(DocRequest::Render {
                doc: self.doc,
                page: self.page,
                px_width: px,
                rotation,
            });
            self.asked = Some((self.page, px, rotation));
        }
    }
}

/// A small painted toolbar button with an accessible name and a tooltip.
fn icon_button(
    ui: &mut Ui,
    name: &str,
    tip: &str,
    enabled: bool,
    paint: fn(&egui::Painter, Rect, Stroke),
) -> egui::Response {
    let r = ui
        .add_enabled_ui(enabled, |ui| {
            let (rect, r) = ui.allocate_exact_size(Vec2::new(28.0, 24.0), Sense::click());
            let visuals = ui.style().interact(&r);
            ui.painter().rect(
                rect,
                visuals.corner_radius,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            paint(
                ui.painter(),
                rect,
                Stroke::new(1.5, visuals.fg_stroke.color),
            );
            let on = ui.is_enabled();
            r.widget_info(|| WidgetInfo::labeled(Role::Button, on, name));
            r
        })
        .inner;
    if enabled {
        r.on_hover_text(tip)
    } else {
        r.on_disabled_hover_text(tip)
    }
}

fn paint_chevron(p: &egui::Painter, rect: Rect, stroke: Stroke, left: bool) {
    let c = rect.center();
    let dx = if left { 3.0 } else { -3.0 };
    p.line_segment(
        [egui::pos2(c.x + dx, c.y - 5.0), egui::pos2(c.x - dx, c.y)],
        stroke,
    );
    p.line_segment(
        [egui::pos2(c.x - dx, c.y), egui::pos2(c.x + dx, c.y + 5.0)],
        stroke,
    );
}

fn paint_minus(p: &egui::Painter, rect: Rect, stroke: Stroke) {
    let c = rect.center();
    p.line_segment(
        [egui::pos2(c.x - 5.0, c.y), egui::pos2(c.x + 5.0, c.y)],
        stroke,
    );
}

fn paint_plus(p: &egui::Painter, rect: Rect, stroke: Stroke) {
    paint_minus(p, rect, stroke);
    let c = rect.center();
    p.line_segment(
        [egui::pos2(c.x, c.y - 5.0), egui::pos2(c.x, c.y + 5.0)],
        stroke,
    );
}

/// A rotate arrow: three quarters of a circle and an arrow head, clockwise
/// for "Rotate right".
fn paint_rotate(p: &egui::Painter, rect: Rect, stroke: Stroke, clockwise: bool) {
    let c = rect.center();
    let radius = 6.0;
    // From the top, round three quarters; mirrored for anticlockwise.
    let side = if clockwise { 1.0 } else { -1.0 };
    let points: Vec<egui::Pos2> = (0..=18u8)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 * 3.0 * f32::from(i) / 18.0;
            egui::pos2(c.x + side * radius * a.sin(), c.y - radius * a.cos())
        })
        .collect();
    p.add(egui::Shape::line(points, stroke));
    // The head at the end (pointing up, at the left for clockwise).
    let end = egui::pos2(c.x - side * radius, c.y);
    p.line_segment([end, egui::pos2(end.x - 3.0, end.y - 3.0)], stroke);
    p.line_segment([end, egui::pos2(end.x + 3.0, end.y - 3.0)], stroke);
}

/// The zoom choices of the zoom menu (A02 plan D1).
const ZOOM_MENU: [Zoom; 10] = [
    Zoom::FitPage,
    Zoom::FitWidth,
    Zoom::Percent(50),
    Zoom::Percent(75),
    Zoom::Percent(100),
    Zoom::Percent(125),
    Zoom::Percent(150),
    Zoom::Percent(200),
    Zoom::Percent(300),
    Zoom::Percent(400),
];

/// A zoom setting as the owner reads it.
fn zoom_text(zoom: Zoom) -> String {
    match zoom {
        Zoom::FitWidth => t("viewer.zoom.fit_width"),
        Zoom::FitPage => t("viewer.zoom.fit_page"),
        Zoom::Percent(p) => t_with("viewer.zoom.percent", &[("percent", &p.to_string())]),
    }
}

/// The zoom menu: the button shows the zoom in force; its accessible name
/// is "Zoom". A pick puts the keyboard back on the page.
fn zoom_menu(ui: &mut Ui, state: &mut ViewerState, ready: bool) -> egui::Response {
    let mut picked = None;
    let r = ui
        .add_enabled_ui(ready, |ui| {
            egui::containers::menu::MenuButton::from_button(egui::Button::new(zoom_text(
                state.zoom,
            )))
            .ui(ui, |ui| {
                for z in ZOOM_MENU {
                    if ui
                        .add(egui::Button::selectable(state.zoom == z, zoom_text(z)))
                        .clicked()
                    {
                        picked = Some(z);
                    }
                }
            })
            .0
        })
        .inner;
    r.widget_info(|| WidgetInfo::labeled(Role::Button, ready, t("viewer.zoom.label")));
    if let Some(z) = picked {
        state.set_zoom(z);
        state.want_focus = true;
    }
    r.on_hover_text(t("viewer.zoom.tip"))
}

/// The arrow keys pressed this frame as a pan, in points (the content
/// moves the other way: Down shows more of the page below).
fn arrow_pan(ui: &Ui) -> Vec2 {
    const STEP: f32 = 40.0;
    ui.input_mut(|i| {
        let mut d = Vec2::ZERO;
        let none = egui::Modifiers::NONE;
        for (key, v) in [
            (egui::Key::ArrowDown, Vec2::new(0.0, -STEP)),
            (egui::Key::ArrowUp, Vec2::new(0.0, STEP)),
            (egui::Key::ArrowRight, Vec2::new(-STEP, 0.0)),
            (egui::Key::ArrowLeft, Vec2::new(STEP, 0.0)),
        ] {
            while i.consume_key(none, key) {
                d += v;
            }
        }
        d
    })
}

/// The shortcut text of a command, for tooltips.
fn keys(id: &str) -> String {
    cl_model::commands::find(id)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

/// The viewer widget.
pub struct DocViewer;

impl DocViewer {
    /// Draw the viewer for `state`.
    pub fn show(ui: &mut Ui, state: &mut ViewerState, tokens: &Tokens) -> ViewerEvent {
        let ctx = ui.ctx().clone();
        state.poll(&ctx);
        let mut event = ViewerEvent::None;
        let mut focused = false;
        if state.source.is_none() {
            state.focused = false;
            ui.add(
                egui::Label::new(RichText::new(t("viewer.empty")).color(tokens.text_secondary))
                    .wrap(),
            );
            return event;
        }
        let ready = state.status == Status::Ready;
        let mut zoom_focused = false;
        // The toolbar wraps on a narrow card instead of running out.
        ui.horizontal_wrapped(|ui| {
            let mut run = |state: &mut ViewerState, r: &egui::Response, id: &'static str| {
                focused |= r.has_focus();
                if r.clicked() {
                    state.command(CommandId::new(id));
                    // The keyboard goes back to the page, so the page and
                    // zoom keys keep working after a click.
                    state.want_focus = true;
                }
            };
            let r = icon_button(
                ui,
                &t("viewer.prev"),
                &t_with("viewer.prev.tip", &[("keys", &keys("viewer.page_prev"))]),
                ready && state.page > 0,
                |p, r, s| paint_chevron(p, r, s, true),
            );
            run(state, &r, "viewer.page_prev");
            let pages = state.pages.max(1);
            ui.label(
                RichText::new(t_with(
                    "viewer.page_of",
                    &[
                        ("page", &(state.page.min(pages - 1) + 1).to_string()),
                        ("pages", &pages.to_string()),
                    ],
                ))
                .color(tokens.text_secondary),
            );
            let r = icon_button(
                ui,
                &t("viewer.next"),
                &t_with("viewer.next.tip", &[("keys", &keys("viewer.page_next"))]),
                ready && state.page + 1 < state.pages,
                |p, r, s| paint_chevron(p, r, s, false),
            );
            run(state, &r, "viewer.page_next");
            ui.separator();
            let r = icon_button(
                ui,
                &t("viewer.zoom_out"),
                &t_with("viewer.zoom_out.tip", &[("keys", &keys("viewer.zoom_out"))]),
                ready,
                paint_minus,
            );
            run(state, &r, "viewer.zoom_out");
            // A02: the zoom shown is a menu of fits and sizes.
            zoom_focused = zoom_menu(ui, state, ready).has_focus();
            let r = icon_button(
                ui,
                &t("viewer.zoom_in"),
                &t_with("viewer.zoom_in.tip", &[("keys", &keys("viewer.zoom_in"))]),
                ready,
                paint_plus,
            );
            run(state, &r, "viewer.zoom_in");
            let r = ui
                .add_enabled(ready, egui::Button::new(t("viewer.fit")))
                .on_hover_text(t_with("viewer.fit.tip", &[("keys", &keys("viewer.fit"))]));
            run(state, &r, "viewer.fit");
            ui.separator();
            // A02: view only; buttons, no shortcut (Q-A02m).
            let r = icon_button(
                ui,
                &t("viewer.rotate_left"),
                &t("viewer.rotate_left.tip"),
                ready,
                |p, r, s| paint_rotate(p, r, s, false),
            );
            run(state, &r, "viewer.rotate_ccw");
            let r = icon_button(
                ui,
                &t("viewer.rotate_right"),
                &t("viewer.rotate_right.tip"),
                ready,
                |p, r, s| paint_rotate(p, r, s, true),
            );
            run(state, &r, "viewer.rotate_cw");
            if state.full_size_button {
                let r = ui
                    .button(t("viewer.full_size"))
                    .on_hover_text(t("viewer.full_size.tip"));
                focused |= r.has_focus();
                if r.clicked() {
                    event = ViewerEvent::OpenFullSize;
                }
            }
            if state.pop_out_button {
                let k = keys("view.toggle_doc_viewer");
                let tip = if k.is_empty() {
                    t("viewer.pop_out.tip_plain")
                } else {
                    t_with("viewer.pop_out.tip", &[("keys", &k)])
                };
                let r = ui.button(t("viewer.pop_out")).on_hover_text(tip);
                focused |= r.has_focus();
                if r.clicked() {
                    event = ViewerEvent::PopOut;
                }
            }
        });
        focused |= zoom_focused;
        if state.truncated {
            ui.label(
                RichText::new(t_with(
                    "viewer.truncated",
                    &[("n", &super::doc_worker::MAX_PAGES.to_string())],
                ))
                .size(12.0)
                .color(tokens.text_tertiary),
            );
        }
        if state.status == Status::Ready && state.failed_page == Some(state.page) {
            ui.add(
                egui::Label::new(
                    RichText::new(t("viewer.page_unreadable")).color(tokens.text_secondary),
                )
                .wrap(),
            );
            // Plan R4: an unreadable page offers Save a copy.
            let r = ui
                .button(t("viewer.save_copy"))
                .on_hover_text(t("viewer.save_copy.tip"));
            focused |= r.has_focus();
            if r.clicked() {
                event = ViewerEvent::SaveCopy;
            }
        }
        state.frame += 1;
        state.strip_shown = state.status == Status::Ready
            && state.pages > 1
            && ui.available_width() >= THUMB_STRIP_MIN_WIDTH;
        match state.status {
            Status::Ready if state.strip_shown => {
                ui.horizontal_top(|ui| {
                    let h = ui.available_height();
                    // A column, whatever the surrounding layout.
                    let column = egui::Layout::top_down(egui::Align::Min);
                    ui.allocate_ui_with_layout(Vec2::new(STRIP_W, h), column, |ui| {
                        ui.set_min_size(Vec2::new(STRIP_W, h));
                        focused |= Self::thumb_strip(ui, state, tokens);
                    });
                    ui.vertical(|ui| focused |= Self::page_area(ui, state, tokens));
                });
            }
            Status::Ready => focused |= Self::page_area(ui, state, tokens),
            Status::Loading | Status::Empty => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(t("viewer.loading")).color(tokens.text_secondary));
                });
            }
            Status::Unavailable | Status::Protected | Status::Unreadable => {
                let (text, hint) = match state.status {
                    Status::Unavailable => ("viewer.unavailable", Some("viewer.unavailable.hint")),
                    Status::Protected => ("viewer.protected", Some("viewer.protected.hint")),
                    _ => ("viewer.unreadable", None),
                };
                ui.add(egui::Label::new(RichText::new(t(text)).color(tokens.text_primary)).wrap());
                if let Some(h) = hint {
                    ui.add(
                        egui::Label::new(
                            RichText::new(t(h)).size(12.0).color(tokens.text_secondary),
                        )
                        .wrap(),
                    );
                }
                let r = ui
                    .button(t("viewer.save_copy"))
                    .on_hover_text(t("viewer.save_copy.tip"));
                focused |= r.has_focus();
                if r.clicked() {
                    event = ViewerEvent::SaveCopy;
                }
            }
        }
        // Replies arrive on another thread: look again soon while waiting
        // (not for a page the worker said it can't draw).
        let failed = state.failed_page == Some(state.page);
        if state.status == Status::Loading
            || (state.status == Status::Ready && !failed && !state.page_drawn())
            || (!failed
                && state.asked.is_some_and(|(_, w, r)| {
                    state
                        .shown
                        .as_ref()
                        .is_none_or(|s| s.width != w || s.rotation != r)
                }))
        {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        state.focused = focused;
        event
    }

    /// A02: the thumbnail strip, one "Page n" button a page. Only the rows in
    /// view are drawn and asked for. Returns true when one has the keyboard.
    fn thumb_strip(ui: &mut Ui, state: &mut ViewerState, tokens: &Tokens) -> bool {
        let mut focused = false;
        let mut asks = 0;
        let mut waiting = false;
        let mut clicked = None;
        let row_h = THUMB_ROW_H + ui.spacing().item_spacing.y;
        let mut area = egui::ScrollArea::vertical()
            .id_salt(("doc-thumbs", state.doc))
            .auto_shrink([false, false]);
        // Follow the page turned by key or button.
        if state.strip_page != Some(state.page) {
            state.strip_page = Some(state.page);
            let top = state.strip_top;
            #[allow(clippy::cast_precision_loss)] // At most 500 pages.
            let y = state.page as f32 * row_h;
            let view_h = ui.available_height();
            if y < top || y + row_h > top + view_h {
                area = area.vertical_scroll_offset((y - (view_h - row_h) / 2.0).max(0.0));
            }
        }
        let out = area.show_rows(ui, THUMB_ROW_H, state.pages as usize, |ui, rows| {
            for row in rows {
                let Ok(page) = u32::try_from(row) else { break };
                let (rect, r) =
                    ui.allocate_exact_size(Vec2::new(STRIP_W - 16.0, THUMB_ROW_H), Sense::click());
                let label = t_with("viewer.thumb", &[("n", &(page + 1).to_string())]);
                r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &label));
                focused |= r.has_focus();
                if r.clicked() {
                    clicked = Some(page);
                }
                let current = page == state.page;
                let painter = ui.painter_at(rect);
                if current || r.hovered() {
                    painter.rect_filled(rect, 4.0, tokens.surface_app);
                }
                // The picture's box above the page number.
                let pic = Rect::from_min_size(
                    rect.min + Vec2::new(4.0, 4.0),
                    Vec2::new(rect.width() - 8.0, THUMB_ROW_H - 26.0),
                );
                if let Some(t) = state.thumbs.get_mut(&page) {
                    t.used = state.frame;
                    let scale = (pic.width() / t.size.x).min(pic.height() / t.size.y);
                    let shown = Rect::from_center_size(pic.center(), t.size * scale);
                    painter.rect_filled(shown, 0.0, Color32::WHITE);
                    painter.image(
                        t.texture.id(),
                        shown,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    let stroke = if current {
                        Stroke::new(2.0, tokens.accent)
                    } else {
                        Stroke::new(1.0, tokens.border_strong)
                    };
                    painter.rect_stroke(shown, 0.0, stroke, egui::StrokeKind::Outside);
                } else {
                    let blank = Rect::from_center_size(
                        pic.center(),
                        Vec2::new(pic.width() * 0.75, pic.height()),
                    );
                    painter.rect_filled(blank, 0.0, Color32::WHITE);
                    painter.rect_stroke(
                        blank,
                        0.0,
                        Stroke::new(1.0, tokens.border_strong),
                        egui::StrokeKind::Outside,
                    );
                    // A page whose thumbnail can't be drawn stays blank;
                    // the page itself says why.
                    if !state.thumb_failed.contains(&page) {
                        if !state.thumb_asked.contains(&page) && asks < THUMB_ASKS_PER_FRAME {
                            if let Some(w) = &state.worker {
                                w.send(DocRequest::Thumb {
                                    doc: state.doc,
                                    page,
                                    px_width: THUMB_PX,
                                });
                                state.thumb_asked.insert(page);
                            }
                            asks += 1;
                        }
                        waiting = true;
                    }
                }
                painter.text(
                    egui::pos2(rect.center().x, rect.max.y - 11.0),
                    egui::Align2::CENTER_CENTER,
                    (page + 1).to_string(),
                    egui::FontId::proportional(12.0),
                    if current {
                        tokens.text_primary
                    } else {
                        tokens.text_secondary
                    },
                );
                if r.has_focus() {
                    painter.rect_stroke(
                        rect.shrink(1.0),
                        4.0,
                        Stroke::new(2.0, tokens.accent),
                        egui::StrokeKind::Inside,
                    );
                }
            }
        });
        state.strip_top = out.state.clamped_offset().y;
        if let Some(page) = clicked {
            state.page = page;
            state.strip_page = Some(page);
            state.want_focus = true;
        }
        if waiting {
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        focused
    }

    /// The page, scrolled when it is larger than the room. Returns true when
    /// it has the keyboard.
    fn page_area(ui: &mut Ui, state: &mut ViewerState, tokens: &Tokens) -> bool {
        // The page sits on the app's surface, a margin all round, so a
        // white page reads as a page in both themes.
        const MARGIN: f32 = 10.0;
        let outer = ui.available_rect_before_wrap();
        ui.painter().rect_filled(outer, 4.0, tokens.surface_app);
        let avail = outer.size();
        // Leave room for the margins and the scroll bars so Fit never
        // needs one.
        let area = Vec2::new(
            (avail.x - 2.0 * MARGIN - 14.0).max(40.0),
            (avail.y - 2.0 * MARGIN - 14.0).max(40.0),
        );
        state.last_area = area;
        let natural = state.natural.unwrap_or(Vec2::new(612.0, 792.0));
        let shown_w = display_width(state.zoom, area, natural);
        let ppp = ui.ctx().pixels_per_point();
        // Physical pixels; the worker clamps the size.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let px = (shown_w * ppp).round().clamp(1.0, 65_535.0) as u32;
        state.ask(px);
        let name = state
            .source
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let label = t_with("viewer.image", &[("name", &name)]);
        let mut focused = false;
        let want_focus = std::mem::take(&mut state.want_focus);
        // A page the worker can't draw shows blank, not the last page.
        let failed = state.failed_page == Some(state.page);
        egui::ScrollArea::both()
            .id_salt(("doc-viewer", state.doc))
            .auto_shrink([false, false])
            // A02: dragging the page pans it, with a mouse too.
            .scroll_source(egui::scroll_area::ScrollSource::ALL)
            .show(ui, |ui| {
                let (size, texture) = match &state.shown {
                    Some(s) if !failed => {
                        let aspect = s.natural.y / s.natural.x.max(1.0);
                        (Vec2::new(shown_w, shown_w * aspect), Some(s.texture.id()))
                    }
                    _ => (
                        Vec2::new(shown_w, shown_w * natural.y / natural.x.max(1.0)),
                        None,
                    ),
                };
                // Centre a page narrower than the room.
                let pad = MARGIN + ((area.x - size.x) / 2.0).max(0.0);
                ui.add_space(MARGIN);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
                    if want_focus || r.clicked() {
                        r.request_focus();
                    }
                    if r.has_focus() {
                        // A02: the arrow keys pan the page (they don't move
                        // the keyboard to another control).
                        ui.memory_mut(|m| {
                            m.set_focus_lock_filter(
                                r.id,
                                egui::EventFilter {
                                    horizontal_arrows: true,
                                    vertical_arrows: true,
                                    ..Default::default()
                                },
                            );
                        });
                        let pan = arrow_pan(ui);
                        if pan != Vec2::ZERO {
                            ui.scroll_with_delta(pan);
                        }
                    }
                    if r.hovered() && (size.x > area.x + 1.0 || size.y > area.y + 1.0) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, 0.0, Color32::WHITE);
                    if let Some(id) = texture {
                        painter.image(
                            id,
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    let border = if r.has_focus() {
                        Stroke::new(2.0, tokens.accent)
                    } else {
                        Stroke::new(1.0, tokens.border_strong)
                    };
                    ui.painter()
                        .rect_stroke(rect, 0.0, border, egui::StrokeKind::Inside);
                    r.widget_info(|| WidgetInfo::labeled(Role::Image, true, &label));
                    focused = r.has_focus();
                });
                ui.add_space(MARGIN);
            });
        focused
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Pos2, RawInput, vec2};

    fn fixture(name: &str) -> DocSource {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/docs")
            .join(name);
        DocSource {
            name: name.to_string(),
            bytes: Arc::from(std::fs::read(p).expect("fixture")),
        }
    }

    #[test]
    fn open_and_clear_keep_the_button_choice() {
        let mut v = ViewerState::default();
        v.set_full_size_button(true);
        v.open(DocSource {
            name: "invoice.pdf".into(),
            bytes: Arc::from(&b"%PDF-1.7"[..]),
        });
        assert!(v.source().is_some() && v.full_size_button);
        assert!(!format!("{:?}", v.source()).contains("invoice"));
        assert!(!format!("{v:?}").contains("invoice"));
        v.clear();
        assert!(v.source().is_none() && v.full_size_button);
        assert_eq!(v.state_json()["open"], false);
        assert_eq!(v.state_json()["status"], "empty");
    }

    #[test]
    fn open_keep_zoom_keeps_the_zoom_and_release_pixels_drops_the_page() {
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        assert!(v.command(CommandId::new("viewer.zoom_in")));
        let zoom = v.zoom();
        v.open_keep_zoom(fixture("receipt.jpg"));
        assert_eq!(v.zoom(), zoom);
        assert_eq!(v.status(), Status::Loading);
        v.open(fixture("receipt.png"));
        assert_eq!(v.zoom(), Zoom::default());
        let _ = draw_until(&mut v, false, |v| v.state_json()["rendered"] == true);
        v.release_pixels();
        assert!(v.shown.is_none() && v.asked.is_none());
        assert_eq!(v.state_json()["rendered"], false);
        assert!(v.source().is_some(), "the document stays open");
        // Drawn again, the page comes back.
        let _ = draw_until(&mut v, false, |v| v.state_json()["rendered"] == true);
    }

    #[test]
    fn zoom_steps_go_up_and_down_within_bounds() {
        assert_eq!(zoom_step_in(100.0), 110);
        assert_eq!(zoom_step_in(99.8), 110);
        assert_eq!(zoom_step_in(104.0), 110);
        assert_eq!(zoom_step_in(400.0), 400);
        assert_eq!(zoom_step_in(10.0), 25);
        assert_eq!(zoom_step_out(100.0), 90);
        assert_eq!(zoom_step_out(25.0), 25);
        assert_eq!(zoom_step_out(1000.0), 400);
        assert_eq!(zoom_step_out(131.0), 125);
    }

    #[test]
    fn fit_and_percent_widths() {
        let letter = vec2(612.0, 792.0);
        let area = vec2(500.0, 400.0);
        assert!((display_width(Zoom::FitWidth, area, letter) - 500.0).abs() < 0.01);
        // The whole page in 400 high: 400 × 612/792.
        assert!((display_width(Zoom::FitPage, area, letter) - 309.09).abs() < 0.01);
        // A wide area: Fit page is limited by the height, a narrow one by the width.
        assert!((display_width(Zoom::FitPage, vec2(100.0, 4000.0), letter) - 100.0).abs() < 0.01);
        assert!((display_width(Zoom::Percent(200), area, letter) - 1224.0).abs() < 0.01);
        assert!((display_width(Zoom::FitPage, area, Vec2::ZERO) - 500.0).abs() < 0.01);
    }

    #[test]
    fn commands_need_a_document_and_stay_in_range() {
        let mut v = ViewerState::default();
        assert!(!v.command(CommandId::new("viewer.zoom_in")));
        v.open(fixture("receipt.png"));
        v.pages = 2;
        assert!(v.command(CommandId::new("viewer.page_next")));
        assert!(v.command(CommandId::new("viewer.page_next")));
        assert_eq!(v.page(), 1);
        assert!(v.command(CommandId::new("viewer.page_prev")));
        assert!(v.command(CommandId::new("viewer.page_prev")));
        assert_eq!(v.page(), 0);
        assert!(v.command(CommandId::new("viewer.zoom_in")));
        assert_eq!(v.zoom(), Zoom::Percent(110));
        assert!(v.command(CommandId::new("viewer.zoom_out")));
        assert_eq!(v.zoom(), Zoom::Percent(100));
        assert!(v.command(CommandId::new("viewer.fit")));
        assert_eq!(v.zoom(), Zoom::FitWidth);
        assert!(v.command(CommandId::new("viewer.fit")));
        assert_eq!(v.zoom(), Zoom::FitPage);
        assert!(!v.command(CommandId::new("form.save_new")));
    }

    /// A page drawn larger than the graphics card's texture limit is scaled
    /// down to fit, not refused by egui with a panic; the asked width is
    /// kept so it is not asked for again.
    #[test]
    fn a_page_over_the_texture_limit_is_scaled_to_fit() {
        let ctx = egui::Context::default();
        let max = ctx.input(|i| i.max_texture_side);
        let (w, h) = (u32::try_from(max).expect("side") + 100, 10);
        let mut v = ViewerState::default();
        v.take_page(
            &ctx,
            &RenderedPage {
                doc: v.doc,
                page: 0,
                width: w,
                height: h,
                rgba: vec![255; (w * h * 4) as usize],
                natural_width: 612,
                natural_height: 792,
                rotation: Rotation::None,
            },
        );
        let shown = v.shown.as_ref().expect("shown");
        assert_eq!(shown.width, w, "the asked width");
        let [tw, th] = shown.texture.size();
        assert_eq!(tw, max);
        assert!((1..=10).contains(&th), "{th}");
    }

    /// Draw `v` until `done` (bounded), returning the accessible (role,
    /// name) pairs of the last frames.
    fn draw_until(
        v: &mut ViewerState,
        full_size: bool,
        done: impl Fn(&ViewerState) -> bool,
    ) -> Vec<(egui::accesskit::Role, String)> {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        v.set_full_size_button(full_size);
        let tokens = Tokens::LIGHT;
        let mut all = std::collections::HashMap::new();
        let end = std::time::Instant::now() + Duration::from_secs(30);
        let mut extra = 3;
        loop {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(640.0, 520.0))),
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let _ = DocViewer::show(ui, v, &tokens);
                });
            });
            out.textures_delta.clear();
            if let Some(t) = out.platform_output.accesskit_update {
                all.extend(t.nodes);
            }
            if done(v) {
                extra -= 1;
                if extra == 0 {
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < end,
                "viewer never got there: {v:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        all.into_values()
            .map(|n| {
                let name = n.label().or(n.value()).unwrap_or_default().to_string();
                (n.role(), name)
            })
            .collect()
    }

    #[test]
    fn an_image_previews_with_its_labels() {
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        v.set_pop_out_button(true);
        let nodes = draw_until(&mut v, true, |v| v.state_json()["rendered"] == true);
        let has = |role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
        assert!(has(Role::Button, "Pop out"), "{nodes:?}");
        assert!(has(Role::Image, "Preview of receipt.png"), "{nodes:?}");
        for b in [
            "Previous page",
            "Next page",
            "Zoom in",
            "Zoom out",
            "Fit",
            "Open full size",
        ] {
            assert!(has(Role::Button, b), "{b}");
        }
        assert!(nodes.iter().any(|(_, n)| n == "Page 1 of 1"));
        let s = v.state_json();
        assert_eq!(
            (s["status"].as_str(), s["pages"].as_u64()),
            (Some("ready"), Some(1))
        );
        // The state never names the file.
        assert!(!s.to_string().contains("receipt"));
    }

    #[test]
    fn a_pdf_without_pdfium_offers_save_a_copy() {
        let mut v = ViewerState {
            worker: Some(DocWorker::start_without_library()),
            ..ViewerState::default()
        };
        v.open(fixture("two_pages.pdf"));
        let nodes = draw_until(&mut v, false, |v| v.status() == Status::Unavailable);
        assert!(
            nodes
                .iter()
                .any(|(r, n)| *r == Role::Button && n == "Save a copy")
        );
        assert!(
            nodes
                .iter()
                .any(|(_, n)| n == "Preview isn't available on this computer")
        );
    }

    #[test]
    fn a_pdf_turns_pages_and_zooms_with_pdfium() {
        if super::super::doc_worker::library_path().is_none() {
            assert!(
                std::env::var("CL_PDFIUM_REQUIRE").as_deref() != Ok("1"),
                "PDFium is required (CL_PDFIUM_REQUIRE=1): run `cargo xtask pdfium-fetch`"
            );
            return;
        }
        let mut v = ViewerState::default();
        v.open(fixture("two_pages.pdf"));
        let _ = draw_until(&mut v, false, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.page_count(), 2);
        assert!(v.command(CommandId::new("viewer.page_next")));
        assert!(v.command(CommandId::new("viewer.zoom_in")));
        let nodes = draw_until(&mut v, false, |v| {
            v.state_json()["rendered"] == true && v.shown.as_ref().is_some_and(|s| s.page == 1)
        });
        assert!(nodes.iter().any(|(_, n)| n == "Page 2 of 2"), "{nodes:?}");
        // Fit width beside the thumbnail strip (A02) is about 85 %: the next
        // step up.
        assert_eq!(v.zoom(), Zoom::Percent(90));
        assert_eq!(v.state_json()["thumbnails"]["shown"], true);
        assert_eq!(v.state_json()["status"], "ready");
    }

    #[test]
    fn an_unreadable_file_says_so() {
        let mut v = ViewerState::default();
        v.open(fixture("bomb_30000.png"));
        let nodes = draw_until(&mut v, false, |v| v.status() == Status::Unreadable);
        assert!(
            nodes
                .iter()
                .any(|(r, n)| *r == Role::Button && n == "Save a copy")
        );
    }

    /// One egui context driven frame by frame, with clicks found by
    /// accessible name (A02 widget tests).
    struct Harness {
        ctx: egui::Context,
        /// The last frame's accessible nodes: (role, name, centre).
        nodes: Vec<(Role, String, Pos2)>,
        size: Vec2,
    }

    impl Harness {
        fn new(size: Vec2) -> Harness {
            let ctx = egui::Context::default();
            crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
            crate::fonts::install(&ctx);
            ctx.enable_accesskit();
            Harness {
                ctx,
                nodes: Vec::new(),
                size,
            }
        }

        fn frame(&mut self, v: &mut ViewerState, events: Vec<egui::Event>) -> ViewerEvent {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
                events,
                ..RawInput::default()
            };
            let mut event = ViewerEvent::None;
            let tokens = Tokens::LIGHT;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    event = DocViewer::show(ui, v, &tokens);
                });
            });
            out.textures_delta.clear();
            if let Some(t) = out.platform_output.accesskit_update {
                self.nodes = t
                    .nodes
                    .into_iter()
                    .filter_map(|(_, n)| {
                        let b = n.bounds()?;
                        let name = n.label().or(n.value()).unwrap_or_default().to_string();
                        #[allow(clippy::cast_possible_truncation)]
                        let centre = Pos2::new(
                            f64::midpoint(b.x0, b.x1) as f32,
                            f64::midpoint(b.y0, b.y1) as f32,
                        );
                        Some((n.role(), name, centre))
                    })
                    .collect();
            }
            event
        }

        /// Draw until `done` (bounded), then a few frames more.
        fn until(&mut self, v: &mut ViewerState, done: impl Fn(&ViewerState) -> bool) {
            let end = std::time::Instant::now() + Duration::from_secs(30);
            let mut extra = 3;
            loop {
                let _ = self.frame(v, Vec::new());
                if done(v) {
                    extra -= 1;
                    if extra == 0 {
                        return;
                    }
                }
                assert!(std::time::Instant::now() < end, "never got there: {v:?}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        fn has(&self, role: Role, name: &str) -> bool {
            self.nodes.iter().any(|(r, n, _)| *r == role && n == name)
        }

        /// Click the control named `name`; the viewer's event of the click.
        fn click(&mut self, v: &mut ViewerState, role: Role, name: &str) -> ViewerEvent {
            let at = self
                .nodes
                .iter()
                .find(|(r, n, _)| *r == role && n == name)
                .map_or_else(
                    || panic!("no {role:?} {name}: {:?}", self.nodes),
                    |(.., p)| *p,
                );
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            let _ = self.frame(v, vec![egui::Event::PointerMoved(at)]);
            let _ = self.frame(v, vec![button(true)]);
            let event = self.frame(v, vec![button(false)]);
            let _ = self.frame(v, Vec::new());
            event
        }
    }

    #[test]
    fn rotate_turns_the_page_for_viewing_only() {
        let mut h = Harness::new(vec2(700.0, 560.0));
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert!(h.has(Role::Button, "Rotate right") && h.has(Role::Button, "Rotate left"));
        let before = v.shown.as_ref().map(|s| s.texture.size()).expect("shown");
        assert!(before[0] > before[1], "a landscape receipt");
        let _ = h.click(&mut v, Role::Button, "Rotate right");
        assert_eq!(v.rotation(), Rotation::Right);
        assert_eq!(v.state_json()["rotation"], 90);
        // Turned a quarter: the size at 100 % swaps at once.
        assert_eq!(v.natural, Some(vec2(200.0, 320.0)));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        let [tw, th] = v.shown.as_ref().map(|s| s.texture.size()).expect("shown");
        assert!(th > tw, "drawn upright now: {tw} x {th}");
        // The keyboard is back on the page after a click.
        assert_eq!(v.state_json()["focused"], true);
        let _ = h.click(&mut v, Role::Button, "Rotate left");
        let _ = h.click(&mut v, Role::Button, "Rotate left");
        assert_eq!(v.state_json()["rotation"], 270);
        assert!(v.command(CommandId::new("viewer.rotate_cw")));
        assert_eq!(v.rotation(), Rotation::None);
        assert!(v.command(CommandId::new("viewer.rotate_ccw")));
        // A new document starts upright (rotation is never kept).
        v.open(fixture("receipt.jpg"));
        assert_eq!(v.rotation(), Rotation::None);
        assert!(!v.state_json().to_string().contains("receipt"));
    }

    #[test]
    fn rotation_is_per_page() {
        let mut v = ViewerState::default();
        v.open(fixture("three_pages.tif"));
        v.pages = 3;
        assert!(v.command(CommandId::new("viewer.rotate_cw")));
        assert!(v.command(CommandId::new("viewer.page_next")));
        assert_eq!(v.rotation(), Rotation::None);
        assert!(v.command(CommandId::new("viewer.page_prev")));
        assert_eq!(v.rotation(), Rotation::Right);
    }

    #[test]
    fn the_zoom_menu_sets_each_fit_and_size() {
        let mut h = Harness::new(vec2(760.0, 560.0));
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert!(h.has(Role::Button, "Zoom"), "{:?}", h.nodes);
        for (choice, want) in [
            ("Fit page", json!("fit_page")),
            ("50%", json!(50)),
            ("75%", json!(75)),
            ("100%", json!(100)),
            ("125%", json!(125)),
            ("150%", json!(150)),
            ("200%", json!(200)),
            ("300%", json!(300)),
            ("400%", json!(400)),
            ("Fit width", json!("fit_width")),
        ] {
            let _ = h.click(&mut v, Role::Button, "Zoom");
            let _ = h.click(&mut v, Role::Button, choice);
            assert_eq!(v.state_json()["zoom"], want, "{choice}");
            assert!(
                !h.has(Role::Button, choice) || choice.starts_with("Fit"),
                "menu closed"
            );
        }
        v.set_zoom(Zoom::Percent(1000));
        assert_eq!(v.zoom(), Zoom::Percent(400));
    }

    #[test]
    fn arrow_keys_pan_a_zoomed_page() {
        let mut h = Harness::new(vec2(500.0, 400.0));
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        v.set_zoom(Zoom::Percent(400));
        v.focus();
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.state_json()["focused"], true);
        let image_at = |h: &Harness| {
            h.nodes
                .iter()
                .find(|(r, ..)| *r == Role::Image)
                .map(|(.., p)| *p)
                .expect("the page")
        };
        let start = image_at(&h);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = h.frame(
            &mut v,
            vec![key(egui::Key::ArrowDown), key(egui::Key::ArrowRight)],
        );
        for _ in 0..3 {
            let _ = h.frame(&mut v, Vec::new());
        }
        let moved = image_at(&h);
        assert!(
            moved.y < start.y && moved.x < start.x,
            "{start:?} -> {moved:?}"
        );
        // The keyboard stayed on the page.
        assert_eq!(v.state_json()["focused"], true);
    }

    #[test]
    fn thumbnails_show_each_page_and_a_click_goes_there() {
        let mut h = Harness::new(vec2(760.0, 600.0));
        let mut v = ViewerState::default();
        v.set_pop_out_button(true);
        v.open(fixture("three_pages.tif"));
        h.until(&mut v, |v| {
            v.state_json()["rendered"] == true && v.state_json()["thumbnails"]["loaded"] == 3
        });
        let s = v.state_json();
        assert_eq!(s["thumbnails"]["shown"], true);
        assert_eq!(s["pages"], 3);
        for name in [
            "Page 1",
            "Page 2",
            "Page 3",
            "Rotate right",
            "Rotate left",
            "Pop out",
        ] {
            assert!(h.has(Role::Button, name), "{name}: {:?}", h.nodes);
        }
        // A column down the left of the page.
        let at = |name: &str| {
            h.nodes
                .iter()
                .find(|(r, n, _)| *r == Role::Button && n == name)
                .map(|(.., p)| *p)
                .expect("thumbnail")
        };
        let (one, two) = (at("Page 1"), at("Page 2"));
        assert!(
            (one.x - two.x).abs() < 1.0 && two.y > one.y,
            "{one:?} {two:?}"
        );
        let page = h
            .nodes
            .iter()
            .find(|(r, ..)| *r == Role::Image)
            .map(|(.., p)| *p)
            .expect("the page");
        assert!(
            page.x > one.x + STRIP_W / 2.0,
            "the page is right of the strip"
        );
        assert_eq!(h.click(&mut v, Role::Button, "Page 2"), ViewerEvent::None);
        assert_eq!(v.page(), 1);
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert!(h.nodes.iter().any(|(_, n, _)| n == "Page 2 of 3"));
        // The keyboard went to the page, so the page keys work.
        assert_eq!(v.state_json()["focused"], true);
        let _ = h.click(&mut v, Role::Button, "Page 3");
        assert_eq!(v.page(), 2);
        // Rotating a page leaves the thumbnails upright and loaded.
        let _ = h.click(&mut v, Role::Button, "Rotate right");
        assert_eq!(v.state_json()["thumbnails"]["loaded"], 3);
        // No names in the state.
        assert!(!v.state_json().to_string().contains("three_pages"));
        // Released pixels drop the thumbnails too.
        v.release_pixels();
        assert_eq!(v.state_json()["thumbnails"]["loaded"], 0);
    }

    #[test]
    fn no_strip_when_narrow_or_one_page() {
        let mut h = Harness::new(vec2(480.0, 600.0));
        let mut v = ViewerState::default();
        v.open(fixture("three_pages.tif"));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.state_json()["thumbnails"]["shown"], false);
        assert!(!h.has(Role::Button, "Page 2"));
        let mut h = Harness::new(vec2(760.0, 600.0));
        let mut v = ViewerState::default();
        v.open(fixture("receipt.png"));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.state_json()["thumbnails"]["shown"], false);
        assert!(!h.has(Role::Button, "Page 1"));
    }

    #[test]
    fn at_most_64_thumbnails_are_kept() {
        let ctx = egui::Context::default();
        let mut v = ViewerState::default();
        for page in 0..70u32 {
            v.frame = u64::from(page);
            v.take_thumb(
                &ctx,
                &RenderedPage {
                    doc: v.doc,
                    page,
                    width: 2,
                    height: 3,
                    rgba: vec![255; 2 * 3 * 4],
                    natural_width: 2,
                    natural_height: 3,
                    rotation: Rotation::None,
                },
            );
        }
        assert_eq!(v.thumbs.len(), MAX_THUMBS);
        // The least recently drawn went first.
        assert!(!v.thumbs.contains_key(&0) && !v.thumbs.contains_key(&5));
        assert!(v.thumbs.contains_key(&6) && v.thumbs.contains_key(&69));
    }

    #[test]
    fn an_unreadable_page_says_so_and_the_others_show() {
        let mut h = Harness::new(vec2(640.0, 520.0));
        let mut v = ViewerState::default();
        v.open(DocSource {
            name: "mixed.tif".into(),
            bytes: Arc::from(super::super::doc_worker::tiff_fixtures::gray_then_lab()),
        });
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.page_count(), 2);
        assert!(v.command(CommandId::new("viewer.page_next")));
        h.until(&mut v, |v| v.state_json()["page_unreadable"] == true);
        let s = v.state_json();
        assert_eq!(
            (s["status"].as_str(), s["rendered"].as_bool()),
            (Some("ready"), Some(false))
        );
        assert!(h.has(Role::Button, "Save a copy"));
        assert_eq!(
            h.click(&mut v, Role::Button, "Save a copy"),
            ViewerEvent::SaveCopy
        );
        // Back to page 1: it shows again.
        assert!(v.command(CommandId::new("viewer.page_prev")));
        h.until(&mut v, |v| v.state_json()["rendered"] == true);
        assert_eq!(v.state_json()["page_unreadable"], false);
    }
}
