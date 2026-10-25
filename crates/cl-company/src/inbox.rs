//! The receipt inbox folder (P17, plan §3 WP5, Q-54, Q-P17p): what is in
//! the owner's inbox folder, whether each file is ready to attach, reading
//! one for upload, and filing it away once a record holds it.
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
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

/// The largest file read (the engine's attachment limit).
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// The most files listed.
pub const MAX_LISTED: usize = 500;

/// How long a file's size and time must stay the same before it is ready.
pub const STABLE_FOR: Duration = Duration::from_secs(2);

/// The sub-folder filed originals go to.
pub const FILED_DIR: &str = "Filed";

/// Windows attributes of files whose content is not on this computer.
#[cfg(windows)]
const ONLINE_ONLY_ATTRIBUTES: u32 = 0x0000_1000 // OFFLINE
    | 0x0004_0000 // RECALL_ON_OPEN
    | 0x0040_0000; // RECALL_ON_DATA_ACCESS

/// Windows hidden and system attributes.
#[cfg(windows)]
const HIDDEN_ATTRIBUTES: u32 = 0x2 | 0x4;

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

fn millis(t: io::Result<SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(windows)]
fn attributes(meta: &fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes()
}

#[cfg(windows)]
fn online_only(meta: &fs::Metadata) -> bool {
    attributes(meta) & ONLINE_ONLY_ATTRIBUTES != 0
}

#[cfg(not(windows))]
fn online_only(_meta: &fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn hidden(meta: &fs::Metadata) -> bool {
    attributes(meta) & HIDDEN_ATTRIBUTES != 0
}

#[cfg(not(windows))]
fn hidden(_meta: &fs::Metadata) -> bool {
    false
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

/// A file read for upload.
#[derive(Clone, PartialEq, Eq)]
pub struct ReadFile {
    /// Its bytes (a PDF, PNG or JPEG by its first bytes, at most 10 MB).
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

/// Why a file was not read or filed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    /// It is gone (moved, deleted).
    #[error("the file is gone")]
    Gone,
    /// A link or not a plain file.
    #[error("not a plain file")]
    NotAFile,
    /// Online only (Windows cloud placeholder).
    #[error("the file is online only")]
    OnlineOnly,
    /// Over 10 MB.
    #[error("the file is too large")]
    TooLarge,
    /// Empty.
    #[error("the file is empty")]
    Empty,
    /// Not a PDF, PNG or JPEG by its bytes.
    #[error("not a PDF or picture")]
    WrongType,
    /// It changed since it was read (different bytes).
    #[error("the file changed")]
    Changed,
    /// It changed while being read.
    #[error("the file changed while it was read")]
    Unstable,
    /// The file system refused.
    #[error("the file could not be read or moved")]
    Io,
}

fn hex(bytes: &[u8]) -> String {
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

fn plain_file(path: &Path) -> Result<fs::Metadata, FileError> {
    let meta = fs::symlink_metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => FileError::Gone,
        _ => FileError::Io,
    })?;
    if !meta.file_type().is_file() {
        return Err(FileError::NotAFile);
    }
    if online_only(&meta) {
        return Err(FileError::OnlineOnly);
    }
    Ok(meta)
}

/// Reads an inbox file for upload, checking again everything the listing
/// said: a plain file (no link), not online only, at most 10 MB, not
/// empty, a PDF, PNG or JPEG by its first bytes, and unchanged while read.
///
/// # Errors
/// [`FileError`].
pub fn read_for_upload(path: &Path) -> Result<ReadFile, FileError> {
    let before = plain_file(path)?;
    if before.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    let file = fs::File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => FileError::Gone,
        _ => FileError::Io,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FileError::Io)?;
    let size = bytes.len() as u64;
    if size > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    if size == 0 {
        return Err(FileError::Empty);
    }
    let after = plain_file(path)?;
    let modified_ms = millis(after.modified());
    if after.len() != size || millis(before.modified()) != modified_ms {
        return Err(FileError::Unstable);
    }
    cl_api::check_file(&bytes).map_err(|_| FileError::WrongType)?;
    let sha256 = sha256_hex(&bytes);
    Ok(ReadFile {
        bytes,
        size,
        modified_ms,
        sha256,
    })
}

fn file_sha256(path: &Path) -> Result<String, FileError> {
    let mut file = fs::File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => FileError::Gone,
        _ => FileError::Io,
    })?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|_| FileError::Io)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// `name` with ` (n)` before its extension (`n` ≥ 2).
fn numbered(name: &str, n: u32) -> String {
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
fn reserve(dir: &Path, name: &str) -> Result<PathBuf, FileError> {
    for n in 1..10_000 {
        let path = dir.join(numbered(name, n));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(FileError::Io),
        }
    }
    Err(FileError::Io)
}

/// The folder a file filed in month `year`-`month` goes to.
#[must_use]
pub fn filed_dir(inbox: &Path, year: i32, month: u32) -> PathBuf {
    inbox.join(FILED_DIR).join(format!("{year:04}-{month:02}"))
}

/// Moves the inbox file `path` to `<inbox>/Filed/<YYYY-MM>/` once a record
/// holds its upload. The file must still be the one uploaded (SHA-256
/// `expected`). The new name is the old one, or ` (2)`, ` (3)`, … when that
/// is taken: nothing is ever overwritten. A rename is used when it can be;
/// across volumes the bytes are copied, the copy is checked against
/// `expected`, and only then is the original removed. Returns where the
/// file is now.
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
    plain_file(path)?;
    if file_sha256(path)? != expected {
        return Err(FileError::Changed);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(FileError::NotAFile)?
        .to_string();
    let dir = filed_dir(inbox, year, month);
    fs::create_dir_all(&dir).map_err(|_| FileError::Io)?;
    let target = reserve(&dir, &name)?;
    // The reserved name is ours: the rename replaces only our empty file.
    if fs::rename(path, &target).is_ok() {
        return Ok(target);
    }
    match copy_verified(path, &target, expected) {
        Ok(()) => {
            if fs::remove_file(path).is_err() {
                // Two copies would be filed again later: keep the original.
                let _ = fs::remove_file(&target);
                return Err(FileError::Io);
            }
            Ok(target)
        }
        Err(e) => {
            let _ = fs::remove_file(&target);
            Err(e)
        }
    }
}

fn copy_verified(from: &Path, to: &Path, expected: &str) -> Result<(), FileError> {
    let mut src = fs::File::open(from).map_err(|_| FileError::Io)?;
    let mut dst = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(to)
        .map_err(|_| FileError::Io)?;
    io::copy(&mut src, &mut dst).map_err(|_| FileError::Io)?;
    dst.flush().map_err(|_| FileError::Io)?;
    dst.sync_all().map_err(|_| FileError::Io)?;
    drop(dst);
    if file_sha256(to)? == expected {
        Ok(())
    } else {
        Err(FileError::Changed)
    }
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

    #[test]
    fn a_copy_is_checked_before_the_original_goes() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write(dir.path(), "r.pdf", PDF);
        let to = dir.path().join("copy.pdf");
        fs::write(&to, b"").expect("reserve");
        assert_eq!(copy_verified(&p, &to, &sha256_hex(PDF)), Ok(()));
        assert_eq!(fs::read(&to).expect("read"), PDF);
        assert_eq!(
            copy_verified(&p, &to, &sha256_hex(b"x")),
            Err(FileError::Changed)
        );
        assert_eq!(numbered("a.b.pdf", 2), "a.b (2).pdf");
        assert_eq!(numbered(".pdf", 2), ".pdf (2)");
        assert_eq!(numbered("a.pdf", 1), "a.pdf");
    }
}
