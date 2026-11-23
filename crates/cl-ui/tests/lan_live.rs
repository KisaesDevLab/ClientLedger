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
//!   request-smuggling shapes refused by the live proxy; a restore on the
//!   host under maintenance (refused from a workstation).
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
    clippy::many_single_char_names,
    clippy::similar_names,
    reason = "each test is one linear scenario over one booted host; the soak's client loop keeps its short names (i, n, s, t, k)"
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
    stores: Option<HostConfig>,
}

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
            stores: None,
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
        let stores = self.stores.get_or_insert_with(|| {
            cl_host::in_memory_stores(HostConfig::new(paths.clone(), self.exe.clone()))
        });
        cfg.pg_password_store.clone_from(&stores.pg_password_store);
        cfg.secrets_store.clone_from(&stores.secrets_store);
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

    /// A runtime for the host's own sessions.
    #[allow(
        clippy::unused_self,
        reason = "read as the host's runtime at each call"
    )]
    fn host_rt(&self) -> &'static tokio::runtime::Runtime {
        static RT: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(rt);
        &RT
    }

    /// The owner signed in on the host itself (the launch-token client
    /// over the supervisor's endpoint, as the host window is), the shared
    /// company open. Drop it before [`Host::stop`].
    fn sign_in_here(&self) -> Session {
        let src = cl_ui::engine_link::VerifiedEndpoint::new(
            Arc::clone(self.sup()) as Arc<dyn SnapshotSource>
        );
        let client = Client::new(Arc::new(src), ClientConfig::default()).expect("host client");
        self.host_rt().block_on(async {
            client
                .desktop_info()
                .await
                .expect("desktop info on the host");
            let s = match client.login(OWNER, &self.password).await {
                Ok(LoginOutcome::Session(s)) => s,
                other => panic!("login on the host: {other:?}"),
            };
            let co = s
                .companies()
                .await
                .expect("companies")
                .into_iter()
                .find(|c| c.tenant_name == BOOKS)
                .expect("the company");
            s.open_company(co.tenant_id).await.expect("open company");
            s
        })
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

/// 409 `RETRY`: a deadlock rolled the save back (U-146); nothing saved.
fn is_retry(e: &ApiError) -> bool {
    e.to_string().contains("RETRY")
}

/// Replays a keyed save after a deadlock refusal (at most 3 times) or a
/// 429 (the per-device limit, 1200 a minute: waits as told, at most 20
/// times). Neither counts as a failure or a lost update.
fn keyed<T>(
    t: &mut Tally,
    mut save: impl FnMut(IdempotencyKey) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let k = key();
    let (mut tries, mut limited) = (0, 0);
    loop {
        match save(k.clone()) {
            Err(e) if is_retry(&e) && tries < 3 => {
                tries += 1;
                t.deadlock_replays += 1;
                std::thread::sleep(Duration::from_millis(200 * tries));
            }
            Err(ApiError::RateLimited { retry_after }) if limited < 20 => {
                limited += 1;
                t.rate_limited += 1;
                std::thread::sleep(retry_after.unwrap_or(Duration::from_secs(1)));
            }
            other => return other,
        }
    }
}

/// A read, backing off on 429 like [`keyed`].
fn read<T>(t: &mut Tally, mut get: impl FnMut() -> Result<T, ApiError>) -> Result<T, ApiError> {
    for _ in 0..20 {
        match get() {
            Err(ApiError::RateLimited { retry_after }) => {
                t.rate_limited += 1;
                std::thread::sleep(retry_after.unwrap_or(Duration::from_secs(1)));
            }
            other => return other,
        }
    }
    get()
}

/// Each soak client's pace: about 6 requests a second, well under the
/// per-device 1200 a minute.
const PACE: Duration = Duration::from_millis(150);

fn is_stale(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        }
    )
}

/// In LAN mode a deactivation, a void and a delete go through on the stamp
/// they were read at (no 428 `STALE_REQUIRED`; the `DELETE` stamp rides in
/// the body, review S3), and on a stamp another workstation has moved since
/// they are refused with 409 `STALE` and change nothing.
fn stamped_removals(
    a: &Desk,
    sa: &Session,
    b: &Desk,
    sb: &Session,
    debit: AccountId,
    credit: AccountId,
) {
    // An account: B deactivates it on its read; A's older read is stale.
    let acct = account(a, sa, AccountType::Expense, "Retired");
    let a_read = a.run(sa.accounts().get(acct)).expect("A reads the account");
    let b_read = b.run(sb.accounts().get(acct)).expect("B reads the account");
    let off = b
        .run(sb.accounts().deactivate(acct, &b_read.version, key()))
        .expect("B deactivates on its stamp (no 428)");
    assert!(!off.record.is_active);
    let e = a
        .run(sa.accounts().deactivate(acct, &a_read.version, key()))
        .expect_err("A's deactivation on the old stamp");
    assert!(is_stale(&e), "409 STALE: {e:?}");

    // A journal entry: B edits it, so A's void on its older read is
    // stale; on a fresh read the void goes through.
    let memo = unique("void me");
    let made = a
        .run(sa.transactions().create_journal_entry(
            &je2(on(), &memo, debit, credit, "7.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("entry");
    let txn = made.record.id;
    let a_old = a.run(sa.transactions().get(txn)).expect("A reads");
    let b_read = b.run(sb.transactions().get(txn)).expect("B reads");
    b.run(sb.transactions().update_journal_entry(
        txn,
        &b_read.version,
        &je2(on(), &format!("{memo} (B)"), debit, credit, "7.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("B edits");
    let e = a
        .run(sa.transactions().void(
            txn,
            a_old.version.stamp(),
            "wrong entry",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("A's void on the old stamp");
    assert!(is_stale(&e), "409 STALE: {e:?}");
    let a_new = a.run(sa.transactions().get(txn)).expect("A reads again");
    assert_ne!(a_new.record.status, cl_api::TxnStatus::Void, "not voided");
    let voided = a
        .run(sa.transactions().void(
            txn,
            a_new.version.stamp(),
            "wrong entry",
            &PostingOptions::default(),
            key(),
        ))
        .expect("A voids on its fresh stamp (no 428)");
    assert_eq!(voided.record.status, cl_api::TxnStatus::Void);

    // A delete (`DELETE /transactions/{id}` reads its stamp from the body).
    let made = a
        .run(sa.transactions().create_journal_entry(
            &je2(on(), &unique("delete me"), debit, credit, "3.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect("entry");
    let gone = made.record.id;
    let a_old = a.run(sa.transactions().get(gone)).expect("A reads");
    let b_read = b.run(sb.transactions().get(gone)).expect("B reads");
    b.run(sb.transactions().update_journal_entry(
        gone,
        &b_read.version,
        &je2(on(), &unique("delete me (B)"), debit, credit, "3.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("B edits");
    let opts = cl_api::DeleteOptions::default();
    let e = a
        .run(
            sa.transactions()
                .delete(gone, a_old.version.stamp(), &opts, key()),
        )
        .expect_err("A's delete on the old stamp");
    assert!(is_stale(&e), "409 STALE: {e:?}");
    let a_new = a.run(sa.transactions().get(gone)).expect("A reads again");
    let deleted = a
        .run(
            sa.transactions()
                .delete(gone, a_new.version.stamp(), &opts, key()),
        )
        .expect("A deletes on its fresh stamp (no 428)");
    assert_eq!(deleted.record.status, cl_api::TxnStatus::Deleted);
}

/// Boots a host, or says why it can't (missing engine or `PostgreSQL`).
/// Review S7: Remove in the Office Network window ends the computer's
/// engine sign-ins (through the window's own event path), so its refresh
/// token stops working at once.
fn removal_ends_sign_ins(host: &Host, hs: &Session, scratch: &Path) {
    let root = scratch.join("ws-removed");
    let store = MemorySecrets::new();
    let cfg = host.pair(&root, &store, "Old laptop");
    let (device, _) =
        Identity::load_or_create(&store, &lan_dir(&root), Role::Device).expect("device");
    let fp = device.fingerprint();
    let e = Desk::start(&cfg, &root, &store);
    e.wait_connected("the computer to remove");
    let se = e.sign_in(&host.password);
    let listed = |hs: &Session| {
        host.host_rt()
            .block_on(hs.lan_sessions())
            .expect("sessions")
            .into_iter()
            .filter(|r| r.device_id.as_deref() == Some(fp.hex().as_str()))
            .count()
    };
    assert!(listed(hs) > 0, "its sign-in is listed");
    // What Remove does: the registry, then the window's events.
    assert!(host.lan().registry().remove(&fp).expect("removed"));
    host.host_rt()
        .block_on(cl_ui::lan_link::record_events(
            hs.client().clone(),
            Some(hs.clone()),
            vec![(
                cl_api::LanAuditEvent::LanDeviceRemoved,
                Some(fp.hex()),
                Some("Old laptop".into()),
            )],
        ))
        .expect("its sign-ins ended");
    assert_eq!(listed(hs), 0, "no sign-in from the removed computer");
    let gone = e.run(se.ledger_version());
    assert!(gone.is_err(), "the removed computer gets nothing: {gone:?}");
    drop(e);
}

/// PR-7 note: a restore on a LAN host, as the host window runs it (the
/// launch-token client, so the engine's host class; under maintenance, as
/// `screens/company/open.rs` does). Workstations can't restore
/// (`tenant-archives/import` is host-only), are signed out by the hold,
/// and afterwards sign in again to the restored company, whose books equal
/// the backed-up ones.
fn host_restore(host: &Host, b: &Desk, scratch: &Path) {
    use cl_settings::secrets::BACKUP_PASSPHRASE;
    use cl_settings::{BackupKind, RecentStore, SecretStore as _};
    let hs = host.sign_in_here();
    let rt = host.host_rt();
    let named = |s: &Session, rt: &tokio::runtime::Runtime| {
        rt.block_on(s.companies())
            .expect("companies")
            .into_iter()
            .find(|c| c.tenant_name == BOOKS)
            .map(|c| c.tenant_id)
            .expect("the books are listed")
    };
    let tenant = named(&hs, rt);
    // The host app's own company list and keychain (the company was made
    // by first-run setup, so the list learns it here as Open would).
    let store = RecentStore::in_dir(&scratch.join("host-settings"));
    store
        .update(|l| {
            l.upsert(tenant, BOOKS);
        })
        .expect("company list");
    let secrets = MemorySecrets::new();
    secrets
        .set(
            BACKUP_PASSPHRASE,
            &Secret::new("lan-live-backup-passphrase-synthetic"),
        )
        .expect("keychain");
    let before = rt
        .block_on(hs.tenants().verification_totals(tenant, on(), false))
        .expect("totals before");
    let made = rt
        .block_on(cl_company::backup(
            &hs,
            &store,
            &secrets,
            &cl_company::BackupRequest {
                tenant_id: tenant,
                kind: BackupKind::Backup,
                dest_dir: scratch.join("host-backups"),
                apply_retention: false,
                protected: None,
            },
            &cl_company::backup::silent(),
        ))
        .expect("a backup on the LAN host");

    // A workstation can't restore: host-only, refused before the engine.
    let sb = b.sign_in(&host.password);
    let refused = b.run(sb.tenants().import_archive(&cl_api::ImportRequest {
        path: made.record.path.clone(),
        passphrase: Secret::new("lan-live-backup-passphrase-synthetic"),
        scratch: false,
        reset_document_drafts: false,
    }));
    assert!(
        matches!(refused, Err(ApiError::Lan(LanRefusal::HostOnly))),
        "a restore from a workstation: {refused:?}"
    );

    // The host restores its own file (a path import, host class only).
    rt.block_on(cl_company::close_company(&hs)).expect("close");
    let restored = rt
        .block_on(cl_ui::lan_status::with_maintenance(
            &hs,
            true,
            cl_company::restore(
                &hs,
                &store,
                &secrets,
                &cl_company::RestoreRequest {
                    path: made.record.path.clone(),
                    passphrase: None,
                },
                &cl_company::backup::silent(),
                &cl_company::Statistics::none(),
            ),
        ))
        .expect("a restore on the LAN host");
    assert_ne!(restored.new_tenant, tenant, "a restore makes a new company");
    let held = rt.block_on(hs.maintenance()).expect("maintenance state");
    assert!(!held.active, "the hold ended with the restore: {held:?}");
    // The hold signed the workstation out.
    let gone = b.run(sb.ledger_version());
    assert!(gone.is_err(), "the workstation was signed out: {gone:?}");

    // The restored company carries the books' name and the same books.
    let after = rt
        .block_on(
            hs.tenants()
                .verification_totals(restored.new_tenant, on(), false),
        )
        .expect("totals after");
    // Money and counts, not `books_differ` (which also wants the same
    // company ids; a restore makes new ones).
    let books = |v: &cl_api::VerificationTotals| {
        (
            v.total_assets,
            v.total_liabilities,
            v.total_equity,
            v.accounts_receivable,
            v.accounts_payable,
            v.trial_balance.clone(),
            v.counts.accounts,
            v.counts.transactions,
            v.counts.posted_transactions,
        )
    };
    assert!(before.counts.transactions > 0, "the books have entries");
    assert_eq!(
        books(&before),
        books(&after),
        "the restored books equal the backed-up ones"
    );
    assert_eq!(
        named(&hs, rt),
        restored.new_tenant,
        "the restored company carries the books' name"
    );
    b.wait_connected("workstation B after the restore");
    let sb = b.sign_in(&host.password);
    b.run(sb.ledger_version())
        .expect("the workstation reads the restored books");
    drop(hs);
}

fn host_or_skip(scratch: &Path) -> Option<Host> {
    let Some(exe) = engine_exe() else {
        skip("the engine executable is missing (run `cargo xtask engine`)");
        return None;
    };
    Host::boot(&scratch.join("host"), exe)
        .map_err(|e| {
            skip(&format!(
                "the host could not start ({e}); run `cargo xtask pg-fetch`"
            ));
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

#[allow(
    clippy::many_single_char_names,
    reason = "two workstations a and b and their sessions, side by side"
)]
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

    // --- 428 STALE_REQUIRED (U-292): a save without its stamp is refused
    // before any handler runs (a client bug; nothing changes) -------------
    let read = a.run(sa.transactions().get(id)).expect("read");
    let RecordVersion::Stamped { fingerprint, .. } = read.version else {
        panic!("a LAN read carries the stamp: {:?}", read.version);
    };
    let e = a
        .run(sa.transactions().update_journal_entry(
            id,
            &RecordVersion::Fingerprint(fingerprint),
            &je2(on(), &a_memo, supplies, cash, "99.00"),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("a save without its stamp");
    assert!(
        matches!(e, ApiError::Lan(LanRefusal::StaleRequired)),
        "428 STALE_REQUIRED: {e:?}"
    );
    let same = a.run(sa.transactions().get(id)).expect("read");
    assert_eq!(debits(&same.record), m("25.00"), "nothing changed");

    // --- Deletes, deactivations and voids carry their stamps (U-292) ------
    stamped_removals(&a, &sa, &b, &sb, supplies, cash);

    // --- Engine restart: reconnects, data intact (AC2) -------------------
    // What a workstation sees is `epoch << 32 | generation` (review S2);
    // the supervisor's own generation is the raw count.
    let before = a.generation().expect("generation");
    let raw_before = host.sup().status().generation;
    let pid = host.sup().status().engine_pid.expect("engine pid");
    kill_pid(pid);
    assert!(
        host.sup().wait_for(Duration::from_mins(3), |s| {
            s.generation > raw_before && s.state == HostState::Running
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
    // QA-6: a save asked for while the host is down is refused at once and
    // never held: after the host is back nothing arrives until the owner
    // saves again, with the same key.
    let held_memo = unique("held-save");
    let held_key = key();
    let started = std::time::Instant::now();
    let e = a
        .run(sa.transactions().update_journal_entry(
            id,
            &same.version,
            &je2(on(), &held_memo, supplies, cash, "25.00"),
            &PostingOptions::default(),
            held_key.clone(),
        ))
        .expect_err("no host: not sent");
    // Not sent: refused by the link, or by a read that has to go first
    // (sign-in refresh) finding the engine away. Either way the call is
    // over, so nothing of it can go out later.
    assert!(
        matches!(
            e,
            ApiError::Lan(LanRefusal::NotSent) | ApiError::EngineUnavailable { permanent: false }
        ),
        "refused as not sent: {e:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "refused at once, not held: {:?}",
        started.elapsed()
    );
    host.start_proxy();
    a.wait_connected("workstation A after the proxy restart");
    std::thread::sleep(Duration::from_secs(5));
    let after = a
        .run(sa.transactions().get(id))
        .expect("read after a proxy restart");
    assert_eq!(debits(&after.record), m("25.00"));
    assert_ne!(
        after.record.memo.as_deref(),
        Some(held_memo.as_str()),
        "nothing was sent on its own"
    );
    // The owner saves again: the same key goes, once.
    a.run(sa.transactions().update_journal_entry(
        id,
        &after.version,
        &je2(on(), &held_memo, supplies, cash, "25.00"),
        &PostingOptions::default(),
        held_key,
    ))
    .expect("saved when the owner saves again");
    let after = a.run(sa.transactions().get(id)).expect("read");
    assert_eq!(after.record.memo.as_deref(), Some(held_memo.as_str()));
    assert_eq!(debits(&after.record), m("25.00"));

    // --- PR-8 B1: a lost reply on a new entry, the memo fixed, Save -------
    // The create commits through the proxy but its reply is lost (the form
    // holds what cl-api reports then: a keyed unknown outcome). The owner
    // fixes the memo and saves again: the form's held key is pinned, so
    // the engine refuses the changed entry (422 IDEMPOTENCY_KEY_REUSED,
    // "your earlier save went through") and the books hold one entry.
    let lost_memo = unique("lost reply");
    let mut held = cl_ui::forms::held_key::HeldKey::<String>::default();
    let k = held.key_for(&lost_memo);
    a.run(sa.transactions().create_journal_entry(
        &je2(on(), &lost_memo, supplies, cash, "12.50"),
        &PostingOptions::default(),
        k.clone(),
    ))
    .expect("the first save commits");
    held.failed(&ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-0000000000b1")
            .expect("id"),
        idempotency_key: Some(k.clone()),
        keyed: true,
    });
    let fixed = format!("{lost_memo} fixed");
    let again = held.key_for(&fixed);
    assert_eq!(again, k, "the edited form goes under the held key");
    let e = a
        .run(sa.transactions().create_journal_entry(
            &je2(on(), &fixed, supplies, cash, "12.50"),
            &PostingOptions::default(),
            again,
        ))
        .expect_err("the changed entry is refused");
    assert!(
        matches!(e, ApiError::IdempotencyKeyReused { .. }),
        "422 IDEMPOTENCY_KEY_REUSED: {e:?}"
    );
    held.failed(&e);
    assert!(held.pinned(), "still refused until the form starts over");
    let found = a
        .run(sa.transactions().list(
            &cl_api::TransactionFilter {
                search: Some(lost_memo.clone()),
                ..cl_api::TransactionFilter::default()
            },
            cl_api::PageRequest::first_max(),
        ))
        .expect("list");
    assert_eq!(found.data.len(), 1, "one entry in the books");

    // --- Remove ends the computer's sign-ins (review S7) ------------------
    let hs = host.sign_in_here();
    removal_ends_sign_ins(host, &hs, scratch);

    // --- A crash while holding maintenance (review B1) --------------------
    // The host holds maintenance (as during a restore, an import commit or
    // a payroll post); workstations are refused; then the host app goes
    // away without ending it, as a crash or a power cut would.
    let held = host
        .host_rt()
        .block_on(hs.start_maintenance())
        .expect("the host starts maintenance");
    assert!(held.active, "{held:?}");
    // Refused: 503 MAINTENANCE, or its sign-in already ended by the start.
    let refused = b.run(sb.transactions().get(id));
    assert!(
        refused.is_err(),
        "a workstation during the hold: {refused:?}"
    );
    drop(hs);

    // --- Host restart: sign in again, data intact ------------------------
    let gen_before = b.generation().expect("generation before the restart");
    host.stop();
    wait_until("the link to notice", Duration::from_secs(30), || {
        !b.connected()
    });
    host.start_supervisor();
    host.sup()
        .wait_ready(Duration::from_mins(3))
        .expect("host ready again");
    // B1: the supervisor cleared the stale hold before it published the
    // engine (nothing here clears it): the owner signs in and can work.
    let hs2 = host.sign_in_here();
    let now = host
        .host_rt()
        .block_on(hs2.maintenance())
        .expect("maintenance state");
    assert!(!now.active, "the crashed host's hold is gone: {now:?}");
    host.start_proxy();
    b.wait_connected("workstation B after the host restart");
    // S2: the generation a workstation sees keeps going up across a host
    // app restart (the raw engine generation starts at 1 again).
    let gen_after = b.generation().expect("generation after the restart");
    assert!(
        gen_after > gen_before,
        "generation {gen_after} after the restart, {gen_before} before"
    );
    let sb2 = b.sign_in(&host.password);
    let after = b
        .run(sb2.transactions().get(id))
        .expect("read after a host restart");
    assert_eq!(debits(&after.record), m("25.00"), "data intact");
    // The memo of the save made again after the proxy restart (QA-6).
    assert_eq!(after.record.memo.as_deref(), Some(held_memo.as_str()));
    // And the books take writes again, from the host and a workstation.
    b.run(sb2.transactions().create_journal_entry(
        &je2(on(), &unique("after the crash"), supplies, cash, "3.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("a workstation saves after the restart");
    host.host_rt()
        .block_on(hs2.transactions().create_journal_entry(
            &je2(
                on(),
                &unique("host after the crash"),
                supplies,
                cash,
                "4.00",
            ),
            &PostingOptions::default(),
            key(),
        ))
        .expect("the host saves after the restart");
    drop(hs2);

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
        // Refused at start, or started and stopped by the policy: either
        // way nothing listens there.
        if let Ok(h) = host.try_proxy(ip) {
            std::thread::sleep(Duration::from_millis(500));
            assert!(
                !matches!(h.status(), cl_lan::host::ProxyStatus::Listening(_)),
                "a listener on {ip} must be refused: {:?}",
                h.status()
            );
        }
    }

    // --- The request-smuggling shapes, against the live proxy ------------
    smuggling(host, &b_root, &b_store);

    // Every workstation still works afterwards.
    b.wait_connected("workstation B after the refusals");
    let ok = b
        .run(sb2.transactions().get(id))
        .expect("read after the refusals");
    assert_eq!(debits(&ok.record), m("25.00"));

    // --- A restore on the LAN host (last: its hold signs everyone out) ----
    host_restore(host, &b, scratch);
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
    /// Saves the engine rolled back twice on a deadlock (409 `RETRY`
    /// after cl-api's own resend; nothing saved), replayed with the same
    /// idempotency key as the owner's next save would be.
    deadlock_replays: u64,
    /// 429 answers waited out (the per-device limit), never failures.
    rate_limited: u64,
    /// Save round trips (ms), for the Q-P33q budget.
    save_ms: Vec<u64>,
    /// 5xx answers or other failures (must stay 0).
    failures: Vec<String>,
    /// Documents saved by kind (review S6): invoices, payments, bills,
    /// bill payments, inventory sales.
    docs: [u64; 5],
    /// What this client left open on A/R and A/P (exact).
    ar_open: Money,
    ap_open: Money,
}

/// AC3: five workstations save for `CL_LAN_SOAK_SECS` seconds through the
/// live proxy. The mix (review S6): journal entries; 10% bumps of shared
/// records whose memo carries a counter (a lost update shows up as a
/// counter that went back); invoices and payments on them, bills and bill
/// payments, and inventory sales of one shared stocked part. End checks:
/// no failure, every shared counter equals the bumps made, the trial
/// balance balances, A/R and A/P equal what the clients left open (the
/// aging reports and the control accounts alike), and, for runs of three
/// minutes or more, the save round trip is within Q-P33q (p95 <= 250 ms,
/// p99 <= 1 s; review B2).
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
        // The host's engine and host logs are what explain a failed soak:
        // keep the scratch folder (synthetic data only).
        let kept = scratch.keep();
        eprintln!("soak: scratch kept at {}", kept.display());
        std::panic::resume_unwind(p);
    }
}

static SEEN_READS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

const CLIENTS: usize = 5;
const SHARED: usize = 3;
/// Q-P33q, for runs of at least [`BUDGET_FROM`].
const P95_MS: u64 = 250;
const P99_MS: u64 = 1000;
const BUDGET_FROM: Duration = Duration::from_mins(3);

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn counter_memo(base: &str, n: u64) -> String {
    format!("{base} #{n}")
}

/// The counter in `shared k #n` or `shared k #n by c`.
fn counter_of(memo: Option<&str>) -> u64 {
    memo.and_then(|m| m.rsplit_once(" #"))
        .and_then(|(_, n)| n.split(' ').next()?.parse().ok())
        .unwrap_or(0)
}

fn qty(s: &str) -> cl_core::Quantity {
    cl_core::Quantity::parse_wire(s).expect("qty")
}

/// A report figure as exact money.
fn amount_of(a: &cl_api::ReportAmount) -> Money {
    Money::parse_wire(&a.display(4).expect("a number")).expect("money")
}

/// The books the soak's documents use, made once on the host's company.
struct SoakBooks {
    cash: AccountId,
    supplies: AccountId,
    income: AccountId,
    bank: AccountId,
    clearing: AccountId,
    ar: AccountId,
    ap: AccountId,
    part: cl_core::ids::ItemId,
    /// One client and one vendor per soak client (their own documents).
    customers: Vec<cl_core::ids::ContactId>,
    vendors: Vec<cl_core::ids::ContactId>,
    /// The stock bill's vendor and what it left open.
    supplier: cl_core::ids::ContactId,
    stock_ap: Money,
}

fn contact(d: &Desk, s: &Session, t: cl_api::ContactType, name: &str) -> cl_core::ids::ContactId {
    d.run(s.contacts().create(
        &cl_api::NewContact {
            fields: cl_api::ContactFields::named(t, unique(name)),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .expect("contact")
    .record
    .id
}

fn system_account(d: &Desk, s: &Session, tag: &str) -> AccountId {
    d.run(s.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .unwrap_or_else(|| panic!("the {tag} account"))
        .id
}

fn bill_draft(
    vendor: cl_core::ids::ContactId,
    lines: Vec<cl_api::BillLineDraft>,
    amount: Money,
) -> cl_api::BillDraft {
    cl_api::BillDraft {
        vendor,
        date: on(),
        vendor_invoice_number: Some(unique("INV").replace(' ', "-")),
        terms: None,
        due_date: Some(on()),
        memo: None,
        amount_due: amount,
        lines,
        received: true,
        draft_attachment: None,
    }
}

fn soak_books(d: &Desk, s: &Session) -> SoakBooks {
    use cl_api::{AccountType as T, ContactType as C};
    d.run(s.set_inventory_enabled(true, key()))
        .expect("inventory on");
    let cash = account(d, s, T::Asset, "Soak cash");
    let supplies = account(d, s, T::Expense, "Soak supplies");
    let income = account(d, s, T::Revenue, "Soak sales");
    let bank = d
        .run(s.accounts().create(
            &AccountFields {
                name: unique("Soak bank"),
                account_number: None,
                account_type: T::Asset,
                detail_type: Some("bank".into()),
                description: None,
                parent_id: None,
            },
            key(),
        ))
        .expect("bank")
        .record
        .id;
    let accounts = d
        .run(s.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("accounts");
    let asset = accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("inventory_asset"))
        .expect("Inventory Asset")
        .id;
    let cogs = accounts
        .iter()
        .find(|a| a.is_active && a.fields.account_type == T::Cogs)
        .expect("a COGS account")
        .id;
    let part = d
        .run(s.items().create(
            &cl_api::ItemFields {
                purchase: cl_api::ItemPurchase {
                    asset_account_id: Some(asset),
                    cogs_account_id: Some(cogs),
                    ..cl_api::ItemPurchase::default()
                },
                name: unique("Soak part"),
                description: None,
                unit_price: Some(m("10.00")),
                income_account_id: income,
                is_taxable: false,
                item_type: cl_api::ItemType::InventoryPart,
                discount_percent: None,
                tax_code_id: None,
            },
            key(),
        ))
        .expect("part")
        .record
        .id;
    // Stock enough that no run can sell out: 100000 at 0.25.
    let supplier = contact(d, s, C::Vendor, "Soak supplier");
    let stock_ap = m("25000.00");
    let stock = cl_api::BillLineDraft {
        item: Some(part),
        quantity: Some(qty("100000")),
        unit_price: Some(m("0.25")),
        unit: None,
        ..cl_api::BillLineDraft::expense(asset, stock_ap)
    };
    d.run(s.bills().create(
        &bill_draft(supplier, vec![stock], stock_ap),
        &PostingOptions::default(),
        key(),
    ))
    .expect("stock bill");
    SoakBooks {
        cash,
        supplies,
        income,
        bank,
        clearing: system_account(d, s, "payments_clearing"),
        ar: system_account(d, s, "accounts_receivable"),
        ap: system_account(d, s, "accounts_payable"),
        part,
        customers: (0..CLIENTS)
            .map(|i| contact(d, s, C::Customer, &format!("Soak client {i}")))
            .collect(),
        vendors: (0..CLIENTS)
            .map(|i| contact(d, s, C::Vendor, &format!("Soak vendor {i}")))
            .collect(),
        supplier,
        stock_ap,
    }
}

/// A report call, waiting out the engine's report limit once.
fn report<T>(d: &Desk, mut call: impl FnMut() -> Result<T, ApiError>) -> T {
    match call() {
        Err(ApiError::RateLimited { retry_after }) => {
            std::thread::sleep(
                retry_after.unwrap_or(Duration::from_mins(1)) + Duration::from_secs(1),
            );
            call().unwrap_or_else(|e| panic!("report: {e}"))
        }
        r => {
            let _ = d;
            r.unwrap_or_else(|e| panic!("report: {e}"))
        }
    }
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
    let books = soak_books(&desks[0], &first);
    let (cash, supplies) = (books.cash, books.supplies);
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
                let (shared, bumps, password, books) = (&shared, &bumps, &host.password, &books);
                scope.spawn(move || soak_client(i, desk, password, books, shared, bumps, deadline))
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
    let replays: u64 = tallies.iter().map(|t| t.deadlock_replays).sum();
    let limited: u64 = tallies.iter().map(|t| t.rate_limited).sum();
    let docs: Vec<u64> = (0..5)
        .map(|k| tallies.iter().map(|t| t.docs[k]).sum())
        .collect();
    let mut lat: Vec<u64> = tallies
        .iter()
        .flat_map(|t| t.save_ms.iter().copied())
        .collect();
    lat.sort_unstable();
    let pct = |p: usize| {
        lat.get((lat.len() * p / 100).min(lat.len().saturating_sub(1)))
            .copied()
            .unwrap_or(0)
    };
    eprintln!(
        "soak: save round trip through the proxy p50 {} ms, p95 {} ms, p99 {} ms, max {:?} ms ({} saves); {limited} 429s waited out",
        pct(50),
        pct(95),
        pct(99),
        lat.last(),
        lat.len()
    );
    let failures: Vec<&String> = tallies.iter().flat_map(|t| &t.failures).collect();
    eprintln!(
        "soak: {CLIENTS} clients, {:?}: {saves} saves ({} invoices, {} payments, {} bills, {} bill payments, {} inventory sales), {reads} reads, {conflicts} resolved conflicts, {replays} deadlock replays, {lost} lost, {} failures",
        started.elapsed(),
        docs[0],
        docs[1],
        docs[2],
        docs[3],
        docs[4],
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
    let d0 = &desks[0];
    // The trial balance balances (accrual, through the soak's date).
    let tb = report(d0, || {
        d0.run(first.reports().trial_balance(
            CalendarDate::from_ymd(2026, 1, 1).expect("date"),
            on(),
            cl_api::ReportBasis::Accrual,
        ))
    });
    assert_eq!(
        amount_of(&tb.total_debits),
        amount_of(&tb.total_credits),
        "the trial balance balances"
    );
    // A/R: what the clients left open = the aging's exact total = the
    // control account.
    let ar_open = Money::sum(tallies.iter().map(|t| t.ar_open)).expect("sum");
    let aging = report(d0, || d0.run(first.reports().ar_aging_summary(on())));
    assert_eq!(aging.total_exact, ar_open, "A/R aging = open invoices");
    let ar_gl = d0
        .run(first.accounts().get(books.ar))
        .expect("A/R")
        .record
        .balance
        .unwrap_or_default();
    assert_eq!(ar_gl, ar_open, "A/R account = open invoices");
    // A/P: the same for bills (the stock bill stays open).
    let ap_open =
        Money::sum(tallies.iter().map(|t| t.ap_open).chain([books.stock_ap])).expect("sum");
    let ap_aging = report(d0, || d0.run(first.reports().ap_aging_summary(on())));
    let aged = Money::sum(
        books
            .vendors
            .iter()
            .chain([&books.supplier])
            .filter_map(|v| ap_aging.total_of(*v).map(amount_of)),
    )
    .expect("sum");
    assert_eq!(aged, ap_open, "A/P aging = open bills");
    let ap_gl = d0
        .run(first.accounts().get(books.ap))
        .expect("A/P")
        .record
        .balance
        .unwrap_or_default();
    assert_eq!(ap_gl.abs(), ap_open, "A/P account = open bills");
    // Review B2: the save budget (Q-P33q), measured through the proxy and
    // the forwarder, on every save of a run of three minutes or more.
    if run_for >= BUDGET_FROM {
        assert!(pct(95) <= P95_MS, "p95 {} ms > {P95_MS} ms", pct(95));
        assert!(pct(99) <= P99_MS, "p99 {} ms > {P99_MS} ms", pct(99));
    }
}

/// Times one keyed save (each attempt's round trip goes into the tally).
fn timed<T>(
    t: &mut Tally,
    mut save: impl FnMut(IdempotencyKey) -> Result<T, ApiError>,
) -> Result<T, ApiError> {
    let mut lat = Vec::new();
    let r = keyed(t, |k| {
        let at = Instant::now();
        let r = save(k);
        lat.push(at.elapsed());
        r
    });
    t.save_ms.extend(lat.iter().map(|d| ms(*d)));
    r
}

#[allow(
    clippy::too_many_arguments,
    clippy::many_single_char_names,
    reason = "one client's whole loop; the scope owns the shared state"
)]
fn soak_client(
    i: usize,
    desk: &Desk,
    password: &Secret,
    books: &SoakBooks,
    shared: &[cl_core::ids::TransactionId],
    bumps: &[AtomicU64],
    deadline: Instant,
) -> Tally {
    let (cash, supplies) = (books.cash, books.supplies);
    let mut t = Tally {
        ar_open: Money::ZERO,
        ap_open: Money::ZERO,
        ..Tally::default()
    };
    let s = desk.sign_in(password);
    let (customer, vendor) = (books.customers[i], books.vendors[i]);
    // This client's open documents, oldest first (id, amount).
    let mut invoices: std::collections::VecDeque<(cl_core::ids::TransactionId, Money)> =
        std::collections::VecDeque::new();
    let mut bills: std::collections::VecDeque<(cl_core::ids::TransactionId, Money)> =
        std::collections::VecDeque::new();
    let mut n: u64 = 0;
    while Instant::now() < deadline {
        n += 1;
        std::thread::sleep(PACE);
        // Amounts from integer minor units: 1.00 to 9.99.
        let cents = 100 + (n * 37 + i as u64 * 11) % 900;
        let amount = Money::from_cents(i64::try_from(cents).unwrap_or(100)).expect("cents");
        match n % 10 {
            0 => bump_shared(i, n, desk, &s, &mut t, books, shared, bumps),
            2 => {
                let line = cl_api::InvoiceLine {
                    is_taxable: Some(false),
                    ..cl_api::InvoiceLine::account(books.income, qty("1"), amount)
                };
                let new = cl_api::NewInvoice::new(customer, on(), vec![line]);
                match timed(&mut t, |k| {
                    desk.run(s.invoices().create(&new, &PostingOptions::default(), k))
                }) {
                    Ok(made) => {
                        t.saves += 1;
                        t.docs[0] += 1;
                        invoices.push_back((made.invoice.record.txn.id, amount));
                        t.ar_open = t.ar_open.checked_add(amount).expect("sum");
                    }
                    Err(e) => t.failures.push(format!("invoice: {e}")),
                }
            }
            4 => {
                if let Some((inv, owed)) = invoices.pop_front() {
                    receive(desk, &s, &mut t, books, customer, inv, owed);
                }
            }
            5 => {
                let draft = bill_draft(
                    vendor,
                    vec![cl_api::BillLineDraft::expense(supplies, amount)],
                    amount,
                );
                match timed(&mut t, |k| {
                    desk.run(s.bills().create(&draft, &PostingOptions::default(), k))
                }) {
                    Ok(made) => {
                        t.saves += 1;
                        t.docs[2] += 1;
                        bills.push_back((made.record.txn.id, amount));
                        t.ap_open = t.ap_open.checked_add(amount).expect("sum");
                    }
                    Err(e) => t.failures.push(format!("bill: {e}")),
                }
            }
            7 => {
                if let Some((bill, owed)) = bills.pop_front() {
                    let req = cl_api::PayBillsRequest {
                        bank_account: books.bank,
                        date: on(),
                        bills: vec![cl_api::BillToPay {
                            bill,
                            cash: owed,
                            discount: Money::ZERO,
                        }],
                        credits: Vec::new(),
                        discount_account: None,
                        method: cl_api::PayMethod::Check(cl_api::CheckNumbering::PrintLater),
                        memo: None,
                    };
                    match timed(&mut t, |k| {
                        desk.run(s.bill_payments().pay(&req, &PostingOptions::default(), k))
                    }) {
                        Ok(_) => {
                            t.saves += 1;
                            t.docs[3] += 1;
                            t.ap_open = t.ap_open.checked_sub(owed).expect("sub");
                        }
                        Err(e) => t.failures.push(format!("bill payment: {e}")),
                    }
                }
            }
            8 => {
                // An inventory sale: one of the shared part at its 10.00.
                let line = cl_api::InvoiceLine {
                    is_taxable: Some(false),
                    ..cl_api::InvoiceLine::item(books.part, qty("1"))
                };
                let new = cl_api::NewInvoice::new(customer, on(), vec![line]);
                match timed(&mut t, |k| {
                    desk.run(s.invoices().create(&new, &PostingOptions::default(), k))
                }) {
                    Ok(made) => {
                        t.saves += 1;
                        t.docs[4] += 1;
                        let price = m("10.00");
                        invoices.push_back((made.invoice.record.txn.id, price));
                        t.ar_open = t.ar_open.checked_add(price).expect("sum");
                    }
                    Err(e) => t.failures.push(format!("inventory sale: {e}")),
                }
            }
            _ => {
                let entry = je2(
                    on(),
                    &unique(&format!("soak {i}")),
                    supplies,
                    cash,
                    &amount.to_wire(),
                );
                match timed(&mut t, |k| {
                    desk.run(s.transactions().create_journal_entry(
                        &entry,
                        &PostingOptions::default(),
                        k,
                    ))
                }) {
                    Ok(_) => t.saves += 1,
                    Err(e) => t.failures.push(format!("entry: {e}")),
                }
                // The UI's read pattern after a change: the entry list.
                if n.is_multiple_of(3) {
                    match read(&mut t, || desk.run(s.ledger_version())) {
                        Ok(_) => t.reads += 1,
                        Err(e) => t.failures.push(format!("ledger version: {e}")),
                    }
                }
            }
        }
    }
    t
}

/// Receives `owed` on `inv` in full (preview, then the save copies its
/// amounts and balance token, as the form does).
fn receive(
    desk: &Desk,
    s: &Session,
    t: &mut Tally,
    books: &SoakBooks,
    customer: cl_core::ids::ContactId,
    inv: cl_core::ids::TransactionId,
    owed: Money,
) {
    let req = cl_api::PaymentPreviewRequest {
        customer_id: customer,
        date: on(),
        amount: Some(owed),
        deposit_to: Some(books.clearing),
        applications: Some(vec![cl_api::ApplicationRequest {
            invoice_id: inv,
            amount: Some(owed),
            discount_amount: None,
            write_off_amount: None,
        }]),
        credit_applications: Vec::new(),
        hold_as_deposit: false,
        editing_payment_id: None,
    };
    let p = match read(t, || desk.run(s.customer_payments().preview(&req))) {
        Ok(p) => p,
        Err(e) => {
            t.failures.push(format!("payment preview: {e}"));
            return;
        }
    };
    t.reads += 1;
    let save = cl_api::NewCustomerPayment {
        customer_id: customer,
        date: on(),
        amount: p.amount,
        deposit_to: books.clearing,
        payment_method: Some("check".into()),
        ref_no: None,
        memo: None,
        applications: p
            .invoices
            .iter()
            .filter(|r| r.payment.is_positive())
            .map(|r| cl_api::SaveApplication {
                invoice_id: r.invoice_id,
                amount: r.payment,
                expected_balance_due: r.balance_due_exact.clone(),
                discount_amount: None,
                write_off_amount: None,
            })
            .collect(),
        credit_applications: Vec::new(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: false,
        bank_feed_item_id: None,
    };
    match timed(t, |k| {
        desk.run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), k),
        )
    }) {
        Ok(_) => {
            t.saves += 1;
            t.docs[1] += 1;
            t.ar_open = t.ar_open.checked_sub(owed).expect("sub");
        }
        Err(e) => t.failures.push(format!("payment: {e}")),
    }
}

/// 10%: bump a shared record's counter (read, then save on it).
#[allow(
    clippy::too_many_arguments,
    reason = "the client loop's state, passed through"
)]
fn bump_shared(
    i: usize,
    n: u64,
    desk: &Desk,
    s: &Session,
    t: &mut Tally,
    books: &SoakBooks,
    shared: &[cl_core::ids::TransactionId],
    bumps: &[AtomicU64],
) {
    let (cash, supplies) = (books.cash, books.supplies);
    let k = (usize::try_from(n / 10).unwrap_or(0) + i) % shared.len();
    loop {
        let got = match read(t, || desk.run(s.transactions().get(shared[k]))) {
            Ok(g) => g,
            Err(e) => {
                t.failures.push(format!("read shared: {e}"));
                return;
            }
        };
        t.reads += 1;
        let now = counter_of(got.record.memo.as_deref());
        // Each client's content is its own: two clients never ask for the
        // same record, so a refused save can never be reconciled as
        // "already saved" from the other's write.
        let memo = format!("{} by {i}", counter_memo(&format!("shared {k}"), now + 1));
        let entry = je2(on(), &memo, supplies, cash, "1.00");
        let saved = timed(t, |k2| {
            desk.run(s.transactions().update_journal_entry(
                shared[k],
                &got.version,
                &entry,
                &PostingOptions::default(),
                k2,
            ))
        });
        match saved {
            Ok(saved) => {
                t.saves += 1;
                // Two saves accepted from the same read of the same
                // record: one overwrote the other (a lost update).
                let read = got.version.stamp().map(|s| format!("{k}@{s:?}"));
                if let Some(r) = read
                    && !SEEN_READS
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(r.clone())
                {
                    t.lost += 1;
                    t.failures.push(format!(
                        "lost update: two saves accepted from read {r} (deadlock replays so far: {})",
                        t.deadlock_replays
                    ));
                }
                let prev = bumps[k].fetch_add(1, Ordering::SeqCst);
                if counter_of(saved.record.memo.as_deref()) != now + 1 || prev + 1 < now + 1 {
                    t.lost += 1;
                }
                return;
            }
            Err(e) if is_stale(&e) => t.conflicts += 1,
            Err(e) => {
                t.failures.push(format!("shared save: {e}"));
                return;
            }
        }
    }
}
