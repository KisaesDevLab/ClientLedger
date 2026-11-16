//! P29 acceptance 3 (U-33, `sensitive-accounts-v1`): a payroll account the
//! Admin marks sensitive is hidden from a Bookkeeper without the right in
//! the P&L, the register and the drill-down (General Ledger), and the Admin
//! still sees it. Amounts are exact strings; nothing is added up here
//! except to compare with what the engine sent.
//!
//! The marked account is unmarked at the end: while anything is marked,
//! every unconverted report route refuses this Bookkeeper (S-L7), which
//! would change other tests in the same run.

use cl_api::{AccountType, ApiError, Capabilities, Method, SensitiveRefusal, Session};
use serde_json::{Value, json};

use crate::common::{account, je2, key, post};
use crate::support::{Harness, unique};

/// A Bookkeeper who may do everything a Bookkeeper does but has no
/// "See sensitive accounts" (an override map listing it as none).
fn as_restricted_bookkeeper(h: &Harness) -> Session {
    let (user, bk) = h.bookkeeper();
    let mut map = serde_json::Map::new();
    for k in [
        "transactions",
        "batch_entry",
        "recurring",
        "invoices",
        "receive_payment",
        "bills",
        "pay_bills",
        "checks",
        "banking",
        "accounts",
        "contacts",
        "tags",
    ] {
        map.insert(k.into(), json!("full"));
    }
    for k in [
        "reports",
        "financial_reports",
        "dashboard",
        "company_settings",
    ] {
        map.insert(k.into(), json!("view"));
    }
    map.insert("sensitive_accounts".into(), json!("none"));
    let r = h
        .run(h.s1.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": Value::Object(map) })),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
    bk
}

fn get(h: &Harness, s: &Session, route: &str, q: &[(&str, &str)]) -> (u16, Value) {
    let r = h.run(s.raw(Method::Get, route, q, None)).expect("raw");
    (r.status, r.body)
}

const DAY: &str = "2033-07-14";

/// Acceptance 3: P&L, register and drill-down hide the payroll account from
/// the Bookkeeper; the owner still sees it.
pub fn payroll_hidden_from_bookkeeper(h: &Harness) {
    if !h.require_cap(Capabilities::ACCOUNT_SENSITIVE, "U-33") {
        return;
    }
    let owner = &h.s1;
    let bank = account(h, owner, AccountType::Asset, "Payroll bank");
    let supplies = account(h, owner, AccountType::Expense, "Visible supplies");
    let wages_name = unique("Payroll Wages");
    let wages = h
        .run(owner.accounts().create(
            &cl_api::AccountFields {
                name: wages_name.clone(),
                account_number: None,
                account_type: AccountType::Expense,
                detail_type: None,
                description: None,
                parent_id: None,
            },
            key(),
        ))
        .expect("wages")
        .record;
    let day = cl_core::CalendarDate::parse_wire(DAY).expect("day");
    post(
        h,
        owner,
        &je2(day, "supplies", supplies.id, bank.id, "100.00"),
    );
    let paycheck = post(h, owner, &je2(day, "paycheck", wages.id, bank.id, "250.00"));

    let bk = as_restricted_bookkeeper(h);
    // Only the Admin marks it.
    let refused = h
        .run(bk.accounts().set_sensitive(wages.id, true, key()))
        .expect_err("a Bookkeeper can't mark");
    assert!(matches!(refused, ApiError::Forbidden { .. }), "{refused:?}");
    let marked = h
        .run(owner.accounts().set_sensitive(wages.id, true, key()))
        .expect("owner marks");
    assert!(marked.is_sensitive);

    // The account list leaves it out.
    let list = h
        .run(bk.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("list");
    assert!(list.iter().all(|a| a.id != wages.id));

    // P&L: no wages row, the visible expense only, and the notice.
    let range = [("startDate", DAY), ("endDate", DAY)];
    let (st, pl) = get(h, &bk, "/api/v1/reports/profit-loss", &range);
    assert_eq!(st, 200, "{pl}");
    let text = pl.to_string();
    assert!(!text.contains(&wages_name), "no hidden name");
    assert!(
        !text.contains("250.00") && !text.contains("350.00"),
        "no hidden amount"
    );
    assert!(text.contains("100.00"), "the visible expense");
    assert_eq!(pl["hiddenNotice"], json!(true));
    // The owner still sees it, and gets no notice.
    let (_, owner_pl) = get(h, owner, "/api/v1/reports/profit-loss", &range);
    let owner_text = owner_pl.to_string();
    assert!(owner_text.contains(&wages_name));
    assert!(owner_text.contains("350.00"), "owner total");
    assert!(owner_pl.get("hiddenNotice").is_none_or(Value::is_null));

    // The bank register keeps the paycheck's own line, other side hidden.
    let reg = format!("/api/v1/accounts/{}/register", bank.id);
    let (st, page) = get(
        h,
        &bk,
        &reg,
        &[("start_date", DAY), ("end_date", DAY), ("exact", "1")],
    );
    assert_eq!(st, 200, "{page}");
    let lines = page["lines"].as_array().cloned().unwrap_or_default();
    assert!(
        lines.iter().any(|l| l["hiddenSplit"] == json!(true)),
        "{page}"
    );
    assert!(!page.to_string().contains(&wages_name));
    // The register of the hidden account itself is not there.
    let (st, _) = get(
        h,
        &bk,
        &format!("/api/v1/accounts/{}/register", wages.id),
        &[("start_date", DAY), ("end_date", DAY)],
    );
    assert_eq!(st, 404);

    // Drill-down (General Ledger over the day): no hidden lines.
    let (st, gl) = get(h, &bk, "/api/v1/reports/general-ledger", &range);
    assert_eq!(st, 200, "{gl}");
    assert!(!gl.to_string().contains(&wages_name));
    assert!(!gl.to_string().contains("250.00"));

    // The paycheck can't be opened or posted to by the Bookkeeper.
    let open = h
        .run(bk.transactions().get(paycheck.record.id))
        .expect_err("refused");
    assert_eq!(open, ApiError::Sensitive(SensitiveRefusal::Transaction));
    let posted = h
        .run(bk.transactions().create_journal_entry(
            &je2(day, "more pay", wages.id, bank.id, "1.00"),
            &cl_api::PostingOptions::default(),
            key(),
        ))
        .expect_err("refused");
    assert_eq!(posted, ApiError::Sensitive(SensitiveRefusal::Transaction));

    // Clean up: nothing marked for the rest of the run.
    let unmarked = h
        .run(owner.accounts().set_sensitive(wages.id, false, key()))
        .expect("unmark");
    assert!(!unmarked.is_sensitive);
}
