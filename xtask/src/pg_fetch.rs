//! `cargo xtask pg-fetch [--force]`: download the pinned `PostgreSQL` binaries
//! for the host platform into `target/pg/<target>/`.
//!
//! Archives and their SHA-256 hashes come from `pg-binaries.lock` (Q-06). A
//! download whose hash does not match is deleted and never unpacked. This is
//! the only place `ClientLedger` touches the network for `PostgreSQL`; tests and
//! the app use the unpacked result.

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Name of the pin file at the repository root.
const LOCK_FILE: &str = "pg-binaries.lock";

/// Marker written inside a finished `target/pg/<target>/` directory.
const STAMP: &str = "pg-fetch.stamp";

/// Executables kept from the server archive (`.exe` is added on Windows).
const KEEP_BIN: [&str; 3] = ["postgres", "initdb", "pg_ctl"];

/// One pinned download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub target: String,
    pub sha256: String,
    pub url: String,
}

/// Parsed `pg-binaries.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    pub version: String,
    pub entries: Vec<Entry>,
}

impl Lock {
    /// Parse the lock file text.
    ///
    /// # Errors
    /// Returns an error for a missing version, a malformed line, or a hash that
    /// is not 64 lowercase hex digits.
    pub fn parse(text: &str) -> Result<Lock> {
        let mut version = None;
        let mut entries = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(v) = line.strip_prefix("version") {
                let v = v.trim_start().strip_prefix('=').map(str::trim);
                version = Some(
                    v.filter(|v| !v.is_empty())
                        .ok_or_else(|| anyhow!("{LOCK_FILE}:{}: bad version line", n + 1))?
                        .to_string(),
                );
                continue;
            }
            let mut parts = line.split_whitespace();
            let (Some(target), Some(sha), Some(url), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                bail!("{LOCK_FILE}:{}: expected `<target> <sha256> <url>`", n + 1);
            };
            if sha.len() != 64
                || !sha
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                bail!(
                    "{LOCK_FILE}:{}: sha256 must be 64 lowercase hex digits",
                    n + 1
                );
            }
            if !url.starts_with("https://") {
                bail!("{LOCK_FILE}:{}: url must be https", n + 1);
            }
            entries.push(Entry {
                target: target.to_string(),
                sha256: sha.to_string(),
                url: url.to_string(),
            });
        }
        let version = version.ok_or_else(|| anyhow!("{LOCK_FILE}: missing `version = ...`"))?;
        if !version.starts_with("16.") {
            bail!("{LOCK_FILE}: version must be a PostgreSQL 16.x release");
        }
        Ok(Lock { version, entries })
    }

    /// The entry for a target directory name such as `windows-x64`.
    #[must_use]
    pub fn entry(&self, target: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.target == target)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// SHA-256 of a file as lowercase hex.
fn sha256_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Should this path (relative to the unpacked root) be deleted by the trim?
///
/// Keeps the three server executables, every runtime library, and all of
/// `share`. Drops import libraries, static libraries, pkg-config files,
/// headers, and bundled GUI toolkit libraries that the server never loads.
#[must_use]
pub fn is_trimmed(rel: &Path) -> bool {
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let Some(first) = comps.first() else {
        return false;
    };
    let name = comps.last().map_or("", String::as_str);
    match first.as_str() {
        "include" => true,
        "bin" if comps.len() == 2 => {
            let stem = name.strip_suffix(".exe").unwrap_or(name);
            let is_dll = Path::new(name)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("dll"));
            if is_dll {
                name.starts_with("wx") || name == "testplug.dll"
            } else {
                !KEEP_BIN.contains(&stem)
            }
        }
        "lib" => {
            comps.iter().any(|c| c == "pkgconfig")
                || Path::new(name)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("a") || e.eq_ignore_ascii_case("lib"))
        }
        _ => false,
    }
}

fn trim_tree(root: &Path) -> Result<()> {
    fn walk(root: &Path, dir: &Path) -> Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            let rel = path.strip_prefix(root)?;
            let is_dir = e.file_type()?.is_dir();
            if is_trimmed(rel) {
                if is_dir {
                    fs::remove_dir_all(&path)?;
                } else {
                    fs::remove_file(&path)?;
                }
            } else if is_dir {
                walk(root, &path)?;
            }
        }
        Ok(())
    }
    walk(root, root)
}

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Download `url` to `dest` with curl (present on Windows 10+, macOS, Linux).
fn download(url: &str, dest: &Path) -> Result<()> {
    println!("==> download {url}");
    let status = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--retry", "3", "--proto", "=https", "--output"])
        .arg(dest)
        .arg(url)
        .status()
        .context("could not start `curl`")?;
    if !status.success() {
        bail!("download failed ({status})");
    }
    Ok(())
}

/// Fetch (or reuse from cache) the archive and verify its hash.
fn fetch_verified(entry: &Entry, cache: &Path) -> Result<PathBuf> {
    fs::create_dir_all(cache)?;
    let file = cache.join(format!("{}.jar", entry.sha256));
    if file.exists() && sha256_file(&file)? == entry.sha256 {
        println!("pg-fetch: cache hit {}", entry.sha256);
        return Ok(file);
    }
    let part = cache.join(format!("{}.part", entry.sha256));
    download(&entry.url, &part)?;
    let actual = sha256_file(&part)?;
    if actual != entry.sha256 {
        let _ = fs::remove_file(&part);
        bail!(
            "SHA-256 mismatch for {}\n  expected {}\n  actual   {actual}\nthe download was deleted; check pg-binaries.lock",
            entry.url,
            entry.sha256
        );
    }
    fs::rename(&part, &file)?;
    Ok(file)
}

/// Unpack the `.txz` inside the Maven jar into `dest`.
fn unpack_jar(jar: &Path, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(jar)?).context("read jar")?;
    let name = zip
        .file_names()
        .filter_map(Result::ok)
        .map(std::borrow::Cow::into_owned)
        .find(|n| {
            Path::new(n)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("txz"))
        })
        .ok_or_else(|| anyhow!("jar contains no .txz archive"))?;
    let entry = zip.by_name(&name)?;
    let mut archive = tar::Archive::new(liblzma::read::XzDecoder::new(entry));
    archive.set_overwrite(true);
    archive.unpack(dest).context("unpack server archive")
}

fn verify_layout(root: &Path) -> Result<()> {
    for b in KEEP_BIN {
        let p = root.join("bin").join(exe_name(b));
        if !p.is_file() {
            bail!("expected {} in the unpacked archive", p.display());
        }
    }
    // Windows archives keep data files in `share/`; Linux and macOS builds use
    // `share/postgresql/`.
    let share = root.join("share");
    if !share.join("postgres.bki").is_file()
        && !share.join("postgresql").join("postgres.bki").is_file()
    {
        bail!("expected postgres.bki under share/ in the unpacked archive");
    }
    Ok(())
}

/// Entry point for `cargo xtask pg-fetch [--force]`.
///
/// # Errors
/// Fails on an unsupported host, a missing lock entry, a download problem, a
/// hash mismatch, or an archive that lacks the expected files.
pub fn pg_fetch(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    if let Some(bad) = args.iter().find(|a| *a != "--force") {
        bail!("unknown pg-fetch option `{bad}`");
    }
    let root = crate::engine::repo_root()?;
    let target = crate::engine::host_target()?;
    let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).context(LOCK_FILE)?)?;
    let entry = lock
        .entry(target.dir)
        .ok_or_else(|| anyhow!("{LOCK_FILE} has no entry for {}", target.dir))?;

    let pg = root.join("target").join("pg");
    let out = pg.join(target.dir);
    let stamp = format!("{} {}\n", lock.version, entry.sha256);
    if !force && fs::read_to_string(out.join(STAMP)).is_ok_and(|s| s == stamp) {
        println!(
            "pg-fetch: {} already present in {}",
            lock.version,
            out.display()
        );
        return Ok(());
    }

    let jar = fetch_verified(entry, &pg.join(".cache"))?;
    let work = pg.join(".work").join(target.dir);
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work)?;
    unpack_jar(&jar, &work)?;
    trim_tree(&work)?;
    verify_layout(&work)?;
    fs::File::create(work.join(STAMP))?.write_all(stamp.as_bytes())?;

    let _ = fs::remove_dir_all(&out);
    fs::rename(&work, &out).context("move into target/pg")?;
    println!(
        "pg-fetch: PostgreSQL {} ready in {}",
        lock.version,
        out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "51c7812dc1af47c9a2ccb64fe74efb88c515cff2da347ce72aab92b4cc8e1191";

    #[test]
    fn parses_lock_text() {
        let text = format!("# c\nversion = 16.15.0\nlinux-x64 {SHA} https://example.test/a.jar\n");
        let lock = Lock::parse(&text).unwrap();
        assert_eq!(lock.version, "16.15.0");
        assert_eq!(lock.entry("linux-x64").unwrap().sha256, SHA);
        assert!(lock.entry("macos-x64").is_none());
    }

    #[test]
    fn rejects_bad_lock_text() {
        let bad = [
            format!("linux-x64 {SHA} https://x.test/a.jar\n"),
            format!("version = 16.1\nlinux-x64 {} https://x.test/a\n", &SHA[1..]),
            format!("version = 16.1\nlinux-x64 {SHA} http://x.test/a\n"),
            format!("version = 16.1\nlinux-x64 {}\n", SHA.to_uppercase()),
            format!("version = 15.4\nlinux-x64 {SHA} https://x.test/a\n"),
        ];
        for b in bad {
            assert!(Lock::parse(&b).is_err(), "should reject: {b}");
        }
    }

    #[test]
    fn the_committed_lock_covers_every_supported_target() {
        let root = crate::engine::repo_root().unwrap();
        let lock = Lock::parse(&fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        for t in ["windows-x64", "macos-arm64", "macos-x64", "linux-x64"] {
            assert!(lock.entry(t).is_some(), "missing {t}");
        }
    }

    #[test]
    fn trim_keeps_server_files_and_drops_the_rest() {
        let t = |p: &str| is_trimmed(Path::new(p));
        assert!(!t("bin/postgres.exe"));
        assert!(!t("bin/initdb"));
        assert!(!t("bin/libpq.dll"));
        assert!(!t("share/postgres.bki"));
        assert!(!t("lib/postgresql/pgcrypto.so"));
        assert!(t("bin/psql.exe"));
        assert!(t("bin/wxbase3211u_vc_x64_custom.dll"));
        assert!(t("lib/libpq.a"));
        assert!(t("lib/libssl.lib"));
        assert!(t("lib/pkgconfig"));
        assert!(t("include"));
    }

    #[test]
    fn hashes_a_file() {
        let dir = std::env::temp_dir().join(format!("cl-xtask-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("abc.txt");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
