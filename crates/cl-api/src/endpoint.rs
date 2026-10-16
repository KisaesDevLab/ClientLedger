//! Where the engine is right now: port, launch token and generation.
//!
//! cl-api never caches these (Q-P04n, P05 plan §2.2). Every attempt calls
//! [`EndpointSource::current`] again, so a restarted engine (new port, new
//! token, new generation) is picked up by the next attempt. The host side
//! implements the trait over `cl_host::Supervisor::snapshot()`; the
//! supervisor publishes an endpoint only after it has verified that the
//! listener on that port is its own engine process.

use std::fmt;
use std::sync::{Mutex, PoisonError};

use cl_core::Secret;

/// Supplies the current engine endpoint. Implemented host-side (cl-ui wraps
/// the supervisor; the contract harness has its own adapter).
pub trait EndpointSource: Send + Sync + 'static {
    /// Cheap and non-blocking (well under a millisecond). Called for every
    /// attempt and never cached by cl-api.
    fn current(&self) -> EndpointState;
}

/// The engine's availability as the host sees it.
///
/// Mapping from the supervisor snapshot (P05 plan §2.2, S2):
/// `(Running, Some(ep))` → [`EndpointState::Ready`]; `(Running, None)` →
/// [`EndpointState::Restarting`] (a torn or mid-transition read; the caller
/// waits); `Starting` → `Starting`; `Restarting` → `Restarting`; `Fault` and
/// `Stopped` → `Down { permanent: true }`.
#[derive(Clone, Debug)]
pub enum EndpointState {
    /// The engine answers on this endpoint.
    Ready(EngineEndpoint),
    /// First start in progress; callers wait.
    Starting,
    /// A restart is in progress; callers wait.
    Restarting,
    /// The engine is not running. `permanent` means it will not come back
    /// without the owner (a host fault or an explicit stop).
    Down {
        /// No automatic recovery is coming.
        permanent: bool,
    },
}

/// One engine process: its loopback port, launch token and generation.
#[derive(Clone)]
pub struct EngineEndpoint {
    /// Port on `127.0.0.1`.
    pub port: u16,
    /// Value for the `X-CL-Token` header.
    pub token: Secret,
    /// Increases by one each time the engine process is (re)started.
    pub generation: u64,
}

impl fmt::Debug for EngineEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineEndpoint")
            .field("port", &self.port)
            .field("token", &"***")
            .field("generation", &self.generation)
            .finish()
    }
}

/// A settable endpoint for stub tests and simple tools.
#[derive(Debug)]
pub struct FixedEndpoint {
    state: Mutex<EndpointState>,
}

impl FixedEndpoint {
    /// A ready endpoint.
    #[must_use]
    pub fn new(port: u16, token: Secret, generation: u64) -> FixedEndpoint {
        FixedEndpoint {
            state: Mutex::new(EndpointState::Ready(EngineEndpoint {
                port,
                token,
                generation,
            })),
        }
    }

    /// Start in any state.
    #[must_use]
    pub fn with_state(state: EndpointState) -> FixedEndpoint {
        FixedEndpoint {
            state: Mutex::new(state),
        }
    }

    /// Replace the state (for example a new generation after a restart).
    pub fn set(&self, state: EndpointState) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = state;
    }
}

impl EndpointSource for FixedEndpoint {
    fn current(&self) -> EndpointState {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_shows_the_token() {
        let src = FixedEndpoint::new(4100, Secret::new("tok-canary-123"), 3);
        let text = format!("{:?} {:?}", src.current(), src);
        assert!(!text.contains("tok-canary-123"), "{text}");
        assert!(text.contains("4100"));
    }

    #[test]
    fn set_replaces_the_state() {
        let src = FixedEndpoint::with_state(EndpointState::Starting);
        assert!(matches!(src.current(), EndpointState::Starting));
        src.set(EndpointState::Down { permanent: true });
        assert!(matches!(
            src.current(),
            EndpointState::Down { permanent: true }
        ));
    }
}
