//! The closing-date rules and password flow (non-negotiable 8, U-15),
//! shared by the register and every transaction form from P13 on.
//!
//! A transaction dated on or before the closing date is locked. Changing
//! it, voiding it, or moving an open transaction into the closed period
//! needs the closing-date password; the engine checks the password and
//! writes the audit trail. Here is only the decision ("does this edit need
//! the password?") and the dialog's state machine.
//!
//! The password is held from Submit until the engine answers, then dropped
//! (Q-P12j): it is asked for every save and never cached. `Debug` never
//! shows it.

use std::fmt;

use cl_core::{CalendarDate, Secret};

use crate::text::TextKey;

/// True when `date` is in the closed period (on or before `closing`). With
/// no closing date nothing is locked.
#[must_use]
pub fn is_locked(date: CalendarDate, closing: Option<CalendarDate>) -> bool {
    closing.is_some_and(|c| date <= c)
}

/// True when an edit needs the closing-date password: the transaction's
/// date before the edit (`old`, `None` for a new one) or after it (`new`)
/// is in the closed period.
#[must_use]
pub fn needs_password(
    old: Option<CalendarDate>,
    new: CalendarDate,
    closing: Option<CalendarDate>,
) -> bool {
    old.is_some_and(|o| is_locked(o, closing)) || is_locked(new, closing)
}

/// Where the closing-password dialog is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClosingStage {
    /// No dialog.
    Idle,
    /// Asking for the password.
    Asking,
    /// The password went to the engine with the change; waiting.
    Sending,
    /// The engine accepted the change.
    Done,
    /// The password was wrong; the dialog stays open for another try.
    Rejected,
    /// The engine cannot take a closing-date password yet
    /// (`closing-password-v1` missing): the dialog only explains.
    Unavailable,
    /// The owner cancelled; nothing changed.
    Cancelled,
    /// The save failed for another reason (shown by its own message).
    Failed,
}

/// What happened, fed to [`ClosingFlow::handle`].
pub enum ClosingEvent<S = Secret> {
    /// The owner started changing a locked transaction (the grid's locked
    /// row edit attempt, a void or delete of a locked row, a date moved into
    /// the closed period).
    LockedEdit,
    /// The engine refused a save with 423 `TB_PERIOD_LOCKED`.
    EngineClosedPeriod {
        /// The engine said a password would let it through.
        password_required: bool,
    },
    /// The owner pressed Change with this password.
    Submit(S),
    /// The engine refused the password (403 `CLOSING_PASSWORD_INVALID`).
    PasswordRejected,
    /// The engine saved the change.
    Saved,
    /// The save failed for another reason.
    SendFailed,
    /// Cancel (or OK on the "not available yet" message).
    Cancel,
}

impl<S> fmt::Debug for ClosingEvent<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClosingEvent::LockedEdit => f.write_str("LockedEdit"),
            ClosingEvent::EngineClosedPeriod { password_required } => f
                .debug_struct("EngineClosedPeriod")
                .field("password_required", password_required)
                .finish(),
            ClosingEvent::Submit(_) => f.write_str("Submit(***)"),
            ClosingEvent::PasswordRejected => f.write_str("PasswordRejected"),
            ClosingEvent::Saved => f.write_str("Saved"),
            ClosingEvent::SendFailed => f.write_str("SendFailed"),
            ClosingEvent::Cancel => f.write_str("Cancel"),
        }
    }
}

enum State<S> {
    Idle,
    Asking { attempt: u32 },
    Sending { attempt: u32, password: S },
    Done,
    Rejected { attempt: u32 },
    Unavailable,
    Cancelled,
    Failed,
}

/// The closing-password dialog's state machine:
///
/// `Idle → Asking{attempt} → Sending → Done | Rejected{attempt} | Failed`,
/// `Rejected → Sending` on another try, `Asking | Rejected | Unavailable →
/// Cancelled` on Cancel, and `Unavailable` instead of `Asking` when the
/// engine cannot take a password. A finished flow (`Done`, `Cancelled`,
/// `Failed`) starts again on the next locked edit.
///
/// `S` is the password type ([`Secret`] in the app; tests use a type that
/// counts drops). It is held only in `Sending`.
pub struct ClosingFlow<S = Secret> {
    supported: bool,
    state: State<S>,
}

impl<S> ClosingFlow<S> {
    /// A flow. `password_supported` is the engine's `closing-password-v1`
    /// capability.
    #[must_use]
    pub fn new(password_supported: bool) -> ClosingFlow<S> {
        ClosingFlow {
            supported: password_supported,
            state: State::Idle,
        }
    }

    /// Where the dialog is.
    #[must_use]
    pub fn stage(&self) -> ClosingStage {
        match self.state {
            State::Idle => ClosingStage::Idle,
            State::Asking { .. } => ClosingStage::Asking,
            State::Sending { .. } => ClosingStage::Sending,
            State::Done => ClosingStage::Done,
            State::Rejected { .. } => ClosingStage::Rejected,
            State::Unavailable => ClosingStage::Unavailable,
            State::Cancelled => ClosingStage::Cancelled,
            State::Failed => ClosingStage::Failed,
        }
    }

    /// Tries so far (1 for the first); 0 when no password is being asked.
    #[must_use]
    pub fn attempt(&self) -> u32 {
        match self.state {
            State::Asking { attempt }
            | State::Sending { attempt, .. }
            | State::Rejected { attempt } => attempt,
            _ => 0,
        }
    }

    /// True while the dialog is shown.
    #[must_use]
    pub fn dialog_open(&self) -> bool {
        matches!(
            self.state,
            State::Asking { .. }
                | State::Sending { .. }
                | State::Rejected { .. }
                | State::Unavailable
        )
    }

    /// The password to send with the change, only while `Sending`.
    #[must_use]
    pub fn password(&self) -> Option<&S> {
        match &self.state {
            State::Sending { password, .. } => Some(password),
            _ => None,
        }
    }

    /// The dialog's message. The `closing.ask` text takes `{date}`, the
    /// closing date written by the screen.
    #[must_use]
    pub fn message(&self) -> Option<TextKey> {
        match self.state {
            State::Asking { .. } | State::Sending { .. } => Some(TextKey("closing.ask")),
            State::Rejected { .. } => Some(TextKey("closing.wrong_password")),
            State::Unavailable => Some(TextKey("closing.unavailable")),
            _ => None,
        }
    }

    /// Applies `event`. Returns false (and changes nothing) when the event
    /// means nothing in the current stage, such as Cancel while the change
    /// is already on its way.
    pub fn handle(&mut self, event: ClosingEvent<S>) -> bool {
        let state = std::mem::replace(&mut self.state, State::Idle);
        let (next, changed) = self.step(state, event);
        self.state = next;
        changed
    }

    fn start(&self) -> State<S> {
        if self.supported {
            State::Asking { attempt: 1 }
        } else {
            State::Unavailable
        }
    }

    fn step(&self, state: State<S>, event: ClosingEvent<S>) -> (State<S>, bool) {
        use ClosingEvent as E;
        match (state, event) {
            (
                State::Idle | State::Done | State::Cancelled | State::Failed,
                E::LockedEdit
                | E::EngineClosedPeriod {
                    password_required: true,
                },
            ) => (self.start(), true),
            (
                State::Idle | State::Done | State::Cancelled | State::Failed,
                E::EngineClosedPeriod {
                    password_required: false,
                },
            ) => (State::Unavailable, true),
            (State::Asking { attempt }, E::Submit(password)) => {
                (State::Sending { attempt, password }, true)
            }
            // Another try after a wrong password counts as the next attempt.
            (State::Rejected { attempt }, E::Submit(password)) => (
                State::Sending {
                    attempt: attempt.saturating_add(1),
                    password,
                },
                true,
            ),
            // A wrong password, or still locked after the password went
            // with the change (the closing date moved): ask again.
            (
                State::Sending { attempt, .. },
                E::PasswordRejected | E::EngineClosedPeriod { .. },
            ) => (State::Rejected { attempt }, true),
            (State::Sending { .. }, E::Saved) => (State::Done, true),
            (State::Sending { .. }, E::SendFailed) => (State::Failed, true),
            (State::Asking { .. } | State::Rejected { .. } | State::Unavailable, E::Cancel) => {
                (State::Cancelled, true)
            }
            (state, _) => (state, false),
        }
    }
}

impl<S> fmt::Debug for ClosingFlow<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClosingFlow")
            .field("supported", &self.supported)
            .field("stage", &self.stage())
            .field("attempt", &self.attempt())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
