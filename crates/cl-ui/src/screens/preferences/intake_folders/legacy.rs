//! P17's single receipt-inbox folder (P09 step 5, Q-54), chosen here while
//! the engine keeps no intake folders (an engine without
//! `intake-settings-v1`, Q-A01q). The inbox then works exactly as in P17:
//! one folder on this computer, refused when it is a network path.
//! Once the engine has intake folders the migration moves this folder over
//! and this chooser is no longer shown.

use std::path::PathBuf;

use eframe::egui::{self, Ui};

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

/// Draws the P17 folder chooser over the company record's `dir`.
pub fn chooser(ui: &mut Ui, tokens: &Tokens, dir: &mut Option<PathBuf>) {
    form::card(ui, tokens, |ui| {
        form::hint(ui, tokens, &t("prefs.inbox.intro"));
        let mut on = dir.is_some();
        if ui.checkbox(&mut on, t("prefs.inbox.watch")).changed() {
            *dir = on.then(PathBuf::new);
        }
        let mut text = dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        ui.add_enabled_ui(on, |ui| {
            if form::field_wide(ui, &t("prefs.inbox.folder"), &mut text).changed() {
                *dir = Some(PathBuf::from(text.trim()));
            }
        });
        form::hint(ui, tokens, &t("prefs.inbox.folder_hint"));
        if let Some(p) = dir.as_ref().filter(|p| !p.as_os_str().is_empty())
            && let Err(e) = cl_settings::check_inbox_folder(p)
        {
            form::hint(ui, tokens, &t(folder_error_key(e)));
        }
        // P17: the inbox itself is Vendors › Receipt Inbox.
        ui.add(egui::Label::new(t("inbox.prefs.where")).selectable(false));
    });
}
