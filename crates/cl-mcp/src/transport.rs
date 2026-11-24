//! The host's loopback listener and the frames the relay sends it (plan D4).
//!
//! On this computer the relay reaches the app through a listener on
//! `127.0.0.1` (never another interface; only `cl-lan` binds those). The
//! host publishes its port and a random per-run key in an owner-only
//! endpoint file (the control-file rules of `cl-automation`, written by the
//! host; this module never touches the file). Every request is one line on
//! its own connection, answered with one line:
//!
//! - the endpoint key proves the caller can read this user's endpoint file;
//! - the connection id and its token (from this computer's keychain,
//!   Q-A11b) say which connection it is; the [`Authenticate`] the host
//!   gives checks them, and on a LAN host the pinned device too (S12);
//! - `kind` is `probe` (is the app there, and what does it show) or `mcp`
//!   (one MCP message for [`protocol::handle`]).
//!
//! Keys and tokens are compared in constant time and never logged.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::policy;
use crate::protocol::{self, Bridge};
use crate::relay::{Seen, Upstream, UpstreamError};

/// The endpoint file's name in the app's run folder.
pub const ENDPOINT_FILE: &str = "mcp-endpoint.json";
/// The frame format.
pub const FRAME_VERSION: u64 = 1;
/// A request line may be at most this long (the payload limit plus room).
pub const MAX_REQUEST: usize = protocol::MAX_PAYLOAD + 64 * 1024;
/// An answer line may be at most this long (the output limit plus room).
pub const MAX_ANSWER: usize = 2 * protocol::MAX_OUTPUT + 64 * 1024;
/// Requests handled at once.
pub const MAX_CONNECTIONS: usize = 16;
/// How long a caller has to send its line.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the relay waits to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the relay waits for an answer (a call may wait on the engine).
const ANSWER_TIMEOUT: Duration = Duration::from_secs(180);

/// Compare two secrets without stopping at the first difference.
#[must_use]
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

/// A connection id as the keychain and the CLI accept it: lowercase
/// letters, digits and dashes, 1–40 characters (an engine UUID fits; the
/// keychain name `mcp-<id>` stays within its 48).
#[must_use]
pub fn valid_connection_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The keychain name of a connection's token on this computer.
#[must_use]
pub fn token_name(connection: &str) -> String {
    format!("mcp-{connection}")
}

/// Checks a connection's token (and, on a LAN host, its device).
pub trait Authenticate: Send + Sync {
    /// `Ok` when `token` is `connection`'s and, when `device` is given, the
    /// connection is bound to that pinned device.
    ///
    /// # Errors
    /// Plain words for the agent (revoked, wrong token, another device).
    fn check(&self, connection: &str, token: &str, device: Option<&str>) -> Result<(), String>;
}

/// What a frame asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Is the app there, and what does it show.
    Probe,
    /// One MCP message.
    Mcp,
}

/// Build a request line.
#[must_use]
pub fn request_line(
    key: &str,
    connection: &str,
    token: &str,
    kind: FrameKind,
    message: Option<&Value>,
) -> String {
    let kind = match kind {
        FrameKind::Probe => "probe",
        FrameKind::Mcp => "mcp",
    };
    let mut line = json!({"v": FRAME_VERSION, "key": key, "connection": connection,
        "token": token, "kind": kind, "message": message})
    .to_string();
    line.push('\n');
    line
}

/// The refusal for a caller who does not hold the endpoint key.
pub const WRONG_KEY: &str = "ClientLedger refused the connection. Close the AI app and open it \
     again; if it keeps happening, connect again in ClientLedger: Settings \u{203a} Claude Desktop.";

/// What the agent is told when a message is too large to send.
pub const TOO_LARGE: &str =
    "The request is too large (at most 256 KB). Send fewer lines at a time.";

/// Answer one request frame (the part shared by the loopback listener and
/// the LAN host's `/cl-mcp`, which passes the pinned `device` and no key).
#[must_use]
pub fn answer_frame(
    frame: &Value,
    key: Option<&str>,
    device: Option<&str>,
    instance: &str,
    auth: &dyn Authenticate,
    bridge: &dyn Bridge,
) -> Value {
    let refused = |why: &str| json!({"v": FRAME_VERSION, "ok": false, "refused": why});
    if frame["v"].as_u64() != Some(FRAME_VERSION) {
        return refused("This ClientLedger does not speak that relay's language; update it.");
    }
    if let Some(key) = key
        && !tokens_equal(frame["key"].as_str().unwrap_or(""), key)
    {
        return refused(WRONG_KEY);
    }
    let connection = frame["connection"].as_str().unwrap_or("");
    if !valid_connection_id(connection) {
        return refused(WRONG_KEY);
    }
    if let Err(why) = auth.check(connection, frame["token"].as_str().unwrap_or(""), device) {
        return refused(&why);
    }
    match frame["kind"].as_str() {
        // Only a probe needs what the connection is shown (empty: no
        // company open).
        Some("probe") => {
            let fingerprint = bridge
                .policy(connection)
                .map(|p| policy::fingerprint(&p))
                .unwrap_or_default();
            json!({"v": FRAME_VERSION, "ok": true, "instance": instance,
                "fingerprint": fingerprint, "answer": null})
        }
        Some("mcp") => {
            let answer =
                protocol::handle(bridge, connection, &frame["message"]).unwrap_or(Value::Null);
            json!({"v": FRAME_VERSION, "ok": true, "instance": instance, "answer": answer})
        }
        _ => refused("The relay sent something ClientLedger does not know."),
    }
}

/// The host's loopback listener. Dropping it stops it.
pub struct Listener {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Listener")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl Listener {
    /// Listen on `127.0.0.1` on a free port. `key` is the endpoint file's
    /// key; `instance` names this run of the app.
    ///
    /// # Errors
    /// The port could not be opened.
    pub fn start(
        key: String,
        instance: String,
        auth: Arc<dyn Authenticate>,
        bridge: Arc<dyn Bridge>,
    ) -> std::io::Result<Listener> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let addr = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let shared = Arc::new(Shared {
            key,
            instance,
            auth,
            bridge,
            busy: AtomicUsize::new(0),
        });
        std::thread::Builder::new()
            .name("mcp-listener".into())
            .spawn(move || accept_loop(&listener, &flag, &shared))?;
        Ok(Listener { addr, stop })
    }

    /// The port and address to publish in the endpoint file.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

struct Shared {
    key: String,
    instance: String,
    auth: Arc<dyn Authenticate>,
    bridge: Arc<dyn Bridge>,
    busy: AtomicUsize,
}

fn accept_loop(listener: &TcpListener, stop: &AtomicBool, shared: &Arc<Shared>) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        // Loopback only; anything else is dropped unread.
        if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
            continue;
        }
        if shared.busy.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.busy.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let shared = Arc::clone(shared);
        let spawned = std::thread::Builder::new()
            .name("mcp-request".into())
            .spawn(move || {
                serve(&stream, &shared);
                shared.busy.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            tracing::warn!("an MCP request could not be served (no thread)");
        }
    }
}

fn serve(stream: &TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut reader = BufReader::new(stream.take(MAX_REQUEST as u64 + 1));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.len() > MAX_REQUEST || !line.ends_with('\n') {
        return;
    }
    let Ok(frame) = serde_json::from_str::<Value>(&line) else {
        return;
    };
    let reply = answer_frame(
        &frame,
        Some(&shared.key),
        None,
        &shared.instance,
        shared.auth.as_ref(),
        shared.bridge.as_ref(),
    );
    let mut out = reply.to_string();
    out.push('\n');
    let mut w = stream;
    let _ = w.write_all(out.as_bytes());
    let _ = w.flush();
}

/// Where the relay finds the host and what it proves itself with.
#[derive(Clone)]
pub struct Endpoint {
    /// The listener's address (loopback).
    pub addr: SocketAddr,
    /// The endpoint file's key.
    pub key: String,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("addr", &self.addr)
            .field("key", &"<redacted>")
            .finish()
    }
}

/// Finds the running app and this connection's token (the CLI reads the
/// endpoint file and the keychain).
pub trait Locate: Send + Sync {
    /// The running app's endpoint.
    ///
    /// # Errors
    /// [`UpstreamError::Closed`] when it is not running, `Refused` when the
    /// endpoint file is unsafe.
    fn endpoint(&self) -> Result<Endpoint, UpstreamError>;
    /// The connection id.
    fn connection(&self) -> &str;
    /// The connection's token.
    ///
    /// # Errors
    /// `Refused` with plain words when there is none on this computer.
    fn token(&self) -> Result<String, UpstreamError>;
}

/// The relay's [`Upstream`] on this computer: one loopback request per
/// message.
pub struct Loopback<L: Locate> {
    locate: L,
}

impl<L: Locate> Loopback<L> {
    /// Over `locate`.
    pub fn new(locate: L) -> Loopback<L> {
        Loopback { locate }
    }

    fn exchange(&self, kind: FrameKind, message: Option<&Value>) -> Result<Value, UpstreamError> {
        let ep = self.locate.endpoint()?;
        let token = self.locate.token()?;
        let line = request_line(&ep.key, self.locate.connection(), &token, kind, message);
        // The listener drops a longer line unread; say so here instead of
        // answering "interrupted" for a call that was never taken.
        if line.len() > MAX_REQUEST {
            return Err(UpstreamError::Refused(TOO_LARGE.into()));
        }
        let Ok(stream) = TcpStream::connect_timeout(&ep.addr, CONNECT_TIMEOUT) else {
            return Err(UpstreamError::Closed);
        };
        let _ = stream.set_read_timeout(Some(ANSWER_TIMEOUT));
        let mut w = &stream;
        if w.write_all(line.as_bytes())
            .and_then(|()| w.flush())
            .is_err()
        {
            return Err(UpstreamError::Closed);
        }
        let mut reader = BufReader::new((&stream).take(MAX_ANSWER as u64 + 1));
        let mut answer = String::new();
        match reader.read_line(&mut answer) {
            Ok(n) if n > 0 && answer.len() <= MAX_ANSWER => {}
            // Sent, never answered: it may have been acted on.
            _ => return Err(lost_or_closed(kind)),
        }
        let Ok(frame) = serde_json::from_str::<Value>(&answer) else {
            return Err(lost_or_closed(kind));
        };
        if frame["ok"] != true {
            let why = frame["refused"].as_str().unwrap_or(WRONG_KEY);
            return Err(UpstreamError::Refused(why.to_string()));
        }
        Ok(frame)
    }
}

fn lost_or_closed(kind: FrameKind) -> UpstreamError {
    match kind {
        FrameKind::Probe => UpstreamError::Closed,
        FrameKind::Mcp => UpstreamError::Lost,
    }
}

impl<L: Locate> Upstream for Loopback<L> {
    fn probe(&self) -> Result<Seen, UpstreamError> {
        let f = self.exchange(FrameKind::Probe, None)?;
        Ok(Seen {
            instance: f["instance"].as_str().unwrap_or("").to_string(),
            fingerprint: f["fingerprint"].as_str().unwrap_or("").to_string(),
        })
    }

    fn send(&self, msg: &Value) -> Result<Option<Value>, UpstreamError> {
        let f = self.exchange(FrameKind::Mcp, Some(msg))?;
        Ok(match &f["answer"] {
            Value::Null => None,
            a => Some(a.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Switches;
    use crate::protocol::tests::{Fake, on};
    use std::sync::Mutex;

    const KEY: &str = "k0123456789abcdef0123456789abcdef";
    const ID: &str = "c-1";

    struct Tokens(Mutex<Vec<String>>);
    impl Authenticate for Tokens {
        fn check(&self, c: &str, t: &str, device: Option<&str>) -> Result<(), String> {
            if self.0.lock().expect("lock").iter().any(|x| x == "revoked") {
                return Err("This connection was revoked in ClientLedger.".into());
            }
            if c == ID && tokens_equal(t, "secret") && device.is_none_or(|d| d == "dev-a") {
                Ok(())
            } else {
                Err("wrong token".into())
            }
        }
    }

    struct At {
        ep: Mutex<Option<Endpoint>>,
        token: String,
    }
    impl Locate for At {
        fn endpoint(&self) -> Result<Endpoint, UpstreamError> {
            self.ep
                .lock()
                .expect("lock")
                .clone()
                .ok_or(UpstreamError::Closed)
        }
        fn connection(&self) -> &str {
            ID
        }
        fn token(&self) -> Result<String, UpstreamError> {
            Ok(self.token.clone())
        }
    }

    fn host(switches: Switches) -> (Listener, Arc<Tokens>) {
        let auth = Arc::new(Tokens(Mutex::new(Vec::new())));
        let l = Listener::start(
            KEY.into(),
            "run-1".into(),
            Arc::clone(&auth) as Arc<dyn Authenticate>,
            Arc::new(Fake::new(switches)),
        )
        .expect("listen");
        (l, auth)
    }

    fn upstream(l: &Listener, key: &str, token: &str) -> Loopback<At> {
        Loopback::new(At {
            ep: Mutex::new(Some(Endpoint {
                addr: l.addr(),
                key: key.into(),
            })),
            token: token.into(),
        })
    }

    #[test]
    fn listens_on_loopback_only() {
        let (l, _) = host(on());
        assert!(l.addr().ip().is_loopback());
        assert!(format!("{l:?}").contains("127.0.0.1"));
        let ep = Endpoint {
            addr: l.addr(),
            key: KEY.into(),
        };
        assert!(!format!("{ep:?}").contains(KEY), "the key is never printed");
    }

    #[test]
    fn a_probe_and_a_message_go_through() {
        let (l, _) = host(on());
        let up = upstream(&l, KEY, "secret");
        let seen = up.probe().expect("probe");
        assert_eq!(seen.instance, "run-1");
        assert_eq!(seen.fingerprint.len(), 16);
        let a = up
            .send(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list"}))
            .expect("send")
            .expect("answer");
        assert!(
            a["result"]["tools"]
                .as_array()
                .is_some_and(|t| !t.is_empty())
        );
        assert_eq!(
            up.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .expect("send"),
            None
        );
    }

    #[test]
    fn a_wrong_key_or_token_is_refused_before_the_bridge() {
        let (l, auth) = host(on());
        let e = upstream(&l, "wrong-key-0123456789abcdef0123456", "secret").probe();
        assert_eq!(e, Err(UpstreamError::Refused(WRONG_KEY.into())));
        let e = upstream(&l, KEY, "not-it").probe();
        assert_eq!(e, Err(UpstreamError::Refused("wrong token".into())));
        auth.0.lock().expect("lock").push("revoked".into());
        let e =
            upstream(&l, KEY, "secret").send(&json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
        assert!(matches!(e, Err(UpstreamError::Refused(w)) if w.contains("revoked")));
    }

    #[test]
    fn no_endpoint_or_a_stopped_host_is_closed() {
        let up = Loopback::new(At {
            ep: Mutex::new(None),
            token: "secret".into(),
        });
        assert_eq!(up.probe(), Err(UpstreamError::Closed));
        let (l, _) = host(on());
        let addr = l.addr();
        drop(l);
        std::thread::sleep(Duration::from_millis(50));
        let up = Loopback::new(At {
            ep: Mutex::new(Some(Endpoint {
                addr,
                key: KEY.into(),
            })),
            token: "secret".into(),
        });
        assert_eq!(up.probe(), Err(UpstreamError::Closed));
    }

    #[test]
    fn a_frame_without_a_key_is_for_the_lan_host_with_its_device() {
        let auth = Tokens(Mutex::new(Vec::new()));
        let bridge = Fake::new(on());
        let frame: Value =
            serde_json::from_str(&request_line("", ID, "secret", FrameKind::Probe, None))
                .expect("json");
        let ok = answer_frame(&frame, None, Some("dev-a"), "lan", &auth, &bridge);
        assert_eq!(ok["ok"], true);
        let other = answer_frame(&frame, None, Some("dev-b"), "lan", &auth, &bridge);
        assert_eq!(
            other["ok"], false,
            "a token presented by another device is refused"
        );
        let mut bad = frame.clone();
        bad["connection"] = json!("../../etc");
        assert_eq!(
            answer_frame(&bad, None, Some("dev-a"), "lan", &auth, &bridge)["ok"],
            false
        );
        let mut old = frame;
        old["v"] = json!(0);
        assert_eq!(
            answer_frame(&old, None, Some("dev-a"), "lan", &auth, &bridge)["ok"],
            false
        );
    }

    #[test]
    fn oversized_and_garbage_requests_get_nothing() {
        let (l, _) = host(on());
        for body in ["hello\n".to_string(), "x".repeat(MAX_REQUEST + 10)] {
            let mut s = TcpStream::connect(l.addr()).expect("connect");
            let _ = s.write_all(body.as_bytes());
            let _ = s.shutdown(std::net::Shutdown::Write);
            let mut back = String::new();
            let _ = s.read_to_string(&mut back);
            assert_eq!(back, "", "nothing answered");
        }
        // Still serving.
        assert!(upstream(&l, KEY, "secret").probe().is_ok());
    }

    #[test]
    fn a_message_too_large_to_send_says_so_and_the_relay_answers_it() {
        let (l, _) = host(on());
        let big = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
            "params": {"name": "bill_change", "arguments": {"action": "create",
                "fields": {"memo": "x".repeat(MAX_REQUEST)}}}});
        let up = upstream(&l, KEY, "secret");
        assert_eq!(up.send(&big), Err(UpstreamError::Refused(TOO_LARGE.into())));
        let up: Arc<dyn Upstream> = Arc::new(up);
        let out = crate::relay::tests::Shared::default();
        let input = format!("{big}\n");
        crate::relay::relay_every(
            &up,
            input.as_bytes(),
            Box::new(out.clone()),
            Duration::from_secs(3600),
        );
        let r = out.replies();
        assert_eq!(r[0]["id"], 9);
        assert_eq!(r[0]["result"]["isError"], true);
        assert_eq!(r[0]["result"]["content"][0]["text"], TOO_LARGE);
    }

    #[test]
    fn only_a_probe_carries_the_fingerprint() {
        let auth = Tokens(Mutex::new(Vec::new()));
        let bridge = Fake::new(on());
        let probe: Value =
            serde_json::from_str(&request_line(KEY, ID, "secret", FrameKind::Probe, None))
                .expect("json");
        let ping = json!({"jsonrpc": "2.0", "id": 1, "method": "ping"});
        let mcp: Value = serde_json::from_str(&request_line(
            KEY,
            ID,
            "secret",
            FrameKind::Mcp,
            Some(&ping),
        ))
        .expect("json");
        let p = answer_frame(&probe, Some(KEY), None, "r", &auth, &bridge);
        assert!(p["fingerprint"].is_string());
        let m = answer_frame(&mcp, Some(KEY), None, "r", &auth, &bridge);
        assert!(m.get("fingerprint").is_none());
        assert_eq!(m["answer"]["id"], 1);
    }

    #[test]
    fn connection_ids_and_token_names_fit_the_keychain() {
        assert!(valid_connection_id("0b7e2a52-6f1d-4c0e-9d1e-3f1f2a9c0d11"));
        for bad in ["", "A", "a b", "a/b", "a_b", &"a".repeat(41)] {
            assert!(!valid_connection_id(bad), "{bad}");
        }
        assert!(token_name(&"a".repeat(40)).len() <= 48);
        assert!(tokens_equal("abc", "abc"));
        assert!(!tokens_equal("abc", "abd"));
        assert!(!tokens_equal("abc", "abcd"));
    }

    /// The relay over a real loopback listener: the same messages a client
    /// sends, answered by the host's protocol.
    #[test]
    fn the_relay_speaks_through_the_listener() {
        let (l, _) = host(on());
        let up: Arc<dyn Upstream> = Arc::new(upstream(&l, KEY, "secret"));
        let out = crate::relay::tests::Shared::default();
        let input = format!(
            "{}\n{}\n{}\n",
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "mcp_status"}}),
        );
        crate::relay::relay_every(
            &up,
            input.as_bytes(),
            Box::new(out.clone()),
            Duration::from_secs(3600),
        );
        let r = out.replies();
        assert_eq!(r.len(), 2, "{r:?}");
        assert!(
            r[0]["result"]["instructions"]
                .as_str()
                .is_some_and(|t| t.contains("can: Read"))
        );
        assert!(
            r[1]["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("ClientLedger is open"))
        );
    }
}
