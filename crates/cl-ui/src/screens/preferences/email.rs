//! The Email panel (P09 step 4, Q-34): SMTP host, port, security, user,
//! sender, and the password, which lives only in the OS keychain
//! (Q-P09b). The connection test sends the password in its body (the
//! engine keeps nothing); real sends wait on U-13.

use cl_api::{SmtpSettings, SmtpSettingsUpdate, SmtpTest};
use cl_core::Secret;
use cl_settings::smtp::{SMTP_PASSWORD, SmtpSecurity};
use cl_settings::{SecretError, SecretStore, SmtpForm};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use super::ai::SecretsSaved;
use super::{Edits, SaveBlock};
use crate::i18n::t;
use crate::screens::company::form;
use crate::theme::Tokens;

/// What the Email panel edits.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct EmailEdits {
    /// The non-secret fields.
    pub form: SmtpForm,
    /// A password typed this session (cleared after it is saved).
    pub password_input: String,
    /// Where the test message goes (optional).
    pub test_email: String,
}

impl std::fmt::Debug for EmailEdits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailEdits")
            .field("host", &self.form.host)
            .field("password_typed", &!self.password_input.is_empty())
            .finish_non_exhaustive()
    }
}

impl EmailEdits {
    /// Overlays the engine's settings.
    pub fn apply_engine(&mut self, s: Option<&SmtpSettings>, security: SmtpSecurity) {
        self.form.security = security;
        if let Some(s) = s {
            self.form.host.clone_from(&s.smtp_host);
            self.form.port = if s.smtp_port == 0 {
                security.default_port().to_string()
            } else {
                s.smtp_port.to_string()
            };
            self.form.user.clone_from(&s.smtp_user);
            self.form.from.clone_from(&s.smtp_from);
            self.form.from_name.clone_from(&s.smtp_from_name);
        } else if self.form.port.is_empty() {
            self.form.port = security.default_port().to_string();
        }
    }

    /// Nothing filled in: the panel is left alone on save.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.form.host.trim().is_empty()
            && self.form.from.trim().is_empty()
            && self.password_input.is_empty()
    }

    /// Checks the fields (a blank panel passes).
    ///
    /// # Errors
    /// The first bad field.
    pub fn validate(&self) -> Result<(), SaveBlock> {
        if self.is_blank() {
            return Ok(());
        }
        self.form.validate().map_err(SaveBlock::Smtp)
    }

    /// Saves a typed password to the keychain.
    ///
    /// # Errors
    /// The keychain's.
    pub fn store_password(
        &self,
        store: &dyn SecretStore,
        saved: &mut SecretsSaved,
    ) -> Result<(), SecretError> {
        if self.password_input.is_empty() {
            return Ok(());
        }
        store.set(SMTP_PASSWORD, &Secret::new(self.password_input.clone()))?;
        saved.smtp_password = true;
        Ok(())
    }

    /// Forgets the typed password (after it was saved).
    pub fn clear_typed(&mut self) {
        self.password_input.clear();
    }

    /// The engine write when the non-secret fields changed.
    #[must_use]
    pub fn update_for(&self, loaded: Option<&SmtpSettings>) -> Option<SmtpSettingsUpdate> {
        if self.is_blank() || self.form.validate().is_err() {
            return None;
        }
        let update = SmtpSettingsUpdate {
            host: self.form.host.trim().to_string(),
            port: self.form.port_number().unwrap_or(587),
            user: self.form.user.trim().to_string(),
            from: self.form.from.trim().to_string(),
            from_name: self.form.from_name.trim().to_string(),
        };
        let same = loaded.is_some_and(|l| {
            l.smtp_host == update.host
                && l.smtp_port == update.port
                && l.smtp_user == update.user
                && l.smtp_from == update.from
                && l.smtp_from_name == update.from_name
        });
        (!same).then_some(update)
    }

    /// The connection test request: the typed password, else the saved one.
    #[must_use]
    pub fn test_request(&self, store: &dyn SecretStore) -> Option<SmtpTest> {
        let password = if self.password_input.is_empty() {
            store.get(SMTP_PASSWORD).ok().flatten()
        } else {
            Some(Secret::new(self.password_input.clone()))
        };
        Some(SmtpTest {
            host: self.form.host.trim().to_string(),
            port: self.form.port_number()?,
            username: self.form.user.trim().to_string(),
            password,
            from: self.form.from.trim().to_string(),
            from_name: Some(self.form.from_name.trim().to_string()).filter(|n| !n.is_empty()),
            test_email: Some(self.test_email.trim().to_string()).filter(|e| !e.is_empty()),
        })
    }

    /// Control-channel view (no values).
    #[must_use]
    pub fn state(&self, saved: &SecretsSaved) -> Value {
        json!({
            "host_set": !self.form.host.trim().is_empty(),
            "security": format!("{:?}", self.form.security).to_ascii_lowercase(),
            "password_saved": saved.smtp_password,
            "password_typed": !self.password_input.is_empty(),
        })
    }
}

/// What the panel asked the screen to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Nothing.
    None,
    /// Remove the saved password.
    RemovePassword,
    /// Test the connection.
    Test,
}

/// Draws the Email panel.
pub fn panel(
    ui: &mut Ui,
    tokens: &Tokens,
    edits: &mut Edits,
    saved: &SecretsSaved,
    testing: bool,
    _demo: bool,
) -> PanelAction {
    let mut action = PanelAction::None;
    let e = &mut edits.email;
    form::heading(ui, tokens, &t("prefs.email.title"));
    form::hint(ui, tokens, &t("prefs.email.intro"));
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        form::field(ui, &t("prefs.email.host"), &mut e.form.host);
        form::field(ui, &t("prefs.email.port"), &mut e.form.port);
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::Label::new(t("prefs.email.security")).selectable(false));
            for s in SmtpSecurity::ALL {
                if ui
                    .radio_value(&mut e.form.security, s, t(s.label_key()))
                    .clicked()
                    && e.form
                        .port_number()
                        .is_none_or(|p| SmtpSecurity::ALL.iter().any(|o| o.default_port() == p))
                {
                    e.form.port = s.default_port().to_string();
                }
            }
        });
        form::field(ui, &t("prefs.email.user"), &mut e.form.user);
        // The SMTP password is a secret: masked on screen and in the tree.
        form::password(ui, &t("prefs.email.password"), &mut e.password_input);
        ui.horizontal(|ui| {
            let status = if saved.smtp_password {
                t("prefs.email.password_saved")
            } else {
                t("prefs.email.password_missing")
            };
            ui.label(RichText::new(status).color(tokens.text_secondary));
            if saved.smtp_password && ui.button(t("prefs.email.password_remove")).clicked() {
                action = PanelAction::RemovePassword;
            }
        });
        form::field(ui, &t("prefs.email.from"), &mut e.form.from);
        form::field(ui, &t("prefs.email.from_name"), &mut e.form.from_name);
    });
    ui.add_space(8.0);
    form::card(ui, tokens, |ui| {
        form::heading(ui, tokens, &t("prefs.email.test_title"));
        form::field(ui, &t("prefs.email.test_to"), &mut e.test_email);
        ui.horizontal(|ui| {
            if testing {
                form::progress_line(ui, tokens, &t("prefs.email.testing"));
            } else if ui.button(t("prefs.email.test")).clicked() {
                action = PanelAction::Test;
            }
        });
        form::hint(ui, tokens, &t("prefs.email.test_hint"));
        form::hint(ui, tokens, &t("prefs.email.sends_later"));
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_settings::MemorySecrets;

    #[test]
    fn password_goes_to_the_keychain_and_never_into_debug_or_state() {
        let store = MemorySecrets::new();
        let mut saved = SecretsSaved::default();
        let mut e = EmailEdits {
            form: SmtpForm {
                host: "smtp.example.test".into(),
                port: "587".into(),
                security: SmtpSecurity::StartTls,
                user: "books".into(),
                from: "books@example.test".into(),
                from_name: String::new(),
            },
            password_input: "smtp-canary-9".into(),
            test_email: String::new(),
        };
        assert_eq!(e.validate(), Ok(()));
        e.store_password(&store, &mut saved).expect("store");
        assert!(saved.smtp_password);
        assert!(!format!("{e:?}").contains("canary"));
        assert!(!e.state(&saved).to_string().contains("canary"));
        let test = e.test_request(&store).expect("test");
        assert_eq!(
            test.password.as_ref().map(Secret::expose),
            Some("smtp-canary-9")
        );
        e.clear_typed();
        let test = e.test_request(&store).expect("test");
        assert_eq!(
            test.password.as_ref().map(Secret::expose),
            Some("smtp-canary-9"),
            "from the keychain"
        );
        let u = e.update_for(None).expect("update");
        assert_eq!(u.port, 587);
        assert!(!serde_json::to_string(&u).expect("json").contains("canary"));
        let loaded = SmtpSettings {
            smtp_host: "smtp.example.test".into(),
            smtp_port: 587,
            smtp_user: "books".into(),
            smtp_from: "books@example.test".into(),
            ..SmtpSettings::default()
        };
        assert!(e.update_for(Some(&loaded)).is_none(), "unchanged");
        e.form.from = "nope".into();
        assert_eq!(
            e.validate(),
            Err(SaveBlock::Smtp(cl_settings::SmtpFieldError::From))
        );
        let blank = EmailEdits::default();
        assert!(blank.is_blank() && blank.validate().is_ok() && blank.update_for(None).is_none());
        let mut from_engine = EmailEdits::default();
        from_engine.apply_engine(Some(&loaded), SmtpSecurity::Tls);
        assert_eq!(from_engine.form.host, "smtp.example.test");
        assert_eq!(from_engine.form.security, SmtpSecurity::Tls);
    }
}
