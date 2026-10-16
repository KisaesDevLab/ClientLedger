//! Shared value types for the desktop app.
//!
//! Pure values only: no I/O, no clock, no random source, no internal
//! dependencies. Callers pass "today" and generate ids themselves.
#![forbid(unsafe_code)]

pub mod ids;
pub mod secret;

pub use ids::IdError;
pub use secret::Secret;
