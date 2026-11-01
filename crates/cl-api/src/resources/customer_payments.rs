//! Customer payments (P21 Receive Payment, engine U-132 to U-136; contract
//! per the P21 plan §2.2-§2.6, verified at the D9 pin bump).
//!
//! - The engine computes every figure: [`CustomerPayments::preview`]
//!   allocates oldest first (Auto: no `applications`) or fills the rows the
//!   owner touched (Explicit), and returns every total, shortfall, credit
//!   and suggestion. Nothing here adds, subtracts or compares money.
//! - A save sends exactly what the owner saw: per-invoice amounts copied
//!   from the preview, each with the invoice's [`BalanceToken`]
//!   (`balanceDueExact`) echoed as `expectedBalanceDue` (409
//!   `INVOICE_BALANCE_CHANGED` when it moved, Q-P21s).
//! - Money goes out as 2-place strings (whole cents, Q-P21g/Q-P21t) and
//!   comes back as strings: a JSON number where money belongs is `Decode`.
//! - `Debug` prints ids, dates, counts and codes only (no amounts, names,
//!   memos or check numbers).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{BalanceToken, CalendarDate, Money, Timestamp};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::feed::FeedItemId;
use crate::resources::operations::{
    CUSTOMER_REFUND_CREATE, PAYMENT_APPLY, PAYMENT_BOUNCE, PAYMENT_DEPOSITS_ON_HAND, PAYMENT_GET,
    PAYMENT_OPEN_INVOICES, PAYMENT_PREVIEW, PAYMENT_PUT, PAYMENT_RECEIVE, PAYMENT_UNAPPLY,
    PAYMENT_VOID, PAYMENTS_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::session::{Session, json_body};
use crate::wire::{Capabilities, Page, PageRequest};

/// The most applications one request carries (400 `TOO_MANY_APPLICATIONS`
/// above it, P21 plan §5).
pub const MAX_APPLICATIONS: usize = 1000;
/// Memo, check or reference number limits as the engine stores them.
pub const PAYMENT_MEMO_MAX: usize = 4000;
/// Reference (check) number limit.
pub const PAYMENT_REF_MAX: usize = 50;

// ---- Money on the wire: whole cents --------------------------------------------

/// `m` as the engine takes it: a 2-place string (`4000.00`).
///
/// # Errors
/// `Local(InvalidValue)` for an amount with a fraction of a cent (never
/// sent; Q-P21t).
pub fn cents_wire(m: Money) -> Result<String, ApiError> {
    let w = m.to_wire();
    match w.strip_suffix("00") {
        Some(t) if m.to_ten_thousandths() % 100 == 0 => Ok(t.to_string()),
        _ => Err(ApiError::Local(LocalValidation::InvalidValue)),
    }
}

fn cents_from_str<E: de::Error>(s: &str) -> Result<Money, E> {
    let m = Money::parse_wire(s).map_err(|_| E::custom("money"))?;
    if m.to_ten_thousandths() % 100 != 0 {
        return Err(E::custom("money to the cent"));
    }
    Ok(m)
}

/// A money string (never a JSON number).
fn de_cents<'de, D: Deserializer<'de>>(d: D) -> Result<Money, D::Error> {
    let s = String::deserialize(d)?;
    cents_from_str(&s)
}

/// An optional money string; `null` or absent is `None`.
fn de_cents_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Money>, D::Error> {
    match Option::<String>::deserialize(d)? {
        Some(s) => cents_from_str(&s).map(Some),
        None => Ok(None),
    }
}

/// A money string, `null` or absent read as 0.00 (the fields later engine
/// rows add).
fn de_cents_zero<'de, D: Deserializer<'de>>(d: D) -> Result<Money, D::Error> {
    Ok(de_cents_opt(d)?.unwrap_or(Money::ZERO))
}

/// A stored (4-place) money string, read exactly (legacy invoice balances
/// and totals may carry more than cents; display only).
fn de_stored<'de, D: Deserializer<'de>>(d: D) -> Result<Money, D::Error> {
    let s = String::deserialize(d)?;
    Money::parse_wire(&s).map_err(|_| de::Error::custom("money"))
}

fn ser_cents<S: Serializer>(m: &Money, s: S) -> Result<S::Ok, S::Error> {
    let w = cents_wire(*m).map_err(|_| serde::ser::Error::custom("money to the cent"))?;
    s.serialize_str(&w)
}

#[allow(clippy::ref_option)]
fn ser_cents_opt<S: Serializer>(m: &Option<Money>, s: S) -> Result<S::Ok, S::Error> {
    match m {
        Some(m) => ser_cents(m, s),
        None => s.serialize_none(),
    }
}

fn null_false<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

fn null_vec<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

macro_rules! ids_debug {
    ($t:ty, $name:literal, $($f:ident),*) => {
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct($name)
                    $(.field(stringify!($f), &self.$f))*
                    .finish_non_exhaustive()
            }
        }
    };
}

// ---- Refusals ----------------------------------------------------------------------

/// The engine's refusal of a payment write, for the form's plain-language
/// message (P21 plan §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaymentRefusal {
    /// Someone changed it since it was read (409 `STALE`).
    Stale,
    /// An invoice's balance moved since the preview (409
    /// `INVOICE_BALANCE_CHANGED`): reload the open invoices.
    InvoiceBalanceChanged,
    /// More than an invoice's balance (400 `OVERAPPLIED`).
    Overapplied,
    /// More applied than received (400 `APPLICATIONS_EXCEED_AMOUNT`).
    ApplicationsExceedAmount,
    /// The invoice belongs to another client.
    InvoiceNotForCustomer,
    /// Not an account money can be deposited to.
    DepositAccountInvalid,
    /// The payment is in a deposit (409 `TXN_DEPOSITED`).
    Deposited,
    /// A bounced check (409 `PAYMENT_BOUNCED`): on the payment, a deposit
    /// holding it, or a draw of it.
    Bounced,
    /// Part of the credit was refunded (409 `PAYMENT_REFUNDED`).
    Refunded,
    /// A held deposit was already applied (409 `DEPOSIT_APPLIED`).
    DepositApplied,
    /// Less credit left than asked (409 `CREDIT_NOT_AVAILABLE`).
    CreditNotAvailable,
    /// Less deposit left than asked (409 `DEPOSIT_NOT_AVAILABLE`).
    DepositNotAvailable,
    /// The edit would take back credit another payment uses
    /// (409 `CREDIT_IN_USE`).
    CreditInUse,
    /// An invoice was voided meanwhile (409 `INVOICE_VOIDED`).
    InvoiceVoided,
    /// The credit's applying payment is gone (409 `CREDIT_SOURCE_CHANGED`).
    CreditSourceChanged,
    /// A bounce dated before the payment or its deposit.
    BounceDateInvalid,
    /// A bank fee without an expense account (Preferences).
    FeeAccountRequired,
    /// The fee account can't take a fee.
    FeeAccountInvalid,
    /// A bank fee on a check never deposited needs the bank it came from.
    FeeBankRequired,
    /// A write-off with no account set (Preferences).
    WriteOffAccountRequired,
    /// The write-off account can't take a write-off.
    WriteOffAccountInvalid,
    /// A discount with no account set (Preferences).
    DiscountAccountRequired,
    /// The discount account can't take a discount.
    DiscountAccountInvalid,
    /// A held deposit with invoices or credits applied.
    DepositWithApplications,
    /// The bank-file deposit doesn't match (account or amount).
    FeedItemMismatch,
    /// The bank-file deposit was matched or dismissed meanwhile.
    FeedItemNotPending,
    /// Over 1,000 invoices in one payment.
    TooManyApplications,
    /// Nothing received: a payment of 0 that only uses credits
    /// (400 `AMOUNT_REQUIRED`); credits alone go through Apply.
    AmountRequired,
    /// The credit or deposit belongs to another client
    /// (400 `CREDIT_NOT_FOR_CUSTOMER`).
    CreditNotForCustomer,
    /// A payment of 0.00 (only written off) can't be bounced or edited
    /// (400 `NO_MONEY_RECEIVED`).
    NoMoneyReceived,
    /// The row is a held deposit's draw: void the draw instead
    /// (400 `APPLIED_FROM_DEPOSIT`).
    AppliedFromDeposit,
}

const REFUSAL_CODES: &[(&str, PaymentRefusal)] = &[
    (crate::resources::ap_doc::STALE, PaymentRefusal::Stale),
    (
        "INVOICE_BALANCE_CHANGED",
        PaymentRefusal::InvoiceBalanceChanged,
    ),
    ("OVERAPPLIED", PaymentRefusal::Overapplied),
    (
        "APPLICATIONS_EXCEED_AMOUNT",
        PaymentRefusal::ApplicationsExceedAmount,
    ),
    (
        "INVOICE_NOT_FOR_CUSTOMER",
        PaymentRefusal::InvoiceNotForCustomer,
    ),
    (
        "DEPOSIT_ACCOUNT_INVALID",
        PaymentRefusal::DepositAccountInvalid,
    ),
    ("TXN_DEPOSITED", PaymentRefusal::Deposited),
    ("PAYMENT_BOUNCED", PaymentRefusal::Bounced),
    ("PAYMENT_REFUNDED", PaymentRefusal::Refunded),
    ("DEPOSIT_APPLIED", PaymentRefusal::DepositApplied),
    ("CREDIT_NOT_AVAILABLE", PaymentRefusal::CreditNotAvailable),
    ("DEPOSIT_NOT_AVAILABLE", PaymentRefusal::DepositNotAvailable),
    ("CREDIT_IN_USE", PaymentRefusal::CreditInUse),
    ("INVOICE_VOIDED", PaymentRefusal::InvoiceVoided),
    ("CREDIT_SOURCE_CHANGED", PaymentRefusal::CreditSourceChanged),
    ("BOUNCE_DATE_INVALID", PaymentRefusal::BounceDateInvalid),
    ("FEE_ACCOUNT_REQUIRED", PaymentRefusal::FeeAccountRequired),
    ("FEE_ACCOUNT_INVALID", PaymentRefusal::FeeAccountInvalid),
    ("FEE_BANK_REQUIRED", PaymentRefusal::FeeBankRequired),
    (
        "WRITE_OFF_ACCOUNT_REQUIRED",
        PaymentRefusal::WriteOffAccountRequired,
    ),
    (
        "WRITE_OFF_ACCOUNT_INVALID",
        PaymentRefusal::WriteOffAccountInvalid,
    ),
    (
        "DISCOUNT_ACCOUNT_REQUIRED",
        PaymentRefusal::DiscountAccountRequired,
    ),
    (
        "DISCOUNT_ACCOUNT_INVALID",
        PaymentRefusal::DiscountAccountInvalid,
    ),
    (
        "DEPOSIT_WITH_APPLICATIONS",
        PaymentRefusal::DepositWithApplications,
    ),
    ("FEED_ITEM_MISMATCH", PaymentRefusal::FeedItemMismatch),
    ("FEED_ITEM_NOT_PENDING", PaymentRefusal::FeedItemNotPending),
    ("TOO_MANY_APPLICATIONS", PaymentRefusal::TooManyApplications),
    ("AMOUNT_REQUIRED", PaymentRefusal::AmountRequired),
    (
        "CREDIT_NOT_FOR_CUSTOMER",
        PaymentRefusal::CreditNotForCustomer,
    ),
    ("NO_MONEY_RECEIVED", PaymentRefusal::NoMoneyReceived),
    ("APPLIED_FROM_DEPOSIT", PaymentRefusal::AppliedFromDeposit),
];

/// Every engine code [`payment_refusal`] knows (tests and the demo).
#[must_use]
pub fn payment_refusal_codes() -> Vec<&'static str> {
    REFUSAL_CODES.iter().map(|(c, _)| *c).collect()
}

fn error_code(e: &ApiError) -> Option<&str> {
    match e {
        ApiError::Conflict { code: Some(c), .. }
        | ApiError::NotFound { code: Some(c) }
        | ApiError::Forbidden { code: Some(c) }
        | ApiError::Server { code: Some(c), .. } => Some(c.as_str()),
        ApiError::Validation(v) => v.code().map(crate::errors::ErrorCode::as_str),
        _ => None,
    }
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// network, plain validation).
#[must_use]
pub fn payment_refusal(e: &ApiError) -> Option<PaymentRefusal> {
    if let ApiError::Conflict {
        reason: ConflictReason::Stale,
        ..
    } = e
    {
        return Some(PaymentRefusal::Stale);
    }
    let code = error_code(e)?;
    REFUSAL_CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

// ---- Reads ------------------------------------------------------------------------

/// One open invoice of a client (`GET /payments/open-invoices/:customerId`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenInvoiceRow {
    /// The invoice.
    #[serde(alias = "invoiceId")]
    pub id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: String,
    /// Its date.
    pub txn_date: CalendarDate,
    /// When it is due.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Its total (stored).
    #[serde(deserialize_with = "de_stored")]
    pub total: Money,
    /// What is still owed (stored).
    #[serde(deserialize_with = "de_stored")]
    pub balance_due: Money,
    /// The stored balance to echo back (E1; absent on older engines).
    #[serde(default)]
    pub balance_due_exact: Option<BalanceToken>,
}
ids_debug!(OpenInvoiceRow, "OpenInvoiceRow", id, txn_date, due_date);

/// Where an amount came from in a preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmountSource {
    /// The owner typed it.
    Typed,
    /// No amount was typed: the sum of the filled rows (each its full
    /// balance).
    FromApplications,
}

/// One open invoice in a preview, with what this payment does to it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInvoice {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: String,
    /// Its date.
    pub txn_date: CalendarDate,
    /// When it is due.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Its total.
    #[serde(deserialize_with = "de_stored")]
    pub total: Money,
    /// Its cent balance before this payment.
    #[serde(deserialize_with = "de_cents")]
    pub balance_due: Money,
    /// Its stored balance, echoed back as `expectedBalanceDue`.
    pub balance_due_exact: BalanceToken,
    /// The cash this payment puts on it.
    #[serde(deserialize_with = "de_cents")]
    pub payment: Money,
    /// What is left after this payment.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
    /// The discount taken on it (U-134).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub discount: Money,
    /// The amount written off on it (U-133).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub write_off: Money,
    /// Credits and deposits applied to it in this payment (U-133, U-135).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub credit: Money,
    /// Its early-payment terms: the percent as the engine wrote it.
    #[serde(default)]
    pub discount_percent: Option<String>,
    /// The last day of the discount window.
    #[serde(default)]
    pub discount_date: Option<CalendarDate>,
    /// The discount the terms allow on the payment date (U-134).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub discount_available: Money,
    /// The engine suggests the discount (Q-P21c): the payment is exactly
    /// the balance less the discount, within the window.
    #[serde(default, deserialize_with = "null_false")]
    pub suggest_discount: bool,
}
ids_debug!(
    PreviewInvoice,
    "PreviewInvoice",
    invoice_id,
    txn_date,
    due_date,
    suggest_discount
);

/// The preview's totals.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentTotals {
    /// Σ open balances.
    #[serde(deserialize_with = "de_cents")]
    pub amount_due: Money,
    /// Cash applied to invoices.
    #[serde(deserialize_with = "de_cents")]
    pub applied: Money,
    /// Discounts, credits and deposits used.
    #[serde(deserialize_with = "de_cents")]
    pub discounts_and_credits: Money,
    /// Cash not applied (a credit for the client).
    #[serde(deserialize_with = "de_cents")]
    pub unapplied: Money,
    /// Amounts written off (U-133).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub write_offs: Money,
}
impl fmt::Debug for PaymentTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PaymentTotals(..)")
    }
}

/// An invoice this payment leaves partly paid.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shortfall {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// What stays open on it.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// The shortfall is within the company's write-off limit (U-133;
    /// false when no limit is set): the form suggests the write-off.
    #[serde(default, deserialize_with = "null_false")]
    pub within_write_off_limit: bool,
}
ids_debug!(Shortfall, "Shortfall", invoice_id, within_write_off_limit);

/// An open credit of the client (an earlier overpayment, U-133).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditSource {
    /// The payment the credit is left on.
    pub source_payment_id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// Its check or reference number (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// What is left of it.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
}
ids_debug!(CreditSource, "CreditSource", source_payment_id, date);

/// A held customer deposit (U-135).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositOnHand {
    /// The deposit (a `customer_payment` held in Customer Deposits).
    pub payment_id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// Its reference (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// What was received.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// What is left to apply.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
}
ids_debug!(DepositOnHand, "DepositOnHand", payment_id, date);

/// A credit or deposit applied in a preview (the engine fills the amount
/// when none was asked).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewCreditUse {
    /// The credit or deposit.
    pub source_payment_id: TransactionId,
    /// The invoice it goes to.
    pub invoice_id: TransactionId,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}
ids_debug!(
    PreviewCreditUse,
    "PreviewCreditUse",
    source_payment_id,
    invoice_id
);

/// Payments waiting in Payments Clearing after this one is saved.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct PendingAfter {
    /// How many.
    pub count: u32,
    /// Their total.
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
}
ids_debug!(PendingAfter, "PendingAfter", count);

/// A problem the preview found (business problems never come as 4xx from
/// the preview).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewProblem {
    /// The engine code (`OVERAPPLIED`, `APPLICATIONS_EXCEED_AMOUNT`, ...).
    pub code: String,
    /// The invoice it is about.
    #[serde(default)]
    pub invoice_id: Option<TransactionId>,
}

/// `POST /payments/preview`: every figure the screen shows.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPreview {
    /// The client's balance before this payment.
    #[serde(deserialize_with = "de_cents")]
    pub contact_balance_before: Money,
    /// The client's balance after it.
    #[serde(deserialize_with = "de_cents")]
    pub contact_balance_after: Money,
    /// The payment amount (typed, or the filled rows' sum).
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Where it came from.
    pub amount_source: AmountSource,
    /// Every open invoice, oldest first.
    pub invoices: Vec<PreviewInvoice>,
    /// The totals.
    pub totals: PaymentTotals,
    /// Partly paid invoices.
    #[serde(default, deserialize_with = "null_vec")]
    pub shortfalls: Vec<Shortfall>,
    /// The overpayment (unapplied cash above zero; U-133).
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub overpayment: Option<Money>,
    /// The client's open credits (U-133).
    #[serde(default, deserialize_with = "null_vec")]
    pub credits: Vec<CreditSource>,
    /// The client's held deposits (U-135).
    #[serde(default, deserialize_with = "null_vec")]
    pub deposits_on_hand: Vec<DepositOnHand>,
    /// Credits and deposits this payment applies, with the engine's
    /// amounts.
    #[serde(default, deserialize_with = "null_vec")]
    pub credit_applications: Vec<PreviewCreditUse>,
    /// Nothing is applied, no credits, an amount above zero: ask "Hold as
    /// a customer deposit?" (U-135, Q-P21e).
    #[serde(default, deserialize_with = "null_false")]
    pub hold_as_deposit_offered: bool,
    /// Payments waiting to be deposited once this one is saved.
    #[serde(default)]
    pub pending_deposits: Option<PendingAfter>,
    /// Days the client usually takes to pay (Q-P21r; `None` below three
    /// paid invoices).
    #[serde(default)]
    pub average_days_to_pay: Option<u32>,
    /// Problems.
    #[serde(default, deserialize_with = "null_vec")]
    pub problems: Vec<PreviewProblem>,
}

impl fmt::Debug for PaymentPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentPreview")
            .field("amount_source", &self.amount_source)
            .field("invoices", &self.invoices.len())
            .field("shortfalls", &self.shortfalls.len())
            .field("credits", &self.credits.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl PaymentPreview {
    /// The preview row of `invoice`.
    #[must_use]
    pub fn invoice(&self, invoice: TransactionId) -> Option<&PreviewInvoice> {
        self.invoices.iter().find(|i| i.invoice_id == invoice)
    }
}

/// How a saved payment stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    /// Posted.
    Posted,
    /// Voided.
    Void,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

/// One application of a saved payment.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentApplication {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: String,
    /// Cash applied.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Discount taken.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub discount: Money,
    /// Written off.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub write_off: Money,
    /// When it was applied (a credit applied later has its own date).
    #[serde(default)]
    pub applied_date: Option<CalendarDate>,
    /// Voided (unapply, edit or void).
    #[serde(default, deserialize_with = "null_false")]
    pub voided: bool,
    /// Reversed by a bounce.
    #[serde(default, deserialize_with = "null_false")]
    pub reversed: bool,
    /// The receipt this row was applied inside, for a credit of this
    /// payment used later by another payment. `None` on the payment's own
    /// rows.
    #[serde(default)]
    pub applied_in_txn_id: Option<TransactionId>,
    /// True for a credit row applied after the payment was received (by
    /// another receipt or `POST /payments/:id/apply`), whatever its date:
    /// the engine's flag for "not the payment's own row".
    #[serde(default, deserialize_with = "null_false")]
    pub applied_later: bool,
}
ids_debug!(
    PaymentApplication,
    "PaymentApplication",
    invoice_id,
    applied_date,
    voided,
    reversed,
    applied_in_txn_id,
    applied_later
);

impl PaymentApplication {
    /// True when the row is the payment's own (received with it), not a
    /// credit of it applied later: an edit sends these back as its
    /// applications. `date` is the payment's date.
    #[must_use]
    pub fn is_own(&self, payment: TransactionId, date: CalendarDate) -> bool {
        !self.applied_later
            && self.applied_in_txn_id.is_none_or(|t| t == payment)
            && self.applied_date.is_none_or(|d| d == date)
    }
}

/// A credit or held deposit applied inside a saved payment (D-B1, P21 plan
/// §2.3 `creditApplications`): an edit sends it back unchanged.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordCreditUse {
    /// The credit or deposit used.
    pub source_payment_id: TransactionId,
    /// The invoice it went to.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: String,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// A draw on a held customer deposit (else a client credit).
    #[serde(default, deserialize_with = "null_false")]
    pub from_deposit: bool,
}
ids_debug!(
    RecordCreditUse,
    "RecordCreditUse",
    source_payment_id,
    invoice_id,
    from_deposit
);

/// The deposit a payment went into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepositedIn {
    /// The deposit.
    pub deposit_id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
}

/// The bounce of a payment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BounceRef {
    /// The "Bounced payment" entry.
    pub return_id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
}

/// A saved customer payment (`GET /payments/:id`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentRecord {
    /// The payment.
    pub id: TransactionId,
    /// The client.
    pub customer_id: ContactId,
    /// Its date.
    pub date: CalendarDate,
    /// What was received.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Where it went: `None` for a payment of 0 that only wrote off or
    /// discounted (no asset line; engine should-fix 6).
    #[serde(default)]
    pub deposit_to: Option<AccountId>,
    /// Method (`check`, `cash`, ...).
    #[serde(default)]
    pub payment_method: Option<String>,
    /// Check or reference number (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// Memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
    /// The version to send back as `expectedUpdatedAt`.
    pub updated_at: Timestamp,
    /// Posted or void.
    pub status: PaymentStatus,
    /// The deposit it is in.
    #[serde(default)]
    pub deposited_in: Option<DepositedIn>,
    /// Its bounce.
    #[serde(default)]
    pub bounce: Option<BounceRef>,
    /// Its applications, live and not.
    #[serde(default, deserialize_with = "null_vec")]
    pub applications: Vec<PaymentApplication>,
    /// Credit left on it (0 with a live bounce).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub unapplied_remaining: Money,
    /// It is a held customer deposit (U-135).
    #[serde(default, deserialize_with = "null_false")]
    pub holds_deposit: bool,
    /// What is left of the held deposit.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub deposit_remaining: Money,
    /// Credits and held deposits applied inside this payment (draws
    /// included; D-B1, E-B2).
    #[serde(default, deserialize_with = "null_vec")]
    pub credit_applications: Vec<RecordCreditUse>,
    /// The account its write-offs went to.
    #[serde(default)]
    pub write_off_account_id: Option<AccountId>,
    /// The account its discounts went to.
    #[serde(default)]
    pub discount_account_id: Option<AccountId>,
}

impl fmt::Debug for PaymentRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaymentRecord")
            .field("id", &self.id)
            .field("customer_id", &self.customer_id)
            .field("date", &self.date)
            .field("status", &self.status)
            .field("deposited_in", &self.deposited_in)
            .field("bounce", &self.bounce)
            .field("applications", &self.applications.len())
            .field("credit_applications", &self.credit_applications.len())
            .finish_non_exhaustive()
    }
}

impl PaymentRecord {
    /// The live applications (not voided, not reversed).
    pub fn live_applications(&self) -> impl Iterator<Item = &PaymentApplication> {
        self.applications
            .iter()
            .filter(|a| !a.voided && !a.reversed)
    }

    /// True once a live bounce points to it (read-only, Q-P21o).
    #[must_use]
    pub fn is_bounced(&self) -> bool {
        self.bounce.is_some()
    }

    /// True when it is void.
    #[must_use]
    pub fn is_void(&self) -> bool {
        self.status == PaymentStatus::Void
    }
}

/// One row of `GET /payments`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentSummary {
    /// The payment.
    pub id: TransactionId,
    /// The client.
    pub customer_id: ContactId,
    /// Its date.
    pub date: CalendarDate,
    /// What was received.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Check or reference number (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// Posted or void.
    pub status: PaymentStatus,
    /// A held customer deposit.
    #[serde(default, deserialize_with = "null_false")]
    pub holds_deposit: bool,
    /// What is left of a held deposit.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub deposit_remaining: Money,
    /// Credit left on it (0.00 for a held deposit or a bounced check).
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub unapplied_remaining: Money,
}
ids_debug!(
    PaymentSummary,
    "PaymentSummary",
    id,
    customer_id,
    date,
    status
);

/// `GET /payments` filters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaymentListQuery {
    /// One client.
    pub contact: Option<ContactId>,
    /// From this date.
    pub from: Option<CalendarDate>,
    /// To this date.
    pub to: Option<CalendarDate>,
}

// ---- Requests ----------------------------------------------------------------------

/// One row the owner touched in Explicit mode: `amount: None` is "fill"
/// (the engine gives it what is left, up to its balance, oldest first).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationRequest {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// The owner's amount, or `None` to fill.
    #[serde(serialize_with = "ser_cents_opt")]
    pub amount: Option<Money>,
    /// A discount on it (the engine's `discountAvailable` copied, or typed).
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub discount_amount: Option<Money>,
    /// An amount written off on it (copied from the preview's shortfall).
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_off_amount: Option<Money>,
}
ids_debug!(ApplicationRequest, "ApplicationRequest", invoice_id);

/// A credit or held deposit to use (`amount: None`: the engine fills it).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditRequest {
    /// The credit or deposit.
    pub source_payment_id: TransactionId,
    /// The invoice it goes to.
    pub invoice_id: TransactionId,
    /// The owner's amount, or `None`.
    #[serde(serialize_with = "ser_cents_opt")]
    pub amount: Option<Money>,
}
ids_debug!(
    CreditRequest,
    "CreditRequest",
    source_payment_id,
    invoice_id
);

/// `POST /payments/preview`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentPreviewRequest {
    /// The client.
    pub customer_id: ContactId,
    /// The payment date.
    pub date: CalendarDate,
    /// The typed amount (`None`: the filled rows' sum).
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub amount: Option<Money>,
    /// Where it goes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit_to: Option<AccountId>,
    /// `None` in Auto mode (oldest first); the touched rows in Explicit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applications: Option<Vec<ApplicationRequest>>,
    /// Credits and deposits to use.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credit_applications: Vec<CreditRequest>,
    /// Hold it as a customer deposit.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hold_as_deposit: bool,
    /// The saved payment being edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editing_payment_id: Option<TransactionId>,
}
ids_debug!(
    PaymentPreviewRequest,
    "PaymentPreviewRequest",
    customer_id,
    date,
    editing_payment_id
);

/// One application of a save, copied from the preview.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveApplication {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Cash on it.
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// The invoice's stored balance as the preview read it.
    pub expected_balance_due: BalanceToken,
    /// Discount (U-134).
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub discount_amount: Option<Money>,
    /// Write-off (U-133).
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_off_amount: Option<Money>,
}
ids_debug!(SaveApplication, "SaveApplication", invoice_id);

/// A credit or deposit of a save, copied from the preview.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveCredit {
    /// The credit or deposit.
    pub source_payment_id: TransactionId,
    /// The invoice it goes to.
    pub invoice_id: TransactionId,
    /// How much.
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// The invoice's stored balance as the preview read it.
    pub expected_balance_due: BalanceToken,
}
ids_debug!(SaveCredit, "SaveCredit", source_payment_id, invoice_id);

/// `POST /payments/receive` (and the body of `PUT /payments/:id`).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCustomerPayment {
    /// The client.
    pub customer_id: ContactId,
    /// The payment date.
    pub date: CalendarDate,
    /// What was received.
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// Where it goes (Undeposited Funds by default, Q-202).
    pub deposit_to: AccountId,
    /// Method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<String>,
    /// Check or reference number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_no: Option<String>,
    /// Memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// The applications, copied from the preview.
    pub applications: Vec<SaveApplication>,
    /// Credits and deposits used, copied from the preview.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub credit_applications: Vec<SaveCredit>,
    /// Write-offs post here (else the company default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_off_account_id: Option<AccountId>,
    /// Discounts post here (else the company default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount_account_id: Option<AccountId>,
    /// Hold it as a customer deposit (U-135).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hold_as_deposit: bool,
    /// The bank-file deposit it matches (U-136).
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "ser_feed")]
    pub bank_feed_item_id: Option<FeedItemId>,
}

#[allow(clippy::ref_option)]
fn ser_feed<S: Serializer>(f: &Option<FeedItemId>, s: S) -> Result<S::Ok, S::Error> {
    match f {
        Some(f) => s.serialize_str(&f.to_string()),
        None => s.serialize_none(),
    }
}

impl fmt::Debug for NewCustomerPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewCustomerPayment")
            .field("customer_id", &self.customer_id)
            .field("date", &self.date)
            .field("deposit_to", &self.deposit_to)
            .field("applications", &self.applications.len())
            .field("credit_applications", &self.credit_applications.len())
            .field("hold_as_deposit", &self.hold_as_deposit)
            .finish_non_exhaustive()
    }
}

/// One invoice of `POST /payments/:id/apply` (`amount: None`: the engine
/// fills min(remaining, balance)).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyInvoice {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// The owner's amount, or `None`.
    #[serde(
        serialize_with = "ser_cents_opt",
        skip_serializing_if = "Option::is_none"
    )]
    pub amount: Option<Money>,
    /// The invoice's stored balance as last read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_balance_due: Option<BalanceToken>,
}
ids_debug!(ApplyInvoice, "ApplyInvoice", invoice_id);

/// `POST /payments/:id/apply` (a credit or a held deposit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyRequest {
    /// The date it is applied.
    pub date: CalendarDate,
    /// The invoices.
    pub applications: Vec<ApplyInvoice>,
    /// The credit's or deposit's `updatedAt` as read.
    pub expected_updated_at: Option<Timestamp>,
}

/// What `/apply` did.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    /// Each invoice and what went on it.
    pub applied: Vec<AppliedAmount>,
    /// What is left of the credit or deposit.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
}
impl fmt::Debug for ApplyResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyResult")
            .field("applied", &self.applied.len())
            .finish_non_exhaustive()
    }
}

/// One invoice of an [`ApplyResult`].
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedAmount {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}
ids_debug!(AppliedAmount, "AppliedAmount", invoice_id);

/// `POST /payments/:id/bounce`.
#[derive(Clone, PartialEq, Eq)]
pub struct BounceRequest {
    /// The date the bank returned it (the only date checked against the
    /// closing date).
    pub date: CalendarDate,
    /// The bank's fee.
    pub bank_fee_amount: Option<Money>,
    /// The expense account for the fee (else the company default).
    pub bank_fee_account_id: Option<AccountId>,
    /// The bank the fee comes out of, when the check never reached a bank.
    pub bank_fee_bank_account_id: Option<AccountId>,
    /// Memo.
    pub memo: Option<String>,
    /// The payment's `updatedAt` as read.
    pub expected_updated_at: Timestamp,
}
ids_debug!(
    BounceRequest,
    "BounceRequest",
    date,
    bank_fee_account_id,
    bank_fee_bank_account_id
);

/// Which kind of account held the money a bounce takes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeldKind {
    /// A bank account (deposited, or deposited straight to it).
    Bank,
    /// Undeposited Funds (never deposited).
    PaymentsClearing,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

/// The account a bounce takes the money back out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldIn {
    /// The account.
    pub account_id: AccountId,
    /// Bank or Undeposited Funds.
    pub kind: HeldKind,
}

/// An invoice a bounce reopened, with the engine's balance.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReopenedInvoice {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its balance now.
    #[serde(deserialize_with = "de_cents")]
    pub balance_due: Money,
}
ids_debug!(ReopenedInvoice, "ReopenedInvoice", invoice_id);

/// What a bounce posted.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BounceResult {
    /// The "Bounced payment" entry.
    pub return_id: TransactionId,
    /// The bank fee expense.
    #[serde(default)]
    pub fee_id: Option<TransactionId>,
    /// Where the money came back out of.
    pub held_in: HeldIn,
    /// The invoices reopened.
    #[serde(default, deserialize_with = "null_vec")]
    pub reopened: Vec<ReopenedInvoice>,
}
impl fmt::Debug for BounceResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BounceResult")
            .field("return_id", &self.return_id)
            .field("fee_id", &self.fee_id)
            .field("held_in", &self.held_in)
            .field("reopened", &self.reopened.len())
            .finish()
    }
}

/// A refund of a credit or a held deposit (Q-P21m; the small dialog after
/// an overpayment). Posted through `POST /customer-refunds` with
/// `sourcePaymentId` (P21 plan §2.3).
#[derive(Clone, PartialEq, Eq)]
pub struct NewCustomerRefund {
    /// The client.
    pub customer_id: ContactId,
    /// The date.
    pub date: CalendarDate,
    /// The bank it is paid from.
    pub from_account: AccountId,
    /// How much (the owner's, prefilled with the preview's overpayment).
    pub amount: Money,
    /// The credit or deposit it refunds.
    pub source_payment: TransactionId,
    /// Method (`check`, ...).
    pub method: Option<String>,
    /// Check number.
    pub ref_no: Option<String>,
    /// Memo.
    pub memo: Option<String>,
}
ids_debug!(
    NewCustomerRefund,
    "NewCustomerRefund",
    customer_id,
    date,
    from_account,
    source_payment
);

/// Options of an edit, void, unapply or bounce.
#[derive(Clone, Debug, Default)]
pub struct PaymentWriteOptions {
    /// The closing-date password.
    pub posting: PostingOptions,
    /// The payment's `updatedAt` when it was read.
    pub expected_updated_at: Option<Timestamp>,
}

// ---- Checks ------------------------------------------------------------------------

fn require(s: &Session, cap: &str) -> Result<(), ApiError> {
    if s.client().capabilities().has(cap) {
        Ok(())
    } else {
        Err(ApiError::Local(LocalValidation::Unsupported))
    }
}

fn too_long(s: Option<&str>, max: usize) -> bool {
    s.is_some_and(|t| t.chars().count() > max)
}

/// `Local(Unsupported)` unless the engine advertises `receive-payment-v1`
/// and the capability of each field that is on.
fn caps_for(caps: &Capabilities, fields: &[(bool, &str)]) -> Result<(), ApiError> {
    let missing = !caps.has(Capabilities::RECEIVE_PAYMENT)
        || fields.iter().any(|(on, cap)| *on && !caps.has(cap));
    if missing {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    Ok(())
}

/// Checks a preview request before anything is sent.
///
/// # Errors
/// `Local(Unsupported)` for a field the engine does not advertise;
/// `Local(TooManyLines)` over [`MAX_APPLICATIONS`]; `Local(InvalidValue)`
/// for a fraction of a cent or a negative amount.
pub fn check_preview(r: &PaymentPreviewRequest, caps: &Capabilities) -> Result<(), ApiError> {
    let apps = r.applications.as_deref().unwrap_or_default();
    caps_for(
        caps,
        &[
            (
                apps.iter().any(|a| a.write_off_amount.is_some())
                    || !r.credit_applications.is_empty(),
                Capabilities::PAYMENT_SETTLE,
            ),
            (
                apps.iter().any(|a| a.discount_amount.is_some()),
                Capabilities::PAYMENT_DISCOUNTS,
            ),
            (r.hold_as_deposit, Capabilities::CUSTOMER_DEPOSITS),
        ],
    )?;
    if apps.len() + r.credit_applications.len() > MAX_APPLICATIONS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    let amounts = r
        .amount
        .iter()
        .chain(apps.iter().flat_map(|a| {
            a.amount
                .iter()
                .chain(&a.discount_amount)
                .chain(&a.write_off_amount)
        }))
        .chain(r.credit_applications.iter().flat_map(|c| c.amount.iter()));
    for m in amounts {
        if m.is_negative() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        cents_wire(*m)?;
    }
    Ok(())
}

/// Checks a save before anything is sent.
///
/// # Errors
/// As [`check_preview`]; `Local(InvalidValue)` for text over its limit.
pub fn check_payment(p: &NewCustomerPayment, caps: &Capabilities) -> Result<(), ApiError> {
    caps_for(
        caps,
        &[
            (
                p.applications.iter().any(|a| a.write_off_amount.is_some())
                    || !p.credit_applications.is_empty(),
                Capabilities::PAYMENT_SETTLE,
            ),
            (
                p.applications.iter().any(|a| a.discount_amount.is_some()),
                Capabilities::PAYMENT_DISCOUNTS,
            ),
            (p.hold_as_deposit, Capabilities::CUSTOMER_DEPOSITS),
            (
                p.bank_feed_item_id.is_some(),
                Capabilities::FEED_CUSTOMER_PAYMENTS,
            ),
        ],
    )?;
    if p.applications.len() + p.credit_applications.len() > MAX_APPLICATIONS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    if too_long(p.memo.as_deref(), PAYMENT_MEMO_MAX)
        || too_long(p.ref_no.as_deref(), PAYMENT_REF_MAX)
    {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let amounts = std::iter::once(&p.amount)
        .chain(p.applications.iter().flat_map(|a| {
            std::iter::once(&a.amount)
                .chain(&a.discount_amount)
                .chain(&a.write_off_amount)
        }))
        .chain(p.credit_applications.iter().map(|c| &c.amount));
    for m in amounts {
        if m.is_negative() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        cents_wire(*m)?;
    }
    Ok(())
}

fn with_version(mut body: Value, t: Option<Timestamp>) -> Value {
    if let (Some(t), Value::Object(map)) = (t, &mut body) {
        map.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
    }
    body
}

fn decode_key<T: for<'de> Deserialize<'de>>(
    body: &[u8],
    key: &str,
    what: &'static str,
) -> Result<T, ApiError> {
    let mut v: Map<String, Value> =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let inner = v.remove(key).ok_or(ApiError::Decode { what })?;
    serde_json::from_value(inner).map_err(|_| ApiError::Decode { what })
}

/// Reads a payment from a save answer (`{payment: …}`).
///
/// # Errors
/// `SavedUnreadable` when the answer can't be read.
pub fn saved_payment(body: &[u8]) -> Result<PaymentRecord, ApiError> {
    decode_key(body, "payment", "payment").map_err(crate::resources::versioning::saved_unreadable)
}

/// The id of a transaction a create answered.
#[derive(Deserialize)]
struct Created {
    id: TransactionId,
}

/// Customer payment operations of a session (`session.customer_payments()`).
#[derive(Clone, Copy, Debug)]
pub struct CustomerPayments<'a>(pub(crate) &'a Session);

impl Session {
    /// Customer payments of the open company (P21).
    #[must_use]
    pub fn customer_payments(&self) -> CustomerPayments<'_> {
        CustomerPayments(self)
    }
}

impl CustomerPayments<'_> {
    /// A client's open invoices, oldest first.
    ///
    /// # Errors
    /// `NotFound` (another company's client); `Decode`; any [`ApiError`].
    pub async fn open_invoices(
        &self,
        customer: ContactId,
    ) -> Result<Vec<OpenInvoiceRow>, ApiError> {
        let call = Call::new(
            Method::Get,
            PAYMENT_OPEN_INVOICES.route(),
            format!("/api/v1/payments/open-invoices/{customer}"),
        );
        let reply = self.0.send(&PAYMENT_OPEN_INVOICES, call, None).await?;
        decode_key(&reply.body, "invoices", "open invoices")
    }

    /// Every figure of the payment on the form, posting nothing.
    ///
    /// # Errors
    /// The local checks of [`check_preview`]; `Validation`; `Decode`; any
    /// [`ApiError`].
    pub async fn preview(&self, r: &PaymentPreviewRequest) -> Result<PaymentPreview, ApiError> {
        let s = &self.0.pinned();
        check_preview(r, &s.client().capabilities())?;
        let call = Call::at(Method::Post, PAYMENT_PREVIEW.route()).json_body(json_body(r)?);
        let reply = s.send(&PAYMENT_PREVIEW, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "payment preview",
        })
    }

    /// Receives a payment (one transaction; keyed under
    /// `idempotency-receive-payment-v1`; a replay returns the same record).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`check_payment`]). From the engine:
    /// see [`payment_refusal`]; `ClosedPeriod`, `ClosingPasswordRejected`;
    /// `SavedUnreadable`; any [`ApiError`].
    pub async fn receive(
        &self,
        p: &NewCustomerPayment,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<PaymentRecord, ApiError> {
        let s = &self.0.pinned();
        check_payment(p, &s.client().capabilities())?;
        let body =
            serde_json::to_value(p).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, PAYMENT_RECEIVE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&PAYMENT_RECEIVE, call, None).await?;
        saved_payment(&reply.body)
    }

    /// Payments, newest first, filtered.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `receive-payment-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn list(
        &self,
        q: &PaymentListQuery,
        page: PageRequest,
    ) -> Result<Page<PaymentSummary>, ApiError> {
        require(self.0, Capabilities::RECEIVE_PAYMENT)?;
        let mut call = Call::at(Method::Get, PAYMENTS_LIST.route());
        if let Some(c) = q.contact {
            call.query.push(("contactId".into(), c.to_string()));
        }
        if let Some(d) = q.from {
            call.query.push(("from".into(), d.to_wire()));
        }
        if let Some(d) = q.to {
            call.query.push(("to".into(), d.to_wire()));
        }
        call.query.extend(page.query());
        let reply = self.0.send(&PAYMENTS_LIST, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// One payment with its applications.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<PaymentRecord, ApiError> {
        require(self.0, Capabilities::RECEIVE_PAYMENT)?;
        let call = Call::new(
            Method::Get,
            PAYMENT_GET.route(),
            format!("/api/v1/payments/{id}"),
        );
        let reply = self.0.send(&PAYMENT_GET, call, None).await?;
        decode_key(&reply.body, "payment", "payment")
    }

    /// Edits a payment if it is still at `expected_updated_at` (Q-P21o).
    ///
    /// # Errors
    /// As [`CustomerPayments::receive`], plus `Conflict { Stale }`,
    /// `TXN_DEPOSITED`, `PAYMENT_BOUNCED`, `CREDIT_IN_USE`.
    pub async fn update(
        &self,
        id: TransactionId,
        p: &NewCustomerPayment,
        opts: &PaymentWriteOptions,
        key: IdempotencyKey,
    ) -> Result<PaymentRecord, ApiError> {
        let s = &self.0.pinned();
        check_payment(p, &s.client().capabilities())?;
        let body =
            serde_json::to_value(p).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(
            s,
            with_version(body, opts.expected_updated_at),
            &opts.posting,
        )?;
        let mut call = Call::new(
            Method::Put,
            PAYMENT_PUT.route(),
            format!("/api/v1/payments/{id}"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&PAYMENT_PUT, call, None).await?;
        saved_payment(&reply.body)
    }

    /// Voids a payment; its invoices reopen.
    ///
    /// # Errors
    /// `Local(Unsupported)`; from the engine: `STALE`, `TXN_DEPOSITED`,
    /// `PAYMENT_BOUNCED`, `PAYMENT_REFUNDED`, `DEPOSIT_APPLIED`, closing.
    pub async fn void(
        &self,
        id: TransactionId,
        reason: Option<&str>,
        opts: &PaymentWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::RECEIVE_PAYMENT)?;
        let mut body = json!({});
        if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
            body["reason"] = json!(r);
        }
        let body = posting_body(
            s,
            with_version(body, opts.expected_updated_at),
            &opts.posting,
        )?;
        let mut call = Call::new(
            Method::Post,
            PAYMENT_VOID.route(),
            format!("/api/v1/payments/{id}/void"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        s.send(&PAYMENT_VOID, call, None).await?;
        Ok(())
    }

    /// Takes one invoice off a payment.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `STALE`, `PAYMENT_BOUNCED`, closing; any
    /// [`ApiError`].
    pub async fn unapply(
        &self,
        payment: TransactionId,
        invoice: TransactionId,
        opts: &PaymentWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::RECEIVE_PAYMENT)?;
        let body = with_version(
            json!({"paymentId": payment, "invoiceId": invoice}),
            opts.expected_updated_at,
        );
        let body = posting_body(s, body, &opts.posting)?;
        let mut call = Call::at(Method::Post, PAYMENT_UNAPPLY.route()).json_body(body);
        call.idempotency_key = Some(key);
        s.send(&PAYMENT_UNAPPLY, call, None).await?;
        Ok(())
    }

    /// Applies a credit (U-133) or a held deposit (U-135) to invoices.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `payment-settle-v1` or
    /// `customer-deposits-v1`; from the engine: `CREDIT_NOT_AVAILABLE`,
    /// `DEPOSIT_NOT_AVAILABLE`, `PAYMENT_BOUNCED`, `INVOICE_BALANCE_CHANGED`,
    /// closing; any [`ApiError`].
    pub async fn apply(
        &self,
        source: TransactionId,
        r: &ApplyRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<ApplyResult, ApiError> {
        let s = &self.0.pinned();
        let caps = s.client().capabilities();
        if !caps.has(Capabilities::PAYMENT_SETTLE) && !caps.has(Capabilities::CUSTOMER_DEPOSITS) {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        if r.applications.is_empty() || r.applications.len() > MAX_APPLICATIONS {
            return Err(ApiError::Local(LocalValidation::TooFewLines));
        }
        for m in r.applications.iter().filter_map(|a| a.amount) {
            cents_wire(m)?;
        }
        let body = with_version(
            json!({"date": r.date, "applications": r.applications}),
            r.expected_updated_at,
        );
        let body = posting_body(s, body, opts)?;
        let mut call = Call::new(
            Method::Post,
            PAYMENT_APPLY.route(),
            format!("/api/v1/payments/{source}/apply"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&PAYMENT_APPLY, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode { what: "apply" })
    }

    /// Records a bounced check: one dated "Bounced payment" entry, invoices
    /// reopened, the bank fee in the same step (Q-P21i/k/u).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `payment-bounce-v1`; from the engine:
    /// `PAYMENT_BOUNCED`, `PAYMENT_REFUNDED`, `STALE`,
    /// `BOUNCE_DATE_INVALID`, `FEE_ACCOUNT_*`, `FEE_BANK_REQUIRED`, closing
    /// on the bounce date; any [`ApiError`].
    pub async fn bounce(
        &self,
        id: TransactionId,
        r: &BounceRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<BounceResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::PAYMENT_BOUNCE)?;
        let mut m = Map::new();
        m.insert("date".into(), json!(r.date));
        if let Some(f) = r.bank_fee_amount {
            if !f.is_positive() {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            m.insert("bankFeeAmount".into(), json!(cents_wire(f)?));
        }
        if let Some(a) = r.bank_fee_account_id {
            m.insert("bankFeeAccountId".into(), json!(a));
        }
        if let Some(a) = r.bank_fee_bank_account_id {
            m.insert("bankFeeBankAccountId".into(), json!(a));
        }
        if let Some(memo) = r.memo.as_deref().filter(|t| !t.trim().is_empty()) {
            m.insert("memo".into(), json!(memo));
        }
        m.insert(
            "expectedUpdatedAt".into(),
            Value::String(r.expected_updated_at.to_wire()),
        );
        let body = posting_body(s, Value::Object(m), opts)?;
        let mut call = Call::new(
            Method::Post,
            PAYMENT_BOUNCE.route(),
            format!("/api/v1/payments/{id}/bounce"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&PAYMENT_BOUNCE, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode { what: "bounce" })
    }

    /// A client's held deposits (U-135).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `customer-deposits-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn deposits_on_hand(
        &self,
        customer: ContactId,
    ) -> Result<Vec<DepositOnHand>, ApiError> {
        require(self.0, Capabilities::CUSTOMER_DEPOSITS)?;
        let mut call = Call::at(Method::Get, PAYMENT_DEPOSITS_ON_HAND.route());
        call.query = vec![("customerId".into(), customer.to_string())];
        let reply = self.0.send(&PAYMENT_DEPOSITS_ON_HAND, call, None).await?;
        decode_key(&reply.body, "data", "deposits on hand")
    }

    /// Refunds a credit or a held deposit: `POST /customer-refunds` with
    /// `sourcePaymentId` (the engine's canonical route: the Receive Payment
    /// permission, keyed under `idempotency-receive-payment-v1`; P21 plan
    /// §2.3). Answers the `customer_refund` transaction's id.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `payment-settle-v1`;
    /// `Local(InvalidValue)` for an amount not above zero or not whole
    /// cents; from the engine: `CREDIT_NOT_AVAILABLE`, closing; any
    /// [`ApiError`].
    pub async fn refund(
        &self,
        r: &NewCustomerRefund,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<TransactionId, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::PAYMENT_SETTLE)?;
        if !r.amount.is_positive() || too_long(r.memo.as_deref(), PAYMENT_MEMO_MAX) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut body = json!({
            "txnDate": r.date,
            "contactId": r.customer_id,
            "refundFromAccountId": r.from_account,
            "amount": cents_wire(r.amount)?,
            "sourcePaymentId": r.source_payment,
        });
        if let Some(m) = &r.method {
            body["paymentMethod"] = json!(m);
        }
        if let Some(n) = r.ref_no.as_deref().filter(|n| !n.trim().is_empty()) {
            body["refNo"] = json!(n);
        }
        if let Some(m) = r.memo.as_deref().filter(|m| !m.trim().is_empty()) {
            body["memo"] = json!(m);
        }
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, CUSTOMER_REFUND_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CUSTOMER_REFUND_CREATE, call, None).await?;
        let created: Created = decode_key(&reply.body, "transaction", "refund")
            .map_err(crate::resources::versioning::saved_unreadable)?;
        Ok(created.id)
    }
}

#[cfg(test)]
#[path = "customer_payments_tests.rs"]
mod tests;
