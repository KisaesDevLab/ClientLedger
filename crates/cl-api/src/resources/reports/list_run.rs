//! Queries and answers of the WP3 list reports (P28 plan §5): who owes
//! what, sales and spending, banking, 1099 and the controls. Every route
//! is read through `?exact=1` except A/R Aging Detail (exact through
//! `openExact` before P28), the duplicate scan, the audit log and Closing
//! Date Exceptions (no float amounts).

use cl_core::CalendarDate;
use cl_core::report::{ReportDates, ReportRequest, ReportTable, Source};

use super::adapters::{controls, lists};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{
    AUDIT_LOG, DUPLICATES_SCAN, Operation, REPORT_ACCOUNT_ACTIVITY_SUMMARY, REPORT_AP_1099_PREP,
    REPORT_AP_AGING, REPORT_AP_AGING_DETAIL, REPORT_AR_AGING_DETAIL, REPORT_BANK_BALANCES,
    REPORT_BILL_PAYMENT_HISTORY, REPORT_CHECK_REGISTER, REPORT_CLOSING_DATE_EXCEPTIONS,
    REPORT_CUSTOMER_BALANCE, REPORT_CUSTOMER_BALANCE_DETAIL, REPORT_DEPOSIT_DETAIL,
    REPORT_EXPENSE_BY_VENDOR, REPORT_INVOICE_LIST, REPORT_SALES_BY_CUSTOMER, REPORT_SALES_BY_ITEM,
    REPORT_TRANSACTION_LIST_BY_VENDOR, REPORT_UNPAID_BILLS, REPORT_VENDOR_1099_SUMMARY,
    REPORT_VENDOR_BALANCE_DETAIL, REPORT_VENDOR_BALANCE_SUMMARY, REPORT_VENDOR_STATEMENT,
};

/// Rows the audit trail and Closing Date Exceptions read at most.
pub const CONTROLS_LIMIT: u32 = 5_000;

/// The operation a list source reads.
#[must_use]
pub(crate) fn operation(source: Source) -> Option<&'static Operation> {
    Some(match source {
        Source::ArAgingDetail => &REPORT_AR_AGING_DETAIL,
        Source::ApAgingSummary => &REPORT_AP_AGING,
        Source::ApAgingDetail => &REPORT_AP_AGING_DETAIL,
        Source::InvoiceList => &REPORT_INVOICE_LIST,
        Source::UnpaidBills => &REPORT_UNPAID_BILLS,
        Source::CustomerBalanceSummary => &REPORT_CUSTOMER_BALANCE,
        Source::CustomerBalanceDetail => &REPORT_CUSTOMER_BALANCE_DETAIL,
        Source::VendorBalanceSummary => &REPORT_VENDOR_BALANCE_SUMMARY,
        Source::VendorBalanceDetail => &REPORT_VENDOR_BALANCE_DETAIL,
        Source::VendorStatement => &REPORT_VENDOR_STATEMENT,
        Source::SalesByCustomer => &REPORT_SALES_BY_CUSTOMER,
        Source::SalesByItem => &REPORT_SALES_BY_ITEM,
        Source::ExpenseByVendor => &REPORT_EXPENSE_BY_VENDOR,
        Source::TransactionListByVendor => &REPORT_TRANSACTION_LIST_BY_VENDOR,
        Source::BillPaymentHistory => &REPORT_BILL_PAYMENT_HISTORY,
        Source::Vendor1099Summary => &REPORT_VENDOR_1099_SUMMARY,
        Source::Ap1099Prep => &REPORT_AP_1099_PREP,
        Source::DepositDetail => &REPORT_DEPOSIT_DETAIL,
        Source::CheckRegister => &REPORT_CHECK_REGISTER,
        Source::BankBalances => &REPORT_BANK_BALANCES,
        Source::AccountActivitySummary => &REPORT_ACCOUNT_ACTIVITY_SUMMARY,
        Source::AuditLog => &AUDIT_LOG,
        Source::ClosingDateExceptions => &REPORT_CLOSING_DATE_EXCEPTIONS,
        Source::Duplicates => &REPORT_DUPLICATES,
        _ => return None,
    })
}

/// The duplicate scan's operation (named for the table above).
const REPORT_DUPLICATES: Operation = DUPLICATES_SCAN;

fn bad() -> ApiError {
    ApiError::Local(LocalValidation::BadRange)
}

/// The as-of date of a balance request.
fn as_of(d: &ReportDates) -> Result<CalendarDate, ApiError> {
    match d {
        ReportDates::AsOf(x) => Ok(*x),
        ReportDates::Range(r) => Ok(r.to),
        ReportDates::Ranges(_) | ReportDates::None => Err(bad()),
    }
}

fn range(d: &ReportDates) -> Result<(CalendarDate, CalendarDate), ApiError> {
    match d {
        ReportDates::Range(r) => Ok((r.from, r.to)),
        _ => Err(bad()),
    }
}

/// The query of a list request.
///
/// # Errors
/// `Local(BadRange)` for dates the route doesn't take; `Local(MissingPick)`
/// when a vendor or bank account must be chosen first.
pub fn query(req: &ReportRequest) -> Result<Vec<(String, String)>, ApiError> {
    lists::require_pick(req)?;
    let mut q: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: String| q.push((k.to_string(), v));
    let exact = !matches!(
        req.source,
        Source::ArAgingDetail
            | Source::Duplicates
            | Source::AuditLog
            | Source::ClosingDateExceptions
    );
    if exact {
        push("exact", "1".into());
    }
    match req.source {
        Source::ArAgingDetail
        | Source::ApAgingSummary
        | Source::ApAgingDetail
        | Source::UnpaidBills
        | Source::CustomerBalanceSummary
        | Source::CustomerBalanceDetail
        | Source::VendorBalanceSummary
        | Source::VendorBalanceDetail
        | Source::BankBalances => push("as_of_date", as_of(&req.dates)?.to_wire()),
        Source::InvoiceList => match &req.dates {
            // Open Invoices: still open on the date (U-121).
            ReportDates::AsOf(d) => {
                push("open", "1".into());
                push("as_of_date", d.to_wire());
            }
            d => {
                let (from, to) = range(d)?;
                push("start_date", from.to_wire());
                push("end_date", to.to_wire());
            }
        },
        Source::Vendor1099Summary | Source::Ap1099Prep => {
            let (_, to) = range(&req.dates)?;
            let key = if req.source == Source::Ap1099Prep {
                "tax_year"
            } else {
                "year"
            };
            push(key, to.year().to_string());
        }
        _ => {
            let (from, to) = range(&req.dates)?;
            push("start_date", from.to_wire());
            push("end_date", to.to_wire());
        }
    }
    match req.source {
        Source::TransactionListByVendor | Source::VendorStatement => {
            if let Some(c) = req.filters.contact_ids.first() {
                push("vendor_id", c.to_string());
            }
        }
        Source::CheckRegister => {
            if let Some(a) = req.filters.account_ids.first() {
                push("account_id", a.to_string());
            }
        }
        Source::SalesByCustomer | Source::SalesByItem | Source::ExpenseByVendor => {
            if let Some(b) = req.basis {
                push("basis", b.as_str().to_string());
            }
            if req.source != Source::ExpenseByVendor
                && let Some(a) = req.filters.adjusting
            {
                push("adjusting", a.as_str().to_string());
            }
        }
        Source::AuditLog => {
            push("company", "current".into());
            push("limit", CONTROLS_LIMIT.to_string());
            if let Some(c) = req.filters.contact_ids.first() {
                push("entity_id", c.to_string());
            }
        }
        Source::ClosingDateExceptions => push("limit", CONTROLS_LIMIT.to_string()),
        _ => {}
    }
    Ok(q)
}

/// A list answer as its table (pure: tests and the demo use it on recorded
/// answers).
///
/// # Errors
/// [`ApiError::ReportShape`] when an exact field is missing;
/// `Local(Unsupported)` for a source outside the lists.
pub fn adapt(req: &ReportRequest, v: &serde_json::Value) -> Result<ReportTable, ApiError> {
    match req.source {
        Source::InvoiceList if matches!(req.dates, ReportDates::AsOf(_)) => {
            lists::table(v, req, lists::open_invoices())
        }
        Source::ApAgingSummary => lists::ap_aging_summary(v, req, as_of(&req.dates)?),
        Source::CustomerBalanceDetail => lists::balance_detail(v, req, "@reports.row.no_client"),
        Source::VendorBalanceDetail => lists::balance_detail(v, req, "@reports.row.no_vendor"),
        Source::Vendor1099Summary => lists::vendor_1099_summary(v, req),
        Source::VendorStatement => lists::vendor_statement(v, req),
        Source::Duplicates => lists::duplicates(v, req),
        Source::AuditLog => controls::audit_trail(v, req),
        Source::ClosingDateExceptions => controls::closing_exceptions(v, req),
        s => match lists::layout(s) {
            Some(l) => lists::table(v, req, l),
            None => Err(ApiError::Local(LocalValidation::Unsupported)),
        },
    }
}
