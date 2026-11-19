//! Send to My Accountant against the real engine and the bundled
//! `PostgreSQL` (P32 D7, engine pin `dd11f2b4`, U-282 to U-284).
//!
//! `transfer_roundtrip_seed` loads the synthetic practice company, takes
//! the check-totals preview, makes a real `.clxfer` with `make_transfer`
//! (backup first), and checks:
//! - the engine honoured transfer mode and the file's totals equal the
//!   preview (money and transaction counts; the backup taken first adds
//!   its own audit rows, so audit counts are not compared);
//! - the raw file holds none of the planted canaries (secrets scan (d));
//! - desktop Restore refuses the `.clxfer` by extension (Q-P32n), and the
//!   engine refuses it renamed to `.clbackup` (`TRANSFER_NOT_A_BACKUP`);
//! - a format 2 backup made by this engine restores (R7);
//! - "Sent before" records the file with its totals.
//!
//! With `CL_TRANSFER_OUT` set (`cargo xtask transfer-roundtrip`) the file
//! goes there as `roundtrip.clxfer` with `expected.json` beside it, for the
//! engine's server-mode import. Without the engine executable or the
//! `PostgreSQL` binaries it prints `SKIPPED` and passes, unless
//! `CL_API_REQUIRE=1`. Synthetic data only; loopback traffic only.
#![allow(clippy::too_many_lines, reason = "one linear scenario")]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cl_api::{
    Client, ClientConfig, EndpointSource, EndpointState, EngineEndpoint, ImportRequest,
    LoginOutcome, VerificationTotals,
};
use cl_company::transfer::{TransferRequest, make_transfer};
use cl_company::{CompanyError, RestoreRequest};
use cl_core::{CalendarDate, Secret};
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{MemorySecrets, RecentStore, SecretStore};

const OWNER: &str = "transfer-owner@example.test";
const BACKUP_CANARY: &str = "backup-canary-passphrase-2026";

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

/// Today (UTC): never after the engine's `CURRENT_DATE` + 1.
fn today() -> CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    CalendarDate::from_ymd(1970, 1, 1)
        .and_then(|d| d.add_days(i64::try_from(days).expect("days")))
        .expect("today")
}

/// The figures the engine's import must reproduce, as decimal strings.
fn expected_json(t: &VerificationTotals, passphrase_is_synthetic: bool) -> serde_json::Value {
    serde_json::json!({
        "definition": t.definition,
        "asOf": t.as_of.to_wire(),
        "synthetic": passphrase_is_synthetic,
        "companies": t.companies.iter().map(|c| serde_json::json!({
            "totalAssets": c.total_assets.to_wire(),
            "totalLiabilities": c.total_liabilities.to_wire(),
            "totalEquity": c.total_equity.to_wire(),
            "netIncome": c.net_income.amount.to_wire(),
            "accountsReceivable": c.accounts_receivable.to_wire(),
            "accountsPayable": c.accounts_payable.to_wire(),
            "trialBalanceDebits": c.trial_balance.debits.to_wire(),
            "trialBalanceCredits": c.trial_balance.credits.to_wire(),
        })).collect::<Vec<_>>(),
        "counts": {
            "accounts": t.counts.accounts,
            "clients": t.counts.clients,
            "vendors": t.counts.vendors,
            "items": t.counts.items,
            "transactions": t.counts.transactions,
            "postedTransactions": t.counts.posted_transactions,
            "attachments": t.counts.attachments,
            "attachmentBytes": t.counts.attachment_bytes,
            "auditEntries": t.counts.audit_entries,
        },
    })
}

fn same_money(a: &VerificationTotals, b: &VerificationTotals) -> bool {
    a.total_assets == b.total_assets
        && a.total_liabilities == b.total_liabilities
        && a.total_equity == b.total_equity
        && a.net_income == b.net_income
        && a.accounts_receivable == b.accounts_receivable
        && a.accounts_payable == b.accounts_payable
        && a.trial_balance == b.trial_balance
        && a.counts.transactions == b.counts.transactions
        && a.counts.accounts == b.counts.accounts
        && a.counts.clients == b.counts.clients
        && a.counts.vendors == b.counts.vendors
}

async fn scenario(client: &Client, password: &Secret, scratch: &Path) {
    let s = match client.login(OWNER, password).await {
        Ok(LoginOutcome::Session(s)) => s,
        other => panic!("login: {other:?}"),
    };
    let caps = client.capabilities();
    for c in [
        cl_api::Capabilities::TENANT_ARCHIVE_V2,
        cl_api::Capabilities::ARCHIVE_VERIFICATION_TOTALS,
        cl_api::Capabilities::TENANT_ARCHIVE_TRANSFER,
    ] {
        assert!(caps.has(c), "the pinned engine has {c}");
    }
    let store = RecentStore::in_dir(&scratch.join("settings"));
    let secrets = MemorySecrets::new();
    secrets
        .set(BACKUP_PASSPHRASE, &Secret::new(BACKUP_CANARY))
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
    let as_of = today();

    // The preview, with the file's own options.
    let preview = s
        .tenants()
        .verification_totals(tenant, as_of, true)
        .await
        .expect("preview");
    assert_eq!(preview.definition, cl_api::TOTALS_DEFINITION);
    assert_eq!(preview.companies.len(), 1, "one company");
    assert!(preview.counts.transactions > 0, "the sample has entries");
    assert_eq!(
        preview.total_assets.as_decimal(),
        preview.total_liabilities.as_decimal() + preview.total_equity.as_decimal(),
        "A = L + E exactly (vt1)"
    );

    // The transfer file (the path CI hands on, or scratch).
    let out = std::env::var_os("CL_TRANSFER_OUT").map(PathBuf::from);
    let dest = out.as_ref().map_or_else(
        || scratch.join("send").join("Practice.clxfer"),
        |d| d.join("roundtrip.clxfer"),
    );
    let pass = std::env::var("CL_TRANSFER_PASSPHRASE")
        .unwrap_or_else(|_| "live-synthetic-transfer-pass".into());
    let req = TransferRequest {
        tenant_id: tenant,
        dest: std::path::absolute(&dest).expect("abs"),
        passphrase: Secret::new(pass.clone()),
        passphrase_again: Secret::new(pass.clone()),
        discard_payroll: true,
        backup_dir: scratch.join("backups"),
        as_of,
    };
    let done = make_transfer(&s, &store, &secrets, &req, &cl_company::backup::silent())
        .await
        .expect("the transfer file is made");
    assert!(req.dest.is_file());
    let file_totals = done.totals.clone().expect("the file carries totals");
    assert!(
        same_money(&preview, &file_totals),
        "a quiet company's preview equals the file's totals"
    );

    // (d) No canary in the raw bytes: the backup passphrase, the owner's
    // sign-in, the transfer passphrase, the scratch path.
    let bytes = std::fs::read(&req.dest).expect("read");
    let text = String::from_utf8_lossy(&bytes);
    for canary in [
        BACKUP_CANARY,
        OWNER,
        pass.as_str(),
        &scratch.display().to_string(),
    ] {
        assert!(!text.contains(canary), "a canary in the transfer file");
    }

    // Sent before.
    let list = store.load().expect("store");
    let rec = list.get(tenant).expect("company");
    assert_eq!(rec.transfers.len(), 1);
    assert_eq!(
        rec.transfers[0].totals.as_ref().map(|t| t.total_assets),
        Some(file_totals.total_assets)
    );
    assert_eq!(rec.backups.len(), 1, "the backup taken first");

    if let Some(dir) = &out {
        std::fs::write(
            dir.join("expected.json"),
            serde_json::to_vec_pretty(&expected_json(&file_totals, true)).expect("json"),
        )
        .expect("expected.json");
    }

    // A format 2 backup of this engine.
    let backup_file = rec.backups[0].path.clone();
    let info = cl_company::inspect_archive(&backup_file, &list).expect("inspect");
    assert_eq!(info.format_version, 2, "this engine writes format 2");

    // Restore refuses the transfer file: by extension, before sending...
    cl_company::close_company(&s).await.expect("close");
    let stats = cl_company::Statistics::none();
    let refused = cl_company::restore(
        &s,
        &store,
        &secrets,
        &RestoreRequest {
            path: req.dest.clone(),
            passphrase: Some(Secret::new(pass.clone())),
        },
        &cl_company::backup::silent(),
        &stats,
    )
    .await
    .expect_err("a .clxfer is not a backup");
    assert!(
        matches!(refused, CompanyError::TransferNotABackup),
        "{refused:?}"
    );
    // ...and renamed, the engine refuses it in desktop mode.
    let renamed = scratch.join("renamed.clbackup");
    std::fs::copy(&req.dest, &renamed).expect("copy");
    let job = s
        .tenants()
        .import_archive(&ImportRequest {
            path: std::path::absolute(&renamed).expect("abs"),
            passphrase: Secret::new(pass.clone()),
            scratch: false,
            reset_document_drafts: false,
        })
        .await
        .expect("import job");
    let failed = cl_company::wait_for_job(&s, &job.job_id, &cl_company::backup::silent())
        .await
        .expect_err("refused");
    match &failed {
        CompanyError::JobFailed { code } => assert_eq!(code, "TRANSFER_NOT_A_BACKUP"),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        cl_core::OwnerFacing::owner_message(&failed).key,
        "error.company.transfer_not_backup"
    );

    // The format 2 backup restores.
    let restored = cl_company::restore(
        &s,
        &store,
        &secrets,
        &RestoreRequest {
            path: backup_file,
            passphrase: None,
        },
        &cl_company::backup::silent(),
        &stats,
    )
    .await
    .expect("a format 2 backup restores");
    let after = store.load().expect("store");
    assert_eq!(
        after.get(restored.new_tenant).map(|c| c.transfers.len()),
        Some(1),
        "Sent before survives the restore"
    );

    let _ = s.logout().await;
}

#[test]
fn transfer_roundtrip_seed() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-transfer-{}", std::process::id()));
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
        &cl_api::IdempotencyKey::new().as_header().replace('-', "")[..20]
    ));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        sup.initialize_engine(
            &AdminAccount {
                email: OWNER.into(),
                display_name: "Transfer Owner".into(),
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
