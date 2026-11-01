//! Credit memos (P22, engine U-137/U-138; contract per the P22 plan §2.2
//! and §2.4, verified at the D8 pin bump).
//!
//! - A credit memo is an A/R credit source like a payment's unapplied
//!   credit: it is applied to invoices at save ("Apply or keep", Q-P22b),
//!   later through [`CreditMemos::apply`], refunded
//!   (`resources::customer_refunds`) or kept.
//! - The engine computes every figure: lines, tax and totals come from
//!   [`CreditMemos::preview`] (the shared invoice compute, so preview =
//!   post), with the client's open invoices, the oldest-first suggestion
//!   and the remaining credit. Nothing here adds or compares money.
//! - A save sends what the owner saw: application amounts copied from the
//!   preview with each invoice's [`BalanceToken`] (409
//!   `INVOICE_BALANCE_CHANGED` when it moved).
//! - Money goes out as 2-place strings and comes back as strings; a JSON
//!   number where money belongs is `Decode`. `Debug` prints ids, dates,
//!   counts and codes only.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, TagId, TransactionId};
use cl_core::{BalanceToken, CalendarDate, Money, Quantity, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{
    ApplyRequest, ApplyResult, MAX_APPLICATIONS, PreviewProblem, cents_wire, de_cents,
    de_cents_opt, de_cents_zero, de_stored, decode_key, error_code, ids_debug, null_false,
    null_vec, require, ser_cents, too_long, with_version,
};
use crate::resources::invoices::{
    EmailMessage, INVOICE_TAGS_MAX, INVOICE_TEXT_MAX, InvoicePreview, OutboxItem, SendEvent,
    SignedPercent, email_body,
};
use crate::resources::operations::{
    CREDIT_MEMO_APPLY, CREDIT_MEMO_CREATE, CREDIT_MEMO_EMAIL, CREDIT_MEMO_EMAIL_LATER,
    CREDIT_MEMO_GET, CREDIT_MEMO_PREVIEW, CREDIT_MEMO_PUT, CREDIT_MEMO_UNAPPLY, Operation,
    PAYMENT_CREDITS,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::sales::{InvoiceLine, LayoutChoice, MAX_LINES};
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// The longest internal memo.
pub const SALES_DOC_MEMO_MAX: usize = 4000;
/// The longest typed document number.
pub const SALES_DOC_NUMBER_MAX: usize = 50;

// ---- Refusals ----------------------------------------------------------------------

/// The engine's refusal of a credit memo, sales receipt or credit write, for
/// the screens' plain-language messages (P22 plan §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CreditRefusal {
    /// Someone changed it since it was read (409 `STALE`).
    Stale,
    /// An invoice's balance moved since the preview: reload the open
    /// invoices (409 `INVOICE_BALANCE_CHANGED`).
    InvoiceBalanceChanged,
    /// More than an invoice's balance (400 `OVERAPPLIED`).
    Overapplied,
    /// The invoice belongs to another client.
    InvoiceNotForCustomer,
    /// Less credit left than asked (409 `CREDIT_NOT_AVAILABLE`; the screen
    /// re-reads the credit for the engine's figure).
    CreditNotAvailable,
    /// The edit would take back credit that is applied or refunded, or
    /// change the client of a used credit (409 `CREDIT_IN_USE`).
    CreditInUse,
    /// Part of the credit was refunded: void the refund first (409
    /// `CREDIT_REFUNDED`).
    CreditRefunded,
    /// A credit used before its own date (400 `APPLY_DATE_BEFORE_SOURCE`).
    ApplyDateBeforeSource,
    /// The credit's date would move after its first use (400
    /// `SOURCE_DATE_AFTER_USE`).
    SourceDateAfterUse,
    /// A credit memo totals zero or less (400
    /// `CREDIT_MEMO_TOTAL_NOT_POSITIVE`, Q-P22r).
    TotalNotPositive,
    /// A negative sales receipt to Payments Clearing (400
    /// `NEGATIVE_RECEIPT_NEEDS_BANK`).
    NegativeReceiptNeedsBank,
    /// The sales receipt is in a deposit (409 `TXN_DEPOSITED`).
    Deposited,
    /// Over 1,000 applications (400 `TOO_MANY_APPLICATIONS`).
    TooManyApplications,
    /// The document number is taken (409 `DOCUMENT_NUMBER_IN_USE`).
    NumberInUse,
}

const CREDIT_REFUSALS: &[(&str, CreditRefusal)] = &[
    (crate::resources::ap_doc::STALE, CreditRefusal::Stale),
    (
        "INVOICE_BALANCE_CHANGED",
        CreditRefusal::InvoiceBalanceChanged,
    ),
    ("OVERAPPLIED", CreditRefusal::Overapplied),
    (
        "INVOICE_NOT_FOR_CUSTOMER",
        CreditRefusal::InvoiceNotForCustomer,
    ),
    ("CREDIT_NOT_AVAILABLE", CreditRefusal::CreditNotAvailable),
    ("CREDIT_IN_USE", CreditRefusal::CreditInUse),
    ("CREDIT_REFUNDED", CreditRefusal::CreditRefunded),
    (
        "APPLY_DATE_BEFORE_SOURCE",
        CreditRefusal::ApplyDateBeforeSource,
    ),
    ("SOURCE_DATE_AFTER_USE", CreditRefusal::SourceDateAfterUse),
    (
        "CREDIT_MEMO_TOTAL_NOT_POSITIVE",
        CreditRefusal::TotalNotPositive,
    ),
    (
        "NEGATIVE_RECEIPT_NEEDS_BANK",
        CreditRefusal::NegativeReceiptNeedsBank,
    ),
    ("TXN_DEPOSITED", CreditRefusal::Deposited),
    ("TOO_MANY_APPLICATIONS", CreditRefusal::TooManyApplications),
    ("DOCUMENT_NUMBER_IN_USE", CreditRefusal::NumberInUse),
];

/// Every engine code [`credit_refusal`] knows (tests and the demo).
#[must_use]
pub fn credit_refusal_codes() -> Vec<&'static str> {
    CREDIT_REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// network, plain validation).
#[must_use]
pub fn credit_refusal(e: &ApiError) -> Option<CreditRefusal> {
    if let ApiError::Conflict {
        reason: ConflictReason::Stale,
        ..
    } = e
    {
        return Some(CreditRefusal::Stale);
    }
    let code = error_code(e)?;
    CREDIT_REFUSALS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

// ---- Open credits (GET /payments/credits) --------------------------------------------

/// What kind of A/R credit a source is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditKind {
    /// A payment's unapplied credit (an overpayment).
    Payment,
    /// A credit memo.
    CreditMemo,
    /// A held customer deposit (P21's deposits on hand; refunds only).
    Deposit,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

/// One open credit of a client (`GET /payments/credits?customerId=`,
/// U-137), oldest first. Named `OpenCredit` (the plan's `CreditSource`)
/// because P21's payment preview already exports a `CreditSource`.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCredit {
    /// The payment or credit memo.
    #[serde(alias = "id")]
    pub source_id: TransactionId,
    /// Payment or credit memo.
    pub kind: CreditKind,
    /// Its date.
    pub date: CalendarDate,
    /// Its number or reference (owner data).
    #[serde(default)]
    pub number: Option<String>,
    /// Its total.
    #[serde(deserialize_with = "de_stored")]
    pub total: Money,
    /// What is left of it.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
    /// The invoice a legacy memo named, while it is open (Q-P22f): the
    /// apply panel preselects it.
    #[serde(default)]
    pub intended_invoice_id: Option<TransactionId>,
}
ids_debug!(
    OpenCredit,
    "OpenCredit",
    source_id,
    kind,
    date,
    intended_invoice_id
);

// ---- Lines of a saved sales document -------------------------------------------------

/// One document line of a saved credit memo or sales receipt.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesDocLine {
    /// The income account.
    #[serde(default)]
    pub account_id: Option<AccountId>,
    /// The item.
    #[serde(default)]
    pub item_id: Option<ItemId>,
    /// Description (owner data).
    #[serde(default)]
    pub description: Option<String>,
    /// Service date.
    #[serde(default)]
    pub service_date: Option<CalendarDate>,
    /// Quantity.
    #[serde(default)]
    pub quantity: Option<Quantity>,
    /// Rate.
    #[serde(default)]
    pub unit_price: Option<Money>,
    /// A discount line's percent.
    #[serde(default)]
    pub discount_percent: Option<SignedPercent>,
    /// Taxable.
    #[serde(default, deserialize_with = "null_false")]
    pub is_taxable: bool,
    /// The line amount.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// The line's tax.
    #[serde(default, deserialize_with = "de_cents_opt")]
    pub tax_amount: Option<Money>,
    /// The line tag.
    #[serde(default)]
    pub tag_id: Option<TagId>,
}
ids_debug!(SalesDocLine, "SalesDocLine", account_id, item_id);

/// How a saved credit memo or sales receipt stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalesDocStatus {
    /// Posted.
    Posted,
    /// Voided.
    Void,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

// ---- Requests --------------------------------------------------------------------

/// One application of a credit memo save, copied from the preview's
/// suggestion or the owner's typed amount.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditMemoApplication {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// How much.
    #[serde(serialize_with = "ser_cents")]
    pub amount: Money,
    /// The invoice's stored balance as the preview read it.
    pub expected_balance_due: BalanceToken,
}
ids_debug!(CreditMemoApplication, "CreditMemoApplication", invoice_id);

/// `POST /credit-memos` (and the bodies of `PUT /credit-memos/{id}` and
/// `POST /credit-memos/preview`). Only what is set goes out.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCreditMemo {
    /// The memo date.
    pub txn_date: CalendarDate,
    /// The client.
    pub contact_id: ContactId,
    /// Lines (1-500): items or income accounts, with tax; never billable
    /// costs.
    pub lines: Vec<InvoiceLine>,
    /// Internal memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// The message printed on the memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_message: Option<String>,
    /// Header tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<TagId>>,
    /// Leave $0 lines off the printed memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_zero_lines: Option<bool>,
    /// The print layout (U-130).
    #[serde(skip_serializing_if = "LayoutChoice::is_omitted")]
    pub layout_id: LayoutChoice,
    /// A typed number (else the counter's next).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn_number: Option<String>,
    /// A number already in use posts with a warning.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warn_only: bool,
    /// Waiting to be printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_later: Option<bool>,
    /// Apply at save (Q-P22b): copied from the preview; empty keeps the
    /// whole memo as a credit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub applications: Vec<CreditMemoApplication>,
}

impl NewCreditMemo {
    /// A credit memo for `contact` dated `date`; every optional field unset.
    #[must_use]
    pub fn new(contact: ContactId, date: CalendarDate, lines: Vec<InvoiceLine>) -> NewCreditMemo {
        NewCreditMemo {
            txn_date: date,
            contact_id: contact,
            lines,
            memo: None,
            customer_message: None,
            tag_ids: None,
            hide_zero_lines: None,
            layout_id: LayoutChoice::Omitted,
            txn_number: None,
            warn_only: false,
            print_later: None,
            applications: Vec::new(),
        }
    }
}

impl fmt::Debug for NewCreditMemo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewCreditMemo")
            .field("txn_date", &self.txn_date)
            .field("contact_id", &self.contact_id)
            .field("lines", &self.lines.len())
            .field("applications", &self.applications.len())
            .finish_non_exhaustive()
    }
}

/// Checks the lines and header text of a credit memo or sales receipt.
pub(crate) fn check_doc_lines(
    lines: &[InvoiceLine],
    customer_message: Option<&str>,
    memo: Option<&str>,
    tags: Option<&Vec<TagId>>,
    number: Option<&str>,
) -> Result<(), ApiError> {
    if lines.is_empty() {
        return Err(ApiError::Local(LocalValidation::TooFewLines));
    }
    if lines.len() > MAX_LINES {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    let bad_line = |l: &InvoiceLine| {
        // Billable costs are invoice-only (400 on these documents, §2.4).
        l.billable_source_line_id.is_some()
            || l.markup_percent.is_some()
            || (l.account_id.is_none() && l.item_id.is_none())
            || too_long(l.description.as_deref(), INVOICE_TEXT_MAX)
    };
    if lines.iter().any(bad_line)
        || too_long(customer_message, INVOICE_TEXT_MAX)
        || too_long(memo, SALES_DOC_MEMO_MAX)
        || too_long(number.map(str::trim), SALES_DOC_NUMBER_MAX)
        || tags.is_some_and(|t| t.len() > INVOICE_TAGS_MAX)
    {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(())
}

/// Checks a credit memo before anything is sent.
///
/// # Errors
/// `Local(Unsupported)` without `credit-memo-edit-v1` (or
/// `credit-memo-apply-v1` for applications); `TooFewLines`,
/// `TooManyLines`; `InvalidValue` for a billable line, a line with neither
/// account nor item, text over its limit, or an application that is not a
/// positive whole-cent amount.
pub fn check_credit_memo(m: &NewCreditMemo, caps: &Capabilities) -> Result<(), ApiError> {
    if !caps.has(Capabilities::CREDIT_MEMO_EDIT)
        || (!m.applications.is_empty() && !caps.has(Capabilities::CREDIT_MEMO_APPLY))
    {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    check_doc_lines(
        &m.lines,
        m.customer_message.as_deref(),
        m.memo.as_deref(),
        m.tag_ids.as_ref(),
        m.txn_number.as_deref(),
    )?;
    if m.applications.len() > MAX_APPLICATIONS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    for a in &m.applications {
        if !a.amount.is_positive() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        cents_wire(a.amount)?;
    }
    Ok(())
}

// ---- Preview ---------------------------------------------------------------------

/// One open invoice of the client in a credit memo preview.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditOpenInvoice {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub number: Option<String>,
    /// Its date.
    pub date: CalendarDate,
    /// When it is due.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Its cent balance.
    #[serde(deserialize_with = "de_cents")]
    pub balance_due: Money,
    /// Its stored balance, echoed back as `expectedBalanceDue`.
    pub balance_due_exact: BalanceToken,
}
ids_debug!(
    CreditOpenInvoice,
    "CreditOpenInvoice",
    invoice_id,
    date,
    due_date
);

/// The engine's oldest-first allocation of the memo (Q-P22b).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedApplication {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
}
ids_debug!(SuggestedApplication, "SuggestedApplication", invoice_id);

/// `POST /credit-memos/preview`: P20's invoice preview figures plus the
/// client's open invoices, the suggested allocation and what would remain.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditMemoPreview {
    /// Lines, subtotal, tax, total and the client's balance after.
    #[serde(flatten)]
    pub figures: InvoicePreview,
    /// The client's open invoices, oldest first.
    #[serde(default, deserialize_with = "null_vec")]
    pub open_invoices: Vec<CreditOpenInvoice>,
    /// Oldest first up to the total.
    #[serde(default, deserialize_with = "null_vec")]
    pub suggested_applications: Vec<SuggestedApplication>,
    /// What stays a credit after the suggestion.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub remaining_credit: Money,
    /// Problems (never 4xx from the preview).
    #[serde(default, deserialize_with = "null_vec")]
    pub problems: Vec<PreviewProblem>,
}

impl fmt::Debug for CreditMemoPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditMemoPreview")
            .field("figures", &self.figures)
            .field("open_invoices", &self.open_invoices.len())
            .field("suggested", &self.suggested_applications.len())
            .field("problems", &self.problems)
            .finish_non_exhaustive()
    }
}

impl CreditMemoPreview {
    /// The open invoice row of `invoice`.
    #[must_use]
    pub fn open_invoice(&self, invoice: TransactionId) -> Option<&CreditOpenInvoice> {
        self.open_invoices.iter().find(|i| i.invoice_id == invoice)
    }
}

// ---- The saved record ------------------------------------------------------------

/// One application of a saved credit memo.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditMemoAppliedRow {
    /// The invoice.
    pub invoice_id: TransactionId,
    /// Its number (owner data).
    #[serde(default)]
    pub invoice_number: Option<String>,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// When (`None`: with the memo, at save).
    #[serde(default)]
    pub applied_date: Option<CalendarDate>,
    /// Unapplied or voided.
    #[serde(default, deserialize_with = "null_false")]
    pub voided: bool,
}
ids_debug!(
    CreditMemoAppliedRow,
    "CreditMemoAppliedRow",
    invoice_id,
    applied_date,
    voided
);

/// One refund of a saved credit memo.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditMemoRefundRow {
    /// The `customer_refund`.
    pub refund_id: TransactionId,
    /// Its date.
    pub date: CalendarDate,
    /// How much.
    #[serde(deserialize_with = "de_cents")]
    pub amount: Money,
    /// Method (`check`, ...).
    #[serde(default)]
    pub method: Option<String>,
    /// Voided.
    #[serde(default, deserialize_with = "null_false")]
    pub voided: bool,
}
ids_debug!(
    CreditMemoRefundRow,
    "CreditMemoRefundRow",
    refund_id,
    date,
    voided
);

/// A saved credit memo (`GET /credit-memos/{id}`, `{creditMemo: …}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditMemoRecord {
    /// The memo.
    pub id: TransactionId,
    /// Its number.
    #[serde(default, alias = "txnNumber")]
    pub number: Option<String>,
    /// Its date.
    #[serde(alias = "txnDate")]
    pub date: CalendarDate,
    /// The client.
    #[serde(alias = "contactId")]
    pub customer_id: ContactId,
    /// Its lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<SalesDocLine>,
    /// Σ lines.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub subtotal: Money,
    /// The tax given back.
    #[serde(default, alias = "taxTotal", deserialize_with = "de_cents_zero")]
    pub tax_amount: Money,
    /// The total credit.
    #[serde(deserialize_with = "de_cents")]
    pub total: Money,
    /// Internal memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
    /// The printed message (owner data).
    #[serde(default)]
    pub customer_message: Option<String>,
    /// The version to send back as `expectedUpdatedAt`.
    pub updated_at: Timestamp,
    /// Posted or void.
    pub status: SalesDocStatus,
    /// Applications, live and not.
    #[serde(default, deserialize_with = "null_vec")]
    pub applications: Vec<CreditMemoAppliedRow>,
    /// Refunds, live and not.
    #[serde(default, deserialize_with = "null_vec")]
    pub refunds: Vec<CreditMemoRefundRow>,
    /// What is left of the credit.
    #[serde(deserialize_with = "de_cents")]
    pub remaining: Money,
    /// Every send, oldest first (U-144, `sales-doc-email-v1`; empty on
    /// older engines).
    #[serde(default, deserialize_with = "null_vec")]
    pub send_history: Vec<SendEvent>,
}

impl fmt::Debug for CreditMemoRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditMemoRecord")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("customer_id", &self.customer_id)
            .field("status", &self.status)
            .field("lines", &self.lines.len())
            .field("applications", &self.applications.len())
            .field("refunds", &self.refunds.len())
            .finish_non_exhaustive()
    }
}

impl CreditMemoRecord {
    /// The live applications.
    pub fn live_applications(&self) -> impl Iterator<Item = &CreditMemoAppliedRow> {
        self.applications.iter().filter(|a| !a.voided)
    }

    /// The live refunds.
    pub fn live_refunds(&self) -> impl Iterator<Item = &CreditMemoRefundRow> {
        self.refunds.iter().filter(|r| !r.voided)
    }

    /// True when it is void.
    #[must_use]
    pub fn is_void(&self) -> bool {
        self.status == SalesDocStatus::Void
    }
}

/// Reads a saved credit memo or sales receipt under `key`. The engine
/// nests the figures in `totals: {subtotal, taxTotal, total}`; they are
/// lifted to the record's own fields (a top-level figure, as older
/// fixtures had it, is kept).
///
/// # Errors
/// `Decode` when the answer can't be read.
pub(crate) fn decode_sales_doc<T: for<'de> Deserialize<'de>>(
    body: &[u8],
    key: &str,
    what: &'static str,
) -> Result<T, ApiError> {
    let mut v: serde_json::Map<String, Value> =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let mut inner = v.remove(key).ok_or(ApiError::Decode { what })?;
    if let Value::Object(doc) = &mut inner
        && let Some(Value::Object(totals)) = doc.remove("totals")
    {
        for (from, to) in [
            ("subtotal", "subtotal"),
            ("taxTotal", "taxAmount"),
            ("total", "total"),
        ] {
            if let Some(x) = totals.get(from)
                && !doc.contains_key(to)
            {
                doc.insert(to.into(), x.clone());
            }
        }
    }
    serde_json::from_value(inner).map_err(|_| ApiError::Decode { what })
}

/// Reads a credit memo from an answer (`{creditMemo: …}`).///
/// # Errors
/// `SavedUnreadable` when the answer can't be read.
pub fn saved_credit_memo(body: &[u8]) -> Result<CreditMemoRecord, ApiError> {
    decode_sales_doc(body, "creditMemo", "credit memo")
        .map_err(crate::resources::versioning::saved_unreadable)
}

/// Options of a credit memo edit, apply or unapply.
#[derive(Clone, Debug, Default)]
pub struct CreditWriteOptions {
    /// The closing-date password.
    pub posting: PostingOptions,
    /// The memo's `updatedAt` when it was read.
    pub expected_updated_at: Option<Timestamp>,
}

/// The body of a credit memo or sales receipt create, edit or preview.
pub(crate) fn doc_body<T: Serialize>(
    doc: &T,
    editing: Option<TransactionId>,
) -> Result<Value, ApiError> {
    let mut body =
        serde_json::to_value(doc).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    if let (Some(id), Value::Object(map)) = (editing, &mut body) {
        map.insert("editingId".into(), Value::String(id.to_string()));
    }
    Ok(body)
}

/// Emails a credit memo or sales receipt (U-144, `sales-doc-email-v1`):
/// the invoice email's body and errors on the document's own route
/// (`to`: the operation and its path).
pub(crate) async fn send_doc_email<T>(
    s: &Session,
    (op, path): (&'static Operation, String),
    message: &EmailMessage,
    pdf: (&str, &[u8]),
    outbox_id: Option<crate::resources::invoices::OutboxId>,
    key: IdempotencyKey,
    read: fn(&[u8]) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    require(s, Capabilities::SALES_DOC_EMAIL)?;
    let body = email_body(message, pdf.0, pdf.1, outbox_id)?;
    let mut call = Call::new(Method::Post, op.route(), path).json_body(json_body(&body)?);
    call.idempotency_key = Some(key);
    read(&s.send(op, call, None).await?.body)
}

/// Puts a credit memo or sales receipt email in the Outbox (U-144):
/// 201 `{item}`.
pub(crate) async fn queue_doc_email(
    s: &Session,
    op: &'static Operation,
    path: String,
    message: &EmailMessage,
    key: IdempotencyKey,
) -> Result<OutboxItem, ApiError> {
    require(s, Capabilities::SALES_DOC_EMAIL)?;
    message.check()?;
    let mut call = Call::new(Method::Post, op.route(), path)
        .json_body(json_body(&Value::Object(message.json()))?);
    call.idempotency_key = Some(key);
    let reply = s.send(op, call, None).await?;
    decode_key(&reply.body, "item", "outbox item")
        .map_err(crate::resources::versioning::saved_unreadable)
}

/// Credit memo operations of a session (`session.credit_memos()`).
#[derive(Clone, Copy, Debug)]
pub struct CreditMemos<'a>(pub(crate) &'a Session);

impl Session {
    /// Credit memos of the open company (P22).
    #[must_use]
    pub fn credit_memos(&self) -> CreditMemos<'_> {
        CreditMemos(self)
    }
}

impl CreditMemos<'_> {
    /// Every figure of `m` as the engine would post it, posting nothing.
    /// `editing` leaves the saved memo's own amount out of the client's
    /// balance and remaining credit.
    ///
    /// # Errors
    /// The local checks of [`check_credit_memo`]; `Decode` (also when the
    /// preview's line count differs from the request's); any [`ApiError`].
    pub async fn preview(
        &self,
        m: &NewCreditMemo,
        editing: Option<TransactionId>,
    ) -> Result<CreditMemoPreview, ApiError> {
        let s = &self.0.pinned();
        check_credit_memo(m, &s.client().capabilities())?;
        let body = doc_body(m, editing)?;
        let call = Call::at(Method::Post, CREDIT_MEMO_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = s.send(&CREDIT_MEMO_PREVIEW, call, None).await?;
        let p: CreditMemoPreview =
            serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
                what: "credit memo preview",
            })?;
        if p.figures.lines.len() != m.lines.len() {
            return Err(ApiError::Decode {
                what: "credit memo preview",
            });
        }
        Ok(p)
    }

    /// Creates a credit memo, applying `m.applications` in the same
    /// transaction (keyed under `idempotency-credit-memo-v1`).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)`; from the engine see
    /// [`credit_refusal`]; `ClosedPeriod`, `ClosingPasswordRejected`;
    /// `SavedUnreadable`; any [`ApiError`].
    pub async fn create(
        &self,
        m: &NewCreditMemo,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<CreditMemoRecord, ApiError> {
        let s = &self.0.pinned();
        check_credit_memo(m, &s.client().capabilities())?;
        let body = posting_body(s, doc_body(m, None)?, opts)?;
        let mut call = Call::at(Method::Post, CREDIT_MEMO_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CREDIT_MEMO_CREATE, call, None).await?;
        saved_credit_memo(&reply.body)
    }

    /// One credit memo with its applications and refunds.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<CreditMemoRecord, ApiError> {
        require(self.0, Capabilities::CREDIT_MEMO_EDIT)?;
        let call = Call::new(
            Method::Get,
            CREDIT_MEMO_GET.route(),
            format!("/api/v1/credit-memos/{id}"),
        );
        let reply = self.0.send(&CREDIT_MEMO_GET, call, None).await?;
        decode_sales_doc(&reply.body, "creditMemo", "credit memo")
    }

    /// Edits a credit memo if it is still at `opts.expected_updated_at`.
    ///
    /// # Errors
    /// As [`CreditMemos::create`], plus `STALE`, `CREDIT_IN_USE`,
    /// `SOURCE_DATE_AFTER_USE`.
    pub async fn update(
        &self,
        id: TransactionId,
        m: &NewCreditMemo,
        opts: &CreditWriteOptions,
        key: IdempotencyKey,
    ) -> Result<CreditMemoRecord, ApiError> {
        let s = &self.0.pinned();
        check_credit_memo(m, &s.client().capabilities())?;
        let body = posting_body(
            s,
            with_version(doc_body(m, None)?, opts.expected_updated_at),
            &opts.posting,
        )?;
        let mut call = Call::new(
            Method::Put,
            CREDIT_MEMO_PUT.route(),
            format!("/api/v1/credit-memos/{id}"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CREDIT_MEMO_PUT, call, None).await?;
        saved_credit_memo(&reply.body)
    }

    /// Applies a credit memo to invoices (`amount: None`: the engine fills
    /// min(remaining, balance)).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `credit-memo-apply-v1`;
    /// `Local(TooFewLines)` for none or over 1,000; from the engine:
    /// `CREDIT_NOT_AVAILABLE`, `INVOICE_BALANCE_CHANGED`,
    /// `APPLY_DATE_BEFORE_SOURCE`, `STALE`, closing; any [`ApiError`].
    pub async fn apply(
        &self,
        memo: TransactionId,
        r: &ApplyRequest,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<ApplyResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::CREDIT_MEMO_APPLY)?;
        if r.applications.is_empty() || r.applications.len() > MAX_APPLICATIONS {
            return Err(ApiError::Local(LocalValidation::TooFewLines));
        }
        for m in r.applications.iter().filter_map(|a| a.amount) {
            if !m.is_positive() {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            cents_wire(m)?;
        }
        let body = with_version(
            json!({"date": r.date, "applications": r.applications}),
            r.expected_updated_at,
        );
        let body = posting_body(s, body, opts)?;
        let mut call = Call::new(
            Method::Post,
            CREDIT_MEMO_APPLY.route(),
            format!("/api/v1/credit-memos/{memo}/apply"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CREDIT_MEMO_APPLY, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode { what: "apply" })
    }

    /// Takes one invoice off a credit memo; answers what is left of it.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `STALE`, closing on the row's date; any
    /// [`ApiError`].
    pub async fn unapply(
        &self,
        memo: TransactionId,
        invoice: TransactionId,
        opts: &CreditWriteOptions,
        key: IdempotencyKey,
    ) -> Result<Money, ApiError> {
        #[derive(Deserialize)]
        struct W {
            #[serde(deserialize_with = "de_cents")]
            remaining: Money,
        }
        let s = &self.0.pinned();
        require(s, Capabilities::CREDIT_MEMO_APPLY)?;
        let body = with_version(json!({"invoiceId": invoice}), opts.expected_updated_at);
        let body = posting_body(s, body, &opts.posting)?;
        let mut call = Call::new(
            Method::Post,
            CREDIT_MEMO_UNAPPLY.route(),
            format!("/api/v1/credit-memos/{memo}/unapply"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&CREDIT_MEMO_UNAPPLY, call, None).await?;
        serde_json::from_slice::<W>(&reply.body)
            .map(|w| w.remaining)
            .map_err(|_| ApiError::Decode { what: "unapply" })
    }

    /// Emails the memo through the company SMTP with `pdf` attached
    /// (`CreditMemo-<number>.pdf`) and records an "email" send (`POST
    /// /credit-memos/{id}/email`, U-144). Keep one key per send and reuse
    /// it on a retry; `outbox_id` (Send all) takes the Outbox message out
    /// in the same commit.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `sales-doc-email-v1`; the local checks
    /// of the invoice email; from the engine `SMTP_NOT_CONFIGURED`,
    /// `SMTP_SEND_FAILED`, `DOCUMENT_VOID`, `OUTBOX_ITEM_NOT_FOUND` (see
    /// [`crate::invoice_refusal`]), any [`ApiError`].
    pub async fn email(
        &self,
        id: TransactionId,
        message: &EmailMessage,
        pdf_file_name: &str,
        pdf: &[u8],
        outbox_id: Option<crate::resources::invoices::OutboxId>,
        key: IdempotencyKey,
    ) -> Result<CreditMemoRecord, ApiError> {
        send_doc_email(
            self.0,
            (
                &CREDIT_MEMO_EMAIL,
                format!("/api/v1/credit-memos/{id}/email"),
            ),
            message,
            (pdf_file_name, pdf),
            outbox_id,
            key,
            saved_credit_memo,
        )
        .await
    }

    /// Puts the memo's email in the Outbox (`POST
    /// /credit-memos/{id}/email-later`, U-144; kind `credit_memo`).
    ///
    /// # Errors
    /// As [`CreditMemos::email`] without the PDF checks.
    pub async fn email_later(
        &self,
        id: TransactionId,
        message: &EmailMessage,
        key: IdempotencyKey,
    ) -> Result<OutboxItem, ApiError> {
        queue_doc_email(
            self.0,
            &CREDIT_MEMO_EMAIL_LATER,
            format!("/api/v1/credit-memos/{id}/email-later"),
            message,
            key,
        )
        .await
    }

    /// A client's open credits (payment credits and credit memos), oldest
    /// first. Held deposits stay on P21's `deposits_on_hand`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `credit-memo-apply-v1`; `NotFound`;
    /// `Decode`; any [`ApiError`].
    pub async fn credits(&self, customer: ContactId) -> Result<Vec<OpenCredit>, ApiError> {
        require(self.0, Capabilities::CREDIT_MEMO_APPLY)?;
        let mut call = Call::at(Method::Get, PAYMENT_CREDITS.route());
        call.query = vec![("customerId".into(), customer.to_string())];
        let reply = self.0.send(&PAYMENT_CREDITS, call, None).await?;
        decode_key(&reply.body, "data", "credits")
    }
}

#[cfg(test)]
#[path = "credit_memos_tests.rs"]
mod tests;
