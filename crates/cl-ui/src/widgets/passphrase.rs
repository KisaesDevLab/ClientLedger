//! The backup passphrase, asked inline where a backup is about to be made
//! (P31 QA-2, Q-P31ad): the import wizards, the spreadsheet import and
//! payroll posting take a verified backup first, and a fresh install has
//! no passphrase yet. Saved exactly as File › Back Up Company saves it
//! (the keychain, then `Change::BackupPassphrase` on the data bus).

use cl_company::CompanyError;
use cl_core::Secret;
use eframe::egui::Ui;

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// The shortest passphrase taken (the Back Up screen's rule).
pub const MIN_CHARS: usize = 12;

/// True when no backup passphrase is set (never in the demo, whose backups
/// are simulated).
#[must_use]
pub fn needed(link: &CompanyLink) -> bool {
    !link.is_demo()
        && link
            .secrets()
            .get(cl_settings::secrets::BACKUP_PASSPHRASE)
            .ok()
            .flatten()
            .is_none_or(|p| p.is_empty())
}

/// Checks `pass` and `confirm` and saves the passphrase to the keychain,
/// telling every open window (the Back Up screen's rule and path).
///
/// # Errors
/// `PassphraseTooShort`, `PassphraseMismatch`, or the keychain's error;
/// nothing is saved then.
pub fn save(link: &CompanyLink, pass: &str, confirm: &str) -> Result<(), CompanyError> {
    if pass.chars().count() < MIN_CHARS {
        return Err(CompanyError::PassphraseTooShort);
    }
    if pass != confirm {
        return Err(CompanyError::PassphraseMismatch);
    }
    let secret = Secret::new(pass.to_string());
    link.secrets()
        .set(cl_settings::secrets::BACKUP_PASSPHRASE, &secret)?;
    link.bus()
        .publish(crate::data_bus::Change::BackupPassphrase);
    Ok(())
}

/// The two passphrase boxes and their error. No `Debug`: it holds the
/// passphrase while typed.
#[derive(Default)]
pub struct PassphraseAsk {
    pass: String,
    confirm: String,
    /// The last save's refusal.
    pub error: Option<CompanyError>,
    /// Told when a passphrase is set in another window.
    bus: Option<crate::data_bus::Subscription>,
}

impl PassphraseAsk {
    /// Call each frame: a passphrase set elsewhere (Back Up, another
    /// wizard) clears `need` so the boxes go.
    pub fn refresh(&mut self, link: &CompanyLink, need: &mut bool) {
        let bus = self.bus.get_or_insert_with(|| link.bus().subscribe());
        if link.bus().take(bus).backup_passphrase {
            *need = false;
            self.error = None;
        }
    }

    /// Draws the heading, the reason, both boxes and the last error.
    /// `why` is the i18n key of the sentence saying why it is asked here.
    pub fn ui(&mut self, ui: &mut Ui, tokens: &Tokens, why: &str) {
        form::heading(ui, tokens, &t("backup.passphrase_title"));
        form::hint(ui, tokens, &t(why));
        form::hint(ui, tokens, &t("backup.passphrase_intro"));
        form::password(ui, &t("backup.passphrase"), &mut self.pass);
        form::password(ui, &t("backup.passphrase_confirm"), &mut self.confirm);
        if let Some(e) = &self.error {
            form::inline_error(ui, tokens, e);
        }
    }

    /// Saves what was typed. True when saved (the boxes are cleared);
    /// false with [`PassphraseAsk::error`] set.
    pub fn save(&mut self, link: &CompanyLink) -> bool {
        // Set meanwhile (Back Up, another window): never replaced from here,
        // or the backups made with it could no longer be opened.
        if !link.is_demo() && !needed(link) {
            self.pass.clear();
            self.confirm.clear();
            self.error = None;
            return true;
        }
        match save(link, &self.pass, &self.confirm) {
            Ok(()) => {
                self.pass.clear();
                self.confirm.clear();
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    /// Types into the boxes (tests and the control channel's `type`).
    #[cfg(test)]
    pub fn fill(&mut self, pass: &str, confirm: &str) {
        self.pass = pass.to_string();
        self.confirm = confirm.to_string();
    }
}
