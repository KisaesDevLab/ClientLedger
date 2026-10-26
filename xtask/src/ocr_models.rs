//! `cargo xtask ocr-models [--force]`: put the pinned Tesseract model data
//! into `target/ocr/<triple>/tessdata/` (A03, plan D1 "Data").
//!
//! `tessdata_fast` English (`eng`) and orientation/script detection (`osd`),
//! Apache-2.0, from one commit named in `ocr.lock`, SHA-256 checked and
//! cached in `target/ocr/.cache`. The repository's license goes to
//! `LICENSES/tessdata-LICENSE.txt`. The app bundles these files and never
//! downloads model data at run time.
//!
//! Run after `ocr-build` (which rebuilds the folder but keeps the models it
//! finds there); an installed file whose hash still matches is left alone.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

use crate::fetch_lock::{Entry, fetch_verified, sha256_file};
use crate::ocr_build::{LOCK_FILE, Lock, host_triple};

/// The model files a complete install has.
pub const REQUIRED: [&str; 2] = ["eng.traineddata", "osd.traineddata"];

/// Where a lock line's file goes, relative to `target/ocr/<triple>/`, or
/// `None` for a name `ocr.lock` may not use.
#[must_use]
pub fn install_name(target: &str) -> Option<PathBuf> {
    if target == "tessdata-LICENSE" {
        return Some(Path::new("LICENSES").join("tessdata-LICENSE.txt"));
    }
    let ok = target
        .strip_suffix(".traineddata")
        .is_some_and(|lang| !lang.is_empty() && lang.bytes().all(|b| b.is_ascii_lowercase()));
    ok.then(|| Path::new("tessdata").join(target))
}

/// The lock's model lines, checked: every required model and the license
/// present, nothing else.
///
/// # Errors
/// A model or the license is missing from `ocr.lock`.
pub fn model_entries(lock: &Lock) -> Result<Vec<(Entry, PathBuf)>> {
    let mut out = Vec::new();
    for e in &lock.tessdata {
        let Some(rel) = install_name(&e.target) else {
            bail!("{LOCK_FILE}: unexpected model line `{}`", e.target);
        };
        out.push((e.clone(), rel));
    }
    for name in REQUIRED.iter().copied().chain(["tessdata-LICENSE"]) {
        if !out.iter().any(|(e, _)| e.target == name) {
            bail!("{LOCK_FILE}: no `{name}` line");
        }
    }
    Ok(out)
}

/// Install the models into `out` (`target/ocr/<triple>`). Returns how many
/// files were (re)written.
///
/// # Errors
/// A download problem or a hash mismatch.
pub fn install(lock: &Lock, base: &Path, out: &Path, force: bool) -> Result<usize> {
    let cache = base.join(".cache");
    let mut written = 0;
    for (entry, rel) in model_entries(lock)? {
        let dest = out.join(&rel);
        if !force && dest.is_file() && sha256_file(&dest)? == entry.sha256 {
            continue;
        }
        let file = fetch_verified("ocr-models", LOCK_FILE, &entry, &cache, "bin")?;
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let part = dest.with_extension("part");
        fs::copy(&file, &part).with_context(|| format!("copy to {}", part.display()))?;
        fs::rename(&part, &dest)?;
        written += 1;
    }
    Ok(written)
}

/// Entry point for `cargo xtask ocr-models [--force]`.
///
/// # Errors
/// An unsupported host, a missing lock line, a download problem or a hash
/// mismatch.
pub fn ocr_models(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    if let Some(bad) = args.iter().find(|a| *a != "--force") {
        bail!("unknown ocr-models option `{bad}`");
    }
    let root = crate::engine::repo_root()?;
    let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).context(LOCK_FILE)?)?;
    let base = root.join("target").join("ocr");
    let out = base.join(host_triple()?);
    let written = install(&lock, &base, &out, force)?;
    println!(
        "ocr-models: tessdata_fast {} in {} ({written} file(s) written)",
        lock.tessdata_commit.as_deref().unwrap_or("?"),
        out.join("tessdata").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "9218e62793116d42a9f6d14cd9348518b27f382096eea3d0f2d1a24616bb5884";
    const COMMIT: &str = "87416418657359cb625c412a48b6e1d6d41c29bd";

    fn lock_with(models: &str) -> Result<Lock> {
        Lock::parse(&format!(
            "tesseract = 5.5.3\nleptonica = 1.87.0\n\
             tesseract {SHA} https://github.com/tesseract-ocr/tesseract/archive/refs/tags/5.5.3.tar.gz\n\
             leptonica {SHA} https://github.com/DanBloomberg/leptonica/releases/download/1.87.0/leptonica-1.87.0.tar.gz\n{models}"
        ))
    }

    fn line(name: &str, commit: &str, file: &str) -> String {
        format!(
            "{name} {SHA} https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{commit}/{file}\n"
        )
    }

    #[test]
    fn the_committed_lock_pins_english_osd_and_the_license() {
        let root = crate::engine::repo_root().unwrap();
        let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        let entries = model_entries(&lock).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(lock.tessdata_commit.as_deref().map(str::len), Some(40));
        let eng = entries
            .iter()
            .find(|(e, _)| e.target == "eng.traineddata")
            .unwrap();
        assert_eq!(eng.1, Path::new("tessdata").join("eng.traineddata"));
    }

    #[test]
    fn model_lines_must_name_the_pinned_commit() {
        let ok = format!(
            "tessdata = {COMMIT}\n{}{}{}",
            line("eng.traineddata", COMMIT, "eng.traineddata"),
            line("osd.traineddata", COMMIT, "osd.traineddata"),
            line("tessdata-LICENSE", COMMIT, "LICENSE"),
        );
        assert!(model_entries(&lock_with(&ok).unwrap()).is_ok());
        let other = "0".repeat(40);
        let wrong = format!(
            "tessdata = {COMMIT}\n{}",
            line("eng.traineddata", &other, "eng.traineddata")
        );
        assert!(lock_with(&wrong).is_err());
        let no_commit = line("eng.traineddata", COMMIT, "eng.traineddata");
        assert!(lock_with(&no_commit).is_err());
        assert!(lock_with("tessdata = 8741641\n").is_err(), "short hash");
        let missing_osd = format!(
            "tessdata = {COMMIT}\n{}{}",
            line("eng.traineddata", COMMIT, "eng.traineddata"),
            line("tessdata-LICENSE", COMMIT, "LICENSE"),
        );
        assert!(model_entries(&lock_with(&missing_osd).unwrap()).is_err());
    }

    #[test]
    fn install_names_stay_inside_the_output() {
        assert_eq!(
            install_name("osd.traineddata"),
            Some(Path::new("tessdata").join("osd.traineddata"))
        );
        assert!(install_name("tessdata-LICENSE").is_some());
        for bad in [
            "../x.traineddata",
            ".traineddata",
            "eng.bin",
            "Eng.traineddata",
            "a/b.traineddata",
        ] {
            assert_eq!(install_name(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_matching_installed_file_is_not_fetched_again() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let files = [
            ("eng.traineddata", "tessdata/eng.traineddata"),
            ("osd.traineddata", "tessdata/osd.traineddata"),
            ("tessdata-LICENSE", "LICENSES/tessdata-LICENSE.txt"),
        ];
        let mut text = format!("tessdata = {COMMIT}\n");
        for (name, rel) in files {
            let p = out.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, name.as_bytes()).unwrap();
            let sha = sha256_file(&p).unwrap();
            text += &line(name, COMMIT, "x").replacen(SHA, &sha, 1);
        }
        let lock = lock_with(&text).unwrap();
        // Nothing to download: a fetch would fail (offline tests, bad url).
        assert_eq!(install(&lock, dir.path(), &out, false).unwrap(), 0);
    }
}
