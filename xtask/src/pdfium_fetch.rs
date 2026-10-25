//! `cargo xtask pdfium-fetch [--force]`: download the pinned `PDFium` shared
//! library for the host platform into `target/pdfium/<triple>/` (P17, Q-13).
//!
//! Archives and their SHA-256 hashes come from `pdfium-binaries.lock`. A
//! download whose hash does not match is deleted and never unpacked. Only the
//! library itself, `LICENSE` and `licenses/*.txt` (the third-party notices
//! that must ship beside it) are taken from the archive; headers, import
//! libraries and build files are skipped. Archive paths are matched against
//! that fixed list and never used to build an output path, so nothing in the
//! archive can write outside the output folder.
//!
//! The app finds the result in debug builds (`cl-ui` `doc_worker`); release
//! layouts copy it to `<exe_dir>/pdfium/` (`xtask preview`, P35 installers).

use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::fetch_lock::{Entry, fetch_verified, key_value, parse_entry};

/// Name of the pin file at the repository root.
const LOCK_FILE: &str = "pdfium-binaries.lock";

/// Marker written inside a finished `target/pdfium/<triple>/` directory.
pub const STAMP: &str = "pdfium-fetch.stamp";

/// Parsed `pdfium-binaries.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// The Chromium build number of the release (`chromium/<version>`).
    pub version: String,
    pub entries: Vec<Entry>,
}

impl Lock {
    /// Parse the lock file text.
    ///
    /// # Errors
    /// A missing or non-numeric version, a malformed line, or a url that is
    /// not this version's bblanchon release.
    pub fn parse(text: &str) -> Result<Lock> {
        let mut version = None;
        let mut entries = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(v) = key_value(LOCK_FILE, n + 1, line, "version")? {
                version = Some(v);
                continue;
            }
            entries.push(parse_entry(LOCK_FILE, n + 1, line)?);
        }
        let version = version.ok_or_else(|| anyhow!("{LOCK_FILE}: missing `version = ...`"))?;
        if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit()) {
            bail!("{LOCK_FILE}: version must be a Chromium build number such as 7881");
        }
        let prefix = format!(
            "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F{version}/"
        );
        for e in &entries {
            if !e.url.starts_with(&prefix) {
                bail!(
                    "{LOCK_FILE}: {} is not the chromium/{version} release",
                    e.url
                );
            }
        }
        Ok(Lock { version, entries })
    }

    /// The entry for a target triple.
    #[must_use]
    pub fn entry(&self, target: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.target == target)
    }
}

/// The host's target triple and its library's place in the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    pub triple: &'static str,
    /// The library inside the archive.
    pub archive_lib: &'static str,
    /// The library's file name.
    pub lib_name: &'static str,
}

fn host_for(os: &str, arch: &str) -> Result<Host> {
    Ok(match (os, arch) {
        ("windows", "x86_64") => Host {
            triple: "x86_64-pc-windows-msvc",
            archive_lib: "bin/pdfium.dll",
            lib_name: "pdfium.dll",
        },
        ("macos", "aarch64") => Host {
            triple: "aarch64-apple-darwin",
            archive_lib: "lib/libpdfium.dylib",
            lib_name: "libpdfium.dylib",
        },
        ("macos", "x86_64") => Host {
            triple: "x86_64-apple-darwin",
            archive_lib: "lib/libpdfium.dylib",
            lib_name: "libpdfium.dylib",
        },
        ("linux", "x86_64") => Host {
            triple: "x86_64-unknown-linux-gnu",
            archive_lib: "lib/libpdfium.so",
            lib_name: "libpdfium.so",
        },
        _ => bail!("PDFium is not pinned for {os}/{arch}"),
    })
}

/// The host platform.
///
/// # Errors
/// A platform `ClientLedger` does not ship.
pub fn host() -> Result<Host> {
    host_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Where an archive member goes, relative to the output folder, or `None`
/// to skip it. Only the library, `LICENSE` and plain `licenses/<file>` names
/// are taken; anything else (headers, import libraries, nested or odd
/// paths) is skipped.
#[must_use]
pub fn keep(host: &Host, member: &str) -> Option<PathBuf> {
    let member = member.strip_prefix("./").unwrap_or(member);
    if member == host.archive_lib {
        return Some(PathBuf::from(host.lib_name));
    }
    if member == "LICENSE" {
        return Some(PathBuf::from("LICENSE"));
    }
    let name = member.strip_prefix("licenses/")?;
    let plain = !name.is_empty()
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
    plain.then(|| Path::new("licenses").join(name))
}

/// Unpack the kept members of the `.tgz` into `dest`.
fn unpack(host: &Host, tgz: &Path, dest: &Path) -> Result<Vec<PathBuf>> {
    let gz = flate2::read::GzDecoder::new(fs::File::open(tgz)?);
    let mut archive = tar::Archive::new(gz);
    let mut written = Vec::new();
    for entry in archive.entries().context("read archive")? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let member = entry.path()?.to_string_lossy().replace('\\', "/");
        let Some(rel) = keep(host, &member) else {
            continue;
        };
        let out = dest.join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&out)
            .with_context(|| format!("write {}", out.display()))?
            .write_all(&bytes)?;
        written.push(rel);
    }
    Ok(written)
}

fn verify_layout(host: &Host, root: &Path) -> Result<()> {
    for f in [
        PathBuf::from(host.lib_name),
        PathBuf::from("LICENSE"),
        Path::new("licenses").join("pdfium.txt"),
        Path::new("licenses").join("freetype.txt"),
    ] {
        if !root.join(&f).is_file() {
            bail!("expected {} in the PDFium archive", f.display());
        }
    }
    Ok(())
}

/// The folder `pdfium-fetch` fills for `host`.
///
/// # Errors
/// The repository root cannot be found.
pub fn out_dir(host: &Host) -> Result<PathBuf> {
    Ok(crate::engine::repo_root()?
        .join("target")
        .join("pdfium")
        .join(host.triple))
}

/// Entry point for `cargo xtask pdfium-fetch [--force]`.
///
/// # Errors
/// An unsupported host, a missing lock entry, a download problem, a hash
/// mismatch, or an archive without the library or its licenses.
pub fn pdfium_fetch(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    if let Some(bad) = args.iter().find(|a| *a != "--force") {
        bail!("unknown pdfium-fetch option `{bad}`");
    }
    let root = crate::engine::repo_root()?;
    let host = host()?;
    let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).context(LOCK_FILE)?)?;
    let entry = lock
        .entry(host.triple)
        .ok_or_else(|| anyhow!("{LOCK_FILE} has no entry for {}", host.triple))?;

    let base = root.join("target").join("pdfium");
    let out = base.join(host.triple);
    let stamp = format!("chromium/{} {}\n", lock.version, entry.sha256);
    if !force && fs::read_to_string(out.join(STAMP)).is_ok_and(|s| s == stamp) {
        println!(
            "pdfium-fetch: chromium/{} already present in {}",
            lock.version,
            out.display()
        );
        return Ok(());
    }

    let tgz = fetch_verified(
        "pdfium-fetch",
        LOCK_FILE,
        entry,
        &base.join(".cache"),
        "tgz",
    )?;
    let work = base.join(".work").join(host.triple);
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    let written = unpack(&host, &tgz, &work)?;
    verify_layout(&host, &work)?;
    fs::File::create(work.join(STAMP))?.write_all(stamp.as_bytes())?;

    let _ = fs::remove_dir_all(&out);
    fs::rename(&work, &out).context("move into target/pdfium")?;
    println!(
        "pdfium-fetch: PDFium chromium/{} ready in {} ({} files)",
        lock.version,
        out.display(),
        written.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08";
    const URL: &str =
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/a.tgz";

    #[test]
    fn parses_lock_text() {
        let lock = Lock::parse(&format!(
            "# c\nversion = 7881\nx86_64-pc-windows-msvc {SHA} {URL}\n"
        ))
        .unwrap();
        assert_eq!(lock.version, "7881");
        assert_eq!(lock.entry("x86_64-pc-windows-msvc").unwrap().sha256, SHA);
        assert!(lock.entry("x86_64-apple-darwin").is_none());
    }

    #[test]
    fn rejects_bad_lock_text() {
        let other = URL.replace("7881", "7000");
        for b in [
            format!("t {SHA} {URL}\n"),
            format!("version = 78a1\nt {SHA} {URL}\n"),
            format!("version = 7881\nt {SHA} {other}\n"),
            format!("version = 7881\nt {SHA} https://example.test/pdfium.tgz\n"),
            format!("version = 7881\nt {} {URL}\n", &SHA[1..]),
        ] {
            assert!(Lock::parse(&b).is_err(), "should reject: {b}");
        }
    }

    #[test]
    fn the_committed_lock_covers_every_target_and_matches_pdfium_render() {
        let root = crate::engine::repo_root().unwrap();
        let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        for (os, arch) in [
            ("windows", "x86_64"),
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
        ] {
            let h = host_for(os, arch).unwrap();
            assert!(lock.entry(h.triple).is_some(), "missing {}", h.triple);
        }
        // The library's API must be the one pdfium-render binds.
        let ui = fs::read_to_string(root.join("crates/cl-ui/Cargo.toml")).unwrap();
        assert!(
            ui.contains(&format!("\"pdfium_{}\"", lock.version)),
            "cl-ui's pdfium-render feature must be pdfium_{}",
            lock.version
        );
        assert!(host_for("windows", "aarch64").is_err());
    }

    #[test]
    fn keeps_only_the_library_and_its_licenses() {
        let win = host_for("windows", "x86_64").unwrap();
        let linux = host_for("linux", "x86_64").unwrap();
        assert_eq!(
            keep(&win, "bin/pdfium.dll"),
            Some(PathBuf::from("pdfium.dll"))
        );
        assert_eq!(
            keep(&win, "./bin/pdfium.dll"),
            Some(PathBuf::from("pdfium.dll"))
        );
        assert_eq!(
            keep(&linux, "lib/libpdfium.so"),
            Some(PathBuf::from("libpdfium.so"))
        );
        assert_eq!(keep(&win, "LICENSE"), Some(PathBuf::from("LICENSE")));
        assert_eq!(
            keep(&win, "licenses/freetype.txt"),
            Some(Path::new("licenses").join("freetype.txt"))
        );
        for skip in [
            "lib/pdfium.dll.lib",
            "include/fpdfview.h",
            "args.gn",
            "PDFiumConfig.cmake",
            "VERSION",
            "lib/libpdfium.so",
            "licenses/../evil.txt",
            "licenses/sub/x.txt",
            "licenses/.hidden",
            "licenses/",
            "../LICENSE",
        ] {
            assert_eq!(keep(&win, skip), None, "{skip}");
        }
    }

    #[test]
    fn unpacks_a_synthetic_archive_without_extra_files() {
        let dir = tempfile::tempdir().unwrap();
        let tgz = dir.path().join("a.tgz");
        {
            let gz = flate2::write::GzEncoder::new(
                fs::File::create(&tgz).unwrap(),
                flate2::Compression::fast(),
            );
            let mut b = tar::Builder::new(gz);
            for (name, body) in [
                ("lib/libpdfium.so", &b"ELF"[..]),
                ("LICENSE", b"mit"),
                ("licenses/pdfium.txt", b"bsd"),
                ("licenses/freetype.txt", b"ftl"),
                ("include/fpdfview.h", b"h"),
                ("args.gn", b"pdf_enable_v8 = false"),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, body).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        let out = dir.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let linux = host_for("linux", "x86_64").unwrap();
        let written = unpack(&linux, &tgz, &out).unwrap();
        assert_eq!(written.len(), 4);
        verify_layout(&linux, &out).unwrap();
        assert!(!out.join("include").exists() && !out.join("args.gn").exists());
        assert_eq!(fs::read(out.join("libpdfium.so")).unwrap(), b"ELF");
        // Without the FreeType notice the layout is refused.
        fs::remove_file(out.join("licenses").join("freetype.txt")).unwrap();
        assert!(verify_layout(&linux, &out).is_err());
    }
}
