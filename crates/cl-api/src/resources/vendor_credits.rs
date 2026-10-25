//! Vendor credits (P17 WP2): the second kind of the Enter Bill form.
//!
//! The same rules as bills (`ap_doc`): exact string money, capability
//! gates with nothing sent, the D2 lost-reply probe, fingerprint-guarded
//! edits. A credit has no terms, due date or received flag. Editing and
//! deleting need `ap-edit-v1` (U-84); until then a saved credit can only be
//! voided.

use std::fmt;

use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Timestamp};
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::ap_doc::{
    ApKind, ApWriteOptions, BillLineDraft, BillRecord, Capped, DocParts, DocRoutes, create_doc,
    delete_doc, list_capped, update_doc,
};
use crate::resources::ap_void::void_with;
use crate::resources::attachments::DraftKey;
use crate::resources::common::{Query, SortDir};
use crate::resources::operations::{
    VENDOR_CREDIT_GET, VENDOR_CREDIT_PUT, VENDOR_CREDIT_VOID, VENDOR_CREDITS_AVAILABLE,
    VENDOR_CREDITS_CREATE, VENDOR_CREDITS_LIST,
};
use crate::resources::posting::PostingOptions;
use crate::resources::transactions::TxnStatus;
use crate::resources::versioning::{Versioned, get_versioned};
use crate::session::{Session, decode};
use crate::wire::{Capabilities, Page, PageRequest};

const BASE: &str = "/api/v1/vendor-credits";

const ROUTES: DocRoutes = DocRoutes {
    create: &VENDOR_CREDITS_CREATE,
    list: &VENDOR_CREDITS_LIST,
    get: &VENDOR_CREDIT_GET,
    put: &VENDOR_CREDIT_PUT,
    base: BASE,
    key: "credit",
};

/// A vendor credit to save.
#[derive(Clone, PartialEq, Eq)]
pub struct VendorCreditDraft {
    /// The vendor.
    pub vendor: ContactId,
    /// Credit date.
    pub date: CalendarDate,
    /// The vendor's credit memo number (trimmed; at most 100 characters).
    pub vendor_invoice_number: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// The credit amount: the lines must add up to it exactly.
    pub amount: Money,
    /// Lines (1 to 500). A tag needs `bill-lines-v1` on a credit.
    pub lines: Vec<BillLineDraft>,
    /// The draft the form's files were uploaded under (create only;
    /// `bill-lines-v1`; otherwise link each file with
    /// `attachments().link`).
    pub draft_attachment: Option<DraftKey>,
}

impl fmt::Debug for VendorCreditDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VendorCreditDraft")
            .field("vendor", &self.vendor)
            .field("date", &self.date)
            .field("draft_attachment", &self.draft_attachment)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl VendorCreditDraft {
    fn parts(&self, with_draft: bool) -> DocParts<'_> {
        DocParts {
            kind: ApKind::VendorCredit,
            vendor: self.vendor,
            date: self.date,
            vendor_ref: self.vendor_invoice_number.as_deref(),
            memo: self.memo.as_deref(),
            amount: self.amount,
            lines: &self.lines,
            terms: None,
            due_date: None,
            received: true,
            draft: if with_draft {
                self.draft_attachment
            } else {
                None
            },
        }
    }

    /// The fingerprint the credit has once saved.
    #[must_use]
    pub fn fingerprint(&self) -> [u8; 32] {
        crate::resources::ap_doc::requested_fingerprint(&self.parts(false))
    }
}

/// Sort keys of the vendor credits list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs, reason = "column names of the vendor credits list")]
pub enum CreditSort {
    Number,
    Vendor,
    Date,
    Total,
    Balance,
}

impl CreditSort {
    const fn as_str(self) -> &'static str {
        match self {
            CreditSort::Number => "txnNumber",
            CreditSort::Vendor => "contactName",
            CreditSort::Date => "txnDate",
            CreditSort::Total => "total",
            CreditSort::Balance => "balanceDue",
        }
    }
}

/// Filters of the vendor credits list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreditListQuery {
    /// One vendor.
    pub vendor: Option<ContactId>,
    /// Memo, number or vendor name contains (with `ap-edit-v1` also the
    /// reference, S6). Owner data: never logged.
    pub search: Option<String>,
    /// Dated on or after.
    pub start: Option<CalendarDate>,
    /// Dated on or before.
    pub end: Option<CalendarDate>,
    /// Sort.
    pub sort: Option<(CreditSort, SortDir)>,
}

impl CreditListQuery {
    fn pairs(&self) -> Vec<(String, String)> {
        let mut q = Query::default();
        if let Some(v) = self.vendor {
            q.put("contactId", v.to_string());
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

/// One row of the vendor credits list.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditSummary {
    /// The credit.
    pub id: TransactionId,
    /// The book number (`VC-00003`).
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// The vendor's reference (rows carry it with `ap-edit-v1`, S6).
    #[serde(default)]
    pub vendor_invoice_number: Option<String>,
    /// Total.
    #[serde(default)]
    pub total: Option<Money>,
    /// What is still open.
    #[serde(default)]
    pub balance_due: Option<Money>,
    /// Posted or void.
    pub status: TxnStatus,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
}

impl fmt::Debug for CreditSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreditSummary")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// A credit with an open balance (`GET /vendor-credits/available/{vendor}`):
/// the engine's figures as sent, never summed here.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableCredit {
    /// The credit.
    pub id: TransactionId,
    /// Book number.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// Total.
    #[serde(default)]
    pub total: Option<Money>,
    /// Still available.
    pub balance_due: Money,
}

impl fmt::Debug for AvailableCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AvailableCredit")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .finish_non_exhaustive()
    }
}

/// Vendor credit operations of a session (`session.vendor_credits()`).
#[derive(Clone, Copy, Debug)]
pub struct VendorCredits<'a>(pub(crate) &'a Session);

impl Session {
    /// Vendor credits of the open company.
    #[must_use]
    pub fn vendor_credits(&self) -> VendorCredits<'_> {
        VendorCredits(self)
    }
}

impl VendorCredits<'_> {
    /// One credit with its lines and version.
    ///
    /// # Errors
    /// `NotFound`; `Validation` when it is not a vendor credit; any
    /// [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<Versioned<BillRecord>, ApiError> {
        get_versioned(self.0, &VENDOR_CREDIT_GET, format!("{BASE}/{id}"), "credit").await
    }

    /// Records a vendor credit (`POST /vendor-credits`).
    ///
    /// # Errors
    /// As `Bills::create`.
    pub async fn create(
        &self,
        draft: &VendorCreditDraft,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<BillRecord>, ApiError> {
        create_doc(self.0, &ROUTES, &draft.parts(true), opts, key).await
    }

    /// Changes an unapplied credit (`PUT /vendor-credits/{id}`,
    /// `ap-edit-v1`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `ap-edit-v1` (nothing sent);
    /// `Conflict` `CREDIT_APPLIED` once applied; otherwise as
    /// `Bills::update`.
    pub async fn update(
        &self,
        id: TransactionId,
        expected: &Versioned<BillRecord>,
        draft: &VendorCreditDraft,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Versioned<BillRecord>, ApiError> {
        if !self.0.client().capabilities().has(Capabilities::AP_EDIT) {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
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

    /// Voids a credit not applied to bills. A reason is required.
    ///
    /// # Errors
    /// As `Bills::void`; `Validation` when the credit is applied.
    pub async fn void(
        &self,
        id: TransactionId,
        reason: &str,
        opts: &ApWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        void_with(self.0, &VENDOR_CREDIT_VOID, BASE, id, reason, opts, key).await
    }

    /// Deletes an unapplied credit (`ap-edit-v1`).
    ///
    /// # Errors
    /// As `Bills::delete`.
    pub async fn delete(
        &self,
        id: TransactionId,
        reason: Option<&str>,
        opts: &ApWriteOptions,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        delete_doc(self.0, id, reason, opts, key).await
    }

    /// One page of credits.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn list(
        &self,
        q: &CreditListQuery,
        page: PageRequest,
    ) -> Result<Page<CreditSummary>, ApiError> {
        let mut call = Call::at(Method::Get, VENDOR_CREDITS_LIST.route());
        call.query = q.pairs();
        call.query.extend(page.query());
        let reply = self.0.send(&VENDOR_CREDITS_LIST, call, None).await?;
        Page::from_json(&reply.body, "data")
    }

    /// Up to `max_pages` pages of credits.
    ///
    /// # Errors
    /// As [`VendorCredits::list`]; `Local(PageLimit)`.
    pub async fn list_capped(
        &self,
        q: &CreditListQuery,
        page_size: u16,
        max_pages: u32,
    ) -> Result<Capped<CreditSummary>, ApiError> {
        list_capped(
            self.0,
            &VENDOR_CREDITS_LIST,
            &q.pairs(),
            page_size,
            max_pages,
        )
        .await
    }

    /// A vendor's credits with something left to apply, oldest first.
    ///
    /// # Errors
    /// `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn available(&self, vendor: ContactId) -> Result<Vec<AvailableCredit>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            credits: Vec<AvailableCredit>,
        }
        let call = Call::new(
            Method::Get,
            VENDOR_CREDITS_AVAILABLE.route(),
            format!("{BASE}/available/{vendor}"),
        );
        let reply = self.0.send(&VENDOR_CREDITS_AVAILABLE, call, None).await?;
        decode::<W>(&reply, "available credits").map(|w| w.credits)
    }
}
