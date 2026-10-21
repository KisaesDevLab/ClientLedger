//! The tab strip: one tab per open window, a dot on tabs with unsaved
//! changes, close buttons, a right-click menu to move a window out to its own
//! operating-system window (Q-57), and Ctrl+Tab cycling (through the keymap).
//!
//! When the tabs don't fit (960 wide, 200 % zoom) the strip scrolls, the
//! window in front is scrolled into view whenever it changes, and a »
//! button at the right lists the tabs out of view.

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
/// Space between tabs.
const GAP: f32 = 2.0;
/// Room kept at the right for the » button when the tabs overflow.
const MORE_WIDTH: f32 = 36.0;

/// The memory key of how many tabs are out of view (control-channel state).
#[must_use]
pub fn hidden_count_id() -> egui::Id {
    egui::Id::unique("tabs/hidden_count")
}

/// True when tabs of these widths don't fit in `room` points.
#[must_use]
pub fn overflowing(widths: &[f32], room: f32) -> bool {
    let gaps = GAP * widths.iter().skip(1).map(|_| 1.0).sum::<f32>();
    widths.iter().sum::<f32>() + gaps > room + 0.5
}

/// The tabs not wholly inside `visible` (left to right), given each tab's
/// left and right edge.
#[must_use]
pub fn out_of_view<T: Copy>(tabs: &[(T, f32, f32)], visible: (f32, f32)) -> Vec<T> {
    tabs.iter()
        .filter(|(_, l, r)| *l < visible.0 - 0.5 || *r > visible.1 + 0.5)
        .map(|(id, _, _)| *id)
        .collect()
}

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
    let last_active_id = egui::Id::unique("tabs/last_active");
    let last_active: Option<WindowId> = ui.data(|d| d.get_temp(last_active_id));
    let reveal = active_id.is_some() && last_active != active_id;
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
                let widths: Vec<f32> = tabs.iter().map(|i| tab_width(ui, i)).collect();
                let room = ui.available_width();
                let more = overflowing(&widths, room);
                let strip = if more { room - MORE_WIDTH } else { room };
                let mut placed: Vec<(WindowId, f32, f32)> = Vec::new();
                let mut visible = (0.0, 0.0);
                egui::ScrollArea::horizontal()
                    .id_salt("tabs/strip")
                    .max_width(strip.max(0.0))
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        visible = (ui.clip_rect().left(), ui.clip_rect().right());
                        ui.horizontal_centered(|ui| {
                            ui.spacing_mut().item_spacing.x = GAP;
                            for info in &tabs {
                                let r = tab(ui, tokens, info, &mut actions);
                                if info.active && reveal {
                                    r.scroll_to_me(Some(egui::Align::Center));
                                }
                                placed.push((info.id, r.rect.left(), r.rect.right()));
                            }
                        });
                    });
                let hidden = if more {
                    out_of_view(&placed, visible)
                } else {
                    Vec::new()
                };
                ui.data_mut(|d| d.insert_temp(hidden_count_id(), hidden.len()));
                if more {
                    more_menu(ui, tokens, &tabs, &hidden, &mut actions);
                }
            });
        });
    ui.data_mut(|d| d.insert_temp(last_active_id, active_id));
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

/// The » button: a menu of the tabs out of view (a click brings one to
/// the front, and the strip scrolls to it).
fn more_menu(
    ui: &mut Ui,
    tokens: &Tokens,
    tabs: &[TabInfo],
    hidden: &[WindowId],
    actions: &mut Vec<Action>,
) {
    let r = ui
        .menu_button(
            egui::RichText::new("\u{bb}").color(tokens.text_secondary),
            |ui| {
                for info in tabs.iter().filter(|i| hidden.contains(&i.id)) {
                    let text = tab_label(&info.title, info.dirty);
                    if ui
                        .add(egui::Button::selectable(info.active, text))
                        .clicked()
                    {
                        actions.push(Action::Activate(info.id));
                        ui.close();
                    }
                }
            },
        )
        .response;
    let label = t_with("tabs.more", &[("count", &hidden.len().to_string())]);
    r.widget_info(|| WidgetInfo::labeled(Role::Button, true, label.clone()));
    r.on_hover_text(label);
}

/// A tab's width: its title, the unsaved dot and the close button.
fn tab_width(ui: &Ui, info: &TabInfo) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(info.title.clone(), font, Color32::PLACEHOLDER);
    let dot_room = if info.dirty { 14.0 } else { 0.0 };
    PAD + galley.size().x + dot_room + 6.0 + CLOSE + 8.0
}

fn tab(ui: &mut Ui, tokens: &Tokens, info: &TabInfo, actions: &mut Vec<Action>) -> egui::Response {
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
    let width = tab_width(ui, info);
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
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> u64 {
        n
    }

    #[test]
    fn tabs_overflow_only_past_the_room() {
        assert!(!overflowing(&[100.0, 100.0], 202.0));
        assert!(overflowing(&[100.0, 100.0], 201.0));
        assert!(!overflowing(&[], 0.0));
    }

    #[test]
    fn the_tabs_out_of_view_are_listed_in_order() {
        let tabs = [
            (id(1), 0.0, 100.0),
            (id(2), 102.0, 300.0),
            (id(3), 302.0, 500.0),
        ];
        assert_eq!(out_of_view(&tabs, (0.0, 320.0)), vec![id(3)]);
        // Scrolled right: the first is out of view on the left.
        assert_eq!(out_of_view(&tabs, (150.0, 500.0)), vec![id(1), id(2)]);
        assert_eq!(out_of_view(&tabs, (0.0, 500.0)), Vec::<u64>::new());
    }
}
