//! Pay Sales Tax and linked outside payments (P23, engine U-150; contract
//! per the P23 plan §2.6, verified at the WP-4 pin bump).
//!
//! - The engine computes everything: what each item owes through a date
//!   (rows counted on the company basis up to `throughDate`, less every
//!   live payment, adjustment and opening line whatever its date; plan
//!   S1), each line's amount and the total. The form shows the strings.
//! - Pay posts a `sales_tax_payment` check (Dr Sales Tax Payable per item,
//!   the adjustment account, Cr bank) through the check queue. Each line
//!   sends back the preview's `expectedOwed`, a byte copy of the preview
//!   string; 409 `SALES_TAX_OWED_CHANGED` when it moved. Keyed under
//!   `idempotency-sales-tax-pay-v1`; closing on the payment date.
//! - An adjustment's amount is signed (negative lowers what is paid, a
//!   discount for filing on time) and needs an account and a reason.
//! - Link marks an existing check or journal entry that debited Sales Tax
//!   Payable as this agency's payment, split over its items.
//! - There is no edit (Q-P23h): void through `/transactions/:id/void`.
//! - Contract per engine PR #219 (U-150): the post carries the preview's
//!   owed total as a top-level `expectedOwed`; `checkNumber` is a JSON
//!   whole number both ways; a check with no number is printed later
//!   (`CHECK_NO_OR_PRINT_LATER` only when both are sent); `GET
//!   /sales-tax/payments/:id` adds `stubLines`; a link is undone with
//!   `POST /sales-tax/payments/:id/unlink`; `POST
//!   /sales-tax/payments/mark-paid-through` records tax paid outside these
//!   books (opening lines, no GL); `POST
//!   /sales-tax/payments/mark-paid-through/:runId/undo` takes a run back
//!   (P23 review fix; provisional route).

use std::fmt;

use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{
    cents_wire, de_cents, de_cents_zero, error_code, null_false, null_vec, require, ser_cents,
    too_long,
};
use crate::resources::operations::{
    SALES_TAX_PAY_CREATE, SALES_TAX_PAY_LINK, SALES_TAX_PAY_MARK_PAID,
    SALES_TAX_PAY_MARK_PAID_UNDO, SALES_TAX_PAY_PREVIEW, SALES_TAX_PAY_UNLINK,
    SALES_TAX_PAYMENT_GET, SALES_TAX_PAYMENTS_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::sales_tax::{SalesTaxAgencyId, SalesTaxItemId, decode_list, decode_one};
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// Most item lines or adjustments in one payment.
pub const PAY_LINES_MAX: usize = 200;
/// Longest adjustment reason.
pub const ADJUSTMENT_REASON_MAX: usize = 200;
/// Longest payment memo.
pub const PAY_MEMO_MAX: usize = 4000;

// ---- Refusals ------------------------------------------------------------------

/// The engine's refusals of Pay Sales Tax and link (plan §2.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaySalesTaxRefusal {
    /// What an item owes moved since the preview (409
    /// `SALES_TAX_OWED_CHANGED`): preview again.
    OwedChanged,
    /// The payment adds up to zero or less (400
    /// `PAYMENT_TOTAL_NOT_POSITIVE`).
    TotalNotPositive,
    /// An item belongs to another agency (400 `TAX_ITEM_NOT_FOR_AGENCY`).
    TaxItemNotForAgency,
    /// The adjustment account can't take it (400
    /// `ADJUSTMENT_ACCOUNT_INVALID`).
    AdjustmentAccountInvalid,
    /// A check needs a number or Print later, not both (400
    /// `CHECK_NO_OR_PRINT_LATER`).
    CheckNoOrPrintLater,
    /// The check number is taken (409 `CHECK_NUMBER_IN_USE`).
    CheckNumberInUse,
    /// The linked lines don't add up to the payment's Sales Tax Payable
    /// debit (400 `LINK_TOTAL_MISMATCH`).
    LinkTotalMismatch,
    /// The transaction didn't debit Sales Tax Payable (400
    /// `NOT_STP_PAYMENT`).
    NotStpPayment,
    /// The transaction is already linked (409 `ALREADY_LINKED`).
    AlreadyLinked,
    /// Unlink on a Pay Sales Tax payment or a transaction with no link
    /// (400 `NOT_LINKED`; void a Pay Sales Tax payment instead).
    NotLinked,
    /// Someone changed it since it was read (409 `STALE`).
    Stale,
}

const REFUSALS: &[(&str, PaySalesTaxRefusal)] = &[
    ("SALES_TAX_OWED_CHANGED", PaySalesTaxRefusal::OwedChanged),
    (
        "PAYMENT_TOTAL_NOT_POSITIVE",
        PaySalesTaxRefusal::TotalNotPositive,
    ),
    (
        "TAX_ITEM_NOT_FOR_AGENCY",
        PaySalesTaxRefusal::TaxItemNotForAgency,
    ),
    (
        "ADJUSTMENT_ACCOUNT_INVALID",
        PaySalesTaxRefusal::AdjustmentAccountInvalid,
    ),
    (
        "CHECK_NO_OR_PRINT_LATER",
        PaySalesTaxRefusal::CheckNoOrPrintLater,
    ),
    ("CHECK_NUMBER_IN_USE", PaySalesTaxRefusal::CheckNumberInUse),
    ("LINK_TOTAL_MISMATCH", PaySalesTaxRefusal::LinkTotalMismatch),
    ("NOT_STP_PAYMENT", PaySalesTaxRefusal::NotStpPayment),
    ("ALREADY_LINKED", PaySalesTaxRefusal::AlreadyLinked),
    ("NOT_LINKED", PaySalesTaxRefusal::NotLinked),
    (crate::resources::ap_doc::STALE, PaySalesTaxRefusal::Stale),
];

/// Every engine code [`pay_sales_tax_refusal`] knows.
#[must_use]
pub fn pay_sales_tax_refusal_codes() -> Vec<&'static str> {
    REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else: closing, network, plain
/// validation).
#[must_use]
pub fn pay_sales_tax_refusal(e: &ApiError) -> Option<PaySalesTaxRefusal> {
    if let ApiError::Conflict {
        reason: ConflictReason::Stale,
        ..
    } = e
    {
        return Some(PaySalesTaxRefusal::Stale);
    }
    let code = error_code(e)?;
    REFUSALS.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
}

// ---- Requests --------------------------------------------------------------------

/// How the tax is paid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalesTaxPayMethod {
    /// A check (printed later or a typed number).
    #[default]
    Check,
    /// Paid on the agency's site or by bank transfer.
    Electronic,
    /// A method this version does not know.
    #[serde(other)]
    Other,
}

/// One item's amount as the owner set it (`None` lines: the engine pays
/// everything owed).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayLine {
    /// The item.
    pub tax_item_id: SalesTaxItemId,
    /// What this payment pays for the item, adjustments included (may be
    /// 0.00 to leave it). `None` (sent without `amount`): the engine pays
    /// what the item owes plus its adjustments, worked out when it posts,
    /// so an untyped item never freezes at an older preview's figure.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::resources::customer_payments::ser_cents_opt"
    )]
    pub amount: Option<Money>,
    /// The preview's `expectedOwed`, copied (post only).
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::resources::customer_payments::ser_cents_opt"
    )]
    pub expected_owed: Option<Money>,
}

impl fmt::Debug for PayLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayLine")
            .field("tax_item_id", &self.tax_item_id)
            .finish_non_exhaustive()
    }
}

/// An adjustment: raises (positive) or lowers (negative) what is paid for
/// an item, posted to `account_id`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayAdjustment {
    /// The item.
    pub tax_item_id: SalesTaxItemId,
    /// The account (a discount income or a penalty expense account).
    pub account_id: AccountId,
    /// Signed amount (not zero; whole cents).
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// Why (owner text, required).
    pub reason: String,
}

impl fmt::Debug for PayAdjustment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayAdjustment")
            .field("tax_item_id", &self.tax_item_id)
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

/// `POST /sales-tax/payments/preview` and `POST /sales-tax/payments`.
#[derive(Clone, PartialEq, Eq)]
pub struct PaySalesTaxRequest {
    /// The agency paid.
    pub agency_id: SalesTaxAgencyId,
    /// Tax owed through this date.
    pub through_date: CalendarDate,
    /// The payment date.
    pub date: CalendarDate,
    /// The bank account paid from.
    pub bank_account_id: AccountId,
    /// Check or electronic.
    pub method: SalesTaxPayMethod,
    /// The owner's amounts (`None`: everything owed).
    pub lines: Option<Vec<PayLine>>,
    /// Adjustments.
    pub adjustments: Vec<PayAdjustment>,
    /// Queue the check for printing (a check with no number is queued
    /// either way).
    pub print_later: bool,
    /// A hand-written check's number (a whole number above zero).
    pub check_number: Option<u32>,
    /// Memo (owner data).
    pub memo: Option<String>,
    /// The preview's owed total (`owed`), copied for the post as the
    /// top-level `expectedOwed` (required by the engine to pay; never a
    /// sum made here). Not sent on a preview.
    pub expected_owed: Option<Money>,
}

impl fmt::Debug for PaySalesTaxRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaySalesTaxRequest")
            .field("agency_id", &self.agency_id)
            .field("through_date", &self.through_date)
            .field("date", &self.date)
            .field("method", &self.method)
            .field("lines", &self.lines.as_ref().map(Vec::len))
            .field("adjustments", &self.adjustments.len())
            .field("print_later", &self.print_later)
            .finish_non_exhaustive()
    }
}

impl PaySalesTaxRequest {
    /// Checks the request before anything is sent (the engine checks the
    /// figures).
    ///
    /// # Errors
    /// `Local(TooManyLines)` over 200 lines or adjustments;
    /// `Local(InvalidValue)` for a negative line amount, an amount with a
    /// fraction of a cent, a zero adjustment, an adjustment without a
    /// reason, a check number with Print later or on an electronic
    /// payment, a long check number or memo, or the same item twice.
    pub fn check(&self) -> Result<(), ApiError> {
        let bad = || ApiError::Local(LocalValidation::InvalidValue);
        let lines = self.lines.as_deref().unwrap_or_default();
        if lines.len() > PAY_LINES_MAX || self.adjustments.len() > PAY_LINES_MAX {
            return Err(ApiError::Local(LocalValidation::TooManyLines));
        }
        let mut seen: Vec<SalesTaxItemId> = lines.iter().map(|l| l.tax_item_id).collect();
        seen.sort();
        seen.dedup();
        if seen.len() != lines.len() {
            return Err(bad());
        }
        for l in lines {
            if let Some(a) = l.amount {
                if a.is_negative() {
                    return Err(bad());
                }
                cents_wire(a)?;
            }
            if let Some(e) = l.expected_owed {
                cents_wire(e)?;
            }
        }
        for a in &self.adjustments {
            if a.amount.is_zero()
                || a.reason.trim().is_empty()
                || a.reason.chars().count() > ADJUSTMENT_REASON_MAX
            {
                return Err(bad());
            }
            cents_wire(a.amount)?;
        }
        if let Some(n) = self.check_number
            && (n == 0
                || i32::try_from(n).is_err()
                || self.print_later
                || self.method != SalesTaxPayMethod::Check)
        {
            return Err(bad());
        }
        if let Some(e) = self.expected_owed {
            cents_wire(e)?;
        }
        if self.print_later && self.method != SalesTaxPayMethod::Check {
            return Err(bad());
        }
        if too_long(self.memo.as_deref(), PAY_MEMO_MAX) {
            return Err(bad());
        }
        Ok(())
    }

    /// The JSON body (checked first).
    ///
    /// # Errors
    /// As [`PaySalesTaxRequest::check`].
    pub fn body(&self) -> Result<Value, ApiError> {
        self.check()?;
        let mut m = serde_json::Map::new();
        m.insert("agencyId".into(), json!(self.agency_id));
        m.insert("throughDate".into(), json!(self.through_date));
        m.insert("date".into(), json!(self.date));
        m.insert("bankAccountId".into(), json!(self.bank_account_id));
        m.insert("method".into(), json!(self.method));
        if let Some(lines) = &self.lines {
            m.insert(
                "lines".into(),
                serde_json::to_value(lines)
                    .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
            );
        }
        if !self.adjustments.is_empty() {
            m.insert(
                "adjustments".into(),
                serde_json::to_value(&self.adjustments)
                    .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
            );
        }
        if self.print_later {
            m.insert("printLater".into(), json!(true));
        }
        if let Some(n) = self.check_number {
            m.insert("checkNumber".into(), json!(n));
        }
        if let Some(memo) = self.memo.as_deref().filter(|s| !s.trim().is_empty()) {
            m.insert("memo".into(), json!(memo));
        }
        if let Some(e) = self.expected_owed {
            m.insert("expectedOwed".into(), json!(cents_wire(e)?));
        }
        Ok(Value::Object(m))
    }
}

// ---- Preview ---------------------------------------------------------------------

/// One item line of a Pay Sales Tax preview.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayPreviewLine {
    /// The item (`None`: unassigned legacy tax).
    #[serde(default)]
    pub tax_item_id: Option<SalesTaxItemId>,
    /// Its name (owner data).
    #[serde(default)]
    pub name: String,
    /// Owed through the date.
    #[serde(deserialize_with = "de_cents")]
    pub owed: Money,
    /// Σ adjustments on it.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub adjustment: Money,
    /// What this payment pays for it.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// The figure to send back on post (byte copy).
    #[serde(deserialize_with = "de_cents")]
    pub expected_owed: Money,
}

impl fmt::Debug for PayPreviewLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPreviewLine")
            .field("tax_item_id", &self.tax_item_id)
            .finish_non_exhaustive()
    }
}

/// A warning of the preview (nothing is refused by a problem). The engine
/// sends `{code, message, details?}` with the item in `details.taxItemId`;
/// the message and any other details (amounts) are not kept.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "ProblemWire")]
pub struct SalesTaxPayProblem {
    /// `AMOUNT_MORE_THAN_OWED`, `LEGACY_UNASSIGNED`, `NO_VENDOR_ADDRESS`, …
    pub code: String,
    /// The item it is about.
    pub tax_item_id: Option<SalesTaxItemId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProblemDetailsWire {
    #[serde(default)]
    tax_item_id: Option<SalesTaxItemId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProblemWire {
    code: String,
    #[serde(default)]
    tax_item_id: Option<SalesTaxItemId>,
    #[serde(default)]
    details: Option<ProblemDetailsWire>,
}

impl From<ProblemWire> for SalesTaxPayProblem {
    fn from(w: ProblemWire) -> Self {
        SalesTaxPayProblem {
            code: w.code,
            tax_item_id: w
                .tax_item_id
                .or_else(|| w.details.and_then(|d| d.tax_item_id)),
        }
    }
}

impl SalesTaxPayProblem {
    /// The known kinds.
    #[must_use]
    pub fn kind(&self) -> SalesTaxPayProblemKind {
        match self.code.as_str() {
            "AMOUNT_MORE_THAN_OWED" => SalesTaxPayProblemKind::AmountMoreThanOwed,
            "LEGACY_UNASSIGNED" => SalesTaxPayProblemKind::LegacyUnassigned,
            "NO_VENDOR_ADDRESS" => SalesTaxPayProblemKind::NoVendorAddress,
            _ => SalesTaxPayProblemKind::Other,
        }
    }
}

/// What a [`SalesTaxPayProblem`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SalesTaxPayProblemKind {
    /// More than owed is paid on an item.
    AmountMoreThanOwed,
    /// Legacy tax not assigned to an item can't be paid here.
    LegacyUnassigned,
    /// The agency's vendor has no address for the check.
    NoVendorAddress,
    /// A code this version does not know.
    Other,
}

/// `POST /sales-tax/payments/preview`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PayPreview {
    /// One per item of the agency with tax owed or paid.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<PayPreviewLine>,
    /// The check amount.
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
    /// What the payment's items owe in total (the engine's `owed`).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub owed: Option<Money>,
    /// The figure to send back unchanged as the post's top-level
    /// `expectedOwed` (engine PR #221; earlier engines: `owed`).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub expected_owed: Option<Money>,
    /// Warnings.
    #[serde(default, deserialize_with = "null_vec")]
    pub problems: Vec<SalesTaxPayProblem>,
}

impl fmt::Debug for PayPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPreview")
            .field("lines", &self.lines)
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl PayPreview {
    /// What the post sends back as `expectedOwed`: the preview's
    /// `expectedOwed`, else its `owed` (never a sum made here; `None` when
    /// the engine sent neither, and then Pay can't post).
    #[must_use]
    pub fn expected_owed_total(&self) -> Option<Money> {
        self.expected_owed.or(self.owed)
    }

    /// The lines to post, each with the preview's amount and
    /// `expectedOwed` copied (unassigned lines can't be paid here).
    #[must_use]
    pub fn post_lines(&self) -> Vec<PayLine> {
        self.lines
            .iter()
            .filter_map(|l| {
                Some(PayLine {
                    tax_item_id: l.tax_item_id?,
                    amount: Some(l.amount),
                    expected_owed: Some(l.expected_owed),
                })
            })
            .collect()
    }
}

// ---- Results and records -----------------------------------------------------------

/// What Pay answered (201).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaySalesTaxResult {
    /// The `sales_tax_payment` transaction.
    pub id: TransactionId,
    /// Its number.
    #[serde(default)]
    pub number: Option<String>,
    /// A hand-written check's number (a JSON whole number).
    #[serde(default, deserialize_with = "de_check_number")]
    pub check_number: Option<u32>,
    /// `queue` when it waits to be printed (`hand_written` for a typed
    /// number).
    #[serde(default)]
    pub print_status: Option<String>,
    /// The check amount (the engine's).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub total: Option<Money>,
}

impl fmt::Debug for PaySalesTaxResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PaySalesTaxResult")
            .field("id", &self.id)
            .field("print_status", &self.print_status)
            .finish_non_exhaustive()
    }
}

/// What a payment line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentLineKind {
    /// Tax paid.
    Paid,
    /// An adjustment.
    Adjustment,
    /// Paid before the item existed (backfill, no GL; plan B2).
    Opening,
    /// A kind this version does not know.
    #[serde(other)]
    Other,
}

/// One line of a sales tax payment.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxPaymentLine {
    /// The item.
    #[serde(default)]
    pub tax_item_id: Option<SalesTaxItemId>,
    /// The item's name (owner data).
    #[serde(default)]
    pub name: String,
    /// Paid, adjustment or opening.
    pub kind: PaymentLineKind,
    /// The amount (signed for adjustments).
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// An adjustment's account.
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// An adjustment's reason (owner text).
    #[serde(default)]
    pub reason: Option<String>,
}

impl fmt::Debug for SalesTaxPaymentLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxPaymentLine")
            .field("tax_item_id", &self.tax_item_id)
            .field("kind", &self.kind)
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

/// A sales tax payment (`GET /sales-tax/payments/:id` and the list): a
/// Pay Sales Tax check or a linked outside payment.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesTaxPaymentRecord {
    /// The transaction.
    #[serde(alias = "transactionId")]
    pub id: TransactionId,
    /// The agency.
    pub agency_id: SalesTaxAgencyId,
    /// The payment date.
    #[serde(alias = "txnDate", alias = "txn_date")]
    pub date: CalendarDate,
    /// Owed through.
    #[serde(default)]
    pub through_date: Option<CalendarDate>,
    /// Check or electronic.
    #[serde(default)]
    pub method: Option<SalesTaxPayMethod>,
    /// The bank account.
    #[serde(default)]
    pub bank_account_id: Option<AccountId>,
    /// The amount paid.
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
    /// The check number (a JSON whole number).
    #[serde(default, deserialize_with = "de_check_number")]
    pub check_number: Option<u32>,
    /// The transaction number.
    #[serde(default)]
    pub number: Option<String>,
    /// `posted` or `void` (GET; the list leaves it out).
    #[serde(default)]
    pub status: Option<String>,
    /// `queue` while it waits to be printed.
    #[serde(default)]
    pub print_status: Option<String>,
    /// An outside check or journal entry linked as this payment.
    #[serde(default, deserialize_with = "null_false")]
    pub linked: bool,
    /// Voided.
    #[serde(default, deserialize_with = "null_false")]
    pub voided: bool,
    /// The lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<SalesTaxPaymentLine>,
    /// What a check stub lists: each item paid, then each adjustment
    /// (GET only; the engine's labels and amounts).
    #[serde(default, deserialize_with = "null_vec")]
    pub stub_lines: Vec<StubLine>,
    /// Memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
}

impl fmt::Debug for SalesTaxPaymentRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesTaxPaymentRecord")
            .field("id", &self.id)
            .field("agency_id", &self.agency_id)
            .field("date", &self.date)
            .field("method", &self.method)
            .field("linked", &self.linked)
            .field("voided", &self.voided)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

impl SalesTaxPaymentRecord {
    /// Voided (`voided`, or `status: void` on GET).
    #[must_use]
    pub fn is_void(&self) -> bool {
        self.voided || self.status.as_deref() == Some("void")
    }
}

/// One line of a sales tax check's stub.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StubLine {
    /// The item's name or the adjustment's reason (owner data).
    #[serde(default)]
    pub label: String,
    /// Its amount (signed for adjustments).
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}

impl fmt::Debug for StubLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StubLine(..)")
    }
}

/// A check number as the engine sends it: a JSON whole number (a numeric
/// string is read too); `null` or absent is `None`.
fn de_check_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum N {
        Num(u32),
        Text(String),
    }
    match Option::<N>::deserialize(d)? {
        None => Ok(None),
        Some(N::Num(n)) => Ok(Some(n)),
        Some(N::Text(s)) => s
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| serde::de::Error::custom("check number")),
    }
}

/// `POST /sales-tax/payments/:id/unlink` answered.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkResult {
    /// The transaction that is no longer a sales tax payment.
    pub transaction_id: TransactionId,
    /// How many paid lines were dropped (0 on a replay).
    #[serde(default)]
    pub unlinked: u32,
}

/// One item marked paid.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkPaidLine {
    /// The item.
    pub tax_item_id: SalesTaxItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: String,
    /// What was owed and is now recorded as paid outside these books.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}

impl fmt::Debug for MarkPaidLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkPaidLine")
            .field("tax_item_id", &self.tax_item_id)
            .finish_non_exhaustive()
    }
}

/// `POST /sales-tax/payments/mark-paid-through` answered (201). A replay
/// answers `{agencyId, throughDate, replayed: true}` without lines.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkPaidResult {
    /// The agency.
    pub agency_id: SalesTaxAgencyId,
    /// Owed through this date is now 0.00.
    pub through_date: CalendarDate,
    /// The opening lines written.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<MarkPaidLine>,
    /// Their total (absent on a replay).
    #[serde(
        default,
        deserialize_with = "crate::resources::customer_payments::de_cents_opt"
    )]
    pub total: Option<Money>,
    /// The run, for "Undo mark paid through" (engines before the P23
    /// review fix send none, and then there is no undo).
    #[serde(default)]
    pub run_id: Option<String>,
}

/// `POST /sales-tax/payments/mark-paid-through/:runId/undo` answered.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoMarkPaidResult {
    /// The run undone.
    #[serde(default)]
    pub run_id: Option<String>,
    /// How many opening lines were removed.
    #[serde(default)]
    pub removed: u32,
}

impl fmt::Debug for MarkPaidResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkPaidResult")
            .field("agency_id", &self.agency_id)
            .field("through_date", &self.through_date)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

/// Longest "mark paid through" reason.
pub const MARK_PAID_REASON_MAX: usize = 255;

/// `POST /sales-tax/payments/link`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkPaymentRequest {
    /// The check or journal entry that debited Sales Tax Payable.
    pub transaction_id: TransactionId,
    /// The agency it paid.
    pub agency_id: SalesTaxAgencyId,
    /// Its split over the agency's items (the amounts the owner typed;
    /// the engine checks they add up, 400 `LINK_TOTAL_MISMATCH`).
    pub lines: Vec<LinkLine>,
}

impl fmt::Debug for LinkPaymentRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkPaymentRequest")
            .field("transaction_id", &self.transaction_id)
            .field("agency_id", &self.agency_id)
            .field("lines", &self.lines.len())
            .finish()
    }
}

/// One item of a linked payment.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkLine {
    /// The item.
    pub tax_item_id: SalesTaxItemId,
    /// The part of the payment for it (above zero, whole cents).
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
}

impl fmt::Debug for LinkLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkLine")
            .field("tax_item_id", &self.tax_item_id)
            .finish_non_exhaustive()
    }
}

impl LinkPaymentRequest {
    /// Checks the request before anything is sent.
    ///
    /// # Errors
    /// `Local(TooFewLines)` / `Local(TooManyLines)` outside 1-200 lines;
    /// `Local(InvalidValue)` for an amount not above zero, a fraction of a
    /// cent or the same item twice.
    pub fn check(&self) -> Result<(), ApiError> {
        if self.lines.is_empty() {
            return Err(ApiError::Local(LocalValidation::TooFewLines));
        }
        if self.lines.len() > PAY_LINES_MAX {
            return Err(ApiError::Local(LocalValidation::TooManyLines));
        }
        let mut ids: Vec<SalesTaxItemId> = self.lines.iter().map(|l| l.tax_item_id).collect();
        ids.sort();
        ids.dedup();
        if ids.len() != self.lines.len() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        for l in &self.lines {
            if !l.amount.is_positive() {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            cents_wire(l.amount)?;
        }
        Ok(())
    }
}

/// The filters of the payments list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SalesTaxPaymentQuery {
    /// One agency.
    pub agency_id: Option<SalesTaxAgencyId>,
    /// From (inclusive).
    pub start_date: Option<CalendarDate>,
    /// To (inclusive).
    pub end_date: Option<CalendarDate>,
}

// ---- Operations ------------------------------------------------------------------

/// Pay Sales Tax operations of a session (`session.sales_tax_payments()`).
#[derive(Clone, Copy, Debug)]
pub struct SalesTaxPayments<'a>(pub(crate) &'a Session);

impl Session {
    /// Pay Sales Tax and linked payments of the open company (P23).
    #[must_use]
    pub fn sales_tax_payments(&self) -> SalesTaxPayments<'_> {
        SalesTaxPayments(self)
    }
}

impl SalesTaxPayments<'_> {
    /// Every figure of the payment, posting nothing (send `lines: None`
    /// for "pay everything owed").
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-tax-pay-v1`; `Local(…)` from
    /// [`PaySalesTaxRequest::check`]; `Decode`; any [`ApiError`].
    pub async fn preview(&self, r: &PaySalesTaxRequest) -> Result<PayPreview, ApiError> {
        require(self.0, Capabilities::SALES_TAX_PAY)?;
        let body = r.body()?;
        let call =
            Call::at(Method::Post, SALES_TAX_PAY_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = self.0.send(&SALES_TAX_PAY_PREVIEW, call, None).await?;
        decode_one(&reply.body, "preview", "sales tax payment preview")
    }

    /// Posts the payment. `r.lines` should be [`PayPreview::post_lines`]
    /// (every line carries `expectedOwed`).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` (also `InvalidValue` without the
    /// top-level `expectedOwed` or for a line without one); from the engine see
    /// [`pay_sales_tax_refusal`]; closing on the date; any [`ApiError`].
    pub async fn create(
        &self,
        r: &PaySalesTaxRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<PaySalesTaxResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::SALES_TAX_PAY)?;
        let lines = r.lines.as_deref().unwrap_or_default();
        if r.expected_owed.is_none()
            || lines.is_empty()
            || lines.iter().any(|l| l.expected_owed.is_none())
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = posting_body(s, r.body()?, opts)?;
        let mut call = Call::at(Method::Post, SALES_TAX_PAY_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&SALES_TAX_PAY_CREATE, call, None).await?;
        decode_one(&reply.body, "payment", "sales tax payment")
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// Links an outside payment as this agency's.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` ([`LinkPaymentRequest::check`]);
    /// `LINK_TOTAL_MISMATCH`, `NOT_STP_PAYMENT`, `ALREADY_LINKED`; closing;
    /// any [`ApiError`].
    pub async fn link(
        &self,
        r: &LinkPaymentRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::SALES_TAX_PAY)?;
        r.check()?;
        let body =
            serde_json::to_value(r).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, SALES_TAX_PAY_LINK.route()).json_body(body);
        call.idempotency_key = Some(key);
        s.send(&SALES_TAX_PAY_LINK, call, None).await?;
        Ok(())
    }

    /// Undoes a link: the transaction stops counting as a payment to the
    /// agency and what it paid is owed again (no GL change).
    ///
    /// # Errors
    /// `Local(Unsupported)`; 400 `NOT_LINKED` (a Pay Sales Tax payment is
    /// voided instead); closing on its date; any [`ApiError`].
    pub async fn unlink(
        &self,
        id: TransactionId,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<UnlinkResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::SALES_TAX_PAY)?;
        let body = posting_body(s, json!({}), opts)?;
        let mut call = Call::new(
            Method::Post,
            SALES_TAX_PAY_UNLINK.route(),
            format!("/api/v1/sales-tax/payments/{id}/unlink"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&SALES_TAX_PAY_UNLINK, call, None).await?;
        decode_one(&reply.body, "unlink", "sales tax unlink")
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// "Mark paid through": everything the agency's items owe through
    /// `through` is recorded as paid before or outside these books
    /// (opening lines dated `through`, no GL, audited; closing on
    /// `through`). [`Self::undo_mark_paid_through`] takes a run back.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a reason over 255
    /// characters; closing on the date; any [`ApiError`].
    pub async fn mark_paid_through(
        &self,
        agency: SalesTaxAgencyId,
        through: CalendarDate,
        reason: Option<&str>,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<MarkPaidResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::SALES_TAX_PAY)?;
        let mut body = json!({"agencyId": agency, "throughDate": through});
        if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
            if r.chars().count() > MARK_PAID_REASON_MAX {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            body["reason"] = json!(r);
        }
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, SALES_TAX_PAY_MARK_PAID.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&SALES_TAX_PAY_MARK_PAID, call, None).await?;
        decode_one(&reply.body, "result", "mark paid through")
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// "Undo mark paid through": the run's opening lines go and what they
    /// covered is owed again (audited; closing on its date). The route is
    /// provisional until the engine's P23 fix pass lands.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a blank run id;
    /// `NotFound` for an unknown or already undone run; closing; any
    /// [`ApiError`].
    pub async fn undo_mark_paid_through(
        &self,
        run_id: &str,
        opts: &PostingOptions,
    ) -> Result<UndoMarkPaidResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::SALES_TAX_PAY)?;
        let run = run_id.trim();
        if run.is_empty() || !run.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = posting_body(s, json!({}), opts)?;
        let call = Call::new(
            Method::Post,
            SALES_TAX_PAY_MARK_PAID_UNDO.route(),
            format!("/api/v1/sales-tax/payments/mark-paid-through/{run}/undo"),
        )
        .json_body(body);
        let reply = s.send(&SALES_TAX_PAY_MARK_PAID_UNDO, call, None).await?;
        decode_one(&reply.body, "result", "undo mark paid through")
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// One payment with its lines.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<SalesTaxPaymentRecord, ApiError> {
        require(self.0, Capabilities::SALES_TAX_PAY)?;
        let call = Call::new(
            Method::Get,
            SALES_TAX_PAYMENT_GET.route(),
            format!("/api/v1/sales-tax/payments/{id}"),
        );
        let reply = self.0.send(&SALES_TAX_PAYMENT_GET, call, None).await?;
        decode_one(&reply.body, "payment", "sales tax payment")
    }

    /// The payments, newest first.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn list(
        &self,
        q: SalesTaxPaymentQuery,
    ) -> Result<Vec<SalesTaxPaymentRecord>, ApiError> {
        require(self.0, Capabilities::SALES_TAX_PAY)?;
        let mut call = Call::at(Method::Get, SALES_TAX_PAYMENTS_LIST.route());
        if let Some(a) = q.agency_id {
            call.query.push(("agency_id".into(), a.to_string()));
        }
        if let Some(d) = q.start_date {
            call.query.push(("start_date".into(), d.to_string()));
        }
        if let Some(d) = q.end_date {
            call.query.push(("end_date".into(), d.to_string()));
        }
        let reply = self.0.send(&SALES_TAX_PAYMENTS_LIST, call, None).await?;
        decode_list(&reply.body, "payments", "sales tax payments")
    }
}

#[cfg(test)]
#[path = "sales_tax_payments_tests.rs"]
mod tests;
