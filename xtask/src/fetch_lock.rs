//! Shared pieces of the pinned-binary fetchers (`pg-fetch`, `pdfium-fetch`):
//! lock lines `<target> <sha256> <url>`, SHA-256 of a file, and a cached,
//! hash-verified download. A download whose hash does not match is deleted
//! and never unpacked.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

/// One pinned download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub target: String,
    pub sha256: String,
    pub url: String,
}

/// Parse one `<target> <sha256> <url>` line (`n` is the 1-based line number
/// for messages).
///
/// # Errors
/// A malformed line, a hash that is not 64 lowercase hex digits, or a url
/// that is not https.
pub fn parse_entry(lock_file: &str, n: usize, line: &str) -> Result<Entry> {
    let mut parts = line.split_whitespace();
    let (Some(target), Some(sha), Some(url), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        bail!("{lock_file}:{n}: expected `<target> <sha256> <url>`");
    };
    if sha.len() != 64
        || !sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("{lock_file}:{n}: sha256 must be 64 lowercase hex digits");
    }
    if !url.starts_with("https://") {
        bail!("{lock_file}:{n}: url must be https");
    }
    Ok(Entry {
        target: target.to_string(),
        sha256: sha.to_string(),
        url: url.to_string(),
    })
}

/// The value of a `key = value` line, if `line` is one.
///
/// # Errors
/// The key is present with an empty value.
pub fn key_value(lock_file: &str, n: usize, line: &str, key: &str) -> Result<Option<String>> {
    let Some(rest) = line.strip_prefix(key) else {
        return Ok(None);
    };
    let Some(v) = rest.trim_start().strip_prefix('=').map(str::trim) else {
        return Ok(None);
    };
    if v.is_empty() {
        bail!("{lock_file}:{n}: bad {key} line");
    }
    Ok(Some(v.to_string()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut acc, b| {
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
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

/// Fetch (or reuse from `cache`) the archive and verify its hash. The cached
/// file is `<sha256>.<ext>`.
///
/// # Errors
/// A download problem or a hash mismatch (the download is then deleted).
pub fn fetch_verified(
    tool: &str,
    lock_file: &str,
    entry: &Entry,
    cache: &Path,
    ext: &str,
) -> Result<PathBuf> {
    fs::create_dir_all(cache)?;
    let file = cache.join(format!("{}.{ext}", entry.sha256));
    if file.exists() && sha256_file(&file)? == entry.sha256 {
        println!("{tool}: cache hit {}", entry.sha256);
        return Ok(file);
    }
    let part = cache.join(format!("{}.part", entry.sha256));
    download(&entry.url, &part)?;
    let actual = sha256_file(&part)?;
    if actual != entry.sha256 {
        let _ = fs::remove_file(&part);
        bail!(
            "SHA-256 mismatch for {}\n  expected {}\n  actual   {actual}\nthe download was deleted; check {lock_file}",
            entry.url,
            entry.sha256
        );
    }
    fs::rename(&part, &file)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "51c7812dc1af47c9a2ccb64fe74efb88c515cff2da347ce72aab92b4cc8e1191";

    #[test]
    fn parses_and_refuses_entry_lines() {
        let e = parse_entry("x.lock", 1, &format!("t {SHA} https://x.test/a")).unwrap();
        assert_eq!(
            (e.target.as_str(), e.url.as_str()),
            ("t", "https://x.test/a")
        );
        for bad in [
            format!("t {SHA}"),
            format!("t {SHA} https://x.test/a extra"),
            format!("t {} https://x.test/a", SHA.to_uppercase()),
            format!("t {} https://x.test/a", &SHA[1..]),
            format!("t {SHA} http://x.test/a"),
        ] {
            assert!(parse_entry("x.lock", 1, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn reads_key_value_lines() {
        assert_eq!(
            key_value("x", 1, "version = 7881", "version").unwrap(),
            Some("7881".into())
        );
        assert_eq!(key_value("x", 1, "linux a b", "version").unwrap(), None);
        assert!(key_value("x", 1, "version =  ", "version").is_err());
    }

    #[test]
    fn hashes_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("abc.txt");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
