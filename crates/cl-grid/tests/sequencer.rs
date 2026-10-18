//! The sequencer inside a real egui `Context` (P10 plan Rev 2 §2.4.2, T2
//! unit part): batched input reaches the app one boundary per frame, in
//! order, also when another plugin installed before it injects the events
//! (the automation bridge's position).

use std::sync::{Arc, Mutex};

use cl_grid::sequencer;
use egui::{Context, Event, Key, Modifiers, Plugin, RawInput, ViewportId};

/// Stands in for the automation bridge: injects a batch in its input hook.
struct Injector(Arc<Mutex<Vec<Event>>>);

impl Plugin for Injector {
    fn debug_name(&self) -> &'static str {
        "test-injector"
    }
    fn input_hook(&mut self, _ctx: &Context, input: &mut RawInput) {
        if let Ok(mut q) = self.0.lock() {
            input.events.append(&mut q);
        }
    }
}

fn tab() -> Event {
    Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn tab_up() -> Event {
    Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: false,
        repeat: false,
        modifiers: Modifiers::NONE,
    }
}

fn run(ctx: &Context, events: Vec<Event>) -> Vec<Event> {
    let mut seen = Vec::new();
    let mut out = ctx.run_ui(
        RawInput {
            events,
            ..RawInput::default()
        },
        |ui| seen = ui.input(|i| i.events.clone()),
    );
    out.textures_delta.clear();
    seen
}

#[test]
fn batches_are_split_after_an_injecting_plugin() {
    let ctx = Context::default();
    let queue = Arc::new(Mutex::new(Vec::new()));
    ctx.add_plugin(Injector(Arc::clone(&queue)));
    sequencer::install(&ctx);
    sequencer::install(&ctx); // idempotent
    run(&ctx, vec![]);
    assert!(sequencer::is_installed(&ctx));
    queue.lock().unwrap().extend([
        Event::Text("842.15".into()),
        tab(),
        tab_up(),
        Event::Text("Perennials".into()),
        tab(),
        tab_up(),
        Event::Text("Fall".into()),
        tab(),
        tab_up(),
    ]);
    let mut frames = Vec::new();
    for _ in 0..6 {
        let seen = run(&ctx, vec![]);
        if !seen.is_empty() {
            frames.push(seen);
        }
        if !sequencer::pending(&ctx, ViewportId::ROOT) {
            break;
        }
    }
    assert_eq!(
        frames,
        vec![
            vec![Event::Text("842.15".into())],
            vec![tab(), tab_up(), Event::Text("Perennials".into())],
            vec![tab(), tab_up(), Event::Text("Fall".into())],
            vec![tab(), tab_up()],
        ]
    );
    assert!(!sequencer::pending(&ctx, ViewportId::ROOT));
}

#[test]
fn new_input_waits_behind_the_stash() {
    let ctx = Context::default();
    sequencer::install(&ctx);
    let f1 = run(
        &ctx,
        vec![Event::Text("a".into()), tab(), Event::Text("b".into())],
    );
    assert_eq!(f1, vec![Event::Text("a".into())]);
    assert!(sequencer::pending(&ctx, ViewportId::ROOT));
    // Fresh input arriving meanwhile goes after the stashed events.
    let f2 = run(&ctx, vec![Event::Text("c".into())]);
    assert_eq!(
        f2,
        vec![tab(), Event::Text("b".into()), Event::Text("c".into())]
    );
}

#[test]
fn hold_navigation_defers_one_frame() {
    let ctx = Context::default();
    sequencer::install(&ctx);
    run(&ctx, vec![]);
    sequencer::hold_navigation(&ctx, ViewportId::ROOT);
    let f1 = run(&ctx, vec![tab()]);
    assert_eq!(f1, Vec::<Event>::new());
    let f2 = run(&ctx, vec![]);
    assert_eq!(f2, vec![tab()]);
}
