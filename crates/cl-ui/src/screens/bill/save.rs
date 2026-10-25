//! Saving a bill or vendor credit (P17, plan §3 WP4, §5).
//!
//! # Handoff (WP0 skeleton, WP4 fills this file)
//!
//! Draft building field by field from the checked form, the PR-1
//! unknown-save block with the bill probe (D2, Q-PR1a/b), the closing-date
//! prompt (`forms::closing_prompt`), 409 `STALE_DOCUMENT`, and the partial
//! failures (bill saved, attach-draft failed). Publishes
//! `data_bus::Change::Bills(vendor)` after every save, void or delete.
