//! The UI side of the control channel (P07).
//!
//! `cl-automation` receives requests on a loopback socket; this module answers
//! them on the UI thread, between frames. It exists only when the app was
//! started with `--control`. Nothing here runs otherwise.
//!
//! How it works: an egui plugin sees every frame's raw input and output. It
//! feeds scheduled events (clicks, keys, text) into the input and keeps the
//! latest AccessKit tree, which is what `inspect` returns. The [`Bridge`]
//! turns each request into events or a shell action and answers once the
//! effect has had a few frames to show.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cl_automation::actions::{
    ClickParams, CommandParams, FocusParams, InspectParams, KeyParams, KeySpec, PasteParams,
    ResizeParams, ScreenshotParams, ScrollParams, ScrollTo, Target, ThemeParams, TypeParams,
    ZoomParams,
};
use cl_automation::inspect::{Bounds, Node, Snapshot, WindowInfo};
use cl_automation::screenshot::{encode_png, write_png};
use cl_automation::{ControlServer, ErrorCode, Pending, Response};
use eframe::egui::{
    self, ColorImage, Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput,
    ViewportCommand, accesskit,
};
use serde_json::{Value, json};

use crate::prefs;
use crate::shell::Shell;
use crate::theme::ThemeChoice;

/// Key of the `Instant` the bridge stores while a screenshot is captured;
/// a screen that can show a secret masks it until then.
#[must_use]
pub fn hide_secrets_id() -> egui::Id {
    egui::Id::unique("cl.hide_secrets_until")
}

/// Frames to wait after an input before answering, so the result is drawn and
/// reported in the tree.
const SETTLE_FRAMES: u64 = 2;
/// How long a screenshot or resize may take.
const JOB_TIMEOUT: Duration = Duration::from_secs(15);
/// One turn of the `scroll` wheel (points) and how many turns it takes:
/// more than any screen is tall, so the area reaches its end.
const WHEEL_STEP: f32 = 4_000.0;
const WHEEL_TURNS: usize = 6;

/// Events to inject into one coming frame.
#[derive(Clone, Default)]
struct Scheduled {
    events: Vec<Event>,
}

#[derive(Default)]
struct Shared {
    /// Frames begun so far.
    frame: u64,
    /// Tree from the last finished frame.
    snapshot: Option<Snapshot>,
    scheduled: VecDeque<Scheduled>,
}

/// The egui plugin: input injection and tree capture.
struct Hooks(Arc<Mutex<Shared>>);

impl egui::Plugin for Hooks {
    fn debug_name(&self) -> &'static str {
        "clientledger-automation"
    }

    fn input_hook(&mut self, _ctx: &Context, input: &mut RawInput) {
        // Driven input is typed into this window, so the window counts as
        // focused whatever the OS says: `Response::has_focus` is false in a
        // background window, which made scripted typing into pickers (and
        // focus rings in screenshots) depend on what else was on screen.
        input.focused = true;
        let Ok(mut s) = self.0.lock() else { return };
        s.frame += 1;
        if let Some(next) = s.scheduled.pop_front() {
            input.events.extend(next.events);
        }
    }

    fn output_hook(&mut self, ctx: &Context, output: &mut FullOutput) {
        let Some(update) = &output.platform_output.accesskit_update else {
            return;
        };
        let rect = ctx.content_rect();
        let window = WindowInfo {
            width: rect.width(),
            height: rect.height(),
            pixels_per_point: ctx.pixels_per_point(),
        };
        let snapshot = snapshot_from(update, window);
        if let (Some(snapshot), Ok(mut s)) = (snapshot, self.0.lock()) {
            s.snapshot = Some(snapshot);
        }
    }
}

/// Events split by the input sequencer are still waiting for a frame: a job
/// is not done until they have been delivered (P10 plan Rev 2 §2.4.2).
fn sequencer_busy(ctx: &Context) -> bool {
    cl_grid::sequencer::pending(ctx, eframe::egui::ViewportId::ROOT)
}

/// `TextInput` becomes `text_input`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn id_text(id: accesskit::NodeId) -> String {
    format!("{:x}", id.0)
}

type NodeMap<'a> = std::collections::HashMap<accesskit::NodeId, &'a accesskit::Node>;

fn build(
    id: accesskit::NodeId,
    nodes: &NodeMap<'_>,
    focus: accesskit::NodeId,
    depth: usize,
) -> Option<Node> {
    let n = nodes.get(&id)?;
    let role = snake(&format!("{:?}", n.role()));
    let mut label = n.label().map(str::to_owned);
    if label.is_none() {
        // egui's `labelled_by` clears the node's own label and points at
        // the visible label widget; name the input after it.
        label = n
            .labelled_by()
            .iter()
            .filter_map(|id| nodes.get(id))
            .find_map(|l| l.label().or(l.value()).map(str::to_owned));
    }
    let mut value = n.value().map(str::to_owned);
    let description = n.description().map(str::to_owned);
    // Plain text is exposed as the node value; treat it as the label so
    // `text equals` works on both buttons and static text.
    if role == "label" && label.is_none() {
        label = value.take();
    }
    let bounds = n.bounds().map(|r| Bounds {
        #[allow(clippy::cast_possible_truncation)]
        x: r.x0 as f32,
        #[allow(clippy::cast_possible_truncation)]
        y: r.y0 as f32,
        #[allow(clippy::cast_possible_truncation)]
        width: (r.x1 - r.x0) as f32,
        #[allow(clippy::cast_possible_truncation)]
        height: (r.y1 - r.y0) as f32,
    });
    let children = if depth < MAX_DEPTH {
        n.children()
            .iter()
            .filter_map(|c| build(*c, nodes, focus, depth + 1))
            .collect()
    } else {
        Vec::new()
    };
    Some(Node {
        id: id_text(id),
        role,
        label,
        value,
        description,
        bounds,
        focused: id == focus,
        enabled: !n.is_disabled(),
        children,
    })
}

/// Convert egui's AccessKit update into the plain tree. Secret values are
/// hidden here, before anything can leave the process.
fn snapshot_from(update: &accesskit::TreeUpdate, window: WindowInfo) -> Option<Snapshot> {
    let nodes: NodeMap<'_> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let root = update.tree.as_ref()?.root;
    let focus = update.focus;

    let mut tree = build(root, &nodes, focus, 0)?;
    tree.redact();
    let focused = nodes.contains_key(&focus).then(|| id_text(focus));
    Some(Snapshot {
        window,
        focused,
        tree,
    })
}

/// Deepest element level returned: well under the 128 levels a JSON reader
/// accepts, so `inspect` output always parses.
const MAX_DEPTH: usize = 50;

/// What to put in the answer when a job finishes.
enum Finish {
    Empty,
    Window,
    Quit,
}

enum JobKind {
    /// Answer once this frame number has passed.
    AfterFrame { until: u64, finish: Finish },
    /// The target was scrolled into view; once the frames settle, find it
    /// again at its new position and click it.
    ClickAfterScroll { target: Box<Target>, until: u64 },
    /// Wait for the rendered pixels.
    Screenshot {
        path: PathBuf,
        slot: Arc<Mutex<Option<Arc<ColorImage>>>>,
    },
}

struct Job {
    pending: Pending,
    kind: JobKind,
    deadline: Instant,
}

enum Started {
    Done(Response),
    Job(JobKind),
}

fn fail(code: ErrorCode, message: impl Into<String>) -> Started {
    Started::Done(Response::failure(code, message))
}

fn ok(value: Value) -> Started {
    Started::Done(Response::success(value))
}

/// Answers control requests for one app.
pub struct Bridge {
    server: ControlServer,
    shared: Arc<Mutex<Shared>>,
    job: Option<Job>,
}

impl Bridge {
    /// Hook the server into this egui context.
    #[must_use]
    pub fn install(ctx: &Context, server: ControlServer) -> Bridge {
        let shared = Arc::new(Mutex::new(Shared::default()));
        // A controlled run never opens a browser: links are recorded.
        let _ = crate::links::LinkSink::install(ctx);
        ctx.add_plugin(Hooks(Arc::clone(&shared)));
        ctx.enable_accesskit();
        let waker_ctx = ctx.clone();
        server.set_waker(move || waker_ctx.request_repaint());
        Bridge {
            server,
            shared,
            job: None,
        }
    }

    fn frame(&self) -> u64 {
        self.shared.lock().map_or(0, |s| s.frame)
    }

    fn snapshot(&self) -> Option<Snapshot> {
        self.shared.lock().ok().and_then(|s| s.snapshot.clone())
    }

    fn schedule(&self, frames: Vec<Scheduled>) {
        if let Ok(mut s) = self.shared.lock() {
            s.scheduled.extend(frames);
        }
    }

    /// Frames still queued for injection.
    fn queued(&self) -> u64 {
        self.shared.lock().map_or(0, |s| s.scheduled.len() as u64)
    }

    /// Run once per frame, from `App::logic`.
    pub fn poll(&mut self, ctx: &Context, shell: &mut Shell) {
        // Some platforms enable the tree lazily; keep it on.
        ctx.enable_accesskit();
        // A controlled run never rewrites the owner's saved preferences.
        shell.set_run_only(true);
        loop {
            if let Some(job) = self.job.take() {
                match self.finish(ctx, shell, job) {
                    Some(job) => {
                        self.job = Some(job);
                        ctx.request_repaint();
                        return;
                    }
                    None => continue,
                }
            }
            let Some(pending) = self.server.try_next() else {
                return;
            };
            match self.start(ctx, shell, &pending.request.method, &pending.request.params) {
                Started::Done(r) => pending.respond(r),
                Started::Job(kind) => {
                    self.job = Some(Job {
                        pending,
                        kind,
                        deadline: Instant::now() + JOB_TIMEOUT,
                    });
                    ctx.request_repaint();
                }
            }
        }
    }

    /// Complete the job if it is ready; hand it back if it is not.
    fn finish(&self, ctx: &Context, shell: &Shell, job: Job) -> Option<Job> {
        if Instant::now() > job.deadline {
            job.pending.respond(Response::failure(
                ErrorCode::Timeout,
                "the app did not finish in time",
            ));
            return None;
        }
        match &job.kind {
            JobKind::AfterFrame { until, finish } => {
                if self.frame() < *until || self.queued() > 0 || sequencer_busy(ctx) {
                    return Some(job);
                }
                let value = match finish {
                    Finish::Empty => json!({}),
                    Finish::Window => window_json(ctx),
                    Finish::Quit => json!({
                        "quit": shell.quit_approved(),
                        "prompt_open": shell.windows.prompt().is_some(),
                    }),
                };
                job.pending.respond(Response::success(value));
                None
            }
            JobKind::ClickAfterScroll { target, until } => {
                if self.frame() < *until || self.queued() > 0 || sequencer_busy(ctx) {
                    return Some(job);
                }
                let target = target.clone();
                match self.click_now(&target) {
                    Ok(()) => Some(Job {
                        pending: job.pending,
                        kind: JobKind::AfterFrame {
                            until: self.frame() + self.queued() + SETTLE_FRAMES + 1,
                            finish: Finish::Empty,
                        },
                        deadline: job.deadline,
                    }),
                    Err(Started::Done(response)) => {
                        job.pending.respond(response);
                        None
                    }
                    Err(Started::Job(_)) => None,
                }
            }
            JobKind::Screenshot { path, slot } => {
                let image = slot.lock().ok().and_then(|mut s| s.take());
                let Some(image) = image else {
                    return Some(job);
                };
                let response = save_screenshot(path, &image);
                job.pending.respond(response);
                None
            }
        }
    }

    fn after(&self, finish: Finish) -> Started {
        Started::Job(JobKind::AfterFrame {
            until: self.frame() + self.queued() + SETTLE_FRAMES + 1,
            finish,
        })
    }

    fn start(&self, ctx: &Context, shell: &mut Shell, method: &str, params: &Value) -> Started {
        // A missing params object means "no parameters".
        let params = if params.is_null() {
            json!({})
        } else {
            params.clone()
        };
        macro_rules! parse {
            ($ty:ty) => {
                match serde_json::from_value::<$ty>(params) {
                    Ok(p) => p,
                    // The parser's message can quote the value (typed text); say
                    // only which method and point at the tool table.
                    Err(_) => {
                        return fail(
                            ErrorCode::BadRequest,
                            format!(
                                "the parameters for `{method}` are not valid (see the tool table)"
                            ),
                        );
                    }
                }
            };
        }
        match method {
            "inspect" => self.inspect(&parse!(InspectParams)),
            "click" => self.click(ctx, &parse!(ClickParams).target),
            "type" => {
                let p = parse!(TypeParams);
                self.send_text(p.target.as_ref(), Event::Text(p.text))
            }
            "paste" => {
                // One paste event, as from the clipboard: a spreadsheet
                // block keeps its tabs and line breaks (P15A).
                let p = parse!(PasteParams);
                self.send_text(p.target.as_ref(), Event::Paste(p.text))
            }
            "key" => self.key(&parse!(KeyParams).key),
            "focus" => self.focus(&parse!(FocusParams).target),
            "scroll" => self.scroll(ctx, &parse!(ScrollParams)),
            "hover" => self.hover(ctx, &parse!(FocusParams).target),
            "command" => Self::command(shell, &parse!(CommandParams).id, self),
            "screenshot" => Self::screenshot(ctx, &parse!(ScreenshotParams)),
            "resize" => {
                let p = parse!(ResizeParams);
                if !(p.width >= 100.0
                    && p.height >= 100.0
                    && p.width <= 10_000.0
                    && p.height <= 10_000.0)
                {
                    return fail(
                        ErrorCode::BadRequest,
                        "width and height must be 100 to 10000",
                    );
                }
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(p.width, p.height)));
                let mut started = self.after(Finish::Window);
                if let Started::Job(JobKind::AfterFrame { until, .. }) = &mut started {
                    // Window managers resize asynchronously.
                    *until += 4;
                }
                started
            }
            "set_theme" => {
                let p = parse!(ThemeParams);
                shell.prefs.theme = match p.theme.as_str() {
                    "light" => ThemeChoice::Light,
                    "dark" => ThemeChoice::Dark,
                    "auto" => ThemeChoice::Auto,
                    other => {
                        return fail(ErrorCode::BadRequest, format!("unknown theme `{other}`"));
                    }
                };
                self.after(Finish::Empty)
            }
            "set_zoom" => {
                let p = parse!(ZoomParams);
                if !(80..=200).contains(&p.percent) {
                    return fail(ErrorCode::BadRequest, "zoom must be between 80 and 200");
                }
                shell.prefs.zoom_percent = prefs::nearest_step(p.percent);
                self.after(Finish::Window)
            }
            "state" => ok(state_json(ctx, shell)),
            "quit" => {
                shell.queue(cl_model::CommandId::new("file.quit"));
                self.after(Finish::Quit)
            }
            other => fail(
                ErrorCode::UnknownMethod,
                format!("unknown method `{other}`"),
            ),
        }
    }

    fn inspect(&self, p: &InspectParams) -> Started {
        let Some(snapshot) = self.snapshot() else {
            return fail(ErrorCode::Failed, "no frame has been drawn yet");
        };
        match &p.target {
            None => ok(serde_json::to_value(&snapshot).unwrap_or(Value::Null)),
            Some(t) => {
                let matches: Vec<Node> = snapshot
                    .find_all(t)
                    .into_iter()
                    .map(|n| Node {
                        children: Vec::new(),
                        ..n.clone()
                    })
                    .collect();
                ok(json!({ "window": snapshot.window, "matches": matches }))
            }
        }
    }

    fn resolve(&self, target: &Target) -> Result<Node, Started> {
        let Some(snapshot) = self.snapshot() else {
            return Err(fail(ErrorCode::Failed, "no frame has been drawn yet"));
        };
        snapshot.resolve(target).cloned().map_err(|e| {
            let code = match e {
                cl_automation::inspect::ResolveError::NotFound(_) => ErrorCode::NotFound,
                cl_automation::inspect::ResolveError::Ambiguous { .. } => ErrorCode::Ambiguous,
            };
            fail(code, e.to_string())
        })
    }

    fn click(&self, ctx: &Context, target: &Target) -> Started {
        let node = match self.resolve(target) {
            Ok(n) => n,
            Err(e) => return e,
        };
        if !node.enabled {
            return fail(ErrorCode::Failed, "that element is disabled");
        }
        let Some(b) = node.bounds else {
            return fail(ErrorCode::Failed, "that element has no position on screen");
        };
        // Below the fold (or above it): ask egui to scroll it into view,
        // then click where it lands.
        let window = ctx.content_rect();
        let (cx, cy) = b.center();
        if !window.contains(Pos2::new(cx, cy))
            && let Some(scroll) = Self::scroll_events(&node.id)
        {
            self.schedule(vec![scroll]);
            ctx.request_repaint();
            return Started::Job(JobKind::ClickAfterScroll {
                target: Box::new(target.clone()),
                until: self.frame() + self.queued() + SETTLE_FRAMES + 1,
            });
        }
        match self.click_now(target) {
            Ok(()) => {
                ctx.request_repaint();
                self.after(Finish::Empty)
            }
            Err(e) => e,
        }
    }

    /// Schedules the pointer events for a click at the target's current
    /// position.
    fn click_now(&self, target: &Target) -> Result<(), Started> {
        let node = self.resolve(target)?;
        if !node.enabled {
            return Err(fail(ErrorCode::Failed, "that element is disabled"));
        }
        let Some(b) = node.bounds else {
            return Err(fail(
                ErrorCode::Failed,
                "that element has no position on screen",
            ));
        };
        let (x, y) = b.center();
        let pos = Pos2::new(x, y);
        let button = |pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        self.schedule(vec![
            Scheduled {
                events: vec![Event::PointerMoved(pos)],
            },
            Scheduled {
                events: vec![button(true)],
            },
            Scheduled {
                events: vec![button(false)],
            },
        ]);
        Ok(())
    }

    fn scroll_events(node_id: &str) -> Option<Scheduled> {
        let id = u64::from_str_radix(node_id, 16).ok()?;
        Some(Scheduled {
            events: vec![Event::AccessKitActionRequest(accesskit::ActionRequest {
                action: accesskit::Action::ScrollIntoView,
                target_tree: accesskit::TreeId::ROOT,
                target_node: accesskit::NodeId(id),
                data: None,
            })],
        })
    }
    fn focus_events(node_id: &str) -> Option<Scheduled> {
        let id = u64::from_str_radix(node_id, 16).ok()?;
        Some(Scheduled {
            events: vec![Event::AccessKitActionRequest(accesskit::ActionRequest {
                action: accesskit::Action::Focus,
                target_tree: accesskit::TreeId::ROOT,
                target_node: accesskit::NodeId(id),
                data: None,
            })],
        })
    }

    /// Moves the pointer over the target (no button), so its hover state
    /// and tooltip show. Nothing is clicked.
    fn hover(&self, ctx: &Context, target: &Target) -> Started {
        let node = match self.resolve(target) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let Some(b) = node.bounds else {
            return fail(ErrorCode::Failed, "that element has no position on screen");
        };
        let (x, y) = b.center();
        self.schedule(vec![Scheduled {
            events: vec![Event::PointerMoved(Pos2::new(x, y))],
        }]);
        ctx.request_repaint();
        self.after(Finish::Empty)
    }

    /// The mouse wheel over the target until what scrolls there reaches
    /// the asked end (the area clamps the far end; egui smooths the turn
    /// over a few frames).
    fn scroll(&self, ctx: &Context, p: &ScrollParams) -> Started {
        let node = match self.resolve(&p.target) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let Some(b) = node.bounds else {
            return fail(ErrorCode::Failed, "that element has no position on screen");
        };
        let (x, y) = b.center();
        let pos = Pos2::new(x, y);
        let dy = match p.to {
            ScrollTo::End => -WHEEL_STEP,
            ScrollTo::Start => WHEEL_STEP,
        };
        let wheel = Scheduled {
            events: vec![
                Event::PointerMoved(pos),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, dy),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Wheel,
                },
            ],
        };
        let mut frames = vec![Scheduled {
            events: vec![Event::PointerMoved(pos)],
        }];
        frames.extend(std::iter::repeat_n(wheel, WHEEL_TURNS));
        self.schedule(frames);
        ctx.request_repaint();
        self.after(Finish::Empty)
    }

    fn focus(&self, target: &Target) -> Started {
        let node = match self.resolve(target) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let Some(ev) = Self::focus_events(&node.id) else {
            return fail(ErrorCode::Failed, "that element cannot take focus");
        };
        self.schedule(vec![ev]);
        self.after(Finish::Empty)
    }

    /// `type` and `paste`: focus the target if any, then deliver `event`.
    fn send_text(&self, target: Option<&Target>, event: Event) -> Started {
        let mut frames = Vec::new();
        if let Some(t) = target {
            let node = match self.resolve(t) {
                Ok(n) => n,
                Err(e) => return e,
            };
            if let Some(ev) = Self::focus_events(&node.id) {
                frames.push(ev);
                // Let focus settle before the text arrives.
                frames.push(Scheduled::default());
            }
        }
        frames.push(Scheduled {
            events: vec![event],
        });
        self.schedule(frames);
        self.after(Finish::Empty)
    }

    fn key(&self, chord: &str) -> Started {
        let spec = match KeySpec::parse(chord) {
            Ok(s) => s,
            Err(m) => return fail(ErrorCode::BadRequest, m),
        };
        let Some(key) = key_from(&spec.key) else {
            return fail(ErrorCode::BadRequest, format!("unknown key `{}`", spec.key));
        };
        let mods = egui_modifiers(&spec, cfg!(target_os = "macos"));
        let event = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        };
        self.schedule(vec![Scheduled {
            events: vec![event(true), event(false)],
        }]);
        self.after(Finish::Empty)
    }

    fn command(shell: &mut Shell, id: &str, bridge: &Bridge) -> Started {
        let Some(cmd) = shell.find_command(id) else {
            return fail(ErrorCode::NotFound, format!("no command `{id}`"));
        };
        shell.queue(cmd.id);
        bridge.after(if id == "file.quit" {
            Finish::Quit
        } else {
            Finish::Empty
        })
    }

    fn screenshot(ctx: &Context, p: &ScreenshotParams) -> Started {
        let path = PathBuf::from(&p.path);
        if let Err(e) = cl_automation::screenshot::check_destination(&path) {
            return fail(ErrorCode::BadRequest, e.to_string());
        }
        // Screens that can show a secret (the recovery key) mask it while
        // the pixels are captured.
        ctx.data_mut(|d| {
            d.insert_temp(
                hide_secrets_id(),
                std::time::Instant::now() + std::time::Duration::from_millis(500),
            );
        });
        let slot: Arc<Mutex<Option<Arc<ColorImage>>>> = Arc::new(Mutex::new(None));
        let cb_slot = Arc::clone(&slot);
        let cb_ctx = ctx.clone();
        ctx.request_screenshot(move |image| {
            if let Ok(mut s) = cb_slot.lock() {
                *s = Some(image);
            }
            cb_ctx.request_repaint();
        });
        ctx.request_repaint();
        Started::Job(JobKind::Screenshot { path, slot })
    }
}

fn save_screenshot(path: &std::path::Path, image: &ColorImage) -> Response {
    let [w, h] = image.size;
    let mut rgba = Vec::with_capacity(w * h * 4);
    for px in &image.pixels {
        rgba.extend_from_slice(&px.to_srgba_unmultiplied());
    }
    let result = u32::try_from(w)
        .ok()
        .zip(u32::try_from(h).ok())
        .ok_or(cl_automation::screenshot::ScreenshotError::Size)
        .and_then(|(w32, h32)| encode_png(w32, h32, &rgba))
        .and_then(|bytes| write_png(path, &bytes));
    match result {
        Ok(()) => Response::success(json!({
            "path": path.display().to_string(),
            "width": w,
            "height": h,
        })),
        Err(e) => Response::failure(ErrorCode::Failed, e.to_string()),
    }
}

/// Look a key up by the names people type (`Esc`, `Enter`, `k`).
/// Modifiers for a chord. `Ctrl` and `Cmd` both mean the keymap's `command`
/// modifier: the Control key on Windows and Linux, the Command key on macOS.
/// Scenarios are written the way DESIGN.md spells shortcuts (`Ctrl+K`) and
/// must pass on every platform.
fn egui_modifiers(spec: &KeySpec, mac: bool) -> Modifiers {
    let command = spec.ctrl || spec.cmd;
    Modifiers {
        alt: spec.alt,
        ctrl: command && !mac,
        shift: spec.shift,
        mac_cmd: command && mac,
        command,
    }
}

fn key_from(name: &str) -> Option<Key> {
    let alias = match name.to_ascii_lowercase().as_str() {
        "esc" => "Escape",
        "return" => "Enter",
        "del" => "Delete",
        "ins" => "Insert",
        "plus" | "+" => "Plus",
        "minus" | "-" => "Minus",
        "equals" | "=" => "Equals",
        "space" => "Space",
        _ => name,
    };
    Key::from_name(alias)
        .or_else(|| Key::from_name(&alias.to_ascii_uppercase()))
        .or_else(|| {
            let mut c = alias.chars();
            let first = c.next()?.to_ascii_uppercase();
            Key::from_name(&format!("{first}{}", c.as_str().to_ascii_lowercase()))
        })
}

fn window_json(ctx: &Context) -> Value {
    let rect = ctx.content_rect();
    json!({
        "width": rect.width(),
        "height": rect.height(),
        "pixels_per_point": ctx.pixels_per_point(),
    })
}

fn state_json(ctx: &Context, shell: &Shell) -> Value {
    let tabs: Vec<Value> = shell
        .windows
        .all()
        .iter()
        .map(|w| {
            json!({
                "title": w.title(),
                "dirty": w.dirty(),
                "active": shell.active_window().is_some_and(|a| a == w.id),
            })
        })
        .collect();
    let host = match shell.host_view() {
        crate::shell::HostView::Unavailable => "unavailable",
        crate::shell::HostView::Starting => "starting",
        crate::shell::HostView::Running => "running",
        crate::shell::HostView::Restarting => "restarting",
        crate::shell::HostView::Failed(_) => "failed",
        crate::shell::HostView::Stopped => "stopped",
    };
    json!({
        "window": window_json(ctx),
        "theme": format!("{:?}", shell.prefs.theme).to_ascii_lowercase(),
        "zoom_percent": shell.prefs.zoom_percent,
        "tabs": tabs,
        "active_tab": shell
            .windows
            .all()
            .iter()
            .find(|w| shell.active_window().is_some_and(|a| a == w.id))
            .map(crate::windows::Window::title),
        "tabs_hidden": ctx
            .data(|d| d.get_temp::<usize>(crate::shell::tabs::hidden_count_id()))
            .unwrap_or(0),
        "palette_open": shell.palette.open,
        "shortcuts_open": shell.shortcuts_open(),
        "about_open": shell.about_open(),
        "error_dialog_open": shell.error_dialog().is_some(),
        "prompt_open": shell.windows.prompt().is_some(),
        "toast": shell.toast_text(),
        "templates_need_you": shell.templates_need_you(),
        "engine": host,
        "quit_approved": shell.quit_approved(),
        "company_open": shell.company.is_some(),
        "signed_in": shell.status.user.is_some(),
        "screen": shell.active_screen_state(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_are_snake_case() {
        assert_eq!(snake("TextInput"), "text_input");
        assert_eq!(snake("Button"), "button");
        assert_eq!(snake("PasswordInput"), "password_input");
    }

    #[test]
    fn key_names() {
        assert_eq!(key_from("Enter"), Some(Key::Enter));
        assert_eq!(key_from("esc"), Some(Key::Escape));
        assert_eq!(key_from("k"), Some(Key::K));
        assert_eq!(key_from("K"), Some(Key::K));
        assert_eq!(key_from("F1"), Some(Key::F1));
        assert_eq!(key_from("tab"), Some(Key::Tab));
        assert_eq!(key_from("nonsense"), None);
    }

    #[test]
    fn ctrl_in_a_chord_is_the_platform_command_modifier() {
        let spec = KeySpec::parse("Ctrl+Shift+K").unwrap();
        let win = egui_modifiers(&spec, false);
        assert!(win.ctrl && win.command && win.shift && !win.mac_cmd);
        let mac = egui_modifiers(&spec, true);
        assert!(mac.mac_cmd && mac.command && mac.shift && !mac.ctrl);
        let cmd = egui_modifiers(&KeySpec::parse("Cmd+K").unwrap(), false);
        assert!(cmd.ctrl && cmd.command && !cmd.mac_cmd);
        let alt = egui_modifiers(&KeySpec::parse("Alt+N").unwrap(), true);
        assert!(alt.alt && !alt.command && !alt.ctrl && !alt.mac_cmd);
    }
}

#[cfg(test)]
mod bridge_tests {
    use std::thread;

    use cl_automation::client::{Client, ClientError, Transport};
    use eframe::egui::{RawInput, Rect, vec2};

    use super::*;
    use crate::shell::ShellOptions;

    pub struct Rig {
        pub ctx: Context,
        pub shell: Shell,
        bridge: Bridge,
        pub control: PathBuf,
        _dir: tempfile::TempDir,
        pub size: egui::Vec2,
    }

    impl Rig {
        pub fn new() -> Rig {
            Rig::with_options(ShellOptions::default())
        }

        pub fn with_options(options: ShellOptions) -> Rig {
            let dir = tempfile::tempdir().unwrap();
            let control = dir.path().join("control.json");
            let server = ControlServer::start(&control).unwrap();
            let ctx = Context::default();
            Shell::install(&ctx);
            let bridge = Bridge::install(&ctx, server);
            cl_grid::sequencer::install(&ctx);
            let shell = Shell::new(ShellOptions {
                demo: true,
                theme: Some(ThemeChoice::Light),
                ..options
            });
            let mut rig = Rig {
                ctx,
                shell,
                bridge,
                control,
                _dir: dir,
                size: vec2(1280.0, 820.0),
            };
            for _ in 0..4 {
                rig.frame();
            }
            rig
        }

        pub fn frame(&mut self) {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                ..RawInput::default()
            };
            let (ctx, shell, bridge) = (&self.ctx, &mut self.shell, &mut self.bridge);
            let mut out = ctx.run_ui(input, |ui| {
                bridge.poll(ui.ctx(), shell);
                shell.ui(ui);
            });
            out.textures_delta.clear();
        }

        /// Run `script` against the server on another thread while this
        /// thread draws frames.
        pub fn run<T: Send + 'static>(
            &mut self,
            script: impl FnOnce(&mut Client) -> T + Send + 'static,
        ) -> T {
            let control = self.control.clone();
            let handle = thread::spawn(move || {
                let mut client = Client::connect(&control).unwrap();
                script(&mut client)
            });
            let start = Instant::now();
            while !handle.is_finished() {
                self.frame();
                thread::sleep(Duration::from_millis(2));
                assert!(start.elapsed() < Duration::from_secs(20), "script hung");
            }
            handle.join().unwrap()
        }
    }

    pub fn call(c: &mut Client, m: &str, p: Value) -> Result<Value, ClientError> {
        c.call(m, p)
    }

    fn err_code(r: Result<Value, ClientError>) -> ErrorCode {
        match r {
            Err(ClientError::Remote(e)) => e.code,
            other => panic!("expected a remote error, got {other:?}"),
        }
    }

    fn tab_target(label: &str) -> Value {
        json!({"target": {"role": "tab", "label": label}})
    }

    /// Driven input lands even when the OS has another window in front:
    /// the window counts as focused, so `Response::has_focus` (pickers'
    /// type-ahead, focus rings) does not depend on the desktop.
    #[test]
    fn a_background_window_still_counts_as_focused() {
        let rig = Rig::new();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, rig.size)),
            focused: false,
            ..RawInput::default()
        };
        let mut seen = false;
        let mut out = rig.ctx.run_ui(input, |ui| seen = ui.input(|i| i.focused));
        out.textures_delta.clear();
        assert!(seen);
    }

    #[test]
    fn inspect_lists_tabs_with_label_role_bounds_and_enabled() {
        let mut rig = Rig::new();
        let v = rig.run(|c| call(c, "inspect", json!({"target": {"role": "tab"}})).unwrap());
        let matches = v["matches"].as_array().unwrap();
        assert_eq!(matches.len(), 4);
        assert_eq!(matches[0]["label"], "Home");
        assert_eq!(matches[0]["enabled"], true);
        assert!(matches[0]["bounds"]["width"].as_f64().unwrap() > 10.0);
        assert!(v["window"]["width"].as_f64().unwrap() > 1000.0);
    }

    #[test]
    fn click_selects_a_tab_and_state_reports_it() {
        let mut rig = Rig::new();
        let v = rig.run(|c| {
            call(c, "click", tab_target("Register \u{b7} Operating Checking")).unwrap();
            call(c, "state", json!({})).unwrap()
        });
        let tabs = v["tabs"].as_array().unwrap();
        let active: Vec<_> = tabs.iter().filter(|t| t["active"] == true).collect();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0]["title"], "Register \u{b7} Operating Checking");
    }

    #[test]
    fn keys_open_and_close_the_palette_and_text_reaches_it() {
        let mut rig = Rig::new();
        let (open, typed, closed) = rig.run(|c| {
            call(c, "key", json!({"key": "Ctrl+K"})).unwrap();
            let open = call(c, "state", json!({})).unwrap()["palette_open"].clone();
            call(c, "type", json!({"text": "bank"})).unwrap();
            let typed = call(c, "inspect", json!({"target": {"role": "text_input"}})).unwrap();
            call(c, "key", json!({"key": "Escape"})).unwrap();
            let closed = call(c, "state", json!({})).unwrap()["palette_open"].clone();
            (open, typed, closed)
        });
        assert_eq!(open, true);
        assert_eq!(closed, false);
        let values: Vec<_> = typed["matches"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["value"].as_str())
            .collect();
        assert!(values.contains(&"bank"), "{values:?}");
    }

    #[test]
    fn a_paste_reaches_the_focused_text_box() {
        let mut rig = Rig::new();
        let typed = rig.run(|c| {
            call(c, "key", json!({"key": "Ctrl+K"})).unwrap();
            call(c, "paste", json!({"text": "deposit"})).unwrap();
            let typed = call(c, "inspect", json!({"target": {"role": "text_input"}})).unwrap();
            call(c, "key", json!({"key": "Escape"})).unwrap();
            typed
        });
        let values: Vec<_> = typed["matches"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|m| m["value"].as_str())
            .collect();
        assert!(values.contains(&"deposit"), "{values:?}");
    }

    #[test]
    fn commands_theme_and_zoom() {
        let mut rig = Rig::new();
        let v = rig.run(|c| {
            call(c, "command", json!({"id": "help.shortcuts"})).unwrap();
            call(c, "set_theme", json!({"theme": "dark"})).unwrap();
            call(c, "set_zoom", json!({"percent": 200})).unwrap();
            call(c, "state", json!({})).unwrap()
        });
        assert_eq!(v["shortcuts_open"], true);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["zoom_percent"], 200);
    }

    #[test]
    fn theme_and_zoom_from_a_script_are_not_saved() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::prefs::UiStore::new(dir.path().to_path_buf());
        let before = store.load_prefs();
        let mut rig = Rig::with_options(ShellOptions {
            store: Some(crate::prefs::UiStore::new(dir.path().to_path_buf())),
            ..ShellOptions::default()
        });
        let v = rig.run(|c| {
            call(c, "set_theme", json!({"theme": "dark"})).unwrap();
            call(c, "set_zoom", json!({"percent": 150})).unwrap();
            call(c, "state", json!({})).unwrap()
        });
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["zoom_percent"], 150);
        let after = crate::prefs::UiStore::new(dir.path().to_path_buf()).load_prefs();
        assert_eq!(after.theme, before.theme);
        assert_eq!(after.zoom_percent, before.zoom_percent);
    }

    #[test]
    fn focus_moves_to_the_goto_box() {
        let mut rig = Rig::new();
        let v = rig.run(|c| {
            let t = json!({"target": {"role": "button", "label": "Go to a screen or command"}});
            call(c, "focus", t.clone()).unwrap();
            call(c, "inspect", t).unwrap()
        });
        assert_eq!(v["matches"][0]["focused"], true, "{v}");
    }

    #[test]
    fn bad_requests_get_typed_errors() {
        let mut rig = Rig::new();
        let codes = rig.run(|c| {
            vec![
                err_code(call(c, "click", json!({"target": {"label": "Nope"}}))),
                err_code(call(c, "click", json!({"target": {"role": "tab"}}))),
                err_code(call(c, "command", json!({"id": "no.such"}))),
                err_code(call(c, "key", json!({"key": "Hyper+Q"}))),
                err_code(call(c, "set_zoom", json!({"percent": 500}))),
                err_code(call(c, "set_theme", json!({"theme": "blue"}))),
                err_code(call(c, "screenshot", json!({"path": "x.txt"}))),
                err_code(call(c, "click", json!({"oops": 1}))),
            ]
        });
        assert_eq!(
            codes,
            vec![
                ErrorCode::NotFound,
                ErrorCode::Ambiguous,
                ErrorCode::NotFound,
                ErrorCode::BadRequest,
                ErrorCode::BadRequest,
                ErrorCode::BadRequest,
                ErrorCode::BadRequest,
                ErrorCode::BadRequest,
            ]
        );
    }

    #[test]
    fn quit_with_unsaved_work_prompts_and_discards_nothing() {
        let mut rig = Rig::new();
        let v = rig.run(|c| call(c, "quit", json!({})).unwrap());
        assert_eq!(v["quit"], false);
        assert_eq!(v["prompt_open"], true);
        assert!(!rig.shell.quit_approved());
        assert!(
            rig.shell
                .windows
                .all()
                .iter()
                .any(crate::windows::Window::dirty)
        );
    }

    #[test]
    fn every_tool_in_the_table_is_handled_by_the_app() {
        let mut rig = Rig::new();
        let names: Vec<&'static str> = cl_automation::tools::tools()
            .iter()
            .map(|t| t.name)
            .filter(|n| *n != "quit")
            .collect();
        let codes = rig.run(move |c| {
            names
                .iter()
                .map(|n| (*n, call(c, n, json!({}))))
                .map(|(n, r)| {
                    (
                        n,
                        r.err().map(|e| {
                            matches!(
                                e,
                                ClientError::Remote(ref b) if b.code == ErrorCode::UnknownMethod
                            )
                        }),
                    )
                })
                .collect::<Vec<_>>()
        });
        for (name, unknown) in codes {
            assert_ne!(
                unknown,
                Some(true),
                "{name} is in the tool table but the app does not handle it"
            );
        }
    }

    #[test]
    fn secret_values_never_leave_the_process() {
        let mut pw = accesskit::Node::new(accesskit::Role::PasswordInput);
        pw.set_label("Anything");
        pw.set_value("hunter2");
        let mut key = accesskit::Node::new(accesskit::Role::TextInput);
        key.set_label("AI key");
        key.set_value("sk-live-abc");
        let mut run = accesskit::Node::new(accesskit::Role::TextRun);
        run.set_value("sk-live-abc");
        key.set_children(vec![accesskit::NodeId(5)]);
        let mut memo = accesskit::Node::new(accesskit::Role::TextInput);
        memo.set_label("Memo");
        memo.set_value("lunch");
        let mut root = accesskit::Node::new(accesskit::Role::Window);
        root.set_children(vec![
            accesskit::NodeId(2),
            accesskit::NodeId(3),
            accesskit::NodeId(4),
        ]);
        let update = accesskit::TreeUpdate {
            nodes: vec![
                (accesskit::NodeId(1), root),
                (accesskit::NodeId(2), pw),
                (accesskit::NodeId(3), key),
                (accesskit::NodeId(4), memo),
                (accesskit::NodeId(5), run),
            ],
            tree: Some(accesskit::TreeInfo::new(accesskit::NodeId(1))),
            tree_id: accesskit::TreeId::ROOT,
            focus: accesskit::NodeId(1),
        };
        let w = WindowInfo {
            width: 100.0,
            height: 100.0,
            pixels_per_point: 1.0,
        };
        let snap = snapshot_from(&update, w).unwrap();
        let text = serde_json::to_string(&snap).unwrap();
        assert!(
            !text.contains("hunter2") && !text.contains("sk-live-abc"),
            "{text}"
        );
        assert!(text.contains("lunch"));
    }
}
