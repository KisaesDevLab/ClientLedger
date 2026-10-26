//! Intake folders (A01, ADD-01): the folders a company's bills and receipts
//! are saved into. Each confirmed folder is watched on its own thread; a
//! file that stopped changing is read, copied into the company as a draft
//! attachment, and once handled its original moves to `Processed/YYYY-MM/`
//! or `Discarded/` inside that folder.
//!
//! The folder may be local, a mapped drive, a network share or a
//! cloud-sync folder, so this module treats it as untrusted and slow:
//! - **No I/O on the caller's thread.** [`check_path_syntax`] is pure;
//!   everything else runs on a folder thread ([`WatchHandle`]) or a probe
//!   thread ([`probe_async`]) with a deadline, so a dead share is a
//!   [`Health`], never a hang. Threads are never joined by the caller.
//! - **The root is pinned, entries are never followed.** The root may be a
//!   link (macOS cloud folders are), so it is resolved once and pinned by
//!   [`canonical_key`]; a changed key pauses the folder ([`Health::Moved`]).
//!   Entries are looked at without following links; only plain files count.
//! - **Cloud placeholders are never read** (Windows offline/recall
//!   attributes, macOS dataless files).
//! - **Entries are opened without following links** (Unix `O_NOFOLLOW`
//!   plus a device/inode check, Windows `FILE_FLAG_OPEN_REPARSE_POINT`), so
//!   a link swapped in after a look is never read or moved.
//! - **Originals are never overwritten or deleted.** A handled file moves
//!   (taken under a reserved name, then a hard link that never replaces,
//!   or a copy written through its own handle and verified by SHA-256),
//!   and a file whose bytes changed is refused ([`move_handled`]).
//! - **Owner data stays out.** Paths and file names are never logged and
//!   never in `Debug`.
//!
//! The P17 single-folder receipt inbox keeps working through [`legacy`]
//! (re-exported as `cl_company::inbox`) until its screen moves to this
//! module.

use std::fmt;

use sha2::{Digest, Sha256};

pub mod fs;
pub mod handled;
pub mod legacy;
pub mod migrate;
pub mod path;
pub mod probe;
pub mod scan;
pub mod watch;

pub use fs::{EntryInfo, FakeFs, FolderFs, RawListing, RealFs};
pub use handled::{DISCARDED_DIR, HandledDir, MoveFailed, PROCESSED_DIR, move_handled};
pub use migrate::{
    IntakeEngine, LocalIntake, MigrateError, MigrationOutcome, PlannedMigration, run_migration,
};
pub use path::{
    MAX_PATH_CHARS, PathProblem, canonical_key, canonical_key_with, check_path_syntax, classify,
    engine_accepts_path,
};
pub use probe::{PROBE_DEADLINE, ProbeCounts, ProbeReport, probe, probe_async, probe_async_with};
pub use scan::{
    DocKind, IntakeFile, ItemState, Listing, MAX_FILE_BYTES, MAX_LISTED, MAX_SCANNED, STABLE_FOR,
    Stability, doc_kind, read_for_upload, scan, skipped_name,
};
pub use watch::{
    MAX_INTERVAL, MoveDone, MoveJob, POLL, REPIN_EVERY, SCAN_DEADLINE, SETTLE_RECHECK, SLOW_SCAN,
    Snapshot, WatchConfig, WatchHandle,
};

/// How a folder is doing (D6). Only the first eight need the owner; `Slow`
/// and `TooMany` are informational.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Health {
    /// Listed fine.
    Ok,
    /// This computer has not confirmed the folder (a restore elsewhere, or
    /// the path was edited on another computer).
    NotConfirmed,
    /// The folder is not there (drive unplugged, renamed, sync paused).
    Missing,
    /// The network folder can't be reached.
    Unreachable,
    /// A look at the folder has not come back within the deadline.
    NotResponding,
    /// The app isn't allowed to read the folder.
    PermissionDenied,
    /// The path is a file, not a folder.
    NotAFolder,
    /// Handled files can't be moved to `Processed/` (files stay).
    CantMove,
    /// The folder now points somewhere else (its pinned root changed).
    Moved,
    /// Another company on this computer already uses this folder.
    MappedByAnother,
    /// Looking at the folder takes long; it is looked at less often.
    Slow,
    /// More than [`MAX_SCANNED`] entries; only the first are looked at.
    TooMany,
}

impl Health {
    /// A short name for the control channel and tests.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::NotConfirmed => "not_confirmed",
            Health::Missing => "missing",
            Health::Unreachable => "unreachable",
            Health::NotResponding => "not_responding",
            Health::PermissionDenied => "permission_denied",
            Health::NotAFolder => "not_a_folder",
            Health::CantMove => "cant_move",
            Health::Moved => "moved",
            Health::MappedByAnother => "mapped_by_another",
            Health::Slow => "slow",
            Health::TooMany => "too_many",
        }
    }

    /// True when the owner should be told (Home's Needs attention line):
    /// everything but `Ok`, `Slow` and `TooMany`.
    #[must_use]
    pub fn needs_attention(self) -> bool {
        !matches!(self, Health::Ok | Health::Slow | Health::TooMany)
    }
}

/// A file read for upload.
#[derive(Clone, PartialEq, Eq)]
pub struct ReadFile {
    /// Its bytes (a PDF, PNG, JPEG or TIFF by its first bytes, at most
    /// 10 MB).
    pub bytes: Vec<u8>,
    /// Size in bytes.
    pub size: u64,
    /// Modified time when read, milliseconds since 1970.
    pub modified_ms: i64,
    /// SHA-256 of the bytes, lower-case hex.
    pub sha256: String,
}

impl fmt::Debug for ReadFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadFile")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// Why a file was not read or moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    /// It is gone (moved, deleted).
    #[error("the file is gone")]
    Gone,
    /// A link, a hard-linked file, or not a plain file.
    #[error("not a plain file")]
    NotAFile,
    /// Online only (a cloud placeholder).
    #[error("the file is online only")]
    OnlineOnly,
    /// Over 10 MB.
    #[error("the file is too large")]
    TooLarge,
    /// Empty.
    #[error("the file is empty")]
    Empty,
    /// Not a PDF, PNG, JPEG or TIFF by its bytes.
    #[error("not a PDF or picture")]
    WrongType,
    /// It changed since it was read (different bytes).
    #[error("the file changed")]
    Changed,
    /// It changed (or was still open for writing) while being read.
    #[error("the file changed while it was read")]
    Unstable,
    /// The file system refused.
    #[error("the file could not be read or moved")]
    Io,
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// SHA-256 of `bytes`, lower-case hex.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// A named point inside a file operation where a test may change the
/// folder (swap a file for a link, write a new file) to check a race
/// deterministically. Does nothing outside this crate's unit tests.
#[cfg(not(test))]
#[inline]
pub(crate) fn test_point(_stage: &'static str) {}

#[cfg(test)]
pub(crate) use race::test_point;

/// The test side of [`test_point`]: one hook per thread.
#[cfg(test)]
pub(crate) mod race {
    use std::cell::RefCell;

    type Hook = Box<dyn FnMut(&'static str)>;

    thread_local! {
        static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) };
        static NO_HARD_LINKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Runs the hook (once taken out, so it can't re-enter).
    pub(crate) fn test_point(stage: &'static str) {
        let hook = HOOK.with(|h| h.borrow_mut().take());
        if let Some(mut f) = hook {
            f(stage);
            HOOK.with(|h| *h.borrow_mut() = Some(f));
        }
    }

    /// Calls `f` at every [`test_point`] on this thread until [`clear`].
    pub(crate) fn set(f: impl FnMut(&'static str) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    /// No hook, hard links work again.
    pub(crate) fn clear() {
        HOOK.with(|h| *h.borrow_mut() = None);
        NO_HARD_LINKS.with(|c| c.set(false));
    }

    /// Hard links fail as on a file system without them (FAT, some
    /// shares), so the copy path runs.
    pub(crate) fn no_hard_links() {
        NO_HARD_LINKS.with(|c| c.set(true));
    }

    pub(crate) fn hard_links_off() -> bool {
        NO_HARD_LINKS.with(std::cell::Cell::get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_names_and_attention() {
        let all = [
            Health::Ok,
            Health::NotConfirmed,
            Health::Missing,
            Health::Unreachable,
            Health::NotResponding,
            Health::PermissionDenied,
            Health::NotAFolder,
            Health::CantMove,
            Health::Moved,
            Health::MappedByAnother,
            Health::Slow,
            Health::TooMany,
        ];
        let names: std::collections::HashSet<_> = all.iter().map(|h| h.as_str()).collect();
        assert_eq!(names.len(), all.len(), "distinct names");
        let quiet: Vec<_> = all.iter().filter(|h| !h.needs_attention()).collect();
        assert_eq!(quiet, [&Health::Ok, &Health::Slow, &Health::TooMany]);
    }

    #[test]
    fn read_file_debug_shows_no_bytes() {
        let r = ReadFile {
            bytes: b"%PDF-secret vendor".to_vec(),
            size: 18,
            modified_ms: 1,
            sha256: sha256_hex(b"x"),
        };
        let s = format!("{r:?}");
        assert!(!s.contains("secret") && !s.contains(&r.sha256), "{s}");
    }
}
