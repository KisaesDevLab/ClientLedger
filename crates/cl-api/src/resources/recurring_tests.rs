//! Stub tests of the catch-up run: the request (a date and a run id,
//! never amounts or a password), the strict reader and the gate.

use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::*;
use crate::resources::testkit::{company_session, json as reply, requests, session_with_caps};

const TPL: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const TXN: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const RUN: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";

fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("date")
}

fn answer() -> String {
    format!(
        r#"{{"asOf":"2026-11-04","runId":"{RUN}","more":true,
        "posted":[{{"templateId":"{TPL}","templateName":"Accounting software","kind":"expense",
            "clientForm":"card_charge","occurrenceDate":"2026-09-15","transactionId":"{TXN}",
            "total":"49.0000"}}],
        "reminders":[{{"templateId":"{TPL}","templateName":"Shop rent","kind":"check",
            "clientForm":"write_check","dueDates":["2026-11-01"],"dueCount":1,"total":"1200.0000"}}],
        "skipped":[{{"templateId":"{TPL}","templateName":"X","occurrenceDate":"2026-10-15",
            "reason":"CLOSED_PERIOD","new":true}},
            {{"templateId":"{TPL}","templateName":"X","occurrenceDate":"2026-10-16",
            "reason":"BRAND_NEW"}}],
        "failed":[{{"templateId":"{TPL}","reason":"INTERNAL"}}]}}"#
    )
}

#[test]
fn reads_every_part_exactly() {
    let r = run_due_from_json(answer().as_bytes()).expect("run");
    assert_eq!(r.as_of, d(2026, 11, 4));
    assert!(r.more);
    assert_eq!(r.posted[0].date, d(2026, 9, 15));
    assert_eq!(
        r.posted[0].total,
        Some(Money::parse_wire("49.00").expect("m"))
    );
    assert_eq!(r.posted[0].kind, TemplateKind::Expense);
    assert_eq!(r.reminders[0].dates, vec![d(2026, 11, 1)]);
    assert_eq!(r.skipped[0].reason, SkipReason::ClosedPeriod);
    assert!(r.skipped[0].new && !r.skipped[1].new);
    assert_eq!(r.skipped[1].reason, SkipReason::Other("BRAND_NEW".into()));
    assert_eq!(r.failed.len(), 1);
    let dbg = format!("{r:?}");
    assert!(!dbg.contains("Accounting") && !dbg.contains("49"));
    // A JSON number total is refused.
    let bad = answer().replace(r#""49.0000""#, "49");
    assert!(run_due_from_json(bad.as_bytes()).is_err());
    let a = attention_from_json(answer().as_bytes()).expect("attention");
    assert_eq!((a.reminders.len(), a.skipped.len()), (1, 2));
    for r in [
        SkipReason::ClosedPeriod,
        SkipReason::InvalidAccount,
        SkipReason::InvalidContact,
        SkipReason::InvalidTag,
        SkipReason::CheckNumber,
        SkipReason::Validation,
        SkipReason::SourceMissing,
        SkipReason::NeedsCheck,
        SkipReason::AlreadyPosted,
    ] {
        assert_eq!(SkipReason::from_wire(r.as_wire()), r);
    }
}

#[tokio::test]
async fn nothing_is_sent_without_the_capability_or_with_a_bad_limit() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    assert_eq!(
        s.recurring()
            .run_due(d(2026, 11, 4), new_run_id(), 200)
            .await
            .expect_err("gated"),
        ApiError::Local(LocalValidation::RunDueUnavailable)
    );
    assert!(s.recurring().attention().await.is_err());
    assert!(
        server.received_requests().await.expect("r").is_empty(),
        "nothing sent"
    );
    let s = session_with_caps(&server, &[Capabilities::RECURRING_RUN_DUE]).await;
    let before = server.received_requests().await.expect("r").len();
    for limit in [0, 501] {
        assert_eq!(
            s.recurring()
                .run_due(d(2026, 11, 4), new_run_id(), limit)
                .await
                .expect_err("limit"),
            ApiError::Local(LocalValidation::PageLimit)
        );
    }
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

#[tokio::test]
async fn a_run_sends_a_date_and_its_run_id_only() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::RECURRING_RUN_DUE]).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/recurring/run-due"))
        .respond_with(reply(200, &answer()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/recurring/attention"))
        .respond_with(reply(200, r#"{"reminders":[],"skipped":[]}"#))
        .mount(&server)
        .await;
    let run = RunId::parse(RUN).expect("run");
    let r = s
        .recurring()
        .run_due(d(2026, 11, 4), run, RUN_LIMIT_DEFAULT)
        .await
        .expect("run");
    assert_eq!(r.run_id, run);
    let sent: Value =
        serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
    assert_eq!(
        sent,
        json!({"asOf":"2026-11-04","runId":RUN,"limit":200}),
        "no amounts, no password"
    );
    assert_eq!(
        s.recurring().attention().await.expect("attention"),
        Attention::default()
    );
    assert_ne!(new_run_id(), new_run_id());
}
