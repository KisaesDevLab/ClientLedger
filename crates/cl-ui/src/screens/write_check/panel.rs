//! The side panel's place (beside the form, or under it when narrow).
//!
//! WP7 fills it: the payee card with the open-bill note, the recent checks
//! and the attachments drop zone (mockup WriteCheck.html). Until then it
//! shows the payee's name and the Attachments heading only, with no
//! figures.

use eframe::egui::{RichText, Ui};

use super::WriteCheckScreen;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

impl WriteCheckScreen {
    /// Draws the side panel.
    pub(crate) fn panel_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(name) = self
            .header
            .current()
            .payee
            .and_then(|p| self.lists.name_text(p))
            .map(str::to_string)
        {
            form::card_tight(ui, tokens, |ui| {
                ui.label(RichText::new(name).strong().color(tokens.text_primary));
            });
            ui.add_space(12.0);
        }
        form::card_tight(ui, tokens, |ui| {
            ui.label(
                RichText::new(t("check.panel.attachments"))
                    .strong()
                    .color(tokens.text_primary),
            );
            form::hint(ui, tokens, &t("check.attach.later"));
        });
    }
}
