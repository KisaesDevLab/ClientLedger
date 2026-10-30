//! List fields in forms (Q-250): a text field whose entries come from a
//! list (payee, name). It behaves like the grid's list cells
//! (`cl_grid::view::chevron`):
//!
//! - a mouse click on the field opens its list at once, showing every
//!   entry until the owner types (typing then filters it);
//! - the chevron at the right end opens or closes it;
//! - Tab into the field never opens it (type to autofill, Ctrl+L for the
//!   list); Esc closes it without changing the value (the screen's
//!   `form.revert` closes the list first).
//!
//! The screen keeps the open flag (so the control channel can read it);
//! this module paints the chevron, reads the mouse and draws the list.

use cl_grid::view::chevron;
use eframe::egui::{self, Context, Id, Margin, Response, Ui, Vec2};

use crate::theme::Tokens;

/// The `TextEdit` margin of a list field: egui's usual padding, plus room
/// on the right so the text stays clear of the chevron.
#[must_use]
pub fn margin() -> Margin {
    // The chevron's width is a small whole number of points.
    #[allow(clippy::cast_possible_truncation)]
    let room = chevron::WIDTH as i8;
    Margin {
        left: 4,
        right: 4 + room,
        top: 2,
        bottom: 2,
    }
}

fn browse_id(field: Id) -> Id {
    field.with("list_field_browse")
}

/// The list was opened by a click and nothing was typed since: it shows
/// every entry.
#[must_use]
pub fn browsing(ctx: &Context, field: Id) -> bool {
    ctx.data(|d| d.get_temp::<bool>(browse_id(field)))
        .unwrap_or(false)
}

fn set_browsing(ctx: &Context, field: Id, on: bool) {
    ctx.data_mut(|d| d.insert_temp(browse_id(field), on));
}

/// Paint the chevron in `field` (drawn after it) and read this frame's
/// mouse: the list's new open state, or `None` to leave it. A pointer
/// click on the field opens a closed list; one on the chevron toggles it.
/// Keyboard focus and Enter or Space never open it.
#[must_use = "the click opens or closes the list"]
pub fn mouse(ui: &Ui, field: &Response, open: bool) -> Option<bool> {
    let ctx = ui.ctx();
    if field.changed() {
        set_browsing(ctx, field.id, false);
    }
    let tokens = Tokens::of(ctx);
    chevron::paint(ui.painter(), field.rect, tokens.text_secondary, open);
    let next = chevron::toggle(chevron::click(field, field.rect), open);
    match next {
        Some(true) => {
            set_browsing(ctx, field.id, true);
            field.request_focus();
        }
        Some(false) => set_browsing(ctx, field.id, false),
        None => {}
    }
    next
}

/// The text the list filters by: nothing while browsing (every entry),
/// else what was typed.
#[must_use]
pub fn query(ctx: &Context, field: Id, typed: &str) -> String {
    if browsing(ctx, field) {
        String::new()
    } else {
        typed.trim().to_string()
    }
}

/// The list under `field`: its entries in a scrolling list, the current
/// one (if any) shown selected; a click picks one (its index).
pub fn popup<'a>(
    ui: &Ui,
    field: &Response,
    labels: impl IntoIterator<Item = &'a str>,
    current: Option<usize>,
) -> Option<usize> {
    let mut picked = None;
    // Bring the current entry into view once, when the list opens or the
    // current entry changes; after that the owner scrolls freely (scrolling
    // to it every frame pulled the list straight back).
    let shown_id = field.id.with("list_shown_current");
    let ctx = ui.ctx();
    let frame = ctx.cumulative_frame_nr();
    // (current entry, last frame drawn, frame it opened or the current
    // entry changed). A few frames, because a new popup's first pass only
    // measures it.
    let last = ctx.data(|d| d.get_temp::<(Option<usize>, u64, u64)>(shown_id));
    let since = match last {
        Some((was, at, since)) if was == current && frame <= at + 1 => since,
        _ => frame,
    };
    let reveal = frame <= since + 2;
    ctx.data_mut(|d| d.insert_temp(shown_id, (current, frame, since)));
    egui::Area::new(field.id.with("list"))
        .order(egui::Order::Foreground)
        .fixed_pos(field.rect.left_bottom() + Vec2::new(0.0, 2.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(field.rect.width());
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (i, label) in labels.into_iter().enumerate() {
                            // Clickable, never focusable: Tab from the
                            // field goes on to the next field, not into
                            // the list (which closes as the field is left).
                            let r = ui.add(
                                egui::Button::selectable(current == Some(i), label)
                                    .sense(egui::Sense::CLICK),
                            );
                            if reveal && current == Some(i) && field.has_focus() {
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                picked = Some(i);
                            }
                        }
                    });
            });
        });
    picked
}

#[cfg(test)]
mod tests {
    use eframe::egui::{self, Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};

    /// A field with the list flag, drawn like a screen does.
    struct Field {
        text: String,
        open: bool,
        rect: Rect,
        focused: bool,
        id: egui::Id,
    }

    fn frame(ctx: &egui::Context, f: &mut Field, events: Vec<Event>) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.add(egui::TextEdit::singleline(&mut String::new()).id_salt("before"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut f.text)
                        .id_salt("payee")
                        .margin(super::margin()),
                );
                if let Some(open) = super::mouse(ui, &r, f.open) {
                    f.open = open;
                }
                // Esc closes the list and keeps the value (the screen's
                // `form.revert`).
                if f.open && ui.input(|i| i.key_pressed(Key::Escape)) {
                    f.open = false;
                }
                f.rect = r.rect;
                f.id = r.id;
                f.focused = r.has_focus();
            });
        });
        out.textures_delta.clear();
    }

    fn click(ctx: &egui::Context, f: &mut Field, p: Pos2) {
        let button = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(ctx, f, vec![Event::PointerMoved(p), button(true)]);
        frame(ctx, f, vec![button(false)]);
        frame(ctx, f, vec![]);
    }

    fn key(k: Key) -> Vec<Event> {
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key: k,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: Modifiers::NONE,
            })
            .collect()
    }

    #[test]
    fn a_click_opens_tab_does_not_esc_closes_and_the_chevron_toggles() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let mut f = Field {
            text: "Ozark Feed & Seed".into(),
            open: false,
            rect: Rect::NOTHING,
            focused: false,
            id: egui::Id::NULL,
        };
        frame(&ctx, &mut f, vec![]);
        // Tab into the field: focused, no list.
        frame(&ctx, &mut f, key(Key::Tab));
        frame(&ctx, &mut f, key(Key::Tab));
        frame(&ctx, &mut f, vec![]);
        assert!(f.focused, "Tab reached the field");
        assert!(!f.open, "Tab never opens the list");
        // A click on the text opens it, showing every entry.
        let text_at = f.rect.left_center() + vec2(20.0, 0.0);
        click(&ctx, &mut f, text_at);
        assert!(f.open, "a click opens the list");
        assert_eq!(super::query(&ctx, f.id, &f.text), "", "every entry");
        // Esc closes it; the value is unchanged.
        frame(&ctx, &mut f, key(Key::Escape));
        assert!(!f.open);
        assert_eq!(f.text, "Ozark Feed & Seed");
        // The chevron opens and closes it.
        let chevron = cl_grid::view::chevron::area(f.rect).center();
        click(&ctx, &mut f, chevron);
        assert!(f.open, "the chevron opens the list");
        click(&ctx, &mut f, chevron);
        assert!(!f.open, "the chevron closes the list");
        assert_eq!(
            super::query(&ctx, f.id, &f.text),
            f.text,
            "typed text filters"
        );
        assert!(f.focused, "focus stays in the field");
    }

    /// The list shows the current entry when it opens, then scrolls freely:
    /// the wheel takes it to the top and it stays there.
    #[test]
    fn the_list_scrolls_away_from_the_current_entry() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        let labels: Vec<String> = (0..100).map(|i| format!("Entry {i}")).collect();
        let field_rect = std::cell::Cell::new(Rect::NOTHING);
        // The y of `label`'s row and whether it is inside the list's window.
        let run = |events: Vec<Event>| -> Vec<(String, f64)> {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 600.0))),
                events,
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut String::new()).id_salt("payee"));
                    r.request_focus();
                    field_rect.set(r.rect);
                    let _ = super::popup(ui, &r, labels.iter().map(String::as_str), Some(80));
                });
            });
            out.textures_delta.clear();
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(_, n)| Some((n.label()?.to_string(), n.bounds()?.y0)))
                .collect()
        };
        let y_of = |nodes: &[(String, f64)], label: &str| {
            nodes.iter().find(|(l, _)| l == label).map(|(_, y)| *y)
        };
        let mut nodes = Vec::new();
        // Scrolling to the current entry is animated over a few frames.
        for _ in 0..60 {
            nodes = run(vec![]);
        }
        let list_top = f64::from(field_rect.get().bottom());
        let list_bottom = list_top + 260.0;
        let shown = |y: Option<f64>| y.is_some_and(|y| y >= list_top && y < list_bottom);
        assert!(
            shown(y_of(&nodes, "Entry 80")),
            "opens on the current entry"
        );
        assert!(!shown(y_of(&nodes, "Entry 0")), "the top is out of view");
        // Wheel up over the list, then let it settle.
        let over = field_rect.get().left_bottom() + vec2(30.0, 60.0);
        let wheel = Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 5000.0),
            modifiers: Modifiers::NONE,
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
        };
        run(vec![Event::PointerMoved(over), wheel]);
        for _ in 0..60 {
            nodes = run(vec![Event::PointerMoved(over)]);
        }
        assert!(
            shown(y_of(&nodes, "Entry 0")),
            "the wheel reached the top and it stays"
        );
    }

    /// Tab with the list open goes to the next field, never into the list
    /// (which closes as the field is left, taking the focus with it).
    #[test]
    fn tab_with_the_list_open_goes_to_the_next_field() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        let mut text = "Mid".to_string();
        let mut open = true;
        let mut focus_field = true;
        let mut next_focused = false;
        let run = |events: Vec<Event>,
                   text: &mut String,
                   open: &mut bool,
                   focus_field: &mut bool,
                   next_focused: &mut bool| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600.0, 400.0))),
                events,
                ..RawInput::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .id_salt("vendor")
                            .margin(super::margin()),
                    );
                    if std::mem::take(focus_field) {
                        r.request_focus();
                    }
                    if *open && (r.has_focus() || r.lost_focus()) {
                        let _ = super::popup(ui, &r, ["Midwest Equipment", "Midland Co"], None);
                    }
                    if r.lost_focus() {
                        *open = false;
                    }
                    let next =
                        ui.add(egui::TextEdit::singleline(&mut String::new()).id_salt("date"));
                    *next_focused = next.has_focus();
                });
            });
            out.textures_delta.clear();
        };
        run(
            vec![],
            &mut text,
            &mut open,
            &mut focus_field,
            &mut next_focused,
        );
        run(
            vec![],
            &mut text,
            &mut open,
            &mut focus_field,
            &mut next_focused,
        );
        run(
            key(Key::Tab),
            &mut text,
            &mut open,
            &mut focus_field,
            &mut next_focused,
        );
        run(
            vec![],
            &mut text,
            &mut open,
            &mut focus_field,
            &mut next_focused,
        );
        assert!(!open, "leaving the field closes the list");
        assert!(next_focused, "Tab went on to the next field");
    }
}
