//! Bills (P13 WP2 payee panel; P17 WP2 Enter Bill).
//!
//! - Readers for the payee panel: a vendor's open bills in brief (`GET
//!   /bills/vendor-summary`) and the unpaid bills (`GET /bills/payable`).
//!   Every amount is the engine's own figure, read exactly from its decimal
//!   string into `Money` and shown as sent: the app never adds these up or
//!   derives a balance from them (CLAUDE.md non-negotiable 7). A JSON number
//!   where money belongs is a decode error, never a rounded value.
//! - Enter Bill: [`Bills::create`], [`Bills::update`], [`Bills::void`],
//!   [`Bills::delete`], [`Bills::get`], [`Bills::list`], the terms helpers
//!   and [`Bills::attach_draft`]. The shared rules (capability gates, the
//!   fingerprint, the D2 lost-reply probe) are in `ap_doc`.

use std::fmt;

use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::ap_doc::{
    ApKind, ApWriteOptions, BillLineDraft, BillRecord, BillStatus, BillTerms, Capped, DocParts,
    DocRoutes, Percent, create_doc, delete_doc, list_capped, update_doc,
};
use crate::resources::ap_void::void_with;
use crate::resources::attachments::DraftKey;
use crate::resources::common::{Query, SortDir};
use crate::resources::operations::{
    BILL_ATTACH_DRAFT, BILL_GET, BILL_PUT, BILL_TERMS_DATES, BILL_VOID, BILLS_CREATE, BILLS_LIST,
    BILLS_PAYABLE, BILLS_VENDOR_SUMMARY, PAYMENT_TERMS,
};
use crate::resources::posting::PostingOptions;
use crate::resources::transactions::TxnStatus;
use crate::resources::versioning::{Versioned, get_versioned, saved_unreadable};
use crate::session::{Session, decode, json_body};
use crate::wire::{Capabilities, Page, PageRequest};

const BASE: &str = "/api/v1/bills";

pub(crate) const ROUTES: DocRoutes = DocRoutes {
    create: &BILLS_CREATE,
    list: &BILLS_LIST,
    get: &BILL_GET,
    put: &BILL_PUT,
    base: BASE,
    key: "bill",
};

/// A vendor's open bills in brief, as the engine counts them.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorSummary {
    /// Bills with a balance due.
    pub unpaid_count: u32,
    /// What those bills still owe.
    pub total_owed: Money,
    /// Open bills past their due date.
    pub overdue_count: u32,
    /// What the overdue bills still owe.
    pub total_overdue: Money,
    /// Vendor credits with something left to apply.
    #[serde(default)]
    pub available_credits_count: u32,
    /// What those credits still hold.
    #[serde(default = "zero")]
    pub total_available_credits: Money,
}

fn zero() -> Money {
    Money::ZERO
}

/// Counts only: the amounts are owner data and never reach a log.
impl std::fmt::Debug for VendorSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorSummary")
            .field("unpaid_count", &self.unpaid_count)
            .field("overdue_count", &self.overdue_count)
            .field("available_credits_count", &self.available_credits_count)
            .finish_non_exhaustive()
    }
}

/// One unpaid bill (`GET /bills/payable` row; the memo and the vendor
/// name are not kept).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenBill {
    /// The bill.
    pub id: TransactionId,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The bill's own number in the books.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// The vendor's invoice number.
    #[serde(default)]
    pub vendor_invoice_number: Option<String>,
    /// Bill date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(default, rename = "dueDate")]
    pub due: Option<CalendarDate>,
    /// The bill's total.
    #[serde(default)]
    pub total: Option<Money>,
    /// What is still owed on it.
    #[serde(rename = "balanceDue")]
    pub amount_due: Money,
    /// Whole days past due (0 when not due yet), as the engine counted
    /// them today.
    #[serde(default)]
    pub days_overdue: u32,
}

/// Ids and dates only: numbers and amounts are owner data.
impl std::fmt::Debug for OpenBill {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenBill")
            .field("id", &self.id)
            .field("due", &self.due)
            .field("days_overdue", &self.days_overdue)
            .finish_non_exhaustive()
    }
}

/// A bill to save (Enter Bill).
#[derive(Clone, PartialEq, Eq)]
pub struct BillDraft {
    /// The vendor.
    pub vendor: ContactId,
    /// Bill date.
    pub date: CalendarDate,
    /// The vendor's invoice number (trimmed; at most 100 characters).
    pub vendor_invoice_number: Option<String>,
    /// Payment terms (`None`: the vendor's).
    pub terms: Option<BillTerms>,
    /// The owner's due date; `None` lets the engine compute it from the
    /// terms (the form previews it until `bill-terms-v1`).
    pub due_date: Option<CalendarDate>,
    /// Memo.
    pub memo: Option<String>,
    /// Amount due: the lines must add up to it exactly.
    pub amount_due: Money,
    /// Expense and item lines (1 to 500).
    pub lines: Vec<BillLineDraft>,
    /// Bill received (`false` needs `bill-received-v1`).
    pub received: bool,
    /// The draft the form's files were uploaded under (create only;
    /// `bill-lines-v1`, otherwise relink with [`Bills::attach_draft`]).
    pub draft_attachment: Option<DraftKey>,
}

/// Ids, dates, flags and counts only.
impl fmt::Debug for BillDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillDraft")
            .field("vendor", &self.vendor)
            .field("date", &self.date)
            .field("terms", &self.terms)
            .field("due_date", &self.due_date)
            .field("received", &self.received)
            .field("draft_attachment", &self.draft_attachment)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl BillDraft {
    fn parts(&self, with_draft: bool) -> DocParts<'_> {
        DocParts {
            kind: ApKind::Bill,
            vendor: self.vendor,
            date: self.date,
            vendor_ref: self.vendor_invoice_number.as_deref(),
            memo: self.memo.as_deref(),
            amount: self.amount_due,
            lines: &self.lines,
            terms: self.terms.as_ref(),
            due_date: self.due_date,
            received: self.received,
            draft: if with_draft {
                self.draft_attachment
            } else {
                None
            },
        }
    }

    /// The fingerprint the bill has once saved (outcome checks; tests).
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        crate::resources::ap_doc::requested_fingerprint(&self.parts(false))
    }
}

/// Sort keys of the bills list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "column names of the bills list")]
pub enum BillSort {
    Number,
    Vendor,
    VendorInvoiceNumber,
    Date,
    DueDate,
    Status,
    Total,
    Balance,
}

impl BillSort {
    const fn as_str(self) -> &'static str {
        match self {
            BillSort::Number => "number",
            BillSort::Vendor => "vendor",
            BillSort::VendorInvoiceNumber => "vendorInvoiceNumber",
            BillSort::Date => "date",
            BillSort::DueDate => "dueDate",
            BillSort::Status => "status",
            BillSort::Total => "total",
            BillSort::Balance => "balance",
        }
    }
}

/// Filters of the bills list (`GET /bills`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BillListQuery {
    /// One vendor.
    pub vendor: Option<ContactId>,
    /// Memo, number, reference or vendor name contains (substring, any
    /// case; owner data: never logged).
    pub search: Option<String>,
    /// Dated on or after.
    pub start: Option<CalendarDate>,
    /// Dated on or before.
    pub end: Option<CalendarDate>,
    /// Sort.
    pub sort: Option<(BillSort, SortDir)>,
    /// Only bills with something still owed (`billStatus=unpaid,partial,
    /// overdue`), received or not: unlike `/bills/payable`, which leaves
    /// not-received bills out (process review S4). Void rows still come;
    /// keep `status == Posted`.
    pub open_only: bool,
}

/// The bill states that still owe something.
const OPEN_STATUSES: &str = "unpaid,partial,overdue";

impl BillListQuery {
    pub(crate) fn pairs(&self) -> Vec<(String, String)> {
        let mut q = Query::default();
        if let Some(v) = self.vendor {
            q.put("contactId", v.to_string());
        }
        if self.open_only {
            q.put("billStatus", OPEN_STATUSES.to_string());
        }
        q.opt_str("search", self.search.as_deref());
        if let Some(d) = self.start {
            q.put("startDate", d.to_wire());
        }
        if let Some(d) = self.end {
            q.put("endDate", d.to_wire());
        }
        q.sort(self.sort.map(|(s, d)| (s.as_str(), d)));
        q.0
    }
}

/// One row of the bills list (posted and void alike: keep `status ==
/// Posted` where only live bills count).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillSummary {
    /// The bill.
    pub id: TransactionId,
    /// The book number (`BILL-00012`).
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// Bill date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The vendor's invoice number (owner data).
    #[serde(default)]
    pub vendor_invoice_number: Option<String>,
    /// Terms code.
    #[serde(default)]
    pub payment_terms: Option<String>,
    /// Total.
    #[serde(default)]
    pub total: Option<Money>,
    /// Paid so far.
    #[serde(default)]
    pub amount_paid: Option<Money>,
    /// Credits applied.
    #[serde(default)]
    pub credits_applied: Option<Money>,
    /// Still owed.
    #[serde(default)]
    pub balance_due: Option<Money>,
    /// Payment state.
    #[serde(default)]
    pub bill_status: Option<BillStatus>,
    /// Posted or void.
    pub status: TxnStatus,
    /// Received (`bill-received-v1`).
    #[serde(default)]
    pub bill_received: Option<bool>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

/// Ids, dates and states only.
impl fmt::Debug for BillSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillSummary")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("due_date", &self.due_date)
            .field("bill_status", &self.bill_status)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// What the due and discount dates come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TermsSource {
    /// The terms sent.
    Input,
    /// The vendor's terms.
    Vendor,
    /// A source this version does not know.
    #[serde(other)]
    Other,
}

/// The dates a bill would get (`GET /bills/terms-dates`, `bill-terms-v1`):
/// the same function the create uses.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermsDates {
    /// Due date (`None` without terms).
    #[serde(default)]
    pub due_date: Option<CalendarDate>,
    /// Last day of the discount.
    #[serde(default)]
    pub discount_date: Option<CalendarDate>,
    /// The terms applied.
    #[serde(default)]
    pub payment_terms: Option<String>,
    /// Their net days.
    #[serde(default)]
    pub terms_days: Option<u32>,
    /// Their discount.
    #[serde(default)]
    pub discount_percent: Option<Percent>,
    /// Their discount days.
    #[serde(default)]
    pub discount_days: Option<u32>,
    /// Where they came from.
    pub source: TermsSource,
}

/// What [`Bills::terms_dates`] asks about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TermsDatesQuery {
    /// Bill date.
    pub date: CalendarDate,
    /// Terms (`None`: the vendor's).
    pub terms: Option<BillTerms>,
    /// The vendor (for its terms).
    pub vendor: Option<ContactId>,
}

/// One fixed payment term (`GET /payment-terms`, `bill-terms-v1`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaymentTerm {
    /// Code (`net_30`, `2_10_net_30`, `custom`).
    pub code: String,
    /// Net days (`None` for `custom`).
    #[serde(default)]
    pub net_days: Option<u32>,
    /// Early-payment discount.
    #[serde(default)]
    pub discount_percent: Option<Percent>,
    /// Discount days.
    #[serde(default)]
    pub discount_days: Option<u32>,
}

/// Bill operations of a session (`session.bills()`).
#[derive(Clone, Copy, Debug)]
pub struct Bills<'a>(pub(crate) &'a Session);

impl Session {
    /// Bills of the open company.
    #[must_use]
    pub fn bills(&self) -> Bills<'_> {
        Bills(self)
    }
}

impl Bills<'_> {
    /// A vendor's open bills in brief.
    ///
    /// # Errors
    /// `Decode` when an amount is not an exact decimal string; any
    /// [`ApiError`].
    pub async fn vendor_summary(&self, vendor: ContactId) -> Result<VendorSummary, ApiError> {
        #[derive(Deserialize)]
        struct W {
            summary: VendorSummary,
        }
        let mut call = Call::at(Method::Get, BILLS_VENDOR_SUMMARY.route());
        call.query = vec![("contactId".into(), vendor.to_string())];
        let reply = self.0.send(&BILLS_VENDOR_SUMMARY, call, None).await?;
        decode::<W>(&reply, "vendor summary").map(|w| w.summary)
    }

    /// A vendor's unpaid bills, soonest due first (the engine's order).
    ///
    /// # Errors
    /// `Decode` when an amount is not an exact decimal string; any
    /// [`ApiError`].
    pub async fn payable(&self, vendor: ContactId) -> Result<Vec<OpenBill>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            bills: Vec<OpenBill>,
        }
        let mut call = Call::at(Method::Get, BILLS_PAYABLE.route());
        call.query = vec![("contactId".into(), vendor.to_string())];
        let reply = self.0.send(&BILLS_PAYABLE, call, None).await?;
        decode::<W>(&reply, "payable bills").map(|w| w.bills)
    }

    /// One bill with its lines and version, to fill the form.
    ///
    /// # Errors
    /// `NotFound`; `Validation` when it is not a bill; any [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<Versioned<BillRecord>, ApiError> {
        get_versioned(self.0, &BILL_GET, format!("{BASE}/{id}"), "bill").await
    }

    /// Enters a bill (`POST /bills`).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)`: `Unsupported` (a field without its
    /// capability, see the module notes of `ap_doc`), `TooFewLines`,
    /// `TooManyLines`, `NonPositiveAmount`, `TotalMismatch`,
    /// `LineAmountMismatch`, `InvalidValue`,
    /// `ClosedPeriodOverrideUnavailable`. From the engine: `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `Validation` (see [`crate::ap_refusal`]),
    /// `OutcomeUnknown` when the outcome could not be told (PR-1 rule:
    /// the save stays blocked until the owner checks), any [`ApiError`].
    pub async fn create(
        &self,
        draft: &BillDraft,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<BillRecord>, ApiError> {
        create_doc(self.0, &ROUTES, &draft.parts(true), opts, key).await
    }

    /// Changes a bill if it is still as `expected` was read. A draft
    /// attachment is not sent on an edit (upload files to the bill itself).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent, or 409 `STALE` with
    /// `ap-edit-v1`); `Validation` when the engine refuses the change of a
    /// bill with payments (vendor, date or total); otherwise as
    /// [`Bills::create`].
    pub async fn update(
        &self,
        id: TransactionId,
        expected: &Versioned<BillRecord>,
        draft: &BillDraft,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<BillRecord>, ApiError> {
        update_doc(
            self.0,
            &ROUTES,
            id,
            expected,
            &draft.parts(false),
            opts,
            key,
        )
        .await
    }

    /// Voids an unpaid bill (`POST /bills/{id}/void`). A reason is
    /// required.
    ///
    /// # Errors
    /// `Local(MissingReason)`, `Local(Unsupported)` (a closing-date password
    /// without `ap-closing-v1`), nothing sent; `Validation` when payments
    /// are applied; `ClosedPeriod`; `Conflict { Stale }` (`ap-edit-v1`);
    /// `OutcomeUnknown`; any [`ApiError`].
    pub async fn void(
        &self,
        id: TransactionId,
        reason: &str,
        opts: &ApWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        void_with(self.0, &BILL_VOID, BASE, id, reason, opts, key).await
    }

    /// Deletes an unpaid bill (`ap-edit-v1`; void it before).
    ///
    /// # Errors
    /// See `ap_doc::delete_doc`: `Local(Unsupported)` without `ap-edit-v1`
    /// (nothing sent).
    pub async fn delete(
        &self,
        id: TransactionId,
        reason: Option<&str>,
        opts: &ApWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        delete_doc(self.0, id, reason, opts, key).await
    }

    /// One page of bills.
    ///
    /// # Errors
    /// `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn list(
        &self,
        q: &BillListQuery,
        page: PageRequest,
    ) -> Result<Page<BillSummary>, ApiError> {
        let mut call = Call::at(Method::Get, BILLS_LIST.route());
        call.query = q.pairs();
        call.query.extend(page.query());
        let reply = self.0.send(&BILLS_LIST, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// Up to `max_pages` pages of `page_size` bills (D3's duplicate check
    /// reads 20 × 50 and says so when that was not every bill).
    ///
    /// # Errors
    /// `Local(PageLimit)` for a page size outside 1..=500; as
    /// [`Bills::list`].
    pub async fn list_capped(
        &self,
        q: &BillListQuery,
        page_size: u16,
        max_pages: u32,
    ) -> Result<Capped<BillSummary>, ApiError> {
        list_capped(self.0, &BILLS_LIST, &q.pairs(), page_size, max_pages).await
    }

    /// The due and discount dates the engine would give a bill
    /// (`bill-terms-v1`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `bill-terms-v1` (nothing sent; the form
    /// previews the date itself); any [`ApiError`].
    pub async fn terms_dates(&self, q: &TermsDatesQuery) -> Result<TermsDates, ApiError> {
        if !self.0.client().capabilities().has(Capabilities::BILL_TERMS) {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        let mut query = Query::default();
        query.put("txnDate", q.date.to_wire());
        if let Some(t) = &q.terms {
            query.put("paymentTerms", t.code.clone());
            if let Some(d) = t.days {
                query.put("termsDays", d.to_string());
            }
            if let Some(p) = t.discount_percent {
                query.put("discountPercent", p.to_wire());
            }
            if let Some(d) = t.discount_days {
                query.put("discountDays", d.to_string());
            }
        }
        if let Some(v) = q.vendor {
            query.put("contactId", v.to_string());
        }
        let mut call = Call::at(Method::Get, BILL_TERMS_DATES.route());
        call.query = query.0;
        let reply = self.0.send(&BILL_TERMS_DATES, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "terms dates",
        })?;
        let inner = v.get("termsDates").cloned().unwrap_or(v);
        serde_json::from_value(inner).map_err(|_| ApiError::Decode {
            what: "terms dates",
        })
    }

    /// The fixed payment terms (`bill-terms-v1`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `bill-terms-v1` (nothing sent); any
    /// [`ApiError`].
    pub async fn payment_terms(&self) -> Result<Vec<PaymentTerm>, ApiError> {
        const WHAT: &str = "payment terms";
        if !self.0.client().capabilities().has(Capabilities::BILL_TERMS) {
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

    /// Moves every file of `draft` onto the saved bill (`POST
    /// /bills/{id}/attach-draft`): the two-call path before `bill-lines-v1`.
    /// Running it again moves nothing more. Returns how many moved.
    ///
    /// # Errors
    /// `SavedUnreadable` when the answer can't be read; any [`ApiError`].
    pub async fn attach_draft(
        &self,
        id: TransactionId,
        draft: DraftKey,
        key: IdempotencyKey,
    ) -> Result<u64, ApiError> {
        #[derive(Deserialize)]
        struct W {
            #[serde(default)]
            reassigned: u64,
        }
        let body = json_body(&json!({ "draftId": draft.to_string() }))?;
        let mut call = Call::new(
            Method::Post,
            BILL_ATTACH_DRAFT.route(),
            format!("{BASE}/{id}/attach-draft"),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&BILL_ATTACH_DRAFT, call, None).await?;
        decode::<W>(&reply, "attach draft")
            .map(|w| w.reassigned)
            .map_err(saved_unreadable)
    }
}

#[cfg(test)]
#[path = "bills_tests.rs"]
mod tests;
