//! Invoices and bills: the request bodies P08 guided opening balances use
//! (open invoices and unpaid bills as of the start date, Q-45) and the
//! invoice line and body P20 Create Invoice sends (`resources::invoices`).
//!
//! Money fields are exact (`Money`, `Quantity`, `Rate`) and travel as
//! strings; the engine computes every line amount, the tax and the totals
//! (U-122, Q-P20n). Nothing here adds up an invoice.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, JournalLineId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Rate};
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::invoices::SignedPercent;
use crate::resources::operations::{BILLS_CREATE, INVOICES_CREATE};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};

/// One invoice line as the owner typed it. Only what is set goes out.
///
/// - A line names an income account, an item (which supplies the account
///   and the taxable flag, `invoice-fields-v1`), or both.
/// - `unit_price` `None` lets the engine price the line from the item and
///   the client's price level (`invoice-pricing-v1`, U-129; refused locally
///   before it). Quantity and price may be negative (a return, a discount
///   typed as a line).
/// - `tax_rate` stays `None` in the desktop app: the engine applies the
///   company's rate (U-122).
#[derive(Clone, PartialEq, Eq)]
pub struct InvoiceLine {
    /// Income account (for opening balances, the opening-balances equity
    /// account). `None` takes the item's.
    pub account_id: Option<AccountId>,
    /// The item (`invoice-fields-v1`).
    pub item_id: Option<ItemId>,
    /// Description, may span several lines (owner data; at most 4000
    /// characters).
    pub description: Option<String>,
    /// Service date (`invoice-fields-v1`).
    pub service_date: Option<CalendarDate>,
    /// Quantity.
    pub quantity: Quantity,
    /// Unit price; `None` asks the engine to price it (U-129).
    pub unit_price: Option<Money>,
    /// Taxable; `None` takes the item's flag (false without an item).
    pub is_taxable: Option<bool>,
    /// The line's own tax rate; the desktop app sends `None`.
    pub tax_rate: Option<Rate>,
    /// Line tag.
    pub tag_id: Option<TagId>,
    /// The billable cost this line invoices (U-128, `billable-costs-v1`;
    /// contract per P20 plan §2, verify when E3 lands).
    pub billable_source_line_id: Option<JournalLineId>,
    /// Markup on that cost in percent (U-128; needs the source line).
    pub markup_percent: Option<SignedPercent>,
}

impl InvoiceLine {
    /// A line on an account with a typed price.
    #[must_use]
    pub fn account(account: AccountId, quantity: Quantity, unit_price: Money) -> InvoiceLine {
        InvoiceLine {
            account_id: Some(account),
            unit_price: Some(unit_price),
            ..InvoiceLine::blank(quantity)
        }
    }

    /// An item line; the price comes from `unit_price` when set later,
    /// else from the engine (U-129).
    #[must_use]
    pub fn item(item: ItemId, quantity: Quantity) -> InvoiceLine {
        InvoiceLine {
            item_id: Some(item),
            ..InvoiceLine::blank(quantity)
        }
    }

    fn blank(quantity: Quantity) -> InvoiceLine {
        InvoiceLine {
            account_id: None,
            item_id: None,
            description: None,
            service_date: None,
            quantity,
            unit_price: None,
            is_taxable: None,
            tax_rate: None,
            tag_id: None,
            billable_source_line_id: None,
            markup_percent: None,
        }
    }
}

/// Ids, dates and flags only: descriptions and amounts are owner data.
impl fmt::Debug for InvoiceLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvoiceLine")
            .field("account_id", &self.account_id)
            .field("item_id", &self.item_id)
            .field("service_date", &self.service_date)
            .field("is_taxable", &self.is_taxable)
            .field("tag_id", &self.tag_id)
            .field("billable_source_line_id", &self.billable_source_line_id)
            .finish_non_exhaustive()
    }
}

impl Serialize for InvoiceLine {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if let Some(a) = &self.account_id {
            m.serialize_entry("accountId", a)?;
        }
        if let Some(i) = &self.item_id {
            m.serialize_entry("itemId", i)?;
        }
        if let Some(d) = &self.description {
            m.serialize_entry("description", d)?;
        }
        if let Some(d) = &self.service_date {
            m.serialize_entry("serviceDate", d)?;
        }
        m.serialize_entry("quantity", &self.quantity.to_wire())?;
        if let Some(p) = &self.unit_price {
            m.serialize_entry("unitPrice", &p.to_wire())?;
        }
        if let Some(t) = &self.is_taxable {
            m.serialize_entry("isTaxable", t)?;
        }
        if let Some(r) = &self.tax_rate {
            m.serialize_entry("taxRate", &r.to_wire())?;
        }
        if let Some(t) = &self.tag_id {
            m.serialize_entry("tagId", t)?;
        }
        if let Some(l) = &self.billable_source_line_id {
            m.serialize_entry("billableSourceLineId", l)?;
        }
        if let Some(p) = &self.markup_percent {
            m.serialize_entry("markupPercent", &p.to_wire())?;
        }
        m.end()
    }
}

/// `POST /api/v1/invoices` (and `PUT /invoices/{id}`, `POST
/// /invoices/preview`). Build it with [`NewInvoice::new`] and struct update
/// syntax; only what is set goes out.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewInvoice {
    /// Invoice date.
    pub txn_date: CalendarDate,
    /// Due date typed by the owner (wins over the terms); `None` lets the
    /// engine compute it (`invoice-terms-v1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_date: Option<CalendarDate>,
    /// The client.
    pub contact_id: ContactId,
    /// Terms code from `GET /payment-terms`; `None`: the invoice's own on
    /// an edit, else the client's, else the company's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_terms: Option<String>,
    /// Days, for `custom` terms only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_days: Option<u32>,
    /// Lines (1-500).
    pub lines: Vec<InvoiceLine>,
    /// Internal memo (registers and reports only, Q-P20k).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// The message printed on the invoice (`invoice-fields-v1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_message: Option<String>,
    /// The client's P.O. number (`invoice-fields-v1`; on an edit `""`
    /// clears it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub po_number: Option<String>,
    /// Header tags (`invoice-fields-v1`; sent replaces, `None` keeps).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_ids: Option<Vec<TagId>>,
    /// Leave $0 lines off the printed invoice (`invoice-fields-v1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_zero_lines: Option<bool>,
    /// Caller-supplied number (the counter is left alone).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txn_number: Option<String>,
    /// A number already in use posts with a warning instead of 409
    /// (`document-numbers-v1`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub warn_only: bool,
    /// Waiting to be printed (`forms-print-queue-v1`; `None` keeps it on
    /// an edit).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_later: Option<bool>,
}

impl NewInvoice {
    /// An invoice for `contact` dated `date`; every optional field unset.
    #[must_use]
    pub fn new(contact: ContactId, date: CalendarDate, lines: Vec<InvoiceLine>) -> NewInvoice {
        NewInvoice {
            txn_date: date,
            due_date: None,
            contact_id: contact,
            payment_terms: None,
            terms_days: None,
            lines,
            memo: None,
            customer_message: None,
            po_number: None,
            tag_ids: None,
            hide_zero_lines: None,
            txn_number: None,
            warn_only: false,
            print_later: None,
        }
    }
}

/// Ids, dates, codes and counts only.
impl fmt::Debug for NewInvoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewInvoice")
            .field("txn_date", &self.txn_date)
            .field("due_date", &self.due_date)
            .field("contact_id", &self.contact_id)
            .field("payment_terms", &self.payment_terms)
            .field("terms_days", &self.terms_days)
            .field("lines", &self.lines.len())
            .field("warn_only", &self.warn_only)
            .finish_non_exhaustive()
    }
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
    /// Creates an opening-balance invoice (P08): every line names its
    /// account and a price of zero or more. Create Invoice uses
    /// [`crate::Invoices::create`].
    ///
    /// # Errors
    /// `Local(TooFewLines | TooManyLines | NonPositiveAmount)` (nothing
    /// sent; a line without an account or a price is `NonPositiveAmount`
    /// too); `ClosedPeriod`; `Validation`; any [`ApiError`].
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
        if new.lines.iter().any(|l| {
            l.account_id.is_none()
                || l.unit_price.is_none_or(Money::is_negative)
                || l.quantity.is_negative()
        }) {
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
            due_date: Some(CalendarDate::from_ymd(2026, 1, 14).expect("d")),
            txn_number: Some("INV-1001".into()),
            ..NewInvoice::new(
                ContactId::parse(CONTACT).expect("id"),
                CalendarDate::from_ymd(2025, 12, 15).expect("d"),
                vec![InvoiceLine {
                    description: Some("Opening balance".into()),
                    is_taxable: Some(false),
                    ..InvoiceLine::account(
                        AccountId::parse(ACCT).expect("id"),
                        Quantity::parse_wire("1").expect("q"),
                        Money::parse_wire("1250").expect("m"),
                    )
                }],
            )
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
        assert_eq!(b0["lines"][0]["isTaxable"], false);
        // The engine applies the company's rate (U-122): none is sent.
        assert!(b0["lines"][0].get("taxRate").is_none());
        assert_eq!(b0["dueDate"], "2026-01-14");
        assert_eq!(
            b0.as_object().map(|o| {
                let mut k: Vec<_> = o.keys().cloned().collect();
                k.sort();
                k
            }),
            Some(vec![
                "contactId".to_string(),
                "dueDate".to_string(),
                "lines".to_string(),
                "txnDate".to_string(),
                "txnNumber".to_string()
            ]),
            "only what is set goes out"
        );
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
