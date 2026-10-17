//! Contract tests against the real engine (P05 plan §2.10, §7). One binary,
//! one engine, one fresh cluster; tests run in the order listed below.
//!
//! Run: `cargo xtask engine && cargo xtask pg-fetch`, then
//! `cargo test -p cl-api --test contract`.

#[path = "../support/mod.rs"]
mod support;

mod contract_harness;

use support::{EngineMode, TestFn, run_suite};

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[(
        "harness::boots_once_and_opens_contract_co",
        contract_harness::boots_once_and_opens_contract_co,
    )];
    run_suite(EngineMode::Normal, tests)
}
