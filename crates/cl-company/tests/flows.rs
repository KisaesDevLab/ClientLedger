//! Lifecycle flows against a stub engine (wiremock): the order of calls,
//! what is sent, what is never sent, and what the local store ends up
//! with. The live-engine behaviour of each route is covered by the cl-api
//! contract suite (PENDING by capability for E5/U-23/U-15).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cl_api::{Client, ClientConfig, EndpointSource, FixedEndpoint, LoginOutcome, Session};
use cl_company::{
    BackupRequest, CompanyError, CompanySetup, DeleteRequest, RestoreRequest, StarterChart, backup,
    create_company, delete_company, export_all, list_companies, restore,
};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{BackupKind, BackupRecord, MemorySecrets, RecentStore, SecretStore};
use serde_json::{Value, json};
use wiremock::matchers::{body_string_contains, method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "0a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00";
const T1: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const T2: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const CO: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const CO_B: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const JWT: &str = "eyJhbGciOiJIUzI1NiJ9.eyJleHAiOjQxMDI0NDQ4MDB9.sig";

fn json(status: u16, body: Value) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(body)
}

fn tokens() -> ResponseTemplate {
    json(
        200,
        json!({"user":{"id":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"},"tokens":{"accessToken":JWT},
               "accessibleTenants":[{"tenantId":T1,"tenantName":"Ozark Ridge Landscaping LLC","role":"owner"}]}),
    )
    .insert_header("set-cookie", "kb_refresh=refresh_token_value_1234; Path=/; HttpOnly")
}

async fn engine(caps: &[&str]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/desktop/info"))
        .respond_with(json(
            200,
            json!({"mode":"desktop","initialized":true,"workspaceTenantId":WS,"capabilities":caps}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/login"))
        .respond_with(tokens())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/switch-tenant"))
        .respond_with(tokens())
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/auth/me"))
        .respond_with(json(
            200,
            json!({"accessibleTenants":[
                {"tenantId":WS,"tenantName":"workspace","role":"owner"},
                {"tenantId":T1,"tenantName":"Ozark Ridge Landscaping LLC","role":"owner"},
                {"tenantId":T2,"tenantName":"Shoal Creek Farm","role":"owner"}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            json!({"company":{"id":CO,"businessName":"Ozark Ridge Landscaping LLC","entityType":"single_member_llc","country":"US"}}),
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
        LoginOutcome::Session(s) => s,
        LoginOutcome::TfaRequired(_) => panic!("tfa"),
    }
}

async fn posts(server: &MockServer, path_prefix: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path().starts_with(path_prefix))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

async fn count(server: &MockServer, m: &str, p: &str) -> usize {
    server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .filter(|r| r.method.as_str() == m && r.url.path() == p)
        .count()
}

fn stores(dir: &Path) -> (RecentStore, MemorySecrets) {
    let store = RecentStore::in_dir(&dir.join("settings"));
    let secrets = MemorySecrets::new();
    secrets
        .set(BACKUP_PASSPHRASE, &Secret::new("backup-passphrase-canary"))
        .expect("set");
    (store, secrets)
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

fn t(s: &str) -> TenantId {
    TenantId::parse(s).expect("id")
}

fn abs(dir: &Path, name: &str) -> PathBuf {
    std::path::absolute(dir.join(name)).expect("abs")
}

async fn mount_export(server: &MockServer, archive_path: &Path, sha: &str) {
    Mock::given(method("POST"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+/archives$"))
        .respond_with(json(202, json!({"jobId":"job_1","archiveId":"arch-1"})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_1"))
        .respond_with(json(
            200,
            json!({"jobId":"job_1","kind":"export","status":"running","progress":40,"step":"tables"}),
        ))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_1"))
        .respond_with(json(
            200,
            json!({"jobId":"job_1","kind":"export","status":"succeeded","progress":100,"step":"done",
                   "result":{"archiveId":"arch-1","fileName":"arch-1.zip",
                   "path":archive_path.to_string_lossy(),"sha256":sha,"bytes":8}}),
        ))
        .mount(server)
        .await;
}

#[tokio::test]
async fn backup_moves_the_archive_verifies_it_and_records_history() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    store
        .update(|l| {
            let r = l.upsert(t(T1), "Ozark Ridge Landscaping LLC");
            r.flags.sales_tax_enabled = true;
            r.prefs.bills_due_days = 21;
            r.prefs.receipt_inbox_dir = Some(std::path::PathBuf::from("C:/Receipts"));
            r.prefs.ai.suggest_rules = true;
        })
        .expect("store");
    let engine_dir = dir.path().join("engine-archives");
    std::fs::create_dir_all(&engine_dir).expect("dir");
    let archive = abs(&engine_dir, "arch-1.zip");
    std::fs::write(&archive, b"ciphered").expect("write");
    mount_export(&server, &archive, &sha256(b"ciphered")).await;

    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = Arc::clone(&seen);
    let progress: cl_company::ProgressSink = Arc::new(move |p| seen2.lock().unwrap().push(p));
    let out = backup(
        &s,
        &store,
        &secrets,
        &BackupRequest {
            tenant_id: t(T1),
            kind: BackupKind::Backup,
            dest_dir: dir.path().join("Backups"),
            apply_retention: true,
        },
        &progress,
    )
    .await
    .expect("backup");
    assert!(out.record.path.exists(), "{:?}", out.record.path);
    assert!(!archive.exists(), "moved, not copied");
    assert_eq!(out.record.sha256, sha256(b"ciphered"));
    assert_eq!(out.record.archive_id, "arch-1");
    assert_eq!(out.bytes, 8);
    assert_eq!(out.pruned.len(), 0);
    let body = &posts(&server, "/api/v1/tenants/").await[0];
    assert_eq!(body["passphrase"], "backup-passphrase-canary");
    assert_eq!(body["includeAttachments"], true);
    assert_eq!(body["host"]["app"], "ClientLedger");
    assert_eq!(body["host"]["kind"], "backup");
    assert_eq!(body["host"]["flags"]["sales_tax_enabled"], true);
    assert_eq!(body["host"]["prefs"]["bills_due_days"], 21);
    assert!(
        body["host"]["prefs"]["receipt_inbox_dir"].is_null(),
        "a folder on this computer never travels in an archive"
    );
    assert_eq!(body["host"]["prefs"]["ai"]["usage"]["tokens"], 0);
    assert!(body["host"]["prefs"]["ai_consent"].is_null());
    assert_eq!(body["host"]["prefs"]["ai"]["suggest_rules"], true);
    // Preferences travel without secrets: the host object never carries a
    // key, a password or the passphrase (non-negotiable 11).
    let host_text = body["host"].to_string().to_ascii_lowercase();
    for word in [
        "passphrase",
        "password",
        "apikey",
        "api_key",
        "canary",
        "sk-",
    ] {
        assert!(!host_text.contains(word), "{word} in host: {host_text}");
    }
    let list = store.load().expect("load");
    let rec = list.get(t(T1)).expect("rec");
    assert_eq!(rec.backups.len(), 1);
    assert!(rec.last_backup.is_some());
    let steps = seen.lock().unwrap().clone();
    assert!(matches!(
        steps.first(),
        Some(cl_company::Progress::Starting)
    ));
    assert!(matches!(steps.last(), Some(cl_company::Progress::Done)));
}

#[tokio::test]
async fn backup_refuses_without_a_passphrase_and_a_bad_copy_is_removed() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let store = RecentStore::in_dir(&dir.path().join("settings"));
    store
        .update(|l| {
            l.upsert(t(T1), "Ozark");
        })
        .expect("store");
    let empty = MemorySecrets::new();
    let req = BackupRequest {
        tenant_id: t(T1),
        kind: BackupKind::Backup,
        dest_dir: dir.path().join("Backups"),
        apply_retention: false,
    };
    let e = backup(&s, &store, &empty, &req, &cl_company::backup::silent())
        .await
        .expect_err("no passphrase");
    assert!(matches!(e, CompanyError::NoPassphrase), "{e:?}");
    assert_eq!(
        posts(&server, "/api/v1/tenants/").await.len(),
        0,
        "nothing sent"
    );

    let (_, secrets) = stores(dir.path());
    let archive = abs(dir.path(), "arch-1.zip");
    std::fs::write(&archive, b"ciphered").expect("write");
    mount_export(&server, &archive, &"0".repeat(64)).await;
    let e = backup(&s, &store, &secrets, &req, &cl_company::backup::silent())
        .await
        .expect_err("mismatch");
    assert!(matches!(e, CompanyError::CopyMismatch), "{e:?}");
    let left = std::fs::read_dir(dir.path().join("Backups")).map_or(0, Iterator::count);
    assert_eq!(left, 0, "a mismatched copy is removed");
    assert_eq!(
        store
            .load()
            .expect("load")
            .get(t(T1))
            .expect("rec")
            .backups
            .len(),
        0
    );
}

async fn mount_import(server: &MockServer, tenant: &str, name: &str, host: Option<Value>) {
    Mock::given(method("POST"))
        .and(path("/api/v1/tenant-archives/import"))
        .respond_with(json(202, json!({"jobId":"imp_1"})))
        .mount(server)
        .await;
    let mut result = json!({"tenantId":tenant,"name":name,"scratch":false,"archiveId":"arch-9",
        "verification":{"tables":40,"rows":500,"contentEqual":true,"trialBalanceEqual":true,"files":0,
        "identity":{"matched":1,"created":0},"warnings":[]},"warnings":[]});
    if let Some(h) = host {
        result["host"] = h;
    }
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/imp_1"))
        .respond_with(json(200, json!({"jobId":"imp_1","kind":"import","status":"succeeded","progress":100,"step":"done","result":result})))
        .mount(server)
        .await;
}

async fn mount_tenants_list(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/v1/tenants"))
        .respond_with(json(
            200,
            json!([
                {"id":T1,"name":"Ozark Ridge Landscaping LLC","role":"owner","createdAt":null,"updatedAt":"2026-11-01T08:00:00.000Z"},
                {"id":T2,"name":"Ozark Ridge Landscaping LLC","role":"owner","createdAt":null,"updatedAt":"2026-11-04T08:12:00.000Z"}
            ]),
        ))
        .mount(server)
        .await;
}

fn write_archive(dir: &Path, archive_id: &str) -> PathBuf {
    let p = abs(dir, "restore-me.clbackup");
    let env = format!(
        r#"{{"format":"mybooks-tenant-archive","formatVersion":1,"archiveId":"{archive_id}","createdAt":"2026-11-04T08:12:00.000Z"}}"#
    );
    let file = std::fs::File::create(&p).expect("create");
    let mut zip = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("envelope.json", opts).expect("entry");
    zip.write_all(env.as_bytes()).expect("write");
    zip.start_file("manifest.json.enc", opts).expect("entry");
    zip.write_all(b"enc").expect("write");
    zip.finish().expect("finish");
    p
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn restore_imports_as_a_new_tenant_swaps_names_and_keeps_the_old_one() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let file = write_archive(dir.path(), "arch-9");
    let sha = cl_company::backup::sha256_file(&file).expect("sha");
    store
        .update(|l| {
            let r = l.upsert(t(T1), "Ozark Ridge Landscaping LLC");
            r.accent = cl_settings::ACCENT_PALETTE[2];
            l.record_backup(
                t(T1),
                BackupRecord {
                    path: file.clone(),
                    sha256: sha.clone(),
                    archive_id: "arch-9".into(),
                    created_at: cl_settings::recent::now(),
                    kind: BackupKind::Backup,
                },
            );
        })
        .expect("store");
    mount_import(
        &server,
        T2,
        "Ozark Ridge Landscaping LLC",
        Some(
            json!({"app":"ClientLedger","accent":"#C23B4A","flags":{"inventory_enabled":true},
            "prefs":{"bills_due_days":30,"ai":{"suggest_rules":true,"monthly_token_cap":250_000}}}),
        ),
    )
    .await;
    mount_tenants_list(&server).await;
    // A rename without expectedUpdatedAt is a 400 (the engine validates it).
    Mock::given(method("PATCH"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+$"))
        .and(body_string_contains("expectedUpdatedAt"))
        .respond_with(json(
            200,
            json!({"id":T2,"name":"x","updatedAt":"2026-11-04T08:12:00.000Z"}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+$"))
        .respond_with(json(
            400,
            json!({"error":{"message":"expectedUpdatedAt required","code":"VALIDATION_ERROR"}}),
        ))
        .mount(&server)
        .await;
    let out = restore(
        &s,
        &store,
        &secrets,
        &RestoreRequest {
            path: file.clone(),
            passphrase: None,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect("restore");
    assert_eq!(out.new_tenant, t(T2));
    assert_eq!(out.kept_previous, Some(t(T1)));
    assert!(out.names_swapped);
    assert!(out.file_known);
    assert_eq!(out.name, "Ozark Ridge Landscaping LLC");
    assert_eq!(
        out.verification
            .as_ref()
            .and_then(|v| v.trial_balance_equal),
        Some(true)
    );
    let import = &posts(&server, "/api/v1/tenant-archives/import").await[0];
    assert_eq!(import["passphrase"], "backup-passphrase-canary");
    assert!(Path::new(import["path"].as_str().unwrap()).is_absolute());
    assert_eq!(
        count(&server, "PATCH", &format!("/api/v1/tenants/{T1}")).await,
        1
    );
    assert_eq!(
        count(&server, "PATCH", &format!("/api/v1/tenants/{T2}")).await,
        1
    );
    assert_eq!(
        posts(&server, &format!("/api/v1/tenants/{T1}/delete"))
            .await
            .len(),
        0,
        "the previous company is never deleted by restore"
    );
    let list = store.load().expect("load");
    assert!(
        list.get(t(T1))
            .expect("old")
            .name
            .contains("before restore")
    );
    let new = list.get(t(T2)).expect("new");
    assert_eq!(
        new.accent,
        cl_settings::Accent([0xC2, 0x3B, 0x4A]),
        "colour comes from the archive host object"
    );
    assert!(
        new.flags.inventory_enabled,
        "flags come from the archive host object"
    );
    assert_eq!(
        new.prefs.bills_due_days, 30,
        "company preferences come from the archive host object"
    );
    assert_eq!(new.prefs.ai.monthly_token_cap, Some(250_000));
    assert!(new.prefs.ai.suggest_rules);
    assert_eq!(new.source.as_deref(), Some("restore"));
}

#[tokio::test]
async fn restore_refuses_a_changed_file_before_anything_is_sent() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let file = write_archive(dir.path(), "arch-9");
    store
        .update(|l| {
            l.upsert(t(T1), "Ozark");
            l.record_backup(
                t(T1),
                BackupRecord {
                    path: file.clone(),
                    sha256: "f".repeat(64),
                    archive_id: "arch-9".into(),
                    created_at: cl_settings::recent::now(),
                    kind: BackupKind::Backup,
                },
            );
        })
        .expect("store");
    let before = server.received_requests().await.expect("r").len();
    let e = restore(
        &s,
        &store,
        &secrets,
        &RestoreRequest {
            path: file,
            passphrase: None,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect_err("tampered");
    assert!(matches!(e, CompanyError::Tampered), "{e:?}");
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

#[tokio::test]
async fn delete_needs_the_capability_the_exact_name_and_a_fresh_backup() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    store
        .update(|l| {
            l.upsert(t(T1), "Ozark Ridge Landscaping LLC");
        })
        .expect("store");
    let req = DeleteRequest {
        tenant_id: t(T1),
        typed_name: "Ozark Ridge Landscaping LLC".into(),
        backup_dir: dir.path().join("Backups"),
        closing_password: None,
    };
    let before = server.received_requests().await.expect("r").len();
    let e = delete_company(&s, &store, &secrets, &req, &cl_company::backup::silent())
        .await
        .expect_err("no capability");
    assert!(
        matches!(e, CompanyError::Unavailable("tenant-delete-v1")),
        "{e:?}"
    );
    assert_eq!(server.received_requests().await.expect("r").len(), before);

    let server = engine(&["tenant-archive-v1", "tenant-delete-v1"]).await;
    let s = session(&server).await;
    let e = delete_company(
        &s,
        &store,
        &secrets,
        &DeleteRequest {
            typed_name: "ozark ridge".into(),
            ..req.clone()
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect_err("name");
    assert!(matches!(e, CompanyError::NameMismatch), "{e:?}");
    assert_eq!(posts(&server, "/api/v1/tenants/").await.len(), 0);

    let archive = abs(dir.path(), "arch-1.zip");
    std::fs::write(&archive, b"ciphered").expect("write");
    mount_export(&server, &archive, &sha256(b"ciphered")).await;
    Mock::given(method("POST"))
        .and(path(format!("/api/v1/tenants/{T1}/delete")))
        .respond_with(json(200, json!({"tenantId":T1,"archiveId":"arch-1"})))
        .mount(&server)
        .await;
    let out = delete_company(&s, &store, &secrets, &req, &cl_company::backup::silent())
        .await
        .expect("deleted");
    assert_eq!(out.backup.kind, BackupKind::BeforeDelete);
    assert!(out.backup.path.exists(), "the before-delete backup is kept");
    let del = posts(&server, &format!("/api/v1/tenants/{T1}/delete")).await;
    assert_eq!(del.len(), 1);
    assert_eq!(del[0]["confirmName"], "Ozark Ridge Landscaping LLC");
    assert_eq!(del[0]["archive"]["sha256"], sha256(b"ciphered"));
    let kept = store.load().expect("load");
    let tomb = kept.get(t(T1)).expect("a tombstone keeps the record");
    assert!(tomb.is_tombstone());
    assert_eq!(
        tomb.delete_receipt.as_ref().map(|r| r["archiveId"].clone()),
        Some(json!("arch-1"))
    );
    assert_eq!(
        tomb.backups.len(),
        1,
        "the before-delete backup stays in the history"
    );
    assert_eq!(kept.live().count(), 0);
    assert_eq!(del[0]["confirmName"], "Ozark Ridge Landscaping LLC");
}

#[tokio::test]
async fn export_all_zips_every_file_with_a_manifest() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    let s = session(&server).await;
    s.open_company(t(T1)).await.expect("open");
    Mock::given(method("GET"))
        .and(path("/api/v1/export/full"))
        .respond_with(json(
            200,
            json!({"files":[{"name":"accounts.csv","rowCount":2},{"name":"journal_lines.csv","rowCount":3}]}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex(r"^/api/v1/export/full/.+\.csv$"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("id,amount\n1,10.0000\n", "text/csv"))
        .mount(&server)
        .await;
    let dest = dir.path().join("out").join("ozark.zip");
    let out = export_all(&s, &dest).await.expect("export");
    assert_eq!(
        out.files,
        vec![("accounts.csv".into(), 2), ("journal_lines.csv".into(), 3)]
    );
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&dest).expect("open")).expect("zip");
    let mut names: Vec<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .map(std::borrow::Cow::into_owned)
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "README.txt",
            "accounts.csv",
            "journal_lines.csv",
            "manifest.json"
        ]
    );
    let mut m = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("manifest.json").expect("m"), &mut m)
        .expect("read");
    let v: Value = serde_json::from_str(&m).expect("json");
    assert_eq!(v["files"][1]["rowCount"], 3);
    assert_eq!(v["app"], "ClientLedger");
}

#[tokio::test]
async fn list_merges_the_engine_with_the_local_store() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    let s = session(&server).await;
    let store = RecentStore::in_dir(&dir.path().join("settings"));
    let gone = t("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a09");
    store
        .update(|l| {
            l.upsert(gone, "Deleted elsewhere");
            let r = l.upsert(t(T2), "Shoal Creek Farm (old name)");
            r.transaction_count = Some(812);
            l.touch_opened(t(T2), cl_settings::recent::now());
        })
        .expect("store");
    let list = list_companies(&s, &store).await.expect("list");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].tenant_id, t(T2), "most recently opened first");
    assert_eq!(list[0].name, "Shoal Creek Farm", "engine name wins");
    assert_eq!(list[0].transaction_count, Some(812));
    assert_ne!(list[0].accent, list[1].accent);
    assert!(list.iter().all(|c| c.tenant_id != t(WS)));
    let saved = store.load().expect("load");
    assert!(
        saved.get(gone).is_none(),
        "dropped companies leave the store"
    );
    assert!(saved.get(t(T1)).is_some(), "new companies are added");
}

#[tokio::test]
async fn create_company_runs_the_interview_steps_in_order() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    let s = session(&server).await;
    let store = RecentStore::in_dir(&dir.path().join("settings"));
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/create-tenant"))
        .respond_with(json(
            201,
            json!({"tenantId":T2,"companyId":CO,"tenantName":"New Co"}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company"))
        .respond_with(json(200, json!({"company":{"id":CO,"businessName":"New Co","legalName":"New Co LLC","entityType":"s_corp","country":"US"}})))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company/settings"))
        .respond_with(json(
            200,
            json!({"settings":{"fiscalYearStartMonth":7,"accountingMethod":"cash"}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/accounts/import"))
        .respond_with(json(201, json!({"imported":40,"updated":0,"skipped":[]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/company/setup-complete"))
        .respond_with(json(200, json!({"message":"Setup complete"})))
        .mount(&server)
        .await;
    let mut setup = CompanySetup::new("New Co");
    setup.legal_name = Some("New Co LLC".into());
    setup.entity_type = Some(cl_api::EntityType::SCorp);
    setup.fiscal_year_start_month = 7;
    setup.basis = cl_api::AccountingMethod::Cash;
    setup.chart = StarterChart::Contractor;
    setup.inventory = true;
    setup.accent = cl_settings::ACCENT_PALETTE[3];
    let made = create_company(&s, &store, &setup).await.expect("created");
    assert_eq!(made.tenant_id, t(T2));
    assert_eq!(made.accounts_imported, 40);
    assert!(!made.closing_password_set);
    let paths: Vec<String> = server
        .received_requests()
        .await
        .expect("r")
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    assert_eq!(
        paths,
        vec![
            "POST /api/v1/auth/login",
            "POST /api/v1/auth/create-tenant",
            "POST /api/v1/auth/switch-tenant",
            "PUT /api/v1/company",
            "PUT /api/v1/company/settings",
            "POST /api/v1/accounts/import",
            "POST /api/v1/company/setup-complete",
        ]
    );
    let create = &posts(&server, "/api/v1/auth/create-tenant").await[0];
    assert_eq!(create["systemAccountsOnly"], true);
    assert_eq!(create["entityType"], "s_corp");
    let import = &posts(&server, "/api/v1/accounts/import").await[0];
    let rows = import["accounts"].as_array().expect("rows");
    assert!(rows.iter().any(|r| r["name"] == "Inventory Asset"));
    assert!(rows.iter().all(|r| r["accountNumber"] != "30000"));
    let rec = store.load().expect("load");
    let rec = rec.get(t(T2)).expect("recorded");
    assert_eq!(rec.accent, cl_settings::ACCENT_PALETTE[3]);
    assert!(rec.flags.inventory_enabled);
    assert!(rec.last_opened.is_some());

    // A closing-date password without the capability is refused before
    // anything is sent.
    let before = server.received_requests().await.expect("r").len();
    setup.closing_password = Some(Secret::new("a-closing-password-12"));
    let e = create_company(&s, &store, &setup).await.expect_err("gated");
    assert!(matches!(e, CompanyError::Setup(_)), "{e:?}");
    assert_eq!(server.received_requests().await.expect("r").len(), before);
}

/// B3: a company switch in the middle of the practice-company load must
/// never send a post into the other company. The loader runs on a pinned
/// session, so after the switch its posts fail with `StaleScope` and
/// nothing carries the other tenant's company id.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn switching_mid_load_sends_nothing_into_the_other_company() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    // The first `GET /company` (the sample tenant) reports CO; every later
    // one (after the switch to T1) reports CO_B.
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            json!({"company":{"id":CO,"businessName":"Practice","country":"US"}}),
        ))
        .with_priority(1)
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            json!({"company":{"id":CO_B,"businessName":"Other","country":"US"}}),
        ))
        .with_priority(2)
        .mount(&server)
        .await;
    let s = session(&server).await;
    let store = RecentStore::in_dir(&dir.path().join("settings"));
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/create-tenant"))
        .respond_with(json(
            201,
            json!({"tenantId":T2,"companyId":CO,"tenantName":"Practice"}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            json!({"company":{"id":CO,"businessName":"Practice","country":"US"}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company/settings"))
        .respond_with(json(200, json!({"settings":{"fiscalYearStartMonth":1}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/accounts/import"))
        .respond_with(json(201, json!({"imported":40,"updated":0,"skipped":[]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/company/setup-complete"))
        .respond_with(json(200, json!({"message":"ok"})))
        .mount(&server)
        .await;
    let accounts: Vec<Value> = cl_company::Sample::load()
        .company
        .accounts
        .iter()
        .enumerate()
        .map(|(i, a)| json!({"id": format!("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e{:04x}", i), "accountNumber": a.number,
            "name": a.name, "accountType": a.account_type, "isActive": true, "isSystem": false}))
        .chain([
            json!({"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea001","accountNumber":"10200","name":"A/R","accountType":"asset","isSystem":true}),
            json!({"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea002","accountNumber":"20100","name":"A/P","accountType":"liability","isSystem":true}),
            json!({"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea003","accountNumber":"30000","name":"Opening","accountType":"equity","isSystem":true}),
        ])
        .collect();
    let total = accounts.len();
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts"))
        .respond_with(json(
            200,
            json!({"data": accounts, "total": total, "limit": 500, "offset": 0}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/contacts"))
        .respond_with(json(201, json!({"contact":{"id":"4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","contactType":"customer","displayName":"x","isActive":true}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(
            json(201, json!({"transaction":{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnType":"journal_entry","txnDate":"2026-01-01","status":"posted","lines":[]}}))
                .set_delay(std::time::Duration::from_millis(15)),
        )
        .mount(&server)
        .await;

    let loader = {
        let s = s.clone();
        let store = store.clone();
        tokio::spawn(async move {
            cl_company::sample::load_into(&s, &store, &cl_company::backup::silent()).await
        })
    };
    // Let a few posts go out, then switch companies underneath the loader.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    s.open_company(t(T1)).await.expect("switch");
    let result = loader.await.expect("join");
    let seen: Vec<String> = server
        .received_requests()
        .await
        .expect("recorded")
        .iter()
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    assert!(
        result.is_err(),
        "the load must stop after the switch: {seen:?}"
    );
    let err = result.expect_err("stopped");

    let posts: Vec<_> = server
        .received_requests()
        .await
        .expect("recorded")
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/v1/transactions")
        .collect();
    assert!(
        !posts.is_empty(),
        "some posts went out before the switch; error {err:?}; seen {seen:?}"
    );
    let into_other = posts
        .iter()
        .filter(|r| {
            r.headers
                .get("x-company-id")
                .is_some_and(|v| v.to_str().unwrap_or("") == CO_B)
        })
        .count();
    assert_eq!(into_other, 0, "a post carried the other company's id");
    assert!(posts.iter().all(|r| {
        r.headers
            .get("x-company-id")
            .is_some_and(|v| v.to_str().unwrap_or("") == CO)
    }));
}

/// The P14 story on a real load: the two waiting payments post as journal
/// entries Dr Payments Clearing (the engine's 10150) / Cr A/R, naming the
/// customer in the memo, so the engine's pending-deposits list (any posted
/// debit to Payments Clearing) shows them.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn the_sample_posts_waiting_payments_into_payments_clearing() {
    const PC: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea004";
    const AR: &str = "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea001";
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&[]).await;
    let s = session(&server).await;
    let store = RecentStore::in_dir(&dir.path().join("settings"));
    Mock::given(method("POST"))
        .and(path("/api/v1/auth/create-tenant"))
        .respond_with(json(
            201,
            json!({"tenantId":T2,"companyId":CO,"tenantName":"Practice"}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company"))
        .respond_with(json(
            200,
            json!({"company":{"id":CO,"businessName":"Practice","country":"US"}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/company/settings"))
        .respond_with(json(200, json!({"settings":{"fiscalYearStartMonth":1}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/accounts/import"))
        .respond_with(json(201, json!({"imported":40,"updated":0,"skipped":[]})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/company/setup-complete"))
        .respond_with(json(200, json!({"message":"ok"})))
        .mount(&server)
        .await;
    let accounts: Vec<Value> = cl_company::Sample::load()
        .company
        .accounts
        .iter()
        .enumerate()
        .map(|(i, a)| json!({"id": format!("2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e{:04x}", i), "accountNumber": a.number,
            "name": a.name, "accountType": a.account_type, "isActive": true, "isSystem": false}))
        .chain([
            json!({"id":AR,"accountNumber":"10200","name":"A/R","accountType":"asset","isSystem":true}),
            json!({"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea002","accountNumber":"20100","name":"A/P","accountType":"liability","isSystem":true}),
            json!({"id":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8ea003","accountNumber":"30000","name":"Opening","accountType":"equity","isSystem":true}),
            json!({"id":PC,"accountNumber":"10150","name":"Payments Clearing","accountType":"asset","isSystem":true}),
        ])
        .collect();
    let total = accounts.len();
    Mock::given(method("GET"))
        .and(path("/api/v1/accounts"))
        .respond_with(json(
            200,
            json!({"data": accounts, "total": total, "limit": 500, "offset": 0}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/contacts"))
        .respond_with(json(201, json!({"contact":{"id":"4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","contactType":"customer","displayName":"x","isActive":true}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/transactions"))
        .respond_with(json(201, json!({"transaction":{"id":"3b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","txnType":"journal_entry","txnDate":"2026-01-01","status":"posted","lines":[]}})))
        .mount(&server)
        .await;

    cl_company::sample::load_into(&s, &store, &cl_company::backup::silent())
        .await
        .expect("loaded");

    let entries = posts(&server, "/api/v1/transactions").await;
    assert_eq!(entries.len(), 1512);
    let into_clearing: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e["lines"]
                .as_array()
                .is_some_and(|ls| ls.iter().any(|l| l["accountId"] == PC))
        })
        .collect();
    let facts: Vec<(&str, &str, &str, &str, &str)> = into_clearing
        .iter()
        .map(|e| {
            (
                e["txnType"].as_str().unwrap_or_default(),
                e["txnDate"].as_str().unwrap_or_default(),
                e["lines"][0]["debit"].as_str().unwrap_or_default(),
                e["lines"][1]["accountId"].as_str().unwrap_or_default(),
                e["lines"][1]["credit"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        facts,
        [
            ("journal_entry", "2026-11-02", "1250.0000", AR, "1250.0000"),
            ("journal_entry", "2026-11-03", "842.5000", AR, "842.5000"),
        ]
    );
    for (e, customer) in into_clearing
        .iter()
        .zip(["Hollister Family Trust", "Maple Street Bakery"])
    {
        assert_eq!(e["lines"][0]["accountId"], PC);
        let memo = e["memo"].as_str().unwrap_or_default();
        assert!(memo.contains(customer), "{memo}");
    }
}

/// A stale previous company (409 STALE on the first rename) does not fail
/// the restore: the restored company keeps the archive's name and the
/// local records stay as they were.
#[tokio::test]
async fn restore_with_a_stale_previous_company_keeps_both_names() {
    let dir = tempfile::tempdir().expect("tmp");
    let server = engine(&["tenant-archive-v1"]).await;
    let s = session(&server).await;
    let (store, secrets) = stores(dir.path());
    let file = write_archive(dir.path(), "arch-9");
    let sha = cl_company::backup::sha256_file(&file).expect("sha");
    store
        .update(|l| {
            l.upsert(t(T1), "Ozark Ridge Landscaping LLC");
            l.record_backup(
                t(T1),
                BackupRecord {
                    path: file.clone(),
                    sha256: sha,
                    archive_id: "arch-9".into(),
                    created_at: cl_settings::recent::now(),
                    kind: BackupKind::Backup,
                },
            );
        })
        .expect("store");
    mount_import(&server, T2, "Ozark Ridge Landscaping LLC", None).await;
    mount_tenants_list(&server).await;
    Mock::given(method("PATCH"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+$"))
        .respond_with(json(
            409,
            json!({"error":{"message":"changed","code":"STALE","updatedAt":"2026-11-04T09:00:00.000Z"}}),
        ))
        .mount(&server)
        .await;
    let out = restore(
        &s,
        &store,
        &secrets,
        &RestoreRequest {
            path: file,
            passphrase: None,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect("the restore itself succeeded");
    assert!(!out.names_swapped);
    assert_eq!(out.kept_previous, Some(t(T1)));
    assert_eq!(out.name, "Ozark Ridge Landscaping LLC");
    assert_eq!(
        count(&server, "PATCH", &format!("/api/v1/tenants/{T1}")).await,
        1
    );
    assert_eq!(
        count(&server, "PATCH", &format!("/api/v1/tenants/{T2}")).await,
        0
    );
    let list = store.load().expect("load");
    assert_eq!(
        list.get(t(T1)).expect("old").name,
        "Ozark Ridge Landscaping LLC"
    );
    assert!(
        list.get(t(T2)).is_some(),
        "the restored company is recorded"
    );
}
