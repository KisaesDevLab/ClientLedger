//! Home's dashboard reads (P16 work package 6a, D9; U-76 `dashboard-exact-v1`).
//!
//! Every call sends `exact=1` and `asOf=<the company's local date>`, and
//! every money field is read by the strict string-only deserializer
//! ([`money_str_strict`]): a JSON number is a decode error, so a pre-U-76
//! float can never be read. Without the capability nothing is sent and the
//! call answers `Local(DashboardUnavailable)`; Home then shows a dash.
//!
//! The figures are the engine's (Non-negotiable 7): nothing here adds,
//! subtracts or re-derives a balance. `Debug` prints counts and ids only,
//! never an amount or a name.

use std::fmt;

use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::client::{Call, EngineReply, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{
    DASHBOARD_ACTIONS, DASHBOARD_CASH, DASHBOARD_PAYABLES, DASHBOARD_RECEIVABLES,
    DASHBOARD_SUMMARY, Operation,
};
use crate::session::Session;
use crate::wire::{Capabilities, money_str_strict};

/// The bills-due window the engine accepts (`dueWithinDays`, 1..=90).
pub const DUE_WITHIN_DAYS_MAX: u16 = 90;

/// One bank, cash or card account with the balance the engine stores.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CashAccount {
    /// The account.
    pub id: AccountId,
    /// Its name (owner data).
    pub name: String,
    /// Its number, when it has one.
    #[serde(default)]
    pub account_number: Option<String>,
    /// The engine's stored balance, exactly.
    #[serde(deserialize_with = "money_str_strict")]
    pub balance: Money,
}

impl fmt::Debug for CashAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashAccount")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// `GET /dashboard/cash-position`: active bank, cash and card accounts.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CashPosition {
    /// Bank, checking, savings, cash and petty cash accounts.
    pub bank_accounts: Vec<CashAccount>,
    /// Credit cards.
    pub credit_cards: Vec<CashAccount>,
    /// The engine's total of the bank accounts.
    #[serde(deserialize_with = "money_str_strict")]
    pub total_bank: Money,
    /// The engine's total of the card balances (absolute values).
    #[serde(rename = "totalCC", deserialize_with = "money_str_strict")]
    pub total_cards: Money,
}

impl fmt::Debug for CashPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashPosition")
            .field("bank_accounts", &self.bank_accounts.len())
            .field("credit_cards", &self.credit_cards.len())
            .finish_non_exhaustive()
    }
}

/// `GET /dashboard/receivables`: open and overdue invoices (one overdue
/// rule shared with the action items).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receivables {
    /// What clients owe in all.
    #[serde(deserialize_with = "money_str_strict")]
    pub total_outstanding: Money,
    /// Overdue invoices.
    pub overdue_count: u64,
    /// Their open amount.
    #[serde(deserialize_with = "money_str_strict")]
    pub overdue_amount: Money,
    /// Open invoices.
    pub invoice_count: u64,
}

impl fmt::Debug for Receivables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receivables")
            .field("overdue_count", &self.overdue_count)
            .field("invoice_count", &self.invoice_count)
            .finish_non_exhaustive()
    }
}

/// `GET /dashboard/payables`: open, overdue and soon-due bills.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payables {
    /// What is owed on open bills.
    #[serde(deserialize_with = "money_str_strict")]
    pub total_owed: Money,
    /// Open bills.
    pub bill_count: u64,
    /// Bills due before today.
    pub overdue_count: u64,
    /// Their open amount.
    #[serde(deserialize_with = "money_str_strict")]
    pub overdue_amount: Money,
    /// Bills due today through today + `due_within_days` (the engine's
    /// `dueThisWeekCount`).
    #[serde(rename = "dueThisWeekCount")]
    pub due_soon_count: u64,
    /// Their open amount (`dueThisWeekAmount`).
    #[serde(rename = "dueThisWeekAmount", deserialize_with = "money_str_strict")]
    pub due_soon_amount: Money,
    /// The window the engine used.
    pub due_within_days: u16,
    /// Vendor credits not yet used.
    pub credit_count: u64,
    /// Their amount.
    #[serde(deserialize_with = "money_str_strict")]
    pub credit_amount: Money,
    /// The Accounts Payable balance.
    #[serde(deserialize_with = "money_str_strict")]
    pub ap_balance: Money,
}

impl fmt::Debug for Payables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Payables")
            .field("bill_count", &self.bill_count)
            .field("overdue_count", &self.overdue_count)
            .field("due_soon_count", &self.due_soon_count)
            .field("due_within_days", &self.due_within_days)
            .finish_non_exhaustive()
    }
}

/// A bank account with no complete reconciliation in the 30 days before
/// today. The engine's `lastReconciled` is not read: it is a database
/// date serialized through a time zone, so it is not a reliable day.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StaleReconciliation {
    /// The account's name (owner data).
    pub account_name: String,
}

impl fmt::Debug for StaleReconciliation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StaleReconciliation")
            .finish_non_exhaustive()
    }
}

/// `GET /dashboard/action-items`: counts of things waiting on a person.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItems {
    /// Bank-feed items waiting (status pending).
    pub pending_feed_count: u64,
    /// Overdue invoices (the `/receivables` rule).
    pub overdue_invoice_count: u64,
    /// Bank accounts not reconciled lately.
    pub stale_reconciliations: Vec<StaleReconciliation>,
    /// Payments waiting to be deposited.
    pub pending_deposit_count: u64,
    /// Their amount.
    #[serde(deserialize_with = "money_str_strict")]
    pub pending_deposit_amount: Money,
    /// Checks waiting to print.
    pub print_queue_count: u64,
    /// Their amount.
    #[serde(deserialize_with = "money_str_strict")]
    pub print_queue_amount: Money,
}

impl fmt::Debug for ActionItems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ActionItems")
            .field("pending_feed_count", &self.pending_feed_count)
            .field("overdue_invoice_count", &self.overdue_invoice_count)
            .field("stale_reconciliations", &self.stale_reconciliations.len())
            .field("pending_deposit_count", &self.pending_deposit_count)
            .field("print_queue_count", &self.print_queue_count)
            .finish_non_exhaustive()
    }
}

/// `GET /dashboard/summary`: the panels Home uses in one request. A panel
/// that failed in the engine is `None` and its label is in `errors`. The
/// snapshot, trend, budget, banking-health, portal and work-queue panels are
/// not read (Home's chart uses the P&L report, D9), so their amounts are
/// never decoded.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    /// The date the engine used as today for every panel.
    pub as_of: CalendarDate,
    /// Bank and card balances.
    #[serde(default)]
    pub cash_position: Option<CashPosition>,
    /// Invoices.
    #[serde(default)]
    pub receivables: Option<Receivables>,
    /// Bills.
    #[serde(default)]
    pub payables: Option<Payables>,
    /// Counts waiting on a person.
    #[serde(default)]
    pub action_items: Option<ActionItems>,
    /// Labels of the panels that failed (engine text, English).
    #[serde(default)]
    pub errors: Vec<String>,
}

impl fmt::Debug for DashboardSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardSummary")
            .field("as_of", &self.as_of)
            .field("cash_position", &self.cash_position)
            .field("receivables", &self.receivables)
            .field("payables", &self.payables)
            .field("action_items", &self.action_items)
            .field("errors", &self.errors.len())
            .finish()
    }
}

fn decode<T: DeserializeOwned>(body: &[u8], what: &'static str) -> Result<T, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })
}

/// Reads a `/dashboard/cash-position` answer (strings only).
///
/// # Errors
/// `Decode` for another shape or a JSON number amount.
pub fn cash_position_from_json(body: &[u8]) -> Result<CashPosition, ApiError> {
    decode(body, "cash position")
}

/// Reads a `/dashboard/receivables` answer (strings only).
///
/// # Errors
/// `Decode` for another shape or a JSON number amount.
pub fn receivables_from_json(body: &[u8]) -> Result<Receivables, ApiError> {
    decode(body, "receivables")
}

/// Reads a `/dashboard/payables` answer (strings only).
///
/// # Errors
/// `Decode` for another shape or a JSON number amount.
pub fn payables_from_json(body: &[u8]) -> Result<Payables, ApiError> {
    decode(body, "payables")
}

/// Reads a `/dashboard/action-items` answer (strings only).
///
/// # Errors
/// `Decode` for another shape or a JSON number amount.
pub fn action_items_from_json(body: &[u8]) -> Result<ActionItems, ApiError> {
    decode(body, "action items")
}

/// Reads a `/dashboard/summary` answer (strings only in the panels read).
///
/// # Errors
/// `Decode` for another shape or a JSON number amount.
pub fn summary_from_json(body: &[u8]) -> Result<DashboardSummary, ApiError> {
    decode(body, "dashboard summary")
}

/// The `exact` and `asOf` pairs every dashboard call sends.
pub(crate) fn dashboard_query(as_of: CalendarDate) -> Vec<(String, String)> {
    vec![
        ("exact".into(), "1".into()),
        ("asOf".into(), as_of.to_wire()),
    ]
}

/// Dashboard reads of a session (`session.dashboard()`).
#[derive(Clone, Copy, Debug)]
pub struct Dashboard<'a>(pub(crate) &'a Session);

impl Session {
    /// The dashboard of the open company.
    #[must_use]
    pub fn dashboard(&self) -> Dashboard<'_> {
        Dashboard(self)
    }
}

impl Dashboard<'_> {
    /// Whether the engine reports dashboard money exactly.
    #[must_use]
    pub fn exact(&self) -> bool {
        self.0
            .client()
            .capabilities()
            .has(Capabilities::DASHBOARD_EXACT)
    }

    async fn get(
        &self,
        op: &Operation,
        as_of: CalendarDate,
        extra: Option<(String, String)>,
    ) -> Result<EngineReply, ApiError> {
        if !self.exact() {
            return Err(ApiError::Local(LocalValidation::DashboardUnavailable));
        }
        let mut call = Call::at(Method::Get, op.route());
        call.query = dashboard_query(as_of);
        call.query.extend(extra);
        self.0.send(op, call, None).await
    }

    /// Stored balances of the active bank, cash and card accounts.
    ///
    /// # Errors
    /// `Local(DashboardUnavailable)` (nothing sent); `Decode`; any
    /// [`ApiError`].
    pub async fn cash_position(&self, as_of: CalendarDate) -> Result<CashPosition, ApiError> {
        cash_position_from_json(&self.get(&DASHBOARD_CASH, as_of, None).await?.body)
    }

    /// Open and overdue invoices as of `as_of` (the local date).
    ///
    /// # Errors
    /// `Local(DashboardUnavailable)` (nothing sent); `Decode`; any
    /// [`ApiError`].
    pub async fn receivables(&self, as_of: CalendarDate) -> Result<Receivables, ApiError> {
        receivables_from_json(&self.get(&DASHBOARD_RECEIVABLES, as_of, None).await?.body)
    }

    /// Open, overdue and due-within-`due_within_days` bills (1..=90).
    ///
    /// # Errors
    /// `Local(DashboardUnavailable)`, `Local(DueWindow)` for a window
    /// outside 1..=90 (nothing sent); `Decode`; any [`ApiError`].
    pub async fn payables(
        &self,
        as_of: CalendarDate,
        due_within_days: u16,
    ) -> Result<Payables, ApiError> {
        let days = due_window(due_within_days)?;
        payables_from_json(&self.get(&DASHBOARD_PAYABLES, as_of, Some(days)).await?.body)
    }

    /// Counts of things waiting on a person.
    ///
    /// # Errors
    /// `Local(DashboardUnavailable)` (nothing sent); `Decode`; any
    /// [`ApiError`].
    pub async fn action_items(&self, as_of: CalendarDate) -> Result<ActionItems, ApiError> {
        action_items_from_json(&self.get(&DASHBOARD_ACTIONS, as_of, None).await?.body)
    }

    /// Home's panels in one request, every one with the same "today".
    ///
    /// # Errors
    /// `Local(DashboardUnavailable)`, `Local(DueWindow)` for a window
    /// outside 1..=90 (nothing sent); `Decode`; any [`ApiError`].
    pub async fn summary(
        &self,
        as_of: CalendarDate,
        due_within_days: u16,
    ) -> Result<DashboardSummary, ApiError> {
        let days = due_window(due_within_days)?;
        summary_from_json(&self.get(&DASHBOARD_SUMMARY, as_of, Some(days)).await?.body)
    }
}

/// The `dueWithinDays` pair, or `Local(DueWindow)` outside 1..=90.
fn due_window(days: u16) -> Result<(String, String), ApiError> {
    if (1..=DUE_WITHIN_DAYS_MAX).contains(&days) {
        Ok(("dueWithinDays".into(), days.to_string()))
    } else {
        Err(ApiError::Local(LocalValidation::DueWindow))
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
