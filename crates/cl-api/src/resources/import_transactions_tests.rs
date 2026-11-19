//! Unit tests of the spreadsheet import resource (P31 D5), against a mock
//! engine. The live engine is covered by
//! `tests/contract/contract_import_transactions.rs`.

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
        vec![RowProblem {
            row_number: 3,
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
    assert!(b.status.is_running());
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
    let e = f.error.expect("error");
    assert_eq!(e.code, "IMPORT_UNBALANCED");
    assert_eq!(e.row_number, Some(7));
    assert_eq!(e.field, None);
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
        import_refusal(&e),
        Some(ImportRefusal::AlreadyCommitted {
            batch: Some(bid()),
            session: Some(SESSION.parse().expect("id")),
            committed_at: Some("2026-11-01T10:00:00Z".into()),
        })
    );
    let e = s
        .import_transactions()
        .undo(bid(), &req(), IdempotencyKey::new())
        .await
        .expect_err("409");
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::UndoBlocked {
            blockers: vec![UndoBlocker {
                kind: "payment".into(),
                id: Some("p1".into()),
                reason: Some("live_application".into()),
                target: None,
            }]
        })
    );
    assert_eq!(import_refusal(&ApiError::NoCompanyOpen), None);
}

/// U-268 as built: a batch that failed on a reference that went away after
/// the upload carries the column at the top level and every reference in
/// `details.refs` (rows, columns, reasons; never a name).
#[test]
fn an_unknown_ref_batch_error_reads_field_and_refs() {
    let f = batch_from(
        format!(
            r#"{{"batch":{{"id":"{BATCH}","status":"failed"}},
            "error":{{"code":"IMPORT_UNKNOWN_REF","message":"Unknown Pine Co","rowNumber":4,"field":"name","sessionId":"{SESSION}",
            "details":{{"field":"name","refs":[{{"rowNumber":4,"field":"name","reason":"ambiguous"}},{{"rowNumber":6,"field":"line_item","reason":"no_account"}},{{"rowNumber":7,"field":"line_tag","reason":"later"}}],"refCount":12}}}}}}"#
        )
        .as_bytes(),
    )
    .expect("batch");
    let e = f.error.expect("error");
    assert_eq!(e.field.as_deref(), Some("name"));
    assert_eq!(e.row_number, Some(4));
    match e.refusal() {
        Some(ImportRefusal::UnknownRef {
            field,
            refs,
            ref_count,
        }) => {
            assert_eq!(field.as_deref(), Some("name"));
            assert_eq!(ref_count, 12);
            assert_eq!(refs.len(), 3);
            assert_eq!(refs[0].reason, Some(RefReason::Ambiguous));
            assert_eq!(refs[1].field.as_deref(), Some("line_item"));
            assert_eq!(refs[1].reason, Some(RefReason::NoAccount));
            assert_eq!(refs[2].reason, None, "an unknown reason is kept as none");
        }
        other => panic!("{other:?}"),
    }
    assert!(!format!("{e:?}").contains("Pine"));
}

/// U-268: every row code arrives on the upload with its column; the
/// document count is read.
#[test]
fn upload_reads_the_e4_row_codes() {
    let u = upload_from(
        format!(
            r#"{{"session":{{"id":"{SESSION}"}},"preview":{{"totalRows":9,"documentCount":4}},
            "validationErrors":[
              {{"rowNumber":2,"field":null,"code":"IMPORT_UNBALANCED"}},
              {{"rowNumber":3,"field":"line_amount","code":"IMPORT_AMOUNT_CENTS"}},
              {{"rowNumber":4,"field":"line_item","code":"IMPORT_FIELD_NOT_ALLOWED"}},
              {{"rowNumber":5,"field":"type","code":"IMPORT_UNKNOWN_TYPE"}},
              {{"rowNumber":6,"field":"date","code":"IMPORT_MISSING_FIELD"}},
              {{"rowNumber":7,"field":"type","code":"IMPORT_MIXED_GROUP"}},
              {{"rowNumber":8,"field":"line_amount","code":"IMPORT_DOCUMENT_TOTAL"}},
              {{"rowNumber":9,"field":"line_tag","code":"IMPORT_UNKNOWN_REF"}}]}}"#
        )
        .as_bytes(),
    )
    .expect("upload");
    assert_eq!(u.total_rows, 9);
    assert_eq!(u.documents, Some(4));
    assert_eq!(u.problems.len(), 8);
    assert_eq!(u.problems[0].field, None);
    assert_eq!(u.problems[7].field.as_deref(), Some("line_tag"));
}

/// 409 `IMPORT_HAS_ERRORS` at commit and `IMPORT_SESSION_ACTIVE` at upload.
#[tokio::test]
async fn has_errors_and_session_active_are_classified() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/imports/upload"))
        .respond_with(json(
            409,
            &format!(
                r#"{{"error":{{"code":"IMPORT_SESSION_ACTIVE","message":"m","details":{{"existingSessionId":"{SESSION}","status":"uploaded"}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"/commit$"))
        .respond_with(json(
            409,
            &format!(
                r#"{{"error":{{"code":"IMPORT_HAS_ERRORS","message":"m","details":{{"sessionId":"{SESSION}","errors":[{{"rowNumber":5,"field":"type","code":"IMPORT_UNKNOWN_TYPE","message":"x"}}]}}}}}}"#
            ),
        ))
        .mount(&server)
        .await;
    let s = session_with_caps(&server, &ALL).await;
    let e = s.import_transactions().upload(b"x").await.expect_err("409");
    assert_eq!(
        import_refusal(&e),
        Some(ImportRefusal::SessionActive {
            session: Some(SESSION.parse().expect("id")),
            status: Some(crate::imports::SessionStatus::Uploaded),
        })
    );
    let e = s
        .import_transactions()
        .commit(bid(), &req(), IdempotencyKey::new())
        .await
        .expect_err("409");
    match import_refusal(&e) {
        Some(ImportRefusal::HasErrors { session, errors }) => {
            assert_eq!(session, Some(SESSION.parse().expect("id")));
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].row_number, 5);
            assert_eq!(errors[0].code, "IMPORT_UNKNOWN_TYPE");
        }
        other => panic!("{other:?}"),
    }
}
