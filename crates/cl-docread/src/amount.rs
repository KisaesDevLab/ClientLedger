//! Document amounts (WP-A, step 8; plan D3 `amount.rs`, §4 "Parsed"): an
//! exact port of the engine's `normalizeBillMoney`
//! (`ai-bill-ocr.service.ts:149-183`): `$`, `USD`, US thousands commas,
//! accounting parentheses; refuses ambiguous input such as `12,50`; never
//! rounds. Then `Money::parse_wire`.
//!
//! The **only** parser of document amounts. **Consumers:** the rules, the
//! AI gate, and the desktop (WP-G) — the signature of [`parse`] is fixed.

use cl_core::Money;

/// The amount printed in `text`, or `None` when it is not exactly one
/// unambiguous amount. Stub (step 2): always `None`.
#[must_use]
pub fn parse(text: &str) -> Option<Money> {
    let _ = text;
    None
}
