//! Did the app end cleanly last time? (P34, Q-P34o.)
//!
//! `run/session.state` says `running` from start-up to an orderly end and
//! `clean` after it. Reading `running` at the next start means the app was
//! stopped without its own shutdown: a crash, a kill, or a power cut. The
//! app then offers the crash recovery view.
//!
//! **No false alarms (plan §10.7).** An end that is not a crash must not
//! count as one:
//! - The marker records the computer's boot time. A `running` marker from an
//!   earlier boot means the computer restarted or shut down with the app
//!   open. That reads as [`LastExit::Unknown`], never `Unclean`. A power cut
//!   looks the same; if it hurt the database, [`crate::Supervisor::database_recovered`]
//!   says so separately.
//! - A marker whose process is still alive is an app being handed over
//!   (`acquire_or_take_over`): `Unknown`.
//! - Each session writes a random id. [`SessionMarker::mark_clean`] only
//!   overwrites its own session, so a slow-quitting old instance cannot mark
//!   a new one clean.
//! - A missing or unreadable file (first run, an upgrade from a build
//!   without the marker) is `Unknown`.
//!
//! **What the app must call.** Call [`begin_session`] once, after the
//! single-instance lock is held. Call [`SessionMarker::mark_clean`] (it is
//! idempotent) on every orderly end:
//! - normal quit;
//! - closing the headless LAN host (`--lan-serve`), including the hand-over;
//! - on Linux and macOS, the termination signals `SIGTERM` and `SIGHUP`
//!   (the desktop app installs a handler that marks clean, then closes the
//!   window).
//!
//! Known gaps (Q-P34w). Windows does not tell a winit window about log-off
//! (winit 0.30 does not handle `WM_ENDSESSION`), and a macOS log-out quits
//! the app through `NSApplication terminate:`, which winit does not hand to
//! the app (reaching it needs Objective-C glue this workspace keeps out).
//! A restart or shut-down is caught by the boot check above; a log-off
//! without a restart can still read as `Unclean` on Windows and macOS.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::{AppPaths, HostError, Result};

/// The marker file inside [`AppPaths::run_dir`].
pub const MARKER_FILE: &str = "session.state";

/// Boot times within this many seconds are the same boot (the OS reports
/// it from the uptime, which can be off by a second or two).
const BOOT_TOLERANCE_SECS: u64 = 120;

/// How the previous session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastExit {
    /// The app ended through its own shutdown.
    Clean,
    /// The app stopped while running, on this boot of the computer, without
    /// its own shutdown: a crash, a kill or a forced end.
    Unclean,
    /// No reliable answer: first run, no marker, an unreadable marker, the
    /// computer restarted since, or the previous process is still alive.
    Unknown,
}

/// What a marker file says.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Marker {
    running: bool,
    session: String,
    pid: Option<u32>,
    boot: Option<u64>,
}

fn parse(text: &str) -> Option<Marker> {
    let mut state = None;
    let mut session = String::new();
    let mut pid = None;
    let mut boot = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "state" => state = Some(v.trim().to_string()),
            "session" => session = v.trim().to_string(),
            "pid" => pid = v.trim().parse().ok(),
            "boot" => boot = v.trim().parse().ok(),
            _ => {}
        }
    }
    let running = match state.as_deref() {
        Some("running") => true,
        Some("clean") => false,
        _ => return None,
    };
    Some(Marker {
        running,
        session,
        pid,
        boot,
    })
}

fn render(m: &Marker) -> String {
    let mut s = format!(
        "state={}\nsession={}\n",
        if m.running { "running" } else { "clean" },
        m.session
    );
    if let Some(p) = m.pid {
        let _ = writeln!(s, "pid={p}");
    }
    if let Some(b) = m.boot {
        let _ = writeln!(s, "boot={b}");
    }
    s
}

/// The machine facts the verdict depends on (injected in tests).
pub(crate) trait Machine {
    /// When the computer booted (Unix seconds), if known.
    fn boot_time(&self) -> Option<u64>;
    /// Whether process `pid` is alive.
    fn alive(&self, pid: u32) -> bool;
}

struct RealMachine;

impl Machine for RealMachine {
    fn boot_time(&self) -> Option<u64> {
        let b = sysinfo::System::boot_time();
        (b > 0).then_some(b)
    }

    fn alive(&self, pid: u32) -> bool {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
        let mut sys = System::new();
        let p = Pid::from_u32(pid);
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[p]),
            true,
            ProcessRefreshKind::nothing(),
        );
        sys.process(p).is_some()
    }
}

fn verdict(previous: Option<&Marker>, machine: &dyn Machine, own_pid: u32) -> LastExit {
    let Some(m) = previous else {
        return LastExit::Unknown;
    };
    if !m.running {
        return LastExit::Clean;
    }
    // Still alive (a hand-over in progress): not a crash.
    if m.pid.is_some_and(|p| p != own_pid && machine.alive(p)) {
        return LastExit::Unknown;
    }
    match (m.boot, machine.boot_time()) {
        (Some(then), Some(now)) if then.abs_diff(now) <= BOOT_TOLERANCE_SECS => LastExit::Unclean,
        // Another boot, or a boot time we cannot compare: the computer
        // restarted or shut down with the app open, which is not a crash.
        _ => LastExit::Unknown,
    }
}

fn new_session_id() -> String {
    // Not a secret; only random enough to tell two sessions apart. A
    // clock-and-pid id does that too when the OS has no randomness.
    cl_pg::Secret::generate().map_or_else(
        |_| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            format!("{now:x}-{:x}", std::process::id())
        },
        |s| s.expose().chars().take(32).collect(),
    )
}

/// This session's marker. Dropping it changes nothing: only
/// [`SessionMarker::mark_clean`] says the end was orderly.
#[derive(Debug, Clone)]
pub struct SessionMarker {
    path: PathBuf,
    session: String,
}

/// Read how the previous session ended, then mark this one `running`.
/// Call once, after the single-instance lock is held.
///
/// # Errors
/// Fails when the run folder or the marker cannot be written; the previous
/// verdict is lost then too (the caller treats it as `Unknown`).
pub fn begin_session(paths: &AppPaths) -> Result<(SessionMarker, LastExit)> {
    begin_at(&paths.run_dir(), &RealMachine, std::process::id())
}

pub(crate) fn begin_at(
    run_dir: &Path,
    machine: &dyn Machine,
    own_pid: u32,
) -> Result<(SessionMarker, LastExit)> {
    crate::paths::create_private_dir(run_dir)?;
    let path = run_dir.join(MARKER_FILE);
    let previous = std::fs::read_to_string(&path).ok().and_then(|t| parse(&t));
    let last = verdict(previous.as_ref(), machine, own_pid);
    let marker = SessionMarker {
        path,
        session: new_session_id(),
    };
    marker.write(&Marker {
        running: true,
        session: marker.session.clone(),
        pid: Some(own_pid),
        boot: machine.boot_time(),
    })?;
    Ok((marker, last))
}

impl SessionMarker {
    /// The marker file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Record an orderly end. Idempotent; does nothing when another session
    /// has taken the marker over since. Cheap and safe to call from a signal
    /// handler thread (one small file write).
    ///
    /// # Errors
    /// Fails when the marker cannot be written.
    pub fn mark_clean(&self) -> Result<()> {
        let current = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| parse(&t));
        if current.as_ref().is_some_and(|m| m.session != self.session) {
            return Ok(());
        }
        self.write(&Marker {
            running: false,
            session: self.session.clone(),
            pid: None,
            boot: None,
        })
    }

    fn write(&self, m: &Marker) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, render(m))
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| HostError::io(format!("write {}", self.path.display()), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake {
        boot: Cell<Option<u64>>,
        alive: Cell<bool>,
    }

    impl Machine for Fake {
        fn boot_time(&self) -> Option<u64> {
            self.boot.get()
        }
        fn alive(&self, _pid: u32) -> bool {
            self.alive.get()
        }
    }

    fn fake(boot: u64) -> Fake {
        Fake {
            boot: Cell::new(Some(boot)),
            alive: Cell::new(false),
        }
    }

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-host-marker-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn first_run_is_unknown_then_a_clean_quit_is_clean() {
        let d = dir("clean");
        let m = fake(1_000_000);
        let (s, last) = begin_at(&d, &m, 10).unwrap();
        assert_eq!(last, LastExit::Unknown);
        s.mark_clean().unwrap();
        s.mark_clean().unwrap(); // idempotent
        let (_, last) = begin_at(&d, &m, 11).unwrap();
        assert_eq!(last, LastExit::Clean);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_crash_on_the_same_boot_is_unclean() {
        let d = dir("crash");
        let m = fake(1_000_000);
        let (_s, _) = begin_at(&d, &m, 10).unwrap();
        // No mark_clean: the process died. The OS reports a boot time a
        // second off; still the same boot.
        m.boot.set(Some(1_000_001));
        let (_, last) = begin_at(&d, &m, 11).unwrap();
        assert_eq!(last, LastExit::Unclean);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_restart_or_shutdown_with_the_app_open_is_not_a_crash() {
        let d = dir("reboot");
        let m = fake(1_000_000);
        let (_s, _) = begin_at(&d, &m, 10).unwrap();
        m.boot.set(Some(1_090_000));
        let (_, last) = begin_at(&d, &m, 11).unwrap();
        assert_eq!(last, LastExit::Unknown);
        // Unknown boot time: no claim either.
        let (_s, _) = begin_at(&d, &m, 12).unwrap();
        m.boot.set(None);
        let (_, last) = begin_at(&d, &m, 13).unwrap();
        assert_eq!(last, LastExit::Unknown);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_live_previous_process_is_a_hand_over_and_cannot_mark_us_clean() {
        let d = dir("handover");
        let m = fake(1_000_000);
        let (old, _) = begin_at(&d, &m, 10).unwrap();
        m.alive.set(true);
        let (new, last) = begin_at(&d, &m, 11).unwrap();
        assert_eq!(last, LastExit::Unknown);
        // The old instance quits after the new one started: the marker
        // stays the new session's `running`.
        old.mark_clean().unwrap();
        let text = std::fs::read_to_string(new.path()).unwrap();
        assert!(text.contains("state=running"), "{text}");
        m.alive.set(false);
        let (_, last) = begin_at(&d, &m, 12).unwrap();
        assert_eq!(last, LastExit::Unclean, "the new session never ended");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_garbled_marker_is_unknown() {
        let d = dir("garbled");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(MARKER_FILE), "\u{0}\u{1}nonsense").unwrap();
        let (_, last) = begin_at(&d, &fake(5), 1).unwrap();
        assert_eq!(last, LastExit::Unknown);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_real_machine_knows_this_process_and_a_boot_time() {
        let m = RealMachine;
        assert!(m.alive(std::process::id()));
        assert!(m.boot_time().is_some());
    }
}
