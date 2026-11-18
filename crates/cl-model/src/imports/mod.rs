//! The import wizards' rules (P31), with no egui and no I/O.
//!
//! - [`wizard`]: the source picker, each source's steps and file slots
//!   (report name, where to export it, kind, role, required), the Q-142
//!   history dates and `can_continue`.
//! - [`history`]: the History list's rows and actions, and the backups that
//!   must never be pruned while a batch still needs them.
//! - [`tieout`]: the tie-out view (rows, differences under a cent shown
//!   exactly, ✓ only on the engine's `allZero`).
//! - [`spot_check`]: the guided spot-check list (Q-144) and its seeded
//!   sample of transactions.
//!
//! Nothing here adds an amount up: the engine computes every balance and
//! difference (non-negotiable 7). Text is named by stable keys the UI maps
//! to its catalog.

// No floating point anywhere near imported amounts (plan §3).
#![deny(clippy::float_arithmetic, clippy::float_cmp)]

pub mod history;
pub mod spot_check;
pub mod tieout;
pub mod wizard;
