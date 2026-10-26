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
//! **Provisional values** until step 12a measures the worker (Q-A03c).

/// The worker's own data limit: the host's 1 GiB watchdog × 1.25 (Unix;
/// on Windows the host's job carries the same cap).
#[cfg_attr(not(unix), allow(dead_code))]
pub const DATA_LIMIT_BYTES: u64 = 1_280 * 1024 * 1024;

/// What `apply` managed to set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applied {
    /// Linux only: the network namespace is in place.
    pub network_isolated: bool,
    /// `RLIMIT_DATA` is set.
    pub data_limited: bool,
}

/// Set the limits. Must run before any thread starts (`unshare` of a user
/// namespace fails in a multi-threaded process).
#[must_use]
pub fn apply() -> Applied {
    Applied {
        network_isolated: isolate_network(),
        data_limited: limit_data(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_only_what_was_refused() {
        let all = Applied {
            network_isolated: true,
            data_limited: true,
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
        }
        assert!(none.iter().all(|l| l.starts_with("docread: limits: ")));
    }
}
