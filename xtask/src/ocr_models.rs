//! `cargo xtask ocr-models [--force]`: put the pinned Tesseract model data
//! into `target/ocr/<triple>/tessdata/` (A03, plan D1 "Data").
//!
//! `tessdata_fast` English (`eng`) and orientation/script detection (`osd`)
//! from one commit, and `tessdata_best` English installed as `engbest`
//! from another (A03 WP-A2 round 2, Q-A03b: pages are read with both
//! English models, re-reads with the best one), all Apache-2.0, named in
//! `ocr.lock`, SHA-256 checked and cached in `target/ocr/.cache`. Each
//! repository's license goes to `LICENSES/`. The app bundles these files
//! and never downloads model data at run time.
//!
//! Run after `ocr-build` (which rebuilds the folder but keeps the models it
//! finds there); an installed file whose hash still matches is left alone.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

use crate::fetch_lock::{Entry, fetch_verified, sha256_file};
use crate::ocr_build::{LOCK_FILE, Lock, host_triple};

/// The model files a complete install has.
pub const REQUIRED: [&str; 3] = ["eng.traineddata", "engbest.traineddata", "osd.traineddata"];

/// The license lines a complete install has.
const LICENSES: [&str; 2] = ["tessdata-LICENSE", "tessdata_best-LICENSE"];

/// Where a lock line's file goes, relative to `target/ocr/<triple>/`, or
/// `None` for a name `ocr.lock` may not use.
#[must_use]
pub fn install_name(target: &str) -> Option<PathBuf> {
    if LICENSES.contains(&target) {
        return Some(Path::new("LICENSES").join(format!("{target}.txt")));
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
    for name in REQUIRED.iter().chain(&LICENSES).copied() {
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
        "ocr-models: tessdata_fast {}, tessdata_best {} in {} ({written} file(s) written)",
        lock.tessdata_commit.as_deref().unwrap_or("?"),
        lock.tessdata_best_commit.as_deref().unwrap_or("?"),
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

    const BEST: &str = "e12c65a915945e4c28e237a9b52bc4a8f39a0cec";

    fn line(name: &str, commit: &str, file: &str) -> String {
        format!(
            "{name} {SHA} https://raw.githubusercontent.com/tesseract-ocr/tessdata_fast/{commit}/{file}\n"
        )
    }

    fn best_line(name: &str, commit: &str, file: &str) -> String {
        format!(
            "{name} {SHA} https://raw.githubusercontent.com/tesseract-ocr/tessdata_best/{commit}/{file}\n"
        )
    }

    #[test]
    fn the_committed_lock_pins_both_english_models_osd_and_the_licenses() {
        let root = crate::engine::repo_root().unwrap();
        let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        let entries = model_entries(&lock).unwrap();
        assert_eq!(entries.len(), 5);
        assert_eq!(lock.tessdata_commit.as_deref().map(str::len), Some(40));
        assert_eq!(lock.tessdata_best_commit.as_deref().map(str::len), Some(40));
        let eng = entries
            .iter()
            .find(|(e, _)| e.target == "eng.traineddata")
            .unwrap();
        assert_eq!(eng.1, Path::new("tessdata").join("eng.traineddata"));
        let best = entries
            .iter()
            .find(|(e, _)| e.target == "engbest.traineddata")
            .unwrap();
        assert!(best.0.url.contains("/tessdata_best/"));
        assert_eq!(best.1, Path::new("tessdata").join("engbest.traineddata"));
    }

    #[test]
    fn model_lines_must_name_the_pinned_commit() {
        let ok = format!(
            "tessdata = {COMMIT}\ntessdata_best = {BEST}\n{}{}{}{}{}",
            line("eng.traineddata", COMMIT, "eng.traineddata"),
            best_line("engbest.traineddata", BEST, "eng.traineddata"),
            line("osd.traineddata", COMMIT, "osd.traineddata"),
            line("tessdata-LICENSE", COMMIT, "LICENSE"),
            best_line("tessdata_best-LICENSE", BEST, "LICENSE"),
        );
        assert!(model_entries(&lock_with(&ok).unwrap()).is_ok());
        let other = "0".repeat(40);
        let wrong = format!(
            "tessdata = {COMMIT}\n{}",
            line("eng.traineddata", &other, "eng.traineddata")
        );
        assert!(lock_with(&wrong).is_err());
        // A best model must name the tessdata_best commit, not the fast one.
        let wrong_best = format!(
            "tessdata = {COMMIT}\ntessdata_best = {BEST}\n{}",
            best_line("engbest.traineddata", COMMIT, "eng.traineddata")
        );
        assert!(lock_with(&wrong_best).is_err());
        let no_best_commit = format!(
            "tessdata = {COMMIT}\n{}",
            best_line("engbest.traineddata", BEST, "eng.traineddata")
        );
        assert!(lock_with(&no_best_commit).is_err());
        let no_commit = line("eng.traineddata", COMMIT, "eng.traineddata");
        assert!(lock_with(&no_commit).is_err());
        assert!(lock_with("tessdata = 8741641\n").is_err(), "short hash");
        assert!(
            lock_with("tessdata_best = 8741641\n").is_err(),
            "short hash"
        );
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
        assert_eq!(
            install_name("tessdata_best-LICENSE"),
            Some(Path::new("LICENSES").join("tessdata_best-LICENSE.txt"))
        );
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
            ("eng.traineddata", "tessdata/eng.traineddata", false),
            ("engbest.traineddata", "tessdata/engbest.traineddata", true),
            ("osd.traineddata", "tessdata/osd.traineddata", false),
            ("tessdata-LICENSE", "LICENSES/tessdata-LICENSE.txt", false),
            (
                "tessdata_best-LICENSE",
                "LICENSES/tessdata_best-LICENSE.txt",
                true,
            ),
        ];
        let mut text = format!("tessdata = {COMMIT}\ntessdata_best = {BEST}\n");
        for (name, rel, best) in files {
            let p = out.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, name.as_bytes()).unwrap();
            let sha = sha256_file(&p).unwrap();
            let l = if best {
                best_line(name, BEST, "x")
            } else {
                line(name, COMMIT, "x")
            };
            text += &l.replacen(SHA, &sha, 1);
        }
        let lock = lock_with(&text).unwrap();
        // Nothing to download: a fetch would fail (offline tests, bad url).
        assert_eq!(install(&lock, dir.path(), &out, false).unwrap(), 0);
    }
}
