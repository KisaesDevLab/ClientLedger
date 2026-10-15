//! Build helper for `ClientLedger`: `cargo xtask <command>`.
#![forbid(unsafe_code)]

mod ci;
mod engine;
mod layering;
mod licenses;
mod meta;
mod pg_fetch;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo xtask <command>

  ci                 fmt --check, clippy -D warnings, test, deny, licenses, layering
  test [args]        cargo test (whole workspace unless -p is given), CL_OFFLINE_TESTS=1
  licenses           direct dependencies must be MIT or Apache-2.0
  layering           crate dependency rules from CLAUDE.md
  engine [--smoke] [--force]
                     build the engine single executable into target/engine/<target>
  pg-fetch [--force] download the pinned PostgreSQL binaries into target/pg/<target>
  run | ui-check | keymap-doc | package   (not implemented yet)";

/// Commands that exist in the CLAUDE.md command list but land in later phases.
const STUBS: [(&str, &str); 4] = [
    ("run", "P06"),
    ("ui-check", "P07"),
    ("keymap-doc", "P06"),
    ("package", "P35"),
];

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

pub(crate) fn run_licenses() -> Result<()> {
    report(
        "licenses",
        "all direct dependencies are MIT or Apache-2.0",
        &licenses::check(&meta::load()?),
    )
}

pub(crate) fn run_layering() -> Result<()> {
    report(
        "layering",
        "crate dependency edges are allowed",
        &layering::check(&meta::load()?),
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci::ci(),
        Some("test") => ci::test(&args[1..]),
        Some("licenses") => run_licenses(),
        Some("layering") => run_layering(),
        Some("engine") => engine::engine(&args[1..]),
        Some("pg-fetch") => pg_fetch::pg_fetch(&args[1..]),
        Some(cmd) => {
            if let Some((_, phase)) = STUBS.iter().find(|(name, _)| *name == cmd) {
                bail!("`{cmd}` is not implemented yet: lands in {phase}");
            }
            bail!("unknown command `{cmd}`\n\n{USAGE}")
        }
        None => bail!("{USAGE}"),
    }
}
