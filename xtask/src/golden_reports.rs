//! `cargo xtask golden-reports [--bless]`: run the golden-report harness
//! (`crates/cl-company/tests/report_golden.rs`) against the real engine.
//! The harness runs its cross-checks first; with `--bless` it then writes
//! `fixtures/golden/reports/`, otherwise it compares against them. xtask
//! can't link `cl-company`, so this runs the test binary through cargo.

use anyhow::{Result, bail};
use std::process::Command;

pub fn golden_reports(args: &[String]) -> Result<()> {
    let mut bless = false;
    for a in args {
        match a.as_str() {
            "--bless" => bless = true,
            other => bail!("golden-reports: unknown option `{other}`"),
        }
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "test",
        "-p",
        "cl-company",
        "--test",
        "report_golden",
        "golden_reports",
        "--",
        "--nocapture",
    ])
    .env("CL_API_REQUIRE", "1")
    .env("CL_OFFLINE_TESTS", "1");
    if bless {
        cmd.env("CL_GOLDEN_BLESS", "1");
    }
    let status = cmd.status()?;
    if !status.success() {
        bail!("golden-reports failed");
    }
    println!(
        "golden-reports: {}",
        if bless {
            "cross-checks passed, goldens written"
        } else {
            "goldens match"
        }
    );
    Ok(())
}
