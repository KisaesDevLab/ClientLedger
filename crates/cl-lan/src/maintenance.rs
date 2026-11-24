//! The proxy's view of the engine (plan D1 step 3, review S4, M9).
//!
//! The host app publishes the running engine here from its supervisor;
//! while none is published (starting, restarting, stopped) the proxy
//! answers 503 `HOST_ENGINE_DOWN` in the engine's error shape and sends
//! nothing on. cl-api never retries a 503 mutation.
//!
//! Maintenance is the engine's own (U-293, `maintenance-v1`): during a
//! hold it answers 503 `MAINTENANCE` itself. Review M9: the proxy-side
//! maintenance gate this module used to have had no caller (the app never
//! restores at the file level; restores go through the engine's archive
//! import under the engine's hold), so it was removed rather than kept as
//! untested surface. A stale hold is cleared by the supervisor at every
//! engine start (review B1).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crate::host::{HostUpstream, LanMcp, Upstream, UpstreamState};

/// A settable [`HostUpstream`]: the host app publishes the engine here
/// (from its supervisor).
#[derive(Default)]
pub struct Gate {
    upstream: Mutex<Option<Upstream>>,
    /// The host app holds the engine's maintenance (restore, import): the
    /// workstations' hello says so, so they show the maintenance banner
    /// (QA-9). Nothing is refused here; the engine refuses itself.
    maintenance: AtomicBool,
    /// The host app's MCP bridge for paired workstations (A11), once it
    /// serves one.
    mcp: Mutex<Option<Arc<dyn LanMcp>>>,
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gate")
            .field("upstream", &self.upstream)
            .field("maintenance", &self.maintenance)
            .field(
                "mcp",
                &self
                    .mcp
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .is_some(),
            )
            .finish()
    }
}

impl Gate {
    /// Serves (or stops serving) MCP to paired workstations (A11).
    pub fn set_mcp(&self, mcp: Option<Arc<dyn LanMcp>>) {
        *self.mcp.lock().unwrap_or_else(PoisonError::into_inner) = mcp;
    }

    /// Publishes the running engine (or `None` while it is down).
    pub fn set_upstream(&self, up: Option<Upstream>) {
        *self.upstream.lock().unwrap_or_else(PoisonError::into_inner) = up;
    }

    /// The host app started (`true`) or ended a maintenance hold.
    pub fn set_maintenance(&self, on: bool) {
        self.maintenance.store(on, Ordering::SeqCst);
    }
}

impl HostUpstream for Gate {
    fn state(&self) -> UpstreamState {
        match &*self.upstream.lock().unwrap_or_else(PoisonError::into_inner) {
            Some(u) => UpstreamState::Ready(u.clone()),
            None => UpstreamState::EngineDown,
        }
    }

    fn maintenance(&self) -> bool {
        self.maintenance.load(Ordering::SeqCst)
    }

    fn mcp(&self) -> Option<Arc<dyn LanMcp>> {
        self.mcp
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
        g.set_upstream(None);
        assert!(matches!(g.state(), UpstreamState::EngineDown));
        assert!(!g.maintenance());
        g.set_maintenance(true);
        assert!(g.maintenance());
        g.set_maintenance(false);
        assert!(!g.maintenance());
    }
}
