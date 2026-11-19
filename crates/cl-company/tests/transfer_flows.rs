//! Send to My Accountant against a stub engine (wiremock): the order
//! backup → discard payroll drafts → transfer export, fail-closed checks
//! that send nothing, the S3 "not honoured" path, missing files listed in
//! one run, `TENANT_BUSY`, and the "Sent before" record.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{Client, ClientConfig, EndpointSource, FixedEndpoint, LoginOutcome, Session};
use cl_company::transfer::{
    MIN_PASSPHRASE_CHARS, TransferError, TransferRequest, check_passphrase, generate_passphrase,
    make_transfer, partial_path, preflight, suggested_file_name,
};
use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Secret};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{MemorySecrets, RecentStore, SecretStore};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "0a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00";
const T1: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CO: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig";
const PAYROLL_CAPS: [&str; 3] = [
    "payroll-import-exact-v1",
    "payroll-import-post-v1",
    "payroll-import-sensitive-v1",
];
const DRAFT: &str = "7d0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

fn json_reply(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

async fn engine(caps: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    let tokens = json_reply(
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
        .respond_with(json_reply(
            200,
            json!({"mode":"desktop","initialized":true,"workspaceTenantId":WS,"capabilities":caps}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(tokens.clone())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/switch-tenant"))
        .respond_with(tokens)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(json_reply(
            200,
            json!({"accessibleTenants":[
                {"tenantId":WS,"tenantName":"workspace","role":"owner"},
                {"tenantId":T1,"tenantName":"Ozark Ridge","role":"owner"}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json_reply(
            200,
            json!({"company":{"id":CO,"businessName":"Ozark Ridge","entityType":"single_member_llc","country":"US"}}),
        ))
        .mount(&server)
        .await;
    server
}

async fn session(server: &MockServer) -> Session {
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
            // The Send screen runs in the open company.
            s.open_company(t(T1)).await.expect("open");
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

const TOTALS: &str = r#"{"definition":"vt1","asOf":"2026-11-19","fiscalYearStart":"2026-01-01",
  "totalAssets":"12500.1250","totalLiabilities":"2000.00","totalEquity":"10500.1250",
  "netIncome":{"from":"2026-01-01","to":"2026-11-19","amount":"4321.0049"},
  "accountsReceivable":"1250.00","accountsPayable":"400.00",
  "trialBalance":{"debits":"20000.00","credits":"20000.00"},
  "counts":{"accounts":42,"clients":7,"vendors":5,"items":3,"transactions":1284,
  "postedTransactions":1270,"attachments":63,"attachmentBytes":1048576,"auditEntries":4200}}"#;

/// Two export jobs: the backup (`job_b`) and the transfer (`job_t`),
/// told apart by the request body. `transfer_result` is the transfer
/// job's final status JSON.
async fn mount_jobs(server: &MockServer, dir: &Path, transfer_status: Value) -> (PathBuf, PathBuf) {
    let engine_dir = dir.join("engine-archives");
    std::fs::create_dir_all(&engine_dir).expect("dir");
    let backup_file = std::path::absolute(engine_dir.join("b.zip")).expect("abs");
    std::fs::write(&backup_file, b"backup-bytes").expect("write");
    let transfer_file = std::path::absolute(engine_dir.join("t.zip")).expect("abs");
    std::fs::write(&transfer_file, b"transfer-bytes").expect("write");
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+/archives$"))
        .and(body_string_contains("\"transfer\":true"))
        .respond_with(json_reply(
            202,
            json!({"jobId":"job_t","archiveId":"arch-t"}),
        ))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+/archives$"))
        .respond_with(json_reply(
            202,
            json!({"jobId":"job_b","archiveId":"arch-b"}),
        ))
        .with_priority(2)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_b"))
        .respond_with(json_reply(
            200,
            json!({"status":"succeeded","result":{"archiveId":"arch-b","fileName":"b.zip",
                   "path":backup_file.to_string_lossy(),"sha256":sha256(b"backup-bytes")}}),
        ))
        .mount(server)
        .await;
    let mut status = transfer_status;
    if let Some(r) = status.get_mut("result") {
        r["path"] = json!(transfer_file.to_string_lossy());
        r["sha256"] = json!(sha256(b"transfer-bytes"));
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_t"))
        .respond_with(json_reply(200, status))
        .mount(server)
        .await;
    (backup_file, transfer_file)
}

fn honoured() -> Value {
    serde_json::from_str(&format!(
        r#"{{"status":"succeeded","result":{{"archiveId":"arch-t","fileName":"t.zip","bytes":14,
        "manifestSummary":{{"options":{{"transfer":true}},"verificationTotals":{TOTALS}}}}}}}"#
    ))
    .expect("json")
}

fn request(dir: &Path, discard: bool) -> TransferRequest {
    TransferRequest {
        tenant_id: t(T1),
        dest: std::path::absolute(dir.join("Send").join("Ozark-Ridge.clxfer")).expect("abs"),
        passphrase: Secret::new("transfer-canary-2026"),
        passphrase_again: Secret::new("transfer-canary-2026"),
        discard_payroll: discard,
        backup_dir: dir.join("Backups"),
        as_of: CalendarDate::from_ymd(2026, 11, 19).expect("date"),
    }
}

/// (method, path) of every request after sign-in, in order.
async fn calls(server: &MockServer) -> Vec<(String, String)> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .map(|r| (r.method.to_string(), r.url.path().to_string()))
        .filter(|(_, p)| {
            !p.starts_with("/api/v1/auth")
                && !p.starts_with("/api/v1/desktop")
                && p != "/api/v1/company"
        })
        .collect()
}

async fn archive_posts(server: &MockServer) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/archives"))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

fn silent() -> cl_company::ProgressSink {
    cl_company::backup::silent()
}

#[tokio::test]
async fn backup_then_discard_then_transfer_and_the_file_is_recorded() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut caps = vec![
        "tenant-archive-v1",
        "tenant-archive-transfer-v1",
        "archive-verification-totals-v1",
        "intake-settings-v1",
        "document-drafts-v1",
    ];
    caps.extend(PAYROLL_CAPS);
    let server = engine(&caps).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payroll-import/sessions"))
        .respond_with(json_reply(
            200,
            json!({"data":[{"id":DRAFT,"status":"validated"}],"total":1}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/payroll-import/sessions/{DRAFT}")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let (_, transfer_file) = mount_jobs(&server, dir.path(), honoured()).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let req = request(dir.path(), true);
    let out = make_transfer(&s, &store, &secrets, &req, &silent())
        .await
        .expect("transfer");

    // Order: backup export, then the draft discarded, then the transfer.
    let c = calls(&server).await;
    let backup_at = c
        .iter()
        .position(|(m, p)| m == "POST" && p.ends_with("/archives"))
        .expect("backup");
    let discard_at = c.iter().position(|(m, _)| m == "DELETE").expect("discard");
    let transfer_at = c
        .iter()
        .rposition(|(m, p)| m == "POST" && p.ends_with("/archives"))
        .expect("transfer");
    assert!(backup_at < discard_at && discard_at < transfer_at, "{c:?}");
    let posts = archive_posts(&server).await;
    assert_eq!(posts.len(), 2);
    assert!(posts[0].get("transfer").is_none(), "the backup is a backup");
    assert_eq!(posts[0]["passphrase"], "backup-passphrase-canary");
    let tx = &posts[1];
    assert_eq!(tx["transfer"], true);
    assert_eq!(tx["asOf"], "2026-11-19");
    assert_eq!(tx["omitIntakeSettings"], true);
    assert_eq!(tx["omitDocumentDrafts"], true);
    assert_eq!(tx["includeAttachments"], true);
    assert_eq!(tx["includeAudit"], true);
    assert_eq!(tx["passphrase"], "transfer-canary-2026");
    // (c) The host is exactly four fields: no paths, colour or history.
    let host = tx["host"].as_object().expect("host");
    let mut keys: Vec<&str> = host.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["app", "appVersion", "kind", "name"]);
    assert_eq!(host["kind"], "transfer");

    assert!(req.dest.is_file());
    assert!(!partial_path(&req.dest).exists());
    assert!(!transfer_file.exists(), "moved from the engine folder");
    assert_eq!(out.discarded_payroll, 1);
    assert_eq!(out.bytes, 14);
    let totals = out.totals.expect("totals");
    assert_eq!(totals.total_assets.to_wire(), "12500.1250");
    let list = store.load().expect("load");
    let rec = list.get(t(T1)).expect("company");
    assert_eq!(rec.transfers.len(), 1);
    assert_eq!(rec.transfers[0].sha256, sha256(b"transfer-bytes"));
    let h = rec.transfers[0].totals.as_ref().expect("history totals");
    assert_eq!(h.net_income.to_wire(), "4321.0049");
    assert_eq!(h.transactions, 1284);
    assert_eq!(rec.backups.len(), 1, "the backup taken first");
    // The settings file never holds the passphrase.
    let raw = std::fs::read_to_string(dir.path().join("settings").join("companies.json"))
        .unwrap_or_default();
    assert!(!raw.contains("transfer-canary"));
}

/// S3 / T4: without the capability nothing is sent; a result without the
/// transfer echo leaves no `.clxfer`, no history row, and the engine's
/// full archive is removed.
#[tokio::test]
async fn transfer_fails_closed() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let e = make_transfer(&s, &store, &secrets, &request(dir.path(), true), &silent())
        .await
        .expect_err("no capability");
    assert!(matches!(e, TransferError::Unsupported), "{e:?}");
    assert!(archive_posts(&server).await.is_empty(), "nothing sent");

    let server = engine(&["tenant-archive-v1", "tenant-archive-transfer-v1"]).await;
    let not_honoured: Value = serde_json::from_str(
        r#"{"status":"succeeded","result":{"archiveId":"arch-t","fileName":"t.zip"}}"#,
    )
    .expect("json");
    let (_, engine_file) = mount_jobs(&server, dir.path(), not_honoured).await;
    let s = session(&server).await;
    let req = request(dir.path(), true);
    let e = make_transfer(&s, &store, &secrets, &req, &silent())
        .await
        .expect_err("not honoured");
    assert!(
        matches!(e, TransferError::NotHonoured { left: None }),
        "{e:?}"
    );
    assert!(!engine_file.exists(), "the full archive is removed");
    assert!(!req.dest.exists() && !partial_path(&req.dest).exists());
    let list = store.load().expect("load");
    assert_eq!(list.get(t(T1)).expect("company").transfers.len(), 0);
}

/// Q-P32f: short, mismatched, or equal to the backup passphrase — nothing
/// is sent, not even the backup.
#[tokio::test]
async fn passphrase_rules_send_nothing() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", "tenant-archive-transfer-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let mut req = request(dir.path(), true);
    req.passphrase = Secret::new("short");
    req.passphrase_again = Secret::new("short");
    assert!(matches!(
        make_transfer(&s, &store, &secrets, &req, &silent()).await,
        Err(TransferError::PassphraseTooShort)
    ));
    req.passphrase = Secret::new("long enough passphrase");
    req.passphrase_again = Secret::new("long enough passphrasE");
    assert!(matches!(
        make_transfer(&s, &store, &secrets, &req, &silent()).await,
        Err(TransferError::PassphraseMismatch)
    ));
    req.passphrase = Secret::new("backup-passphrase-canary");
    req.passphrase_again = Secret::new("backup-passphrase-canary");
    assert!(matches!(
        make_transfer(&s, &store, &secrets, &req, &silent()).await,
        Err(TransferError::SameAsBackupPassphrase)
    ));
    assert_eq!(archive_posts(&server).await.len(), 0);
    // Twelve characters, counted as characters.
    let twelve = Secret::new("ééééééééééé1");
    assert!(check_passphrase(&twelve, &twelve, &secrets).is_ok());
    assert_eq!(MIN_PASSPHRASE_CHARS, 12);
}

/// Q-P30c: unposted payroll imports stop the transfer unless the owner
/// agreed to discard them (nothing sent).
#[tokio::test]
async fn payroll_drafts_need_the_owner_s_answer() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut caps = vec!["tenant-archive-v1", "tenant-archive-transfer-v1"];
    caps.extend(PAYROLL_CAPS);
    let server = engine(&caps).await;
    Mock::given(method("GET"))
        .and(path("/api/v1/payroll-import/sessions"))
        .respond_with(json_reply(
            200,
            json!({"data":[{"id":DRAFT,"status":"mapped"}],"total":1}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/document-drafts"))
        .respond_with(json_reply(200, json!({"drafts":[],"total":3})))
        .mount(&server)
        .await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let e = make_transfer(&s, &store, &secrets, &request(dir.path(), false), &silent())
        .await
        .expect_err("drafts");
    assert!(matches!(e, TransferError::PayrollDraftsWaiting(1)), "{e:?}");
    assert_eq!(archive_posts(&server).await.len(), 0);
    let p = preflight(&s).await.expect("preflight");
    assert_eq!(p.unposted_payroll, 1);
    assert!(p.capability_ok);
}

/// Q-P32k / T7: every missing or changed file is listed in one run, with
/// its name and record when the engine knows the attachment.
#[tokio::test]
async fn missing_files_are_all_listed() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", "tenant-archive-transfer-v1"]).await;
    let a1 = "8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
    Mock::given(method("GET"))
        .and(path(format!("/api/v1/attachments/{a1}")))
        .respond_with(json_reply(
            200,
            json!({"attachment":{"id":a1,"fileName":"receipt-0412.pdf","attachableType":"bill",
                   "attachableId":"9a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"}}),
        ))
        .mount(&server)
        .await;
    let failed = json!({"status":"failed","error":{"code":"FILE_MISSING","message":"x","details":{"files":[
        {"table":"attachments","rowId":a1,"reason":"FILE_MISSING"},
        {"table":"attachments","rowId":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02","reason":"FILE_SIZE_MISMATCH"},
        {"table":"attachments","rowId":"8a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03","reason":"EXTERNAL_FILE"}]}}});
    mount_jobs(&server, dir.path(), failed).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let req = request(dir.path(), true);
    match make_transfer(&s, &store, &secrets, &req, &silent()).await {
        Err(TransferError::MissingFiles(files)) => {
            assert_eq!(files.len(), 3);
            assert_eq!(files[0].file_name.as_deref(), Some("receipt-0412.pdf"));
            assert_eq!(files[0].record_type.as_deref(), Some("bill"));
            assert!(files[1].file_name.is_none());
            assert_eq!(files[2].reason, "EXTERNAL_FILE");
        }
        other => panic!("{other:?}"),
    }
    assert!(!req.dest.exists());
}

/// Q-P32u: 409 `TENANT_BUSY` is "busy", never queued.
#[tokio::test]
async fn a_busy_engine_is_busy() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1", "tenant-archive-transfer-v1"]).await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+/archives$"))
        .respond_with(json_reply(
            409,
            json!({"error":{"code":"TENANT_BUSY","message":"busy"}}),
        ))
        .mount(&server)
        .await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let e = make_transfer(&s, &store, &secrets, &request(dir.path(), true), &silent())
        .await
        .expect_err("busy");
    assert!(matches!(e, TransferError::Busy), "{e:?}");
    assert_eq!(
        cl_core::OwnerFacing::owner_message(&e).key,
        "error.company.busy"
    );
}

#[test]
fn generated_passphrases_are_five_groups_of_four() {
    let a = generate_passphrase().expect("random");
    let b = generate_passphrase().expect("random");
    let s = a.expose();
    assert_eq!(s.len(), 24);
    let groups: Vec<&str> = s.split('-').collect();
    assert_eq!(groups.len(), 5);
    assert!(groups.iter().all(|g| {
        g.len() == 4
            && g.bytes()
                .all(|c| cl_company::transfer::PASSPHRASE_ALPHABET.contains(&c))
    }));
    assert_ne!(a.expose(), b.expose());
    assert!(!s.contains(['I', 'O', '0', '1']));
    assert_eq!(
        suggested_file_name(
            "Ozark Ridge, LLC",
            CalendarDate::from_ymd(2026, 11, 19).expect("d")
        ),
        "Ozark-Ridge-LLC-2026-11-19.clxfer"
    );
}
