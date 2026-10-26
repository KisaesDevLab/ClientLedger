//! Build helper for `ClientLedger`: `cargo xtask <command>`.
#![forbid(unsafe_code)]

mod api_spec;
mod ci;
mod docread_accuracy;
mod engine;
mod engine_test;
mod fetch_lock;
mod gen_invoices;
mod gen_sample;
mod grid_bench;
mod keymap_doc;
mod layering;
mod licenses;
mod meta;
mod nofloat;
mod ocr_build;
mod ocr_models;
mod pdfium_fetch;
mod pg_fetch;
mod preview;
mod run;
mod ui_check;
mod unsafe_check;

use anyhow::{Result, bail};

const USAGE: &str = "\
usage: cargo xtask <command>

  ci [--verbose]     fmt --check, clippy -D warnings, test, deny, licenses, layering
                     (quiet locally: failures only; full output in CI or with --verbose)
  test [args]        cargo test (whole workspace unless -p is given), CL_OFFLINE_TESTS=1
  licenses           direct dependencies must be MIT or Apache-2.0
  layering           crate dependency rules from CLAUDE.md
  nofloat            no floating point in the money crates and cl-docread rules (syn scan)
  unsafe-check       no `unsafe` outside cl-host's one Windows job memory call (Q-A03s)
  engine [--smoke] [--force]
                     build the engine single executable into target/engine/<target>
  engine-test [-- <vitest args>]
                     run the engine's api tests against a throwaway bundled PostgreSQL (port 5434)
  api-spec [--update | --check]
                     engine OpenAPI / Zod-source fixtures for cl-api (check skips without engine/)
  pg-fetch [--force] download the pinned PostgreSQL binaries into target/pg/<target>
  pdfium-fetch [--force]
                     download the pinned PDFium library (document previews) into
                     target/pdfium/<triple>
  ocr-build [--force]
                     build the tesseract program from pinned source (ocr.lock), optional
                     dependencies off, linkage checked, into target/ocr/<triple>
  ocr-models [--force]
                     put the pinned tessdata_fast models (eng, osd) into target/ocr/<triple>/tessdata
  keymap-doc [--check]
                     generate docs/KEYBOARD.md and the shortcut cheat sheet from the cl-model keymap
  grid-bench         ledger grid scroll benchmark (200k rows, real shell), optimised; fails
                     when p95 frame time is over 8 ms (AccessKit off) or 12 ms (on), one retry
  gen-invoices [--holdout] [--no-scans] [--samples] [--out DIR] [--layouts id,id]
                     synthetic bills and receipts with ground truth for the reading harness into
                     target/docread-synth (A03); --samples refreshes fixtures/docs/read
  docread-accuracy [--gate] [--regen] [--stress] [--tessdata DIR] [--routes text,scan,photo]
                     reading accuracy per field and path, measured memory and time, into
                     target/docread-accuracy; --gate fails below the Q-285 minimums
  gen-sample [--check]
                     write fixtures/sample/ozark_ridge (the practice company, Q-219); --check verifies
  run [--no-prepare] [-- <app args>]
                     build the engine and fetch PostgreSQL and PDFium if needed, then start the desktop app
  ui-check <screen> [--compare] [--no-build]
                     launch the app with a temporary control file, run the screen's scenario
                     (crates/cl-automation/scenarios/<screen>.yaml) in light and dark, save PNGs to
                     target/ui-check/<screen>/; --compare adds compare.html beside the mockup
  preview            unsigned portable PREVIEW build for testers (Q-253): release apps, engine,
                     PostgreSQL in target/preview/<name>/ plus a .zip/.tar.gz and .sha256
  package            (not implemented yet)";

/// Commands that exist in the CLAUDE.md command list but land in later phases.
const STUBS: [(&str, &str); 1] = [("package", "P35")];

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
                "-p",
                "clientledger-docread",
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
        "no floating point in cl-core, cl-api, cl-model, cl-grid core or cl-docread rules",
        &nofloat::check(&root)?,
    )
}

pub(crate) fn run_unsafe_check() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    report(
        "unsafe-check",
        "no unsafe code outside the one Windows job memory call",
        &unsafe_check::check(&root)?,
    )
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ci") => ci::ci(&args[1..]),
        Some("test") => ci::test(&args[1..]),
        Some("licenses") => run_licenses(),
        Some("layering") => run_layering(),
        Some("nofloat") => run_nofloat(),
        Some("unsafe-check") => run_unsafe_check(),
        Some("engine") => engine::engine(&args[1..]),
        Some("engine-test") => engine_test::engine_test(&args[1..]),
        Some("api-spec") => api_spec::api_spec(&args[1..]),
        Some("pg-fetch") => pg_fetch::pg_fetch(&args[1..]),
        Some("pdfium-fetch") => pdfium_fetch::pdfium_fetch(&args[1..]),
        Some("ocr-build") => ocr_build::ocr_build(&args[1..]),
        Some("ocr-models") => ocr_models::ocr_models(&args[1..]),
        Some("preview") => preview::preview(&args[1..]),
        Some("run") => run::run(&args[1..]),
        Some("ui-check") => ui_check::ui_check(&args[1..]),
        Some("keymap-doc") => keymap_doc::keymap_doc(&args[1..]),
        Some("gen-sample") => gen_sample::gen_sample(&args[1..]),
        Some("gen-invoices") => gen_invoices::gen_invoices(&args[1..]),
        Some("docread-accuracy") => docread_accuracy::docread_accuracy(&args[1..]),
        Some("grid-bench") => grid_bench::grid_bench(&args[1..]),
        Some(cmd) => {
            if let Some((_, phase)) = STUBS.iter().find(|(name, _)| *name == cmd) {
                bail!("`{cmd}` is not implemented yet: lands in {phase}");
            }
            bail!("unknown command `{cmd}`\n\n{USAGE}")
        }
        None => bail!("{USAGE}"),
    }
}
