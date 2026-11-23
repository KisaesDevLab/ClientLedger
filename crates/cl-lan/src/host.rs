//! The host proxy (Q-P33a, Q-P33z, review B1, S3, S5, M2–M4).
//!
//! Listens on one private interface, terminates TLS 1.3 with mutual
//! pinning, and relays each request to the engine on `127.0.0.1` over a
//! fresh connection. For every request it:
//! 1. checks the raw head and refuses the smuggling shapes ([`crate::head`]);
//! 2. checks the device registry against this connection's certificate;
//! 3. rebuilds the headers: hop-by-hop, `Connection`-named and every
//!    trust header (`X-CL-*`, `X-Forwarded-*`, `Forwarded`, `X-Real-IP`,
//!    `Host`) are dropped, then `X-CL-Token` (the **workstation** token, the
//!    only one this module ever holds), `X-Forwarded-For`, `X-CL-Device`
//!    and `Host` are set;
//! 4. streams both bodies without buffering or parsing them.
//!
//! One request per connection: hyper runs with keep-alive off, so nothing
//! after the first request on a connection is ever read.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cl_core::Secret;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::{Deserialize, Serialize};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;

use crate::PROTOCOL;
use crate::head::{self, HostRule, Prefixed};
use crate::pairing::{APPROVAL_WAIT, PairRefusal, Pairing};
use crate::pinning::Fingerprint;
use crate::policy::{self, Interfaces, Recheck, Refusal, net_key};
use crate::registry::Registry;
use crate::relay::{self, Body, UpstreamError};
use crate::tls::{self, Identity};

/// The default port (Q-P33d).
pub const DEFAULT_PORT: u16 = 47615;
/// Unpaired connections at once, in total (M2).
pub const MAX_UNPAIRED: usize = 8;
/// Unpaired connections at once from one address (M2).
pub const MAX_UNPAIRED_PER_IP: usize = 2;
/// TLS handshakes in progress at once, in total (review S1).
pub const MAX_HANDSHAKES: usize = 64;
/// Of those, at most this many from addresses with no paired device seen
/// lately: the rest is kept for the office's workstations, so unpaired
/// peers holding half-open handshakes can never starve a paired save.
pub const MAX_UNKNOWN_HANDSHAKES: usize = 32;
/// Handshakes at once from one unknown address (an IPv6 address counts
/// by its /64, so one machine can't spread over its many addresses).
pub const MAX_HANDSHAKES_PER_IP: usize = 4;
/// Handshakes at once from one address where a paired device was seen.
pub const MAX_KNOWN_HANDSHAKES_PER_IP: usize = 16;
/// How long an address stays "known" after a paired device's handshake.
pub const KNOWN_FOR: Duration = Duration::from_mins(30);
/// Connections (one request each) at once per paired device.
pub const MAX_DEVICE_CONNECTIONS: usize = 64;
/// Long-lived event streams at once per paired device.
pub const MAX_DEVICE_STREAMS: usize = 4;
/// How often the interface is re-checked (M4).
pub const RECHECK_EVERY: Duration = Duration::from_secs(30);
/// A TLS 1.3 handshake on a LAN takes milliseconds; a peer that has not
/// finished in 3 seconds gives its slot back (review S1).
const TLS_TIMEOUT: Duration = Duration::from_secs(3);
const PAIR_BODY_MAX: usize = 1024;

/// The engine as the proxy may use it: a loopback port and the
/// workstation-class token. Never the host's own launch token.
#[derive(Clone)]
pub struct Upstream {
    /// Engine port on `127.0.0.1`.
    pub port: u16,
    /// `CL_LAN_TOKEN`.
    pub lan_token: Secret,
    /// The engine process generation.
    pub generation: u64,
    /// The engine's commit (paired hello, version check).
    pub engine_commit: String,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream")
            .field("port", &self.port)
            .field("lan_token", &"***")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What the supervisor says about the engine right now.
#[derive(Clone, Debug)]
pub enum UpstreamState {
    /// Serving. (Maintenance is answered by the engine itself, U-293.)
    Ready(Upstream),
    /// The engine is starting, restarting or stopped.
    EngineDown,
}

/// Implemented by the host app over its supervisor (cl-lan never depends
/// on cl-host).
pub trait HostUpstream: Send + Sync + 'static {
    /// Cheap and non-blocking.
    fn state(&self) -> UpstreamState;

    /// The host app holds the engine's maintenance (said in the hello so
    /// workstations show the banner; QA-9).
    fn maintenance(&self) -> bool {
        false
    }
}

/// The proxy's settings.
#[derive(Clone, Debug)]
pub struct ProxyConfig {
    /// The interface name the owner chose.
    pub interface: String,
    /// Its address.
    pub ip: IpAddr,
    /// The port.
    pub port: u16,
    /// This app's version (paired hello, Q-P33p).
    pub app_version: String,
}

/// Why the proxy is not listening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stopped {
    /// The address is not a private office network.
    Refused(Refusal),
    /// The interface went away.
    InterfaceGone,
    /// Binding failed (port in use and the like); the OS error kind.
    Bind(String),
    /// Turned off.
    Shutdown,
}

/// The proxy's state for the setup screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProxyStatus {
    /// Listening.
    Listening(SocketAddr),
    /// Not listening.
    Stopped(Stopped),
}

/// Paired hello (`GET /lan/hello` from a paired device).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// [`PROTOCOL`].
    pub protocol: u32,
    /// False for an unpaired device (then only the prefix is filled).
    pub paired: bool,
    /// The host fingerprint's first 12 base32 characters.
    pub fingerprint_prefix: String,
    /// The host app's version.
    #[serde(default)]
    pub app_version: Option<String>,
    /// The host engine's commit.
    #[serde(default)]
    pub engine_commit: Option<String>,
    /// The host engine's generation.
    #[serde(default)]
    pub generation: Option<u64>,
    /// The host's clock, ms since the Unix epoch.
    #[serde(default)]
    pub host_time_ms: Option<i64>,
    /// `ready`, `maintenance` or `down`.
    #[serde(default)]
    pub engine: Option<String>,
}

#[derive(Deserialize)]
struct PairBody {
    secret: String,
    #[serde(default)]
    label: String,
}

/// The pair answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairOk {
    /// The host's full fingerprint (hex).
    pub host_fingerprint: String,
}

#[derive(Default)]
struct Counters {
    handshakes_total: usize,
    handshakes_unknown: usize,
    handshakes_by_ip: HashMap<IpAddr, usize>,
    /// Addresses (by [`net_key`]) where a paired device finished a
    /// handshake, and when.
    known: HashMap<IpAddr, std::time::Instant>,
    unpaired_total: usize,
    unpaired_by_ip: HashMap<IpAddr, usize>,
    device_conns: HashMap<Fingerprint, usize>,
    device_streams: HashMap<Fingerprint, usize>,
}

impl Counters {
    fn is_known(&self, key: &IpAddr) -> bool {
        self.known.get(key).is_some_and(|t| t.elapsed() < KNOWN_FOR)
    }

    /// A paired device finished a handshake from `ip`.
    fn mark_known(&mut self, ip: IpAddr) {
        let now = std::time::Instant::now();
        self.known.retain(|_, t| now.duration_since(*t) < KNOWN_FOR);
        self.known.insert(net_key(ip), now);
    }
}

enum Slot {
    /// A handshake from this address ([`net_key`]); `known` is filled in
    /// when the slot is taken.
    Handshake {
        key: IpAddr,
        known: bool,
    },
    Unpaired(IpAddr),
    DeviceConn(Fingerprint),
    DeviceStream(Fingerprint),
}

impl Slot {
    fn handshake(ip: IpAddr) -> Slot {
        Slot::Handshake {
            key: net_key(ip),
            known: false,
        }
    }
}

struct SlotGuard {
    counters: Arc<Mutex<Counters>>,
    slot: Slot,
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        let mut c = self.counters.lock().unwrap_or_else(PoisonError::into_inner);
        match &self.slot {
            Slot::Handshake { key, known } => {
                c.handshakes_total = c.handshakes_total.saturating_sub(1);
                if !known {
                    c.handshakes_unknown = c.handshakes_unknown.saturating_sub(1);
                }
                if let Some(n) = c.handshakes_by_ip.get_mut(key) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        c.handshakes_by_ip.remove(key);
                    }
                }
            }
            Slot::Unpaired(ip) => {
                c.unpaired_total = c.unpaired_total.saturating_sub(1);
                if let Some(n) = c.unpaired_by_ip.get_mut(ip) {
                    *n = n.saturating_sub(1);
                    if *n == 0 {
                        c.unpaired_by_ip.remove(ip);
                    }
                }
            }
            Slot::DeviceConn(fp) => dec(&mut c.device_conns, fp),
            Slot::DeviceStream(fp) => dec(&mut c.device_streams, fp),
        }
    }
}

fn dec(m: &mut HashMap<Fingerprint, usize>, fp: &Fingerprint) {
    if let Some(n) = m.get_mut(fp) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            m.remove(fp);
        }
    }
}

fn take_slot(counters: &Arc<Mutex<Counters>>, mut slot: Slot) -> Option<SlotGuard> {
    let mut c = counters.lock().unwrap_or_else(PoisonError::into_inner);
    let ok = match &mut slot {
        Slot::Handshake { key, known } => {
            *known = c.is_known(key);
            let per = c.handshakes_by_ip.get(key).copied().unwrap_or(0);
            let (per_max, pool_full) = if *known {
                (MAX_KNOWN_HANDSHAKES_PER_IP, false)
            } else {
                (
                    MAX_HANDSHAKES_PER_IP,
                    c.handshakes_unknown >= MAX_UNKNOWN_HANDSHAKES,
                )
            };
            if c.handshakes_total >= MAX_HANDSHAKES || pool_full || per >= per_max {
                false
            } else {
                c.handshakes_total += 1;
                if !*known {
                    c.handshakes_unknown += 1;
                }
                *c.handshakes_by_ip.entry(*key).or_default() += 1;
                true
            }
        }
        Slot::Unpaired(ip) => {
            let per = c.unpaired_by_ip.get(ip).copied().unwrap_or(0);
            if c.unpaired_total >= MAX_UNPAIRED || per >= MAX_UNPAIRED_PER_IP {
                false
            } else {
                c.unpaired_total += 1;
                *c.unpaired_by_ip.entry(*ip).or_default() += 1;
                true
            }
        }
        Slot::DeviceConn(fp) => bump(&mut c.device_conns, fp, MAX_DEVICE_CONNECTIONS),
        Slot::DeviceStream(fp) => bump(&mut c.device_streams, fp, MAX_DEVICE_STREAMS),
    };
    drop(c);
    ok.then(|| SlotGuard {
        counters: counters.clone(),
        slot,
    })
}

fn bump(m: &mut HashMap<Fingerprint, usize>, fp: &Fingerprint, max: usize) -> bool {
    let n = m.entry(*fp).or_default();
    if *n >= max {
        return false;
    }
    *n += 1;
    true
}

struct Shared {
    cfg: ProxyConfig,
    identity: Identity,
    registry: Arc<Registry>,
    pairing: Arc<Pairing>,
    upstream: Arc<dyn HostUpstream>,
    counters: Arc<Mutex<Counters>>,
    acceptor: TlsAcceptor,
}

/// A running proxy. Dropping it stops the listener (open streams end when
/// their peers go away or on Remove).
pub struct HostProxy {
    status: watch::Receiver<ProxyStatus>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for HostProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostProxy")
            .field("status", &*self.status.borrow())
            .finish_non_exhaustive()
    }
}

/// Checks the listen address against the policy (Q-P33c). The
/// `lan-test-listener` build also allows loopback.
///
/// # Errors
/// The refusal.
pub fn check_listen(ip: IpAddr) -> Result<(), Refusal> {
    let ip = policy::canonical(ip);
    if cfg!(feature = "lan-test-listener") && ip.is_loopback() {
        return Ok(());
    }
    policy::check_private(ip)
}

impl HostProxy {
    /// Starts the proxy on a tokio runtime (the caller's).
    ///
    /// # Errors
    /// `Tls` when the identity is unusable. A refused or unbindable address
    /// is not an error: the proxy reports [`ProxyStatus::Stopped`].
    pub async fn start(
        cfg: ProxyConfig,
        identity: Identity,
        registry: Arc<Registry>,
        pairing: Arc<Pairing>,
        upstream: Arc<dyn HostUpstream>,
        interfaces: Arc<dyn Interfaces>,
    ) -> Result<HostProxy, tls::TlsSetupError> {
        let acceptor = TlsAcceptor::from(tls::server_config(&identity)?);
        let shared = Arc::new(Shared {
            cfg,
            identity,
            registry,
            pairing,
            upstream,
            counters: Arc::new(Mutex::new(Counters::default())),
            acceptor,
        });
        let first = bind(&shared.cfg.ip, shared.cfg.port).await;
        let (status_tx, status_rx) = watch::channel(match &first {
            Ok(l) => ProxyStatus::Listening(
                l.local_addr()
                    .unwrap_or(SocketAddr::new(shared.cfg.ip, shared.cfg.port)),
            ),
            Err(s) => ProxyStatus::Stopped(s.clone()),
        });
        let (stop_tx, stop_rx) = watch::channel(false);
        let task = tokio::spawn(run(shared, first.ok(), status_tx, stop_rx, interfaces));
        Ok(HostProxy {
            status: status_rx,
            stop: stop_tx,
            task,
        })
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> ProxyStatus {
        self.status.borrow().clone()
    }

    /// A receiver of status changes (re-announce on rebind, withdraw on stop).
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<ProxyStatus> {
        self.status.clone()
    }

    /// Waits for the status to change.
    pub async fn changed(&mut self) -> ProxyStatus {
        let _ = self.status.changed().await;
        self.status()
    }

    /// Stops listening and waits for the accept loop to end.
    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

async fn bind(ip: &IpAddr, port: u16) -> Result<TcpListener, Stopped> {
    if let Err(r) = check_listen(*ip) {
        return Err(Stopped::Refused(r));
    }
    TcpListener::bind(SocketAddr::new(*ip, port))
        .await
        .map_err(|e| Stopped::Bind(e.kind().to_string()))
}

async fn run(
    shared: Arc<Shared>,
    mut listener: Option<TcpListener>,
    status: watch::Sender<ProxyStatus>,
    mut stop: watch::Receiver<bool>,
    interfaces: Arc<dyn Interfaces>,
) {
    let mut bound_ip = shared.cfg.ip;
    let mut tick = tokio::time::interval(RECHECK_EVERY);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = stop.changed() => {
                let _ = status.send(ProxyStatus::Stopped(Stopped::Shutdown));
                return;
            }
            _ = tick.tick() => {
                if listener.is_none() { continue; }
                if cfg!(feature = "lan-test-listener") && bound_ip.is_loopback() { continue; }
                match policy::recheck(&shared.cfg.interface, bound_ip, &interfaces.list()) {
                    Recheck::Keep => {}
                    Recheck::Rebind(ip) => {
                        listener = None;
                        match bind(&ip, shared.cfg.port).await {
                            Ok(l) => {
                                bound_ip = ip;
                                if let Ok(a) = l.local_addr() {
                                    let _ = status.send(ProxyStatus::Listening(a));
                                }
                                listener = Some(l);
                            }
                            Err(s) => { let _ = status.send(ProxyStatus::Stopped(s)); }
                        }
                    }
                    Recheck::Stop(r) => {
                        listener = None;
                        let why = r.map_or(Stopped::InterfaceGone, Stopped::Refused);
                        tracing::warn!(reason = ?why, "LAN listener stopped: interface changed");
                        let _ = status.send(ProxyStatus::Stopped(why));
                    }
                }
            }
            accepted = accept(listener.as_ref()) => {
                let Ok((tcp, peer)) = accepted else { continue; };
                let local = tcp.local_addr().map_or(bound_ip, |a| a.ip());
                let peer_ip = policy::canonical(peer.ip());
                if policy::check_peer(local, peer_ip).is_err() {
                    drop(tcp);
                    continue;
                }
                let sh = shared.clone();
                tokio::spawn(async move { serve_conn(sh, tcp, peer_ip).await; });
            }
        }
    }
}

async fn accept(l: Option<&TcpListener>) -> std::io::Result<(TcpStream, SocketAddr)> {
    match l {
        Some(l) => l.accept().await,
        None => std::future::pending().await,
    }
}

async fn serve_conn(sh: Arc<Shared>, tcp: TcpStream, peer_ip: IpAddr) {
    let _ = tcp.set_nodelay(true);
    // Capped before the handshake: nobody is known yet (review M2), but an
    // address where a paired device was seen lately draws on its own
    // share (review S1).
    let Some(handshake) = take_slot(&sh.counters, Slot::handshake(peer_ip)) else {
        return;
    };
    let Ok(Ok(mut tls)) = tokio::time::timeout(TLS_TIMEOUT, sh.acceptor.accept(tcp)).await else {
        return;
    };
    drop(handshake);
    let (_, conn) = tls.get_ref();
    if conn.alpn_protocol() != Some(tls::ALPN) {
        return;
    }
    let Some(fp) = tls::peer_fingerprint(conn.peer_certificates()) else {
        return;
    };
    let paired = sh.registry.contains(&fp);
    if paired {
        sh.counters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .mark_known(peer_ip);
    }
    let _slot = if paired {
        let Some(g) = take_slot(&sh.counters, Slot::DeviceConn(fp)) else {
            head::write_refusal(
                &mut tls,
                429,
                "LAN_TOO_MANY",
                "Too many connections from this workstation.",
            )
            .await;
            return;
        };
        g
    } else {
        let Some(g) = take_slot(&sh.counters, Slot::Unpaired(net_key(peer_ip))) else {
            return;
        };
        g
    };
    let (buf, head_len) = match head::read_head(&mut tls).await {
        Ok(x) => x,
        Err(r) => {
            head::write_refusal(&mut tls, r.status(), r.code(), "The request was refused.").await;
            return;
        }
    };
    let info = match head::check_head(&buf[..head_len], HostRule::Any) {
        Ok(i) => i,
        Err(r) => {
            tracing::debug!(code = r.code(), "LAN request refused");
            head::write_refusal(&mut tls, r.status(), r.code(), "The request was refused.").await;
            return;
        }
    };
    tracing::debug!(method = %info.method, path = %redact_path(&info.path), "LAN request");
    let stream_slot = if info.wants_stream && paired {
        let Some(g) = take_slot(&sh.counters, Slot::DeviceStream(fp)) else {
            head::write_refusal(
                &mut tls,
                429,
                "LAN_TOO_MANY_STREAMS",
                "Too many live views open on this workstation.",
            )
            .await;
            return;
        };
        Some(g)
    } else {
        None
    };
    let mut removals = sh.registry.removals();
    let io = TokioIo::new(Prefixed::new(buf, tls));
    let sh2 = sh.clone();
    let svc = service_fn(move |req| {
        let sh = sh2.clone();
        async move { Ok::<_, std::convert::Infallible>(handle(sh, req, fp, peer_ip).await) }
    });
    let conn = hyper::server::conn::http1::Builder::new()
        .keep_alive(false)
        .timer(TokioTimer::new())
        .header_read_timeout(head::HEAD_TIMEOUT)
        .serve_connection(io, svc);
    tokio::pin!(conn);
    loop {
        tokio::select! {
            _ = &mut conn => break,
            changed = removals.changed() => {
                if changed.is_err() || (paired && !sh.registry.contains(&fp)) {
                    // Removed while connected: close at once (S3).
                    break;
                }
            }
        }
    }
    drop(stream_slot);
}

async fn handle(
    sh: Arc<Shared>,
    req: Request<Incoming>,
    fp: Fingerprint,
    peer: IpAddr,
) -> Response<Body> {
    let path = req.uri().path().to_string();
    let paired = sh.registry.contains(&fp);
    match (req.method(), path.as_str()) {
        (&Method::GET, "/lan/hello") => return hello(&sh, paired),
        (&Method::POST, "/lan/pair") => return pair(&sh, req, fp, peer, paired).await,
        _ => {}
    }
    if !paired {
        return relay::error_response(
            403,
            "LAN_NOT_PAIRED",
            "This computer is not set up to use the host. Pair it again.",
        );
    }
    if !forwardable(req.method(), &path) {
        return relay::error_response(404, "NOT_FOUND", "Not found.");
    }
    let up = match sh.upstream.state() {
        UpstreamState::Ready(u) => u,
        UpstreamState::EngineDown => {
            return relay::error_response(
                503,
                "HOST_ENGINE_DOWN",
                "The host's books are restarting. Try again shortly.",
            );
        }
    };
    let host = relay::loopback(up.port).to_string();
    let device = fp.hex();
    let peer_s = peer.to_string();
    let Ok(out) = relay::upstream_request(
        req,
        &host,
        &[
            ("x-cl-token", up.lan_token.expose()),
            ("x-forwarded-for", &peer_s),
            ("x-cl-device", &device),
        ],
    ) else {
        return relay::error_response(400, "LAN_BAD_REQUEST", "The request was refused.");
    };
    let Ok(tcp) = TcpStream::connect(relay::loopback(up.port)).await else {
        return relay::error_response(
            503,
            "HOST_ENGINE_DOWN",
            "The host's books are restarting. Try again shortly.",
        );
    };
    let _ = tcp.set_nodelay(true);
    match relay::send_once(tcp, out).await {
        Ok(resp) => relay::relay_response(resp),
        Err(UpstreamError::NotSent) => relay::error_response(
            503,
            "HOST_ENGINE_DOWN",
            "The host's books are restarting. Try again shortly.",
        ),
        Err(UpstreamError::Sent) => {
            relay::error_response(502, "LAN_UPSTREAM_FAILED", "The host stopped answering.")
        }
    }
}

/// Paths the proxy relays (Q-P33ae): the engine API, plus the probes
/// cl-api reads (`GET /health`, `/ping`, `/openapi.json`). Anything else
/// is answered 404 here and never reaches the engine.
#[must_use]
pub fn forwardable(method: &Method, path: &str) -> bool {
    path.starts_with("/api/")
        || (method == Method::GET && matches!(path, "/health" | "/ping" | "/openapi.json"))
}

/// A request path fit for the log (review M8): the query is dropped and
/// every segment that could name a record (a UUID, a number, a long hex
/// or token-like run) becomes `:id`, so even debug logs hold no record
/// ids. `/api/v1/transactions/3f2a…/void` → `/api/v1/transactions/:id/void`.
#[must_use]
pub fn redact_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let looks_like_id = |seg: &str| {
        let digits = seg.bytes().filter(u8::is_ascii_digit).count();
        // Route words are lowercase letters and dashes (`bank-feeds`,
        // `v1`); anything with several digits, or long, is data.
        digits >= 2
            || seg.len() > 24
            || !seg
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    };
    path.split('/')
        .map(|seg| {
            if !seg.is_empty() && looks_like_id(seg) {
                ":id"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn hello(sh: &Shared, paired: bool) -> Response<Body> {
    let prefix = sh.identity.fingerprint().prefix();
    let h = if paired {
        let (engine, generation, commit) = match sh.upstream.state() {
            UpstreamState::Ready(u) if sh.upstream.maintenance() => {
                ("maintenance", Some(u.generation), Some(u.engine_commit))
            }
            UpstreamState::Ready(u) => ("ready", Some(u.generation), Some(u.engine_commit)),
            UpstreamState::EngineDown => ("down", None, None),
        };
        Hello {
            protocol: PROTOCOL,
            paired: true,
            fingerprint_prefix: prefix,
            app_version: Some(sh.cfg.app_version.clone()),
            engine_commit: commit,
            generation,
            host_time_ms: Some(now_ms()),
            engine: Some(engine.to_string()),
        }
    } else {
        Hello {
            protocol: PROTOCOL,
            paired: false,
            fingerprint_prefix: prefix,
            app_version: None,
            engine_commit: None,
            generation: None,
            host_time_ms: None,
            engine: None,
        }
    };
    relay::json_response(200, serde_json::to_string(&h).unwrap_or_default())
}

fn pair_refused(r: PairRefusal) -> Response<Body> {
    let (status, msg) = match r {
        PairRefusal::NoCode => (
            403,
            "No pairing code is showing on the host. Make a new code there.",
        ),
        PairRefusal::WrongCode => (403, "That code is not right. Check it and try again."),
        PairRefusal::Burned => (403, "Too many wrong tries. Make a new code on the host."),
        PairRefusal::Busy => (
            409,
            "Another computer is waiting to be allowed. Try again in a minute.",
        ),
        PairRefusal::Denied => (403, "The host did not allow this computer."),
        PairRefusal::TimedOut => (408, "Nobody answered on the host. Try again."),
        PairRefusal::Throttled => (
            429,
            "Too many wrong tries from this computer. Make a new code on the host.",
        ),
    };
    relay::error_response(status, r.code(), msg)
}

async fn pair(
    sh: &Shared,
    req: Request<Incoming>,
    fp: Fingerprint,
    peer: IpAddr,
    paired: bool,
) -> Response<Body> {
    let ok = || {
        relay::json_response(
            200,
            serde_json::to_string(&PairOk {
                host_fingerprint: sh.identity.fingerprint().hex(),
            })
            .unwrap_or_default(),
        )
    };
    let body: Bytes = match Limited::new(req.into_body(), PAIR_BODY_MAX).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return relay::error_response(413, "LAN_BAD_REQUEST", "The request was refused."),
    };
    let Ok(pb) = serde_json::from_slice::<PairBody>(&body) else {
        return relay::error_response(400, "LAN_BAD_REQUEST", "The request was refused.");
    };
    if paired {
        return ok();
    }
    if let Err(r) = sh.pairing.try_secret(peer, &pb.secret) {
        tracing::info!(code = r.code(), "LAN pairing refused");
        return pair_refused(r);
    }
    // The device is the TLS client certificate of this connection (M2).
    let label = crate::registry::clean_label(&pb.label);
    let (id, rx) = match sh.pairing.request(fp, label.clone(), peer) {
        Ok(x) => x,
        Err(r) => return pair_refused(r),
    };
    match tokio::time::timeout(APPROVAL_WAIT, rx).await {
        Ok(Ok(true)) => {
            let at = chrono_like_now();
            if sh.registry.add(fp, &label, at).is_err() {
                return relay::error_response(
                    500,
                    "LAN_REGISTRY_WRITE",
                    "The host could not save the workstation list.",
                );
            }
            tracing::info!("LAN workstation paired");
            ok()
        }
        Ok(Ok(false) | Err(_)) => pair_refused(PairRefusal::Denied),
        Err(_) => {
            sh.pairing.forget(id);
            pair_refused(PairRefusal::TimedOut)
        }
    }
}

/// RFC 3339 UTC seconds, without a date crate.
fn chrono_like_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_api_and_probes_are_forwarded() {
        assert!(forwardable(&Method::POST, "/api/v1/transactions"));
        assert!(forwardable(&Method::GET, "/health"));
        assert!(forwardable(&Method::GET, "/openapi.json"));
        assert!(!forwardable(&Method::POST, "/health"));
        assert!(!forwardable(&Method::GET, "/admin"));
        assert!(!forwardable(&Method::GET, "/"));
    }

    #[test]
    fn logged_paths_hold_no_record_ids() {
        for (raw, want) in [
            (
                "/api/v1/transactions/3f2a9c1e-55b0-4c1d-9a7e-0b1c2d3e4f50/void",
                "/api/v1/transactions/:id/void",
            ),
            ("/api/v1/contacts/12345?q=Smith", "/api/v1/contacts/:id"),
            ("/api/v1/bank-feeds/items", "/api/v1/bank-feeds/items"),
            ("/api/v1/tags/AbCdEf/merge", "/api/v1/tags/:id/merge"),
            ("/health", "/health"),
            (
                "/api/v1/desktop/sessions/0123456789abcdef0123456789abcdef",
                "/api/v1/desktop/sessions/:id",
            ),
        ] {
            assert_eq!(redact_path(raw), want, "{raw}");
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_411), (2025, 11, 19));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn listen_policy() {
        assert_eq!(
            check_listen("0.0.0.0".parse().expect("ip")),
            Err(Refusal::Unspecified)
        );
        assert_eq!(
            check_listen("8.8.8.8".parse().expect("ip")),
            Err(Refusal::Public)
        );
        assert!(check_listen("192.168.0.2".parse().expect("ip")).is_ok());
        // Tests build with lan-test-listener (dev-dependency).
        assert!(check_listen("127.0.0.1".parse().expect("ip")).is_ok());
    }

    #[test]
    fn slots_are_capped_and_released() {
        let c = Arc::new(Mutex::new(Counters::default()));
        let ip: IpAddr = "192.168.1.2".parse().expect("ip");
        let a = take_slot(&c, Slot::Unpaired(ip)).expect("1");
        let b = take_slot(&c, Slot::Unpaired(ip)).expect("2");
        assert!(take_slot(&c, Slot::Unpaired(ip)).is_none());
        drop(a);
        assert!(take_slot(&c, Slot::Unpaired(ip)).is_some());
        drop(b);
        let mut held = Vec::new();
        for i in 0..MAX_UNPAIRED {
            let ip: IpAddr = format!("10.0.0.{}", i + 1).parse().expect("ip");
            held.push(take_slot(&c, Slot::Unpaired(ip)).expect("slot"));
        }
        assert!(take_slot(&c, Slot::Unpaired("10.0.1.1".parse().expect("ip"))).is_none());
        let fp = Fingerprint::of_der(b"d");
        let s: Vec<_> = (0..MAX_DEVICE_STREAMS)
            .map(|_| take_slot(&c, Slot::DeviceStream(fp)).expect("stream"))
            .collect();
        assert!(take_slot(&c, Slot::DeviceStream(fp)).is_none());
        drop(s);
        assert!(take_slot(&c, Slot::DeviceStream(fp)).is_some());
        let hip: IpAddr = "192.168.9.9".parse().expect("ip");
        let hs: Vec<_> = (0..MAX_HANDSHAKES_PER_IP)
            .map(|_| take_slot(&c, Slot::handshake(hip)).expect("handshake"))
            .collect();
        assert!(take_slot(&c, Slot::handshake(hip)).is_none());
        drop(hs);
        assert!(take_slot(&c, Slot::handshake(hip)).is_some());
    }

    /// Review S1: 64 stalled handshakes from unpaired peers (16 addresses,
    /// 4 each, over and over) never take the share kept for the addresses
    /// where paired workstations were seen; their saves still get in.
    #[test]
    fn stalled_unpaired_handshakes_never_starve_a_paired_workstation() {
        let c = Arc::new(Mutex::new(Counters::default()));
        let desk: IpAddr = "192.168.1.20".parse().expect("ip");
        c.lock().expect("lock").mark_known(desk);
        let mut stalled = Vec::new();
        for i in 0..16 {
            let ip: IpAddr = format!("192.168.1.{}", 100 + i).parse().expect("ip");
            for _ in 0..4 {
                if let Some(g) = take_slot(&c, Slot::handshake(ip)) {
                    stalled.push(g);
                }
            }
            // A fifth from the same address is refused at once.
            assert!(take_slot(&c, Slot::handshake(ip)).is_none());
        }
        assert_eq!(stalled.len(), MAX_UNKNOWN_HANDSHAKES, "the unknown share");
        // Another unknown address: refused (its share is full).
        assert!(take_slot(&c, Slot::handshake("192.168.1.250".parse().expect("ip"))).is_none());
        // The paired workstation: its handshakes still get slots.
        let desk_slots: Vec<_> = (0..MAX_KNOWN_HANDSHAKES_PER_IP)
            .map(|_| take_slot(&c, Slot::handshake(desk)).expect("a paired handshake"))
            .collect();
        drop(desk_slots);
        drop(stalled);
        let c = c.lock().expect("lock");
        assert_eq!((c.handshakes_total, c.handshakes_unknown), (0, 0));
        assert!(c.handshakes_by_ip.is_empty());
    }

    /// One machine's IPv6 addresses count as one (by /64).
    #[test]
    fn ipv6_addresses_count_by_their_slash_64() {
        let c = Arc::new(Mutex::new(Counters::default()));
        let held: Vec<_> = (1..=20)
            .filter_map(|i| {
                let ip: IpAddr = format!("fd00:1:2:3::{i:x}").parse().expect("ip");
                take_slot(&c, Slot::handshake(ip))
            })
            .collect();
        assert_eq!(held.len(), MAX_HANDSHAKES_PER_IP);
        assert!(take_slot(&c, Slot::handshake("fd00:1:2:4::1".parse().expect("ip"))).is_some());
    }
}
