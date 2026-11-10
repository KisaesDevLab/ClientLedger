//! Undo the last reconciliation (Q-P27e): Admin only, "Reopen it with your
//! checks kept" or "Discard it"; service charge and interest entries stay
//! in the books; a closed period needs the closing password; one in
//! progress must be discarded first.

use cl_api::UndoMode;
use eframe::egui::{self, Ui};

use super::{Dialog, ReconcileScreen};
use crate::i18n::{t, t_with};

/// Draws the Undo dialog.
pub(crate) fn draw(s: &mut ReconcileScreen, ui: &Ui) {
    let last = s.last_finished().copied();
    egui::Window::new(t("reconcile.undo.title"))
        .id(egui::Id::unique("reconcile_undo"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .show(ui.ctx(), |ui| {
            ui.set_max_width(480.0);
            let Some(last) = last else {
                ui.label(t("reconcile.undo.none"));
                if ui.button(t("reconcile.cancel")).clicked() {
                    s.dialog = None;
                }
                return;
            };
            ui.add(
                egui::Label::new(t_with(
                    "reconcile.undo.text",
                    &[("date", &crate::shell::us_date(last.statement_date))],
                ))
                .wrap(),
            );
            ui.add_space(6.0);
            ui.radio_value(
                &mut s.undo_mode,
                UndoMode::KeepChecks,
                t("reconcile.undo.keep"),
            );
            ui.radio_value(
                &mut s.undo_mode,
                UndoMode::Discard,
                t("reconcile.undo.discard"),
            );
            ui.add_space(6.0);
            ui.add(egui::Label::new(t("reconcile.undo.charges_stay")).wrap());
            if s.unfinished().is_some() {
                ui.add_space(6.0);
                ui.add(egui::Label::new(t("reconcile.undo.in_progress")).wrap());
                if ui.button(t("reconcile.begin.start_over")).clicked() {
                    s.start_over();
                    s.dialog = Some(Dialog::Undo);
                }
            }
            let tokens = crate::theme::Tokens::of(ui.ctx());
            if s.undo_locked {
                ui.add_space(6.0);
                ui.add(egui::Label::new(t("reconcile.undo.closed")).wrap());
                if s.password_rejected {
                    crate::screens::company::form::inline_warning(
                        ui,
                        &tokens,
                        &t("reconcile.closing.rejected"),
                    );
                }
                crate::screens::company::form::password(
                    ui,
                    &t("reconcile.closing.password"),
                    &mut s.password,
                );
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if crate::widgets::primary_button(ui, &t("reconcile.undo.confirm")).clicked() {
                    s.undo_error = None;
                    s.undo_last();
                }
                if ui.button(t("reconcile.cancel")).clicked() {
                    s.close_undo();
                }
            });
            // Plain words only, never the engine's code (QA-12).
            if let Some(e) = &s.undo_error {
                crate::screens::company::form::inline_error(ui, &tokens, e);
            }
        });
}
