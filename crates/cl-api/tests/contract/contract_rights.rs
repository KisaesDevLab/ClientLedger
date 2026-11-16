//! P29 WP1: the rights the desktop reads from `/auth/me`, and the engine's
//! refusals that match what the desktop hides (plan §6 AC1).
//!
//! The desktop's table lives in `cl-model::permissions` (layering keeps
//! cl-api from depending on it); [`AP_REPORTS`] mirrors its `report_area`
//! for the Accounts payable built-in role, route by route, and the cl-model
//! unit tests check the same decisions. A route the desktop hides for A/P
//! must be refused (403) by the engine, and a route it shows must not be.

use cl_api::{Method, Session};
use serde_json::json;

use crate::support::Harness;

/// U-227 report areas.
const REPORT_AREAS: &str = "report-areas-v1";

/// The A/P built-in's map (plan §1 table), every key listed. The two area
/// keys go only to an engine that knows them (U-227, U-228).
fn ap_map(with_areas: bool) -> serde_json::Value {
    let mut m = json!({
        "transactions": "none", "batch_entry": "none", "recurring": "full",
        "duplicates": "none", "bulk_import": "none", "invoices": "none",
        "receive_payment": "none", "estimates": "none", "items": "view",
        "sales_tax": "none", "bills": "full", "pay_bills": "full",
        "vendor_credits": "full", "payroll_import": "none", "checks": "full",
        "banking": "none", "daily_sales": "none", "reports": "view",
        "budgets": "none", "dashboard": "view", "trial_balance": "none",
        "tax_estimator": "none", "accounts": "view", "contacts": "full",
        "attachments": "full", "tags": "view", "company_settings": "view",
        "ai_chat": "none", "audit_log": "none",
    });
    if with_areas && let Some(o) = m.as_object_mut() {
        o.insert("financial_reports".into(), json!("none"));
        o.insert("item_costs".into(), json!("view"));
    }
    m
}

/// `(route, hidden for A/P)`: every catalog report route and the
/// `/reports/*` reads outside the Reports window (`ReportUse`).
const AP_REPORTS: &[(&str, bool)] = &[
    ("/api/v1/reports/profit-loss", true),
    ("/api/v1/reports/balance-sheet", true),
    ("/api/v1/reports/trial-balance", true),
    ("/api/v1/reports/general-ledger", true),
    ("/api/v1/reports/revenue-by-category", true),
    ("/api/v1/reports/expense-by-category", true),
    ("/api/v1/reports/account-activity-summary", true),
    ("/api/v1/reports/account-report", true),
    ("/api/v1/reports/ar-aging-summary", true),
    ("/api/v1/reports/ar-aging-detail", true),
    ("/api/v1/reports/invoice-list", true),
    ("/api/v1/reports/customer-balance-summary", true),
    ("/api/v1/reports/customer-balance-detail", true),
    ("/api/v1/reports/sales-by-customer", true),
    ("/api/v1/reports/sales-by-item", true),
    ("/api/v1/reports/customer-statement", true),
    ("/api/v1/reports/sales-tax-liability", true),
    ("/api/v1/reports/deposit-detail", true),
    ("/api/v1/reports/check-register", true),
    ("/api/v1/reports/bank-balances", true),
    ("/api/v1/reports/ap-aging-summary", false),
    ("/api/v1/reports/ap-aging-detail", false),
    ("/api/v1/reports/unpaid-bills", false),
    ("/api/v1/reports/vendor-balance-summary", false),
    ("/api/v1/reports/vendor-balance-detail", false),
    ("/api/v1/reports/expense-by-vendor", false),
    ("/api/v1/reports/transaction-list-by-vendor", false),
    ("/api/v1/reports/bill-payment-history", false),
    ("/api/v1/reports/vendor-1099-summary", false),
    ("/api/v1/reports/inventory-stock-status", false),
    ("/api/v1/reports/inventory-valuation", false),
];

fn as_ap(h: &Harness, with_areas: bool) -> Session {
    let s = &h.s1;
    let (user, bk) = h.bookkeeper();
    let r = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": ap_map(with_areas) })),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
    bk
}

fn status(h: &Harness, s: &Session, route: &str) -> u16 {
    h.run(s.raw(
        Method::Get,
        route,
        &[
            ("startDate", "2026-01-01"),
            ("endDate", "2026-01-31"),
            ("asOfDate", "2026-01-31"),
        ],
        None,
    ))
    .expect("raw")
    .status
}

/// `/auth/me`: the owner's role in the open company and a full map; the
/// A/P bookkeeper's map has bills full and invoices none.
pub fn me_reports_the_role_and_the_map(h: &Harness) {
    let me = h.run(h.s1.me()).expect("me");
    assert_eq!(me.role, "owner");
    assert_eq!(
        me.permissions.get("bills").map(String::as_str),
        Some("full")
    );
    let bk = as_ap(h, false);
    let me = h.run(bk.me()).expect("bookkeeper me");
    assert_eq!(me.role, "bookkeeper");
    assert!(!me.is_super_admin);
    assert_eq!(
        me.permissions.get("bills").map(String::as_str),
        Some("full")
    );
    assert_eq!(
        me.permissions.get("invoices").map(String::as_str),
        Some("none")
    );
}

/// Positive and negative saves for A/P on today's engine: invoices are
/// refused, A/P aging runs.
pub fn ap_is_refused_invoices_and_runs_ap_aging(h: &Harness) {
    let bk = as_ap(h, false);
    assert_eq!(status(h, &bk, "/api/v1/invoices"), 403);
    assert_ne!(status(h, &bk, "/api/v1/reports/ap-aging-summary"), 403);
}

/// AC1: every report the desktop hides for A/P is refused by the engine,
/// and every report it shows is not (U-227).
pub fn ap_hidden_reports_are_refused(h: &Harness) {
    if !h.require_cap(REPORT_AREAS, "U-227") {
        return;
    }
    let bk = as_ap(h, true);
    let mut wrong = Vec::new();
    for (route, hidden) in AP_REPORTS {
        let got = status(h, &bk, route);
        if (got == 403) != *hidden {
            wrong.push(format!("{route}: {got} (hidden on desktop: {hidden})"));
        }
    }
    assert!(wrong.is_empty(), "desktop and engine disagree: {wrong:#?}");
}
