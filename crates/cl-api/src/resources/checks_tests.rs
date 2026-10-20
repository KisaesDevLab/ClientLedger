//! Check records, the checks list and settings, and the check edit.

use super::*;
use crate::errors::ConflictReason;
use crate::resources::bank_postings::SplitLine;
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use crate::resources::transactions::{BilledStatus, TxnType};
use cl_core::ids::ContactId;
use cl_core::{Money, Secret};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const RECORD: &str = include_str!("../../../../fixtures/engine/check-record.json");
const QUEUED: &str = include_str!("../../../../fixtures/engine/check-record-queued.json");
const LIST: &str = include_str!("../../../../fixtures/engine/checks-list.json");

const ID: &str = "7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a18";
const BANK: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PLANTS: &str = "2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const MULCH: &str = "2c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const PAYEE: &str = "4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const JOB: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const INSTALL: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CREW: &str = "3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const MEMO: &str = "Acct 7712 · perennials and mulch";

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn a(s: &str) -> AccountId {
    AccountId::parse(s).expect("id")
}

fn id() -> TransactionId {
    TransactionId::parse(ID).expect("id")
}

fn line(account: &str, amount: &str) -> SplitLine {
    SplitLine {
        account: a(account),
        amount: m(amount),
        memo: None,
        contact: None,
        billable: false,
        tag: None,
    }
}

/// The check the golden record holds, as the form would send it.
fn story() -> CheckDraft {
    CheckDraft {
        bank: a(BANK),
        date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        payee: Some(ContactId::parse(PAYEE).expect("payee")),
        payee_name: "Sample Feed & Seed".into(),
        check_number: Some(4418),
        print_later: false,
        payee_address: Some("1200 Commerce Dr\nAurora, MO 65605".into()),
        memo: Some(MEMO.into()),
        printed_memo: Some(MEMO.into()),
        tag_ids: vec![TagId::parse(CREW).expect("tag")],
        draft_attachment: None,
        amount: m("1286.40"),
        lines: vec![
            SplitLine {
                contact: Some(ContactId::parse(JOB).expect("job")),
                billable: true,
                tag: Some(TagId::parse(INSTALL).expect("tag")),
                ..line(PLANTS, "842.15")
            },
            line(MULCH, "444.25"),
        ],
    }
}

fn posting(d: CheckDraft) -> BankPosting {
    BankPosting::check_with(d, true).expect("check")
}

fn record(body: &str) -> CheckRecord {
    let v: Value = serde_json::from_str(body).expect("json");
    CheckRecord::deserialize(&v["transaction"]).expect("check record")
}

fn inner(body: &str) -> Value {
    let v: Value = serde_json::from_str(body).expect("json");
    v["transaction"].clone()
}

#[test]
fn the_golden_check_record_parses_exactly() {
    let r = record(RECORD);
    assert_eq!(r.txn.id, id());
    assert_eq!(r.txn.txn_type, TxnType::Expense);
    assert_eq!(r.check_number, Some(4418));
    assert_eq!(r.print_status, PrintStatus::HandWritten);
    assert_eq!(r.payee_name, "Sample Feed & Seed");
    assert_eq!(
        r.payee_address.as_deref(),
        Some("1200 Commerce Dr\nAurora, MO 65605")
    );
    assert_eq!(r.printed_memo.as_deref(), Some(MEMO));
    assert_eq!(r.tag_ids, vec![TagId::parse(CREW).expect("tag")]);
    // Exact 4-decimal strings, never rounded: 842.15 + 444.25 = 1,286.40.
    assert_eq!(
        r.txn.total.map(Money::to_wire).as_deref(),
        Some("1286.4000")
    );
    let sides: Vec<(String, String)> = r
        .txn
        .lines
        .iter()
        .map(|l| (l.debit.to_wire(), l.credit.to_wire()))
        .collect();
    assert_eq!(
        sides,
        [
            ("842.1500", "0.0000"),
            ("444.2500", "0.0000"),
            ("0.0000", "1286.4000")
        ]
        .map(|(d, c)| (d.to_string(), c.to_string()))
    );
    assert!(r.txn.lines[0].is_billable);
    assert_eq!(r.txn.lines[0].billed_status, Some(BilledStatus::Unbilled));
    assert_eq!(r.txn.lines[0].contact_id, ContactId::parse(JOB).ok());
    assert!(!r.txn.lines[2].is_billable);
    // Debug: ids, number and status only.
    let shown = format!("{r:?}");
    for secret in ["Feed", "Commerce", "7712", "1286", "842"] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }

    let q = record(QUEUED);
    assert_eq!(q.check_number, None);
    assert_eq!(q.print_status, PrintStatus::Queue);
    assert_eq!(q.tag_ids, Vec::<TagId>::new());
    assert!(!q.txn.lines[0].is_billable, "older engines: not billable");
}

#[test]
fn unknown_statuses_are_kept_and_a_plain_expense_is_not_a_check() {
    let mut v = inner(RECORD);
    v["printStatus"] = Value::String("reprint_pending".into());
    let r = CheckRecord::deserialize(&v).expect("unknown status");
    assert_eq!(r.print_status, PrintStatus::Other("reprint_pending".into()));
    assert_eq!(r.print_status.as_wire(), "reprint_pending");
    v["printStatus"] = Value::Null;
    assert!(CheckRecord::deserialize(&v).is_err(), "not a check");
    let mut v = inner(RECORD);
    // A JSON number where money belongs (parsed from text: no float here).
    v["lines"][0]["debit"] = serde_json::from_str::<Value>("842.15").expect("number");
    assert!(CheckRecord::deserialize(&v).is_err(), "money as a number");
    let mut v = inner(RECORD);
    v.as_object_mut().expect("obj").remove("tags");
    v["payeeNameOnCheck"] = Value::Null;
    let r = CheckRecord::deserialize(&v).expect("no tags");
    assert_eq!(r.tag_ids, Vec::<TagId>::new());
    assert_eq!(r.payee_name, "");
}

/// The fingerprint `update_check` expects after `PUT /checks` equals the
/// stored check's, and the stale guard sees every check field.
#[test]
fn the_requested_fingerprint_is_the_stored_checks() {
    let d = story();
    let p = posting(d.clone());
    let stored = record(RECORD);
    assert_eq!(stored.fingerprint(), requested_fingerprint(&p, &d));
    // A printed check that keeps its number stays printed: same kind.
    let mut printed = inner(RECORD);
    printed["printStatus"] = Value::String("printed".into());
    printed["printedAt"] = Value::String("2026-11-05T10:00:00.000Z".into());
    let printed = CheckRecord::deserialize(&printed).expect("printed");
    assert_eq!(printed.fingerprint(), stored.fingerprint());
    // Header tags and joined names are not hashed.
    let mut other = inner(RECORD);
    other["tags"] = json!([]);
    other["lines"][0]["accountName"] = Value::String("Renamed".into());
    assert_eq!(
        CheckRecord::deserialize(&other).expect("r").fingerprint(),
        stored.fingerprint()
    );
    // Every check field is.
    for (k, v) in [
        ("checkNumber", json!(4419)),
        ("printStatus", json!("queue")),
        ("payeeNameOnCheck", json!("Other name")),
        ("payeeAddress", json!(null)),
        ("printedMemo", json!("x")),
    ] {
        let mut changed = inner(RECORD);
        changed[k] = v;
        let changed = CheckRecord::deserialize(&changed).expect("r");
        assert_ne!(changed.fingerprint(), stored.fingerprint(), "{k}");
    }
    // An empty printed text is stored as null.
    let mut empty = inner(RECORD);
    empty["payeeAddress"] = json!("");
    let mut null = inner(RECORD);
    null["payeeAddress"] = json!(null);
    assert_eq!(
        CheckRecord::deserialize(&empty).expect("r").fingerprint(),
        CheckRecord::deserialize(&null).expect("r").fingerprint()
    );

    // Print later: no number, `queue`.
    let q = CheckDraft {
        check_number: None,
        print_later: true,
        payee_address: None,
        memo: None,
        printed_memo: None,
        tag_ids: Vec::new(),
        amount: m("100"),
        lines: vec![line(MULCH, "100")],
        ..story()
    };
    let mut queued = record(QUEUED);
    assert_eq!(
        queued.fingerprint(),
        requested_fingerprint(&posting(q.clone()), &q)
    );
    queued.txn.lines[0].debit = m("100.0001");
    assert_ne!(
        queued.fingerprint(),
        requested_fingerprint(&posting(q.clone()), &q)
    );
    assert_eq!(CheckRecord::EDITABLE[..14], *Transaction::EDITABLE);
}

#[test]
fn the_checks_list_reads_exact_and_display_only_amounts() {
    let rows = checks_from_json(LIST.as_bytes()).expect("list");
    assert_eq!(rows.len(), 3);
    let date = |y, mo, d| CalendarDate::from_ymd(y, mo, d).ok();
    // Local midnight in UTC−5 and UTC+1, and a plain date.
    assert_eq!(
        rows.iter().map(|r| r.date).collect::<Vec<_>>(),
        vec![date(2026, 11, 4), date(2026, 11, 4), date(2026, 10, 2)]
    );
    assert_eq!(
        rows[0].amount.as_ref().and_then(RegisterAmount::exact),
        Some(m("1286.40"))
    );
    assert_eq!(rows[0].number, Some(4418));
    assert_eq!(rows[0].print_status, PrintStatus::HandWritten);
    assert_eq!(rows[1].number, None);
    assert_eq!(rows[1].print_status, PrintStatus::Queue);
    assert_eq!(
        rows[1].payee.as_deref(),
        Some("Sample Feed & Seed"),
        "the payee's name when none is printed"
    );
    // A JSON number is display-only: never exact money.
    let third = rows[2].amount.as_ref().expect("amount");
    assert!(!third.is_exact());
    assert_eq!(third.exact(), None);
    assert_eq!(third.display(false).as_deref(), Some("2318.60"));
    assert_eq!(rows[2].status, TxnStatus::Void);
    assert_eq!(rows[2].print_status, PrintStatus::Printed);
    let shown = format!("{rows:?}");
    assert!(
        !shown.contains("Feed") && !shown.contains("1286"),
        "{shown}"
    );
    for bad in [
        &br#"{"data":[{"id":"x","status":"posted"}]}"#[..],
        br#"{"rows":[]}"#,
        br#"{"data":[{"id":"7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a18","status":"posted","amount":"1.2.3"}]}"#,
    ] {
        assert!(checks_from_json(bad).is_err());
    }
    assert_eq!(list_date("2026-11-04T12:00:00.000Z"), date(2026, 11, 5));
    assert_eq!(list_date("not a date"), None);
}

#[test]
fn settings_read_only_the_next_numbers() {
    let body = format!(
        r#"{{"settings":{{"format":"voucher","accountNumber":"canary-123","routingNumber":"000000000",
            "nextCheckNumber":1001,"nextCheckNumbers":{{"{BANK}":4418,"{MULCH}":0,"not-an-id":7}}}}}}"#
    );
    let s = check_settings_from_json(body.as_bytes()).expect("settings");
    assert_eq!(s.next_for(a(BANK)), Some(4418));
    assert_eq!(
        s.next_for(a(MULCH)),
        Some(1001),
        "bad counter: company-wide"
    );
    assert_eq!(s.next_numbers.len(), 1);
    assert!(!format!("{s:?}").contains("canary"));
    let empty = check_settings_from_json(br#"{"settings":{}}"#).expect("empty");
    assert_eq!(empty.next_for(a(BANK)), None);
    assert!(check_settings_from_json(br#"{"settings":null}"#).is_err());
}

#[tokio::test]
async fn list_and_settings_send_their_queries() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/checks"))
        .respond_with(json(200, LIST))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/checks/settings"))
        .respond_with(json(
            200,
            &format!(r#"{{"settings":{{"nextCheckNumbers":{{"{BANK}":4419}}}}}}"#),
        ))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let rows = s
        .checks()
        .list(
            Some(a(BANK)),
            CalendarDate::from_ymd(2026, 1, 1).ok(),
            CalendarDate::from_ymd(2026, 12, 31).ok(),
            PageRequest::new(50, 50).expect("page"),
        )
        .await
        .expect("list");
    assert_eq!(rows.len(), 3);
    let next = s.checks().settings().await.expect("settings");
    assert_eq!(next.next_for(a(BANK)), Some(4419));
    let gets = requests(&server, "GET").await;
    assert_eq!(
        query(&gets[0]),
        format!(
            "bank_account_id={BANK}&start_date=2026-01-01&end_date=2026-12-31&limit=50&offset=50"
        )
    );
    assert_eq!(query(&gets[1]), "");
}

#[tokio::test]
async fn get_reads_a_check_and_refuses_a_plain_expense() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{ID}").as_str()))
        .respond_with(json(200, RECORD))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let mut plain = inner(RECORD);
    plain["printStatus"] = Value::Null;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{ID}").as_str()))
        .respond_with(json(200, &json!({ "transaction": plain }).to_string()))
        .mount(&server)
        .await;
    let s = company_session(&server);
    let got = s.checks().get(id()).await.expect("check");
    assert_eq!(got.record.check_number, Some(4418));
    assert_eq!(
        got.version,
        RecordVersion::Fingerprint(got.record.fingerprint())
    );
    let e = s.checks().get(id()).await.expect_err("not a check");
    assert!(matches!(e, ApiError::Decode { .. }), "{e:?}");
}

#[tokio::test]
async fn update_check_is_refused_locally_without_the_capability_or_a_number() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[]).await;
    let p = posting(story());
    let version = RecordVersion::Fingerprint(record(RECORD).fingerprint());
    let opts = PostingOptions::default();
    let e = s
        .checks()
        .update_check(id(), &version, &p, &opts, IdempotencyKey::new())
        .await
        .expect_err("gated");
    assert_eq!(e, ApiError::Local(LocalValidation::CheckEditUnavailable));

    let server2 = MockServer::start().await;
    let s = session_with_caps(&server2, &[Capabilities::CHECK_EDIT]).await;
    let mut unnumbered = story();
    unnumbered.check_number = None;
    let e = s
        .checks()
        .update_check(
            id(),
            &version,
            &posting(unnumbered),
            &opts,
            IdempotencyKey::new(),
        )
        .await
        .expect_err("no number");
    assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    let transfer = BankPosting::transfer(crate::resources::bank_postings::TransferDraft {
        from: a(BANK),
        to: a(MULCH),
        date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        amount: m("1"),
        memo: None,
    })
    .expect("transfer");
    let e = s
        .checks()
        .update_check(id(), &version, &transfer, &opts, IdempotencyKey::new())
        .await
        .expect_err("not a check");
    assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    // A closing password without `closing-password-v1`.
    let with_pw = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-canary-1",
        ))),
    };
    let e = s
        .checks()
        .update_check(id(), &version, &p, &with_pw, IdempotencyKey::new())
        .await
        .expect_err("password gated");
    assert_eq!(
        e,
        ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
    );
    for server in [&server, &server2] {
        assert!(requests(server, "PUT").await.is_empty(), "nothing sent");
        assert!(
            requests(server, "GET")
                .await
                .iter()
                .all(|r| r.url.path() == "/api/v1/desktop/info"),
            "not even the pre-read"
        );
    }
}

#[tokio::test]
async fn a_stale_check_edit_sends_nothing() {
    let server = MockServer::start().await;
    let mut changed = inner(RECORD);
    changed["checkNumber"] = json!(4420);
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{ID}").as_str()))
        .respond_with(json(200, &json!({ "transaction": changed }).to_string()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::CHECK_EDIT]).await;
    let e = s
        .checks()
        .update_check(
            id(),
            &RecordVersion::Fingerprint(record(RECORD).fingerprint()),
            &posting(story()),
            &PostingOptions::default(),
            IdempotencyKey::new(),
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
    assert!(requests(&server, "PUT").await.is_empty());
}

#[tokio::test]
async fn a_check_edit_goes_to_checks_with_the_password_and_reads_back() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/transactions/{ID}").as_str()))
        .respond_with(json(200, RECORD))
        .mount(&server)
        .await;
    // The PUT answer: the stored header and lines, no tags or names.
    let mut answer = inner(RECORD);
    answer.as_object_mut().expect("obj").remove("tags");
    answer["printLater"] = json!(false);
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/checks/{ID}").as_str()))
        .respond_with(json(200, &json!({ "check": answer }).to_string()))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/checks/{ID}").as_str()))
        .respond_with(json(
            409,
            r#"{"error":{"message":"Check #4417 already exists on this bank account."}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[Capabilities::CHECK_EDIT, Capabilities::CLOSING_PASSWORD],
    )
    .await;
    let stored = record(RECORD);
    let version = RecordVersion::Fingerprint(stored.fingerprint());
    let opts = PostingOptions {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "pw-canary-1",
        ))),
    };
    let saved = s
        .checks()
        .update_check(
            id(),
            &version,
            &posting(story()),
            &opts,
            IdempotencyKey::new(),
        )
        .await
        .expect("saved");
    assert_eq!(saved.record.check_number, Some(4418));
    assert_eq!(saved.version, version, "the next edit is not stale");
    let puts = requests(&server, "PUT").await;
    assert_eq!(puts.len(), 1);
    assert!(puts[0].headers.get("idempotency-key").is_some());
    let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
    assert_eq!(body["checkNumber"], 4418);
    assert_eq!(body["printLater"], false);
    assert_eq!(body["amount"], "1286.4000");
    assert_eq!(body["lines"][0]["amount"], "842.1500");
    assert_eq!(body["lines"][0]["isBillable"], true);
    assert_eq!(body["closingPassword"], "pw-canary-1");
    assert!(body.get("draftAttachmentId").is_none());
    assert!(body.get("overrideConfirmed").is_none());

    // A number used on the account: 409, the owner picks another.
    let mut dup = story();
    dup.check_number = Some(4417);
    let e = s
        .checks()
        .update_check(
            id(),
            &version,
            &posting(dup),
            &PostingOptions::default(),
            IdempotencyKey::new(),
        )
        .await
        .expect_err("duplicate");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Unspecified,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(!format!("{e} {e:?} {opts:?}").contains("canary"));
}
