//! Open bills for the Write Check payee panel (P13 WP2): a vendor's open
//! bills in brief (`GET /bills/vendor-summary`) and the unpaid bills
//! themselves (`GET /bills/payable`).
//!
//! Every amount is the engine's own figure, read exactly from its decimal
//! string into `Money` and shown as sent: the app never adds these up or
//! derives a balance from them (CLAUDE.md non-negotiable 7). A JSON number
//! where money belongs is a decode error, never a rounded value.

use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::{BILLS_PAYABLE, BILLS_VENDOR_SUMMARY};
use crate::session::{Session, decode};

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

/// Bill readers of a session (`session.bills()`).
#[derive(Clone, Copy, Debug)]
pub struct Bills<'a>(pub(crate) &'a Session);

impl Session {
    /// Open bills of the open company.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    fn vendor() -> ContactId {
        ContactId::parse(VENDOR).expect("id")
    }

    #[tokio::test]
    async fn summary_and_payable_read_exact_strings() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bills/vendor-summary"))
            .respond_with(json(
                200,
                r#"{"summary":{"unpaidCount":1,"totalOwed":"1286.4000","overdueCount":0,
                    "totalOverdue":"0","availableCreditsCount":0,"totalAvailableCredits":"0"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bills/payable"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"bills":[{{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnNumber":null,
                    "contactId":"{VENDOR}","contactName":"Canary Supply","txnDate":"2026-10-06",
                    "dueDate":"2026-11-05","vendorInvoiceNumber":"F-77","total":"1286.4000",
                    "amountPaid":"0.0000","creditsApplied":"0.0000","balanceDue":"1286.4000",
                    "billStatus":"unpaid","paymentTerms":"net_30","memo":"canary memo","daysOverdue":0}}],
                    "credits":[]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let sum = s.bills().vendor_summary(vendor()).await.expect("summary");
        assert_eq!(sum.unpaid_count, 1);
        assert_eq!(sum.total_owed.to_wire(), "1286.4000");
        assert_eq!(sum.total_overdue, Money::ZERO);
        let shown = format!("{sum:?}");
        assert!(!shown.contains("1286"), "{shown}");

        let bills = s.bills().payable(vendor()).await.expect("payable");
        assert_eq!(bills.len(), 1);
        assert_eq!(bills[0].amount_due.to_wire(), "1286.4000");
        assert_eq!(bills[0].due, CalendarDate::from_ymd(2026, 11, 5).ok());
        assert_eq!(bills[0].vendor_invoice_number.as_deref(), Some("F-77"));
        let shown = format!("{:?}", bills[0]);
        assert!(
            !shown.contains("1286") && !shown.contains("canary"),
            "{shown}"
        );
        for r in requests(&server, "GET").await {
            assert_eq!(query(&r), format!("contactId={VENDOR}"));
        }
    }

    #[tokio::test]
    async fn a_number_where_money_belongs_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bills/vendor-summary"))
            .respond_with(json(
                200,
                r#"{"summary":{"unpaidCount":1,"totalOwed":1286.4,"overdueCount":0,"totalOverdue":"0"}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e = s.bills().vendor_summary(vendor()).await.expect_err("float");
        assert_eq!(
            e,
            ApiError::Decode {
                what: "vendor summary"
            }
        );
    }
}
