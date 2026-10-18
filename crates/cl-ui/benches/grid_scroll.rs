//! Grid scroll benchmark (P10 plan §6, S9): the real `Grid` with its
//! editors, header, scrollbar and AccessKit nodes, inside the demo screen
//! under the full `Shell` chrome, at 1440x940 with 200,000 register rows.
//!
//! 600 frames per run: 60% wheel, 20% Page Down, 10% Ctrl+Home / Ctrl+End
//! jumps, 10% near the end (Ctrl+End then Page Up). Each frame is timed in
//! wall time for one egui pass plus tessellation. Runs with AccessKit off
//! and on, and prints one line per run:
//!
//! `grid-bench accesskit=off frames=600 p50_us=.. p95_us=.. max_us=..`
//!
//! `cargo xtask grid-bench` builds this in release, parses the lines and
//! applies the budget. Lives in cl-ui rather than cl-grid because it needs
//! the shell, and cl-grid may not depend on cl-ui (crate layering rules).

use std::time::{Duration, Instant};

use cl_model::CommandId;
use cl_ui::shell::{Shell, ShellOptions};
use cl_ui::theme::ThemeChoice;
use eframe::egui::{
    self, Context, Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, PointerButton, Pos2,
    RawInput, Rect, TouchPhase, accesskit, vec2,
};

const FRAMES: usize = 600;
const SIZE: egui::Vec2 = vec2(1440.0, 940.0);

const CTRL: Modifiers = Modifiers {
    alt: false,
    ctrl: true,
    shift: false,
    mac_cmd: false,
    command: true,
};

struct Bench {
    ctx: Context,
    shell: Shell,
}

impl Bench {
    fn new(accesskit: bool) -> Bench {
        let ctx = Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        if accesskit {
            ctx.enable_accesskit();
        }
        let mut shell = Shell::new(ShellOptions {
            demo: true,
            theme: Some(ThemeChoice::Light),
            zoom_percent: Some(100),
            ..ShellOptions::default()
        });
        shell.set_run_only(true);
        shell.dispatch_now(CommandId::new("dev.grid_demo"));
        Bench { ctx, shell }
    }

    /// One frame; returns the wall time of the pass plus tessellation and
    /// the AccessKit nodes when asked.
    fn frame(&mut self, events: Vec<Event>) -> (Duration, Option<accesskit::TreeUpdate>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            events,
            ..RawInput::default()
        };
        let shell = &mut self.shell;
        let start = Instant::now();
        let mut out = self.ctx.run_ui(input, |ui| shell.ui(ui));
        let primitives = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let took = start.elapsed();
        std::hint::black_box(primitives);
        out.textures_delta.clear();
        (took, out.platform_output.accesskit_update)
    }

    fn find(&mut self, pred: impl Fn(&accesskit::Node) -> bool) -> Rect {
        for _ in 0..10 {
            if let (_, Some(update)) = self.frame(vec![])
                && let Some(r) = update
                    .nodes
                    .iter()
                    .find(|(_, n)| pred(n))
                    .and_then(|(_, n)| n.bounds())
            {
                #[allow(clippy::cast_possible_truncation)]
                return Rect::from_min_max(
                    Pos2::new(r.x0 as f32, r.y0 as f32),
                    Pos2::new(r.x1 as f32, r.y1 as f32),
                );
            }
        }
        panic!("the demo did not show the expected control");
    }

    fn click(&mut self, p: Pos2) {
        self.frame(vec![
            Event::PointerMoved(p),
            Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        self.frame(vec![Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        for _ in 0..4 {
            self.frame(vec![]);
        }
    }

    /// Open the register and put the cursor on row 1, clicking where
    /// `locate` found the controls (the layout is deterministic).
    fn setup(&mut self, show: Pos2, row: Pos2) {
        for _ in 0..4 {
            self.frame(vec![]);
        }
        self.click(show);
        self.click(row);
    }
}

/// Where the controls are, read from the AccessKit tree of a separate run
/// (the layout is deterministic).
#[derive(Clone, Copy)]
struct Places {
    show: Pos2,
    row: Pos2,
    /// The scrollbar's track.
    bar: Rect,
}

fn locate() -> Places {
    let mut b = Bench::new(true);
    let show = b.find(|n| n.label() == Some("Show 200,000 rows")).center();
    b.click(show);
    let row = b
        .find(|n| {
            n.role() == accesskit::Role::Row
                && n.label().is_some_and(|l| l.ends_with("row 1 of 200,000"))
        })
        .center();
    let bar = b.find(|n| n.role() == accesskit::Role::ScrollBar);
    Places { show, row, bar }
}

fn tap(k: Key, m: Modifiers) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: m,
        })
        .collect()
}

fn wheel(at: Pos2, lines: f32) -> Vec<Event> {
    vec![
        Event::PointerMoved(at),
        Event::MouseWheel {
            unit: MouseWheelUnit::Line,
            delta: vec2(0.0, -lines),
            modifiers: Modifiers::NONE,
            phase: TouchPhase::Move,
            source: MouseWheelSource::Wheel,
        },
    ]
}

/// A press and release on the scrollbar track: a drag-style jump.
fn jump(bar: Rect, fraction: f32) -> Vec<Event> {
    let p = Pos2::new(bar.center().x, bar.top() + bar.height() * fraction);
    vec![
        Event::PointerMoved(p),
        Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
        Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        },
    ]
}

/// A fixed, repeatable mix: 60% wheel, 20% Page Down, 10% scrollbar
/// jumps, 10% near the end.
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        // Knuth's MMIX LCG: deterministic, no dependency.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn input(&mut self, at: &Places) -> Vec<Event> {
        let r = self.next() % 100;
        match r {
            0..=59 => {
                // Mostly down, sometimes up, 1 to 12 lines.
                #[allow(clippy::cast_precision_loss)]
                let lines = (self.next() % 12 + 1) as f32;
                wheel(at.row, if r < 45 { lines } else { -lines })
            }
            60..=79 => tap(Key::PageDown, Modifiers::NONE),
            80..=89 => {
                #[allow(clippy::cast_precision_loss)]
                let f = (self.next() % 1000) as f32 / 1000.0;
                jump(at.bar, f)
            }
            _ => {
                let mut e = tap(Key::End, CTRL);
                e.extend(tap(Key::PageUp, Modifiers::NONE));
                e
            }
        }
    }
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let i = (sorted.len() * p).div_ceil(100).saturating_sub(1);
    sorted[i.min(sorted.len() - 1)]
}

fn run(accesskit: bool, at: Places) {
    let mut b = Bench::new(accesskit);
    b.setup(at.show, at.row);
    let mut mix = Mix(0x5eed);
    // Warm up: glyph atlas and caches.
    for _ in 0..30 {
        b.frame(mix.input(&at));
    }
    let mut tops = std::collections::BTreeSet::new();
    let mut times: Vec<Duration> = (0..FRAMES)
        .map(|_| {
            let took = b.frame(mix.input(&at)).0;
            tops.insert(b.shell.active_screen_state()["top_row"].as_u64());
            took
        })
        .collect();
    // The run measured scrolling, not an idle screen.
    let state = b.shell.active_screen_state();
    assert_eq!(state["mode"], "register", "{state}");
    assert_eq!(state["focused"], true, "{state}");
    assert!(tops.len() > 200, "only {} distinct top rows", tops.len());
    times.sort();
    println!(
        "grid-bench accesskit={} frames={FRAMES} top_rows={} p50_us={} p95_us={} max_us={}",
        if accesskit { "on" } else { "off" },
        tops.len(),
        percentile(&times, 50).as_micros(),
        percentile(&times, 95).as_micros(),
        times[times.len() - 1].as_micros(),
    );
}

fn main() {
    // `cargo test --all-targets` runs bench binaries without `--bench`;
    // keep that a quick smoke run.
    let at = locate();
    if !std::env::args().any(|a| a == "--bench") {
        let mut b = Bench::new(true);
        b.setup(at.show, at.row);
        let mut mix = Mix(1);
        for _ in 0..20 {
            b.frame(mix.input(&at));
        }
        return;
    }
    run(false, at);
    run(true, at);
}
