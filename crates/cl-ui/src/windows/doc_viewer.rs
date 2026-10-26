//! The pop-out document viewer (A02, plan D3): one operating-system window
//! the shell owns, reused for every form. It follows the window in front
//! through [`crate::windows::Screen::document`], shows the form's name and
//! the company stripe, remembers where it sat (`doc-viewer.json`, Q-A02c),
//! and docks back when closed without touching the form.
//!
//! It is a **deferred** viewport (Q-A02a): the A02 spike on eframe 0.37 wgpu
//! found that a `Screenshot` sent to an immediate child never answers, while
//! a deferred one does. The window's state therefore lives behind an
//! `Arc<Mutex<..>>` the callback shares; the shell never holds that lock
//! while it calls `show_viewport_deferred`, which runs the callback at once
//! when viewports are embedded. When `embed_viewports()` is true (tests,
//! platforms without windows) the viewer is an egui window inside the main
//! one and the main window's keys reach it.
//!
//! Never call `ctx.request_repaint()` (without a delay) from code the
//! window draws: in the spike a deferred child repainting every pass
//! starved the main window, which stopped drawing. `request_repaint_after`
//! with a delay (the viewer polls its worker every 30 ms) is fine.
//!
//! Keys (Q-A02g): in its own window the viewer resolves keys in
//! [`Scope::Viewer`]; `viewer.*`, F6 and Ctrl+Shift+D act there and every
//! other command runs in the main window, which comes forward.
//!
//! Nothing here names a file: titles carry the form's name only, and the
//! saved placement is geometry.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use cl_model::{CommandId, Keymap, Scope, Shortcut};
use eframe::egui::{
    self, Color32, Context, Frame, Id, Margin, RichText, ViewportBuilder, ViewportClass,
    ViewportCommand, ViewportId,
};

use crate::i18n::{t, t_with};
use crate::prefs::{self, DocViewerPlacement};
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocSource, DocViewer, ViewerState};

/// The viewer window's viewport (the control channel's `doc_viewer`).
#[must_use]
pub fn viewport_id() -> ViewportId {
    crate::automation::doc_viewer_viewport_id()
}

/// Identifies one document a screen shows: a new key makes the pop-out
/// window open it; the same key keeps its page and zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DocKey(u64);

impl DocKey {
    /// A key from a number the screen chooses (an attachment's id hash).
    #[must_use]
    pub fn new(raw: u64) -> DocKey {
        DocKey(raw)
    }

    /// A key for the bytes `source` holds: the same shared bytes give the
    /// same key, and a reloaded document (new bytes) a new one.
    ///
    /// It is the allocation's address and length. An address can be reused
    /// only after its bytes are freed, and the window keeps a clone of the
    /// shown document's `Arc` for as long as it shows it, so a document
    /// it shows is never confused with a new one at the same address.
    #[must_use]
    pub fn of(source: &DocSource) -> DocKey {
        let addr = Arc::as_ptr(&source.bytes).cast::<u8>().addr();
        // A pointer and a length are geometry of memory, not owner data.
        DocKey((addr as u64).rotate_left(17) ^ source.bytes.len() as u64)
    }
}

/// What a screen shows in its document viewer ([`crate::windows::Screen::document`]).
#[derive(Clone, PartialEq, Eq)]
pub struct DocRef {
    /// Which document.
    pub key: DocKey,
    /// Its name and bytes (shared, cheap to clone).
    pub source: DocSource,
    /// The form's name for the window title ("Enter Bill"); empty uses the
    /// window's command label. Never a file name.
    pub owner_title: String,
}

impl DocRef {
    /// A reference keyed by [`DocKey::of`], titled by the window's command.
    #[must_use]
    pub fn new(source: DocSource) -> DocRef {
        DocRef {
            key: DocKey::of(&source),
            source,
            owner_title: String::new(),
        }
    }
}

/// No file name.
impl std::fmt::Debug for DocRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocRef")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// What a docked pane asked for this frame (`widgets::doc_pane`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRequest {
    /// "Pop out" was clicked.
    PopOut,
    /// "Dock" was clicked on the popped-out placeholder.
    Dock,
}

fn popped_id() -> Id {
    Id::unique("cl-doc-viewer-popped")
}

fn request_id() -> Id {
    Id::unique("cl-doc-viewer-request")
}

fn focus_toggle_id() -> Id {
    Id::unique("cl-doc-pane-focus-toggle")
}

/// True while the document shows in its own window: docked panes then draw
/// a placeholder with "Dock" (set by the shell each frame).
#[must_use]
pub fn is_popped(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(popped_id()).unwrap_or(false))
}

/// Ask the shell to pop out or dock (a docked pane's buttons).
pub fn request(ctx: &Context, what: PaneRequest) {
    ctx.data_mut(|d| d.insert_temp(request_id(), what));
}

fn take_request(ctx: &Context) -> Option<PaneRequest> {
    ctx.data_mut(|d| {
        let r = d.get_temp::<PaneRequest>(request_id());
        d.remove::<PaneRequest>(request_id());
        r
    })
}

/// F6 with the viewer docked: the pane drawn in `viewport` moves the
/// keyboard between its form and its viewer next frame.
pub(crate) fn ask_focus_toggle(ctx: &Context, viewport: ViewportId) {
    ctx.data_mut(|d| d.insert_temp(focus_toggle_id(), viewport));
}

/// Taken by the docked pane drawn in the viewport F6 was pressed in.
#[must_use]
pub(crate) fn take_focus_toggle(ctx: &Context) -> bool {
    let here = ctx.viewport_id();
    ctx.data_mut(|d| {
        if d.get_temp::<ViewportId>(focus_toggle_id()) == Some(here) {
            d.remove_temp::<ViewportId>(focus_toggle_id());
            true
        } else {
            false
        }
    })
}

/// Where a key pressed in the viewer's own window goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// A `viewer.*` command for the window's viewer.
    Viewer(CommandId),
    /// F6: back to the form in the main window.
    BackToForm,
    /// Ctrl+Shift+D: dock.
    Dock,
    /// Anything else runs in the main window, which comes forward.
    Main(CommandId),
    /// Not a command here.
    Ignore,
}

/// The state the window's callback shares with the shell.
struct PopoutShared {
    viewer: ViewerState,
    shown: Option<DocKey>,
    /// The form's name ("Enter Bill"); empty with no document.
    form: String,
    accent: Color32,
    pending: Vec<CommandId>,
    focus_req: bool,
    back_to_form: bool,
    dock_req: bool,
    keymap: Keymap,
}

impl PopoutShared {
    fn route(&self, s: Shortcut) -> Route {
        let Some(id) = self.keymap.resolve(Scope::Viewer, s) else {
            return Route::Ignore;
        };
        match id.as_str() {
            "view.focus_doc" => Route::BackToForm,
            "view.toggle_doc_viewer" => Route::Dock,
            // Esc has no form field to revert here.
            "form.revert" => Route::Ignore,
            x if x.starts_with("viewer.") => Route::Viewer(id),
            _ => Route::Main(id),
        }
    }

    /// The window's own keys (only when it is its own OS window; embedded,
    /// the main window's input already went through the shell).
    fn handle_keys(&mut self, ctx: &Context) {
        let pressed = ctx.input(crate::input::pressed_shortcuts);
        let mut handled: Vec<Shortcut> = Vec::new();
        for s in pressed {
            match self.route(s) {
                Route::Ignore => continue,
                // A key the viewer doesn't act on stays with the window's
                // buttons (A04: Enter is `viewer.fill_word`, and Enter on a
                // focused Dock button must still dock).
                Route::Viewer(id) => {
                    if !self.viewer.command(id) {
                        continue;
                    }
                }
                Route::BackToForm => {
                    self.back_to_form = true;
                    // The viewer lets go of the keyboard in its window too,
                    // so it no longer counts as focused (review S5).
                    ctx.memory_mut(|m| {
                        if let Some(f) = m.focused() {
                            m.surrender_focus(f);
                        }
                    });
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                }
                Route::Dock => self.dock_req = true,
                Route::Main(id) => {
                    self.pending.push(id);
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                }
            }
            handled.push(s);
        }
        if !handled.is_empty() {
            ctx.input_mut(|i| {
                i.events.retain(|e| match e {
                    egui::Event::Key { key, modifiers, .. } => crate::input::key_from_egui(*key)
                        .is_none_or(|k| {
                            !handled.contains(&Shortcut::new(
                                crate::input::mods_from_egui(*modifiers, k),
                                k,
                            ))
                        }),
                    _ => true,
                });
            });
            ctx.request_repaint_of(ViewportId::ROOT);
        }
    }

    fn title(&self) -> String {
        if self.form.is_empty() {
            t("docviewer.title_empty")
        } else {
            t_with("docviewer.title", &[("form", &self.form)])
        }
    }
}

/// What the shell hands the window each frame.
pub struct Follow<'a> {
    /// The followed window's document.
    pub doc: Option<DocRef>,
    /// The followed window's command kind (`vendors.enter_bill`).
    pub kind: Option<CommandId>,
    /// The company stripe.
    pub accent: Color32,
    /// The shell's keymap (cloned into the window when it changed).
    pub keymap: &'a Keymap,
}

/// The control channel's view of the viewer window (geometry and ids only).
#[derive(Debug, Clone, PartialEq)]
pub struct DocViewerState {
    /// In its own window.
    pub popped: bool,
    /// The command kind of the window it follows.
    pub follows: Option<&'static str>,
    /// A document is open in it.
    pub has_doc: bool,
    /// The page shown, from 0.
    pub page: u32,
    /// Pages known.
    pub pages: u32,
    /// The viewer's state (`ViewerState::state_json`, no names).
    pub viewer: serde_json::Value,
    /// The remembered outer position and inner size, `[x, y, w, h]`.
    pub placement: Option<[f32; 4]>,
    /// `deferred` (its own window), `embedded` (inside the main window) or
    /// `none` (docked).
    pub class: &'static str,
}

impl DocViewerState {
    /// As JSON for `state.doc_viewer`.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "popped": self.popped,
            "follows": self.follows,
            "has_doc": self.has_doc,
            "page": self.page,
            "pages": self.pages,
            "viewer": self.viewer,
            "placement": self.placement.map(|[x, y, w, h]| serde_json::json!({
                "x": x, "y": y, "w": w, "h": h,
            })),
            "class": self.class,
        })
    }
}

/// The shell's pop-out viewer window.
pub struct DocViewerWindow {
    popped: bool,
    placement: DocViewerPlacement,
    /// The placement the window opened from (the gone-monitor check).
    opened_from: DocViewerPlacement,
    changed_at: Option<f64>,
    shared: Arc<Mutex<PopoutShared>>,
    builder: Option<ViewportBuilder>,
    frames_open: u32,
    follows: Option<CommandId>,
    /// The form widget that had the keyboard before F6 moved it here.
    form_focus: Option<Id>,
    /// The window's place was seen this session: it opened there and the
    /// owner moved it since (S1). A place from `doc-viewer.json` alone may
    /// be on a monitor that is gone.
    place_seen: bool,
    /// The owner asked for the window (Pop out, Ctrl+Shift+D) with a place
    /// not seen this session: the gone-monitor check brings it beside the
    /// main window even when the monitor sizes match.
    recall: bool,
}

impl DocViewerWindow {
    /// A docked viewer with a remembered placement.
    #[must_use]
    pub fn new(placement: DocViewerPlacement, keymap: &Keymap) -> DocViewerWindow {
        let mut viewer = ViewerState::default();
        viewer.set_full_size_button(false);
        viewer.set_pop_out_button(false);
        if let Some(z) = placement.zoom {
            viewer.set_zoom(z);
        }
        DocViewerWindow {
            popped: false,
            placement,
            opened_from: placement,
            changed_at: None,
            shared: Arc::new(Mutex::new(PopoutShared {
                viewer,
                shown: None,
                form: String::new(),
                accent: Color32::TRANSPARENT,
                pending: Vec::new(),
                focus_req: false,
                back_to_form: false,
                dock_req: false,
                keymap: keymap.clone(),
            })),
            builder: None,
            frames_open: 0,
            follows: None,
            form_focus: None,
            place_seen: false,
            recall: false,
        }
    }

    fn lock(&self) -> Option<MutexGuard<'_, PopoutShared>> {
        // A poisoned lock means a panic mid-frame; the viewer then stays
        // blank rather than taking the app down.
        self.shared.lock().ok()
    }

    /// In its own window.
    #[must_use]
    pub fn popped(&self) -> bool {
        self.popped
    }

    /// The remembered placement.
    #[must_use]
    pub fn placement(&self) -> &DocViewerPlacement {
        &self.placement
    }

    fn mark(&mut self, now: f64) {
        // Saved a second after the last move or resize.
        self.changed_at = Some(now);
    }

    /// Move the document into its own window (the owner asked: a window
    /// whose place nobody saw this session comes beside the main one when
    /// it is off the main window's monitor).
    pub fn pop_out(&mut self, now: f64) {
        if self.popped {
            return;
        }
        self.popped = true;
        self.placement.popped = true;
        self.opened_from = self.placement;
        self.builder = None;
        self.frames_open = 0;
        self.recall = !self.place_seen;
        self.mark(now);
    }

    /// Put the document back in the form. The form is not touched.
    pub fn dock(&mut self, now: f64) {
        if !self.popped {
            return;
        }
        self.close_window();
        self.placement.popped = false;
        self.mark(now);
    }

    /// Pop out or dock (Ctrl+Shift+D).
    pub fn toggle(&mut self, now: f64) {
        if self.popped {
            self.dock(now);
        } else {
            self.pop_out(now);
        }
    }

    /// Close the window, keeping `popped` in the placement (the company
    /// closed: it comes back popped with the next one).
    pub fn close_window(&mut self) {
        self.popped = false;
        self.builder = None;
        self.form_focus = None;
        self.clear_doc();
    }

    /// Forget the document shown (the company switched).
    pub fn clear_doc(&mut self) {
        if let Some(mut s) = self.lock() {
            // Clearing closes the worker's copy of the document.
            s.viewer.clear();
            s.shown = None;
            s.form.clear();
            s.pending.clear();
            s.focus_req = false;
            s.back_to_form = false;
            s.dock_req = false;
        }
        self.follows = None;
    }

    /// A company opened: the viewer comes back the way it was left.
    pub fn restore_popped(&mut self) {
        if self.placement.popped && !self.popped {
            self.popped = true;
            self.opened_from = self.placement;
            self.builder = None;
            self.frames_open = 0;
            self.recall = false;
        }
    }

    /// The builder the window opens with: the remembered size and, when
    /// known, position, fitted to the monitor it was on.
    #[must_use]
    pub fn builder(&self, title: &str) -> ViewportBuilder {
        builder_for(&self.placement, title)
    }

    /// True when the viewer, shown inside the main window, has the
    /// keyboard (the main window's keys then act on it).
    #[must_use]
    pub fn embedded_has_focus(&self, ctx: &Context) -> bool {
        self.popped && ctx.embed_viewports() && self.lock().is_some_and(|s| s.viewer.has_focus())
    }

    /// Run a `viewer.*` command on the window's viewer.
    pub fn viewer_command(&mut self, id: CommandId) -> bool {
        self.lock().is_some_and(|mut s| s.viewer.command(id))
    }

    /// F6 (`view.focus_doc`). Docked, the pane in `ctx`'s viewport takes it
    /// (the form widget it leaves is remembered there). Popped, the
    /// keyboard moves to the viewer window, or back to the form.
    pub fn focus_toggle(&mut self, ctx: &Context) {
        if !self.popped {
            ask_focus_toggle(ctx, ctx.viewport_id());
            return;
        }
        let in_viewer = if ctx.embed_viewports() {
            self.lock().is_some_and(|s| s.viewer.has_focus())
        } else {
            false
        };
        if in_viewer {
            self.back_to_form(ctx);
            return;
        }
        self.form_focus = ctx.memory(eframe::egui::Memory::focused);
        if let Some(mut s) = self.lock() {
            s.focus_req = true;
        }
        if !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::Focus);
        }
        ctx.request_repaint_of(viewport_id());
    }

    fn back_to_form(&mut self, ctx: &Context) {
        let back = self.form_focus.take();
        if back.is_some() || ctx.embed_viewports() {
            ctx.memory_mut(|m| match back {
                Some(id) => m.request_focus(id),
                // Nothing had the keys before: the viewer just lets go.
                None => {
                    if let Some(f) = m.focused() {
                        m.surrender_focus(f);
                    }
                }
            });
        }
        if !ctx.embed_viewports() {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
        }
    }

    /// Publish whether the viewer is popped (before the screens draw, so
    /// their panes show the placeholder).
    pub fn publish(&self, ctx: &Context) {
        ctx.data_mut(|d| d.insert_temp(popped_id(), self.popped));
    }

    /// The window's frame, after the screens drew. Applies the panes'
    /// requests, follows `follow`, draws the window and tracks where it
    /// sits. Returns the commands pressed in the window for the main window
    /// to run.
    pub fn frame(&mut self, ctx: &Context, follow: Follow<'_>) -> Vec<CommandId> {
        let now = ctx.input(|i| i.time);
        match take_request(ctx) {
            Some(PaneRequest::PopOut) => self.pop_out(now),
            Some(PaneRequest::Dock) => self.dock(now),
            None => {}
        }
        // Unclaimed F6 toggles do not linger into a later frame.
        ctx.data_mut(|d| d.remove_temp::<ViewportId>(focus_toggle_id()));
        if !self.popped {
            return Vec::new();
        }
        self.follows = follow.kind;
        let remembered = self.placement.zoom;
        let (pending, back, dock, title) = {
            let Some(mut s) = self.lock() else {
                return Vec::new();
            };
            if s.keymap.mode() != follow.keymap.mode() {
                s.keymap = follow.keymap.clone();
            }
            s.accent = follow.accent;
            if let Some(doc) = follow.doc {
                s.form = if doc.owner_title.is_empty() {
                    follow
                        .kind
                        .map(|k| t(&format!("cmd.{k}")))
                        .unwrap_or_default()
                } else {
                    doc.owner_title
                };
                if s.shown != Some(doc.key) {
                    if s.viewer.source().is_none()
                        && let Some(z) = remembered
                    {
                        // Cleared (docked, or nothing to show): back to the
                        // window's own zoom.
                        s.viewer.set_zoom(z);
                    }
                    s.viewer.open_keep_zoom(doc.source);
                    s.shown = Some(doc.key);
                    ctx.request_repaint_of(viewport_id());
                }
            } else {
                if s.shown.take().is_some() {
                    s.viewer.clear();
                    ctx.request_repaint_of(viewport_id());
                }
                s.form.clear();
            }
            let title = s.title();
            (
                std::mem::take(&mut s.pending),
                std::mem::take(&mut s.back_to_form),
                std::mem::take(&mut s.dock_req),
                title,
            )
        };
        if back {
            self.back_to_form(ctx);
        }
        if dock {
            self.dock(now);
            return pending;
        }
        // The lock is free here: embedded, the callback runs right away.
        let builder = self
            .builder
            .get_or_insert_with(|| builder_for(&self.placement, &title))
            .clone()
            .with_title(title);
        let shared = Arc::clone(&self.shared);
        ctx.show_viewport_deferred(viewport_id(), builder, move |ui, class| {
            draw(ui, class, &shared);
        });
        let (closed, zoom) = self.lock().map_or((false, None), |mut s| {
            let zoom = s.viewer.source().is_some().then(|| s.viewer.zoom());
            (std::mem::take(&mut s.dock_req), zoom)
        });
        if closed {
            self.dock(now);
            return pending;
        }
        self.note_zoom(zoom, now);
        self.track(ctx, now);
        pending
    }

    /// Follow the window's OS position and size; two frames after opening,
    /// move it beside the main window when its monitor is gone.
    fn track(&mut self, ctx: &Context, now: f64) {
        if ctx.embed_viewports() {
            return;
        }
        let (info, root) = ctx.input(|i| {
            (
                i.raw.viewports.get(&viewport_id()).cloned(),
                i.raw
                    .viewports
                    .get(&ViewportId::ROOT)
                    .and_then(|v| v.outer_rect),
            )
        });
        let Some(info) = info else {
            // Not created yet: ask for another frame.
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        };
        self.frames_open = self.frames_open.saturating_add(1);
        if self.frames_open == 3
            && let (Some(outer), Some(monitor), Some(root)) =
                (info.outer_rect, info.monitor_size, root)
            && let Some(pos) = self.gone_monitor_fix(outer, monitor, root)
        {
            ctx.send_viewport_cmd_to(viewport_id(), ViewportCommand::OuterPosition(pos));
        }
        if self.frames_open < 3 {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let before = self.placement.outer_pos;
        self.note_moved(
            info.outer_rect.map(|r| r.min),
            info.inner_rect.map(|r| r.size()),
            info.monitor_size,
            now,
        );
        if self.frames_open > 3 && self.placement.outer_pos != before {
            self.owner_moved();
        }
    }

    /// The window moved after it settled: the owner put it where they can
    /// see it, so a later Pop out keeps that place.
    pub fn owner_moved(&mut self) {
        self.place_seen = true;
        self.recall = false;
    }

    /// Where the window must move when the monitor it was saved on is gone
    /// (`outer` and `monitor` are what the OS reports now, `root` the main
    /// window's outer rect); `None` leaves it.
    #[must_use]
    pub fn gone_monitor_fix(
        &self,
        outer: egui::Rect,
        monitor: egui::Vec2,
        root: egui::Rect,
    ) -> Option<egui::Pos2> {
        prefs::gone_monitor_fix(&self.opened_from, outer, monitor, root, self.recall)
    }

    /// The OS reported the window's place (`now` in egui seconds): remember
    /// it, to be written a second later.
    pub fn note_moved(
        &mut self,
        outer_pos: Option<egui::Pos2>,
        inner_size: Option<egui::Vec2>,
        monitor_size: Option<egui::Vec2>,
        now: f64,
    ) {
        let mut next = self.placement;
        if outer_pos.is_some() {
            next.outer_pos = outer_pos;
        }
        if let Some(size) = inner_size {
            next.inner_size = size;
        }
        if monitor_size.is_some() {
            next.monitor_size = monitor_size;
        }
        if differs(&next, &self.placement) {
            self.placement = next;
            self.mark(now);
        }
    }

    /// The window's zoom changed: remember it (written a second later).
    fn note_zoom(&mut self, zoom: Option<crate::widgets::pdf_viewer::Zoom>, now: f64) {
        if zoom.is_some() && zoom != self.placement.zoom {
            // The default zoom is not worth a write until it was changed.
            if self.placement.zoom.is_none()
                && zoom == Some(crate::widgets::pdf_viewer::Zoom::default())
            {
                return;
            }
            self.placement.zoom = zoom;
            self.mark(now);
        }
    }

    /// Write the placement a second after the last change (`now` in egui
    /// seconds), or at once with `None` (quitting). The caller skips this
    /// under the control channel (`run_only`).
    pub fn flush(&mut self, dir: &std::path::Path, now: Option<f64>) {
        let due = self
            .changed_at
            .is_some_and(|at| now.is_none_or(|n| n - at >= 1.0));
        if !due {
            return;
        }
        self.changed_at = None;
        if let Err(e) = self.placement.save(dir) {
            tracing::warn!(error = %e, "could not save the document window's place");
        }
    }

    /// True while a placement change waits to be written.
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.changed_at.is_some()
    }

    /// The control channel's view.
    #[must_use]
    pub fn state(&self, ctx: &Context) -> DocViewerState {
        let (has_doc, page, pages, viewer) = self.lock().map_or_else(
            || (false, 0, 0, serde_json::Value::Null),
            |s| {
                (
                    s.viewer.source().is_some(),
                    s.viewer.page(),
                    s.viewer.page_count(),
                    s.viewer.state_json(),
                )
            },
        );
        let p = &self.placement;
        DocViewerState {
            popped: self.popped,
            follows: self.follows.filter(|_| self.popped).map(CommandId::as_str),
            has_doc,
            page,
            pages,
            viewer,
            placement: p
                .outer_pos
                .map(|pos| [pos.x, pos.y, p.inner_size.x, p.inner_size.y]),
            class: match (self.popped, ctx.embed_viewports()) {
                (false, _) => "none",
                (true, true) => "embedded",
                (true, false) => "deferred",
            },
        }
    }

    /// Route a key pressed in the window's own OS window (tests drive this
    /// directly; the callback calls it with the window's input).
    #[must_use]
    pub fn route(&self, s: Shortcut) -> Route {
        self.lock().map_or(Route::Ignore, |p| p.route(s))
    }

    /// Feed a shortcut as if pressed in the window's own OS window (tests;
    /// with embedded viewports the callback never sees its own keys).
    #[cfg(test)]
    pub(crate) fn press_in_window(&mut self, ctx: &Context, s: Shortcut) {
        if let Some(mut p) = self.lock() {
            match p.route(s) {
                Route::Viewer(id) => {
                    let _ = p.viewer.command(id);
                }
                Route::BackToForm => p.back_to_form = true,
                Route::Dock => p.dock_req = true,
                Route::Main(id) => p.pending.push(id),
                Route::Ignore => {}
            }
        }
        ctx.request_repaint();
    }
}

/// The builder for p: the remembered size and, when known, position,
/// fitted to the monitor it was on.
fn builder_for(p: &DocViewerPlacement, title: &str) -> ViewportBuilder {
    let fitted = p.saved_screen().map_or_else(
        || prefs::ClampedPlacement {
            pos: None,
            size: p
                .inner_size
                .clamp(DocViewerPlacement::MIN_SIZE, egui::vec2(4000.0, 4000.0)),
        },
        |screen| prefs::clamp_placement(p, screen),
    );
    let mut b = ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(fitted.size)
        .with_min_inner_size(DocViewerPlacement::MIN_SIZE);
    if let Some(pos) = fitted.pos {
        b = b.with_position(pos);
    }
    if let Some(icon) = crate::icon::viewport_icon() {
        b = b.with_icon(icon);
    }
    b
}

fn differs(a: &DocViewerPlacement, b: &DocViewerPlacement) -> bool {
    let near = |x: f32, y: f32| (x - y).abs() < 0.5;
    let pos = match (a.outer_pos, b.outer_pos) {
        (Some(p), Some(q)) => !(near(p.x, q.x) && near(p.y, q.y)),
        (None, None) => false,
        _ => true,
    };
    let size = !(near(a.inner_size.x, b.inner_size.x) && near(a.inner_size.y, b.inner_size.y));
    pos || size || a.monitor_size != b.monitor_size || a.popped != b.popped
}

/// The window's body: the company stripe, the form's name with Dock, and
/// the viewer.
fn draw(ui: &mut egui::Ui, class: ViewportClass, shared: &Arc<Mutex<PopoutShared>>) {
    let ctx = ui.ctx().clone();
    let tokens = Tokens::of(&ctx);
    let Ok(mut s) = shared.lock() else {
        return;
    };
    let native = matches!(class, ViewportClass::Deferred | ViewportClass::Immediate);
    if native {
        s.handle_keys(&ctx);
    }
    if s.focus_req {
        s.focus_req = false;
        s.viewer.focus();
    }
    let accent = s.accent;
    let form = s.form.clone();
    let mut dock = false;
    egui::CentralPanel::default()
        .frame(Frame::new().fill(tokens.surface_window))
        .show(ui, |ui| {
            // The 4 pt company stripe (the title bar's accent, Q-184).
            let (stripe, _) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.0), egui::Sense::hover());
            ui.painter().rect_filled(stripe, 0.0, accent);
            Frame::new()
                .fill(tokens.chrome_title)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t("docviewer.eyebrow"))
                                .size(12.0)
                                .color(tokens.chrome_title_muted),
                        );
                        if !form.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&form)
                                        .size(14.0)
                                        .family(crate::fonts::bold())
                                        .color(tokens.chrome_title_text),
                                )
                                .truncate()
                                .selectable(false),
                            );
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let keys = cl_model::commands::find("view.toggle_doc_viewer")
                                .and_then(|c| c.shortcut)
                                .map(crate::input::shortcut_text)
                                .unwrap_or_default();
                            if ui
                                .button(t("docviewer.dock"))
                                .on_hover_text(t_with("docviewer.dock.tip", &[("keys", &keys)]))
                                .clicked()
                            {
                                dock = true;
                            }
                        });
                    });
                });
            Frame::new().inner_margin(Margin::same(8)).show(ui, |ui| {
                if s.viewer.source().is_none() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(t("docviewer.empty")).size(15.0));
                        ui.label(
                            RichText::new(t("docviewer.empty.hint")).color(tokens.text_secondary),
                        );
                    });
                } else {
                    let _ = DocViewer::show(ui, &mut s.viewer, &tokens);
                }
            });
        });
    if native && ctx.input(|i| i.viewport().close_requested()) {
        // Closing the window docks it; the form is never asked anything.
        dock = true;
    }
    if dock {
        s.dock_req = true;
        ctx.request_repaint_of(ViewportId::ROOT);
    }
    if !s.pending.is_empty() || s.back_to_form {
        ctx.request_repaint_of(ViewportId::ROOT);
    }
}

#[cfg(test)]
mod tests {
    use cl_model::{COMMANDS, Key, Mods};

    use super::*;

    fn window() -> DocViewerWindow {
        DocViewerWindow::new(
            DocViewerPlacement::default(),
            &Keymap::new(COMMANDS).unwrap(),
        )
    }

    #[test]
    fn keys_in_the_window_route_as_planned() {
        let w = window();
        let at = |mods, k| w.route(Shortcut::new(mods, k));
        assert_eq!(
            at(Mods::NONE, Key::PageDown),
            Route::Viewer(CommandId::new("viewer.page_next"))
        );
        assert_eq!(
            at(Mods::CMD, Key::Equals),
            Route::Viewer(CommandId::new("viewer.zoom_in"))
        );
        assert_eq!(at(Mods::NONE, Key::F(6)), Route::BackToForm);
        assert_eq!(at(Mods::CMD_SHIFT, Key::Letter('D')), Route::Dock);
        assert_eq!(
            at(Mods::CMD, Key::Letter('B')),
            Route::Main(CommandId::new("vendors.enter_bill"))
        );
        assert_eq!(at(Mods::NONE, Key::Escape), Route::Ignore);
        assert_eq!(at(Mods::NONE, Key::F(12)), Route::Ignore);
    }

    /// Plan §5: the key path the window's own OS window runs
    /// (`handle_keys`, not the test-only `press_in_window`): a main-window
    /// command is queued and its key taken out of the input, F6 and
    /// Ctrl+Shift+D are noted, Esc is left alone.
    #[test]
    fn the_window_own_key_path_routes_and_consumes() {
        let w = window();
        let ctx = egui::Context::default();
        let press = |k: egui::Key, modifiers: egui::Modifiers| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        let ctrl = egui::Modifiers::COMMAND;
        let raw = egui::RawInput {
            events: vec![
                press(egui::Key::B, ctrl),
                press(egui::Key::F6, egui::Modifiers::NONE),
                press(egui::Key::D, ctrl | egui::Modifiers::SHIFT),
                press(egui::Key::Escape, egui::Modifiers::NONE),
            ],
            ..egui::RawInput::default()
        };
        let mut left = Vec::new();
        let mut out = ctx.run_ui(raw, |ui| {
            let mut s = w.shared.lock().expect("lock");
            s.handle_keys(ui.ctx());
            left = ui.input(|i| i.events.clone());
        });
        out.textures_delta.clear();
        let s = w.shared.lock().expect("lock");
        assert_eq!(s.pending, vec![CommandId::new("vendors.enter_bill")]);
        assert!(s.back_to_form && s.dock_req);
        let keys: Vec<_> = left
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key { key, .. } => Some(*key),
                _ => None,
            })
            .collect();
        assert_eq!(keys, vec![egui::Key::Escape], "only Esc is left");
    }

    #[test]
    fn keys_and_debug_carry_no_file_name() {
        let src = DocSource {
            name: "secret-vendor.pdf".into(),
            bytes: Arc::from(vec![1u8, 2, 3]),
        };
        let r = DocRef::new(src.clone());
        assert_eq!(r.key, DocKey::of(&src), "the same bytes, the same key");
        let other = DocSource {
            name: "secret-vendor.pdf".into(),
            bytes: Arc::from(vec![1u8, 2, 3]),
        };
        assert_ne!(DocKey::of(&other), r.key, "new bytes, a new key");
        assert!(!format!("{r:?}").contains("secret"));
    }

    #[test]
    fn the_builder_uses_the_saved_place_fitted_to_its_monitor() {
        let mut w = window();
        w.placement = DocViewerPlacement {
            outer_pos: Some(egui::pos2(2000.0, 50.0)),
            inner_size: egui::vec2(700.0, 5000.0),
            monitor_size: Some(egui::vec2(1920.0, 1080.0)),
            popped: true,
            zoom: None,
        };
        let b = w.builder("x");
        assert_eq!(b.position, Some(egui::pos2(2000.0, 50.0)));
        assert_eq!(b.inner_size, Some(egui::vec2(700.0, 1080.0)));
        let fresh = window().builder("x");
        assert_eq!(fresh.position, None);
        assert_eq!(fresh.inner_size, Some(DocViewerPlacement::DEFAULT_SIZE));
    }

    #[test]
    fn toggling_marks_the_placement_and_closing_the_company_keeps_popped() {
        let mut w = window();
        w.toggle(1.0);
        assert!(w.popped() && w.placement().popped && w.unsaved());
        w.close_window();
        assert!(!w.popped());
        assert!(w.placement().popped, "remembered for the next company");
        w.restore_popped();
        assert!(w.popped());
        w.toggle(2.0);
        assert!(!w.popped() && !w.placement().popped);
    }

    #[test]
    fn the_zoom_is_remembered_and_restored() {
        use crate::widgets::pdf_viewer::Zoom;
        let mut w = window();
        w.note_zoom(Some(Zoom::FitWidth), 1.0);
        assert!(!w.unsaved(), "the default zoom is not a change");
        w.note_zoom(Some(Zoom::Percent(150)), 2.0);
        assert!(w.unsaved());
        assert_eq!(w.placement().zoom, Some(Zoom::Percent(150)));
        let again = DocViewerWindow::new(*w.placement(), &Keymap::new(COMMANDS).unwrap());
        let zoom = again.lock().map(|s| s.viewer.zoom());
        assert_eq!(zoom, Some(Zoom::Percent(150)), "a new window opens at it");
    }
}
