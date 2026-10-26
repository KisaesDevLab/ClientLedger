//! Child process hygiene: no console window, no orphans.
//!
//! * Windows: every engine runs inside a job object marked
//!   "kill on close", so the whole process tree dies when we drop the job,
//!   and also if the host itself crashes (the OS closes our handles).
//! * Unix: the engine gets its own process group; stopping sends `SIGTERM` to
//!   the group, then `SIGKILL` after a grace period.
//! * If the host died without cleaning up (Unix crash, or an engine that
//!   survived), the next start finds the pid recorded in `run/engine.pid` and
//!   kills that process, but only when its executable and start time match
//!   what was recorded, so a recycled pid is never killed.

#[cfg(windows)]
mod win_job_memory;

use crate::paths::AppPaths;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Set up `cmd` so the child gets no console window and can be killed as a tree.
pub(crate) fn prepare(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(not(any(windows, unix)))]
    let _ = cmd;
}

#[cfg(all(windows, test))]
thread_local! {
    /// Unit tests only: make [`TreeKiller::confine`] fail on this thread.
    pub(crate) static FAIL_CONFINE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Owns the means to kill a child and everything it started.
#[derive(Debug)]
pub(crate) struct TreeKiller {
    #[cfg(windows)]
    job: Option<win32job::Job>,
    #[cfg(unix)]
    pgid: i32,
}

impl TreeKiller {
    /// Put `child` under tree-kill control. Call right after spawning.
    pub(crate) fn adopt(child: &Child) -> TreeKiller {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let job = (|| {
                let job = win32job::Job::create()?;
                let mut info = job.query_extended_limit_info()?;
                info.limit_kill_on_job_close();
                job.set_extended_limit_info(&info)?;
                job.assign_process(child.as_raw_handle() as isize)?;
                Ok::<_, win32job::JobError>(job)
            })();
            match job {
                Ok(job) => TreeKiller { job: Some(job) },
                Err(e) => {
                    tracing::warn!("could not create a job object for a child process: {e}");
                    TreeKiller { job: None }
                }
            }
        }
        #[cfg(unix)]
        {
            TreeKiller {
                pgid: i32::try_from(child.id()).unwrap_or(0),
            }
        }
    }

    /// Confine a reading worker or tesseract (A03, Q-A03s; security review
    /// M2, M3): on Windows the job's per-process commit cap of `bytes` (an
    /// allocation past it fails at once instead of waiting for the
    /// watchdog's next poll), one active process, and no crash reporting
    /// (`win_job_memory`). False when there is no job or the limits could
    /// not be set: the caller must then stop the child (fail closed,
    /// review M5).
    ///
    /// Elsewhere the watchdog and the limits the worker sets on itself apply
    /// (and, on Linux, `limit_tesseract`), and this returns false.
    #[cfg_attr(not(windows), allow(clippy::unused_self))]
    pub(crate) fn confine(&self, bytes: u64) -> bool {
        #[cfg(all(windows, test))]
        if FAIL_CONFINE.with(std::cell::Cell::get) {
            return false;
        }
        #[cfg(windows)]
        {
            self.job
                .as_ref()
                .is_some_and(|job| win_job_memory::confine_job(job, bytes))
        }
        #[cfg(not(windows))]
        {
            let _ = bytes;
            false
        }
    }

    /// Stop the child and its descendants, then reap it. On Unix the group
    /// gets `SIGTERM` and up to `grace` to leave; Windows has no equivalent
    /// for a windowless process, so the job is terminated at once.
    pub(crate) fn terminate(&mut self, child: &mut Child, grace: Duration) {
        #[cfg(windows)]
        {
            let _ = grace;
            // Dropping the job closes its last handle, which kills every
            // process in it, including the engine itself.
            self.job = None;
            let _ = child.kill();
            let _ = child.wait();
        }
        #[cfg(unix)]
        {
            use nix::sys::signal::{Signal, killpg};
            use nix::unistd::Pid as NixPid;
            use std::time::Instant;
            if self.pgid > 0 {
                let _ = killpg(NixPid::from_raw(self.pgid), Signal::SIGTERM);
            }
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if self.pgid > 0 {
                // Also removes any descendants that outlived the leader.
                let _ = killpg(NixPid::from_raw(self.pgid), Signal::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Limits on a child the host started without a `pre_exec` hook (Linux,
/// `prlimit`; security review M1, M2, L4): no core file, at most
/// `cpu_secs` of CPU time (`SIGXCPU`, then `SIGKILL` a few seconds later)
/// and at most `data_bytes` of private writable memory. Call before the
/// child is given any input. False when a limit could not be set.
///
/// The child must not have read anything yet: tesseract waits for its
/// page on stdin, which the host writes only after this.
#[cfg(target_os = "linux")]
pub(crate) fn limit_child(pid: u32, cpu_secs: u64, data_bytes: u64) -> bool {
    use rustix::process::{Pid as RPid, Resource, Rlimit, prlimit};
    let Some(pid) = i32::try_from(pid).ok().and_then(RPid::from_raw) else {
        return false;
    };
    let set = |resource, soft: u64, hard: u64| {
        prlimit(
            Some(pid),
            resource,
            Rlimit {
                current: Some(soft),
                maximum: Some(hard),
            },
        )
        .is_ok()
    };
    // Each is tried: one refused limit must not skip the others.
    let core = set(Resource::Core, 0, 0);
    let cpu = set(Resource::Cpu, cpu_secs, cpu_secs + 5);
    let data = set(Resource::Data, data_bytes, data_bytes);
    core && cpu && data
}

/// Seconds since the Unix epoch of a process start, as sysinfo reports it.
pub(crate) fn process_identity(pid: u32) -> Option<(PathBuf, u64)> {
    let mut sys = System::new();
    let pid = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let p = sys.process(pid)?;
    Some((p.exe()?.to_path_buf(), p.start_time()))
}

fn record_path(paths: &AppPaths) -> PathBuf {
    paths.run_dir().join("engine.pid")
}

/// Remember the engine we started so a later run can clean up after a crash.
pub(crate) fn record_child(paths: &AppPaths, exe: &Path, pid: u32) {
    let Some((_, start)) = process_identity(pid) else {
        return;
    };
    let text = format!("{pid}\n{start}\n{}\n", exe.display());
    if let Err(e) = std::fs::write(record_path(paths), text) {
        tracing::warn!("could not record the engine process: {e}");
    }
}

/// Forget the recorded engine (after a clean stop).
pub(crate) fn forget_child(paths: &AppPaths) {
    let _ = std::fs::remove_file(record_path(paths));
}

/// Kill an engine left behind by a previous run that crashed. Returns true
/// when a process was killed.
pub(crate) fn reap_stale_child(paths: &AppPaths, expected_exe: &Path) -> bool {
    let path = record_path(paths);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let _ = std::fs::remove_file(&path);
    let mut lines = text.lines();
    let (Some(pid), Some(start)) = (
        lines.next().and_then(|l| l.parse::<u32>().ok()),
        lines.next().and_then(|l| l.parse::<u64>().ok()),
    ) else {
        return false;
    };
    if pid == std::process::id() {
        return false;
    }
    let Some((exe, started)) = process_identity(pid) else {
        return false;
    };
    // Same program and same start time (to the second): it is our old engine.
    let same_exe = same_file(&exe, expected_exe);
    if !same_exe || started != start {
        return false;
    }
    tracing::warn!(pid, "stopping an engine left over from a previous run");
    kill_process(pid)
}

pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Forcefully end one process (and, on Windows or Unix, nothing else).
pub(crate) fn kill_process(pid: u32) -> bool {
    let mut sys = System::new();
    let p = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[p]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let killed = sys.process(p).is_some_and(sysinfo::Process::kill);
    if killed {
        // Give the OS a moment to release the socket and files.
        for _ in 0..40 {
            if process_identity(pid).is_none() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    killed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> AppPaths {
        let root =
            std::env::temp_dir().join(format!("cl-host-procs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = AppPaths::at(root).unwrap();
        p.ensure_dirs().unwrap();
        p
    }

    #[test]
    fn a_missing_or_garbled_record_kills_nothing() {
        let paths = scratch("garbled");
        let exe = std::env::current_exe().unwrap();
        assert!(!reap_stale_child(&paths, &exe));
        std::fs::write(record_path(&paths), "not a record").unwrap();
        assert!(!reap_stale_child(&paths, &exe));
        assert!(
            !record_path(&paths).exists(),
            "the record is always consumed"
        );
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn our_own_pid_or_a_recycled_pid_is_never_killed() {
        let paths = scratch("recycled");
        let exe = std::env::current_exe().unwrap();
        // Our own process, recorded exactly: must be refused.
        record_child(&paths, &exe, std::process::id());
        assert!(!reap_stale_child(&paths, &exe));
        // Right pid, wrong start time (pid reuse): refused.
        let text = format!("{}\n1\n{}\n", std::process::id(), exe.display());
        std::fs::write(record_path(&paths), text).unwrap();
        assert!(!reap_stale_child(&paths, &exe));
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn a_leftover_child_with_matching_identity_is_killed() {
        // Use a long-lived helper process that is not this test binary.
        let paths = scratch("leftover");
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("60");
            c
        };
        cmd.stdout(std::process::Stdio::null());
        let mut child = cmd.spawn().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        let (exe, _) = process_identity(child.id()).expect("child is visible");
        record_child(&paths, &exe, child.id());
        assert!(reap_stale_child(&paths, &exe));
        // It really ended.
        let ended = (0..60).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            matches!(child.try_wait(), Ok(Some(_)))
        });
        if !ended {
            let _ = child.kill();
        }
        assert!(ended, "the leftover process should have been killed");
        let _ = std::fs::remove_dir_all(paths.root());
    }

    #[test]
    fn terminate_kills_the_child_and_reaps_it() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("60");
            c
        };
        cmd.stdout(std::process::Stdio::null());
        prepare(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let mut killer = TreeKiller::adopt(&child);
        killer.terminate(&mut child, Duration::from_secs(2));
        assert!(matches!(child.try_wait(), Ok(Some(_))));
    }
}
