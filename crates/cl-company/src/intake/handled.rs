//! Moving a handled original out of the way (A01 plan D4, Q-288, Q-A01e):
//! into `Processed/YYYY-MM/` once a record holds its copy, or into
//! `Discarded/` when the owner discards it. Both are inside the intake
//! folder itself.
//!
//! Rules ([`move_handled`]):
//! - The file must be a plain file directly in the folder, and still the
//!   one copied in (its SHA-256): a file whose bytes changed is refused
//!   (`Changed`) and stays where it is.
//! - Nothing is ever overwritten: a taken name becomes ` (2)`, ` (3)`, ….
//! - `Processed`, `Discarded` and the month folder must be plain folders,
//!   never links, so a file never lands outside the intake folder.
//! - A rename is used when it can be. Across volumes the bytes are copied,
//!   the copy's SHA-256 is checked, and only then is the original removed.
//!   If the original can't be removed, the copy is removed instead: there
//!   is never a second copy to bring in again, and the original is never
//!   deleted without a verified copy.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use super::FileError;
use super::fs::{file_sha256, links, online_only};

/// The sub-folder handled originals go to (by month).
pub const PROCESSED_DIR: &str = "Processed";

/// The sub-folder discarded originals go to (flat).
pub const DISCARDED_DIR: &str = "Discarded";

/// Where a handled original goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HandledDir {
    /// `Processed/<YYYY-MM>/` (the local calendar month it was handled).
    Processed {
        /// Year.
        year: i32,
        /// Month, 1–12.
        month: u32,
    },
    /// `Discarded/`.
    Discarded,
}

impl HandledDir {
    /// The folder names below the intake folder.
    #[must_use]
    pub fn parts(self) -> Vec<String> {
        match self {
            HandledDir::Processed { year, month } => {
                vec![PROCESSED_DIR.into(), format!("{year:04}-{month:02}")]
            }
            HandledDir::Discarded => vec![DISCARDED_DIR.into()],
        }
    }

    /// The folder it names inside `root`.
    #[must_use]
    pub fn dir_in(self, root: &Path) -> PathBuf {
        self.parts()
            .iter()
            .fold(root.to_path_buf(), |p, s| p.join(s))
    }
}

/// Why a move failed: the error, and whether the system refused for lack
/// of permission (the folder's health becomes `CantMove`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MoveFailed {
    /// What went wrong.
    pub error: FileError,
    /// The system said "access denied".
    pub denied: bool,
}

impl fmt::Debug for MoveFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoveFailed")
            .field("error", &self.error)
            .field("denied", &self.denied)
            .finish()
    }
}

impl From<FileError> for MoveFailed {
    fn from(error: FileError) -> Self {
        MoveFailed {
            error,
            denied: false,
        }
    }
}

fn io_failed(e: &io::Error) -> MoveFailed {
    MoveFailed {
        error: if e.kind() == io::ErrorKind::NotFound {
            FileError::Gone
        } else {
            FileError::Io
        },
        denied: e.kind() == io::ErrorKind::PermissionDenied,
    }
}

fn plain_file(path: &Path) -> Result<fs::Metadata, MoveFailed> {
    let meta = fs::symlink_metadata(path).map_err(|e| io_failed(&e))?;
    if !meta.file_type().is_file() || links(&meta) > 1 {
        return Err(FileError::NotAFile.into());
    }
    if online_only(&meta) {
        return Err(FileError::OnlineOnly.into());
    }
    Ok(meta)
}

/// `name` with ` (n)` before its extension (`n` ≥ 2).
pub(crate) fn numbered(name: &str, n: u32) -> String {
    if n < 2 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem} ({n}).{ext}"),
        _ => format!("{name} ({n})"),
    }
}

/// Claims a new, empty file in `dir` named `name`, or `name (2)`, … — never
/// an existing one.
fn reserve(dir: &Path, name: &str) -> Result<PathBuf, MoveFailed> {
    for n in 1..10_000 {
        let path = dir.join(numbered(name, n));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io_failed(&e)),
        }
    }
    Err(FileError::Io.into())
}

/// Creates each of `parts` below `root` without following a link: one that
/// is a link (or not a folder) is refused. The root itself is the pinned,
/// confirmed folder and may be a link.
fn make_dirs(root: &Path, parts: &[String]) -> Result<PathBuf, MoveFailed> {
    let mut d = root.to_path_buf();
    for part in parts {
        d.push(part);
        match fs::symlink_metadata(&d) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(FileError::NotAFile.into()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&d) {
                Ok(()) => {}
                // Made by someone else just now: check it again.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let meta = fs::symlink_metadata(&d).map_err(|e| io_failed(&e))?;
                    if !meta.file_type().is_dir() {
                        return Err(FileError::NotAFile.into());
                    }
                }
                Err(e) => return Err(io_failed(&e)),
            },
            Err(e) => return Err(io_failed(&e)),
        }
    }
    Ok(d)
}

/// Copies `from` over the reserved `to` and checks the copy's SHA-256.
pub(crate) fn copy_verified(from: &Path, to: &Path, expected: &str) -> Result<(), MoveFailed> {
    let mut src = fs::File::open(from).map_err(|e| io_failed(&e))?;
    let mut dst = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(to)
        .map_err(|e| io_failed(&e))?;
    io::copy(&mut src, &mut dst).map_err(|e| io_failed(&e))?;
    dst.flush().map_err(|e| io_failed(&e))?;
    dst.sync_all().map_err(|e| io_failed(&e))?;
    drop(dst);
    if file_sha256(to)? == expected {
        Ok(())
    } else {
        Err(FileError::Changed.into())
    }
}

/// [`move_handled`] into the folders `parts` below `root`, telling a
/// permission refusal apart.
pub(crate) fn move_into(
    root: &Path,
    path: &Path,
    expected: &str,
    parts: &[String],
) -> Result<PathBuf, MoveFailed> {
    // Only a file directly in the folder is ever moved.
    if path.parent() != Some(root) {
        return Err(FileError::NotAFile.into());
    }
    plain_file(path)?;
    if file_sha256(path)? != expected {
        return Err(FileError::Changed.into());
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(FileError::NotAFile)?
        .to_string();
    let dir = make_dirs(root, parts)?;
    let target = reserve(&dir, &name)?;
    // The reserved name is ours: the rename replaces only our empty file.
    if fs::rename(path, &target).is_ok() {
        return Ok(target);
    }
    match copy_verified(path, &target, expected) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(path) {
                // Two copies would be brought in again: keep the original.
                let _ = fs::remove_file(&target);
                return Err(io_failed(&e));
            }
            Ok(target)
        }
        Err(e) => {
            let _ = fs::remove_file(&target);
            Err(e)
        }
    }
}

/// Moves the handled original `path` (directly in the intake folder
/// `root`) to `to`, if it is still the file copied in (SHA-256
/// `sha256`). Returns where it is now. Never overwrites, never deletes an
/// original without a verified copy.
///
/// # Errors
/// `Gone`, `NotAFile` (a link, hard-linked, not in the folder, or a
/// `Processed`/`Discarded` that is a link), `OnlineOnly`, `Changed`
/// (different bytes now; nothing moved), `Io` (nothing lost).
pub fn move_handled(
    root: &Path,
    path: &Path,
    sha256: &str,
    to: HandledDir,
) -> Result<PathBuf, FileError> {
    move_into(root, path, sha256, &to.parts()).map_err(|f| f.error)
}

#[cfg(test)]
mod tests {
    use super::super::sha256_hex;
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%handled test\n";

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).expect("write");
        p
    }

    const NOV: HandledDir = HandledDir::Processed {
        year: 2026,
        month: 11,
    };

    #[test]
    fn processed_by_month_and_never_overwritten() {
        let dir = tempfile::tempdir().expect("tmp");
        let month = NOV.dir_in(dir.path());
        assert!(month.ends_with("Processed/2026-11"));
        fs::create_dir_all(&month).expect("dir");
        write(&month, "r.pdf", b"%PDF-older");
        write(&month, "r (2).pdf", b"%PDF-older 2");
        let p = write(dir.path(), "r.pdf", PDF);
        let to = move_handled(dir.path(), &p, &sha256_hex(PDF), NOV).expect("moved");
        assert_eq!(to, month.join("r (3).pdf"));
        assert!(!p.exists(), "the original moved");
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert_eq!(
            fs::read(month.join("r.pdf")).expect("read"),
            b"%PDF-older",
            "never overwritten"
        );
        let q = write(dir.path(), "noext", PDF);
        write(&month, "noext", b"x");
        assert_eq!(
            move_handled(dir.path(), &q, &sha256_hex(PDF), NOV).expect("moved"),
            month.join("noext (2)")
        );
    }

    #[test]
    fn discarded_is_flat() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "dup.pdf", PDF);
        let to =
            move_handled(dir.path(), &p, &sha256_hex(PDF), HandledDir::Discarded).expect("moved");
        assert_eq!(to, dir.path().join(DISCARDED_DIR).join("dup.pdf"));
        let p = write(dir.path(), "dup.pdf", PDF);
        let to =
            move_handled(dir.path(), &p, &sha256_hex(PDF), HandledDir::Discarded).expect("moved");
        assert_eq!(to, dir.path().join(DISCARDED_DIR).join("dup (2).pdf"));
    }

    #[test]
    fn a_changed_or_foreign_file_is_not_moved() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        assert_eq!(
            move_handled(dir.path(), &p, &sha256_hex(b"other bytes"), NOV),
            Err(FileError::Changed)
        );
        assert!(p.exists(), "left where it is");
        assert!(!NOV.dir_in(dir.path()).exists());
        assert_eq!(
            move_handled(dir.path(), &dir.path().join("gone.pdf"), "x", NOV),
            Err(FileError::Gone)
        );
        // Not directly in the folder.
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).expect("dir");
        let deeper = write(&sub, "d.pdf", PDF);
        assert_eq!(
            move_handled(dir.path(), &deeper, &sha256_hex(PDF), NOV),
            Err(FileError::NotAFile)
        );
        assert!(deeper.exists());
        assert_eq!(
            move_handled(dir.path(), &sub, &sha256_hex(PDF), NOV),
            Err(FileError::NotAFile)
        );
    }

    /// A `Processed` (or `Discarded`) that is a link is never followed:
    /// the file stays instead of landing wherever the link points.
    #[test]
    fn a_linked_or_odd_processed_folder_is_refused() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        #[cfg(unix)]
        let linked =
            std::os::unix::fs::symlink(outside.path(), dir.path().join(PROCESSED_DIR)).is_ok();
        // Needs developer mode or admin rights on Windows.
        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_dir(outside.path(), dir.path().join(PROCESSED_DIR))
                .is_ok();
        if linked {
            assert_eq!(
                move_handled(dir.path(), &p, &sha256_hex(PDF), NOV),
                Err(FileError::NotAFile)
            );
            assert!(p.exists(), "left where it is");
            assert_eq!(
                fs::read_dir(outside.path()).expect("dir").count(),
                0,
                "nothing was made through the link"
            );
            fs::remove_dir(dir.path().join(PROCESSED_DIR))
                .or_else(|_| fs::remove_file(dir.path().join(PROCESSED_DIR)))
                .expect("unlink");
        }
        write(dir.path(), PROCESSED_DIR, b"not a folder");
        assert_eq!(
            move_handled(dir.path(), &p, &sha256_hex(PDF), NOV),
            Err(FileError::NotAFile)
        );
        write(dir.path(), DISCARDED_DIR, b"not a folder");
        assert_eq!(
            move_handled(dir.path(), &p, &sha256_hex(PDF), HandledDir::Discarded),
            Err(FileError::NotAFile)
        );
        assert!(p.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_hard_linked_file_is_not_moved() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let target = write(outside.path(), "secret.pdf", PDF);
        let p = dir.path().join("hard.pdf");
        fs::hard_link(&target, &p).expect("link");
        assert_eq!(
            move_handled(dir.path(), &p, &sha256_hex(PDF), NOV),
            Err(FileError::NotAFile)
        );
    }

    #[test]
    fn a_copy_is_checked_before_the_original_goes() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        let to = dir.path().join("copy.pdf");
        fs::write(&to, b"").expect("reserve");
        assert_eq!(copy_verified(&p, &to, &sha256_hex(PDF)), Ok(()));
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert_eq!(
            copy_verified(&p, &to, &sha256_hex(b"x")).map_err(|f| f.error),
            Err(FileError::Changed)
        );
        assert_eq!(numbered("a.b.pdf", 2), "a.b (2).pdf");
        assert_eq!(numbered(".pdf", 2), ".pdf (2)");
        assert_eq!(numbered("a.pdf", 1), "a.pdf");
    }

    #[test]
    fn permission_refusals_are_told_apart() {
        let denied = io_failed(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!((denied.error, denied.denied), (FileError::Io, true));
        let gone = io_failed(&io::Error::from(io::ErrorKind::NotFound));
        assert_eq!((gone.error, gone.denied), (FileError::Gone, false));
    }
}
