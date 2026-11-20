//! P33 D5: the office network, live. A real host (the bundled `PostgreSQL`,
//! the engine in LAN mode, and the app's own [`LanHost`] proxy on loopback
//! through the test-only `lan-test-listener` feature) and real workstations
//! ([`LanWorkstation`]: the loopback forwarder and heartbeat), talking
//! through cl-api exactly as the app does.
//!
//! - `office_network_live`: sign-in, company list and open through the
//!   host; two workstations editing the same journal entry (the second save
//!   gets the conflict, and nothing is lost once it is merged); host-only
//!   routes refused from a workstation; reconnect after an engine restart,
//!   a proxy restart, and a host restart with a new sign-in, data intact;
//!   a public interface refused; a pin mismatch refused; the
//!   request-smuggling shapes refused by the live proxy.
//! - `office_network_soak` (`cargo xtask lan-soak`): five workstations
//!   saving for `CL_LAN_SOAK_SECS` seconds (unset: not run).
//!
//! Without the engine executable or the `PostgreSQL` binaries the tests
//! print `SKIPPED` and pass, unless `CL_API_REQUIRE=1` (CI's engine job).
//! Only loopback traffic; scratch folders under the temp dir; the host is
//! stopped at the end, also when an assertion fails. Money is built from
//! exact strings, never floats.
#![allow(
    clippy::too_many_lines,
    reason = "each test is one linear scenario over one booted host"
)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use cl_api::{
    AccountFields, AccountType, ApiError, Basis, Client, ClientConfig, ConflictReason,
    IdempotencyKey, JeLine, LanRefusal, LoginOutcome, NewJournalEntry, PostingOptions,
    RecordVersion, Session, Side,
};
use cl_core::ids::AccountId;
use cl_core::{CalendarDate, Money, Secret};
use cl_host::{AdminAccount, CompanySeed, HostConfig, HostState, Supervisor};
use cl_lan::client::LinkState;
use cl_lan::discovery::FakeDiscovery;
use cl_lan::policy::FakeInterfaces;
use cl_lan::settings::{HostSettings, WorkstationSettings};
use cl_lan::tls::{Identity, Role};
use cl_settings::MemorySecrets;
use cl_ui::engine_link::SnapshotSource;
use cl_ui::lan_link::{LanHost, LanWorkstation, lan_dir};

const OWNER: &str = "owner@example.com";
const BOOKS: &str = "Office Network Books";

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
    // Where the built programs go: cargo says (the build folder may be
    // elsewhere); else the workspace's own `target`.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let target = std::process::Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .and_then(|v| v.get("target_directory")?.as_str().map(PathBuf::from))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    std::fs::read_dir(target.join("engine"))
        .ok()?
        .flatten()
        .map(|e| e.path().join(cl_host::engine_exe_name()))
        .find(|p| p.is_file())
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

fn unique(prefix: &str) -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    format!("{prefix} {t}-{n}")
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime")
}

fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let until = Instant::now() + timeout;
    while Instant::now() < until {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("timed out waiting for {what}");
}

/// Kill one of the host's own child processes hard (the engine), as a
/// crash would. Test-only, our own child.
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

/// The host computer: supervisor (database + engine in LAN mode) and the
/// app's proxy, with its own keychain (in memory) and data folder.
struct Host {
    root: PathBuf,
    exe: PathBuf,
    store: Arc<MemorySecrets>,
    sup: Option<Arc<Supervisor>>,
    lan: Option<LanHost>,
    /// The proxy's port (fixed after the first start, so workstations find
    /// the host again after a restart).
    port: u16,
    password: Secret,
    /// The host keychain's database and engine secrets (kept across
    /// restarts).
    host_stores: Option<HostStores>,
}

type HostStores = (
    Option<Arc<dyn cl_host::PasswordStore>>,
    Option<Arc<dyn cl_host::PasswordStore>>,
);

impl Host {
    fn boot(root: &Path, exe: PathBuf) -> Result<Host, String> {
        let password = Secret::new(format!("Lan-{}", &key().as_header()[..18]));
        let mut h = Host {
            root: root.to_path_buf(),
            exe,
            store: Arc::new(MemorySecrets::new()),
            sup: None,
            lan: None,
            port: 0,
            password,
            host_stores: None,
        };
        h.try_start_supervisor()?;
        h.sup()
            .initialize_engine(
                &AdminAccount {
                    email: OWNER.into(),
                    display_name: "Office Owner".into(),
                    password: cl_host::Secret::new(h.password.expose().to_string()),
                },
                Some(&CompanySeed {
                    name: BOOKS.into(),
                    business_type: Some("default".into()),
                    ..CompanySeed::default()
                }),
            )
            .map_err(|e| format!("first-run setup on the host: {e}"))?;
        h.start_proxy();
        Ok(h)
    }

    fn sup(&self) -> &Arc<Supervisor> {
        self.sup.as_ref().expect("host running")
    }

    fn start_supervisor(&mut self) {
        self.try_start_supervisor().expect("host supervisor");
    }

    fn try_start_supervisor(&mut self) -> Result<(), String> {
        let paths = cl_host::AppPaths::at(self.root.join("data")).map_err(|e| e.to_string())?;
        let mut cfg = HostConfig::new(paths.clone(), self.exe.clone());
        // The host's keychain outlives a restart (the database password and
        // the engine's master secret), as the OS keychain does.
        let stores = self
            .host_stores
            .get_or_insert_with(|| {
                let c = cl_host::in_memory_stores(HostConfig::new(paths.clone(), self.exe.clone()));
                (c.pg_password_store, c.secrets_store)
            })
            .clone();
        (cfg.pg_password_store, cfg.secrets_store) = stores;
        cfg.lan_host = true;
        cfg.policy.ready_timeout = Duration::from_mins(3);
        cfg.policy.health_ready_timeout = Duration::from_mins(2);
        cfg.policy.health_interval = Duration::from_secs(1);
        let guard = match cl_host::acquire(&paths).map_err(|e| e.to_string())? {
            cl_host::Acquired::First(g) => g,
            cl_host::Acquired::Second => return Err("instance lock held".into()),
        };
        let sup = Supervisor::start(cfg, guard).map_err(|e| e.to_string())?;
        self.sup = Some(Arc::new(sup));
        Ok(())
    }

    fn settings(&self, ip: &str) -> HostSettings {
        HostSettings {
            interface: "lo".into(),
            ip: ip.parse().expect("ip"),
            port: self.port,
            announce: false,
            background: false,
        }
    }

    fn try_proxy(&self, ip: &str) -> Result<LanHost, cl_ui::lan_link::LanStartError> {
        LanHost::start(
            Arc::clone(self.sup()) as Arc<dyn SnapshotSource>,
            &self.settings(ip),
            &self.root,
            self.store.as_ref(),
            Arc::new(FakeInterfaces::default()),
            Arc::new(FakeDiscovery::default()),
        )
    }

    fn start_proxy(&mut self) {
        let lan = self.try_proxy("127.0.0.1").expect("proxy");
        let mut addr = None;
        wait_until("the proxy to listen", Duration::from_secs(10), || {
            if let cl_lan::host::ProxyStatus::Listening(a) = lan.status() {
                addr = Some(a);
                true
            } else {
                false
            }
        });
        let addr = addr.expect("addr");
        if self.port == 0 {
            self.port = addr.port();
        }
        assert_eq!(addr.port(), self.port, "the host keeps its port");
        self.lan = Some(lan);
    }

    fn stop_proxy(&mut self) {
        drop(self.lan.take());
    }

    fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    fn lan(&self) -> &LanHost {
        self.lan.as_ref().expect("proxy running")
    }

    /// Pairs a fresh workstation (its own data folder and keychain),
    /// clicking Allow on the host as the owner would.
    fn pair(&self, ws_root: &Path, store: &MemorySecrets, label: &str) -> WorkstationSettings {
        let lan = self.lan();
        let code = lan
            .pairing()
            .new_code(&lan.fingerprint())
            .expect("pairing code");
        let pairing = Arc::clone(lan.pairing());
        let clicker = std::thread::spawn(move || {
            for _ in 0..1000 {
                if let Some(p) = pairing.pending() {
                    return pairing.decide(p.id, true);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        });
        let (device, _) =
            Identity::load_or_create(store, &lan_dir(ws_root), Role::Device).expect("device");
        let fp = rt()
            .block_on(cl_lan::client::pair(self.addr(), code, &device, label))
            .expect("pair");
        assert!(clicker.join().expect("clicker"), "Allow was clicked");
        assert_eq!(fp, lan.fingerprint());
        WorkstationSettings {
            host: self.addr(),
            host_fingerprint: fp.hex(),
        }
    }

    /// Stops everything (proxy first, then the engine and the database).
    fn stop(&mut self) {
        self.stop_proxy();
        if let Some(sup) = self.sup.take() {
            // The proxy's pump lets go of its reference within a beat.
            let mut sup = sup;
            for _ in 0..100 {
                match Arc::try_unwrap(sup) {
                    Ok(s) => {
                        s.stop();
                        return;
                    }
                    Err(back) => {
                        sup = back;
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
            panic!("the host supervisor is still shared");
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop_proxy();
        drop(self.sup.take());
    }
}

/// One workstation: its link, a cl-api client over it, and a runtime.
struct Desk {
    link: Arc<LanWorkstation>,
    client: Client,
    rt: tokio::runtime::Runtime,
}

impl Desk {
    fn start(settings: &WorkstationSettings, root: &Path, store: &MemorySecrets) -> Desk {
        let link = Arc::new(LanWorkstation::start(settings, root, store).expect("workstation"));
        let client = Client::new(link.endpoint_source(), ClientConfig::default()).expect("client");
        Desk {
            link,
            client,
            rt: rt(),
        }
    }

    fn connected(&self) -> bool {
        matches!(self.link.state(), LinkState::Connected { .. })
    }

    fn generation(&self) -> Option<u64> {
        match self.link.state() {
            LinkState::Connected { generation, .. } => Some(generation),
            _ => None,
        }
    }

    fn wait_connected(&self, what: &str) {
        wait_until(what, Duration::from_mins(3), || self.connected());
    }

    fn run<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        self.rt.block_on(f)
    }

    /// Signs in with the host's owner and opens the shared company.
    fn sign_in(&self, password: &Secret) -> Session {
        self.run(async {
            self.client.desktop_info().await.expect("desktop info");
            let s = match self.client.login(OWNER, password).await {
                Ok(LoginOutcome::Session(s)) => s,
                other => panic!("login through the host: {other:?}"),
            };
            let co = s
                .companies()
                .await
                .expect("the host's company list")
                .into_iter()
                .find(|c| c.tenant_name == BOOKS)
                .expect("the host's company is listed");
            s.open_company(co.tenant_id).await.expect("open company");
            s
        })
    }
}

fn account(d: &Desk, s: &Session, t: AccountType, prefix: &str) -> AccountId {
    d.run(s.accounts().create(
        &AccountFields {
            name: unique(prefix),
            account_number: None,
            account_type: t,
            detail_type: None,
            description: None,
            parent_id: None,
        },
        key(),
    ))
    .expect("account")
    .record
    .id
}

fn line(account: AccountId, side: Side) -> JeLine {
    JeLine {
        account,
        side,
        description: None,
        tag: None,
        contact: None,
    }
}

fn je2(
    on: CalendarDate,
    memo: &str,
    debit: AccountId,
    credit: AccountId,
    amount: &str,
) -> NewJournalEntry {
    NewJournalEntry::new(
        on,
        Some(memo.to_string()),
        Basis::Both,
        vec![
            line(debit, Side::Debit(m(amount))),
            line(credit, Side::Credit(m(amount))),
        ],
    )
    .expect("balanced")
}

/// The entry's amount: the sum of its debit lines (exact).
fn debits(t: &cl_api::Transaction) -> Money {
    t.lines
        .iter()
        .try_fold(Money::ZERO, |sum, l| sum.checked_add(l.debit))
        .expect("sum")
}

fn on() -> CalendarDate {
    CalendarDate::from_ymd(2026, 8, 14).expect("date")
}

fn is_stale(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        }
    )
}

/// Boots a host, or says why it can't (missing engine or `PostgreSQL`).
fn host_or_skip(scratch: &Path) -> Option<Host> {
    let Some(exe) = engine_exe() else {
        skip("the engine executable is missing (run `cargo xtask engine`)");
        return None;
    };
    Host::boot(&scratch.join("host"), exe)
        .map_err(|e| {
            skip(&format!(
                "the host could not start ({e}); run `cargo xtask pg-fetch`"
            ))
        })
        .ok()
}

#[test]
fn office_network_live() {
    let scratch = tempfile::tempdir().expect("scratch");
    let Some(mut host) = host_or_skip(scratch.path()) else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        live_scenario(&mut host, scratch.path());
    }));
    host.stop();
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

fn live_scenario(host: &mut Host, scratch: &Path) {
    // --- Two workstations: sign-in, list and open through the host -------
    let (a_root, b_root) = (scratch.join("ws-a"), scratch.join("ws-b"));
    let (a_store, b_store) = (MemorySecrets::new(), MemorySecrets::new());
    let a_cfg = host.pair(&a_root, &a_store, "Front desk");
    let b_cfg = host.pair(&b_root, &b_store, "Back office");
    let a = Desk::start(&a_cfg, &a_root, &a_store);
    let b = Desk::start(&b_cfg, &b_root, &b_store);
    a.wait_connected("workstation A");
    b.wait_connected("workstation B");
    let sa = a.sign_in(&host.password);
    let sb = b.sign_in(&host.password);

    // --- Host-only work is refused from a workstation (T1, T12) ----------
    let refused = a.run(sa.put_session_secrets(&cl_api::SessionSecrets {
        anthropic_key: Some(Secret::new("sk-test-never-sent-anywhere")),
        ..cl_api::SessionSecrets::default()
    }));
    assert!(
        matches!(refused, Err(ApiError::Lan(LanRefusal::HostOnly))),
        "session secrets from a workstation: {refused:?}"
    );

    // --- Two workstations, one journal entry: no lost update (AC1) -------
    let cash = account(&a, &sa, AccountType::Asset, "Cash");
    let supplies = account(&a, &sa, AccountType::Expense, "Supplies");
    let memo = unique("shared entry");
    let made = a
        .run(sa.transactions().create_journal_entry(
            &je2(on(), &memo, supplies, cash, "10.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("entry");
    let id = made.record.id;
    let mine = a.run(sa.transactions().get(id)).expect("A reads");
    let theirs = b.run(sb.transactions().get(id)).expect("B reads");
    // A changes the memo and saves first.
    let a_memo = format!("{memo} (A)");
    a.run(sa.transactions().update_journal_entry(
        id,
        &mine.version,
        &je2(on(), &a_memo, supplies, cash, "10.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("A saves first");
    // B changes the amount on its old read: the conflict, nothing saved.
    let e = b
        .run(sb.transactions().update_journal_entry(
            id,
            &theirs.version,
            &je2(on(), &memo, supplies, cash, "25.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("B's save on the old read is stale");
    assert!(is_stale(&e), "{e:?}");
    let ApiError::Conflict {
        current_version: Some(RecordVersion::Stamp(now)),
        ..
    } = &e
    else {
        panic!("the conflict carries the current stamp: {e:?}");
    };
    // B reads theirs and keeps its own change on top (Keep mine).
    let fresh = b.run(sb.transactions().get(id)).expect("B reads again");
    assert_eq!(fresh.version.stamp(), Some(*now));
    assert_eq!(fresh.record.memo.as_deref(), Some(a_memo.as_str()));
    b.run(sb.transactions().update_journal_entry(
        id,
        &fresh.version,
        &je2(on(), &a_memo, supplies, cash, "25.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("B saves on the fresh read");
    let last = a
        .run(sa.transactions().get(id))
        .expect("A reads the result");
    assert_eq!(
        last.record.memo.as_deref(),
        Some(a_memo.as_str()),
        "A's memo kept"
    );
    assert_eq!(debits(&last.record), m("25.00"), "B's amount kept");

    // --- Engine restart: reconnects, data intact (AC2) -------------------
    let before = a.generation().expect("generation");
    let pid = host.sup().status().engine_pid.expect("engine pid");
    kill_pid(pid);
    assert!(
        host.sup().wait_for(Duration::from_mins(3), |s| {
            s.generation > before && s.state == HostState::Running
        }),
        "the host's engine came back"
    );
    wait_until(
        "workstation A on the new engine",
        Duration::from_mins(2),
        || a.generation().is_some_and(|g| g > before),
    );
    let after = a
        .run(sa.transactions().get(id))
        .expect("read after an engine restart");
    assert_eq!(debits(&after.record), m("25.00"), "data intact");

    // --- Proxy restart: down, then back on the same port -----------------
    host.stop_proxy();
    wait_until("the link to notice", Duration::from_secs(30), || {
        !a.connected()
    });
    let not_sent = a.run(sa.transactions().get(id));
    assert!(not_sent.is_err(), "no host, no answer");
    host.start_proxy();
    a.wait_connected("workstation A after the proxy restart");
    let after = a
        .run(sa.transactions().get(id))
        .expect("read after a proxy restart");
    assert_eq!(debits(&after.record), m("25.00"));

    // --- Host restart: sign in again, data intact ------------------------
    host.stop();
    wait_until("the link to notice", Duration::from_secs(30), || {
        !b.connected()
    });
    host.start_supervisor();
    host.sup()
        .wait_ready(Duration::from_mins(3))
        .expect("host ready again");
    host.start_proxy();
    b.wait_connected("workstation B after the host restart");
    let sb2 = b.sign_in(&host.password);
    let after = b
        .run(sb2.transactions().get(id))
        .expect("read after a host restart");
    assert_eq!(debits(&after.record), m("25.00"), "data intact");
    assert_eq!(after.record.memo.as_deref(), Some(a_memo.as_str()));

    // --- A pin mismatch is refused ---------------------------------------
    let other = Identity::generate().expect("other host");
    let wrong = WorkstationSettings {
        host: host.addr(),
        host_fingerprint: other.fingerprint().hex(),
    };
    let c_root = scratch.join("ws-c");
    let c_store = MemorySecrets::new();
    let c = Desk::start(&wrong, &c_root, &c_store);
    std::thread::sleep(Duration::from_secs(5));
    assert!(
        !c.connected(),
        "a wrong pin never connects: {:?}",
        c.link.state()
    );
    let refused = c.run(c.client.desktop_info());
    assert!(refused.is_err(), "nothing reaches the host: {refused:?}");
    // An unpaired device with the right pin reaches hello only.
    let unpaired = WorkstationSettings {
        host: host.addr(),
        host_fingerprint: host.lan().fingerprint().hex(),
    };
    let d_root = scratch.join("ws-d");
    let d_store = MemorySecrets::new();
    let d = Desk::start(&unpaired, &d_root, &d_store);
    wait_until("the unpaired verdict", Duration::from_secs(30), || {
        matches!(d.link.state(), LinkState::Unpaired)
    });

    // --- A public interface is refused -----------------------------------
    for ip in ["8.8.8.8", "0.0.0.0", "100.64.0.1"] {
        assert!(
            host.try_proxy(ip).is_err(),
            "a listener on {ip} must be refused"
        );
    }

    // --- The request-smuggling shapes, against the live proxy ------------
    smuggling(host, &b_root, &b_store);

    // Every workstation still works afterwards.
    b.wait_connected("workstation B after the refusals");
    let ok = b
        .run(sb2.transactions().get(id))
        .expect("read after the refusals");
    assert_eq!(debits(&ok.record), m("25.00"));
    drop((a, b, c, d));
}

/// T2/T3: each shape over a paired TLS connection is refused and never
/// reaches the engine; a token smuggled on a second request on the same
/// connection is never parsed.
fn smuggling(host: &Host, ws_root: &Path, store: &MemorySecrets) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (device, _) =
        Identity::load_or_create(store, &lan_dir(ws_root), Role::Device).expect("device");
    let (cfg, _) = cl_lan::tls::client_config(
        &device,
        cl_lan::tls::HostPin::Full(host.lan().fingerprint()),
    )
    .expect("tls config");
    let rt = rt();
    let addr = host.addr();
    let send = |bytes: Vec<u8>| -> String {
        rt.block_on(async {
            let tcp = tokio::net::TcpStream::connect(addr).await.expect("tcp");
            let mut tls = tokio_rustls::TlsConnector::from(Arc::clone(&cfg))
                .connect(cl_lan::tls::server_name(), tcp)
                .await
                .expect("tls");
            tls.write_all(&bytes).await.expect("write");
            let mut out = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(10), tls.read_to_end(&mut out)).await;
            String::from_utf8_lossy(&out).into_owned()
        })
    };
    let status = |resp: &str| -> u16 {
        resp.split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    // A plain paired request works (the baseline).
    let ok = send(b"GET /api/v1/desktop/info HTTP/1.1\r\nHost: clientledger\r\n\r\n".to_vec());
    assert_eq!(status(&ok), 200, "{ok}");
    let shapes: [(&str, &[u8]); 7] = [
        (
            "Content-Length with Transfer-Encoding",
            b"POST /api/v1/auth/login HTTP/1.1\r\nHost: x\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
        ),
        (
            "duplicate Content-Length",
            b"POST /api/v1/auth/login HTTP/1.1\r\nHost: x\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n{}",
        ),
        (
            "Upgrade",
            b"GET /api/v1/desktop/info HTTP/1.1\r\nHost: x\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
        ),
        (
            "absolute-form target",
            b"GET http://127.0.0.1/api/v1/desktop/info HTTP/1.1\r\nHost: x\r\n\r\n",
        ),
        ("CONNECT", b"CONNECT 127.0.0.1:5432 HTTP/1.1\r\nHost: x\r\n\r\n"),
        (
            "Origin (a browser page)",
            b"GET /api/v1/desktop/info HTTP/1.1\r\nHost: x\r\nOrigin: http://evil.example\r\n\r\n",
        ),
        (
            "HTTP/2 prior knowledge",
            b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
        ),
    ];
    for (what, bytes) in shapes {
        let resp = send(bytes.to_vec());
        let code = status(&resp);
        assert!(
            code == 0 || (400..500).contains(&code),
            "{what} must be refused, got {code}: {resp}"
        );
    }
    // `Connection: X-CL-Device` (hop-by-hop stripping) cannot strip the
    // proxy's own device header: the request still runs as this device.
    let resp = send(
        b"GET /api/v1/desktop/info HTTP/1.1\r\nHost: x\r\nConnection: X-CL-Device\r\nX-CL-Device: 00\r\n\r\n"
            .to_vec(),
    );
    assert!(matches!(status(&resp), 200 | 400), "{resp}");
    // A spoofed host token on a second request on the same connection is
    // never parsed (one request per connection), and a spoofed token on
    // the first is replaced by the workstation token: host-only routes stay
    // refused.
    let host_token = host
        .sup()
        .endpoint()
        .map(|e| e.token.expose().to_string())
        .expect("host token");
    let two = format!(
        "GET /api/v1/desktop/info HTTP/1.1\r\nHost: x\r\n\r\nPUT /api/v1/desktop/session-secrets HTTP/1.1\r\nHost: x\r\nX-CL-Token: {host_token}\r\nContent-Length: 2\r\n\r\n{{}}"
    );
    let resp = send(two.into_bytes());
    assert_eq!(
        resp.matches("HTTP/1.1 ").count(),
        1,
        "only the first request is answered: {resp}"
    );
    let spoof = format!(
        "PUT /api/v1/desktop/session-secrets HTTP/1.1\r\nHost: x\r\nX-CL-Token: {host_token}\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}"
    );
    let resp = send(spoof.into_bytes());
    assert_eq!(status(&resp), 403, "the spoofed token is stripped: {resp}");
    assert!(resp.contains("HOST_ONLY"), "{resp}");
}

// ---------------------------------------------------------------- soak

/// What one soak client saw.
#[derive(Debug, Default)]
struct Tally {
    saves: u64,
    reads: u64,
    /// Stale answers on shared records, each resolved by a re-read and a
    /// re-save (explained conflicts).
    conflicts: u64,
    /// A shared counter update that went missing (must stay 0).
    lost: u64,
    /// 5xx answers or other failures (must stay 0).
    failures: Vec<String>,
}

/// AC3: five workstations save for `CL_LAN_SOAK_SECS` seconds through the
/// live proxy. 10% of saves hit shared records whose memo carries a counter
/// each client bumps; a lost update shows up as a counter that went back.
/// End checks: no failure, no unexplained conflict, every shared counter
/// equals the bumps made, and the books balance.
#[test]
fn office_network_soak() {
    let Some(secs) = std::env::var("CL_LAN_SOAK_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    else {
        eprintln!("SKIPPED: set CL_LAN_SOAK_SECS (cargo xtask lan-soak) to run the soak");
        return;
    };
    let scratch = tempfile::tempdir().expect("scratch");
    let Some(mut host) = host_or_skip(scratch.path()) else {
        return;
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        soak(&host, scratch.path(), Duration::from_secs(secs));
    }));
    host.stop();
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}

const CLIENTS: usize = 5;
const SHARED: usize = 3;

fn counter_memo(base: &str, n: u64) -> String {
    format!("{base} #{n}")
}

fn counter_of(memo: Option<&str>) -> u64 {
    memo.and_then(|m| m.rsplit_once(" #"))
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

fn soak(host: &Host, scratch: &Path, run_for: Duration) {
    let started = Instant::now();
    let mut desks = Vec::new();
    for i in 0..CLIENTS {
        let root = scratch.join(format!("soak-{i}"));
        let store = MemorySecrets::new();
        let cfg = host.pair(&root, &store, &format!("Desk {i}"));
        let desk = Desk::start(&cfg, &root, &store);
        desk.wait_connected("a soak workstation");
        desks.push(desk);
    }
    let first = desks[0].sign_in(&host.password);
    let cash = account(&desks[0], &first, AccountType::Asset, "Soak cash");
    let supplies = account(&desks[0], &first, AccountType::Expense, "Soak supplies");
    let shared: Vec<_> = (0..SHARED)
        .map(|i| {
            desks[0]
                .run(first.transactions().create_journal_entry(
                    &je2(
                        on(),
                        &counter_memo(&format!("shared {i}"), 0),
                        supplies,
                        cash,
                        "1.00",
                    ),
                    &PostingOptions::default(),
                    key(),
                ))
                .expect("shared entry")
                .record
                .id
        })
        .collect();
    let bumps: Vec<AtomicU64> = (0..SHARED).map(|_| AtomicU64::new(0)).collect();
    let deadline = Instant::now() + run_for;
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let handles: Vec<_> = desks
            .iter()
            .enumerate()
            .map(|(i, desk)| {
                let (shared, bumps, password) = (&shared, &bumps, &host.password);
                scope.spawn(move || {
                    soak_client(i, desk, password, cash, supplies, shared, bumps, deadline)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("soak client"))
            .collect()
    });
    let saves: u64 = tallies.iter().map(|t| t.saves).sum();
    let reads: u64 = tallies.iter().map(|t| t.reads).sum();
    let conflicts: u64 = tallies.iter().map(|t| t.conflicts).sum();
    let lost: u64 = tallies.iter().map(|t| t.lost).sum();
    let failures: Vec<&String> = tallies.iter().flat_map(|t| &t.failures).collect();
    eprintln!(
        "soak: {CLIENTS} clients, {:?}: {saves} saves, {reads} reads, {conflicts} resolved conflicts, {lost} lost, {} failures",
        started.elapsed(),
        failures.len()
    );
    assert!(failures.is_empty(), "failures: {failures:?}");
    assert_eq!(lost, 0, "no lost update");
    assert!(saves > 0, "the soak saved something");
    // Each shared counter equals the bumps that succeeded.
    for (i, id) in shared.iter().enumerate() {
        let t = desks[0]
            .run(first.transactions().get(*id))
            .expect("shared entry");
        assert_eq!(
            counter_of(t.record.memo.as_deref()),
            bumps[i].load(Ordering::SeqCst),
            "shared entry {i}: every bump kept"
        );
    }
    // The books balance: cash and supplies move by the same amounts.
    let cash_now = desks[0]
        .run(first.accounts().get(cash))
        .expect("cash")
        .record
        .balance
        .unwrap_or_default();
    let supplies_now = desks[0]
        .run(first.accounts().get(supplies))
        .expect("supplies")
        .record
        .balance
        .unwrap_or_default();
    assert_eq!(
        cash_now.abs(),
        supplies_now.abs(),
        "every entry posted both sides"
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "one client's whole loop; the scope owns the shared state"
)]
fn soak_client(
    i: usize,
    desk: &Desk,
    password: &Secret,
    cash: AccountId,
    supplies: AccountId,
    shared: &[cl_core::ids::TransactionId],
    bumps: &[AtomicU64],
    deadline: Instant,
) -> Tally {
    let mut t = Tally::default();
    let s = desk.sign_in(password);
    let mut n: u64 = 0;
    while Instant::now() < deadline {
        n += 1;
        // Amounts from integer minor units: 1.00 to 9.99.
        let cents = 100 + (n * 37 + i as u64 * 11) % 900;
        let amount = Money::from_cents(i64::try_from(cents).unwrap_or(100))
            .expect("cents")
            .to_wire();
        if n % 10 == 0 {
            // 10%: bump a shared record's counter (read, then save on it).
            let k = (n as usize + i) % shared.len();
            loop {
                let got = match desk.run(s.transactions().get(shared[k])) {
                    Ok(g) => g,
                    Err(e) => {
                        t.failures.push(format!("read shared: {e}"));
                        break;
                    }
                };
                t.reads += 1;
                let now = counter_of(got.record.memo.as_deref());
                let memo = counter_memo(&format!("shared {k}"), now + 1);
                match desk.run(s.transactions().update_journal_entry(
                    shared[k],
                    &got.version,
                    &je2(on(), &memo, supplies, cash, "1.00"),
                    &PostingOptions::default(),
                    key(),
                )) {
                    Ok(saved) => {
                        t.saves += 1;
                        let prev = bumps[k].fetch_add(1, Ordering::SeqCst);
                        if counter_of(saved.record.memo.as_deref()) != now + 1 || prev + 1 < now + 1
                        {
                            t.lost += 1;
                        }
                        break;
                    }
                    Err(e) if is_stale(&e) => t.conflicts += 1,
                    Err(e) => {
                        t.failures.push(format!("shared save: {e}"));
                        break;
                    }
                }
            }
        } else {
            match desk.run(s.transactions().create_journal_entry(
                &je2(on(), &unique(&format!("soak {i}")), supplies, cash, &amount),
                &PostingOptions::default(),
                key(),
            )) {
                Ok(_) => t.saves += 1,
                Err(e) => t.failures.push(format!("entry: {e}")),
            }
            // The UI's read pattern after a change: the entry list.
            if n % 3 == 0 {
                match desk.run(s.ledger_version()) {
                    Ok(_) => t.reads += 1,
                    Err(e) => t.failures.push(format!("ledger version: {e}")),
                }
            }
        }
    }
    t
}
