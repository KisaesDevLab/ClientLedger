//! `cargo xtask qa-company [--out target/qa-company] [--fresh]` (P29 WP9,
//! Q-PRA2i): builds the engine if stale, fetches `PostgreSQL` if missing,
//! then runs `clientledger-cli qa seed` into `<out>/data` with file-based
//! secret stores (never the owner's keychain). Sign-in details go to the
//! git-ignored `<out>/credentials.txt`. Prints the command that opens the
//! QA company in the app.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

/// What `qa-company` does, as data (unit tested).
#[derive(Debug, PartialEq, Eq)]
pub struct Plan {
    /// The output folder.
    pub out: PathBuf,
    /// Remove an earlier QA company in `out` first.
    pub fresh: bool,
}

/// Splits the arguments.
///
/// # Errors
/// An unknown option or a missing value.
pub fn plan(args: &[String]) -> Result<Plan> {
    let mut out = PathBuf::from("target").join("qa-company");
    let mut fresh = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(it.next().context("--out needs a folder")?),
            "--fresh" => fresh = true,
            other => bail!("unknown qa-company option `{other}` (use --out <dir>, --fresh)"),
        }
    }
    Ok(Plan { out, fresh })
}

/// `--fresh` deletes only inside the checkout's `target` folder.
fn safe_to_remove(out: &Path, target: &Path) -> bool {
    out.starts_with(target) && out != target
}

/// Entry point.
pub fn qa_company(args: &[String]) -> Result<()> {
    let plan = plan(args)?;
    let root = crate::engine::repo_root()?;
    let out = if plan.out.is_absolute() {
        plan.out.clone()
    } else {
        root.join(&plan.out)
    };
    let data = out.join("data");
    let credentials = out.join("credentials.txt");
    if data.exists() {
        if !plan.fresh {
            bail!(
                "{} already holds a QA company; pass --fresh to build a new one",
                data.display()
            );
        }
        if !safe_to_remove(&out, &root.join("target")) {
            bail!(
                "--fresh only removes a folder inside target/ ({})",
                out.display()
            );
        }
        std::fs::remove_dir_all(&data).with_context(|| format!("removing {}", data.display()))?;
        let _ = std::fs::remove_file(&credentials);
    }
    crate::engine::engine(&[]).context("building the engine failed")?;
    crate::pg_fetch::pg_fetch(&[]).context("fetching PostgreSQL failed")?;
    let started = Instant::now();
    let status = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(&root)
        .args([
            "run",
            "-p",
            "clientledger-cli",
            // Security review minor 9: `qa seed` exists only in this build.
            "--features",
            "qa-seed",
            "--",
            "qa",
            "seed",
            "--data-dir",
        ])
        .arg(&data)
        .arg("--credentials")
        .arg(&credentials)
        .status()
        .context("could not start cargo")?;
    if !status.success() {
        bail!("the QA seed failed ({status})");
    }
    println!(
        "qa-company: ready in {:.0?} (including the CLI build)",
        started.elapsed()
    );
    println!("qa-company: sign-in details: {}", credentials.display());
    println!("qa-company: open it with:");
    println!("  cargo xtask run -- --data-dir \"{}\"", data.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn defaults_and_options() {
        let p = plan(&[]).unwrap();
        assert_eq!(p.out, Path::new("target").join("qa-company"));
        assert!(!p.fresh);
        let p = plan(&v(&["--out", "x", "--fresh"])).unwrap();
        assert_eq!(p.out, PathBuf::from("x"));
        assert!(p.fresh);
        assert!(plan(&v(&["--out"])).is_err());
        assert!(plan(&v(&["--wipe"])).is_err());
    }

    #[test]
    fn fresh_removes_only_inside_target() {
        let t = Path::new("/repo/target");
        assert!(safe_to_remove(Path::new("/repo/target/qa-company"), t));
        assert!(!safe_to_remove(t, t));
        assert!(!safe_to_remove(Path::new("/repo/qa-company"), t));
        assert!(!safe_to_remove(Path::new("/elsewhere"), t));
    }
}
