//! The workstation side (Q-P33a, review S5, S12, Q-P33o, Q-P33p).
//!
//! cl-api stays loopback-only: it talks plain HTTP to a forwarder on
//! `127.0.0.1` with its own per-launch token. The forwarder checks the
//! head (no `Origin`, `Host` must be its own loopback address), the token
//! (constant time), then opens a **fresh** TLS connection to the host for
//! that one request, with no pooling and no retry. When the connection or
//! the TLS handshake fails before any request byte left, it answers
//! 503 `LAN_NOT_SENT`, so cl-api knows the save did not happen.
//!
//! A heartbeat (`GET /lan/hello` every 2 s) keeps the link state: host
//! version and engine commit must equal this app's (else "Update needed"),
//! the clock offset must be under 5 minutes, and the engine generation
//! always comes from the host.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use cl_api::{EndpointSource, EndpointState, EngineEndpoint};
use cl_core::Secret;
use http_body_util::{BodyExt, Empty, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioIo, TokioTimer};
use rustls::ClientConfig;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::PROTOCOL;
use crate::head::{self, HostRule, Prefixed};
use crate::host::{Hello, PairOk};
use crate::pairing::PairRefusal;
use crate::pinning::{Fingerprint, PairingCode, constant_time_eq};
use crate::relay::{self, Body, UpstreamError};
use crate::tls::{self, HostPin, Identity};

/// Heartbeat period.
pub const HEARTBEAT: Duration = Duration::from_secs(2);
/// Largest clock difference accepted (Q-P33o).
pub const MAX_CLOCK_OFFSET_MS: i64 = 5 * 60 * 1000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the pair request may wait (the host owner's click included).
const PAIR_TIMEOUT: Duration = Duration::from_secs(150);

/// The workstation's settings for one host.
#[derive(Clone, Debug)]
pub struct WorkstationConfig {
    /// The host's address and port.
    pub host: SocketAddr,
    /// The host's pinned fingerprint.
    pub host_fingerprint: Fingerprint,
    /// This app's version; must equal the host's.
    pub app_version: String,
    /// This app's bundled engine commit; must equal the host's.
    pub engine_commit: String,
}

/// The link to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// First heartbeat not answered yet.
    Connecting,
    /// Working.
    Connected {
        /// The host engine's generation.
        generation: u64,
        /// Host clock minus this clock, ms.
        clock_offset_ms: i64,
    },
    /// The host cannot be reached.
    HostDown,
    /// The host answers but its engine is restarting.
    HostEngineDown,
    /// The host is restoring or otherwise in maintenance.
    HostMaintenance,
    /// The host runs another app version or engine (Q-P33p).
    UpdateNeeded {
        /// The host's app version.
        host_app_version: String,
    },
    /// The clocks differ by more than 5 minutes (Q-P33o).
    ClockSkew {
        /// Host minus this clock, ms.
        offset_ms: i64,
    },
    /// The host no longer knows this computer (removed or regenerated).
    Unpaired,
}

impl LinkState {
    /// The stable key for i18n and the control channel.
    #[must_use]
    pub fn key(&self) -> &'static str {
        match self {
            LinkState::Connecting => "connecting",
            LinkState::Connected { .. } => "connected",
            LinkState::HostDown => "host-down",
            LinkState::HostEngineDown => "host-engine-down",
            LinkState::HostMaintenance => "host-maintenance",
            LinkState::UpdateNeeded { .. } => "update-needed",
            LinkState::ClockSkew { .. } => "clock-skew",
            LinkState::Unpaired => "unpaired",
        }
    }
}

/// Judges one paired hello against this workstation.
#[must_use]
pub fn judge_hello(h: &Hello, cfg: &WorkstationConfig, sent_ms: i64, recv_ms: i64) -> LinkState {
    if h.protocol != PROTOCOL || !h.paired {
        return LinkState::Unpaired;
    }
    let host_app = h.app_version.clone().unwrap_or_default();
    if host_app != cfg.app_version {
        return LinkState::UpdateNeeded {
            host_app_version: host_app,
        };
    }
    match h.engine.as_deref() {
        Some("ready") => {}
        Some("maintenance") => return LinkState::HostMaintenance,
        _ => return LinkState::HostEngineDown,
    }
    if h.engine_commit.as_deref() != Some(cfg.engine_commit.as_str()) {
        return LinkState::UpdateNeeded {
            host_app_version: host_app,
        };
    }
    let mid = sent_ms + (recv_ms - sent_ms) / 2;
    let offset = h.host_time_ms.unwrap_or(mid) - mid;
    if offset.abs() > MAX_CLOCK_OFFSET_MS {
        return LinkState::ClockSkew { offset_ms: offset };
    }
    LinkState::Connected {
        generation: h.generation.unwrap_or(0),
        clock_offset_ms: offset,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Opens TCP and TLS to the host.
async fn connect_tls(addr: SocketAddr, cfg: Arc<ClientConfig>) -> Option<TlsStream<TcpStream>> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .ok()?
        .ok()?;
    let _ = tcp.set_nodelay(true);
    let tls = tokio::time::timeout(
        HANDSHAKE_TIMEOUT,
        TlsConnector::from(cfg).connect(tls::server_name(), tcp),
    )
    .await
    .ok()?
    .ok()?;
    (tls.get_ref().1.alpn_protocol() == Some(tls::ALPN)).then_some(tls)
}

/// Errors of a direct call to the host (hello, pair).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// No connection: wrong address, host off, firewall, or pin mismatch.
    Unreachable,
    /// The host refused pairing.
    Pair(PairRefusal),
    /// Anything else (bad answer).
    Protocol,
}

async fn get_json<T: serde::de::DeserializeOwned>(
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
    path: &str,
    body: Option<String>,
    timeout: Duration,
) -> Result<(u16, Result<T, String>), CallError> {
    let fut = async {
        let io = connect_tls(addr, cfg).await.ok_or(CallError::Unreachable)?;
        let req = match body {
            Some(b) => Request::post(path)
                .header("host", "clientledger-host")
                .header("content-type", "application/json")
                .body(Full::new(Bytes::from(b)).map_err(|n| match n {}).boxed()),
            None => Request::get(path)
                .header("host", "clientledger-host")
                .body(Empty::<Bytes>::new().map_err(|n| match n {}).boxed()),
        }
        .map_err(|_| CallError::Protocol)?;
        let resp = relay::send_once(io, req)
            .await
            .map_err(|_| CallError::Unreachable)?;
        let status = resp.status().as_u16();
        let bytes = Limited::new(resp.into_body(), 64 * 1024)
            .collect()
            .await
            .map_err(|_| CallError::Protocol)?
            .to_bytes();
        let parsed = serde_json::from_slice::<T>(&bytes).map_err(|_| {
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/code")
                        .and_then(|c| c.as_str())
                        .map(String::from)
                })
                .unwrap_or_default()
        });
        Ok((status, parsed))
    };
    tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| CallError::Unreachable)?
}

/// Pairs this workstation with a host using the code shown there. Returns
/// the host's full fingerprint, which the caller saves as the pin.
///
/// # Errors
/// `Unreachable` (wrong address, or the certificate does not match the
/// code), `Pair(refusal)`, `Protocol`.
pub async fn pair(
    host: SocketAddr,
    code: PairingCode,
    device: &Identity,
    label: &str,
) -> Result<Fingerprint, CallError> {
    let (cfg, verifier) =
        tls::client_config(device, HostPin::Pairing(code.pin)).map_err(|_| CallError::Protocol)?;
    let body = serde_json::json!({ "secret": code.secret_text(), "label": label }).to_string();
    let (status, parsed) =
        get_json::<PairOk>(host, cfg, "/lan/pair", Some(body), PAIR_TIMEOUT).await?;
    match parsed {
        Ok(ok) if status == 200 => {
            let fp = Fingerprint::from_hex(&ok.host_fingerprint).ok_or(CallError::Protocol)?;
            // The fingerprint the host claims must be the certificate the
            // handshake actually verified.
            if verifier.seen() != Some(fp) || !code.matches_host(&fp) {
                return Err(CallError::Protocol);
            }
            Ok(fp)
        }
        Ok(_) => Err(CallError::Protocol),
        Err(code) => {
            Err(PairRefusal::from_code(&code).map_or(CallError::Protocol, CallError::Pair))
        }
    }
}

/// One hello with a pinned host.
///
/// # Errors
/// `Unreachable`, `Protocol`.
pub async fn hello(host: SocketAddr, pin: HostPin, device: &Identity) -> Result<Hello, CallError> {
    let (cfg, _) = tls::client_config(device, pin).map_err(|_| CallError::Protocol)?;
    let (_, parsed) =
        get_json::<Hello>(host, cfg, "/lan/hello", None, HANDSHAKE_TIMEOUT * 2).await?;
    parsed.map_err(|_| CallError::Protocol)
}

struct Shared {
    cfg: WorkstationConfig,
    tls: Arc<ClientConfig>,
    local_token: Secret,
    port: u16,
    state: watch::Sender<LinkState>,
    last_ok: Mutex<Option<Instant>>,
}

/// A running workstation link: the loopback forwarder plus the heartbeat.
/// Implements [`EndpointSource`] for cl-api.
pub struct Workstation {
    shared: Arc<Shared>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl std::fmt::Debug for Workstation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workstation")
            .field("port", &self.shared.port)
            .field("state", &*self.shared.state.borrow())
            .finish_non_exhaustive()
    }
}

impl Drop for Workstation {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

/// A fresh random token (64 hex characters).
///
/// # Errors
/// The OS random source failed.
pub fn random_token() -> Result<Secret, getrandom::Error> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b)?;
    let mut s = String::with_capacity(64);
    for x in b {
        s.push(char::from(b"0123456789abcdef"[usize::from(x >> 4)]));
        s.push(char::from(b"0123456789abcdef"[usize::from(x & 0xf)]));
    }
    Ok(Secret::new(s))
}

/// Errors starting the workstation link.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// TLS setup failed.
    #[error(transparent)]
    Tls(#[from] tls::TlsSetupError),
    /// The loopback forwarder could not listen.
    #[error("the local link could not start: {0}")]
    Bind(String),
    /// No random numbers.
    #[error("the system random source failed")]
    Random,
}

impl Workstation {
    /// Starts the forwarder on `127.0.0.1` and the heartbeat.
    ///
    /// # Errors
    /// `Tls`, `Bind`, `Random`.
    pub async fn start(
        cfg: WorkstationConfig,
        device: &Identity,
    ) -> Result<Workstation, StartError> {
        let (tls_cfg, _) = tls::client_config(device, HostPin::Full(cfg.host_fingerprint))?;
        let listener = TcpListener::bind(relay::loopback(0))
            .await
            .map_err(|e| StartError::Bind(e.kind().to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| StartError::Bind(e.kind().to_string()))?
            .port();
        let shared = Arc::new(Shared {
            cfg,
            tls: tls_cfg,
            local_token: random_token().map_err(|_| StartError::Random)?,
            port,
            state: watch::channel(LinkState::Connecting).0,
            last_ok: Mutex::new(None),
        });
        let s1 = shared.clone();
        let fwd = tokio::spawn(async move { forward_loop(s1, listener).await });
        let s2 = shared.clone();
        let hb = tokio::spawn(async move { heartbeat_loop(s2).await });
        Ok(Workstation {
            shared,
            tasks: vec![fwd, hb],
        })
    }

    /// The link state now.
    #[must_use]
    pub fn state(&self) -> LinkState {
        self.shared.state.borrow().clone()
    }

    /// A receiver for link changes (the UI's host-down overlay).
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<LinkState> {
        self.shared.state.subscribe()
    }

    /// The forwarder's port (tests).
    #[must_use]
    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// The forwarder's local token (tests and the in-process cl-api source
    /// only; never logged).
    #[must_use]
    pub fn local_token(&self) -> Secret {
        self.shared.local_token.clone()
    }

    /// Runs a heartbeat now ("Check again").
    pub async fn check_now(&self) -> LinkState {
        beat(&self.shared).await;
        self.state()
    }

    /// The cl-api endpoint source for this link.
    #[must_use]
    pub fn endpoint_source(&self) -> LanEndpoint {
        LanEndpoint {
            shared: self.shared.clone(),
        }
    }
}

/// cl-api's view of the LAN link.
#[derive(Clone)]
pub struct LanEndpoint {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for LanEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LanEndpoint(..)")
    }
}

/// Maps the link state to cl-api's endpoint state.
#[must_use]
pub fn endpoint_state(state: &LinkState, port: u16, token: &Secret) -> EndpointState {
    match state {
        LinkState::Connected {
            generation,
            clock_offset_ms,
        } => EndpointState::Ready(EngineEndpoint {
            port,
            token: token.clone(),
            generation: *generation,
            clock_offset_ms: *clock_offset_ms,
        }),
        LinkState::Connecting => EndpointState::Starting,
        LinkState::HostDown | LinkState::HostEngineDown | LinkState::HostMaintenance => {
            EndpointState::Down { permanent: false }
        }
        LinkState::UpdateNeeded { .. } | LinkState::ClockSkew { .. } | LinkState::Unpaired => {
            EndpointState::Down { permanent: true }
        }
    }
}

impl EndpointSource for LanEndpoint {
    fn current(&self) -> EndpointState {
        let s = self.shared.state.borrow().clone();
        endpoint_state(&s, self.shared.port, &self.shared.local_token)
    }
}

async fn heartbeat_loop(sh: Arc<Shared>) {
    let mut tick = tokio::time::interval(HEARTBEAT);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        beat(&sh).await;
    }
}

async fn beat(sh: &Shared) {
    let sent = now_ms();
    let r = get_json::<Hello>(
        sh.cfg.host,
        sh.tls.clone(),
        "/lan/hello",
        None,
        HANDSHAKE_TIMEOUT * 2,
    )
    .await;
    let recv = now_ms();
    let next = match r {
        Ok((_, Ok(h))) => {
            *sh.last_ok.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
            judge_hello(&h, &sh.cfg, sent, recv)
        }
        Ok((_, Err(_))) | Err(_) => LinkState::HostDown,
    };
    sh.state.send_if_modified(|cur| {
        if *cur == next {
            false
        } else {
            tracing::info!(link = next.key(), "LAN link changed");
            *cur = next;
            true
        }
    });
}

async fn forward_loop(sh: Arc<Shared>, listener: TcpListener) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            continue;
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        let sh = sh.clone();
        tokio::spawn(async move { forward_conn(sh, tcp).await });
    }
}

async fn forward_conn(sh: Arc<Shared>, mut tcp: TcpStream) {
    let _ = tcp.set_nodelay(true);
    let (buf, n) = match head::read_head(&mut tcp).await {
        Ok(x) => x,
        Err(r) => {
            head::write_refusal(&mut tcp, r.status(), r.code(), "The request was refused.").await;
            return;
        }
    };
    if let Err(r) = head::check_head(&buf[..n], HostRule::Loopback(sh.port)) {
        head::write_refusal(&mut tcp, r.status(), r.code(), "The request was refused.").await;
        return;
    }
    let io = TokioIo::new(Prefixed::new(buf, tcp));
    let sh2 = sh.clone();
    let svc = service_fn(move |req| {
        let sh = sh2.clone();
        async move { Ok::<_, std::convert::Infallible>(forward(sh, req).await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .keep_alive(false)
        .timer(TokioTimer::new())
        .header_read_timeout(head::HEAD_TIMEOUT)
        .serve_connection(io, svc)
        .await;
}

fn not_sent(message: &str) -> Response<Body> {
    relay::error_response(503, "LAN_NOT_SENT", message)
}

async fn forward(sh: Arc<Shared>, req: Request<Incoming>) -> Response<Body> {
    let token_ok = req
        .headers()
        .get("x-cl-token")
        .is_some_and(|v| constant_time_eq(v.as_bytes(), sh.local_token.expose().as_bytes()));
    if !token_ok {
        return relay::error_response(401, "DESKTOP_TOKEN_INVALID", "Not allowed.");
    }
    let state = sh.state.borrow().clone();
    match state {
        LinkState::Connected { .. } => {}
        LinkState::UpdateNeeded { .. } => {
            return relay::error_response(
                503,
                "LAN_UPDATE_NEEDED",
                "This computer needs the same ClientLedger version as the host.",
            );
        }
        _ => return not_sent("The host computer cannot be reached. Nothing was sent."),
    }
    let Some(io) = connect_tls(sh.cfg.host, sh.tls.clone()).await else {
        return not_sent("The host computer cannot be reached. Nothing was sent.");
    };
    let host = sh.cfg.host.to_string();
    let Ok(out) = relay::upstream_request(req, &host, &[]) else {
        return relay::error_response(400, "LAN_BAD_REQUEST", "The request was refused.");
    };
    match relay::send_once(io, out).await {
        Ok(resp) => relay::relay_response(resp),
        Err(UpstreamError::NotSent) => {
            not_sent("The host computer cannot be reached. Nothing was sent.")
        }
        Err(UpstreamError::Sent) => relay::error_response(
            502,
            "LAN_LINK_LOST",
            "The connection to the host dropped. Check whether the change was saved.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> WorkstationConfig {
        WorkstationConfig {
            host: "192.168.1.2:47615".parse().expect("addr"),
            host_fingerprint: Fingerprint::of_der(b"h"),
            app_version: "1.0.0".into(),
            engine_commit: "abc".into(),
        }
    }

    fn hello_ok() -> Hello {
        Hello {
            protocol: PROTOCOL,
            paired: true,
            fingerprint_prefix: String::new(),
            app_version: Some("1.0.0".into()),
            engine_commit: Some("abc".into()),
            generation: Some(7),
            host_time_ms: Some(10_000),
            engine: Some("ready".into()),
        }
    }

    #[test]
    fn hello_is_judged() {
        let c = cfg();
        assert_eq!(
            judge_hello(&hello_ok(), &c, 9_000, 9_200),
            LinkState::Connected {
                generation: 7,
                clock_offset_ms: 900
            }
        );
        let mut h = hello_ok();
        h.app_version = Some("1.0.1".into());
        assert!(matches!(
            judge_hello(&h, &c, 0, 0),
            LinkState::UpdateNeeded { .. }
        ));
        let mut h = hello_ok();
        h.engine_commit = Some("def".into());
        assert!(matches!(
            judge_hello(&h, &c, 0, 0),
            LinkState::UpdateNeeded { .. }
        ));
        let mut h = hello_ok();
        h.host_time_ms = Some(10_000 + MAX_CLOCK_OFFSET_MS + 1);
        assert!(matches!(
            judge_hello(&h, &c, 10_000, 10_000),
            LinkState::ClockSkew { .. }
        ));
        let mut h = hello_ok();
        h.engine = Some("maintenance".into());
        assert_eq!(judge_hello(&h, &c, 0, 0), LinkState::HostMaintenance);
        h.engine = Some("down".into());
        assert_eq!(judge_hello(&h, &c, 0, 0), LinkState::HostEngineDown);
        let mut h = hello_ok();
        h.paired = false;
        assert_eq!(judge_hello(&h, &c, 0, 0), LinkState::Unpaired);
    }

    #[test]
    fn endpoint_mapping() {
        let t = Secret::new("t");
        assert!(matches!(
            endpoint_state(
                &LinkState::Connected {
                    generation: 3,
                    clock_offset_ms: -5
                },
                9,
                &t
            ),
            EndpointState::Ready(EngineEndpoint {
                port: 9,
                generation: 3,
                clock_offset_ms: -5,
                ..
            })
        ));
        assert!(matches!(
            endpoint_state(&LinkState::HostDown, 9, &t),
            EndpointState::Down { permanent: false }
        ));
        assert!(matches!(
            endpoint_state(&LinkState::Unpaired, 9, &t),
            EndpointState::Down { permanent: true }
        ));
        assert!(matches!(
            endpoint_state(&LinkState::Connecting, 9, &t),
            EndpointState::Starting
        ));
    }
}
