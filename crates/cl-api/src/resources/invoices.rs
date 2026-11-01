//! Create Invoice (P20 WP-A): the engine's invoice routes, the preview,
//! send history, email and the Outbox, billable costs, price levels and the
//! client's sales figures.
//!
//! - **The engine computes every figure** (U-122, U-129, Q-P20n): line
//!   amounts, discounts, subtotals, tax, totals, the due date and the
//!   client's balance after the invoice come from [`Invoices::preview`] or
//!   the saved record. Nothing here adds money up.
//! - Built against engine PR #197 (E1: `sales-cents-v1`, `invoice-edit-v1`,
//!   `idempotency-invoice-v1`, `invoice-terms-v1`, `invoice-fields-v1`,
//!   `invoice-sent-history-v1`, `contact-balance-v1`). The preview and price
//!   levels (E2, U-129/U-40), billable costs (E3, U-128) and email and the
//!   Outbox (E4, U-127) follow the P20 plan §2 contract table: their wire
//!   shapes are marked "contract per P20 plan §2; verify when E2/E3/E4
//!   land".
//! - A field or call the engine does not advertise is refused locally,
//!   `Local(Unsupported)`, with nothing sent.
//! - `Debug` never prints names, memos, messages, recipients or amounts;
//!   nothing here logs. The PDF and the message body never reach a log.

use std::fmt;
use std::str::FromStr;

use base64::Engine as _;
use cl_core::ids::{
    AccountId, ContactId, IdError, ItemId, JournalLineId, TagId, TransactionId, parse_uuid,
};
use cl_core::{CalendarDate, Money, Quantity, Rate, Timestamp};
use rust_decimal::Decimal;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::uuid_id;
use crate::resources::bills::PaymentTerm;
use crate::resources::common::{Query, remember_created};
use crate::resources::operations::{
    BILLABLE_COSTS, CONTACT_GET, CONTACT_PUT, EMAIL_QUEUE, EMAIL_QUEUE_DELETE, INVOICE_EMAIL,
    INVOICE_EMAIL_LATER, INVOICE_GET, INVOICE_PREVIEW, INVOICE_PUT, INVOICE_SENT, INVOICE_VOID,
    INVOICES_CREATE, INVOICES_LIST, Operation, PAYMENT_TERMS, PRICE_LEVEL_DELETE, PRICE_LEVEL_GET,
    PRICE_LEVEL_PUT, PRICE_LEVELS_CREATE, PRICE_LEVELS_LIST,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::printing::{DocumentNumberWarning, number_warnings};
use crate::resources::sales::{MAX_LINES, NewInvoice};
use crate::resources::transactions::{Transaction, TxnStatus, TxnType};
use crate::resources::versioning::{
    EditableFields, Versioned, get_versioned, saved_from, saved_unreadable,
};
use crate::session::{Session, decode, json_body};
use crate::wire::{Capabilities, Page, PageRequest};

const BASE: &str = "/api/v1/invoices";

/// The longest description of a line and the longest customer message.
pub const INVOICE_TEXT_MAX: usize = 4000;
/// The longest P.O. number.
pub const PO_NUMBER_MAX: usize = 50;
/// Header tags on one invoice.
pub const INVOICE_TAGS_MAX: usize = 50;
/// The largest PDF an email may carry (Q-P20q).
pub const EMAIL_PDF_MAX_BYTES: usize = 10 * 1024 * 1024;
/// Recipients per address field of an email (and per send event).
pub const EMAIL_RECIPIENTS_MAX: usize = 50;
/// The longest custom terms period (days, `invoice-terms-v1`).
pub const INVOICE_TERMS_DAYS_MAX: u32 = 3650;

// ---- Engine refusals ----------------------------------------------------------

/// 409: a payment or credit is applied; void it first (U-124).
pub const INVOICE_HAS_PAYMENTS: &str = "INVOICE_HAS_PAYMENTS";
/// 400: a void invoice can't be recorded as sent (U-126).
pub const INVOICE_VOID_CODE: &str = "INVOICE_VOID";
/// 409: the billable cost was invoiced meanwhile (U-128;
/// `details.conflicts`).
pub const BILLABLE_ALREADY_BILLED: &str = "BILLABLE_ALREADY_BILLED";
/// 400: the company has no SMTP server set up, or its host changed (U-127).
pub const SMTP_NOT_CONFIGURED: &str = "SMTP_NOT_CONFIGURED";
/// 400: the SMTP server refused the message (`details.reason: "auth"` for a
/// login failure; U-127).
pub const SMTP_SEND_FAILED: &str = "SMTP_SEND_FAILED";

/// Every engine code [`invoice_refusal`] knows, with its refusal.
const REFUSAL_CODES: &[(&str, InvoiceRefusal)] = &[
    (crate::resources::ap_doc::STALE, InvoiceRefusal::Stale),
    (INVOICE_HAS_PAYMENTS, InvoiceRefusal::HasPayments),
    (INVOICE_VOID_CODE, InvoiceRefusal::Void),
    (
        BILLABLE_ALREADY_BILLED,
        InvoiceRefusal::BillableAlreadyBilled,
    ),
    (
        "BILLABLE_COST_UNAVAILABLE",
        InvoiceRefusal::BillableUnavailable,
    ),
    (
        "BILLABLE_LINES_MISSING",
        InvoiceRefusal::BillableLinesMissing,
    ),
    ("INVOICE_LINES_MISSING", InvoiceRefusal::LinesMissing),
    (
        "BILLABLE_CONTACT_MISMATCH",
        InvoiceRefusal::BillableClientMismatch,
    ),
    ("BILLABLE_DUPLICATE_LINE", InvoiceRefusal::BillableDuplicate),
    (
        "BILLABLE_FIELDS_READONLY",
        InvoiceRefusal::BillableFieldsReadOnly,
    ),
    (
        "BILLABLE_INCOME_ACCOUNT_REQUIRED",
        InvoiceRefusal::BillableAccountRequired,
    ),
    ("BILLABLE_LINK_BROKEN", InvoiceRefusal::BillableUnavailable),
    (
        "MARKUP_ACCOUNT_REQUIRED",
        InvoiceRefusal::MarkupAccountRequired,
    ),
    (
        "MARKUP_ACCOUNT_INVALID",
        InvoiceRefusal::MarkupAccountInvalid,
    ),
    ("INVOICE_TOTAL_BELOW_PAID", InvoiceRefusal::TotalBelowPaid),
    (
        "FINANCE_CHARGE_INVOICE",
        InvoiceRefusal::FinanceChargeInvoice,
    ),
    (
        "RECURRING_HAS_BILLABLE_COSTS",
        InvoiceRefusal::RecurringHasBillable,
    ),
    ("RETRY", InvoiceRefusal::Retry),
    (
        "FINANCE_CHARGE_CLIENT_CHANGE",
        InvoiceRefusal::FinanceChargeClientChange,
    ),
    // U-144: a void credit memo or sales receipt can't be emailed.
    ("DOCUMENT_VOID", InvoiceRefusal::Void),
    ("ATTACHMENT_TOO_LARGE", InvoiceRefusal::AttachmentTooLarge),
    ("OUTBOX_ITEM_NOT_FOUND", InvoiceRefusal::OutboxItemGone),
    ("OFFLINE_UNAVAILABLE", InvoiceRefusal::Offline),
    (SMTP_NOT_CONFIGURED, InvoiceRefusal::SmtpNotConfigured),
    ("INVOICE_LAYOUT_NOT_FOUND", InvoiceRefusal::LayoutGone),
    ("INVOICE_LAYOUT_ARCHIVED", InvoiceRefusal::LayoutArchived),
];

/// The engine's refusal of an invoice save, void or send, for the form's
/// plain-language message (plan §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvoiceRefusal {
    /// Someone changed it since it was read (409 `STALE`): reload or keep.
    Stale,
    /// Payments or credits are applied (409 `INVOICE_HAS_PAYMENTS`).
    HasPayments,
    /// The invoice is void (`INVOICE_VOID`).
    Void,
    /// A billable cost was billed meanwhile: reload the costs.
    BillableAlreadyBilled,
    /// A billable cost is gone (voided or changed; `BILLABLE_COST_UNAVAILABLE`).
    BillableUnavailable,
    /// The edit leaves out a billed cost without releasing it (409
    /// `BILLABLE_LINES_MISSING`).
    BillableLinesMissing,
    /// The edit drops discount or subtotal lines the invoice has (409
    /// `INVOICE_LINES_MISSING`).
    LinesMissing,
    /// The cost belongs to another client (`BILLABLE_CONTACT_MISMATCH`).
    BillableClientMismatch,
    /// The same cost twice (`BILLABLE_DUPLICATE_LINE`).
    BillableDuplicate,
    /// Item, quantity or rate sent on a billable line.
    BillableFieldsReadOnly,
    /// A billable line without its income account.
    BillableAccountRequired,
    /// A markup with no markup account set (Preferences).
    MarkupAccountRequired,
    /// The markup account is not an income account.
    MarkupAccountInvalid,
    /// The edit would make the total less than what was paid.
    TotalBelowPaid,
    /// A finance-charge invoice's amount, lines or date can't be edited
    /// (409 `FINANCE_CHARGE_INVOICE`, U-141): void it and assess again.
    FinanceChargeInvoice,
    /// A finance-charge invoice keeps its client (409
    /// `FINANCE_CHARGE_CLIENT_CHANGE`, U-145, Q-P22v): void it and assess
    /// again for the right client.
    FinanceChargeClientChange,
    /// A recurring schedule can't copy billable costs.
    RecurringHasBillable,
    /// The engine asks to try again (409 `RETRY`; same key).
    Retry,
    /// The PDF is over 10 MB.
    AttachmentTooLarge,
    /// The Outbox message is gone (sent or removed meanwhile).
    OutboxItemGone,
    /// The engine is offline for email (503 `OFFLINE_UNAVAILABLE`).
    Offline,
    /// No SMTP server set up: link to Preferences.
    SmtpNotConfigured,
    /// The mail server refused it: offer the Outbox or Save PDF.
    SmtpSendFailed,
    /// The mail server refused the login (`SMTP_SEND_FAILED`, reason
    /// `auth`).
    SmtpLoginFailed,
    /// The invoice layout is gone.
    LayoutGone,
    /// The invoice layout is archived.
    LayoutArchived,
}

impl InvoiceRefusal {
    /// True when the mail server can't send anything now (Send all stops).
    #[must_use]
    pub const fn stops_sending(self) -> bool {
        matches!(
            self,
            InvoiceRefusal::SmtpNotConfigured
                | InvoiceRefusal::SmtpLoginFailed
                | InvoiceRefusal::Offline
        )
    }
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
pub fn invoice_refusal(e: &ApiError) -> Option<InvoiceRefusal> {
    use crate::errors::ConflictReason;
    match e {
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => return Some(InvoiceRefusal::Stale),
        ApiError::OfflineUnavailable { .. } => return Some(InvoiceRefusal::Offline),
        ApiError::PayloadTooLarge | ApiError::Local(LocalValidation::AttachmentTooLarge) => {
            return Some(InvoiceRefusal::AttachmentTooLarge);
        }
        ApiError::Validation(v) if v.code().is_some_and(|c| c.as_str() == SMTP_SEND_FAILED) => {
            let auth = v.reason().is_some_and(|r| r.as_str() == "AUTH");
            return Some(if auth {
                InvoiceRefusal::SmtpLoginFailed
            } else {
                InvoiceRefusal::SmtpSendFailed
            });
        }
        _ => {}
    }
    let code = error_code(e)?;
    if code == SMTP_SEND_FAILED {
        return Some(InvoiceRefusal::SmtpSendFailed);
    }
    REFUSAL_CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, r)| *r)
}

// ---- A signed percent ----------------------------------------------------------

/// A percent such as a price level's `-7.5` or a markup's `10`: at most 4
/// decimals, above -100 and at most 1000 (the engine's markup range is 0 to
/// 1000 inclusive). Exact; never a float.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SignedPercent(Decimal);

impl SignedPercent {
    /// The largest percent (inclusive).
    const MAX: Decimal = Decimal::from_parts(1000, 0, 0, false, 0);

    /// Strict parse: an optional `-`, digits with an optional `.` and 1 to 4
    /// decimals, above -100 and at most 1000.
    ///
    /// # Errors
    /// `InvalidValue` for anything else.
    pub fn parse(s: &str) -> Result<SignedPercent, LocalValidation> {
        let body = s.strip_prefix('-').unwrap_or(s);
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if int.is_empty()
            || int.len() > 4
            || !digits(int)
            || !digits(frac)
            || frac.len() > 4
            || (body.contains('.') && frac.is_empty())
        {
            return Err(LocalValidation::InvalidValue);
        }
        let d = Decimal::from_str(s).map_err(|_| LocalValidation::InvalidValue)?;
        if d <= -Decimal::ONE_HUNDRED || d > Self::MAX {
            return Err(LocalValidation::InvalidValue);
        }
        Ok(SignedPercent(d.normalize()))
    }

    /// The wire text, trailing zeros removed (`-7.5`, `10`).
    #[must_use]
    pub fn to_wire(self) -> String {
        self.0.normalize().to_string()
    }

    /// The exact value.
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        self.0
    }

    /// Below zero (a price level that lowers prices).
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.0.is_sign_negative() && !self.0.is_zero()
    }
}

/// A percent alone is not owner data.
impl fmt::Debug for SignedPercent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SignedPercent({})", self.to_wire())
    }
}

impl Serialize for SignedPercent {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

/// A JSON string only; `numeric(…,4)` text with trailing zeros is fine.
impl<'de> Deserialize<'de> for SignedPercent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let trimmed = if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.')
        } else {
            s.as_str()
        };
        SignedPercent::parse(trimmed).map_err(|_| de::Error::custom("percent"))
    }
}

uuid_id!(
    /// One price level (U-40).
    PriceLevelId
);

uuid_id!(
    /// One message waiting in the Outbox (U-127).
    OutboxId
);

uuid_id!(
    /// One invoice print layout (U-130).
    InvoiceLayoutId
);

// ---- The saved invoice -----------------------------------------------------------

/// An invoice's payment and send state (`invoiceStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    /// Saved, never sent.
    Draft,
    /// Sent (printed, emailed or marked).
    Sent,
    /// Opened through a share link.
    Viewed,
    /// Partly paid.
    Partial,
    /// Paid in full.
    Paid,
    /// Void.
    Void,
    /// A state this version does not know.
    #[serde(other)]
    Other,
}

impl InvoiceStatus {
    /// The wire value (list filters); `None` for [`InvoiceStatus::Other`].
    #[must_use]
    pub const fn as_wire(self) -> Option<&'static str> {
        Some(match self {
            InvoiceStatus::Draft => "draft",
            InvoiceStatus::Sent => "sent",
            InvoiceStatus::Viewed => "viewed",
            InvoiceStatus::Partial => "partial",
            InvoiceStatus::Paid => "paid",
            InvoiceStatus::Void => "void",
            InvoiceStatus::Other => return None,
        })
    }
}

/// How an invoice went out (U-126, Q-P20e).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SendMethod {
    /// Printed (Print, or Mark as printed in the print queue).
    Printed,
    /// Emailed.
    Email,
    /// Saved as a PDF with Mark as sent ticked.
    Pdf,
    /// A method this version does not know.
    #[serde(other)]
    Other,
}

/// One entry of an invoice's send history.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendEvent {
    /// How.
    pub method: SendMethod,
    /// Email addresses (email only; owner data, never logged).
    #[serde(default)]
    pub recipients: Vec<String>,
    /// When.
    #[serde(default)]
    pub sent_at: Option<Timestamp>,
}

impl fmt::Debug for SendEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendEvent")
            .field("method", &self.method)
            .field("recipients", &self.recipients.len())
            .field("sent_at", &self.sent_at)
            .finish()
    }
}

/// One document line of a saved invoice (the income side; the A/R and
/// sales tax lines are the engine's).
#[derive(Clone, PartialEq, Eq)]
pub struct InvoiceRecordLine {
    /// Income account.
    pub account: AccountId,
    /// Item.
    pub item: Option<ItemId>,
    /// Description (owner data).
    pub description: Option<String>,
    /// Service date.
    pub service_date: Option<CalendarDate>,
    /// Quantity (`None` on a subtotal or discount line).
    pub quantity: Option<Quantity>,
    /// Unit price as stored (`None` on a subtotal or a percent discount).
    pub unit_price: Option<Money>,
    /// A percent discount line's percent as stored (resent on an edit so
    /// the line keeps it even if the item's percent changed).
    pub discount_percent: Option<SignedPercent>,
    /// Where the engine took the price from.
    pub price_source: Option<PriceSource>,
    /// The line amount the engine posted (negative for a discount or a
    /// return line; a billable line's cost and markup together, Q-P20w).
    pub amount: Money,
    /// Taxable.
    pub is_taxable: bool,
    /// The rate the engine applied.
    pub tax_rate: Option<Rate>,
    /// The line's share of the document tax.
    pub tax_amount: Option<Money>,
    /// Line tag.
    pub tag: Option<TagId>,
    /// The billable cost it invoices (U-128; contract, E3).
    pub billable_source_line_id: Option<JournalLineId>,
    /// Markup on that cost (U-128).
    pub markup_percent: Option<SignedPercent>,
    /// A billable line's rate as shown (amount / quantity, 4 places).
    pub display_rate: Option<Money>,
}

impl InvoiceRecordLine {
    /// A subtotal line (posts $0; the engine shows the group's sum).
    #[must_use]
    pub fn is_subtotal(&self) -> bool {
        self.quantity.is_none()
            && self.unit_price.is_none()
            && self.discount_percent.is_none()
            && self.price_source == Some(PriceSource::Unpriced)
    }
}

/// One billable cost an invoice bills (`GET /invoices/{id}`
/// `billableCosts[]`, U-128): the cost and markup journal lines joined
/// again into the one row the form shows (Q-P20w).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceBillableCost {
    /// The source cost line.
    pub source_line_id: JournalLineId,
    /// The bill, check or expense it is on.
    #[serde(default)]
    pub source_transaction_id: Option<TransactionId>,
    /// The cost billed (before markup).
    pub cost_amount: Money,
    /// The markup percent.
    #[serde(default)]
    pub markup_percent: Option<SignedPercent>,
    /// The markup amount.
    #[serde(default)]
    pub markup_amount: Option<Money>,
    /// Cost plus markup: the row's amount.
    pub amount: Money,
    /// The source's quantity (1 when it had none).
    #[serde(default)]
    pub quantity: Option<Quantity>,
    /// amount / quantity, 4 places.
    #[serde(default)]
    pub display_rate: Option<Money>,
    /// The income account the cost credits.
    #[serde(default)]
    pub income_account_id: Option<AccountId>,
}

impl fmt::Debug for InvoiceBillableCost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceBillableCost")
            .field("source_line_id", &self.source_line_id)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for InvoiceRecordLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceRecordLine")
            .field("account", &self.account)
            .field("item", &self.item)
            .field("service_date", &self.service_date)
            .field("is_taxable", &self.is_taxable)
            .field("tag", &self.tag)
            .finish_non_exhaustive()
    }
}

/// A saved invoice (`GET /invoices/{id}` and the save answers).
#[derive(Clone, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags the engine reports on the invoice"
)]
pub struct InvoiceRecord {
    /// The ledger record (its lines include A/R and sales tax).
    pub txn: Transaction,
    /// The document lines, in order.
    pub lines: Vec<InvoiceRecordLine>,
    /// Sum of the line amounts.
    pub subtotal: Option<Money>,
    /// The document tax.
    pub tax_amount: Option<Money>,
    /// Still owed.
    pub balance_due: Option<Money>,
    /// Paid so far.
    pub amount_paid: Option<Money>,
    /// Payment and send state.
    pub invoice_status: Option<InvoiceStatus>,
    /// Due date.
    pub due_date: Option<CalendarDate>,
    /// The owner typed the due date (it no longer follows the terms).
    pub due_date_overridden: bool,
    /// Terms code.
    pub payment_terms: Option<String>,
    /// Days of custom terms.
    pub terms_days: Option<u32>,
    /// The client's P.O. number.
    pub po_number: Option<String>,
    /// The printed message (owner data).
    pub customer_message: Option<String>,
    /// Leave $0 lines off the printout.
    pub hide_zero_lines: bool,
    /// Header tags.
    pub tag_ids: Vec<TagId>,
    /// First sent.
    pub sent_at: Option<Timestamp>,
    /// Every send, oldest first (kept across edits).
    pub send_history: Vec<SendEvent>,
    /// Waiting to be printed.
    pub print_later: bool,
    /// The billable costs it bills (U-128).
    pub billable_costs: Vec<InvoiceBillableCost>,
    /// The print layout (`None`: the company default; U-130).
    pub layout_id: Option<InvoiceLayoutId>,
    /// A finance-charge invoice (U-141): it keeps its client (U-145).
    pub is_finance_charge: bool,
}

impl InvoiceRecord {
    /// The billable source lines this invoice bills (sent back on an edit
    /// or released, U-128).
    #[must_use]
    pub fn billable_sources(&self) -> Vec<JournalLineId> {
        let mut v: Vec<JournalLineId> = self
            .lines
            .iter()
            .filter_map(|l| l.billable_source_line_id)
            .chain(self.billable_costs.iter().map(|c| c.source_line_id))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// True once payments or credits are applied (void is refused).
    #[must_use]
    pub fn has_payments(&self) -> bool {
        matches!(
            self.invoice_status,
            Some(InvoiceStatus::Partial | InvoiceStatus::Paid)
        ) || self.amount_paid.is_some_and(Money::is_positive)
    }

    /// True once it was printed, emailed or marked sent (Q-P20f banner).
    #[must_use]
    pub fn was_sent(&self) -> bool {
        !self.send_history.is_empty() || self.sent_at.is_some()
    }

    /// True when void.
    #[must_use]
    pub fn is_void(&self) -> bool {
        self.txn.status == TxnStatus::Void
    }
}

impl fmt::Debug for InvoiceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceRecord")
            .field("id", &self.txn.id)
            .field("date", &self.txn.txn_date)
            .field("status", &self.txn.status)
            .field("invoice_status", &self.invoice_status)
            .field("due_date", &self.due_date)
            .field("lines", &self.lines.len())
            .field("send_history", &self.send_history.len())
            .finish_non_exhaustive()
    }
}

fn null_false<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the engine's flags as sent"
)]
struct RecordWire {
    #[serde(default)]
    subtotal: Option<Money>,
    #[serde(default)]
    tax_amount: Option<Money>,
    #[serde(default)]
    balance_due: Option<Money>,
    #[serde(default)]
    amount_paid: Option<Money>,
    #[serde(default)]
    invoice_status: Option<InvoiceStatus>,
    #[serde(default)]
    due_date: Option<CalendarDate>,
    #[serde(default, deserialize_with = "null_false")]
    due_date_overridden: bool,
    #[serde(default)]
    payment_terms: Option<String>,
    #[serde(default)]
    terms_days: Option<u32>,
    #[serde(default)]
    po_number: Option<String>,
    #[serde(default)]
    customer_message: Option<String>,
    #[serde(default, deserialize_with = "null_false")]
    hide_zero_lines: bool,
    #[serde(default)]
    tag_ids: Option<Vec<TagId>>,
    #[serde(default)]
    sent_at: Option<Timestamp>,
    #[serde(default)]
    send_history: Option<Vec<SendEvent>>,
    #[serde(default, deserialize_with = "null_false")]
    print_later: bool,
    #[serde(default)]
    lines: Vec<LineWire>,
    #[serde(default)]
    billable_costs: Option<Vec<InvoiceBillableCost>>,
    #[serde(default)]
    layout_id: Option<InvoiceLayoutId>,
    #[serde(default, deserialize_with = "null_false")]
    is_finance_charge: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineWire {
    #[serde(default)]
    item_id: Option<ItemId>,
    #[serde(default)]
    quantity: Option<Quantity>,
    #[serde(default)]
    unit_price: Option<Money>,
    #[serde(default)]
    discount_percent: Option<SignedPercent>,
    #[serde(default)]
    unit_price_source: Option<PriceSource>,
    #[serde(default)]
    line_role: Option<String>,
    #[serde(default, deserialize_with = "null_false")]
    is_taxable: bool,
    #[serde(default)]
    tax_rate: Option<Rate>,
    #[serde(default)]
    tax_amount: Option<Money>,
    #[serde(default)]
    service_date: Option<CalendarDate>,
    #[serde(default)]
    billable_source_line_id: Option<JournalLineId>,
    #[serde(default)]
    markup_percent: Option<SignedPercent>,
}

/// Reads the object as the [`Transaction`] and again for the invoice
/// fields. The document lines are the ones the engine marks as such (a
/// price source, a line role, an item or a quantity; its A/R and sales tax
/// lines carry none): subtotal and discount lines have no quantity and are
/// kept. A billable cost's markup line (`lineRole: billable_markup`) is
/// merged into its cost line from `billableCosts[]` (Q-P20w). Amounts are
/// credit less debit, so a discount line reads negative. A record that is
/// not an invoice is refused.
impl<'de> Deserialize<'de> for InvoiceRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let txn = Transaction::deserialize(&v).map_err(|_| de::Error::custom("transaction"))?;
        if txn.txn_type != TxnType::Invoice {
            return Err(de::Error::custom("not an invoice"));
        }
        let w = RecordWire::deserialize(&v).map_err(|_| de::Error::custom("invoice fields"))?;
        if w.lines.len() != txn.lines.len() {
            return Err(de::Error::custom("lines"));
        }
        let mut pairs: Vec<_> = txn.lines.iter().zip(w.lines).collect();
        pairs.sort_by_key(|(l, _)| l.line_order.unwrap_or(0));
        let costs = w.billable_costs.clone().unwrap_or_default();
        let mut lines = Vec::new();
        for (l, x) in pairs {
            let document = x.unit_price_source.is_some()
                || x.line_role.is_some()
                || x.item_id.is_some()
                || x.quantity.is_some();
            if !document || x.line_role.as_deref() == Some("billable_markup") {
                continue;
            }
            let mut amount = l
                .credit
                .checked_sub(l.debit)
                .map_err(|_| de::Error::custom("line amount"))?;
            let mut quantity = x.quantity;
            let mut markup_percent = x.markup_percent;
            let mut display_rate = None;
            if let Some(src) = x.billable_source_line_id
                && let Some(c) = costs.iter().find(|c| c.source_line_id == src)
            {
                amount = c.amount;
                quantity = c.quantity.or(quantity);
                markup_percent = c.markup_percent.or(markup_percent);
                display_rate = c.display_rate;
            }
            lines.push(InvoiceRecordLine {
                account: l.account_id,
                item: x.item_id,
                description: l.description.clone(),
                service_date: x.service_date,
                quantity,
                unit_price: x.unit_price,
                discount_percent: x.discount_percent,
                price_source: x.unit_price_source,
                amount,
                is_taxable: x.is_taxable,
                tax_rate: x.tax_rate,
                tax_amount: x.tax_amount,
                tag: l.tag_id,
                billable_source_line_id: x.billable_source_line_id,
                markup_percent,
                display_rate,
            });
        }
        Ok(InvoiceRecord {
            txn,
            lines,
            subtotal: w.subtotal,
            tax_amount: w.tax_amount,
            balance_due: w.balance_due,
            amount_paid: w.amount_paid,
            invoice_status: w.invoice_status,
            due_date: w.due_date,
            due_date_overridden: w.due_date_overridden,
            payment_terms: w.payment_terms,
            terms_days: w.terms_days,
            po_number: w.po_number,
            customer_message: w.customer_message,
            hide_zero_lines: w.hide_zero_lines,
            tag_ids: w.tag_ids.unwrap_or_default(),
            sent_at: w.sent_at,
            send_history: w.send_history.unwrap_or_default(),
            print_later: w.print_later,
            billable_costs: costs,
            layout_id: w.layout_id,
            is_finance_charge: w.is_finance_charge,
        })
    }
}

impl EditableFields for InvoiceRecord {
    const EDITABLE: &'static [&'static str] = &[
        "txnDate",
        "status",
        "contactId",
        "txnNumber",
        "memo",
        "total",
        "dueDate",
        "paymentTerms",
        "termsDays",
        "poNumber",
        "customerMessage",
        "hideZeroLines",
        "tagIds",
        "lines.accountId",
        "lines.itemId",
        "lines.description",
        "lines.serviceDate",
        "lines.quantity",
        "lines.unitPrice",
        "lines.discountPercent",
        "lines.amount",
        "lines.isTaxable",
        "lines.tagId",
        "lines.billableSourceLineId",
        "lines.markupPercent",
    ];

    fn editable(&self) -> Value {
        let lines: Vec<Value> = self
            .lines
            .iter()
            .map(|l| {
                json!({
                    "accountId": l.account,
                    "itemId": l.item,
                    "description": l.description.as_deref().filter(|d| !d.is_empty()),
                    "serviceDate": l.service_date,
                    "quantity": l.quantity.map(Quantity::to_wire),
                    "unitPrice": l.unit_price.map(Money::to_wire),
                    "discountPercent": l.discount_percent,
                    "amount": l.amount.to_wire(),
                    "isTaxable": l.is_taxable,
                    "tagId": l.tag,
                    "billableSourceLineId": l.billable_source_line_id,
                    "markupPercent": l.markup_percent,
                })
            })
            .collect();
        let mut tags = self.tag_ids.clone();
        tags.sort();
        json!({
            "txnDate": self.txn.txn_date,
            "status": self.txn.status,
            "contactId": self.txn.contact_id,
            "txnNumber": self.txn.txn_number,
            "memo": self.txn.memo.as_deref().filter(|m| !m.is_empty()),
            "total": self.txn.total.map(Money::to_wire),
            "dueDate": self.due_date,
            "paymentTerms": self.payment_terms,
            "termsDays": self.terms_days,
            "poNumber": self.po_number,
            "customerMessage": self.customer_message,
            "hideZeroLines": self.hide_zero_lines,
            "tagIds": tags,
            "lines": lines,
        })
    }
}

/// What a create or edit answered: the invoice and the duplicate-number
/// warnings of a `warn_only` save (none on an idempotent replay).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedInvoice {
    /// The invoice as committed (its `updatedAt` is the next
    /// `expectedUpdatedAt`).
    pub invoice: Versioned<InvoiceRecord>,
    /// `DOCUMENT_NUMBER_IN_USE` warnings (Q-94).
    pub warnings: Vec<DocumentNumberWarning>,
}

// ---- The list ----------------------------------------------------------------

/// Filters of `GET /invoices`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InvoiceListQuery {
    /// One client.
    pub contact: Option<ContactId>,
    /// Keep these states.
    pub statuses: Vec<InvoiceStatus>,
    /// `Some(false)`: never sent, whatever the payment state (the Outbox's
    /// Not sent yet tab, Q-P20s; `invoice-sent-history-v1`).
    pub sent: Option<bool>,
    /// Dated on or after.
    pub start: Option<CalendarDate>,
    /// Dated on or before.
    pub end: Option<CalendarDate>,
    /// Number, memo or name contains (owner data: never logged).
    pub search: Option<String>,
}

impl InvoiceListQuery {
    fn pairs(&self) -> Vec<(String, String)> {
        let mut q = Query::default();
        if let Some(c) = self.contact {
            q.put("contactId", c.to_string());
        }
        let s: Vec<&str> = self.statuses.iter().filter_map(|s| s.as_wire()).collect();
        if !s.is_empty() {
            q.put("invoiceStatus", s.join(","));
        }
        q.opt_bool("sent", self.sent);
        if let Some(d) = self.start {
            q.put("startDate", d.to_wire());
        }
        if let Some(d) = self.end {
            q.put("endDate", d.to_wire());
        }
        q.opt_str("search", self.search.as_deref());
        q.0
    }
}

/// One row of the invoice list.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoiceSummary {
    /// The invoice.
    pub id: TransactionId,
    /// Its number.
    #[serde(default)]
    pub txn_number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// The client.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The client's name (joined; owner data).
    #[serde(default)]
    pub contact_name: Option<String>,
    /// Total.
    #[serde(default)]
    pub total: Option<Money>,
    /// Still owed.
    #[serde(default)]
    pub balance_due: Option<Money>,
    /// Payment and send state.
    #[serde(default)]
    pub invoice_status: Option<InvoiceStatus>,
    /// Posted or void.
    pub status: TxnStatus,
}

impl fmt::Debug for InvoiceSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceSummary")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("invoice_status", &self.invoice_status)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

// ---- Preview (U-129, E2) -----------------------------------------------------------

/// Where a line's unit price came from (`unitPriceSource`, U-129; the
/// engine's `invoice-compute.ts` names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    /// The owner typed it.
    Input,
    /// The item's price.
    Item,
    /// The price the client's level sets for this item.
    PriceLevel,
    /// The client's level's % of the item's price.
    PriceLevelPercent,
    /// No price (a subtotal line).
    #[serde(rename = "none")]
    Unpriced,
    /// A billable cost (with any markup).
    BillableCost,
    /// A source this version does not know.
    #[serde(other)]
    Other,
}

/// A previewed line the engine could not bill as asked (U-128): another
/// invoice holds the cost.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLineError {
    /// `BILLABLE_ALREADY_BILLED`.
    pub code: String,
    /// The invoice that bills it.
    #[serde(default)]
    pub invoice_id: Option<TransactionId>,
    /// Its number (display).
    #[serde(default)]
    pub invoice_number: Option<String>,
}

impl fmt::Debug for PreviewLineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewLineError")
            .field("invoice_id", &self.invoice_id)
            .finish_non_exhaustive()
    }
}

/// One previewed line.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewLine {
    /// The quantity it used (`None` on subtotal and discount lines).
    #[serde(default)]
    pub quantity: Option<Quantity>,
    /// The unit price the engine used.
    #[serde(default)]
    pub unit_price: Option<Money>,
    /// Where it came from.
    #[serde(default)]
    pub unit_price_source: Option<PriceSource>,
    /// A discount line's percent.
    #[serde(default)]
    pub discount_percent: Option<SignedPercent>,
    /// The line amount.
    pub amount: Money,
    /// The line's share of the tax.
    #[serde(default)]
    pub tax_amount: Option<Money>,
    /// A billable line's cost (U-128).
    #[serde(default)]
    pub cost_amount: Option<Money>,
    /// A billable line's markup percent.
    #[serde(default)]
    pub markup_percent: Option<SignedPercent>,
    /// A billable line's markup amount.
    #[serde(default)]
    pub markup_amount: Option<Money>,
    /// A billable line's rate as shown (amount / quantity, 4 places).
    #[serde(default)]
    pub display_rate: Option<Money>,
    /// Another invoice holds this line's cost.
    #[serde(default)]
    pub error: Option<PreviewLineError>,
}

impl fmt::Debug for PreviewLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewLine")
            .field("unit_price_source", &self.unit_price_source)
            .finish_non_exhaustive()
    }
}

/// Every figure of an invoice as the engine would post it (`POST
/// /invoices/preview`, U-129; contract per P20 plan §2, verify when E2
/// lands). The form shows these strings; it never adds them up.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InvoicePreview {
    /// One per line sent, in order.
    pub lines: Vec<PreviewLine>,
    /// Sum of the line amounts.
    pub subtotal: Money,
    /// Signed sum of the taxable line amounts.
    #[serde(default)]
    pub taxable_base: Option<Money>,
    /// The document's tax rate.
    #[serde(default)]
    pub tax_rate: Option<Rate>,
    /// The document tax.
    pub tax_total: Money,
    /// Total.
    pub total: Money,
    /// Due date from the terms (or the typed one).
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// The client's open balance once this invoice posts.
    #[serde(default)]
    pub contact_balance_after: Option<Money>,
}

impl fmt::Debug for InvoicePreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoicePreview")
            .field("lines", &self.lines.len())
            .field("due_date", &self.due_date)
            .finish_non_exhaustive()
    }
}

// ---- Send history, email and the Outbox ---------------------------------------------

/// What `POST /invoices/{id}/sent` records (U-126).
#[derive(Clone, PartialEq, Eq)]
pub struct SendRecord {
    /// How it went out.
    pub method: SendMethod,
    /// Email addresses (email only; at most 50).
    pub recipients: Vec<String>,
    /// When (`None`: now).
    pub sent_at: Option<Timestamp>,
}

impl fmt::Debug for SendRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SendRecord")
            .field("method", &self.method)
            .field("recipients", &self.recipients.len())
            .finish_non_exhaustive()
    }
}

/// An email of an invoice (U-127; contract per P20 plan §2, verify when E4
/// lands). Owner data throughout: `Debug` shows counts only.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct EmailMessage {
    /// To (at least one).
    pub to: Vec<String>,
    /// Cc.
    pub cc: Vec<String>,
    /// Bcc (the From address when BCC-self is on, Q-P20j).
    pub bcc: Vec<String>,
    /// Subject.
    pub subject: String,
    /// Body text.
    pub body: String,
}

impl fmt::Debug for EmailMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmailMessage")
            .field("to", &self.to.len())
            .field("cc", &self.cc.len())
            .field("bcc", &self.bcc.len())
            .finish_non_exhaustive()
    }
}

impl EmailMessage {
    pub(crate) fn check(&self) -> Result<(), ApiError> {
        let bad = |list: &[String]| {
            list.len() > EMAIL_RECIPIENTS_MAX
                || list
                    .iter()
                    .any(|a| a.trim().is_empty() || !a.contains('@') || a.len() > 254)
        };
        if self.to.is_empty() || bad(&self.to) || bad(&self.cc) || bad(&self.bcc) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(())
    }

    pub(crate) fn json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("to".into(), json!(self.to));
        if !self.cc.is_empty() {
            m.insert("cc".into(), json!(self.cc));
        }
        if !self.bcc.is_empty() {
            m.insert("bcc".into(), json!(self.bcc));
        }
        m.insert("subject".into(), Value::String(self.subject.clone()));
        m.insert("body".into(), Value::String(self.body.clone()));
        m
    }
}

/// The body of an email send (`POST …/{id}/email`, U-127 and U-144): the
/// message, the PDF in base64 with its file name and the Outbox message it
/// sends, if any.
///
/// # Errors
/// `Local(InvalidValue)` for no recipient or a bad address;
/// `Local(AttachmentTooLarge)` for an empty PDF or one over 10 MB.
pub(crate) fn email_body(
    message: &EmailMessage,
    pdf_file_name: &str,
    pdf: &[u8],
    outbox_id: Option<OutboxId>,
) -> Result<Value, ApiError> {
    message.check()?;
    if pdf.is_empty() || pdf.len() > EMAIL_PDF_MAX_BYTES {
        return Err(ApiError::Local(LocalValidation::AttachmentTooLarge));
    }
    let mut m = message.json();
    m.insert(
        "pdfBase64".into(),
        Value::String(base64::engine::general_purpose::STANDARD.encode(pdf)),
    );
    m.insert(
        "pdfFileName".into(),
        Value::String(pdf_file_name.to_string()),
    );
    if let Some(o) = outbox_id {
        m.insert("outboxId".into(), Value::String(o.to_string()));
    }
    Ok(Value::Object(m))
}

/// A message waiting in the Outbox (`GET /forms/email-queue`, U-127;
/// contract per P20 plan §2, verify when E4 lands). The PDF is made when it
/// is sent (Q-P20q).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    /// The queued message.
    pub id: OutboxId,
    /// What it sends (P22, U-140: a transaction or a customer statement;
    /// absent on older engines: a transaction).
    #[serde(default)]
    pub kind: OutboxKind,
    /// The invoice (or, from P22, credit memo or sales receipt) it sends;
    /// `None` for a customer statement.
    #[serde(default)]
    pub transaction_id: Option<TransactionId>,
    /// The customer statement it sends (U-140).
    #[serde(default)]
    pub customer_statement_id: Option<crate::resources::customer_statements::CustomerStatementId>,
    /// Its number (display).
    #[serde(default)]
    pub txn_number: Option<String>,
    /// To.
    #[serde(default)]
    pub to: Vec<String>,
    /// Cc.
    #[serde(default)]
    pub cc: Vec<String>,
    /// Bcc.
    #[serde(default)]
    pub bcc: Vec<String>,
    /// Subject.
    #[serde(default)]
    pub subject: String,
    /// Body.
    #[serde(default)]
    pub body: String,
    /// Queued.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

/// What an Outbox message sends (U-140).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxKind {
    /// An invoice, credit memo or sales receipt.
    #[default]
    Transaction,
    /// A customer statement.
    CustomerStatement,
    /// A credit memo (U-144, `sales-doc-email-v1`): sent with
    /// `POST /credit-memos/{id}/email` and its `outboxId`.
    CreditMemo,
    /// A sales receipt (U-144): sent with `POST /sales-receipts/{id}/email`.
    SalesReceipt,
    /// Anything a newer engine adds.
    #[serde(other)]
    Other,
}

impl OutboxItem {
    /// The message to send.
    #[must_use]
    pub fn message(&self) -> EmailMessage {
        EmailMessage {
            to: self.to.clone(),
            cc: self.cc.clone(),
            bcc: self.bcc.clone(),
            subject: self.subject.clone(),
            body: self.body.clone(),
        }
    }
}

impl fmt::Debug for OutboxItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutboxItem")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("transaction_id", &self.transaction_id)
            .field("customer_statement_id", &self.customer_statement_id)
            .field("created_at", &self.created_at)
            .finish_non_exhaustive()
    }
}

// ---- Billable costs (U-128, E3) -------------------------------------------------------

/// One unbilled cost of a client (`GET /billable-costs` `items[]`, U-128).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillableCost {
    /// The source line (sent back as `billableSourceLineId`).
    pub source_line_id: JournalLineId,
    /// The bill, check or expense it is on.
    pub source_transaction_id: TransactionId,
    /// Its kind (`bill`, `check`, `expense`, …).
    #[serde(default)]
    pub source_txn_type: Option<String>,
    /// Its number or reference.
    #[serde(default)]
    pub source_number: Option<String>,
    /// Its date.
    pub source_date: CalendarDate,
    /// Who it was paid to (display).
    #[serde(default)]
    pub vendor_name: Option<String>,
    /// The line description (owner data).
    #[serde(default)]
    pub description: Option<String>,
    /// The expense account it posted to.
    pub expense_account_id: AccountId,
    /// The item, when it came with one.
    #[serde(default)]
    pub item_id: Option<ItemId>,
    /// Quantity on the source.
    #[serde(default)]
    pub quantity: Option<Quantity>,
    /// Cost per unit on the source.
    #[serde(default)]
    pub unit_cost: Option<Money>,
    /// The source line's amount, 4 places.
    #[serde(default)]
    pub source_amount: Option<Money>,
    /// What the invoice bills before markup (2 places).
    pub cost_amount: Money,
    /// The engine's suggestion for the invoice line's income account (the
    /// cost item's), sent back as `accountId` unless the owner picks one.
    #[serde(default)]
    pub suggested_income_account_id: Option<AccountId>,
}

impl fmt::Debug for BillableCost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillableCost")
            .field("source_line_id", &self.source_line_id)
            .field("source_transaction_id", &self.source_transaction_id)
            .field("source_date", &self.source_date)
            .finish_non_exhaustive()
    }
}

/// A client's unbilled costs (`GET /billable-costs`): one page of
/// `items`, with the count and total over every page (the side panel shows
/// `total_amount` as the engine summed it).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillableCosts {
    /// This page.
    pub items: Vec<BillableCost>,
    /// Costs over every page.
    pub total_count: u64,
    /// Sum of `costAmount` over every page.
    pub total_amount: Money,
    /// The client's suggested markup.
    #[serde(default)]
    pub default_markup_percent: Option<SignedPercent>,
}

impl fmt::Debug for BillableCosts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillableCosts")
            .field("items", &self.items.len())
            .field("total_count", &self.total_count)
            .finish_non_exhaustive()
    }
}
// ---- Price levels (U-40, E2) -----------------------------------------------------------

/// A price set for one item by a price level.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPrice {
    /// The item.
    pub item_id: ItemId,
    /// Its price on this level (the engine's `unitPrice`, E2).
    #[serde(rename = "unitPrice")]
    pub price: Money,
}

impl fmt::Debug for ItemPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ItemPrice")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// A price level (U-40, Q-P20l: a name and a % up or down, plus per-item
/// prices; contract per P20 plan §2, verify when E2 lands).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceLevel {
    /// Id.
    pub id: PriceLevelId,
    /// Name.
    pub name: String,
    /// The % applied to item prices (`-7.5` lowers them by 7.5%).
    pub percent: SignedPercent,
    /// Active.
    #[serde(default = "crate::resources::common::yes")]
    pub is_active: bool,
    /// Per-item prices that win over the %.
    #[serde(default)]
    pub item_prices: Vec<ItemPrice>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl fmt::Debug for PriceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PriceLevel")
            .field("id", &self.id)
            .field("percent", &self.percent)
            .field("is_active", &self.is_active)
            .field("item_prices", &self.item_prices.len())
            .finish_non_exhaustive()
    }
}

/// A price level to create or replace.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceLevelDraft {
    /// Name (1-100 characters).
    pub name: String,
    /// The %.
    pub percent: SignedPercent,
    /// Active.
    pub is_active: bool,
    /// Per-item prices (each item once).
    pub item_prices: Vec<ItemPrice>,
}

impl fmt::Debug for PriceLevelDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PriceLevelDraft")
            .field("percent", &self.percent)
            .field("item_prices", &self.item_prices.len())
            .finish_non_exhaustive()
    }
}

/// The longest price level name.
pub const PRICE_LEVEL_NAME_MAX: usize = 100;

impl PriceLevelDraft {
    fn check(&self) -> Result<(), ApiError> {
        let name = self.name.trim();
        let mut items: Vec<ItemId> = self.item_prices.iter().map(|p| p.item_id).collect();
        items.sort();
        items.dedup();
        if name.is_empty()
            || name.chars().count() > PRICE_LEVEL_NAME_MAX
            || items.len() != self.item_prices.len()
            || self.item_prices.iter().any(|p| p.price.is_negative())
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(())
    }
}

// ---- The client's sales figures (U-38; U-40 and U-128 defaults) -------------------------

/// What the invoice side panel reads about a client: the engine's open
/// balance and oldest open date (`contact-balance-v1`, U-38), and the
/// client's price level (U-40) and default markup (U-128) (both contract per
/// P20 plan §2, verify when E2/E3 land).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientSales {
    /// Open invoices less payments, as the engine computed it.
    pub open_balance: Option<Money>,
    /// Date of the oldest invoice with a balance.
    pub oldest_open_date: Option<CalendarDate>,
    /// The client's price level.
    pub price_level_id: Option<PriceLevelId>,
    /// The client's default markup on billable costs.
    pub default_markup_percent: Option<SignedPercent>,
    /// The layout the client's new invoices start on (U-130).
    pub default_invoice_layout_id: Option<InvoiceLayoutId>,
}

impl fmt::Debug for ClientSales {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientSales")
            .field("oldest_open_date", &self.oldest_open_date)
            .field("price_level_id", &self.price_level_id)
            .field("default_markup_percent", &self.default_markup_percent)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientSalesWire {
    #[serde(default)]
    open_balance: Option<Money>,
    #[serde(default)]
    oldest_open_date: Option<CalendarDate>,
    #[serde(default)]
    price_level_id: Option<PriceLevelId>,
    #[serde(default)]
    default_markup_percent: Option<SignedPercent>,
    #[serde(default)]
    default_invoice_layout_id: Option<InvoiceLayoutId>,
}

// ---- Options -----------------------------------------------------------------------

/// Options of an invoice edit or void.
#[derive(Clone, Debug, Default)]
pub struct InvoiceWriteOptions {
    /// The closing-date password (`invoice-edit-v1`; refused locally
    /// without it).
    pub posting: PostingOptions,
    /// The invoice's `updatedAt` when it was read: sent as
    /// `expectedUpdatedAt` (409 `STALE` when it moved).
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

/// Checks an invoice before anything is sent.
///
/// # Errors
/// `Local(…)`: `TooFewLines`, `TooManyLines`; `Unsupported` for a field
/// the engine does not advertise (an item, a service date or a header field
/// without `invoice-fields-v1`; a line without a price without
/// `invoice-pricing-v1`; a billable line without `billable-costs-v1`);
/// `InvalidValue` for a line with neither account nor item, markup without
/// a billable source, a negative markup, text over its limit or custom days
/// out of range.
pub fn check_invoice(new: &NewInvoice, caps: &Capabilities) -> Result<(), ApiError> {
    let local = |v| Err(ApiError::Local(v));
    if new.lines.is_empty() {
        return local(LocalValidation::TooFewLines);
    }
    if new.lines.len() > MAX_LINES {
        return local(LocalValidation::TooManyLines);
    }
    let fields = caps.has(Capabilities::INVOICE_FIELDS);
    let header_fields = new.customer_message.is_some()
        || new.po_number.is_some()
        || new.tag_ids.is_some()
        || new.hide_zero_lines.is_some();
    let line_fields = new
        .lines
        .iter()
        .any(|l| l.item_id.is_some() || l.service_date.is_some());
    if (header_fields || line_fields) && !fields {
        return local(LocalValidation::Unsupported);
    }
    if new
        .lines
        .iter()
        .any(|l| l.unit_price.is_none() && l.billable_source_line_id.is_none())
        && !caps.has(Capabilities::INVOICE_PRICING)
    {
        return local(LocalValidation::Unsupported);
    }
    if new
        .lines
        .iter()
        .any(|l| l.billable_source_line_id.is_some() || l.markup_percent.is_some())
        && !caps.has(Capabilities::BILLABLE_COSTS)
    {
        return local(LocalValidation::Unsupported);
    }
    for l in &new.lines {
        // A billable cost's item, quantity and price are the source's
        // (400 BILLABLE_FIELDS_READONLY) and its income account is required
        // (400 BILLABLE_INCOME_ACCOUNT_REQUIRED).
        let billable_bad = l.billable_source_line_id.is_some()
            && (l.account_id.is_none()
                || l.item_id.is_some()
                || l.quantity.is_some()
                || l.unit_price.is_some()
                || l.discount_percent.is_some());
        let bad = billable_bad
            || (l.account_id.is_none() && l.item_id.is_none())
            || (l.markup_percent.is_some() && l.billable_source_line_id.is_none())
            || l.markup_percent.is_some_and(SignedPercent::is_negative)
            || l.description
                .as_deref()
                .is_some_and(|d| d.chars().count() > INVOICE_TEXT_MAX);
        if bad {
            return local(LocalValidation::InvalidValue);
        }
    }
    let custom = new.payment_terms.as_deref() == Some(crate::resources::ap_doc::TERMS_CUSTOM);
    let bad_days = match (custom, new.terms_days) {
        (true, Some(d)) => d > INVOICE_TERMS_DAYS_MAX,
        (true, None) | (false, Some(_)) => true,
        (false, None) => false,
    };
    if bad_days
        || new
            .payment_terms
            .as_deref()
            .is_some_and(|t| t.trim().is_empty())
        || new
            .customer_message
            .as_deref()
            .is_some_and(|m| m.chars().count() > INVOICE_TEXT_MAX)
        || new
            .po_number
            .as_deref()
            .is_some_and(|p| p.trim().chars().count() > PO_NUMBER_MAX)
        || new
            .tag_ids
            .as_ref()
            .is_some_and(|t| t.len() > INVOICE_TAGS_MAX)
    {
        return local(LocalValidation::InvalidValue);
    }
    Ok(())
}

/// The body of a create, edit or preview: `new`, the stale guard and the
/// closing-date password (which needs `invoice-edit-v1`).
fn invoice_body(
    s: &Session,
    new: &NewInvoice,
    opts: &PostingOptions,
    expected_updated_at: Option<Timestamp>,
) -> Result<Vec<u8>, ApiError> {
    let caps = s.client().capabilities();
    if opts.closed_period.is_some() && !caps.has(Capabilities::INVOICE_EDIT) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    let mut body = serde_json::to_value(new).map_err(|_| ApiError::Decode { what: "invoice" })?;
    if let (Some(t), Value::Object(map)) = (expected_updated_at, &mut body) {
        map.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
    }
    posting_body(s, body, opts)
}

fn saved_invoice(body: &[u8]) -> Result<SavedInvoice, ApiError> {
    let invoice = saved_from(body, "invoice")?;
    let warnings = serde_json::from_slice::<Value>(body)
        .map(|v| number_warnings(&v))
        .unwrap_or_default();
    Ok(SavedInvoice { invoice, warnings })
}

/// Invoice operations of a session (`session.invoices()`).
#[derive(Clone, Copy, Debug)]
pub struct Invoices<'a>(pub(crate) &'a Session);

impl Session {
    /// Invoices of the open company.
    #[must_use]
    pub fn invoices(&self) -> Invoices<'_> {
        Invoices(self)
    }
}

impl Invoices<'_> {
    /// One invoice with its lines, fields, send history and version.
    ///
    /// # Errors
    /// `NotFound` (also an invoice of another company); `Validation` when
    /// it is not an invoice; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<Versioned<InvoiceRecord>, ApiError> {
        get_versioned(self.0, &INVOICE_GET, format!("{BASE}/{id}"), "invoice").await
    }

    /// One page of invoices. `sent` needs `invoice-sent-history-v1`.
    ///
    /// # Errors
    /// `Local(Unsupported)` for `sent` without the capability (nothing
    /// sent); `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn list(
        &self,
        q: &InvoiceListQuery,
        page: PageRequest,
    ) -> Result<Page<InvoiceSummary>, ApiError> {
        if q.sent.is_some() {
            require(self.0, Capabilities::INVOICE_SENT_HISTORY)?;
        }
        let mut call = Call::at(Method::Get, INVOICES_LIST.route());
        call.query = q.pairs();
        call.query.extend(page.query());
        let reply = self.0.send(&INVOICES_LIST, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// Creates an invoice (`POST /invoices`, keyed under `idempotency-v1`).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)` (see [`check_invoice`]; a closing-date
    /// password without `invoice-edit-v1` is `Unsupported`). From the
    /// engine: `ClosedPeriod`, `ClosingPasswordRejected`, `Conflict`
    /// (`DOCUMENT_NUMBER_IN_USE`), `Validation`, `SavedUnreadable`, any
    /// [`ApiError`].
    pub async fn create(
        &self,
        new: &NewInvoice,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<SavedInvoice, ApiError> {
        let s = &self.0.pinned();
        check_invoice(new, &s.client().capabilities())?;
        let body = invoice_body(s, new, opts, None)?;
        let mut call = Call::at(Method::Post, INVOICES_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVOICES_CREATE, call, None).await?;
        remember_created(s, &reply.body, "invoice");
        saved_invoice(&reply.body)
    }

    /// Changes an invoice if it is still as `expected` was read: its
    /// `updatedAt` goes out as `expectedUpdatedAt` (`invoice-edit-v1`; 409
    /// `STALE` when it moved, nothing changes). Keyed under
    /// `idempotency-invoice-v1`. Omitted header fields keep their values.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `invoice-edit-v1` (nothing sent);
    /// otherwise as [`Invoices::create`], plus `Conflict { Stale }` and
    /// `Validation` for a void or paid invoice.
    pub async fn update(
        &self,
        id: TransactionId,
        expected: &InvoiceRecord,
        new: &NewInvoice,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<SavedInvoice, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVOICE_EDIT)?;
        check_invoice(new, &s.client().capabilities())?;
        let mut body: Value =
            serde_json::from_slice(&invoice_body(s, new, opts, expected.txn.updated_at)?)
                .map_err(|_| ApiError::Decode { what: "invoice" })?;
        // The desktop always sends the whole line list: a discount or
        // subtotal line it leaves out was removed by the owner.
        if s.client()
            .capabilities()
            .has(Capabilities::INVOICE_PRICING_LINES)
            && let Value::Object(m) = &mut body
        {
            m.insert("pricingLinesComplete".into(), Value::Bool(true));
        }
        let mut call = Call::new(Method::Put, INVOICE_PUT.route(), format!("{BASE}/{id}"))
            .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVOICE_PUT, call, None).await?;
        saved_invoice(&reply.body)
    }

    /// Voids an invoice (`POST /invoices/{id}/void`). A reason is required.
    ///
    /// # Errors
    /// `Local(MissingReason)`; `Local(Unsupported)` for a closing-date
    /// password or a stale guard without `invoice-edit-v1` (nothing sent).
    /// From the engine: `Conflict` (`INVOICE_HAS_PAYMENTS`, `STALE`; see
    /// [`invoice_refusal`]), `ClosedPeriod`, `ClosingPasswordRejected`, any
    /// [`ApiError`].
    pub async fn void(
        &self,
        id: TransactionId,
        reason: &str,
        opts: &InvoiceWriteOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<InvoiceRecord>, ApiError> {
        let s = &self.0.pinned();
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(ApiError::Local(LocalValidation::MissingReason));
        }
        let caps = s.client().capabilities();
        if (opts.posting.closed_period.is_some() || opts.expected_updated_at.is_some())
            && !caps.has(Capabilities::INVOICE_EDIT)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let mut body = json!({ "reason": reason });
        if let (Some(t), Value::Object(map)) = (opts.expected_updated_at, &mut body) {
            map.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
        }
        let body = posting_body(s, body, &opts.posting)?;
        let mut call = Call::new(
            Method::Post,
            INVOICE_VOID.route(),
            format!("{BASE}/{id}/void"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVOICE_VOID, call, None).await?;
        saved_from(&reply.body, "invoice")
    }

    /// Records that the invoice was printed, emailed or saved as a PDF
    /// (`invoice-sent-history-v1`). Keyed under `idempotency-invoice-v1`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability; `Local(InvalidValue)`
    /// for recipients on a non-email send or more than 50 (nothing sent).
    /// From the engine: `Validation` (`INVOICE_VOID`), `NotFound`, any
    /// [`ApiError`].
    pub async fn record_sent(
        &self,
        id: TransactionId,
        sent: &SendRecord,
        key: IdempotencyKey,
    ) -> Result<Versioned<InvoiceRecord>, ApiError> {
        require(self.0, Capabilities::INVOICE_SENT_HISTORY)?;
        if sent.method == SendMethod::Other
            || (sent.method != SendMethod::Email && !sent.recipients.is_empty())
            || sent.recipients.len() > EMAIL_RECIPIENTS_MAX
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut m = Map::new();
        m.insert("method".into(), json!(sent.method));
        if !sent.recipients.is_empty() {
            m.insert("recipients".into(), json!(sent.recipients));
        }
        if let Some(t) = sent.sent_at {
            m.insert("sentAt".into(), Value::String(t.to_wire()));
        }
        let mut call = Call::new(
            Method::Post,
            INVOICE_SENT.route(),
            format!("{BASE}/{id}/sent"),
        )
        .json_body(json_body(&Value::Object(m))?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&INVOICE_SENT, call, None).await?;
        saved_from(&reply.body, "invoice")
    }

    /// Every figure of `new` as the engine would post it, posting nothing
    /// (`POST /invoices/preview`, `invoice-pricing-v1`; contract per P20
    /// plan §2, verify when E2 lands). Send `expected` on an edit so the
    /// client's balance after leaves the invoice's own amount out.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `invoice-pricing-v1`; the local checks
    /// of [`check_invoice`]; `Validation`; `Decode`; any [`ApiError`].
    pub async fn preview(
        &self,
        new: &NewInvoice,
        editing: Option<TransactionId>,
    ) -> Result<InvoicePreview, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVOICE_PRICING)?;
        check_invoice(new, &s.client().capabilities())?;
        let mut body =
            serde_json::to_value(new).map_err(|_| ApiError::Decode { what: "invoice" })?;
        if let (Some(id), Value::Object(map)) = (editing, &mut body) {
            map.insert("invoiceId".into(), Value::String(id.to_string()));
        }
        let call = Call::at(Method::Post, INVOICE_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = s.send(&INVOICE_PREVIEW, call, None).await?;
        let v: Value = decode(&reply, "invoice preview")?;
        let inner = v.get("preview").cloned().unwrap_or(v);
        let p: InvoicePreview = serde_json::from_value(inner).map_err(|_| ApiError::Decode {
            what: "invoice preview",
        })?;
        if p.lines.len() != new.lines.len() {
            return Err(ApiError::Decode {
                what: "invoice preview",
            });
        }
        Ok(p)
    }

    /// Emails the invoice through the company SMTP with `pdf` attached
    /// (`Invoice-<number>.pdf`) and records an "email" send (`POST
    /// /invoices/{id}/email`, `invoice-email-v1`). Keyed under that
    /// capability: the same key on a retry sends once, so keep one key per
    /// send and reuse it. `outbox_id` (Send all) takes the Outbox message
    /// out in the same commit (a second Send all gets 404
    /// `OUTBOX_ITEM_NOT_FOUND`, never a second email).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability; `Local(InvalidValue)`
    /// for no recipient or a bad address; `Local(AttachmentTooLarge)` for a
    /// PDF over 10 MB (nothing sent). From the engine: `SMTP_NOT_CONFIGURED`,
    /// `SMTP_SEND_FAILED` (see [`invoice_refusal`]), any [`ApiError`].
    pub async fn email(
        &self,
        id: TransactionId,
        message: &EmailMessage,
        pdf_file_name: &str,
        pdf: &[u8],
        outbox_id: Option<OutboxId>,
        key: IdempotencyKey,
    ) -> Result<Versioned<InvoiceRecord>, ApiError> {
        require(self.0, Capabilities::INVOICE_EMAIL)?;
        let body = email_body(message, pdf_file_name, pdf, outbox_id)?;
        let mut call = Call::new(
            Method::Post,
            INVOICE_EMAIL.route(),
            format!("{BASE}/{id}/email"),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&INVOICE_EMAIL, call, None).await?;
        saved_from(&reply.body, "invoice")
    }

    /// Puts the message in the Outbox, stored with the company in the
    /// engine (Q-P20q); it goes when the owner clicks Send all (`POST
    /// /invoices/{id}/email-later`; keyed under `invoice-email-v1`: reuse
    /// the key on a retry).
    ///
    /// # Errors
    /// As [`Invoices::email`] without the PDF checks.
    pub async fn email_later(
        &self,
        id: TransactionId,
        message: &EmailMessage,
        key: IdempotencyKey,
    ) -> Result<OutboxItem, ApiError> {
        #[derive(Deserialize)]
        struct W {
            item: OutboxItem,
        }
        require(self.0, Capabilities::INVOICE_EMAIL)?;
        message.check()?;
        let mut call = Call::new(
            Method::Post,
            INVOICE_EMAIL_LATER.route(),
            format!("{BASE}/{id}/email-later"),
        )
        .json_body(json_body(&Value::Object(message.json()))?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&INVOICE_EMAIL_LATER, call, None).await?;
        decode::<W>(&reply, "outbox item")
            .map(|w| w.item)
            .map_err(saved_unreadable)
    }

    /// The Outbox, oldest first (contract per P20 plan §2, verify when E4
    /// lands).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `invoice-email-v1`; any [`ApiError`].
    pub async fn outbox(&self) -> Result<Vec<OutboxItem>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<OutboxItem>,
        }
        require(self.0, Capabilities::INVOICE_EMAIL)?;
        let call = Call::at(Method::Get, EMAIL_QUEUE.route());
        let reply = self.0.send(&EMAIL_QUEUE, call, None).await?;
        decode::<W>(&reply, "outbox").map(|w| w.data)
    }

    /// Takes a message out of the Outbox (the owner removed it). Already
    /// gone (404 `OUTBOX_ITEM_NOT_FOUND`: sent or removed meanwhile) is
    /// success.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `invoice-email-v1`; any other
    /// [`ApiError`].
    pub async fn remove_from_outbox(&self, id: OutboxId) -> Result<(), ApiError> {
        require(self.0, Capabilities::INVOICE_EMAIL)?;
        let call = Call::new(
            Method::Delete,
            EMAIL_QUEUE_DELETE.route(),
            format!("/api/v1/forms/email-queue/{id}"),
        );
        match self.0.send(&EMAIL_QUEUE_DELETE, call, None).await {
            Ok(_) | Err(ApiError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// A client's unbilled costs, one page (`GET /billable-costs`,
    /// `billable-costs-v1`; contract per P20 plan §2, verify when E3 lands).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability; `Decode`; any
    /// [`ApiError`].
    pub async fn billable_costs(
        &self,
        client: ContactId,
        page: PageRequest,
    ) -> Result<BillableCosts, ApiError> {
        require(self.0, Capabilities::BILLABLE_COSTS)?;
        let mut call = Call::at(Method::Get, BILLABLE_COSTS.route());
        call.query = vec![("contactId".into(), client.to_string())];
        call.query.extend(page.query());
        let reply = self.0.send(&BILLABLE_COSTS, call, None).await?;
        decode::<BillableCosts>(&reply, "billable costs")
    }

    /// Every unbilled cost of a client, page by page until
    /// `totalCount` is read.
    ///
    /// # Errors
    /// As [`Invoices::billable_costs`].
    pub async fn all_billable_costs(&self, client: ContactId) -> Result<BillableCosts, ApiError> {
        let mut first = self
            .billable_costs(client, PageRequest::first_max())
            .await?;
        while (first.items.len() as u64) < first.total_count {
            let page = PageRequest::new(PageRequest::MAX_LIMIT, first.items.len() as u64)?;
            let next = self.billable_costs(client, page).await?;
            if next.items.is_empty() {
                break;
            }
            first.items.extend(next.items);
        }
        Ok(first)
    }

    /// The fixed payment terms (`invoice-terms-v1` or `bill-terms-v1`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without either capability; any [`ApiError`].
    pub async fn payment_terms(&self) -> Result<Vec<PaymentTerm>, ApiError> {
        const WHAT: &str = "payment terms";
        let caps = self.0.client().capabilities();
        if !caps.has(Capabilities::INVOICE_TERMS) && !caps.has(Capabilities::BILL_TERMS) {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let call = Call::at(Method::Get, PAYMENT_TERMS.route());
        let reply = self.0.send(&PAYMENT_TERMS, call, None).await?;
        let v: Value =
            serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode { what: WHAT })?;
        let list = if v.is_array() {
            v
        } else {
            ["terms", "paymentTerms", "data"]
                .iter()
                .find_map(|k| v.get(*k).filter(|x| x.is_array()).cloned())
                .ok_or(ApiError::Decode { what: WHAT })?
        };
        serde_json::from_value(list).map_err(|_| ApiError::Decode { what: WHAT })
    }

    /// The client's open balance and oldest open date as the engine
    /// computed them (`contact-balance-v1`), with the price level and
    /// default markup when the engine has them.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `contact-balance-v1`; `NotFound`; any
    /// [`ApiError`].
    pub async fn client_sales(&self, client: ContactId) -> Result<ClientSales, ApiError> {
        #[derive(Deserialize)]
        struct W {
            contact: ClientSalesWire,
        }
        require(self.0, Capabilities::CONTACT_OPEN_BALANCE)?;
        let call = Call::new(
            Method::Get,
            CONTACT_GET.route(),
            format!("/api/v1/contacts/{client}"),
        );
        let reply = self.0.send(&CONTACT_GET, call, None).await?;
        let w = decode::<W>(&reply, "client balance")?.contact;
        Ok(ClientSales {
            open_balance: w.open_balance,
            oldest_open_date: w.oldest_open_date,
            price_level_id: w.price_level_id,
            default_markup_percent: w.default_markup_percent,
            default_invoice_layout_id: w.default_invoice_layout_id,
        })
    }

    /// Sets a client's price level (`price-levels-v1`) and default markup
    /// (`billable-costs-v1`); `None` clears. Only the fields whose
    /// capability the engine advertises go out (`PUT /contacts/{id}` takes a
    /// partial body; contract per P20 plan §2, verify when E2/E3 land).
    ///
    /// # Errors
    /// `Local(Unsupported)` when neither capability is advertised;
    /// `Local(InvalidValue)` for a negative markup; any [`ApiError`].
    pub async fn set_client_sales(
        &self,
        client: ContactId,
        price_level: Option<PriceLevelId>,
        markup: Option<SignedPercent>,
    ) -> Result<(), ApiError> {
        let caps = self.0.client().capabilities();
        let levels = caps.has(Capabilities::PRICE_LEVELS);
        let billable = caps.has(Capabilities::BILLABLE_COSTS);
        if !levels && !billable {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        if markup.is_some_and(SignedPercent::is_negative) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut m = Map::new();
        if levels {
            m.insert("priceLevelId".into(), json!(price_level));
        }
        if billable {
            m.insert("defaultMarkupPercent".into(), json!(markup));
        }
        let call = Call::new(
            Method::Put,
            CONTACT_PUT.route(),
            format!("/api/v1/contacts/{client}"),
        )
        .json_body(json_body(&Value::Object(m))?);
        self.0.send(&CONTACT_PUT, call, None).await?;
        Ok(())
    }

    /// Sets (or with `None` clears) the layout a client's new invoices
    /// start on (`defaultInvoiceLayoutId`, `invoice-layouts-v1`; partial
    /// `PUT /contacts/{id}`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability (nothing sent); 400
    /// `INVOICE_LAYOUT_NOT_FOUND` / `INVOICE_LAYOUT_ARCHIVED`; any
    /// [`ApiError`].
    pub async fn set_client_layout(
        &self,
        client: ContactId,
        layout: Option<InvoiceLayoutId>,
    ) -> Result<(), ApiError> {
        require(self.0, Capabilities::INVOICE_LAYOUTS)?;
        let call = Call::new(
            Method::Put,
            CONTACT_PUT.route(),
            format!("/api/v1/contacts/{client}"),
        )
        .json_body(json_body(&json!({ "defaultInvoiceLayoutId": layout }))?);
        self.0.send(&CONTACT_PUT, call, None).await?;
        Ok(())
    }

    /// Price levels (`price-levels-v1`).
    #[must_use]
    pub fn price_levels(&self) -> PriceLevels<'_> {
        PriceLevels(self.0)
    }
}

/// Price level operations (`session.invoices().price_levels()`; contract
/// per P20 plan §2, verify when E2 lands).
#[derive(Clone, Copy, Debug)]
pub struct PriceLevels<'a>(&'a Session);

const LEVELS: &str = "/api/v1/price-levels";

impl PriceLevels<'_> {
    async fn one(
        &self,
        op: &'static Operation,
        path: String,
        body: Option<Vec<u8>>,
    ) -> Result<PriceLevel, ApiError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct W {
            price_level: PriceLevel,
        }
        require(self.0, Capabilities::PRICE_LEVELS)?;
        let mut call = Call::new(op.method(), op.route(), path);
        if let Some(b) = body {
            call = call.json_body(b);
        }
        let reply = self.0.send(op, call, None).await?;
        let read = decode::<W>(&reply, "price level").map(|w| w.price_level);
        if op.method().is_read() {
            read
        } else {
            read.map_err(saved_unreadable)
        }
    }

    /// Every price level, active first then by name (the engine's order).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `price-levels-v1`; any [`ApiError`].
    pub async fn list(&self) -> Result<Vec<PriceLevel>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<PriceLevel>,
        }
        require(self.0, Capabilities::PRICE_LEVELS)?;
        let call = Call::at(Method::Get, PRICE_LEVELS_LIST.route());
        let reply = self.0.send(&PRICE_LEVELS_LIST, call, None).await?;
        decode::<W>(&reply, "price levels").map(|w| w.data)
    }

    /// One price level with its item prices.
    ///
    /// # Errors
    /// As [`PriceLevels::list`], plus `NotFound`.
    pub async fn get(&self, id: PriceLevelId) -> Result<PriceLevel, ApiError> {
        self.one(&PRICE_LEVEL_GET, format!("{LEVELS}/{id}"), None)
            .await
    }

    /// Creates a price level.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or long name, an item twice or a
    /// negative price (nothing sent); as [`PriceLevels::list`].
    pub async fn create(&self, draft: &PriceLevelDraft) -> Result<PriceLevel, ApiError> {
        draft.check()?;
        self.one(
            &PRICE_LEVELS_CREATE,
            LEVELS.to_string(),
            Some(json_body(draft)?),
        )
        .await
    }

    /// Replaces a price level.
    ///
    /// # Errors
    /// As [`PriceLevels::create`], plus `NotFound`.
    pub async fn update(
        &self,
        id: PriceLevelId,
        draft: &PriceLevelDraft,
    ) -> Result<PriceLevel, ApiError> {
        draft.check()?;
        self.one(
            &PRICE_LEVEL_PUT,
            format!("{LEVELS}/{id}"),
            Some(json_body(draft)?),
        )
        .await
    }

    /// Deletes a price level (clients on it go back to item prices).
    ///
    /// # Errors
    /// As [`PriceLevels::get`].
    pub async fn delete(&self, id: PriceLevelId) -> Result<(), ApiError> {
        require(self.0, Capabilities::PRICE_LEVELS)?;
        let call = Call::new(
            Method::Delete,
            PRICE_LEVEL_DELETE.route(),
            format!("{LEVELS}/{id}"),
        );
        self.0.send(&PRICE_LEVEL_DELETE, call, None).await?;
        Ok(())
    }
}

/// The file name of an invoice PDF (Q-P20q): `Invoice-<number>.pdf`,
/// matching the engine's `pdfFileName` rule
/// (`^[A-Za-z0-9][A-Za-z0-9 ._()#-]{0,115}\.pdf$`): ASCII letters and
/// digits and ` ._()#-` only (anything else becomes `-`), at most 116
/// characters before `.pdf`.
#[must_use]
pub fn invoice_pdf_name(number: &str) -> String {
    let clean: String = number
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '(' | ')' | '#') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if clean.is_empty() {
        return "Invoice.pdf".to_string();
    }
    let mut stem = format!("Invoice-{clean}");
    stem.truncate(116);
    format!("{stem}.pdf")
}

#[cfg(test)]
#[path = "invoices_tests.rs"]
mod tests;
