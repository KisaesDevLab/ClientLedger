//! Contract harness: the real bundled `PostgreSQL` and the real engine
//! executable, started through `cl-host`, driven through `cl-api`
//! (P05 plan §2.10).
//!
//! - One engine and one fresh cluster per test binary (a scratch app-data
//!   folder under the temp dir; `CL_API_KEEP=1` keeps it).
//! - The binaries use their own runner (`harness = false`): tests run in a
//!   fixed order on one tokio runtime, each under a time budget, and the
//!   supervisor is always stopped at the end, so no engine or postgres
//!   process outlives the binary even when a test panics.
//! - Missing engine or `PostgreSQL` binaries: `SKIPPED` locally, a failure
//!   when `CL_API_REQUIRE=1` (CI).
//! - Logs in once per session (the engine rate-limits logins: 10/15 min per
//!   email, 10/min per IP) and fails the binary after more than 8 calls to
//!   rate-limited auth routes.
//! - Capability-gated assertions print `PENDING(U-xx)` and are listed in
//!   `target/<binary>-pending.txt`; `CL_API_REQUIRE_CAPS=a,b` turns a missing
//!   capability into a failure.
//! - Only loopback traffic: the engine, `PostgreSQL` and the client all use
//!   `127.0.0.1`.
#![allow(
    dead_code,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cl_api::testing::{ObserverAction, RequestObserver};
use cl_api::{
    Capabilities, Client, ClientConfig, CompanySummary, EndpointSource, EndpointState,
    EngineEndpoint, LoginOutcome, Method, Session,
};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_host::{AdminAccount, CompanySeed, HostConfig, HostState, OfflineSetting, Supervisor};

/// The owner account the harness creates at first run.
pub const OWNER_EMAIL: &str = "owner@example.com";
/// The company created by `/desktop/initialize`.
pub const CONTRACT_CO: &str = "Contract Co";
/// Calls to rate-limited auth routes allowed per binary (limit 10/min/IP).
const AUTH_LIMIT: u64 = 8;

fn env_is(key: &str, value: &str) -> bool {
    std::env::var(key).as_deref() == Ok(value)
}

fn env_secs(key: &str, default: u64) -> Duration {
    Duration::from_secs(
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default),
    )
}

/// Which engine a binary needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineMode {
    /// `CL_OFFLINE=auto` (default product setting).
    Normal,
    /// `CL_OFFLINE=on` ("Work offline").
    Offline,
}

// ---------------------------------------------------------------- logging

/// Everything `tracing` printed during the run (DEBUG and above), for the
/// logging canary.
#[derive(Clone, Default)]
pub struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl LogCapture {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl std::io::Write for LogCapture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogCapture;
    fn make_writer(&'a self) -> LogCapture {
        self.clone()
    }
}

// --------------------------------------------------------------- observer

/// Counts attempts per route and can lose one reply on purpose.
#[derive(Default)]
pub struct Observer {
    sent: Mutex<BTreeMap<String, u64>>,
    drop_next: Mutex<Option<(Method, String)>>,
    dropped: Mutex<u64>,
}

fn key(method: Method, route: &str) -> String {
    format!("{} {route}", method.as_str())
}

impl Observer {
    /// Attempts sent so far for `method route` (route template).
    pub fn sent(&self, method: Method, route: &str) -> u64 {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key(method, route))
            .copied()
            .unwrap_or(0)
    }

    /// All attempts sent so far.
    pub fn total(&self) -> u64 {
        self.sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .sum()
    }

    /// Lose the next 2xx reply to `method route` (after the engine answered).
    pub fn drop_next_reply(&self, method: Method, route: &str) {
        *self
            .drop_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((method, route.to_string()));
    }

    /// Replies lost so far.
    pub fn dropped(&self) -> u64 {
        *self.dropped.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn auth_limited(&self) -> u64 {
        let sent = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        sent.iter()
            .filter(|(k, _)| {
                k.ends_with("/api/v1/auth/login")
                    || k.contains("/api/v1/auth/tfa/")
                    || k.ends_with("/api/v1/auth/create-tenant")
            })
            .map(|(_, v)| *v)
            .sum()
    }
}

impl RequestObserver for Observer {
    fn before_send(&self, method: Method, route: &str) {
        *self
            .sent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key(method, route))
            .or_default() += 1;
        let used = self.auth_limited();
        assert!(
            used <= AUTH_LIMIT,
            "the contract run made {used} rate-limited auth calls (budget {AUTH_LIMIT})"
        );
    }

    fn on_reply(&self, method: Method, route: &str, status: u16) -> ObserverAction {
        let mut slot = self
            .drop_next
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let hit = slot
            .as_ref()
            .is_some_and(|(m, r)| *m == method && r == route && (200..300).contains(&status));
        if hit {
            *slot = None;
            *self.dropped.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            return ObserverAction::DropReply;
        }
        ObserverAction::Pass
    }
}

// --------------------------------------------------------------- endpoint

/// cl-api's view of the supervisor (the production adapter lives in cl-ui,
/// P06; this one maps the same snapshot the same way).
pub struct SupervisorSource(pub Arc<Supervisor>);

impl EndpointSource for SupervisorSource {
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

// ---------------------------------------------------------------- scratch

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        if env_is("CL_API_KEEP", "1") {
            eprintln!("kept {}", self.0.display());
            return;
        }
        for _ in 0..40 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

/// The workspace `target/` directory (for the PENDING list).
fn target_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|d| d.file_name().is_some_and(|n| n == "target"))
        .map(Path::to_path_buf)
}

fn engine_exe() -> Option<PathBuf> {
    if let Some(p) = cl_host::find_engine_exe(None) {
        return Some(p);
    }
    let name = cl_host::engine_exe_name();
    let dir = target_dir()?.join("engine");
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path().join(name))
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------- harness

/// One running engine plus the owner's sessions.
pub struct Harness {
    rt: tokio::runtime::Runtime,
    pub sup: Arc<Supervisor>,
    pub source: Arc<dyn EndpointSource>,
    /// The client every session of this binary uses.
    pub client: Client,
    pub observer: Arc<Observer>,
    pub logs: LogCapture,
    pub password: Secret,
    /// S1: the owner in Contract Co.
    pub s1: Session,
    /// Contract Co's tenant.
    pub contract_co: TenantId,
    pub caps: Capabilities,
    pub data_dir: PathBuf,
    budget: Duration,
    pending: Mutex<Vec<String>>,
    current_test: Mutex<String>,
    s2: Mutex<Option<Session>>,
    s3: Mutex<Option<Session>>,
    tenant_b: Mutex<Option<TenantId>>,
    scratch: Scratch,
}

fn random_password() -> Secret {
    let mut s = String::from("Cl-");
    while s.len() < 23 {
        s.push_str(&uuid_hex());
    }
    s.truncate(23);
    Secret::new(s)
}

fn uuid_hex() -> String {
    // The engine is the only dependency that needs randomness here; a fresh
    // v4 id from cl-api's idempotency key is enough for test data.
    cl_api::IdempotencyKey::new().as_header().replace('-', "")
}

/// A short random suffix for unique names inside the shared engine.
pub fn unique(prefix: &str) -> String {
    format!("{prefix} {}", &uuid_hex()[..10])
}

impl Harness {
    fn boot(mode: EngineMode, logs: LogCapture) -> Result<Harness, String> {
        let engine =
            engine_exe().ok_or("the engine executable is missing (run `cargo xtask engine`)")?;
        let pg = cl_pg::PgBinaries::discover(None)
            .map_err(|_| "the PostgreSQL binaries are missing (run `cargo xtask pg-fetch`)")?;
        let tag = match mode {
            EngineMode::Normal => "contract",
            EngineMode::Offline => "contract-offline",
        };
        let root = std::env::temp_dir().join(format!("cl-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).map_err(|e| format!("scratch dir: {e}"))?;
        let scratch = Scratch(root.clone());

        let started = Instant::now();
        let paths = cl_host::AppPaths::at(root.join("data")).map_err(|e| e.to_string())?;
        let mut cfg = cl_host::in_memory_stores(HostConfig::new(paths.clone(), engine));
        cfg.pg_binaries = Some(pg);
        cfg.offline = match mode {
            EngineMode::Normal => OfflineSetting::Auto,
            EngineMode::Offline => OfflineSetting::On,
        };
        cfg.policy.ready_timeout = Duration::from_mins(3);
        cfg.policy.health_ready_timeout = Duration::from_mins(2);
        cfg.policy.health_interval = Duration::from_secs(1);
        cfg.policy.listener_check_every = Duration::from_secs(2);
        let guard = match cl_host::acquire(&cfg.paths).map_err(|e| e.to_string())? {
            cl_host::Acquired::First(g) => g,
            cl_host::Acquired::Second => return Err("instance lock held".into()),
        };
        let sup = Arc::new(Supervisor::start(cfg, guard).map_err(|e| format!("start: {e}"))?);

        let password = random_password();
        sup.initialize_engine(
            &AdminAccount {
                email: OWNER_EMAIL.into(),
                display_name: "Contract Owner".into(),
                password: cl_host::Secret::new(password.expose().to_string()),
            },
            Some(&CompanySeed {
                name: CONTRACT_CO.into(),
                business_type: Some("default".into()),
                ..CompanySeed::default()
            }),
        )
        .map_err(|e| format!("initialize: {e}"))?;

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let source: Arc<dyn EndpointSource> = Arc::new(SupervisorSource(Arc::clone(&sup)));
        let client =
            Client::new(Arc::clone(&source), ClientConfig::default()).map_err(|e| e.to_string())?;
        let observer = Arc::new(Observer::default());
        client.set_observer(Some(Arc::clone(&observer) as Arc<dyn RequestObserver>));

        let (caps, s1, contract_co) = rt.block_on(async {
            client
                .desktop_info()
                .await
                .map_err(|e| format!("desktop info: {e}"))?;
            let s1 = login(&client, &password).await?;
            let co = find_company(&s1, CONTRACT_CO).await?;
            s1.open_company(co.tenant_id)
                .await
                .map_err(|e| format!("open company: {e}"))?;
            Ok::<_, String>((client.capabilities(), s1, co.tenant_id))
        })?;
        let boot = started.elapsed();
        let boot_budget = env_secs("CL_API_BOOT_BUDGET_SECS", 90);
        eprintln!("contract harness ({tag}) booted in {boot:.1?} (budget {boot_budget:?})");
        if boot > boot_budget {
            return Err(format!(
                "boot took {boot:?}, over the {boot_budget:?} budget"
            ));
        }
        Ok(Harness {
            rt,
            sup,
            source,
            client,
            observer,
            logs,
            password,
            s1,
            contract_co,
            caps,
            data_dir: root.join("data"),
            budget: env_secs("CL_API_TEST_BUDGET_SECS", 20),
            pending: Mutex::new(Vec::new()),
            current_test: Mutex::new(String::new()),
            s2: Mutex::new(None),
            s3: Mutex::new(None),
            tenant_b: Mutex::new(None),
            scratch,
        })
    }

    /// Run `fut` on the harness runtime under the per-test budget.
    pub fn run<T>(&self, fut: impl Future<Output = T>) -> T {
        self.run_for(self.budget, fut)
    }

    /// Like [`Harness::run`] with an explicit budget (scaled like the default
    /// when `CL_API_TEST_BUDGET_SECS` raises it, e.g. under coverage).
    pub fn run_for<T>(&self, budget: Duration, fut: impl Future<Output = T>) -> T {
        let scale = self.budget.as_secs().max(20) / 20;
        let budget = budget * u32::try_from(scale).unwrap_or(1);
        match self
            .rt
            .block_on(async { tokio::time::timeout(budget, fut).await })
        {
            Ok(v) => v,
            Err(tokio::time::error::Elapsed { .. }) => {
                panic!("test ran over its {budget:?} budget")
            }
        }
    }

    /// Log the owner in again (a new session; at most 3 live per user).
    pub fn login(&self) -> Session {
        self.run(login(&self.client, &self.password))
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// A second session opened on Contract Co.
    pub fn second_session(&self) -> Session {
        let s = self.login();
        self.run(s.open_company(self.contract_co))
            .unwrap_or_else(|e| panic!("open company: {e}"));
        s
    }

    /// S2: a second owner session on Contract Co (the "other window" of the
    /// conflict tests). Logged in on first use.
    pub fn s2(&self) -> Session {
        let mut slot = self.s2.lock().unwrap_or_else(PoisonError::into_inner);
        slot.get_or_insert_with(|| self.second_session()).clone()
    }

    /// Tenant B: a second company, created on first use. Tests that change
    /// company-wide state (the closing date, the deliberate money-hole probe)
    /// use it so no other assertion can see their effects.
    pub fn tenant_b(&self) -> TenantId {
        let mut slot = self.tenant_b.lock().unwrap_or_else(PoisonError::into_inner);
        *slot.get_or_insert_with(|| {
            let made = self
                .run(self.s1.create_company(&cl_api::NewCompany {
                    company_name: unique("Tenant B"),
                    business_type: Some("default".into()),
                    entity_type: None,
                }))
                .unwrap_or_else(|e| panic!("create tenant B: {e}"));
            made.tenant_id
        })
    }

    /// S3: the "switcher" session, opened on tenant B.
    pub fn s3(&self) -> Session {
        let b = self.tenant_b();
        let mut slot = self.s3.lock().unwrap_or_else(PoisonError::into_inner);
        let s = slot.get_or_insert_with(|| self.login()).clone();
        drop(slot);
        if !matches!(s.scope(), cl_api::Scope::Company { tenant_id, .. } if tenant_id == b) {
            self.run(s.open_company(b))
                .unwrap_or_else(|e| panic!("open tenant B: {e}"));
        }
        s
    }

    /// `true` when the engine advertises `cap`; otherwise prints
    /// `PENDING(<uitem>)` and records it (or fails under
    /// `CL_API_REQUIRE_CAPS`).
    pub fn require_cap(&self, cap: &str, uitem: &str) -> bool {
        if self.caps.has(cap) {
            return true;
        }
        let required = std::env::var("CL_API_REQUIRE_CAPS").unwrap_or_default();
        assert!(
            !required.split(',').any(|c| c.trim() == cap),
            "CL_API_REQUIRE_CAPS lists {cap} but the engine does not advertise it"
        );
        let test = self
            .current_test
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let line = format!("PENDING({uitem}): {test} (needs capability `{cap}`)");
        eprintln!("    {line}");
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line);
        false
    }

    /// Kill the engine process hard (our own child) and wait for the
    /// supervisor to bring up the next generation.
    pub fn kill_engine_and_wait(&self) -> u64 {
        let before = self.sup.status();
        let pid = before.engine_pid.expect("engine pid");
        kill_pid(pid);
        let next = before.generation + 1;
        assert!(
            self.sup
                .wait_for(Duration::from_mins(3), |s| s.generation >= next
                    && s.state == HostState::Running),
            "engine did not come back: {}",
            self.sup.status().summary()
        );
        self.sup.status().generation
    }

    fn shutdown(self) -> (Vec<String>, Vec<u32>) {
        let status = self.sup.status();
        let children: Vec<u32> = [status.engine_pid, status.database_pid]
            .into_iter()
            .flatten()
            .collect();
        let Harness {
            rt,
            sup,
            source,
            client,
            s1,
            s2,
            s3,
            pending,
            scratch,
            ..
        } = self;
        let pending = pending.into_inner().unwrap_or_else(PoisonError::into_inner);
        // Every session holds the client, which holds the endpoint source,
        // which holds the supervisor: log them out and drop them all first.
        let others = [s2, s3]
            .into_iter()
            .filter_map(|m| m.into_inner().unwrap_or_else(PoisonError::into_inner));
        for s in std::iter::once(s1).chain(others) {
            let _ = rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), s.logout()).await
            });
        }
        drop(client);
        drop(source);
        rt.shutdown_timeout(Duration::from_secs(5));
        match Arc::try_unwrap(sup) {
            Ok(s) => s.stop(),
            Err(still_shared) => {
                eprintln!("warning: supervisor still shared at teardown; stopping on drop");
                drop(still_shared);
            }
        }
        // No orphans: both children must be gone shortly after the stop.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut alive: Vec<u32> = children.clone();
        while !alive.is_empty() && Instant::now() < deadline {
            alive.retain(|p| pid_alive(*p));
            if !alive.is_empty() {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        drop(scratch);
        (pending, alive)
    }
}

/// Whether `pid` is still running (our own children only).
fn pid_alive(pid: u32) -> bool {
    if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\"")))
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
}

fn kill_pid(pid: u32) {
    let status = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .status()
    } else {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
    };
    assert!(status.is_ok_and(|s| s.success()), "could not kill {pid}");
}

/// Log the owner in (no TFA in a fresh engine).
pub async fn login(client: &Client, password: &Secret) -> Result<Session, String> {
    match client.login(OWNER_EMAIL, password).await {
        Ok(LoginOutcome::Session(s)) => Ok(s),
        Ok(LoginOutcome::TfaRequired(_)) => Err("unexpected two-step verification".into()),
        Err(e) => Err(format!("login: {e}")),
    }
}

/// The company named `name` in the session's company list.
pub async fn find_company(s: &Session, name: &str) -> Result<CompanySummary, String> {
    s.companies()
        .await
        .map_err(|e| format!("companies: {e}"))?
        .into_iter()
        .find(|c| c.tenant_name == name)
        .ok_or_else(|| format!("company {name} not listed"))
}

// ----------------------------------------------------------------- runner

/// A contract test: a plain function over the shared harness.
pub type TestFn = fn(&Harness);

/// Run `tests` in order against one engine. Arguments: substring filters;
/// `--list` lists the tests; libtest flags (`--test-threads`, `--nocapture`,
/// …) are accepted and ignored (the order is fixed and serial).
pub fn run_suite(mode: EngineMode, tests: &[(&str, TestFn)]) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        for (name, _) in tests {
            println!("{name}: test");
        }
        return ExitCode::SUCCESS;
    }
    let mut filters = Vec::new();
    let mut skip_value = false;
    for a in &args {
        if skip_value {
            skip_value = false;
            continue;
        }
        if a.starts_with("--") {
            skip_value = matches!(
                a.as_str(),
                "--test-threads" | "--format" | "--color" | "--skip"
            );
            continue;
        }
        filters.push(a.clone());
    }
    let selected: Vec<_> = tests
        .iter()
        .filter(|(n, _)| filters.is_empty() || filters.iter().any(|f| n.contains(f.as_str())))
        .collect();
    if selected.is_empty() {
        println!("running 0 tests\n\ntest result: ok. 0 passed; 0 failed");
        return ExitCode::SUCCESS;
    }

    let logs = LogCapture::default();
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(logs.clone())
        .try_init();

    let h = match catch_unwind(AssertUnwindSafe(|| Harness::boot(mode, logs))) {
        Ok(Ok(h)) => h,
        Ok(Err(why)) if why.contains("is missing") && !env_is("CL_API_REQUIRE", "1") => {
            eprintln!("SKIPPED: {why} (CI sets CL_API_REQUIRE=1)");
            println!("running 0 tests\n\ntest result: ok. 0 passed; 0 failed");
            return ExitCode::SUCCESS;
        }
        Ok(Err(why)) => {
            eprintln!("contract harness failed to start: {why}");
            return ExitCode::FAILURE;
        }
        Err(_) => {
            eprintln!("contract harness panicked while starting");
            return ExitCode::FAILURE;
        }
    };

    println!("running {} tests", selected.len());
    let mut failed = Vec::new();
    for (name, f) in &selected {
        *h.current_test
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = (*name).to_string();
        let t0 = Instant::now();
        let ok = catch_unwind(AssertUnwindSafe(|| f(&h))).is_ok();
        let took = t0.elapsed();
        println!(
            "test {name} ... {} ({:.2}s)",
            if ok { "ok" } else { "FAILED" },
            took.as_secs_f64()
        );
        if !ok {
            failed.push(*name);
        }
    }
    let (pending, orphans) = h.shutdown();
    if !orphans.is_empty() {
        eprintln!("orphan processes after teardown: {orphans:?}");
        failed.push("teardown::no_orphan_processes");
    }
    if !pending.is_empty() {
        println!("\npending (capability-gated, not failures):");
        for p in &pending {
            println!("    {p}");
        }
    }
    // One file per binary, rewritten each run, for the PR body.
    if let Some(dir) = target_dir() {
        let tag = match mode {
            EngineMode::Normal => "contract",
            EngineMode::Offline => "contract-offline",
        };
        let _ = std::fs::write(
            dir.join(format!("{tag}-pending.txt")),
            pending.iter().fold(String::new(), |mut acc, p| {
                acc.push_str(p);
                acc.push('\n');
                acc
            }),
        );
    }
    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed.is_empty() { "ok" } else { "FAILED" },
        selected.len().saturating_sub(failed.len()),
        failed.len()
    );
    if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        for f in failed {
            eprintln!("failed: {f}");
        }
        ExitCode::FAILURE
    }
}
