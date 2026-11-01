//! Sales receipts (P22, engine U-143; contract per the P22 plan §2.4,
//! verified at the D8 pin bump): an item-and-tax sale paid on the spot,
//! posted straight to the deposit account with no A/R.
//!
//! - Lines, tax and totals come from [`SalesReceipts::preview`] (the shared
//!   invoice compute: preview = post). A negative total (a return) to
//!   Payments Clearing is the preview problem `NEGATIVE_RECEIPT_NEEDS_BANK`
//!   and a 400 on save.
//! - Keyed under `idempotency-credit-memo-v1` (a desktop-side assumption
//!   until the pin bump: the plan names no sales receipt key set).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::credit_memos::{
    CreditWriteOptions, SalesDocLine, SalesDocStatus, check_doc_lines, doc_body,
};
use crate::resources::customer_payments::{
    PAYMENT_REF_MAX, PreviewProblem, de_cents, de_cents_zero, decode_key, null_vec, require,
    too_long, with_version,
};
use crate::resources::invoices::InvoicePreview;
use crate::resources::operations::{
    SALES_RECEIPT_CREATE, SALES_RECEIPT_GET, SALES_RECEIPT_PREVIEW, SALES_RECEIPT_PUT,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::sales::{InvoiceLine, LayoutChoice};
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// How a sale was paid (the engine's `PAYMENT_METHODS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    /// Check.
    Check,
    /// Cash.
    Cash,
    /// Credit or debit card.
    CreditCard,
    /// Bank transfer.
    Ach,
    /// Anything else.
    #[serde(other)]
    Other,
}

/// `POST /sales-receipts` (and the bodies of PUT and preview).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSalesReceipt {
    /// The sale date.
    pub txn_date: CalendarDate,
    /// The client (optional for a walk-in sale).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_id: Option<ContactId>,
    /// Where the money goes (`None`: Payments Clearing, Q-P20h).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit_to_account_id: Option<AccountId>,
    /// How it was paid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<PaymentMethod>,
    /// Check or reference number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_no: Option<String>,
    /// Lines (1-500).
    pub lines: Vec<InvoiceLine>,
    /// Internal memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// The printed message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_message: Option<String>,
    /// Header tags.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<TagId>>,
    /// Leave $0 lines off the printed receipt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_zero_lines: Option<bool>,
    /// The print layout.
    #[serde(skip_serializing_if = "LayoutChoice::is_omitted")]
    pub layout_id: LayoutChoice,
    /// A typed number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn_number: Option<String>,
    /// A number already in use posts with a warning.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warn_only: bool,
    /// Waiting to be printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_later: Option<bool>,
}

impl NewSalesReceipt {
    /// A sales receipt dated `date`; every optional field unset.
    #[must_use]
    pub fn new(date: CalendarDate, lines: Vec<InvoiceLine>) -> NewSalesReceipt {
        NewSalesReceipt {
            txn_date: date,
            contact_id: None,
            deposit_to_account_id: None,
            payment_method: None,
            ref_no: None,
            lines,
            memo: None,
            customer_message: None,
            tag_ids: None,
            hide_zero_lines: None,
            layout_id: LayoutChoice::Omitted,
            txn_number: None,
            warn_only: false,
            print_later: None,
        }
    }
}

impl fmt::Debug for NewSalesReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewSalesReceipt")
            .field("txn_date", &self.txn_date)
            .field("contact_id", &self.contact_id)
            .field("deposit_to_account_id", &self.deposit_to_account_id)
            .field("payment_method", &self.payment_method)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// Checks a sales receipt before anything is sent.
///
/// # Errors
/// `Local(Unsupported)` without `sales-receipt-lines-v1`; as the credit
/// memo's line checks; `InvalidValue` for a reference over 50 characters.
pub fn check_sales_receipt(r: &NewSalesReceipt, caps: &Capabilities) -> Result<(), ApiError> {
    if !caps.has(Capabilities::SALES_RECEIPT_LINES) {
        return Err(ApiError::Local(LocalValidation::Unsupported));
    }
    check_doc_lines(
        &r.lines,
        r.customer_message.as_deref(),
        r.memo.as_deref(),
        r.tag_ids.as_ref(),
        r.txn_number.as_deref(),
    )?;
    if too_long(r.ref_no.as_deref(), PAYMENT_REF_MAX) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(())
}

/// `POST /sales-receipts/preview`: P20's invoice preview figures and the
/// problems (`NEGATIVE_RECEIPT_NEEDS_BANK`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesReceiptPreview {
    /// Lines, subtotal, tax and total.
    #[serde(flatten)]
    pub figures: InvoicePreview,
    /// Problems (never 4xx from the preview).
    #[serde(default, deserialize_with = "null_vec")]
    pub problems: Vec<PreviewProblem>,
}

impl fmt::Debug for SalesReceiptPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesReceiptPreview")
            .field("figures", &self.figures)
            .field("problems", &self.problems)
            .finish()
    }
}

/// A saved sales receipt (`GET /sales-receipts/{id}`, `{salesReceipt: …}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SalesReceiptRecord {
    /// The receipt.
    pub id: TransactionId,
    /// Its number.
    #[serde(default, alias = "txnNumber")]
    pub number: Option<String>,
    /// Its date.
    #[serde(alias = "txnDate")]
    pub date: CalendarDate,
    /// The client.
    #[serde(default, alias = "contactId")]
    pub customer_id: Option<ContactId>,
    /// Where the money went.
    #[serde(default)]
    pub deposit_to_account_id: Option<AccountId>,
    /// How it was paid.
    #[serde(default)]
    pub payment_method: Option<PaymentMethod>,
    /// Reference (owner data).
    #[serde(default)]
    pub ref_no: Option<String>,
    /// Its lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<SalesDocLine>,
    /// Σ lines.
    #[serde(default, deserialize_with = "de_cents_zero")]
    pub subtotal: Money,
    /// Tax.
    #[serde(default, alias = "taxTotal", deserialize_with = "de_cents_zero")]
    pub tax_amount: Money,
    /// Total (negative for a return).
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
    /// The deposit it is in (depositTo and amount are then locked).
    #[serde(default)]
    pub deposit_id: Option<TransactionId>,
    /// It is in a deposit (the engine's flag; `deposit_id` when known).
    #[serde(default)]
    pub deposited: bool,
}

impl fmt::Debug for SalesReceiptRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesReceiptRecord")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("customer_id", &self.customer_id)
            .field("status", &self.status)
            .field("lines", &self.lines.len())
            .field("deposit_id", &self.deposit_id)
            .finish_non_exhaustive()
    }
}

/// Reads a sales receipt from an answer (`{salesReceipt: …}`).
///
/// # Errors
/// `SavedUnreadable` when the answer can't be read.
pub fn saved_sales_receipt(body: &[u8]) -> Result<SalesReceiptRecord, ApiError> {
    decode_key(body, "salesReceipt", "sales receipt")
        .map_err(crate::resources::versioning::saved_unreadable)
}

/// Sales receipt operations of a session (`session.sales_receipts()`).
#[derive(Clone, Copy, Debug)]
pub struct SalesReceipts<'a>(pub(crate) &'a Session);

impl Session {
    /// Sales receipts of the open company (P22).
    #[must_use]
    pub fn sales_receipts(&self) -> SalesReceipts<'_> {
        SalesReceipts(self)
    }
}

impl SalesReceipts<'_> {
    /// Every figure of `r` as the engine would post it, posting nothing.
    ///
    /// # Errors
    /// The local checks of [`check_sales_receipt`]; `Decode`; any
    /// [`ApiError`].
    pub async fn preview(
        &self,
        r: &NewSalesReceipt,
        editing: Option<TransactionId>,
    ) -> Result<SalesReceiptPreview, ApiError> {
        let s = &self.0.pinned();
        check_sales_receipt(r, &s.client().capabilities())?;
        let body = doc_body(r, editing)?;
        let call =
            Call::at(Method::Post, SALES_RECEIPT_PREVIEW.route()).json_body(json_body(&body)?);
        let reply = s.send(&SALES_RECEIPT_PREVIEW, call, None).await?;
        let p: SalesReceiptPreview =
            serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
                what: "sales receipt preview",
            })?;
        if p.figures.lines.len() != r.lines.len() {
            return Err(ApiError::Decode {
                what: "sales receipt preview",
            });
        }
        Ok(p)
    }

    /// Records a sales receipt.
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)`; from the engine:
    /// `NEGATIVE_RECEIPT_NEEDS_BANK`, `DOCUMENT_NUMBER_IN_USE`, closing;
    /// `SavedUnreadable`; any [`ApiError`].
    pub async fn create(
        &self,
        r: &NewSalesReceipt,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<SalesReceiptRecord, ApiError> {
        let s = &self.0.pinned();
        check_sales_receipt(r, &s.client().capabilities())?;
        let body = posting_body(s, doc_body(r, None)?, opts)?;
        let mut call = Call::at(Method::Post, SALES_RECEIPT_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&SALES_RECEIPT_CREATE, call, None).await?;
        saved_sales_receipt(&reply.body)
    }

    /// One sales receipt.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<SalesReceiptRecord, ApiError> {
        require(self.0, Capabilities::SALES_RECEIPT_LINES)?;
        let call = Call::new(
            Method::Get,
            SALES_RECEIPT_GET.route(),
            format!("/api/v1/sales-receipts/{id}"),
        );
        let reply = self.0.send(&SALES_RECEIPT_GET, call, None).await?;
        decode_key(&reply.body, "salesReceipt", "sales receipt")
    }

    /// Edits a sales receipt if it is still at `opts.expected_updated_at`.
    ///
    /// # Errors
    /// As [`SalesReceipts::create`], plus `STALE`, `TXN_DEPOSITED`.
    pub async fn update(
        &self,
        id: TransactionId,
        r: &NewSalesReceipt,
        opts: &CreditWriteOptions,
        key: IdempotencyKey,
    ) -> Result<SalesReceiptRecord, ApiError> {
        let s = &self.0.pinned();
        check_sales_receipt(r, &s.client().capabilities())?;
        let body = posting_body(
            s,
            with_version(doc_body(r, None)?, opts.expected_updated_at),
            &opts.posting,
        )?;
        let mut call = Call::new(
            Method::Put,
            SALES_RECEIPT_PUT.route(),
            format!("/api/v1/sales-receipts/{id}"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&SALES_RECEIPT_PUT, call, None).await?;
        saved_sales_receipt(&reply.body)
    }
}
