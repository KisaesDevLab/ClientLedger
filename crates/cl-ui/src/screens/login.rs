//! Sign-in (P29 WP2), moved out of Open Company: the email and password
//! form, the plain-words messages for each engine refusal (§9: a wrong
//! password always reads "doesn't match"; "locked" and "turned off" only
//! come after the right password), the must-change step for a temporary
//! password (U-230), "Forgot password?" and automatic sign-in (Q-P29o:
//! one try per start; any failure clears it).
//!
//! Passwords live in zeroizing buffers, go to the engine as
//! [`Secret`]s and are wiped after sending; nothing here is logged.

use std::sync::Arc;

use cl_api::{ApiError, AuthFailure, LoginOutcome, Session};
use cl_core::Secret;
use eframe::egui::{self, Ui};

use super::company::form::{self, SecretText};
use crate::company::{CompanyLink, Task, spawn_lifecycle};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// What the owner should read for a sign-in or password refusal.
#[must_use]
pub fn sign_in_message(e: &ApiError) -> &'static str {
    match e {
        ApiError::Unauthorized(AuthFailure::InvalidCredentials) => "login.msg.wrong",
        ApiError::Unauthorized(AuthFailure::AccountLocked { .. }) => "login.msg.locked",
        ApiError::Unauthorized(AuthFailure::Deactivated) => "login.msg.turned_off",
        ApiError::Unauthorized(AuthFailure::TfaInvalid) => "login.msg.tfa",
        ApiError::Unauthorized(_) => "login.msg.again",
        ApiError::RateLimited { .. } => "login.msg.too_many",
        ApiError::PasswordChangeRequired => "login.msg.must_change",
        ApiError::Local(_) => "login.msg.policy",
        ApiError::Validation(v) => match v.code().map(cl_api::ErrorCode::as_str) {
            Some("INVALID_CURRENT_PASSWORD") => "login.msg.current_wrong",
            Some("PASSWORD_UNCHANGED") => "login.msg.unchanged",
            Some("PASSWORD_BREACHED") => "login.msg.breached",
            _ => "login.msg.policy",
        },
        ApiError::EngineUnavailable { .. } | ApiError::Timeout => "login.msg.engine",
        _ => "login.msg.failed",
    }
}

/// The local check of a new password and its confirmation (12+
/// characters, at most 72 bytes, both the same).
#[must_use]
pub fn new_password_problem(new: &str, confirm: &str) -> Option<&'static str> {
    if !cl_api::password_fits(&Secret::new(new)) {
        return Some("login.msg.policy");
    }
    (new != confirm).then_some("login.msg.mismatch")
}

/// What the sign-in panel reports to the window around it.
#[derive(Debug)]
pub enum SignInEvent {
    /// Signed in (`session` is `None` in the demo).
    SignedIn {
        /// The live session.
        session: Option<Session>,
        /// Name for the status bar (no email).
        name: String,
        /// The email, kept in memory for the idle unlock.
        email: String,
        /// The password was just chosen or changed (automatic sign-in is
        /// cleared, Q-P29o).
        password_changed: bool,
    },
    /// "Forgot password?" was chosen.
    Forgot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Form,
    MustChange,
}

/// A good sign-in, before the must-change step.
struct Outcome {
    session: Option<Session>,
    name: String,
    must_change: bool,
}

/// The sign-in form.
pub struct SignInPanel {
    link: Arc<CompanyLink>,
    group: Option<cl_api::RequestGroup>,
    /// The email typed (kept between tries).
    pub email: String,
    password: SecretText,
    new_password: SecretText,
    confirm: SecretText,
    step: Step,
    /// The signed-in session waiting for its new password, its name, and
    /// the temporary password it signed in with (wiped once used).
    waiting: Option<(Option<Session>, String, Secret)>,
    task: Option<Task<Result<Outcome, ApiError>>>,
    change_task: Option<Task<Result<(), ApiError>>>,
    message: Option<&'static str>,
    /// Automatic sign-in was tried this start (once).
    auto_tried: bool,
    /// The running sign-in is the automatic one.
    auto_running: bool,
}

impl std::fmt::Debug for SignInPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInPanel")
            .field("step", &self.step)
            .field("message", &self.message)
            .finish_non_exhaustive()
    }
}

impl SignInPanel {
    /// A panel over the link.
    #[must_use]
    pub fn new(link: Arc<CompanyLink>) -> SignInPanel {
        let group = link.group().and_then(Result::ok);
        SignInPanel {
            link,
            group,
            email: String::new(),
            password: SecretText::default(),
            new_password: SecretText::default(),
            confirm: SecretText::default(),
            step: Step::Form,
            waiting: None,
            task: None,
            change_task: None,
            message: None,
            auto_tried: false,
            auto_running: false,
        }
    }

    /// The message key shown now, if any.
    #[must_use]
    pub fn message(&self) -> Option<&'static str> {
        self.message
    }

    /// True while the must-change step shows.
    #[must_use]
    pub fn must_change(&self) -> bool {
        self.step == Step::MustChange
    }

    /// Tests elsewhere: signs in with these words (demo link).
    #[cfg(test)]
    pub(crate) fn test_sign_in(&mut self, email: &str, pw: &str) -> Option<SignInEvent> {
        self.email = email.into();
        self.password = SecretText::from(pw);
        self.submit();
        self.poll()
    }

    /// True while a sign-in or a password change runs.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.task.is_some() || self.change_task.is_some()
    }

    /// Tries automatic sign-in once per start, when it is on, this run has
    /// not signed out, and the keychain still has the password (Q-P29o).
    pub fn try_auto(&mut self) {
        if self.auto_tried || self.link.is_demo() || self.link.auto_sign_in_skipped() {
            return;
        }
        self.auto_tried = true;
        let auto = self.link.auto_sign_in();
        if !auto.is_on() {
            return;
        }
        let Some((email, password)) = auto.credentials(self.link.secrets().as_ref()) else {
            self.link.clear_auto_sign_in();
            return;
        };
        self.email = email;
        self.auto_running = true;
        self.start(password);
    }

    fn submit(&mut self) {
        if self.email.trim().is_empty() || self.password.is_empty() {
            self.message = Some("login.msg.missing");
            return;
        }
        let password = self.password.to_secret();
        self.password.clear();
        self.start(password);
    }

    fn start(&mut self, password: Secret) {
        self.message = None;
        let email = self.email.trim().to_string();
        let demo = self.link.demo_state().map(|mut d| {
            d.auth.sign_in(&email, &password).map(|s| Outcome {
                session: None,
                name: s.display_name,
                must_change: s.must_change,
            })
        });
        if let Some(r) = demo {
            // Answered at once; read by the next poll like a live answer.
            self.task = Some(Task::ready(r));
            self.waiting = Some((None, String::new(), password));
            return;
        }
        let (Some(group), Some(client)) = (self.group.as_ref(), self.link.client().cloned()) else {
            return;
        };
        let keep = password.clone();
        self.task = spawn_lifecycle(&self.link, group, move || async move {
            match client.login(&email, &password).await? {
                LoginOutcome::Session(session) => {
                    let name = email.split('@').next().unwrap_or("Owner").to_string();
                    Ok(Outcome {
                        must_change: session.password_change_required(),
                        session: Some(session),
                        name,
                    })
                }
                // Two-step sign-in is a MyBooks Online feature.
                LoginOutcome::TfaRequired(_) => {
                    Err(ApiError::Unauthorized(AuthFailure::TfaInvalid))
                }
            }
        })
        .ok();
        // The password stays only until the answer says whether it must be
        // changed (it is the "current" password of that change).
        self.waiting = Some((None, String::new(), keep));
    }

    fn after_sign_in(
        &mut self,
        r: Result<Outcome, ApiError>,
        password: Secret,
    ) -> Option<SignInEvent> {
        let auto = std::mem::take(&mut self.auto_running);
        match r {
            Ok(o) if o.must_change => {
                // A temporary password: never kept for automatic sign-in.
                if auto {
                    self.link.clear_auto_sign_in();
                }
                self.waiting = Some((o.session, o.name, password));
                self.step = Step::MustChange;
                None
            }
            Ok(o) => {
                self.waiting = None;
                Some(SignInEvent::SignedIn {
                    session: o.session,
                    name: o.name,
                    email: self.email.trim().to_string(),
                    password_changed: false,
                })
            }
            Err(e) => {
                self.waiting = None;
                if auto {
                    // One try per start: a stale saved password must not
                    // use up sign-in tries at every launch.
                    self.link.clear_auto_sign_in();
                    self.message = Some("login.msg.auto_failed");
                } else {
                    self.message = Some(sign_in_message(&e));
                }
                None
            }
        }
    }

    fn submit_change(&mut self) {
        if let Some(key) = new_password_problem(self.new_password.text(), self.confirm.text()) {
            self.message = Some(key);
            return;
        }
        let Some((session, _, current)) = self.waiting.as_ref() else {
            self.step = Step::Form;
            return;
        };
        let new = self.new_password.to_secret();
        self.new_password.clear();
        self.confirm.clear();
        self.message = None;
        let demo = self
            .link
            .demo_state()
            .map(|mut d| d.auth.change_password(current, &new));
        if let Some(r) = demo {
            self.change_task = Some(Task::ready(r));
            return;
        }
        let (Some(group), Some(session)) = (self.group.as_ref(), session.clone()) else {
            return;
        };
        let current = current.clone();
        self.change_task = spawn_lifecycle(&self.link, group, move || async move {
            session.change_password(&current, &new).await
        })
        .ok();
    }

    /// Back to the form from the must-change step (the waiting session is
    /// signed out).
    fn cancel_change(&mut self) {
        if let Some((Some(session), _, _)) = self.waiting.take() {
            self.link.set_session(Some(session));
            self.link.sign_out();
        } else if let Some(mut d) = self.link.demo_state() {
            d.auth.signed_in = None;
        }
        self.step = Step::Form;
        self.new_password.clear();
        self.confirm.clear();
        self.message = None;
    }

    /// Reads finished work; returns what the window should do.
    pub fn poll(&mut self) -> Option<SignInEvent> {
        if let Some(task) = self.task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.task = None;
            let password = self
                .waiting
                .take()
                .map_or_else(Secret::default, |(_, _, p)| p);
            return self.after_sign_in(r, password);
        }
        if let Some(task) = self.change_task.as_mut()
            && let Some(r) = task.poll_result()
        {
            self.change_task = None;
            match r {
                Ok(()) => {
                    let (session, name, _) = self.waiting.take()?;
                    self.step = Step::Form;
                    return Some(SignInEvent::SignedIn {
                        session,
                        name,
                        email: self.email.trim().to_string(),
                        password_changed: true,
                    });
                }
                Err(e) => self.message = Some(sign_in_message(&e)),
            }
        }
        None
    }

    /// Draws the panel; returns what happened this frame.
    pub fn ui(&mut self, ui: &mut Ui, tokens: &Tokens) -> Option<SignInEvent> {
        let polled = self.poll();
        if polled.is_some() {
            return polled;
        }
        let mut out = None;
        form::card(ui, tokens, |ui| match self.step {
            Step::Form => out = self.form_ui(ui, tokens),
            Step::MustChange => self.change_ui(ui, tokens),
        });
        if self.busy() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(50));
        }
        out
    }

    fn show_message(&self, ui: &mut Ui, tokens: &Tokens) {
        if let Some(key) = self.message {
            form::inline_warning(ui, tokens, &t(key));
            ui.add_space(4.0);
        }
    }

    fn form_ui(&mut self, ui: &mut Ui, tokens: &Tokens) -> Option<SignInEvent> {
        form::heading(ui, tokens, &t("login.title"));
        form::field(ui, &t("login.email"), &mut self.email);
        let r = form::password_secret(ui, &t("login.password"), &mut self.password);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.add_space(6.0);
        self.show_message(ui, tokens);
        if self.busy() {
            form::progress_line(ui, tokens, &t("login.working"));
        } else if primary_button(ui, &t("login.go")).clicked() || enter {
            self.submit();
        }
        ui.add_space(4.0);
        let forgot = ui.link(t("login.forgot"));
        (forgot.clicked() || form::activated(&forgot)).then_some(SignInEvent::Forgot)
    }

    fn change_ui(&mut self, ui: &mut Ui, tokens: &Tokens) {
        form::heading(ui, tokens, &t("login.change.title"));
        form::hint(ui, tokens, &t("login.change.intro"));
        ui.add_space(4.0);
        form::password_secret(ui, &t("login.change.new"), &mut self.new_password);
        let r = form::password_secret(ui, &t("login.change.confirm"), &mut self.confirm);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        form::hint(ui, tokens, &t("login.rule"));
        ui.add_space(6.0);
        self.show_message(ui, tokens);
        if self.busy() {
            form::progress_line(ui, tokens, &t("login.change.working"));
            return;
        }
        ui.horizontal(|ui| {
            if primary_button(ui, &t("login.change.go")).clicked() || enter {
                self.submit_change();
            }
            if ui.button(t("login.change.cancel")).clicked() {
                self.cancel_change();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::auth::{OWNER_EMAIL, OWNER_PASSWORD};

    fn panel() -> (tempfile::TempDir, SignInPanel) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        link.demo_state().expect("demo").auth.signed_in = None;
        (dir, SignInPanel::new(link))
    }

    fn sign_in(p: &mut SignInPanel, email: &str, pw: &str) -> Option<SignInEvent> {
        p.email = email.into();
        p.password = SecretText::from(pw);
        p.submit();
        p.poll()
    }

    #[test]
    fn every_refusal_has_plain_words() {
        use cl_api::LocalValidation;
        let cases = [
            (
                ApiError::Unauthorized(AuthFailure::InvalidCredentials),
                "login.msg.wrong",
            ),
            (
                ApiError::Unauthorized(AuthFailure::AccountLocked { until: None }),
                "login.msg.locked",
            ),
            (
                ApiError::Unauthorized(AuthFailure::Deactivated),
                "login.msg.turned_off",
            ),
            (
                ApiError::RateLimited { retry_after: None },
                "login.msg.too_many",
            ),
            (ApiError::PasswordChangeRequired, "login.msg.must_change"),
            (
                ApiError::Local(LocalValidation::InvalidValue),
                "login.msg.policy",
            ),
            (ApiError::Timeout, "login.msg.engine"),
        ];
        for (e, key) in cases {
            assert_eq!(sign_in_message(&e), key);
            assert!(crate::i18n::has(key), "{key}");
        }
        for (code, key) in [
            ("INVALID_CURRENT_PASSWORD", "login.msg.current_wrong"),
            ("PASSWORD_UNCHANGED", "login.msg.unchanged"),
            ("PASSWORD_BREACHED", "login.msg.breached"),
            ("PASSWORD_TOO_SHORT", "login.msg.policy"),
        ] {
            let e = cl_api::engine_refusal(
                400,
                &serde_json::json!({"error": {"code": code, "message": "x"}}),
            );
            assert_eq!(sign_in_message(&e), key, "{code}");
        }
        for key in [
            "login.msg.failed",
            "login.msg.again",
            "login.msg.tfa",
            "login.msg.mismatch",
            "login.msg.missing",
            "login.msg.auto_failed",
        ] {
            assert!(crate::i18n::has(key), "{key}");
        }
    }

    #[test]
    fn new_passwords_are_checked_before_sending() {
        assert_eq!(
            new_password_problem("short", "short"),
            Some("login.msg.policy")
        );
        assert_eq!(
            new_password_problem(&"x".repeat(73), &"x".repeat(73)),
            Some("login.msg.policy")
        );
        assert_eq!(
            new_password_problem("a-long-password", "a-long-passwrd"),
            Some("login.msg.mismatch")
        );
        assert_eq!(
            new_password_problem("a-long-password", "a-long-password"),
            None
        );
    }

    #[test]
    fn wrong_then_right_signs_in_and_wipes_the_field() {
        let (_d, mut p) = panel();
        assert!(sign_in(&mut p, OWNER_EMAIL, "not-it").is_none());
        assert_eq!(p.message(), Some("login.msg.wrong"));
        assert!(p.password.is_empty(), "wiped after sending");
        let Some(SignInEvent::SignedIn {
            name,
            email,
            password_changed,
            ..
        }) = sign_in(&mut p, OWNER_EMAIL, OWNER_PASSWORD)
        else {
            panic!("signed in");
        };
        assert_eq!(
            (name.as_str(), email.as_str(), password_changed),
            ("Pat Owner", OWNER_EMAIL, false)
        );
        assert!(p.link.signed_in());
    }

    #[test]
    fn locked_and_turned_off_read_in_plain_words() {
        let (_d, mut p) = panel();
        assert!(sign_in(&mut p, "kim@example.com", "kim-password-1").is_none());
        assert_eq!(p.message(), Some("login.msg.locked"));
        assert!(sign_in(&mut p, "alex@example.com", "alex-password-1").is_none());
        assert_eq!(p.message(), Some("login.msg.turned_off"));
        assert!(sign_in(&mut p, "", "").is_none());
        assert_eq!(p.message(), Some("login.msg.missing"));
    }

    #[test]
    fn a_temporary_password_asks_for_a_new_one_first() {
        let (_d, mut p) = panel();
        assert!(sign_in(&mut p, "lee@example.com", "TEMP-0000-DEMO-1").is_none());
        assert!(p.must_change());
        p.new_password = SecretText::from("a-fresh-password");
        p.confirm = SecretText::from("a-fresh-passw0rd");
        p.submit_change();
        assert_eq!(p.message(), Some("login.msg.mismatch"));
        p.new_password = SecretText::from("a-fresh-password");
        p.confirm = SecretText::from("a-fresh-password");
        p.submit_change();
        let Some(SignInEvent::SignedIn {
            password_changed,
            name,
            ..
        }) = p.poll()
        else {
            panic!("signed in after the change");
        };
        assert!(password_changed);
        assert_eq!(name, "Lee Bookkeeper");
        assert!(!p.must_change());
        assert!(p.waiting.is_none(), "the temporary password is gone");
        // Cancel instead signs the waiting user out.
        let (_d2, mut q) = panel();
        let _ = sign_in(&mut q, "lee@example.com", "TEMP-0000-DEMO-1");
        assert!(q.must_change());
        q.cancel_change();
        assert!(!q.must_change());
        assert!(!q.link.signed_in());
    }
}
