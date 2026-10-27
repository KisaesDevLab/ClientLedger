//! `cargo xtask docread-accuracy` (A03 step 13, plan D6 "Harness"): builds
//! the **normal** reading worker optimised (profile `docread-bench`, no test
//! hooks), writes the synthetic set if it is missing (`gen-invoices
//! --holdout`), then runs `cl-host/tests/docread_accuracy.rs` under the same
//! profile. The report lands in `target/docread-accuracy/` (never committed).
//!
//! It runs the statements harness too (`docread_statement_accuracy.rs`,
//! A06: lines ≥ 95 % on text PDFs, ≥ 85 % on scans, photos 75 % seen / 70 % held-out, Q-A06h);
//! `--statements` runs only that one, report only.
//!
//! `--gate` fails on any Q-285 minimum not met (and on missing tesseract);
//! without it the numbers are only reported. `--tessdata DIR` reads with
//! other model data (step 12a's fast-vs-best comparison); `--routes
//! scan,photo` limits the paths read.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    gate: bool,
    regen: bool,
    stress: bool,
    /// Only the statements run (report only; `--gate` runs both).
    statements: bool,
    tessdata: Option<PathBuf>,
    routes: Option<String>,
    report: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--gate" => o.gate = true,
            "--regen" => o.regen = true,
            "--stress" => o.stress = true,
            "--statements" => o.statements = true,
            "--tessdata" => {
                o.tessdata = Some(std::path::absolute(
                    it.next().context("--tessdata needs a folder")?,
                )?);
            }
            "--routes" => o.routes = Some(it.next().context("--routes needs a list")?.clone()),
            "--report" => {
                o.report = Some(std::path::absolute(
                    it.next().context("--report needs a folder")?,
                )?);
            }
            other => bail!(
                "unknown option `{other}`\nusage: cargo xtask docread-accuracy [--gate] [--regen] [--stress] [--statements] [--tessdata DIR] [--routes text,scan,photo] [--report DIR]"
            ),
        }
    }
    if o.gate && (o.routes.is_some() || o.stress || o.statements) {
        // A gate on part of the set proves nothing (A03 review S3).
        bail!(
            "--gate reads the whole set: it can't be combined with --routes, --stress or --statements"
        );
    }
    Ok(o)
}

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

/// Whether `set` already holds a full set with held-out layouts and pictures.
fn set_ready(set: &Path) -> bool {
    full(&set.join("set.json"), crate::gen_invoices::layout_count())
        && full(
            &set.join("statements").join("set.json"),
            crate::gen_invoices::statement_layout_count(),
        )
        && set.join("stress").join("ten-pages-scanned.pdf").is_file()
}

/// Whether a `set.json` describes a full set of `layouts` with held-out
/// layouts and pictures.
fn full(set_json: &Path, layouts: usize) -> bool {
    std::fs::read(set_json)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| {
            v["holdout"].as_bool() == Some(true)
                && v["scans"].as_bool() == Some(true)
                && v["layouts"].as_array().is_some_and(|l| l.len() == layouts)
        })
}

#[allow(clippy::too_many_lines)]
pub fn docread_accuracy(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    let root = crate::engine::repo_root()?;
    let target = crate::meta::target_dir(&root);
    let set = root.join("target").join("docread-synth");
    if o.stress {
        crate::gen_invoices::gen_invoices(&["--stress-only".to_string()])?;
    } else if o.statements
        && !o.regen
        && !full(
            &set.join("statements").join("set.json"),
            crate::gen_invoices::statement_layout_count(),
        )
    {
        crate::gen_invoices::gen_invoices(&[
            "--holdout".to_string(),
            "--statements-only".to_string(),
        ])?;
    } else if !o.statements && (o.regen || !set_ready(&set)) {
        crate::gen_invoices::gen_invoices(&["--holdout".to_string()])?;
    }
    println!("docread-accuracy: building the reading worker (profile docread-bench)");
    let status = cargo()
        .args([
            "build",
            "--quiet",
            "-p",
            "clientledger-docread",
            "--profile",
            "docread-bench",
        ])
        .status()
        .context("run cargo build")?;
    if !status.success() {
        bail!("building the reading worker failed ({status})");
    }
    let worker = target.join("docread-bench").join(if cfg!(windows) {
        "clientledger-docread.exe"
    } else {
        "clientledger-docread"
    });
    if !worker.is_file() {
        bail!("the worker was not built at {}", worker.display());
    }
    let triple = crate::ocr_build::host_triple()?;
    let ocr = root.join("target").join("ocr").join(triple);
    let pdfium = root.join("target").join("pdfium").join(triple);
    // The bills and receipts run (A03), then the statements run (A06).
    let runs: [(&str, &str, bool); 2] = [
        ("docread_accuracy", "reading_accuracy", !o.statements),
        (
            "docread_statement_accuracy",
            "statement_accuracy",
            !o.stress,
        ),
    ];
    for (file, filter, wanted) in runs {
        if !wanted {
            continue;
        }
        let mut cmd = cargo();
        cmd.args([
            "test",
            "--quiet",
            "-p",
            "cl-host",
            "--profile",
            "docread-bench",
            "--test",
            file,
            "--",
            "--nocapture",
            filter,
        ])
        .env("CL_OFFLINE_TESTS", "1")
        .env("CL_DOCREAD_ACCURACY", "1")
        .env("CL_DOCREAD_WORKER", &worker)
        .env("CL_DOCREAD_SET", &set)
        .env("CL_OCR_PATH", &ocr)
        .env("CL_PDFIUM_PATH", &pdfium);
        if o.gate {
            cmd.env("CL_DOCREAD_GATE", "1")
                .env("CL_OCR_REQUIRE", "1")
                .env("CL_PDFIUM_REQUIRE", "1");
        }
        if let Some(t) = &o.tessdata {
            cmd.env("CL_DOCREAD_TESSDATA", t);
        }
        if let Some(r) = &o.routes {
            cmd.env("CL_DOCREAD_ROUTES", r);
        }
        if let Some(r) = &o.report {
            cmd.env("CL_DOCREAD_REPORT", r);
        } else if o.stress {
            cmd.env(
                "CL_DOCREAD_REPORT",
                root.join("target").join("docread-accuracy").join("stress"),
            );
        }
        if o.stress {
            cmd.env("CL_DOCREAD_STRESS_ONLY", "1");
        }
        let status = cmd.status().context("run cargo test")?;
        if !status.success() {
            bail!(
                "the reading accuracy run ({file}) failed ({status}); see target/docread-accuracy/"
            );
        }
    }
    println!("docread-accuracy: reports in target/docread-accuracy/ (report.md, statements.md)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        let o = parse(&["--routes".into(), "scan".into()]).unwrap();
        assert!(!o.gate && !o.regen);
        assert_eq!(o.routes.as_deref(), Some("scan"));
        assert!(parse(&["--gate".into()]).unwrap().gate);
        // A gate is never run on part of the set (review S3).
        assert!(parse(&["--gate".into(), "--routes".into(), "scan".into()]).is_err());
        assert!(parse(&["--stress".into(), "--gate".into()]).is_err());
        assert!(parse(&["--statements".into()]).unwrap().statements);
        assert!(parse(&["--statements".into(), "--gate".into()]).is_err());
        assert!(parse(&["--tessdata".into()]).is_err());
        assert!(parse(&["-x".into()]).is_err());
    }

    #[test]
    fn a_partial_set_is_regenerated() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!set_ready(dir.path()));
        std::fs::write(
            dir.path().join("set.json"),
            r#"{"holdout": false, "scans": true, "layouts": []}"#,
        )
        .unwrap();
        assert!(!set_ready(dir.path()));
    }
}
