//! Bank-file deposits waiting for an account and the engine's suggested
//! groupings of pending payments that add up to one
//! (`GET /banking/feed/{id}/deposit-groupings`, `feed-deposit-groupings-v1`;
//! P14 lane A).

use crate::session::Session;

/// Bank feed readers of a session (`session.feed()`).
#[derive(Clone, Copy, Debug)]
pub struct Feed<'a>(
    #[allow(dead_code, reason = "P14 lane A adds the calls")] pub(crate) &'a Session,
);

impl Session {
    /// The bank feed of the open company.
    #[must_use]
    pub fn feed(&self) -> Feed<'_> {
        Feed(self)
    }
}
