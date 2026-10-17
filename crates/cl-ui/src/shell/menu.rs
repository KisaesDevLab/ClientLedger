//! The menu bar. Windows and Linux draw it in the window (Q-70); the native
//! macOS menu bar is deferred (Q-P06b). Every item comes from the command
//! table, so labels and shortcut text match the keymap by construction.

use cl_model::{CommandId, Menu, commands};
use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Stroke, Ui};

use super::{Shell, chip_text};
use crate::i18n::t;
use crate::input;
use crate::theme::Tokens;
use crate::widgets::kbd;

/// Menu bar height.
pub const HEIGHT: f32 = 32.0;

/// i18n key for a menu title.
#[must_use]
pub fn title_key(m: Menu) -> String {
    format!("menu.{}", m.title().to_ascii_lowercase())
}

/// Draw the menu bar and its right-hand "Go to" box.
pub fn show(ui: &mut Ui, shell: &mut Shell, narrow: bool, tokens: &Tokens) {
    let mut queued: Vec<CommandId> = Vec::new();
    egui::Panel::top("menu_bar")
        .exact_size(HEIGHT)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(tokens.surface_panel)
                .stroke(Stroke::NONE)
                .inner_margin(Margin::symmetric(8, 0)),
        )
        .show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                if narrow {
                    ui.menu_button(t("menu.menu"), |ui| {
                        for m in Menu::ALL {
                            ui.menu_button(t(&title_key(*m)), |ui| items(ui, *m, &mut queued));
                        }
                    });
                } else {
                    for m in Menu::ALL {
                        ui.menu_button(t(&title_key(*m)), |ui| items(ui, *m, &mut queued));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    go_to_box(ui, &mut queued, narrow, tokens);
                });
            });
        });
    for id in queued {
        shell.queue(id);
    }
}

fn go_to_box(ui: &mut Ui, queued: &mut Vec<CommandId>, narrow: bool, tokens: &Tokens) {
    let palette = commands::find("help.palette");
    if let Some(s) = palette.and_then(|c| c.shortcut) {
        kbd(ui, &chip_text(s));
    }
    let width = if narrow { 120.0 } else { 280.0 };
    let label = if narrow {
        t("shell.go_to")
    } else {
        t("shell.go_to_hint")
    };
    let response = ui.add_sized(
        [width, 22.0],
        egui::Button::new(RichText::new(label.clone()).color(tokens.text_placeholder))
            .fill(tokens.surface_window)
            .stroke(Stroke::new(1.0, tokens.border_strong)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::Role::Button, true, t("shell.go_to_label"))
    });
    if response.clicked() {
        queued.push(CommandId::new("help.palette"));
    }
    if !narrow {
        ui.label(RichText::new(t("shell.go_to")).color(tokens.text_secondary));
    }
}

fn items(ui: &mut Ui, menu: Menu, queued: &mut Vec<CommandId>) {
    let mut last_group: Option<u8> = None;
    for c in commands::in_menu(menu) {
        if last_group.is_some_and(|g| g != c.group) {
            ui.separator();
        }
        last_group = Some(c.group);
        let mut button = egui::Button::new(t(&format!("cmd.{}", c.id)));
        if let Some(s) = c.shortcut {
            button = button.shortcut_text(input::shortcut_text(s));
        }
        if ui.add(button).clicked() {
            queued.push(c.id);
            ui.close();
        }
    }
}
