//! The owner's recheck of a save whose outcome stayed unknown (B2), and
//! the provenance probe of "Enter now" on a template (B2, Q-PR1c).

use std::sync::Arc;

use cl_core::ids::{CompanyId, TemplateId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::Recheck;
use crate::endpoint::{EndpointState, EngineEndpoint, FixedEndpoint};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::posting::PostingOptions;
use crate::resources::testkit::{COMPANY, TENANT, json, requests};
use crate::resources::transactions::{Basis, JeLine, NewJournalEntry, Side};
use crate::session::tests::session_with;
use crate::session::{Scope, Session};

const A: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const B: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const T1: &str = "7a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TPL: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04";
const TOKEN: &str = "launch-token-canary-0123456789";

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 15).expect("date")
}

fn entry() -> NewJournalEntry {
    let m = |s: &str| Money::parse_wire(s).expect("money");
    let line = |a: &str, side| JeLine {
        account: cl_core::ids::AccountId::parse(a).expect("a"),
        side,
        description: None,
        tag: None,
        contact: None,
    };
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

fn wrapped(id: &str) -> String {
    format!(
        r#"{{"transaction":{{"id":"{id}","txnType":"journal_entry","txnDate":"2026-10-15","status":"posted",
        "basis":"both","contactId":null,"memo":"rent","createdAt":"2199-01-01T00:00:00.000Z","lines":[
        {{"id":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{A}","debit":"10.0000",
          "credit":"0.0000","description":null,"lineOrder":0,"tagId":null,"contactId":null}},
        {{"id":"8b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","accountId":"{B}","debit":"0.0000",
          "credit":"10.0000","description":null,"lineOrder":1,"tagId":null,"contactId":null}}]}}}}"#
    )
}

fn session(server: &MockServer) -> (Session, Arc<FixedEndpoint>) {
    let src = Arc::new(FixedEndpoint::new(
        server.address().port(),
        Secret::new(TOKEN),
        1,
    ));
    let scope = Scope::Company {
        tenant_id: TenantId::parse(TENANT).expect("t"),
        company_id: CompanyId::parse(COMPANY).expect("c"),
    };
    (session_with(src.clone(), scope), src)
}

fn restart(src: &FixedEndpoint, port: u16) {
    src.set(EndpointState::Ready(EngineEndpoint {
        port,
        token: Secret::new(TOKEN),
        generation: 2,
    }));
}

async fn stamp(server: &MockServer, value: u64) {
    Mock::given(method("GET"))
        .and(path("/api/v1/ledger-version"))
        .respond_with(json(200, &format!(r#"{{"stamp":{value}}}"#)))
        .mount(server)
        .await;
}

async fn rows(server: &MockServer, rows: &[String], priority: u8) {
    Mock::given(method("GET"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(
            200,
            &format!(r#"{{"data":[{}],"total":{}}}"#, rows.join(","), rows.len()),
        ))
        .with_priority(priority)
        .mount(server)
        .await;
}

async fn unknown_journal(s: &Session, key: &IdempotencyKey) {
    let e = s
        .transactions()
        .create_journal_entry(&entry(), &PostingOptions::default(), key.clone())
        .await
        .expect_err("unknown");
    assert!(
        matches!(e, ApiError::OutcomeUnknown { idempotency_key: Some(ref k), .. } if k == key),
        "{e:?}"
    );
}

#[tokio::test]
async fn recheck_sends_nothing_and_says_not_committed_only_once_the_engine_moved_on() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    stamp(&server, 5).await;
    Mock::given(method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(502))
        .mount(&server)
        .await;
    rows(&server, &[], 5).await;
    let key = IdempotencyKey::new();
    unknown_journal(&s, &key).await;
    // Same engine: it cannot tell (the request may still be running).
    assert_eq!(s.recheck(&key).await, Ok(Recheck::Unknown));
    // The engine restarted, the stamp did not move: provably not saved.
    restart(&src, server.address().port());
    assert_eq!(s.recheck(&key).await, Ok(Recheck::NotCommitted));
    assert_eq!(
        requests(&server, "POST").await.len(),
        1,
        "recheck never posts"
    );
    // Answered: the probe is gone; another key was never lost.
    assert_eq!(s.recheck(&key).await, Ok(Recheck::Unknown));
    assert_eq!(
        s.recheck(&IdempotencyKey::new()).await,
        Ok(Recheck::Unknown)
    );
}

#[tokio::test]
async fn recheck_finds_the_saved_entry_once_it_shows_up() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    stamp(&server, 5).await;
    Mock::given(method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(502))
        .mount(&server)
        .await;
    rows(&server, &[], 5).await;
    let key = IdempotencyKey::new();
    unknown_journal(&s, &key).await;
    // Later the entry is there (the slow first request finished).
    rows(
        &server,
        &[format!(
            r#"{{"id":"{T1}","createdAt":"2199-01-01T00:00:00.000Z"}}"#
        )],
        1,
    )
    .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{T1}")))
        .respond_with(json(200, &wrapped(T1)))
        .mount(&server)
        .await;
    assert_eq!(
        s.recheck(&key).await,
        Ok(Recheck::Committed {
            transaction: TransactionId::parse(T1).ok()
        })
    );
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

const DAY: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a05";

fn day_entry(status: &str, txn: Option<&str>) -> String {
    let txn = txn.map_or_else(|| "null".to_string(), |t| format!(r#""{t}""#));
    format!(
        r#"{{"entry":{{"id":"{DAY}","templateId":"{TPL}","businessDate":"2026-10-15",
        "status":"{status}","transactionId":{txn},"tagId":null,"notes":null,
        "overShortAmount":"0.0000","totalSales":"0.0000","totalTax":"0.0000",
        "totalPayments":"0.0000","values":[]}}}}"#
    )
}

async fn day_reads(server: &MockServer, status: &str, txn: Option<&str>, priority: u8) {
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/daily-sales/entries/{DAY}")))
        .respond_with(json(200, &day_entry(status, txn)))
        .with_priority(priority)
        .mount(server)
        .await;
}

async fn lost_day_post(s: &Session) -> IdempotencyKey {
    let key = IdempotencyKey::new();
    let e = s
        .daily_sales()
        .post(
            cl_core::ids::DailyEntryId::parse(DAY).expect("day"),
            &crate::resources::daily_sales::DailyPostOptions::default(),
            key.clone(),
        )
        .await
        .expect_err("unknown");
    assert!(
        matches!(e, ApiError::OutcomeUnknown { idempotency_key: Some(ref k), .. } if k == &key),
        "{e:?}"
    );
    key
}

/// Review fix-PR-1 blocking 1: a day still a draft on the same engine is
/// "can't tell" (a slow post may still flip it), never "not saved".
#[tokio::test]
async fn a_lost_daily_post_still_a_draft_is_unknown_until_the_engine_moved_on() {
    let server = MockServer::start().await;
    let (s, src) = session(&server);
    Mock::given(method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(502))
        .mount(&server)
        .await;
    day_reads(&server, "draft", None, 5).await;
    let key = lost_day_post(&s).await;
    assert_eq!(s.recheck(&key).await, Ok(Recheck::Unknown));
    restart(&src, server.address().port());
    assert_eq!(s.recheck(&key).await, Ok(Recheck::NotCommitted));
    assert_eq!(
        requests(&server, "POST").await.len(),
        1,
        "recheck never posts"
    );
}

#[tokio::test]
async fn a_lost_daily_post_that_shows_up_posted_names_its_journal_entry() {
    let server = MockServer::start().await;
    let (s, _) = session(&server);
    Mock::given(method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(502))
        .mount(&server)
        .await;
    day_reads(&server, "draft", None, 5).await;
    let key = lost_day_post(&s).await;
    day_reads(&server, "posted", Some(T1), 1).await;
    assert_eq!(
        s.recheck(&key).await,
        Ok(Recheck::Committed {
            transaction: TransactionId::parse(T1).ok()
        })
    );
    assert_eq!(requests(&server, "POST").await.len(), 1);
}

async fn templates_session(server: &MockServer) -> (Session, Arc<FixedEndpoint>) {
    Mock::given(method("GET"))
        .and(path("/api/v1/desktop/info"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"mode":"desktop","initialized":true,"capabilities":["{}"]}}"#,
                crate::wire::Capabilities::TEMPLATES
            ),
        ))
        .mount(server)
        .await;
    let (s, src) = session(server);
    s.client().desktop_info().await.expect("desktop info");
    (s, src)
}

fn template_row(id: &str, source: &str, template: &str) -> String {
    format!(
        r#"{{"id":"{id}","source":"{source}","sourceId":"{template}","createdAt":"2199-01-01T00:00:00.000Z"}}"#
    )
}

#[tokio::test]
async fn enter_now_without_a_schedule_is_found_by_its_provenance() {
    let server = MockServer::start().await;
    let (s, _) = templates_session(&server).await;
    stamp(&server, 5).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/templates/{TPL}/enter")))
        .respond_with(wiremock::ResponseTemplate::new(502))
        .mount(&server)
        .await;
    // Another template's posting that day is never ours.
    let other = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a99";
    rows(
        &server,
        &[template_row(
            "7f0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            "template",
            other,
        )],
        5,
    )
    .await;
    let id = TemplateId::parse(TPL).expect("tpl");
    let key = IdempotencyKey::new();
    let e = s
        .templates()
        .enter(id, Some(day()), &PostingOptions::default(), key.clone())
        .await
        .expect_err("unknown");
    assert!(matches!(e, ApiError::OutcomeUnknown { .. }), "{e:?}");
    let search = server
        .received_requests()
        .await
        .expect("r")
        .into_iter()
        .find(|r| r.url.path() == "/api/v1/transactions")
        .expect("searched by provenance");
    assert_eq!(
        search.url.query(),
        Some("source=template&startDate=2026-10-15&endDate=2026-10-15&limit=500&offset=0")
    );
    // The posting shows up: exactly one, named by the template.
    rows(&server, &[template_row(T1, "template", TPL)], 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{T1}")))
        .respond_with(json(200, &wrapped(T1)))
        .mount(&server)
        .await;
    assert_eq!(
        s.recheck(&key).await,
        Ok(Recheck::Committed {
            transaction: TransactionId::parse(T1).ok()
        })
    );
    assert_eq!(
        requests(&server, "POST").await.len(),
        1,
        "exactly one posting"
    );
}

#[tokio::test]
async fn lost_enter_now_after_a_restart_with_nothing_posted_is_retried_once() {
    let server = MockServer::start().await;
    let (s, src) = templates_session(&server).await;
    stamp(&server, 5).await;
    let port = server.address().port();
    let src2 = src.clone();
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/templates/{TPL}/enter")))
        .respond_with(move |_: &wiremock::Request| {
            restart(&src2, port);
            wiremock::ResponseTemplate::new(500)
        })
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/templates/{TPL}/enter")))
        .respond_with(json(
            201,
            &format!(r#"{{"transactionId":"{T1}","occurrence":null}}"#),
        ))
        .mount(&server)
        .await;
    rows(&server, &[], 5).await;
    let id = TemplateId::parse(TPL).expect("tpl");
    let got = s
        .templates()
        .enter(
            id,
            Some(day()),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect("retried");
    assert_eq!(got.transaction, TransactionId::parse(T1).expect("t"));
    assert_eq!(
        requests(&server, "POST").await.len(),
        2,
        "one same-key retry"
    );
}
