//! Prev/Next on unsaved changes (DESIGN.md form rules): the standard
//! Save / Discard changes / Keep editing question, as Write Check asks it.

use eframe::egui::{self, Ui};

use crate::i18n::t;
use crate::windows::PromptAnswer;

/// Draws the question for the window titled `title`; the owner's answer
/// when one was clicked this frame.
#[must_use]
pub fn ui(ui: &Ui, salt: &str, title: &str) -> Option<PromptAnswer> {
    let mut answer = None;
    egui::Window::new(t("window.prompt.title"))
        .id(egui::Id::unique(("nav_prompt", salt)))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.label(crate::windows::prompt_message(title));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if crate::widgets::primary_button(ui, &t("window.prompt.save")).clicked() {
                    answer = Some(PromptAnswer::Save);
                }
                if ui.button(t("window.prompt.discard")).clicked() {
                    answer = Some(PromptAnswer::Discard);
                }
                if ui.button(t("window.prompt.keep")).clicked() {
                    answer = Some(PromptAnswer::Keep);
                }
            });
        });
    answer
}
