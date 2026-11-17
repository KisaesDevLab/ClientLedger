//! Forgot password? (P29 WP4, Q-210, Q-P29m): the main Admin resets their
//! password with the recovery key saved at setup, through
//! `POST /desktop/recover-password` (no session needed).
//!
//! - The key is used up; the engine's reply carries a fresh one, which goes
//!   through the P08 stash and is shown once by the recovery-key window.
//!   An engine that sends none: the owner is asked to make a new key.
//! - A wrong key and an unknown email read the same (nothing tells which).
//! - Recovery clears the lock, the must-change flag and automatic sign-in.
//!
//! Key and passwords live in zeroizing buffers and are wiped after
//! sending; nothing here is logged.

use std::sync::Arc;

use cl_api::{ApiError, AuthFailure, PasswordRecovery, normalize_recovery_key};
use cl_core::Secret;
use cl_model::CommandId;
use eframe::egui::Ui;

use super::company::form::{self, SecretText};
use super::login::new_password_problem;
use crate::company::{CompanyEvent, CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// What the owner reads for a recovery refusal.
#[must_use]
pub fn recovery_message(e: &ApiError) -> &'static str {
    match e {
        ApiError::Unauthorized(AuthFailure::InvalidCredentials) => "reset.msg.wrong",
        ApiError::RateLimited { .. } => "login.msg.too_many",
        ApiError::Local(_) | ApiError::Validation(_) => "login.msg.policy",
        ApiError::NotFound { .. } | ApiError::Unsupported { .. } => "reset.msg.unavailable",
        ApiError::EngineUnavailable { .. } | ApiError::Timeout => "login.msg.engine",
        _ => "reset.msg.failed",
    }
}

/// What the recovery panel reports.
#[derive(Debug, PartialEq, Eq)]
pub enum RecoveryEvent {
    /// The password was reset; sign in with it (the email to fill in).
    Recovered(String),
    /// Back to sign-in without a change.
    Cancel,
}

/// The recovery form.
pub struct RecoveryPanel {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    /// The main Admin's email.
    pub email: String,
    key: SecretText,
    new_password: SecretText,
    confirm: SecretText,
    task: Option<Task<Result<Option<Secret>, ApiError>>>,
    message: Option<&'static str>,
}

impl std::fmt::Debug for RecoveryPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecoveryPanel")
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

impl RecoveryPanel {
    /// A panel over the link, the email filled in when known.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>, email: &str) -> RecoveryPanel {
        let group = link.group().and_then(Result::ok);
        RecoveryPanel {
            link,
            group,
            email: email.trim().to_string(),
            key: SecretText::default(),
            new_password: SecretText::default(),
            confirm: SecretText::default(),
            task: None,
            message: None,
        }
    }

    /// The message key shown now.
    #[must_use]
    pub fn message(&self) -> Option<&'static str> {
        self.message
    }

    fn submit(&mut self) {
        if self.email.trim().is_empty() || self.key.is_empty() {
            self.message = Some("reset.msg.missing");
            return;
        }
        if let Some(key) = new_password_problem(self.new_password.text(), self.confirm.text()) {
            self.message = Some(key);
            return;
        }
        let recovery = PasswordRecovery {
            email: self.email.trim().to_string(),
            recovery_key: normalize_recovery_key(self.key.text()),
            new_password: self.new_password.to_secret(),
        };
        self.key.clear();
        self.new_password.clear();
        self.confirm.clear();
        self.message = None;
        let demo = self.link.demo_state().map(|mut d| {
            d.auth.recover(
                &recovery.email,
                &recovery.recovery_key,
                &recovery.new_password,
            )
        });
        if let Some(r) = demo {
            self.task = Some(Task::ready(r));
            return;
        }
        let (Some(group), Some(client)) = (self.group.as_ref(), self.link.client().cloned()) else {
            self.message = Some("reset.msg.failed");
            return;
        };
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            client.recover_password(&recovery).await
        })
        .ok();
    }

    /// Reads the answer.
    pub fn poll(&mut self) -> Option<RecoveryEvent> {
        let r = self.task.as_mut()?.poll_result()?;
        self.task = None;
        match r {
            Ok(fresh) => {
                tracing::info!("password reset with the recovery key");
                // Q-P29o: a recovery ends automatic sign-in.
                self.link.clear_auto_sign_in();
                match fresh {
                    Some(key) => self.link.stash_recovery_key(key),
                    // An older engine: ask for a new key after sign-in.
                    None => self.link.push(CompanyEvent::Toast("reset.no_new_key")),
                }
                self.link.push(CompanyEvent::OpenWindow(CommandId::new(
                    super::company::RECOVERY_KEY,
                )));
                Some(RecoveryEvent::Recovered(self.email.trim().to_string()))
            }
            Err(e) => {
                self.message = Some(recovery_message(&e));
                None
            }
        }
    }

    /// Draws the panel.
    pub fn ui(&mut self, ui: &mut Ui, tokens: &Tokens) -> Option<RecoveryEvent> {
        if let Some(e) = self.poll() {
            return Some(e);
        }
        let mut out = None;
        form::card(ui, tokens, |ui| {
            form::heading(ui, tokens, &t("reset.title"));
            form::hint(ui, tokens, &t("reset.intro"));
            ui.add_space(4.0);
            form::field(ui, &t("reset.email"), &mut self.email);
            form::password_secret(ui, &t("reset.key"), &mut self.key);
            form::password_secret(ui, &t("login.change.new"), &mut self.new_password);
            form::password_secret(ui, &t("login.change.confirm"), &mut self.confirm);
            form::hint(ui, tokens, &t("login.rule"));
            ui.add_space(6.0);
            if let Some(key) = self.message {
                form::inline_warning(ui, tokens, &t(key));
                ui.add_space(4.0);
            }
            if self.task.is_some() {
                form::progress_line(ui, tokens, &t("reset.working"));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            ui.horizontal(|ui| {
                if primary_button(ui, &t("reset.go")).clicked() {
                    self.submit();
                }
                if ui.button(t("reset.cancel")).clicked() {
                    out = Some(RecoveryEvent::Cancel);
                }
            });
            ui.add_space(4.0);
            form::hint(ui, tokens, &t("reset.others"));
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::auth::{NEXT_RECOVERY_KEY, OWNER_EMAIL, RECOVERY_KEY};

    fn panel() -> (tempfile::TempDir, RecoveryPanel) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        link.demo_state().expect("demo").auth.signed_in = None;
        let p = RecoveryPanel::new(link, OWNER_EMAIL);
        (dir, p)
    }

    fn fill(p: &mut RecoveryPanel, key: &str, pw: &str) {
        p.key = SecretText::from(key);
        p.new_password = SecretText::from(pw);
        p.confirm = SecretText::from(pw);
    }

    #[test]
    fn messages_are_plain_and_one_for_wrong_key_or_email() {
        assert_eq!(
            recovery_message(&ApiError::Unauthorized(AuthFailure::InvalidCredentials)),
            "reset.msg.wrong"
        );
        assert_eq!(
            recovery_message(&ApiError::NotFound { code: None }),
            "reset.msg.unavailable"
        );
        for key in [
            "reset.msg.wrong",
            "reset.msg.unavailable",
            "reset.msg.failed",
            "reset.msg.missing",
            "reset.no_new_key",
        ] {
            assert!(crate::i18n::has(key), "{key}");
        }
    }

    #[test]
    fn a_wrong_key_changes_nothing_and_the_right_one_shows_a_fresh_key_once() {
        let (_d, mut p) = panel();
        fill(&mut p, "WRONG-KEY", "recovered-password-1");
        p.submit();
        assert_eq!(p.poll(), None);
        assert_eq!(p.message(), Some("reset.msg.wrong"));
        assert!(
            p.key.is_empty() && p.new_password.is_empty(),
            "wiped after sending"
        );
        // Typed in lower case with spaces: still the key.
        fill(
            &mut p,
            &RECOVERY_KEY.to_lowercase().replace('-', " "),
            "recovered-password-1",
        );
        p.submit();
        assert_eq!(p.poll(), Some(RecoveryEvent::Recovered(OWNER_EMAIL.into())));
        let shown = p
            .link
            .take_recovery_key()
            .expect("stashed for the key window");
        assert_eq!(shown.expose(), NEXT_RECOVERY_KEY);
        assert!(p.link.take_recovery_key().is_none(), "shown once");
        // The old key works no more.
        fill(&mut p, RECOVERY_KEY, "again-password-12");
        p.submit();
        assert_eq!(p.poll(), None);
        assert_eq!(p.message(), Some("reset.msg.wrong"));
    }

    #[test]
    fn missing_or_short_answers_send_nothing() {
        let (_d, mut p) = panel();
        p.submit();
        assert_eq!(p.message(), Some("reset.msg.missing"));
        fill(&mut p, RECOVERY_KEY, "short");
        p.submit();
        assert_eq!(p.message(), Some("login.msg.policy"));
        assert!(p.task.is_none());
    }
}
