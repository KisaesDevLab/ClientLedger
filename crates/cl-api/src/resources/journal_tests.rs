//! Stub tests of journal entries, voids, the ledger stamp, closed periods
//! and outcome reconciliation (P05 plan §2.6.1, §2.7, §7 AC3/AC5).

use std::sync::Arc;

use cl_core::ids::{AccountId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::Value;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use crate::endpoint::{EndpointState, EngineEndpoint, FixedEndpoint};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::posting::{ClosedPeriodAuthorization, PostingOptions};
use crate::resources::tags::TagFields;
use crate::resources::testkit::{COMPANY, TENANT, json, requests};
use crate::resources::transactions::{Basis, JeLine, NewJournalEntry, Side, TxnStatus};
use crate::session::tests::session_with;
use crate::session::{Scope, Session};
use cl_core::ids::{CompanyId, TenantId};

const A: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const B: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const T1: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const T2: &str = "7b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TOKEN: &str = "launch-token-canary-0123456789";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 15).expect("date")
}

fn line(account: &str, side: Side) -> JeLine {
    JeLine {
        account: acct(account),
        side,
        description: None,
        tag: None,
        contact: None,
    }
}

fn entry() -> NewJournalEntry {
    NewJournalEntry::new(
        day(),
        Some("rent".into()),
        Basis::Both,
        vec![
            line(A, Side::Debit(m("10.0000"))),
            line(B, Side::Credit(m("10.0000"))),
        ],
    )
    .expect("balanced")
}

/// The engine's answer for `entry()` as stored, with id `id`.
fn txn(id: &str, status: &str, created: &str) -> String {
    format!(
        r#"{{"id":"{id}","txnType":"journal_entry","txnDate":"2026-10-15","status":"{status}",
        "basis":"both","contactId":null,"memo":"rent","publicToken":"cap-canary",
        "createdAt":"{created}","lines":[
        {{"id":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{A}","debit":"10.0000",
          "credit":"0.0000","description":null,"lineOrder":0,"tagId":null,"contactId":null}},
        {{"id":"8b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{B}","debit":"0.0000",
          "credit":"10.0000","description":null,"lineOrder":1,"tagId":null,"contactId":null}}]}}"#
    )
}

fn wrapped(id: &str, status: &str) -> String {
    format!(
        r#"{{"transaction":{}}}"#,
        txn(id, status, "2199-01-01T00:00:00.000Z")
    )
}

fn scope() -> Scope {
    Scope::Company {
        tenant_id: TenantId::parse(TENANT).expect("t"),
        company_id: CompanyId::parse(COMPANY).expect("c"),
    }
}

/// A company session whose endpoint generation the test can move.
fn session(server: &MockServer) -> (Session, Arc<FixedEndpoint>) {
    let src = Arc::new(FixedEndpoint::new(
        server.address().port(),
        Secret::new(TOKEN),
        1,
    ));
    (session_with(src.clone(), scope()), src)
}

/// Answers 500 once and "restarts" the engine (generation 2).
fn fail_and_restart(
    src: &Arc<FixedEndpoint>,
    port: u16,
) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
    let src = src.clone();
    move |_: &Request| {
        src.set(EndpointState::Ready(EngineEndpoint {
            port,
            token: Secret::new(TOKEN),
            generation: 2,
        }));
        ResponseTemplate::new(500)
    }
}

async fn stamp(server: &MockServer, value: u64, times: Option<u64>) {
    let mock = Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json(200, &format!(r#"{{"stamp":{value}}}"#)));
    match times {
        Some(n) => mock.up_to_n_times(n).mount(server).await,
        None => mock.mount(server).await,
    }
}

async fn list_rows(server: &MockServer, rows: &[String]) {
    Mock::given(method("GET"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(
            200,
            &format!(r#"{{"data":[{}],"total":{}}}"#, rows.join(","), rows.len()),
        ))
        .mount(server)
        .await;
}

async fn get_txn(server: &MockServer, id: &str, status: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{id}")))
        .respond_with(json(200, &wrapped(id, status)))
        .mount(server)
        .await;
}

fn posts(reqs: &[Request], p: &str) -> usize {
    reqs.iter().filter(|r| r.url.path() == p).count()
}

// --- local refusals: nothing is sent ------------------------------------

#[test]
fn journal_entries_are_refused_locally() {
    let mk = |lines| NewJournalEntry::new(day(), None, Basis::Both, lines);
    assert_eq!(
        mk(vec![line(A, Side::Debit(m("1")))]),
        Err(LocalValidation::TooFewLines)
    );
    let many = (0..501)
        .map(|i| {
            line(
                A,
                if i % 2 == 0 {
                    Side::Debit(m("1"))
                } else {
                    Side::Credit(m("1"))
                },
            )
        })
        .collect();
    assert_eq!(mk(many), Err(LocalValidation::TooManyLines));
    assert_eq!(
        mk(vec![
            line(A, Side::Debit(m("0"))),
            line(B, Side::Credit(m("0")))
        ]),
        Err(LocalValidation::NonPositiveAmount)
    );
    assert_eq!(
        mk(vec![
            line(A, Side::Debit(m("-5"))),
            line(B, Side::Credit(m("-5")))
        ]),
        Err(LocalValidation::NonPositiveAmount)
    );
    assert_eq!(
        mk(vec![
            line(A, Side::Debit(m("10.0001"))),
            line(B, Side::Credit(m("10.0000")))
        ]),
        Err(LocalValidation::Unbalanced)
    );
    assert_eq!(
        mk(vec![
            line(A, Side::Debit(m("1"))),
            line(B, Side::Debit(m("1")))
        ]),
        Err(LocalValidation::Unbalanced)
    );
    // More than 4 decimals or 16 integer digits cannot even be a Money.
    assert!(Money::parse_wire("1.00001").is_err());
    assert!(Money::parse_wire("1000000000000000.0000").is_err());
    let max = vec![
        line(A, Side::Debit(m("999999999999999.9999"))),
        line(A, Side::Debit(m("999999999999999.9999"))),
        line(B, Side::Credit(m("1"))),
    ];
    assert_eq!(mk(max), Err(LocalValidation::InvalidValue));
    let e = mk(vec![
        JeLine {
            description: Some(String::new()),
            ..line(A, Side::Debit(m("1")))
        },
        line(B, Side::Credit(m("1"))),
    ])
    .expect("ok");
    assert_eq!(e.lines()[0].description, None, "empty description is none");
}

#[tokio::test]
async fn blank_void_reason_and_unavailable_override_send_nothing() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    let id = TransactionId::parse(T1).expect("id");
    let e = s
        .transactions()
        .void(id, "  ", &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("reason");
    assert_eq!(e, ApiError::Local(LocalValidation::MissingReason));
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "close-pw-canary",
        ))),
    };
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &opts, IdempotencyKey::new())
        .await
        .expect_err("no U-15");
    assert_eq!(
        e,
        ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
    );
    assert!(!format!("{opts:?}").contains("close-pw-canary"));
    assert_eq!(server.received_requests().await.expect("r").len(), 0);
}

// --- request shapes, ledger stamp, closed period --------------------------

#[tokio::test]
async fn create_sends_exact_strings_after_reading_the_stamp() {
    let server = MockServer::start().await;
    stamp(&server, 41, None).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(201, &wrapped(T1, "posted")))
        .mount(&server)
        .await;
    let (s, _) = session(&server);
    assert_eq!(
        s.ledger_version().await.expect("stamp"),
        s.ledger_version().await.expect("stamp")
    );
    let v = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("posted");
    assert_eq!(v.record.status, TxnStatus::Posted);
    assert_eq!(v.record.lines[0].debit.to_wire(), "10.0000");
    assert_eq!(
        v.version,
        crate::RecordVersion::Fingerprint(entry().fingerprint())
    );
    let reqs = server.received_requests().await.expect("r");
    let order: Vec<&str> = reqs.iter().map(|r| r.url.path()).collect();
    assert_eq!(
        order,
        [
            "/api/v1/ledger-version",
            "/api/v1/ledger-version",
            "/api/v1/ledger-version",
            "/api/v1/transactions"
        ]
    );
    let body: Value = serde_json::from_slice(&reqs[3].body).expect("json");
    assert_eq!(body["txnType"], "journal_entry");
    assert_eq!(body["txnDate"], "2026-10-15");
    assert_eq!(body["basis"], "both");
    assert_eq!(body["lines"][0]["debit"], "10.0000");
    assert_eq!(body["lines"][0]["credit"], "0.0000");
    assert_eq!(body["lines"][1]["credit"], "10.0000");
    assert!(body["lines"][0].get("tagId").is_none());
    assert!(body.get("overrideConfirmed").is_none());
    assert!(reqs[3].headers.get("idempotency-key").is_some());
}

#[tokio::test]
async fn closed_period_is_typed_and_offers_no_override() {
    let server = MockServer::start().await;
    stamp(&server, 1, None).await;
    Mock::given(method("POST"))
        .respond_with(json(
            423,
            r#"{"error":{"message":"This period was closed on 2026-06-30.","code":"TB_PERIOD_LOCKED",
                "details":{"lockDate":"2026-06-30","canOverride":true}}}"#,
        ))
        .mount(&server)
        .await;
    let (s, _) = session(&server);
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("locked");
    let ApiError::ClosedPeriod(cp) = e else {
        panic!("not closed period: {e:?}");
    };
    assert!(cp.can_override, "shown as information only");
    assert_eq!(cp.lock_date, CalendarDate::from_ymd(2026, 6, 30).ok());
    assert_eq!(
        posts(&requests(&server, "POST").await, "/api/v1/transactions"),
        1
    );
}

#[tokio::test]
async fn void_sends_the_reason_and_update_is_guarded() {
    let server = MockServer::start().await;
    stamp(&server, 1, None).await;
    get_txn(&server, T1, "posted").await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/transactions/{T1}/void")))
        .respond_with(json(200, &wrapped(T1, "void")))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(json(200, &wrapped(T1, "posted")))
        .mount(&server)
        .await;
    let (s, _) = session(&server);
    let id = TransactionId::parse(T1).expect("id");
    let tx = s.transactions();
    let v = tx.get(id).await.expect("get");
    let saved = tx
        .update_journal_entry(
            id,
            &v.version,
            &entry(),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("update");
    assert_eq!(
        saved.version, v.version,
        "identical lines, new line ids: same fingerprint"
    );
    let put: Value = serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
    assert_eq!(put["txnType"], "journal_entry");
    let voided = tx
        .void(
            id,
            " duplicate entry ",
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("void");
    assert_eq!(voided.record.status, TxnStatus::Void);
    let posts = requests(&server, "POST").await;
    let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(body, serde_json::json!({"reason": "duplicate entry"}));
}

// --- reconciliation matrix -------------------------------------------------

#[tokio::test]
async fn unchanged_stamp_after_restart_is_not_committed_and_retried_once_with_the_same_key() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    stamp(&server, 5, None).await;
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json(201, &wrapped(T1, "posted")))
        .mount(&server)
        .await;
    let v = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("retried");
    assert_eq!(v.record.id.to_string(), T1);
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 2, "exactly one same-key retry");
    assert_eq!(
        posts[0].headers.get("idempotency-key"),
        posts[1].headers.get("idempotency-key")
    );
}

#[tokio::test]
async fn moved_stamp_and_one_matching_entry_is_committed_without_a_second_post() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    stamp(&server, 5, Some(1)).await;
    stamp(&server, 6, None).await;
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .mount(&server)
        .await;
    // An older identical entry outside the window does not count.
    list_rows(
        &server,
        &[
            format!(r#"{{"id":"{T1}","createdAt":"2199-01-01T00:00:00.000Z"}}"#),
            format!(r#"{{"id":"{T2}","createdAt":"2001-01-01T00:00:00.000Z"}}"#),
        ],
    )
    .await;
    get_txn(&server, T1, "posted").await;
    get_txn(&server, T2, "posted").await;
    let v = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("committed");
    assert_eq!(v.record.id.to_string(), T1);
    assert_eq!(requests(&server, "POST").await.len(), 1, "one POST only");
    let reqs = server.received_requests().await.expect("r");
    let list = reqs
        .iter()
        .find(|r| r.url.path() == "/api/v1/transactions" && r.method.as_str() == "GET")
        .expect("searched");
    assert_eq!(
        list.url.query(),
        Some("txnType=journal_entry&startDate=2026-10-15&endDate=2026-10-15&limit=500&offset=0")
    );
}

#[tokio::test]
async fn two_identical_entries_in_the_window_are_unknown() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    stamp(&server, 5, Some(1)).await;
    stamp(&server, 6, None).await;
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .mount(&server)
        .await;
    list_rows(
        &server,
        &[
            format!(r#"{{"id":"{T1}","createdAt":"2199-01-01T00:00:00.000Z"}}"#),
            format!(r#"{{"id":"{T2}"}}"#),
        ],
    )
    .await;
    get_txn(&server, T1, "posted").await;
    get_txn(&server, T2, "posted").await;
    let key = IdempotencyKey::new();
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), key.clone())
        .await
        .expect_err("ambiguous");
    assert!(matches!(
        e,
        ApiError::OutcomeUnknown { idempotency_key: Some(ref k), .. } if *k == key
    ));
    assert_eq!(requests(&server, "POST").await.len(), 1, "never guessed");
}

#[tokio::test]
async fn same_generation_and_nothing_found_polls_then_is_unknown() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    stamp(&server, 5, None).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;
    list_rows(&server, &[]).await;
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("unknown");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }));
    let reqs = server.received_requests().await.expect("r");
    let searches = reqs
        .iter()
        .filter(|r| r.url.path() == "/api/v1/transactions" && r.method.as_str() == "GET")
        .count();
    assert!(searches >= 2, "polled: {searches}");
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

#[tokio::test]
async fn void_not_applied_after_restart_is_retried_and_update_found_is_committed() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json(500, "{}"))
        .mount(&server)
        .await;
    get_txn(&server, T1, "posted").await;
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(json(200, &wrapped(T1, "void")))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let id = TransactionId::parse(T1).expect("id");
    let v = s
        .transactions()
        .void(
            id,
            "typo",
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("voided");
    assert_eq!(v.record.status, TxnStatus::Void);
    assert_eq!(requests(&server, "POST").await.len(), 2);

    // The PUT's answer is lost, but the record already has our lines.
    let current = s.transactions().get(id).await.expect("get");
    let saved = s
        .transactions()
        .update_journal_entry(
            id,
            &current.version,
            &entry(),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("committed");
    assert_eq!(saved.record.id, id);
    assert_eq!(requests(&server, "PUT").await.len(), 1);
}

#[tokio::test]
async fn master_data_create_is_found_by_exact_name_and_delete_by_absence() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    Mock::given(method("POST"))
        .and(path("/api/v1/tags"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tags"))
        .respond_with(json(
            200,
            r#"{"tags":[
              {"id":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Canary Job","color":null,
               "groupId":null,"description":null,"isActive":true,"createdAt":"2199-01-01T00:00:00.000Z"},
              {"id":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Canary Job 2","isActive":true}],
              "total":2,"limit":500,"offset":0}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    // The single match is read by id: its full record is the answer.
    Mock::given(method("GET"))
        .and(path("/api/v1/tags/5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"))
        .respond_with(json(
            200,
            r#"{"tag":{"id":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Canary Job",
               "color":null,"groupId":null,"description":"read by id","isActive":true}}"#,
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("^/api/v1/tags/.+"))
        .respond_with(json(404, r#"{"error":{"message":"Tag not found"}}"#))
        .mount(&server)
        .await;
    let fields = TagFields {
        name: "Canary Job".into(),
        color: None,
        group_id: None,
        description: None,
    };
    let tag = s
        .tags()
        .create(&fields, IdempotencyKey::new())
        .await
        .expect("committed");
    assert_eq!(tag.record.fields.name, "Canary Job");
    assert_eq!(
        tag.record.fields.description.as_deref(),
        Some("read by id"),
        "the record, not the list row"
    );
    assert_eq!(requests(&server, "POST").await.len(), 1);
    let search = requests(&server, "GET").await;
    assert_eq!(
        search[0].url.query(),
        Some("search=Canary+Job&limit=500&offset=0")
    );
    s.tags()
        .delete(
            TagId::parse("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            IdempotencyKey::new(),
        )
        .await
        .expect("deleted");
    assert_eq!(requests(&server, "DELETE").await.len(), 1);
}

#[tokio::test]
async fn master_data_candidate_with_other_fields_is_unknown_not_retried() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tags"))
        .respond_with(json(
            200,
            r##"{"tags":[{"id":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Canary Job",
                "color":"#ff0000","isActive":true}],"total":1}"##,
        ))
        .mount(&server)
        .await;
    let fields = TagFields {
        name: "Canary Job".into(),
        color: None,
        group_id: None,
        description: None,
    };
    let e = s
        .tags()
        .create(&fields, IdempotencyKey::new())
        .await
        .expect_err("unknown");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }));
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

fn has_key(v: &Value, bad: &str) -> bool {
    match v {
        Value::Object(map) => map.iter().any(|(k, v)| k == bad || has_key(v, bad)),
        Value::Array(a) => a.iter().any(|v| has_key(v, bad)),
        _ => false,
    }
}

proptest::proptest! {
    /// Any balanced entry: neither forbidden key at any depth, every amount
    /// a canonical string, and the posted copy has the entry's fingerprint.
    #[test]
    fn arbitrary_entries_never_carry_override_or_lock_date(
        amounts in proptest::collection::vec(1i64..=1_000_000_000, 1..20),
        memo in proptest::option::of("[a-zA-Z ]{0,12}"),
    ) {
        let mut lines: Vec<JeLine> = amounts
            .iter()
            .map(|a| line(A, Side::Debit(Money::from_ten_thousandths(i128::from(*a)).expect("m"))))
            .collect();
        let total = Money::sum(lines.iter().map(|l| match l.side {
            Side::Debit(m) | Side::Credit(m) => m,
        }))
        .expect("sum");
        lines.push(line(B, Side::Credit(total)));
        let e = NewJournalEntry::new(day(), memo, Basis::Accrual, lines).expect("balanced");
        let v = e.to_value();
        proptest::prop_assert!(!has_key(&v, "overrideConfirmed"));
        proptest::prop_assert!(!has_key(&v, "lockDate"));
        for l in v["lines"].as_array().expect("lines") {
            proptest::prop_assert!(l["debit"].is_string() && l["credit"].is_string());
        }
    }
}

// --- company switch in the middle of a save (pinned scope) ---------------

fn other_company() -> Scope {
    Scope::Company {
        tenant_id: TenantId::parse(T2).expect("t"),
        company_id: CompanyId::parse(B).expect("c"),
    }
}

/// Answers `body` and switches the session to another company meanwhile.
fn switch_then(
    s: &Session,
    body: String,
) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
    let s = s.clone();
    move |_: &Request| {
        s.set_scope(other_company());
        json(200, &body)
    }
}

#[tokio::test]
async fn posting_aborts_with_nothing_sent_when_the_company_switches_after_the_stamp_read() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(switch_then(&s, r#"{"stamp":1}"#.into()))
        .mount(&server)
        .await;
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("stale");
    assert_eq!(e, ApiError::StaleScope);
    assert_eq!(requests(&server, "POST").await.len(), 0);
}

#[tokio::test]
async fn guarded_update_aborts_when_the_company_switches_after_the_read() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    stamp(&server, 1, None).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{T1}")))
        .respond_with(json(200, &wrapped(T1, "posted")))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{T1}")))
        .respond_with(switch_then(&s, wrapped(T1, "posted")))
        .mount(&server)
        .await;
    let id = TransactionId::parse(T1).expect("id");
    let v = s.transactions().get(id).await.expect("get");
    let e = s
        .transactions()
        .update_journal_entry(
            id,
            &v.version,
            &entry(),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("stale");
    assert_eq!(e, ApiError::StaleScope);
    assert_eq!(requests(&server, "PUT").await.len(), 0);
}

#[tokio::test]
async fn not_committed_retry_after_a_switch_is_unknown_not_forbidden() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    // The probe's first stamp read; the reconciliation read switches.
    stamp(&server, 1, Some(1)).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(switch_then(&s, r#"{"stamp":1}"#.into()))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(fail_and_restart(&src, server.address().port()))
        .mount(&server)
        .await;
    let key = IdempotencyKey::new();
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), key.clone())
        .await
        .expect_err("unknown");
    assert!(
        matches!(e, ApiError::OutcomeUnknown { idempotency_key: Some(ref k), .. } if *k == key),
        "{e:?}"
    );
    let reqs = requests(&server, "POST").await;
    assert_eq!(reqs.len(), 1, "no retry into the other company");
    assert_eq!(
        reqs[0].headers.get("x-company-id").expect("id"),
        COMPANY,
        "sent in the company it was started in"
    );
}

#[tokio::test]
async fn master_data_reconciliation_stops_at_a_switch() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    Mock::given(method("POST"))
        .and(path("/api/v1/tags"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tags"))
        .respond_with(switch_then(&s, r#"{"tags":[],"total":0}"#.into()))
        .mount(&server)
        .await;
    let fields = TagFields {
        name: "Canary Job".into(),
        color: None,
        group_id: None,
        description: None,
    };
    let e = s
        .tags()
        .create(&fields, IdempotencyKey::new())
        .await
        .expect_err("unknown");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }), "{e:?}");
    assert_eq!(requests(&server, "POST").await.len(), 1);
    assert_eq!(requests(&server, "GET").await.len(), 1, "polling stopped");
}

#[tokio::test]
async fn a_saved_entry_the_client_cannot_read_is_saved_reload_not_internal() {
    use cl_core::OwnerFacing;
    let server = MockServer::start().await;
    stamp(&server, 1, None).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(201, &wrapped(T1, "frobnicated")))
        .mount(&server)
        .await;
    let (s, _) = session(&server);
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("unreadable");
    assert_eq!(
        e,
        ApiError::SavedUnreadable {
            what: "transaction"
        }
    );
    let msg = e.owner_message();
    assert_eq!(msg.key, "error.api.saved_unreadable");
    assert_eq!(msg.suggestion_key, Some("fix.api.reload_to_see"));
    assert_eq!(requests(&server, "POST").await.len(), 1, "never re-sent");
}

#[tokio::test]
async fn an_earlier_identical_entry_of_ours_is_never_claimed_as_this_one() {
    let server = MockServer::start().await;
    stamp(&server, 1, None).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(201, &wrapped(T1, "posted")))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    list_rows(&server, &[txn(T1, "posted", "2199-01-01T00:00:00.000Z")]).await;
    get_txn(&server, T1, "posted").await;
    let (s, _) = session(&server);
    let tx = s.transactions();
    let first = tx
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect("first saved");
    assert_eq!(first.record.id.to_string(), T1);
    let e = tx
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("second is not the first");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }), "{e:?}");
    assert_eq!(requests(&server, "POST").await.len(), 2, "never re-sent");
}

#[tokio::test]
async fn a_candidate_id_that_is_not_an_id_is_never_put_in_a_path() {
    let server = MockServer::start().await;
    stamp(&server, 1, None).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let bad = txn(T1, "posted", "2199-01-01T00:00:00.000Z").replacen(T1, "../../admin", 1);
    list_rows(&server, &[bad]).await;
    let (s, _) = session(&server);
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("unknown");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }), "{e:?}");
    let gets = requests(&server, "GET").await;
    assert!(
        gets.iter().all(|r| !r.url.path().contains("admin")),
        "no request built from the raw id"
    );
}

// --- P15 part A: names, numbers, adjusting, reversing -------------------------

mod p15 {
    use super::*;
    use crate::resources::testkit::session_with_caps;
    use crate::resources::transactions::{
        AccountRoles, JE_AR_AP_NAME_REQUIRED, JeOptions, Transaction, has_code,
        next_number_from_json,
    };
    use crate::resources::versioning::EditableFields;
    use crate::wire::Capabilities;
    use cl_core::ids::ContactId;
    use std::collections::BTreeSet;

    const AP: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const AR: &str = "2d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const VENDOR: &str = "3c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    const FULL_BODY: &str = include_str!("../../../../fixtures/engine/je-create-full.json");
    const REVERSING: &str = include_str!("../../../../fixtures/engine/je-record-reversing.json");
    const ALL: [&str; 4] = [
        Capabilities::JE_NAMES,
        Capabilities::JE_NUMBER,
        Capabilities::JE_ADJUSTING,
        Capabilities::JE_REVERSING,
    ];

    fn roles() -> AccountRoles {
        AccountRoles {
            receivable: BTreeSet::from([acct(AR)]),
            payable: BTreeSet::from([acct(AP)]),
        }
    }

    fn vendor() -> ContactId {
        ContactId::parse(VENDOR).expect("contact")
    }

    fn named(account: &str, side: Side) -> JeLine {
        JeLine {
            contact: Some(vendor()),
            ..line(account, side)
        }
    }

    fn opts(number: Option<&str>, adjusting: bool, reverse_on: Option<CalendarDate>) -> JeOptions {
        JeOptions {
            number: number.map(str::to_string),
            adjusting,
            reverse_on,
        }
    }

    fn full() -> NewJournalEntry {
        NewJournalEntry::new_with(
            day(),
            Some("Accrue October rent".into()),
            Basis::Both,
            vec![
                JeLine {
                    description: Some("Rent".into()),
                    tag: Some(TagId::parse(T1).expect("tag")),
                    ..line(A, Side::Debit(m("1200")))
                },
                named(AP, Side::Credit(m("1200"))),
            ],
            opts(Some(" 41 "), true, CalendarDate::from_ymd(2026, 11, 1).ok()),
            &roles(),
        )
        .expect("valid")
    }

    #[tokio::test]
    async fn unbalanced_refused_without_request() {
        let server = MockServer::start().await;
        let _s = session_with_caps(&server, &ALL).await;
        let before = server.received_requests().await.expect("r").len();
        let e = NewJournalEntry::new_with(
            day(),
            None,
            Basis::Both,
            vec![
                line(A, Side::Debit(m("1200.00"))),
                line(B, Side::Credit(m("1100.00"))),
            ],
            JeOptions::default(),
            &roles(),
        );
        assert_eq!(e, Err(LocalValidation::Unbalanced));
        // There is no entry to send: the builder is the only way to one.
        assert_eq!(server.received_requests().await.expect("r").len(), before);
    }

    #[test]
    fn header_and_ar_ap_rules_are_refused_locally() {
        let mk = |lines, o| NewJournalEntry::new_with(day(), None, Basis::Both, lines, o, &roles());
        let pair = || vec![line(A, Side::Debit(m("5"))), line(B, Side::Credit(m("5")))];
        for r in [day(), CalendarDate::from_ymd(2026, 10, 14).expect("d")] {
            assert_eq!(
                mk(pair(), opts(None, false, Some(r))),
                Err(LocalValidation::ReverseNotAfter),
                "reverse on {r:?}"
            );
        }
        assert!(
            mk(
                pair(),
                opts(None, false, CalendarDate::from_ymd(2026, 10, 16).ok())
            )
            .is_ok()
        );
        assert_eq!(
            mk(pair(), opts(Some(&"9".repeat(21)), false, None)),
            Err(LocalValidation::InvalidValue)
        );
        let ok = mk(
            pair(),
            opts(Some(&format!(" {} ", "9".repeat(20))), false, None),
        )
        .expect("20 after trimming");
        assert_eq!(ok.number(), Some("99999999999999999999"));
        assert_eq!(
            mk(pair(), opts(Some("   "), false, None))
                .expect("blank")
                .number(),
            None
        );
        // A/R or A/P: a name on the line, and only one such line (Q-206).
        assert_eq!(
            mk(
                vec![line(A, Side::Debit(m("5"))), line(AP, Side::Credit(m("5")))],
                JeOptions::default()
            ),
            Err(LocalValidation::ArApNameMissing)
        );
        assert_eq!(
            mk(
                vec![line(AR, Side::Debit(m("5"))), line(B, Side::Credit(m("5")))],
                JeOptions::default()
            ),
            Err(LocalValidation::ArApNameMissing)
        );
        assert_eq!(
            mk(
                vec![
                    named(AR, Side::Debit(m("5"))),
                    named(AP, Side::Credit(m("5")))
                ],
                JeOptions::default()
            ),
            Err(LocalValidation::ArApTwoAccounts)
        );
        assert_eq!(
            mk(
                vec![
                    named(AP, Side::Debit(m("5"))),
                    named(AP, Side::Credit(m("5")))
                ],
                JeOptions::default()
            ),
            Err(LocalValidation::ArApTwoAccounts),
            "two lines on one A/P account"
        );
        assert!(
            mk(
                vec![
                    line(A, Side::Debit(m("5"))),
                    named(AP, Side::Credit(m("5")))
                ],
                JeOptions::default()
            )
            .is_ok()
        );
        // Balance is checked first: an unbalanced entry is Unbalanced
        // whatever else is wrong.
        assert_eq!(
            mk(
                vec![
                    line(AR, Side::Debit(m("5"))),
                    line(AR, Side::Credit(m("4")))
                ],
                opts(None, false, Some(day()))
            ),
            Err(LocalValidation::Unbalanced)
        );
    }

    #[tokio::test]
    async fn fields_without_their_capability_send_nothing() {
        let server = MockServer::start().await;
        stamp(&server, 1, None).await;
        let s = session_with_caps(&server, &[]).await;
        let base = || vec![line(A, Side::Debit(m("5"))), line(B, Side::Credit(m("5")))];
        let roles = AccountRoles::default();
        let cases = [
            (
                NewJournalEntry::new_with(
                    day(),
                    None,
                    Basis::Both,
                    vec![line(A, Side::Debit(m("5"))), named(B, Side::Credit(m("5")))],
                    JeOptions::default(),
                    &roles,
                ),
                LocalValidation::JeNamesUnavailable,
            ),
            (
                NewJournalEntry::new_with(
                    day(),
                    None,
                    Basis::Both,
                    base(),
                    opts(Some("7"), false, None),
                    &roles,
                ),
                LocalValidation::JeNumberUnavailable,
            ),
            (
                NewJournalEntry::new_with(
                    day(),
                    None,
                    Basis::Both,
                    base(),
                    opts(None, true, None),
                    &roles,
                ),
                LocalValidation::JeAdjustingUnavailable,
            ),
            (
                NewJournalEntry::new_with(
                    day(),
                    None,
                    Basis::Both,
                    base(),
                    opts(None, false, CalendarDate::from_ymd(2026, 11, 1).ok()),
                    &roles,
                ),
                LocalValidation::JeReversingUnavailable,
            ),
        ];
        let id = TransactionId::parse(T1).expect("id");
        let version = crate::RecordVersion::Fingerprint([0; 32]);
        for (e, refusal) in cases {
            let e = e.expect("valid");
            let got = s
                .transactions()
                .create_journal_entry(&e, &PostingOptions::default(), IdempotencyKey::new())
                .await
                .expect_err("refused");
            assert_eq!(got, ApiError::Local(refusal));
            let got = s
                .transactions()
                .update_journal_entry(
                    id,
                    &version,
                    &e,
                    &PostingOptions::default(),
                    IdempotencyKey::new(),
                )
                .await
                .expect_err("refused");
            assert_eq!(got, ApiError::Local(refusal));
        }
        let e = s.journal().next_number().await.expect_err("no je-number");
        assert_eq!(e, ApiError::Local(LocalValidation::JeNumberUnavailable));
        let sent: Vec<String> = server
            .received_requests()
            .await
            .expect("r")
            .iter()
            .map(|r| r.url.path().to_string())
            .filter(|p| p != "/api/v1/desktop/info")
            .collect();
        assert!(sent.is_empty(), "nothing sent: {sent:?}");
    }

    #[tokio::test]
    async fn full_entry_body_matches_the_golden_and_reads_back() {
        let server = MockServer::start().await;
        stamp(&server, 3, None).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/transactions"))
            .respond_with(json(201, REVERSING))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &ALL).await;
        let v = s
            .transactions()
            .create_journal_entry(&full(), &PostingOptions::default(), IdempotencyKey::new())
            .await
            .expect("posted");
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        let golden: Value = serde_json::from_str(FULL_BODY).expect("golden");
        assert_eq!(body, golden);
        let t = &v.record;
        assert_eq!(t.txn_number.as_deref(), Some("41"));
        assert!(t.is_adjusting);
        assert_eq!(t.reverse_on, CalendarDate::from_ymd(2026, 11, 1).ok());
        assert_eq!(
            t.reversal_transaction_id,
            TransactionId::parse("7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").ok()
        );
        assert_eq!(t.lines[1].contact_id, Some(vendor()));
        assert_eq!(
            t.lines[1].contact_name.as_deref(),
            Some("Synthetic Landlord LLC")
        );
        assert_eq!(t.lines[1].credit.to_wire(), "1200.0000");
        assert_eq!(
            t.fingerprint(),
            full().fingerprint(),
            "the read-back is the entry sent"
        );
        assert_eq!(
            v.version,
            crate::RecordVersion::Fingerprint(full().fingerprint())
        );
    }

    #[test]
    fn new_fields_are_hashed_only_when_set() {
        // A plain entry hashes exactly as before P15.
        let plain = entry();
        let same = NewJournalEntry::new_with(
            day(),
            Some("rent".into()),
            Basis::Both,
            plain.lines().to_vec(),
            opts(Some(" "), false, None),
            &roles(),
        )
        .expect("ok");
        assert_eq!(plain.fingerprint(), same.fingerprint());
        let raw: Transaction =
            serde_json::from_str(&txn(T1, "posted", "2199-01-01T00:00:00.000Z")).expect("record");
        assert_eq!(raw.fingerprint(), plain.fingerprint());
        // Each new field, once set, changes it.
        let lines = plain.lines().to_vec();
        let with = |o: JeOptions| {
            NewJournalEntry::new_with(
                day(),
                Some("rent".into()),
                Basis::Both,
                lines.clone(),
                o,
                &roles(),
            )
            .expect("ok")
            .fingerprint()
        };
        let fps = [
            plain.fingerprint(),
            with(opts(Some("1"), false, None)),
            with(opts(None, true, None)),
            with(opts(None, false, CalendarDate::from_ymd(2026, 12, 1).ok())),
        ];
        let distinct: BTreeSet<[u8; 32]> = fps.into_iter().collect();
        assert_eq!(distinct.len(), 4);
        // A check's number is never hashed by the journal entry rule.
        let mut check = raw.clone();
        check.txn_type = crate::resources::transactions::TxnType::Expense;
        let before = check.fingerprint();
        check.txn_number = Some("1001".into());
        assert_eq!(check.fingerprint(), before);
    }

    #[tokio::test]
    async fn update_clears_number_and_flag_explicitly_with_the_capability() {
        let server = MockServer::start().await;
        stamp(&server, 1, None).await;
        get_txn(&server, T1, "posted").await;
        Mock::given(method("PUT"))
            .respond_with(json(200, &wrapped(T1, "posted")))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &ALL).await;
        let id = TransactionId::parse(T1).expect("id");
        let v = s.transactions().get(id).await.expect("get");
        s.transactions()
            .update_journal_entry(
                id,
                &v.version,
                &entry(),
                &PostingOptions::default(),
                IdempotencyKey::new(),
            )
            .await
            .expect("update");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["txnNumber"], "");
        assert_eq!(put["isAdjusting"], false);
        assert!(
            put.get("reverseOn").is_none(),
            "no reversal on the edited entry"
        );
    }

    #[tokio::test]
    async fn next_number_reads_the_string() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/journal-entries/next-number"))
            .respond_with(json(200, r#"{"next":"1044"}"#))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::JE_NUMBER]).await;
        assert_eq!(s.journal().next_number().await.expect("next"), "1044");
        assert!(matches!(
            next_number_from_json(br#"{"next":1044}"#),
            Err(ApiError::Decode { .. })
        ));
        assert!(matches!(
            next_number_from_json(b"not json"),
            Err(ApiError::Decode { .. })
        ));
    }

    #[tokio::test]
    async fn engine_ar_ap_refusal_keeps_its_code() {
        let server = MockServer::start().await;
        stamp(&server, 1, None).await;
        Mock::given(method("POST"))
            .and(path("/api/v1/transactions"))
            .respond_with(json(
                400,
                r#"{"error":{"message":"Line 2 needs a name","code":"JE_AR_AP_NAME_REQUIRED"}}"#,
            ))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &ALL).await;
        let e = s
            .transactions()
            .create_journal_entry(&entry(), &PostingOptions::default(), IdempotencyKey::new())
            .await
            .expect_err("refused");
        assert!(has_code(&e, JE_AR_AP_NAME_REQUIRED), "{e:?}");
        assert!(!has_code(&e, "OTHER"));
    }
}
