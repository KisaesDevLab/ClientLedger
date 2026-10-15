//! Build helper for `ClientLedger`: `cargo xtask <command>`.
#![forbid(unsafe_code)]

mod licenses;
mod meta;

use anyhow::{Result, bail};

fn run_licenses() -> Result<()> {
    let violations = licenses::check(&meta::load()?);
    if violations.is_empty() {
        println!("licenses: all direct dependencies are MIT or Apache-2.0");
        return Ok(());
    }
    for v in &violations {
        eprintln!("licenses: {v}");
    }
    bail!(
        "{} direct-dependency license violation(s)",
        violations.len()
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("licenses") => run_licenses(),
        other => bail!("unknown command {other:?}; try: licenses"),
    }
}
