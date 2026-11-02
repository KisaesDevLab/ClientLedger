//! Table statistics after a company is created, imported or restored
//! (Q-335). With no statistics yet, `PostgreSQL` can pick report plans that
//! take minutes instead of milliseconds until autovacuum catches up
//! (U-155), so each of those flows asks for a refresh at its end.
//!
//! This crate never sees the database: the caller supplies the refresh
//! (the desktop app wraps `cl_host::Supervisor::refresh_statistics`). A
//! failed refresh is logged and never fails the owner's create, import or
//! restore.

use std::sync::Arc;

type Refresh = dyn Fn() -> Result<(), String> + Send + Sync;

/// How to refresh the database's table statistics, or nothing.
#[derive(Clone, Default)]
pub struct Statistics(Option<Arc<Refresh>>);

impl Statistics {
    /// Refresh with `f` (blocking; run off the async threads).
    pub fn new(f: impl Fn() -> Result<(), String> + Send + Sync + 'static) -> Statistics {
        Statistics(Some(Arc::new(f)))
    }

    /// No refresh (tests, the demo backend, no local database).
    #[must_use]
    pub fn none() -> Statistics {
        Statistics(None)
    }

    /// Runs the refresh on a blocking thread. Never fails: an error is
    /// logged at warn (it carries the database's message, never company
    /// names or amounts) and the flow goes on.
    pub(crate) async fn refresh(&self) {
        let Some(f) = self.0.clone() else { return };
        match tokio::task::spawn_blocking(move || f()).await {
            Ok(Ok(())) => tracing::debug!("table statistics refreshed"),
            Ok(Err(e)) => tracing::warn!(error = %e, "could not refresh table statistics"),
            Err(_) => tracing::warn!("table statistics refresh stopped"),
        }
    }
}

impl std::fmt::Debug for Statistics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Statistics")
            .field(&self.0.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_refresh_runs_once_and_a_failure_is_swallowed() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&calls);
        let ok = Statistics::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        rt().block_on(ok.refresh());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let failing = Statistics::new(|| Err("server went away".into()));
        rt().block_on(failing.refresh());
        let panicking = Statistics::new(|| panic!("refresher bug"));
        rt().block_on(panicking.refresh());
        rt().block_on(Statistics::none().refresh());
    }
}
