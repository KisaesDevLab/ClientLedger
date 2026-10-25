//! The Receipt inbox panel (P09 step 5, Q-54): the folder the app watches
//! for new photos and PDFs to attach to bills, checks and bank items
//! (the inbox itself is `screens::receipt_inbox`, P17).

use eframe::egui::{self, Ui};

use super::Edits;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// i18n key for a refused folder.
#[must_use]
pub fn folder_error_key(e: cl_settings::FolderError) -> &'static str {
    match e {
        cl_settings::FolderError::NotAbsolute => "prefs.inbox.err_absolute",
        cl_settings::FolderError::Network => "prefs.inbox.err_network",
        cl_settings::FolderError::Missing => "prefs.inbox.err_missing",
    }
}

/// Draws the Receipt inbox panel.
pub fn receipt_inbox(ui: &mut Ui, tokens: &Tokens, edits: &mut Edits) {
    form::heading(ui, tokens, &t("prefs.cat.inbox"));
    form::hint(ui, tokens, &t("prefs.inbox.intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        let mut on = edits.company.receipt_inbox_dir.is_some();
        if ui.checkbox(&mut on, t("prefs.inbox.watch")).changed() {
            edits.company.receipt_inbox_dir = on.then(std::path::PathBuf::new);
        }
        let mut text = edits
            .company
            .receipt_inbox_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        ui.add_enabled_ui(on, |ui| {
            if form::field_wide(ui, &t("prefs.inbox.folder"), &mut text).changed() {
                edits.company.receipt_inbox_dir = Some(std::path::PathBuf::from(text.trim()));
            }
        });
        form::hint(ui, tokens, &t("prefs.inbox.folder_hint"));
        if let Some(p) = edits
            .company
            .receipt_inbox_dir
            .as_ref()
            .filter(|p| !p.as_os_str().is_empty())
            && let Err(e) = cl_settings::check_inbox_folder(p)
        {
            form::hint(ui, tokens, &t(folder_error_key(e)));
        }
        // P17: the inbox itself is Vendors › Receipt Inbox.
        ui.add(egui::Label::new(t("inbox.prefs.where")).selectable(false));
    });
}
