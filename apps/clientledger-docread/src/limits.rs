//! Limits the worker sets on itself, first thing in `main`, before
//! `PDFium`, threads or stdin (plan D2).
//!
//! * Linux: a best-effort new user + network namespace
//!   (`unshare(CLONE_NEWUSER | CLONE_NEWNET)`: only a downed loopback is
//!   left), refused on some distributions (Ubuntu 24.04's `AppArmor`), which
//!   is reported, not fatal. Then `RLIMIT_DATA` (private writable mappings,
//!   not address-space reservations, review S3).
//! * macOS: `RLIMIT_DATA` attempted (the kernel may not enforce it).
//! * Windows: nothing in-process; `cl-host` puts the worker in a job object
//!   with a hard memory cap and watches its commit.
//!
//! The host's watchdog is the operative limit everywhere; these are the
//! backstop for a spike faster than its poll.
//!
//! After those (security review M1, M2), on Linux and macOS:
//!
//! * `RLIMIT_CORE` 0: a crash inside `PDFium` leaves no core file holding
//!   document content (and `systemd-coredump` stores none);
//! * `RLIMIT_CPU`: a worker the host can no longer stop (the desktop
//!   crashed) ends by itself. The soft limit is one job's CPU budget,
//!   moved forward at the start of each job ([`job_started`]); the kernel
//!   sends `SIGXCPU` (which ends the worker) when a job goes past it, and
//!   `SIGKILL` at the hard limit, a whole worker's budget;
//! * Linux only, **after** `unshare` (a new user namespace changes the
//!   credentials, which clears both settings): not dumpable (no core dump,
//!   no `ptrace` attach by other programs of the same user), and
//!   `SIGKILL` when the host dies (`PR_SET_PDEATHSIG`), then a check that
//!   the host is still the parent, for a host that died first.
//!
//! On Windows the job object does all of this (`cl-host`
//! `procs/win_job_memory.rs`): kill-on-close and no Windows Error Reporting.
//!
//! Values from step 12a (Q-A03c): the worker's worst legitimate peak was
//! 759 MiB of commit (the largest picture it accepts) under a 1 GiB watchdog;
//! the data limit is the watchdog × 1.25. Linux `RLIMIT_DATA` also counts
//! allocator arenas, so the `docread-accuracy` CI job's Linux run is the
//! check that legitimate documents stay under it.

/// The worker's own data limit: the host's 1 GiB watchdog × 1.25 (Unix;
/// on Windows the host's job carries the same cap).
#[cfg_attr(not(unix), allow(dead_code))]
pub const DATA_LIMIT_BYTES: u64 = 1_280 * 1024 * 1024;

/// What `apply` managed to set.
// One flag per independent limit: a state machine would not fit.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// Linux only: the network namespace is in place.
    pub network_isolated: bool,
    /// `RLIMIT_DATA` is set.
    pub data_limited: bool,
    /// `RLIMIT_CPU` is set (Unix).
    pub cpu_limited: bool,
    /// `RLIMIT_CORE` is 0 (Unix).
    pub core_off: bool,
    /// Linux only: not dumpable.
    pub undumpable: bool,
    /// Linux only: the worker gets `SIGKILL` when the host dies.
    pub parent_watched: bool,
    /// The host is already gone: the worker must exit at once.
    pub orphaned: bool,
}

/// Set the limits. Must run before any thread starts (`unshare` of a user
/// namespace fails in a multi-threaded process).
#[must_use]
pub fn apply() -> Applied {
    let mut a = Applied {
        network_isolated: isolate_network(),
        data_limited: limit_data(),
        ..Applied::default()
    };
    a.cpu_limited = limit_cpu();
    a.core_off = no_core_file();
    a.undumpable = not_dumpable();
    (a.parent_watched, a.orphaned) = watch_parent();
    a
}

/// The outcome classes reported on stderr (the host logs them as is: no
/// paths, no content).
#[must_use]
pub fn report(a: Applied) -> Vec<&'static str> {
    let mut out = Vec::new();
    if cfg!(target_os = "linux") && !a.network_isolated {
        out.push("docread: limits: network namespace refused");
    }
    if cfg!(unix) && !a.data_limited {
        out.push("docread: limits: data limit refused");
    }
    if cfg!(unix) && !a.cpu_limited {
        out.push("docread: limits: cpu limit refused");
    }
    if cfg!(unix) && !a.core_off {
        out.push("docread: limits: core limit refused");
    }
    if cfg!(target_os = "linux") && !a.undumpable {
        out.push("docread: limits: dump protection refused");
    }
    if cfg!(target_os = "linux") && !a.parent_watched {
        out.push("docread: limits: parent watch refused");
    }
    if a.orphaned {
        out.push("docread: limits: host gone");
    }
    out
}

#[cfg(target_os = "linux")]
fn isolate_network() -> bool {
    use nix::sched::{CloneFlags, unshare};
    unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn isolate_network() -> bool {
    false
}

#[cfg(unix)]
fn limit_data() -> bool {
    use nix::sys::resource::{Resource, getrlimit, setrlimit};
    let Ok((_, hard)) = getrlimit(Resource::RLIMIT_DATA) else {
        return false;
    };
    // Never raise a lower hard limit someone else set.
    let limit = DATA_LIMIT_BYTES.min(hard);
    setrlimit(Resource::RLIMIT_DATA, limit, limit).is_ok()
}

#[cfg(not(unix))]
fn limit_data() -> bool {
    false
}

/// CPU seconds one job may use: the host's longest job (`cl-host`
/// `DocreadLimits::job_ceiling`, at most 330 s of wall time, so of CPU
/// time on the worker's one thread) plus a margin.
#[cfg_attr(not(unix), allow(dead_code))]
pub const JOB_CPU_SECONDS: u64 = 360;

// The longest job the host allows fits one budget.
const _: () = assert!(JOB_CPU_SECONDS > 330);

/// CPU seconds one worker may use in its life: the host replaces it after
/// 20 jobs (`DocreadLimits::recycle_jobs`), plus start-up.
#[cfg_attr(not(unix), allow(dead_code))]
pub const WORKER_CPU_SECONDS: u64 = 20 * JOB_CPU_SECONDS + 60;

/// The `RLIMIT_CPU` pair for a worker that has used `used` CPU seconds,
/// under an existing hard limit `hard`: one job's budget from now, never
/// past the worker's whole budget or `hard`.
#[must_use]
#[cfg_attr(not(unix), allow(dead_code))]
pub fn cpu_limits(used: u64, hard: u64) -> (u64, u64) {
    let hard = WORKER_CPU_SECONDS.min(hard);
    (used.saturating_add(JOB_CPU_SECONDS).min(hard), hard)
}

#[cfg(unix)]
fn set_cpu_from_now() -> bool {
    use nix::sys::resource::{Resource, UsageWho, getrlimit, getrusage, setrlimit};
    let Ok((_, hard)) = getrlimit(Resource::RLIMIT_CPU) else {
        return false;
    };
    let Ok(usage) = getrusage(UsageWho::RUSAGE_SELF) else {
        return false;
    };
    let secs = |t: nix::sys::time::TimeVal| u64::try_from(t.tv_sec()).unwrap_or(0);
    let used = secs(usage.user_time()) + secs(usage.system_time()) + 1;
    let (soft, hard) = cpu_limits(used, hard);
    setrlimit(Resource::RLIMIT_CPU, soft, hard).is_ok()
}

#[cfg(unix)]
fn limit_cpu() -> bool {
    set_cpu_from_now()
}

#[cfg(not(unix))]
fn limit_cpu() -> bool {
    false
}

/// Give the next job its own CPU budget (see the module comment). Called
/// at the start of each job; a refusal keeps the previous limit.
pub fn job_started() {
    #[cfg(unix)]
    {
        let _ = set_cpu_from_now();
    }
}

#[cfg(unix)]
fn no_core_file() -> bool {
    use nix::sys::resource::{Resource, setrlimit};
    setrlimit(Resource::RLIMIT_CORE, 0, 0).is_ok()
}

#[cfg(not(unix))]
fn no_core_file() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn not_dumpable() -> bool {
    nix::sys::prctl::set_dumpable(false).is_ok()
}

#[cfg(not(target_os = "linux"))]
fn not_dumpable() -> bool {
    false
}

/// The host's process id, as it passed it (`CL_DOCREAD_PARENT`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn expected_parent() -> Option<i32> {
    std::env::var("CL_DOCREAD_PARENT").ok()?.parse().ok()
}

/// Whether a worker whose parent is now `actual` has lost its host:
/// `expected` when the host said who it is, else "re-parented to init".
#[must_use]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn host_gone(expected: Option<i32>, actual: i32) -> bool {
    match expected {
        Some(p) => p != actual,
        None => actual == 1,
    }
}

/// `(watched, orphaned)`. The death signal follows the thread that
/// spawned the worker, not the whole host: `cl-host` starts and stops a
/// worker on the same long-lived thread (`Worker::start`).
#[cfg(target_os = "linux")]
fn watch_parent() -> (bool, bool) {
    use nix::sys::signal::Signal;
    let watched = nix::sys::prctl::set_pdeathsig(Signal::SIGKILL).is_ok();
    // The host may have died before the line above.
    let parent = nix::unistd::getppid().as_raw();
    (watched, host_gone(expected_parent(), parent))
}

#[cfg(not(target_os = "linux"))]
fn watch_parent() -> (bool, bool) {
    (false, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_only_what_was_refused() {
        let all = Applied {
            network_isolated: true,
            data_limited: true,
            cpu_limited: true,
            core_off: true,
            undumpable: true,
            parent_watched: true,
            orphaned: false,
        };
        assert_eq!(report(all), Vec::<&str>::new());
        let none = report(Applied::default());
        if cfg!(windows) {
            assert_eq!(
                none,
                Vec::<&str>::new(),
                "Windows limits live in the host's job"
            );
        } else {
            assert!(none.contains(&"docread: limits: data limit refused"));
            assert!(none.contains(&"docread: limits: cpu limit refused"));
            assert!(none.contains(&"docread: limits: core limit refused"));
        }
        assert!(none.iter().all(|l| l.starts_with("docread: limits: ")));
        let gone = Applied {
            orphaned: true,
            ..all
        };
        assert_eq!(report(gone), ["docread: limits: host gone"]);
    }

    #[test]
    fn each_job_gets_its_own_cpu_budget_within_the_workers() {
        let inf = u64::MAX;
        assert_eq!(
            cpu_limits(1, inf),
            (1 + JOB_CPU_SECONDS, WORKER_CPU_SECONDS)
        );
        assert_eq!(
            cpu_limits(1000, inf),
            (1000 + JOB_CPU_SECONDS, WORKER_CPU_SECONDS)
        );
        // Never past the worker's budget.
        assert_eq!(
            cpu_limits(WORKER_CPU_SECONDS - 10, inf),
            (WORKER_CPU_SECONDS, WORKER_CPU_SECONDS)
        );
        // Never raises a lower hard limit someone else set.
        assert_eq!(cpu_limits(1, 100), (100, 100));
    }

    #[test]
    fn a_worker_whose_host_is_gone_knows_it() {
        assert!(!host_gone(Some(4242), 4242));
        assert!(host_gone(Some(4242), 1));
        assert!(host_gone(Some(4242), 777), "re-parented to a subreaper");
        assert!(host_gone(None, 1));
        assert!(!host_gone(None, 4242));
    }

    /// Harmless in this test process (the CPU limit is not: it would bound
    /// every later test). cl-host's worker tests read a live worker's
    /// limits on Linux (`a_worker_runs_with_its_unix_limits`).
    #[cfg(unix)]
    #[test]
    fn no_core_file_is_allowed_unprivileged() {
        use nix::sys::resource::{Resource, getrlimit};
        assert!(no_core_file());
        assert_eq!(getrlimit(Resource::RLIMIT_CORE).unwrap(), (0, 0));
    }
}
