//! P34 acceptance "scheduled backups run while open; missed ones run at
//! next launch", against the real engine and the bundled `PostgreSQL`, in
//! simulated time (an injected `Clock`).
//!
//! 1. The app is open and signed in when a slot comes: one tick writes a
//!    verified backup into the schedule's folder, records it (trigger
//!    `scheduled`) and applies the schedule's retention.
//! 2. A new runner (the next launch) whose last run is before a slot it
//!    slept through runs exactly once, 2 minutes after sign-in (trigger
//!    `missed`), never one per missed slot.
//!
//! Without the engine executable or the `PostgreSQL` binaries it prints
//! `SKIPPED` and passes, unless `CL_API_REQUIRE=1`. Loopback only.
#![allow(clippy::too_many_lines, reason = "one linear scenario")]

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::NaiveDateTime;
use cl_api::{
    Client, ClientConfig, EndpointSource, EndpointState, EngineEndpoint, IdempotencyKey,
    LoginOutcome, Session,
};
use cl_company::schedule::{
    Clock, LocalZone, Resolved, Runner, RunnerEnv, RunnerHost, run_loop, tick,
};
use cl_core::{Secret, Timestamp};
use cl_host::{AdminAccount, HostConfig, HostState, Supervisor};
use cl_settings::secrets::BACKUP_PASSPHRASE;
use cl_settings::{
    BackupKind, BackupRecord, BackupSchedule, BackupTrigger, Frequency, MemorySecrets, RecentStore,
    RunOutcome, SecretStore, TimeOfDay,
};

const OWNER: &str = "schedule-owner@example.test";
const BACKUP_PASS: &str = "schedule-live-backup-pass";

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

fn utc(s: &str) -> Timestamp {
    Timestamp::parse_wire(s).expect("ts")
}

/// Simulated time: `sleep` moves the clock instead of waiting.
struct SimClock(Mutex<i64>);

impl SimClock {
    fn at(t: Timestamp) -> SimClock {
        SimClock(Mutex::new(t.unix_millis()))
    }

    fn set(&self, t: Timestamp) {
        *self.0.lock().expect("clock") = t.unix_millis();
    }
}

impl Clock for SimClock {
    fn now(&self) -> Timestamp {
        Timestamp::from_unix_millis(*self.0.lock().expect("clock")).expect("ts")
    }

    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        *self.0.lock().expect("clock") += i64::try_from(d.as_millis()).expect("ms");
        Box::pin(tokio::task::yield_now())
    }
}

/// UTC as the local zone (the DST rules have their own unit tests).
struct Utc;

impl LocalZone for Utc {
    fn to_local(&self, t: Timestamp) -> NaiveDateTime {
        t.as_utc().naive_utc()
    }

    fn instants(&self, local: NaiveDateTime) -> Resolved {
        Timestamp::from_unix_millis(local.and_utc().timestamp_millis())
            .map_or(Resolved::Gap, Resolved::One)
    }
}

struct Host {
    env: Mutex<RunnerEnv>,
    session: Session,
    store: RecentStore,
    secrets: Arc<dyn SecretStore>,
}

impl RunnerHost for Host {
    fn env(&self) -> RunnerEnv {
        *self.env.lock().expect("env")
    }

    fn session(&self) -> Option<Session> {
        Some(self.session.clone())
    }

    fn store(&self) -> RecentStore {
        self.store.clone()
    }

    fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
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
    .expect("a company loads");
    let tenant = created.tenant_id;
    let folder = std::path::absolute(scratch.join("scheduled")).expect("abs");
    std::fs::create_dir_all(&folder).expect("folder");

    // Three older scheduled backups in the folder and the history.
    let olds = [
        "2026-08-01T18:00:00.000Z",
        "2026-09-01T18:00:00.000Z",
        "2026-10-01T18:00:00.000Z",
    ];
    store
        .update(|l| {
            let c = l.get_mut(tenant).expect("company");
            // The practice company never runs on a schedule; this test
            // uses its books as an ordinary company's.
            c.is_sample = false;
            c.schedule = Some(BackupSchedule {
                enabled: true,
                frequency: Frequency::Daily,
                at: TimeOfDay::new(18, 0).expect("t"),
                folder: folder.clone(),
                keep_last: 2,
                keep_months: 0,
                test_each: false,
                since: Some(utc("2026-11-01T00:00:00.000Z")),
            });
            for (i, at) in olds.iter().enumerate() {
                let path = folder.join(format!("Old-{i}.clbackup"));
                std::fs::write(&path, b"old").expect("old file");
                c.backups.push(BackupRecord {
                    path,
                    sha256: format!("{i:064}"),
                    archive_id: format!("old-{i}"),
                    created_at: utc(at),
                    kind: BackupKind::Backup,
                    trigger: Some(BackupTrigger::Scheduled),
                    tested: None,
                });
            }
        })
        .expect("store");

    let host = Arc::new(Host {
        env: Mutex::new(RunnerEnv {
            signed_in_since: Some(utc("2026-11-10T17:00:00.000Z")),
            ..RunnerEnv::default()
        }),
        session: s.clone(),
        store: store.clone(),
        secrets: Arc::new(secrets),
    });

    // 1. Open and signed in when the 18:00 slot comes.
    let clock = Arc::new(SimClock::at(utc("2026-11-10T18:00:10.000Z")));
    let mut runner = Runner::new();
    tick(&mut runner, clock.as_ref(), &Utc, host.as_ref()).await;
    let list = store.load().expect("load");
    let c = list.get(tenant).expect("company");
    assert_eq!(c.schedule_runs.len(), 1, "{:?}", c.schedule_runs);
    assert_eq!(c.schedule_runs[0].slot, utc("2026-11-10T18:00:00.000Z"));
    let RunOutcome::Done { sha256 } = &c.schedule_runs[0].outcome else {
        panic!("{:?}", c.schedule_runs[0]);
    };
    let made = c.backup_by_sha256(sha256).expect("recorded").clone();
    assert_eq!(made.trigger, Some(BackupTrigger::Scheduled));
    assert_eq!(
        made.created_at,
        utc("2026-11-10T18:00:10.000Z"),
        "the runner's clock"
    );
    assert!(made.path.starts_with(&folder));
    assert_eq!(
        cl_company::backup::sha256_file(&made.path).expect("hash"),
        *sha256,
        "the file on disk is the one recorded"
    );
    // Retention with the schedule's values: the new one and the newest old.
    let names = files_in(&folder);
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&"Old-2.clbackup".to_string()), "{names:?}");
    assert!(
        names
            .iter()
            .all(|n| Path::new(n).extension().is_none_or(|x| x != "part")),
        "{names:?}"
    );
    // The same slot never runs twice.
    tick(&mut runner, clock.as_ref(), &Utc, host.as_ref()).await;
    assert_eq!(
        store
            .load()
            .expect("load")
            .get(tenant)
            .expect("c")
            .schedule_runs
            .len(),
        1
    );

    // 2. The next launch, two days later, after sleeping through two slots.
    let launch = utc("2026-11-12T09:00:00.000Z");
    clock.set(launch);
    *host.env.lock().expect("env") = RunnerEnv {
        signed_in_since: Some(launch),
        ..RunnerEnv::default()
    };
    let stop_at = utc("2026-11-12T09:05:00.000Z");
    let c2 = Arc::clone(&clock);
    run_loop(
        Arc::clone(&clock) as Arc<dyn Clock>,
        Arc::new(Utc),
        Arc::clone(&host) as Arc<dyn RunnerHost>,
        move || c2.now() >= stop_at,
    )
    .await;
    let list = store.load().expect("load");
    let c = list.get(tenant).expect("company");
    assert_eq!(
        c.schedule_runs.len(),
        2,
        "one catch-up: {:?}",
        c.schedule_runs
    );
    let caught = &c.schedule_runs[1];
    assert_eq!(
        caught.slot,
        utc("2026-11-11T18:00:00.000Z"),
        "the newest missed slot"
    );
    assert!(
        caught.at >= utc("2026-11-12T09:02:00.000Z"),
        "2 minutes after sign-in: {:?}",
        caught.at
    );
    let RunOutcome::Done { sha256 } = &caught.outcome else {
        panic!("{caught:?}");
    };
    assert_eq!(
        c.backup_by_sha256(sha256).expect("recorded").trigger,
        Some(BackupTrigger::Missed)
    );
    let names = files_in(&folder);
    assert_eq!(names.len(), 2, "keep last 2: {names:?}");

    let _ = s.logout().await;
}

#[test]
fn scheduled_backups_run_while_open_and_missed_ones_at_next_launch() {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = cl_pg::PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    let root = std::env::temp_dir().join(format!("cl-company-schedule-{}", std::process::id()));
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
                display_name: "Schedule Owner".into(),
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
