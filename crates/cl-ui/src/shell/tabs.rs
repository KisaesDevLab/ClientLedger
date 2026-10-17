//! The tab strip: one tab per open window, a dot on tabs with unsaved
//! changes, close buttons, a right-click menu to move a window out to its own
//! operating-system window (Q-57), and Ctrl+Tab cycling (through the keymap).

use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, Pos2, Rect, Role, Sense, Stroke, Ui, Vec2,
    WidgetInfo,
};

use super::Shell;
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::widgets::paint_dot;
use crate::windows::WindowId;

/// Tab strip height.
pub const HEIGHT: f32 = 36.0;
const PAD: f32 = 12.0;
const CLOSE: f32 = 16.0;

enum Action {
    Activate(WindowId),
    Close(WindowId),
    PopOut(WindowId),
    Dock(WindowId),
}

struct TabInfo {
    id: WindowId,
    title: String,
    dirty: bool,
    active: bool,
    popped: bool,
}

/// Draw the strip.
pub fn show(ui: &mut Ui, shell: &mut Shell, narrow: bool, tokens: &Tokens) {
    let active_id = shell.windows.active_id();
    let tabs: Vec<TabInfo> = shell
        .windows
        .all()
        .iter()
        .map(|w| TabInfo {
            id: w.id,
            title: w.title(),
            dirty: w.dirty(),
            active: active_id == Some(w.id),
            popped: w.popped_out,
        })
        .collect();
    let mut actions: Vec<Action> = Vec::new();
    let mut toggle_rail = false;
    egui::Panel::top("tabs")
        .exact_size(HEIGHT)
        .frame(
            Frame::new()
                .fill(tokens.surface_app)
                .inner_margin(Margin::symmetric(8, 0)),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if narrow && ui.button(t("shell.rail.toggle")).clicked() {
                    toggle_rail = true;
                }
                egui::ScrollArea::horizontal()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for info in &tabs {
                                tab(ui, tokens, info, &mut actions);
                            }
                        });
                    });
            });
        });
    if toggle_rail {
        shell.toggle_rail();
    }
    for a in actions {
        match a {
            Action::Activate(id) => shell.windows.activate(id),
            Action::Close(id) => {
                shell.close_window(id);
            }
            Action::PopOut(id) => shell.windows.pop_out(id),
            Action::Dock(id) => shell.windows.dock(id),
        }
    }
}

/// The accessible and tooltip text of a tab.
#[must_use]
pub fn tab_label(title: &str, dirty: bool) -> String {
    if dirty {
        t_with("tabs.label_dirty", &[("title", title)])
    } else {
        title.to_string()
    }
}

fn tab(ui: &mut Ui, tokens: &Tokens, info: &TabInfo, actions: &mut Vec<Action>) {
    let TabInfo {
        id,
        title,
        dirty,
        active,
        popped,
    } = info;
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = if *active {
        tokens.text_primary
    } else {
        tokens.text_secondary
    };
    let galley = ui.painter().layout_no_wrap(title.clone(), font, color);
    let dot_room = if *dirty { 14.0 } else { 0.0 };
    let width = PAD + galley.size().x + dot_room + 6.0 + CLOSE + 8.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, HEIGHT - 6.0), Sense::click());
    let label = tab_label(title, *dirty);
    resp.widget_info(|| WidgetInfo::selected(Role::Tab, true, *active, label.clone()));
    let resp = resp.on_hover_text(label);
    let hovered = resp.hovered();

    let fill = if *active {
        tokens.surface_window
    } else if hovered {
        tokens.surface_panel
    } else {
        Color32::TRANSPARENT
    };
    let top = CornerRadius {
        nw: 6,
        ne: 6,
        sw: 0,
        se: 0,
    };
    // Tabs sit on the bottom edge of the strip.
    let tab_rect = Rect::from_min_max(
        Pos2::new(rect.left(), rect.top() + 6.0),
        Pos2::new(rect.right(), rect.bottom() + 6.0),
    );
    ui.painter().rect_filled(tab_rect, top, fill);
    if *active {
        ui.painter().rect_stroke(
            tab_rect,
            top,
            Stroke::new(1.0, tokens.border_default),
            egui::StrokeKind::Inside,
        );
    }
    let text_pos = Pos2::new(
        tab_rect.left() + PAD,
        tab_rect.center().y - galley.size().y / 2.0,
    );
    let text_width = galley.size().x;
    ui.painter().galley(text_pos, galley, color);
    let mut x = tab_rect.left() + PAD + text_width;
    if *dirty {
        paint_dot(
            ui.painter(),
            Pos2::new(x + 9.0, tab_rect.center().y),
            4.0,
            tokens.text_primary,
        );
        x += dot_room;
    }
    let close_rect = Rect::from_center_size(
        Pos2::new(x + 6.0 + CLOSE / 2.0, tab_rect.center().y),
        Vec2::splat(CLOSE),
    );
    let close = ui.interact(close_rect, resp.id.with("close"), Sense::click());
    close.widget_info(|| {
        WidgetInfo::labeled(
            Role::Button,
            true,
            t_with("tabs.close", &[("title", title)]),
        )
    });
    if hovered || *active || close.hovered() {
        let c = if close.hovered() {
            tokens.accent
        } else {
            tokens.text_tertiary
        };
        let m = close_rect.center();
        let d = 3.5;
        let stroke = Stroke::new(1.5, c);
        ui.painter().line_segment(
            [Pos2::new(m.x - d, m.y - d), Pos2::new(m.x + d, m.y + d)],
            stroke,
        );
        ui.painter().line_segment(
            [Pos2::new(m.x - d, m.y + d), Pos2::new(m.x + d, m.y - d)],
            stroke,
        );
    }

    if close.clicked() || resp.middle_clicked() {
        actions.push(Action::Close(*id));
    } else if resp.clicked() {
        actions.push(Action::Activate(*id));
    }
    resp.context_menu(|ui| {
        if *popped {
            if ui.button(t("tabs.dock")).clicked() {
                actions.push(Action::Dock(*id));
                ui.close();
            }
        } else if ui.button(t("tabs.pop_out")).clicked() {
            actions.push(Action::PopOut(*id));
            ui.close();
        }
        if ui.button(t("tabs.close_menu")).clicked() {
            actions.push(Action::Close(*id));
            ui.close();
        }
    });
}
