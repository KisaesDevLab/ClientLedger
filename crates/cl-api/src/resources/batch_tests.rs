//! Stub tests of the Bulk Entry calls (P15A WP-A1): golden bodies for
//! every kind, amounts only as strings, the closing-date password in the
//! body only, the capability gates (nothing sent), refused rows mapped to
//! their groups, and `parse-csv` read exactly.

#![allow(clippy::too_many_lines)]

use cl_core::Secret;
use cl_core::ids::TagId;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

use super::*;
use crate::resources::testkit::{json as reply, query, requests, session_with_caps};

const BANK: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CARD: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const RENT: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const VENDOR: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a04";
const TAG: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a05";
const TXN: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a06";
const BATCH: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a07";

const ALL: [&str; 6] = [
    Capabilities::BATCH_ATOMIC,
    Capabilities::BATCH_CHECK,
    Capabilities::BATCH_MIXED,
    Capabilities::BATCH_EXACT,
    Capabilities::BATCH_IDEMPOTENT,
    Capabilities::BATCH_CLOSING,
];

#[allow(clippy::unnecessary_wraps)] // fills `Option` fields
fn amt(s: &str) -> Option<BatchAmount> {
    Some(BatchAmount::new(Money::parse_wire(s).expect("money")).expect("cents"))
}

fn day() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 21).expect("date")
}

fn a(s: &str) -> AccountId {
    AccountId::parse(s).expect("account")
}

fn row(n: u32) -> BatchRowIn {
    BatchRowIn::new(n, day())
}

/// A check with a picked payee and account, and a typed-name check.
fn checks(first: u32) -> BatchGroup {
    let mut picked = row(first);
    picked.number = Some("1043".into());
    picked.contact = Some(ContactId::parse(VENDOR).expect("vendor"));
    picked.contact_name = Some("Canary Vendor".into());
    picked.account = Some(a(RENT));
    picked.amount = amt("1200.5");
    picked.memo = Some("canary memo".into());
    picked.tag = Some(TagId::parse(TAG).expect("tag"));
    let mut typed = row(first + 1);
    typed.print_later = true;
    typed.contact_name = Some("Canary Landlord".into());
    typed.account_name = Some("6100".into());
    typed.amount = amt("0.05");
    BatchGroup {
        kind: BatchKind::Check,
        context: Some(a(BANK)),
        rows: vec![picked, typed],
    }
}

fn simple(kind: BatchKind, context: Option<&str>, n: u32, amount: &str) -> BatchGroup {
    let mut r = row(n);
    r.contact_name = Some("Canary Name".into());
    r.account_name = Some("Supplies".into());
    r.number = Some("R-7".into());
    r.amount = amt(amount);
    BatchGroup {
        kind,
        context: context.map(a),
        rows: vec![r],
    }
}

fn journal(first: u32) -> BatchGroup {
    let mut dr = row(first);
    dr.number = Some("JE-9".into());
    dr.account = Some(a(RENT));
    dr.debit = amt("10.01");
    dr.memo = Some("accrual".into());
    let mut cr = row(first + 1);
    cr.number = Some("JE-9".into());
    cr.account_name = Some("Accrued liabilities".into());
    cr.credit = amt("10.01");
    BatchGroup {
        kind: BatchKind::JournalEntry,
        context: None,
        rows: vec![dr, cr],
    }
}

fn every_kind() -> Vec<BatchGroup> {
    vec![
        checks(1),
        simple(BatchKind::Expense, Some(BANK), 3, "19.99"),
        simple(BatchKind::CardCharge, Some(CARD), 4, "45.10"),
        simple(BatchKind::CardCredit, Some(CARD), 5, "5.00"),
        simple(BatchKind::Deposit, Some(BANK), 6, "999.99"),
        journal(7),
    ]
}

async fn body_of(server: &MockServer) -> Value {
    let posts = requests(server, "POST").await;
    serde_json::from_slice(&posts.last().expect("a POST").body).expect("json body")
}

/// Every JSON number in `v`, by key.
fn numbers(v: &Value, key: &str, out: &mut Vec<String>) {
    match v {
        Value::Number(_) => out.push(key.to_string()),
        Value::Array(items) => items.iter().for_each(|i| numbers(i, key, out)),
        Value::Object(map) => map.iter().for_each(|(k, i)| numbers(i, k, out)),
        _ => {}
    }
}

async fn no_request_sent(server: &MockServer) {
    assert!(requests(server, "POST").await.is_empty(), "nothing sent");
}

#[tokio::test]
async fn validate_golden_body_for_every_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/validate"))
        .respond_with(reply(200, r#"{"validCount":0,"invalidCount":0,"rows":[]}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    s.batch().validate(&every_kind()).await.expect("validate");
    let body = body_of(&server).await;
    let expected = json!({"groups": [
        {"txn_type": "check", "context_account_id": BANK, "rows": [
            {"rowNumber": 1, "date": "2026-10-21", "refNo": "1043", "contactId": VENDOR,
             "accountId": RENT, "memo": "canary memo", "amount": "1200.50", "tagId": TAG},
            {"rowNumber": 2, "date": "2026-10-21", "contactName": "Canary Landlord",
             "accountName": "6100", "amount": "0.05", "printLater": true}
        ]},
        {"txn_type": "expense", "context_account_id": BANK, "rows": [
            {"rowNumber": 3, "date": "2026-10-21", "refNo": "R-7", "contactName": "Canary Name",
             "accountName": "Supplies", "amount": "19.99"}]},
        {"txn_type": "credit_card_charge", "context_account_id": CARD, "rows": [
            {"rowNumber": 4, "date": "2026-10-21", "refNo": "R-7", "contactName": "Canary Name",
             "accountName": "Supplies", "amount": "45.10"}]},
        {"txn_type": "credit_card_credit", "context_account_id": CARD, "rows": [
            {"rowNumber": 5, "date": "2026-10-21", "refNo": "R-7", "contactName": "Canary Name",
             "accountName": "Supplies", "amount": "5.00"}]},
        {"txn_type": "deposit", "context_account_id": BANK, "rows": [
            {"rowNumber": 6, "date": "2026-10-21", "refNo": "R-7", "contactName": "Canary Name",
             "accountName": "Supplies", "amount": "999.99"}]},
        {"txn_type": "journal_entry", "rows": [
            {"rowNumber": 7, "date": "2026-10-21", "refNo": "JE-9", "accountId": RENT,
             "memo": "accrual", "debit": "10.01"},
            {"rowNumber": 8, "date": "2026-10-21", "refNo": "JE-9",
             "accountName": "Accrued liabilities", "credit": "10.01"}
        ]}
    ]});
    assert_eq!(body, expected);
    let mut nums = Vec::new();
    numbers(&body, "", &mut nums);
    assert!(
        nums.iter().all(|k| k == "rowNumber"),
        "only row numbers are JSON numbers: {nums:?}"
    );
}

#[tokio::test]
async fn validate_reads_rows_into_their_groups() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/validate"))
        .respond_with(reply(
            200,
            &format!(
                r#"{{"validCount":2,"invalidCount":2,"closingDate":null,"closingPasswordRequired":false,
                "closedPeriodRowNumbers":[],"rows":[
                {{"rowNumber":1,"groupIndex":0,"txnType":"check","status":"valid",
                  "resolvedContactId":"{VENDOR}","resolvedAccountId":"{RENT}","errors":[],"closedPeriod":false}},
                {{"rowNumber":2,"txnType":"check","status":"warning","resolvedContactId":null,
                  "resolvedAccountId":"{RENT}","errors":[],"newContact":{{"displayName":"Canary Landlord","contactType":"vendor"}}}},
                {{"rowNumber":7,"groupIndex":5,"status":"invalid","resolvedContactId":null,"resolvedAccountId":null,
                  "errors":[{{"field":"debit","message":"Debits and credits differ","code":"TXN_UNBALANCED"}}]}},
                {{"rowNumber":8,"status":"invalid","resolvedContactId":null,"resolvedAccountId":null,
                  "errors":[{{"field":"account_name","message":"Account 'Canary' not found"}},
                            {{"field":"ref_no","message":"Check number 1043 is already used","code":"CHECK_NUMBER_DUPLICATE"}}]}}
                ]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let v = s.batch().validate(&every_kind()).await.expect("validate");
    assert_eq!(v.rows.len(), 4);
    assert!(v.rows[0].ok && !v.rows[0].new_contact);
    assert_eq!(v.rows[0].contact, ContactId::parse(VENDOR).ok());
    assert_eq!(v.rows[0].account, Some(a(RENT)));
    assert!(
        v.rows[1].ok && v.rows[1].new_contact,
        "a warning still saves"
    );
    assert_eq!(v.rows[1].group, 0, "group from the row number");
    assert!(!v.rows[2].ok);
    assert_eq!(v.rows[2].group, 5);
    assert_eq!(
        (v.rows[2].problems[0].field, v.rows[2].problems[0].kind),
        (BatchField::Debit, ProblemKind::Unbalanced)
    );
    assert_eq!(v.rows[3].group, 5, "row 8 is in the journal group");
    let kinds: Vec<_> = v.rows[3]
        .problems
        .iter()
        .map(|p| (p.field, p.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            (BatchField::Account, ProblemKind::NotInLists),
            (BatchField::Number, ProblemKind::Invalid)
        ]
    );
    assert_eq!(
        v.rows[3].problems[0].message.as_str(),
        "Account 'Canary' not found"
    );
    let text = format!("{v:?}");
    assert!(!text.contains("Canary"), "{text}");
}

#[tokio::test]
async fn validate_on_an_older_engine_sends_one_section_in_its_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/validate"))
        .respond_with(reply(
            200,
            r#"{"validCount":1,"invalidCount":0,"rows":[{"rowNumber":3,"status":"valid",
            "resolvedContactId":null,"resolvedAccountId":null,"errors":[]}]}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let one = [simple(BatchKind::Expense, Some(BANK), 3, "19.99")];
    let v = s.batch().validate(&one).await.expect("validate");
    assert!(v.rows[0].ok);
    assert_eq!(
        body_of(&server).await,
        json!({"txn_type": "expense", "context_account_id": BANK, "rows": [
            {"rowNumber": 3, "date": "2026-10-21", "refNo": "R-7", "contactName": "Canary Name",
             "accountName": "Supplies", "amount": "19.99"}]})
    );
    let before = requests(&server, "POST").await.len();
    let two = [
        simple(BatchKind::Expense, Some(BANK), 1, "1.00"),
        simple(BatchKind::Deposit, Some(BANK), 2, "1.00"),
    ];
    assert_eq!(
        s.batch().validate(&two).await.expect_err("two sections"),
        ApiError::Local(LocalValidation::BatchUnavailable)
    );
    assert_eq!(
        s.batch().validate(&[checks(1)]).await.expect_err("checks"),
        ApiError::Local(LocalValidation::BatchUnavailable)
    );
    assert_eq!(
        requests(&server, "POST").await.len(),
        before,
        "nothing sent"
    );
}

#[tokio::test]
async fn malformed_batches_are_refused_before_sending() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &ALL).await;
    let bad = ApiError::Local(LocalValidation::InvalidValue);
    let mut dup = every_kind();
    dup[1].rows[0].row_number = 1;
    let mut zero = every_kind();
    zero[0].rows[0].row_number = 0;
    let mut je_amount = every_kind();
    je_amount[5].rows[0].amount = amt("1.00");
    let mut expense_debit = every_kind();
    expense_debit[1].rows[0].debit = amt("1.00");
    let mut print_later = every_kind();
    print_later[1].rows[0].print_later = true;
    let mut no_context = every_kind();
    no_context[1].context = None;
    let mut je_context = every_kind();
    je_context[5].context = Some(a(BANK));
    let mut empty_group = every_kind();
    empty_group[2].rows.clear();
    let too_many: Vec<BatchGroup> = (1..=51)
        .map(|n| simple(BatchKind::Expense, Some(BANK), n, "1.00"))
        .collect();
    for groups in [
        dup,
        zero,
        je_amount,
        expense_debit,
        print_later,
        no_context,
        je_context,
        empty_group,
        Vec::new(),
        too_many,
    ] {
        assert_eq!(s.batch().validate(&groups).await.expect_err("bad"), bad);
        assert_eq!(
            s.batch()
                .save(&groups, IdempotencyKey::new(), false)
                .await
                .expect_err("bad"),
            bad
        );
    }
    no_request_sent(&server).await;
}

fn saved_reply() -> String {
    format!(
        r#"{{"batchId":"{BATCH}","savedCount":8,"createdContacts":[{{"displayName":"Canary Landlord","id":"{VENDOR}","contactType":"vendor"}}],
        "transactions":[{{"rowNumber":1,"id":"{TXN}","txnType":"expense","txnNumber":"1043","checkNumber":1043}},
                        {{"rowNumber":7,"id":"{TXN}","txnType":"journal_entry","txnNumber":"JE-9"}},
                        {{"rowNumber":8,"id":"{TXN}","txnType":"journal_entry","txnNumber":"JE-9"}}]}}"#
    )
}

#[tokio::test]
async fn save_sends_groups_key_names_flag_and_one_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/save"))
        .respond_with(reply(201, &saved_reply()))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut groups = every_kind();
    groups[0].rows[0].closing_password = Some(Secret::new("pw-canary-123"));
    groups[4].rows[0].closing_password = Some(Secret::new("pw-canary-123"));
    let key = IdempotencyKey::new();
    let saved = s
        .batch()
        .save(&groups, key.clone(), true)
        .await
        .expect("save");
    assert_eq!(saved.batch_id, BatchId::parse(BATCH).ok());
    assert_eq!(saved.created_contacts, 1);
    let placed: Vec<_> = saved
        .saved
        .iter()
        .map(|t| (t.group, t.row_number))
        .collect();
    assert_eq!(placed, [(0, 1), (5, 7), (5, 8)]);
    assert!(
        saved
            .saved
            .iter()
            .all(|t| t.id == TransactionId::parse(TXN).expect("id"))
    );

    let post = requests(&server, "POST").await.pop().expect("post");
    assert_eq!(
        post.headers
            .get("idempotency-key")
            .and_then(|h| h.to_str().ok()),
        Some(key.as_header().as_str())
    );
    let body: Value = serde_json::from_slice(&post.body).expect("json");
    assert_eq!(body["auto_create_contacts"], json!(true));
    assert_eq!(body["closingPassword"], json!("pw-canary-123"));
    assert!(body.get("overrideConfirmed").is_none(), "NN8");
    let raw = String::from_utf8(post.body.clone()).expect("utf8");
    assert_eq!(raw.matches("pw-canary-123").count(), 1, "sent once");
    assert_eq!(body["groups"].as_array().map(Vec::len), Some(6));
    let mut nums = Vec::new();
    numbers(&body, "", &mut nums);
    assert!(nums.iter().all(|k| k == "rowNumber"), "{nums:?}");
    for text in [format!("{groups:?}"), format!("{saved:?}")] {
        for leak in ["pw-canary-123", "Canary", "1200", "canary memo"] {
            assert!(!text.contains(leak), "{text}");
        }
    }
}

#[tokio::test]
async fn validate_never_sends_the_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/validate"))
        .respond_with(reply(200, r#"{"rows":[]}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut groups = every_kind();
    groups[0].rows[0].closing_password = Some(Secret::new("pw-canary-123"));
    s.batch().validate(&groups).await.expect("validate");
    let post = requests(&server, "POST").await.pop().expect("post");
    let raw = String::from_utf8(post.body.clone()).expect("utf8");
    assert!(!raw.contains("pw-canary-123") && !raw.contains("closingPassword"));
}

#[tokio::test]
async fn save_is_gated_by_capabilities_and_passwords() {
    let groups = every_kind();
    for missing in [
        Capabilities::BATCH_ATOMIC,
        Capabilities::BATCH_EXACT,
        Capabilities::BATCH_IDEMPOTENT,
        Capabilities::BATCH_MIXED,
        Capabilities::BATCH_CHECK,
    ] {
        let server = MockServer::start().await;
        let caps: Vec<&str> = ALL.iter().copied().filter(|c| *c != missing).collect();
        let s = session_with_caps(&server, &caps).await;
        assert_eq!(
            s.batch()
                .save(&groups, IdempotencyKey::new(), false)
                .await
                .expect_err(missing),
            ApiError::Local(LocalValidation::BatchUnavailable),
            "{missing}"
        );
        no_request_sent(&server).await;
    }

    // A password without batch-closing-v1, and two different passwords.
    let server = MockServer::start().await;
    let no_closing: Vec<&str> = ALL
        .iter()
        .copied()
        .filter(|c| *c != Capabilities::BATCH_CLOSING)
        .collect();
    let s = session_with_caps(&server, &no_closing).await;
    let mut locked = every_kind();
    locked[0].rows[0].closing_password = Some(Secret::new("pw-one"));
    assert_eq!(
        s.batch()
            .save(&locked, IdempotencyKey::new(), false)
            .await
            .expect_err("no closing"),
        ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
    );
    let s = session_with_caps(&server, &ALL).await;
    locked[1].rows[0].closing_password = Some(Secret::new("pw-two"));
    assert_eq!(
        s.batch()
            .save(&locked, IdempotencyKey::new(), false)
            .await
            .expect_err("two passwords"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    no_request_sent(&server).await;
}

async fn save_error(status: u16, body: &str) -> ApiError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/save"))
        .respond_with(reply(status, body))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    s.batch()
        .save(&every_kind(), IdempotencyKey::new(), false)
        .await
        .expect_err("refused")
}

#[tokio::test]
async fn refused_rows_name_their_group_and_row() {
    let e = save_error(
        400,
        r#"{"error":{"message":"Batch has 2 invalid rows","code":"BATCH_INVALID","details":{
            "rowNumbers":[6,8],"rows":[
              {"rowNumber":6,"errors":[{"field":"account_name","message":"Account 'Canary' is inactive"}]},
              {"rowNumber":8,"errors":[{"field":"date","message":"Date is required"},
                                       {"field":"credit","message":"Bad amount","code":"VALIDATION_ERROR"}]}]}}}"#,
    )
    .await;
    let ApiError::BatchRows(rows) = e else {
        panic!("{e:?}")
    };
    let got: Vec<_> = rows
        .iter()
        .map(|r| (r.group, r.row_number, r.problem.field, r.problem.kind))
        .collect();
    assert_eq!(
        got,
        [
            (4, 6, BatchField::Account, ProblemKind::Inactive),
            (5, 8, BatchField::Date, ProblemKind::Required),
            (5, 8, BatchField::Credit, ProblemKind::Invalid),
        ]
    );

    // Unknown names without "Add new names to the list".
    let e = save_error(
        400,
        r#"{"error":{"message":"Unknown names","code":"BATCH_CONTACT_NOT_FOUND","details":{
            "rowNumbers":[2],"rows":[{"rowNumber":2,"errors":[{"field":"contact_name","message":"Contact 'Canary' not found"}]}]}}}"#,
    )
    .await;
    assert!(
        matches!(&e, ApiError::BatchRows(r) if r.len() == 1
            && r[0].group == 0 && r[0].problem.kind == ProblemKind::NotInLists
            && r[0].problem.field == BatchField::Contact),
        "{e:?}"
    );

    // A posting's own failure (409 check number taken), message prefixed.
    let e = save_error(
        409,
        r#"{"error":{"message":"Row 2: Check number 1044 is already used","code":"CHECK_NUMBER_DUPLICATE",
            "details":{"rowNumbers":[2]}}}"#,
    )
    .await;
    let ApiError::BatchRows(rows) = e else {
        panic!("{e:?}")
    };
    assert_eq!((rows[0].group, rows[0].row_number), (0, 2));
    assert_eq!(rows[0].problem.field, BatchField::Row);
    assert_eq!(rows[0].problem.kind, ProblemKind::Invalid);
    assert_eq!(
        rows[0].problem.message.as_str(),
        "Check number 1044 is already used"
    );
}

#[tokio::test]
async fn closing_idempotency_and_schema_errors_keep_their_meaning() {
    let e = save_error(
        423,
        r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED","details":{
            "rowNumbers":[1],"lockDate":"2026-09-30","canOverride":true,"closingPasswordRequired":true}}}"#,
    )
    .await;
    assert!(
        matches!(&e, ApiError::ClosedPeriod(c) if c.password_required == Some(true)),
        "{e:?}"
    );
    let e = save_error(
        403,
        r#"{"error":{"message":"wrong","code":"CLOSING_PASSWORD_INVALID","details":{"rowNumbers":[1]}}}"#,
    )
    .await;
    assert_eq!(e, ApiError::ClosingPasswordRejected);
    let e = save_error(
        429,
        r#"{"error":{"message":"slow down","code":"CLOSING_PASSWORD_RATE_LIMIT"}}"#,
    )
    .await;
    assert!(
        matches!(e, ApiError::ClosingPasswordRateLimited { .. }),
        "{e:?}"
    );
    let e = save_error(
        409,
        &format!(
            r#"{{"error":{{"message":"reused","code":"IDEMPOTENCY_KEY_REUSED","details":{{"batchId":"{BATCH}"}}}}}}"#
        ),
    )
    .await;
    assert!(is_key_reused(&e), "{e:?}");
    assert!(!is_key_reused(&ApiError::Timeout));
    // A schema error (no rows named) refuses the whole request.
    let e = save_error(
        400,
        r#"{"error":{"message":"bad","code":"VALIDATION_ERROR","details":[{"path":["groups"],"message":"Required"}]}}"#,
    )
    .await;
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    // Rows named: the rows are refused, whatever the code.
    let e = save_error(
        400,
        r#"{"error":{"message":"bad","code":"VALIDATION_ERROR","details":{"rowNumbers":[1]}}}"#,
    )
    .await;
    assert!(
        matches!(&e, ApiError::BatchRows(r) if r[0].row_number == 1),
        "{e:?}"
    );
    let e = save_error(
        500,
        r#"{"error":{"message":"Row 1: This row could not be saved","code":"BATCH_ROW_FAILED","details":{"rowNumbers":[1]}}}"#,
    )
    .await;
    assert!(
        matches!(&e, ApiError::BatchRows(r) if r[0].row_number == 1),
        "{e:?}"
    );
    // A refused group: its index, as sent.
    let e = save_error(
        403,
        r#"{"error":{"message":"Permission denied","code":"PERMISSION_DENIED","details":{"groupIndex":0,"resource":"checks"}}}"#,
    )
    .await;
    assert_eq!(
        e,
        ApiError::BatchGroup {
            group: 0,
            problem: crate::BatchGroupProblem::NotAllowed
        }
    );
}

#[tokio::test]
async fn parse_csv_sends_the_file_and_mapping_and_reads_strings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/parse-csv"))
        .respond_with(reply(
            200,
            r#"{"rows":[
              {"rowNumber":1,"date":"2026-10-01","refNo":"101","contactName":"Canary Feed","accountName":"Supplies",
               "memo":"line one\nline two","amount":"-1234.50"},
              {"rowNumber":2,"date":"2026-10-02","amount":"12.345","debit":null},
              {"rowNumber":3,"date":"","memo":null}],"count":3}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut mapping = ColumnMapping::default();
    mapping.0.insert(CsvField::Date, 0);
    mapping.0.insert(CsvField::RefNo, 1);
    mapping.0.insert(CsvField::Amount, 4);
    let file = "\u{feff}Date,No,Payee,Account,Amount\n10/01/2026,101,Canary Feed,Supplies,\"(1,234.50)\"\n";
    let parsed = s
        .batch()
        .parse_csv(BatchKind::Check, file.as_bytes(), &mapping)
        .await
        .expect("parsed");
    assert_eq!(parsed.rows.len(), 3);
    assert_eq!(parsed.rows[0].amount.as_deref(), Some("-1234.50"));
    assert_eq!(parsed.rows[0].memo.as_deref(), Some("line one\nline two"));
    assert_eq!(
        parsed.rows[1].amount.as_deref(),
        Some("12.345"),
        "not rounded"
    );
    assert_eq!(parsed.rows[1].debit, None);
    assert_eq!(parsed.rows[2].date, None);
    assert!(!format!("{parsed:?}").contains("Canary"));

    let post = requests(&server, "POST").await.pop().expect("post");
    let ctype = post
        .headers
        .get("content-type")
        .and_then(|h| h.to_str().ok())
        .expect("content type")
        .to_string();
    assert!(
        ctype.starts_with("multipart/form-data; boundary=cl-"),
        "{ctype}"
    );
    let raw = String::from_utf8(post.body.clone()).expect("utf8");
    assert!(raw.contains("name=\"txn_type\"\r\n\r\ncheck\r\n"), "{raw}");
    assert!(
        raw.contains("name=\"column_mapping\"\r\n\r\n{\"date\":0,\"ref_no\":1,\"amount\":4}\r\n"),
        "{raw}"
    );
    assert!(raw.contains("name=\"file\"; filename=\"batch.csv\"\r\nContent-Type: text/csv"));
    assert!(raw.contains(file), "the file goes byte for byte");
}

#[tokio::test]
async fn parse_csv_refuses_numbers_and_bad_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/batch/parse-csv"))
        .respond_with(reply(
            200,
            r#"{"rows":[{"rowNumber":1,"date":"2026-10-01","amount":12.35,"debit":0,"credit":0}],"count":1}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let mut mapping = ColumnMapping::default();
    mapping.0.insert(CsvField::Amount, 1);
    assert_eq!(
        s.batch()
            .parse_csv(
                BatchKind::Check,
                b"Date,Amount\n2026-10-01,12.35\n",
                &mapping
            )
            .await
            .expect_err("numbers"),
        ApiError::InexactMoney {
            what: "parse-csv amounts"
        }
    );
    let post = requests(&server, "POST").await.pop().expect("post");
    let raw = String::from_utf8(post.body.clone()).expect("utf8");
    assert!(
        raw.contains("name=\"txn_type\"\r\n\r\nexpense\r\n"),
        "no check rows on this engine"
    );
    let before = requests(&server, "POST").await.len();
    assert_eq!(
        s.batch()
            .parse_csv(BatchKind::Expense, b"", &mapping)
            .await
            .expect_err("empty"),
        ApiError::Local(LocalValidation::BatchFileSize)
    );
    let big = vec![b'a'; PARSE_CSV_MAX_BYTES + 1];
    assert_eq!(
        s.batch()
            .parse_csv(BatchKind::Expense, &big, &mapping)
            .await
            .expect_err("big"),
        ApiError::Local(LocalValidation::BatchFileSize)
    );
    assert_eq!(
        s.batch()
            .parse_csv(BatchKind::Expense, b"a,b\n1,2\n", &ColumnMapping::default())
            .await
            .expect_err("no mapping"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
    assert_eq!(
        requests(&server, "POST").await.len(),
        before,
        "nothing sent"
    );
    assert!(matches!(
        parsed_from_json(br#"{"rows":[{"rowNumber":1,"amount":true}]}"#),
        Err(ApiError::Decode { .. })
    ));
    assert!(matches!(
        parsed_from_json(br#"{"rows":[{"amount":"1.00"}]}"#),
        Err(ApiError::Decode { .. })
    ));
}

#[tokio::test]
async fn suggest_account_asks_by_contact() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/batch/suggest-account"))
        .respond_with(reply(
            200,
            &format!(r#"{{"accountId":"{RENT}","source":"default"}}"#),
        ))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/batch/suggest-account"))
        .respond_with(reply(200, r#"{"accountId":null,"source":null}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[]).await;
    let vendor = ContactId::parse(VENDOR).expect("vendor");
    assert_eq!(
        s.batch().suggest_account(vendor).await.expect("suggest"),
        Some(SuggestedAccount {
            account: a(RENT),
            source: SuggestSource::Default
        })
    );
    assert_eq!(s.batch().suggest_account(vendor).await.expect("none"), None);
    let gets: Vec<_> = requests(&server, "GET")
        .await
        .into_iter()
        .filter(|r| r.url.path() == "/api/v1/batch/suggest-account")
        .collect();
    assert_eq!(query(&gets[0]), format!("contact_id={VENDOR}"));
}

#[test]
fn row_prefixes_are_stripped_only_when_they_are_row_numbers() {
    assert_eq!(strip_row_prefix("Row 12: Bad"), "Bad");
    assert_eq!(strip_row_prefix("Row x: Bad"), "Row x: Bad");
    assert_eq!(strip_row_prefix("Rows 1: Bad"), "Rows 1: Bad");
    assert_eq!(strip_row_prefix("Plain"), "Plain");
    assert_eq!(
        rows_from_details(&json!({"rowNumbers": []}), "x", None),
        None
    );
    assert_eq!(rows_from_details(&json!({}), "x", None), None);
}
