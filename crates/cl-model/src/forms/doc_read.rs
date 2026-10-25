//! The "read from the document" state of a bill's fields (P17, plan §3
//! WP3; non-negotiable 4).
//!
//! # Handoff (WP0 skeleton, WP3 fills this file)
//!
//! WP3 adds `ReadField` {`Read{level}`, `Accepted`, `Edited`},
//! `reconcile(reading) -> ReadCheck {lines_sum, subtotal_tax_ok, total_ok,
//! difference}` (exact `Money` on the document's own figures) and
//! `apply(reading, &mut BillForm)`, which never marks an amount accepted.
//! The UI side (the banner, Read with AI) is `cl-ui`
//! `screens/bill/doc_read.rs` (WP7).
