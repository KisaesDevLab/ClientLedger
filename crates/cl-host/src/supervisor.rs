//! The supervisor: starts the database, then the engine; watches both; stops
//! them in reverse order; restarts a child that dies, up to three times with
//! backoff, then reports a [`HostFault`] with the log locations.
//!
//! Everything runs on one background thread so the UI thread never blocks.
//! The UI polls [`Supervisor::status`] each frame and uses
//! [`Supervisor::endpoint`] to learn where (and with which token) to talk to
//! the engine; the endpoint changes after every engine restart.

use crate::database::{DatabaseFactory, DatabaseService, DbEndpoint, PgService};
use crate::desktop::{self, AdminAccount, CompanySeed, DesktopError, DesktopInfo, Initialized};
use crate::engine::{self, EngineProcess, LaunchError, LaunchPlan, Scrubber};
use crate::health::{self, HealthError, HealthReport};
use crate::logs::{self, HostLog, LogPolicy};
use crate::procs;
use crate::secrets::{EngineSecrets, MASTER_PURPOSE};
use crate::{AppPaths, HostError, Result};
use cl_pg::{MemoryStore, PasswordStore, PgBinaries, Secret};
use std::fmt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One of the two supervised programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Component {
    /// The bundled `PostgreSQL` server.
    Database,
    /// The accounting engine.
    Engine,
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Component::Database => "database",
            Component::Engine => "engine",
        })
    }
}

/// The supervisor gave up on a component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFault {
    /// What failed.
    pub component: Component,
    /// Plain-language reason, without secrets.
    pub reason: String,
    /// How many restarts were tried before giving up.
    pub restarts: u32,
    /// Log files and folders to look in (host log, engine log, database logs).
    pub log_paths: Vec<PathBuf>,
    /// Last lines the component printed, scrubbed.
    pub last_output: Vec<String>,
}

impl fmt::Display for HostFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The {} stopped working and could not be restarted ({}). Logs: {}",
            self.component,
            self.reason,
            self.log_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// Where the supervisor is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostState {
    /// First start in progress.
    Starting,
    /// Both programs are up and healthy.
    Running,
    /// A program died or hung; it is being brought back (`attempt` counts from 1).
    Restarting {
        /// Which program.
        component: Component,
        /// Restart number, 1 to 3.
        attempt: u32,
    },
    /// Gave up. Terminal.
    Fault(HostFault),
    /// Shutting down.
    Stopping,
    /// Shut down. Terminal.
    Stopped,
}

/// Restarts so far, per component, since the last stable period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RestartCounts {
    /// Database restarts.
    pub database: u32,
    /// Engine restarts.
    pub engine: u32,
}

/// A snapshot for the UI and the CLI. Contains no secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostStatus {
    /// Current state.
    pub state: HostState,
    /// Database server process id, when running.
    pub database_pid: Option<u32>,
    /// Engine process id, when running.
    pub engine_pid: Option<u32>,
    /// Restart counters.
    pub restarts: RestartCounts,
    /// Increases every time the engine is (re)started; clients reconnect when it changes.
    pub generation: u64,
    /// Things the owner should be told (for example, keychain fallback).
    pub notices: Vec<String>,
    /// The engine's last health report.
    pub engine_commit: Option<String>,
    /// Engine schema version from the last health report.
    pub schema_version: Option<u64>,
}

impl HostStatus {
    /// One line for `host status`; no ports, no secrets.
    #[must_use]
    pub fn summary(&self) -> String {
        let state = match &self.state {
            HostState::Starting => "starting".to_string(),
            HostState::Running => "running".to_string(),
            HostState::Restarting { component, attempt } => {
                format!("restarting-{component}-{attempt}")
            }
            HostState::Fault(f) => format!("fault-{}", f.component),
            HostState::Stopping => "stopping".to_string(),
            HostState::Stopped => "stopped".to_string(),
        };
        let pid = |p: Option<u32>| p.map_or_else(|| "-".to_string(), |p| p.to_string());
        format!(
            "state={state} database_pid={} engine_pid={} restarts_database={} restarts_engine={} generation={}",
            pid(self.database_pid),
            pid(self.engine_pid),
            self.restarts.database,
            self.restarts.engine,
            self.generation
        )
    }
}

/// How to reach the running engine.
#[derive(Clone)]
pub struct Endpoint {
    /// Loopback port.
    pub port: u16,
    /// Value for the `X-CL-Token` header.
    pub token: Secret,
    /// Matches [`HostStatus::generation`].
    pub generation: u64,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("port", &self.port)
            .field("token", &"***")
            .field("generation", &self.generation)
            .finish()
    }
}

/// Timing and retry rules.
#[derive(Debug, Clone)]
pub struct RestartPolicy {
    /// Restarts allowed per component before a fault (default 3).
    pub max_restarts: u32,
    /// Wait before restart 1, 2, 3 (the last value repeats).
    pub backoff: Vec<Duration>,
    /// A child that stays healthy this long gets its restart count reset.
    pub stable_after: Duration,
    /// How long the engine may take to print its ready line (migrations run first).
    pub ready_timeout: Duration,
    /// How long `/health` may take to turn healthy after the ready line.
    pub health_ready_timeout: Duration,
    /// How often `/health` and the database are polled.
    pub health_interval: Duration,
    /// Consecutive failed polls before the engine is considered hung.
    pub health_failures: u32,
    /// How often child exit is checked.
    pub poll_interval: Duration,
    /// How often port ownership is re-verified.
    pub listener_check_every: Duration,
    /// Time the engine gets to exit after being asked (Unix).
    pub stop_grace: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        RestartPolicy {
            max_restarts: 3,
            backoff: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
            ],
            stable_after: Duration::from_mins(2),
            ready_timeout: Duration::from_mins(2),
            health_ready_timeout: Duration::from_secs(30),
            health_interval: Duration::from_secs(2),
            health_failures: 3,
            poll_interval: Duration::from_millis(250),
            listener_check_every: Duration::from_secs(15),
            stop_grace: Duration::from_secs(10),
        }
    }
}

impl RestartPolicy {
    fn backoff_for(&self, failures: u32) -> Duration {
        let i = usize::try_from(failures.saturating_sub(1)).unwrap_or(usize::MAX);
        self.backoff
            .get(i)
            .or_else(|| self.backoff.last())
            .copied()
            .unwrap_or(Duration::from_secs(1))
    }
}

/// Everything the supervisor needs.
pub struct HostConfig {
    /// App data folders.
    pub paths: AppPaths,
    /// The engine executable (absolute path).
    pub engine_exe: PathBuf,
    /// `PostgreSQL` install; `None` finds the bundled one.
    pub pg_binaries: Option<PgBinaries>,
    /// Where the cluster superuser password lives; `None` uses the keychain.
    pub pg_password_store: Option<Arc<dyn PasswordStore>>,
    /// Where the engine master secret lives; `None` uses the keychain.
    pub secrets_store: Option<Arc<dyn PasswordStore>>,
    /// Retry and timing rules.
    pub policy: RestartPolicy,
    /// When set, the engine must report exactly this source commit.
    pub expected_engine_commit: Option<String>,
    extra_engine_env: Vec<(String, String)>,
}

impl fmt::Debug for HostConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostConfig")
            .field("paths", &self.paths)
            .field("engine_exe", &self.engine_exe)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl HostConfig {
    /// Defaults for `paths` and the engine at `engine_exe`.
    #[must_use]
    pub fn new(paths: AppPaths, engine_exe: impl Into<PathBuf>) -> HostConfig {
        HostConfig {
            paths,
            engine_exe: engine_exe.into(),
            pg_binaries: None,
            pg_password_store: None,
            secrets_store: None,
            policy: RestartPolicy::default(),
            expected_engine_commit: None,
            extra_engine_env: Vec::new(),
        }
    }

    /// Extra environment for the engine process. Only for tests (a fake
    /// engine's behaviour switches).
    #[doc(hidden)]
    #[must_use]
    pub fn with_engine_env(mut self, key: &str, value: &str) -> HostConfig {
        self.extra_engine_env.push((key.into(), value.into()));
        self
    }
}

#[derive(Debug)]
struct Inner {
    status: HostStatus,
    endpoint: Option<Endpoint>,
}

#[derive(Debug)]
struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
}

impl Shared {
    fn update(&self, f: impl FnOnce(&mut Inner)) {
        if let Ok(mut g) = self.inner.lock() {
            f(&mut g);
        }
        self.changed.notify_all();
    }

    fn set_state(&self, state: HostState) {
        self.update(|i| i.status.state = state);
    }
}

/// Handle to the running supervisor. Dropping it shuts everything down.
#[derive(Debug)]
pub struct Supervisor {
    shared: Arc<Shared>,
    stop_tx: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    start_budget: Duration,
}

impl Supervisor {
    /// Start supervising on a background thread and return at once; the state
    /// is `Starting` until both programs are healthy.
    #[must_use]
    pub fn spawn(config: HostConfig) -> Supervisor {
        let paths = config.paths.clone();
        let bins = config.pg_binaries.clone();
        let pg_store = config.pg_password_store.clone();
        let factory: DatabaseFactory = Box::new(move |secrets: &EngineSecrets| {
            let (svc, notices) =
                PgService::open(&paths, bins, pg_store, secrets.app_db_password.clone())?;
            Ok((Box::new(svc) as Box<dyn DatabaseService>, notices))
        });
        Self::spawn_with(config, factory)
    }

    /// [`Supervisor::spawn`] with a custom database (tests use a fake).
    #[must_use]
    pub fn spawn_with(config: HostConfig, database: DatabaseFactory) -> Supervisor {
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                status: HostStatus {
                    state: HostState::Starting,
                    database_pid: None,
                    engine_pid: None,
                    restarts: RestartCounts::default(),
                    generation: 0,
                    notices: Vec::new(),
                    engine_commit: None,
                    schema_version: None,
                },
                endpoint: None,
            }),
            changed: Condvar::new(),
        });
        let (stop_tx, stop_rx) = mpsc::channel();
        let start_budget =
            config.policy.ready_timeout * (config.policy.max_restarts + 1) + Duration::from_mins(2);
        let log_paths = vec![config.paths.host_log(), config.paths.engine_log()];
        let worker = Worker::new(config, Arc::clone(&shared), stop_rx);
        let on_panic = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("cl-supervisor".into())
            .spawn(move || {
                // A bug in the supervisor must end in a visible fault, never in a
                // silent dead thread that leaves the UI waiting. Unwinding also
                // drops (and so terminates) the engine process.
                let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker.run(database);
                }));
                if ran.is_err() {
                    on_panic.update(|i| {
                        i.endpoint = None;
                        i.status.engine_pid = None;
                        i.status.state = HostState::Fault(HostFault {
                            component: Component::Engine,
                            reason: "an internal error stopped the supervisor".into(),
                            restarts: 0,
                            log_paths,
                            last_output: Vec::new(),
                        });
                    });
                }
            })
            .ok();
        if thread.is_none() {
            shared.set_state(HostState::Stopped);
        }
        Supervisor {
            shared,
            stop_tx: Some(stop_tx),
            thread,
            start_budget,
        }
    }

    /// Start and wait until the engine is ready.
    ///
    /// # Errors
    /// [`HostError::Fault`] if a component could not be started.
    pub fn start(config: HostConfig) -> Result<Supervisor> {
        let sup = Self::spawn(config);
        sup.wait_ready(sup.start_budget)?;
        Ok(sup)
    }

    /// A snapshot of the state.
    #[must_use]
    pub fn status(&self) -> HostStatus {
        self.shared
            .inner
            .lock()
            .map_or_else(|p| p.into_inner().status.clone(), |g| g.status.clone())
    }

    /// Where to reach the engine, or `None` while it is down.
    #[must_use]
    pub fn endpoint(&self) -> Option<Endpoint> {
        self.shared
            .inner
            .lock()
            .ok()
            .and_then(|g| g.endpoint.clone())
    }

    /// Block until `Running`.
    ///
    /// # Errors
    /// [`HostError::Fault`] when the supervisor gave up, [`HostError::Timeout`]
    /// when `timeout` passed first, [`HostError::NotRunning`] after shutdown.
    pub fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let Ok(mut guard) = self.shared.inner.lock() else {
            return Err(HostError::NotRunning);
        };
        loop {
            match &guard.status.state {
                HostState::Running => return Ok(()),
                HostState::Fault(f) => return Err(HostError::Fault(f.clone())),
                HostState::Stopped | HostState::Stopping => return Err(HostError::NotRunning),
                _ => {}
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(HostError::Timeout(timeout));
            }
            guard = match self.shared.changed.wait_timeout(guard, deadline - now) {
                Ok((g, _)) => g,
                Err(_) => return Err(HostError::NotRunning),
            };
        }
    }

    /// Block until `pred(status)` holds or `timeout` passes. Returns whether it held.
    #[must_use]
    pub fn wait_for(&self, timeout: Duration, pred: impl Fn(&HostStatus) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        let Ok(mut guard) = self.shared.inner.lock() else {
            return false;
        };
        loop {
            if pred(&guard.status) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            guard = match self.shared.changed.wait_timeout(guard, deadline - now) {
                Ok((g, _)) => g,
                Err(_) => return false,
            };
        }
    }

    /// Ask the engine whether first-run setup has happened (`GET /info`).
    ///
    /// # Errors
    /// [`HostError::NotRunning`] while the engine is down.
    pub fn engine_info(&self) -> Result<DesktopInfo> {
        let ep = self.endpoint().ok_or(HostError::NotRunning)?;
        Ok(desktop::info(ep.port, &ep.token)?)
    }

    /// First-run setup: create the owner (and optionally the first company)
    /// through `POST /api/v1/desktop/initialize`. Builds no UI.
    ///
    /// # Errors
    /// [`HostError::Desktop`] with the engine's error code, for example
    /// `ALREADY_INITIALIZED`.
    pub fn initialize_engine(
        &self,
        admin: &AdminAccount,
        company: Option<&CompanySeed>,
    ) -> Result<Initialized> {
        let ep = self.endpoint().ok_or(HostError::NotRunning)?;
        desktop::initialize(ep.port, &ep.token, admin, company).map_err(|e| match e {
            DesktopError::Unreachable(_) => HostError::NotRunning,
            other => HostError::Desktop(other),
        })
    }

    /// Stop the engine, then the database, and wait for both.
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        // Dropping the sender wakes the worker even if it is sleeping.
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What the main loop does after handling a failure.
enum Flow {
    /// Go round again.
    Retry,
    /// Shut down (stop requested, or the supervisor gave up).
    Quit,
}

/// Why one start attempt failed.
struct StartFailure {
    permanent: bool,
    reason: String,
    tail: Vec<String>,
}

struct Worker {
    shared: Arc<Shared>,
    paths: AppPaths,
    engine_exe: PathBuf,
    policy: RestartPolicy,
    expected_commit: Option<String>,
    extra_env: Vec<(String, String)>,
    secrets_store: Option<Arc<dyn PasswordStore>>,
    log: HostLog,
    stop_rx: Receiver<()>,
}

impl Worker {
    fn new(config: HostConfig, shared: Arc<Shared>, stop_rx: Receiver<()>) -> Worker {
        // The log folder must exist before the log is opened; `run` repeats the
        // call and reports a failure as a fault.
        let _ = config.paths.ensure_dirs();
        Worker {
            log: HostLog::open(config.paths.host_log()),
            shared,
            paths: config.paths,
            engine_exe: config.engine_exe,
            policy: config.policy,
            expected_commit: config.expected_engine_commit,
            extra_env: config.extra_engine_env,
            secrets_store: config.secrets_store,
            stop_rx,
        }
    }

    /// Sleep up to `d`; true when a stop was requested meanwhile.
    fn wait_stop(&self, d: Duration) -> bool {
        match self.stop_rx.recv_timeout(d) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => true,
            Err(RecvTimeoutError::Timeout) => false,
        }
    }

    fn stop_requested(&self) -> bool {
        matches!(
            self.stop_rx.try_recv(),
            Ok(()) | Err(mpsc::TryRecvError::Disconnected)
        )
    }

    fn fault(
        &self,
        component: Component,
        reason: &str,
        restarts: u32,
        tail: Vec<String>,
        db: Option<&dyn DatabaseService>,
    ) {
        let mut log_paths = vec![self.paths.host_log(), self.paths.engine_log()];
        if let Some(db) = db {
            log_paths.extend(db.log_paths());
        }
        self.log.error(&format!(
            "giving up on the {component} after {restarts} restart(s): {reason}"
        ));
        let fault = HostFault {
            component,
            reason: reason.to_string(),
            restarts,
            log_paths,
            last_output: tail,
        };
        self.shared.update(|i| {
            i.endpoint = None;
            i.status.engine_pid = None;
            i.status.state = HostState::Fault(fault);
        });
    }

    fn load_secrets(&self) -> Result<(EngineSecrets, Vec<String>)> {
        let mut notices = Vec::new();
        let store: Arc<dyn PasswordStore> = if let Some(s) = &self.secrets_store {
            Arc::clone(s)
        } else {
            let choice = cl_pg::default_store_for(&self.paths.pg_data(), MASTER_PURPOSE);
            if let Some(n) = &choice.notice {
                notices.push(n.to_string());
            }
            Arc::from(choice.store)
        };
        // Only a cluster that does not exist yet may get a new secret (M2).
        let brand_new = !cl_pg::is_initialized(&self.paths.pg_data());
        let fingerprint = self.paths.run_dir().join("master.fp");
        let secrets = EngineSecrets::open(
            store.as_ref(),
            &self.paths.pg_data(),
            brand_new,
            &fingerprint,
        )?;
        Ok((secrets, notices))
    }

    #[allow(clippy::too_many_lines)] // one linear state machine reads better than pieces
    fn run(self, database: DatabaseFactory) {
        self.log.info("host starting");
        if let Err(e) = self.paths.ensure_dirs() {
            self.fault(Component::Database, &e.to_string(), 0, vec![], None);
            return;
        }
        self.prune_logs();
        if procs::reap_stale_child(&self.paths, &self.engine_exe) {
            self.log
                .warn("stopped an engine left over from a previous run");
        }
        let (secrets, mut notices) = match self.load_secrets() {
            Ok(s) => s,
            Err(e) => {
                self.fault(Component::Database, &e.to_string(), 0, vec![], None);
                return;
            }
        };
        let (mut db, db_notices) = match database(&secrets) {
            Ok(parts) => parts,
            Err(e) => {
                self.fault(Component::Database, &e.to_string(), 0, vec![], None);
                return;
            }
        };
        notices.extend(db_notices);
        self.shared.update(|i| i.status.notices = notices);

        let mut scrub_values: Vec<Secret> = secrets.all().into_iter().cloned().collect();
        let mut db_ep: Option<DbEndpoint> = None;
        let mut engine: Option<EngineProcess> = None;
        let mut db_fails = 0u32;
        let mut engine_fails = 0u32;
        let mut first = true;
        let mut generation = 0u64;
        let mut last_prune = Instant::now();

        'life: loop {
            if self.stop_requested() {
                break;
            }

            // 0. a database that died while the engine was down or starting
            if db_ep.as_ref().is_some_and(|ep| !db.is_alive(ep)) {
                match self.db_died(db.as_mut(), &mut engine, &mut db_ep, &mut db_fails) {
                    Flow::Quit => break 'life,
                    Flow::Retry => continue 'life,
                }
            }

            // 1. database
            if db_ep.is_none() {
                self.shared.set_state(if first {
                    HostState::Starting
                } else {
                    HostState::Restarting {
                        component: Component::Database,
                        attempt: db_fails,
                    }
                });
                match db.start() {
                    Ok(ep) => {
                        self.log.info("database ready");
                        scrub_values.push(ep.url.clone());
                        self.shared.update(|i| i.status.database_pid = Some(ep.pid));
                        db_ep = Some(ep);
                    }
                    Err(e) => {
                        db_fails += 1;
                        self.log.error(&format!("database start failed: {e}"));
                        if db_fails > self.policy.max_restarts {
                            self.fault(
                                Component::Database,
                                &e.to_string(),
                                db_fails - 1,
                                vec![],
                                Some(db.as_ref()),
                            );
                            break 'life;
                        }
                        self.shared
                            .update(|i| i.status.restarts.database = db_fails);
                        if self.wait_stop(self.policy.backoff_for(db_fails)) {
                            break 'life;
                        }
                        continue 'life;
                    }
                }
            }
            let Some(ep) = db_ep.clone() else { continue };

            // 2. engine
            let mut token = Secret::new("");
            let mut report: Option<HealthReport> = None;
            if engine.is_none() {
                self.shared.set_state(if first {
                    HostState::Starting
                } else {
                    HostState::Restarting {
                        component: Component::Engine,
                        attempt: engine_fails,
                    }
                });
                token = match Secret::generate() {
                    Ok(t) => t,
                    Err(e) => {
                        self.fault(
                            Component::Engine,
                            &e.to_string(),
                            0,
                            vec![],
                            Some(db.as_ref()),
                        );
                        break 'life;
                    }
                };
                let mut all = scrub_values.clone();
                all.push(token.clone());
                match self.start_engine(&ep, &token, &secrets, &Scrubber::new(all)) {
                    Ok((p, r)) => {
                        engine = Some(p);
                        report = Some(r);
                    }
                    Err(f) => {
                        if self.stop_requested() {
                            break 'life;
                        }
                        if !db.is_alive(&ep) {
                            // The engine fell over because the database did; the
                            // database is handled first and the engine's restart
                            // budget is left alone.
                            continue 'life;
                        }
                        engine_fails += 1;
                        self.log
                            .error(&format!("engine start failed: {}", f.reason));
                        if f.permanent || engine_fails > self.policy.max_restarts {
                            self.fault(
                                Component::Engine,
                                &f.reason,
                                engine_fails - 1,
                                f.tail,
                                Some(db.as_ref()),
                            );
                            break 'life;
                        }
                        self.shared
                            .update(|i| i.status.restarts.engine = engine_fails);
                        if self.wait_stop(self.policy.backoff_for(engine_fails)) {
                            break 'life;
                        }
                        continue 'life;
                    }
                }
            }

            // 3. running
            if let (Some(proc), Some(r)) = (engine.as_ref(), report) {
                generation += 1;
                let (pid, port) = (proc.pid, proc.port);
                self.log.info("engine ready");
                let tok = token.clone();
                self.shared.update(|i| {
                    i.status.engine_pid = Some(pid);
                    i.status.generation = generation;
                    i.status.engine_commit.clone_from(&r.engine_commit);
                    i.status.schema_version = r.schema_version;
                    i.endpoint = Some(Endpoint {
                        port,
                        token: tok,
                        generation,
                    });
                    i.status.state = HostState::Running;
                });
                first = false;
                // The token for health polling is the one this engine was started with.
                let (health_token, health_port) = (token.clone(), port);
                let mut stable_since = Instant::now();
                let mut last_health = Instant::now();
                let mut last_listener = Instant::now();
                let mut health_misses = 0u32;

                // 4. watch
                loop {
                    if self.wait_stop(self.policy.poll_interval) {
                        break 'life;
                    }
                    if last_prune.elapsed() >= Duration::from_hours(1) {
                        last_prune = Instant::now();
                        self.prune_logs();
                    }
                    let Some(proc) = engine.as_mut() else {
                        continue 'life;
                    };
                    let mut engine_problem: Option<(String, Vec<String>, bool)> = None;

                    if let Ok(Some(status)) = proc.child.try_wait() {
                        std::thread::sleep(Duration::from_millis(100));
                        let code = status.code();
                        let permanent = code == Some(engine::EXIT_CONFIG);
                        let fatal = proc.fatal_code();
                        let reason = format!(
                            "the engine stopped unexpectedly ({}{})",
                            code.map_or_else(|| "killed".to_string(), |c| format!("exit code {c}")),
                            fatal.map(|f| format!(", {f}")).unwrap_or_default()
                        );
                        engine_problem = Some((reason, proc.output_tail(), permanent));
                    } else if last_health.elapsed() >= self.policy.health_interval {
                        last_health = Instant::now();
                        if !db.is_alive(&ep) {
                            // The engine cannot work without the database. Take it
                            // down first, then bring the database back (not counted
                            // against the engine's restarts).
                            match self.db_died(db.as_mut(), &mut engine, &mut db_ep, &mut db_fails)
                            {
                                Flow::Quit => break 'life,
                                Flow::Retry => continue 'life,
                            }
                        }
                        match health::check_health(
                            health_port,
                            &health_token,
                            Duration::from_secs(3),
                        ) {
                            Ok(_) => health_misses = 0,
                            Err(HealthError::TokenRefused) => {
                                engine_problem = Some((
                                    "another program answered on the engine's port".into(),
                                    vec![],
                                    false,
                                ));
                            }
                            Err(e) => {
                                health_misses += 1;
                                self.log.warn(&format!(
                                    "engine health check failed ({health_misses}): {e}"
                                ));
                                if health_misses >= self.policy.health_failures {
                                    engine_problem = Some((
                                        "the engine stopped answering".into(),
                                        proc.output_tail(),
                                        false,
                                    ));
                                }
                            }
                        }
                    }
                    if engine_problem.is_none()
                        && last_listener.elapsed() >= self.policy.listener_check_every
                    {
                        last_listener = Instant::now();
                        if let Err(HealthError::NotOurProcess) =
                            health::verify_listener(health_port, proc.pid)
                        {
                            engine_problem = Some((
                                "the engine's port is held by another program".into(),
                                vec![],
                                false,
                            ));
                        }
                    }

                    if let Some((reason, tail, permanent)) = engine_problem {
                        self.log.error(&reason);
                        self.stop_engine(&mut engine);
                        self.shared.update(|i| {
                            i.endpoint = None;
                            i.status.engine_pid = None;
                        });
                        if !db.is_alive(&ep) {
                            // An engine without a database is not the engine's
                            // fault: handle the database first.
                            continue 'life;
                        }
                        engine_fails += 1;
                        if permanent || engine_fails > self.policy.max_restarts {
                            self.fault(
                                Component::Engine,
                                &reason,
                                engine_fails - 1,
                                tail,
                                Some(db.as_ref()),
                            );
                            break 'life;
                        }
                        self.shared.update(|i| {
                            i.status.restarts.engine = engine_fails;
                            i.status.state = HostState::Restarting {
                                component: Component::Engine,
                                attempt: engine_fails,
                            };
                        });
                        if self.wait_stop(self.policy.backoff_for(engine_fails)) {
                            break 'life;
                        }
                        continue 'life;
                    }

                    if stable_since.elapsed() >= self.policy.stable_after
                        && (engine_fails > 0 || db_fails > 0)
                    {
                        engine_fails = 0;
                        db_fails = 0;
                        stable_since = Instant::now();
                        self.shared
                            .update(|i| i.status.restarts = RestartCounts::default());
                    }
                }
            }
        }

        // Shutdown: engine first, then the database.
        let faulted = matches!(
            self.shared.inner.lock().map(|g| g.status.state.clone()),
            Ok(HostState::Fault(_))
        );
        if !faulted {
            self.shared.set_state(HostState::Stopping);
        }
        self.stop_engine(&mut engine);
        if let Err(e) = db.stop() {
            self.log
                .error(&format!("database did not stop cleanly: {e}"));
        }
        self.shared.update(|i| {
            i.endpoint = None;
            i.status.engine_pid = None;
            i.status.database_pid = None;
            if !matches!(i.status.state, HostState::Fault(_)) {
                i.status.state = HostState::Stopped;
            }
        });
        self.log.info("host stopped");
    }

    /// Apply the log retention rule (30 days or 100 MB, Q-118).
    fn prune_logs(&self) {
        let report = logs::prune_logs(
            &self.paths.logs_dir(),
            &LogPolicy::default(),
            std::time::SystemTime::now(),
        );
        if !report.removed.is_empty() {
            self.log.info(&format!(
                "removed {} old log file(s), {} bytes",
                report.removed.len(),
                report.freed_bytes
            ));
        }
    }

    /// The database stopped answering: stop the engine, stop the database
    /// (so a half-dead server is gone), and schedule a restart.
    fn db_died(
        &self,
        db: &mut dyn DatabaseService,
        engine: &mut Option<EngineProcess>,
        db_ep: &mut Option<DbEndpoint>,
        db_fails: &mut u32,
    ) -> Flow {
        self.log.warn("database stopped answering");
        self.stop_engine(engine);
        self.shared.update(|i| {
            i.endpoint = None;
            i.status.engine_pid = None;
        });
        let _ = db.stop();
        *db_ep = None;
        *db_fails += 1;
        if *db_fails > self.policy.max_restarts {
            self.fault(
                Component::Database,
                "the database stopped and could not be restarted",
                *db_fails - 1,
                vec![],
                Some(&*db),
            );
            return Flow::Quit;
        }
        let attempt = *db_fails;
        self.shared.update(|i| {
            i.status.restarts.database = attempt;
            i.status.state = HostState::Restarting {
                component: Component::Database,
                attempt,
            };
        });
        if self.wait_stop(self.policy.backoff_for(attempt)) {
            Flow::Quit
        } else {
            Flow::Retry
        }
    }

    fn stop_engine(&self, engine: &mut Option<EngineProcess>) {
        if let Some(mut p) = engine.take() {
            self.log.info("stopping the engine");
            p.terminate(self.policy.stop_grace);
        }
        procs::forget_child(&self.paths);
    }

    /// Launch the engine and wait until it is verifiably ours and healthy.
    fn start_engine(
        &self,
        ep: &DbEndpoint,
        token: &Secret,
        secrets: &EngineSecrets,
        scrubber: &Scrubber,
    ) -> std::result::Result<(EngineProcess, HealthReport), StartFailure> {
        let plan = LaunchPlan {
            exe: &self.engine_exe,
            paths: &self.paths,
            token,
            db_url: &ep.url,
            secrets,
            extra_env: &self.extra_env,
        };
        let cancelled = || self.stop_requested();
        let mut proc = engine::launch(&plan, scrubber, self.policy.ready_timeout, &cancelled)
            .map_err(|e: LaunchError| StartFailure {
                permanent: e.is_permanent(),
                reason: e.describe(),
                tail: e.tail(),
            })?;

        let verdict = self.await_healthy(&mut proc, token);
        match verdict {
            Ok(report) => {
                if let Some(want) = &self.expected_commit
                    && report.engine_commit.as_deref() != Some(want.as_str())
                {
                    let tail = proc.output_tail();
                    proc.terminate(Duration::from_secs(2));
                    procs::forget_child(&self.paths);
                    return Err(StartFailure {
                        permanent: true,
                        reason: "the engine is not the version this app was built with".into(),
                        tail,
                    });
                }
                if report.superuser != Some(false) {
                    let tail = proc.output_tail();
                    proc.terminate(Duration::from_secs(2));
                    procs::forget_child(&self.paths);
                    return Err(StartFailure {
                        permanent: true,
                        reason: "the engine is not running with the restricted database role"
                            .into(),
                        tail,
                    });
                }
                Ok((proc, report))
            }
            Err(f) => {
                let tail = if f.tail.is_empty() {
                    proc.output_tail()
                } else {
                    f.tail.clone()
                };
                proc.terminate(Duration::from_secs(2));
                procs::forget_child(&self.paths);
                Err(StartFailure { tail, ..f })
            }
        }
    }

    fn await_healthy(
        &self,
        proc: &mut EngineProcess,
        token: &Secret,
    ) -> std::result::Result<HealthReport, StartFailure> {
        let fail = |permanent: bool, reason: &str| StartFailure {
            permanent,
            reason: reason.to_string(),
            tail: Vec::new(),
        };
        // Who is listening? Check before the token is sent to anyone.
        match health::verify_listener(proc.port, proc.pid) {
            Ok(()) => {}
            Err(HealthError::NotOurProcess) => {
                return Err(fail(false, "the engine's port is held by another program"));
            }
            Err(e) => self
                .log
                .warn(&format!("could not verify the engine's port: {e}")),
        }
        let deadline = Instant::now() + self.policy.health_ready_timeout;
        loop {
            if self.stop_requested() {
                return Err(fail(false, "start cancelled"));
            }
            if let Ok(Some(status)) = proc.child.try_wait() {
                return Err(fail(
                    status.code() == Some(engine::EXIT_CONFIG),
                    "the engine stopped while starting",
                ));
            }
            match health::check_health(proc.port, token, Duration::from_secs(3)) {
                Ok(r) => return Ok(r),
                Err(HealthError::TokenRefused) => {
                    return Err(fail(false, "another program answered on the engine's port"));
                }
                Err(_) => {}
            }
            if Instant::now() >= deadline {
                return Err(fail(false, "the engine never reported healthy"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// A [`HostConfig`] that keeps every secret in memory instead of the OS
/// keychain. For tests and tools that must not touch the owner's keychain.
#[must_use]
pub fn in_memory_stores(mut config: HostConfig) -> HostConfig {
    config.pg_password_store = Some(Arc::new(MemoryStore::new()));
    config.secrets_store = Some(Arc::new(MemoryStore::new()));
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_repeats_the_last_value() {
        let p = RestartPolicy::default();
        assert_eq!(p.backoff_for(1), Duration::from_millis(500));
        assert_eq!(p.backoff_for(2), Duration::from_secs(1));
        assert_eq!(p.backoff_for(3), Duration::from_secs(2));
        assert_eq!(p.backoff_for(9), Duration::from_secs(2));
        assert_eq!(p.backoff_for(0), Duration::from_millis(500));
    }

    #[test]
    fn the_default_policy_is_three_restarts() {
        assert_eq!(RestartPolicy::default().max_restarts, 3);
    }

    #[test]
    fn endpoint_debug_hides_the_token() {
        let e = Endpoint {
            port: 1,
            token: Secret::new("sekret-token"),
            generation: 2,
        };
        assert!(!format!("{e:?}").contains("sekret-token"));
    }

    #[test]
    fn status_summary_has_no_ports_or_secrets() {
        let s = HostStatus {
            state: HostState::Restarting {
                component: Component::Engine,
                attempt: 2,
            },
            database_pid: Some(10),
            engine_pid: None,
            restarts: RestartCounts {
                database: 0,
                engine: 2,
            },
            generation: 3,
            notices: vec![],
            engine_commit: None,
            schema_version: None,
        };
        let line = s.summary();
        assert!(line.contains("state=restarting-engine-2"));
        assert!(line.contains("engine_pid=-"));
        assert!(line.contains("restarts_engine=2"));
    }

    #[test]
    fn a_fault_names_the_logs() {
        let f = HostFault {
            component: Component::Database,
            reason: "x".into(),
            restarts: 3,
            log_paths: vec![PathBuf::from("/a/host.log")],
            last_output: vec![],
        };
        let text = f.to_string();
        assert!(text.contains("database") && text.contains("host.log"));
    }
}
