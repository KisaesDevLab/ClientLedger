//! What is in an intake folder, and whether each file is ready to copy in
//! (A01 plan D2, D3).
//!
//! - The root is looked at **following** links (it may be a link into a
//!   cloud folder; [`super::canonical_key`] pins it). Entries are looked at
//!   **without** following links: only plain files count, so a link,
//!   junction or folder inside never brings in a file from elsewhere.
//! - Only the folder itself is listed, never sub-folders, so `Processed/`,
//!   `Discarded/` and P17's `Filed/` are never offered again.
//! - Skipped without a trace: dot-files, Office lock files (`~$…`,
//!   `.~lock.…#`), partial downloads (`*.part`, `*.crdownload`,
//!   `*.download`, `*.tmp`), `Thumbs.db`, `desktop.ini`, hidden or system
//!   files, names that aren't valid UTF-8, and other kinds of file.
//! - Zero-byte files are not listed (Q-A01o), but still tracked so a file
//!   that is being written settles as soon as it has its bytes.
//! - Listed, never read: over 10 MB (Q-50), HEIC/HEIF (Q-292), cloud
//!   placeholders (online only), and hard-linked files on Unix (a hard
//!   link could bring in a file from outside the folder).
//! - A file is **ready** when its size and modified time stayed the same
//!   across two looks at least [`STABLE_FOR`] apart **and** it opens while
//!   refusing writers ([`super::FolderFs::openable`]).
//! - At most [`MAX_SCANNED`] entries are read per look ([`Listing::truncated`])
//!   and [`MAX_LISTED`] files listed, oldest first ([`Listing::more`]).

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::fs::{FolderFs, links, millis, online_only, open_shared_read};
use super::path::classify;
use super::{FileError, Health, ReadFile, sha256_hex};

/// The largest file read (the engine's attachment limit), inclusive.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// The most files listed per folder.
pub const MAX_LISTED: usize = 500;

/// The most directory entries read per look.
pub const MAX_SCANNED: usize = 10_000;

/// How long a file's size and time must stay the same before it is ready.
pub const STABLE_FOR: Duration = Duration::from_secs(5);

/// What kind of document a file's name says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DocKind {
    /// `.pdf`.
    Pdf,
    /// `.png`.
    Png,
    /// `.jpg` or `.jpeg`.
    Jpeg,
    /// `.tif` or `.tiff` (Q-A01n).
    Tiff,
    /// `.heic` or `.heif`: listed, never read (Q-292).
    Heic,
}

impl DocKind {
    /// A short name for the control channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            DocKind::Pdf => "pdf",
            DocKind::Png => "png",
            DocKind::Jpeg => "jpeg",
            DocKind::Tiff => "tiff",
            DocKind::Heic => "heic",
        }
    }
}

/// Where a listed file stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemState {
    /// Seen, but not yet unchanged across two looks (or still held open
    /// for writing).
    Settling,
    /// Unchanged and openable: it can be copied in.
    Ready,
    /// Over 10 MB: never read ("Too large (10 MB limit)").
    TooLarge,
    /// HEIC/HEIF: never read ("Save as JPG or PDF instead").
    NotSupported,
    /// Its content is online only (a cloud placeholder): never read
    /// ("Online only — choose Always keep on this device").
    OnlineOnly,
    /// A file with more than one hard link (Unix): never read ("Linked
    /// file, not taken").
    Linked,
}

impl ItemState {
    /// True when the file may be copied in.
    #[must_use]
    pub fn offerable(self) -> bool {
        self == ItemState::Ready
    }

    /// A short name for the control channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ItemState::Settling => "settling",
            ItemState::Ready => "ready",
            ItemState::TooLarge => "too_large",
            ItemState::NotSupported => "not_supported",
            ItemState::OnlineOnly => "online_only",
            ItemState::Linked => "linked",
        }
    }
}

/// One file in an intake folder.
#[derive(Clone, PartialEq, Eq)]
pub struct IntakeFile {
    /// Its full path (owner data).
    pub path: PathBuf,
    /// Its file name (owner data).
    pub name: String,
    /// The kind its name says.
    pub kind: DocKind,
    /// Size in bytes.
    pub size: u64,
    /// Modified time, milliseconds since 1970 (0 when unknown).
    pub modified_ms: i64,
    /// Where it stands.
    pub state: ItemState,
}

/// Kind, size and state only: names and paths are owner data.
impl fmt::Debug for IntakeFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IntakeFile")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// What a look at a folder found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The files, oldest first, at most [`MAX_LISTED`].
    pub files: Vec<IntakeFile>,
    /// Matching files left out over the cap.
    pub more: usize,
    /// The folder has more than [`MAX_SCANNED`] entries; only the first
    /// were looked at ([`Health::TooMany`]). Never prune entries from a
    /// truncated listing.
    pub truncated: bool,
    /// Zero-byte files tracked but not listed (still being written, or
    /// empty).
    pub waiting: usize,
}

impl Listing {
    /// Files that can be copied in now.
    #[must_use]
    pub fn ready(&self) -> usize {
        self.files.iter().filter(|f| f.state.offerable()).count()
    }

    /// True while anything is settling (the watcher looks again sooner).
    #[must_use]
    pub fn settling(&self) -> bool {
        self.waiting > 0 || self.files.iter().any(|f| f.state == ItemState::Settling)
    }

    /// True when every matching file is in [`Listing::files`] (nothing
    /// capped, nothing truncated): only then may missing files be pruned.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.more == 0 && !self.truncated
    }
}

/// True for names skipped without a trace (temporary, lock and system
/// files).
#[must_use]
pub fn skipped_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('.') || name.starts_with("~$") {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    lower == "thumbs.db"
        || lower == "desktop.ini"
        || [".part", ".crdownload", ".download", ".tmp"]
            .iter()
            .any(|s| lower.ends_with(s))
}

/// The kind a name says, or `None` when intake does not take it (or the
/// name is skipped).
#[must_use]
pub fn doc_kind(name: &str) -> Option<DocKind> {
    if skipped_name(name) {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    let (stem, ext) = lower.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    match ext {
        "pdf" => Some(DocKind::Pdf),
        "png" => Some(DocKind::Png),
        "jpg" | "jpeg" => Some(DocKind::Jpeg),
        "tif" | "tiff" => Some(DocKind::Tiff),
        "heic" | "heif" => Some(DocKind::Heic),
        _ => None,
    }
}

/// Looks at `root` once: every matching file comes back with its state;
/// `stability` decides which settling files are ready (and must be the
/// same one across looks at the same folder).
///
/// # Errors
/// The folder's [`Health`] (`Missing`, `Unreachable`, `PermissionDenied`,
/// `NotAFolder`).
pub fn scan(
    fs: &dyn FolderFs,
    root: &Path,
    stability: &mut Stability,
    now: Instant,
) -> Result<Listing, Health> {
    match fs.root_is_dir(root) {
        Ok(true) => {}
        Ok(false) => return Err(Health::NotAFolder),
        Err(e) => return Err(classify(&e, true)),
    }
    let raw = fs.list(root, MAX_SCANNED).map_err(|e| classify(&e, true))?;
    let mut files = Vec::new();
    for e in raw.entries {
        let Some(name) = e.name else {
            continue;
        };
        if !e.is_file || e.hidden {
            continue;
        }
        let Some(kind) = doc_kind(&name) else {
            continue;
        };
        let state = if e.online_only {
            ItemState::OnlineOnly
        } else if e.links > 1 {
            ItemState::Linked
        } else if kind == DocKind::Heic {
            ItemState::NotSupported
        } else if e.size > MAX_FILE_BYTES {
            ItemState::TooLarge
        } else {
            ItemState::Settling
        };
        files.push(IntakeFile {
            path: e.path,
            name,
            kind,
            size: e.size,
            modified_ms: e.modified_ms,
            state,
        });
    }
    Ok(stability.apply(fs, files, raw.truncated, now))
}

/// Remembers what each file looked like at earlier looks, so a file is
/// ready only after its size and time stayed the same for [`STABLE_FOR`]
/// across at least two looks.
#[derive(Default)]
pub struct Stability {
    seen: HashMap<PathBuf, Seen>,
}

struct Seen {
    size: u64,
    modified_ms: i64,
    since: Instant,
    /// It opened while refusing writers at this size and time.
    openable: bool,
}

impl fmt::Debug for Stability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stability")
            .field("tracked", &self.seen.len())
            .finish()
    }
}

impl Stability {
    /// Marks the settling `files` that have been unchanged since a look at
    /// least [`STABLE_FOR`] before `now` (and open) as ready, forgets files
    /// that are gone, and returns the listing: zero-byte files left out,
    /// oldest first, capped.
    pub fn apply(
        &mut self,
        fs: &dyn FolderFs,
        files: Vec<IntakeFile>,
        truncated: bool,
        now: Instant,
    ) -> Listing {
        let mut next = HashMap::with_capacity(files.len());
        let mut listed = Vec::with_capacity(files.len());
        let mut waiting = 0;
        for mut f in files {
            let (since, mut openable) = match self.seen.remove(&f.path) {
                Some(s) if s.size == f.size && s.modified_ms == f.modified_ms => {
                    (s.since, s.openable)
                }
                _ => (now, false),
            };
            if f.state == ItemState::Settling
                && f.size > 0
                && now.duration_since(since) >= STABLE_FOR
            {
                openable = openable || fs.openable(&f.path);
                if openable {
                    f.state = ItemState::Ready;
                }
            }
            next.insert(
                f.path.clone(),
                Seen {
                    size: f.size,
                    modified_ms: f.modified_ms,
                    since,
                    openable,
                },
            );
            if f.size == 0 {
                waiting += 1;
            } else {
                listed.push(f);
            }
        }
        self.seen = next;
        listed.sort_by(|a, b| {
            a.modified_ms
                .cmp(&b.modified_ms)
                .then_with(|| a.name.cmp(&b.name))
        });
        let more = listed.len().saturating_sub(MAX_LISTED);
        listed.truncate(MAX_LISTED);
        Listing {
            files: listed,
            more,
            truncated,
            waiting,
        }
    }

    /// Forget everything (another folder).
    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

/// True for TIFF's first bytes (`II*\0` or `MM\0*`).
fn is_tiff(bytes: &[u8]) -> bool {
    bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*")
}

/// Checks a file's bytes: not empty, at most 10 MB, a PDF, PNG, JPEG or
/// TIFF by its first bytes.
pub(crate) fn check_bytes(bytes: &[u8]) -> Result<(), FileError> {
    if bytes.is_empty() {
        return Err(FileError::Empty);
    }
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    // TIFF until `cl_api::check_file` takes it (A01 E3).
    if cl_api::check_file(bytes).is_ok() || is_tiff(bytes) {
        Ok(())
    } else {
        Err(FileError::WrongType)
    }
}

fn plain_file(path: &Path) -> Result<std::fs::Metadata, FileError> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => FileError::Gone,
        _ => FileError::Io,
    })?;
    if !meta.file_type().is_file() || links(&meta) > 1 {
        return Err(FileError::NotAFile);
    }
    if online_only(&meta) {
        return Err(FileError::OnlineOnly);
    }
    Ok(meta)
}

/// The open handle's metadata `held` against the path's `before` (checked
/// without following links): a plain file, not online only, the same size
/// and modification time.
pub(crate) fn same_file(
    before: &std::fs::Metadata,
    held: &std::fs::Metadata,
) -> Result<(), FileError> {
    if !held.file_type().is_file() {
        return Err(FileError::NotAFile);
    }
    if online_only(held) {
        return Err(FileError::OnlineOnly);
    }
    if held.len() != before.len() || millis(held.modified()) != millis(before.modified()) {
        return Err(FileError::Unstable);
    }
    Ok(())
}

/// Reads a file for upload, checking again everything the listing said: a
/// plain file (no link, one hard link), not online only, at most 10 MB,
/// not empty, a PDF, PNG, JPEG or TIFF by its first bytes, not held open
/// for writing (Windows), and unchanged while read.
///
/// # Errors
/// [`FileError`].
pub fn read_for_upload(path: &Path) -> Result<ReadFile, FileError> {
    let before = plain_file(path)?;
    if before.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    let file = open_shared_read(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => FileError::Gone,
        // A sharing violation (still being written) on Windows.
        _ if e.raw_os_error() == Some(32) && cfg!(windows) => FileError::Unstable,
        _ => FileError::Io,
    })?;
    // What was opened is checked on the open handle itself, so a file
    // swapped in between the check and the open is noticed.
    let held = file.metadata().map_err(|_| FileError::Io)?;
    same_file(&before, &held)?;
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_FILE_BYTES + 1)
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
    same_file(&after, &file.metadata().map_err(|_| FileError::Io)?)?;
    let modified_ms = millis(after.modified());
    if after.len() != size || millis(before.modified()) != modified_ms {
        return Err(FileError::Unstable);
    }
    check_bytes(&bytes)?;
    let sha256 = sha256_hex(&bytes);
    Ok(ReadFile {
        bytes,
        size,
        modified_ms,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::super::fs::{FakeFs, RealFs};
    use super::*;

    const PDF: &[u8] = b"%PDF-1.4\n%intake test\n";
    const TIFF: &[u8] = b"II*\0\x08\0\0\0tiff";
    const ROOT: &str = "/intake/bills";

    fn root() -> &'static Path {
        Path::new(ROOT)
    }

    fn state_of(l: &Listing, name: &str) -> Option<ItemState> {
        l.files.iter().find(|f| f.name == name).map(|f| f.state)
    }

    #[test]
    fn names_are_filtered() {
        assert_eq!(doc_kind("Receipt.PDF"), Some(DocKind::Pdf));
        assert_eq!(doc_kind("a.png"), Some(DocKind::Png));
        assert_eq!(doc_kind("a.JPG"), Some(DocKind::Jpeg));
        assert_eq!(doc_kind("a.jpeg"), Some(DocKind::Jpeg));
        assert_eq!(doc_kind("scan.TIF"), Some(DocKind::Tiff));
        assert_eq!(doc_kind("scan.tiff"), Some(DocKind::Tiff));
        assert_eq!(doc_kind("IMG_1.HEIC"), Some(DocKind::Heic));
        assert_eq!(doc_kind("IMG_1.heif"), Some(DocKind::Heic));
        for skipped in [
            ".hidden.pdf",
            "~$lock.pdf",
            ".~lock.bill.pdf#",
            "x.pdf.part",
            "x.pdf.crdownload",
            "x.pdf.download",
            "x.pdf.tmp",
            "x.TMP",
            "Thumbs.db",
            "DESKTOP.INI",
            "notes.txt",
            "pdf",
            ".pdf",
            "x.gif",
            "x.exe",
            "",
        ] {
            assert_eq!(doc_kind(skipped), None, "{skipped}");
        }
        assert!(skipped_name("thumbs.DB") && !skipped_name("a.pdf"));
    }

    #[test]
    fn what_is_listed_and_what_is_never_read() {
        let fs = FakeFs::new();
        fs.put(root(), "bill.pdf", PDF, 10);
        fs.put(root(), "scan.tif", TIFF, 11);
        fs.put(root(), "phone.heic", b"ftypheic", 12);
        fs.put(root(), "empty.pdf", b"", 13);
        fs.put(root(), "notes.txt", b"x", 14);
        fs.put_hidden(root(), "hidden.pdf", PDF);
        fs.put_online_only(root(), "cloud.pdf", 100);
        fs.put_link(root(), "link.pdf");
        fs.put_hard_linked(root(), "hard.pdf", PDF, 2);
        let mut big = PDF.to_vec();
        big.resize(usize::try_from(MAX_FILE_BYTES).expect("fits") + 1, b' ');
        fs.put(root(), "big.pdf", &big, 15);
        let mut s = Stability::default();
        let l = scan(&fs, root(), &mut s, Instant::now()).expect("scan");
        let mut names: Vec<_> = l.files.iter().map(|f| f.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "big.pdf",
                "bill.pdf",
                "cloud.pdf",
                "hard.pdf",
                "phone.heic",
                "scan.tif"
            ]
        );
        assert_eq!(state_of(&l, "bill.pdf"), Some(ItemState::Settling));
        assert_eq!(state_of(&l, "scan.tif"), Some(ItemState::Settling));
        assert_eq!(state_of(&l, "phone.heic"), Some(ItemState::NotSupported));
        assert_eq!(state_of(&l, "cloud.pdf"), Some(ItemState::OnlineOnly));
        assert_eq!(state_of(&l, "hard.pdf"), Some(ItemState::Linked));
        assert_eq!(state_of(&l, "big.pdf"), Some(ItemState::TooLarge));
        assert_eq!(l.waiting, 1, "the zero-byte file is tracked, not listed");
        assert!(l.settling() && l.complete());
        let shown = format!("{:?}", l.files[0]);
        assert!(
            !shown.contains(".pdf") && !shown.contains("bill"),
            "{shown}"
        );
    }

    #[test]
    fn ready_only_after_two_equal_looks_five_seconds_apart_and_openable() {
        let fs = FakeFs::new();
        fs.put(root(), "r.pdf", PDF, 1);
        fs.put(root(), "w.pdf", b"", 1);
        let t0 = Instant::now();
        let mut s = Stability::default();
        let mut look = |at: Instant| scan(&fs, root(), &mut s, at).expect("scan");
        let l = look(t0);
        assert_eq!(
            state_of(&l, "r.pdf"),
            Some(ItemState::Settling),
            "first sight"
        );
        let l = look(t0 + Duration::from_millis(4900));
        assert_eq!(state_of(&l, "r.pdf"), Some(ItemState::Settling), "too soon");
        let l = look(t0 + STABLE_FOR);
        assert_eq!(state_of(&l, "r.pdf"), Some(ItemState::Ready));
        assert_eq!(l.ready(), 1);
        assert_eq!(l.waiting, 1);

        // The zero-byte file gets its bytes (two halves, 3 s apart).
        fs.put(root(), "w.pdf", &PDF[..8], 2);
        let l = look(t0 + Duration::from_secs(6));
        assert_eq!(state_of(&l, "w.pdf"), Some(ItemState::Settling));
        fs.put(root(), "w.pdf", PDF, 3);
        let l = look(t0 + Duration::from_secs(9));
        assert_eq!(state_of(&l, "w.pdf"), Some(ItemState::Settling), "grew");
        let l = look(t0 + Duration::from_secs(13));
        assert_eq!(state_of(&l, "w.pdf"), Some(ItemState::Settling), "4 s");
        // Still held open by the scanner: stays settling.
        fs.set_locked(&root().join("w.pdf"), true);
        let l = look(t0 + Duration::from_secs(14));
        assert_eq!(state_of(&l, "w.pdf"), Some(ItemState::Settling), "locked");
        fs.set_locked(&root().join("w.pdf"), false);
        let l = look(t0 + Duration::from_secs(15));
        assert_eq!(state_of(&l, "w.pdf"), Some(ItemState::Ready));
        assert!(!l.settling());

        // Gone files are forgotten.
        fs.remove(root(), "r.pdf");
        let l = look(t0 + Duration::from_secs(16));
        assert_eq!(state_of(&l, "r.pdf"), None);
        assert_eq!(format!("{s:?}"), "Stability { tracked: 1 }");
    }

    #[test]
    fn caps_entries_and_listing() {
        let fs = FakeFs::new();
        for n in 0..(MAX_LISTED + 3) {
            fs.put(
                root(),
                &format!("r{n:04}.pdf"),
                PDF,
                i64::try_from(n).expect("n"),
            );
        }
        let l = scan(&fs, root(), &mut Stability::default(), Instant::now()).expect("scan");
        assert_eq!(l.files.len(), MAX_LISTED);
        assert_eq!(l.more, 3);
        assert_eq!(l.files[0].name, "r0000.pdf", "oldest first");
        assert!(!l.truncated && !l.complete());

        let many = FakeFs::new();
        for n in 0..=MAX_SCANNED {
            many.put(root(), &format!("n{n}.txt"), b"x", 1);
        }
        let l = scan(&many, root(), &mut Stability::default(), Instant::now()).expect("scan");
        assert!(l.truncated && !l.complete());
    }

    #[test]
    fn root_errors_are_health() {
        let fs = FakeFs::new();
        let mut s = Stability::default();
        assert_eq!(
            scan(&fs, Path::new("/none"), &mut s, Instant::now()),
            Err(Health::Missing)
        );
        fs.add_dir(root());
        fs.fail(root(), if cfg!(windows) { 53 } else { 107 }, 1);
        let unreachable = scan(&fs, root(), &mut s, Instant::now());
        assert_eq!(unreachable, Err(Health::Unreachable));
        fs.fail(root(), if cfg!(windows) { 5 } else { 13 }, 1);
        assert_eq!(
            scan(&fs, root(), &mut s, Instant::now()),
            Err(Health::PermissionDenied)
        );
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("a.pdf");
        std::fs::write(&file, PDF).expect("write");
        assert_eq!(
            scan(&RealFs, &file, &mut s, Instant::now()),
            Err(Health::NotAFolder)
        );
        assert_eq!(
            scan(&RealFs, &dir.path().join("nope"), &mut s, Instant::now()),
            Err(Health::Missing)
        );
    }

    #[test]
    fn the_10_mb_edge_and_bytes() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut ok = PDF.to_vec();
        ok.resize(usize::try_from(MAX_FILE_BYTES).expect("fits"), b' ');
        let exact = dir.path().join("exact.pdf");
        std::fs::write(&exact, &ok).expect("write");
        ok.push(b' ');
        let over = dir.path().join("over.pdf");
        std::fs::write(&over, &ok).expect("write");
        assert_eq!(read_for_upload(&exact).map(|r| r.size), Ok(MAX_FILE_BYTES));
        assert_eq!(read_for_upload(&over), Err(FileError::TooLarge));
        let tif = dir.path().join("s.tif");
        std::fs::write(&tif, TIFF).expect("write");
        assert_eq!(
            read_for_upload(&tif).map(|r| r.sha256),
            Ok(sha256_hex(TIFF))
        );
        std::fs::write(&tif, b"MM\0*big-endian").expect("write");
        assert!(read_for_upload(&tif).is_ok());
        let heic = dir.path().join("p.heic");
        std::fs::write(&heic, b"\0\0\0\x18ftypheic").expect("write");
        assert_eq!(read_for_upload(&heic), Err(FileError::WrongType));
        let empty = dir.path().join("e.pdf");
        std::fs::write(&empty, b"").expect("write");
        assert_eq!(read_for_upload(&empty), Err(FileError::Empty));
        assert_eq!(
            read_for_upload(&dir.path().join("gone.pdf")),
            Err(FileError::Gone)
        );
        assert_eq!(read_for_upload(dir.path()), Err(FileError::NotAFile));
        assert_eq!(check_bytes(b""), Err(FileError::Empty));
    }

    /// Half written (Windows: still held open for writing): not read.
    #[cfg(windows)]
    #[test]
    fn a_file_still_being_written_is_not_read() {
        use std::io::Write as _;
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("scan.pdf");
        let mut w = std::fs::File::create(&p).expect("create");
        w.write_all(&PDF[..6]).expect("half");
        w.flush().expect("flush");
        assert_eq!(read_for_upload(&p), Err(FileError::Unstable));
        w.write_all(&PDF[6..]).expect("rest");
        drop(w);
        assert_eq!(read_for_upload(&p).map(|r| r.bytes), Ok(PDF.to_vec()));
    }

    #[cfg(unix)]
    #[test]
    fn links_and_hard_links_are_never_taken() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let target = outside.path().join("secret.pdf");
        std::fs::write(&target, PDF).expect("write");
        std::os::unix::fs::symlink(&target, dir.path().join("link.pdf")).expect("link");
        std::fs::hard_link(&target, dir.path().join("hard.pdf")).expect("hard link");
        let l = scan(
            &RealFs,
            dir.path(),
            &mut Stability::default(),
            Instant::now(),
        )
        .expect("scan");
        assert_eq!(l.files.len(), 1, "the symlink is skipped");
        assert_eq!(l.files[0].state, ItemState::Linked);
        assert_eq!(
            read_for_upload(&dir.path().join("link.pdf")),
            Err(FileError::NotAFile)
        );
        assert_eq!(
            read_for_upload(&dir.path().join("hard.pdf")),
            Err(FileError::NotAFile)
        );
    }

    #[cfg(windows)]
    #[test]
    fn links_are_never_followed() {
        let dir = tempfile::tempdir().expect("tmp");
        let outside = tempfile::tempdir().expect("tmp");
        let target = outside.path().join("secret.pdf");
        std::fs::write(&target, PDF).expect("write");
        // Creating a symlink needs developer mode or admin rights on
        // Windows; without them there is nothing to check here.
        if std::os::windows::fs::symlink_file(&target, dir.path().join("link.pdf")).is_err() {
            return;
        }
        let l = scan(
            &RealFs,
            dir.path(),
            &mut Stability::default(),
            Instant::now(),
        )
        .expect("scan");
        assert!(l.files.is_empty(), "the link is skipped");
        assert_eq!(
            read_for_upload(&dir.path().join("link.pdf")),
            Err(FileError::NotAFile)
        );
    }

    /// A linked root is followed (macOS `~/Dropbox` is a link into
    /// `~/Library/CloudStorage`); its entries still aren't.
    #[cfg(unix)]
    #[test]
    fn a_linked_root_is_followed() {
        let real = tempfile::tempdir().expect("tmp");
        std::fs::write(real.path().join("a.pdf"), PDF).expect("write");
        let holder = tempfile::tempdir().expect("tmp");
        let link = holder.path().join("Dropbox");
        std::os::unix::fs::symlink(real.path(), &link).expect("link");
        let l = scan(&RealFs, &link, &mut Stability::default(), Instant::now()).expect("scan");
        assert_eq!(l.files.len(), 1);
    }

    #[test]
    fn the_open_handle_must_be_the_checked_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let a = dir.path().join("a.pdf");
        std::fs::write(&a, PDF).expect("write");
        let b = dir.path().join("b.pdf");
        std::fs::write(&b, b"%PDF-1.4 a longer, different file\n").expect("write");
        let meta = |p: &Path| {
            std::fs::File::open(p)
                .and_then(|f| f.metadata())
                .expect("meta")
        };
        assert_eq!(same_file(&meta(&a), &meta(&a)), Ok(()));
        assert_eq!(same_file(&meta(&a), &meta(&b)), Err(FileError::Unstable));
        let d = std::fs::metadata(dir.path()).expect("dir");
        assert_eq!(same_file(&meta(&a), &d), Err(FileError::NotAFile));
    }
}
