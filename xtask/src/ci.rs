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

/// How much of a gate's own output reaches the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Output {
    /// Everything, as the tool prints it (CI logs, `--verbose`).
    Full,
    /// One line per passing gate; for a failing gate only the lines that
    /// explain the failure. Keeps local runs and agent transcripts short.
    Quiet,
}

impl Output {
    /// Quiet unless running in CI (`CI` is set) or asked for `--verbose`.
    #[must_use]
    pub fn choose(args: &[String], in_ci: bool) -> Output {
        if in_ci || args.iter().any(|a| a == "--verbose" || a == "-v") {
            Output::Full
        } else {
            Output::Quiet
        }
    }
}

/// Most lines of a failing gate's output shown in quiet mode.
const QUIET_TAIL: usize = 60;

/// The lines of a failing gate's output that explain the failure: every
/// error, failed test, panic and assertion line, then the last
/// [`QUIET_TAIL`] lines, without repeats and in their original order.
#[must_use]
pub fn failure_excerpt(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    let tail_start = lines.len().saturating_sub(QUIET_TAIL);
    lines
        .iter()
        .enumerate()
        .filter(|(i, l)| {
            *i >= tail_start
                || l.starts_with("error")
                || l.contains("FAILED")
                || l.contains("panicked at")
                || l.contains("assertion")
                || l.trim_start().starts_with("left:")
                || l.trim_start().starts_with("right:")
                || l.starts_with("Diff in")
        })
        .map(|(_, l)| *l)
        .collect()
}

fn run(mut cmd: Command, what: &str, output: Output) -> Result<()> {
    if output == Output::Full {
        println!("==> {what}");
        let status = cmd
            .status()
            .with_context(|| format!("could not start `{what}`"))?;
        if !status.success() {
            bail!("`{what}` failed ({status})");
        }
        return Ok(());
    }
    let out = cmd
        .output()
        .with_context(|| format!("could not start `{what}`"))?;
    if out.status.success() {
        println!("ok   {what}");
        return Ok(());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    println!("FAIL {what}");
    for line in failure_excerpt(&text) {
        println!("  {line}");
    }
    bail!("`{what}` failed ({})", out.status)
}

/// `cargo xtask test [cargo test args...]`
pub fn test(user_args: &[String]) -> Result<()> {
    run_tests(user_args, Output::Full)
}

fn run_tests(user_args: &[String], output: Output) -> Result<()> {
    let mut cmd = cargo();
    cmd.args(test_args(user_args)).env(OFFLINE_ENV, "1");
    run(cmd, "cargo test", output)
}

/// `cargo xtask ci [--verbose]`: run every gate, report all failures at the
/// end. Local runs are quiet by default; CI (and `--verbose`) shows
/// everything.
pub fn ci(args: &[String]) -> Result<()> {
    let output = Output::choose(args, std::env::var_os("CI").is_some());
    let mut failures: Vec<String> = Vec::new();
    let mut step = |name: &str, result: Result<()>| {
        if let Err(e) = result {
            eprintln!("ci: {name} failed: {e:#}");
            failures.push(name.to_string());
        }
    };

    let mut fmt = cargo();
    fmt.args(["fmt", "--all", "--", "--check"]);
    step("fmt", run(fmt, "cargo fmt --check", output));

    let mut clippy = cargo();
    clippy.args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]);
    step("clippy", run(clippy, "cargo clippy -D warnings", output));

    step("test", run_tests(&[], output));

    let mut deny = cargo();
    deny.args(["deny", "check"]);
    step("deny", run(deny, "cargo deny check", output));

    step("licenses", crate::run_licenses());
    step("layering", crate::run_layering());
    step("release-features", crate::run_release_features());
    step("nofloat", crate::run_nofloat());
    step("unsafe-check", crate::run_unsafe_check());
    step("docread-check", crate::run_docread_check());
    step(
        "keymap-doc",
        crate::keymap_doc::keymap_doc(&["--check".to_string()]),
    );
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

    #[test]
    fn quiet_locally_full_in_ci_or_verbose() {
        assert_eq!(Output::choose(&[], false), Output::Quiet);
        assert_eq!(Output::choose(&[], true), Output::Full);
        assert_eq!(Output::choose(&v(&["--verbose"]), false), Output::Full);
        assert_eq!(Output::choose(&v(&["-v"]), false), Output::Full);
    }

    #[test]
    fn failure_excerpt_keeps_the_explaining_lines_and_the_tail() {
        use std::fmt::Write as _;
        let mut text = String::from("error[E0308]: mismatched types\n");
        for i in 0..200 {
            let _ = writeln!(text, "   Compiling crate{i}");
        }
        text.push_str("test a::b ... FAILED\nthread 'a::b' panicked at src/a.rs:3:5:\n");
        text.push_str("  left: 1\n right: 2\ntest result: FAILED. 1 passed; 1 failed\n");
        let out = failure_excerpt(&text);
        assert_eq!(out[0], "error[E0308]: mismatched types");
        assert!(out.contains(&"test a::b ... FAILED"));
        assert!(out.contains(&"  left: 1"));
        assert!(out.iter().any(|l| l.starts_with("test result: FAILED")));
        assert!(
            !out.contains(&"   Compiling crate10"),
            "old noise is dropped"
        );
        assert!(out.len() <= QUIET_TAIL + 1);
    }

    #[test]
    fn failure_excerpt_of_short_output_is_all_of_it() {
        assert_eq!(failure_excerpt("a\nb"), vec!["a", "b"]);
        assert_eq!(failure_excerpt(""), Vec::<&str>::new());
    }
}
