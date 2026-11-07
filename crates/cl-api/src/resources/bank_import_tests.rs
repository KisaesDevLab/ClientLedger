//! Unit tests of the bank import resource (P25, U-06), against a mock
//! engine. The live engine's behaviour is covered by
//! `tests/contract/contract_bank_import.rs`.

use super::*;
use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const ACCOUNT: &str = "5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const BATCH: &str = "6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const ITEM: &str = "7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03";
const SHA: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";

fn d(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn row(line: u32, amount: &str) -> BankImportRow {
    BankImportRow {
        line,
        date: day("2026-11-02"),
        amount: d(amount),
        description: "CHECK".into(),
        memo: None,
        check_number: Some(4412),
        fitid: Some(format!("F{line}")),
        include: None,
        changed_action: None,
    }
}

fn request() -> BankImportRequest {
    BankImportRequest {
        account: ACCOUNT.parse().expect("account"),
        format: BankFileFormat::Qfx,
        file_name: "operating_2026-11-03.qfx".into(),
        source_file_sha256: SHA.into(),
        section_index: 0,
        section_start: Some(day("2026-10-28")),
        section_end: Some(day("2026-11-03")),
        amounts_flipped: false,
        csv_mapping: None,
        relink_identity: false,
        bank_identity: Some(BankIdentity {
            bank_id: Some("123456789".into()),
            acct_id: "000012342208".into(),
        }),
        file_ending_balance: Some(d("92431.07")),
        file_ending_date: Some(day("2026-11-03")),
        rows: vec![row(12, "925"), row(13, "-8450.5")],
    }
}

const PREVIEW: &str = r#"{"insertedItemIds":[],"rows":[
  {"line":12,"status":"new","closedPeriod":false,"inserted":false},
  {"line":13,"status":"duplicate","reason":"same_bank_id","existingItemId":"7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03",
   "existingImportedAt":"2026-10-29T14:03:00.000Z","existingPending":true,"closedPeriod":true,"inserted":false}],
  "counts":{"inFile":2,"new":1,"duplicate":1,"possibleDuplicate":0,"changed":0,"closedPeriod":1,"inserted":0},
  "insertedTotal":"0.00",
  "identityLinkedElsewhere":{"accountName":"Savings","mask":"2208"},
  "balanceCheck":{"fileBalance":"92431.07","fileDate":"2026-11-03","booksAtDate":"93356.0700","pendingTotal":"0.00",
   "newTotal":"925.00","likelyInBooks":{"count":1,"total":"925.00"},"projected":"92431.07","difference":"0.00"}}"#;

const COMMIT: &str = r#"{"batchId":"6b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","insertedItemIds":["7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03"],
  "postProcessing":"scheduled","rows":[{"line":12,"status":"new","closedPeriod":false,"inserted":true}],
  "counts":{"inFile":1,"new":1,"duplicate":0,"possibleDuplicate":0,"changed":0,"closedPeriod":0,"inserted":1},
  "insertedTotal":"925.00"}"#;

#[test]
fn wire_decimals_have_two_places_and_twelve_digits() {
    assert_eq!(wire_decimal(d("925")).expect("ok"), "925.00");
    assert_eq!(wire_decimal(d("-45.1000")).expect("ok"), "-45.10");
    assert_eq!(wire_decimal(d("0.01")).expect("ok"), "0.01");
    assert_eq!(
        wire_decimal(d("-999999999999.99")).expect("ok"),
        "-999999999999.99"
    );
    assert_eq!(
        wire_decimal(d("12.345")),
        Err(LocalValidation::NotWholeCents)
    );
    assert_eq!(
        wire_decimal(d("1000000000000")),
        Err(LocalValidation::InvalidValue)
    );
}

#[test]
fn the_body_has_the_contract_fields() {
    let b = request().body(true).expect("body");
    assert_eq!(b["accountId"], ACCOUNT);
    assert_eq!(b["format"], "qfx");
    assert_eq!(b["dryRun"], true);
    assert_eq!(b["amountsFlipped"], false);
    assert_eq!(b["fileEndingBalance"], "92431.07");
    assert_eq!(b["bankIdentity"]["acctId"], "000012342208");
    assert_eq!(b["bankIdentity"]["bankId"], "123456789");
    assert!(b["bankIdentity"].get("accountKeyHash").is_none());
    assert!(!format!("{:?}", request()).contains("2208"));
    assert_eq!(b["rows"][0]["amount"], "925.00");
    assert_eq!(b["rows"][0]["checkNumber"], 4412);
    assert_eq!(b["rows"][1]["amount"], "-8450.50");
    assert!(b["rows"][0].get("include").is_none());
    assert!(b.get("relinkIdentity").is_none());
    assert_eq!(request().body(false).expect("body")["dryRun"], false);
}

#[test]
fn the_body_refuses_what_the_engine_would() {
    let mut r = request();
    r.rows[0].amount = Decimal::ZERO;
    assert_eq!(r.body(true), Err(LocalValidation::NonPositiveAmount));
    let mut r = request();
    r.rows[0].amount = d("1.005");
    assert_eq!(r.body(true), Err(LocalValidation::NotWholeCents));
    let mut r = request();
    r.rows[0].description = "x".repeat(MAX_DESCRIPTION + 1);
    assert_eq!(r.body(true), Err(LocalValidation::InvalidValue));
    let mut r = request();
    r.file_name = "C:\\Users\\owner\\x.qfx".into();
    assert_eq!(r.body(true), Err(LocalValidation::InvalidValue));
    let mut r = request();
    r.rows = (1..=u32::try_from(MAX_ROWS).expect("n") + 1)
        .map(|l| row(l, "1"))
        .collect();
    assert_eq!(r.body(true), Err(LocalValidation::TooManyLines));
    let mut r = request();
    r.rows[0].memo = Some("m".repeat(600));
    r.rows = std::iter::repeat_n(r.rows[0].clone(), MAX_ROWS).collect();
    assert_eq!(r.body_bytes(true), Err(LocalValidation::BatchFileSize));
}

#[test]
fn debug_output_has_no_owner_data() {
    let r = request();
    let text = format!("{r:?} {:?} {:?}", r.rows[0], r.bank_identity);
    for secret in ["2208", "CHECK", "925", "operating", SHA] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
    let o = outcome_from(PREVIEW.as_bytes(), false).expect("outcome");
    let text = format!("{o:?} {:?}", o.balance_check);
    for secret in ["92431", "Savings", "2208"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

#[test]
fn a_preview_reads_rows_counts_and_the_balance_check() {
    let o = outcome_from(PREVIEW.as_bytes(), false).expect("outcome");
    assert_eq!(o.batch, None);
    assert_eq!(o.counts.in_file, 2);
    assert_eq!(o.counts.closed_period, 1);
    assert_eq!(o.rows[1].status, ImportRowStatus::Duplicate);
    assert_eq!(o.rows[1].reason, Some(ImportRowReason::SameBankId));
    assert_eq!(o.rows[1].existing_pending, Some(true));
    assert!(o.rows[1].existing_imported_at.is_some());
    assert!(o.rows[1].closed_period);
    let b = o.balance_check.expect("balance");
    assert_eq!(b.difference, Money::ZERO);
    assert_eq!(b.books_at_date, Money::parse_wire("93356.07").expect("m"));
    assert_eq!(b.likely_in_books_count, 1);
    assert_eq!(
        o.identity_linked_elsewhere.expect("linked").account_name,
        "Savings"
    );
}

#[test]
fn a_number_where_an_amount_belongs_is_unreadable() {
    let body = PREVIEW.replace(r#""insertedTotal":"0.00""#, r#""insertedTotal":0"#);
    assert!(matches!(
        outcome_from(body.as_bytes(), false),
        Err(ApiError::Decode { .. })
    ));
}

#[tokio::test]
async fn previews_carry_no_key_and_commits_carry_the_given_key() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/banking/feed/import-batch"))
        .respond_with(json(200, COMMIT))
        .mount(&server)
        .await;
    let s = session_with_caps(
        &server,
        &[Capabilities::BANK_IMPORT_BATCH, Capabilities::IDEMPOTENCY],
    )
    .await;
    s.bank_import().preview(&request()).await.expect("preview");
    let key = IdempotencyKey::new();
    let header = key.as_header();
    let o = s
        .bank_import()
        .commit(&request(), key)
        .await
        .expect("commit");
    assert_eq!(o.batch, Some(ImportBatchId::parse(BATCH).expect("id")));
    assert_eq!(o.post_processing, Some(ImportPostProcessing::Scheduled));
    assert_eq!(o.inserted_total, Money::parse_wire("925.00").expect("m"));
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 2);
    assert!(
        posts[0].headers.get("idempotency-key").is_none(),
        "a dry run must not carry a key"
    );
    let sent: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(sent["dryRun"], true);
    assert_eq!(
        posts[1]
            .headers
            .get("idempotency-key")
            .and_then(|v| v.to_str().ok()),
        Some(header.as_str())
    );
    let sent: Value = serde_json::from_slice(&posts[1].body).expect("json");
    assert_eq!(sent["dryRun"], false);
}

#[tokio::test]
async fn a_replayed_commit_says_so() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(200, COMMIT).insert_header("Idempotent-Replayed", "true"))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BANK_IMPORT_BATCH]).await;
    let o = s
        .bank_import()
        .commit(&request(), IdempotencyKey::new())
        .await
        .expect("commit");
    assert!(o.replayed);
}

#[tokio::test]
async fn nothing_is_sent_without_the_capability() {
    let server = MockServer::start().await;
    let s = company_session(&server);
    assert!(matches!(
        s.bank_import().preview(&request()).await,
        Err(ApiError::Local(LocalValidation::Unsupported))
    ));
    assert!(requests(&server, "POST").await.is_empty());
}

#[tokio::test]
async fn refusals_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(
            422,
            r#"{"error":{"message":"Line 13: bad amount","code":"invalid_row","details":{"line":13,"field":"amount"}}}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(json(
            409,
            r#"{"error":{"message":"Some rows of this import were already used.","code":"rows_already_used","details":{"lines":[3,9]}}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BANK_IMPORT_BATCH]).await;
    let e = s.bank_import().preview(&request()).await.expect_err("422");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::InvalidRow {
            line: Some(13),
            field: Some("amount".into())
        })
    );
    let e = s
        .bank_import()
        .undo(
            ImportBatchId::parse(BATCH).expect("id"),
            false,
            IdempotencyKey::new(),
        )
        .await
        .expect_err("409");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::RowsAlreadyUsed(vec![3, 9]))
    );
    let deletes = requests(&server, "DELETE").await;
    assert_eq!(
        deletes[0].url.path(),
        format!("/api/v1/banking/feed/import-batches/{BATCH}")
    );
    assert_eq!(query(&deletes[0]), "");
}

#[tokio::test]
async fn a_commit_linked_elsewhere_names_the_account() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(
            409,
            r#"{"error":{"message":"linked","code":"identity_linked_elsewhere","details":{"accountName":"Savings","mask":"2208"}}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BANK_IMPORT_BATCH]).await;
    let e = s
        .bank_import()
        .commit(&request(), IdempotencyKey::new())
        .await
        .expect_err("409");
    let Some(BankImportRefusal::LinkedElsewhere(l)) = bank_import_refusal(&e) else {
        panic!("{e:?}");
    };
    assert_eq!(
        (l.account_name.as_str(), l.mask.as_str()),
        ("Savings", "2208")
    );
}

#[tokio::test]
async fn batches_undo_and_lookup_read_their_answers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/banking/feed/import-batches"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"batches":[{{"id":"{BATCH}","accountId":"{ACCOUNT}","format":"csv","amountsFlipped":true,
                "csvMapping":{{"date":0}},"fileEndingBalance":"10.00","fileEndingDate":"2026-11-03",
                "counts":{{"inserted":4}},"postProcessing":"done","undoneAt":null,"createdAt":"2026-11-03T10:00:00.000Z"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/banking/connections/lookup"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"matches":[{{"connectionId":"{ITEM}","accountId":"{ACCOUNT}","accountName":"Operating","mask":"2208","via":"hash"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .respond_with(json(
            200,
            &format!(
                r#"{{"batchId":"{BATCH}","removed":3,"kept":1,"undoneAt":"2026-11-03T10:00:00.000Z"}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &[Capabilities::BANK_IMPORT_BATCH]).await;
    let account: AccountId = ACCOUNT.parse().expect("account");
    let listed = s
        .bank_import()
        .batches(Some(account), Some(BankFileFormat::Csv))
        .await
        .expect("batches");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].amounts_flipped);
    assert_eq!(listed[0].csv_mapping, Some(json!({"date": 0})));
    assert_eq!(listed[0].inserted, 4);
    assert_eq!(listed[0].post_processing, Some(ImportPostProcessing::Done));
    let gets = requests(&server, "GET").await;
    let sent = query(gets.last().expect("get"));
    assert!(
        sent.contains("format=csv") && sent.contains("accountId="),
        "{sent}"
    );
    let found = s
        .bank_import()
        .lookup(&BankLookupBy::Account(BankIdentity {
            bank_id: None,
            acct_id: "000012342208".into(),
        }))
        .await
        .expect("lookup");
    assert_eq!(found[0].via, BankLookupVia::Hash);
    assert_eq!(found[0].account, account);
    let posts = requests(&server, "POST").await;
    let sent = posts.last().expect("post");
    assert_eq!(query(sent), "", "the number never goes in the URL");
    let body: serde_json::Value = serde_json::from_slice(&sent.body).expect("json");
    assert_eq!(body, json!({"acctId": "000012342208"}));
    assert!(sent.headers.get("idempotency-key").is_none());
    assert!(matches!(
        s.bank_import()
            .lookup(&BankLookupBy::Account(BankIdentity {
                bank_id: None,
                acct_id: "  ".into(),
            }))
            .await,
        Err(ApiError::Local(LocalValidation::InvalidValue))
    ));
    let undone = s
        .bank_import()
        .undo(
            ImportBatchId::parse(BATCH).expect("id"),
            true,
            IdempotencyKey::new(),
        )
        .await
        .expect("undo");
    assert_eq!((undone.removed, undone.kept), (3, 1));
    let deletes = requests(&server, "DELETE").await;
    assert_eq!(query(&deletes[0]), "pendingOnly=true");
}
