//! "Just for me" panels (P09 step 1): look and feel (with "Enter moves
//! between fields", Q-264), dates and numbers, reminders; plus the app-wide
//! Backups and Updates panels (step 5).

use cl_settings::user_prefs::{DateFormat, NegativeStyle, StartScreen, ThemeChoice, ZOOM_STEPS};
use eframe::egui::{self, Ui};

use super::Edits;
use crate::i18n::{t, t_with};
use crate::screens::company::form;
use crate::theme::Tokens;

/// A row of radio choices with one label.
fn radios<T: Copy + PartialEq>(
    ui: &mut Ui,
    label: &str,
    value: &mut T,
    choices: &[T],
    name: impl Fn(T) -> String,
) {
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Label::new(label).selectable(false));
        for c in choices {
            ui.radio_value(value, *c, name(*c));
        }
    });
}

/// Theme and zoom.
pub fn look_and_feel(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.look"));
    form::hint(ui, tokens, &t("prefs.user.look_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        radios(
            ui,
            &t("prefs.user.theme"),
            &mut edits.user.theme,
            &ThemeChoice::ALL,
            |c| t(c.label_key()),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let l = ui.add(egui::Label::new(t("prefs.user.zoom")).selectable(false));
            egui::ComboBox::from_id_salt("prefs-zoom")
                .selected_text(format!("{}%", edits.user.zoom_percent))
                .show_ui(ui, |ui| {
                    for z in ZOOM_STEPS {
                        ui.selectable_value(&mut edits.user.zoom_percent, z, format!("{z}%"));
                    }
                })
                .response
                .labelled_by(l.id);
        });
        form::hint(ui, tokens, &t("prefs.user.zoom_hint"));
        ui.add_space(6.0);
        radios(
            ui,
            &t("prefs.user.start"),
            &mut edits.user.start_screen,
            &StartScreen::ALL,
            |c| t(c.label_key()),
        );
        ui.add_space(6.0);
        // Q-264: off by default; Enter is Save & New then (Q-190).
        ui.checkbox(
            &mut edits.user.enter_moves_fields,
            t("prefs.user.enter_moves"),
        )
        .on_hover_text(t("prefs.user.enter_moves.tip"));
    });
}

/// Date format, negatives, date warnings (Q-97).
pub fn dates_numbers(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.dates"));
    form::hint(ui, tokens, &t("prefs.user.dates_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        radios(
            ui,
            &t("prefs.user.date_format"),
            &mut edits.user.date_format,
            &DateFormat::ALL,
            |c| t(c.label_key()),
        );
        radios(
            ui,
            &t("prefs.user.negatives"),
            &mut edits.user.negatives,
            &NegativeStyle::ALL,
            |c| t(c.label_key()),
        );
        ui.add_space(6.0);
        form::heading(ui, tokens, &t("prefs.user.warn_title"));
        form::hint(ui, tokens, &t("prefs.user.warn_intro"));
        form::field(ui, &t("prefs.user.warn_past"), &mut edits.past_days);
        form::field(ui, &t("prefs.user.warn_future"), &mut edits.future_days);
    });
}

/// Bills-due window (Q-169) and the backup reminder.
pub fn reminders(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.reminders"));
    form::hint(ui, tokens, &t("prefs.user.reminders_intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        form::field(ui, &t("prefs.user.bills_due"), &mut edits.bills_due_days);
        form::hint(ui, tokens, &t("prefs.user.bills_due_hint"));
        ui.add_space(6.0);
        ui.checkbox(&mut edits.backup_on_close, t("prefs.user.backup_on_close"));
        form::hint(ui, tokens, &t("prefs.user.backup_on_close_hint"));
    });
}

/// Backup folder and backup on close.
pub fn backups(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.backups"));
    form::hint(ui, tokens, &t("prefs.backups.intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        form::field_wide(ui, &t("prefs.backups.folder"), &mut edits.backup_folder);
        form::hint(ui, tokens, &t("prefs.backups.folder_hint"));
        ui.checkbox(&mut edits.backup_on_close, t("prefs.user.backup_on_close"));
        form::hint(ui, tokens, &t("prefs.backups.schedule_later"));
    });
}

/// Update checks: manual only in beta.
pub fn updates(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.updates"));
    form::hint(ui, tokens, &t("prefs.updates.intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        let _ = &edits.user.update_check;
        ui.add(egui::Label::new(t("prefs.updates.manual_only")).selectable(false));
        form::hint(
            ui,
            tokens,
            &t_with("prefs.updates.version", &[("v", cl_company::APP_VERSION)]),
        );
        ui.add_enabled_ui(false, |ui| {
            let _ = ui.button(t("prefs.updates.check"));
        });
        form::hint(ui, tokens, &t("prefs.updates.later"));
    });
}
