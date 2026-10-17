//! Offline and engine-restart contract (P05 plan §2.10, §6, AC3, AC5): a
//! second engine started with `CL_OFFLINE=on`.

#[path = "support/mod.rs"]
mod support;

use support::{EngineMode, Harness, TestFn, run_suite};

fn placeholder(_h: &Harness) {}

fn main() -> std::process::ExitCode {
    let tests: &[(&str, TestFn)] = &[("offline::boots", placeholder)];
    run_suite(EngineMode::Offline, tests)
}
