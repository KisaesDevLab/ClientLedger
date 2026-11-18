//! Unit tests of the spreadsheet import resource (P31 D5), against a mock
//! engine. The live engine is covered by
//! `tests/contract/contract_import_transactions.rs` (pending until the pin
//! has `imports-batch-v1` and `imports-transactions-v1`).

use super::*;
use crate::errors::LocalValidation;
use crate::resources::testkit::{json, requests, session_with_caps};
use cl_core::Secret;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer};

const SESSION: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a11";
const BATCH: &str = "4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a12";

const ALL: [&str; 4] = [
    Capabilities::IMPORTS_BATCH,
    Capabilities::IMPORTS_TRANSACTIONS,
    Capabilities::CLOSING_PASSWORD,
    Capabilities::IDEMPOTENCY,
];

fn bid() -> ImportBatchId {
    BATCH.parse().expect("batch")
}

fn req() -> BatchRunRequest {
    BatchRunRequest {
        backup_ref: "Books-20261118-before-import.clbackup".into(),
        closed_period: None,
    }
}

#[tokio::test]
async fn nothing_is_sent_without_both_capabilities() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &[Capabilities::IMPORTS_BATCH]).await;
    let e = s
        .import_transactions()
        .upload(b"group_id\r\n")
        .await
        .expect_err("refused");
    assert!(matches!(e, ApiError::Local(LocalValidation::Unsupported)));
    assert!(requests(&server, "POST").await.is_empty());
    assert!(!transactions_supported(&s.client().capabilities()));
}

#[tokio::test]
async fn upload_sends_kind_transactions_and_keeps_codes_not_messages() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/imports/upload"))
        .respond_with(json(
            201,
            &format!(
                r#"{{"session":{{"id":"{SESSION}","rowCount":4}},"preview":{{"totalRows":4,"errorCount":1}},
                "validationErrors":[{{"rowNumber":3,"field":"line_account","code":"IMPORT_UNKNOWN_REF","message":"No account named Secret Name"}}]}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let out = s
        .import_transactions()
        .upload(b"group_id,type\r\n1,invoice\r\n")
        .await
        .expect("upload");
    assert_eq!(out.session, SESSION.parse().expect("id"));
    assert_eq!(out.total_rows, 4);
    assert_eq!(
        out.problems,
        vec![EngineRowProblem {
            row: 3,
            field: Some("line_account".into()),
            code: "IMPORT_UNKNOWN_REF".into(),
        }]
    );
    assert!(!format!("{out:?}").contains("Secret"));
    let sent = &requests(&server, "POST").await[0];
    let body = String::from_utf8_lossy(&sent.body);
    assert!(body.contains("name=\"kind\"\r\n\r\ntransactions\r\n"));
    assert!(body.contains("name=\"sourceSystem\"\r\n\r\ngeneric\r\n"));
    assert!(body.contains("filename=\"transactions.csv\""));
}

#[tokio::test]
async fn too_large_a_file_is_refused_before_sending() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &ALL).await;
    let big = vec![b'a'; MAX_UPLOAD_BYTES + 1];
    let e = s.import_transactions().upload(&big).await.expect_err("big");
    assert!(matches!(
        e,
        ApiError::Local(LocalValidation::AttachmentTooLarge)
    ));
    assert!(requests(&server, "POST").await.is_empty());
}

#[tokio::test]
async fn commit_is_keyed_sent_once_and_names_the_backup() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/imports/batches/{BATCH}/commit")))
        .respond_with(json(202, &format!(r#"{{"batchId":"{BATCH}"}}"#)))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let key = IdempotencyKey::new();
    s.import_transactions()
        .commit(bid(), &req(), key.clone())
        .await
        .expect("202");
    let posts = requests(&server, "POST").await;
    assert_eq!(posts.len(), 1);
    assert_eq!(
        posts[0].headers.get("idempotency-key").expect("key"),
        key.as_header().as_str()
    );
    let sent: Value = serde_json::from_slice(&posts[0].body).expect("json");
    assert_eq!(sent["backupRef"], "Books-20261118-before-import.clbackup");
    assert!(sent.get("closingPassword").is_none());
}

#[tokio::test]
async fn commit_and_undo_are_never_resent_on_a_gateway_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"/(commit|undo)$"))
        .respond_with(json(503, r#"{"error":{"message":"busy"}}"#))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let it = s.import_transactions();
    let _ = it.commit(bid(), &req(), IdempotencyKey::new()).await;
    let _ = it.undo(bid(), &req(), IdempotencyKey::new()).await;
    assert_eq!(requests(&server, "POST").await.len(), 2, "one each");
}

#[tokio::test]
async fn no_backup_no_commit() {
    let server = MockServer::start().await;
    let s = session_with_caps(&server, &ALL).await;
    let e = s
        .import_transactions()
        .commit(bid(), &BatchRunRequest::default(), IdempotencyKey::new())
        .await
        .expect_err("refused");
    assert!(matches!(
        e,
        ApiError::Local(LocalValidation::BackupRequired)
    ));
    assert!(requests(&server, "POST").await.is_empty());
}

#[tokio::test]
async fn the_closing_password_goes_in_the_body_and_never_in_debug() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json(202, "{}"))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let r = BatchRunRequest {
        closed_period: Some(ClosedPeriodAuthorization::with_password(Secret::new(
            "hunter2-pw",
        ))),
        ..req()
    };
    assert!(!format!("{r:?}").contains("hunter2"));
    s.import_transactions()
        .undo(bid(), &r, IdempotencyKey::new())
        .await
        .expect("202");
    let sent: Value =
        serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
    assert_eq!(sent["closingPassword"], "hunter2-pw");
}

#[test]
fn batch_reads_status_progress_and_error() {
    let b = batch_from(
        format!(
            r#"{{"batch":{{"id":"{BATCH}","status":"committing","backupRef":"x.clbackup"}},
            "progress":{{"step":"posting","done":3,"total":10}}}}"#
        )
        .as_bytes(),
    )
    .expect("batch");
    assert_eq!(b.status, BatchStatus::Committing);
    assert!(b.status.running());
    assert_eq!(b.progress.done, 3);
    assert_eq!(b.progress.total, 10);
    assert_eq!(b.error, None);
    let f = batch_from(
        format!(
            r#"{{"batch":{{"id":"{BATCH}","status":"failed"}},
            "error":{{"code":"IMPORT_UNBALANCED","rowNumber":7,"sessionId":"{SESSION}"}}}}"#
        )
        .as_bytes(),
    )
    .expect("batch");
    assert_eq!(f.status, BatchStatus::Failed);
    assert_eq!(
        f.error,
        Some(BatchError {
            code: "IMPORT_UNBALANCED".into(),
            row: Some(7),
            field: None
        })
    );
    assert!(batch_from(br#"{"batch":{"id":"nope","status":"draft"}}"#).is_err());
    assert!(
        batch_from(format!(r#"{{"batch":{{"id":"{BATCH}","status":"weird"}}}}"#).as_bytes())
            .is_err()
    );
}

#[tokio::test]
async fn refusals_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/imports/upload"))
        .respond_with(json(
            409,
            &format!(
                r#"{{"error":{{"code":"IMPORT_ALREADY_COMMITTED","message":"m","details":{{"batchId":"{BATCH}","sessionId":"{SESSION}","committedAt":"2026-11-01T10:00:00Z"}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/undo$"))
        .respond_with(json(
            409,
            r#"{"error":{"code":"IMPORT_UNDO_BLOCKED","message":"m","details":{"blockers":[{"kind":"payment","id":"p1","reason":"live_application"}]}}}"#,
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let e = s.import_transactions().upload(b"x").await.expect_err("409");
    assert_eq!(
        txn_import_refusal(&e),
        Some(TxnImportRefusal::AlreadyCommitted {
            batch: Some(bid()),
            committed_at: Some("2026-11-01T10:00:00Z".into()),
        })
    );
    let e = s
        .import_transactions()
        .undo(bid(), &req(), IdempotencyKey::new())
        .await
        .expect_err("409");
    assert_eq!(
        txn_import_refusal(&e),
        Some(TxnImportRefusal::UndoBlocked {
            blockers: vec![UndoBlocker {
                kind: "payment".into(),
                id: Some("p1".into()),
                reason: Some("live_application".into()),
            }]
        })
    );
    assert_eq!(txn_import_refusal(&ApiError::NoCompanyOpen), None);
}
