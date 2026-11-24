//! The stdio relay an MCP client runs: `clientledger-cli mcp --connection
//! <id>` (plan D4, Q-325, Q-A11b; the design of the Trial Balance app's relay).
//!
//! Claude Desktop, or any MCP client, starts the relay and speaks MCP on its
//! stdin and stdout for as long as the client is open. The app may open
//! and close many times meanwhile, so the relay finds the running app for
//! every request through an [`Upstream`] (the host's loopback listener on
//! this computer, or the LAN host's `/cl-mcp` on a workstation). While the
//! app is closed the relay answers itself, in plain words: open the app
//! first. When the app comes back the agent's handshake is given to it
//! again, and the agent is told `notifications/tools/list_changed`. A
//! second thread polls the app (N5: polling, no stream held open) so the
//! agent also hears when the app opens, closes or its switches change
//! between requests.
//!
//! Only MCP lines go to stdout, each written whole under one lock; logs go
//! to stderr or a file. Every request with an id is answered, including one
//! the app took and never answered (it closed mid-request).

use std::io::{BufRead, Write};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::protocol::{self, INTERNAL_ERROR, METHOD_NOT_FOUND};

/// What the agent is told while the app is closed.
pub const CLOSED: &str = "ClientLedger is not open on this computer. Ask the user to open \
     ClientLedger and the company, then try again.";
/// What the agent is told when the app took a request and never answered.
pub const INTERRUPTED: &str = "ClientLedger closed or restarted before it answered. If that was \
     a change, check it with mcp_status (action get_action) or ask the user to look in MCP \
     Activity before trying again.";
/// What the agent is told before any instructions while closed.
pub const CLOSED_INSTRUCTIONS: &str = "What this connection can do is shown once ClientLedger is \
     open; the tool list will change then.";
/// How often the app is looked for between requests (N5).
pub const POLL_EVERY: Duration = Duration::from_secs(3);

/// What the app is, when it answered: which run of it, and what it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    /// This run of the app (a restart gives a new one).
    pub instance: String,
    /// [`crate::policy::fingerprint`] of what it shows this connection
    /// (empty when no company is open).
    pub fingerprint: String,
}

/// Why the app could not be asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamError {
    /// It is not running (no endpoint, nothing listening).
    Closed,
    /// It refused this connection, in plain words (revoked, wrong token,
    /// another device, an unsafe endpoint file).
    Refused(String),
    /// It took the request and never answered.
    Lost,
}

/// The running app, as the relay reaches it.
pub trait Upstream: Send + Sync {
    /// Look for the app.
    ///
    /// # Errors
    /// [`UpstreamError`].
    fn probe(&self) -> Result<Seen, UpstreamError>;

    /// Send one MCP message; the app's answer (`None` for a notification).
    ///
    /// # Errors
    /// [`UpstreamError`].
    fn send(&self, msg: &Value) -> Result<Option<Value>, UpstreamError>;
}

type Out = Arc<Mutex<Box<dyn Write + Send>>>;

fn write_line(out: &Out, v: &Value) {
    let mut w = out.lock().unwrap_or_else(PoisonError::into_inner);
    let mut line = v.to_string();
    line.push('\n');
    // The client went away: nothing to tell; the input ends next.
    let _ = w.write_all(line.as_bytes());
    let _ = w.flush();
}

/// What the relay saw of the app last.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Last {
    /// Nothing yet.
    #[default]
    Unknown,
    /// Closed (or refusing).
    Closed,
    /// Open.
    Open(Seen),
}

#[derive(Default)]
struct State {
    /// The agent's `initialize` params.
    handshake: Option<Value>,
    /// The agent sent `notifications/initialized`.
    initialized: bool,
    /// What was seen last.
    last: Last,
    replays: u64,
}

/// Record what was seen; tell the agent when the list changed, and give a
/// new run of the app the agent's handshake.
fn note(state: &Mutex<State>, upstream: &dyn Upstream, out: &Out, seen: Option<&Seen>) {
    let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
    let now = seen.map_or(Last::Closed, |n| Last::Open(n.clone()));
    let before = match std::mem::replace(&mut s.last, now) {
        Last::Unknown => return, // the first look
        Last::Closed => None,
        Last::Open(b) => Some(b),
    };
    let new_run = seen.is_some_and(|n| before.as_ref().is_none_or(|b| b.instance != n.instance));
    if new_run && let Some(params) = s.handshake.clone() {
        s.replays += 1;
        let hello = json!({"jsonrpc": "2.0", "id": format!("relay-replay-{}", s.replays),
            "method": "initialize", "params": params});
        let _ = upstream.send(&hello);
        if s.initialized {
            let _ =
                upstream.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        }
    }
    let changed =
        before.as_ref().map(|b| &b.fingerprint) != seen.map(|n| &n.fingerprint) || new_run;
    if changed && s.initialized {
        tracing::info!(
            open = seen.is_some(),
            "the tool list changed; telling the agent"
        );
        write_line(
            out,
            &json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
        );
    }
}

/// `text` as the answer to `msg`: a tool call's error result, an error for
/// anything else; `None` for a notification.
#[must_use]
pub fn answer(msg: &Value, text: &str) -> Option<Value> {
    let id = msg.get("id")?;
    if msg.get("method").and_then(Value::as_str) == Some("tools/call") {
        return Some(protocol::result(id, protocol::tool_text(text, true)));
    }
    Some(protocol::error(id, INTERNAL_ERROR, text))
}

/// The relay's own answer while the app cannot be asked (`why`: closed, or
/// the app's refusal).
#[must_use]
pub fn closed_reply(msg: &Value, why: &str) -> Option<Value> {
    let id = msg.get("id")?;
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        // The app's own answer never reaches an agent that connected while
        // it was closed: say what is known.
        "initialize" => protocol::initialize_result(
            &params,
            &format!("{why} {CLOSED_INSTRUCTIONS} {}", crate::policy::NEVER),
        ),
        "ping" => json!({}),
        "tools/list" => json!({"tools": [protocol::status_only()]}),
        "tools/call" => protocol::tool_text(why, true),
        "resources/list" => json!({"resources": []}),
        "resources/templates/list" => json!({"resourceTemplates": []}),
        "prompts/list" => json!({"prompts": []}),
        _ => return Some(protocol::error(id, METHOD_NOT_FOUND, why)),
    };
    Some(protocol::result(id, result))
}

/// Relay MCP between an agent (`input`, `output`) and the app. Ends when
/// `input` ends.
pub fn relay(upstream: &Arc<dyn Upstream>, input: impl BufRead, output: Box<dyn Write + Send>) {
    relay_every(upstream, input, output, POLL_EVERY);
}

/// [`relay`] with the poll interval given (tests).
pub fn relay_every(
    upstream: &Arc<dyn Upstream>,
    input: impl BufRead,
    output: Box<dyn Write + Send>,
    poll: Duration,
) {
    let out: Out = Arc::new(Mutex::new(output));
    let state = Arc::new(Mutex::new(State::default()));
    let (stop, stopped) = channel::<()>();
    let poller = {
        let (up, out, state) = (Arc::clone(upstream), Arc::clone(&out), Arc::clone(&state));
        std::thread::Builder::new()
            .name("mcp-relay-poll".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(poll) {
                    // Nothing to tell before the agent is ready.
                    if !state
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .initialized
                    {
                        continue;
                    }
                    match up.probe() {
                        Ok(seen) => note(&state, up.as_ref(), &out, Some(&seen)),
                        Err(UpstreamError::Closed | UpstreamError::Refused(_)) => {
                            note(&state, up.as_ref(), &out, None);
                        }
                        Err(UpstreamError::Lost) => {}
                    }
                }
            })
            .ok()
    };
    for line in input.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            write_line(
                &out,
                &json!({"jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": "The message is not JSON."}}),
            );
            continue;
        };
        one(upstream.as_ref(), &state, &out, &msg);
    }
    drop(stop);
    if let Some(p) = poller {
        let _ = p.join();
    }
}

fn one(upstream: &dyn Upstream, state: &Mutex<State>, out: &Out, msg: &Value) {
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let is_init = method == "initialize";
    if is_init {
        let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
        s.handshake = Some(msg.get("params").cloned().unwrap_or(Value::Null));
        s.initialized = false;
        tracing::info!(
            client = msg["params"]["clientInfo"]["name"].as_str().unwrap_or(""),
            "an agent connected"
        );
    }
    if msg.get("id").is_none() {
        if method == "notifications/initialized" {
            state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .initialized = true;
        }
        // Notifications go to the app when it is there; nothing else needs them.
        let _ = upstream.send(msg);
        return;
    }
    // Find the app for this request.
    let probe = upstream.probe();
    match &probe {
        Ok(seen) if !is_init => note(state, upstream, out, Some(seen)),
        Ok(seen) => {
            // An initialize is its own handshake: just record the run.
            state.lock().unwrap_or_else(PoisonError::into_inner).last = Last::Open(seen.clone());
        }
        Err(UpstreamError::Closed | UpstreamError::Refused(_)) => {
            note(state, upstream, out, None);
        }
        Err(UpstreamError::Lost) => {}
    }
    let reply = match probe {
        Err(UpstreamError::Closed) => closed_reply(msg, CLOSED),
        Err(UpstreamError::Refused(why)) => closed_reply(msg, &why),
        Ok(_) | Err(UpstreamError::Lost) => match upstream.send(msg) {
            Ok(Some(a)) => Some(a),
            // Taken and never answered.
            Ok(None) | Err(UpstreamError::Lost) => answer(msg, INTERRUPTED),
            Err(UpstreamError::Closed) => closed_reply(msg, CLOSED),
            Err(UpstreamError::Refused(why)) => closed_reply(msg, &why),
        },
    };
    if let Some(r) = reply {
        write_line(out, &r);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Output kept for the test.
    #[derive(Clone, Default)]
    pub(crate) struct Shared(pub Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Shared {
        pub(crate) fn replies(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().expect("lock").clone())
                .expect("utf8")
                .lines()
                .map(|l| serde_json::from_str(l).expect("only JSON on stdout"))
                .collect()
        }
    }

    /// An app that opens and closes on cue.
    pub(crate) struct FakeApp {
        pub open: AtomicBool,
        pub instance: Mutex<String>,
        pub fingerprint: Mutex<String>,
        pub refused: Mutex<Option<String>>,
        pub lose_next: AtomicBool,
        pub sent: Mutex<Vec<Value>>,
    }

    impl FakeApp {
        pub(crate) fn new(open: bool) -> Arc<FakeApp> {
            Arc::new(FakeApp {
                open: AtomicBool::new(open),
                instance: Mutex::new("run-1".into()),
                fingerprint: Mutex::new("fp-a".into()),
                refused: Mutex::new(None),
                lose_next: AtomicBool::new(false),
                sent: Mutex::new(Vec::new()),
            })
        }
        fn methods(&self) -> Vec<String> {
            self.sent
                .lock()
                .expect("lock")
                .iter()
                .map(|m| m["method"].as_str().unwrap_or("").to_string())
                .collect()
        }
    }

    impl Upstream for FakeApp {
        fn probe(&self) -> Result<Seen, UpstreamError> {
            if let Some(r) = self.refused.lock().expect("lock").clone() {
                return Err(UpstreamError::Refused(r));
            }
            if !self.open.load(Ordering::SeqCst) {
                return Err(UpstreamError::Closed);
            }
            Ok(Seen {
                instance: self.instance.lock().expect("lock").clone(),
                fingerprint: self.fingerprint.lock().expect("lock").clone(),
            })
        }
        fn send(&self, msg: &Value) -> Result<Option<Value>, UpstreamError> {
            self.probe()?;
            self.sent.lock().expect("lock").push(msg.clone());
            if self.lose_next.swap(false, Ordering::SeqCst) {
                return Err(UpstreamError::Lost);
            }
            let Some(id) = msg.get("id") else {
                return Ok(None);
            };
            Ok(Some(protocol::result(id, json!({"from": "app"}))))
        }
    }

    /// stdin that stays open until the test drops its sender.
    pub(crate) struct ChanRead(pub std::sync::mpsc::Receiver<String>, pub Vec<u8>);
    impl std::io::Read for ChanRead {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(s) => self.1 = s.into_bytes(),
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    fn line(v: &Value) -> String {
        format!("{v}\n")
    }

    fn init() -> String {
        line(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "claude-ai"}}}))
    }

    fn initialized() -> String {
        line(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
    }

    fn call(id: i64) -> String {
        line(&json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": "bill_read", "arguments": {"action": "list"}}}))
    }

    fn run(app: &Arc<FakeApp>, input: &str) -> Vec<Value> {
        let out = Shared::default();
        let up: Arc<dyn Upstream> = Arc::clone(app) as Arc<dyn Upstream>;
        relay_every(
            &up,
            input.as_bytes(),
            Box::new(out.clone()),
            Duration::from_secs(3600),
        );
        out.replies()
    }

    #[test]
    fn closed_answers_the_handshake_and_says_to_open_the_app() {
        let app = FakeApp::new(false);
        let input = format!(
            "{}{}{}{}",
            init(),
            initialized(),
            line(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})),
            call(3)
        );
        let r = run(&app, &input);
        assert_eq!(r.len(), 3, "{r:?}");
        assert_eq!(r[0]["result"]["capabilities"]["tools"]["listChanged"], true);
        assert!(
            r[0]["result"]["instructions"]
                .as_str()
                .is_some_and(|t| t.contains("not open"))
        );
        let tools = r[1]["result"]["tools"].as_array().expect("tools");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["annotations"]["readOnlyHint"], true);
        assert_eq!(r[2]["result"]["isError"], true);
        assert_eq!(r[2]["result"]["content"][0]["text"], CLOSED);
        assert!(app.sent.lock().expect("lock").is_empty());
    }

    #[test]
    fn open_requests_go_to_the_app_and_its_answers_come_back() {
        let app = FakeApp::new(true);
        let r = run(&app, &format!("{}{}{}", init(), initialized(), call(2)));
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["result"]["from"], "app");
        assert_eq!(r[1]["id"], 2);
        assert_eq!(
            app.methods(),
            ["initialize", "notifications/initialized", "tools/call"]
        );
    }

    /// The TB cycle with a fake: closed, opened, closed, opened again on
    /// one connection; the app coming back is told the handshake and the
    /// agent is told the list changed.
    #[test]
    fn one_connection_lives_through_the_app_closing_and_opening() {
        let app = FakeApp::new(false);
        let out = Shared::default();
        let state = Mutex::new(State::default());
        let o: Out = Arc::new(Mutex::new(Box::new(out.clone())));
        let feed = |s: &str| {
            for l in s.lines() {
                one(
                    app.as_ref(),
                    &state,
                    &o,
                    &serde_json::from_str(l).expect("json"),
                );
            }
        };
        feed(&format!("{}{}{}", init(), initialized(), call(2)));
        assert_eq!(out.replies()[1]["result"]["content"][0]["text"], CLOSED);
        // Opened.
        app.open.store(true, Ordering::SeqCst);
        feed(&call(3));
        let r = out.replies();
        assert_eq!(r[2]["method"], "notifications/tools/list_changed");
        assert_eq!(r[3]["result"]["from"], "app");
        assert_eq!(
            app.methods(),
            ["initialize", "notifications/initialized", "tools/call"]
        );
        assert_eq!(
            app.sent.lock().expect("lock")[0]["params"]["clientInfo"]["name"],
            "claude-ai",
            "the agent's own handshake"
        );
        // Closed again.
        app.open.store(false, Ordering::SeqCst);
        feed(&call(4));
        let r = out.replies();
        assert_eq!(r[4]["method"], "notifications/tools/list_changed");
        assert_eq!(r[5]["result"]["content"][0]["text"], CLOSED);
        // Reopened as a new run.
        *app.instance.lock().expect("lock") = "run-2".into();
        app.open.store(true, Ordering::SeqCst);
        feed(&call(5));
        let r = out.replies();
        assert_eq!(r[6]["method"], "notifications/tools/list_changed");
        assert_eq!(r[7]["id"], 5);
        assert_eq!(r.len(), 8);
        assert_eq!(
            app.methods().iter().filter(|m| *m == "initialize").count(),
            2
        );
    }

    #[test]
    fn a_new_run_gets_the_handshake_again_without_a_closed_gap() {
        let app = FakeApp::new(true);
        let out = Shared::default();
        let state = Mutex::new(State::default());
        let o: Out = Arc::new(Mutex::new(Box::new(out.clone())));
        for l in format!("{}{}{}", init(), initialized(), call(2)).lines() {
            one(
                app.as_ref(),
                &state,
                &o,
                &serde_json::from_str(l).expect("json"),
            );
        }
        *app.instance.lock().expect("lock") = "run-2".into();
        one(
            app.as_ref(),
            &state,
            &o,
            &serde_json::from_str(call(3).trim()).expect("json"),
        );
        assert_eq!(
            app.methods(),
            [
                "initialize",
                "notifications/initialized",
                "tools/call",
                "initialize",
                "notifications/initialized",
                "tools/call"
            ]
        );
        assert!(
            out.replies()
                .iter()
                .any(|r| r["method"] == "notifications/tools/list_changed")
        );
    }

    #[test]
    fn a_request_the_app_took_and_lost_is_answered() {
        let app = FakeApp::new(true);
        app.lose_next.store(false, Ordering::SeqCst);
        let out = Shared::default();
        let state = Mutex::new(State::default());
        let o: Out = Arc::new(Mutex::new(Box::new(out.clone())));
        one(
            app.as_ref(),
            &state,
            &o,
            &serde_json::from_str(init().trim()).expect("json"),
        );
        app.lose_next.store(true, Ordering::SeqCst);
        one(
            app.as_ref(),
            &state,
            &o,
            &serde_json::from_str(call(2).trim()).expect("json"),
        );
        let r = out.replies();
        assert_eq!(r[1]["id"], 2);
        assert_eq!(r[1]["result"]["isError"], true);
        assert_eq!(r[1]["result"]["content"][0]["text"], INTERRUPTED);
    }

    #[test]
    fn a_refused_connection_says_why_in_plain_words() {
        let app = FakeApp::new(true);
        *app.refused.lock().expect("lock") = Some("This connection was revoked.".into());
        let r = run(&app, &format!("{}{}", init(), call(2)));
        assert!(
            r[0]["result"]["instructions"]
                .as_str()
                .is_some_and(|t| t.starts_with("This connection was revoked."))
        );
        assert_eq!(
            r[1]["result"]["content"][0]["text"],
            "This connection was revoked."
        );
    }

    #[test]
    fn garbage_gets_a_parse_error_and_the_relay_goes_on() {
        let app = FakeApp::new(true);
        let r = run(&app, &format!("not json\n\n{}", call(2)));
        assert_eq!(r[0]["error"]["code"], -32700);
        assert_eq!(r[1]["id"], 2);
    }

    #[test]
    fn the_poller_tells_the_agent_when_the_app_opens() {
        let app = FakeApp::new(false);
        let out = Shared::default();
        // stdin that stays open until the app has opened and been seen.
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let up: Arc<dyn Upstream> = Arc::clone(&app) as Arc<dyn Upstream>;
        let o = out.clone();
        let t = std::thread::spawn(move || {
            relay_every(
                &up,
                std::io::BufReader::new(ChanRead(rx, Vec::new())),
                Box::new(o),
                Duration::from_millis(10),
            );
        });
        tx.send(format!("{}{}", init(), initialized()))
            .expect("send");
        let wait = |pred: &dyn Fn(&[Value]) -> bool| {
            for _ in 0..500 {
                if pred(&out.replies()) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            false
        };
        assert!(wait(&|r| !r.is_empty()), "initialize answered");
        app.open.store(true, Ordering::SeqCst);
        assert!(
            wait(&|r| r
                .iter()
                .any(|m| m["method"] == "notifications/tools/list_changed")),
            "told without a request"
        );
        drop(tx);
        t.join().expect("relay ends with its input");
    }
}
