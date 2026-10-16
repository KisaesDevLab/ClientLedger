//! One running `ClientLedger` per OS user.
//!
//! The first instance holds an exclusive lock on `run/instance.lock` (the
//! operating system releases it if the process dies) and listens on a loopback
//! port. The port and a random 256-bit key are written to `run/instance.ctl`,
//! a file only the owner can read (the app data folder is per user; on Unix
//! the folder is mode 0700 and the file 0600). A second launch, or a CLI
//! command, reads that file and sends one line `<key> <command>`; the first
//! instance answers with one line. Commands: `focus` (bring the window to the
//! front), `status`, `stop`.
//!
//! Why loopback TCP and not a named pipe or Unix socket (Q-P04c): one code
//! path on every OS with no `unsafe` and no extra dependency. Other local
//! users cannot connect usefully because every request needs the key. A
//! program that grabbed the port after a crash can only learn the key from a
//! second launch, and the key opens nothing but these three harmless commands.

use crate::paths::AppPaths;
use crate::{HostError, Result};
use cl_pg::Secret;
use std::fs::{self, File, TryLockError};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const LOCK_FILE: &str = "instance.lock";
const CONTROL_FILE: &str = "instance.ctl";

/// How long a client waits for a starting instance to publish its endpoint.
const PUBLISH_WAIT: Duration = Duration::from_secs(3);
/// Longest request line accepted.
const MAX_LINE: u64 = 256;

/// What another process can ask the running instance to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Bring the main window to the front.
    Focus,
    /// Report the supervisor state (one line, no secrets).
    Status,
    /// Shut down the supervisor and exit.
    Stop,
}

impl Request {
    fn word(self) -> &'static str {
        match self {
            Request::Focus => "focus",
            Request::Status => "status",
            Request::Stop => "stop",
        }
    }

    fn parse(word: &str) -> Option<Request> {
        match word {
            "focus" => Some(Request::Focus),
            "status" => Some(Request::Status),
            "stop" => Some(Request::Stop),
            _ => None,
        }
    }
}

/// State shared between the listener thread and the owner of the guard.
#[derive(Debug, Default)]
struct Shared {
    focus: AtomicBool,
    stop: AtomicBool,
    status: Mutex<String>,
    shutdown: AtomicBool,
}

/// Proof that this process is the one running instance. Dropping it releases
/// the lock and removes the control endpoint.
#[derive(Debug)]
pub struct InstanceGuard {
    // Held for its lock; closing the handle releases it.
    _lock: File,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    control_path: PathBuf,
}

/// Result of [`acquire`].
#[derive(Debug)]
pub enum Acquired {
    /// This process is the first instance.
    First(InstanceGuard),
    /// Another instance is running.
    Second,
}

fn control_path(paths: &AppPaths) -> PathBuf {
    paths.run_dir().join(CONTROL_FILE)
}

/// Try to become the one running instance.
///
/// # Errors
/// Fails when the lock file or the control endpoint cannot be created.
pub fn acquire(paths: &AppPaths) -> Result<Acquired> {
    crate::paths::create_private_dir(&paths.run_dir())?;
    let lock_path = paths.run_dir().join(LOCK_FILE);
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .read(true)
        .open(&lock_path)
        .map_err(|e| HostError::io("open the instance lock", e))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Acquired::Second),
        Err(TryLockError::Error(e)) => return Err(HostError::io("lock the instance lock", e)),
    }

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| HostError::io("open the control endpoint", e))?;
    let addr = listener
        .local_addr()
        .map_err(|e| HostError::io("open the control endpoint", e))?;
    let key = Secret::generate()?;
    let control = control_path(paths);
    write_control_file(&control, addr.port(), &key)?;

    let shared = Arc::new(Shared::default());
    let thread = {
        let shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("cl-instance".into())
            .spawn(move || serve(&listener, &key, &shared))
            .map_err(|e| HostError::io("start the control endpoint", e))?
    };
    Ok(Acquired::First(InstanceGuard {
        _lock: lock,
        shared,
        thread: Some(thread),
        control_path: control,
    }))
}

impl InstanceGuard {
    /// True once, after another launch asked to focus this instance.
    #[must_use]
    pub fn take_focus_request(&self) -> bool {
        self.shared.focus.swap(false, Ordering::SeqCst)
    }

    /// True once `stop` was requested from outside (the CLI).
    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.shared.stop.load(Ordering::SeqCst)
    }

    /// The text returned to `status` requests. Must not contain secrets.
    pub fn set_status(&self, text: &str) {
        if let Ok(mut s) = self.shared.status.lock() {
            text.clone_into(&mut s);
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = fs::remove_file(&self.control_path);
    }
}

fn write_control_file(path: &Path, port: u16, key: &Secret) -> Result<()> {
    let tmp = path.with_extension("ctl.tmp");
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .map_err(|e| HostError::io("write the control endpoint", e))?;
    write!(f, "{port}\n{}\n", key.expose())
        .and_then(|()| f.sync_all())
        .map_err(|e| HostError::io("write the control endpoint", e))?;
    drop(f);
    fs::rename(&tmp, path).map_err(|e| HostError::io("publish the control endpoint", e))
}

/// Equality that does not stop at the first difference.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn serve(listener: &TcpListener, key: &Secret, shared: &Shared) {
    // Polling accept: shutting down never depends on a wake-up connection.
    let _ = listener.set_nonblocking(true);
    while !shared.shutdown.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // One slow or malicious client must not block the next one for long.
                let _ = stream.set_nonblocking(false);
                let _ = handle(stream, key, shared);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn handle(mut stream: TcpStream, key: &Secret, shared: &Shared) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE)).read_line(&mut line)?;
    let line = line.trim();
    let reply = match line.split_once(' ') {
        Some((given, word)) if constant_time_eq(given, key.expose()) => {
            match Request::parse(word.trim()) {
                Some(Request::Focus) => {
                    shared.focus.store(true, Ordering::SeqCst);
                    "ok".to_string()
                }
                Some(Request::Stop) => {
                    shared.stop.store(true, Ordering::SeqCst);
                    "ok".to_string()
                }
                Some(Request::Status) => shared
                    .status
                    .lock()
                    .map_or_else(|_| "unknown".into(), |s| s.clone()),
                None => "error unknown command".to_string(),
            }
        }
        _ => "denied".to_string(),
    };
    stream.write_all(reply.replace('\n', " ").as_bytes())?;
    stream.write_all(b"\n")
}

/// Why a request to the running instance did not go through.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// No instance is running (no control endpoint, or nothing answers).
    #[error("ClientLedger is not running")]
    NotRunning,
    /// The instance refused the request.
    #[error("the running instance refused the request")]
    Denied,
    /// Talking to it failed.
    #[error("could not talk to the running instance: {0}")]
    Io(#[from] std::io::Error),
}

/// Send `request` to the running instance and return its one-line answer.
///
/// # Errors
/// [`SendError::NotRunning`] when no instance answers.
pub fn send(paths: &AppPaths, request: Request) -> std::result::Result<String, SendError> {
    let deadline = Instant::now() + PUBLISH_WAIT;
    loop {
        match send_once(&control_path(paths), request) {
            Err(SendError::NotRunning)
                if instance_is_starting(paths) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// True when the lock is held (an instance exists) but it has not published yet.
fn instance_is_starting(paths: &AppPaths) -> bool {
    let lock = paths.run_dir().join(LOCK_FILE);
    let Ok(f) = fs::OpenOptions::new().read(true).write(true).open(lock) else {
        return false;
    };
    matches!(f.try_lock(), Err(TryLockError::WouldBlock))
}

fn send_once(control: &Path, request: Request) -> std::result::Result<String, SendError> {
    let text = fs::read_to_string(control).map_err(|_| SendError::NotRunning)?;
    let mut lines = text.lines();
    let port: u16 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or(SendError::NotRunning)?;
    let key = lines.next().ok_or(SendError::NotRunning)?.trim();
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(1))
        .map_err(|_| SendError::NotRunning)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    writeln!(stream, "{key} {}", request.word())?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    let reply = reply.trim().to_string();
    if reply == "denied" {
        Err(SendError::Denied)
    } else {
        Ok(reply)
    }
}

/// The normal start-up check: become the first instance, or ask the running
/// one to come to the front.
///
/// # Errors
/// Fails like [`acquire`]. A failed focus request is only logged: the other
/// instance may be shutting down.
pub fn acquire_or_focus(paths: &AppPaths) -> Result<Option<InstanceGuard>> {
    match acquire(paths)? {
        Acquired::First(g) => Ok(Some(g)),
        Acquired::Second => {
            if let Err(e) = send(paths, Request::Focus) {
                tracing::warn!("could not ask the running instance to focus: {e}");
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> AppPaths {
        let root = std::env::temp_dir().join(format!("cl-host-inst-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        AppPaths::at(root).unwrap()
    }

    fn guard(a: Acquired) -> InstanceGuard {
        match a {
            Acquired::First(g) => g,
            Acquired::Second => panic!("expected to be first"),
        }
    }

    #[test]
    fn the_second_launch_is_told_so_and_focuses_the_first() {
        let paths = scratch("focus");
        let first = guard(acquire(&paths).unwrap());
        assert!(!first.take_focus_request());
        assert!(matches!(acquire(&paths).unwrap(), Acquired::Second));
        assert_eq!(send(&paths, Request::Focus).unwrap(), "ok");
        assert!(first.take_focus_request());
        assert!(!first.take_focus_request(), "the request is consumed once");
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn acquire_or_focus_sends_focus_for_a_second_launch() {
        let paths = scratch("aof");
        let first = acquire_or_focus(&paths).unwrap().unwrap();
        assert!(acquire_or_focus(&paths).unwrap().is_none());
        assert!(first.take_focus_request());
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn status_and_stop_requests_work() {
        let paths = scratch("status");
        let first = guard(acquire(&paths).unwrap());
        first.set_status("state=running\nengine=up");
        assert_eq!(
            send(&paths, Request::Status).unwrap(),
            "state=running engine=up"
        );
        assert!(!first.stop_requested());
        assert_eq!(send(&paths, Request::Stop).unwrap(), "ok");
        assert!(first.stop_requested());
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn the_lock_is_free_again_after_the_guard_drops() {
        let paths = scratch("release");
        drop(guard(acquire(&paths).unwrap()));
        assert!(matches!(
            send(&paths, Request::Focus),
            Err(SendError::NotRunning)
        ));
        let again = guard(acquire(&paths).unwrap());
        drop(again);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn nothing_running_means_not_running() {
        let paths = scratch("none");
        paths.ensure_dirs().unwrap();
        assert!(matches!(
            send(&paths, Request::Status),
            Err(SendError::NotRunning)
        ));
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn a_wrong_key_is_denied_and_changes_nothing() {
        let paths = scratch("denied");
        let first = guard(acquire(&paths).unwrap());
        let text = fs::read_to_string(control_path(&paths)).unwrap();
        let port: u16 = text.lines().next().unwrap().parse().unwrap();
        for line in ["wrong focus\n", "stop\n", "\n", "a b c\n"] {
            let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
            s.write_all(line.as_bytes()).unwrap();
            let mut reply = String::new();
            BufReader::new(&s).read_line(&mut reply).unwrap();
            assert_eq!(reply.trim(), "denied", "for {line:?}");
        }
        assert!(!first.take_focus_request());
        assert!(!first.stop_requested());
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn the_endpoint_listens_on_loopback_only() {
        let paths = scratch("loop");
        let first = guard(acquire(&paths).unwrap());
        let text = fs::read_to_string(control_path(&paths)).unwrap();
        let port: u16 = text.lines().next().unwrap().parse().unwrap();
        let all = listeners::get_all().unwrap();
        let mine: Vec<_> = all
            .iter()
            .filter(|l| l.socket.port() == port && l.process.pid == std::process::id())
            .collect();
        assert!(!mine.is_empty(), "our endpoint is listening");
        assert!(mine.iter().all(|l| l.socket.ip().is_loopback()), "{mine:?}");
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[cfg(unix)]
    #[test]
    fn the_control_file_and_folder_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let paths = scratch("perm");
        let first = guard(acquire(&paths).unwrap());
        let file_mode = fs::metadata(control_path(&paths))
            .unwrap()
            .permissions()
            .mode();
        let dir_mode = fs::metadata(paths.run_dir()).unwrap().permissions().mode();
        assert_eq!(file_mode & 0o077, 0, "control file mode {file_mode:o}");
        assert_eq!(dir_mode & 0o077, 0, "run folder mode {dir_mode:o}");
        drop(first);
        let _ = fs::remove_dir_all(paths.root());
    }

    #[test]
    fn comparison_is_exact() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("", "a"));
    }
}
