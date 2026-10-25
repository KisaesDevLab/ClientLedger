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
    // The box takes what the menu titles leave: full size when there is
    // room, then narrower, then without its "Go to" caption, so nothing is
    // drawn over the last menu title (960 x 640, the minimum window).
    let gap = ui.spacing().item_spacing.x;
    let caption = t("shell.go_to");
    let caption_width = ui
        .painter()
        .layout_no_wrap(
            caption.clone(),
            egui::TextStyle::Body.resolve(ui.style()),
            tokens.text_secondary,
        )
        .size()
        .x
        + gap;
    let room = ui.available_width() - gap;
    let full: f32 = if narrow { 120.0 } else { 280.0 };
    let with_caption = !narrow && room - caption_width >= MIN_GO_TO_WIDTH;
    let width = if with_caption {
        full.min(room - caption_width)
    } else {
        full.min(room).max(MIN_GO_TO_WIDTH.min(room.max(0.0)))
    };
    let label = if !narrow && width >= HINT_WIDTH {
        t("shell.go_to_hint")
    } else if with_caption {
        String::new()
    } else {
        caption.clone()
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
    if with_caption {
        ui.label(RichText::new(caption).color(tokens.text_secondary));
    }
}

/// The narrowest the "Go to" box gets before its caption is dropped.
const MIN_GO_TO_WIDTH: f32 = 160.0;
/// The width the box needs to show its hint text.
const HINT_WIDTH: f32 = 220.0;

/// One menu row as drawn: label, shortcut text, and whether a separator
/// comes before it (a new group).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    /// Command id.
    pub id: CommandId,
    /// Label from the catalog.
    pub label: String,
    /// Shortcut text, when the command has one.
    pub shortcut: Option<String>,
    /// A separator is drawn above this row.
    pub separator_before: bool,
}

/// The rows of a menu in table order (pure, so it is unit-tested).
#[must_use]
pub fn rows(menu: Menu) -> Vec<MenuRow> {
    let mut last_group: Option<u8> = None;
    commands::in_menu(menu)
        .map(|c| {
            let separator_before = last_group.is_some_and(|g| g != c.group);
            last_group = Some(c.group);
            MenuRow {
                id: c.id,
                label: t(&format!("cmd.{}", c.id)),
                shortcut: c.shortcut.map(input::shortcut_text),
                separator_before,
            }
        })
        .collect()
}

fn items(ui: &mut Ui, menu: Menu, queued: &mut Vec<CommandId>) {
    for row in rows(menu) {
        if row.separator_before {
            ui.separator();
        }
        // The receipt inbox shows how many files wait (P17, Q-P17t).
        let label = if row.id.as_str() == crate::screens::receipt_inbox::KIND {
            match crate::screens::receipt_inbox::badge(ui.ctx()) {
                0 => row.label,
                n => crate::i18n::t_with(
                    "inbox.menu_label",
                    &[("label", &row.label), ("n", &n.to_string())],
                ),
            }
        } else {
            row.label
        };
        let mut button = egui::Button::new(label);
        if let Some(s) = row.shortcut {
            button = button.shortcut_text(s);
        }
        if ui.add(button).clicked() {
            queued.push(row.id);
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_menu_rows_follow_the_command_table() {
        let file = rows(Menu::File);
        assert_eq!(file[0].id.as_str(), "file.open_company");
        assert_eq!(file[0].label, "Open or Switch Company");
        assert!(file[0].shortcut.as_deref().is_some_and(|s| s.contains('O')));
        assert!(!file[0].separator_before);
        // Back Up starts the next group.
        let backup = file
            .iter()
            .find(|r| r.id.as_str() == "file.backup")
            .expect("backup row");
        assert!(backup.separator_before);
        assert_eq!(backup.shortcut, None);
        assert_eq!(
            file.iter().filter(|r| r.separator_before).count(),
            5,
            "six groups in the File menu"
        );
        for m in Menu::ALL {
            assert!(!rows(*m).is_empty(), "{m:?} has rows");
            assert_ne!(title_key(*m), "");
        }
    }
}
