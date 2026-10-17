//! `cargo xtask ci` and `cargo xtask test`.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// Environment flag that tests check to refuse real network access (Q-116 era
/// policy: tests run offline by default). It is a convention the test helpers
/// honour; `xtask test` always sets it.
pub const OFFLINE_ENV: &str = "CL_OFFLINE_TESTS";

/// Build the argument list for `cargo test`. The whole workspace is tested
/// unless the caller already picked a scope with `-p`/`--package`/`--workspace`.
#[must_use]
pub fn test_args(user_args: &[String]) -> Vec<String> {
    let scoped = user_args.iter().any(|a| {
        a == "-p"
            || a == "--package"
            || a == "--workspace"
            || a == "--all"
            || a.starts_with("--package=")
    });
    // Everything before a bare `--` is for cargo; keep user order intact.
    let mut out = vec!["test".to_string()];
    if !scoped {
        out.push("--workspace".into());
    }
    out.extend(user_args.iter().cloned());
    out
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn run(mut cmd: Command, what: &str) -> Result<()> {
    println!("==> {what}");
    let status = cmd
        .status()
        .with_context(|| format!("could not start `{what}`"))?;
    if !status.success() {
        bail!("`{what}` failed ({status})");
    }
    Ok(())
}

/// `cargo xtask test [cargo test args...]`
pub fn test(user_args: &[String]) -> Result<()> {
    let mut cmd = cargo();
    cmd.args(test_args(user_args)).env(OFFLINE_ENV, "1");
    run(cmd, "cargo test")
}

/// `cargo xtask ci`: run every gate, report all failures at the end.
pub fn ci() -> Result<()> {
    let mut failures: Vec<String> = Vec::new();
    let mut step = |name: &str, result: Result<()>| {
        if let Err(e) = result {
            eprintln!("ci: {name} failed: {e:#}");
            failures.push(name.to_string());
        }
    };

    let mut fmt = cargo();
    fmt.args(["fmt", "--all", "--", "--check"]);
    step("fmt", run(fmt, "cargo fmt --check"));

    let mut clippy = cargo();
    clippy.args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]);
    step("clippy", run(clippy, "cargo clippy -D warnings"));

    step("test", test(&[]));

    let mut deny = cargo();
    deny.args(["deny", "check"]);
    step("deny", run(deny, "cargo deny check"));

    step("licenses", crate::run_licenses());
    step("layering", crate::run_layering());
    step("nofloat", crate::run_nofloat());
    step(
        "api-spec",
        crate::api_spec::api_spec(&["--check".to_string()]),
    );

    if failures.is_empty() {
        println!("ci: all gates passed");
        Ok(())
    } else {
        bail!("ci failed: {}", failures.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn defaults_to_workspace() {
        assert_eq!(test_args(&[]), v(&["test", "--workspace"]));
    }

    #[test]
    fn package_and_filter_are_forwarded() {
        assert_eq!(
            test_args(&v(&["-p", "cl-core", "money"])),
            v(&["test", "-p", "cl-core", "money"])
        );
    }

    #[test]
    fn explicit_workspace_is_not_duplicated() {
        assert_eq!(
            test_args(&v(&["--workspace", "x"])),
            v(&["test", "--workspace", "x"])
        );
    }

    #[test]
    fn test_binary_args_follow_double_dash() {
        assert_eq!(
            test_args(&v(&["-p", "cl-core", "--", "--exact", "a::b"])),
            v(&["test", "-p", "cl-core", "--", "--exact", "a::b"])
        );
    }
}
