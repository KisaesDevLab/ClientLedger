//! Proof that every `disallowed-methods` entry in `crates/cl-grid/clippy.toml`
//! resolves and fires (P10 plan Rev 2 N7, as cl-core's P05 R19: a path that does not resolve
//! would silently enforce nothing).
//!
//! Each function calls one banned method under
//! `#[expect(clippy::disallowed_methods)]`. If an entry stopped resolving
//! (a renamed path, a crate update), the expectation would be unfulfilled and
//! `cargo clippy --all-targets -D warnings` (part of `cargo xtask ci`) fails.
//! The functions also run as ordinary tests so they are not dead code.
//!
//! This file lives outside `src/`: the calls are the one sanctioned use of
//! decimal/float conversions in this crate.
#![allow(clippy::float_cmp, clippy::float_arithmetic)]

use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_from_f32_retain() -> Option<Decimal> {
    Decimal::from_f32_retain(1.5)
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_from_f64_retain() -> Option<Decimal> {
    Decimal::from_f64_retain(1.5)
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_as_f64() -> f64 {
    Decimal::ONE.as_f64()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn from_primitive_f32() -> Option<Decimal> {
    Decimal::from_f32(1.5)
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn from_primitive_f64() -> Option<Decimal> {
    Decimal::from_f64(1.5)
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn to_primitive_f32() -> Option<f32> {
    Decimal::ONE.to_f32()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn to_primitive_f64() -> Option<f64> {
    Decimal::ONE.to_f64()
}

#[test]
fn every_banned_call_still_compiles() {
    assert!(decimal_from_f32_retain().is_some());
    assert!(decimal_from_f64_retain().is_some());
    assert!(decimal_as_f64() > 0.0);
    assert!(from_primitive_f32().is_some());
    assert!(from_primitive_f64().is_some());
    assert!(to_primitive_f32().is_some());
    assert!(to_primitive_f64().is_some());
}
