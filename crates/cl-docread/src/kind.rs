//! Document kind (WP-A, step 9; plan D3 `kind.rs`): PAID, change given,
//! paid = total, or card last 4 → a receipt; a receipt with card last 4 →
//! card charge; "Check #" → check; "Cash" → cash expense; otherwise a bill;
//! conflicting signs → `Unknown` (with a `KindUnclear` check). The card's
//! digits are reported in the header, never an account.
//!
//! Internal to the rules (WP-A may refine).

use crate::model::{Confidence, DocKind, Header, PageWords};
use crate::protocol::JobHint;

/// Decide the kind. Stub (step 2): `Unknown`, no confidence.
#[must_use]
pub fn classify(header: &Header, pages: &[PageWords], hint: JobHint) -> (DocKind, Confidence) {
    let _ = (header, pages, hint);
    (DocKind::Unknown, Confidence::NONE)
}
