//! Idle lock (Q-69, Q-P29p): after the chosen idle minutes (Preferences
//! kept per computer; off by default), or File › Lock ClientLedger, the
//! whole app is covered by a lock screen.
//!
//! - Windows are not drawn while locked, so nothing behind the lock is in
//!   the accessibility tree or reachable by keys; every command but the
//!   unlock is refused. Unsaved windows stay as they were.
//! - Unlocking always asks for the password and checks it by signing in
//!   through `POST /auth/login` (lockout counting applies). The new
//!   session is only the proof: it must be the same user, it is signed out
//!   at once, and the app keeps its own session (the open company and any
//!   queued saves). Automatic sign-in is never used here.
//! - A different person signs out from the lock screen; with unsaved
//!   windows that needs the owner's password first (unlock, then save or
//!   discard).

use std::sync::Arc;
use std::time::Duration;

use cl_api::{ApiError, AuthFailure, LoginOutcome};
use cl_core::Secret;
use eframe::egui::{self, Context, Frame, Margin, RichText};

use crate::company::{CompanyLink, Task, spawn_lifecycle};
use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, SecretText};
use crate::screens::login::sign_in_message;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// What the lock screen asks the shell to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockAction {
    /// The password was right: the app is unlocked.
    Unlocked,
    /// Sign out (only offered with no unsaved windows).
    SignOut,
}

/// The idle timer and the lock screen.
#[derive(Default)]
pub struct IdleLock {
    locked: bool,
    /// egui time of the last input.
    last_input: Option<f64>,
    password: SecretText,
    task: Option<Task<Result<(), ApiError>>>,
    message: Option<&'static str>,
}

impl std::fmt::Debug for IdleLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdleLock")
            .field("locked", &self.locked)
            .finish_non_exhaustive()
    }
}

/// True when `minutes` of idle time have passed since `last` (0 = never).
#[must_use]
pub fn idle_expired(minutes: u16, last: f64, now: f64) -> bool {
    minutes > 0 && now - last >= f64::from(minutes) * 60.0
}

impl IdleLock {
    /// True while the app is locked.
    #[must_use]
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The message key the lock screen shows, if any.
    #[must_use]
    pub fn message(&self) -> Option<&'static str> {
        self.message
    }

    /// Lock now (File › Lock ClientLedger, or the timer).
    pub fn lock_now(&mut self) {
        if !self.locked {
            tracing::info!("app locked");
        }
        self.locked = true;
        self.message = None;
        self.password.clear();
    }

    /// Clears the lock without a password (only after Sign out, which
    /// shows the sign-in form instead).
    pub fn reset(&mut self) {
        self.locked = false;
        self.password.clear();
        self.task = None;
        self.message = None;
        self.last_input = None;
    }

    /// Counts input and locks after `minutes` idle (0 = off). Only while
    /// someone is signed in. Asks for a repaint when the lock is due, so it
    /// happens even while nothing else draws.
    pub fn tick(&mut self, ctx: &Context, minutes: u16, signed_in: bool) {
        let (now, active) = ctx.input(|i| {
            let active = !i.events.is_empty()
                || i.pointer.delta() != egui::Vec2::ZERO
                || i.pointer.any_down();
            (i.time, active)
        });
        if self.locked {
            return;
        }
        if active || self.last_input.is_none() || !signed_in || minutes == 0 {
            self.last_input = Some(now);
            if minutes > 0 && signed_in {
                ctx.request_repaint_after(Duration::from_secs(u64::from(minutes) * 60));
            }
            return;
        }
        let last = self.last_input.unwrap_or(now);
        if idle_expired(minutes, last, now) {
            self.lock_now();
        } else {
            let left = f64::from(minutes) * 60.0 - (now - last);
            ctx.request_repaint_after(Duration::from_secs_f64(left.max(0.5)));
        }
    }

    fn start_unlock(&mut self, link: &Arc<CompanyLink>) {
        if self.password.is_empty() {
            self.message = Some("lock.msg.missing");
            return;
        }
        let password = self.password.to_secret();
        self.password.clear();
        self.message = None;
        let Some(email) = link.signed_in_email() else {
            self.message = Some("login.msg.failed");
            return;
        };
        let demo = link
            .demo_state()
            .map(|mut d| d.auth.check(&email, &password).map(drop));
        if let Some(r) = demo {
            self.task = Some(Task::ready(r));
            return;
        }
        let (Some(group), Some(client), Some(own)) = (
            link.group().and_then(Result::ok),
            link.client().cloned(),
            link.session(),
        ) else {
            self.message = Some("login.msg.failed");
            return;
        };
        self.task = spawn_lifecycle(link, &group, move || async move {
            proof_of_password(&client, &own, &email, &password).await
        })
        .ok();
    }

    /// Reads the unlock answer.
    fn poll(&mut self) -> Option<LockAction> {
        let r = self.task.as_mut()?.poll_result()?;
        self.task = None;
        match r {
            Ok(()) => {
                self.locked = false;
                self.last_input = None;
                tracing::info!("app unlocked");
                Some(LockAction::Unlocked)
            }
            Err(e) => {
                self.message = Some(sign_in_message(&e));
                None
            }
        }
    }

    /// Draws the lock screen over the whole app. `dirty` is the number of
    /// unsaved windows (Sign out waits for them).
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        link: &Arc<CompanyLink>,
        user: Option<&str>,
        dirty: usize,
    ) -> Option<LockAction> {
        if let Some(a) = self.poll() {
            return Some(a);
        }
        let ctx = ui.ctx().clone();
        let tokens = Tokens::of(&ctx);
        let mut action = None;
        Frame::new().fill(tokens.surface_app).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.add_space((ui.available_height() * 0.2).max(24.0));
            ui.vertical_centered(|ui| {
                ui.set_max_width(460.0);
                Frame::new()
                    .fill(tokens.surface_window)
                    .stroke(egui::Stroke::new(1.0, tokens.border_strong))
                    .corner_radius(8.0)
                    .inner_margin(Margin::symmetric(28, 22))
                    .show(ui, |ui| action = self.card(ui, &tokens, link, user, dirty));
            });
        });
        if self.task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        action
    }

    fn card(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        link: &Arc<CompanyLink>,
        user: Option<&str>,
        dirty: usize,
    ) -> Option<LockAction> {
        form::heading(ui, tokens, &t("lock.title"));
        let who = user.map_or_else(
            || t("lock.intro"),
            |u| t_with("lock.intro_user", &[("user", u)]),
        );
        form::hint(ui, tokens, &who);
        if dirty > 0 {
            form::hint(ui, tokens, &t("lock.unsaved"));
        }
        ui.add_space(6.0);
        let r = form::password_secret(ui, &t("lock.password"), &mut self.password);
        if !r.has_focus() && self.task.is_none() && !ui.ctx().memory(|m| m.focused().is_some()) {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if let Some(key) = self.message {
            form::inline_warning(ui, tokens, &t(key));
        }
        ui.add_space(6.0);
        if self.task.is_some() {
            form::progress_line(ui, tokens, &t("lock.working"));
            return None;
        }
        let mut action = None;
        ui.horizontal(|ui| {
            if primary_button(ui, &t("lock.unlock")).clicked() || enter {
                self.start_unlock(link);
            }
            let out = ui.add_enabled(dirty == 0, egui::Button::new(t("lock.sign_out")));
            let out = if dirty > 0 {
                out.on_disabled_hover_text(t("lock.sign_out_unsaved"))
            } else {
                out
            };
            if out.clicked() {
                action = Some(LockAction::SignOut);
            }
        });
        ui.add_space(4.0);
        ui.label(
            RichText::new(t("lock.footer"))
                .size(12.0)
                .color(tokens.text_secondary),
        );
        action
    }
}

/// Signs in again with `email` and `password` as proof: the answer must be
/// the same user as `own`; the proof session is signed out at once.
async fn proof_of_password(
    client: &cl_api::Client,
    own: &cl_api::Session,
    email: &str,
    password: &Secret,
) -> Result<(), ApiError> {
    match client.login(email, password).await? {
        LoginOutcome::Session(proof) => {
            let same = proof.user_id().is_some() && proof.user_id() == own.user_id();
            let _ = proof.logout().await;
            if same {
                Ok(())
            } else {
                Err(ApiError::Unauthorized(AuthFailure::InvalidCredentials))
            }
        }
        LoginOutcome::TfaRequired(_) => Err(ApiError::Unauthorized(AuthFailure::TfaInvalid)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::company::demo::auth::OWNER_PASSWORD;

    #[test]
    fn the_timer_counts_whole_minutes_and_zero_is_off() {
        assert!(!idle_expired(0, 0.0, 1.0e9));
        assert!(!idle_expired(5, 100.0, 399.0));
        assert!(idle_expired(5, 100.0, 400.0));
        assert!(idle_expired(60, 0.0, 3600.0));
    }

    #[test]
    fn unlock_needs_the_right_password_and_counts_wrong_ones() {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let mut lock = IdleLock::default();
        lock.lock_now();
        lock.start_unlock(&link);
        assert_eq!(lock.message(), Some("lock.msg.missing"));
        lock.password = SecretText::from("not-the-one");
        lock.start_unlock(&link);
        assert_eq!(lock.poll(), None);
        assert!(lock.locked());
        assert_eq!(lock.message(), Some("login.msg.wrong"));
        assert!(lock.password.is_empty(), "wiped after sending");
        let failed = link
            .demo_state()
            .and_then(|d| d.auth.current().map(|u| u.failed))
            .unwrap_or_default();
        assert_eq!(failed, 1, "lockout counting applies");
        lock.password = SecretText::from(OWNER_PASSWORD);
        lock.start_unlock(&link);
        assert_eq!(lock.poll(), Some(LockAction::Unlocked));
        assert!(!lock.locked());
    }

    #[test]
    fn idle_time_locks_and_input_keeps_it_open() {
        let ctx = egui::Context::default();
        let mut lock = IdleLock::default();
        let frame = |ctx: &egui::Context, lock: &mut IdleLock, time: f64, key: bool| {
            let mut input = egui::RawInput {
                time: Some(time),
                ..egui::RawInput::default()
            };
            if key {
                input.events.push(egui::Event::Text("a".into()));
            }
            let mut o = ctx.run_ui(input, |ui| lock.tick(ui.ctx(), 5, true));
            o.textures_delta.clear();
        };
        frame(&ctx, &mut lock, 0.0, false);
        frame(&ctx, &mut lock, 200.0, true);
        frame(&ctx, &mut lock, 400.0, false);
        assert!(!lock.locked(), "input at 200 s restarted the five minutes");
        frame(&ctx, &mut lock, 501.0, false);
        assert!(lock.locked());
        // Signed out: never locks.
        let mut off = IdleLock::default();
        let mut o = ctx.run_ui(
            egui::RawInput {
                time: Some(0.0),
                ..Default::default()
            },
            |ui| off.tick(ui.ctx(), 5, false),
        );
        o.textures_delta.clear();
        let mut o = ctx.run_ui(
            egui::RawInput {
                time: Some(9999.0),
                ..Default::default()
            },
            |ui| off.tick(ui.ctx(), 5, false),
        );
        o.textures_delta.clear();
        assert!(!off.locked());
    }
}
