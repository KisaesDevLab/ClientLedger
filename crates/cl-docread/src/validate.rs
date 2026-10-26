//! Reconciliation checks (WP-A, step 10; plan D3 `validate.rs`): lines =
//! subtotal; subtotal + tax (+ labelled fees) = total; total − paid =
//! balance; due date = date + terms. Each problem becomes a `Check` with
//! expected, found and difference. Checks never change a value (AC3).
//!
//! Internal to the rules (WP-A may refine).

use crate::model::{Check, Header, Line};

/// Check the reading's numbers against each other. Stub (step 2): no
/// checks.
#[must_use]
pub fn check(header: &Header, lines: &[Line]) -> Vec<Check> {
    let _ = (header, lines);
    Vec::new()
}
