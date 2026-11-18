//! Spot-check a few items (step 7, QuickBooks Desktop only, Q-144). D4
//! builds the guided list (`cl_model::imports::spot_check`), saved with the
//! conversion record on change and on close; until then the step says what
//! comes and shows the verified tie-out's summary.

use eframe::egui::{self, RichText, Ui};

use super::ImportScreen;
use crate::i18n::t;
use crate::theme::Tokens;

/// Draws the step.
pub fn spot_check_ui(s: &mut ImportScreen, ui: &mut Ui, tokens: &Tokens) {
    ui.add(
        egui::Label::new(RichText::new(t("imports.spot.intro")).color(tokens.text_secondary))
            .wrap(),
    );
    ui.add_space(8.0);
    if let Some(sum) = s.tieout.as_ref().map(super::tieout::summary) {
        let mark = if sum.all_zero { " ✓" } else { "" };
        ui.label(format!(
            "{} {}{mark}",
            crate::i18n::t_with("imports.tieout.tile", &[("date", &sum.as_of)]),
            sum.difference
        ));
    }
    if s.wizard.verified {
        ui.label(t("imports.spot.verified"));
    }
}
