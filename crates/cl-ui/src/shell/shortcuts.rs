//! The shortcut overlay (Ctrl+/): the keys that work where the owner is now,
//! read from the keymap (Q-161).

use cl_model::keymap::{BANK_FEED_KEYS, DATE_KEYS, FIXED_KEYS};
use cl_model::{COMMANDS, Command, Keymap, Scope};
use eframe::egui::{self, Align2, Context, RichText};

use crate::i18n::t;
use crate::input;
use crate::theme::Tokens;

/// Commands with a shortcut that work while focus is in `focus`, innermost
/// scope first, in table order.
#[must_use]
pub fn rows_for(focus: Scope) -> Vec<(Scope, &'static Command)> {
    let mut rows = Vec::new();
    for scope in focus.active_from() {
        for c in COMMANDS
            .iter()
            .filter(|c| c.scope == *scope && c.shortcut.is_some())
        {
            rows.push((*scope, c));
        }
    }
    rows
}

/// Draw the overlay. Returns true when it should close.
#[must_use]
pub fn show(ctx: &Context, focus: Scope, _keymap: &Keymap) -> bool {
    let tokens = Tokens::of(ctx);
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Window::new(t("shortcuts.title"))
        .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        .default_height(480.0)
        .max_height(560.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.label(RichText::new(t("shortcuts.intro")).color(tokens.text_secondary));
                ui.add_space(8.0);
                let mut last: Option<Scope> = None;
                for (scope, c) in rows_for(focus) {
                    if last != Some(scope) {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(t(&format!("shortcuts.scope.{}", scope_key(scope))))
                                .text_style(crate::theme::styles::section()),
                        );
                        last = Some(scope);
                    }
                    key_row(
                        ui,
                        &tokens,
                        &c.shortcut.map(input::shortcut_text).unwrap_or_default(),
                        &t(&format!("cmd.{}", c.id)),
                    );
                }
                for (title, rows) in [
                    ("shortcuts.fixed", FIXED_KEYS),
                    ("shortcuts.dates", DATE_KEYS),
                    ("shortcuts.bank_feed", BANK_FEED_KEYS),
                ] {
                    ui.add_space(10.0);
                    ui.label(RichText::new(t(title)).text_style(crate::theme::styles::section()));
                    for (keys, what) in rows {
                        key_row(ui, &tokens, keys, what);
                    }
                }
            });
            ui.add_space(8.0);
            if ui.button(t("shortcuts.close")).clicked() {
                close = true;
            }
        });
    close
}

fn scope_key(s: Scope) -> &'static str {
    match s {
        Scope::Global => "global",
        Scope::Window => "window",
        Scope::Form => "form",
    }
}

fn key_row(ui: &mut egui::Ui, tokens: &Tokens, keys: &str, what: &str) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(170.0, 18.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.label(
                    RichText::new(keys)
                        .family(egui::FontFamily::Monospace)
                        .size(12.0)
                        .color(tokens.text_primary),
                );
            },
        );
        ui.label(what);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_focus_sees_form_window_and_global_keys() {
        let rows = rows_for(Scope::Form);
        assert!(
            rows.iter()
                .any(|(s, c)| *s == Scope::Form && c.id.as_str() == "form.save_new")
        );
        assert!(
            rows.iter()
                .any(|(s, c)| *s == Scope::Global && c.id.as_str() == "help.palette")
        );
        assert!(rows.iter().any(|(s, _)| *s == Scope::Window));
    }

    #[test]
    fn global_focus_hides_form_keys() {
        let rows = rows_for(Scope::Global);
        assert!(rows.iter().all(|(s, _)| *s == Scope::Global));
        assert!(rows.iter().all(|(_, c)| c.id.as_str() != "form.save_new"));
    }
}
