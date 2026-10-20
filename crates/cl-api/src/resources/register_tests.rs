//! Register query building, exact parsing and display-only legacy amounts.

use super::*;
use crate::resources::testkit::{json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const ACCT: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TAG: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const EXACT: &str = include_str!("../../../../fixtures/engine/register-exact.json");
const LEGACY: &str = include_str!("../../../../fixtures/engine/register-legacy.json");
const SUMMARY_EXACT: &str = include_str!("../../../../fixtures/engine/register-summary-exact.json");
const SUMMARY_LEGACY: &str =
    include_str!("../../../../fixtures/engine/register-summary-legacy.json");

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn wire(a: Option<&RegisterAmount>) -> Option<String> {
    a.and_then(RegisterAmount::exact).map(Money::to_wire)
}

fn caps(exact: bool) -> Capabilities {
    if exact {
        [Capabilities::REGISTER_EXACT].into_iter().collect()
    } else {
        Capabilities::default()
    }
}

fn get<'a>(q: &'a [(String, String)], k: &str) -> Option<&'a str> {
    q.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
}

#[test]
fn both_dates_and_exact_are_always_sent() {
    let q = RegisterQuery::new(d(1900, 1, 1), d(2199, 12, 31));
    let pairs = q.to_query(&caps(false), 1, 200).expect("query");
    assert_eq!(get(&pairs, "start_date"), Some("1900-01-01"));
    assert_eq!(get(&pairs, "end_date"), Some("2199-12-31"));
    assert_eq!(get(&pairs, "exact"), Some("1"));
    assert_eq!(get(&pairs, "include_void"), Some("true"));
    assert_eq!(get(&pairs, "sort_by"), Some("date"));
    assert_eq!(get(&pairs, "sort_dir"), Some("asc"));
    assert_eq!(get(&pairs, "page"), Some("1"));
    assert_eq!(get(&pairs, "per_page"), Some("200"));
    for absent in [
        "txn_type",
        "payee",
        "search",
        "reconciled",
        "tag_id",
        "min_amount",
        "max_amount",
    ] {
        assert_eq!(get(&pairs, absent), None, "{absent}");
    }
    assert!(!q.filters_active());
}

#[test]
fn filters_are_written_exactly() {
    let mut q = RegisterQuery::new(d(2026, 9, 1), d(2026, 9, 30));
    q.txn_type = Some(TxnType::Expense);
    q.payee = Some("Sample".into());
    q.search = Some(String::new());
    q.cleared = Some(ClearedFilter::Uncleared);
    q.tag = Some(TagId::parse(TAG).expect("tag"));
    q.min = Some(m("10"));
    q.max = Some(m("1084.66"));
    q.include_void = false;
    q.sort = RegisterSort::Number;
    q.dir = SortDir::Desc;
    assert!(q.filters_active());
    let pairs = q.to_query(&caps(true), 3, 50).expect("query");
    assert_eq!(get(&pairs, "txn_type"), Some("expense"));
    assert_eq!(get(&pairs, "payee"), Some("Sample"));
    assert_eq!(get(&pairs, "search"), None, "empty search is not sent");
    assert_eq!(get(&pairs, "reconciled"), Some("uncleared"));
    assert_eq!(get(&pairs, "tag_id"), Some(TAG));
    assert_eq!(get(&pairs, "min_amount"), Some("10.0000"));
    assert_eq!(get(&pairs, "max_amount"), Some("1084.6600"));
    assert_eq!(get(&pairs, "include_void"), None);
    assert_eq!(get(&pairs, "sort_by"), Some("ref_no"));
    assert_eq!(get(&pairs, "sort_dir"), Some("desc"));
    assert_eq!(get(&pairs, "page"), Some("3"));
    for (sort, wire) in [
        (RegisterSort::Date, "date"),
        (RegisterSort::Amount, "amount"),
        (RegisterSort::Type, "type"),
    ] {
        q.sort = sort;
        let p = q.to_query(&caps(true), 1, 1).expect("query");
        assert_eq!(get(&p, "sort_by"), Some(wire));
    }
    for (c, wire) in [
        (ClearedFilter::Cleared, "cleared"),
        (ClearedFilter::Reconciled, "reconciled"),
    ] {
        q.cleared = Some(c);
        let p = q.to_query(&caps(true), 1, 1).expect("query");
        assert_eq!(get(&p, "reconciled"), Some(wire));
    }
}

#[test]
fn bad_queries_are_refused_locally() {
    let ok = RegisterQuery::new(d(2026, 9, 1), d(2026, 9, 30));
    let backwards = RegisterQuery::new(d(2026, 9, 30), d(2026, 9, 1));
    assert_eq!(
        backwards.to_query(&caps(true), 1, 10),
        Err(LocalValidation::BadRange)
    );
    let mut amounts = ok.clone();
    amounts.min = Some(m("20"));
    amounts.max = Some(m("10"));
    assert_eq!(
        amounts.to_query(&caps(true), 1, 10),
        Err(LocalValidation::BadRange)
    );
    amounts.min = Some(m("-1"));
    amounts.max = None;
    assert_eq!(
        amounts.to_query(&caps(true), 1, 10),
        Err(LocalValidation::InvalidValue)
    );
    for (page, per) in [(0, 10), (1, 0), (1, 201)] {
        assert_eq!(
            ok.to_query(&caps(true), page, per),
            Err(LocalValidation::PageLimit)
        );
    }
    let mut cleared = ok.clone();
    cleared.cleared = Some(ClearedFilter::Cleared);
    assert_eq!(
        cleared.to_query(&caps(false), 1, 10),
        Err(LocalValidation::RegisterFilterUnavailable)
    );
    let mut tagged = ok;
    tagged.tag = Some(TagId::parse(TAG).expect("tag"));
    assert_eq!(
        tagged.to_query(&caps(false), 1, 10),
        Err(LocalValidation::RegisterFilterUnavailable)
    );
}

#[test]
fn exact_fixture_parses_every_amount_exactly() {
    let p = page_from_json(EXACT.as_bytes()).expect("page");
    assert_eq!(p.account.id, AccountId::parse(ACCT).expect("id"));
    assert_eq!(p.account.account_type, AccountType::Asset);
    assert_eq!(p.account.detail_type.as_deref(), Some("bank"));
    assert!(p.exact);
    assert_eq!(wire(Some(&p.balance_forward)).as_deref(), Some("1000.0000"));
    assert_eq!(
        wire(p.ending_balance.as_ref()).as_deref(),
        Some("2084.6612")
    );
    assert_eq!(
        (p.page, p.per_page, p.total_rows, p.total_pages),
        (1, 200, 4, 1)
    );
    assert_eq!(
        p.allowed,
        vec![
            EntryType::Expense,
            EntryType::Deposit,
            EntryType::Transfer,
            EntryType::JournalEntry,
            EntryType::Other("bulk_import".into()),
        ]
    );
    let l = &p.lines;
    assert_eq!(l.len(), 4);
    assert_eq!(wire(l[0].payment.as_ref()).as_deref(), Some("15.3388"));
    assert!(l[0].deposit.is_none());
    assert_eq!(wire(Some(&l[0].balance)).as_deref(), Some("984.6612"));
    assert_eq!(l[0].balance.display(false).as_deref(), Some("984.66"));
    assert_eq!(l[0].balance.display(true).as_deref(), Some("-984.66"));
    assert_eq!(l[0].cleared, ClearState::Reconciled);
    assert_eq!(l[0].check_number, Some(1042));
    assert!(!l[0].engine_editable);
    assert!(
        matches!(&l[0].category, Category::Account { name: Some(n), .. } if n == "Office Supplies")
    );
    assert_eq!(l[1].category, Category::Split);
    assert_eq!(wire(l[1].deposit.as_ref()).as_deref(), Some("1100.0000"));
    assert_eq!(l[1].cleared, ClearState::Cleared);
    assert!(l[1].has_splits);
    assert_eq!(l[2].status, TxnStatus::Void);
    assert_eq!(l[2].number.as_deref(), Some("1043"));
    assert_eq!(l[2].cleared, ClearState::None);
    assert_eq!(l[3].status, TxnStatus::Draft);
    assert_eq!(l[3].category, Category::None);
    assert_eq!(wire(l[3].payment.as_ref()).as_deref(), Some("12.0456"));
    assert_eq!(
        l[3].payment
            .as_ref()
            .and_then(|a| a.display(false))
            .as_deref(),
        Some("12.05")
    );
}

#[test]
fn json_number_money_is_display_only_never_money() {
    let p = page_from_json(LEGACY.as_bytes()).expect("legacy page");
    assert!(!p.exact, "numbers mark the page display-only");
    assert!(p.ending_balance.is_none());
    assert!(p.balance_forward.exact().is_none());
    assert_eq!(p.balance_forward.display(false).as_deref(), Some("1000.00"));
    let l = &p.lines[0];
    assert!(l.payment.as_ref().is_some_and(|a| !a.is_exact()));
    assert_eq!(l.payment.as_ref().and_then(RegisterAmount::exact), None);
    assert_eq!(
        l.payment.as_ref().and_then(|a| a.display(false)).as_deref(),
        Some("1084.66")
    );
    // "-0" shows as an unsigned zero, either sign.
    assert_eq!(l.balance.display(false).as_deref(), Some("0.00"));
    assert_eq!(l.balance.display(true).as_deref(), Some("0.00"));

    // Any one number on an exact page makes the page display-only; the rest
    // stay exact.
    let base: serde_json::Value = serde_json::from_str(EXACT).expect("json");
    for (ptr, num, shown) in [
        ("/balanceForward", "1084.66", "1084.66"),
        ("/endingBalance", "-0", "0.00"),
        ("/lines/0/payment", "1e21", "1000000000000000000000.00"),
        ("/lines/1/deposit", "1100", "1100.00"),
        ("/lines/2/runningBalance", "0.30000000000000004", "0.30"),
    ] {
        let mut v = base.clone();
        *v.pointer_mut(ptr).expect(ptr) = serde_json::from_str(num).expect(num);
        let body = serde_json::to_vec(&v).expect("body");
        let p = page_from_json(&body).expect(ptr);
        assert!(!p.exact, "{ptr}");
        let a = match ptr {
            "/balanceForward" => Some(&p.balance_forward),
            "/endingBalance" => p.ending_balance.as_ref(),
            "/lines/0/payment" => p.lines[0].payment.as_ref(),
            "/lines/1/deposit" => p.lines[1].deposit.as_ref(),
            _ => Some(&p.lines[2].balance),
        }
        .expect("amount");
        assert!(a.exact().is_none(), "{ptr}");
        assert_eq!(a.display(false).as_deref(), Some(shown), "{ptr}");
        assert!(p.lines[3].balance.is_exact(), "{ptr}: the rest stay exact");
    }
    // Strings outside the engine grammar, and non-numbers, are a decode
    // error, not a guess.
    for (ptr, bad) in [
        ("/lines/0/payment", "\"1e3\""),
        ("/lines/0/payment", "\"12.00001\""),
        ("/lines/0/payment", "true"),
        ("/lines/0/runningBalance", "null"),
        ("/lines/0/reconciliationStatus", "\"maybe\""),
    ] {
        let mut v = base.clone();
        *v.pointer_mut(ptr).expect(ptr) = serde_json::from_str(bad).expect(bad);
        let body = serde_json::to_vec(&v).expect("body");
        assert!(
            matches!(page_from_json(&body), Err(ApiError::Decode { .. })),
            "{ptr} {bad}"
        );
    }
    // An exact page must carry its ending balance.
    let mut v = base.clone();
    v.as_object_mut().expect("obj").remove("endingBalance");
    let body = serde_json::to_vec(&v).expect("body");
    assert!(matches!(
        page_from_json(&body),
        Err(ApiError::Decode { .. })
    ));

    let s = summary_from_json(SUMMARY_LEGACY.as_bytes()).expect("legacy summary");
    assert!(!s.exact);
    assert!(s.current_balance.exact().is_none());
    assert_eq!(s.current_balance.display(false).as_deref(), Some("2084.66"));
    assert!(s.ending_balance.is_none() && s.cleared_balance.is_none());
    assert_eq!(s.uncleared_count, 2);
}

#[test]
fn summary_fixture_parses_exactly() {
    let s = summary_from_json(SUMMARY_EXACT.as_bytes()).expect("summary");
    assert!(s.exact);
    assert_eq!(wire(Some(&s.current_balance)).as_deref(), Some("2084.6612"));
    assert_eq!(
        wire(s.ending_balance.as_ref()).as_deref(),
        Some("2084.6612")
    );
    assert_eq!(
        wire(s.cleared_balance.as_ref()).as_deref(),
        Some("2069.3224")
    );
    assert_eq!(s.as_of, Some(d(2026, 9, 30)));
    assert_eq!(s.uncleared_count, 2);
    assert_eq!(s.last_reconciled, Some(d(2026, 8, 31)));
}

#[test]
fn debug_shows_no_amounts_names_or_memos() {
    let p = page_from_json(EXACT.as_bytes()).expect("page");
    let s = summary_from_json(SUMMARY_EXACT.as_bytes()).expect("summary");
    let mut q = RegisterQuery::new(d(2026, 9, 1), d(2026, 9, 30));
    q.payee = Some("Sample Supply Co".into());
    q.min = Some(m("15.3388"));
    let legacy = page_from_json(LEGACY.as_bytes()).expect("legacy");
    let shown = format!(
        "{p:?} {:?} {s:?} {q:?} {legacy:?} {:?}",
        p.lines, legacy.lines
    );
    for secret in [
        "15.3388", "984", "1100", "2084", "2069", "1084", "Sample", "Paper", "Office", "accrual",
    ] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

#[tokio::test]
async fn page_and_summary_requests() {
    let server = MockServer::start().await;
    let reg = format!("/api/v1/accounts/{ACCT}/register");
    Mock::given(method("GET"))
        .and(path(reg.as_str()))
        .respond_with(json(200, EXACT))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{reg}/summary").as_str()))
        .respond_with(json(200, SUMMARY_EXACT))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::REGISTER_EXACT]).await;
    let acct = AccountId::parse(ACCT).expect("id");
    let q = RegisterQuery::new(d(2026, 9, 1), d(2026, 9, 30));
    let p = s.register().page(acct, &q, 1, 200).await.expect("page");
    assert_eq!(p.lines.len(), 4);
    let sum = s
        .register()
        .summary(acct, Some(d(2026, 9, 30)))
        .await
        .expect("summary");
    assert_eq!(sum.uncleared_count, 2);
    let gets: Vec<String> = requests(&server, "GET")
        .await
        .iter()
        .filter(|r| r.url.path().contains("/register"))
        .map(query)
        .collect();
    assert_eq!(gets.len(), 2);
    assert!(gets[0].contains("start_date=2026-09-01"), "{}", gets[0]);
    assert!(gets[0].contains("end_date=2026-09-30"), "{}", gets[0]);
    assert!(gets[0].contains("exact=1"), "{}", gets[0]);
    assert_eq!(gets[1], "exact=1&end_date=2026-09-30");

    // A refused query sends nothing.
    let before = server.received_requests().await.expect("r").len();
    let mut tagged = q.clone();
    tagged.tag = Some(TagId::parse(TAG).expect("tag"));
    tagged.start = d(2027, 1, 1);
    let e = s
        .register()
        .page(acct, &tagged, 1, 200)
        .await
        .expect_err("bad");
    assert_eq!(e, ApiError::Local(LocalValidation::BadRange));
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

#[tokio::test]
async fn an_engine_without_exact_money_gives_display_only_pages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/accounts/{ACCT}/register").as_str()))
        .respond_with(json(200, LEGACY))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let q = RegisterQuery::new(d(2026, 9, 1), d(2026, 9, 30));
    let p = s
        .register()
        .page(AccountId::parse(ACCT).expect("id"), &q, 1, 50)
        .await
        .expect("display-only page");
    assert!(!p.exact);
    assert_eq!(p.lines.len(), 1);
}

#[test]
fn display_rounds_half_away_and_flips_text_sign() {
    for (w, plain, neg) in [
        ("0.005", "0.01", "-0.01"),
        ("-0.005", "-0.01", "0.01"),
        ("0.0049", "0.00", "0.00"),
        ("-0.0001", "0.00", "0.00"),
        ("1234567.8950", "1234567.90", "-1234567.90"),
    ] {
        let a = RegisterAmount::Exact(m(w));
        assert_eq!(a.display(false).as_deref(), Some(plain), "{w}");
        assert_eq!(a.display(true).as_deref(), Some(neg), "{w}");
        let r: ReportAmount = serde_json::from_str(w).expect("num");
        let d = RegisterAmount::DisplayOnly(r);
        assert_eq!(d.display(false).as_deref(), Some(plain), "{w}");
        assert_eq!(d.display(true).as_deref(), Some(neg), "{w}");
    }
    assert_eq!(
        format!("{:?}", RegisterAmount::Exact(m("12.34"))),
        "RegisterAmount::Exact(..)"
    );
}
