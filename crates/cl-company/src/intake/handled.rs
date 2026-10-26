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
//! - Every step is safe against the folder changing under it (security
//!   review M1; the folder may be shared with other people and programs):
//!   1. The original is first **renamed** to a reserved hidden name in the
//!      same folder (`.clientledger-moving-…`). From then on nothing acts on
//!      its old name, so a new file a scanner saves there meanwhile is
//!      never touched. What was renamed is opened without following links
//!      and checked again (a plain file with the expected SHA-256); a link
//!      or another file swapped in is put back and refused.
//!   2. It is placed with a **hard link** to a name that doesn't exist yet
//!      (a hard link never replaces anything; a taken name becomes ` (2)`,
//!      …), then the reserved name is removed.
//!   3. Where hard links don't work (another volume, FAT, some shares) the
//!      bytes are copied into a file made with `create_new` and written
//!      **through that handle** (never reopened by name), its SHA-256 read
//!      back through the same handle, and only then is the reserved name
//!      removed.
//!   4. After placing, `Processed/…` is checked again; if a link was
//!      swapped in meanwhile the placed file is taken back.
//!
//!   If anything fails, the original goes back to its name (or ` (2)`, …
//!   when a new file took it): there is never a second copy to bring in
//!   again, and the original is never deleted without a verified copy.
//! - A name that can't exist (over 255 characters once numbered) and a
//!   `Processed`/`Discarded` that isn't a plain folder are reported as
//!   [`MoveFailed::denied`] (the folder shows "can't move") instead of
//!   being retried quietly.

use std::fmt;
use std::fs;
use std::io::{self, Seek as _, Write as _};
use std::path::{Path, PathBuf};

use super::fs::{handle_sha256, open_plain, unique_suffix};
use super::{FileError, test_point};

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

/// Why a move failed: the error, and whether it can't work as things
/// stand (the folder's health becomes `CantMove`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MoveFailed {
    /// What went wrong.
    pub error: FileError,
    /// The system said "access denied", or the move can't work until the
    /// owner changes something (`Processed` isn't a plain folder, the name
    /// would be too long).
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

/// The move can't work until the owner changes something: `what`, shown
/// as "can't move".
fn cant_move(what: FileError) -> MoveFailed {
    MoveFailed {
        error: what,
        denied: true,
    }
}

/// The longest file name most systems take (Windows counts UTF-16 units,
/// the others bytes).
const MAX_NAME: usize = 255;

/// True when `name` is short enough to be a file name.
pub(crate) fn name_fits(name: &str) -> bool {
    if cfg!(windows) {
        name.encode_utf16().count() <= MAX_NAME
    } else {
        name.len() <= MAX_NAME
    }
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

/// The most ` (n)` tried before giving up.
const MAX_NUMBERED: u32 = 10_000;

/// The reserved hidden name an original holds while it moves (a dot-file:
/// never listed, never copied in).
const MOVING_PREFIX: &str = ".clientledger-moving-";

/// Creates each of `parts` below `root` without following a link: one that
/// is a link, a junction or not a folder is refused (can't move). The root
/// itself is the pinned, confirmed folder and may be a link.
fn make_dirs(root: &Path, parts: &[String]) -> Result<PathBuf, MoveFailed> {
    let mut d = root.to_path_buf();
    for part in parts {
        d.push(part);
        match fs::symlink_metadata(&d) {
            Ok(meta) if meta.file_type().is_dir() => {}
            Ok(_) => return Err(cant_move(FileError::NotAFile)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match fs::create_dir(&d) {
                Ok(()) => {}
                // Made by someone else just now: check it again.
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let meta = fs::symlink_metadata(&d).map_err(|e| io_failed(&e))?;
                    if !meta.file_type().is_dir() {
                        return Err(cant_move(FileError::NotAFile));
                    }
                }
                Err(e) => return Err(io_failed(&e)),
            },
            Err(e) => return Err(io_failed(&e)),
        }
    }
    Ok(d)
}

/// True while every one of `parts` below `root` is still a plain folder.
fn dirs_still_plain(root: &Path, parts: &[String]) -> bool {
    let mut d = root.to_path_buf();
    parts.iter().all(|part| {
        d.push(part);
        fs::symlink_metadata(&d).is_ok_and(|m| m.file_type().is_dir())
    })
}

/// Renames the original `path` to a reserved hidden name in `root` (step
/// 1). The name is claimed with `create_new` first, so the rename replaces
/// only that empty file of ours.
fn take_original(root: &Path, path: &Path) -> Result<PathBuf, MoveFailed> {
    for _ in 0..3 {
        let temp = root.join(format!("{MOVING_PREFIX}{}", unique_suffix()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
        {
            Ok(f) => drop(f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_failed(&e)),
        }
        return match fs::rename(path, &temp) {
            Ok(()) => Ok(temp),
            Err(e) => {
                let _ = fs::remove_file(&temp);
                Err(io_failed(&e))
            }
        };
    }
    Err(FileError::Io.into())
}

/// Opens what was renamed to `temp` without following a link and checks it
/// is the plain file copied in. Returns the handle, rewound.
fn check_taken(temp: &Path, expected: &str) -> Result<fs::File, MoveFailed> {
    let (mut file, _) = open_plain(temp, false)?;
    if handle_sha256(&file)? != expected {
        return Err(FileError::Changed.into());
    }
    file.rewind().map_err(|e| io_failed(&e))?;
    Ok(file)
}

/// True when making `at` failed because something already has the name.
fn taken(e: &io::Error, at: &Path) -> bool {
    e.kind() == io::ErrorKind::AlreadyExists || fs::symlink_metadata(at).is_ok()
}

/// A hard link `to` → `from` (never replaces an existing `to`).
fn link_new(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(test)]
    if super::race::hard_links_off() {
        return Err(io::Error::from(io::ErrorKind::Unsupported));
    }
    fs::hard_link(from, to)
}

/// Places the checked file `temp` (open as `src`) in `dir` as `name`, or
/// `name (2)`, …, never replacing anything (steps 2 and 3). Returns where.
fn place(
    temp: &Path,
    src: &mut fs::File,
    dir: &Path,
    name: &str,
    expected: &str,
) -> Result<PathBuf, MoveFailed> {
    let candidates = (1..MAX_NUMBERED).map(|n| numbered(name, n));
    for candidate in candidates.clone() {
        if !name_fits(&candidate) {
            return Err(cant_move(FileError::Io));
        }
        let target = dir.join(&candidate);
        match link_new(temp, &target) {
            Ok(()) => return Ok(target),
            Err(e) if taken(&e, &target) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(io_failed(&e)),
            // No hard links here (another volume, FAT, a share): copy.
            Err(_) => break,
        }
    }
    for candidate in candidates {
        if !name_fits(&candidate) {
            return Err(cant_move(FileError::Io));
        }
        let target = dir.join(&candidate);
        let mut o = fs::OpenOptions::new();
        o.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt as _;
            /// `FILE_SHARE_READ`: nobody renames, removes or writes it while
            /// it is being written.
            const FILE_SHARE_READ: u32 = 0x1;
            o.share_mode(FILE_SHARE_READ);
        }
        let mut dst = match o.open(&target) {
            Ok(f) => f,
            // Taken (Windows says "access denied" for a folder or junction).
            Err(e) if taken(&e, &target) => continue,
            Err(e) => return Err(io_failed(&e)),
        };
        test_point("copying");
        let copied = copy_through(src, &mut dst, expected).and_then(|()| {
            if still_ours(&target, &dst) {
                Ok(())
            } else {
                Err(FileError::Changed.into())
            }
        });
        drop(dst);
        return match copied {
            Ok(()) => Ok(target),
            Err(e) => {
                // Ours, made just now by `create_new` (or whatever was
                // swapped in for it, which is only a name in the folder).
                let _ = fs::remove_file(&target);
                Err(e)
            }
        };
    }
    Err(FileError::Io.into())
}

/// Copies `src` (from its start) into `dst` through the handle `dst` was
/// made with, and checks the SHA-256 read back through that same handle.
fn copy_through(src: &mut fs::File, dst: &mut fs::File, expected: &str) -> Result<(), MoveFailed> {
    src.rewind().map_err(|e| io_failed(&e))?;
    io::copy(src, dst).map_err(|e| io_failed(&e))?;
    dst.flush().map_err(|e| io_failed(&e))?;
    dst.sync_all().map_err(|e| io_failed(&e))?;
    dst.rewind().map_err(|e| io_failed(&e))?;
    if handle_sha256(dst)? == expected {
        Ok(())
    } else {
        Err(FileError::Changed.into())
    }
}

/// True when `target` still names the file open as `dst` (a plain file;
/// on Unix the same device and inode). On Windows `dst` is open without
/// delete sharing, so its name can't have changed.
fn still_ours(target: &Path, dst: &fs::File) -> bool {
    let (Ok(at), Ok(held)) = (fs::symlink_metadata(target), dst.metadata()) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if (at.dev(), at.ino()) != (held.dev(), held.ino()) {
            return false;
        }
    }
    at.file_type().is_file() && at.len() == held.len()
}

/// Puts what is at the reserved `temp` back in `root` as `name` (or
/// `name (2)`, … when a new file took the name meanwhile), never replacing
/// anything.
fn put_back(root: &Path, temp: &Path, name: &str) {
    for n in 1..100 {
        let candidate = numbered(name, n);
        if !name_fits(&candidate) {
            break;
        }
        let back = root.join(candidate);
        match link_new(temp, &back) {
            Ok(()) => {
                let _ = fs::remove_file(temp);
                return;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            // No hard links here: a rename to a name that is free.
            Err(_) => match fs::symlink_metadata(&back) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let _ = fs::rename(temp, &back);
                    return;
                }
                _ => {}
            },
        }
    }
    tracing::warn!("an intake file could not be put back; it keeps a hidden name in its folder");
}

/// [`move_handled`] into the folders `parts` below `root`, telling a
/// refusal that needs the owner apart.
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
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(FileError::NotAFile)?
        .to_string();
    // Nothing is made for a file whose bytes changed (checked again once
    // it is taken).
    drop(check_taken(path, expected)?);
    let dir = make_dirs(root, parts)?;
    test_point("taking");
    let temp = take_original(root, path)?;
    test_point("taken");
    let placed = check_taken(&temp, expected).and_then(|mut src| {
        let placed = place(&temp, &mut src, &dir, &name, expected);
        drop(src);
        placed
    });
    let target = match placed {
        Ok(t) => t,
        Err(e) => {
            put_back(root, &temp, &name);
            return Err(e);
        }
    };
    test_point("placed");
    // A link swapped in for `Processed/…` meanwhile: take the file back.
    if !dirs_still_plain(root, parts) {
        let _ = fs::remove_file(&target);
        put_back(root, &temp, &name);
        return Err(cant_move(FileError::NotAFile));
    }
    if let Err(e) = fs::remove_file(&temp) {
        // Two copies would be brought in again: keep the original.
        let _ = fs::remove_file(&target);
        put_back(root, &temp, &name);
        return Err(io_failed(&e));
    }
    Ok(target)
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
    use super::super::{race, sha256_hex};
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
    fn names_are_numbered() {
        assert_eq!(numbered("a.b.pdf", 2), "a.b (2).pdf");
        assert_eq!(numbered(".pdf", 2), ".pdf (2)");
        assert_eq!(numbered("a.pdf", 1), "a.pdf");
        assert!(name_fits(&"a".repeat(255)));
        assert!(!name_fits(&"a".repeat(256)));
    }

    /// No reserved `.clientledger-moving-…` name is left behind.
    fn no_moving_left(dir: &Path) {
        let left: Vec<_> = fs::read_dir(dir)
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with(MOVING_PREFIX))
            .collect();
        assert!(left.is_empty(), "{} reserved names left", left.len());
    }

    /// Makes a link `link` to the folder `target` (a junction on Windows,
    /// which needs no special rights). False when this system can't.
    fn dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|o| o.status.success())
        }
    }

    /// Makes a file link `link` → `target`. False when this system can't
    /// (Windows without developer mode or admin rights).
    fn file_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
    }

    /// A scanner saves a new file under the same name while the original
    /// moves: the new file is never touched (the old code removed whatever
    /// was at the name after copying).
    #[test]
    fn a_new_file_saved_under_the_name_meanwhile_is_kept() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "scan.pdf", PDF);
        let newer = p.clone();
        race::set(move |stage| {
            if stage == "taken" {
                fs::write(&newer, b"%PDF-the next scan").expect("write");
            }
        });
        let r = move_handled(dir.path(), &p, &sha256_hex(PDF), NOV);
        race::clear();
        let to = r.expect("moved");
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert_eq!(fs::read(&p).expect("read"), b"%PDF-the next scan");
        no_moving_left(dir.path());
    }

    /// Different bytes swapped in just before the move: put back, refused.
    #[test]
    fn a_file_swapped_in_before_the_move_is_put_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        let swap = p.clone();
        race::set(move |stage| {
            if stage == "taking" {
                fs::write(&swap, b"%PDF-someone else's").expect("write");
            }
        });
        let r = move_handled(dir.path(), &p, &sha256_hex(PDF), NOV);
        race::clear();
        assert_eq!(r, Err(FileError::Changed));
        assert_eq!(fs::read(&p).expect("read"), b"%PDF-someone else's");
        assert_eq!(
            fs::read_dir(NOV.dir_in(dir.path())).expect("dir").count(),
            0
        );
        no_moving_left(dir.path());
    }

    /// A link to a file outside swapped in for the original: never moved,
    /// put back as it was, the outside file untouched.
    #[test]
    fn a_link_swapped_in_for_the_original_is_refused() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let secret = write(outside.path(), "secret.pdf", PDF);
        if !file_link(&secret, &dir.path().join("probe")) {
            return;
        }
        fs::remove_file(dir.path().join("probe")).expect("rm");
        let p = write(dir.path(), "r.pdf", PDF);
        let (swap, to) = (p.clone(), secret.clone());
        race::set(move |stage| {
            if stage == "taking" {
                fs::remove_file(&swap).expect("rm");
                assert!(file_link(&to, &swap));
            }
        });
        let r = move_into(dir.path(), &p, &sha256_hex(PDF), &NOV.parts());
        race::clear();
        assert_eq!(r.map_err(|f| f.error), Err(FileError::NotAFile));
        assert!(fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()));
        assert_eq!(fs::read(&secret).expect("read"), PDF);
        assert_eq!(
            fs::read_dir(NOV.dir_in(dir.path())).expect("dir").count(),
            0
        );
        no_moving_left(dir.path());
    }

    /// `Processed/2026-11` swapped for a link (a junction on Windows) to a
    /// folder outside after it was checked: the placed file is taken back,
    /// the folder shows "can't move", nothing stays outside.
    #[test]
    fn a_link_swapped_in_for_processed_takes_the_file_back() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        let month = NOV.dir_in(dir.path());
        let target = outside.path().to_path_buf();
        let swapped = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = std::rc::Rc::clone(&swapped);
        race::set(move |stage| {
            if stage == "taking" {
                fs::remove_dir(&month).expect("rm");
                seen.set(dir_link(&target, &month));
            }
        });
        let r = move_into(dir.path(), &p, &sha256_hex(PDF), &NOV.parts());
        race::clear();
        assert!(
            swapped.get() || !cfg!(windows),
            "junctions need no rights on Windows"
        );
        if !swapped.get() {
            return;
        }
        let f = r.expect_err("refused");
        assert!(f.denied, "shown as can't move");
        assert_eq!(fs::read(&p).expect("read"), PDF, "back where it was");
        assert_eq!(
            fs::read_dir(outside.path()).expect("dir").count(),
            0,
            "nothing left outside the folder"
        );
        no_moving_left(dir.path());
    }

    /// Without hard links the bytes are copied into a new file through its
    /// own handle: a name taken by a link is skipped, and a link swapped in
    /// for the new file while it is written never sends the bytes (or a
    /// truncation) anywhere else.
    #[test]
    fn the_copy_path_writes_only_through_its_own_handle() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let victim = write(outside.path(), "victim.pdf", b"%PDF-keep me");
        let month = NOV.dir_in(dir.path());
        fs::create_dir_all(&month).expect("dir");
        // The plain name is taken by a link to a folder outside.
        let taken = dir_link(outside.path(), &month.join("r.pdf"));
        assert!(
            taken || !cfg!(windows),
            "junctions need no rights on Windows"
        );
        let p = write(dir.path(), "r.pdf", PDF);
        race::no_hard_links();
        let to = move_handled(dir.path(), &p, &sha256_hex(PDF), NOV).expect("copied");
        assert_eq!(
            to,
            month.join(if taken { "r (2).pdf" } else { "r.pdf" }),
            "never through the link"
        );
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert!(!p.exists());
        assert_eq!(fs::read(&victim).expect("read"), b"%PDF-keep me");
        no_moving_left(dir.path());
        // A link swapped in for the new file while it is being written.
        let q = write(dir.path(), "s.pdf", PDF);
        let can_link = file_link(&victim, &dir.path().join("probe"));
        if can_link {
            fs::remove_file(dir.path().join("probe")).expect("rm");
            let (at, to) = (month.join("s.pdf"), victim.clone());
            race::set(move |stage| {
                if stage == "copying" {
                    // Windows: our handle refuses deletion meanwhile.
                    if fs::remove_file(&at).is_ok() {
                        assert!(file_link(&to, &at));
                    }
                }
            });
            let r = move_handled(dir.path(), &q, &sha256_hex(PDF), NOV);
            assert_eq!(fs::read(&victim).expect("read"), b"%PDF-keep me");
            if r.is_err() {
                assert_eq!(fs::read(&q).expect("read"), PDF, "the original stays");
            }
        }
        race::clear();
        no_moving_left(dir.path());
    }

    /// A numbered name that would be too long, and a `Processed` that is a
    /// file, show "can't move" instead of failing quietly for ever.
    #[test]
    fn moves_that_cant_work_are_cant_move() {
        let dir = tempfile::tempdir().expect("tmp");
        let month = NOV.dir_in(dir.path());
        fs::create_dir_all(&month).expect("dir");
        let long = format!("{}.pdf", "a".repeat(250));
        write(&month, &long, b"%PDF-older");
        let p = write(dir.path(), &long, PDF);
        let f = move_into(dir.path(), &p, &sha256_hex(PDF), &NOV.parts()).expect_err("too long");
        assert!(f.denied);
        assert_eq!(fs::read(&p).expect("read"), PDF);
        no_moving_left(dir.path());
        let other = tempfile::tempdir().expect("tmp");
        write(other.path(), PROCESSED_DIR, b"not a folder");
        let q = write(other.path(), "q.pdf", PDF);
        let f = move_into(other.path(), &q, &sha256_hex(PDF), &NOV.parts()).expect_err("file");
        assert_eq!((f.error, f.denied), (FileError::NotAFile, true));
    }

    #[test]
    fn permission_refusals_are_told_apart() {
        let denied = io_failed(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!((denied.error, denied.denied), (FileError::Io, true));
        let gone = io_failed(&io::Error::from(io::ErrorKind::NotFound));
        assert_eq!((gone.error, gone.denied), (FileError::Gone, false));
    }
}
