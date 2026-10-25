//! Bill payments, read-only (P17 WP2): the books side of the vendor
//! statement match (D4). Paying bills is P18.

use std::fmt;

use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::BILL_PAYMENTS_LIST;
use crate::resources::transactions::TxnStatus;
use crate::session::{Session, decode};
use crate::wire::PageRequest;

/// One bill payment (`GET /bill-payments` row).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillPaymentSummary {
    /// The payment.
    pub id: TransactionId,
    /// Book number.
    #[serde(default, rename = "txnNumber")]
    pub number: Option<String>,
    /// Check number, for a check.
    #[serde(default)]
    pub check_number: Option<u64>,
    /// ACH trace or confirmation number (owner data).
    #[serde(default)]
    pub reference_number: Option<String>,
    /// Date.
    pub txn_date: CalendarDate,
    /// The vendor.
    #[serde(default)]
    pub contact_id: Option<ContactId>,
    /// Amount paid.
    #[serde(default)]
    pub total: Option<Money>,
    /// Posted or void (the statement match keeps posted ones).
    pub status: TxnStatus,
}

impl fmt::Debug for BillPaymentSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BillPaymentSummary")
            .field("id", &self.id)
            .field("txn_date", &self.txn_date)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Bill payment readers of a session (`session.bill_payments()`).
#[derive(Clone, Copy, Debug)]
pub struct BillPayments<'a>(pub(crate) &'a Session);

impl Session {
    /// Bill payments of the open company (read-only).
    #[must_use]
    pub fn bill_payments(&self) -> BillPayments<'_> {
        BillPayments(self)
    }
}

impl BillPayments<'_> {
    /// One page of a vendor's bill payments dated within `start..=end`
    /// (either end open), newest first. The engine sends no total count: a
    /// page shorter than its limit is the last.
    ///
    /// # Errors
    /// `Decode` (a number where money belongs); any [`ApiError`].
    pub async fn list(
        &self,
        vendor: Option<ContactId>,
        start: Option<CalendarDate>,
        end: Option<CalendarDate>,
        page: PageRequest,
    ) -> Result<Vec<BillPaymentSummary>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            data: Vec<BillPaymentSummary>,
        }
        let mut call = Call::at(Method::Get, BILL_PAYMENTS_LIST.route());
        if let Some(v) = vendor {
            call.query.push(("contactId".into(), v.to_string()));
        }
        if let Some(d) = start {
            call.query.push(("startDate".into(), d.to_wire()));
        }
        if let Some(d) = end {
            call.query.push(("endDate".into(), d.to_wire()));
        }
        call.query.extend(page.query());
        let reply = self.0.send(&BILL_PAYMENTS_LIST, call, None).await?;
        decode::<W>(&reply, "bill payments").map(|w| w.data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    #[tokio::test]
    async fn rows_read_exact_and_filters_go_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bill-payments"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"data":[{{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnType":"bill_payment",
                    "txnNumber":null,"checkNumber":1042,"referenceNumber":"canary-ref","txnDate":"2026-10-07",
                    "contactId":"{VENDOR}","total":"148.0000","status":"posted","memo":"canary"}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let vendor = ContactId::parse(VENDOR).expect("id");
        let rows = s
            .bill_payments()
            .list(
                Some(vendor),
                CalendarDate::from_ymd(2026, 10, 1).ok(),
                CalendarDate::from_ymd(2026, 10, 31).ok(),
                PageRequest::new(50, 0).expect("page"),
            )
            .await
            .expect("list");
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].total.map(Money::to_wire).as_deref(),
            Some("148.0000")
        );
        assert_eq!(rows[0].check_number, Some(1042));
        assert_eq!(rows[0].status, TxnStatus::Posted);
        let shown = format!("{:?}", rows[0]);
        assert!(
            !shown.contains("148") && !shown.contains("canary"),
            "{shown}"
        );
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            format!("contactId={VENDOR}&startDate=2026-10-01&endDate=2026-10-31&limit=50&offset=0")
        );
    }

    #[tokio::test]
    async fn a_number_where_money_belongs_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/bill-payments"))
            .respond_with(json(
                200,
                r#"{"data":[{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnDate":"2026-10-07",
                "total":148.0,"status":"posted"}]}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let e = s
            .bill_payments()
            .list(None, None, None, PageRequest::first_max())
            .await
            .expect_err("float");
        assert_eq!(
            e,
            ApiError::Decode {
                what: "bill payments"
            }
        );
    }
}
