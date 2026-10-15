//! Build helper for `ClientLedger`: `cargo xtask <command>`.
#![forbid(unsafe_code)]

mod layering;
mod licenses;
mod meta;

use anyhow::{Result, bail};

fn report(gate: &str, ok_message: &str, violations: &[String]) -> Result<()> {
    if violations.is_empty() {
        println!("{gate}: {ok_message}");
        return Ok(());
    }
    for v in violations {
        eprintln!("{gate}: {v}");
    }
    bail!("{} {gate} violation(s)", violations.len())
}

fn run_licenses() -> Result<()> {
    report(
        "licenses",
        "all direct dependencies are MIT or Apache-2.0",
        &licenses::check(&meta::load()?),
    )
}

fn run_layering() -> Result<()> {
    report(
        "layering",
        "crate dependency edges are allowed",
        &layering::check(&meta::load()?),
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("licenses") => run_licenses(),
        Some("layering") => run_layering(),
        other => bail!("unknown command {other:?}; try: licenses, layering"),
    }
}
