//! Export IIF File against a stub engine (wiremock), P32 review B2: the
//! engine path of [`export_iif`] — the ledger-version guard (T9), the
//! hidden-lines abort, the first/last posted dates (`bound`), month windows
//! and their query names, lines of one transaction interleaved in a window
//! grouped together (`split_rows`), and a 429 retried.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use cl_api::{Client, ClientConfig, EndpointSource, FixedEndpoint, LoginOutcome, Session};
use cl_company::iif::fetch::{IifRequest, export_iif, partial_path};
use cl_company::iif::{IifError, Options};
use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Secret};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "0a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00";
const T1: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CO: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig";
const BANK: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e0001";
const SALES: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e0002";
const TXN_A: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e0001";
const TXN_B: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e0002";
const TXN_C: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e0003";
const LIST: &str = "/api/v1/reports/transaction-list";

fn json_reply(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

/// Sign-in, the open company, the lists, and the first (January) and
/// last (February) posted dates.
async fn engine() -> MockServer {
    let server = MockServer::start().await;
    let tokens = json_reply(
        200,
        json!({"user":{"id":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"},"tokens":{"accessToken":JWT},
               "accessibleTenants":[{"tenantId":T1,"tenantName":"Ozark Ridge","role":"owner"}]}),
    )
    .insert_header(
        "set-cookie",
        "kb_refresh=refresh_token_value_1234; Path=/; HttpOnly",
    );
    let routes: Vec<(&str, &str, ResponseTemplate)> = vec![
        (
            "GET",
            "/api/v1/desktop/info",
            json_reply(
                200,
                json!({"mode":"desktop","initialized":true,"workspaceTenantId":WS,
                       "capabilities":["reports-exact-v1"]}),
            ),
        ),
        ("POST", "/api/v1/auth/login", tokens.clone()),
        ("POST", "/api/v1/auth/switch-tenant", tokens),
        (
            "GET",
            "/api/v1/auth/me",
            json_reply(
                200,
                json!({"accessibleTenants":[
                    {"tenantId":WS,"tenantName":"workspace","role":"owner"},
                    {"tenantId":T1,"tenantName":"Ozark Ridge","role":"owner"}]}),
            ),
        ),
        (
            "GET",
            "/api/v1/company",
            json_reply(
                200,
                json!({"company":{"id":CO,"businessName":"Ozark Ridge","entityType":"single_member_llc","country":"US"}}),
            ),
        ),
        (
            "GET",
            "/api/v1/accounts",
            json_reply(
                200,
                json!({"data":[
                    {"id":BANK,"accountNumber":"1000","name":"Checking","accountType":"asset",
                     "detailType":"bank","isActive":true,"isSystem":false},
                    {"id":SALES,"accountNumber":"4000","name":"Sales","accountType":"revenue",
                     "isActive":true,"isSystem":false}],
                    "total":2,"limit":500,"offset":0}),
            ),
        ),
        (
            "GET",
            "/api/v1/contacts",
            json_reply(200, json!({"data":[],"total":0,"limit":500,"offset":0})),
        ),
        (
            "GET",
            "/api/v1/items",
            json_reply(200, json!({"data":[],"total":0,"limit":500,"offset":0})),
        ),
    ];
    for (m, p, reply) in routes {
        Mock::given(method(m))
            .and(path(p))
            .respond_with(reply)
            .mount(&server)
            .await;
    }
    for (dir, date, id) in [("asc", "2026-01-05", TXN_A), ("desc", "2026-02-10", TXN_C)] {
        Mock::given(method("GET"))
            .and(path("/api/v1/transactions"))
            .and(query_param("sortDir", dir))
            .and(query_param("status", "posted"))
            .respond_with(json_reply(
                200,
                json!({"data":[{"id":id,"txnType":"journal_entry","txnDate":date,"status":"posted"}],
                       "total":3,"limit":1,"offset":0}),
            ))
            .mount(&server)
            .await;
    }
    server
}

/// A ledger-version stamp: `first` once, then `then` for every later read.
async fn stamps(server: &MockServer, first: u64, then: u64) {
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json_reply(200, json!({"stamp":first})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json_reply(200, json!({"stamp":then})))
        .with_priority(2)
        .mount(server)
        .await;
}

fn row(txn: &str, line: &str, account: &str, date: &str, amount: &str) -> Value {
    json!({"id":txn,"lineId":line,"accountId":account,"contactId":null,
           "txn_type":"journal_entry","txn_number":null,"txn_date":date,
           "memo":null,"line_description":null,"amountExact":amount})
}

/// January: two entries whose lines arrive interleaved (A1, B1, A2, B2).
fn january(hidden: bool) -> Value {
    json!({"data":[
        row(TXN_A, "a1", BANK, "2026-01-05", "100.0000"),
        row(TXN_B, "b1", BANK, "2026-01-20", "25.5000"),
        row(TXN_A, "a2", SALES, "2026-01-05", "-100.0000"),
        row(TXN_B, "b2", SALES, "2026-01-20", "-25.5000"),
    ], "hiddenNotice": hidden})
}

/// February: one entry.
fn february() -> Value {
    json!({"data":[
        row(TXN_C, "c1", BANK, "2026-02-10", "7.2500"),
        row(TXN_C, "c2", SALES, "2026-02-10", "-7.2500"),
    ], "hiddenNotice": false})
}

/// The two month windows: January has no start, February no end.
async fn windows(server: &MockServer, jan: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("end_date", "2026-01-31"))
        .respond_with(jan)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("start_date", "2026-02-01"))
        .respond_with(json_reply(200, february()))
        .mount(server)
        .await;
}

async fn session(server: &MockServer) -> Session {
    let src: Arc<dyn EndpointSource> = Arc::new(FixedEndpoint::new(
        server.address().port(),
        Secret::new("launch-token"),
        1,
    ));
    let client = Client::new(src, ClientConfig::default()).expect("client");
    client.desktop_info().await.expect("info");
    match client
        .login("owner@example.com", &Secret::new("password-123456"))
        .await
        .expect("login")
    {
        LoginOutcome::Session(s) => {
            s.open_company(TenantId::parse(T1).expect("id"))
                .await
                .expect("open");
            s
        }
        LoginOutcome::TfaRequired(_) => panic!("tfa"),
    }
}

fn request(dir: &Path) -> IifRequest {
    IifRequest {
        dest: dir.join("Ozark.iif"),
        start: None,
        end: None,
        options: Options::default(),
        sees_hidden: true,
    }
}

async fn run(
    server: &MockServer,
    req: &IifRequest,
) -> Result<cl_company::iif::fetch::IifOutcome, IifError> {
    let s = session(server).await;
    export_iif(
        &s,
        req,
        &cl_company::backup::silent(),
        &AtomicBool::new(false),
    )
    .await
}

fn nothing_written(req: &IifRequest) -> bool {
    !req.dest.exists() && !partial_path(&req.dest).exists()
}

/// The query of every transaction-list read, in order.
async fn list_queries(server: &MockServer) -> Vec<Vec<(String, String)>> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.url.path() == LIST)
        .map(|r| {
            let mut q: Vec<(String, String)> = r.url.query_pairs().into_owned().collect();
            q.sort();
            q
        })
        .collect()
}

fn pairs(p: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = p
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    v.sort();
    v
}

/// Interleaved lines are grouped by transaction; the windows are January
/// (no start) and February (no end) with the engine's parameter names.
#[tokio::test]
async fn iif_engine_interleaved_lines_are_grouped_and_windows_are_named() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine().await;
    stamps(&server, 7, 7).await;
    windows(&server, json_reply(200, january(false))).await;
    let req = request(dir.path());
    let out = run(&server, &req).await.expect("export");
    assert_eq!(out.counts.transactions, 3, "A and B grouped, plus C");
    assert_eq!(out.counts.lines, 6);
    let text = std::fs::read_to_string(&req.dest).expect("file");
    // Each TRNS block is one transaction: A's split follows A's header.
    let trns: Vec<&str> = text
        .lines()
        .filter(|l| l.starts_with("TRNS\t") || l.starts_with("SPL\t"))
        .collect();
    assert_eq!(trns.len(), 6, "{text}");
    assert!(trns[0].starts_with("TRNS") && trns[1].starts_with("SPL"));
    assert!(
        trns[0].contains("100.00") && trns[1].contains("-100.00"),
        "{text}"
    );
    assert!(
        trns[2].contains("25.50") && trns[3].contains("-25.50"),
        "{text}"
    );
    assert!(!partial_path(&req.dest).exists());
    let qs = list_queries(&server).await;
    assert_eq!(
        qs,
        vec![
            pairs(&[
                ("exact", "1"),
                ("basis", "accrual"),
                ("end_date", "2026-01-31")
            ]),
            pairs(&[
                ("exact", "1"),
                ("basis", "accrual"),
                ("start_date", "2026-02-01")
            ]),
        ],
        "the first window has no start, the last no end"
    );
}

/// T9: a ledger-version change between the first and last read aborts,
/// and nothing is written.
#[tokio::test]
async fn iif_engine_books_changed_writes_nothing() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine().await;
    stamps(&server, 7, 8).await;
    windows(&server, json_reply(200, january(false))).await;
    let req = request(dir.path());
    let e = run(&server, &req).await.expect_err("changed");
    assert!(matches!(e, IifError::BooksChanged), "{e:?}");
    assert!(nothing_written(&req));
}

/// Q-P32i: a window that hid lines aborts; nothing is written.
#[tokio::test]
async fn iif_engine_hidden_notice_aborts() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine().await;
    stamps(&server, 7, 7).await;
    windows(&server, json_reply(200, january(true))).await;
    let req = request(dir.path());
    let e = run(&server, &req).await.expect_err("hidden");
    assert!(matches!(e, IifError::HiddenAccounts), "{e:?}");
    assert!(nothing_written(&req));
}

/// A 429 on a window is waited out and retried; the export succeeds.
#[tokio::test]
async fn iif_engine_rate_limit_is_retried() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine().await;
    stamps(&server, 7, 7).await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .and(query_param("end_date", "2026-01-31"))
        .respond_with(
            json_reply(
                429,
                json!({"error":{"code":"RATE_LIMITED","message":"slow down"}}),
            )
            .insert_header("retry-after", "0"),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    windows(&server, json_reply(200, january(false))).await;
    let req = request(dir.path());
    let out = run(&server, &req).await.expect("retried");
    assert_eq!(out.counts.transactions, 3);
    assert!(req.dest.is_file());
    let jan_reads = list_queries(&server)
        .await
        .iter()
        .filter(|q| q.iter().any(|(k, v)| k == "end_date" && v == "2026-01-31"))
        .count();
    assert_eq!(jan_reads, 2, "the 429, then the answer");
}

/// The date range narrows the windows, and its own ends are sent.
#[tokio::test]
async fn iif_engine_a_range_keeps_its_own_ends() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine().await;
    stamps(&server, 7, 7).await;
    Mock::given(method("GET"))
        .and(path(LIST))
        .respond_with(json_reply(200, json!({"data":[],"hiddenNotice":false})))
        .mount(&server)
        .await;
    let mut req = request(dir.path());
    req.start = Some(CalendarDate::from_ymd(2026, 1, 10).expect("d"));
    req.end = Some(CalendarDate::from_ymd(2026, 1, 25).expect("d"));
    let out = run(&server, &req).await.expect("export");
    assert_eq!(out.counts.transactions, 0);
    assert_eq!(
        list_queries(&server).await,
        vec![pairs(&[
            ("exact", "1"),
            ("basis", "accrual"),
            ("start_date", "2026-01-10"),
            ("end_date", "2026-01-25"),
        ])]
    );
}
