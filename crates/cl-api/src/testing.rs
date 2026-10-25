//! Test-only hooks for the contract harness (cargo feature `test-support`).
//!
//! Never enabled by an app or by a non-dev dependency edge (the xtask guard
//! fails CI otherwise, P05 plan §2.14). Nothing here weakens the typed API:
//! the hooks let tests count requests, lose a reply on purpose (to exercise
//! outcome reconciliation against the live engine), send raw bodies the typed
//! API refuses to build, and age the session's tokens.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use reqwest::header::HeaderMap;
use zeroize::Zeroizing;

use crate::client::{Method, TransportError};
use crate::endpoint::{EndpointSource, EndpointState};

/// What the observer wants done with an engine reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverAction {
    /// Deliver the reply.
    Pass,
    /// Throw the reply away and report a post-write connection failure, as if
    /// the socket broke after the engine answered (an ambiguous outcome).
    DropReply,
}

/// Sees every attempt the transport makes. Hooks must be cheap.
pub trait RequestObserver: Send + Sync {
    /// Called just before an attempt is sent.
    fn before_send(&self, method: Method, route: &str) {
        let _ = (method, route);
    }
    /// Called with the HTTP status of an attempt that got an answer.
    fn on_reply(&self, method: Method, route: &str, status: u16) -> ObserverAction {
        let _ = (method, route, status);
        ObserverAction::Pass
    }
    /// Called for a delivered reply that carries `Idempotent-Replayed:
    /// true` (U-18): the engine answered with an earlier request's result.
    fn on_replayed(&self, method: Method, route: &str) {
        let _ = (method, route);
    }
}

/// Holder for the optional observer inside the transport.
#[derive(Default)]
pub(crate) struct ObserverSlot(RwLock<Option<Arc<dyn RequestObserver>>>);

type Answer = Result<(u16, HeaderMap, Zeroizing<Vec<u8>>), TransportError>;

impl ObserverSlot {
    pub(crate) fn set(&self, observer: Option<Arc<dyn RequestObserver>>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = observer;
    }

    fn get(&self) -> Option<Arc<dyn RequestObserver>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn before_send(&self, method: Method, route: &str) {
        if let Some(o) = self.get() {
            o.before_send(method, route);
        }
    }

    pub(crate) fn after_reply(
        &self,
        method: Method,
        route: &str,
        answer: Answer,
        generation: u64,
    ) -> Answer {
        let Some(o) = self.get() else { return answer };
        match &answer {
            Ok((status, headers, _)) => match o.on_reply(method, route, *status) {
                ObserverAction::Pass => {
                    let replayed = headers
                        .get("idempotent-replayed")
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
                    if replayed {
                        o.on_replayed(method, route);
                    }
                    answer
                }
                ObserverAction::DropReply => Err(TransportError::Ambiguous { generation }),
            },
            Err(_) => answer,
        }
    }
}

impl crate::session::Client {
    /// Install (or remove) a request observer on this client's transport.
    pub fn set_observer(&self, observer: Option<Arc<dyn RequestObserver>>) {
        self.exec().transport().observer.set(observer);
    }
}

/// An [`EndpointSource`] wrapper that counts how often the endpoint was read.
/// cl-api reads it once per attempt, so an unchanged count proves that no
/// request was sent.
pub struct CountingSource {
    inner: Arc<dyn EndpointSource>,
    reads: AtomicU64,
}

impl CountingSource {
    /// Wrap `inner`.
    #[must_use]
    pub fn new(inner: Arc<dyn EndpointSource>) -> CountingSource {
        CountingSource {
            inner,
            reads: AtomicU64::new(0),
        }
    }

    /// Endpoint reads so far.
    #[must_use]
    pub fn reads(&self) -> u64 {
        self.reads.load(Ordering::SeqCst)
    }
}

impl EndpointSource for CountingSource {
    fn current(&self) -> EndpointState {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.current()
    }
}

/// The answer to a raw request: status and JSON body (`Null` when empty or
/// not JSON). Numbers in the body are never read as floats by the harness:
/// tests compare JSON strings only.
pub use crate::session::stub_session;

/// A raw engine reply (status and JSON body) for the contract harness.
#[derive(Debug)]
pub struct RawReply {
    /// HTTP status.
    pub status: u16,
    /// Parsed body.
    pub body: serde_json::Value,
}
