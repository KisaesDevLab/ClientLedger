//! `cargo xtask run [-- <app args>]`: make sure the engine executable, the
//! `PostgreSQL` binaries and the `PDFium` library exist (all cached, so this is
//! fast when nothing changed), then start the desktop app. Arguments after `--` go to
//! the app, for example `cargo xtask run -- --demo`.

use anyhow::{Context, Result, bail};
use std::process::Command;

/// What `run` does, as data, so the argument split is unit tested.
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// Skip building the engine and fetching `PostgreSQL` (`--no-prepare`).
    pub prepare: bool,
    /// Arguments for the app.
    pub app_args: Vec<String>,
}

/// Split `xtask run` arguments.
///
/// # Errors
/// An option before `--` that `run` does not know.
pub fn plan(args: &[String]) -> Result<Plan> {
    let split = args.iter().position(|a| a == "--");
    let (own, app) = match split {
        Some(i) => (&args[..i], args[i + 1..].to_vec()),
        None => (args, Vec::new()),
    };
    let mut prepare = true;
    for a in own {
        match a.as_str() {
            "--no-prepare" => prepare = false,
            other => bail!("unknown run option `{other}` (use --no-prepare, or -- <app args>)"),
        }
    }
    Ok(Plan {
        prepare,
        app_args: app,
    })
}

/// Entry point.
pub fn run(args: &[String]) -> Result<()> {
    let plan = plan(args)?;
    // The demo shows synthetic data and starts no engine, so it needs neither.
    let demo = plan
        .app_args
        .iter()
        .any(|a| a == "--demo" || a == "--no-engine");
    if plan.prepare && !demo {
        crate::engine::engine(&[]).context("building the engine failed")?;
        crate::pg_fetch::pg_fetch(&[]).context("fetching PostgreSQL failed")?;
    }
    // PDF previews (P17), the demo's too. Without PDFium only the PDF
    // preview is missing, so a failed fetch (offline) does not stop the run.
    if plan.prepare
        && let Err(e) = crate::pdfium_fetch::pdfium_fetch(&[])
    {
        eprintln!("run: PDFium not fetched, PDF previews will be unavailable: {e:#}");
    }
    let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.args(["run", "-p", "clientledger", "--"])
        .args(&plan.app_args);
    let status = cmd.status().context("could not start cargo")?;
    if !status.success() {
        bail!("the app exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn app_arguments_follow_the_double_dash() {
        let p = plan(&v(&["--", "--demo", "--theme", "dark"])).unwrap();
        assert!(p.prepare);
        assert_eq!(p.app_args, v(&["--demo", "--theme", "dark"]));
    }

    #[test]
    fn no_arguments_prepares_and_passes_nothing() {
        assert_eq!(
            plan(&[]).unwrap(),
            Plan {
                prepare: true,
                app_args: vec![]
            }
        );
    }

    #[test]
    fn no_prepare_is_recognised_and_unknown_options_fail() {
        assert!(!plan(&v(&["--no-prepare"])).unwrap().prepare);
        assert!(plan(&v(&["--bogus"])).is_err());
    }
}
