//! Unit tests of the import resource (P31), against a mock engine. The live
//! engine is covered by `tests/contract/contract_imports.rs` (pending until
//! the pin has the import capabilities).

use super::*;
use crate::resources::testkit::{json, query, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SESSION: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b01";
const SESSION2: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b02";
const BATCH: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b03";
const ACCOUNT: &str = "5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b04";

const ALL: [&str; 8] = [
    Capabilities::IMPORTS_AMOUNTS,
    Capabilities::IMPORTS_BATCH,
    Capabilities::QBD_OPEN_ITEMS,
    Capabilities::QBD_MAPPING,
    Capabilities::QBD_DATES,
    Capabilities::IMPORTS_TIEOUT,
    Capabilities::IMPORTS_TRANSACTIONS,
    Capabilities::CLOSING_PASSWORD,
];

fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn bid() -> ImportBatchId {
    BATCH.parse().expect("batch")
}

fn body_of(r: &wiremock::Request) -> Value {
    serde_json::from_slice(&r.body).expect("json body")
}

fn tb_upload() -> UploadRequest {
    UploadRequest {
        kind: ImportKind::TrialBalance,
        source: SourceSystem::QuickBooksDesktop,
        role: FileRole::Reference,
        bytes: b"Debit,Credit\n".to_vec(),
        file_name: "Avery Testworker books TB.CSV".into(),
        contact_kind: None,
        tb_column: None,
        tb_report_date: None,
    }
}

const UPLOADED: &str = r#"{"session":{"id":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b01","kind":"trial_balance",
 "sourceSystem":"quickbooks_desktop","role":"reference","status":"uploaded","rowCount":15,"errorCount":1,
 "reportDate":"2026-10-27","batchId":null,"originalFilename":"import.csv"},
 "preview":{"totalRows":15,"errorCount":1,"sampleRows":[{"accountName":"Riverside Café"}],
  "reportDate":"2026-10-27","totalDebit":"53785.4600","totalCredit":"53785.46"},
 "validationErrors":[{"rowNumber":7,"field":"Debit","code":"IMPORT_AMOUNT_INVALID","message":"Could not read \"1.234,56\" for Riverside Café"}]}"#;

#[tokio::test]
async fn nothing_is_sent_without_the_capabilities_a_file_needs() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::IMPORTS_AMOUNTS]).await;
    // A reference file needs imports-batch-v1 (role arrives with E1).
    let e = s.imports().upload(&tb_upload()).await.expect_err("refused");
    assert!(matches!(e, ApiError::Local(LocalValidation::Unsupported)));
    let mut open = tb_upload();
    open.kind = ImportKind::OpenInvoices;
    open.role = FileRole::Post;
    let e = s.imports().upload(&open).await.expect_err("refused");
    assert!(matches!(e, ApiError::Local(LocalValidation::Unsupported)));
    assert!(requests(&server, "POST").await.is_empty());
    assert!(!Flow::QbdConversion.supported(&s.client().capabilities()));
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &ALL).await;
    for f in [Flow::QbdConversion, Flow::OtherSystem, Flow::Transactions] {
        assert!(f.supported(&s.client().capabilities()), "{f:?}");
    }
}

#[tokio::test]
async fn upload_sends_a_neutral_name_and_never_suspense_and_reads_codes_only() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/imports/upload"))
        .respond_with(json(201, UPLOADED))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let out = s.imports().upload(&tb_upload()).await.expect("upload");
    assert_eq!(out.session.id, SESSION.parse().expect("id"));
    assert_eq!(out.session.kind, ImportKind::TrialBalance);
    assert_eq!(out.session.role, Some(FileRole::Reference));
    assert_eq!(out.session.report_date, Some(day("2026-10-27")));
    assert_eq!(out.preview.total_debit, Some(m("53785.46")));
    assert_eq!(out.preview.total_credit, out.preview.total_debit);
    assert_eq!(
        out.problems,
        vec![RowProblem {
            row_number: 7,
            field: Some("Debit".into()),
            code: "IMPORT_AMOUNT_INVALID".into()
        }]
    );
    assert!(!out.is_clean());
    let dbg = format!("{out:?}");
    assert!(
        !dbg.contains("Café") && !dbg.contains("53785"),
        "Debug hides names and amounts: {dbg}"
    );
    let posts = requests(&server, "POST").await;
    let sent = String::from_utf8_lossy(&posts[0].body).to_string();
    assert!(sent.contains("name=\"kind\"\r\n\r\ntrial_balance\r\n"));
    assert!(sent.contains("name=\"sourceSystem\"\r\n\r\nquickbooks_desktop\r\n"));
    assert!(sent.contains("name=\"role\"\r\n\r\nreference\r\n"));
    assert!(sent.contains(r#""unmappedAccountsToSuspense":false"#));
    assert!(sent.contains("filename=\"import.csv\""));
    assert!(!sent.contains("Avery"), "the owner's file name never goes");
    assert!(
        posts[0]
            .headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("multipart/form-data; boundary="))
    );
}

#[test]
fn upload_checks_refuse_before_sending() {
    let mut r = tb_upload();
    assert_eq!(r.check(), Ok("csv".into()));
    r.file_name = "tb.pdf".into();
    assert_eq!(r.check(), Err(LocalValidation::AttachmentType));
    let mut r = tb_upload();
    r.bytes = vec![b'x'; MAX_UPLOAD_BYTES + 1];
    assert_eq!(r.check(), Err(LocalValidation::AttachmentTooLarge));
    let mut r = tb_upload();
    r.bytes.clear();
    assert_eq!(r.check(), Err(LocalValidation::InvalidValue));
    let mut r = tb_upload();
    r.kind = ImportKind::Contacts;
    assert_eq!(
        r.check(),
        Err(LocalValidation::InvalidValue),
        "needs a list"
    );
    r.contact_kind = Some(ContactKind::Other);
    assert!(r.check().is_ok());
    assert_eq!(r.options()["contactKind"], "other");
    let mut r = tb_upload();
    r.kind = ImportKind::GlTransactions;
    r.tb_report_date = Some(day("2025-12-31"));
    assert_eq!(r.check(), Err(LocalValidation::InvalidValue));
    let mut r = tb_upload();
    r.source = SourceSystem::AccountingPower;
    r.tb_column = Some(TbColumn::Adjusted);
    r.tb_report_date = Some(day("2025-12-31"));
    let o = r.options();
    assert_eq!(o["tbColumn"], "adjusted");
    assert_eq!(o["tbReportDate"], "2025-12-31");
    assert_eq!(o["unmappedAccountsToSuspense"], false);
}

#[test]
fn a_json_number_total_is_refused_never_rounded() {
    let body = UPLOADED.replace(r#""totalDebit":"53785.4600""#, r#""totalDebit":53785.46"#);
    assert!(matches!(
        upload_from(body.as_bytes()),
        Err(ApiError::InexactMoney { .. })
    ));
    let unknown = UPLOADED.replace(r#""kind":"trial_balance""#, r#""kind":"payroll""#);
    assert!(matches!(
        upload_from(unknown.as_bytes()),
        Err(ApiError::Decode { .. })
    ));
}

/// E2b (assumed shape): every account a file names, with how it matched;
/// an unknown `match` is skipped, never counted as matched.
#[test]
fn account_matches_are_read_with_their_keys() {
    let body = UPLOADED.replace(
        r#""preview":{"#,
        r#""preview":{"accountMatches":[
            {"sourceKey":"1000","number":"1000","name":"Checking","match":"existing","accountId":"7b3f6f0e-1c2d-4e5f-8a9b-0c1d2e3f4a5b","matchedBy":"number","rows":14},
            {"sourceKey":"1200","number":"1200","name":"Accounts Receivable","match":"create","rows":1,"systemRole":"accounts_receivable"},
            {"number":"6100","name":"Feed","match":"create"},
            {"name":"Mystery","match":"unmatched"},
            {"name":"Odd","match":"later"}],"#,
    );
    let u = upload_from(body.as_bytes()).expect("read");
    let m = &u.preview.account_matches;
    assert_eq!(m.len(), 4, "{m:?}");
    assert_eq!(m[0].status, MatchStatus::Existing);
    assert!(m[0].account.is_some());
    assert_eq!(m[0].matched_by, Some(MatchedBy::Number));
    assert_eq!(m[0].rows, 14);
    assert_eq!(m[1].system_role, Some(SystemRole::AccountsReceivable));
    assert_eq!(m[1].matched_by, None);
    assert_eq!(m[2].source_key, "6100", "the number when no key");
    assert_eq!(m[2].rows, 0);
    assert_eq!(m[3].source_key, "Mystery", "else the name");
    assert_eq!(m[3].status, MatchStatus::Unmatched);
    let dbg = format!("{m:?}");
    assert!(!dbg.contains("Mystery") && !dbg.contains("Feed"), "{dbg}");
    let none = upload_from(UPLOADED.as_bytes()).expect("read");
    assert_eq!(none.preview.account_matches, Vec::new(), "before E2b");
}

#[test]
fn wire_names_round_trip() {
    for k in ImportKind::ALL {
        assert_eq!(ImportKind::from_wire(k.as_wire()), Some(*k));
    }
    for s in SourceSystem::ALL {
        assert_eq!(SourceSystem::from_wire(s.as_wire()), Some(*s));
    }
    for s in BatchStatus::ALL {
        assert_eq!(BatchStatus::from_wire(s.as_wire()), Some(*s));
    }
    assert!(BatchStatus::Committing.is_running() && BatchStatus::Undoing.is_running());
    assert!(!BatchStatus::Failed.is_running());
    assert_eq!(
        ImportKind::OpenInvoices.capability(),
        Some(Capabilities::QBD_OPEN_ITEMS)
    );
    assert_eq!(ImportKind::Coa.capability(), None);
}

const BATCH_JSON: &str = r#"{"id":"4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b03","sourceSystem":"quickbooks_desktop",
 "kind":"conversion","status":"draft",
 "sessions":[{"id":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b01","kind":"contacts","sourceSystem":"quickbooks_desktop","role":"post","status":"uploaded","originalFilename":"Riverside Cafe.csv","rowCount":4,"errorCount":0,"reportDate":null,"commitResult":null},
  {"id":"3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b02","kind":"trial_balance","sourceSystem":"quickbooks_desktop","role":"reference","status":"uploaded","originalFilename":"tb.csv","rowCount":30,"errorCount":0,"reportDate":"2026-10-27","commitResult":null}],
 "dateCheck":{"conversionDate":"2026-10-27","historyFrom":"2024-01-01","historySkipped":12,"openingBalanceDate":"2023-12-31","firstJournalDate":"2024-01-03",
  "warnings":[{"code":"IMPORT_HISTORY_GAP","message":"Riverside Cafe","openingBalanceDate":"2023-12-31","firstJournalDate":"2024-01-03","days":2},{"code":"IMPORT_LATER","message":"x"}]},
 "backupRef":null,"companyWasEmpty":true,"conversionDate":"2026-10-27","historyFrom":"2024-01-01",
 "verifiedAt":null,"verifiedBy":null,"spotCheck":[{"key":"bank:1000","label":"Checking balance","done":true,"note":""}],
 "createdAt":"2026-11-18T10:00:00.000Z"}"#;

#[tokio::test]
async fn create_batch_sends_sessions_and_options() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/imports/batches"))
        .respond_with(json(201, &format!(r#"{{"batch":{BATCH_JSON}}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let mut options = BatchOptions {
        fiscal_year_start_month: Some(1),
        history_from: Some(day("2024-01-01")),
        require_empty_company: true,
        ..BatchOptions::default()
    };
    options.conversion_date = Some(day("2026-10-27"));
    options.account_map.insert(
        "1200".into(),
        AccountTarget::Account(ACCOUNT.parse().expect("account")),
    );
    options
        .account_map
        .insert("Repairs".into(), AccountTarget::Create);
    options.class_map.insert(
        "Enterprise:Commercial".into(),
        ClassTarget {
            group: Some("Enterprise".into()),
            tag: "Commercial".into(),
        },
    );
    let new = NewBatch {
        source: SourceSystem::QuickBooksDesktop,
        kind: BatchKind::Conversion,
        sessions: vec![SESSION.parse().expect("id"), SESSION2.parse().expect("id")],
        options,
    };
    let b = s.imports().create_batch(&new).await.expect("batch");
    assert_eq!(b.id, bid());
    assert_eq!(b.status, BatchStatus::Draft);
    assert_eq!(b.sessions.len(), 2);
    assert_eq!(b.files[1].role, Some(FileRole::Reference));
    assert_eq!(b.files[1].report_date, Some(day("2026-10-27")));
    assert_eq!(b.files[0].result, None);
    assert_eq!(b.history_from, Some(day("2024-01-01")));
    let dc = b.date_check.clone().expect("date check");
    assert_eq!(dc.history_skipped, 12);
    assert_eq!(dc.opening_balance_date, Some(day("2023-12-31")));
    assert_eq!(dc.warnings.len(), 1, "an unknown warning is left out");
    assert_eq!(dc.warnings[0].code, DateWarningCode::HistoryGap);
    assert_eq!(dc.warnings[0].days, Some(2));
    assert!(
        !format!("{b:?}").contains("Riverside"),
        "no file names or messages"
    );
    assert_eq!(b.spot_check[0].key, "bank:1000");
    assert!(b.spot_check[0].done);
    assert!(!b.is_verified());
    let sent = body_of(&requests(&server, "POST").await[0]);
    assert_eq!(sent["sourceSystem"], "quickbooks_desktop");
    assert_eq!(sent["kind"], "conversion");
    assert_eq!(sent["sessionIds"][1], SESSION2);
    assert_eq!(sent["options"]["fiscalYearStartMonth"], 1);
    assert_eq!(sent["options"]["historyFrom"], "2024-01-01");
    assert_eq!(sent["options"]["requireEmptyCompany"], true);
    assert_eq!(sent["options"]["accountMap"]["1200"], ACCOUNT);
    assert_eq!(sent["options"]["accountMap"]["Repairs"], "create");
    assert_eq!(sent["options"]["conversionDate"], "2026-10-27");
    assert_eq!(
        sent["options"]["classMap"]["Enterprise:Commercial"]["group"],
        "Enterprise"
    );
    assert_eq!(
        sent["options"]["classMap"]["Enterprise:Commercial"]["tag"],
        "Commercial"
    );
    // A repeated file, no files, or a bad month never go.
    let mut twice = new.clone();
    twice.sessions.push(SESSION.parse().expect("id"));
    let mut none = new.clone();
    none.sessions.clear();
    let mut month = new.clone();
    month.options.fiscal_year_start_month = Some(13);
    for bad in [twice, none, month] {
        assert!(matches!(
            s.imports().create_batch(&bad).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
    }
    assert_eq!(requests(&server, "POST").await.len(), 1);
    assert!(!format!("{:?}", new.options).contains("Commercial"));
}

#[tokio::test]
async fn commit_is_keyed_sent_once_and_needs_a_backup() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/commit")))
        .respond_with(json(202, &format!(r#"{{"batchId":"{BATCH}"}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let blank = CommitRequest {
        backup_ref: "  ".into(),
        ..CommitRequest::default()
    };
    assert!(matches!(
        s.imports()
            .commit(bid(), &blank, IdempotencyKey::new())
            .await,
        Err(ApiError::Local(LocalValidation::BackupRequired))
    ));
    assert!(requests(&server, "POST").await.is_empty());
    let req = CommitRequest {
        backup_ref: "before-import-2026-11-18.clbackup".into(),
        ..CommitRequest::default()
    };
    let out = s
        .imports()
        .commit(bid(), &req, IdempotencyKey::new())
        .await
        .expect("accepted");
    assert_eq!(
        out,
        JobAccepted {
            batch: bid(),
            replayed: false
        }
    );
    let posts = requests(&server, "POST").await;
    let sent = body_of(&posts[0]);
    assert_eq!(sent["backupRef"], "before-import-2026-11-18.clbackup");
    assert!(
        sent.get("confirmClosedPeriod").is_none(),
        "never sent unless the owner confirmed"
    );
    assert!(sent.get("closingPassword").is_none());
    assert!(posts[0].headers.get("idempotency-key").is_some());
    let confirmed = CommitRequest {
        confirm_closed_period: true,
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
        ..req
    };
    s.imports()
        .commit(bid(), &confirmed, IdempotencyKey::new())
        .await
        .expect("accepted");
    let sent = body_of(&requests(&server, "POST").await[1]);
    assert_eq!(sent["confirmClosedPeriod"], true);
    assert_eq!(sent["closingPassword"], "pw");
}

#[tokio::test]
async fn commit_and_undo_are_sent_once_even_on_a_gateway_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/(commit|undo)$"))
        .respond_with(json(503, r#"{"error":{"message":"busy"}}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let req = CommitRequest {
        backup_ref: "b.clbackup".into(),
        ..CommitRequest::default()
    };
    let _ = s
        .imports()
        .commit(bid(), &req, IdempotencyKey::new())
        .await
        .expect_err("busy");
    let _ = s
        .imports()
        .undo(bid(), "b.clbackup", None, IdempotencyKey::new())
        .await
        .expect_err("busy");
    assert_eq!(
        requests(&server, "POST").await.len(),
        2,
        "one each, never retried"
    );
}

#[tokio::test]
async fn undo_needs_a_backup_and_carries_the_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/undo")))
        .respond_with(json(202, &format!(r#"{{"batchId":"{BATCH}"}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    assert!(matches!(
        s.imports()
            .undo(bid(), "", None, IdempotencyKey::new())
            .await,
        Err(ApiError::Local(LocalValidation::BackupRequired))
    ));
    s.imports()
        .undo(
            bid(),
            "b.clbackup",
            Some(ClosedPeriodAuthorization::with_password(Secret::new("pw"))),
            IdempotencyKey::new(),
        )
        .await
        .expect("accepted");
    let sent = body_of(&requests(&server, "POST").await[0]);
    assert_eq!(sent["backupRef"], "b.clbackup");
    assert_eq!(sent["closingPassword"], "pw");
    assert!(sent.get("confirmClosedPeriod").is_none());
}

/// The engine's refusal of a commit as the client maps it.
async fn refusal(status: u16, code: &str, details: &str) -> ApiError {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/commit$"))
        .respond_with(json(
            status,
            &format!(
                r#"{{"error":{{"message":"Riverside Café","code":"{code}","details":{details}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let req = CommitRequest {
        backup_ref: "b.clbackup".into(),
        ..CommitRequest::default()
    };
    s.imports()
        .commit(bid(), &req, IdempotencyKey::new())
        .await
        .expect_err("refused")
}

#[tokio::test]
async fn refusals_keep_codes_and_ids_never_messages() {
    let e = refusal(
        409,
        "IMPORT_ALREADY_COMMITTED",
        &format!(
            r#"{{"batchId":"{BATCH}","sessionId":"{SESSION}","committedAt":"2026-11-01T09:00:00Z"}}"#
        ),
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::AlreadyCommitted {
            batch: Some(bid()),
            session: Some(SESSION.parse().expect("id")),
            committed_at: Some("2026-11-01T09:00:00Z".into()),
        })
    );
    let e = refusal(
        409,
        "IMPORT_PERIOD_OVERLAP",
        r#"{"from":"2025-12-01","to":"2026-01-31"}"#,
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::PeriodOverlap {
            batch: None,
            from: Some(day("2025-12-01")),
            to: Some(day("2026-01-31")),
        })
    );
    let e = refusal(
        409,
        "IMPORT_UNDO_BLOCKED",
        r#"{"blockers":[{"kind":"payment","id":"p-1","reason":"LIVE_APPLICATION"},{"kind":"reconciliation"}]}"#,
    )
    .await;
    let Some(ImportRefusal::UndoBlocked { blockers }) = import_refusal(&e) else {
        panic!("undo blocked: {e:?}")
    };
    assert_eq!(blockers.len(), 2);
    assert_eq!(blockers[0].kind, "payment");
    assert_eq!(blockers[0].reason.as_deref(), Some("LIVE_APPLICATION"));
    assert_eq!(blockers[1].id, None);
    let e = refusal(
        409,
        "TIEOUT_NOT_ZERO",
        r#"{"firstDifferences":[{"accountId":"x"},{"accountId":"y"}]}"#,
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::TieOutNotZero { differences: 2 })
    );
    for (code, want) in [
        ("IMPORT_BUSY", ImportRefusal::Busy),
        ("IMPORT_COMPANY_NOT_EMPTY", ImportRefusal::CompanyNotEmpty),
        ("IMPORT_SESSION_IN_BATCH", ImportRefusal::SessionInBatch),
        ("IMPORT_FISCAL_YEAR_LOCKED", ImportRefusal::FiscalYearLocked),
    ] {
        let e = refusal(409, code, "{}").await;
        assert_eq!(import_refusal(&e), Some(want), "{code}");
        assert!(!format!("{e:?}").contains("Café"), "{code}: no message");
    }
    let e = refusal(403, "IMPORT_ADMIN_REQUIRED", "{}").await;
    assert_eq!(import_refusal(&e), Some(ImportRefusal::AdminRequired));
    let e = refusal(
        422,
        "IMPORT_AMOUNT_INVALID",
        r#"{"rows":[{"rowNumber":9},{"rowNumber":3},9]}"#,
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::AmountInvalid { rows: vec![3, 9] })
    );
    assert_eq!(import_refusal(&refusal(409, "SOMETHING", "{}").await), None);
}

#[tokio::test]
async fn the_batch_codes_of_e1_are_refusals() {
    // E1 as built: the batch codes, at 409 and 422 alike.
    for (status, code, want) in [
        (409, "IMPORT_BATCH_UNDONE", ImportRefusal::BatchUndone),
        (
            409,
            "IMPORT_BATCH_NOT_COMMITTED",
            ImportRefusal::BatchNotCommitted,
        ),
        (422, "IMPORT_BATCH_EMPTY", ImportRefusal::BatchEmpty),
        (409, "IMPORT_BATCH_EMPTY", ImportRefusal::BatchEmpty),
        (422, "IMPORT_SOURCE_MISMATCH", ImportRefusal::SourceMismatch),
        (409, "IMPORT_INTERRUPTED", ImportRefusal::Interrupted),
    ] {
        let e = refusal(status, code, "{}").await;
        assert_eq!(import_refusal(&e), Some(want), "{code}");
    }
    // A batch's failure code reads the same way (Check status).
    let f = BatchFailure {
        code: "IMPORT_INTERRUPTED".into(),
        ..BatchFailure::default()
    };
    assert_eq!(f.refusal(), Some(ImportRefusal::Interrupted));
}

/// E2b and E2c as built: the mapping, inventory and date codes, with their
/// details read as keys, ids, dates and counts (never messages or names
/// in `Debug`).
#[tokio::test]
async fn the_mapping_inventory_and_date_codes_are_refusals() {
    let e = refusal(
        400,
        "IMPORT_ACCOUNT_MAP_INVALID",
        r#"{"keys":["Riverside Repairs","1200"]}"#,
    )
    .await;
    let Some(ImportRefusal::AccountMapInvalid { keys }) = import_refusal(&e) else {
        panic!("map invalid: {e:?}")
    };
    assert_eq!(keys.0, vec!["Riverside Repairs".to_string(), "1200".into()]);
    assert_eq!(format!("{keys:?}"), "SourceKeys(2)");
    let e = refusal(
        422,
        "IMPORT_DATE_MISMATCH",
        &format!(
            r#"{{"conversionDate":"2026-10-27","files":[{{"sessionId":"{SESSION}","kind":"open_invoices","role":"post","originalFilename":"Riverside.csv","reportDate":"2026-10-26","reason":"not_conversion_date"}},{{"sessionId":"{SESSION2}","kind":"unpaid_bills","role":"post","reportDate":null,"reason":"no_date"}}]}}"#
        ),
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::DateMismatch {
            conversion_date: Some(day("2026-10-27")),
            files: vec![
                DateMismatchFile {
                    session: Some(SESSION.parse().expect("id")),
                    kind: Some(ImportKind::OpenInvoices),
                    report_date: Some(day("2026-10-26")),
                    reason: Some(DateMismatchReason::NotConversionDate),
                },
                DateMismatchFile {
                    session: Some(SESSION2.parse().expect("id")),
                    kind: Some(ImportKind::UnpaidBills),
                    report_date: None,
                    reason: Some(DateMismatchReason::NoDate),
                },
            ],
        })
    );
    assert!(!format!("{:?}", import_refusal(&e)).contains("Riverside"));
    let e = refusal(409, "IMPORT_INVENTORY_OFF", "{}").await;
    assert_eq!(import_refusal(&e), Some(ImportRefusal::InventoryOff));
    let e = refusal(
        409,
        "INVENTORY_PERIOD_CLOSED",
        &format!(r#"{{"closingDate":"2026-09-30","sessionId":"{SESSION}"}}"#),
    )
    .await;
    assert!(
        !matches!(e, ApiError::ClosedPeriod(_)),
        "no password lifts it"
    );
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::InventoryPeriodClosed {
            closing_date: Some(day("2026-09-30"))
        })
    );
    let e = refusal(
        422,
        "IMPORT_JOURNAL_AFTER_CONVERSION",
        &format!(
            r#"{{"conversionDate":"2026-10-27","rowCount":3,"rows":[{{"sessionId":"{SESSION}","rowNumber":41,"date":"2026-10-28","code":"IMPORT_JOURNAL_AFTER_CONVERSION","message":"Riverside"}}]}}"#
        ),
    )
    .await;
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::JournalAfterConversion {
            conversion_date: Some(day("2026-10-27")),
            row_count: 3,
            rows: vec![(Some(SESSION.parse().expect("id")), 41)],
        })
    );
    let e = refusal(
        400,
        "IMPORT_BAD_DATE",
        r#"{"historyFrom":"2026-11-01","conversionDate":"2026-10-27"}"#,
    )
    .await;
    assert_eq!(import_refusal(&e), Some(ImportRefusal::BadDate));
}

/// A commit that failed after the 202 keeps the engine's details: the
/// unmapped keys, the accounts a valuation is needed for. A committed
/// batch's files carry their results; the valuation total stays exact.
#[tokio::test]
async fn a_failed_batch_keeps_its_details_and_a_committed_one_its_results() {
    let failure = |details: &str| BatchFailure {
        code: String::new(),
        details: crate::ConflictDetails::new(serde_json::from_str(details).expect("json")),
        ..BatchFailure::default()
    };
    let f = BatchFailure {
        code: "IMPORT_UNMAPPED_ACCOUNT".into(),
        ..failure(r#"{"unmappedAccountKeys":["6999","Riverside Supplies"],"unmappedCount":7}"#)
    };
    let Some(ImportRefusal::UnmappedAccount { keys, count }) = f.refusal() else {
        panic!("unmapped")
    };
    assert_eq!(keys.0.len(), 2);
    assert_eq!(count, 7);
    assert!(!format!("{f:?}").contains("Riverside"), "{f:?}");
    let f = BatchFailure {
        code: "IMPORT_VALUATION_REQUIRED".into(),
        ..failure(&format!(
            r#"{{"sessionId":"{SESSION}","accounts":[{{"sourceKey":"1300","balance":"18250.00"}}]}}"#
        ))
    };
    assert_eq!(
        f.refusal(),
        Some(ImportRefusal::ValuationRequired {
            keys: SourceKeys(vec!["1300".into()])
        })
    );

    let server = MockServer::start().await;
    let committed = BATCH_JSON
        .replace(r#""status":"draft""#, r#""status":"committed""#)
        .replace(
            r#""commitResult":null},
  {"#,
            r#""commitResult":{"created":11,"skipped":1,"mergedToBoth":2,"jobsFolded":3,"inactive":1}},
  {"#,
        )
        .replace(
            r#""reportDate":"2026-10-27","commitResult":null"#,
            r#""reportDate":"2026-10-27","commitResult":{"created":4,"itemsValued":4,"valuationTotal":"18250.0050","itemsSkipped":{"sales_tax":2,"group":1},"skippedBeforeHistory":0}"#,
        );
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"batch":{committed},"progress":null,
                "error":{{"code":"IMPORT_UNMAPPED_ACCOUNT","message":"Riverside","status":422,"sessionId":"{SESSION2}","details":{{"unmappedAccountKeys":["Riverside"],"unmappedCount":1}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let st = s.imports().batch(bid()).await.expect("state");
    let names = st.batch.files[0].result.clone().expect("result");
    assert_eq!(
        (
            names.created,
            names.merged_to_both,
            names.jobs_folded,
            names.inactive
        ),
        (11, 2, 3, 1)
    );
    let stock = st.batch.files[1].result.clone().expect("result");
    assert_eq!(stock.items_valued, 4);
    assert_eq!(
        stock.valuation_total,
        Some(m("18250.0050")),
        "exact, never rounded"
    );
    assert_eq!(stock.items_skipped.map(|i| i.total()), Some(3));
    assert_eq!(stock.skipped_before_history, Some(0));
    assert!(!format!("{stock:?}").contains("18250"));
    assert!(matches!(
        st.error.expect("failure").refusal(),
        Some(ImportRefusal::UnmappedAccount { count: 1, .. })
    ));
}
#[tokio::test]
async fn closed_period_password_and_rate_limit_are_the_shared_errors() {
    let e = refusal(
        423,
        "TB_PERIOD_LOCKED",
        r#"{"lockDate":"2025-12-31","canOverride":false,"closingPasswordRequired":true}"#,
    )
    .await;
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    assert_eq!(import_refusal(&e), None);
    let e = refusal(403, "CLOSING_PASSWORD_INVALID", "{}").await;
    assert!(matches!(e, ApiError::ClosingPasswordRejected), "{e:?}");
    let e = refusal(429, "CLOSING_PASSWORD_RATE_LIMIT", "{}").await;
    assert!(
        matches!(e, ApiError::ClosingPasswordRateLimited { .. }),
        "{e:?}"
    );
}

#[tokio::test]
async fn batch_state_reads_progress_and_a_failure_with_the_same_codes() {
    let server = MockServer::start().await;
    let failed = BATCH_JSON.replace(r#""status":"draft""#, r#""status":"failed""#);
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}")))
        .respond_with(json(
            200,
            &format!(
                r#"{{"batch":{failed},"progress":{{"step":"journal","done":120,"total":400}},
                "error":{{"code":"IMPORT_PERIOD_OVERLAP","rowNumber":42,"sessionId":"{SESSION2}"}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let st = s.imports().batch(bid()).await.expect("state");
    assert_eq!(st.batch.status, BatchStatus::Failed);
    assert_eq!(
        st.progress,
        Some(Progress {
            step: "journal".into(),
            done: 120,
            total: 400
        })
    );
    let err = st.error.expect("failure");
    assert_eq!(err.row_number, Some(42));
    assert_eq!(err.session, Some(SESSION2.parse().expect("id")));
    assert!(matches!(
        err.refusal(),
        Some(ImportRefusal::PeriodOverlap { .. })
    ));
}

#[tokio::test]
async fn batches_all_follows_the_cursor() {
    let server = MockServer::start().await;
    let second = BATCH_JSON.replace("4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b03", SESSION);
    Mock::given(method("GET"))
        .and(path("/api/v1/imports/batches"))
        .respond_with(move |r: &wiremock::Request| {
            let q = r.url.query().unwrap_or("");
            let body = if q.contains("cursor=c2") {
                format!(r#"{{"data":[{second}],"nextCursor":null}}"#)
            } else {
                format!(r#"{{"data":[{BATCH_JSON}],"nextCursor":"c2"}}"#)
            };
            ResponseTemplate::new(200).set_body_raw(body, "application/json")
        })
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let all = s.imports().batches_all().await.expect("history");
    assert_eq!(all.len(), 2);
    let gets: Vec<String> = requests(&server, "GET")
        .await
        .iter()
        .filter(|r| r.url.path() == "/api/v1/imports/batches")
        .map(query)
        .collect();
    assert_eq!(gets.len(), 2);
    assert!(gets[0].contains("limit=50") && !gets[0].contains("cursor"));
    assert!(gets[1].contains("cursor=c2"));
}

const TIEOUT: &str = r#"{"asOf":"2026-10-27",
 "accounts":[
  {"accountId":"5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9b04","name":"Accounts Receivable","sourceRow":6,"sourceKeys":["1200"],
   "sourceExact":"4311.67","booksExact":"4321.67","diffExact":"10.00"},
  {"accountId":null,"name":"Fuel","sourceRow":12,"sourceKeys":["6100"],
   "sourceExact":"339.44","booksExact":"339.4440","diffExact":"0.0040"}],
 "arItems":[{"sourceRow":9,"contactName":"Riverside Café","number":null,"sourceExact":"-300.00","booksExact":"-300.00","diffExact":"0.00"}],
 "apItems":[],
 "inventoryItems":[{"sourceRow":5,"itemName":"Live Oak 15gal","sourceExact":"1125.00","booksExact":"1125.00","diffExact":"0.00"}],
 "balanceRows":[{"key":"history_ar","diffExact":"-10.00"},{"key":"valuation_vs_asset","diffExact":"0.00"},{"key":"from_the_future","diffExact":"0.00"}],
 "totals":{"sourceExact":"0.00","booksExact":"0.00","diffExact":"0.0040"},
 "allZero":false}"#;

#[tokio::test]
async fn tieout_reads_exact_differences_and_hides_owner_data() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/tieout")))
        .respond_with(json(200, TIEOUT))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let t = s.imports().tieout(bid()).await.expect("tieout");
    assert_eq!(t.as_of, day("2026-10-27"));
    assert!(!t.all_zero);
    assert_eq!(t.accounts[0].diff, m("10.00"));
    assert_eq!(t.accounts[0].source_keys, vec!["1200".to_string()]);
    assert_eq!(
        t.accounts[1].diff,
        m("0.0040"),
        "a difference under a cent stays exact"
    );
    assert_eq!(t.accounts[1].account, None);
    assert_eq!(t.ar_items[0].source, m("-300.00"));
    assert_eq!(t.inventory_items[0].name, "Live Oak 15gal");
    assert_eq!(
        t.balance_rows,
        vec![
            BalanceRow {
                key: BalanceKey::HistoryAr,
                diff: m("-10.00")
            },
            BalanceRow {
                key: BalanceKey::ValuationVsAsset,
                diff: m("0.00")
            },
        ],
        "an unknown check is skipped"
    );
    assert_eq!(t.totals.diff, Some(m("0.0040")));
    let dbg = format!("{t:?}");
    for owner in ["Café", "Fuel", "Live Oak", "4311", "339"] {
        assert!(!dbg.contains(owner), "{owner} in {dbg}");
    }
    let number = TIEOUT.replace(r#""diffExact":"10.00""#, r#""diffExact":10.0"#);
    assert!(matches!(
        tieout_from(number.as_bytes()),
        Err(ApiError::InexactMoney { .. })
    ));
    let no_flag = TIEOUT.replace(r#""allZero":false"#, r#""allZeroX":true"#);
    assert!(
        tieout_from(no_flag.as_bytes()).is_err(),
        "allZero is required"
    );
}

#[tokio::test]
async fn verify_and_spot_check() {
    let server = MockServer::start().await;
    let verified = BATCH_JSON
        .replace(r#""status":"draft""#, r#""status":"committed""#)
        .replace(
            r#""verifiedAt":null"#,
            r#""verifiedAt":"2026-11-18T11:00:00Z""#,
        );
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/verify")))
        .respond_with(json(200, &format!(r#"{{"batch":{verified}}}"#)))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/spot-check")))
        .respond_with(json(200, &format!(r#"{{"batch":{verified}}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let b = s.imports().verify(bid()).await.expect("verified");
    assert!(b.is_verified());
    let items = vec![
        SpotCheckItem {
            key: "bank:1000".into(),
            label: "Checking".into(),
            done: true,
            note: "Matches the October statement".into(),
        },
        SpotCheckItem {
            key: "sample:1".into(),
            label: "Sample".into(),
            done: false,
            note: String::new(),
        },
    ];
    s.imports()
        .save_spot_check(bid(), &items)
        .await
        .expect("saved");
    let sent = body_of(&requests(&server, "PUT").await[0]);
    assert_eq!(sent["items"][0]["key"], "bank:1000");
    assert_eq!(sent["items"][0]["done"], true);
    assert_eq!(sent["items"][1]["note"], "");
    let mut dup = items.clone();
    dup[1].key = "bank:1000".into();
    let mut long = items.clone();
    long[0].note = "x".repeat(MAX_NOTE_CHARS + 1);
    for bad in [dup, long] {
        assert!(matches!(
            s.imports().save_spot_check(bid(), &bad).await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
    }
    assert_eq!(requests(&server, "PUT").await.len(), 1);
    assert!(!format!("{items:?}").contains("October"));
}

#[test]
fn the_key_set_is_imports_batch() {
    use crate::resources::operations::KeySet;
    assert_eq!(IMPORT_BATCH_COMMIT.key_set(), Some(KeySet::ImportsV1));
    assert_eq!(IMPORT_BATCH_UNDO.key_set(), Some(KeySet::ImportsV1));
    assert_eq!(KeySet::ImportsV1.capability(), "imports-batch-v1");
    assert_eq!(IMPORT_BATCH_TIEOUT.key_set(), None);
    assert_eq!(IMPORTS_UPLOAD.key_set(), None);
}
