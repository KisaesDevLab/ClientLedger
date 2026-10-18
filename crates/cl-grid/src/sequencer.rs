//! The input sequencer (P10 plan Rev 2 §2.4.2-2.4.3, B2, Q-P10p).
//!
//! egui hands a frame's input to everyone at once: the shell resolves
//! shortcuts first, then the grid handles navigation keys, then the focused
//! `TextEdit` takes all the text. When several keystrokes arrive in one frame
//! (fast typing, remote desktop, a barcode wedge, the control channel), text
//! typed *after* a Tab would land in the cell *before* it, and `1250` then
//! Alt+S would save before the amount was committed.
//!
//! The sequencer is an egui [`Plugin`] that splits each frame's events so a
//! frame holds **at most one boundary event, and it comes first**. The rest
//! wait for the next frame (a repaint is requested). Boundaries are
//! navigation keys, shortcuts, clipboard events, pointer presses and
//! AccessKit requests. Key releases and pointer moves travel with their
//! neighbours. State is kept per viewport.
//!
//! It also holds Tab, the arrows and Esc for one frame after a focus-lock
//! widget gains focus ([`hold_navigation`]): egui applies a widget's
//! focus-lock filter only from the frame after it was focused, so a Tab in
//! that frame would otherwise run egui's own focus traversal.
//!
//! Install order matters: plugins run in insertion order, and the automation
//! bridge injects events in its own hook, so the sequencer is installed
//! **after** the bridge (`cl_ui::app`).

use std::collections::{HashMap, VecDeque};

use egui::{Context, Event, Id, Key, Modifiers, Plugin, RawInput, ViewportId};

/// The plugin. Install with [`install`].
#[derive(Default)]
pub struct Sequencer {
    stash: HashMap<ViewportId, VecDeque<Event>>,
}

fn hold_id(viewport: ViewportId) -> Id {
    Id::unique(("cl_grid.sequencer.hold", viewport))
}

fn pending_id(viewport: ViewportId) -> Id {
    Id::unique(("cl_grid.sequencer.pending", viewport))
}

fn installed_id() -> Id {
    Id::unique("cl_grid.sequencer.installed")
}

/// Add the sequencer to `ctx` (a second call does nothing).
pub fn install(ctx: &Context) {
    ctx.add_plugin(Sequencer::default());
}

/// True once [`install`] ran and the sequencer saw a frame.
#[must_use]
pub fn is_installed(ctx: &Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(installed_id()).unwrap_or(false))
}

/// Events are still waiting for a later frame in this viewport. The
/// automation bridge does not answer a job until this is false.
#[must_use]
pub fn pending(ctx: &Context, viewport: ViewportId) -> bool {
    ctx.data(|d| d.get_temp::<bool>(pending_id(viewport)).unwrap_or(false))
}

/// A focus-lock widget gained focus this pass: hold the next Tab, arrow or
/// Esc press for one frame so the widget's filter is in force when it
/// arrives (§2.4.3).
pub fn hold_navigation(ctx: &Context, viewport: ViewportId) {
    ctx.data_mut(|d| d.insert_temp(hold_id(viewport), true));
}

fn any_command(m: Modifiers) -> bool {
    m.ctrl || m.command || m.mac_cmd || m.alt
}

/// Does this event end the frame's "before" part?
#[must_use]
pub fn is_boundary(e: &Event) -> bool {
    match e {
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => {
            any_command(*modifiers)
                || matches!(
                    key,
                    Key::Tab
                        | Key::Enter
                        | Key::Escape
                        | Key::ArrowUp
                        | Key::ArrowDown
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::PageUp
                        | Key::PageDown
                )
                || (matches!(key, Key::Home | Key::End | Key::Insert | Key::Delete)
                    && modifiers.any())
        }
        Event::Copy
        | Event::Cut
        | Event::Paste(_)
        | Event::PointerButton { pressed: true, .. }
        | Event::AccessKitActionRequest(_) => true,
        _ => false,
    }
}

/// Events that never count as content: they ride along with their
/// neighbours.
fn travels(e: &Event) -> bool {
    matches!(
        e,
        Event::Key { pressed: false, .. }
            | Event::PointerMoved(_)
            | Event::MouseMoved(_)
            | Event::ModifiersChanged(_)
            | Event::PointerButton { pressed: false, .. }
    )
}

fn is_navigation_press(e: &Event) -> bool {
    matches!(
        e,
        Event::Key {
            key: Key::Tab
                | Key::Escape
                | Key::ArrowUp
                | Key::ArrowDown
                | Key::ArrowLeft
                | Key::ArrowRight,
            pressed: true,
            ..
        }
    )
}

/// Split `events` into this frame's part and the part that waits. With
/// `hold`, the first navigation press and everything after it wait.
#[must_use]
pub fn split(events: Vec<Event>, hold: bool) -> (Vec<Event>, Vec<Event>) {
    let mut cut = events.len();
    let mut seen_content = false;
    let mut seen_boundary = false;
    for (i, e) in events.iter().enumerate() {
        if hold && is_navigation_press(e) {
            cut = i;
            break;
        }
        if is_boundary(e) {
            if seen_content || seen_boundary {
                cut = i;
                break;
            }
            seen_boundary = true;
        } else if !travels(e) {
            seen_content = true;
        }
    }
    let mut now = events;
    let later = now.split_off(cut);
    (now, later)
}

impl Plugin for Sequencer {
    fn debug_name(&self) -> &'static str {
        "cl-grid-sequencer"
    }

    fn input_hook(&mut self, ctx: &Context, input: &mut RawInput) {
        let viewport = input.viewport_id;
        let hold = ctx.data_mut(|d| d.remove_temp::<bool>(hold_id(viewport)).unwrap_or(false));
        let mut events: Vec<Event> = self
            .stash
            .remove(&viewport)
            .map(Vec::from)
            .unwrap_or_default();
        events.append(&mut input.events);
        let (now, later) = split(events, hold);
        input.events = now;
        let waiting = !later.is_empty();
        if waiting {
            self.stash.insert(viewport, later.into());
            ctx.request_repaint_of(viewport);
        }
        ctx.data_mut(|d| {
            d.insert_temp(pending_id(viewport), waiting);
            d.insert_temp(installed_id(), true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: m,
        }
    }

    fn up(k: Key) -> Event {
        Event::Key {
            key: k,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    fn text(s: &str) -> Event {
        Event::Text(s.into())
    }

    fn frames(mut events: Vec<Event>) -> Vec<Vec<Event>> {
        let mut out = Vec::new();
        while !events.is_empty() {
            let (now, later) = split(events, false);
            assert!(!now.is_empty(), "a frame always makes progress");
            out.push(now);
            events = later;
        }
        out
    }

    #[test]
    fn text_then_tab_then_text_is_two_frames() {
        let f = frames(vec![
            text("842.15"),
            key(Key::Tab, Modifiers::NONE),
            up(Key::Tab),
            text("Perennials"),
            key(Key::Tab, Modifiers::NONE),
            text("Fall"),
        ]);
        assert_eq!(f.len(), 3);
        assert_eq!(f[0], vec![text("842.15")]);
        assert_eq!(
            f[1],
            vec![
                key(Key::Tab, Modifiers::NONE),
                up(Key::Tab),
                text("Perennials")
            ]
        );
        assert_eq!(f[2], vec![key(Key::Tab, Modifiers::NONE), text("Fall")]);
    }

    #[test]
    fn shortcut_after_text_waits() {
        let f = frames(vec![text("1250"), key(Key::S, Modifiers::ALT)]);
        assert_eq!(
            f,
            vec![vec![text("1250")], vec![key(Key::S, Modifiers::ALT)]]
        );
    }

    #[test]
    fn plain_text_and_moves_stay_together() {
        let evs = vec![
            Event::PointerMoved(egui::pos2(1.0, 1.0)),
            text("a"),
            key(Key::B, Modifiers::NONE),
            text("b"),
            key(Key::Home, Modifiers::NONE),
            key(Key::Delete, Modifiers::NONE),
        ];
        assert_eq!(frames(evs.clone()), vec![evs]);
    }

    #[test]
    fn boundaries() {
        assert!(is_boundary(&key(Key::Home, Modifiers::CTRL)));
        assert!(is_boundary(&key(Key::Delete, Modifiers::CTRL)));
        assert!(!is_boundary(&key(Key::Delete, Modifiers::NONE)));
        assert!(is_boundary(&key(Key::A, Modifiers::COMMAND)));
        assert!(is_boundary(&Event::Paste("x".into())));
        assert!(is_boundary(&Event::Copy));
        assert!(!is_boundary(&up(Key::Tab)));
        assert!(!is_boundary(&text("x")));
        // Two boundaries in a row: one per frame.
        let f = frames(vec![
            key(Key::Tab, Modifiers::NONE),
            key(Key::Tab, Modifiers::NONE),
            key(Key::ArrowDown, Modifiers::NONE),
        ]);
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn hold_defers_the_first_navigation_press() {
        let (now, later) = split(
            vec![text("x"), key(Key::Tab, Modifiers::NONE), text("y")],
            true,
        );
        assert_eq!(now, vec![text("x")]);
        assert_eq!(later.len(), 2);
        let (now, later) = split(vec![key(Key::Escape, Modifiers::NONE)], true);
        assert_eq!(now, Vec::<Event>::new(), "held entirely");
        assert_eq!(later.len(), 1);
        // Enter is not held (the focus filter never claims it).
        let (now, _) = split(vec![key(Key::Enter, Modifiers::NONE)], true);
        assert_eq!(now.len(), 1);
    }
}
