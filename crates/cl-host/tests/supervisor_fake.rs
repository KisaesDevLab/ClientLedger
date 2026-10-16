//! Supervisor logic against a fake engine and a fake database: ordering,
//! restarts and backoff, faults, token and environment handling.

mod common;

use cl_host::{
    AdminAccount, CompanySeed, Component, DatabaseFactory, DatabaseParts, DatabaseService,
    DbEndpoint, DesktopError, EngineSecrets, HostConfig, HostError, HostState, Result, Secret,
    Supervisor, check_health,
};
use cl_pg::PasswordStore as _;
use common::{Scratch, fake_config, process_alive, wait_until};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Shared knobs and records of the fake database.
#[derive(Default)]
struct FakeDbState {
    alive: AtomicBool,
    starts: AtomicU32,
    stops: AtomicU32,
    fail_next_starts: AtomicU32,
    engine_pid_at_stop: Mutex<Vec<Option<u32>>>,
    engine_pid: AtomicU32,
}

struct FakeDb(Arc<FakeDbState>);

impl DatabaseService for FakeDb {
    fn start(&mut self) -> Result<DbEndpoint> {
        let s = &self.0;
        if s.fail_next_starts.load(Ordering::SeqCst) > 0 {
            s.fail_next_starts.fetch_sub(1, Ordering::SeqCst);
            return Err(HostError::Io {
                context: "fake database start".into(),
                source: std::io::Error::other("refused on purpose"),
            });
        }
        s.alive.store(true, Ordering::SeqCst);
        s.starts.fetch_add(1, Ordering::SeqCst);
        Ok(DbEndpoint {
            port: 5999,
            pid: 0,
            url: Secret::new("postgres://app:FAKE-DB-PASSWORD@127.0.0.1:5999/db"),
        })
    }

    fn is_alive(&mut self, _: &DbEndpoint) -> bool {
        self.0.alive.load(Ordering::SeqCst)
    }

    fn stop(&mut self) -> Result<()> {
        let pid = self.0.engine_pid.load(Ordering::SeqCst);
        let still_running = (pid != 0 && process_alive(pid)).then_some(pid);
        self.0
            .engine_pid_at_stop
            .lock()
            .unwrap()
            .push(still_running);
        self.0.alive.store(false, Ordering::SeqCst);
        self.0.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn fake_db() -> (Arc<FakeDbState>, DatabaseFactory) {
    let state = Arc::new(FakeDbState::default());
    let s = Arc::clone(&state);
    let factory: DatabaseFactory = Box::new(move |_: &EngineSecrets| -> Result<DatabaseParts> {
        Ok((Box::new(FakeDb(s)) as Box<dyn DatabaseService>, vec![]))
    });
    (state, factory)
}

fn start(cfg: HostConfig, factory: DatabaseFactory) -> Result<Supervisor> {
    let sup = Supervisor::spawn_with(cfg, factory);
    sup.wait_ready(Duration::from_mins(1))?;
    Ok(sup)
}

#[test]
fn starts_in_order_serves_health_and_stops_in_reverse() {
    let scratch = Scratch::new("order");
    let (db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    let status = sup.status();
    assert_eq!(status.state, HostState::Running);
    assert_eq!(status.generation, 1);
    assert_eq!(db.starts.load(Ordering::SeqCst), 1);
    let ep = sup.endpoint().expect("endpoint while running");
    let report = check_health(ep.port, &ep.token, Duration::from_secs(3)).unwrap();
    assert_eq!(report.mode, "desktop");
    assert_eq!(report.superuser, Some(false));

    let engine_pid = status.engine_pid.unwrap();
    db.engine_pid.store(engine_pid, Ordering::SeqCst);
    sup.stop();

    // The engine was gone before the database was asked to stop.
    assert_eq!(*db.engine_pid_at_stop.lock().unwrap(), vec![None]);
    assert!(!process_alive(engine_pid), "engine left running");
    assert_eq!(db.stops.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_the_supervisor_leaves_no_engine_behind() {
    let scratch = Scratch::new("drop");
    let (_db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    let pid = sup.status().engine_pid.unwrap();
    assert!(process_alive(pid));
    drop(sup);
    assert!(wait_until(Duration::from_secs(5), || !process_alive(pid)));
}

#[test]
fn the_token_is_never_an_argument_and_the_environment_is_not_inherited() {
    let scratch = Scratch::new("env");
    let (_db, factory) = fake_db();
    let report = scratch.file("report.txt");
    let cfg = fake_config(&scratch).with_engine_env("FAKE_REPORT", report.to_str().unwrap());
    let sup = start(cfg, factory).unwrap();
    let ep = sup.endpoint().unwrap();
    let pid = sup.status().engine_pid.unwrap();

    // What the operating system shows other programs: no arguments at all.
    let cmd = common::command_line(pid);
    assert!(cmd.len() <= 1, "unexpected arguments: {}", cmd.len());
    let shown = cmd.join(" ");
    assert!(!shown.contains(ep.token.expose()), "token in process list");
    assert!(!shown.contains("FAKE-DB-PASSWORD"));

    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains("args=\n"), "no arguments: {text}");
    assert!(text.contains(&format!("env:CL_TOKEN={}", ep.token.expose())));
    assert!(text.contains("env:CL_MODE=desktop"));
    assert!(text.contains("env:CL_REQUIRE_NON_SUPERUSER=1"));
    // Nothing inherited from the test process (cargo sets many variables).
    for line in text.lines().filter_map(|l| l.strip_prefix("env:")) {
        let key = line.split('=').next().unwrap();
        assert!(
            !key.starts_with("CARGO") && !key.starts_with("RUST") && key != "CL_OFFLINE_TESTS",
            "inherited variable {key}"
        );
    }
    // Absolute paths only.
    for want in ["CL_DATA_DIR", "CL_LOG_FILE", "CL_ENGINE_RUNTIME_DIR"] {
        let line = text
            .lines()
            .find_map(|l| l.strip_prefix(&format!("env:{want}=")))
            .unwrap_or_else(|| panic!("{want} missing"));
        assert!(std::path::Path::new(line).is_absolute(), "{want}={line}");
    }
    sup.stop();
}

#[test]
fn no_secret_reaches_the_host_log() {
    let scratch = Scratch::new("log");
    let (_db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    let token = sup.endpoint().unwrap().token;
    sup.stop();
    let log = std::fs::read_to_string(scratch.paths().host_log()).unwrap();
    assert!(log.contains("engine ready"));
    assert!(!log.contains(token.expose()));
    assert!(!log.contains("FAKE-DB-PASSWORD"));
}

#[test]
fn an_engine_killed_while_idle_is_restarted() {
    let scratch = Scratch::new("kill-engine");
    let (_db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    let first = sup.status();
    let old_ep = sup.endpoint().unwrap();
    common::kill_hard(first.engine_pid.unwrap());

    assert!(
        sup.wait_for(Duration::from_secs(30), |s| s.generation == 2
            && s.state == HostState::Running),
        "engine was not restarted: {:?}",
        sup.status()
    );
    let after = sup.status();
    assert_eq!(after.restarts.engine, 1);
    assert_eq!(after.restarts.database, 0);
    assert_ne!(after.engine_pid, first.engine_pid);
    let ep = sup.endpoint().unwrap();
    assert_ne!(
        ep.token.expose(),
        old_ep.token.expose(),
        "fresh token per launch"
    );
    check_health(ep.port, &ep.token, Duration::from_secs(3)).unwrap();
    sup.stop();
}

#[test]
fn the_reconnect_state_is_visible_while_restarting() {
    let scratch = Scratch::new("reconnect");
    let (_db, factory) = fake_db();
    // A slower restart so the intermediate state can be observed.
    let mut cfg = fake_config(&scratch);
    cfg.policy.backoff = vec![Duration::from_millis(800)];
    let sup = start(cfg, factory).unwrap();
    common::kill_hard(sup.status().engine_pid.unwrap());
    assert!(sup.wait_for(Duration::from_secs(10), |s| matches!(
        s.state,
        HostState::Restarting {
            component: Component::Engine,
            attempt: 1
        }
    )));
    assert!(
        sup.endpoint().is_none(),
        "no endpoint while the engine is down"
    );
    assert!(sup.wait_for(Duration::from_secs(30), |s| s.state == HostState::Running));
    sup.stop();
}

#[test]
fn a_database_that_dies_is_restarted_and_the_engine_follows() {
    let scratch = Scratch::new("kill-db");
    let (db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    let first_engine = sup.status().engine_pid.unwrap();

    db.alive.store(false, Ordering::SeqCst); // the database "crashes"
    assert!(
        sup.wait_for(Duration::from_secs(30), |s| s.generation == 2
            && s.state == HostState::Running),
        "{:?}",
        sup.status()
    );
    let after = sup.status();
    assert_eq!(db.starts.load(Ordering::SeqCst), 2);
    assert_eq!(after.restarts.database, 1);
    assert_eq!(after.restarts.engine, 0, "the engine's budget is untouched");
    assert!(!process_alive(first_engine), "old engine must not linger");
    sup.stop();
}

#[test]
fn three_failed_restarts_become_a_host_fault_with_log_paths() {
    let scratch = Scratch::new("fault-engine");
    let (_db, factory) = fake_db();
    let counter = scratch.file("starts.txt");
    let cfg = fake_config(&scratch)
        .with_engine_env("FAKE_COUNTER", counter.to_str().unwrap())
        .with_engine_env("FAKE_FAIL_FIRST", "100");
    let sup = Supervisor::spawn_with(cfg, factory);
    let err = sup.wait_ready(Duration::from_mins(1)).unwrap_err();
    let HostError::Fault(fault) = err else {
        panic!("expected a fault, got {err:?}");
    };
    assert_eq!(fault.component, Component::Engine);
    assert_eq!(fault.restarts, 3);
    let paths = scratch.paths();
    assert!(fault.log_paths.contains(&paths.host_log()));
    assert!(fault.log_paths.contains(&paths.engine_log()));
    // One first attempt plus exactly three restarts.
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "4");
    assert!(matches!(sup.status().state, HostState::Fault(_)));
    assert!(sup.endpoint().is_none());
    assert!(
        fault
            .last_output
            .iter()
            .any(|l| l.contains("failing on purpose")),
        "{:?}",
        fault.last_output
    );
    sup.stop();
}

#[test]
fn an_engine_that_recovers_within_three_restarts_is_fine() {
    let scratch = Scratch::new("recover");
    let (_db, factory) = fake_db();
    let counter = scratch.file("starts.txt");
    let cfg = fake_config(&scratch)
        .with_engine_env("FAKE_COUNTER", counter.to_str().unwrap())
        .with_engine_env("FAKE_FAIL_FIRST", "3");
    let sup = start(cfg, factory).unwrap();
    assert_eq!(sup.status().restarts.engine, 3);
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "4");
    sup.stop();
}

#[test]
fn an_engine_that_keeps_dying_after_it_is_ready_ends_in_a_fault() {
    let scratch = Scratch::new("flapping");
    let (_db, factory) = fake_db();
    let counter = scratch.file("starts.txt");
    let cfg = fake_config(&scratch)
        .with_engine_env("FAKE_COUNTER", counter.to_str().unwrap())
        .with_engine_env("FAKE_DIE_AFTER_MS", "400");
    let sup = Supervisor::spawn_with(cfg, factory);
    assert!(
        sup.wait_for(Duration::from_mins(1), |s| matches!(
            s.state,
            HostState::Fault(_)
        )),
        "{:?}",
        sup.status()
    );
    assert_eq!(std::fs::read_to_string(counter).unwrap(), "4");
    sup.stop();
}

#[test]
fn a_configuration_error_is_not_retried() {
    let scratch = Scratch::new("config-error");
    let (_db, factory) = fake_db();
    let counter = scratch.file("starts.txt");
    let cfg = fake_config(&scratch)
        .with_engine_env("FAKE_COUNTER", counter.to_str().unwrap())
        .with_engine_env("FAKE_FATAL", "1");
    let sup = Supervisor::spawn_with(cfg, factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_secs(30)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert!(fault.reason.contains("CL_ENV_INVALID"), "{}", fault.reason);
    assert_eq!(
        std::fs::read_to_string(counter).unwrap(),
        "1",
        "exit 78 must not restart"
    );
    sup.stop();
}

#[test]
fn a_superuser_engine_is_refused() {
    let scratch = Scratch::new("superuser");
    let (_db, factory) = fake_db();
    let cfg = fake_config(&scratch).with_engine_env("FAKE_SUPERUSER", "1");
    let sup = Supervisor::spawn_with(cfg, factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_secs(30)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert!(
        fault.reason.contains("restricted database role"),
        "{}",
        fault.reason
    );
    sup.stop();
}

#[test]
fn a_wrong_engine_version_is_refused() {
    let scratch = Scratch::new("commit");
    let (_db, factory) = fake_db();
    let mut cfg = fake_config(&scratch);
    cfg.expected_engine_commit = Some("somethingelse".into());
    let sup = Supervisor::spawn_with(cfg, factory);
    assert!(matches!(
        sup.wait_ready(Duration::from_secs(30)),
        Err(HostError::Fault(_))
    ));
    sup.stop();
}

#[test]
fn an_engine_that_never_says_ready_is_a_fault_after_the_retries() {
    let scratch = Scratch::new("no-ready");
    let (_db, factory) = fake_db();
    let mut cfg = fake_config(&scratch).with_engine_env("FAKE_NO_READY", "1");
    cfg.policy.ready_timeout = Duration::from_millis(400);
    let sup = Supervisor::spawn_with(cfg, factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_mins(1)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert!(
        fault.reason.contains("did not become ready"),
        "{}",
        fault.reason
    );
    sup.stop();
}

#[test]
fn an_engine_that_never_answers_health_is_a_fault() {
    let scratch = Scratch::new("hang");
    let (_db, factory) = fake_db();
    let mut cfg = fake_config(&scratch).with_engine_env("FAKE_HANG_HEALTH", "1");
    cfg.policy.health_ready_timeout = Duration::from_millis(600);
    let sup = Supervisor::spawn_with(cfg, factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_mins(1)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert!(
        fault.reason.contains("never reported healthy"),
        "{}",
        fault.reason
    );
    sup.stop();
}

#[test]
fn a_database_that_will_not_start_is_a_fault_with_its_log_paths() {
    let scratch = Scratch::new("fault-db");
    let (db, factory) = fake_db();
    db.fail_next_starts.store(100, Ordering::SeqCst);
    let sup = Supervisor::spawn_with(fake_config(&scratch), factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_mins(1)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert_eq!(fault.component, Component::Database);
    assert_eq!(fault.restarts, 3);
    assert!(!fault.log_paths.is_empty());
    // 4 attempts consumed 4 of the 100 refusals.
    assert_eq!(db.fail_next_starts.load(Ordering::SeqCst), 96);
    sup.stop();
}

#[test]
fn a_database_failure_never_starts_the_engine() {
    let scratch = Scratch::new("db-first");
    let (db, factory) = fake_db();
    db.fail_next_starts.store(100, Ordering::SeqCst);
    let report = scratch.file("report.txt");
    let cfg = fake_config(&scratch).with_engine_env("FAKE_REPORT", report.to_str().unwrap());
    let sup = Supervisor::spawn_with(cfg, factory);
    let _ = sup.wait_ready(Duration::from_mins(1));
    assert!(
        !report.exists(),
        "the engine must not start before the database"
    );
    sup.stop();
}

#[test]
fn a_stale_engine_error_text_never_contains_the_token() {
    let scratch = Scratch::new("leak");
    let (_db, factory) = fake_db();
    let report = scratch.file("report.txt");
    let cfg = fake_config(&scratch)
        .with_engine_env("FAKE_REPORT", report.to_str().unwrap())
        .with_engine_env("FAKE_LEAK_TOKEN", "1")
        .with_engine_env("FAKE_DIE_AFTER_MS", "300");
    let sup = Supervisor::spawn_with(cfg, factory);
    assert!(sup.wait_for(Duration::from_mins(1), |s| matches!(
        s.state,
        HostState::Fault(_)
    )));
    let HostState::Fault(fault) = sup.status().state else {
        unreachable!()
    };
    // The token of the last launch is in the report file.
    let text = std::fs::read_to_string(&report).unwrap();
    let token = text
        .lines()
        .find_map(|l| l.strip_prefix("env:CL_TOKEN="))
        .unwrap();
    let shown = format!("{fault} {:?} {:?}", fault.last_output, fault.reason);
    assert!(!shown.contains(token), "token leaked into the fault report");
    assert!(
        fault.last_output.iter().any(|l| l.contains("token=***")),
        "{:?}",
        fault.last_output
    );
    sup.stop();
}

#[test]
fn first_run_setup_goes_through_the_engine() {
    let scratch = Scratch::new("init");
    let (_db, factory) = fake_db();
    let sup = start(fake_config(&scratch), factory).unwrap();
    assert!(!sup.engine_info().unwrap().initialized);

    let admin = AdminAccount {
        email: "owner@example.com".into(),
        display_name: "Pat Owner".into(),
        password: Secret::new("a long enough password"),
    };
    let company = CompanySeed {
        name: "Acme Plumbing".into(),
        ..CompanySeed::default()
    };
    let made = sup.initialize_engine(&admin, Some(&company)).unwrap();
    assert_eq!(made.workspace_tenant_id.as_deref(), Some("ws-1"));
    assert_eq!(made.user_id, "user-1");
    assert_eq!(made.company_tenant_id.as_deref(), Some("co-1"));
    let info = sup.engine_info().unwrap();
    assert!(info.initialized);
    assert_eq!(info.workspace_tenant_id.as_deref(), Some("ws-1"));

    let again = sup.initialize_engine(&admin, None).unwrap_err();
    match again {
        HostError::Desktop(DesktopError::Refused { status, code }) => {
            assert_eq!(status, 409);
            assert_eq!(code, "ALREADY_INITIALIZED");
        }
        other => panic!("unexpected {other:?}"),
    }
    sup.stop();
}

#[test]
fn setup_calls_report_not_running_while_the_engine_is_down() {
    let scratch = Scratch::new("init-down");
    let (db, factory) = fake_db();
    db.fail_next_starts.store(100, Ordering::SeqCst);
    let sup = Supervisor::spawn_with(fake_config(&scratch), factory);
    let _ = sup.wait_ready(Duration::from_mins(1));
    assert!(matches!(sup.engine_info(), Err(HostError::NotRunning)));
    sup.stop();
}

#[test]
fn relative_paths_never_reach_the_engine() {
    // AppPaths refuses relative roots, so a relative CL_DATA_DIR is impossible.
    assert!(cl_host::AppPaths::at("relative/dir").is_err());
}

/// Pretend the cluster already exists, as it does on every run after the first.
fn make_cluster_exist(scratch: &Scratch) {
    let data = scratch.paths().pg_data();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("PG_VERSION"), "16\n").unwrap();
}

#[test]
fn an_existing_cluster_with_no_master_secret_is_never_given_a_new_one() {
    let scratch = Scratch::new("no-master");
    make_cluster_exist(&scratch);
    let (db, factory) = fake_db();
    let sup = Supervisor::spawn_with(fake_config(&scratch), factory);
    let HostError::Fault(fault) = sup.wait_ready(Duration::from_secs(30)).unwrap_err() else {
        panic!("expected a fault");
    };
    assert!(fault.reason.contains("master secret"), "{}", fault.reason);
    assert_eq!(
        db.starts.load(Ordering::SeqCst),
        0,
        "no database, no engine"
    );
    sup.stop();
}

#[test]
fn a_keychain_that_fails_its_probe_cannot_cause_a_new_secret() {
    // `default_store_for` falls back to an (empty) encrypted file when the
    // keychain probe fails; on an existing cluster that must be an error.
    let scratch = Scratch::new("probe-fail");
    make_cluster_exist(&scratch);
    let (_db, factory) = fake_db();
    let mut cfg = fake_config(&scratch);
    let fallback =
        cl_pg::EncryptedFileStore::beside_for_purpose(&scratch.paths().pg_data(), "host-master");
    cfg.secrets_store = Some(Arc::new(fallback.clone()));
    let sup = Supervisor::spawn_with(cfg, factory);
    assert!(matches!(
        sup.wait_ready(Duration::from_secs(30)),
        Err(HostError::Fault(_))
    ));
    assert!(
        !fallback.path().exists(),
        "no weaker file store was created"
    );
    sup.stop();
}

#[test]
fn a_brand_new_cluster_creates_the_master_secret_once() {
    let scratch = Scratch::new("new-master");
    let store = Arc::new(cl_pg::MemoryStore::new());
    let (_db, factory) = fake_db();
    let mut cfg = fake_config(&scratch);
    cfg.secrets_store = Some(Arc::clone(&store) as Arc<dyn cl_pg::PasswordStore>);
    let sup = start(cfg, factory).unwrap();
    sup.stop();
    assert!(store.get().unwrap().is_some());
}

#[test]
fn a_panic_in_the_supervisor_becomes_a_fault_and_stops_the_engine() {
    struct PanickyDb(Arc<AtomicBool>, FakeDb);
    impl DatabaseService for PanickyDb {
        fn start(&mut self) -> Result<DbEndpoint> {
            self.1.start()
        }
        fn is_alive(&mut self, ep: &DbEndpoint) -> bool {
            assert!(!self.0.load(Ordering::SeqCst), "boom (on purpose)");
            self.1.is_alive(ep)
        }
        fn stop(&mut self) -> Result<()> {
            self.1.stop()
        }
    }
    let scratch = Scratch::new("panic");
    let bomb = Arc::new(AtomicBool::new(false));
    let state = Arc::new(FakeDbState::default());
    let (b, s) = (Arc::clone(&bomb), Arc::clone(&state));
    let factory: DatabaseFactory = Box::new(move |_: &EngineSecrets| -> Result<DatabaseParts> {
        Ok((
            Box::new(PanickyDb(b, FakeDb(s))) as Box<dyn DatabaseService>,
            vec![],
        ))
    });
    let sup = start(fake_config(&scratch), factory).unwrap();
    let pid = sup.status().engine_pid.unwrap();
    bomb.store(true, Ordering::SeqCst);
    assert!(sup.wait_for(Duration::from_secs(20), |s| matches!(
        s.state,
        HostState::Fault(_)
    )));
    assert!(
        wait_until(Duration::from_secs(10), || !process_alive(pid)),
        "engine survived the panic"
    );
    assert!(sup.endpoint().is_none());
    sup.stop();
}
