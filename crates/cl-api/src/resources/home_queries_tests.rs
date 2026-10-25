//! Stub tests of Home's counts: only `total` (or a status) is read, the
//! query names follow the engine's filters, and no amount is decoded.

use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer};

use super::*;
use crate::resources::testkit::{company_session, json as reply, query, requests};

const ACC1: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const ACC2: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";

fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("date")
}

fn acc(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

#[test]
fn bill_queries_follow_the_local_date() {
    let today = d(2026, 11, 4);
    let due = BillQuery::due_within(today, 7).expect("due");
    assert_eq!(due.due_on_or_before, Some(d(2026, 11, 11)));
    assert_eq!(
        due.query(),
        [
            (
                "billStatus".to_string(),
                "unpaid,partial,overdue".to_string()
            ),
            ("dueOnOrBefore".into(), "2026-11-11".into()),
        ]
    );
    let overdue = BillQuery::overdue(today).expect("overdue");
    assert_eq!(overdue.due_on_or_before, Some(d(2026, 11, 3)));
    let engine_rule = BillQuery {
        status: vec![],
        due_on_or_before: None,
        overdue_only: true,
    };
    assert_eq!(
        engine_rule.query(),
        [("overdueOnly".to_string(), "true".to_string())]
    );
    assert!(BillQuery::due_within(CalendarDate::max(), 1).is_err());
    assert!(BillQuery::overdue(CalendarDate::min()).is_err());
    assert_eq!(
        PayrollStatus::UNPOSTED.map(PayrollStatus::as_wire),
        ["uploaded", "mapped", "validated"]
    );
}

#[test]
fn totals_and_counts_ignore_the_rows() {
    // Row amounts may be floats on old engines: they are never decoded.
    let bills = br#"{"data":[{"id":"x","total":12.5,"balanceDue":12.5}],"total":3}"#;
    assert_eq!(total_of(bills, "bills").expect("total"), 3);
    assert!(total_of(br#"{"data":[],"total":"3"}"#, "bills").is_err());
    assert!(total_of(br#"{"data":[]}"#, "bills").is_err());
    let actions = br#"{"pendingFeedCount":37,"overdueInvoiceCount":2,"pendingDepositAmount":250.5,
        "printQueueAmount":0,"staleReconciliations":[]}"#;
    assert_eq!(feed_count_from(actions).expect("count"), 37);
    assert!(feed_count_from(br#"{"pendingFeedCount":"37"}"#).is_err());
}

#[test]
fn reconciliations_fold_open_and_latest_finished() {
    let mut st = ReconcileState::default();
    let a = acc(ACC1);
    let b = acc(ACC2);
    add_reconciliations(
        &mut st,
        a,
        br#"{"reconciliations":[
            {"id":"r1","status":"in_progress","statementDate":"2026-10-31","statementEndingBalance":"10.0000"},
            {"id":"r2","status":"complete","statementDate":"2026-09-30","statementEndingBalance":10}]}"#,
    )
    .expect("a");
    add_reconciliations(
        &mut st,
        b,
        br#"{"reconciliations":[
            {"id":"r3","status":"complete","statementDate":"2026-08-31"},
            {"id":"r4","status":"cancelled","statementDate":"2026-10-31"}]}"#,
    )
    .expect("b");
    assert_eq!(st.in_progress, vec![a]);
    assert_eq!(
        st.last_completed,
        Some(LastReconciled {
            account: a,
            statement_date: d(2026, 9, 30)
        })
    );
    assert!(add_reconciliations(&mut st, a, br#"{"data":[]}"#).is_err());
}

/// Answers for every list Home counts.
async fn mount_lists(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/bills"))
        .respond_with(reply(
            200,
            r#"{"data":[{"status":"posted","total":1.5},{"status":"void"},{"status":"posted"},{}],"total":4}"#,
        ))
        .mount(server)
        .await;
    for (status, n) in [("uploaded", 1), ("mapped", 0), ("validated", 2)] {
        Mock::given(method("GET"))
            .and(path("/api/v1/payroll-import/sessions"))
            .and(query_param("status", status))
            .respond_with(reply(200, &format!(r#"{{"data":[],"total":{n}}}"#)))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/reconciliations"))
        .and(query_param("account_id", ACC1))
        .respond_with(reply(
            200,
            r#"{"reconciliations":[{"status":"in_progress","statementDate":"2026-10-31"}]}"#,
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/reconciliations"))
        .and(query_param("account_id", ACC2))
        .respond_with(reply(200, r#"{"reconciliations":[]}"#))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/feed"))
        .respond_with(reply(200, r#"{"data":[],"total":0}"#))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/transactions"))
        .respond_with(reply(
            200,
            r#"{"data":[{"id":"t","total":5.5}],"total":12}"#,
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/dashboard/action-items"))
        .respond_with(reply(
            200,
            r#"{"pendingFeedCount":37,"pendingDepositAmount":250.5}"#,
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn each_count_asks_the_engine_list_for_its_total() {
    let server = MockServer::start().await;
    mount_lists(&server).await;
    let s = company_session(&server);
    let home = s.home();
    let today = d(2026, 11, 4);
    let q = BillQuery::due_within(today, 7).expect("q");
    assert_eq!(
        home.bills_count(&q).await.expect("bills"),
        3,
        "the voided bill left out"
    );
    assert_eq!(
        home.payroll_sessions_count(&PayrollStatus::UNPOSTED)
            .await
            .expect("payroll"),
        3
    );
    let st = home
        .reconcile_state(&[acc(ACC1), acc(ACC2)])
        .await
        .expect("reconcile");
    assert_eq!(st.in_progress, vec![acc(ACC1)]);
    assert_eq!(st.last_completed, None);
    assert!(!home.bank_feed_any().await.expect("feed items"));
    assert!(home.has_any_transaction().await.expect("txns"));
    assert_eq!(home.feed_pending_count(today).await.expect("feed"), 37);
    let too_many = vec![acc(ACC1); RECONCILE_ACCOUNTS_MAX + 1];
    assert_eq!(
        home.reconcile_state(&too_many).await.err(),
        Some(ApiError::Local(LocalValidation::PageLimit))
    );
    let asked: Vec<(String, String)> = requests(&server, "GET")
        .await
        .iter()
        .map(|r| (r.url.path().to_string(), query(r)))
        .collect();
    assert_eq!(
        asked,
        [
            (
                "/api/v1/bills".to_string(),
                "billStatus=unpaid%2Cpartial%2Coverdue&dueOnOrBefore=2026-11-11&limit=500&offset=0"
                    .to_string()
            ),
            (
                "/api/v1/payroll-import/sessions".into(),
                "status=uploaded&limit=1".into()
            ),
            (
                "/api/v1/payroll-import/sessions".into(),
                "status=mapped&limit=1".into()
            ),
            (
                "/api/v1/payroll-import/sessions".into(),
                "status=validated&limit=1".into()
            ),
            (
                "/api/v1/banking/reconciliations".into(),
                format!("account_id={ACC1}")
            ),
            (
                "/api/v1/banking/reconciliations".into(),
                format!("account_id={ACC2}")
            ),
            ("/api/v1/banking/feed".into(), "limit=1".into()),
            (
                "/api/v1/transactions".into(),
                "status=posted&limit=1".into()
            ),
            (
                "/api/v1/dashboard/action-items".into(),
                "exact=1&asOf=2026-11-04".into()
            ),
        ]
    );
}

#[test]
fn a_bill_page_counts_posted_rows_only() {
    let page = br#"{"data":[{"status":"posted","balanceDue":7.25},{"status":"void"},{"status":"draft"},{}],"total":9}"#;
    assert_eq!(count_bill_page(page).expect("page"), (2, 4, 9));
    assert!(count_bill_page(br#"{"data":[{"status":1}],"total":1}"#).is_err());
    assert!(count_bill_page(br#"{"total":1}"#).is_err());
}

#[test]
fn reconcile_batches_merge() {
    let (a, b) = (acc(ACC1), acc(ACC2));
    let mut st = ReconcileState {
        in_progress: vec![a],
        last_completed: Some(LastReconciled {
            account: a,
            statement_date: d(2026, 8, 31),
        }),
    };
    st.merge(ReconcileState {
        in_progress: vec![b],
        last_completed: Some(LastReconciled {
            account: b,
            statement_date: d(2026, 9, 30),
        }),
    });
    st.merge(ReconcileState::default());
    assert_eq!(st.in_progress, vec![a, b]);
    assert_eq!(st.last_completed.map(|l| l.account), Some(b));
}

/// Pages of 500 until the list's total; past the page cap a dash (an
/// error), never a short count.
#[tokio::test]
async fn bills_count_pages_and_stops_at_the_cap() {
    let server = MockServer::start().await;
    let rows = |n: usize, status: &str| {
        let row = format!(r#"{{"status":"{status}"}}"#);
        vec![row; n].join(",")
    };
    let first = format!(r#"{{"data":[{}],"total":501}}"#, rows(500, "posted"));
    Mock::given(method("GET"))
        .and(path("/api/v1/bills"))
        .and(query_param("offset", "0"))
        .respond_with(reply(200, &first))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/bills"))
        .and(query_param("offset", "500"))
        .respond_with(reply(200, r#"{"data":[{"status":"void"}],"total":501}"#))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let q = BillQuery::due_within(d(2026, 11, 4), 7).expect("q");
    assert_eq!(s.home().bills_count(&q).await.expect("count"), 500);
    assert_eq!(requests(&server, "GET").await.len(), 2);

    // A list that never ends (every page full): stops at the cap.
    let endless = MockServer::start().await;
    let full = format!(r#"{{"data":[{}],"total":1000000}}"#, rows(500, "posted"));
    Mock::given(method("GET"))
        .and(path("/api/v1/bills"))
        .respond_with(reply(200, &full))
        .mount(&endless)
        .await;
    let s = company_session(&endless);
    assert_eq!(
        s.home().bills_count(&q).await.err(),
        Some(ApiError::Local(LocalValidation::PageLimit))
    );
    assert_eq!(
        requests(&endless, "GET").await.len(),
        usize::try_from(BILLS_PAGES_MAX).expect("pages")
    );
}
