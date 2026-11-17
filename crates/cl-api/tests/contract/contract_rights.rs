//! P29 WP1: the rights the desktop reads from `/auth/me`, and the engine's
//! refusals that match what the desktop hides (plan §6 AC1).
//!
//! The desktop's table lives in `cl-model::permissions` (layering keeps
//! cl-api from depending on it); [`AP_REPORTS`] and [`AR_REPORTS`] mirror
//! its `report_area` for the Accounts payable and Accounts receivable
//! built-in roles, route by route, and the cl-model unit tests check the
//! same decisions. A route the desktop hides for the role must be refused
//! (403) by the engine, and a route it shows must not be.
//!
//! The two roles are the harness's shared members ([`Member::Payables`],
//! [`Member::Receivables`]): signed in once per run, their overrides set
//! once to the built-in's map.

use cl_api::Method;
use cl_api::{
    AccountType, ApiError, BillDraft, BillLineDraft, ContactFields, ContactType, InvoiceLine,
    NewContact, NewInvoice, PostingOptions, Session,
};
use cl_core::{Money, Quantity};

use crate::common::{account, date, key, m};
use crate::contract_invoices::customer;
use crate::support::{Harness, Member, unique};

/// U-227 report areas.
const REPORT_AREAS: &str = "report-areas-v1";

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
    // Inventory value follows `items` (U-228 was cut; no `item_costs`).
    ("/api/v1/reports/inventory-valuation", false),
    // P29 review: the routes the first tables skipped.
    ("/api/v1/reports/vendor-statement", false),
    ("/api/v1/reports/ap-1099-prep", false),
    ("/api/v1/reports/reconciliation-detail", true),
    ("/api/v1/reports/reconciliation-discrepancy", true),
    ("/api/v1/reports/inventory-valuation-detail", false),
    ("/api/v1/reports/reorder", false),
];

/// `(route, hidden for A/R)`: the same routes for the A/R built-in (sales
/// tax full, items view, no banking, no bills, no financial reports).
const AR_REPORTS: &[(&str, bool)] = &[
    ("/api/v1/reports/profit-loss", true),
    ("/api/v1/reports/balance-sheet", true),
    ("/api/v1/reports/trial-balance", true),
    ("/api/v1/reports/general-ledger", true),
    ("/api/v1/reports/revenue-by-category", true),
    ("/api/v1/reports/expense-by-category", true),
    ("/api/v1/reports/account-activity-summary", true),
    ("/api/v1/reports/account-report", true),
    ("/api/v1/reports/ar-aging-summary", false),
    ("/api/v1/reports/ar-aging-detail", false),
    ("/api/v1/reports/invoice-list", false),
    ("/api/v1/reports/customer-balance-summary", false),
    ("/api/v1/reports/customer-balance-detail", false),
    ("/api/v1/reports/sales-by-customer", false),
    ("/api/v1/reports/sales-by-item", false),
    ("/api/v1/reports/customer-statement", false),
    ("/api/v1/reports/sales-tax-liability", false),
    ("/api/v1/reports/deposit-detail", true),
    ("/api/v1/reports/check-register", true),
    ("/api/v1/reports/bank-balances", true),
    ("/api/v1/reports/ap-aging-summary", true),
    ("/api/v1/reports/ap-aging-detail", true),
    ("/api/v1/reports/unpaid-bills", true),
    ("/api/v1/reports/vendor-balance-summary", true),
    ("/api/v1/reports/vendor-balance-detail", true),
    ("/api/v1/reports/expense-by-vendor", true),
    ("/api/v1/reports/transaction-list-by-vendor", true),
    ("/api/v1/reports/bill-payment-history", true),
    ("/api/v1/reports/vendor-1099-summary", true),
    ("/api/v1/reports/inventory-stock-status", false),
    ("/api/v1/reports/inventory-valuation", false),
    ("/api/v1/reports/vendor-statement", true),
    ("/api/v1/reports/ap-1099-prep", true),
    ("/api/v1/reports/reconciliation-detail", true),
    ("/api/v1/reports/reconciliation-discrepancy", true),
    ("/api/v1/reports/inventory-valuation-detail", false),
    ("/api/v1/reports/reorder", false),
];

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

/// Routes where the engine's answer differs from the desktop's table.
fn disagreements(h: &Harness, s: &Session, table: &[(&str, bool)]) -> Vec<String> {
    let mut wrong = Vec::new();
    for (route, hidden) in table {
        let got = status(h, s, route);
        if (got == 403) != *hidden {
            wrong.push(format!("{route}: {got} (hidden on desktop: {hidden})"));
        }
    }
    wrong
}

fn vendor(h: &Harness) -> cl_core::ids::ContactId {
    h.run(h.s1.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Vendor, unique("Rights Vendor")),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("vendor: {e}"))
    .record
    .id
}

/// A one-line bill of 42.00 (synthetic), its vendor and account made by
/// the owner.
fn bill(h: &Harness) -> BillDraft {
    let a = account(h, &h.s1, AccountType::Expense, "Rights supplies").id;
    BillDraft {
        vendor: vendor(h),
        date: date(2026, 11, 3),
        vendor_invoice_number: Some(unique("R").replace(' ', "-")),
        terms: None,
        due_date: None,
        memo: None,
        amount_due: m("42.00"),
        lines: vec![BillLineDraft::expense(a, m("42.00"))],
        received: true,
        draft_attachment: None,
    }
}

/// A one-line invoice of 2 × 21.00 (synthetic), not taxable.
fn invoice(h: &Harness) -> NewInvoice {
    let c = customer(h, &h.s1, "Rights Client");
    let a = account(h, &h.s1, AccountType::Revenue, "Rights income").id;
    NewInvoice::new(
        c,
        date(2026, 11, 4),
        vec![InvoiceLine {
            is_taxable: Some(false),
            ..InvoiceLine::account(a, Quantity::parse_wire("2").expect("qty"), m("21.00"))
        }],
    )
}

/// `/auth/me`: the owner's role in the open company and a full map; the
/// A/P member's map has bills full and invoices none.
pub fn me_reports_the_role_and_the_map(h: &Harness) {
    let me = h.run(h.s1.me()).expect("me");
    assert_eq!(me.role, "owner");
    assert_eq!(
        me.permissions.get("bills").map(String::as_str),
        Some("full")
    );
    let (_, ap) = h.member(Member::Payables);
    let me = h.run(ap.me()).expect("A/P me");
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

/// Positive and negative reads for A/P: invoices are refused, A/P aging
/// runs.
pub fn ap_is_refused_invoices_and_runs_ap_aging(h: &Harness) {
    let (_, ap) = h.member(Member::Payables);
    assert_eq!(status(h, &ap, "/api/v1/invoices"), 403);
    assert_ne!(status(h, &ap, "/api/v1/reports/ap-aging-summary"), 403);
}

/// AC1 saves: A/P saves a bill and is refused an invoice; A/R saves an
/// invoice and is refused a bill.
pub fn ap_saves_a_bill_and_ar_saves_an_invoice(h: &Harness) {
    let (_, ap) = h.member(Member::Payables);
    let (_, ar) = h.member(Member::Receivables);
    let b = bill(h);
    let saved = h
        .run(ap.bills().create(&b, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("A/P saves a bill: {e:?}"));
    assert_eq!(saved.record.txn.total, Some(m("42.0000")));
    let i = invoice(h);
    let e = h
        .run(ap.invoices().create(&i, &PostingOptions::default(), key()))
        .expect_err("A/P is refused invoices");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");

    let made = h
        .run(ar.invoices().create(&i, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("A/R saves an invoice: {e:?}"));
    assert_eq!(made.invoice.record.txn.total, Some(m("42.00")));
    let e = h
        .run(
            ar.bills()
                .create(&bill(h), &PostingOptions::default(), key()),
        )
        .expect_err("A/R is refused bills");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");
}

/// AC1: every report the desktop hides for A/P is refused by the engine,
/// and every report it shows is not (U-227).
pub fn ap_hidden_reports_are_refused(h: &Harness) {
    if !h.require_cap(REPORT_AREAS, "U-227") {
        return;
    }
    let (_, ap) = h.member(Member::Payables);
    let wrong = disagreements(h, &ap, AP_REPORTS);
    assert!(wrong.is_empty(), "desktop and engine disagree: {wrong:#?}");
}

/// The same for A/R.
pub fn ar_hidden_reports_are_refused(h: &Harness) {
    if !h.require_cap(REPORT_AREAS, "U-227") {
        return;
    }
    let (_, ar) = h.member(Member::Receivables);
    let wrong = disagreements(h, &ar, AR_REPORTS);
    assert!(wrong.is_empty(), "desktop and engine disagree: {wrong:#?}");
}
