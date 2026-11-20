//! QA-1 against the real engine and the bundled `PostgreSQL`: an
//! attachment whose stored file is gone stops File › Back Up Company and
//! the backup Send to My Accountant takes first. The engine lists every
//! missing file (`FILE_MISSING`, `details.files[]`), and the desktop shows
//! that list (`CompanyError::MissingFiles`, `TransferError::MissingFiles`)
//! with each file's name and record, never "try again".
//!
//! The probe prints the failed backup job's code and details (synthetic
//! data only). Without the engine executable or the `PostgreSQL` binaries
//! it prints `SKIPPED` and passes, unless `CL_API_REQUIRE=1`. Loopback
//! traffic only.
#![allow(clippy::too_many_lines, reason = "one linear scenario")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cl_api::{
    ArchiveRequest, AttachTo, Client, ClientConfig, EndpointSource, EndpointState, EngineEndpoint,
    IdempotencyKey, JobState, LoginOutcome,
};
use cl_company::transfer::{TransferError, TransferRequest, make_transfer};
use cl_company::{BackupRequest, CompanyError};
use cl_core::{CalendarDate, Secret};
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{MemorySecrets, RecentStore, SecretStore};

const OWNER: &str = "missing-owner@example.test";
const BACKUP_PASS: &str = "missing-files-backup-pass";

fn require() -> bool {
    std::env::var("CL_API_REQUIRE").as_deref() == Ok("1")
}

fn skip(why: &str) {
    assert!(!require(), "CL_API_REQUIRE=1 but {why}");
    eprintln!("SKIPPED: {why} (CI's engine job sets CL_API_REQUIRE=1)");
}

fn engine_exe() -> Option<PathBuf> {
    if let Some(p) = cl_host::find_engine_exe(None) {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let target = exe
        .ancestors()
        .find(|d| d.file_name().is_some_and(|n| n == "target"))?
        .to_path_buf();
    std::fs::read_dir(target.join("engine"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(cl_host::engine_exe_name()))
        .find(|p| p.is_file())
}

struct Source(Arc<Supervisor>);

impl EndpointSource for Source {
    fn current(&self) -> EndpointState {
        let snap = self.0.snapshot();
        match (snap.status.state, snap.endpoint) {
            (HostState::Running, Some(ep)) => EndpointState::Ready(EngineEndpoint {
                port: ep.port,
                token: Secret::new(ep.token.expose().to_string()),
                generation: ep.generation,
            }),
            (HostState::Running | HostState::Restarting { .. }, _) => EndpointState::Restarting,
            (HostState::Starting, _) => EndpointState::Starting,
            (HostState::Fault(_) | HostState::Stopping | HostState::Stopped, _) => {
                EndpointState::Down { permanent: true }
            }
        }
    }
}

fn today() -> CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    CalendarDate::from_ymd(1970, 1, 1)
        .and_then(|d| d.add_days(i64::try_from(days).expect("days")))
        .expect("today")
}

/// A small synthetic PDF carrying `marker`, so its stored copy can be
/// found on disk.
fn pdf(marker: &str) -> Vec<u8> {
    format!(
        "%PDF-1.4\n% {marker}\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF\n"
    )
    .into_bytes()
}

/// Every file under `dir` whose bytes are exactly `want` (never a
/// database page that merely mentions the file name).
fn find_stored(dir: &Path, want: &[u8], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            find_stored(&p, want, out);
        } else if std::fs::metadata(&p).is_ok_and(|m| m.len() == want.len() as u64)
            && std::fs::read(&p).is_ok_and(|b| b == want)
        {
            out.push(p);
        }
    }
}

async fn scenario(client: &Client, password: &Secret, scratch: &Path) {
    let s = match client.login(OWNER, password).await {
        Ok(LoginOutcome::Session(s)) => s,
        other => panic!("login: {other:?}"),
    };
    let store = RecentStore::in_dir(&scratch.join("settings"));
    let secrets = MemorySecrets::new();
    secrets
        .set(BACKUP_PASSPHRASE, &Secret::new(BACKUP_PASS))
        .expect("keychain");
    let created = cl_company::sample::load_into(
        &s,
        &store,
        &cl_company::backup::silent(),
        &cl_company::Statistics::none(),
    )
    .await
    .expect("the practice company loads");
    let tenant = created.tenant_id;

    // Two expenses, each with a file.
    let list = s
        .reports()
        .transaction_list_exact(None, None)
        .await
        .expect("transaction list");
    let mut bills: Vec<_> = list
        .rows
        .iter()
        .filter(|r| r.txn_type == "expense")
        .map(|r| r.id)
        .collect();
    bills.sort_by_key(ToString::to_string);
    bills.dedup();
    let mut kinds: Vec<&str> = list.rows.iter().map(|r| r.txn_type.as_str()).collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert!(
        bills.len() >= 2,
        "the practice company has expenses: {} rows, types {kinds:?}",
        list.rows.len()
    );
    let markers = ["qa1-marker-one-7f3a", "qa1-marker-two-9c4e"];
    let mut ids = Vec::new();
    for (bill, marker) in bills.iter().zip(markers) {
        let a = s
            .attachments()
            .upload(
                AttachTo::Expense(*bill),
                &format!("{marker}.pdf"),
                &pdf(marker),
                IdempotencyKey::new(),
            )
            .await
            .expect("upload");
        ids.push((a.id.to_string(), bill.to_string(), format!("{marker}.pdf")));
    }

    // Take both stored files away.
    let data = scratch.join("data");
    for marker in markers {
        let mut found = Vec::new();
        find_stored(&data, &pdf(marker), &mut found);
        assert_eq!(found.len(), 1, "one stored copy of {marker}");
        std::fs::remove_file(&found[0]).expect("remove the stored file");
    }

    // Probe: what the engine says for an ordinary desktop backup.
    let job = s
        .tenants()
        .export_archive(
            tenant,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: false,
                transfer: false,
                as_of: None,
                host: serde_json::json!({}),
                passphrase: Secret::new(BACKUP_PASS),
            },
        )
        .await
        .expect("export job");
    let status = cl_company::backup::poll_job(&s, &job.job_id, &cl_company::backup::silent())
        .await
        .expect("job polled");
    let err = status.error.clone().expect("the backup failed");
    eprintln!(
        "PROBE backup job: status={:?} code={} details={}",
        status.status,
        err.code,
        err.details.clone().unwrap_or_default()
    );
    assert_eq!(status.status, JobState::Failed);
    assert_eq!(err.code, "FILE_MISSING");
    let listed = err.missing_files();
    assert_eq!(listed.len(), 2, "both files listed in one run");

    // File › Back Up Company.
    let backup_err = cl_company::backup(
        &s,
        &store,
        &secrets,
        &BackupRequest {
            tenant_id: tenant,
            kind: cl_settings::BackupKind::Backup,
            dest_dir: scratch.join("backups"),
            apply_retention: true,
            protected: None,
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect_err("the backup stops");
    let CompanyError::MissingFiles(files) = &backup_err else {
        panic!("{backup_err:?}");
    };
    assert_eq!(files.len(), 2);
    for (id, bill, name) in &ids {
        let f = files
            .iter()
            .find(|f| &f.attachment_id == id)
            .expect("listed");
        assert_eq!(f.reason, "FILE_MISSING");
        assert_eq!(f.file_name.as_deref(), Some(name.as_str()));
        assert_eq!(f.record_id.as_deref(), Some(bill.as_str()));
        assert_eq!(f.record_type.as_deref(), Some("expense"));
    }
    let m = cl_core::OwnerFacing::owner_message(&backup_err);
    assert_eq!(m.key, "error.backup.missing_files");
    assert_ne!(m.suggestion_key, Some("fix.api.try_again_soon"));

    // Send to My Accountant: the backup taken first stops with the same list.
    let dest = std::path::absolute(scratch.join("send").join("Missing.clxfer")).expect("abs");
    let e = make_transfer(
        &s,
        &store,
        &secrets,
        &TransferRequest {
            tenant_id: tenant,
            dest: dest.clone(),
            passphrase: Secret::new("missing-live-transfer-pass"),
            passphrase_again: Secret::new("missing-live-transfer-pass"),
            discard_payroll: true,
            backup_dir: scratch.join("backups"),
            as_of: today(),
        },
        &cl_company::backup::silent(),
    )
    .await
    .expect_err("the transfer stops");
    assert!(e.discarded().is_none(), "nothing discarded");
    match e.kind() {
        TransferError::MissingFiles(f) => assert_eq!(f.len(), 2),
        other => panic!("{other:?}"),
    }
    assert!(!dest.exists(), "no transfer file");
    let rec = store.load().expect("store");
    let rec = rec.get(tenant).expect("company");
    assert!(rec.transfers.is_empty(), "nothing in Sent before");

    let _ = s.logout().await;
}

#[test]
fn backup_lists_missing_attachment_files() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("scratch");
    let paths = cl_host::AppPaths::at(root.join("data")).expect("paths");
    let mut cfg = cl_host::in_memory_stores(HostConfig::new(paths, engine));
    cfg.pg_binaries = Some(pg);
    cfg.policy.ready_timeout = Duration::from_mins(3);
    cfg.policy.health_ready_timeout = Duration::from_mins(2);
    cfg.policy.health_interval = Duration::from_secs(1);
    let guard = match cl_host::acquire(&cfg.paths).expect("lock") {
        cl_host::Acquired::First(g) => g,
        cl_host::Acquired::Second => panic!("instance lock held"),
    };
    let sup = Arc::new(Supervisor::start(cfg, guard).expect("start"));
    let password = Secret::new(format!(
        "Cl-{}",
        &IdempotencyKey::new().as_header().replace('-', "")[..20]
    ));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        sup.initialize_engine(
            &AdminAccount {
                email: OWNER.into(),
                display_name: "Missing Owner".into(),
                password: cl_host::Secret::new(password.expose().to_string()),
            },
            None,
        )
        .expect("initialize");
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("runtime");
        {
            let source: Arc<dyn EndpointSource> = Arc::new(Source(Arc::clone(&sup)));
            let client = Client::new(source, ClientConfig::default()).expect("client");
            rt.block_on(async {
                tokio::time::timeout(Duration::from_mins(8), async {
                    client.desktop_info().await.expect("info");
                    scenario(&client, &password, &root).await;
                })
                .await
                .expect("within eight minutes");
            });
        }
        rt.shutdown_timeout(Duration::from_secs(5));
    }));
    match Arc::try_unwrap(sup) {
        Ok(s) => s.stop(),
        Err(shared) => drop(shared),
    }
    if std::env::var("CL_API_KEEP").as_deref() != Ok("1") {
        for _ in 0..40 {
            if std::fs::remove_dir_all(&root).is_ok() || !root.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    if let Err(p) = outcome {
        std::panic::resume_unwind(p);
    }
}
