//! egui events to [`GridInput`]s (P10 plan Rev 2 §2.4.4, S3).
//!
//! Walks the frame's events in order and removes the ones the grid
//! handles, so the cursor cell's `TextEdit` (drawn later in the pass) never
//! sees a Tab, an Enter or a navigation arrow. Modifiers are matched
//! exactly (`consume_key` matches loosely, F8). Repeats of keys the shell
//! already acted on (Esc, Ctrl+Insert/Delete, Ctrl+Z/Y) are dropped; Page
//! Up/Down repeats page the grid.

use cl_model::amount_entry::single_paste;
use cl_model::date_rules::DateKey;
use egui::{Event, InputState, Key, Modifiers};

use crate::core::keys::{GridInput, GridKey, Mods};
use crate::core::model::{CellKind, CellProblem};
use crate::core::spec::GridCommand;

/// What the collector needs to know about the cursor cell.
// Independent facts about the cursor cell, set together each event.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    /// The grid edits (line or fixed-row grid).
    pub editable_grid: bool,
    /// The cursor cell's kind.
    pub kind: Option<CellKind>,
    /// A text editor is open on it.
    pub editing: bool,
    /// The editor's text is empty.
    pub buffer_empty: bool,
    /// The grid toggles selection with Space.
    pub selects: bool,
    /// An IME composition is active (no date-key interception, R6).
    pub ime_active: bool,
}

/// Exact modifiers (Cmd counts as Ctrl).
#[must_use]
pub fn mods(m: Modifiers) -> Mods {
    Mods {
        ctrl: m.ctrl || m.command || m.mac_cmd,
        shift: m.shift,
        alt: m.alt,
    }
}

/// What a paste did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteOutcome {
    /// No paste this frame.
    None,
    /// A paste was refused (two values).
    Refused(CellProblem),
}

/// Per-frame collector state: walk events in order and call
/// [`Collector::event`] with fresh cursor info for each, because a Tab
/// earlier in the frame may have moved the cursor to a cell of another kind.
#[derive(Debug, Default)]
pub struct Collector {
    swallow_space_text: bool,
    /// What happened to a paste this frame.
    pub paste: Option<CellProblem>,
}

impl Collector {
    /// Map one event. Pushes grid inputs to `out`; returns the event when it
    /// stays for egui (the editor or other widgets).
    pub fn event(
        &mut self,
        event: &Event,
        info: CursorInfo,
        out: &mut Vec<GridInput>,
    ) -> Option<Event> {
        let mut paste = PasteOutcome::None;
        let keep = map(event, info, out, &mut paste, &mut self.swallow_space_text);
        if let PasteOutcome::Refused(p) = paste {
            self.paste = Some(p);
        }
        keep
    }
}

/// Collect and consume all of `input`'s grid events with one cursor info
/// (tests and simple callers).
pub fn collect(input: &mut InputState, info: CursorInfo) -> (Vec<GridInput>, PasteOutcome) {
    let mut c = Collector::default();
    let mut out = Vec::new();
    let mut kept = Vec::with_capacity(input.events.len());
    for event in std::mem::take(&mut input.events) {
        if let Some(keep) = c.event(&event, info, &mut out) {
            kept.push(keep);
        }
    }
    input.events = kept;
    let paste = c.paste.map_or(PasteOutcome::None, PasteOutcome::Refused);
    (out, paste)
}

/// One event: push inputs, return the event if it stays for egui.
fn map(
    event: &Event,
    info: CursorInfo,
    out: &mut Vec<GridInput>,
    paste: &mut PasteOutcome,
    swallow_space_text: &mut bool,
) -> Option<Event> {
    let keep = Some(event.clone());
    match event {
        Event::Key {
            key,
            pressed: true,
            repeat,
            modifiers,
            ..
        } => {
            let m = mods(*modifiers);
            if *repeat
                && (*key == Key::Escape
                    || (m.ctrl && matches!(key, Key::Insert | Key::Delete | Key::Z | Key::Y)))
            {
                return None; // the shell acted on the first press
            }
            let grid_key = match (key, m) {
                (Key::Tab, Mods::NONE | Mods::SHIFT) => Some(GridKey::Tab),
                (Key::Enter, Mods::NONE | Mods::SHIFT) => Some(GridKey::Enter),
                (Key::Escape, Mods::NONE) => Some(GridKey::Escape),
                (Key::ArrowUp, Mods::NONE) => Some(GridKey::Up),
                (Key::ArrowDown, Mods::NONE) => Some(GridKey::Down),
                (Key::ArrowUp, Mods::SHIFT) if !info.editable_grid => Some(GridKey::Up),
                (Key::ArrowDown, Mods::SHIFT) if !info.editable_grid => Some(GridKey::Down),
                (Key::ArrowLeft, Mods::NONE) if !info.editing => Some(GridKey::Left),
                (Key::ArrowRight, Mods::NONE) if !info.editing => Some(GridKey::Right),
                (Key::Home, Mods::CTRL | Mods::CTRL_SHIFT) => Some(GridKey::Home),
                (Key::End, Mods::CTRL | Mods::CTRL_SHIFT) => Some(GridKey::End),
                (Key::PageUp, Mods::NONE) => Some(GridKey::PageUp),
                (Key::PageDown, Mods::NONE) => Some(GridKey::PageDown),
                (Key::Space, Mods::NONE)
                    if info.kind == Some(CellKind::Checkbox) || (!info.editing && info.selects) =>
                {
                    *swallow_space_text = true;
                    Some(GridKey::Space)
                }
                (Key::Backspace, Mods::NONE)
                    if info.buffer_empty && matches!(info.kind, Some(CellKind::Tags { .. })) =>
                {
                    Some(GridKey::Backspace)
                }
                _ => None,
            };
            let Some(k) = grid_key else { return keep };
            // Shift+Enter is a newline only in a multiline memo (step 13a).
            let m = if k == GridKey::Enter { Mods::NONE } else { m };
            out.push(GridInput::Key(k, m));
            None
        }
        Event::Text(t) => {
            if *swallow_space_text && t == " " {
                *swallow_space_text = false;
                return None;
            }
            if !info.editable_grid {
                out.push(GridInput::Text(t.clone()));
                return None;
            }
            if !info.editing {
                // Locked or read-only cell: the core reports the attempt.
                out.push(GridInput::Text(t.clone()));
                return None;
            }
            let mut chars = t.chars();
            let single = matches!((chars.next(), chars.next()), (Some(_), None));
            if info.kind == Some(CellKind::Date)
                && single
                && !info.ime_active
                && t.chars().next().and_then(DateKey::from_char).is_some()
            {
                out.push(GridInput::Text(t.clone()));
                return None;
            }
            keep
        }
        Event::Paste(text)
            if info.editing && matches!(info.kind, Some(CellKind::Amount(_) | CellKind::Date)) =>
        {
            match single_paste(text) {
                Ok(one) => Some(Event::Paste(one.to_string())),
                Err(e) => {
                    *paste = PasteOutcome::Refused(CellProblem::new(e.key()));
                    None
                }
            }
        }
        Event::Copy if !info.editing => {
            out.push(GridInput::Command(GridCommand::Copy));
            None
        }
        _ => keep,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_model::amount_entry::AmountRules;

    fn info(kind: Option<CellKind>, editing: bool) -> CursorInfo {
        CursorInfo {
            editable_grid: true,
            kind,
            editing,
            buffer_empty: false,
            selects: false,
            ime_active: false,
        }
    }

    fn press(key: Key, modifiers: Modifiers, repeat: bool) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat,
            modifiers,
        }
    }

    fn run(events: Vec<Event>, info: CursorInfo) -> (Vec<GridInput>, Vec<Event>, PasteOutcome) {
        let mut i = InputState::default();
        i.events = events;
        let (out, paste) = collect(&mut i, info);
        (out, i.events, paste)
    }

    #[test]
    fn navigation_keys_are_taken_and_text_is_left_for_the_editor() {
        let (out, left, _) = run(
            vec![
                Event::Text("8".into()),
                press(Key::Tab, Modifiers::NONE, false),
                press(Key::Tab, Modifiers::SHIFT, false),
                press(Key::Tab, Modifiers::ALT, false),
                press(Key::ArrowLeft, Modifiers::NONE, false),
            ],
            info(Some(CellKind::Amount(AmountRules::MONEY)), true),
        );
        assert_eq!(
            out,
            vec![
                GridInput::Key(GridKey::Tab, Mods::NONE),
                GridInput::Key(GridKey::Tab, Mods::SHIFT)
            ]
        );
        assert_eq!(
            left.len(),
            3,
            "text, Alt+Tab (exact match) and the caret arrow stay"
        );
    }

    #[test]
    fn repeats_of_shell_keys_are_dropped_and_page_repeats_page() {
        let (out, left, _) = run(
            vec![
                press(Key::Escape, Modifiers::NONE, true),
                press(Key::Delete, Modifiers::CTRL, true),
                press(Key::PageDown, Modifiers::NONE, true),
            ],
            info(Some(CellKind::Date), true),
        );
        assert_eq!(out, vec![GridInput::Key(GridKey::PageDown, Mods::NONE)]);
        assert_eq!(left, Vec::<Event>::new());
    }

    #[test]
    fn space_key_and_text_are_one_toggle() {
        let (out, left, _) = run(
            vec![
                press(Key::Space, Modifiers::NONE, false),
                Event::Text(" ".into()),
            ],
            info(Some(CellKind::Checkbox), false),
        );
        assert_eq!(out, vec![GridInput::Key(GridKey::Space, Mods::NONE)]);
        assert_eq!(left, Vec::<Event>::new(), "no stray Text(\" \")");
    }

    #[test]
    fn date_keys_and_pastes() {
        let (out, left, _) = run(
            vec![Event::Text("t".into()), Event::Text("1".into())],
            info(Some(CellKind::Date), true),
        );
        assert_eq!(out, vec![GridInput::Text("t".into())]);
        assert_eq!(left, vec![Event::Text("1".into())]);
        let mut ime = info(Some(CellKind::Date), true);
        ime.ime_active = true;
        let (out, _, _) = run(vec![Event::Text("t".into())], ime);
        assert_eq!(
            out,
            Vec::<GridInput>::new(),
            "no interception during IME composition"
        );
        let amount = info(Some(CellKind::Amount(AmountRules::MONEY)), true);
        let (_, left, p) = run(vec![Event::Paste("12.50\r\n".into())], amount);
        assert_eq!(left, vec![Event::Paste("12.50".into())]);
        assert_eq!(p, PasteOutcome::None);
        let (_, left, p) = run(vec![Event::Paste("1\t2".into())], amount);
        assert_eq!(left, Vec::<Event>::new());
        assert_eq!(
            p,
            PasteOutcome::Refused(CellProblem::new(cl_model::TextKey("grid.paste.one_value")))
        );
    }

    #[test]
    fn browse_takes_text_and_copy() {
        let browse = CursorInfo {
            editable_grid: false,
            kind: Some(CellKind::Display),
            editing: false,
            buffer_empty: true,
            selects: true,
            ime_active: false,
        };
        let (out, left, _) = run(
            vec![
                Event::Text("j".into()),
                Event::Copy,
                press(Key::ArrowDown, Modifiers::SHIFT, false),
                press(Key::ArrowRight, Modifiers::NONE, false),
            ],
            browse,
        );
        assert_eq!(
            out,
            vec![
                GridInput::Text("j".into()),
                GridInput::Command(GridCommand::Copy),
                GridInput::Key(GridKey::Down, Mods::SHIFT),
                GridInput::Key(GridKey::Right, Mods::NONE),
            ]
        );
        assert_eq!(left, Vec::<Event>::new());
    }
}
