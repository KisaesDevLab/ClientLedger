//! `cargo xtask transfer-roundtrip` (P32, Q-74, Q-P32d): the CI round trip
//! of a transfer file, with no secrets.
//!
//! 1. The desktop harness (`crates/cl-company/tests/transfer_live.rs`,
//!    `transfer_roundtrip_seed`) starts the pinned engine on a throwaway
//!    bundled PostgreSQL, seeds a synthetic company, makes a real `.clxfer`
//!    with `make_transfer`, and writes `expected.json` (the totals in the
//!    file and the desktop's own preview) next to it.
//! 2. The engine suite `transfer-external` (`cargo xtask engine-test`, the
//!    same engine commit in server mode on port 5434) imports the file
//!    through `POST /tenant-archives/import` as firm staff with
//!    `transferOnly`, compares expected and actual totals, re-exports and
//!    re-imports, and scans every table and the upload folder for the
//!    planted canaries.
//!
//! The passphrase is a synthetic constant: nothing here is secret. xtask
//! can't link `cl-company`, so both steps run through cargo.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The synthetic passphrase of the round-trip file (not a secret).
pub const PASSPHRASE: &str = "roundtrip-synthetic-passphrase-2026";

fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

pub fn transfer_roundtrip(args: &[String]) -> Result<()> {
    if let Some(a) = args.first() {
        bail!("transfer-roundtrip: unknown option `{a}`");
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = crate::meta::target_dir(&root).join("transfer-roundtrip");
    if out.exists() {
        std::fs::remove_dir_all(&out).context("clear target/transfer-roundtrip")?;
    }
    std::fs::create_dir_all(&out)?;
    let file = out.join("roundtrip.clxfer");
    let expected = out.join("expected.json");

    println!("transfer-roundtrip: 1/2 make the transfer file on the desktop side");
    let status = Command::new(cargo())
        .args([
            "test",
            "-p",
            "cl-company",
            "--test",
            "transfer_live",
            "transfer_roundtrip_seed",
            "--",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CL_OFFLINE_TESTS", "1")
        .env("CL_HOST_REQUIRE", "1")
        .env("CL_TRANSFER_OUT", &out)
        .env("CL_TRANSFER_PASSPHRASE", PASSPHRASE)
        .current_dir(&root)
        .status()?;
    if !status.success() || !file.is_file() || !expected.is_file() {
        bail!("transfer-roundtrip: the desktop harness did not make the file");
    }

    println!("transfer-roundtrip: 2/2 import it with the pinned engine in server mode");
    let status = Command::new(cargo())
        .args(["xtask", "engine-test", "--", "transfer-external"])
        .env("CL_OFFLINE_TESTS", "1")
        .env("CL_TRANSFER_FILE", &file)
        .env("CL_TRANSFER_PASSPHRASE", PASSPHRASE)
        .env("CL_TRANSFER_EXPECTED", &expected)
        .current_dir(&root)
        .status()?;
    if !status.success() {
        bail!("transfer-roundtrip: the engine import or its checks failed");
    }
    println!("transfer-roundtrip: totals match and no secret travelled");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_refused_and_the_passphrase_is_synthetic() {
        assert!(transfer_roundtrip(&["--x".to_string()]).is_err());
        assert!(PASSPHRASE.contains("synthetic"));
    }
}
