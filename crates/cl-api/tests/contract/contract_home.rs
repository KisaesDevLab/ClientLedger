//! Home's reads against the real engine (P16 work package 6a, D9).
//!
//! - The counts that work on every engine (bills by due date, unposted
//!   payroll imports, reconciliations, statements, transactions, waiting
//!   bank-feed items) equal a direct query of the same engine list.
//! - The pinned engine's dashboard money is a JSON number, and the strict
//!   decoders refuse it, so a float amount can never reach Home.
//! - With `dashboard-exact-v1` (U-76, engine PR #168) every dashboard
//!   amount is a decimal string read exactly, `asOf` is the day used, and
//!   the payables window follows `dueWithinDays`. PENDING(U-76) until the
//!   pin carries it.

use cl_api::{
    AccountType, BillLine, BillQuery, Capabilities, ContactFields, ContactFilter, ContactType,
    InvoiceLine, Method, NewBill, NewContact, NewInvoice, PageRequest, PayrollStatus, Session,
    action_items_from_json, cash_position_from_json, payables_from_json, receivables_from_json,
};
use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money, Quantity, Rate};
use serde_json::{Value, json};

use crate::common::{account, date, key, m};
use crate::support::{Harness, unique};

/// Today (UTC); the engine's `CURRENT_DATE` is this day or the next.
fn today() -> CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    date(1970, 1, 1)
        .add_days(i64::try_from(days).expect("days"))
        .expect("today")
}

fn day(offset: i64) -> CalendarDate {
    today().add_days(offset).expect("day")
}

fn contact(h: &Harness, s: &Session, kind: ContactType, prefix: &str) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(kind, unique(prefix)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("contact: {e}"))
    .record
    .id
}

fn bill(
    h: &Harness,
    s: &Session,
    vendor: ContactId,
    expense: AccountId,
    due: CalendarDate,
    amount: &str,
) {
    h.run(s.create_bill(
        &NewBill {
            contact_id: vendor,
            txn_date: due.add_days(-30).expect("date"),
            due_date: Some(due),
            vendor_invoice_number: Some(unique("H")),
            memo: None,
            lines: vec![BillLine {
                account_id: expense,
                description: None,
                amount: m(amount),
            }],
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("bill: {e}"));
}

/// `total` of a raw list query.
fn raw_total(h: &Harness, s: &Session, path: &str, query: &[(&str, &str)]) -> u64 {
    let r = h
        .run(s.raw(Method::Get, path, query, None))
        .unwrap_or_else(|e| panic!("GET {path}: {e}"));
    assert_eq!(r.status, 200, "GET {path}: {}", r.body);
    r.body["total"]
        .as_u64()
        .unwrap_or_else(|| panic!("GET {path}: no integer total: {}", r.body))
}

fn bills_total(h: &Harness, s: &Session, q: &BillQuery) -> u64 {
    let pairs = q.query();
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    raw_total(h, s, "/api/v1/bills", &refs)
}

/// Two overdue bills and one due in five days: the counts move by exactly
/// that, and always equal the engine's own list total.
pub fn bills_count_matches_the_list(h: &Harness) {
    let s = &h.s1;
    let vendor = contact(h, s, ContactType::Vendor, "Home vendor");
    let expense = account(h, s, AccountType::Expense, "Home supplies").id;
    let today = today();
    let due7 = BillQuery::due_within(today, 7).expect("q");
    let overdue = BillQuery::overdue(today).expect("q");
    let count = |q: &BillQuery| h.run(s.home().bills_count(q)).expect("bills count");
    let (due_before, overdue_before) = (count(&due7), count(&overdue));
    assert_eq!(due_before, bills_total(h, s, &due7));
    bill(h, s, vendor, expense, day(-10), "120.00");
    bill(h, s, vendor, expense, day(-3), "75.50");
    bill(h, s, vendor, expense, day(5), "7471.90");
    let (due_after, overdue_after) = (count(&due7), count(&overdue));
    assert_eq!(due_after, bills_total(h, s, &due7), "Home equals the list");
    assert_eq!(overdue_after, bills_total(h, s, &overdue));
    assert_eq!(
        due_after - due_before,
        3,
        "two overdue and one due in 5 days"
    );
    assert_eq!(overdue_after - overdue_before, 2, "only the two past due");
    let three_days = BillQuery::due_within(today, 3).expect("q");
    assert_eq!(count(&three_days), bills_total(h, s, &three_days));
}

/// Unposted payroll imports, reconciliations, statements, transactions and
/// waiting bank-feed items equal the engine's own answers.
pub fn counts_match_the_engine(h: &Harness) {
    let s = &h.s1;
    let home = s.home();
    let payroll: u64 = PayrollStatus::UNPOSTED
        .iter()
        .map(|st| {
            raw_total(
                h,
                s,
                "/api/v1/payroll-import/sessions",
                &[("status", st.as_wire()), ("limit", "1")],
            )
        })
        .sum();
    assert_eq!(
        h.run(home.payroll_sessions_count(&PayrollStatus::UNPOSTED))
            .expect("payroll"),
        payroll
    );
    assert_eq!(
        h.run(home.bank_statements_any()).expect("statements"),
        raw_total(h, s, "/api/v1/banking/statements", &[("limit", "1")]) > 0
    );
    assert_eq!(
        h.run(home.has_any_transaction()).expect("transactions"),
        raw_total(h, s, "/api/v1/transactions", &[("limit", "1")]) > 0
    );
    let raw = h
        .run(s.raw(Method::Get, "/api/v1/dashboard/action-items", &[], None))
        .expect("action items");
    assert_eq!(
        h.run(home.feed_pending_count(today())).expect("feed"),
        raw.body["pendingFeedCount"]
            .as_u64()
            .expect("integer count")
    );

    // A reconciliation left open shows; once canceled it does not.
    let bank = account(h, s, AccountType::Asset, "Home checking").id;
    let started = h
        .run(s.raw(
            Method::Post,
            "/api/v1/banking/reconciliations",
            &[],
            Some(&json!({
                "accountId": bank.to_string(),
                "statementDate": day(-5).to_wire(),
                "statementEndingBalance": "0.00",
            })),
        ))
        .expect("start");
    assert_eq!(started.status, 201, "{}", started.body);
    let id = started.body["reconciliation"]["id"]
        .as_str()
        .expect("id")
        .to_string();
    let other = account(h, s, AccountType::Asset, "Home savings").id;
    let state = h
        .run(home.reconcile_state(&[bank, other]))
        .expect("reconcile");
    assert_eq!(state.in_progress, vec![bank]);
    let cancel = h
        .run(s.raw(
            Method::Post,
            &format!("/api/v1/banking/reconciliations/{id}/cancel"),
            &[],
            None,
        ))
        .expect("cancel");
    assert!(cancel.status < 300, "{}", cancel.body);
    let state = h.run(home.reconcile_state(&[bank])).expect("reconcile");
    assert_eq!(state.in_progress, Vec::<AccountId>::new());
}

/// P16 WP7: the rail's client and vendor counts read one row and take the
/// list's `total`; it equals the number of active contacts the whole list
/// returns.
pub fn rail_counts_match_the_lists(h: &Harness) {
    let s = &h.s1;
    contact(h, s, ContactType::Customer, "Home rail client");
    contact(h, s, ContactType::Vendor, "Home rail vendor");
    for kind in [ContactType::Customer, ContactType::Vendor] {
        let filter = ContactFilter {
            contact_type: Some(kind),
            is_active: Some(true),
            ..ContactFilter::default()
        };
        let page = h
            .run(
                s.contacts()
                    .list(&filter, PageRequest::new(1, 0).expect("page")),
            )
            .expect("one row");
        let all = h.run(s.contacts().list_all(&filter)).expect("all");
        assert!(page.data.len() <= 1, "one row read");
        assert_eq!(
            page.total,
            u64::try_from(all.len()).expect("len"),
            "{kind:?}"
        );
        assert!(page.total > 0, "{kind:?}");
    }
}

fn raw_dashboard(h: &Harness, s: &Session, panel: &str, query: &[(&str, &str)]) -> Vec<u8> {
    let r = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/dashboard/{panel}"),
            query,
            None,
        ))
        .unwrap_or_else(|e| panic!("{panel}: {e}"));
    assert_eq!(r.status, 200, "{panel}: {}", r.body);
    serde_json::to_vec(&r.body).expect("bytes")
}

/// The pinned engine answers dashboard money as JSON numbers; the strict
/// decoders refuse every panel, so Home shows a dash, never a float.
pub fn float_dashboard_refused(h: &Harness) {
    let s = &h.s1;
    if h.caps.has(Capabilities::DASHBOARD_EXACT) {
        // The default (no `exact`) still answers numbers on the new engine.
        assert!(receivables_from_json(&raw_dashboard(h, s, "receivables", &[])).is_err());
        return;
    }
    assert!(!s.dashboard().exact());
    assert!(cash_position_from_json(&raw_dashboard(h, s, "cash-position", &[])).is_err());
    assert!(receivables_from_json(&raw_dashboard(h, s, "receivables", &[])).is_err());
    assert!(payables_from_json(&raw_dashboard(h, s, "payables", &[])).is_err());
    assert!(action_items_from_json(&raw_dashboard(h, s, "action-items", &[])).is_err());
    let e = h
        .run(s.dashboard().receivables(today()))
        .expect_err("gated");
    assert_eq!(
        e,
        cl_api::ApiError::Local(cl_api::LocalValidation::DashboardUnavailable)
    );
}

fn diff(after: Money, before: Money) -> String {
    after.checked_sub(before).expect("difference").to_wire()
}

/// U-76: an overdue 12,345.67 invoice and a 7,471.90 bill due in five days
/// move the engine's string figures by exactly those amounts; every money
/// field on the wire is a string; `asOf` is the day sent.
pub fn exact_strings_u76(h: &Harness) {
    if !h.require_cap(Capabilities::DASHBOARD_EXACT, "U-76 dashboard-exact-v1") {
        return;
    }
    let s = &h.s1;
    let dash = s.dashboard();
    let today = today();
    let wire = raw_dashboard(
        h,
        s,
        "payables",
        &[("exact", "1"), ("asOf", &today.to_wire())],
    );
    let v: Value = serde_json::from_slice(&wire).expect("json");
    for f in [
        "totalOwed",
        "overdueAmount",
        "dueThisWeekAmount",
        "creditAmount",
        "apBalance",
    ] {
        assert!(v[f].is_string(), "{f} is a string: {v}");
    }
    let r0 = h.run(dash.receivables(today)).expect("receivables");
    let p0 = h.run(dash.payables(today, 7)).expect("payables");
    let client = contact(h, s, ContactType::Customer, "Home client");
    let income = account(h, s, AccountType::Revenue, "Home sales").id;
    h.run(s.create_invoice(
        &NewInvoice {
            txn_date: day(-45),
            due_date: Some(day(-15)),
            contact_id: client,
            lines: vec![InvoiceLine {
                account_id: income,
                description: None,
                quantity: Quantity::parse_wire("1").expect("q"),
                unit_price: m("12345.67"),
                is_taxable: false,
                tax_rate: Rate::parse_wire("0").expect("r"),
            }],
            memo: None,
            txn_number: None,
        },
        key(),
    ))
    .expect("invoice");
    let vendor = contact(h, s, ContactType::Vendor, "Home vendor exact");
    let expense = account(h, s, AccountType::Expense, "Home rent").id;
    bill(h, s, vendor, expense, day(5), "7471.90");
    let r1 = h.run(dash.receivables(today)).expect("receivables");
    let p1 = h.run(dash.payables(today, 7)).expect("payables");
    assert_eq!(diff(r1.overdue_amount, r0.overdue_amount), "12345.6700");
    assert_eq!(
        diff(r1.total_outstanding, r0.total_outstanding),
        "12345.6700"
    );
    assert_eq!(r1.overdue_count - r0.overdue_count, 1);
    assert_eq!(diff(p1.due_soon_amount, p0.due_soon_amount), "7471.9000");
    assert_eq!(p1.due_soon_count - p0.due_soon_count, 1);
    assert_eq!(p1.due_within_days, 7);
    // Four days ahead leaves the bill due in five days out.
    let p4 = h.run(dash.payables(today, 4)).expect("payables");
    assert_eq!(p4.due_within_days, 4);
    assert_eq!(diff(p1.due_soon_amount, p4.due_soon_amount), "7471.9000");
    // One overdue rule: the action items agree with the receivables.
    let a = h.run(dash.action_items(today)).expect("actions");
    assert_eq!(a.overdue_invoice_count, r1.overdue_count);
    let sum = h.run(dash.summary(today, 7)).expect("summary");
    assert_eq!(sum.as_of, today);
    assert_eq!(sum.receivables.as_ref(), Some(&r1));
    assert_eq!(sum.payables.as_ref(), Some(&p1));
    let cash = h.run(dash.cash_position(today)).expect("cash");
    assert_eq!(sum.cash_position.as_ref(), Some(&cash));
    // Home's bill count agrees with the engine's soon-due figure: overdue
    // plus due within the window.
    let due7 = BillQuery::due_within(today, 7).expect("q");
    assert_eq!(
        h.run(s.home().bills_count(&due7)).expect("bills"),
        p1.overdue_count + p1.due_soon_count
    );
}
