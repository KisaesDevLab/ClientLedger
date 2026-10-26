//! The P17 receipt inbox folder (plan §3 WP5, Q-54, Q-P17p), kept with its
//! exact types for the receipt inbox screen until it moves to the A01
//! intake folders (WP-C); `cl_company::inbox` re-exports it. Reading for
//! upload is the intake one ([`super::read_for_upload`]).
//!
//! What is in the owner's inbox folder, whether each file is ready to
//! attach, reading one for upload, and filing it away once a record holds
//! it.
//!
//! Rules this module keeps:
//! - Plain `std::fs` polling; no watcher crate (`notify` fails the direct
//!   dependency licence rule).
//! - Only the folder itself is listed (never sub-folders, so `Filed/` is
//!   never offered again); links are never followed (`symlink_metadata`).
//! - `.pdf`, `.png`, `.jpg`, `.jpeg` by name (any case); the bytes are
//!   checked again when a file is read for upload. Hidden files, Office
//!   lock files (`~$…`) and partial downloads (`*.part`, `*.crdownload`,
//!   `*.tmp`) are skipped.
//! - A file is **ready** only when its size and modified time stayed the
//!   same across two polls at least [`STABLE_FOR`] apart ([`Stability`]),
//!   so a scanner or a sync client still writing it is left alone.
//! - Over 10 MB (the engine's limit) is listed as too large and never read.
//! - On Windows, files that are online only (`OneDrive` and similar:
//!   `OFFLINE`, `RECALL_ON_OPEN`, `RECALL_ON_DATA_ACCESS`) are listed and
//!   never read: reading would download them.
//! - At most [`MAX_LISTED`] files, oldest first, with a count of the rest.
//! - Filing away moves the original to `<inbox>/Filed/<YYYY-MM>/<name>`. An
//!   existing file is never overwritten (` (2)`, ` (3)`, … instead) and the
//!   original is never deleted unless an identical copy is in `Filed/`.
//! - File names and paths are owner data: never logged, never in `Debug`.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::fs::{hidden, millis, online_only};
pub use super::scan::{MAX_FILE_BYTES, MAX_LISTED, read_for_upload};
pub use super::{FileError, ReadFile, sha256_hex};

/// How long a file's size and time must stay the same before it is ready
/// (P17; intake folders use [`super::STABLE_FOR`]).
pub const STABLE_FOR: Duration = Duration::from_secs(2);

/// The sub-folder filed originals go to.
pub const FILED_DIR: &str = "Filed";

/// What kind of file the name says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NamedKind {
    /// `.pdf`.
    Pdf,
    /// `.png`.
    Png,
    /// `.jpg` or `.jpeg`.
    Jpeg,
}

/// Where a listed file stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileState {
    /// Seen, but not yet unchanged for two polls (still being written).
    Settling,
    /// Unchanged across two polls: it can be attached.
    Ready,
    /// Over 10 MB: never read.
    TooLarge,
    /// Empty (unchanged at zero bytes): nothing to attach.
    Empty,
    /// Its content is online only (Windows cloud placeholder): never read.
    OnlineOnly,
}

impl FileState {
    /// True when the file may be offered for attaching.
    #[must_use]
    pub fn offerable(self) -> bool {
        self == FileState::Ready
    }

    /// A short name for the control channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FileState::Settling => "settling",
            FileState::Ready => "ready",
            FileState::TooLarge => "too_large",
            FileState::Empty => "empty",
            FileState::OnlineOnly => "online_only",
        }
    }
}

/// One file in the inbox folder.
#[derive(Clone, PartialEq, Eq)]
pub struct InboxFile {
    /// Its full path (owner data).
    pub path: PathBuf,
    /// Its file name (owner data).
    pub name: String,
    /// The kind its name says.
    pub kind: NamedKind,
    /// Size in bytes.
    pub size: u64,
    /// Modified time, milliseconds since 1970 (0 when unknown).
    pub modified_ms: i64,
    /// Where it stands.
    pub state: FileState,
}

/// Size, kind and state only: names and paths are owner data.
impl fmt::Debug for InboxFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InboxFile")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The files, oldest first, at most [`MAX_LISTED`].
    pub files: Vec<InboxFile>,
    /// Matching files left out over the cap.
    pub more: usize,
}

impl Listing {
    /// Files that can be attached now.
    #[must_use]
    pub fn ready(&self) -> usize {
        self.files.iter().filter(|f| f.state.offerable()).count()
    }
}

/// Why the folder could not be listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScanError {
    /// The folder is not there (deleted, renamed, a drive not attached).
    #[error("the inbox folder is missing")]
    Missing,
    /// The path is a file or a link, not a folder.
    #[error("the inbox path is not a folder")]
    NotAFolder,
    /// The folder could not be read (permissions, a device error).
    #[error("the inbox folder could not be read")]
    Unreadable,
}

/// The kind a name says, or `None` when the inbox does not take it.
#[must_use]
pub fn named_kind(name: &str) -> Option<NamedKind> {
    if name.starts_with('.') || name.starts_with("~$") {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    if [".part", ".crdownload", ".tmp"]
        .iter()
        .any(|s| lower.ends_with(s))
    {
        return None;
    }
    let (_, ext) = lower.rsplit_once('.')?;
    match ext {
        "pdf" => Some(NamedKind::Pdf),
        "png" => Some(NamedKind::Png),
        "jpg" | "jpeg" => Some(NamedKind::Jpeg),
        _ => None,
    }
}

/// Lists the inbox folder (not its sub-folders; links are not followed).
/// Every file comes back [`FileState::Settling`], [`FileState::TooLarge`]
/// or [`FileState::OnlineOnly`]; [`Stability::apply`] decides which
/// settling files are ready.
///
/// # Errors
/// [`ScanError`].
pub fn scan(dir: &Path) -> Result<Listing, ScanError> {
    let meta = fs::symlink_metadata(dir).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => ScanError::Missing,
        _ => ScanError::Unreadable,
    })?;
    if !meta.is_dir() {
        return Err(ScanError::NotAFolder);
    }
    let entries = fs::read_dir(dir).map_err(|_| ScanError::Unreadable)?;
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(kind) = named_kind(&name) else {
            continue;
        };
        let path = entry.path();
        // Not followed: a link is skipped, whatever it points at.
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() || hidden(&meta) {
            continue;
        }
        let size = meta.len();
        let state = if online_only(&meta) {
            FileState::OnlineOnly
        } else if size > MAX_FILE_BYTES {
            FileState::TooLarge
        } else {
            FileState::Settling
        };
        files.push(InboxFile {
            path,
            name,
            kind,
            size,
            modified_ms: millis(meta.modified()),
            state,
        });
    }
    files.sort_by(|a, b| {
        a.modified_ms
            .cmp(&b.modified_ms)
            .then_with(|| a.name.cmp(&b.name))
    });
    let more = files.len().saturating_sub(MAX_LISTED);
    files.truncate(MAX_LISTED);
    Ok(Listing { files, more })
}

/// Remembers what each file looked like at earlier polls, so a file is
/// ready only after its size and time stayed the same for
/// [`STABLE_FOR`] across at least two polls.
#[derive(Default)]
pub struct Stability {
    seen: HashMap<PathBuf, Seen>,
}

struct Seen {
    size: u64,
    modified_ms: i64,
    since: Instant,
}

impl fmt::Debug for Stability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stability")
            .field("tracked", &self.seen.len())
            .finish()
    }
}

impl Stability {
    /// Marks the settling files of `listing` that have been unchanged since
    /// a poll at least [`STABLE_FOR`] before `now` as ready (or empty), and
    /// forgets files that are gone.
    pub fn apply(&mut self, listing: &mut Listing, now: Instant) {
        let mut next = HashMap::with_capacity(listing.files.len());
        for f in &mut listing.files {
            let since = match self.seen.remove(&f.path) {
                Some(s) if s.size == f.size && s.modified_ms == f.modified_ms => s.since,
                _ => now,
            };
            if f.state == FileState::Settling && now.duration_since(since) >= STABLE_FOR {
                f.state = if f.size == 0 {
                    FileState::Empty
                } else {
                    FileState::Ready
                };
            }
            next.insert(
                f.path.clone(),
                Seen {
                    size: f.size,
                    modified_ms: f.modified_ms,
                    since,
                },
            );
        }
        self.seen = next;
    }

    /// Forget everything (another folder was chosen).
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

/// The folder a file filed in month `year`-`month` goes to.
#[must_use]
pub fn filed_dir(inbox: &Path, year: i32, month: u32) -> PathBuf {
    inbox.join(FILED_DIR).join(format!("{year:04}-{month:02}"))
}

/// Moves the inbox file `path` to `<inbox>/Filed/<YYYY-MM>/` once a record
/// holds its upload, by the rules of [`super::move_handled`]: the file must
/// still be the one uploaded (SHA-256 `expected`), nothing is ever
/// overwritten (` (2)`, ` (3)`, …), a `Filed` that is a link is refused,
/// and across volumes the copy is checked before the original is removed.
/// Returns where the file is now.
///
/// # Errors
/// `Gone`, `NotAFile`, `OnlineOnly`, `Changed` (different bytes now; nothing
/// moved), `Io` (nothing lost: the original stays when the copy failed).
pub fn file_away(
    inbox: &Path,
    path: &Path,
    expected: &str,
    year: i32,
    month: u32,
) -> Result<PathBuf, FileError> {
    let parts = [FILED_DIR.to_string(), format!("{year:04}-{month:02}")];
    super::handled::move_into(inbox, path, expected, &parts).map_err(|f| f.error)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%inbox test\n";
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0];

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).expect("write");
        p
    }

    #[test]
    fn names_are_filtered() {
        assert_eq!(named_kind("Receipt.PDF"), Some(NamedKind::Pdf));
        assert_eq!(named_kind("a.png"), Some(NamedKind::Png));
        assert_eq!(named_kind("a.JPG"), Some(NamedKind::Jpeg));
        assert_eq!(named_kind("a.jpeg"), Some(NamedKind::Jpeg));
        for skipped in [
            ".hidden.pdf",
            "~$lock.pdf",
            "x.pdf.part",
            "x.pdf.crdownload",
            "x.pdf.tmp",
            "x.TMP",
            "notes.txt",
            "pdf",
            "x.gif",
            "x.exe",
        ] {
            assert_eq!(named_kind(skipped), None, "{skipped}");
        }
    }

    #[test]
    fn scan_lists_only_this_folder_oldest_first() {
        let dir = tempfile::tempdir().expect("tmp");
        write(dir.path(), "b.pdf", PDF);
        write(dir.path(), "a.png", PNG);
        write(dir.path(), "skip.txt", b"x");
        fs::create_dir(dir.path().join("Filed")).expect("dir");
        write(&dir.path().join("Filed"), "old.pdf", PDF);
        fs::create_dir(dir.path().join("folder.pdf")).expect("dir");
        let l = scan(dir.path()).expect("scan");
        let mut names: Vec<_> = l.files.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["a.png", "b.pdf"]);
        assert!(l.files.iter().all(|f| f.state == FileState::Settling));
        assert_eq!(l.more, 0);
        assert_eq!(scan(&dir.path().join("nope")), Err(ScanError::Missing));
        assert_eq!(
            scan(&dir.path().join("skip.txt")),
            Err(ScanError::NotAFolder)
        );
        let shown = format!("{:?}", l.files[0]);
        assert!(!shown.contains(".p"), "no names in Debug: {shown}");
    }

    #[test]
    fn the_10_mb_edge() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut ok = PDF.to_vec();
        ok.resize(usize::try_from(MAX_FILE_BYTES).expect("fits"), b' ');
        let exact = write(dir.path(), "exact.pdf", &ok);
        ok.push(b' ');
        let over = write(dir.path(), "over.pdf", &ok);
        let l = scan(dir.path()).expect("scan");
        let state = |n: &str| l.files.iter().find(|f| f.name == n).map(|f| f.state);
        assert_eq!(state("exact.pdf"), Some(FileState::Settling));
        assert_eq!(state("over.pdf"), Some(FileState::TooLarge));
        assert_eq!(read_for_upload(&exact).map(|r| r.size), Ok(MAX_FILE_BYTES));
        assert_eq!(read_for_upload(&over), Err(FileError::TooLarge));
    }

    #[test]
    fn the_list_is_capped() {
        let dir = tempfile::tempdir().expect("tmp");
        for n in 0..(MAX_LISTED + 3) {
            write(dir.path(), &format!("r{n:04}.pdf"), PDF);
        }
        let l = scan(dir.path()).expect("scan");
        assert_eq!(l.files.len(), MAX_LISTED);
        assert_eq!(l.more, 3);
    }

    #[test]
    fn ready_only_after_two_equal_polls_two_seconds_apart() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        write(dir.path(), "empty.pdf", b"");
        let t0 = Instant::now();
        let mut s = Stability::default();
        let mut poll = |at: Instant| {
            let mut l = scan(dir.path()).expect("scan");
            s.apply(&mut l, at);
            l
        };
        let state = |l: &Listing, n: &str| {
            l.files
                .iter()
                .find(|f| f.name == n)
                .map(|f| f.state)
                .expect("listed")
        };
        let l = poll(t0);
        assert_eq!(state(&l, "r.pdf"), FileState::Settling, "first sight");
        let l = poll(t0 + Duration::from_millis(1500));
        assert_eq!(state(&l, "r.pdf"), FileState::Settling, "too soon");
        let l = poll(t0 + STABLE_FOR);
        assert_eq!(state(&l, "r.pdf"), FileState::Ready);
        assert_eq!(state(&l, "empty.pdf"), FileState::Empty);
        assert_eq!(l.ready(), 1);

        // It grows: settling again, from the new sight.
        fs::write(&p, [PDF, b"more"].concat()).expect("grow");
        let l = poll(t0 + Duration::from_secs(3));
        assert_eq!(state(&l, "r.pdf"), FileState::Settling);
        let l = poll(t0 + Duration::from_secs(4));
        assert_eq!(state(&l, "r.pdf"), FileState::Settling);
        let l = poll(t0 + Duration::from_secs(5));
        assert_eq!(state(&l, "r.pdf"), FileState::Ready);
    }

    #[test]
    fn read_checks_the_bytes() {
        let dir = tempfile::tempdir().expect("tmp");
        let good = write(dir.path(), "good.pdf", PDF);
        let r = read_for_upload(&good).expect("read");
        assert_eq!(r.bytes, PDF);
        assert_eq!(r.sha256, sha256_hex(PDF));
        let fake = write(dir.path(), "fake.pdf", b"MZ\x90\0not a pdf");
        assert_eq!(read_for_upload(&fake), Err(FileError::WrongType));
        let empty = write(dir.path(), "empty.pdf", b"");
        assert_eq!(read_for_upload(&empty), Err(FileError::Empty));
        assert_eq!(
            read_for_upload(&dir.path().join("gone.pdf")),
            Err(FileError::Gone)
        );
        assert_eq!(read_for_upload(dir.path()), Err(FileError::NotAFile));
    }

    #[cfg(unix)]
    #[test]
    fn links_are_never_followed() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let target = write(outside.path(), "secret.pdf", PDF);
        std::os::unix::fs::symlink(&target, dir.path().join("link.pdf")).expect("link");
        let l = scan(dir.path()).expect("scan");
        assert!(l.files.is_empty(), "the link is skipped");
        assert_eq!(
            read_for_upload(&dir.path().join("link.pdf")),
            Err(FileError::NotAFile)
        );
    }

    #[cfg(windows)]
    #[test]
    fn links_are_never_followed() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let target = write(outside.path(), "secret.pdf", PDF);
        // Creating a symlink needs developer mode or admin rights on
        // Windows; without them there is nothing to check here.
        if std::os::windows::fs::symlink_file(&target, dir.path().join("link.pdf")).is_err() {
            return;
        }
        let l = scan(dir.path()).expect("scan");
        assert!(l.files.is_empty(), "the link is skipped");
        assert_eq!(
            read_for_upload(&dir.path().join("link.pdf")),
            Err(FileError::NotAFile)
        );
    }

    #[test]
    fn filing_never_overwrites() {
        let dir = tempfile::tempdir().expect("tmp");
        let filed = filed_dir(dir.path(), 2026, 11);
        fs::create_dir_all(&filed).expect("dir");
        write(&filed, "r.pdf", b"%PDF-older");
        write(&filed, "r (2).pdf", b"%PDF-older 2");
        let p = write(dir.path(), "r.pdf", PDF);
        let to = file_away(dir.path(), &p, &sha256_hex(PDF), 2026, 11).expect("filed");
        assert_eq!(to, filed.join("r (3).pdf"));
        assert!(!p.exists(), "the original moved");
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert_eq!(
            fs::read(filed.join("r.pdf")).expect("read"),
            b"%PDF-older",
            "never overwritten"
        );
        // A name without an extension.
        let q = write(dir.path(), "noext", PDF);
        write(&filed, "noext", b"x");
        assert_eq!(
            file_away(dir.path(), &q, &sha256_hex(PDF), 2026, 11).expect("filed"),
            filed.join("noext (2)")
        );
    }

    #[test]
    fn a_changed_file_is_not_filed() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        assert_eq!(
            file_away(dir.path(), &p, &sha256_hex(b"other bytes"), 2026, 11),
            Err(FileError::Changed)
        );
        assert!(p.exists(), "left where it is");
        assert!(!filed_dir(dir.path(), 2026, 11).exists());
        assert_eq!(
            file_away(dir.path(), &dir.path().join("gone.pdf"), "x", 2026, 11),
            Err(FileError::Gone)
        );
    }

    /// A `Filed` that is a link is never followed: the bill stays in the
    /// inbox instead of landing wherever the link points.
    #[test]
    fn a_linked_or_odd_filed_folder_is_refused() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(outside.path(), dir.path().join(FILED_DIR)).is_ok();
        // Needs developer mode or admin rights on Windows.
        #[cfg(windows)]
        let linked =
            std::os::windows::fs::symlink_dir(outside.path(), dir.path().join(FILED_DIR)).is_ok();
        if linked {
            assert_eq!(
                file_away(dir.path(), &p, &sha256_hex(PDF), 2026, 11),
                Err(FileError::NotAFile)
            );
            assert!(p.exists(), "left where it is");
            assert_eq!(
                fs::read_dir(outside.path()).expect("dir").count(),
                0,
                "nothing was made through the link"
            );
            fs::remove_dir(dir.path().join(FILED_DIR))
                .or_else(|_| fs::remove_file(dir.path().join(FILED_DIR)))
                .expect("unlink");
        }
        // A plain file named `Filed` (works on every system).
        write(dir.path(), FILED_DIR, b"not a folder");
        assert_eq!(
            file_away(dir.path(), &p, &sha256_hex(PDF), 2026, 11),
            Err(FileError::NotAFile)
        );
        assert!(p.exists());
    }
}
