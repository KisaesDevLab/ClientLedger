//! Payments waiting to be deposited, deposits read back, and vendor
//! refunds (P14 lane A): `GET /payments/pending-deposits?exact=1`,
//! [`DepositRecord`] (from `GET /transactions/{id}`) and
//! [`VendorRefundDraft`] (a deposit with a vendor-credit source line,
//! `vendor-refund-v1`).
//!
//! With `deposit-grouping-v1` the engine sends every amount as a decimal
//! string: each becomes exact `Money` ([`PendingAmount::Exact`]) and may be
//! deposited as a whole ([`PendingPayment::grouped`]). An older engine sends
//! JSON numbers; those are kept as their text for display only
//! (`RegisterAmount::DisplayOnly`) and have no path into `Money`, so they can
//! never be summed or sent (P14 plan §4). The Payments Clearing balance is
//! the engine's figure, never the sum of the items.

use cl_core::ids::{AccountId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::bank_postings::{
    BankPosting, DepositCaps, DepositDraft, DepositSource, GroupedPayment,
};
use crate::resources::operations::PENDING_DEPOSITS;
use crate::resources::register::{RegisterAmount, amount};
use crate::resources::transactions::{LINE_ROLE_CASH_BACK, Transaction, TxnStatus, TxnType};
use crate::resources::versioning::{RecordVersion, Versioned};
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

// ---------------------------------------------------------------- record

/// A payment (or refunded vendor credit) a deposit took in whole.
#[derive(Clone, PartialEq, Eq)]
pub struct DepositedPayment {
    /// The payment's transaction.
    pub source: TransactionId,
    /// Payments Clearing (or Accounts Payable for a vendor refund).
    pub account: AccountId,
    /// The amount deposited (the engine's).
    pub amount: Money,
    /// Received from.
    pub contact: Option<ContactId>,
    /// Line memo (owner data).
    pub memo: Option<String>,
}

impl std::fmt::Debug for DepositedPayment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositedPayment")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// One other line of a saved deposit: money deposited (`amount` above
/// zero) or a fee taken out (`amount` below zero, as the owner typed it).
#[derive(Clone, PartialEq, Eq)]
pub struct DepositedLine {
    /// The account.
    pub account: AccountId,
    /// Signed: negative for a fee (the engine's debit).
    pub amount: Money,
    /// Received from.
    pub contact: Option<ContactId>,
    /// Memo (owner data).
    pub memo: Option<String>,
    /// Tag.
    pub tag: Option<TagId>,
}

impl std::fmt::Debug for DepositedLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositedLine")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// The cash kept back from a saved deposit.
#[derive(Clone, PartialEq, Eq)]
pub struct DepositedCashBack {
    /// Where the cash went.
    pub account: AccountId,
    /// How much.
    pub amount: Money,
    /// Memo (owner data).
    pub memo: Option<String>,
}

impl std::fmt::Debug for DepositedCashBack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositedCashBack")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A saved deposit as the engine stores it, split into what Make Deposit
/// shows. Every figure is the engine's; `total` is the bank line.
#[derive(Clone, PartialEq, Eq)]
pub struct DepositRecord {
    /// The ledger record.
    pub txn: Transaction,
    /// The account the money went into.
    pub account: AccountId,
    /// What the bank got (the bank line's debit; zero for a void deposit).
    pub total: Money,
    /// Payments and vendor credits taken in whole (`sourceTransactionId`).
    pub payments: Vec<DepositedPayment>,
    /// Other lines, fees negative.
    pub lines: Vec<DepositedLine>,
    /// Cash back (`lineRole` `cash_back`).
    pub cash_back: Option<DepositedCashBack>,
    /// The record has exactly the shape Make Deposit writes: one bank
    /// debit on `account`, every other line one-sided and on another
    /// account. Anything else is shown read-only.
    pub exact: bool,
}

impl std::fmt::Debug for DepositRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DepositRecord")
            .field("id", &self.txn.id)
            .field("account", &self.account)
            .field("payments", &self.payments)
            .field("lines", &self.lines.len())
            .field("cash_back", &self.cash_back.is_some())
            .field("exact", &self.exact)
            .finish_non_exhaustive()
    }
}

impl DepositRecord {
    /// Splits a deposit's journal for the account it went into.
    ///
    /// # Errors
    /// `Decode` when the record is not a deposit, or a posted one has no
    /// debit on `account`.
    pub fn from_transaction(
        txn: Transaction,
        account: AccountId,
    ) -> Result<DepositRecord, ApiError> {
        const WHAT: &str = "deposit";
        if txn.txn_type != TxnType::Deposit {
            return Err(ApiError::Decode { what: WHAT });
        }
        let bank: Vec<usize> = txn
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| {
                l.account_id == account
                    && l.source_transaction_id.is_none()
                    && l.line_role.is_none()
                    && l.debit.is_positive()
            })
            .map(|(i, _)| i)
            .collect();
        let Some(&bank_line) = bank.first() else {
            // A void deposit keeps its lines at zero: still shown.
            if txn.status == TxnStatus::Void {
                return Ok(DepositRecord {
                    account,
                    total: Money::ZERO,
                    payments: Vec::new(),
                    lines: Vec::new(),
                    cash_back: None,
                    exact: false,
                    txn,
                });
            }
            return Err(ApiError::Decode { what: WHAT });
        };
        let one_sided = |d: Money, c: Money| {
            (d.is_positive() && c.is_zero()) || (c.is_positive() && d.is_zero())
        };
        let mut exact = bank.len() == 1;
        let total = txn.lines[bank_line].debit;
        let mut payments = Vec::new();
        let mut lines = Vec::new();
        let mut cash_back = None;
        for (i, l) in txn.lines.iter().enumerate() {
            if i == bank_line {
                exact &= l.credit.is_zero();
                continue;
            }
            exact &= one_sided(l.debit, l.credit) && l.account_id != account;
            if let Some(source) = l.source_transaction_id {
                exact &= l.debit.is_zero();
                payments.push(DepositedPayment {
                    source,
                    account: l.account_id,
                    amount: l.credit,
                    contact: l.contact_id,
                    memo: l.description.clone(),
                });
            } else if l.line_role.as_deref() == Some(LINE_ROLE_CASH_BACK) {
                exact &= cash_back.is_none() && l.credit.is_zero();
                cash_back = Some(DepositedCashBack {
                    account: l.account_id,
                    amount: l.debit,
                    memo: l.description.clone(),
                });
            } else {
                // A credit is money deposited; a debit is a fee, signed
                // negative as the owner types it (one side is zero).
                let amount = l
                    .credit
                    .checked_sub(l.debit)
                    .map_err(|_| ApiError::Decode { what: WHAT })?;
                lines.push(DepositedLine {
                    account: l.account_id,
                    amount,
                    contact: l.contact_id,
                    memo: l.description.clone(),
                    tag: l.tag_id,
                });
            }
        }
        Ok(DepositRecord {
            txn,
            account,
            total,
            payments,
            lines,
            cash_back,
            exact,
        })
    }

    /// The deposit took payments (or a vendor credit) in whole: it is
    /// shown read-only except Void (Q-P14g).
    #[must_use]
    pub fn is_grouped(&self) -> bool {
        !self.payments.is_empty()
    }
}

// ---------------------------------------------------------------- refund

/// A refund from a vendor against one of its vendor credits (Q-87 option
/// 2, `vendor-refund-v1`): a deposit whose source line credits Accounts
/// Payable for the vendor. `amount` is at most the credit's open balance
/// (the engine checks it: 409 `VENDOR_CREDIT_EXCEEDED`).
#[derive(Clone, PartialEq, Eq)]
pub struct VendorRefundDraft {
    /// The vendor.
    pub vendor: ContactId,
    /// The account the refund goes into.
    pub deposit_to: AccountId,
    /// The tenant's Accounts Payable account.
    pub payables: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// The refund (the owner's typed amount, greater than zero).
    pub amount: Money,
    /// The vendor credit it is applied to.
    pub applies_to: TransactionId,
    /// Memo (owner data).
    pub memo: Option<String>,
}

impl std::fmt::Debug for VendorRefundDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VendorRefundDraft")
            .field("vendor", &self.vendor)
            .field("deposit_to", &self.deposit_to)
            .field("date", &self.date)
            .field("applies_to", &self.applies_to)
            .finish_non_exhaustive()
    }
}

impl VendorRefundDraft {
    /// The refund as a deposit draft.
    #[must_use]
    pub fn to_deposit(&self) -> DepositDraft {
        let mut d = DepositDraft::plain(self.deposit_to, self.date, self.memo.clone(), Vec::new());
        d.payments.push(GroupedPayment {
            source: self.applies_to,
            kind: DepositSource::VendorCredit,
            account: self.payables,
            amount: self.amount,
            contact: Some(self.vendor),
            memo: None,
        });
        d
    }

    /// The checked posting, ready for `create_bank_posting`.
    ///
    /// # Errors
    /// `VendorRefundUnavailable` without the capability, or any refusal of
    /// [`BankPosting::deposit_with`]; nothing is sent.
    pub fn posting(&self, caps: DepositCaps) -> Result<BankPosting, LocalValidation> {
        BankPosting::deposit_with(self.to_deposit(), caps)
    }
}

impl Payments<'_> {
    /// A saved deposit into `account`, split for Make Deposit, with its
    /// version (exact amounts from `GET /transactions/{id}`).
    ///
    /// # Errors
    /// `Decode` when it is not a deposit into `account`; any [`ApiError`].
    pub async fn deposit_record(
        &self,
        id: TransactionId,
        account: AccountId,
    ) -> Result<(DepositRecord, RecordVersion), ApiError> {
        let Versioned { record, version } = self.0.transactions().get(id).await?;
        Ok((DepositRecord::from_transaction(record, account)?, version))
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

    const FEES: &str = include_str!("../../../../fixtures/engine/deposit-grouped-fees.json");
    const REFUND: &str = include_str!("../../../../fixtures/engine/deposit-grouped-refund.json");

    fn txn(body: &str) -> Transaction {
        #[derive(Deserialize)]
        struct W {
            transaction: Transaction,
        }
        serde_json::from_str::<W>(body).expect("txn").transaction
    }

    fn id<T: std::str::FromStr>(s: &str) -> T
    where
        T::Err: std::fmt::Debug,
    {
        s.parse().expect("id")
    }

    #[test]
    fn a_saved_deposit_splits_into_payments_lines_fees_and_cash_back() {
        let bank: AccountId = AccountId::parse("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
        let r = DepositRecord::from_transaction(txn(FEES), bank).expect("record");
        assert!(r.exact);
        assert!(r.is_grouped());
        assert_eq!(
            r.total.to_wire(),
            "1487.5000",
            "the bank line, the engine's"
        );
        assert_eq!(r.payments.len(), 1);
        assert_eq!(r.payments[0].amount.to_wire(), "1250.0000");
        assert_eq!(r.lines.len(), 2);
        assert_eq!(r.lines[0].amount.to_wire(), "300.0000");
        assert_eq!(
            r.lines[1].amount.to_wire(),
            "-12.5000",
            "a fee reads negative"
        );
        let cb = r.cash_back.as_ref().expect("cash back");
        assert_eq!(cb.amount.to_wire(), "50.0000");
        // The parts add up to the bank line exactly.
        let parts = Money::sum(
            r.payments
                .iter()
                .map(|p| p.amount)
                .chain(r.lines.iter().map(|l| l.amount)),
        )
        .and_then(|s| s.checked_sub(cb.amount))
        .expect("sum");
        assert_eq!(parts, r.total);
        let shown = format!("{r:?}");
        assert!(
            !shown.contains("1487") && !shown.contains("Plant"),
            "{shown}"
        );
        // Another account's view is not this deposit.
        let other: AccountId = id("2e0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01");
        assert!(DepositRecord::from_transaction(txn(FEES), other).is_err());
    }

    #[test]
    fn a_vendor_refund_is_a_deposit_with_a_payables_source_line() {
        let refund = VendorRefundDraft {
            vendor: id("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09"),
            deposit_to: id("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"),
            payables: id("2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"),
            date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
            amount: Money::parse_wire("40.00").expect("m"),
            applies_to: id("7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09"),
            memo: Some("Returned bags".into()),
        };
        let d = refund.to_deposit();
        assert_eq!(d.payments.len(), 1);
        assert_eq!(d.payments[0].kind, DepositSource::VendorCredit);
        assert_eq!(d.payments[0].contact, Some(refund.vendor));
        assert_eq!(d.total(), Some(refund.amount));
        assert_eq!(
            refund.posting(DepositCaps::default()).err(),
            Some(LocalValidation::VendorRefundUnavailable),
            "nothing is sent without vendor-refund-v1"
        );
        let caps = DepositCaps {
            vendor_refund: true,
            ..DepositCaps::default()
        };
        assert!(refund.posting(caps).is_ok());
        let zero = VendorRefundDraft {
            amount: Money::ZERO,
            ..refund.clone()
        };
        assert_eq!(
            zero.posting(caps).err(),
            Some(LocalValidation::NonPositiveAmount)
        );
        assert!(!format!("{refund:?}").contains("Returned"));
        // The golden refund deposit reads back as one grouped part.
        let r = DepositRecord::from_transaction(txn(REFUND), refund.deposit_to).expect("record");
        assert!(r.is_grouped() && r.exact);
        assert_eq!(r.total.to_wire(), "40.0000");
        assert_eq!(r.payments[0].amount, r.total);
    }
}
