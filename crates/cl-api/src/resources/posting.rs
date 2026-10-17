//! Closed periods (NN8; P05 plan §2.7).
//!
//! - A save dated on or before the closing date is refused by the engine
//!   with 423 `TB_PERIOD_LOCKED`, which maps to `ApiError::ClosedPeriod`.
//!   Its `can_override` is information only.
//! - No request type has an `overrideConfirmed` or a `lockDate` field, and
//!   nothing here can add one: the engine reads `overrideConfirmed` from
//!   the request body, so the typed API simply never writes that key
//!   (checked by `tests/api_surface.rs` and the proptest below).
//! - The only way past a closed period will be the closing-date password
//!   (U-15). Until the engine advertises `closing-password-v1`, giving one
//!   fails locally with `Local(ClosedPeriodOverrideUnavailable)` and nothing
//!   is sent.

use std::fmt;

use cl_core::Secret;
use serde_json::Value;

use crate::errors::{ApiError, LocalValidation};
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// The owner's closing-date password for one save (U-15).
#[derive(Clone)]
pub struct ClosedPeriodAuthorization {
    password: Secret,
}

impl ClosedPeriodAuthorization {
    /// Authorizes a closed-period save with the closing-date password.
    #[must_use]
    pub fn with_password(password: Secret) -> ClosedPeriodAuthorization {
        ClosedPeriodAuthorization { password }
    }
}

impl fmt::Debug for ClosedPeriodAuthorization {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ClosedPeriodAuthorization(..)")
    }
}

/// Options of a posting (create, update, void).
#[derive(Clone, Debug, Default)]
pub struct PostingOptions {
    /// Closed-period authorization (only usable once U-15 is in the engine).
    pub closed_period: Option<ClosedPeriodAuthorization>,
}

/// The body of a posting with its options applied.
pub(crate) fn posting_body(
    session: &Session,
    mut body: Value,
    opts: &PostingOptions,
) -> Result<Vec<u8>, ApiError> {
    if let Some(auth) = &opts.closed_period {
        if !session
            .client()
            .capabilities()
            .has(Capabilities::CLOSING_PASSWORD)
        {
            return Err(ApiError::Local(
                LocalValidation::ClosedPeriodOverrideUnavailable,
            ));
        }
        if let Value::Object(map) = &mut body {
            map.insert(
                "closingPassword".into(),
                Value::String(auth.password.expose().to_string()),
            );
        }
    }
    json_body(&body)
}
