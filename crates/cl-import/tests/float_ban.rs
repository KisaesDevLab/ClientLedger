//! Proof that the float bans in `crates/cl-import/clippy.toml` resolve and
//! fire (review S12). Each item below uses a banned type or method under
//! `#[expect(…)]`; if an entry stopped resolving, the expectation would be
//! unfulfilled and `cargo clippy --all-targets -D warnings` fails. The
//! functions also run as ordinary tests so they are not dead code.
//!
//! This file lives outside `src/` on purpose: the xtask `nofloat` scan covers
//! `src/**` only, and these are the one sanctioned use of floats here.
#![allow(clippy::float_cmp, clippy::float_arithmetic)]

use rust_decimal::Decimal;
use std::time::Duration;

#[expect(clippy::disallowed_types, reason = "proves the f64 ban fires")]
fn parse_f64(s: &str) -> bool {
    s.parse::<f64>().is_ok()
}

#[expect(clippy::disallowed_types, reason = "proves the f32 ban fires")]
fn parse_f32(s: &str) -> bool {
    s.parse::<f32>().is_ok()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_from_f64_retain() -> bool {
    Decimal::from_f64_retain(1.5).is_some()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_from_f32_retain() -> bool {
    Decimal::from_f32_retain(1.5).is_some()
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn decimal_as_f64() -> bool {
    Decimal::ONE.as_f64() > 0.5
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn duration_secs_f64() -> bool {
    Duration::from_secs(1).as_secs_f64() > 0.5
}

#[expect(clippy::disallowed_methods, reason = "proves the ban fires")]
fn duration_secs_f32() -> bool {
    Duration::from_secs(1).as_secs_f32() > 0.5
}

#[test]
fn every_ban_fires() {
    assert!(parse_f64("1.5"));
    assert!(parse_f32("1.5"));
    assert!(decimal_from_f64_retain());
    assert!(decimal_from_f32_retain());
    assert!(decimal_as_f64());
    assert!(duration_secs_f64());
    assert!(duration_secs_f32());
}
