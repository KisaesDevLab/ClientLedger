//! The closing-date password prompt every transaction form shares (P13
//! WP5.2; non-negotiable 8). It drives `cl_model::closing::ClosingFlow`
//! and draws the dialog; the screen decides what a change is (`P`) and
//! sends it.
//!
//! The password is typed into a masked, zeroizing field, handed to the one
//! change it is for, and dropped when the engine answers (Q-P12j: asked for
//! every change, never kept). A wrong password keeps the dialog open; too
//! many tries is a plain message and Change stays off until the engine's
//! wait is over. Without `closing-password-v1` the dialog only explains
//! that closed-period transactions can't be changed yet.
//!
//! Nothing here is logged; the control-channel state carries flags only.

use std::time::{Duration, Instant};

use cl_api::ApiError;
use cl_core::{CalendarDate, Secret};
use cl_model::CommandId;
use cl_model::closing::{ClosingEvent, ClosingFlow, ClosingStage};
use eframe::egui::{self, RichText, Ui};

use crate::i18n::{t, t_with};
use crate::screens::company::form::{self, SecretText};
use crate::theme::Tokens;

/// The wait after too many wrong closing-date passwords when the engine
/// does not say (its window: 10 tries in 15 minutes).
pub const RATE_LIMIT_FALLBACK: Duration = Duration::from_mins(15);

/// What the owner clicked in the dialog this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosingClick {
    /// Nothing.
    None,
    /// Change (or Enter in the password field).
    Submit,
    /// Cancel, or OK on the "not available yet" message.
    Cancel,
}

/// What the password is for: the dialog's words and its button (P15: a
/// delete asks "to delete it" and its button says Delete, drawn as a
/// destructive action).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verb {
    /// A save of an edit (Change).
    #[default]
    Change,
    /// Void (Void).
    Void,
    /// Delete (Delete, destructive).
    Delete,
}

impl Verb {
    fn ask_key(self) -> &'static str {
        match self {
            Verb::Change => "closing.ask",
            Verb::Void => "closing.ask.void",
            Verb::Delete => "closing.ask.delete",
        }
    }

    fn button_key(self) -> &'static str {
        match self {
            Verb::Change => "closing.change",
            Verb::Void => "closing.void",
            Verb::Delete => "closing.delete",
        }
    }
}

/// The closing-date password flow for changes of type `P` (no `Debug` of
/// the change or the password).
pub struct ClosingPrompt<P> {
    /// The flow (stage, attempts, the password on its way).
    pub(crate) flow: ClosingFlow<Secret>,
    /// The closing date the dialog names.
    closing_date: Option<CalendarDate>,
    /// The change the password is for (kept for another try).
    pub(crate) pending: Option<P>,
    /// The password as typed (masked, zeroized).
    pub(crate) password: SecretText,
    focus_password: bool,
    /// Too many wrong passwords: Change stays off until then.
    pub(crate) limited_until: Option<Instant>,
    /// What the password is for.
    verb: Verb,
}

impl<P> Default for ClosingPrompt<P> {
    fn default() -> ClosingPrompt<P> {
        ClosingPrompt {
            flow: ClosingFlow::new(false),
            closing_date: None,
            pending: None,
            password: SecretText::default(),
            focus_password: false,
            limited_until: None,
            verb: Verb::Change,
        }
    }
}

impl<P> std::fmt::Debug for ClosingPrompt<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosingPrompt")
            .field("flow", &self.flow)
            .field("pending", &self.pending.is_some())
            .finish_non_exhaustive()
    }
}

impl<P: Clone> ClosingPrompt<P> {
    /// The dialog is up.
    #[must_use]
    pub fn dialog_open(&self) -> bool {
        self.flow.dialog_open()
    }

    /// Where the flow is.
    #[must_use]
    pub fn stage(&self) -> ClosingStage {
        self.flow.stage()
    }

    /// Passwords tried for the current change.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        self.flow.attempt()
    }

    /// The change waiting for the password.
    #[must_use]
    pub fn pending(&self) -> Option<&P> {
        self.pending.as_ref()
    }

    /// A fresh flow unless the dialog is up.
    fn restart(&mut self, supported: bool) {
        if !self.flow.dialog_open() {
            self.flow = ClosingFlow::new(supported);
            self.password.clear();
            self.verb = Verb::Change;
        }
    }

    /// What the password is for (the dialog's words and button). Set it
    /// after [`ClosingPrompt::ask`] or [`ClosingPrompt::failed`]; a fresh
    /// flow starts at [`Verb::Change`].
    pub fn set_verb(&mut self, verb: Verb) {
        self.verb = verb;
    }

    /// What the password is for.
    #[must_use]
    pub fn verb(&self) -> Verb {
        self.verb
    }

    /// Starts the password flow for `pending` (a locked change found by
    /// the screen). `supported`: the engine takes a closing-date password.
    pub fn ask(&mut self, pending: P, supported: bool, closing_date: Option<CalendarDate>) {
        self.restart(supported);
        self.closing_date = closing_date;
        self.flow.handle(ClosingEvent::LockedEdit);
        self.pending = Some(pending);
        self.focus_password = true;
    }

    /// A locked change can't be made with this engine: explain.
    pub fn unavailable(&mut self, supported: bool, closing_date: Option<CalendarDate>) {
        self.restart(supported);
        self.closing_date = closing_date;
        self.flow.handle(ClosingEvent::LockedEdit);
        self.pending = None;
    }

    /// Feeds a failed change to the flow. True when the dialog took it
    /// (nothing else should show it). `closing_date` is the screen's own
    /// copy, named when the engine's answer has none.
    pub fn failed(
        &mut self,
        e: &ApiError,
        pending: P,
        supported: bool,
        closing_date: Option<CalendarDate>,
    ) -> bool {
        let sending = self.flow.stage() == ClosingStage::Sending;
        match e {
            // Too many wrong passwords: the dialog stays open with the
            // change kept, and Change stays off until the engine's wait is
            // over (its window is 15 minutes when it does not say).
            ApiError::ClosingPasswordRateLimited { retry_after } if sending => {
                let wait = retry_after.unwrap_or(RATE_LIMIT_FALLBACK);
                self.limited_until = Some(Instant::now() + wait);
                self.flow.handle(ClosingEvent::PasswordRejected);
                self.pending = Some(pending);
                true
            }
            ApiError::ClosingPasswordRejected if sending => {
                self.flow.handle(ClosingEvent::PasswordRejected);
                self.pending = Some(pending);
                self.focus_password = true;
                true
            }
            ApiError::ClosedPeriod(c) => {
                if c.lock_date.is_some() {
                    self.closing_date = c.lock_date;
                } else if self.closing_date.is_none() {
                    self.closing_date = closing_date;
                }
                if !sending {
                    self.restart(supported);
                }
                let required = c.password_required == Some(true) && supported;
                self.flow.handle(ClosingEvent::EngineClosedPeriod {
                    password_required: required,
                });
                self.pending = required.then_some(pending);
                self.focus_password = required;
                true
            }
            _ => {
                if sending {
                    self.flow.handle(ClosingEvent::SendFailed);
                    self.pending = None;
                }
                false
            }
        }
    }

    /// The change the password was for went through.
    pub fn saved(&mut self) {
        if self.flow.stage() == ClosingStage::Sending {
            self.flow.handle(ClosingEvent::Saved);
        }
        self.pending = None;
    }

    /// The minutes left before another password may be tried, while the
    /// engine's attempt limit is in force.
    #[must_use]
    pub fn wait_minutes(&self) -> Option<u64> {
        let left = self.limited_until?.checked_duration_since(Instant::now())?;
        (!left.is_zero()).then(|| left.as_secs().div_ceil(60).max(1))
    }

    /// Change: the pending change and the typed password, for the screen
    /// to send. `None` while the attempt limit is in force, with no
    /// password typed (the field gets the focus) or no change waiting. The
    /// typed password is wiped either way once taken.
    pub fn submit(&mut self) -> Option<(P, Secret)> {
        if self.wait_minutes().is_some() {
            return None;
        }
        if self.password.is_empty() {
            self.focus_password = true;
            return None;
        }
        let pending = self.pending.clone()?;
        let secret = Secret::new(egui::TextBuffer::as_str(&self.password).to_string());
        self.password.clear();
        if !self.flow.handle(ClosingEvent::Submit(secret.clone())) {
            return None;
        }
        Some((pending, secret))
    }

    /// The screen sent nothing after [`ClosingPrompt::submit`] (the entry
    /// needs fixing, or no session): the password is dropped and the
    /// dialog closes.
    pub fn nothing_sent(&mut self) {
        if self.flow.stage() == ClosingStage::Sending {
            self.flow.handle(ClosingEvent::SendFailed);
            self.pending = None;
        }
    }

    /// Cancel (or OK on the "not available yet" message): nothing changes.
    pub fn cancel(&mut self) {
        self.limited_until = None;
        self.flow.handle(ClosingEvent::Cancel);
        self.pending = None;
        self.password.clear();
    }

    /// Keymap commands while the dialog is up: Esc cancels (not while the
    /// password is on its way) and the saves wait. `None` when the dialog
    /// is closed or the command is not the dialog's.
    pub fn command(&mut self, id: CommandId) -> Option<bool> {
        if !self.flow.dialog_open() {
            return None;
        }
        match id.as_str() {
            "form.revert" => {
                if self.flow.stage() != ClosingStage::Sending {
                    self.cancel();
                }
                Some(true)
            }
            // The dialog has the keyboard: nothing else runs under it.
            "form.save_new" | "form.save_close" | "form.delete_item" | "edit.void_transaction" => {
                Some(true)
            }
            _ => None,
        }
    }

    /// The control-channel state (flags only; never the password or the
    /// change).
    #[must_use]
    pub fn state_json(&self, supported: bool) -> serde_json::Value {
        serde_json::json!({
            "closing": format!("{:?}", self.flow.stage()).to_lowercase(),
            "closing_attempt": self.flow.attempt(),
            "closing_wait_minutes": self.wait_minutes(),
            "closing_supported": supported,
            "closing_verb": format!("{:?}", self.verb).to_lowercase(),
        })
    }

    /// Draws the dialog (when it is up) in the form and says what was
    /// clicked. The screen runs [`ClosingPrompt::submit`] or
    /// [`ClosingPrompt::cancel`].
    pub fn ui(&mut self, ui: &mut Ui, tokens: &Tokens) -> ClosingClick {
        if !self.flow.dialog_open() {
            return ClosingClick::None;
        }
        form::card(ui, tokens, |ui| self.body(ui, tokens, true))
    }

    /// [`ClosingPrompt::ui`] in a window over the form (P15), so the
    /// form's own buttons stay where they are; `salt` names the window.
    pub fn window(&mut self, ctx: &egui::Context, tokens: &Tokens, salt: &str) -> ClosingClick {
        if !self.flow.dialog_open() {
            return ClosingClick::None;
        }
        let width = (ctx.content_rect().width() - 48.0).clamp(240.0, WINDOW_WIDTH);
        let mut click = ClosingClick::None;
        egui::Window::new(t("closing.title"))
            .id(egui::Id::unique(("closing-prompt", salt)))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_width(width);
                click = self.body(ui, tokens, false);
            });
        click
    }

    /// The dialog's content; `heading`: draw the title (the window has its
    /// own).
    fn body(&mut self, ui: &mut Ui, tokens: &Tokens, heading: bool) -> ClosingClick {
        let stage = self.flow.stage();
        let date = self
            .closing_date
            .map(crate::shell::us_date)
            .unwrap_or_default();
        let mut submit = false;
        let mut cancel = false;
        {
            if heading {
                form::heading(ui, tokens, &t("closing.title"));
            }
            if stage == ClosingStage::Unavailable {
                ui.add(egui::Label::new(t("closing.unavailable")).wrap());
                if ui.button(t("closing.ok")).clicked() {
                    return ClosingClick::Cancel;
                }
                return ClosingClick::None;
            }
            ui.add(egui::Label::new(t_with(self.verb.ask_key(), &[("date", &date)])).wrap());
            let wait = self.wait_minutes();
            if let Some(minutes) = wait {
                ui.add(
                    egui::Label::new(
                        RichText::new(t_with(
                            "closing.rate_limited",
                            &[("minutes", &minutes.to_string())],
                        ))
                        .color(tokens.warn_text),
                    )
                    .wrap(),
                );
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            } else if stage == ClosingStage::Rejected {
                ui.add(
                    egui::Label::new(
                        RichText::new(t("closing.wrong_password")).color(tokens.warn_text),
                    )
                    .wrap(),
                );
            }
            let sending = stage == ClosingStage::Sending;
            ui.add_enabled_ui(!sending, |ui| {
                ui.add_enabled_ui(wait.is_none(), |ui| {
                    let r = form::password_secret(ui, &t("closing.password"), &mut self.password);
                    if std::mem::take(&mut self.focus_password) {
                        r.request_focus();
                    }
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                });
                ui.horizontal(|ui| {
                    let label = t(self.verb.button_key());
                    let verb = self.verb;
                    submit |= ui
                        .add_enabled_ui(wait.is_none(), |ui| {
                            if verb == Verb::Delete {
                                crate::widgets::destructive_button(ui, &label)
                            } else {
                                ui.button(label)
                            }
                        })
                        .inner
                        .clicked();
                    // Beside the tall Delete, a Cancel of the same height.
                    let height = if verb == Verb::Delete { 44.0 } else { 0.0 };
                    cancel = ui
                        .add(
                            egui::Button::new(t("closing.cancel"))
                                .min_size(egui::Vec2::new(0.0, height)),
                        )
                        .clicked();
                });
            });
            if sending {
                form::progress_line(ui, tokens, &t("closing.sending"));
            }
        }
        if submit {
            ClosingClick::Submit
        } else if cancel {
            ClosingClick::Cancel
        } else {
            ClosingClick::None
        }
    }
}

/// The window's width when the app has room.
const WINDOW_WIDTH: f32 = 420.0;

#[cfg(test)]
mod tests {
    use super::*;
    fn closed(required: bool) -> ApiError {
        ApiError::ClosedPeriod(cl_api::ClosedPeriod {
            lock_date: None,
            can_override: false,
            password_required: Some(required),
            message: cl_api::UserText::default(),
        })
    }

    #[test]
    fn a_wrong_password_keeps_the_change_and_wipes_the_field() {
        let mut p: ClosingPrompt<u8> = ClosingPrompt::default();
        p.ask(7, true, None);
        assert!(p.dialog_open());
        assert!(p.submit().is_none(), "no password typed");
        p.password = SecretText::from("wrong");
        let (change, _secret) = p.submit().expect("sent");
        assert_eq!(change, 7);
        assert!(p.password.is_empty());
        assert_eq!(p.stage(), ClosingStage::Sending);
        assert!(p.failed(&ApiError::ClosingPasswordRejected, 7, true, None));
        assert_eq!(p.stage(), ClosingStage::Rejected);
        assert_eq!(p.pending(), Some(&7));
        p.password = SecretText::from("right");
        assert!(p.submit().is_some());
        p.saved();
        assert!(!p.dialog_open());
        assert!(p.pending().is_none());
        assert!(p.flow.password().is_none());
    }

    #[test]
    fn engine_closed_period_and_unavailable() {
        let mut p: ClosingPrompt<u8> = ClosingPrompt::default();
        assert!(p.failed(&closed(true), 1, false, None));
        assert_eq!(p.stage(), ClosingStage::Unavailable);
        assert!(p.pending().is_none());
        assert_eq!(p.command(CommandId::new("form.save_new")), Some(true));
        assert_eq!(p.command(CommandId::new("form.revert")), Some(true));
        assert!(!p.dialog_open());
        assert_eq!(p.command(CommandId::new("form.revert")), None);
        assert!(p.failed(&closed(true), 2, true, None));
        assert_eq!(p.pending(), Some(&2));
        assert_eq!(p.state_json(true)["closing_supported"], true);
        p.password = SecretText::from("x");
        assert!(p.submit().is_some());
        p.nothing_sent();
        assert!(!p.dialog_open());
        assert!(!p.failed(&ApiError::ClosingPasswordRejected, 3, true, None));
    }

    #[test]
    fn the_verb_names_the_action_until_a_fresh_flow() {
        let mut p: ClosingPrompt<u8> = ClosingPrompt::default();
        p.ask(1, true, None);
        p.set_verb(Verb::Delete);
        assert_eq!(p.verb(), Verb::Delete);
        assert_eq!(p.state_json(true)["closing_verb"], "delete");
        assert_eq!(Verb::Delete.button_key(), "closing.delete");
        for v in [Verb::Change, Verb::Void, Verb::Delete] {
            assert!(crate::i18n::has(v.ask_key()));
            assert!(crate::i18n::has(v.button_key()));
        }
        p.cancel();
        p.ask(2, true, None);
        assert_eq!(p.verb(), Verb::Change, "a fresh flow is a change again");
    }

    #[test]
    fn too_many_tries_turns_change_off() {
        let mut p: ClosingPrompt<u8> = ClosingPrompt::default();
        p.ask(1, true, None);
        p.password = SecretText::from("x");
        assert!(p.submit().is_some());
        let limited = ApiError::ClosingPasswordRateLimited {
            retry_after: Some(Duration::from_secs(150)),
        };
        assert!(p.failed(&limited, 1, true, None));
        assert_eq!(p.wait_minutes(), Some(3));
        p.password = SecretText::from("y");
        assert!(p.submit().is_none());
        p.cancel();
        assert!(p.limited_until.is_none());
        assert!(p.password.is_empty());
    }
}
