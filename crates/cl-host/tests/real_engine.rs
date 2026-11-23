//! The supervisor with the real bundled `PostgreSQL` and the real engine
//! executable (built by `cargo xtask engine`, fetched by `cargo xtask pg-fetch`).
//!
//! Without them a test prints `SKIPPED` and passes, unless `CL_HOST_REQUIRE=1`
//! (set in CI), which turns the skip into a failure. Timing budgets can be
//! changed with `CL_HOST_BUDGET_SECS` (cold; warm is a third of it).

mod common;

use cl_host::{AdminAccount, CompanySeed, HostConfig, HostState, Supervisor, check_health};
use cl_pg::{MemoryStore, PgBinaries};
use common::{Scratch, kill_hard, process_alive, quick_policy, wait_until};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The timing test must not compete with the other real-engine test for the
/// disk and CPU, so the tests in this file run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn require() -> bool {
    std::env::var("CL_HOST_REQUIRE").as_deref() == Ok("1")
}

fn skip<T>(why: &str) -> Option<T> {
    assert!(!require(), "CL_HOST_REQUIRE=1 but {why}");
    eprintln!("SKIPPED: {why} (CI sets CL_HOST_REQUIRE=1)");
    None
}

/// The engine executable under `target/engine/<target>/`.
fn engine_exe() -> Option<PathBuf> {
    // With a separate cargo build-dir the test program is not under the
    // checkout's `target/`: CL_ENGINE_EXE names the engine then.
    if let Some(p) = std::env::var_os("CL_ENGINE_EXE").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) {
        "clientledger-engine.exe"
    } else {
        "clientledger-engine"
    };
    for dir in exe.ancestors() {
        let engine_dir = dir.join("engine");
        if dir.file_name().is_some_and(|n| n == "target") && engine_dir.is_dir() {
            for entry in std::fs::read_dir(&engine_dir).ok()?.flatten() {
                let candidate = entry.path().join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

struct Env {
    engine: PathBuf,
    pg: PgBinaries,
}

fn env() -> Option<Env> {
    let Some(engine) = engine_exe() else {
        return skip("the engine executable is missing (run `cargo xtask engine`)");
    };
    let Ok(pg) = PgBinaries::discover(None) else {
        return skip("the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)");
    };
    Some(Env { engine, pg })
}

fn start_real(cfg: HostConfig) -> Supervisor {
    let guard = match cl_host::acquire(&cfg.paths).unwrap() {
        cl_host::Acquired::First(g) => g,
        cl_host::Acquired::Second => panic!("another instance holds the lock"),
    };
    Supervisor::start(cfg, guard).unwrap()
}

fn budget_secs() -> u64 {
    std::env::var("CL_HOST_BUDGET_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15)
}

fn real_config(
    scratch: &Scratch,
    env: &Env,
    pg_store: &Arc<MemoryStore>,
    secrets: &Arc<MemoryStore>,
) -> HostConfig {
    let mut cfg = HostConfig::new(scratch.paths(), &env.engine);
    cfg.pg_binaries = Some(env.pg.clone());
    cfg.pg_password_store = Some(Arc::clone(pg_store) as Arc<dyn cl_pg::PasswordStore>);
    cfg.secrets_store = Some(Arc::clone(secrets) as Arc<dyn cl_pg::PasswordStore>);
    cfg.policy = quick_policy();
    cfg.policy.ready_timeout = Duration::from_mins(2);
    cfg.policy.health_ready_timeout = Duration::from_mins(1);
    cfg.policy.health_interval = Duration::from_secs(1);
    cfg
}

#[test]
fn cold_and_warm_starts_meet_the_budget_and_nothing_is_left_behind() {
    let _one_at_a_time = serial();
    let Some(env) = env() else { return };
    let scratch = Scratch::new("real-cold");
    let pg_store = Arc::new(MemoryStore::new());
    let secrets = Arc::new(MemoryStore::new());

    // Cold: new cluster, new database role, engine migrations.
    let t0 = Instant::now();
    let sup = start_real(real_config(&scratch, &env, &pg_store, &secrets));
    let cold = t0.elapsed();
    eprintln!("cold start: {cold:?}");
    assert!(
        cold < Duration::from_secs(budget_secs()),
        "cold start took {cold:?}"
    );

    let status = sup.status();
    assert_eq!(status.state, HostState::Running);
    let ep = sup.endpoint().unwrap();
    let health = check_health(ep.port, &ep.token, Duration::from_secs(5)).unwrap();
    assert_eq!(
        health.superuser,
        Some(false),
        "engine must use the restricted role"
    );
    assert!(health.schema_version.is_some());

    // The wrong token is refused by the real engine.
    let wrong = cl_host::Secret::new("0".repeat(64));
    assert!(matches!(
        check_health(ep.port, &wrong, Duration::from_secs(5)),
        Err(cl_host::HealthError::TokenRefused)
    ));

    // The token is in the environment only.
    let engine_pid = status.engine_pid.unwrap();
    let shown = common::command_line(engine_pid).join(" ");
    assert!(
        !shown.contains(ep.token.expose()),
        "token in the process list"
    );
    let pg_pid = status.database_pid.unwrap();
    for pid in [engine_pid, pg_pid] {
        let line = common::command_line(pid).join(" ");
        assert!(!line.contains(ep.token.expose()));
    }

    // First-run setup through the host.
    let info = sup.engine_info().unwrap();
    assert!(!info.initialized);
    assert_ne!(info.db_superuser, Some(true));
    let made = sup
        .initialize_engine(
            &AdminAccount {
                email: "owner@example.com".into(),
                display_name: "Test Owner".into(),
                password: cl_host::Secret::new("a long test password 123"),
            },
            Some(&CompanySeed {
                name: "Test Company".into(),
                ..CompanySeed::default()
            }),
        )
        .unwrap();
    // The pinned engine build may predate the workspace tenant (Q-P04g): either
    // a workspace or a company id comes back, plus the owner.
    assert!(!made.user_id.is_empty(), "{made:?}");
    assert!(
        made.workspace_tenant_id.is_some() || made.company_tenant_id.is_some(),
        "{made:?}"
    );
    assert!(sup.engine_info().unwrap().initialized);
    // Table statistics refresh on the engine's database (Q-335).
    sup.refresh_statistics().unwrap();

    sup.stop();
    // No orphans: neither child is alive and nothing mentions our folders.
    assert!(!process_alive(engine_pid), "engine left running");
    assert!(
        wait_until(Duration::from_secs(15), || !process_alive(pg_pid)),
        "postgres left running"
    );
    let marker = scratch.root.to_string_lossy().into_owned();
    let leftovers = wait_until(Duration::from_secs(15), || {
        common::processes_mentioning(&marker)
            .iter()
            .all(|(pid, _)| *pid == std::process::id())
    });
    assert!(
        leftovers,
        "processes still reference {marker}: {:?}",
        common::processes_mentioning(&marker)
    );

    // Warm: same data, secrets from the same stores.
    let t1 = Instant::now();
    let sup = start_real(real_config(&scratch, &env, &pg_store, &secrets));
    let warm = t1.elapsed();
    eprintln!("warm start: {warm:?}");
    assert!(
        warm < Duration::from_secs(budget_secs() / 3),
        "warm start took {warm:?}"
    );
    // The data survived: the owner exists.
    assert!(sup.engine_info().unwrap().initialized);
    sup.stop();
}

#[test]
fn killing_the_engine_or_the_database_restarts_them() {
    let _one_at_a_time = serial();
    let Some(env) = env() else { return };
    let scratch = Scratch::new("real-kill");
    let pg_store = Arc::new(MemoryStore::new());
    let secrets = Arc::new(MemoryStore::new());
    let sup = start_real(real_config(&scratch, &env, &pg_store, &secrets));

    // Engine killed while idle.
    let first = sup.status();
    kill_hard(first.engine_pid.unwrap());
    assert!(
        sup.wait_for(Duration::from_mins(2), |s| s.generation == 2
            && s.state == HostState::Running),
        "engine did not come back: {:?}",
        sup.status()
    );
    let ep = sup.endpoint().unwrap();
    check_health(ep.port, &ep.token, Duration::from_secs(5)).unwrap();
    assert_eq!(sup.status().restarts.engine, 1);

    // Database killed (the postmaster, like a crash).
    let before = sup.status();
    kill_hard(before.database_pid.unwrap());
    assert!(
        sup.wait_for(Duration::from_mins(3), |s| s.generation == 3
            && s.state == HostState::Running),
        "database/engine did not come back: {:?}",
        sup.status()
    );
    let after = sup.status();
    assert_eq!(after.restarts.database, 1);
    assert_ne!(after.database_pid, before.database_pid);
    let ep = sup.endpoint().unwrap();
    let health = check_health(ep.port, &ep.token, Duration::from_secs(5)).unwrap();
    assert_eq!(health.superuser, Some(false));
    // The refresher follows the restarted database (Q-335).
    sup.refresh_statistics().unwrap();
    // P34: the restart after the kill replayed the journal.
    assert!(sup.database_recovered());
    sup.stop();
}

/// P34 A4: a whole-database backup and restore with the supervisor stopped
/// and started again; a maintenance hold restored with the database is
/// cleared when the engine starts (P33 follow-up 8).
#[test]
fn a_full_backup_and_restore_bring_the_data_back_and_clear_a_restored_hold() {
    let _one_at_a_time = serial();
    let Some(env) = env() else { return };
    let scratch = Scratch::new("real-cluster-ops");
    let pg_store = Arc::new(MemoryStore::new());
    let secrets = Arc::new(MemoryStore::new());
    let sup = start_real(real_config(&scratch, &env, &pg_store, &secrets));
    assert!(!sup.database_recovered(), "a fresh cluster replays nothing");
    sup.exec_sql_for_tests(
        "CREATE TABLE p34_marker (v int); INSERT INTO p34_marker VALUES (1);
         INSERT INTO desktop_maintenance (id, holder_session, holder_user)
         VALUES (1, gen_random_uuid(), gen_random_uuid())",
    )
    .unwrap();
    let dest = scratch.paths().root().join("full").join("cluster.tar.gz");

    let (sup, backup) = cl_host::cluster_ops::full_backup(
        sup,
        real_config(&scratch, &env, &pg_store, &secrets),
        &dest,
    );
    let info = backup.unwrap();
    assert!(info.path.is_file() && info.files > 0);
    sup.wait_ready(Duration::from_mins(3)).unwrap();
    // After the backup's restart the hold is gone and the data is there.
    sup.exec_sql_for_tests(
        "DO $$ BEGIN
           IF EXISTS (SELECT 1 FROM desktop_maintenance) THEN RAISE EXCEPTION 'hold kept'; END IF;
           IF (SELECT v FROM p34_marker) <> 1 THEN RAISE EXCEPTION 'marker'; END IF;
         END $$;
         UPDATE p34_marker SET v = 2;",
    )
    .unwrap();

    // A file this install did not write is refused without confirmation; the
    // supervisor comes back on the old database.
    let foreign = scratch.paths().root().join("full").join("copy.tar.gz");
    std::fs::copy(&dest, &foreign).unwrap();
    let mut bytes = std::fs::read(&foreign).unwrap();
    bytes.push(0);
    std::fs::write(&foreign, bytes).unwrap();
    let (sup, refused) = cl_host::cluster_ops::full_restore(
        sup,
        real_config(&scratch, &env, &pg_store, &secrets),
        &foreign,
        false,
    );
    assert!(
        matches!(
            refused,
            Err(cl_host::cluster_ops::ClusterOpError::Unregistered)
        ),
        "{refused:?}"
    );
    sup.wait_ready(Duration::from_mins(3)).unwrap();
    sup.exec_sql_for_tests("DO $$ BEGIN IF (SELECT v FROM p34_marker) <> 2 THEN RAISE EXCEPTION 'changed'; END IF; END $$")
        .unwrap();

    let (sup, restored) = cl_host::cluster_ops::full_restore(
        sup,
        real_config(&scratch, &env, &pg_store, &secrets),
        &dest,
        false,
    );
    let r = restored.unwrap();
    assert!(
        r.previous.as_ref().is_some_and(|p| p.is_dir()),
        "the old data folder is kept"
    );
    sup.wait_ready(Duration::from_mins(3)).unwrap();
    // The backup held a hold row: the engine start cleared it; the data is the backup's.
    sup.exec_sql_for_tests(
        "DO $$ BEGIN
           IF EXISTS (SELECT 1 FROM desktop_maintenance) THEN RAISE EXCEPTION 'restored hold kept'; END IF;
           IF (SELECT v FROM p34_marker) <> 1 THEN RAISE EXCEPTION 'not the backup'; END IF;
         END $$",
    )
    .unwrap();
    let ep = sup.endpoint().unwrap();
    check_health(ep.port, &ep.token, Duration::from_secs(5)).unwrap();
    sup.stop();
}
