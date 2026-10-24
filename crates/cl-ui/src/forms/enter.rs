//! Enter in a form's single-line header field (Q-190, Q-264).
//!
//! With the default preference Enter is Save & New. With "Enter moves
//! between fields" on, Enter goes to the next field like Tab, Shift+Enter
//! to the previous one like Shift+Tab, and Ctrl+Enter (a keymap binding the
//! shell adds) is Save & New. Every form header field calls [`entered`] (or
//! [`header_enter`]) once with its response, after it is drawn.
//!
//! egui moves focus forward within the frame (the field gave its focus up
//! on Enter, so the next field drawn takes it). Going back needs the field
//! to hold focus while egui's traversal passes it, so Shift+Enter keeps the
//! focus on the field for one frame and [`begin_frame`] (called by the
//! shell before the screens draw) asks egui to step back from it.

use cl_model::forms::{EnterAction, EnterContext, EnterMode, enter_action};
use eframe::egui::{self, Context, FocusDirection, Id, Response};

/// What Enter did in a header field this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderEnter {
    /// No Enter left this field.
    None,
    /// Enter means Save & New (the default preference, Q-190).
    SaveAndNew,
    /// Focus went to the next field (Enter moves between fields).
    MovedNext,
    /// Focus goes back to the previous field (Shift+Enter).
    MovedPrev,
}

impl HeaderEnter {
    /// The form should Save & New.
    #[must_use]
    pub fn saves(self) -> bool {
        self == HeaderEnter::SaveAndNew
    }
}

fn pending_id(viewport: egui::ViewportId) -> Id {
    Id::unique(("cl.enter.back", viewport))
}

/// Enter (or Shift+Enter) left the field `r` this frame: what it means in
/// `mode`, with the focus already moved in the Move mode.
#[must_use]
pub fn header_enter(r: &Response, mode: EnterMode) -> HeaderEnter {
    let ctx = &r.ctx;
    let shift = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key {
                key: egui::Key::Enter,
                pressed: true,
                modifiers,
                ..
            } => Some(modifiers.shift),
            _ => None,
        })
    });
    let Some(shift) = shift.filter(|_| r.lost_focus()) else {
        return HeaderEnter::None;
    };
    match enter_action(EnterContext::Header, mode, shift, false) {
        EnterAction::PrevField => {
            // Hold the focus here one frame; `begin_frame` steps back.
            ctx.memory_mut(|m| m.request_focus(r.id));
            let viewport = ctx.viewport_id();
            ctx.data_mut(|d| d.insert_temp(pending_id(viewport), Some(r.id)));
            ctx.request_repaint();
            HeaderEnter::MovedPrev
        }
        EnterAction::NextField | EnterAction::NextLine => {
            // The field gave its focus up: the next field drawn takes it.
            ctx.memory_mut(|m| m.move_focus(FocusDirection::Next));
            ctx.request_repaint();
            HeaderEnter::MovedNext
        }
        EnterAction::SaveAndNew | EnterAction::RecordRow => HeaderEnter::SaveAndNew,
    }
}

/// Enter in a one-field prompt inside a form (a void reason): it submits
/// the prompt in both Enter modes, like a dialog's default button.
#[must_use]
pub fn submitted(r: &Response) -> bool {
    r.lost_focus() && r.ctx.input(|i| i.key_pressed(egui::Key::Enter))
}

/// [`header_enter`] with the shell's current Enter preference.
#[must_use]
pub fn entered(r: &Response) -> HeaderEnter {
    header_enter(r, crate::grid_env::enter_mode(&r.ctx))
}

/// Before the screens of this viewport draw: a Shift+Enter of the last
/// frame moves the focus to the field before the one that held it.
pub fn begin_frame(ctx: &Context) {
    let key = pending_id(ctx.viewport_id());
    let Some(id) = ctx.data_mut(|d| d.remove_temp::<Option<Id>>(key)).flatten() else {
        return;
    };
    ctx.memory_mut(|m| {
        if m.focused() == Some(id) {
            m.move_focus(FocusDirection::Previous);
        }
    });
    ctx.request_repaint();
}

/// Frames for form tests: a context with the app's fonts and theme, and a
/// shell frame publishing the Enter mode the way the shell does.
#[cfg(test)]
pub(crate) mod testing {
    use cl_model::forms::EnterMode;
    use eframe::egui::{self, Event, Key, Modifiers, Pos2, RawInput, Rect, Ui, ViewportId, vec2};

    /// A context like the app's.
    pub fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx
    }

    /// One frame of `draw` with `events` under `mode`.
    pub fn frame(
        ctx: &egui::Context,
        mode: EnterMode,
        events: Vec<Event>,
        draw: &mut dyn FnMut(&mut Ui),
    ) {
        let prefs = crate::prefs::UiPrefs {
            enter_moves_fields: mode == EnterMode::MoveBetweenFields,
            ..crate::prefs::UiPrefs::default()
        };
        let shell = crate::grid_env::ShellFrame::new(crate::shell::today(), &prefs, false);
        crate::grid_env::publish(ctx, ViewportId::ROOT, shell);
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            super::begin_frame(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| draw(ui));
        });
        out.textures_delta.clear();
    }

    /// A few empty frames.
    pub fn settle(ctx: &egui::Context, mode: EnterMode, draw: &mut dyn FnMut(&mut Ui)) {
        for _ in 0..3 {
            frame(ctx, mode, vec![], draw);
        }
    }

    /// Enter pressed and released, with Shift or not.
    pub fn enter(shift: bool) -> Vec<Event> {
        let modifiers = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::NONE
        };
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, vec2};

    const FIELDS: [&str; 3] = ["a", "b", "c"];

    struct Form {
        text: [String; 3],
        seen: Vec<HeaderEnter>,
        ids: [Id; 3],
    }

    fn frame(ctx: &Context, f: &mut Form, mode: EnterMode, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            begin_frame(ui.ctx());
            egui::CentralPanel::default().show(ui, |ui| {
                for (i, name) in FIELDS.iter().enumerate() {
                    let r = ui.add(egui::TextEdit::singleline(&mut f.text[i]).id_salt(*name));
                    f.ids[i] = r.id;
                    let e = header_enter(&r, mode);
                    if e != HeaderEnter::None {
                        f.seen.push(e);
                    }
                }
            });
        });
        out.textures_delta.clear();
    }

    fn enter(shift: bool) -> Vec<Event> {
        let modifiers = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::NONE
        };
        vec![
            Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
            Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
        ]
    }

    fn setup(mode: EnterMode) -> (Context, Form) {
        let ctx = Context::default();
        let mut f = Form {
            text: Default::default(),
            seen: Vec::new(),
            ids: [Id::NULL; 3],
        };
        frame(&ctx, &mut f, mode, vec![]);
        ctx.memory_mut(|m| m.request_focus(f.ids[1]));
        frame(&ctx, &mut f, mode, vec![]);
        assert_eq!(ctx.memory(egui::Memory::focused), Some(f.ids[1]));
        (ctx, f)
    }

    #[test]
    fn enter_saves_by_default() {
        let mode = EnterMode::SaveAndNew;
        let (ctx, mut f) = setup(mode);
        frame(&ctx, &mut f, mode, enter(false));
        assert_eq!(f.seen, vec![HeaderEnter::SaveAndNew]);
        assert!(f.seen[0].saves());
        frame(&ctx, &mut f, mode, vec![]);
        assert_ne!(ctx.memory(egui::Memory::focused), Some(f.ids[2]), "no move");
    }

    #[test]
    fn enter_moves_to_the_next_field_when_the_option_is_on() {
        let mode = EnterMode::MoveBetweenFields;
        let (ctx, mut f) = setup(mode);
        frame(&ctx, &mut f, mode, enter(false));
        assert_eq!(f.seen, vec![HeaderEnter::MovedNext]);
        assert!(!f.seen[0].saves());
        frame(&ctx, &mut f, mode, vec![]);
        assert_eq!(ctx.memory(egui::Memory::focused), Some(f.ids[2]));
    }

    #[test]
    fn shift_enter_moves_back_when_the_option_is_on() {
        let mode = EnterMode::MoveBetweenFields;
        let (ctx, mut f) = setup(mode);
        frame(&ctx, &mut f, mode, enter(true));
        assert_eq!(f.seen, vec![HeaderEnter::MovedPrev]);
        for _ in 0..3 {
            frame(&ctx, &mut f, mode, vec![]);
        }
        assert_eq!(ctx.memory(egui::Memory::focused), Some(f.ids[0]));
        assert_eq!(f.seen.len(), 1, "the step back is not another Enter");
    }

    #[test]
    fn nothing_without_enter() {
        let mode = EnterMode::MoveBetweenFields;
        let (ctx, mut f) = setup(mode);
        frame(&ctx, &mut f, mode, vec![Event::Text("x".into())]);
        assert_eq!(f.seen, Vec::<HeaderEnter>::new());
        assert_eq!(f.text[1], "x");
    }
}
