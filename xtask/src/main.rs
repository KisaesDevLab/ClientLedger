//! Build helper for `ClientLedger`: `cargo xtask <command>`.
#![forbid(unsafe_code)]

mod api_spec;
mod ci;
mod engine;
mod engine_test;
mod keymap_doc;
mod layering;
mod licenses;
mod meta;
mod nofloat;
mod pg_fetch;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo xtask <command>

  ci                 fmt --check, clippy -D warnings, test, deny, licenses, layering
  test [args]        cargo test (whole workspace unless -p is given), CL_OFFLINE_TESTS=1
  licenses           direct dependencies must be MIT or Apache-2.0
  layering           crate dependency rules from CLAUDE.md
  nofloat            no floating point in cl-core and cl-api src (syn scan)
  engine [--smoke] [--force]
                     build the engine single executable into target/engine/<target>
  engine-test [-- <vitest args>]
                     run the engine's api tests against a throwaway bundled PostgreSQL (port 5434)
  api-spec [--update | --check]
                     engine OpenAPI / Zod-source fixtures for cl-api (check skips without engine/)
  pg-fetch [--force] download the pinned PostgreSQL binaries into target/pg/<target>
  keymap-doc [--check]
                     generate docs/KEYBOARD.md and the shortcut cheat sheet from the cl-model keymap
  run | ui-check | package   (not implemented yet)";

/// Commands that exist in the CLAUDE.md command list but land in later phases.
const STUBS: [(&str, &str); 3] = [("run", "P06"), ("ui-check", "P07"), ("package", "P35")];

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

/// A normal build of the apps must not switch on a test-only feature,
/// whatever edge (or feature unification) would bring it in.
pub(crate) fn run_release_features() -> Result<()> {
    let out =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args([
                "tree",
                "-e",
                "features,no-dev",
                "-p",
                "clientledger",
                "-p",
                "clientledger-cli",
                "--prefix",
                "none",
            ])
            .output()?;
    if !out.status.success() {
        bail!("cargo tree failed ({})", out.status);
    }
    report(
        "release-features",
        "no test-only feature in a normal build of the apps",
        &layering::test_features_in_tree(&String::from_utf8_lossy(&out.stdout)),
    )
}

pub(crate) fn run_nofloat() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    report(
        "nofloat",
        "no floating point in cl-core or cl-api",
        &nofloat::check(&root)?,
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci::ci(),
        Some("test") => ci::test(&args[1..]),
        Some("licenses") => run_licenses(),
        Some("layering") => run_layering(),
        Some("nofloat") => run_nofloat(),
        Some("engine") => engine::engine(&args[1..]),
        Some("engine-test") => engine_test::engine_test(&args[1..]),
        Some("api-spec") => api_spec::api_spec(&args[1..]),
        Some("pg-fetch") => pg_fetch::pg_fetch(&args[1..]),
        Some("keymap-doc") => keymap_doc::keymap_doc(&args[1..]),
        Some(cmd) => {
            if let Some((_, phase)) = STUBS.iter().find(|(name, _)| *name == cmd) {
                bail!("`{cmd}` is not implemented yet: lands in {phase}");
            }
            bail!("unknown command `{cmd}`\n\n{USAGE}")
        }
        None => bail!("{USAGE}"),
    }
}
