//! Glue between the supervised engine and the rest of the UI.
//!
//! [`VerifiedEndpoint`] is the production `cl_api::EndpointSource` (the P05
//! follow-up L3b, Q-P05ad item 6). `cl-api` may not depend on `cl-host`, so the
//! adapter lives here. Before it hands out the launch token it asks the
//! operating system who owns the engine's port
//! ([`cl_host::verify_listener`]): another program that grabbed the port after
//! the engine died must never receive the token or a login. `cl-api` re-reads
//! the endpoint for every attempt, so this check covers login, TFA verify,
//! switch-tenant and refresh as well as ordinary calls, and it runs again as
//! soon as the engine restarts (new generation). A successful check is
//! remembered for a short time for the same port, process and generation, so
//! a burst of requests does not list sockets for each one.
//!
//! [`SupervisorProbe`] feeds the status bar.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use cl_api::endpoint::{EndpointSource, EndpointState, EngineEndpoint};
use cl_core::Secret;
use cl_host::{HealthError, HostConfig, HostSnapshot, HostState, InstanceGuard, Supervisor};

use crate::i18n::t;
use crate::shell::{HostProbe, HostView};

/// How long a successful listener check is trusted for one engine generation.
pub const VERIFY_TTL: Duration = Duration::from_secs(2);

/// Where a host snapshot comes from (the supervisor; a fake in tests).
pub trait SnapshotSource: Send + Sync + 'static {
    /// One consistent reading of the host.
    fn snapshot(&self) -> HostSnapshot;
}

impl SnapshotSource for Supervisor {
    fn snapshot(&self) -> HostSnapshot {
        Supervisor::snapshot(self)
    }
}

/// Checks that exactly the engine process we started owns `port`.
pub type Verifier = dyn Fn(u16, u32) -> Result<(), HealthError> + Send + Sync;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Checked {
    port: u16,
    pid: u32,
    generation: u64,
    at: Instant,
}

/// The production endpoint source: supervisor snapshot plus listener check.
pub struct VerifiedEndpoint {
    source: Arc<dyn SnapshotSource>,
    verify: Box<Verifier>,
    ttl: Duration,
    checked: Mutex<Option<Checked>>,
    /// Generation for which an unreadable socket table was already reported.
    warned: Mutex<Option<u64>>,
}

impl VerifiedEndpoint {
    /// Use the real listener check.
    #[must_use]
    pub fn new(source: Arc<dyn SnapshotSource>) -> VerifiedEndpoint {
        VerifiedEndpoint::with_verifier(source, Box::new(cl_host::verify_listener), VERIFY_TTL)
    }

    /// Supply the check and its time-to-live (tests).
    #[must_use]
    pub fn with_verifier(
        source: Arc<dyn SnapshotSource>,
        verify: Box<Verifier>,
        ttl: Duration,
    ) -> VerifiedEndpoint {
        VerifiedEndpoint {
            source,
            verify,
            ttl,
            checked: Mutex::new(None),
            warned: Mutex::new(None),
        }
    }

    fn already_checked(&self, port: u16, pid: u32, generation: u64) -> bool {
        let guard = self.checked.lock().unwrap_or_else(PoisonError::into_inner);
        guard.is_some_and(|c| {
            c.port == port
                && c.pid == pid
                && c.generation == generation
                && c.at.elapsed() < self.ttl
        })
    }

    /// Say once per engine generation that the socket list could not be read,
    /// so a silently skipped check is visible in the log. Returns whether it
    /// warned this time.
    fn warn_unreadable(&self, generation: u64) -> bool {
        let mut w = self.warned.lock().unwrap_or_else(PoisonError::into_inner);
        if *w == Some(generation) {
            return false;
        }
        *w = Some(generation);
        tracing::warn!(
            generation,
            "could not read the socket list; the engine port owner was not verified"
        );
        true
    }

    fn remember(&self, c: Option<Checked>) {
        *self.checked.lock().unwrap_or_else(PoisonError::into_inner) = c;
    }
}

impl EndpointSource for VerifiedEndpoint {
    fn current(&self) -> EndpointState {
        let snap = self.source.snapshot();
        match &snap.status.state {
            HostState::Starting => return EndpointState::Starting,
            HostState::Restarting { .. } => return EndpointState::Restarting,
            HostState::Fault(_) | HostState::Stopping | HostState::Stopped => {
                self.remember(None);
                return EndpointState::Down { permanent: true };
            }
            HostState::Running => {}
        }
        let (Some(ep), Some(pid)) = (snap.endpoint, snap.status.engine_pid) else {
            // Running without an endpoint is a transition caught half-way.
            self.remember(None);
            return EndpointState::Restarting;
        };
        if !self.already_checked(ep.port, pid, ep.generation) {
            match (self.verify)(ep.port, pid) {
                Err(HealthError::NotOurProcess) => {
                    tracing::error!(
                        port = ep.port,
                        "another program holds the engine's port; refusing to send the launch token"
                    );
                    self.remember(None);
                    return EndpointState::Down { permanent: true };
                }
                // A socket listing that cannot be read does not block, as in
                // the supervisor (Q-P04m): the launch token and the health
                // check still bind the engine.
                Ok(()) => self.remember(Some(Checked {
                    port: ep.port,
                    pid,
                    generation: ep.generation,
                    at: Instant::now(),
                })),
                Err(_) => {
                    self.warn_unreadable(ep.generation);
                    self.remember(Some(Checked {
                        port: ep.port,
                        pid,
                        generation: ep.generation,
                        at: Instant::now(),
                    }));
                }
            }
        }
        EndpointState::Ready(EngineEndpoint {
            port: ep.port,
            token: Secret::new(ep.token.expose()),
            generation: ep.generation,
        })
    }
}

/// Owner text for a host fault: a catalog sentence for the component, then
/// the host's reason with anything recognisable scrubbed. Log paths and
/// output lines are not shown.
#[must_use]
pub fn fault_text(f: &cl_host::HostFault) -> String {
    let key = match f.component {
        cl_host::Component::Database => "status.fault.database",
        cl_host::Component::Engine => "status.fault.engine",
    };
    let reason = crate::errors::scrub_line(&f.reason);
    if reason.is_empty() {
        t(key)
    } else {
        format!("{} ({reason})", t(key))
    }
}

/// Status-bar view of a running supervisor.
pub struct SupervisorProbe(pub Arc<dyn SnapshotSource>);

impl HostProbe for SupervisorProbe {
    fn view(&self) -> HostView {
        match self.0.snapshot().status.state {
            HostState::Starting => HostView::Starting,
            HostState::Running => HostView::Running,
            HostState::Restarting { .. } => HostView::Restarting,
            HostState::Fault(f) => HostView::Failed(fault_text(&f)),
            HostState::Stopping | HostState::Stopped => HostView::Stopped,
        }
    }
}

/// The running host as the app uses it: status for the bar, the verified
/// endpoint for `cl-api`, and the flags a second launch or `host stop` sets.
pub struct HostLink {
    supervisor: Arc<Supervisor>,
    endpoint: Arc<VerifiedEndpoint>,
    /// A second launch asked for the window.
    pub focus: Arc<AtomicBool>,
    /// `host stop` asked the app to quit.
    pub stop: Arc<AtomicBool>,
}

impl HostLink {
    /// Start the supervisor in the background and begin relaying the
    /// single-instance requests. Returns at once; the status bar shows
    /// "starting" until both programs are healthy.
    #[must_use]
    pub fn start(config: HostConfig, guard: InstanceGuard) -> HostLink {
        let supervisor = Arc::new(Supervisor::spawn(config, guard));
        let endpoint = Arc::new(VerifiedEndpoint::new(
            Arc::clone(&supervisor) as Arc<dyn SnapshotSource>
        ));
        let focus = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(&supervisor);
        let (f, s) = (Arc::clone(&focus), Arc::clone(&stop));
        let spawned = std::thread::Builder::new()
            .name("cl-instance-relay".into())
            .spawn(move || {
                // Ends by itself once the supervisor is gone.
                while let Some(sup) = weak.upgrade() {
                    let guard = sup.instance();
                    if guard.take_focus_request() {
                        f.store(true, Ordering::SeqCst);
                    }
                    if guard.stop_requested() {
                        s.store(true, Ordering::SeqCst);
                    }
                    guard.set_status(&sup.status().summary());
                    drop(sup);
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "could not start the second-launch relay");
        }
        HostLink {
            supervisor,
            endpoint,
            focus,
            stop,
        }
    }

    /// Status for the status bar.
    #[must_use]
    pub fn probe(&self) -> Arc<dyn HostProbe> {
        Arc::new(SupervisorProbe(
            Arc::clone(&self.supervisor) as Arc<dyn SnapshotSource>
        ))
    }

    /// The endpoint source to give `cl_api::Client::new`.
    #[must_use]
    pub fn endpoint_source(&self) -> Arc<dyn EndpointSource> {
        Arc::clone(&self.endpoint) as Arc<dyn EndpointSource>
    }

    /// The supervisor, for host operations (initialize, info).
    #[must_use]
    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }
}

/// A fixed status line when there is no supervisor to ask (the engine program
/// was not found, for example).
pub struct StaticProbe(pub HostView);

impl HostProbe for StaticProbe {
    fn view(&self) -> HostView {
        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_host::{Component, Endpoint, HostFault, HostStatus, RestartCounts};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn status(state: HostState, pid: Option<u32>, generation: u64) -> HostStatus {
        HostStatus {
            state,
            database_pid: Some(1),
            engine_pid: pid,
            restarts: RestartCounts::default(),
            generation,
            notices: Vec::new(),
            engine_commit: None,
            schema_version: None,
        }
    }

    fn running(port: u16, pid: u32, generation: u64) -> HostSnapshot {
        HostSnapshot {
            status: status(HostState::Running, Some(pid), generation),
            endpoint: Some(Endpoint {
                port,
                token: cl_host::Secret::new("tok-canary-9"),
                generation,
            }),
            generation,
        }
    }

    struct Fake(Mutex<HostSnapshot>);
    impl Fake {
        fn set(&self, s: HostSnapshot) {
            *self.0.lock().unwrap() = s;
        }
    }
    impl SnapshotSource for Fake {
        fn snapshot(&self) -> HostSnapshot {
            self.0.lock().unwrap().clone()
        }
    }

    fn adapter(
        snap: HostSnapshot,
        ttl: Duration,
        result: fn() -> Result<(), HealthError>,
    ) -> (Arc<Fake>, VerifiedEndpoint, Arc<AtomicU32>) {
        let fake = Arc::new(Fake(Mutex::new(snap)));
        let calls = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&calls);
        let ep = VerifiedEndpoint::with_verifier(
            Arc::clone(&fake) as Arc<dyn SnapshotSource>,
            Box::new(move |_, _| {
                c.fetch_add(1, Ordering::SeqCst);
                result()
            }),
            ttl,
        );
        (fake, ep, calls)
    }

    #[test]
    fn a_verified_listener_gives_a_ready_endpoint_with_the_token() {
        let (_f, ep, calls) = adapter(running(4100, 77, 3), VERIFY_TTL, || Ok(()));
        let EndpointState::Ready(e) = ep.current() else {
            panic!("expected ready");
        };
        assert_eq!((e.port, e.generation), (4100, 3));
        assert_eq!(e.token.expose(), "tok-canary-9");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_check_runs_before_the_first_use_and_is_remembered_briefly() {
        let (_f, ep, calls) = adapter(running(4100, 77, 3), Duration::from_mins(1), || Ok(()));
        for _ in 0..5 {
            assert!(matches!(ep.current(), EndpointState::Ready(_)));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one check for a burst");
    }

    #[test]
    fn a_zero_ttl_checks_every_time() {
        let (_f, ep, calls) = adapter(running(4100, 77, 3), Duration::ZERO, || Ok(()));
        for _ in 0..3 {
            let _ = ep.current();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_restart_is_checked_again_even_inside_the_ttl() {
        let (f, ep, calls) = adapter(running(4100, 77, 3), Duration::from_mins(1), || Ok(()));
        let _ = ep.current();
        f.set(running(4100, 78, 4)); // same port, new process and generation
        let _ = ep.current();
        f.set(running(4200, 78, 4)); // new port
        let _ = ep.current();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_foreign_listener_gets_neither_token_nor_requests() {
        let (_f, ep, _c) = adapter(running(4100, 77, 3), Duration::ZERO, || {
            Err(HealthError::NotOurProcess)
        });
        let state = ep.current();
        assert!(
            matches!(state, EndpointState::Down { permanent: true }),
            "{state:?}"
        );
        assert!(!format!("{state:?}").contains("tok-canary"));
    }

    #[test]
    fn a_foreign_listener_is_not_remembered_as_checked() {
        let (f, ep, calls) = adapter(running(4100, 77, 3), Duration::from_mins(1), || {
            Err(HealthError::NotOurProcess)
        });
        let _ = ep.current();
        let _ = ep.current();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        f.set(running(4100, 77, 3));
        assert!(matches!(ep.current(), EndpointState::Down { .. }));
    }

    #[test]
    fn an_unreadable_socket_table_does_not_block_like_the_supervisor() {
        let (_f, ep, _c) = adapter(running(4100, 77, 3), Duration::ZERO, || {
            Err(HealthError::Unreachable("no /proc".into()))
        });
        assert!(matches!(ep.current(), EndpointState::Ready(_)));
    }

    #[test]
    fn an_unreadable_socket_table_is_reported_once_per_generation() {
        let (f, ep, calls) = adapter(running(4100, 77, 3), Duration::ZERO, || {
            Err(HealthError::Unreachable("no /proc".into()))
        });
        for _ in 0..4 {
            let _ = ep.current();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 4, "still checked each time");
        assert!(!ep.warn_unreadable(3), "generation 3 was already reported");
        assert!(ep.warn_unreadable(4), "a new generation is reported again");
        f.set(running(4100, 77, 5));
        let _ = ep.current();
        assert!(!ep.warn_unreadable(5));
    }

    #[test]
    fn host_states_map_to_endpoint_states() {
        let (f, ep, calls) = adapter(running(4100, 77, 3), Duration::ZERO, || Ok(()));
        let snap = |state: HostState| HostSnapshot {
            status: status(state, None, 3),
            endpoint: None,
            generation: 3,
        };
        f.set(snap(HostState::Starting));
        assert!(matches!(ep.current(), EndpointState::Starting));
        f.set(snap(HostState::Restarting {
            component: Component::Engine,
            attempt: 1,
        }));
        assert!(matches!(ep.current(), EndpointState::Restarting));
        for s in [
            HostState::Stopped,
            HostState::Stopping,
            HostState::Fault(HostFault {
                component: Component::Database,
                reason: "x".into(),
                restarts: 3,
                log_paths: vec![],
                last_output: vec![],
            }),
        ] {
            f.set(snap(s));
            assert!(matches!(
                ep.current(),
                EndpointState::Down { permanent: true }
            ));
        }
        // Running with no endpoint is a transition: wait, do not verify.
        f.set(snap(HostState::Running));
        assert!(matches!(ep.current(), EndpointState::Restarting));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "nothing to verify");
    }

    #[test]
    fn the_real_listener_check_accepts_us_and_rejects_a_stranger() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let me = std::process::id();
        let (_f, ep, _c) = {
            let fake = Arc::new(Fake(Mutex::new(running(port, me, 1))));
            (
                Arc::clone(&fake),
                VerifiedEndpoint::with_verifier(
                    fake as Arc<dyn SnapshotSource>,
                    Box::new(cl_host::verify_listener),
                    Duration::ZERO,
                ),
                (),
            )
        };
        assert!(
            matches!(ep.current(), EndpointState::Ready(_)),
            "this process owns the port"
        );
        let fake = Arc::new(Fake(Mutex::new(running(port, me.wrapping_add(1), 1))));
        let stranger = VerifiedEndpoint::with_verifier(
            fake as Arc<dyn SnapshotSource>,
            Box::new(cl_host::verify_listener),
            Duration::ZERO,
        );
        assert!(
            matches!(stranger.current(), EndpointState::Down { permanent: true }),
            "a different pid must be refused"
        );
        drop(listener);
    }

    #[test]
    fn fault_text_is_translated_and_scrubbed() {
        let f = cl_host::HostFault {
            component: Component::Database,
            reason: "failed for kurt@example.com amount 12.50".into(),
            restarts: 3,
            log_paths: vec![std::path::PathBuf::from("C:/Users/Kurt/logs")],
            last_output: vec!["secret=abc".into()],
        };
        let text = fault_text(&f);
        assert!(text.starts_with("the database stopped working"), "{text}");
        assert!(!text.contains("kurt@") && !text.contains("12.50") && !text.contains("Kurt"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn the_status_bar_probe_maps_every_state() {
        let probe = |state: HostState| {
            let fake = Arc::new(Fake(Mutex::new(HostSnapshot {
                status: status(state, None, 1),
                endpoint: None,
                generation: 1,
            })));
            SupervisorProbe(fake as Arc<dyn SnapshotSource>).view()
        };
        assert_eq!(probe(HostState::Starting), HostView::Starting);
        assert_eq!(probe(HostState::Running), HostView::Running);
        assert_eq!(probe(HostState::Stopped), HostView::Stopped);
        assert_eq!(
            probe(HostState::Restarting {
                component: Component::Database,
                attempt: 2
            }),
            HostView::Restarting
        );
        let HostView::Failed(reason) = probe(HostState::Fault(HostFault {
            component: Component::Engine,
            reason: "crashed".into(),
            restarts: 3,
            log_paths: vec![],
            last_output: vec![],
        })) else {
            panic!("expected failed");
        };
        assert!(reason.contains("engine") && reason.contains("crashed"));
        assert!(
            !reason.contains("status.fault"),
            "the key must be translated"
        );
    }
}
