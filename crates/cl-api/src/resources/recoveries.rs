//! Bad-debt recoveries (P23, engine U-154; contract per the P23 plan §2.8,
//! verified at the WP-4 pin bump).
//!
//! A recovery reopens part of a written-off invoice (Dr A/R, Cr the
//! write-off account and, with SSUTA on, Sales Tax Payable for the
//! recovered tax the engine works out); the owner then receives the
//! payment normally (Q-P23q). Keyed under `idempotency-recovery-v1`;
//! closing on the date. The recovered tax and the reopened balance are the
//! engine's figures.

use std::fmt;

use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{
    PAYMENT_MEMO_MAX, de_cents, de_cents_opt, error_code, require, ser_cents, too_long,
};
use crate::resources::operations::{WRITE_OFF_RECOVERY_CREATE, WRITE_OFFS_LIST};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::sales_tax::decode_one;
use crate::session::Session;
use crate::wire::Capabilities;

/// The engine's refusals of a recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecoveryRefusal {
    /// More than is left written off (409 `RECOVERY_EXCEEDS_WRITE_OFF`
    /// `{available}`; read it with [`recovery_available`]).
    ExceedsWriteOff,
    /// The recovery's reopened balance was paid: void the payment first
    /// (409 `RECOVERY_IN_USE`).
    InUse,
    /// The date is before the write-off (400 `RECOVERY_DATE_INVALID`
    /// `{earliestDate}`).
    DateInvalid,
}

const REFUSALS: &[(&str, RecoveryRefusal)] = &[
    (
        "RECOVERY_EXCEEDS_WRITE_OFF",
        RecoveryRefusal::ExceedsWriteOff,
    ),
    ("RECOVERY_IN_USE", RecoveryRefusal::InUse),
    ("RECOVERY_DATE_INVALID", RecoveryRefusal::DateInvalid),
];

/// Every engine code [`recovery_refusal`] knows.
#[must_use]
pub fn recovery_refusal_codes() -> Vec<&'static str> {
    REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn recovery_refusal(e: &ApiError) -> Option<RecoveryRefusal> {
    let code = error_code(e)?;
    REFUSALS.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
}

/// What is left to recover, from a 409 `RECOVERY_EXCEEDS_WRITE_OFF`
/// (`None` for any other error or a malformed figure).
#[must_use]
pub fn recovery_available(e: &ApiError) -> Option<Money> {
    match e {
        ApiError::Conflict { details, .. }
            if recovery_refusal(e) == Some(RecoveryRefusal::ExceedsWriteOff) =>
        {
            details.money("available")
        }
        _ => None,
    }
}

/// `POST /payments/write-off-recoveries`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewRecovery {
    /// The written-off invoice.
    pub invoice_id: TransactionId,
    /// The recovery date.
    pub date: CalendarDate,
    /// The amount recovered (above zero, whole cents).
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// Memo (owner data).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
}

impl fmt::Debug for NewRecovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewRecovery")
            .field("invoice_id", &self.invoice_id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

impl NewRecovery {
    /// Checks the request before anything is sent.
    ///
    /// # Errors
    /// `Local(NonPositiveAmount)` for an amount not above zero;
    /// `Local(InvalidValue)` for a fraction of a cent or a long memo.
    pub fn check(&self) -> Result<(), ApiError> {
        if !self.amount.is_positive() {
            return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
        }
        crate::resources::customer_payments::cents_wire(self.amount)?;
        if too_long(self.memo.as_deref(), PAYMENT_MEMO_MAX) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(())
    }
}

/// What a recovery answered (201).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryResult {
    /// The `bad_debt_recovery` transaction.
    pub id: TransactionId,
    /// The invoice's balance due now (the engine's figure).
    #[serde(deserialize_with = "de_cents")]
    pub invoice_balance_due: Money,
    /// The sales tax recovered (`None` with SSUTA off or from engines that
    /// don't send it).
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub recovered_tax: Option<Money>,
}

impl fmt::Debug for RecoveryResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecoveryResult")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Bad-debt recovery operations of a session (`session.recoveries()`).
#[derive(Clone, Copy, Debug)]
pub struct Recoveries<'a>(pub(crate) &'a Session);

impl Session {
    /// Bad-debt recoveries of the open company (P23).
    #[must_use]
    pub fn recoveries(&self) -> Recoveries<'_> {
        Recoveries(self)
    }
}

impl Recoveries<'_> {
    /// Records a recovery; the invoice reopens by the amount.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `bad-debt-recovery-v1`; `Local(…)`
    /// from [`NewRecovery::check`]; from the engine see
    /// [`recovery_refusal`]; closing on the date; any [`ApiError`].
    pub async fn create(
        &self,
        r: &NewRecovery,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<RecoveryResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::BAD_DEBT_RECOVERY)?;
        r.check()?;
        let body =
            serde_json::to_value(r).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, WRITE_OFF_RECOVERY_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&WRITE_OFF_RECOVERY_CREATE, call, None).await?;
        decode_one(&reply.body, "recovery", "bad-debt recovery")
            .map_err(crate::resources::versioning::saved_unreadable)
    }
}

/// One written-off invoice (`GET /payments/write-offs`, engine PR #221).
/// Every figure is the engine's.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteOff {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: Option<String>,
    /// Its client.
    #[serde(default)]
    pub customer_id: Option<cl_core::ids::ContactId>,
    /// The latest write-off date (a recovery can't be dated before the
    /// first, `firstWriteOffDate`).
    #[serde(default)]
    pub write_off_date: Option<CalendarDate>,
    /// The first write-off date.
    #[serde(default)]
    pub first_write_off_date: Option<CalendarDate>,
    /// Written off in total.
    #[serde(deserialize_with = "de_cents")]
    pub written_off: Money,
    /// Recovered so far.
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_zero"
    )]
    pub recovered: Money,
    /// What can still be recovered.
    #[serde(deserialize_with = "de_cents")]
    pub recoverable: Money,
}

impl fmt::Debug for WriteOff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteOff")
            .field("invoice_id", &self.invoice_id)
            .finish_non_exhaustive()
    }
}

impl Recoveries<'_> {
    /// The written-off invoices, oldest first (`customer` filters;
    /// `include_recovered` keeps fully recovered ones).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `bad-debt-recovery-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn write_offs(
        &self,
        customer: Option<cl_core::ids::ContactId>,
        include_recovered: bool,
    ) -> Result<Vec<WriteOff>, ApiError> {
        require(self.0, Capabilities::BAD_DEBT_RECOVERY)?;
        let mut call = Call::at(Method::Get, WRITE_OFFS_LIST.route());
        if let Some(c) = customer {
            call.query.push(("customerId".into(), c.to_string()));
        }
        if include_recovered {
            call.query.push(("includeRecovered".into(), "true".into()));
        }
        let reply = self.0.send(&WRITE_OFFS_LIST, call, None).await?;
        crate::resources::sales_tax::decode_list(&reply.body, "writeOffs", "write-offs")
    }
}
