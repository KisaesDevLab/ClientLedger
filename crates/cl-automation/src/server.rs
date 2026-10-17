//! The loopback control server.
//!
//! It only exists when the app is started with `--control <path>`. It binds
//! `127.0.0.1` on a random port, publishes the port and a random token in the
//! control file, and speaks one JSON object per line. A request without the
//! right token is answered with an error and the connection is closed. Valid
//! requests are handed to the UI thread, which answers them between frames.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::actions::{ErrorCode, Request, Response};
use crate::control_file::{
    self, ControlFileError, ControlInfo, generate_token, remove_if_ours, tokens_equal,
};
use crate::tools::is_method;

/// Longest request line accepted.
const MAX_LINE: u64 = 64 * 1024;
/// How long a connection may sit silent.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
/// Total time a new connection has to deliver its first complete line (which
/// must carry the token). A client that drips bytes does not get longer.
const FIRST_LINE_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(400)
} else {
    Duration::from_secs(5)
};
/// How long the UI has to answer one request.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Simultaneous connections.
const MAX_CONNECTIONS: usize = 8;

/// A request waiting for the UI thread.
pub struct Pending {
    /// The request (token already checked).
    pub request: Request,
    reply: Sender<Response>,
}

impl Pending {
    /// Send the answer. A client that went away is not an error.
    pub fn respond(self, response: Response) {
        let _ = self.reply.send(response);
    }
}

type Waker = Box<dyn Fn() + Send + Sync>;

struct Shared {
    waker: Mutex<Option<Waker>>,
    stop: AtomicBool,
    connections: Mutex<usize>,
}

/// The running server. Dropping it stops listening and removes the control
/// file.
pub struct ControlServer {
    requests: Receiver<Pending>,
    shared: Arc<Shared>,
    path: PathBuf,
    token: String,
    addr: SocketAddr,
}

impl std::fmt::Debug for ControlServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlServer")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl ControlServer {
    /// Start listening and publish the control file at `path`.
    ///
    /// # Errors
    /// The control file was refused or could not be written, or no port could
    /// be bound.
    pub fn start(path: &Path) -> Result<ControlServer, ControlFileError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let token = generate_token()?;
        let info = ControlInfo {
            version: 1,
            port: addr.port(),
            token: token.clone(),
            pid: std::process::id(),
        };
        // Written last, so a client that finds the file can connect.
        control_file::write_new(path, &info)?;

        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            waker: Mutex::new(None),
            stop: AtomicBool::new(false),
            connections: Mutex::new(0),
        });
        let accept_shared = Arc::clone(&shared);
        let accept_token = token.clone();
        thread::Builder::new()
            .name("control-accept".into())
            .spawn(move || accept_loop(&listener, &accept_shared, &accept_token, &tx))?;
        Ok(ControlServer {
            requests: rx,
            shared,
            path: path.to_path_buf(),
            token,
            addr,
        })
    }

    /// The address being listened on (always loopback).
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Set the function that wakes the UI when a request arrives.
    ///
    /// # Panics
    /// Only if another thread panicked while holding the waker lock.
    pub fn set_waker(&self, waker: impl Fn() + Send + Sync + 'static) {
        *self.shared.waker.lock().expect("waker lock") = Some(Box::new(waker));
    }

    /// The next request, if one is waiting (never blocks).
    #[must_use]
    pub fn try_next(&self) -> Option<Pending> {
        self.requests.try_recv().ok()
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the accept loop so it sees the flag and closes the socket.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
        remove_if_ours(&self.path, &self.token);
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>, token: &str, tx: &Sender<Pending>) {
    for stream in listener.incoming() {
        if shared.stop.load(Ordering::SeqCst) {
            break;
        }
        let Ok(stream) = stream else { continue };
        // Only this machine: refuse anything that is not loopback.
        if !stream.peer_addr().is_ok_and(|a| a.ip().is_loopback()) {
            continue;
        }
        {
            let Ok(mut n) = shared.connections.lock() else {
                continue;
            };
            if *n >= MAX_CONNECTIONS {
                continue;
            }
            *n += 1;
        }
        let shared = Arc::clone(shared);
        let token = token.to_string();
        let tx = tx.clone();
        let spawned = thread::Builder::new().name("control-conn".into()).spawn({
            let shared = Arc::clone(&shared);
            move || {
                serve(&stream, &shared, &token, &tx);
                if let Ok(mut n) = shared.connections.lock() {
                    *n = n.saturating_sub(1);
                }
            }
        });
        if spawned.is_err()
            && let Ok(mut n) = shared.connections.lock()
        {
            *n = n.saturating_sub(1);
        }
    }
}

/// Reads from the socket, enforcing one overall deadline while it is set.
struct DeadlineReader<'a> {
    stream: &'a TcpStream,
    deadline: Option<std::time::Instant>,
}

impl std::io::Read for DeadlineReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let timeout = match self.deadline {
            Some(d) => {
                let left = d.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
                left
            }
            None => READ_TIMEOUT,
        };
        self.stream.set_read_timeout(Some(timeout))?;
        std::io::Read::read(&mut self.stream, buf)
    }
}

fn serve(stream: &TcpStream, shared: &Arc<Shared>, token: &str, tx: &Sender<Pending>) {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let first = DeadlineReader {
        stream,
        deadline: Some(std::time::Instant::now() + FIRST_LINE_DEADLINE),
    };
    let mut reader = BufReader::new(std::io::Read::take(first, MAX_LINE));
    let mut line = String::new();
    loop {
        line.clear();
        // Each line gets a fresh allowance.
        reader.get_mut().set_limit(MAX_LINE);
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !line.ends_with('\n') {
            // Too long, or the client closed mid-line.
            let r = Response::failure(ErrorCode::BadRequest, "request line too long or incomplete");
            let _ = write_line(&mut writer, &r);
            break;
        }
        let (response, close) = handle_line(line.trim_end(), shared, token, tx);
        if write_line(&mut writer, &response).is_err() || close {
            break;
        }
        // The first line carried a valid token: from now on only the idle
        // timeout applies.
        reader.get_mut().get_mut().deadline = None;
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn write_line(w: &mut TcpStream, r: &Response) -> std::io::Result<()> {
    let mut text = serde_json::to_string(r).unwrap_or_else(|_| "{\"ok\":false}".into());
    text.push('\n');
    w.write_all(text.as_bytes())?;
    w.flush()
}

/// Returns the response and whether to close the connection.
fn handle_line(
    line: &str,
    shared: &Arc<Shared>,
    token: &str,
    tx: &Sender<Pending>,
) -> (Response, bool) {
    // Not JSON (for example a web page poking the port): drop it.
    let Ok(request) = serde_json::from_str::<Request>(line) else {
        return (
            Response::failure(ErrorCode::BadRequest, "not a valid request"),
            true,
        );
    };
    if !tokens_equal(&request.token, token) {
        return (
            Response::failure(ErrorCode::Unauthorized, "wrong or missing token"),
            true,
        );
    }
    let id = request.id.clone();
    if !is_method(&request.method) {
        let mut r = Response::failure(
            ErrorCode::UnknownMethod,
            format!("unknown method `{}`", request.method),
        );
        r.id = id;
        return (r, false);
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx
        .send(Pending {
            request,
            reply: reply_tx,
        })
        .is_err()
    {
        return (
            Response::failure(ErrorCode::Closed, "the app is closing"),
            true,
        );
    }
    if let Ok(guard) = shared.waker.lock()
        && let Some(w) = guard.as_ref()
    {
        w();
    }
    let mut response = match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            Response::failure(ErrorCode::Timeout, "the app did not answer in time")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Response::failure(ErrorCode::Closed, "the app closed before answering")
        }
    };
    response.id = id;
    (response, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Read;

    /// Answers every request with `{"echo": method}` until dropped.
    fn start() -> (tempfile::TempDir, ControlServer, ControlInfo) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let server = ControlServer::start(&path).unwrap();
        let info = control_file::read_checked(&path).unwrap();
        (dir, server, info)
    }

    fn connect(info: &ControlInfo) -> TcpStream {
        let s = TcpStream::connect(("127.0.0.1", info.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s
    }

    fn exchange(s: &mut TcpStream, text: &str) -> Option<Value> {
        s.write_all(text.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        let mut line = String::new();
        BufReader::new(s.try_clone().unwrap())
            .read_line(&mut line)
            .ok()?;
        serde_json::from_str(&line).ok()
    }

    fn poll_until_answered(server: &ControlServer, method_result: Value) {
        let start = std::time::Instant::now();
        loop {
            if let Some(p) = server.try_next() {
                p.respond(Response::success(method_result));
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "no request arrived"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn binds_loopback_only_and_publishes_the_port() {
        let (_d, server, info) = start();
        assert!(server.addr().ip().is_loopback());
        assert_eq!(server.addr().port(), info.port);
    }

    #[test]
    fn wrong_token_is_refused_and_never_reaches_the_app() {
        let (_d, server, info) = start();
        let mut s = connect(&info);
        let r = exchange(
            &mut s,
            &json!({"token": "x".repeat(64), "method": "state"}).to_string(),
        )
        .unwrap();
        assert_eq!(r["ok"], false);
        assert_eq!(r["error"]["code"], "unauthorized");
        assert!(server.try_next().is_none());
        // The connection was closed.
        let mut rest = Vec::new();
        assert_eq!(s.read_to_end(&mut rest).unwrap_or(0), 0);
    }

    #[test]
    fn non_json_such_as_an_http_request_is_dropped() {
        let (_d, server, info) = start();
        let mut s = connect(&info);
        let r = exchange(&mut s, "POST / HTTP/1.1").unwrap();
        assert_eq!(r["error"]["code"], "bad_request");
        assert!(server.try_next().is_none());
    }

    #[test]
    fn unknown_methods_are_rejected_before_the_app_sees_them() {
        let (_d, server, info) = start();
        let mut s = connect(&info);
        let r = exchange(
            &mut s,
            &json!({"token": info.token, "method": "delete_everything"}).to_string(),
        )
        .unwrap();
        assert_eq!(r["error"]["code"], "unknown_method");
        assert!(server.try_next().is_none());
    }

    #[test]
    fn valid_request_reaches_the_app_and_the_answer_comes_back_with_the_id() {
        let (_d, server, info) = start();
        let token = info.token.clone();
        let client = thread::spawn(move || {
            let mut s = connect(&info);
            exchange(
                &mut s,
                &json!({"token": token, "method": "state", "id": 7}).to_string(),
            )
            .unwrap()
        });
        poll_until_answered(&server, json!({"theme": "light"}));
        let r = client.join().unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["result"]["theme"], "light");
        assert_eq!(r["id"], 7);
    }

    #[test]
    fn waker_runs_when_a_request_arrives() {
        let (_d, server, info) = start();
        let woke = Arc::new(AtomicBool::new(false));
        let w = Arc::clone(&woke);
        server.set_waker(move || w.store(true, Ordering::SeqCst));
        let token = info.token.clone();
        let client = thread::spawn(move || {
            let mut s = connect(&info);
            exchange(
                &mut s,
                &json!({"token": token, "method": "state"}).to_string(),
            )
        });
        poll_until_answered(&server, json!({}));
        client.join().unwrap();
        assert!(woke.load(Ordering::SeqCst));
    }

    #[test]
    fn oversized_line_is_refused() {
        let (_d, _server, info) = start();
        let mut s = connect(&info);
        let big = "a".repeat(70_000);
        s.write_all(big.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        let mut line = String::new();
        BufReader::new(s.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.contains("bad_request"), "{line}");
    }

    #[test]
    fn dropping_the_server_removes_the_control_file_and_closes_the_port() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let server = ControlServer::start(&path).unwrap();
        let addr = server.addr();
        assert!(path.exists());
        drop(server);
        assert!(!path.exists());
        thread::sleep(Duration::from_millis(100));
        assert!(TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_err());
    }

    #[test]
    fn refuses_to_start_on_an_unsafe_control_file() {
        // A directory in the way is not a regular file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        std::fs::create_dir(&path).unwrap();
        assert!(matches!(
            ControlServer::start(&path),
            Err(ControlFileError::Unsafe(_))
        ));
    }

    #[test]
    fn a_connection_that_drips_bytes_is_cut_off_at_the_first_line_deadline() {
        let (_d, _server, info) = start();
        let mut s = connect(&info);
        let began = std::time::Instant::now();
        let mut closed = false;
        while began.elapsed() < Duration::from_secs(3) {
            if s.write_all(b" ").is_err() {
                closed = true;
                break;
            }
            thread::sleep(Duration::from_millis(50));
            s.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
            let mut b = [0u8; 1];
            if matches!(s.read(&mut b), Ok(0)) {
                closed = true;
                break;
            }
        }
        assert!(closed, "the server must close a slow first line");
        assert!(began.elapsed() < Duration::from_secs(2));
    }
}
