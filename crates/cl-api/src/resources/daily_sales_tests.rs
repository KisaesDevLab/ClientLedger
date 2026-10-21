//! Stub tests of the daily-sales calls: exact string bodies, the local
//! refusals (nothing sent), the 4-dp parsers and unknown words.

use cl_core::Secret;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::*;
use crate::resources::testkit::{
    company_session, json as reply, query, requests, session_with_caps,
};

const TPL: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const SALES: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const TAX: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const CASH: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04";
const ENTRY: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a05";
const ACCT: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a06";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn lid(s: &str) -> DailyLineId {
    DailyLineId::parse(s).expect("line")
}

fn value(line: &str, amount: &str) -> DailyValue {
    DailyValue {
        line: lid(line),
        amount: m(amount),
        tag: None,
    }
}

fn draft() -> DailyEntryDraft {
    DailyEntryDraft {
        template: DailyTemplateId::parse(TPL).expect("tpl"),
        date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        tag: None,
        notes: Some("canary note".into()),
        values: vec![
            value(SALES, "1000.00"),
            value(TAX, "80"),
            value(CASH, "1077.65"),
        ],
    }
}

fn entry_json(status: &str) -> String {
    format!(
        r#"{{"entry":{{"id":"{ENTRY}","templateId":"{TPL}","businessDate":"2026-11-04",
        "status":"{status}","transactionId":null,"tagId":null,"notes":"canary note",
        "overShortAmount":"-2.3500","totalSales":"1000.0000","totalTax":"80.0000",
        "totalPayments":"1077.6500","values":[{{"id":"x","templateLineId":"{SALES}",
        "amount":"1000.0000","tagId":null}}],
        "template":{{"id":"{TPL}","name":"Counter","presetType":"retail","isActive":true,
        "defaultTagId":null,"lines":[
          {{"id":"{TAX}","section":"tax","label":"Tax","accountId":"{ACCT}","tagId":null,
            "normalSide":"credit","sortOrder":1,"isRequired":false,"allowTag":false,"isActive":true}},
          {{"id":"{SALES}","section":"sales","label":"Plant sales","accountId":null,"tagId":null,
            "normalSide":"credit","sortOrder":0,"isRequired":true,"allowTag":false,"isActive":true}},
          {{"id":"{CASH}","section":"gift_cards","label":"Odd","accountId":null,"tagId":null,
            "normalSide":"debit","sortOrder":2,"isRequired":null,"allowTag":null,"isActive":null}}
        ]}}}}}}"#
    )
}

#[test]
fn parses_four_decimal_strings_and_unknown_words() {
    let e = entry_from_json(entry_json("posted").as_bytes()).expect("entry");
    assert_eq!(e.over_short, m("-2.35"));
    assert_eq!(e.totals.payments.to_wire(), "1077.6500");
    assert_eq!(e.status, EntryStatus::Posted);
    assert_eq!(e.values[0].amount, m("1000"));
    let layout = e.layout.clone().expect("layout");
    assert_eq!(layout.preset, Preset::Retail);
    let odd = &layout.lines[2];
    assert_eq!(odd.section, Section::Unknown("gift_cards".into()));
    assert!(
        odd.active && !odd.required,
        "nulls read as the engine's defaults"
    );
    let weird = entry_from_json(entry_json("archived").as_bytes()).expect("entry");
    assert_eq!(weird.status, EntryStatus::Unknown("archived".into()));
    // A JSON number for an amount is refused.
    let bad = entry_json("posted").replace(r#""-2.3500""#, "-2.35");
    assert_eq!(
        entry_from_json(bad.as_bytes()).expect_err("number"),
        ApiError::Decode {
            what: "daily-sales day"
        }
    );
    let dbg = format!("{e:?} {:?} {:?}", draft(), layout.lines);
    assert!(!dbg.contains("canary") && !dbg.contains("Plant") && !dbg.contains("1000"));
}

#[test]
fn preview_reads_exact_figures() {
    let p = preview_from_json(
        br#"{"totalDebits":"1077.6500","totalCredits":"1080.0000","overShort":"-2.3500",
        "balanced":false,"totalSales":"1000.0000","totalTax":"80.0000",
        "totalPayments":"1077.6500","unmappedLabels":["Dept 2"]}"#,
    )
    .expect("preview");
    assert_eq!(p.over_short, m("-2.35"));
    assert_eq!(p.debits.checked_sub(p.credits).expect("sub"), p.over_short);
    assert_eq!(p.unmapped, vec!["Dept 2".to_string()]);
    assert!(preview_from_json(br#"{"totalDebits":1,"totalCredits":"0","overShort":"0"}"#).is_err());
}

#[test]
fn draft_and_line_bodies_carry_strings_only() {
    let body = draft_body(&draft(), true).expect("body");
    assert_eq!(body["templateId"], TPL);
    assert_eq!(body["businessDate"], "2026-11-04");
    let amounts: Vec<&Value> = body["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| &v["amount"])
        .collect();
    assert_eq!(
        amounts,
        [&json!("1000.0000"), &json!("80.0000"), &json!("1077.6500")]
    );
    let update = draft_body(&draft(), false).expect("body");
    assert!(
        update.get("templateId").is_none(),
        "a draft keeps its layout"
    );

    let lines = lines_body(&[DailyLineInput {
        id: None,
        section: Section::Tax,
        label: "  Sales tax collected ".into(),
        account: AccountId::parse(ACCT).ok(),
        tag: None,
        side: NormalSide::Credit,
        required: false,
        active: true,
    }])
    .expect("lines");
    assert_eq!(
        lines,
        json!({"lines":[{"section":"tax","label":"Sales tax collected","accountId":ACCT,
            "tagId":null,"normalSide":"credit","sortOrder":0,"isRequired":false,
            "allowTag":false,"isActive":true}]})
    );
}

#[test]
fn local_refusals() {
    let mut d = draft();
    d.values.push(value(SALES, "1.00"));
    assert_eq!(
        draft_body(&d, true).expect_err("twice"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    let mut d = draft();
    d.values[0].amount = m("-0.01");
    assert_eq!(
        draft_body(&d, true).expect_err("negative"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    let mut d = draft();
    d.values = (0..=MAX_DAILY_VALUES)
        .map(|_| DailyValue {
            line: DailyLineId::from_uuid(uuid::Uuid::new_v4()),
            amount: m("1"),
            tag: None,
        })
        .collect();
    assert_eq!(
        draft_body(&d, true).expect_err("201"),
        ApiError::Local(LocalValidation::TooManyLines)
    );
    let mut d = draft();
    d.notes = Some("x".repeat(DAILY_NOTES_MAX_CHARS + 1));
    assert!(draft_body(&d, true).is_err());
    let line = |section: Section, label: &str| DailyLineInput {
        id: None,
        section,
        label: label.into(),
        account: None,
        tag: None,
        side: NormalSide::Debit,
        required: false,
        active: true,
    };
    assert!(lines_body(&[line(Section::Unknown("x".into()), "A")]).is_err());
    assert!(lines_body(&[line(Section::Payment, "   ")]).is_err());
    assert!(lines_body(&[line(Section::Payment, &"x".repeat(121))]).is_err());
    let many: Vec<_> = (0..=MAX_DAILY_LINES)
        .map(|_| line(Section::Payment, "Cash"))
        .collect();
    assert_eq!(
        lines_body(&many).expect_err("201"),
        ApiError::Local(LocalValidation::TooManyLines)
    );
}

fn password() -> DailyPostOptions {
    DailyPostOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "demo-close".to_string(),
        ))),
    }
}

#[tokio::test]
async fn a_password_needs_the_lock_capability() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let id = DailyEntryId::parse(ENTRY).expect("id");
    for e in [
        s.daily_sales()
            .post(id, &password(), IdempotencyKey::new())
            .await,
        s.daily_sales()
            .void(id, &password(), IdempotencyKey::new())
            .await,
    ] {
        assert_eq!(
            e.expect_err("gated"),
            ApiError::Local(LocalValidation::DailySalesLockUnavailable)
        );
    }
    assert!(server.received_requests().await.expect("r").is_empty());
}

#[tokio::test]
async fn post_sends_the_password_with_the_capability() {
    let server = MockServer::start().await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::DAILY_SALES_LOCK,
            Capabilities::CLOSING_PASSWORD,
        ],
    )
    .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/daily-sales/entries/{ENTRY}/post")))
        .respond_with(reply(200, &entry_json("posted")))
        .mount(&server)
        .await;
    let id = DailyEntryId::parse(ENTRY).expect("id");
    let e = s
        .daily_sales()
        .post(id, &password(), IdempotencyKey::new())
        .await
        .expect("posted");
    assert_eq!(e.status, EntryStatus::Posted);
    let sent: Value =
        serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
    assert_eq!(sent, json!({"closingPassword":"demo-close"}));
    // Without a password the body is empty.
    s.daily_sales()
        .post(id, &DailyPostOptions::default(), IdempotencyKey::new())
        .await
        .expect("posted");
    let sent: Value =
        serde_json::from_slice(&requests(&server, "POST").await[1].body).expect("json");
    assert_eq!(sent, json!({}));
}

#[tokio::test]
async fn calls_reach_the_documented_routes() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    Mock::given(method("GET"))
        .and(path("/api/v1/daily-sales/templates"))
        .respond_with(reply(
            200,
            &format!(r#"{{"templates":[{{"id":"{TPL}","name":"Counter","presetType":"custom","isActive":true}}]}}"#),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/daily-sales/templates/{TPL}")))
        .respond_with(reply(
            200,
            &entry_json("draft")
                .split_once(r#""template":"#)
                .map(|(_, t)| format!(r#"{{"template":{}"#, &t[..t.len() - 1]))
                .expect("template"),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/daily-sales/entries"))
        .respond_with(reply(
            200,
            &format!(
                r#"{{"total":2,"entries":[
                {{"id":"{ENTRY}","templateId":"{TPL}","templateName":"C","businessDate":"2026-11-04",
                  "status":"draft","transactionId":null,"overShortAmount":"0.0000","totalSales":"0.0000"}},
                {{"id":"{SALES}","templateId":"{TPL}","templateName":"C","businessDate":"2026-11-03",
                  "status":"draft","transactionId":null,"overShortAmount":"0.6500","totalSales":"1.0000"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/daily-sales/entries"))
        .respond_with(reply(201, &entry_json("draft")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/daily-sales/entries/preview"))
        .respond_with(reply(
            200,
            r#"{"totalDebits":"1077.6500","totalCredits":"1080.0000","overShort":"-2.3500",
            "balanced":false,"totalSales":"1000.0000","totalTax":"80.0000",
            "totalPayments":"1077.6500","unmappedLabels":[]}"#,
        ))
        .mount(&server)
        .await;

    let ds = s.daily_sales();
    let list = ds.templates().await.expect("templates");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].lines.len(), 0, "the list has no lines");
    let t = ds.template(list[0].id).await.expect("template");
    let order: Vec<_> = t.lines.iter().map(|l| l.sort).collect();
    assert_eq!(order, [0, 1, 2], "lines come back in form order");
    let day = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let drafts = ds
        .list(&DailyEntryQuery {
            status: Some(EntryStatus::Draft),
            template: Some(t.id),
            date: Some(day),
            limit: Some(50),
        })
        .await
        .expect("list");
    assert_eq!(drafts.len(), 1, "the date filter runs here");
    let q = query(&requests(&server, "GET").await[2]);
    assert!(q.contains("status=draft") && q.contains("limit=50") && !q.contains("2026"));
    assert_eq!(
        ds.list(&DailyEntryQuery {
            limit: Some(501),
            ..DailyEntryQuery::default()
        })
        .await
        .expect_err("limit"),
        ApiError::Local(LocalValidation::PageLimit)
    );
    let made = ds
        .create_draft(&draft(), IdempotencyKey::new())
        .await
        .expect("draft");
    assert_eq!(made.status, EntryStatus::Draft);
    let p = ds.preview(t.id, &draft().values).await.expect("preview");
    assert_eq!(p.over_short, m("-2.35"));
    let posts = requests(&server, "POST").await;
    assert!(posts[0].headers.get("idempotency-key").is_some());
    let sent: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(sent["values"][2]["amount"], "1077.6500");
    assert_eq!(sent["templateId"], TPL);
}
