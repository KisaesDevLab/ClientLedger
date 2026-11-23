//! P34 acceptance against the real engine and the bundled `PostgreSQL`,
//! one engine start for both (they share the company):
//! - **Test restore reports pass/fail with TB comparison** (U-296): a
//!   fresh backup passes with the trial balance equal; a cut-short copy is
//!   recorded as failed with a reason; no company is created.
//! - **Verify Data finds and fixes seeded faults after a backup** (U-12):
//!   a cached account balance is drifted in SQL; a full check finds it;
//!   `fix_after_backup` writes a verified before-fix backup first; the
//!   check after it finds nothing fixable.
//!
//! The pinned engine (f52b64ae, engine PRs #102 and #103) carries both
//! capabilities; their absence fails. Without the engine or `PostgreSQL`
//! it skips as the other live tests do. Loopback only; synthetic data.
#![allow(clippy::too_many_lines, reason = "one linear scenario")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cl_api::{
    Capabilities, Client, ClientConfig, EndpointSource, EndpointState, EngineEndpoint,
    IdempotencyKey, IntegrityLevel, LoginOutcome,
};
use cl_company::BackupRequest;
use cl_company::health::{FixRequest, fix_after_backup, run_check};
use cl_company::verify::test_restore;
use cl_core::Secret;
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{BackupKind, BackupTrigger, MemorySecrets, RecentStore, SecretStore};

const OWNER: &str = "verify-owner@example.test";
const BACKUP_PASS: &str = "verify-live-backup-pass";

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
                clock_offset_ms: 0,
            }),
            (HostState::Running | HostState::Restarting { .. }, _) => EndpointState::Restarting,
            (HostState::Starting, _) => EndpointState::Starting,
            (HostState::Fault(_) | HostState::Stopping | HostState::Stopped, _) => {
                EndpointState::Down { permanent: true }
            }
        }
    }
}

async fn scenario(sup: &Supervisor, client: &Client, password: &Secret, scratch: &Path) {
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
    .expect("a company loads");
    let tenant = created.tenant_id;
    let backups = std::path::absolute(scratch.join("backups")).expect("abs");
    let caps = s.client().capabilities();

    // --- Test restore -------------------------------------------------------
    assert!(
        caps.has(Capabilities::TENANT_ARCHIVE_TEST_RESTORE),
        "the pinned engine (f52b64ae) advertises tenant-archive-test-restore-v1"
    );
    {
        let tenants_before = s.tenants().list().await.expect("tenants").len();
        let made = cl_company::backup(
            &s,
            &store,
            &secrets,
            &BackupRequest {
                tenant_id: tenant,
                kind: BackupKind::Backup,
                dest_dir: backups.clone(),
                apply_retention: false,
                protected: None,
            },
            &cl_company::backup::silent(),
        )
        .await
        .expect("backup");
        let out = test_restore(
            &s,
            &store,
            &secrets,
            &made.record.path,
            &cl_company::backup::silent(),
        )
        .await
        .expect("tested");
        assert!(out.test.passed, "{:?} {:?}", out.test.reason, out.failures);
        let tb = out.test.tb.as_ref().expect("trial balance compared");
        assert!(tb.rows_equal && tb.totals_equal);
        assert_eq!(tb.expected_debit, tb.actual_debit, "the engine's figures");
        assert!(out.recorded);
        // A copy cut short: a failed test with a reason, not an error.
        let bytes = std::fs::read(&made.record.path).expect("read");
        let cut = backups.join("cut-short.clbackup");
        std::fs::write(&cut, &bytes[..bytes.len() / 2]).expect("write");
        let bad = test_restore(&s, &store, &secrets, &cut, &cl_company::backup::silent())
            .await
            .expect("a result");
        assert!(!bad.test.passed);
        assert!(bad.test.reason.is_some());
        assert_eq!(
            s.tenants().list().await.expect("tenants").len(),
            tenants_before,
            "no company was created"
        );
    }

    // --- Verify Data ----------------------------------------------------------
    assert!(
        caps.has(Capabilities::INTEGRITY),
        "the pinned engine (f52b64ae) advertises integrity-v1"
    );
    {
        s.open_company(tenant).await.expect("open");
        // Drift one account's cached balance (its entries stay as they are).
        sup.exec_sql_for_tests(&format!(
            "ALTER TABLE accounts DISABLE TRIGGER USER;
             UPDATE accounts SET balance = balance + 25.0000 WHERE id = (
               SELECT id FROM accounts WHERE tenant_id = '{tenant}' AND is_active
               ORDER BY account_number NULLS LAST LIMIT 1);
             ALTER TABLE accounts ENABLE TRIGGER USER;"
        ))
        .expect("seed the drift");
        let run = run_check(&s, IntegrityLevel::Full).await.expect("check");
        let ids: Vec<_> = run
            .findings
            .iter()
            .filter(|f| f.code == "ACCOUNT_BALANCE_DRIFT")
            .map(|f| f.id.clone())
            .collect();
        assert_eq!(ids.len(), 1, "the drift is found: {:?}", run.checks);
        let out = fix_after_backup(
            cl_company::backup::GATE.lock().await,
            &s,
            &store,
            &secrets,
            &FixRequest {
                tenant_id: tenant,
                backup_dir: &backups,
                run: &run,
                finding_ids: &ids,
            },
            &cl_company::backup::silent(),
        )
        .await
        .expect("fixed");
        assert_eq!(out.backup.trigger, Some(BackupTrigger::BeforeFix));
        assert_eq!(
            cl_company::backup::sha256_file(&out.backup.path).expect("hash"),
            out.backup.sha256,
            "a verified file came first"
        );
        assert_eq!(out.result.fixed.len(), 1);
        let after = out.recheck.expect("rechecked");
        assert_eq!(
            after
                .findings
                .iter()
                .filter(|f| f.code == "ACCOUNT_BALANCE_DRIFT")
                .count(),
            0
        );
    }
    let _ = s.logout().await;
}

#[test]
fn test_restore_and_verify_data_against_the_engine() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-verify-{}", std::process::id()));
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
                display_name: "Verify Owner".into(),
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
                tokio::time::timeout(Duration::from_mins(10), async {
                    client.desktop_info().await.expect("info");
                    scenario(&sup, &client, &password, &root).await;
                })
                .await
                .expect("within ten minutes");
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
