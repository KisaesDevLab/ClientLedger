//! Shared value types for the desktop app.
//!
//! Pure values only: no I/O, no clock, no random source, no internal
//! dependencies. Callers pass "today" and generate ids themselves.
//!
//! Money is exact (CLAUDE.md non-negotiable 6): the float lints below are
//! deny-level, `clippy.toml` bans every float conversion of `rust_decimal`,
//! `num-traits` and `serde_json`, and `tests/float_ban.rs` proves each ban
//! fires.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

pub mod ids;
pub mod money;
pub mod secret;

pub use ids::IdError;
pub use money::{
    AllocUnit, Money, MoneyError, MoneyInputError, MoneyStyle, NegativeStyle, Quantity, Rate,
    UserInput,
};
pub use secret::Secret;
