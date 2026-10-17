//! Proof that every `disallowed-methods` entry in `crates/cl-core/clippy.toml`
//! resolves and fires (P05 plan R19: a path that does not resolve would
//! silently enforce nothing).
//!
//! Each function calls one banned method under
//! `#[expect(clippy::disallowed_methods)]`. If an entry stopped resolving
//! (a renamed path, a crate update), the expectation would be unfulfilled and
//! `cargo clippy --all-targets -D warnings` (part of `cargo xtask ci`) fails.
//! The functions also run as ordinary tests so they are not dead code.
//!
//! This file lives outside `src/` on purpose: the xtask `nofloat` scan covers
//! `src/**` only, and these calls are the one sanctioned use of floats here.
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

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn json_number_as_f64() -> Option<f64> {
    serde_json::Number::from(1u8).as_f64()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn json_number_from_f64() -> Option<serde_json::Number> {
    serde_json::Number::from_f64(1.5)
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn json_value_as_f64() -> Option<f64> {
    serde_json::Value::from(1u8).as_f64()
}

#[test]
fn every_banned_method_is_reachable() {
    assert!(decimal_from_f32_retain().is_some());
    assert!(decimal_from_f64_retain().is_some());
    assert_eq!(decimal_as_f64(), 1.0);
    assert!(from_primitive_f32().is_some());
    assert!(from_primitive_f64().is_some());
    assert_eq!(to_primitive_f32(), Some(1.0));
    assert_eq!(to_primitive_f64(), Some(1.0));
    assert_eq!(json_number_as_f64(), Some(1.0));
    assert!(json_number_from_f64().is_some());
    assert_eq!(json_value_as_f64(), Some(1.0));
}

#[test]
fn clippy_config_lists_every_proven_entry() {
    let cfg = include_str!("../clippy.toml");
    for path in [
        "rust_decimal::Decimal::from_f32_retain",
        "rust_decimal::Decimal::from_f64_retain",
        "rust_decimal::Decimal::as_f64",
        "num_traits::FromPrimitive::from_f32",
        "num_traits::FromPrimitive::from_f64",
        "num_traits::ToPrimitive::to_f32",
        "num_traits::ToPrimitive::to_f64",
        "serde_json::Number::as_f64",
        "serde_json::Number::from_f64",
        "serde_json::Value::as_f64",
    ] {
        assert!(
            cfg.contains(&format!("\"{path}\"")),
            "{path} missing from clippy.toml"
        );
    }
}
