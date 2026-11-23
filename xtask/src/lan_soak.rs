//! `cargo xtask lan-soak [--minutes N | --seconds N]`: the P33 five-client
//! office-network soak (AC3) against a real host, through the live proxy
//! on loopback (`crates/cl-ui/tests/lan_live.rs`, test
//! `office_network_soak`). xtask can't link `cl-ui`, so this runs the test
//! binary through cargo.
//!
//! - PR CI: `--minutes 3` (correctness only: no failure, no lost update,
//!   no unexplained conflict, the books balance).
//! - Nightly (and once locally before the PR): `--minutes 30`.

use anyhow::{Result, bail};
use std::process::Command;

/// The default run (the PR CI length).
const DEFAULT_SECS: u64 = 180;

fn parse(args: &[String]) -> Result<u64> {
    let mut secs = DEFAULT_SECS;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let unit = match a.as_str() {
            "--minutes" => 60,
            "--seconds" => 1,
            other => bail!("lan-soak: unknown option `{other}`"),
        };
        let Some(n) = it.next().and_then(|v| v.parse::<u64>().ok()) else {
            bail!("lan-soak: `{a}` needs a whole number");
        };
        if n == 0 {
            bail!("lan-soak: the run must be longer than zero");
        }
        secs = n * unit;
    }
    Ok(secs)
}

pub fn lan_soak(args: &[String]) -> Result<()> {
    let secs = parse(args)?;
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "test",
            "-p",
            "cl-ui",
            "--test",
            "lan_live",
            "office_network_soak",
            "--",
            "--exact",
            "--nocapture",
        ])
        .env("CL_LAN_SOAK_SECS", secs.to_string())
        .env("CL_API_REQUIRE", "1")
        .env("CL_OFFLINE_TESTS", "1")
        .status()?;
    if !status.success() {
        bail!("lan-soak failed");
    }
    println!("lan-soak: {secs} s with five workstations passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn lengths_parse_and_bad_ones_are_refused() {
        assert_eq!(parse(&[]).unwrap(), 180);
        assert_eq!(parse(&s(&["--minutes", "30"])).unwrap(), 1800);
        assert_eq!(parse(&s(&["--seconds", "45"])).unwrap(), 45);
        assert!(parse(&s(&["--minutes"])).is_err());
        assert!(parse(&s(&["--minutes", "0"])).is_err());
        assert!(parse(&s(&["--hours", "1"])).is_err());
    }
}
