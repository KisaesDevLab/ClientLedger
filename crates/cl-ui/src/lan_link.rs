//! The app's LAN link (P33 D2): wires cl-lan to the supervisor and the
//! keychain. Used by the window and by the headless `--lan-serve` host.
//!
//! - [`LanHost`]: the proxy on the chosen interface, fed from the
//!   supervisor's snapshot (port, workstation token, generation, engine
//!   commit) by a small pump thread; announces on mDNS when asked.
//! - [`LanWorkstation`]: the loopback forwarder and heartbeat; its
//!   [`EndpointSource`] replaces the local engine's for cl-api.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cl_api::EndpointSource;
use cl_host::HostState;
use cl_lan::client::{LinkState, Workstation, WorkstationConfig};
use cl_lan::discovery::Discovery;
use cl_lan::host::{HostProxy, ProxyConfig, ProxyStatus, Upstream};
use cl_lan::maintenance::Gate;
use cl_lan::pairing::Pairing;
use cl_lan::pinning::Fingerprint;
use cl_lan::policy::Interfaces;
use cl_lan::registry::Registry;
use cl_lan::settings::{HostSettings, WorkstationSettings};
use cl_lan::tls::{Identity, Role};
use cl_settings::SecretStore;

use crate::engine_link::SnapshotSource;

/// Folder for the LAN files (certificate, workstation list, `lan.json`).
#[must_use]
pub fn lan_dir(root: &Path) -> PathBuf {
    root.join("lan")
}

/// `lan.json` under the app data root.
#[must_use]
pub fn settings_path(root: &Path) -> PathBuf {
    lan_dir(root).join("lan.json")
}

/// The app version compared between host and workstations (Q-P33p).
#[must_use]
pub fn app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// The engine commit this build ships with, when the build knows it.
#[must_use]
pub fn bundled_engine_commit() -> Option<String> {
    option_env!("CL_ENGINE_COMMIT").map(str::to_string)
}

/// Why the LAN link could not start.
#[derive(Debug, thiserror::Error)]
pub enum LanStartError {
    /// The async runtime could not start.
    #[error("the network service could not start")]
    Runtime,
    /// Certificate or keychain trouble.
    #[error("the security certificate could not be prepared: {0}")]
    Identity(String),
    /// The workstation link could not start.
    #[error("the connection to the host could not start: {0}")]
    Link(String),
    /// `lan.json` has no pinned host.
    #[error("this computer is not paired with a host")]
    NotPaired,
}

fn runtime() -> Result<tokio::runtime::Runtime, LanStartError> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("cl-lan")
        .enable_all()
        .build()
        .map_err(|_| LanStartError::Runtime)
}

/// Turns a supervisor snapshot into the proxy's upstream.
#[must_use]
pub fn upstream_of(snap: &cl_host::HostSnapshot) -> Option<Upstream> {
    if snap.status.state != HostState::Running {
        return None;
    }
    let ep = snap.endpoint.as_ref()?;
    let lan = ep.lan_token.as_ref()?;
    Some(Upstream {
        port: ep.port,
        lan_token: cl_core::Secret::new(lan.expose()),
        generation: ep.generation,
        engine_commit: snap.status.engine_commit.clone().unwrap_or_default(),
    })
}

/// The host side, running.
pub struct LanHost {
    rt: tokio::runtime::Runtime,
    proxy: Option<HostProxy>,
    gate: Arc<Gate>,
    registry: Arc<Registry>,
    pairing: Arc<Pairing>,
    identity: Identity,
    discovery: Arc<dyn Discovery>,
    pump_stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for LanHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanHost")
            .field("fingerprint", &self.identity.fingerprint())
            .finish_non_exhaustive()
    }
}

impl LanHost {
    /// Starts the proxy for `settings`, fed from `snapshots`.
    ///
    /// # Errors
    /// `Runtime`, `Identity`.
    pub fn start(
        snapshots: Arc<dyn SnapshotSource>,
        settings: &HostSettings,
        root: &Path,
        store: &dyn SecretStore,
        interfaces: Arc<dyn Interfaces>,
        discovery: Arc<dyn Discovery>,
    ) -> Result<LanHost, LanStartError> {
        let rt = runtime()?;
        let dir = lan_dir(root);
        let (identity, _) = Identity::load_or_create(store, &dir, Role::Host)
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        let registry = Arc::new(Registry::load(dir.join("lan-devices.json")));
        let pairing = Arc::new(Pairing::default());
        let gate = Arc::new(Gate::default());
        let pump_stop = Arc::new(AtomicBool::new(false));
        let proxy = rt
            .block_on(HostProxy::start(
                ProxyConfig {
                    interface: settings.interface.clone(),
                    ip: settings.ip,
                    port: settings.port,
                    app_version: app_version(),
                },
                identity.clone(),
                Arc::clone(&registry),
                Arc::clone(&pairing),
                Arc::clone(&gate) as Arc<dyn cl_lan::host::HostUpstream>,
                interfaces,
            ))
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        {
            // Feeds the gate from the supervisor, and keeps the mDNS
            // announcement on the address the proxy listens on (rebind:
            // re-announce; stop: withdraw).
            let (gate, stop) = (Arc::clone(&gate), Arc::clone(&pump_stop));
            let mut status = proxy.subscribe();
            let (disc, announce) = (Arc::clone(&discovery), settings.announce);
            let tag = identity.fingerprint().prefix();
            let _ = std::thread::Builder::new()
                .name("cl-lan-gate".into())
                .spawn(move || {
                    let mut announced: Option<SocketAddr> = None;
                    while !stop.load(Ordering::SeqCst) {
                        gate.set_upstream(upstream_of(&snapshots.snapshot()));
                        let now = match &*status.borrow_and_update() {
                            ProxyStatus::Listening(a) if announce => Some(*a),
                            _ => None,
                        };
                        if now != announced {
                            disc.stop();
                            if let Some(a) = now
                                && let Err(e) = disc.announce(a.ip(), a.port(), &tag)
                            {
                                tracing::warn!(
                                    error = %e,
                                    "LAN announce failed; the manual address still works"
                                );
                            }
                            announced = now;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                });
        }
        tracing::info!("LAN host mode on");
        Ok(LanHost {
            rt,
            proxy: Some(proxy),
            gate,
            registry,
            pairing,
            identity,
            discovery,
            pump_stop,
        })
    }

    /// The proxy status.
    #[must_use]
    pub fn status(&self) -> ProxyStatus {
        self.proxy.as_ref().map_or(
            ProxyStatus::Stopped(cl_lan::host::Stopped::Shutdown),
            HostProxy::status,
        )
    }

    /// The host certificate's fingerprint.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        self.identity.fingerprint()
    }

    /// Pairing state (codes, the Allow prompt).
    #[must_use]
    pub fn pairing(&self) -> &Arc<Pairing> {
        &self.pairing
    }

    /// Paired workstations.
    #[must_use]
    pub fn registry(&self) -> &Arc<Registry> {
        &self.registry
    }

    /// The gate closed around restores.
    #[must_use]
    pub fn gate(&self) -> &Arc<Gate> {
        &self.gate
    }

    /// Stops the proxy and the announcement.
    pub fn stop(&mut self) {
        self.pump_stop.store(true, Ordering::SeqCst);
        self.discovery.stop();
        if let Some(p) = self.proxy.take() {
            self.rt.block_on(p.shutdown());
        }
        tracing::info!("LAN host mode off");
    }
}

impl Drop for LanHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The workstation side, running.
pub struct LanWorkstation {
    _rt: tokio::runtime::Runtime,
    link: Workstation,
}

impl std::fmt::Debug for LanWorkstation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanWorkstation")
            .field("link", &self.link)
            .finish_non_exhaustive()
    }
}

impl LanWorkstation {
    /// Starts the forwarder and heartbeat for the paired host.
    ///
    /// # Errors
    /// `NotPaired`, `Runtime`, `Identity`, `Link`.
    pub fn start(
        settings: &WorkstationSettings,
        root: &Path,
        store: &dyn SecretStore,
    ) -> Result<LanWorkstation, LanStartError> {
        let fp = settings.fingerprint().ok_or(LanStartError::NotPaired)?;
        let rt = runtime()?;
        let (device, _) = Identity::load_or_create(store, &lan_dir(root), Role::Device)
            .map_err(|e| LanStartError::Identity(e.to_string()))?;
        let link = rt
            .block_on(Workstation::start(
                WorkstationConfig {
                    host: settings.host,
                    host_fingerprint: fp,
                    app_version: app_version(),
                    engine_commit: bundled_engine_commit(),
                },
                &device,
            ))
            .map_err(|e| LanStartError::Link(e.to_string()))?;
        Ok(LanWorkstation { _rt: rt, link })
    }

    /// cl-api's endpoint for this link.
    #[must_use]
    pub fn endpoint_source(&self) -> Arc<dyn EndpointSource> {
        Arc::new(self.link.endpoint_source())
    }

    /// The link state now.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.link.state()
    }

    /// The host address.
    #[must_use]
    pub fn host(&self, settings: &WorkstationSettings) -> SocketAddr {
        settings.host
    }
}

/// cl-api's endpoint when a workstation's link could not start: nothing
/// is reachable, and nothing local stands in for the host.
#[derive(Debug, Default)]
pub struct NoHost;

impl EndpointSource for NoHost {
    fn current(&self) -> cl_api::EndpointState {
        cl_api::EndpointState::Down { permanent: true }
    }
}

/// The status bar's view of a workstation link: the host's books, as
/// far as this computer can tell.
pub struct WorkstationProbe(pub std::sync::Weak<LanWorkstation>);

impl crate::shell::HostProbe for WorkstationProbe {
    fn view(&self) -> crate::shell::HostView {
        use crate::shell::HostView;
        match self.0.upgrade().map(|w| w.state()) {
            None => HostView::Stopped,
            Some(LinkState::Connected { .. }) => HostView::Running,
            Some(LinkState::Connecting) => HostView::Starting,
            Some(LinkState::HostEngineDown | LinkState::HostMaintenance) => HostView::Restarting,
            Some(s) => HostView::Failed(crate::i18n::t(&format!("lan.link.{}", s.key()))),
        }
    }
}

/// A workstation app run (P33): the loopback forwarder to the paired host
/// (in this process: no database, no engine, no child process), and the
/// single-instance relay a supervisor would otherwise run (a second
/// launch brings this window forward; `host stop` closes it).
pub struct WorkstationRun {
    /// The link; the only strong reference (the status code holds weak
    /// ones), so dropping this stops the forwarder.
    pub link: Arc<LanWorkstation>,
    /// A second launch asked for the window.
    pub focus: Arc<AtomicBool>,
    /// `host stop` asked the app to quit.
    pub stop: Arc<AtomicBool>,
    relay_stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkstationRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkstationRun")
            .field("link", &self.link)
            .finish_non_exhaustive()
    }
}

impl WorkstationRun {
    /// Starts the forwarder for `settings` and the relay over `guard`.
    /// Registers the link with [`crate::lan_status::set_workstation`], so
    /// the overlay, banners and refresh run.
    ///
    /// # Errors
    /// See [`LanWorkstation::start`], with the guard handed back so the
    /// app keeps the single-instance lock. The caller must not fall back
    /// to a local engine: the owner would be keying into an empty book.
    pub fn start(
        settings: &WorkstationSettings,
        root: &Path,
        store: &dyn SecretStore,
        guard: cl_host::InstanceGuard,
    ) -> Result<WorkstationRun, (LanStartError, cl_host::InstanceGuard)> {
        let link = match LanWorkstation::start(settings, root, store) {
            Ok(l) => Arc::new(l),
            Err(e) => return Err((e, guard)),
        };
        crate::lan_status::set_workstation(&link);
        let focus = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let relay_stop = Arc::new(AtomicBool::new(false));
        let (f, s, end) = (
            Arc::clone(&focus),
            Arc::clone(&stop),
            Arc::clone(&relay_stop),
        );
        let weak = Arc::downgrade(&link);
        let spawned = std::thread::Builder::new()
            .name("cl-instance-relay".into())
            .spawn(move || {
                // Holds the lock for the whole run; ends with the app.
                while !end.load(Ordering::SeqCst) {
                    if guard.take_focus_request() {
                        f.store(true, Ordering::SeqCst);
                    }
                    if guard.stop_requested() {
                        s.store(true, Ordering::SeqCst);
                    }
                    let state = weak.upgrade().map_or("stopped", |w| w.state().key());
                    guard.set_status(&format!("workstation {state}"));
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the second-launch relay");
        }
        tracing::info!("office network: workstation mode");
        Ok(WorkstationRun {
            link,
            focus,
            stop,
            relay_stop,
        })
    }

    /// The status bar's probe.
    #[must_use]
    pub fn probe(&self) -> Arc<dyn crate::shell::HostProbe> {
        Arc::new(WorkstationProbe(Arc::downgrade(&self.link)))
    }
}

impl Drop for WorkstationRun {
    fn drop(&mut self) {
        self.relay_stop.store(true, Ordering::SeqCst);
    }
}

// ---- The live office network for the setup window (P33 D4) ----------------

/// What the app hands the setup window: its data folder, the keychain and
/// the running host (weak, so closing the app still stops it in order).
struct LiveContext {
    root: PathBuf,
    store: Arc<dyn SecretStore>,
    host: std::sync::Mutex<std::sync::Weak<LanHost>>,
}

static LIVE: std::sync::OnceLock<LiveContext> = std::sync::OnceLock::new();

/// Called once by the app at start-up.
pub fn install(root: PathBuf, store: Arc<dyn SecretStore>) {
    let _ = LIVE.set(LiveContext {
        root,
        store,
        host: std::sync::Mutex::new(std::sync::Weak::new()),
    });
}

/// The host running in this process (the setup window's code, Allow and
/// paired computers act on it).
pub fn set_running_host(h: &Arc<LanHost>) {
    if let Some(l) = LIVE.get() {
        *l.host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(h);
    }
}

fn running_host() -> Option<Arc<LanHost>> {
    LIVE.get().and_then(|l| {
        l.host
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
    })
}

/// A host runs in this process (the status bar's "Sharing these books").
#[must_use]
pub fn host_running() -> bool {
    running_host().is_some()
}

type Event = (cl_api::LanAuditEvent, Option<String>, Option<String>);

/// The live office network behind the setup window. Settings changes are
/// written to `lan.json` and take effect at the next start ("Restart
/// ClientLedger"); the code, Allow and Remove act on the running host.
/// Network work (discovery, pairing) runs on its own thread, never on the
/// window's.
pub struct LiveLan {
    settings: cl_lan::settings::LanSettings,
    interfaces: Vec<cl_lan::policy::Candidate>,
    restart: bool,
    found: Vec<(String, String)>,
    events: Vec<Event>,
    pair_rx: Option<
        std::sync::mpsc::Receiver<Result<cl_lan::settings::WorkstationSettings, &'static str>>,
    >,
    answer: Option<Result<(), &'static str>>,
    browse_rx: Option<std::sync::mpsc::Receiver<Vec<(String, String)>>>,
}

impl LiveLan {
    /// Reads `lan.json` and this computer's connections.
    #[must_use]
    pub fn open() -> LiveLan {
        use cl_lan::policy::Interfaces;
        let settings = LIVE
            .get()
            .map(|l| cl_lan::settings::LanSettings::load(&settings_path(&l.root)))
            .unwrap_or_default();
        LiveLan {
            interfaces: cl_lan::policy::candidates(&cl_lan::policy::OsInterfaces.list()),
            settings,
            restart: false,
            found: Vec::new(),
            events: Vec::new(),
            pair_rx: None,
            answer: None,
            browse_rx: None,
        }
    }

    fn save(&mut self) -> Result<(), &'static str> {
        let l = LIVE.get().ok_or("lan.restart")?;
        let _ = std::fs::create_dir_all(lan_dir(&l.root));
        self.settings
            .save(&settings_path(&l.root))
            .map_err(|_| "lan.save_failed")?;
        self.restart = true;
        Ok(())
    }
}

impl crate::screens::lan_setup::LanControl for LiveLan {
    fn view(&self) -> crate::screens::lan_setup::LanView {
        use crate::screens::lan_setup::{CodeShown, DeviceRow, LanView};
        use cl_lan::pairing::CodeView;
        let host = running_host();
        let chosen = self.settings.host.as_ref().and_then(|h| {
            self.interfaces
                .iter()
                .position(|c| c.addr.name == h.interface && c.addr.ip == h.ip)
        });
        let (code, request, devices, fingerprint) = match &host {
            Some(h) => (
                match h.pairing().view() {
                    CodeView::None => CodeShown::None,
                    CodeView::Active {
                        display,
                        seconds_left,
                        tries_left,
                    } => CodeShown::Active {
                        display,
                        minutes: seconds_left.div_ceil(60),
                        tries: tries_left,
                    },
                    CodeView::Burned => CodeShown::Burned,
                },
                h.pairing().pending().map(|p| (p.label, p.peer.to_string())),
                h.registry()
                    .list()
                    .into_iter()
                    .map(|d| DeviceRow {
                        label: d.label,
                        fingerprint: d.fingerprint,
                        paired: d.paired_at.get(..10).unwrap_or_default().to_string(),
                        signed_in: None,
                    })
                    .collect(),
                Some(h.fingerprint().hex()),
            ),
            None => (CodeShown::None, None, Vec::new(), None),
        };
        LanView {
            mode: self.settings.mode,
            interfaces: self.interfaces.clone(),
            chosen,
            fingerprint,
            code,
            request,
            devices,
            announce: self.settings.host.as_ref().is_none_or(|h| h.announce),
            start_at_sign_in: self.settings.host.as_ref().is_some_and(|h| h.background),
            found: self.found.clone(),
            paired_host: self
                .settings
                .workstation
                .as_ref()
                .map(|w| w.host.to_string()),
            // The running workstation link's state (it drives the status
            // bar too); "connecting" before one runs.
            link: crate::lan_status::workstation_link_key().unwrap_or("connecting"),
            restart_needed: self.restart,
        }
    }

    fn set_host(&mut self, on: bool, chosen: Option<usize>) -> Result<(), &'static str> {
        use cl_lan::settings::{LanSettings, Mode};
        if on {
            let c = chosen
                .and_then(|i| self.interfaces.get(i))
                .filter(|c| c.refusal.is_none())
                .ok_or("lan.host.iface.none")?;
            let keep = self.settings.host.clone();
            let mut h = LanSettings::host_on(c.addr.name.clone(), c.addr.ip);
            if let Some(k) = keep {
                h.announce = k.announce;
                h.background = k.background;
            }
            self.settings.host = Some(h);
            self.settings.mode = Mode::Host;
            self.events
                .push((cl_api::LanAuditEvent::LanHostModeOn, None, None));
        } else {
            self.settings.mode = Mode::Off;
            if let Some(h) = self.settings.host.as_mut()
                && h.background
            {
                h.background = false;
                let _ = cl_host::service::uninstall();
            }
            self.events
                .push((cl_api::LanAuditEvent::LanHostModeOff, None, None));
        }
        self.save()
    }

    fn new_code(&mut self) {
        if let Some(h) = running_host()
            && let Err(e) = h.pairing().new_code(&h.fingerprint())
        {
            tracing::warn!(error = %e, "pairing code could not be made");
        }
    }

    fn cancel_code(&mut self) {
        if let Some(h) = running_host() {
            h.pairing().cancel_code();
        }
    }

    fn decide(&mut self, allow: bool) {
        if let Some(h) = running_host()
            && let Some(p) = h.pairing().pending()
            && h.pairing().decide(p.id, allow)
            && allow
        {
            self.events.push((
                cl_api::LanAuditEvent::LanDevicePaired,
                Some(p.fingerprint.hex()),
                Some(p.label),
            ));
        }
    }

    fn remove(&mut self, i: usize) {
        let Some(h) = running_host() else {
            return;
        };
        let Some(d) = h.registry().list().into_iter().nth(i) else {
            return;
        };
        if let Some(fp) = Fingerprint::from_hex(&d.fingerprint)
            && h.registry().remove(&fp).unwrap_or(false)
        {
            self.events.push((
                cl_api::LanAuditEvent::LanDeviceRemoved,
                Some(d.fingerprint),
                Some(d.label),
            ));
        }
    }

    fn set_announce(&mut self, on: bool) {
        if let Some(h) = self.settings.host.as_mut() {
            h.announce = on;
            let _ = self.save();
        }
    }

    fn set_start_at_sign_in(&mut self, on: bool) -> Result<(), &'static str> {
        let Some(h) = self.settings.host.as_mut() else {
            return Err("lan.host.iface.none");
        };
        let r = if on {
            std::env::current_exe()
                .map_err(|_| "lan.service_failed")
                .and_then(|exe| {
                    cl_host::service::install(&exe, None).map_err(|_| "lan.service_failed")
                })
        } else {
            cl_host::service::uninstall().map_err(|_| "lan.service_failed")
        };
        r?;
        h.background = on;
        self.save()
    }

    fn search(&mut self) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.browse_rx = Some(rx);
        let _ = std::thread::Builder::new()
            .name("cl-lan-browse".into())
            .spawn(move || {
                let found = cl_lan::discovery::MdnsDiscovery::default()
                    .browse(Duration::from_secs(3))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| (f.name, f.addr.to_string()))
                    .collect();
                let _ = tx.send(found);
            });
    }

    fn pair(&mut self, addr: &str, code: &str) {
        let Some(l) = LIVE.get() else {
            self.answer = Some(Err("lan.restart"));
            return;
        };
        let addr = addr.trim();
        let host: Option<SocketAddr> = addr.parse().ok().or_else(|| {
            addr.parse::<std::net::IpAddr>()
                .ok()
                .map(|ip| SocketAddr::new(ip, cl_lan::host::DEFAULT_PORT))
        });
        let Some(host) = host else {
            self.answer = Some(Err("lan.ws.no_address"));
            return;
        };
        let Some(code) = cl_lan::pinning::PairingCode::parse(code) else {
            self.answer = Some(Err("lan.pair.refused.wrong_code"));
            return;
        };
        let (store, dir) = (Arc::clone(&l.store), lan_dir(&l.root));
        let (tx, rx) = std::sync::mpsc::channel();
        self.pair_rx = Some(rx);
        let _ = std::thread::Builder::new()
            .name("cl-lan-pair".into())
            .spawn(move || {
                let r = (|| {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|_| "lan.ws.unreachable")?;
                    let (device, _) = Identity::load_or_create(&*store, &dir, Role::Device)
                        .map_err(|_| "lan.ws.unreachable")?;
                    let label = std::env::var("COMPUTERNAME")
                        .or_else(|_| std::env::var("HOSTNAME"))
                        .unwrap_or_else(|_| "Workstation".into());
                    let fp = rt
                        .block_on(cl_lan::client::pair(host, code, &device, &label))
                        .map_err(|e| match e {
                            cl_lan::client::CallError::Pair(r) => refusal_key(r),
                            _ => "lan.ws.unreachable",
                        })?;
                    Ok(WorkstationSettings {
                        host,
                        host_fingerprint: fp.hex(),
                    })
                })();
                let _ = tx.send(r);
            });
    }

    fn poll_pair(&mut self) -> Option<Result<(), &'static str>> {
        if let Some(rx) = &self.browse_rx
            && let Ok(found) = rx.try_recv()
        {
            self.found = found;
            self.browse_rx = None;
        }
        if let Some(rx) = &self.pair_rx
            && let Ok(r) = rx.try_recv()
        {
            self.pair_rx = None;
            self.answer = Some(r.and_then(|ws| {
                self.settings.workstation = Some(ws);
                self.settings.mode = cl_lan::settings::Mode::Workstation;
                self.save()
            }));
        }
        self.answer.take()
    }

    fn pairing(&self) -> bool {
        self.pair_rx.is_some()
    }

    fn forget_host(&mut self) {
        self.settings.workstation = None;
        self.settings.mode = cl_lan::settings::Mode::Off;
        let _ = self.save();
    }

    fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}

/// The i18n key of a pairing refusal.
#[must_use]
pub fn refusal_key(r: cl_lan::pairing::PairRefusal) -> &'static str {
    use cl_lan::pairing::PairRefusal as P;
    match r {
        P::NoCode => "lan.pair.refused.no_code",
        P::WrongCode => "lan.pair.refused.wrong_code",
        P::Burned => "lan.pair.refused.burned",
        P::Busy => "lan.pair.refused.busy",
        P::Denied => "lan.pair.refused.denied",
        P::TimedOut => "lan.pair.refused.timed_out",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use cl_host::{Endpoint, HostSnapshot, HostStatus, Secret};

    fn snap(state: HostState, lan: Option<&str>) -> HostSnapshot {
        HostSnapshot {
            status: HostStatus {
                state,
                database_pid: Some(1),
                engine_pid: Some(2),
                restarts: cl_host::RestartCounts {
                    database: 0,
                    engine: 0,
                },
                generation: 3,
                notices: Vec::new(),
                engine_commit: Some("abc".into()),
                schema_version: None,
            },
            endpoint: Some(Endpoint {
                port: 4100,
                token: Secret::new("host-token"),
                generation: 3,
                lan_token: lan.map(Secret::new),
            }),
            generation: 3,
        }
    }

    #[test]
    fn upstream_carries_only_the_workstation_token() {
        let up = upstream_of(&snap(HostState::Running, Some("lan-token"))).expect("up");
        assert_eq!(up.lan_token.expose(), "lan-token");
        assert_eq!(up.port, 4100);
        assert_eq!(up.engine_commit, "abc");
        assert!(!format!("{up:?}").contains("host-token"));
        // Not host mode, or not running: no upstream.
        assert!(upstream_of(&snap(HostState::Running, None)).is_none());
        assert!(upstream_of(&snap(HostState::Starting, Some("lan-token"))).is_none());
    }
}
