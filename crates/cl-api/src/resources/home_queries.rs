//! Home's counts that work on every engine (P16 work package 6a, D9): integers from
//! list queries, never amounts. Each answer's `total` (or a status) is the
//! only thing read, so no row amount is ever decoded here.
//!
//! - [`HomeQueries::bills_count`]: `GET /bills` with a status set and a due
//!   date (`total`).
//! - [`HomeQueries::payroll_sessions_count`]: unposted payroll imports.
//! - [`HomeQueries::reconcile_state`]: reconciliations left open, and the
//!   latest finished one, of the accounts named.
//! - [`HomeQueries::bank_statements_any`], [`HomeQueries::has_any_transaction`]:
//!   setup checklist ticks.
//! - [`HomeQueries::feed_pending_count`]: `pendingFeedCount` of
//!   `/dashboard/action-items` (an integer, so allowed before U-76; nothing
//!   else of that answer is decoded).

use std::fmt;

use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::dashboard::dashboard_query;
use crate::resources::operations::{
    BANK_STATEMENTS, BILLS_LIST, DASHBOARD_ACTIONS, Operation, PAYROLL_SESSIONS, RECONCILIATIONS,
    TXNS_LIST,
};
use crate::session::Session;

/// The most accounts [`HomeQueries::reconcile_state`] reads (one call each).
pub const RECONCILE_ACCOUNTS_MAX: usize = 50;

/// An open bill status (`billStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OpenBillStatus {
    /// Nothing paid yet.
    Unpaid,
    /// Partly paid.
    Partial,
    /// Past its due date (the engine also counts unpaid and partial bills
    /// whose due date has passed).
    Overdue,
}

impl OpenBillStatus {
    /// Every open status.
    pub const ALL: [OpenBillStatus; 3] = [
        OpenBillStatus::Unpaid,
        OpenBillStatus::Partial,
        OpenBillStatus::Overdue,
    ];

    /// The engine's name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            OpenBillStatus::Unpaid => "unpaid",
            OpenBillStatus::Partial => "partial",
            OpenBillStatus::Overdue => "overdue",
        }
    }
}

/// Which bills to count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BillQuery {
    /// Statuses (empty = any).
    pub status: Vec<OpenBillStatus>,
    /// Due on or before this date.
    pub due_on_or_before: Option<CalendarDate>,
    /// The engine's `overdueOnly` (due before the database's date, open).
    /// Home uses [`BillQuery::overdue`] instead, which follows the local
    /// date.
    pub overdue_only: bool,
}

impl BillQuery {
    /// Open bills overdue or due within `days` days of `today` (Q-169):
    /// due on or before `today + days`.
    ///
    /// # Errors
    /// `Local(DueWindow)` when the date leaves the calendar.
    pub fn due_within(today: CalendarDate, days: u16) -> Result<BillQuery, ApiError> {
        let last = today
            .add_days(i64::from(days))
            .map_err(|_| ApiError::Local(LocalValidation::DueWindow))?;
        Ok(BillQuery {
            status: OpenBillStatus::ALL.to_vec(),
            due_on_or_before: Some(last),
            overdue_only: false,
        })
    }

    /// Open bills due before `today` (the local date): due on or before
    /// yesterday.
    ///
    /// # Errors
    /// `Local(DueWindow)` when the date leaves the calendar.
    pub fn overdue(today: CalendarDate) -> Result<BillQuery, ApiError> {
        let yesterday = today
            .add_days(-1)
            .map_err(|_| ApiError::Local(LocalValidation::DueWindow))?;
        Ok(BillQuery {
            status: OpenBillStatus::ALL.to_vec(),
            due_on_or_before: Some(yesterday),
            overdue_only: false,
        })
    }

    /// The query pairs (`limit=1`: only `total` is read).
    #[must_use]
    pub fn query(&self) -> Vec<(String, String)> {
        let mut q = Vec::new();
        if !self.status.is_empty() {
            let set: Vec<&str> = self.status.iter().map(|s| s.as_wire()).collect();
            q.push(("billStatus".into(), set.join(",")));
        }
        if let Some(d) = self.due_on_or_before {
            q.push(("dueOnOrBefore".into(), d.to_wire()));
        }
        if self.overdue_only {
            q.push(("overdueOnly".into(), "true".into()));
        }
        q.push(("limit".into(), "1".into()));
        q
    }
}

/// An unposted payroll import status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PayrollStatus {
    /// File read, columns not mapped.
    Uploaded,
    /// Columns mapped.
    Mapped,
    /// Checked and ready to post.
    Validated,
}

impl PayrollStatus {
    /// The imports that still need the owner (not posted, failed or
    /// cancelled).
    pub const UNPOSTED: [PayrollStatus; 3] = [
        PayrollStatus::Uploaded,
        PayrollStatus::Mapped,
        PayrollStatus::Validated,
    ];

    /// The engine's name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            PayrollStatus::Uploaded => "uploaded",
            PayrollStatus::Mapped => "mapped",
            PayrollStatus::Validated => "validated",
        }
    }
}

/// The latest finished reconciliation among the accounts read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LastReconciled {
    /// The account.
    pub account: AccountId,
    /// Its statement date.
    pub statement_date: CalendarDate,
}

/// Reconciliations of the accounts read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileState {
    /// Accounts with a reconciliation started and not finished, in the
    /// order asked.
    pub in_progress: Vec<AccountId>,
    /// The finished reconciliation with the latest statement date.
    pub last_completed: Option<LastReconciled>,
}

#[derive(Deserialize)]
struct Total {
    total: u64,
}

fn total_of(body: &[u8], what: &'static str) -> Result<u64, ApiError> {
    serde_json::from_slice::<Total>(body)
        .map(|t| t.total)
        .map_err(|_| ApiError::Decode { what })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireReconciliation {
    #[serde(default)]
    status: Option<String>,
    statement_date: CalendarDate,
}

#[derive(Deserialize)]
struct WireReconciliations {
    reconciliations: Vec<WireReconciliation>,
}

/// Folds one account's reconciliation list into `state`.
pub(crate) fn add_reconciliations(
    state: &mut ReconcileState,
    account: AccountId,
    body: &[u8],
) -> Result<(), ApiError> {
    let w: WireReconciliations = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "reconciliations",
    })?;
    let mut open = false;
    for r in w.reconciliations {
        match r.status.as_deref() {
            Some("in_progress") | None => open = true,
            Some("complete") => {
                if state
                    .last_completed
                    .is_none_or(|l| r.statement_date > l.statement_date)
                {
                    state.last_completed = Some(LastReconciled {
                        account,
                        statement_date: r.statement_date,
                    });
                }
            }
            Some(_) => {}
        }
    }
    if open {
        state.in_progress.push(account);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeedCount {
    pending_feed_count: u64,
}

/// Reads `pendingFeedCount` and nothing else.
pub(crate) fn feed_count_from(body: &[u8]) -> Result<u64, ApiError> {
    serde_json::from_slice::<FeedCount>(body)
        .map(|c| c.pending_feed_count)
        .map_err(|_| ApiError::Decode {
            what: "pending feed count",
        })
}

/// Home's counts of a session (`session.home()`).
#[derive(Clone, Copy)]
pub struct HomeQueries<'a>(pub(crate) &'a Session);

impl fmt::Debug for HomeQueries<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("HomeQueries")
    }
}

impl Session {
    /// Home's counts for the open company.
    #[must_use]
    pub fn home(&self) -> HomeQueries<'_> {
        HomeQueries(self)
    }
}

impl HomeQueries<'_> {
    async fn total(
        &self,
        op: &Operation,
        query: Vec<(String, String)>,
        what: &'static str,
    ) -> Result<u64, ApiError> {
        let mut call = Call::at(Method::Get, op.route());
        call.query = query;
        let reply = self.0.send(op, call, None).await?;
        total_of(&reply.body, what)
    }

    /// The number of bills matching `q` (the engine's `total`).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn bills_count(&self, q: &BillQuery) -> Result<u64, ApiError> {
        self.total(&BILLS_LIST, q.query(), "bills total").await
    }

    /// Payroll imports in one of `statuses` (one call each, summed).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn payroll_sessions_count(
        &self,
        statuses: &[PayrollStatus],
    ) -> Result<u64, ApiError> {
        let mut n: u64 = 0;
        for s in statuses {
            let q = vec![
                ("status".into(), s.as_wire().into()),
                ("limit".into(), "1".into()),
            ];
            let t = self
                .total(&PAYROLL_SESSIONS, q, "payroll sessions total")
                .await?;
            n = n.saturating_add(t);
        }
        Ok(n)
    }

    /// Reconciliations of `accounts` (the bank and card accounts Home
    /// shows; at most [`RECONCILE_ACCOUNTS_MAX`], one call each).
    ///
    /// # Errors
    /// `Local(PageLimit)` for more accounts (nothing sent); `Decode`; any
    /// [`ApiError`].
    pub async fn reconcile_state(
        &self,
        accounts: &[AccountId],
    ) -> Result<ReconcileState, ApiError> {
        if accounts.len() > RECONCILE_ACCOUNTS_MAX {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        let mut state = ReconcileState::default();
        for a in accounts {
            let mut call = Call::at(Method::Get, RECONCILIATIONS.route());
            call.query = vec![("account_id".into(), a.to_string())];
            let reply = self.0.send(&RECONCILIATIONS, call, None).await?;
            add_reconciliations(&mut state, *a, &reply.body)?;
        }
        Ok(state)
    }

    /// Whether any bank statement was imported.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn bank_statements_any(&self) -> Result<bool, ApiError> {
        let q = vec![("limit".into(), "1".into())];
        Ok(self
            .total(&BANK_STATEMENTS, q, "bank statements total")
            .await?
            > 0)
    }

    /// Whether the company has any transaction.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn has_any_transaction(&self) -> Result<bool, ApiError> {
        let q = vec![("limit".into(), "1".into())];
        Ok(self.total(&TXNS_LIST, q, "transactions total").await? > 0)
    }

    /// Bank-feed items waiting (`pendingFeedCount`; sends `exact=1` and
    /// `asOf` like every dashboard call).
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn feed_pending_count(&self, as_of: CalendarDate) -> Result<u64, ApiError> {
        let mut call = Call::at(Method::Get, DASHBOARD_ACTIONS.route());
        call.query = dashboard_query(as_of);
        let reply = self.0.send(&DASHBOARD_ACTIONS, call, None).await?;
        feed_count_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "home_queries_tests.rs"]
mod tests;
