//! The collapsible detail panel (Q-266): a panel at the right of a list that
//! the owner can hide (the list takes the freed width) and show again, with
//! the control on the panel's edge and F9 (`window.toggle_details`, Q-P16n).
//! The choice is remembered per screen kind in
//! [`crate::prefs::PanelPrefsBook`].
//!
//! Collapsed, the panel leaves a 22-point strip with a chevron button named
//! "Show details"; expanded, the same strip holds "Hide details". While the
//! panel shows a form (New, Edit), the panel stays open and the control is
//! off with a tooltip.

use eframe::egui::{self, Rect, Sense, Stroke, Ui, UiBuilder, Vec2};

use crate::i18n::{t, t_with};

/// The edge strip's width.
pub const STRIP: f32 = 22.0;

/// Space between the list and the strip.
const GAP: f32 = 6.0;

/// The list's width when there is room for it beside the panel.
const MAIN_MIN: f32 = 320.0;

/// The panel's narrowest width while the list is above `MAIN_LEAST`.
const SIDE_MIN: f32 = 200.0;

/// The list never gets narrower than this.
const MAIN_LEAST: f32 = 120.0;

/// The panel's command.
pub const TOGGLE: &str = "window.toggle_details";

/// One screen's panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidePanel {
    /// The screen kind the choice is remembered under (`clients.center`).
    pub key: &'static str,
    /// The panel's width when shown.
    pub width: f32,
}

/// Which part [`with_side_panel`] asks the screen to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The list (takes the freed width when the panel is hidden).
    Main,
    /// The panel's content (only drawn when shown).
    Side,
}

/// What the owner did with the panel control this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    /// Nothing.
    None,
    /// The control was clicked; `collapsed` changed.
    Toggled,
}

/// The shortcut text of the toggle ("F9").
fn keys() -> String {
    cl_model::commands::find(TOGGLE)
        .and_then(|c| c.shortcut)
        .map(crate::input::shortcut_text)
        .unwrap_or_default()
}

/// Draws the list and the panel side by side in what is left of `ui`.
/// `collapsed` is the remembered choice; `forced_open` (a form in the panel)
/// shows the panel without changing it and turns the control off. `add` is
/// called with [`Part::Main`], then with [`Part::Side`] when the panel shows.
pub fn with_side_panel(
    ui: &mut Ui,
    panel: &SidePanel,
    collapsed: &mut bool,
    forced_open: bool,
    mut add: impl FnMut(&mut Ui, Part),
) -> PanelEvent {
    let full = ui.available_rect_before_wrap();
    let hidden = *collapsed && !forced_open;
    // The list keeps MAIN_MIN where it can; a narrow window shares the room
    // (the panel keeps SIDE_MIN, the list MAIN_LEAST).
    let room = full.width() - STRIP - GAP;
    let side_w = if hidden {
        0.0
    } else {
        panel
            .width
            .min((room - MAIN_MIN).max(SIDE_MIN))
            .min((room - MAIN_LEAST).max(0.0))
    };
    let main_right = (full.right() - STRIP - GAP - side_w).max(full.left());
    let main = Rect::from_min_max(full.min, egui::pos2(main_right, full.bottom()));
    let strip = Rect::from_min_max(
        egui::pos2(main_right + GAP, full.top()),
        egui::pos2(main_right + GAP + STRIP, full.bottom()),
    );
    let side = Rect::from_min_max(egui::pos2(strip.right(), full.top()), full.max);

    let mut part = |ui: &mut Ui, rect: Rect, salt: &str, which: Part| {
        let mut child = ui.new_child(
            UiBuilder::new()
                .id_salt(("cl.side_panel", panel.key, salt))
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.shrink_clip_rect(rect);
        add(&mut child, which);
    };
    part(ui, main, "main", Part::Main);

    let mut event = PanelEvent::None;
    let mut strip_ui = ui.new_child(
        UiBuilder::new()
            .id_salt(("cl.side_panel", panel.key, "strip"))
            .max_rect(strip)
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    let line = strip_ui.visuals().widgets.noninteractive.bg_stroke;
    strip_ui.painter().vline(
        strip.center().x,
        strip.y_range(),
        Stroke::new(1.0, line.color),
    );
    if toggle_button(&mut strip_ui, hidden, forced_open) {
        *collapsed = !hidden;
        event = PanelEvent::Toggled;
    }
    if !hidden {
        part(ui, side, "side", Part::Side);
    }
    ui.advance_cursor_after_rect(full);
    event
}

/// The chevron on the panel edge; true when clicked.
fn toggle_button(ui: &mut Ui, hidden: bool, locked: bool) -> bool {
    let (name, tip) = if hidden {
        ("panel.show_details", "panel.show_details.tip")
    } else {
        ("panel.hide_details", "panel.hide_details.tip")
    };
    let name = t(name);
    let size = Vec2::new(STRIP - 2.0, 30.0);
    let r = ui.add_enabled_ui(!locked, |ui| {
        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
        let visuals = ui.style().interact(&r);
        ui.painter().rect(
            rect,
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        // A chevron pointing where the panel goes: left to show, right to
        // hide.
        let c = rect.center();
        let dx = if hidden { 3.0 } else { -3.0 };
        let stroke = Stroke::new(1.5, visuals.fg_stroke.color);
        ui.painter().line_segment(
            [egui::pos2(c.x + dx, c.y - 5.0), egui::pos2(c.x - dx, c.y)],
            stroke,
        );
        ui.painter().line_segment(
            [egui::pos2(c.x - dx, c.y), egui::pos2(c.x + dx, c.y + 5.0)],
            stroke,
        );
        let enabled = ui.is_enabled();
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &name));
        r
    });
    let r = r.inner;
    let r = if locked {
        r.on_disabled_hover_text(t("panel.form_open.tip"))
    } else {
        r.on_hover_text(t_with(tip, &[("keys", &keys())]))
    };
    r.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(collapsed: &mut bool, forced: bool) -> (Vec<(Part, f32)>, PanelEvent) {
        let ctx = egui::Context::default();
        let mut seen = Vec::new();
        let mut event = PanelEvent::None;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1000.0, 600.0),
            )),
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            let p = SidePanel {
                key: "clients.center",
                width: 300.0,
            };
            event = with_side_panel(ui, &p, collapsed, forced, |ui, part| {
                seen.push((part, ui.available_width()));
            });
        });
        out.textures_delta.clear();
        (seen, event)
    }

    #[test]
    fn hidden_panel_gives_its_width_to_the_list() {
        let (open, _) = run(&mut false, false);
        assert_eq!(open.len(), 2);
        assert_eq!(open[1].0, Part::Side);
        let (closed, event) = run(&mut true, false);
        assert_eq!(event, PanelEvent::None);
        assert_eq!(closed.len(), 1, "the side is not drawn when hidden");
        assert_eq!(closed[0].0, Part::Main);
        assert!(
            closed[0].1 >= open[0].1 + 299.0,
            "{} vs {}",
            closed[0].1,
            open[0].1
        );
    }

    #[test]
    fn a_form_keeps_the_panel_open_without_changing_the_choice() {
        let mut collapsed = true;
        let (seen, _) = run(&mut collapsed, true);
        assert_eq!(seen.len(), 2);
        assert!(collapsed, "the remembered choice stays");
    }
}
