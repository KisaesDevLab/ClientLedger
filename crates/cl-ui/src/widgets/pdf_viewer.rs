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
//! The pages come from [`super::doc_worker`] (a background thread): the
//! viewer asks for the page at the pixel width it shows, keeps one texture
//! (freed when the page, the document or the state goes) and polls replies
//! each frame, so drawing never waits. Zoom is UI geometry (`f32`), not
//! money. Without PDFium a PDF shows "Preview isn't available on this
//! computer" with Save a copy; pictures still preview.

use std::sync::Arc;
use std::time::Duration;

use cl_model::CommandId;
use eframe::egui::{
    self, Color32, ColorImage, Rect, RichText, Role, Sense, Stroke, TextureHandle, TextureOptions,
    Ui, Vec2, WidgetInfo,
};
use serde_json::{Value, json};

use super::doc_worker::{DocReply, DocRequest, DocWorker, RenderedPage};
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
    texture: TextureHandle,
    natural: Vec2,
}

/// One viewer's state.
#[derive(Default)]
pub struct ViewerState {
    source: Option<DocSource>,
    page: u32,
    pages: u32,
    zoom: Zoom,
    focused: bool,
    full_size_button: bool,
    status: Status,
    truncated: bool,
    worker: Option<DocWorker>,
    /// The worker's number for the open document (new on every open).
    doc: u64,
    /// The last (page, px width) asked for.
    asked: Option<(u32, u32)>,
    shown: Option<Shown>,
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

    /// Show nothing.
    pub fn clear(&mut self) {
        self.reset();
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
            "rendered": self.shown.as_ref().is_some_and(|s| s.page == self.page),
            "focused": self.focused,
        })
    }

    /// Take the worker's replies for the open document.
    fn poll(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.worker else {
            return;
        };
        let mut rendered = None;
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
                    // A failed render keeps a page that is already showing.
                    if self.shown.is_none() {
                        self.status = Status::Unreadable;
                    }
                }
                DocReply::Unavailable { doc } if doc == self.doc => {
                    self.status = Status::Unavailable;
                }
                DocReply::Rendered(p) if p.doc == self.doc => rendered = Some(p),
                // An earlier document's answer.
                _ => {}
            }
        }
        if let Some(p) = rendered {
            self.take_page(ctx, &p);
        }
    }

    fn take_page(&mut self, ctx: &egui::Context, p: &RenderedPage) {
        let (Ok(w), Ok(h)) = (usize::try_from(p.width), usize::try_from(p.height)) else {
            return;
        };
        if p.rgba.len() != w * h * 4 {
            return;
        }
        let image = ColorImage::from_rgba_unmultiplied([w, h], &p.rgba);
        // The texture name carries numbers only, never the file name.
        let texture = ctx.load_texture(
            format!("doc-viewer-{}-{}", self.doc, p.page),
            image,
            TextureOptions::LINEAR,
        );
        #[allow(clippy::cast_precision_loss)] // Page sizes are small.
        let natural = Vec2::new(p.natural_width as f32, p.natural_height as f32);
        self.natural = Some(natural);
        // The old texture drops here, which frees it.
        self.shown = Some(Shown {
            page: p.page,
            width: p.width,
            texture,
            natural,
        });
    }

    /// Ask for the current page at `px` pixels wide unless that is what
    /// shows or was asked for (within 3 %).
    fn ask(&mut self, px: u32) {
        let close = |a: u32, b: u32| a.abs_diff(b) <= (b / 33).max(2);
        if let Some(s) = &self.shown
            && s.page == self.page
            && close(px, s.width)
        {
            return;
        }
        if let Some((page, w)) = self.asked
            && page == self.page
            && close(px, w)
        {
            return;
        }
        if let Some(worker) = &self.worker {
            worker.send(DocRequest::Render {
                doc: self.doc,
                page: self.page,
                px_width: px,
            });
            self.asked = Some((self.page, px));
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
            let zoom_text = match state.zoom {
                Zoom::FitWidth => t("viewer.zoom.fit_width"),
                Zoom::FitPage => t("viewer.zoom.fit_page"),
                Zoom::Percent(p) => t_with("viewer.zoom.percent", &[("percent", &p.to_string())]),
            };
            ui.label(RichText::new(zoom_text).color(tokens.text_secondary));
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
            if state.full_size_button {
                let r = ui
                    .button(t("viewer.full_size"))
                    .on_hover_text(t("viewer.full_size.tip"));
                focused |= r.has_focus();
                if r.clicked() {
                    event = ViewerEvent::OpenFullSize;
                }
            }
        });
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
        match state.status {
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
        // Replies arrive on another thread: look again soon while waiting.
        if state.status == Status::Loading
            || (state.status == Status::Ready
                && state.shown.as_ref().is_none_or(|s| s.page != state.page))
            || state
                .asked
                .is_some_and(|(_, w)| state.shown.as_ref().is_none_or(|s| s.width != w))
        {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
        state.focused = focused;
        event
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
        egui::ScrollArea::both()
            .id_salt(("doc-viewer", state.doc))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (size, texture) = match &state.shown {
                    Some(s) => {
                        let aspect = s.natural.y / s.natural.x.max(1.0);
                        (Vec2::new(shown_w, shown_w * aspect), Some(s.texture.id()))
                    }
                    None => (
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
        let nodes = draw_until(&mut v, true, |v| v.state_json()["rendered"] == true);
        let has = |role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
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
        // Fit width here is about 95 %: the next step up.
        assert_eq!(v.zoom(), Zoom::Percent(100));
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
}
