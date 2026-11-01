//! Customer refunds (P22, engine U-139; contract per the P22 plan §2.5,
//! verified at the D8 pin bump): money paid back to a client from a
//! payment credit, a credit memo or a held deposit, by check (queued to
//! print by default, Q-P22d) or another method.
//!
//! - The desktop always names the source (Q-P22k). The engine checks the
//!   amount against what is left (409 `CREDIT_NOT_AVAILABLE` /
//!   `DEPOSIT_NOT_AVAILABLE`); nothing here compares money.
//! - `POST /customer-refunds` stays in P21's `idempotency-receive-payment-v1`
//!   set (append-only); P21's [`crate::CustomerPayments::refund`] keeps
//!   working for the inline dialog until D5 retires it.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::credit_memos::{CreditKind, SalesDocStatus};
use crate::resources::customer_payments::{
    PAYMENT_MEMO_MAX, PAYMENT_REF_MAX, cents_wire, de_cents, decode_key, error_code, ids_debug,
    require, too_long,
};
use crate::resources::operations::{CUSTOMER_REFUND_CREATE, CUSTOMER_REFUND_GET};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::session::Session;
use crate::wire::Capabilities;

/// The engine's refusal of a refund (P22 plan §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefundRefusal {
    /// Less credit left than asked (409 `CREDIT_NOT_AVAILABLE`).
    CreditNotAvailable,
    /// Less deposit left than asked (409 `DEPOSIT_NOT_AVAILABLE`).
    DepositNotAvailable,
    /// Dated before its source (400 `REFUND_DATE_BEFORE_SOURCE`).
    DateBeforeSource,
    /// Not a bank-type account of the company (400 `REFUND_ACCOUNT_INVALID`,
    /// Q-P22l).
    AccountInvalid,
    /// The check number is taken (409 `CHECK_NUMBER_IN_USE`).
    CheckNumberInUse,
    /// The source belongs to another client.
    SourceNotForCustomer,
    /// The source payment bounced (409 `PAYMENT_BOUNCED`).
    Bounced,
    /// Someone changed it since it was read.
    Stale,
}

const REFUND_REFUSALS: &[(&str, RefundRefusal)] = &[
    ("CREDIT_NOT_AVAILABLE", RefundRefusal::CreditNotAvailable),
    ("DEPOSIT_NOT_AVAILABLE", RefundRefusal::DepositNotAvailable),
    ("REFUND_DATE_BEFORE_SOURCE", RefundRefusal::DateBeforeSource),
    ("REFUND_ACCOUNT_INVALID", RefundRefusal::AccountInvalid),
    ("CHECK_NUMBER_IN_USE", RefundRefusal::CheckNumberInUse),
    (
        "CREDIT_NOT_FOR_CUSTOMER",
        RefundRefusal::SourceNotForCustomer,
    ),
    ("PAYMENT_BOUNCED", RefundRefusal::Bounced),
    (crate::resources::ap_doc::STALE, RefundRefusal::Stale),
];

/// Every engine code [`refund_refusal`] knows.
#[must_use]
pub fn refund_refusal_codes() -> Vec<&'static str> {
    REFUND_REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn refund_refusal(e: &ApiError) -> Option<RefundRefusal> {
    if let ApiError::Conflict {
        reason: ConflictReason::Stale,
        ..
    } = e
    {
        return Some(RefundRefusal::Stale);
    }
    let code = error_code(e)?;
    REFUND_REFUSALS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

/// How a refund is paid (Q-88: check, cash, card, bank transfer, other;
/// the engine's `PAYMENT_METHODS` on the wire).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundMethod {
    /// A check (printed later or typed number).
    Check,
    /// Cash.
    Cash,
    /// Back to the client's card.
    CreditCard,
    /// Bank transfer.
    Ach,
    /// Anything else.
    #[serde(other)]
    Other,
}

/// What a refund pays back. All three go out as `sourcePaymentId`; the
/// engine reads the kind from the transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RefundSource {
    /// A payment's unapplied credit.
    Payment(TransactionId),
    /// A credit memo's remaining credit (`refund-credit-memo-v1`).
    CreditMemo(TransactionId),
    /// A held customer deposit.
    Deposit(TransactionId),
}

impl RefundSource {
    /// The source transaction.
    #[must_use]
    pub const fn id(self) -> TransactionId {
        match self {
            RefundSource::Payment(t) | RefundSource::CreditMemo(t) | RefundSource::Deposit(t) => t,
        }
    }
}

/// How a refund check is numbered.
#[derive(Clone, PartialEq, Eq, Default)]
pub enum RefundCheck {
    /// Not a check.
    #[default]
    NotACheck,
    /// Queued to print (`printLater`, the default for checks, Q-P22d).
    PrintLater,
    /// A hand-written check with this number.
    Number(String),
}

impl fmt::Debug for RefundCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RefundCheck::NotACheck => "NotACheck",
            RefundCheck::PrintLater => "PrintLater",
            RefundCheck::Number(_) => "Number(..)",
        })
    }
}

/// `POST /customer-refunds` with a source (P22).
#[derive(Clone, PartialEq, Eq)]
pub struct NewRefund {
    /// The client.
    pub customer_id: ContactId,
    /// The date.
    pub date: CalendarDate,
    /// The bank it is paid from (bank-type only, Q-P22l).
    pub from_account: AccountId,
    /// How much: the source's remaining string unless the owner typed one.
    pub amount: Money,
    /// What it refunds.
    pub source: RefundSource,
    /// How it is paid.
    pub method: RefundMethod,
    /// Check numbering (checks only).
    pub check: RefundCheck,
    /// A reference for other methods.
    pub ref_no: Option<String>,
    /// Memo.
    pub memo: Option<String>,
}
ids_debug!(
    NewRefund,
    "NewRefund",
    customer_id,
    date,
    from_account,
    source,
    method,
    check
);

impl NewRefund {
    /// The request body.
    ///
    /// # Errors
    /// `Local(Unsupported)` for a credit memo source without
    /// `refund-credit-memo-v1` or a check option without
    /// `refund-checks-v1`; `Local(InvalidValue)` for an amount not above
    /// zero or not whole cents, a check option on another method, a blank
    /// check number or text over its limit.
    pub fn body(&self, caps: &Capabilities) -> Result<serde_json::Value, ApiError> {
        let unsupported = (matches!(self.source, RefundSource::CreditMemo(_))
            && !caps.has(Capabilities::REFUND_CREDIT_MEMO))
            || (self.check != RefundCheck::NotACheck && !caps.has(Capabilities::REFUND_CHECKS));
        if unsupported {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let wrong_check = match &self.check {
            RefundCheck::NotACheck => false,
            RefundCheck::PrintLater => self.method != RefundMethod::Check,
            RefundCheck::Number(n) => {
                self.method != RefundMethod::Check
                    || n.trim().is_empty()
                    || too_long(Some(n.trim()), PAYMENT_REF_MAX)
            }
        };
        if !self.amount.is_positive()
            || wrong_check
            || too_long(self.memo.as_deref(), PAYMENT_MEMO_MAX)
            || too_long(self.ref_no.as_deref(), PAYMENT_REF_MAX)
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut body = json!({
            "txnDate": self.date,
            "contactId": self.customer_id,
            "refundFromAccountId": self.from_account,
            "amount": cents_wire(self.amount)?,
            "sourcePaymentId": self.source.id(),
            "paymentMethod": self.method,
        });
        match &self.check {
            RefundCheck::NotACheck => {}
            RefundCheck::PrintLater => body["printLater"] = json!(true),
            RefundCheck::Number(n) => {
                // The engine takes the check number as a JSON integer.
                let number: u64 = n
                    .trim()
                    .parse()
                    .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
                body["printLater"] = json!(false);
                body["checkNumber"] = json!(number);
            }
        }
        if let Some(n) = self.ref_no.as_deref().filter(|n| !n.trim().is_empty()) {
            body["refNo"] = json!(n.trim());
        }
        if let Some(m) = self.memo.as_deref().filter(|m| !m.trim().is_empty()) {
            body["memo"] = json!(m);
        }
        Ok(body)
    }
}

/// Where a saved refund stands in the check print queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefundPrintStatus {
    /// Waiting in the print queue.
    Queue,
    /// Printed.
    Printed,
    /// Not a printed check.
    #[serde(other)]
    None,
}

/// The source of a saved refund.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundSourceRef {
    /// The payment, credit memo or deposit.
    pub id: TransactionId,
    /// Which.
    pub kind: CreditKind,
    /// Its number (owner data).
    #[serde(default)]
    pub number: Option<String>,
}
ids_debug!(RefundSourceRef, "RefundSourceRef", id, kind);

/// A saved refund (`GET /customer-refunds/{id}`, `{refund: …}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefundRecord {
    /// The `customer_refund`.
    pub id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// The client.
    pub customer_id: ContactId,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Method.
    #[serde(default)]
    pub method: Option<RefundMethod>,
    /// Reference (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// The check number once printed or typed (owner data; the engine
    /// writes an integer).
    #[serde(default, deserialize_with = "de_number_text")]
    pub check_number: Option<String>,
    /// The bank it was paid from.
    #[serde(default, rename = "refundFromAccountId")]
    pub from_account: Option<AccountId>,
    /// Print queue state.
    #[serde(default)]
    pub print_status: Option<RefundPrintStatus>,
    /// What it refunded.
    #[serde(default)]
    pub source: Option<RefundSourceRef>,
    /// The version.
    pub updated_at: Timestamp,
    /// Posted or void.
    pub status: SalesDocStatus,
}
ids_debug!(
    RefundRecord,
    "RefundRecord",
    id,
    date,
    customer_id,
    method,
    print_status,
    source,
    status
);

/// A number the engine writes as an integer (or, from older rows, a
/// string), kept as text.
fn de_number_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Option::<serde_json::Value>::deserialize(d)? {
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        Some(serde_json::Value::String(s)) => Some(s),
        _ => None,
    })
}

/// The id of a transaction a create answered.
#[derive(Deserialize)]
struct Created {
    id: TransactionId,
}

/// Customer refund operations of a session (`session.customer_refunds()`).
#[derive(Clone, Copy, Debug)]
pub struct CustomerRefunds<'a>(pub(crate) &'a Session);

impl Session {
    /// Customer refunds of the open company (P22).
    #[must_use]
    pub fn customer_refunds(&self) -> CustomerRefunds<'_> {
        CustomerRefunds(self)
    }
}

impl CustomerRefunds<'_> {
    /// Pays a refund from its source; answers the `customer_refund` id.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`NewRefund::body`]); from the
    /// engine see [`refund_refusal`]; closing; `SavedUnreadable`; any
    /// [`ApiError`].
    pub async fn create(
        &self,
        r: &NewRefund,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<TransactionId, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::PAYMENT_SETTLE)?;
        let body = posting_body(s, r.body(&s.client().capabilities())?, opts)?;
        let mut call = Call::at(Method::Post, CUSTOMER_REFUND_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CUSTOMER_REFUND_CREATE, call, None).await?;
        let created: Created = decode_key(&reply.body, "transaction", "refund")
            .map_err(crate::resources::versioning::saved_unreadable)?;
        Ok(created.id)
    }

    /// One refund with its source.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `refund-credit-memo-v1`; `NotFound`;
    /// `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<RefundRecord, ApiError> {
        require(self.0, Capabilities::REFUND_CREDIT_MEMO)?;
        let call = Call::new(
            Method::Get,
            CUSTOMER_REFUND_GET.route(),
            format!("/api/v1/customer-refunds/{id}"),
        );
        let reply = self.0.send(&CUSTOMER_REFUND_GET, call, None).await?;
        decode_key(&reply.body, "refund", "refund")
    }
}
