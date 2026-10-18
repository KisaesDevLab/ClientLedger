//! Invoices and bills, the minimum P08 needs for guided opening balances
//! (open invoices and unpaid bills as of the start date, Q-45). The full
//! forms arrive in P17 (Enter Bill) and P20 (Create Invoice).
//!
//! Money fields are exact (`Money`, `Quantity`, `Rate`) and travel as
//! strings; the engine computes totals and tax.

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Rate};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{BILLS_CREATE, INVOICES_CREATE};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};

/// One invoice line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvoiceLine {
    /// Income (or, for opening balances, the opening-balances equity)
    /// account.
    pub account_id: AccountId,
    /// Description.
    pub description: Option<String>,
    /// Quantity.
    pub quantity: Quantity,
    /// Unit price.
    pub unit_price: Money,
    /// Taxable.
    pub is_taxable: bool,
    /// Tax rate (0 when not taxable).
    pub tax_rate: Rate,
}

impl Serialize for InvoiceLine {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("InvoiceLine", 6)?;
        st.serialize_field("accountId", &self.account_id)?;
        if let Some(d) = &self.description {
            st.serialize_field("description", d)?;
        }
        st.serialize_field("quantity", &self.quantity.to_wire())?;
        st.serialize_field("unitPrice", &self.unit_price.to_wire())?;
        st.serialize_field("isTaxable", &self.is_taxable)?;
        st.serialize_field("taxRate", &self.tax_rate.to_wire())?;
        st.end()
    }
}

/// `POST /api/v1/invoices`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInvoice {
    /// Invoice date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<CalendarDate>,
    /// The client.
    pub contact_id: ContactId,
    /// Lines (1-500).
    pub lines: Vec<InvoiceLine>,
    /// Memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Caller-supplied number (the counter is left alone).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn_number: Option<String>,
}

/// One bill line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillLine {
    /// Expense (or opening-balances equity) account.
    pub account_id: AccountId,
    /// Description.
    pub description: Option<String>,
    /// Amount.
    pub amount: Money,
}

impl Serialize for BillLine {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("BillLine", 3)?;
        st.serialize_field("accountId", &self.account_id)?;
        if let Some(d) = &self.description {
            st.serialize_field("description", d)?;
        }
        st.serialize_field("amount", &self.amount.to_wire())?;
        st.end()
    }
}

/// `POST /api/v1/bills`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewBill {
    /// The vendor.
    pub contact_id: ContactId,
    /// Bill date.
    pub txn_date: CalendarDate,
    /// Due date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<CalendarDate>,
    /// The vendor's reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor_invoice_number: Option<String>,
    /// Memo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// Lines (1-500).
    pub lines: Vec<BillLine>,
}

/// What a create answered (the transaction row's identity and total).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Posted {
    /// The transaction.
    pub id: TransactionId,
    /// Its number, when assigned.
    #[serde(default)]
    pub txn_number: Option<String>,
    /// Its total as the engine computed it.
    #[serde(default)]
    pub total: Option<Money>,
}

/// Maximum lines per invoice or bill.
pub const MAX_LINES: usize = 500;

fn check_lines(n: usize) -> Result<(), ApiError> {
    if n == 0 {
        Err(ApiError::Local(LocalValidation::TooFewLines))
    } else if n > MAX_LINES {
        Err(ApiError::Local(LocalValidation::TooManyLines))
    } else {
        Ok(())
    }
}

impl Session {
    /// Creates an invoice.
    ///
    /// # Errors
    /// `Local(TooFewLines | TooManyLines | NonPositiveAmount)` (nothing
    /// sent); `ClosedPeriod`; `Validation`; any [`ApiError`].
    pub async fn create_invoice(
        &self,
        new: &NewInvoice,
        key: IdempotencyKey,
    ) -> Result<Posted, ApiError> {
        #[derive(Deserialize)]
        struct W {
            invoice: Posted,
        }
        check_lines(new.lines.len())?;
        if new
            .lines
            .iter()
            .any(|l| l.unit_price.is_negative() || l.quantity.is_negative())
        {
            return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
        }
        let mut call = Call::at(Method::Post, INVOICES_CREATE.route()).json_body(json_body(new)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&INVOICES_CREATE, call, None).await?;
        decode::<W>(&reply, "invoice")
            .map(|w| w.invoice)
            .map_err(saved_unreadable)
    }

    /// Creates a bill.
    ///
    /// # Errors
    /// `Local(TooFewLines | TooManyLines | NonPositiveAmount)` (nothing
    /// sent); `ClosedPeriod`; `Validation`; any [`ApiError`].
    pub async fn create_bill(
        &self,
        new: &NewBill,
        key: IdempotencyKey,
    ) -> Result<Posted, ApiError> {
        #[derive(Deserialize)]
        struct W {
            bill: Posted,
        }
        check_lines(new.lines.len())?;
        if new.lines.iter().any(|l| !l.amount.is_positive()) {
            return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
        }
        let mut call = Call::at(Method::Post, BILLS_CREATE.route()).json_body(json_body(new)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&BILLS_CREATE, call, None).await?;
        decode::<W>(&reply, "bill")
            .map(|w| w.bill)
            .map_err(saved_unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const ACCT: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const CONTACT: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    #[tokio::test]
    async fn invoice_and_bill_bodies_are_exact_strings() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/invoices"))
            .respond_with(json(
                201,
                r#"{"invoice":{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnNumber":"INV-1001","total":"1250.0000"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/bills"))
            .respond_with(json(
                201,
                r#"{"bill":{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","total":"300.0000"}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let inv = NewInvoice {
            txn_date: CalendarDate::from_ymd(2025, 12, 15).expect("d"),
            due_date: Some(CalendarDate::from_ymd(2026, 1, 14).expect("d")),
            contact_id: ContactId::parse(CONTACT).expect("id"),
            lines: vec![InvoiceLine {
                account_id: AccountId::parse(ACCT).expect("id"),
                description: Some("Opening balance".into()),
                quantity: Quantity::parse_wire("1").expect("q"),
                unit_price: Money::parse_wire("1250").expect("m"),
                is_taxable: false,
                tax_rate: Rate::parse_wire("0").expect("r"),
            }],
            memo: None,
            txn_number: Some("INV-1001".into()),
        };
        let p = s
            .create_invoice(&inv, IdempotencyKey::new())
            .await
            .expect("inv");
        assert_eq!(p.txn_number.as_deref(), Some("INV-1001"));
        assert_eq!(p.total.map(Money::to_wire).as_deref(), Some("1250.0000"));
        let bill = NewBill {
            contact_id: ContactId::parse(CONTACT).expect("id"),
            txn_date: CalendarDate::from_ymd(2025, 12, 20).expect("d"),
            due_date: None,
            vendor_invoice_number: Some("A-7".into()),
            memo: None,
            lines: vec![BillLine {
                account_id: AccountId::parse(ACCT).expect("id"),
                description: None,
                amount: Money::parse_wire("300").expect("m"),
            }],
        };
        s.create_bill(&bill, IdempotencyKey::new())
            .await
            .expect("bill");
        let posts = requests(&server, "POST").await;
        let b0: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(b0["lines"][0]["unitPrice"], "1250.0000");
        assert_eq!(b0["lines"][0]["quantity"], "1.0000");
        assert_eq!(b0["lines"][0]["taxRate"], "0.0000");
        assert_eq!(b0["dueDate"], "2026-01-14");
        let b1: Value = serde_json::from_slice(&posts[1].body).expect("json");
        assert_eq!(b1["lines"][0]["amount"], "300.0000");
        assert_eq!(b1["vendorInvoiceNumber"], "A-7");
        assert!(b1.get("dueDate").is_none());
    }

    #[tokio::test]
    async fn local_refusals_send_nothing() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let bill = NewBill {
            contact_id: ContactId::parse(CONTACT).expect("id"),
            txn_date: CalendarDate::from_ymd(2025, 12, 20).expect("d"),
            due_date: None,
            vendor_invoice_number: None,
            memo: None,
            lines: vec![],
        };
        let e = s
            .create_bill(&bill, IdempotencyKey::new())
            .await
            .expect_err("no lines");
        assert_eq!(e, ApiError::Local(LocalValidation::TooFewLines));
        let bill = NewBill {
            lines: vec![BillLine {
                account_id: AccountId::parse(ACCT).expect("id"),
                description: None,
                amount: Money::ZERO,
            }],
            ..bill
        };
        let e = s
            .create_bill(&bill, IdempotencyKey::new())
            .await
            .expect_err("zero");
        assert_eq!(e, ApiError::Local(LocalValidation::NonPositiveAmount));
        assert!(server.received_requests().await.expect("r").is_empty());
    }
}
