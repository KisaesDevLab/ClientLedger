//! `cargo xtask api-spec [--update | --check]`: the engine spec fixtures
//! (P05 plan §2.8, §2.14): `openapi.sha256`, `covered_operations.json`
//! and `engine_schemas.sha256` under `crates/cl-api/tests/fixtures/`.
//!
//! The computation is `crates/cl-api/tests/support/spec.rs`, compiled into
//! this tool and into the cl-api tests, so a fixture written here is exactly
//! what the tests recompute.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[path = "../../crates/cl-api/tests/support/spec.rs"]
mod spec;

const FILES: [&str; 3] = [
    "openapi.sha256",
    "covered_operations.json",
    "engine_schemas.sha256",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn fixture_dir(root: &Path) -> PathBuf {
    root.join("crates/cl-api/tests/fixtures")
}

fn computed(root: &Path) -> Result<[String; 3]> {
    let ops = std::fs::read_to_string(root.join("crates/cl-api/operations.txt"))
        .context("crates/cl-api/operations.txt")?;
    let f = spec::compute(root, &ops).map_err(anyhow::Error::msg)?;
    Ok([f.openapi, f.covered, f.schemas])
}

/// Lines that differ, as `- old` / `+ new`, for the report. Lines are keyed
/// by their trailing identity (path or operation) so the report names what
/// changed.
fn changed_lines(old: &str, new: &str) -> Vec<String> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut out = Vec::new();
    for l in &old_lines {
        if !new_lines.contains(l) {
            out.push(format!("- {l}"));
        }
    }
    for l in &new_lines {
        if !old_lines.contains(l) {
            out.push(format!("+ {l}"));
        }
    }
    out
}

fn have_engine(root: &Path) -> bool {
    root.join("engine/openapi.json").is_file()
}

/// `cargo xtask api-spec`.
pub fn api_spec(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("--update") => update(),
        Some("--check") | None => check(),
        Some(other) => bail!("usage: cargo xtask api-spec [--update | --check] (got `{other}`)"),
    }
}

/// Rewrites the fixtures and prints what changed.
fn update() -> Result<()> {
    let root = repo_root();
    if !have_engine(&root) {
        bail!("engine/openapi.json is missing: run `git submodule update --init engine`");
    }
    let dir = fixture_dir(&root);
    std::fs::create_dir_all(&dir)?;
    let new = computed(&root)?;
    let mut any = false;
    for (name, text) in FILES.iter().zip(new.iter()) {
        let path = dir.join(name);
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        if old == *text {
            println!("api-spec: {name} unchanged");
            continue;
        }
        any = true;
        println!("api-spec: {name} updated");
        for l in changed_lines(&old, text) {
            println!("  {l}");
        }
        std::fs::write(&path, text).with_context(|| path.display().to_string())?;
    }
    if any {
        println!(
            "api-spec: review the changes above against the cl-api models \
             (covered operations and Zod sources) before committing"
        );
    }
    Ok(())
}

/// Fails when a fixture is stale. Skips (with a note) when the engine
/// submodule is not checked out, unless `CL_API_REQUIRE=1`.
fn check() -> Result<()> {
    let root = repo_root();
    if !have_engine(&root) {
        if std::env::var("CL_API_REQUIRE").as_deref() == Ok("1") {
            bail!("api-spec: engine/openapi.json is missing and CL_API_REQUIRE=1");
        }
        println!(
            "api-spec: SKIPPED, engine/openapi.json is absent (submodule not checked out); \
             CI with the submodule runs this check"
        );
        return Ok(());
    }
    let dir = fixture_dir(&root);
    let new = computed(&root)?;
    let mut stale = Vec::new();
    for (name, text) in FILES.iter().zip(new.iter()) {
        let old = std::fs::read_to_string(dir.join(name)).unwrap_or_default();
        if old != *text {
            stale.push(format!(
                "{name}:\n  {}",
                changed_lines(&old, text).join("\n  ")
            ));
        }
    }
    if stale.is_empty() {
        println!("api-spec: engine spec fixtures are current");
        return Ok(());
    }
    bail!(
        "engine spec fixtures are stale; run `cargo xtask api-spec --update`, review the diff \
         against the cl-api models, and commit:\n{}",
        stale.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_lines_names_added_and_removed_rows() {
        let d = changed_lines(
            "a
b
", "b
c
",
        );
        assert_eq!(d, vec!["- a".to_string(), "+ c".to_string()]);
        assert_eq!(
            changed_lines(
                "x
", "x
"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn fixtures_are_current_when_the_engine_is_checked_out() {
        // Same computation as `api-spec --check`; skipped without engine/.
        let root = repo_root();
        if !have_engine(&root) {
            return;
        }
        check().expect("fixtures current");
    }

    #[test]
    fn unknown_flag_is_refused() {
        assert!(api_spec(&["--nope".to_string()]).is_err());
    }
}
