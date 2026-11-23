//! P34 B2 and B3 against a stub engine (wiremock), until the engine pin
//! carries U-296 and U-12 (then `test_restore_live.rs` and
//! `verify_live.rs` run them for real):
//! - a test restore records the engine's verdict and trial balance on the
//!   backup's history entry; a changed file is a failed test and nothing
//!   is sent; a wrong passphrase is a failed test with the engine's code;
//! - a fix makes a verified before-fix backup first, sends the run's
//!   ledger version and that backup's archive id, then checks again; a
//!   backup that fails means no fix.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{Client, ClientConfig, EndpointSource, FixedEndpoint, LoginOutcome, Session};
use cl_company::CompanyError;
use cl_company::health::{FixRequest, fix_after_backup, quick_check_due, run_check};
use cl_company::verify::{reason, test_restore};
use cl_core::ids::TenantId;
use cl_core::{Secret, Timestamp};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{
    BackupKind, BackupRecord, BackupTrigger, MemorySecrets, RecentStore, SecretStore,
};
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "0a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00";
const T1: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CO: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig";

fn reply(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

async fn engine(caps: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    let tokens = reply(
        200,
        json!({"user":{"id":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"},"tokens":{"accessToken":JWT},
               "accessibleTenants":[{"tenantId":T1,"tenantName":"Ozark Ridge","role":"owner"}]}),
    )
    .insert_header(
        "set-cookie",
        "kb_refresh=refresh_token_value_1234; Path=/; HttpOnly",
    );
    Mock::given(method("GET"))
        .and(path("/api/v1/desktop/info"))
        .respond_with(reply(
            200,
            json!({"mode":"desktop","initialized":true,"workspaceTenantId":WS,"capabilities":caps}),
        ))
        .mount(&server)
        .await;
    for p in ["/api/v1/auth/login", "/api/v1/auth/switch-tenant"] {
        Mock::given(method("POST"))
            .and(path(p))
            .respond_with(tokens.clone())
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(reply(
            200,
            json!({"accessibleTenants":[
                {"tenantId":WS,"tenantName":"workspace","role":"owner"},
                {"tenantId":T1,"tenantName":"Ozark Ridge","role":"owner"}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(reply(
            200,
            json!({"company":{"id":CO,"businessName":"Ozark Ridge","entityType":"single_member_llc","country":"US"}}),
        ))
        .mount(&server)
        .await;
    server
}

async fn session(server: &MockServer, open: bool) -> Session {
    let src: Arc<dyn EndpointSource> = Arc::new(FixedEndpoint::new(
        server.address().port(),
        Secret::new("launch-token"),
        1,
    ));
    let client = Client::new(src, ClientConfig::default()).expect("client");
    client.desktop_info().await.expect("info");
    match client
        .login("owner@example.com", &Secret::new("password-123456"))
        .await
        .expect("login")
    {
        LoginOutcome::Session(s) => {
            if open {
                s.open_company(t(T1)).await.expect("open");
            }
            s
        }
        LoginOutcome::TfaRequired(_) => panic!("tfa"),
    }
}

fn t(s: &str) -> TenantId {
    TenantId::parse(s).expect("id")
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn stores(dir: &Path) -> (RecentStore, MemorySecrets) {
    let store = RecentStore::in_dir(&dir.join("settings"));
    let secrets = MemorySecrets::new();
    secrets
        .set(BACKUP_PASSPHRASE, &Secret::new("backup-passphrase-canary"))
        .expect("set");
    store
        .update(|l| {
            l.upsert(t(T1), "Ozark Ridge");
        })
        .expect("store");
    (store, secrets)
}

/// A readable archive envelope; returns the bytes written.
fn write_archive(p: &Path, archive_id: &str) -> Vec<u8> {
    write_archive_with(p, archive_id, b"ciphered")
}

fn write_archive_with(p: &Path, archive_id: &str, payload: &[u8]) -> Vec<u8> {
    let env = format!(
        r#"{{"format":"mybooks-tenant-archive","formatVersion":2,"archiveId":"{archive_id}","createdAt":"2026-11-04T08:12:00.000Z"}}"#
    );
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).expect("dir");
    }
    let file = std::fs::File::create(p).expect("create");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("envelope.json", opts).expect("entry");
    zip.write_all(env.as_bytes()).expect("write");
    zip.start_file("manifest.json.enc", opts).expect("entry");
    zip.write_all(payload).expect("write");
    zip.finish().expect("finish");
    std::fs::read(p).expect("read")
}

/// A backup this app wrote, in the history.
fn recorded_backup(store: &RecentStore, dir: &Path) -> (PathBuf, String) {
    let p = std::path::absolute(dir.join("Backups").join("Ozark-20261104-081200.clbackup"))
        .expect("abs");
    let bytes = write_archive(&p, "arch-1");
    let sha = sha256(&bytes);
    store
        .update(|l| {
            l.record_backup(
                t(T1),
                BackupRecord {
                    path: p.clone(),
                    sha256: sha.clone(),
                    archive_id: "arch-1".into(),
                    created_at: Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts"),
                    kind: BackupKind::Backup,
                    trigger: Some(BackupTrigger::Scheduled),
                    tested: None,
                },
            );
        })
        .expect("store");
    (p, sha)
}

async fn mount_test_job(server: &MockServer, status: Value) {
    Mock::given(method("POST"))
        .and(path("/api/v1/tenant-archives/test-restore"))
        .respond_with(reply(202, json!({"jobId":"job_t"})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_t"))
        .respond_with(reply(200, status))
        .mount(server)
        .await;
}

async fn posts(server: &MockServer, p: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == p)
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

fn tb(equal: bool) -> Value {
    json!({"rowsEqual":equal,"totalsEqual":equal,
        "expected":{"debitExact":"20000.0000","creditExact":"20000.0000","rows":42},
        "actual":{"debitExact": if equal {"20000.0000"} else {"19990.0000"},"creditExact":"20000.0000","rows":42},
        "differenceExact":{"debit": if equal {"0.0000"} else {"-10.0000"},"credit":"0.0000"}})
}

const TEST_CAP: &str = "tenant-archive-test-restore-v1";

#[tokio::test]
async fn a_test_restore_records_the_engines_verdict_and_trial_balance() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", TEST_CAP]).await;
    let s = session(&server, false).await;
    let (store, secrets) = stores(dir.path());
    let (p, sha) = recorded_backup(&store, dir.path());
    mount_test_job(
        &server,
        json!({"jobId":"job_t","kind":"test-restore","status":"succeeded","progress":100,
            "result":{"testRestore":{"passed":true,"archiveId":"arch-1","sourceTenantId":T1,
            "trialBalance":tb(true),"unbalanced":{"expected":0,"actual":0},"failures":[],"warnings":[]}}}),
    )
    .await;
    let out = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect("tested");
    assert!(out.test.passed);
    assert!(out.recorded);
    assert_eq!(out.sha256.as_deref(), Some(sha.as_str()));
    let body = &posts(&server, "/api/v1/tenant-archives/test-restore").await[0];
    assert_eq!(body["passphrase"], "backup-passphrase-canary");
    assert_eq!(body["path"], p.to_string_lossy().as_ref());
    let list = store.load().expect("load");
    let rec = list
        .get(t(T1))
        .expect("c")
        .backup_by_sha256(&sha)
        .expect("b");
    let tested = rec.tested.as_ref().expect("tested");
    assert!(tested.passed);
    let tbv = tested.tb.as_ref().expect("tb");
    assert!(tbv.rows_equal && tbv.totals_equal);
    assert_eq!(tbv.expected_debit.to_wire(), "20000.0000", "exact strings");
    assert_eq!(
        list.get(t(T1)).expect("c").backups.len(),
        1,
        "no company made"
    );
}

#[tokio::test]
async fn a_mismatch_is_a_failed_test_with_the_engines_figures() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", TEST_CAP]).await;
    let s = session(&server, false).await;
    let (store, secrets) = stores(dir.path());
    let (p, _) = recorded_backup(&store, dir.path());
    mount_test_job(
        &server,
        json!({"jobId":"job_t","kind":"test-restore","status":"succeeded","progress":100,
            "result":{"testRestore":{"passed":false,"archiveId":"arch-1",
            "trialBalance":tb(false),"failures":[{"check":"trialBalance"}],"warnings":[]}}}),
    )
    .await;
    let out = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect("tested");
    assert!(!out.test.passed);
    assert_eq!(out.test.reason.as_deref(), Some(reason::MISMATCH));
    assert_eq!(out.failures, vec!["trialBalance".to_string()]);
    let tbv = out.test.tb.expect("tb");
    assert!(!tbv.totals_equal);
    assert_eq!(tbv.actual_debit.to_wire(), "19990.0000");
}

#[tokio::test]
async fn a_changed_or_wrong_file_is_a_failed_test_not_an_error() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", TEST_CAP]).await;
    let s = session(&server, false).await;
    let (store, secrets) = stores(dir.path());
    let (p, _) = recorded_backup(&store, dir.path());
    // Changed after it was written: same archive id, other bytes.
    write_archive_with(&p, "arch-1", b"changed afterwards");
    let out = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect("a result");
    assert!(!out.test.passed);
    assert_eq!(out.test.reason.as_deref(), Some(reason::TAMPERED));
    assert!(out.recorded, "kept on the entry this app wrote");
    assert_eq!(
        posts(&server, "/api/v1/tenant-archives/test-restore")
            .await
            .len(),
        0,
        "nothing sent"
    );
    // Not a backup at all.
    let junk = dir.path().join("junk.clbackup");
    std::fs::write(&junk, b"no").expect("write");
    let out = test_restore(&s, &store, &secrets, &junk, &cl_company::backup::silent())
        .await
        .expect("a result");
    assert_eq!(out.test.reason.as_deref(), Some(reason::NOT_A_BACKUP));
    assert!(!out.recorded);
    // The engine refuses the passphrase: its code is the reason.
    let fresh = std::path::absolute(dir.path().join("Backups").join("Other.clbackup")).expect("p");
    write_archive(&fresh, "arch-2");
    mount_test_job(
        &server,
        json!({"jobId":"job_t","kind":"test-restore","status":"failed",
            "error":{"code":"ARCHIVE_PASSPHRASE_INVALID","message":"wrong"}}),
    )
    .await;
    let out = test_restore(&s, &store, &secrets, &fresh, &cl_company::backup::silent())
        .await
        .expect("a result");
    assert!(!out.test.passed);
    assert_eq!(
        out.test.reason.as_deref(),
        Some(reason::PASSPHRASE),
        "review M5: its own reason, not a damaged file"
    );
    // QA-14: the recorded file cut short no longer reads as a backup; it
    // is this app's file, so it changed, not "not a backup".
    std::fs::write(&p, b"PK").expect("truncate");
    let out = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect("a result");
    assert_eq!(out.test.reason.as_deref(), Some(reason::TAMPERED));
    assert!(out.recorded);
}

/// Q-P34bi: no disk space (or a busy engine) says nothing about the file:
/// an error, and nothing recorded on the backup.
#[tokio::test]
async fn an_engine_that_could_not_run_it_records_nothing() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", TEST_CAP]).await;
    let s = session(&server, false).await;
    let (store, secrets) = stores(dir.path());
    let (p, sha) = recorded_backup(&store, dir.path());
    mount_test_job(
        &server,
        json!({"jobId":"job_t","kind":"test-restore","status":"failed",
            "error":{"code":"INSUFFICIENT_STORAGE","message":"full"}}),
    )
    .await;
    let e = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect_err("could not run");
    assert!(
        matches!(&e, CompanyError::JobFailed { code } if code == "INSUFFICIENT_STORAGE"),
        "{e:?}"
    );
    let list = store.load().expect("load");
    let b = list
        .get(t(T1))
        .expect("c")
        .backup_by_sha256(&sha)
        .expect("b");
    assert_eq!(b.tested, None);
}

#[tokio::test]
async fn without_the_capability_nothing_is_sent() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server, false).await;
    let (store, secrets) = stores(dir.path());
    let (p, _) = recorded_backup(&store, dir.path());
    let e = test_restore(&s, &store, &secrets, &p, &cl_company::backup::silent())
        .await
        .expect_err("unsupported");
    assert!(matches!(e, CompanyError::Unavailable(_)), "{e:?}");
}

const RUN: &str = r#"{"id":"run-1","level":"full","companyId":"c1",
    "startedAt":"2026-11-23T10:00:00.000Z","finishedAt":"2026-11-23T10:00:02.000Z",
    "ledgerVersion":"77",
    "checks":[{"code":"ACCOUNT_BALANCE_DRIFT","status":"found","found":1}],
    "findings":[{"id":"ACCOUNT_BALANCE_DRIFT:a1","code":"ACCOUNT_BALANCE_DRIFT","severity":"warning",
        "subject":{"type":"account","id":"a1","label":"Checking"},
        "amounts":{"expectedExact":"1250.1250","actualExact":"1200.0000","differenceExact":"50.1250"},
        "fix":"recompute_account_balance"}],
    "truncated":false}"#;

const CLEAN: &str = r#"{"id":"run-2","level":"full","companyId":"c1",
    "startedAt":"2026-11-23T10:01:00.000Z","finishedAt":"2026-11-23T10:01:02.000Z",
    "ledgerVersion":"77","checks":[{"code":"ACCOUNT_BALANCE_DRIFT","status":"ok","found":0}],
    "findings":[],"truncated":false}"#;

async fn mount_backup(server: &MockServer, dir: &Path, ok: bool) {
    let engine_dir = dir.join("engine-archives");
    let file = std::path::absolute(engine_dir.join("b.zip")).expect("abs");
    let bytes = write_archive(&file, "arch-fix");
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+/archives$"))
        .respond_with(reply(202, json!({"jobId":"job_b","archiveId":"arch-fix"})))
        .mount(server)
        .await;
    let status = if ok {
        json!({"status":"succeeded","result":{"archiveId":"arch-fix","fileName":"b.zip",
            "path":file.to_string_lossy(),"sha256":sha256(&bytes)}})
    } else {
        json!({"status":"failed","error":{"code":"ARCHIVE_NO_SPACE","message":"full"}})
    };
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_b"))
        .respond_with(reply(200, status))
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_fix_backs_up_first_then_fixes_with_the_runs_version_then_checks_again() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", "integrity-v1"]).await;
    let s = session(&server, true).await;
    let (store, secrets) = stores(dir.path());
    mount_backup(&server, dir.path(), true).await;
    Mock::given(method("POST"))
        .and(path("/api/v1/integrity/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(RUN, "application/json"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/integrity/checks"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CLEAN, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/integrity/fixes"))
        .respond_with(reply(
            200,
            json!({"fixed":["ACCOUNT_BALANCE_DRIFT:a1"],"skipped":[],"ledgerVersion":"77"}),
        ))
        .mount(&server)
        .await;
    let run = run_check(&s, cl_api::IntegrityLevel::Full)
        .await
        .expect("check");
    assert_eq!(run.findings.len(), 1);
    let ids: Vec<_> = run.fixable().map(|f| f.id.clone()).collect();
    let backups = dir.path().join("Backups");
    let out = fix_after_backup(
        cl_company::backup::GATE.lock().await,
        &s,
        &store,
        &secrets,
        &FixRequest {
            tenant_id: t(T1),
            backup_dir: &backups,
            run: &run,
            finding_ids: &ids,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect("fixed");
    assert_eq!(out.backup.trigger, Some(BackupTrigger::BeforeFix));
    assert!(out.backup.path.is_file(), "a verified file on disk");
    assert_eq!(out.result.fixed.len(), 1);
    assert_eq!(out.recheck.expect("rechecked").findings.len(), 0);
    let fix = &posts(&server, "/api/v1/integrity/fixes").await[0];
    assert_eq!(fix["runId"], "run-1");
    assert_eq!(fix["expectedLedgerVersion"], "77");
    assert_eq!(fix["backupArchiveId"], "arch-fix");
    assert_eq!(fix["findingIds"], json!(["ACCOUNT_BALANCE_DRIFT:a1"]));
    // The backup came before the fix.
    let order: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r| r.method.as_str() == "POST")
        .map(|r| r.url.path().to_string())
        .filter(|p| p.contains("archives") || p.contains("integrity"))
        .collect();
    let b = order
        .iter()
        .position(|p| p.contains("archives"))
        .expect("b");
    let f = order.iter().position(|p| p.ends_with("fixes")).expect("f");
    assert!(b < f, "{order:?}");
}

#[tokio::test]
async fn no_backup_means_no_fix() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", "integrity-v1"]).await;
    let s = session(&server, true).await;
    let (store, secrets) = stores(dir.path());
    mount_backup(&server, dir.path(), false).await;
    let run: cl_api::IntegrityRun = serde_json::from_str(RUN).expect("run");
    let ids: Vec<_> = run.fixable().map(|f| f.id.clone()).collect();
    let backups = dir.path().join("Backups");
    let e = fix_after_backup(
        cl_company::backup::GATE.lock().await,
        &s,
        &store,
        &secrets,
        &FixRequest {
            tenant_id: t(T1),
            backup_dir: &backups,
            run: &run,
            finding_ids: &ids,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect_err("no fix");
    assert!(matches!(e, CompanyError::JobFailed { .. }), "{e:?}");
    assert_eq!(posts(&server, "/api/v1/integrity/fixes").await.len(), 0);
}

#[test]
fn the_weekly_quick_check_is_due_after_seven_days() {
    let now = Timestamp::parse_wire("2026-11-23T10:00:00.000Z").expect("ts");
    let none: cl_api::IntegritySummary =
        serde_json::from_value(json!({"lastQuick":null,"lastFull":null})).expect("s");
    assert!(quick_check_due(&none, now));
    let at = |d: &str| -> cl_api::IntegritySummary {
        serde_json::from_value(json!({"lastQuick":{"id":"r1","finishedAt":d,
            "counts":{"error":0,"warning":0},"fixable":0},"lastFull":null}))
        .expect("s")
    };
    assert!(!quick_check_due(&at("2026-11-17T10:00:01.000Z"), now));
    assert!(quick_check_due(&at("2026-11-16T10:00:00.000Z"), now));
    assert!(
        !quick_check_due(&at("2026-12-01T10:00:00.000Z"), now),
        "clock went back"
    );
}
