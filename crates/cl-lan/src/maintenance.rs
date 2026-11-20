//! The proxy's gate while the host is busy (plan D1 step 3, review S4).
//!
//! Engine-level maintenance (U-293) is answered by the engine itself. This
//! gate covers what the engine cannot answer: a file-level restore, a
//! restart, or the engine being down. While it is closed the proxy answers
//! 503 `MAINTENANCE` or `HOST_ENGINE_DOWN` in the engine's error shape and
//! sends nothing to the engine. cl-api never retries a 503 mutation.

use std::sync::{Mutex, PoisonError};

use crate::host::{HostUpstream, Upstream, UpstreamState};

/// Why the gate is closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// A restore or another backup-first operation is running on the host.
    Maintenance,
    /// The engine is starting, restarting or stopped.
    EngineDown,
}

#[derive(Debug, Default)]
struct Inner {
    upstream: Option<Upstream>,
    closed: Option<Closed>,
}

/// A settable [`HostUpstream`]: the host app publishes the engine here
/// (from its supervisor) and closes the gate around restores.
#[derive(Debug, Default)]
pub struct Gate {
    inner: Mutex<Inner>,
}

impl Gate {
    /// Publishes the running engine (or `None` while it is down).
    pub fn set_upstream(&self, up: Option<Upstream>) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .upstream = up;
    }

    /// Closes the gate.
    pub fn close(&self, why: Closed) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = Some(why);
    }

    /// Opens the gate.
    pub fn open(&self) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed = None;
    }

    /// The reason it is closed, if it is.
    #[must_use]
    pub fn closed(&self) -> Option<Closed> {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
    }
}

impl HostUpstream for Gate {
    fn state(&self) -> UpstreamState {
        let g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        match (g.closed, &g.upstream) {
            (Some(Closed::Maintenance), _) => UpstreamState::Maintenance,
            (Some(Closed::EngineDown), _) | (None, None) => UpstreamState::EngineDown,
            (None, Some(u)) => UpstreamState::Ready(u.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Secret;

    #[test]
    fn gate_states() {
        let g = Gate::default();
        assert!(matches!(g.state(), UpstreamState::EngineDown));
        g.set_upstream(Some(Upstream {
            port: 1,
            lan_token: Secret::new("t"),
            generation: 1,
            engine_commit: "c".into(),
        }));
        assert!(matches!(g.state(), UpstreamState::Ready(_)));
        g.close(Closed::Maintenance);
        assert!(matches!(g.state(), UpstreamState::Maintenance));
        assert_eq!(g.closed(), Some(Closed::Maintenance));
        g.open();
        assert!(matches!(g.state(), UpstreamState::Ready(_)));
    }
}
