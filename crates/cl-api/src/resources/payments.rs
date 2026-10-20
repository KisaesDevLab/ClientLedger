//! Payments waiting to be deposited (P14 lane A):
//! `GET /payments/pending-deposits?exact=1`.
//!
//! With `deposit-grouping-v1` the engine sends every amount as a decimal
//! string: each becomes exact `Money` ([`PendingAmount::Exact`]) and may be
//! deposited as a whole ([`PendingPayment::grouped`]). An older engine sends
//! JSON numbers; those are kept as their text for display only
//! ([`PendingAmount::Display`]) and have no path into `Money`, so they can
//! never be summed or sent (P14 plan §4). The Payments Clearing balance is
//! the engine's figure, never the sum of the items.

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::bank_postings::{DepositSource, GroupedPayment};
use crate::resources::operations::PENDING_DEPOSITS;
use crate::resources::register::{RegisterAmount, amount};
use crate::session::Session;

/// A pending payment's amount as the engine sent it.
pub type PendingAmount = RegisterAmount;

/// One payment (or daily-sales tender, or cash sale) waiting in Payments
/// Clearing. `Debug` prints ids, the kind and the date only.
#[derive(Clone)]
pub struct PendingPayment {
    /// The payment's transaction.
    pub source: TransactionId,
    /// The engine's transaction type (`customer_payment`, `cash_sale`,
    /// `daily_sales`, `journal_entry`, …) as sent.
    pub txn_type: String,
    /// Its date.
    pub date: CalendarDate,
    /// Received from (owner data).
    pub received_from: Option<String>,
    /// The payer (`deposit-grouping-v1`; older engines don't send it).
    pub contact: Option<ContactId>,
    /// Reference number (check number, confirmation).
    pub ref_no: Option<String>,
    /// Payment method as the engine names it (`check`, `cash`, …).
    pub method: Option<String>,
    /// Memo (owner data).
    pub memo: Option<String>,
    /// What it holds in Payments Clearing.
    pub amount: PendingAmount,
}

impl std::fmt::Debug for PendingPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingPayment")
            .field("source", &self.source)
            .field("txn_type", &self.txn_type)
            .field("date", &self.date)
            .field("exact", &self.amount.is_exact())
            .finish_non_exhaustive()
    }
}

impl PendingPayment {
    /// The exact amount, `None` when the engine sent a display-only number.
    #[must_use]
    pub fn exact(&self) -> Option<Money> {
        self.amount.exact()
    }

    /// This payment deposited as a whole: a source line on `clearing` (the
    /// tenant's Payments Clearing account) at exactly the engine's amount.
    /// `None` for a display-only amount: an amount that is not exact can't
    /// be deposited (P14 plan §4).
    #[must_use]
    pub fn grouped(&self, clearing: AccountId) -> Option<GroupedPayment> {
        Some(GroupedPayment {
            source: self.source,
            kind: DepositSource::Payment,
            account: clearing,
            amount: self.exact()?,
            contact: self.contact,
            memo: None,
        })
    }
}

/// The payments waiting to be deposited, and the engine's Payments
/// Clearing balance.
#[derive(Clone)]
pub struct PendingDeposits {
    /// Every amount (items and balance) is exact.
    pub exact: bool,
    /// The Payments Clearing balance as the engine reports it.
    pub clearing_balance: PendingAmount,
    /// Oldest first (the engine's order).
    pub items: Vec<PendingPayment>,
}

impl std::fmt::Debug for PendingDeposits {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingDeposits")
            .field("exact", &self.exact)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireItem {
    transaction_id: TransactionId,
    txn_type: String,
    date: CalendarDate,
    #[serde(default)]
    customer_name: Option<String>,
    #[serde(default)]
    contact_id: Option<ContactId>,
    #[serde(default)]
    ref_no: Option<String>,
    #[serde(default)]
    payment_method: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    amount: Box<RawValue>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePending {
    payments_clearing_balance: Box<RawValue>,
    items: Vec<WireItem>,
}

/// Reads a `pending-deposits` answer.
pub(crate) fn pending_from(body: &[u8]) -> Result<PendingDeposits, ApiError> {
    const WHAT: &str = "pending deposits";
    let w: WirePending =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let clearing_balance = amount(&w.payments_clearing_balance, WHAT)?;
    let mut items = Vec::with_capacity(w.items.len());
    for i in w.items {
        items.push(PendingPayment {
            source: i.transaction_id,
            txn_type: i.txn_type,
            date: i.date,
            received_from: i.customer_name,
            contact: i.contact_id,
            ref_no: i.ref_no,
            method: i.payment_method,
            memo: i.memo,
            amount: amount(&i.amount, WHAT)?,
        });
    }
    let exact = clearing_balance.is_exact() && items.iter().all(|i| i.amount.is_exact());
    Ok(PendingDeposits {
        exact,
        clearing_balance,
        items,
    })
}

/// Payment readers of a session (`session.payments()`).
#[derive(Clone, Copy, Debug)]
pub struct Payments<'a>(pub(crate) &'a Session);

impl Session {
    /// Payments of the open company.
    #[must_use]
    pub fn payments(&self) -> Payments<'_> {
        Payments(self)
    }
}

impl Payments<'_> {
    /// The payments waiting in Payments Clearing (always asked with
    /// `exact=1`; an engine without `deposit-grouping-v1` answers with
    /// display-only numbers).
    ///
    /// # Errors
    /// `Decode` for an unreadable answer (a `null` amount, a bad date); any
    /// [`ApiError`].
    pub async fn pending_deposits(&self) -> Result<PendingDeposits, ApiError> {
        let mut call = Call::at(Method::Get, PENDING_DEPOSITS.route());
        call.query = vec![("exact".into(), "1".into())];
        let reply = self.0.send(&PENDING_DEPOSITS, call, None).await?;
        pending_from(&reply.body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const FIXTURE: &str = include_str!("../../../../fixtures/engine/pending-deposits.json");
    const LEGACY: &str = include_str!("../../../../fixtures/engine/pending-deposits-legacy.json");

    fn clearing() -> AccountId {
        AccountId::parse("10150000-0000-4000-8000-000000000001").expect("id")
    }

    #[tokio::test]
    async fn exact_strings_become_money_and_group_whole() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/payments/pending-deposits"))
            .respond_with(json(200, FIXTURE))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let p = s.payments().pending_deposits().await.expect("pending");
        assert!(p.exact);
        assert_eq!(p.items.len(), 2);
        assert_eq!(
            p.clearing_balance.exact().map(Money::to_wire).as_deref(),
            Some("2092.5000")
        );
        let a = &p.items[0];
        assert_eq!(a.exact().map(Money::to_wire).as_deref(), Some("1250.0000"));
        assert_eq!(a.received_from.as_deref(), Some("Hollister Family Trust"));
        assert_eq!(a.ref_no.as_deref(), Some("5521"));
        assert!(a.contact.is_some());
        let g = a.grouped(clearing()).expect("exact");
        assert_eq!(g.source, a.source);
        assert_eq!(g.amount.to_wire(), "1250.0000");
        assert_eq!(g.kind, DepositSource::Payment);
        assert_eq!(g.account, clearing());
        // Asked with exact=1; no owner data in Debug.
        let got = requests(&server, "GET").await;
        assert_eq!(query(&got[0]), "exact=1");
        let shown = format!("{p:?} {a:?}");
        assert!(
            !shown.contains("1250") && !shown.contains("Hollister"),
            "{shown}"
        );
    }

    #[test]
    fn numbers_from_an_older_engine_are_display_only_and_never_grouped() {
        let p = pending_from(LEGACY.as_bytes()).expect("pending");
        assert!(!p.exact);
        assert_eq!(p.items.len(), 2);
        for i in &p.items {
            assert!(i.exact().is_none());
            assert!(
                i.grouped(clearing()).is_none(),
                "a display amount can't be deposited"
            );
            assert!(i.contact.is_none());
        }
        assert_eq!(p.items[1].amount.display(false).as_deref(), Some("842.50"));
        assert_eq!(
            p.clearing_balance.display(false).as_deref(),
            Some("2092.50")
        );
    }

    #[test]
    fn a_null_amount_is_unreadable_not_zero() {
        let body = br#"{"paymentsClearingBalance":"0","items":[{"transactionId":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnType":"customer_payment","date":"2026-11-02","amount":null}]}"#;
        assert!(matches!(pending_from(body), Err(ApiError::Decode { .. })));
        let empty = br#"{"paymentsClearingBalance":0,"items":[]}"#;
        let p = pending_from(empty).expect("empty");
        assert!(p.items.is_empty());
    }
}
