//! Stub tests of the template calls: the draft bodies (strings only, no
//! date or numbers), the local refusals (nothing sent), the strict readers,
//! and each call's route.

#![allow(clippy::too_many_lines, clippy::many_single_char_names)]

use cl_core::Secret;
use cl_core::ids::TagId;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::*;
use crate::errors::ConflictReason;
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{
    company_session, json as reply, query, requests, session_with_caps,
};

const BANK: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const RENT: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const CARD: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const TPL: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04";
const TXN: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a05";
const GRP: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a06";
const TAG: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a07";
const LAYOUT: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a08";
const LINE: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn a(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn split(account: &str, amount: &str) -> SplitLine {
    SplitLine {
        account: a(account),
        amount: m(amount),
        memo: Some("canary memo".into()),
        contact: None,
        billable: false,
        tag: Some(TagId::parse(TAG).expect("tag")),
    }
}

fn check() -> CheckDraft {
    CheckDraft {
        bank: a(BANK),
        date: day(),
        payee: None,
        payee_name: "Canary Landlord".into(),
        check_number: Some(1043),
        print_later: false,
        payee_address: None,
        memo: None,
        printed_memo: None,
        tag_ids: vec![TagId::parse(TAG).expect("tag")],
        draft_attachment: None,
        amount: m("1200.00"),
        lines: vec![split(RENT, "1200.00")],
    }
}

fn deposit() -> DepositDraft {
    DepositDraft {
        account: a(BANK),
        date: day(),
        memo: Some("weekly".into()),
        payments: Vec::new(),
        lines: vec![split(RENT, "500.00")],
        deductions: vec![split(CARD, "12.50")],
        cash_back: Some(CashBack {
            account: a(CARD),
            amount: m("20.00"),
            memo: None,
        }),
    }
}

fn journal() -> NewJournalEntry {
    NewJournalEntry::new(
        day(),
        Some("health".into()),
        Basis::Both,
        vec![
            JeLine {
                account: a(RENT),
                side: Side::Debit(m("412.50")),
                description: None,
                tag: None,
                contact: None,
            },
            JeLine {
                account: a(BANK),
                side: Side::Credit(m("412.50")),
                description: None,
                tag: None,
                contact: None,
            },
        ],
    )
    .expect("balanced")
}

fn all() -> Vec<TemplateDraft> {
    vec![
        TemplateDraft::Check(check()),
        TemplateDraft::CardCharge(CardChargeDraft {
            pay_from: a(CARD),
            date: day(),
            number: Some("R-9".into()),
            payee: None,
            memo: None,
            lines: vec![split(RENT, "49.00")],
        }),
        TemplateDraft::Deposit(deposit()),
        TemplateDraft::Transfer(TransferDraft {
            from: a(BANK),
            to: a(CARD),
            date: day(),
            amount: m("250.00"),
            memo: None,
        }),
        TemplateDraft::Journal(journal()),
        TemplateDraft::DailySales(DailyEntryDraft {
            template: DailyTemplateId::parse(LAYOUT).expect("layout"),
            date: day(),
            tag: None,
            notes: None,
            values: vec![DailyValue {
                line: DailyLineId::parse(LINE).expect("line"),
                amount: m("1000.00"),
                tag: None,
            }],
        }),
    ]
}

fn caps(names: &[&str]) -> Capabilities {
    serde_json::from_value(json!(names)).expect("caps")
}

fn full_caps() -> Capabilities {
    caps(&[
        Capabilities::DEPOSIT_CASHBACK,
        Capabilities::LINE_BILLABLE,
        Capabilities::TEMPLATES,
    ])
}

/// Every number anywhere in `v` (none may be an amount).
fn numbers(v: &Value, at: &str, out: &mut Vec<String>) {
    match v {
        Value::Number(_) => out.push(at.to_string()),
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                numbers(x, &format!("{at}[{i}]"), out);
            }
        }
        Value::Object(o) => {
            for (k, x) in o {
                numbers(x, &format!("{at}.{k}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn each_draft_is_its_create_body_without_date_or_numbers() {
    for d in all() {
        let v = d.to_value(&full_caps()).expect("body");
        for k in [
            "txnDate",
            "businessDate",
            "checkNumber",
            "txnNumber",
            "reverseOn",
            "draftAttachmentId",
            "txnType",
        ] {
            assert!(v.get(k).is_none(), "{:?} keeps {k}", d.kind());
        }
        let mut found = Vec::new();
        numbers(&v, "", &mut found);
        assert!(found.is_empty(), "{:?}: numbers at {found:?}", d.kind());
        // Read back for the same date: the same draft, minus the numbers.
        let back = TemplateDraft::from_value(&d.kind(), day(), &v).expect("reads");
        let again = back.to_value(&full_caps()).expect("body");
        assert_eq!(again, v, "{:?} round trip", d.kind());
    }
    let v = TemplateDraft::Check(check())
        .to_value(&full_caps())
        .expect("check");
    assert_eq!(v["amount"], "1200.0000");
    assert_eq!(v["tagIds"], json!([TAG]));
    assert_eq!(v["lines"][0]["amount"], "1200.0000");
    let v = TemplateDraft::Deposit(deposit())
        .to_value(&full_caps())
        .expect("deposit");
    assert_eq!(
        v["lines"][1]["amount"], "-12.5000",
        "a fee goes out negative"
    );
    assert_eq!(v["cashBack"]["amount"], "20.0000");
    let v = TemplateDraft::Journal(journal())
        .to_value(&full_caps())
        .expect("je");
    assert_eq!(
        (&v["lines"][0]["debit"], &v["lines"][1]["credit"]),
        (&json!("412.5000"), &json!("412.5000"))
    );
}

#[test]
fn a_check_number_is_never_kept_and_a_fee_reads_back_positive() {
    let d = TemplateDraft::Check(check());
    let v = d.to_value(&full_caps()).expect("body");
    let TemplateDraft::Check(back) =
        TemplateDraft::from_value(&TemplateKind::Check, day(), &v).expect("check")
    else {
        panic!("a check");
    };
    assert_eq!(back.check_number, None, "Q-P15f");
    assert_eq!(back.amount, m("1200"));
    let v = TemplateDraft::Deposit(deposit())
        .to_value(&full_caps())
        .expect("body");
    let TemplateDraft::Deposit(back) =
        TemplateDraft::from_value(&TemplateKind::Deposit, day(), &v).expect("deposit")
    else {
        panic!("a deposit");
    };
    assert_eq!(back.deductions[0].amount, m("12.50"));
    assert_eq!(back.total(), deposit().total(), "exactly the same deposit");
}

#[test]
fn local_refusals_run_the_forms_own_builders() {
    let mut grouped = deposit();
    grouped
        .payments
        .push(crate::resources::bank_postings::GroupedPayment {
            source: TransactionId::parse(TXN).expect("txn"),
            kind: crate::resources::bank_postings::DepositSource::Payment,
            account: a(CARD),
            amount: m("10.00"),
            contact: None,
            memo: None,
        });
    assert_eq!(
        TemplateDraft::Deposit(grouped).to_value(&full_caps()),
        Err(LocalValidation::TemplateGrouping)
    );
    let mut off = check();
    off.amount = m("1199.99");
    assert_eq!(
        TemplateDraft::Check(off).to_value(&full_caps()),
        Err(LocalValidation::TotalMismatch),
        "a template can never hold what Save refuses"
    );
    // Fees need the cash-back capability, as on Save.
    assert_eq!(
        TemplateDraft::Deposit(deposit()).to_value(&caps(&[])),
        Err(LocalValidation::DepositCashBackUnavailable)
    );
    let s = TemplateSchedule {
        mode: ScheduleMode::Remind,
        frequency: Frequency::Monthly,
        interval: 0,
        next_date: day(),
        end: ScheduleEnd::Never,
    };
    assert_eq!(s.to_value(), Err(LocalValidation::InvalidValue));
    let early = TemplateSchedule {
        interval: 1,
        end: ScheduleEnd::On(CalendarDate::from_ymd(2026, 11, 3).expect("d")),
        ..s
    };
    assert_eq!(early.to_value(), Err(LocalValidation::InvalidValue));
    let count = TemplateSchedule {
        interval: 1,
        end: ScheduleEnd::After(0),
        ..s
    };
    assert_eq!(count.to_value(), Err(LocalValidation::InvalidValue));
    let ok = TemplateSchedule {
        interval: 2,
        end: ScheduleEnd::After(3),
        ..s
    };
    assert_eq!(
        ok.to_value().expect("ok"),
        json!({"mode":"remind","frequency":"monthly","intervalValue":2,
            "nextDate":"2026-11-04","remainingCount":3})
    );
}

#[test]
fn reads_are_strict() {
    let mut v = TemplateDraft::Transfer(TransferDraft {
        from: a(BANK),
        to: a(CARD),
        date: day(),
        amount: m("250.00"),
        memo: None,
    })
    .to_value(&full_caps())
    .expect("body");
    v["amount"] = serde_json::from_str::<Value>("250.5").expect("a JSON number");
    assert!(TemplateDraft::from_value(&TemplateKind::Transfer, day(), &v).is_err());
    v["amount"] = json!("250.00001");
    assert!(TemplateDraft::from_value(&TemplateKind::Transfer, day(), &v).is_err());
    // A dropped account reads as the missing marker, never another one.
    v["amount"] = json!("250.0000");
    v["fromAccountId"] = Value::Null;
    let TemplateDraft::Transfer(t) =
        TemplateDraft::from_value(&TemplateKind::Transfer, day(), &v).expect("reads")
    else {
        panic!("transfer");
    };
    assert_eq!(t.from, MISSING_ACCOUNT);
    assert!(TemplateDraft::from_value(&TemplateKind::Legacy, day(), &v).is_err());
    // A list row: the total is an exact string, a number is refused.
    let row = json!({"id":TPL,"name":"Shop rent","kind":"check","clientForm":"write_check",
        "groupId":GRP,"groupName":"Monthly bills","accountId":BANK,"contactId":null,
        "total":"1200.0000","lastEnteredDate":"2026-10-01T05:00:00.000Z","openSkipped":2,
        "legacy":false,"version":3,
        "schedule":{"mode":"reminder","frequency":"monthly","intervalValue":1,
            "nextDate":"2026-12-01","endDate":null,"remainingCount":null}});
    let t = Template::from_value(&row).expect("row");
    assert_eq!(t.total, Some(m("1200")));
    assert_eq!(t.version, 3);
    assert_eq!(t.open_skipped, 2);
    assert_eq!(
        t.last_entered,
        Some(CalendarDate::from_ymd(2026, 10, 1).expect("d"))
    );
    let s = t.schedule.expect("schedule");
    assert_eq!(
        (s.mode, s.frequency, s.end),
        (ScheduleMode::Remind, Frequency::Monthly, ScheduleEnd::Never)
    );
    let dbg = format!("{t:?}");
    assert!(!dbg.contains("Shop") && !dbg.contains("1200"));
    let mut bad = row.clone();
    bad["total"] = json!(1200);
    assert!(Template::from_value(&bad).is_err());
    let mut legacy = row;
    legacy["kind"] = json!("legacy");
    legacy["schedule"] = Value::Null;
    let t = Template::from_value(&legacy).expect("legacy");
    assert!(t.legacy && !t.kind.is_editable() && t.schedule.is_none());
}

#[tokio::test]
async fn nothing_is_sent_without_the_capability() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    let t = s.templates();
    let id = TemplateId::parse(TPL).expect("id");
    let gated = ApiError::Local(LocalValidation::TemplatesUnavailable);
    assert_eq!(t.list(None, 50, 0).await.expect_err("list"), gated);
    assert_eq!(t.get(id).await.expect_err("get"), gated);
    assert_eq!(t.draft(id, day()).await.expect_err("draft"), gated);
    assert_eq!(t.skip(id, day()).await.expect_err("skip"), gated);
    assert_eq!(t.groups().await.expect_err("groups"), gated);
    assert_eq!(
        t.enter(id, None, &PostingOptions::default(), IdempotencyKey::new())
            .await
            .expect_err("enter"),
        gated
    );
    assert!(
        server.received_requests().await.expect("r").is_empty(),
        "nothing sent"
    );
}

#[tokio::test]
async fn a_grouped_deposit_or_bad_name_is_refused_without_a_request() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::TEMPLATES]).await;
    let mut grouped = deposit();
    grouped.deductions.clear();
    grouped.cash_back = None;
    grouped
        .payments
        .push(crate::resources::bank_postings::GroupedPayment {
            source: TransactionId::parse(TXN).expect("txn"),
            kind: crate::resources::bank_postings::DepositSource::Payment,
            account: a(CARD),
            amount: m("10.00"),
            contact: None,
            memo: None,
        });
    let new = |draft, name: &str| NewTemplate {
        name: name.into(),
        group: None,
        client_form: None,
        draft,
        schedule: None,
    };
    let before = server.received_requests().await.expect("r").len();
    assert_eq!(
        s.templates()
            .create(&new(TemplateDraft::Deposit(grouped), "Weekly"))
            .await
            .expect_err("grouped"),
        ApiError::Local(LocalValidation::TemplateGrouping)
    );
    let tr = TemplateDraft::Transfer(TransferDraft {
        from: a(BANK),
        to: a(CARD),
        date: day(),
        amount: m("1"),
        memo: None,
    });
    for name in ["   ", &"x".repeat(256)] {
        assert_eq!(
            s.templates()
                .create(&new(tr.clone(), name))
                .await
                .expect_err("name"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
    }
    // Daily sales never enters automatically (Q-P15d).
    let mut daily = new(all().pop().expect("daily"), "Counter");
    daily.schedule = Some(TemplateSchedule {
        mode: ScheduleMode::Auto,
        frequency: Frequency::Daily,
        interval: 1,
        next_date: day(),
        end: ScheduleEnd::Never,
    });
    assert!(s.templates().create(&daily).await.is_err());
    assert_eq!(
        server.received_requests().await.expect("r").len(),
        before,
        "nothing sent"
    );
}

fn template_json(version: u32) -> String {
    format!(
        r#"{{"template":{{"id":"{TPL}","name":"Shop rent","kind":"check","clientForm":"write_check",
        "groupId":null,"groupName":null,"accountId":"{BANK}","contactId":null,"total":"1200.0000",
        "schedule":null,"lastEnteredDate":null,"openSkipped":0,"legacy":false,"version":{version},
        "draft":{{"bankAccountId":"{BANK}","payeeNameOnCheck":"Canary Landlord","amount":"1200.0000",
          "printLater":false,"tagIds":[],"lines":[{{"accountId":"{RENT}","amount":"1200.0000","tagId":null}}]}}}}}}"#
    )
}

#[tokio::test]
async fn calls_reach_their_routes_with_string_bodies() {
    let server = MockServer::start().await;
    let s = session_with_caps(
        &server,
        &[
            Capabilities::TEMPLATES,
            Capabilities::LINE_BILLABLE,
            Capabilities::CLOSING_PASSWORD,
        ],
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/templates"))
        .respond_with(reply(
            200,
            &json!({
                "templates": [serde_json::from_str::<Value>(&template_json(1))
                    .expect("json")["template"]
                    .clone()],
                "total": 1,
            })
            .to_string(),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/templates"))
        .respond_with(reply(201, &template_json(1)))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/templates/{TPL}")))
        .respond_with(reply(200, &template_json(1)))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/templates/{TPL}")))
        .respond_with(reply(200, &template_json(2)))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/templates/{TPL}")))
        .respond_with(wiremock::ResponseTemplate::new(204))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/templates/{TPL}/draft")))
        .respond_with(reply(
            200,
            &format!(
                r#"{{"kind":"transfer","clientForm":"transfer",
                "draft":{{"fromAccountId":null,"toAccountId":"{CARD}","amount":"250.0000"}},
                "warnings":[{{"field":"fromAccountId","code":"ACCOUNT_INACTIVE"}},
                            {{"field":"memo","code":"SOMETHING_NEW"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/templates/{TPL}/enter")))
        .respond_with(reply(
            201,
            &format!(
                r#"{{"transaction":{{"id":"{TXN}"}},"occurrence":{{"occurrenceDate":"2026-11-01"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/templates/{TPL}/occurrences/2026-11-01/skip"
        )))
        .respond_with(reply(200, "{}"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/v1/templates/{TPL}/occurrences/2026-11-01/resolve"
        )))
        .respond_with(reply(200, "{}"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/templates/groups"))
        .respond_with(reply(
            200,
            &format!(r#"{{"groups":[{{"id":"{GRP}","name":"Monthly bills"}}]}}"#),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/templates/groups"))
        .respond_with(reply(
            201,
            &format!(r#"{{"group":{{"id":"{GRP}","name":"Monthly bills"}}}}"#),
        ))
        .mount(&server)
        .await;

    let t = s.templates();
    let id = TemplateId::parse(TPL).expect("id");
    let (rows, total) = t.list(None, 50, 0).await.expect("list");
    assert_eq!((rows.len(), total), (1, 1));
    assert_eq!(
        t.list(None, 501, 0).await.expect_err("limit"),
        ApiError::Local(LocalValidation::PageLimit)
    );
    assert!(query(requests(&server, "GET").await.last().expect("get")).contains("limit=50"));
    let made = t
        .create(&NewTemplate {
            name: "  Shop rent ".into(),
            group: Some(TemplateGroupId::parse(GRP).expect("g")),
            client_form: None,
            draft: TemplateDraft::Check(check()),
            schedule: Some(TemplateSchedule {
                mode: ScheduleMode::Remind,
                frequency: Frequency::Monthly,
                interval: 1,
                next_date: CalendarDate::from_ymd(2026, 12, 1).expect("d"),
                end: ScheduleEnd::Never,
            }),
        })
        .await
        .expect("made");
    assert_eq!(made.version, 1);
    let posts = requests(&server, "POST").await;
    let sent: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(sent["name"], "Shop rent");
    assert_eq!(sent["kind"], "check");
    assert_eq!(sent["clientForm"], "write_check");
    assert_eq!(sent["groupId"], GRP);
    assert_eq!(sent["draft"]["amount"], "1200.0000");
    assert!(sent["draft"].get("checkNumber").is_none());
    assert_eq!(sent["schedule"]["nextDate"], "2026-12-01");
    assert!(posts[0].headers.get("idempotency-key").is_some());

    let got = t.get(id).await.expect("get");
    let TemplateDraft::Check(c) = got.draft_for(day()).expect("draft") else {
        panic!("check");
    };
    assert_eq!((c.amount, c.date), (m("1200"), day()));

    let changed = t
        .update(
            id,
            1,
            &TemplateChange {
                schedule: Some(None),
                ..TemplateChange::default()
            },
        )
        .await
        .expect("put");
    assert_eq!(changed.version, 2);
    let put: Value = serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
    assert_eq!(put, json!({"version":1,"schedule":null}));
    t.delete(id, 2).await.expect("delete");
    assert_eq!(query(&requests(&server, "DELETE").await[0]), "version=2");

    let u = t.draft(id, day()).await.expect("use");
    let TemplateDraft::Transfer(tr) = &u.draft else {
        panic!("transfer");
    };
    assert_eq!(
        (tr.from, tr.amount, tr.date),
        (MISSING_ACCOUNT, m("250"), day())
    );
    assert_eq!(u.warnings[0].code, DraftWarningCode::AccountInactive);
    assert_eq!(
        u.warnings[1].code,
        DraftWarningCode::Other("SOMETHING_NEW".into())
    );
    assert!(query(requests(&server, "GET").await.last().expect("get")).contains("date=2026-11-04"));

    let on = CalendarDate::from_ymd(2026, 11, 1).expect("d");
    let e = t
        .enter(
            id,
            Some(on),
            &PostingOptions {
                closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
                    "demo-close".to_string(),
                ))),
            },
            IdempotencyKey::new(),
        )
        .await
        .expect("entered");
    assert_eq!(e.transaction, TransactionId::parse(TXN).expect("txn"));
    assert_eq!(e.occurrence, Some(on));
    let posts = requests(&server, "POST").await;
    let sent: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(
        sent,
        json!({"occurrenceDate":"2026-11-01","closingPassword":"demo-close"})
    );
    t.skip(id, on).await.expect("skip");
    t.resolve(id, on, TransactionId::parse(TXN).expect("txn"))
        .await
        .expect("resolve");
    let posts = requests(&server, "POST").await;
    let sent: Value = serde_json::from_slice(&posts[3].body).expect("json");
    assert_eq!(sent, json!({"transactionId": TXN}));
    let groups = t.groups().await.expect("groups");
    assert_eq!(groups[0].name, "Monthly bills");
    let g = t.create_group("Monthly bills").await.expect("group");
    assert_eq!(g.id, groups[0].id);
}

#[tokio::test]
async fn stale_and_taken_map_to_their_conflicts() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::TEMPLATES]).await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/templates/{TPL}")))
        .respond_with(reply(
            409,
            r#"{"error":{"message":"changed","code":"STALE","details":{"version":4}}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/templates/{TPL}/enter")))
        .respond_with(reply(
            409,
            r#"{"error":{"message":"taken","code":"OCCURRENCE_TAKEN"}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/templates"))
        .respond_with(reply(
            409,
            r#"{"error":{"message":"taken","code":"TEMPLATE_NAME_TAKEN"}}"#,
        ))
        .mount(&server)
        .await;
    let id = TemplateId::parse(TPL).expect("id");
    let e = s
        .templates()
        .update(
            id,
            3,
            &TemplateChange {
                name: Some("New".into()),
                ..TemplateChange::default()
            },
        )
        .await
        .expect_err("stale");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    let e = s
        .templates()
        .enter(id, None, &PostingOptions::default(), IdempotencyKey::new())
        .await
        .expect_err("taken");
    assert!(is_occurrence_taken(&e) && !is_name_taken(&e));
    let e = s
        .templates()
        .create(&NewTemplate {
            name: "Shop rent".into(),
            group: None,
            client_form: None,
            draft: TemplateDraft::Journal(journal()),
            schedule: None,
        })
        .await
        .expect_err("name");
    assert!(is_name_taken(&e));
    let sent: Value =
        serde_json::from_slice(&requests(&server, "POST").await.last().expect("p").body)
            .expect("json");
    assert_eq!(sent["kind"], "journal_entry");
    assert_eq!(sent["clientForm"], "journal_entry");
}
