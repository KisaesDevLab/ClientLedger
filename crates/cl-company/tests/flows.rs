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
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WS: &str = "0a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a00";
const T1: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
const T2: &str = "1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02";
const CO: &str = "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";
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
            json!({"status":"running","progress":40,"step":"tables"}),
        ))
        .up_to_n_times(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/tenant-archives/jobs/job_1"))
        .respond_with(json(
            200,
            json!({"status":"done","progress":100,"result":{"archiveId":"arch-1","fileName":"arch-1.zip",
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
    Mock::given(method("POST"))
        .and(path("/api/v1/tenant-archives/import"))
        .respond_with(json(
            201,
            json!({"tenantId":T2,"name":"Ozark Ridge Landscaping LLC","scratch":false,
                   "verification":{"tables":40,"contentEqual":true,"trialBalanceEqual":true,"files":0}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path_regex(r"^/api/v1/tenants/[^/]+$"))
        .respond_with(json(200, json!({"tenant":{"id":T2,"name":"x"}})))
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
        cl_settings::ACCENT_PALETTE[2],
        "colour carried over"
    );
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
    assert!(store.load().expect("load").get(t(T1)).is_none());
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
