//! The catch-up run (P15 lane T, U-63, `recurring-run-due-v1`):
//! `POST /recurring/run-due {asOf, runId, limit}` and the side-effect-free
//! `GET /recurring/attention`.
//!
//! The engine decides which dates are due and posts each occurrence exactly
//! once under its `(template, date)` key; the app sends a date and a
//! `runId`, never amounts. A lost reply is recovered by re-asking with the
//! same `runId`. Amounts in the answer are display-only (strict
//! `Money::parse_wire`), never summed.
//!
//! WP-S scaffold: lane T adds the calls.

use crate::session::Session;

/// Catch-up calls of a session (`session.recurring()`).
#[derive(Clone, Copy, Debug)]
pub struct Recurring<'a>(
    #[allow(dead_code, reason = "P15 lane T adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// Recurring catch-up of the open company.
    #[must_use]
    pub fn recurring(&self) -> Recurring<'_> {
        Recurring(self)
    }
}
