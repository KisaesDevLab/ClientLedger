//! One entry point for the report window (P28 plan §0 step 3):
//! [`Reports::run`] sends a [`ReportRequest`] to its route family and turns
//! the answer into a [`ReportTable`].

use std::collections::HashMap;

use cl_core::CalendarDate;
use cl_core::report::{
    Adjusting, Cleared, Compare, DateRange, ReportDates, ReportRequest, ReportTable, Source,
};

use super::adapters;
use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{
    Operation, REPORT_BALANCE_SHEET, REPORT_EXPENSE_BY_CATEGORY, REPORT_GENERAL_LEDGER,
    REPORT_PROFIT_LOSS, REPORT_REVENUE_BY_CATEGORY, REPORT_TRIAL_BALANCE,
};
use crate::resources::reports::Reports;
use crate::resources::sales_tax_reports::LiabilityQuery;
use crate::resources::transactions::TxnType;

/// The engine's name of a transaction type (`journal_entry`, ...).
#[must_use]
pub fn txn_name(t: TxnType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The as-of date of a balance request (a range's end).
fn as_of(d: &ReportDates) -> Result<CalendarDate, ApiError> {
    d.span()
        .map(|r| r.to)
        .ok_or(ApiError::Local(LocalValidation::BadRange))
}

/// The period of a range request.
fn range(d: &ReportDates) -> Result<DateRange, ApiError> {
    match d {
        ReportDates::Range(r) => Ok(*r),
        ReportDates::AsOf(_) | ReportDates::Ranges(_) | ReportDates::None => {
            Err(ApiError::Local(LocalValidation::BadRange))
        }
    }
}

/// True when this build reads `source`: the routes exact before P28 (WP1)
/// and the `reports-exact-v1` statements and ledgers (WP2).
#[must_use]
pub const fn supported(source: Source) -> bool {
    matches!(
        source,
        Source::ArAgingSummary
            | Source::SalesTaxLiability
            | Source::InventoryValuation
            | Source::InventoryValuationDetail
            | Source::InventoryStockStatus
            | Source::ProfitLoss
            | Source::BalanceSheet
            | Source::TrialBalance
            | Source::GeneralLedger
            | Source::Journal
            | Source::RevenueByCategory
            | Source::ExpenseByCategory
    )
}

/// U-14: exact amounts.
pub const EXACT: &str = crate::wire::Capabilities::REPORTS_EXACT;
/// U-201: period, YTD, prior and compare columns, tag split.
pub const COLUMNS: &str = "reports-columns-v1";
/// U-202: the GL drill filters (client, amount, cleared, types).
pub const DRILL: &str = "reports-drill-v1";

/// The statements and ledgers read through `?exact=1`.
const fn exact_family(source: Source) -> bool {
    matches!(
        source,
        Source::ProfitLoss
            | Source::BalanceSheet
            | Source::TrialBalance
            | Source::GeneralLedger
            | Source::Journal
            | Source::RevenueByCategory
            | Source::ExpenseByCategory
    )
}

/// The capability `req` still needs on an engine that has those `has`
/// names, or `None` when it can run (Q-220). The window says "Needs an
/// engine update" for these rather than sending a parameter the engine
/// would ignore: PR-A answers `period_ranges` with the plain report.
#[must_use]
pub fn missing_capability(req: &ReportRequest, has: impl Fn(&str) -> bool) -> Option<&'static str> {
    if !exact_family(req.source) {
        return None;
    }
    if !has(EXACT) {
        return Some(EXACT);
    }
    let columns = matches!(req.dates, ReportDates::Ranges(_))
        || !req.extras.is_empty()
        || req.columns != Compare::None
        || req.tag_split.is_some();
    if columns {
        // U-201: columns on the P&L and Balance Sheet only (compare and YTD
        // on the P&L); the Trial Balance's and the ledger's tag split read
        // another shape this build doesn't read yet.
        let ok = match req.source {
            Source::ProfitLoss => true,
            Source::BalanceSheet => {
                req.columns == Compare::None
                    && !req.extras.contains(&cl_core::report::Extra::Ytd)
                    && !req.extras.contains(&cl_core::report::Extra::PctOfIncome)
            }
            _ => false,
        };
        if !ok || !has(COLUMNS) {
            return Some(COLUMNS);
        }
    }
    let f = &req.filters;
    let drill_filters = !f.contact_ids.is_empty()
        || f.amount_min.is_some()
        || f.amount_max.is_some()
        || f.cleared.is_some()
        || !f.txn_types.is_empty()
        || f.untagged_in_group.is_some();
    match req.source {
        Source::GeneralLedger if drill_filters && !has(DRILL) => Some(DRILL),
        // The Journal is U-202's.
        Source::Journal if !has(DRILL) => Some(DRILL),
        // The statements take a tag list and the adjusting lens only.
        Source::ProfitLoss | Source::BalanceSheet | Source::TrialBalance
            if drill_filters || !f.account_ids.is_empty() || !f.item_ids.is_empty() =>
        {
            Some(DRILL)
        }
        Source::RevenueByCategory | Source::ExpenseByCategory
            if drill_filters
                || !f.item_ids.is_empty()
                || f.adjusting.is_some_and(|a| a != Adjusting::Include) =>
        {
            Some(DRILL)
        }
        _ => None,
    }
}

fn join<T: std::fmt::Display>(ids: &[T]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The query of an exact statement or ledger request (U-14, plus U-202's
/// GL filters, which [`missing_capability`] lets through only when the
/// engine has them).
///
/// # Errors
/// `Local(BadRange)` for dates the route doesn't take.
pub fn exact_query(req: &ReportRequest) -> Result<Vec<(String, String)>, ApiError> {
    let mut q: Vec<(String, String)> = vec![("exact".into(), "1".into())];
    let mut push = |k: &str, v: String| q.push((k.to_string(), v));
    match req.source {
        Source::BalanceSheet => {
            push("as_of_date", as_of(&req.dates)?.to_wire());
            if let ReportDates::Ranges(v) = &req.dates {
                push("period_ranges", ranges_param(v));
            }
        }
        Source::TrialBalance => {
            // The engine's Trial Balance is a balance at its end date
            // (income and expenses from the fiscal year's start); the start
            // only has to come first.
            let to = as_of(&req.dates)?;
            let from = req.fiscal_year_start.filter(|f| *f <= to).unwrap_or(to);
            push("start_date", from.to_wire());
            push("end_date", to.to_wire());
        }
        Source::ProfitLoss => {
            let r = req
                .dates
                .span()
                .filter(|_| !matches!(req.dates, ReportDates::AsOf(_) | ReportDates::None))
                .ok_or(ApiError::Local(LocalValidation::BadRange))?;
            push("start_date", r.from.to_wire());
            push("end_date", r.to.to_wire());
            if let ReportDates::Ranges(v) = &req.dates {
                push("period_ranges", ranges_param(v));
            }
        }
        _ => {
            let r = range(&req.dates)?;
            push("start_date", r.from.to_wire());
            push("end_date", r.to.to_wire());
        }
    }
    if let Some(b) = req.basis {
        push("basis", b.as_str().to_string());
    }
    let f = &req.filters;
    if let Some(a) = f.adjusting
        && !matches!(
            req.source,
            Source::RevenueByCategory | Source::ExpenseByCategory
        )
    {
        push("adjusting", a.as_str().to_string());
    }
    if !f.tag_ids.is_empty() {
        push("tag_id", join(&f.tag_ids));
    }
    if matches!(
        req.source,
        Source::GeneralLedger
            | Source::Journal
            | Source::RevenueByCategory
            | Source::ExpenseByCategory
    ) && !f.account_ids.is_empty()
    {
        push("account_ids", join(&f.account_ids));
    }
    if matches!(req.source, Source::ProfitLoss | Source::BalanceSheet) {
        columns_query(req, &mut push);
    }
    if let Some(g) = f.untagged_in_group {
        push("tag_scope", "untagged_in_group".to_string());
        push("tag_group_id", g.to_string());
    }
    if req.source == Source::Journal {
        push("group_by", "transaction".to_string());
    }
    if matches!(req.source, Source::GeneralLedger | Source::Journal) {
        push("limit", adapters::ledger::SCREEN_LIMIT.to_string());
        if let Some(c) = f.contact_ids.first() {
            push("contact_id", c.to_string());
        }
        if let Some(a) = f.amount_min {
            push("amount_min", a.to_string());
        }
        if let Some(a) = f.amount_max {
            push("amount_max", a.to_string());
        }
        if let Some(c) = f.cleared {
            let v = match c {
                Cleared::Cleared => "cleared",
                Cleared::Uncleared => "uncleared",
                Cleared::Reconciled => "reconciled",
            };
            push("cleared", v.to_string());
        }
        if !f.txn_types.is_empty() {
            push("txn_types", f.txn_types.join(","));
        }
    }
    Ok(q)
}

/// `a..b,c..d`.
fn ranges_param(v: &[DateRange]) -> String {
    v.iter()
        .map(|r| format!("{}..{}", r.from.to_wire(), r.to.to_wire()))
        .collect::<Vec<_>>()
        .join(",")
}

/// U-201's column options (period ranges go with the dates).
fn columns_query(req: &ReportRequest, push: &mut impl FnMut(&str, String)) {
    use cl_core::report::Extra;
    if req.extras.contains(&Extra::Ytd) {
        push("ytd", "1".into());
        if let Some(f) = req.fiscal_year_start {
            push("fiscal_year_start", f.to_wire());
        }
    }
    // One prior column per period: the prior year wins when both are on.
    if req.extras.contains(&Extra::PriorYear) {
        push("prior", "year".into());
    } else if req.extras.contains(&Extra::PriorPeriod) {
        push("prior", "period".into());
    }
    if req.extras.contains(&Extra::PctOfIncome) {
        push("pct_of_income", "1".into());
    }
    match req.columns {
        Compare::None => {}
        Compare::ByContact => push("compare", "by_contact".into()),
        Compare::ByTag { tag_group_id } => {
            push("compare", "by_tag".into());
            push("tag_group_id", tag_group_id.to_string());
        }
    }
    if let Some(g) = req.tag_split {
        push("tag_split", "1".into());
        push("tag_group_id", g.to_string());
    }
}

/// The operation an exact source reads.
fn exact_operation(source: Source) -> Option<&'static Operation> {
    Some(match source {
        Source::ProfitLoss => &REPORT_PROFIT_LOSS,
        Source::BalanceSheet => &REPORT_BALANCE_SHEET,
        Source::TrialBalance => &REPORT_TRIAL_BALANCE,
        Source::GeneralLedger | Source::Journal => &REPORT_GENERAL_LEDGER,
        Source::RevenueByCategory => &REPORT_REVENUE_BY_CATEGORY,
        Source::ExpenseByCategory => &REPORT_EXPENSE_BY_CATEGORY,
        _ => return None,
    })
}

/// An exact answer as its table (pure: the tests and the demo use it on
/// recorded answers).
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing;
/// `Local(BadRange)` for dates the route doesn't take; `Local(Unsupported)`
/// for a source outside the exact family.
pub fn adapt_exact(req: &ReportRequest, v: &serde_json::Value) -> Result<ReportTable, ApiError> {
    use adapters::{categories, ledger, statements};
    match req.source {
        Source::ProfitLoss | Source::BalanceSheet if adapters::columns::is_columns(v) => {
            adapters::columns::table(v, req, req.source == Source::BalanceSheet)
        }
        Source::ProfitLoss => statements::profit_loss(v, req, range(&req.dates)?),
        Source::BalanceSheet => statements::balance_sheet(v, req, as_of(&req.dates)?),
        Source::TrialBalance => statements::trial_balance(v, req, as_of(&req.dates)?),
        Source::GeneralLedger => ledger::general_ledger(v, req, range(&req.dates)?),
        Source::Journal => adapters::journal::journal(v, req, range(&req.dates)?),
        Source::RevenueByCategory | Source::ExpenseByCategory => {
            categories::by_category(v, req, range(&req.dates)?)
        }
        _ => Err(ApiError::Local(LocalValidation::Unsupported)),
    }
}

impl Reports<'_> {
    /// Runs `req` and returns its table.
    ///
    /// # Errors
    /// `Local(Unsupported)` for a route family this build doesn't read yet
    /// or an engine without its capability; `Local(BadRange)` for dates the
    /// route doesn't take; [`ApiError::ReportShape`] when the answer lacks
    /// its exact fields; any [`ApiError`].
    pub async fn run(&self, req: &ReportRequest) -> Result<ReportTable, ApiError> {
        if let Some(op) = exact_operation(req.source) {
            let caps = self.0.client().capabilities();
            if missing_capability(req, |c| caps.has(c)).is_some() {
                return Err(ApiError::Local(LocalValidation::Unsupported));
            }
            let mut call = Call::at(Method::Get, op.route());
            call.query = exact_query(req)?;
            let reply = self.0.send(op, call, None).await?;
            let v: serde_json::Value = crate::session::decode(&reply, "report")?;
            return adapt_exact(req, &v);
        }
        match req.source {
            Source::ArAgingSummary => {
                let d = as_of(&req.dates)?;
                let r = self.ar_aging_summary(d).await?;
                Ok(adapters::ar_aging::summary(&r, d))
            }
            Source::SalesTaxLiability => {
                let span = range(&req.dates)?;
                let r = self
                    .sales_tax_liability(LiabilityQuery {
                        start_date: span.from,
                        end_date: span.to,
                        basis: None,
                        agency_id: None,
                    })
                    .await?;
                Ok(adapters::sales_tax::liability(&r, span))
            }
            Source::InventoryValuation => {
                let v = self.0.inventory().valuation(as_of(&req.dates)?).await?;
                Ok(adapters::inventory::valuation(&v))
            }
            Source::InventoryValuationDetail => {
                let span = range(&req.dates)?;
                let inv = self.0.inventory();
                let item = req.filters.item_ids.first().copied();
                let rows = inv.valuation_detail(span.from, span.to, item).await?;
                // The detail carries no names: the parts' names as of the end.
                let names: HashMap<_, _> = inv
                    .valuation(span.to)
                    .await?
                    .rows
                    .into_iter()
                    .filter_map(|r| r.name.map(|n| (r.item_id, n)))
                    .collect();
                Ok(adapters::inventory::valuation_detail(&rows, &names, span))
            }
            Source::InventoryStockStatus => {
                let d = as_of(&req.dates)?;
                let rows = self.0.inventory().stock_status(d).await?;
                Ok(adapters::inventory::stock_status(&rows, d))
            }
            _ => Err(ApiError::Local(LocalValidation::Unsupported)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_type_names_are_the_engine_s() {
        assert_eq!(txn_name(TxnType::JournalEntry), "journal_entry");
        assert_eq!(txn_name(TxnType::Bill), "bill");
    }

    #[test]
    fn this_build_reads_the_exact_statements_but_not_the_lists_yet() {
        assert!(supported(Source::ArAgingSummary));
        assert!(supported(Source::ProfitLoss));
        assert!(supported(Source::GeneralLedger));
        assert!(!supported(Source::InvoiceList), "WP3");
        assert!(!supported(Source::BudgetVsActual), "WP4");
    }

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn oct() -> DateRange {
        DateRange {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        }
    }

    #[test]
    fn options_the_engine_would_ignore_wait_for_their_update() {
        use cl_core::report::Extra;
        let pr_a = |c: &str| c == EXACT;
        let mut r = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(oct()));
        assert_eq!(missing_capability(&r, pr_a), None);
        assert_eq!(missing_capability(&r, |_| false), Some(EXACT));
        r.extras.insert(Extra::PctOfIncome);
        assert_eq!(missing_capability(&r, pr_a), Some(COLUMNS));
        r.extras.clear();
        r.dates = ReportDates::Ranges(vec![oct(), oct()]);
        assert_eq!(missing_capability(&r, pr_a), Some(COLUMNS));
        let mut g = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(oct()));
        g.filters.amount_min = Some(rust_decimal::Decimal::ONE);
        assert_eq!(missing_capability(&g, pr_a), Some(DRILL));
        assert_eq!(missing_capability(&g, |c| c == EXACT || c == DRILL), None);
        // Account and tag filters are PR-A's on the ledger, not on the P&L.
        g.filters.amount_min = None;
        g.filters.account_ids = vec![
            cl_core::ids::AccountId::parse("11111111-1111-4111-8111-111111111111").expect("id"),
        ];
        assert_eq!(missing_capability(&g, pr_a), None);
        let mut p = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(oct()));
        p.filters.account_ids.clone_from(&g.filters.account_ids);
        assert_eq!(missing_capability(&p, pr_a), Some(DRILL));
        // Routes exact before PR-A need nothing more.
        let a = ReportRequest::new(Source::ArAgingSummary, ReportDates::AsOf(d(2026, 10, 31)));
        assert_eq!(missing_capability(&a, |_| false), None);
    }

    #[test]
    #[allow(clippy::many_single_char_names, reason = "one request per route")]
    fn queries_carry_explicit_dates_and_the_exact_flag() {
        let q = |r: &ReportRequest| -> Vec<String> {
            exact_query(r)
                .expect("query")
                .into_iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect()
        };
        let mut r = ReportRequest::new(Source::ProfitLoss, ReportDates::Range(oct()));
        r.basis = Some(cl_core::report::Basis::Cash);
        r.filters.adjusting = Some(Adjusting::Exclude);
        assert_eq!(
            q(&r),
            [
                "exact=1",
                "start_date=2026-10-01",
                "end_date=2026-10-31",
                "basis=cash",
                "adjusting=exclude"
            ]
        );
        let mut t = ReportRequest::new(Source::TrialBalance, ReportDates::AsOf(d(2026, 10, 31)));
        t.fiscal_year_start = Some(d(2026, 1, 1));
        assert_eq!(
            q(&t),
            ["exact=1", "start_date=2026-01-01", "end_date=2026-10-31"]
        );
        let b = ReportRequest::new(Source::BalanceSheet, ReportDates::AsOf(d(2026, 10, 31)));
        assert_eq!(q(&b), ["exact=1", "as_of_date=2026-10-31"]);
        let mut g = ReportRequest::new(Source::GeneralLedger, ReportDates::Range(oct()));
        g.filters.cleared = Some(Cleared::Reconciled);
        g.filters.txn_types = vec!["bill".into(), "expense".into()];
        let gq = q(&g);
        assert!(gq.contains(&"limit=50000".to_string()));
        assert!(gq.contains(&"cleared=reconciled".to_string()));
        assert!(gq.contains(&"txn_types=bill,expense".to_string()));
        // A balance date the P&L doesn't take.
        let bad = ReportRequest::new(Source::ProfitLoss, ReportDates::AsOf(d(2026, 10, 31)));
        assert!(exact_query(&bad).is_err());
    }
}
