//! Duplicate keys (WP-A, step 11; plan D3 `dupes.rs`): the normalized
//! reference, the total and the date, sent to the engine, which decides
//! (U-92: same file hash; same vendor + reference; same vendor + amount +
//! date within 60 days).
//!
//! **Consumer:** the desktop (WP-F/G); the signatures here are fixed.

use crate::model::Header;
use cl_core::{CalendarDate, Money};
use std::fmt;

/// What the engine's duplicate check needs from a reading.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct DupeKey {
    /// [`normalize_reference`] of the reference.
    pub reference: Option<String>,
    pub total: Option<Money>,
    pub date: Option<CalendarDate>,
}

impl fmt::Debug for DupeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DupeKey")
            .field("reference", &self.reference.is_some())
            .field("total", &self.total.is_some())
            .field("date", &self.date.is_some())
            .finish()
    }
}

/// The key of a reading's header. Stub (step 2): empty.
#[must_use]
pub fn key(header: &Header) -> DupeKey {
    let _ = header;
    DupeKey::default()
}

/// A reference normalized for comparison (case, spaces, punctuation and
/// leading zeros, as the engine compares). Stub (step 2): unchanged.
#[must_use]
pub fn normalize_reference(reference: &str) -> String {
    reference.to_string()
}
