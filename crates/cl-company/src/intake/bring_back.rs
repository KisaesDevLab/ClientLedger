//! Handled originals found again by their bytes (PR-A1 S2, N7; Q-PRA1c).
//!
//! - [`find_processed`]: the files under `Processed/<month>/` of an intake
//!   folder whose SHA-256 is wanted (plain folders and plain files only,
//!   never a link; at most [`MAX_LOOKED_AT`] files, none over 10 MB).
//! - [`copy_back`]: after a restore, a document entered after the backup
//!   was made is **copied** back to the folder root (the owner said yes),
//!   so the inbox reads it again. The file in `Processed/` stays; the copy
//!   is made with `create_new` under a free name (` (2)`, …) and its bytes
//!   checked through the same handle; nothing is overwritten.
//! - [`refile_discarded`]: a document whose original already went to
//!   `Processed/` and was then discarded (after a Bank Feed Undo) is put in
//!   `Discarded/`: a verified copy there first, only then is the one in
//!   `Processed/` removed. Never deleted without a verified copy.
//!
//! Nothing here logs a file name.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use super::FileError;
use super::fs::{handle_sha256, open_plain};
use super::handled::{DISCARDED_DIR, PROCESSED_DIR, name_fits, numbered};
use super::scan::MAX_FILE_BYTES;

/// Files looked at, at most, per folder.
pub const MAX_LOOKED_AT: usize = 5000;

/// A plain folder (not a link or junction).
fn plain_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// The files under `root/Processed/*/` whose SHA-256 is in `wanted`:
/// `(sha256, path)`, one per hash.
#[must_use]
pub fn find_processed(root: &Path, wanted: &BTreeSet<String>) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if wanted.is_empty() {
        return out;
    }
    let processed = root.join(PROCESSED_DIR);
    if !plain_dir(&processed) {
        return out;
    }
    let Ok(months) = fs::read_dir(&processed) else {
        return out;
    };
    let mut found = BTreeSet::new();
    let mut looked = 0usize;
    for month in months.flatten() {
        let dir = month.path();
        if !plain_dir(&dir) {
            continue;
        }
        let Ok(files) = fs::read_dir(&dir) else {
            continue;
        };
        for f in files.flatten() {
            looked += 1;
            if looked > MAX_LOOKED_AT {
                return out;
            }
            let path = f.path();
            let Ok(m) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !m.is_file() || m.len() == 0 || m.len() > MAX_FILE_BYTES {
                continue;
            }
            let Ok((file, _)) = open_plain(&path, false) else {
                continue;
            };
            let Ok(sha) = handle_sha256(&file) else {
                continue;
            };
            if wanted.contains(&sha) && found.insert(sha.clone()) {
                out.push((sha, path));
            }
        }
    }
    out
}

/// Copies `src` (SHA-256 `sha256`) into `dir` under its own name, or the
/// first free ` (n)`; the copy's bytes are checked through its handle.
fn copy_into(dir: &Path, src: &Path, sha256: &str) -> Result<PathBuf, FileError> {
    let name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(FileError::NotAFile)?
        .to_string();
    let (mut from, _) = open_plain(src, false)?;
    if handle_sha256(&from)? != sha256 {
        return Err(FileError::Changed);
    }
    for n in 1..10_000 {
        let candidate = numbered(&name, n);
        if !name_fits(&candidate) {
            break;
        }
        let target = dir.join(&candidate);
        let mut to = match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(FileError::Io),
        };
        let ok = io::Seek::rewind(&mut from).is_ok()
            && io::copy(&mut from, &mut to).is_ok()
            && to.flush().is_ok()
            && to.sync_all().is_ok()
            && io::Seek::rewind(&mut to).is_ok()
            && handle_sha256(&to).is_ok_and(|s| s == sha256);
        drop(to);
        if ok {
            return Ok(target);
        }
        let _ = fs::remove_file(&target);
        return Err(FileError::Io);
    }
    Err(FileError::Io)
}

/// Checks `path` is a file in `root/Processed/<month>/`.
fn in_processed(root: &Path, path: &Path) -> bool {
    let processed = root.join(PROCESSED_DIR);
    path.parent()
        .and_then(Path::parent)
        .is_some_and(|p| p == processed)
        && path.parent().is_some_and(plain_dir)
        && plain_dir(&processed)
}

/// After a restore, the owner said yes: `path` (in `Processed/`) is copied
/// back to the folder root. Returns the copy.
///
/// # Errors
/// `NotAFile` (not in `Processed/<month>/`, a link), `Changed`, `Io`.
pub fn copy_back(root: &Path, path: &Path, sha256: &str) -> Result<PathBuf, FileError> {
    if !in_processed(root, path) {
        return Err(FileError::NotAFile);
    }
    copy_into(root, path, sha256)
}

/// N7: `path` (in `Processed/`) is filed in `Discarded/` instead. Returns
/// where it is now.
///
/// # Errors
/// `NotAFile` (not in `Processed/<month>/`, a link, `Discarded` a link),
/// `Changed`, `Io` (the original stays where it was).
pub fn refile_discarded(root: &Path, path: &Path, sha256: &str) -> Result<PathBuf, FileError> {
    if !in_processed(root, path) {
        return Err(FileError::NotAFile);
    }
    let discarded = root.join(DISCARDED_DIR);
    match fs::symlink_metadata(&discarded) {
        Ok(_) if !plain_dir(&discarded) => return Err(FileError::NotAFile),
        Ok(_) => {}
        Err(_) => fs::create_dir(&discarded).map_err(|_| FileError::Io)?,
    }
    let placed = copy_into(&discarded, path, sha256)?;
    if fs::remove_file(path).is_err() {
        // Two copies would show it twice: keep the original.
        let _ = fs::remove_file(&placed);
        return Err(FileError::Io);
    }
    Ok(placed)
}

#[cfg(test)]
mod tests {
    use super::super::sha256_hex;
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%bring back test\n";

    fn processed(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let dir = root.join(PROCESSED_DIR).join("2026-11");
        fs::create_dir_all(&dir).expect("dir");
        let p = dir.join(name);
        fs::write(&p, bytes).expect("write");
        p
    }

    #[test]
    fn handled_files_are_found_by_their_bytes_and_copied_back() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path();
        let p = processed(root, "receipt.pdf", PDF);
        processed(root, "other.pdf", b"%PDF-1.4\n%other\n");
        let sha = sha256_hex(PDF);
        let found = find_processed(root, &BTreeSet::from([sha.clone()]));
        assert_eq!(found, vec![(sha.clone(), p.clone())]);
        assert_eq!(find_processed(root, &BTreeSet::new()), Vec::new());
        // A taken name at the root is never overwritten.
        fs::write(root.join("receipt.pdf"), b"someone else's").expect("w");
        let back = copy_back(root, &p, &sha).expect("copied");
        assert_eq!(back, root.join("receipt (2).pdf"));
        assert_eq!(fs::read(&back).expect("r"), PDF);
        assert!(p.exists(), "the processed file stays");
        assert_eq!(
            fs::read(root.join("receipt.pdf")).expect("r"),
            b"someone else's"
        );
        // Only from Processed/<month>/; changed bytes are refused.
        assert_eq!(
            copy_back(root, &root.join("receipt.pdf"), &sha),
            Err(FileError::NotAFile)
        );
        assert_eq!(copy_back(root, &p, "00"), Err(FileError::Changed));
    }

    #[test]
    fn a_processed_original_discarded_later_moves_to_discarded() {
        let dir = tempfile::tempdir().expect("dir");
        let root = dir.path();
        let p = processed(root, "receipt.pdf", PDF);
        let sha = sha256_hex(PDF);
        let now = refile_discarded(root, &p, &sha).expect("refiled");
        assert_eq!(now, root.join(DISCARDED_DIR).join("receipt.pdf"));
        assert!(!p.exists());
        assert_eq!(fs::read(&now).expect("r"), PDF);
        // Gone now: nothing to refile.
        assert!(refile_discarded(root, &p, &sha).is_err());
    }
}
