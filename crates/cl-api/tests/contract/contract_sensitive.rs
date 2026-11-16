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
use cl_core::report::{Cell, DateRange, ReportDates, ReportRequest, ReportTable, Source};
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
    // Unmarked even when an assert below fails: while anything is marked,
    // every unconverted report route refuses this Bookkeeper.
    let _unmark = Unmark { h, id: wages.id };

    // The account list leaves it out.
    let list = h
        .run(bk.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("list");
    assert!(list.iter().all(|a| a.id != wages.id));

    // P&L (exact, through the desktop's adapter): no wages row, the visible
    // expense only, and the notice; the owner sees both and no notice.
    let pl = ReportRequest::new(
        Source::ProfitLoss,
        ReportDates::Range(DateRange { from: day, to: day }),
    );
    let t = h.run(bk.reports().run(&pl)).expect("bookkeeper P&L");
    assert!(t.meta.hidden_notice, "the notice");
    assert!(!captions(&t).iter().any(|c| c.contains(&wages_name)));
    let seen_amounts = amounts(&t);
    assert!(
        seen_amounts.contains(&"100".to_string()),
        "{seen_amounts:?}"
    );
    assert!(
        !seen_amounts.contains(&"250".to_string()),
        "{seen_amounts:?}"
    );
    assert!(
        !seen_amounts.contains(&"350".to_string()),
        "{seen_amounts:?}"
    );
    let t = h.run(owner.reports().run(&pl)).expect("owner P&L");
    assert!(!t.meta.hidden_notice);
    assert!(captions(&t).iter().any(|c| c.contains(&wages_name)));
    assert!(amounts_of(&t, "350"), "the owner's total");

    // The bank register keeps the paycheck's own line, other side hidden;
    // the owner sees the wages account there.
    let reg = format!("/api/v1/accounts/{}/register", bank.id);
    let q = [("start_date", DAY), ("end_date", DAY), ("exact", "1")];
    let (st, page) = get(h, &bk, &reg, &q);
    assert_eq!(st, 200, "{page}");
    let lines = page["lines"].as_array().cloned().unwrap_or_default();
    assert!(
        lines.iter().any(|l| l["hiddenSplit"] == json!(true)),
        "{page}"
    );
    assert!(!page.to_string().contains(&wages_name));
    let (_, owner_page) = get(h, owner, &reg, &q);
    assert!(owner_page.to_string().contains(&wages_name), "{owner_page}");
    // The register of the hidden account itself is not there.
    let (st, _) = get(
        h,
        &bk,
        &format!("/api/v1/accounts/{}/register", wages.id),
        &[("start_date", DAY), ("end_date", DAY)],
    );
    assert_eq!(st, 404);

    // Drill-down (the General Ledger over the day): no hidden lines; the
    // owner's has them.
    let gl = ReportRequest::new(
        Source::GeneralLedger,
        ReportDates::Range(DateRange { from: day, to: day }),
    );
    let t = h.run(bk.reports().run(&gl)).expect("bookkeeper GL");
    assert!(t.meta.hidden_notice);
    assert!(!captions(&t).iter().any(|c| c.contains(&wages_name)));
    // The bank account's own side of the paycheck stays (Q-P29e), so the
    // amount still appears there; the wages side and its section total don't.
    let count = |t: &ReportTable| amounts(t).iter().filter(|a| *a == "250").count();
    let seen = count(&t);
    let t = h.run(owner.reports().run(&gl)).expect("owner GL");
    assert!(captions(&t).iter().any(|c| c.contains(&wages_name)));
    assert!(
        count(&t) > seen,
        "the owner sees the wages side too: owner {} vs bookkeeper {seen}",
        count(&t)
    );

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
}

/// Unmarks the account when dropped (also after a failed assert).
struct Unmark<'a> {
    h: &'a Harness,
    id: cl_core::ids::AccountId,
}

impl Drop for Unmark<'_> {
    fn drop(&mut self) {
        let r = self
            .h
            .run(self.h.s1.accounts().set_sensitive(self.id, false, key()));
        if let Ok(a) = r {
            assert!(!a.is_sensitive || std::thread::panicking());
        }
    }
}

fn captions(t: &ReportTable) -> Vec<String> {
    t.rows.iter().map(|r| r.caption.clone()).collect()
}

/// Every amount cell, normalized (`100.0000` reads `100`).
fn amounts(t: &ReportTable) -> Vec<String> {
    t.rows
        .iter()
        .flat_map(|r| r.cells.iter())
        .filter_map(|c| match c {
            Cell::Amount(d) => Some(d.abs().normalize().to_string()),
            _ => None,
        })
        .collect()
}

fn amounts_of(t: &ReportTable, want: &str) -> bool {
    amounts(t).iter().any(|a| a == want)
}
