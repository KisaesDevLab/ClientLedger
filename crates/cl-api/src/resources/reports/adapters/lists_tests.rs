//! Tests of the list adapters on answers recorded from the engine
//! (`tests/fixtures/reports/list_*.json`).

use super::lists::*;
use crate::errors::{ApiError, LocalValidation};
use crate::resources::reports::list_run;
use cl_core::CalendarDate;
use cl_core::ids::ContactId;
use cl_core::report::{
    AgingBucket, Cell, DateRange, Drill, ReportDates, ReportRequest, ReportTable, RowKind, Source,
};
use rust_decimal::Decimal;
use serde_json::Value;
use std::str::FromStr;

fn fixture(name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/reports")
        .join(format!("list_{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("fixture")).expect("json")
}

fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("date")
}

fn at(source: Source) -> ReportRequest {
    ReportRequest::new(source, ReportDates::AsOf(d(2026, 11, 30)))
}

fn q4(source: Source) -> ReportRequest {
    ReportRequest::new(
        source,
        ReportDates::Range(DateRange {
            from: d(2026, 10, 1),
            to: d(2026, 12, 31),
        }),
    )
}

fn dec(s: &str) -> Cell {
    Cell::Amount(Decimal::from_str(s).expect("dec"))
}

fn run(req: &ReportRequest, name: &str) -> ReportTable {
    list_run::adapt(req, &fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn col(t: &ReportTable, key: &str) -> usize {
    t.columns
        .iter()
        .position(|c| c.key == key)
        .unwrap_or_else(|| panic!("column {key}"))
}

/// Every recorded list reads, and its total line is the engine's.
#[test]
#[allow(clippy::too_many_lines, reason = "one row per recorded route")]
fn every_recorded_list_reads_with_the_engine_s_total() {
    for (req, name, key, total) in [
        (
            at(Source::ApAgingSummary),
            "ap_aging_summary",
            "total",
            "19171.93",
        ),
        (
            at(Source::ApAgingDetail),
            "ap_aging_summary",
            "open",
            "19171.93",
        ),
        (
            at(Source::ArAgingDetail),
            "ar_aging_detail",
            "open",
            "52385.98",
        ),
        (at(Source::UnpaidBills), "unpaid_bills", "open", "3481"),
        (at(Source::InvoiceList), "open_invoices", "open", "3623.58"),
        (q4(Source::InvoiceList), "invoice_list", "open", "2033.33"),
        (
            at(Source::CustomerBalanceSummary),
            "customer_balance_summary",
            "balance",
            "52385.98",
        ),
        (
            at(Source::CustomerBalanceDetail),
            "customer_balance_detail",
            "balance",
            "52385.98",
        ),
        (
            at(Source::VendorBalanceSummary),
            "vendor_balance_summary",
            "balance",
            "19171.93",
        ),
        (
            at(Source::VendorBalanceDetail),
            "vendor_balance_detail",
            "balance",
            "19171.93",
        ),
        (
            q4(Source::BillPaymentHistory),
            "bill_payment_history",
            "amount",
            "2180.4",
        ),
        (
            q4(Source::TransactionListByVendor),
            "transactions_by_vendor",
            "amount",
            "3416.2",
        ),
        (
            q4(Source::SalesByCustomer),
            "sales_by_customer",
            "total",
            "4473.58",
        ),
        (q4(Source::SalesByItem), "sales_by_item", "total", "4473.58"),
        (
            q4(Source::ExpenseByVendor),
            "expense_by_vendor",
            "total",
            "82629.28",
        ),
        (q4(Source::DepositDetail), "deposit_detail", "amount", "0"),
        (
            q4(Source::CheckRegister),
            "check_register",
            "payment",
            "117968.89",
        ),
        (
            at(Source::BankBalances),
            "bank_balances",
            "balance",
            "101160.55",
        ),
        (
            q4(Source::AccountActivitySummary),
            "account_activity_summary",
            "net",
            "0",
        ),
        (q4(Source::Ap1099Prep), "ap_1099_prep", "paid", "0"),
        (
            q4(Source::Vendor1099Summary),
            "vendor_1099_summary",
            "total",
            "0",
        ),
        (
            q4(Source::VendorStatement),
            "vendor_statement",
            "balance",
            "1185",
        ),
    ] {
        let t = run(&req, name);
        let last = t.rows.last().unwrap_or_else(|| panic!("{name}: rows"));
        assert_eq!(last.kind, RowKind::Grand, "{name}");
        assert_eq!(last.cells[col(&t, key)], dec(total), "{name}");
    }
}

/// A/P: open vendor credits are negative rows; "(no vendor)" is the label,
/// not the engine's words.
#[test]
fn credits_are_negative_rows_and_no_contact_is_a_label() {
    let t = run(&at(Source::ApAgingDetail), "ap_aging_summary");
    let open = col(&t, "open");
    assert!(
        t.rows
            .iter()
            .any(|r| matches!(r.cells[open], Cell::Amount(x) if x.is_sign_negative()))
    );
    let ev = run(&q4(Source::ExpenseByVendor), "expense_by_vendor");
    assert!(
        ev.rows
            .iter()
            .any(|r| r.caption == "@reports.row.no_vendor")
    );
    assert!(!ev.rows.iter().any(|r| r.caption == "Uncategorized"));
    let items = run(&q4(Source::SalesByItem), "sales_by_item");
    assert_eq!(items.rows[0].caption, "@reports.row.no_item");
}

/// An aging cell's drill keeps that contact's rows in that bucket, and a
/// narrowed list has no total line.
#[test]
fn an_aging_drill_narrows_by_contact_and_the_engine_s_bucket() {
    let mut r = at(Source::ApAgingDetail);
    r.filters.contact_ids =
        vec![ContactId::parse("441603de-7eae-4582-b712-a335f4b1eaed").expect("id")];
    r.filters.bucket = Some(AgingBucket::Days31To60);
    let t = run(&r, "ap_aging_summary");
    assert_eq!(t.rows.len(), 2);
    assert!(t.rows.iter().all(|x| x.kind == RowKind::Detail));
    r.filters.bucket = Some(AgingBucket::Over90);
    assert_eq!(run(&r, "ap_aging_summary").rows, Vec::new());
    let mut ar = at(Source::ArAgingDetail);
    ar.filters.bucket = Some(AgingBucket::Over90);
    let t = run(&ar, "ar_aging_detail");
    let b = col(&t, "bucket");
    assert!(
        t.rows
            .iter()
            .all(|x| x.cells[b] == Cell::Text("@reports.bucket.90_up".into()))
    );
    assert_eq!(t.rows.len(), 3);
}

#[test]
fn rows_open_their_transactions() {
    let t = run(&at(Source::UnpaidBills), "unpaid_bills");
    assert!(matches!(
        &t.rows[0].drill,
        Some(Drill::Transaction { txn_type, .. }) if txn_type == "bill"
    ));
    let s = run(&at(Source::ApAgingSummary), "ap_aging_summary");
    assert!(matches!(s.rows[0].drill, Some(Drill::Aging { .. })));
    let cbd = run(
        &at(Source::CustomerBalanceDetail),
        "customer_balance_detail",
    );
    assert!(cbd.rows.iter().any(|r| r.kind == RowKind::Subtotal));
}

#[test]
fn a_tin_shows_its_last_four_digits_only() {
    assert_eq!(last4("12-3456789"), "\u{2022}\u{2022}\u{2022}\u{2022} 6789");
    assert_eq!(last4(""), "");
    let v = serde_json::json!({
        "meta": {"exact": true},
        "data": [{"vendor_name": "A", "tax_id": "123-45-6789", "address": "", "total_paidExact": "700.0000"}],
        "totalPaidExact": "700.0000"
    });
    let t = list_run::adapt(&q4(Source::Ap1099Prep), &v).expect("t");
    assert_eq!(
        t.rows[0].cells[0],
        Cell::Text("\u{2022}\u{2022}\u{2022}\u{2022} 6789".into())
    );
    assert!(!t.to_canonical_csv().contains("123-45"));
}

/// The engine's `check_number` is a pg integer: the Check Register shows
/// its digits (the P28 golden's check 5001 was blank).
#[test]
fn an_integer_check_number_shows_its_digits() {
    let mut v = fixture("check_register");
    v["data"][0]["check_number"] = serde_json::json!(5001);
    v["data"][1]["check_number"] = serde_json::json!("5002");
    let t = list_run::adapt(&q4(Source::CheckRegister), &v).expect("register");
    let c = col(&t, "check_number");
    let numbers: Vec<&Cell> = t.rows.iter().filter_map(|r| r.cells.get(c)).collect();
    assert!(numbers.contains(&&Cell::Text("5001".into())));
    assert!(numbers.contains(&&Cell::Text("5002".into())));
}

#[test]
fn a_number_where_exact_text_belongs_is_refused() {
    let v = serde_json::json!({
        "meta": {"exact": true},
        "data": [{"display_name": "A", "id": null, "balanceExact": 125}],
        "totalsExact": {"balance": "12.5"}
    });
    assert!(list_run::adapt(&at(Source::CustomerBalanceSummary), &v).is_err());
    let dup = serde_json::json!({
        "pairs": [{"a": {"id": null, "total": 5}, "b": {}, "daysApart": 0}],
        "count": 1, "limit": 100
    });
    assert!(list_run::adapt(&q4(Source::Duplicates), &dup).is_err());
}

#[test]
fn a_vendor_or_bank_account_must_be_chosen_first() {
    assert!(matches!(
        list_run::query(&q4(Source::TransactionListByVendor)),
        Err(ApiError::Local(LocalValidation::MissingPick))
    ));
    let q: Vec<String> = list_run::query(&at(Source::InvoiceList))
        .expect("q")
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    assert_eq!(q, ["exact=1", "open=1", "as_of_date=2026-11-30"]);
    let y = list_run::query(&q4(Source::Ap1099Prep)).expect("y");
    assert_eq!(y[1], ("tax_year".to_string(), "2026".to_string()));
}

#[test]
fn engine_codes_outside_the_known_words_show_as_words_not_keys() {
    assert_eq!(code_label("reports.bucket", "90+"), "@reports.bucket.90_up");
    assert_eq!(
        code_label("reports.audit.action", "entry_changed"),
        "entry changed"
    );
    assert_eq!(code_label("reports.status", "paid"), "@reports.status.paid");
}

/// The Invoice List's rows carry no contact id: the name shows.
#[test]
fn the_invoice_list_shows_client_names() {
    let t = run(&q4(Source::InvoiceList), "invoice_list");
    assert_eq!(t.rows[0].caption, "Table Rock Dental");
}

/// The aging's "(no client)" row opens only rows without a client.
#[test]
fn the_no_client_aging_drill_keeps_rows_without_a_client() {
    let mut r = at(Source::ArAgingDetail);
    r.filters.no_contact = true;
    let t = run(&r, "ar_aging_detail");
    assert_eq!(t.rows, Vec::new(), "every recorded row has a client");
}
