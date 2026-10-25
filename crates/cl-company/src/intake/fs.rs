//! The file system as the intake folders see it ([`FolderFs`]): the real
//! one ([`RealFs`]) and an in-memory double ([`FakeFs`]) that injects
//! errors and delays, so a dead or slow network share can be tested
//! without one (no test resolves a network host name).
//!
//! Every call here may block for as long as the operating system takes
//! (an SMB share that went away can hold a call for a minute): callers run
//! them on a folder or probe thread, never on the egui thread.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use super::handled::{HandledDir, MoveFailed};
use super::{FileError, ReadFile, sha256_hex};

/// One directory entry, looked at without following links.
#[derive(Clone, PartialEq, Eq)]
pub struct EntryInfo {
    /// Its name, or `None` when it is not valid UTF-8 (skipped).
    pub name: Option<String>,
    /// Its full path (owner data).
    pub path: PathBuf,
    /// A plain file: not a folder, a link, a junction or another reparse
    /// point that names something else.
    pub is_file: bool,
    /// Size in bytes.
    pub size: u64,
    /// Modified time, milliseconds since 1970 (0 when unknown).
    pub modified_ms: i64,
    /// Hidden or system (Windows attributes, macOS `UF_HIDDEN`).
    pub hidden: bool,
    /// A cloud placeholder whose content is not on this computer (Windows
    /// offline/recall attributes, macOS dataless).
    pub online_only: bool,
    /// Hard links to the file (1 where the system doesn't say).
    pub links: u64,
}

/// Size and flags only: names and paths are owner data.
impl fmt::Debug for EntryInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EntryInfo")
            .field("is_file", &self.is_file)
            .field("size", &self.size)
            .field("hidden", &self.hidden)
            .field("online_only", &self.online_only)
            .field("links", &self.links)
            .finish_non_exhaustive()
    }
}

/// What one look at a folder found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawListing {
    /// The entries, in the order the system gave them.
    pub entries: Vec<EntryInfo>,
    /// More entries than the cap: only the first were read.
    pub truncated: bool,
}

/// The file operations the intake folders use.
pub trait FolderFs: Send + Sync {
    /// Whether `root` is a folder, **following** links (the root may be a
    /// link into a cloud folder; it is pinned by [`FolderFs::canonicalize`]).
    ///
    /// # Errors
    /// The system's error (classified by `path::classify`).
    fn root_is_dir(&self, root: &Path) -> io::Result<bool>;

    /// The root with every link resolved.
    ///
    /// # Errors
    /// The system's error.
    fn canonicalize(&self, root: &Path) -> io::Result<PathBuf>;

    /// The folder's entries (not its sub-folders' contents), at most `cap`.
    /// A failure part-way is an error, never a short listing.
    ///
    /// # Errors
    /// The system's error.
    fn list(&self, root: &Path, cap: usize) -> io::Result<RawListing>;

    /// True when the file opens for reading while refusing writers (on
    /// Windows a file a scanner still holds open for writing fails with a
    /// sharing violation).
    fn openable(&self, path: &Path) -> bool;

    /// Reads a file for upload ([`super::read_for_upload`]).
    ///
    /// # Errors
    /// [`FileError`].
    fn read(&self, path: &Path) -> Result<ReadFile, FileError>;

    /// Checks that files can be created in `root`: creates a new hidden
    /// `.clientledger-write-test-…` file (`create_new`, so no existing file
    /// is ever opened) and removes it.
    ///
    /// # Errors
    /// The system's error.
    fn write_probe(&self, root: &Path) -> io::Result<()>;

    /// Moves a handled original ([`super::move_handled`]), telling a
    /// permission refusal apart.
    ///
    /// # Errors
    /// [`MoveFailed`].
    fn move_handled(
        &self,
        root: &Path,
        path: &Path,
        sha256: &str,
        to: HandledDir,
    ) -> Result<PathBuf, MoveFailed>;
}

/// A name no other file has (time, process and a counter).
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(
        "{nanos:x}-{:x}-{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

// ---------------------------------------------------------------------------
// The real file system.

/// Windows attributes of files whose content is not on this computer.
#[cfg(windows)]
const ONLINE_ONLY_ATTRIBUTES: u32 = 0x0000_1000 // OFFLINE
    | 0x0004_0000 // RECALL_ON_OPEN
    | 0x0040_0000; // RECALL_ON_DATA_ACCESS

/// Windows hidden and system attributes.
#[cfg(windows)]
const HIDDEN_ATTRIBUTES: u32 = 0x2 | 0x4;

/// macOS `UF_HIDDEN`.
#[cfg(target_os = "macos")]
const UF_HIDDEN: u32 = 0x0000_8000;

/// macOS `SF_DATALESS`: the content is in the cloud (iCloud Drive, File
/// Provider folders). `lstat` reads the flag without fetching the file.
#[cfg(target_os = "macos")]
const SF_DATALESS: u32 = 0x4000_0000;

pub(crate) fn millis(t: io::Result<SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(windows)]
pub(crate) fn online_only(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes() & ONLINE_ONLY_ATTRIBUTES != 0
}

#[cfg(target_os = "macos")]
pub(crate) fn online_only(meta: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags() & SF_DATALESS != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn online_only(_meta: &fs::Metadata) -> bool {
    // Linux cloud clients are FUSE folders: not detectable; the
    // size-stable rule applies.
    false
}

#[cfg(windows)]
pub(crate) fn hidden(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes() & HIDDEN_ATTRIBUTES != 0
}

#[cfg(target_os = "macos")]
pub(crate) fn hidden(meta: &fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt as _;
    meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(crate) fn hidden(_meta: &fs::Metadata) -> bool {
    false
}

/// Hard links to the file. Windows has no stable link count in `std`
/// (accepted risk, plan D2.4): 1.
#[cfg(unix)]
pub(crate) fn links(meta: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt as _;
    meta.nlink()
}

#[cfg(not(unix))]
pub(crate) fn links(_meta: &fs::Metadata) -> u64 {
    1
}

/// Opens `path` for reading. On Windows other processes may keep reading
/// it but not writing (`FILE_SHARE_READ`), so a file still held open for
/// writing fails with a sharing violation instead of being read half
/// written.
pub(crate) fn open_shared_read(path: &Path) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        /// `FILE_SHARE_READ`.
        const FILE_SHARE_READ: u32 = 0x1;
        o.share_mode(FILE_SHARE_READ);
    }
    o.open(path)
}

/// The real file system.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

impl FolderFs for RealFs {
    fn root_is_dir(&self, root: &Path) -> io::Result<bool> {
        fs::metadata(root).map(|m| m.is_dir())
    }

    fn canonicalize(&self, root: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(root)
    }

    fn list(&self, root: &Path, cap: usize) -> io::Result<RawListing> {
        let mut out = RawListing::default();
        for entry in fs::read_dir(root)? {
            // A network folder that drops mid-listing fails the whole look:
            // a short listing would look like files the owner moved away.
            let entry = entry?;
            if out.entries.len() >= cap {
                out.truncated = true;
                break;
            }
            // Never follows a link (on Windows from the listing itself).
            let Ok(meta) = entry.metadata() else {
                // Gone between the listing and the look.
                continue;
            };
            out.entries.push(EntryInfo {
                name: entry.file_name().to_str().map(str::to_string),
                path: entry.path(),
                is_file: meta.file_type().is_file(),
                size: meta.len(),
                modified_ms: millis(meta.modified()),
                hidden: hidden(&meta),
                online_only: online_only(&meta),
                links: links(&meta),
            });
        }
        Ok(out)
    }

    fn openable(&self, path: &Path) -> bool {
        open_shared_read(path).is_ok()
    }

    fn read(&self, path: &Path) -> Result<ReadFile, FileError> {
        super::read_for_upload(path)
    }

    fn write_probe(&self, root: &Path) -> io::Result<()> {
        for _ in 0..3 {
            let path = root.join(format!(".clientledger-write-test-{}", unique_suffix()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(f) => {
                    drop(f);
                    // Only the file made just now is removed.
                    return fs::remove_file(&path);
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }

    fn move_handled(
        &self,
        root: &Path,
        path: &Path,
        sha256: &str,
        to: HandledDir,
    ) -> Result<PathBuf, MoveFailed> {
        super::handled::move_into(root, path, sha256, &to.parts())
    }
}

// ---------------------------------------------------------------------------
// The test double.

#[derive(Clone)]
struct FakeFile {
    bytes: Vec<u8>,
    modified_ms: i64,
    hidden: bool,
    online_only: bool,
    is_file: bool,
    links: u64,
}

#[derive(Default)]
struct FakeDir {
    files: Vec<(String, FakeFile)>,
    /// Errors to return (raw OS codes), once each, before working again;
    /// `u32::MAX` repeats for ever.
    fail: Option<(i32, u32)>,
    delay: Duration,
    canonical: Option<PathBuf>,
    read_only: bool,
}

#[derive(Default)]
struct FakeInner {
    dirs: HashMap<PathBuf, FakeDir>,
    locked: HashSet<PathBuf>,
    lists: usize,
    handled: Vec<PathBuf>,
}

/// An in-memory [`FolderFs`] for tests: folders of files, with injected
/// OS errors (by raw code, as a share returns them), delays and locked
/// (still being written) files. Safe to share between threads.
#[derive(Default)]
pub struct FakeFs {
    inner: Mutex<FakeInner>,
}

impl fmt::Debug for FakeFs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeFs").finish_non_exhaustive()
    }
}

impl FakeFs {
    /// An empty file system.
    #[must_use]
    pub fn new() -> FakeFs {
        FakeFs::default()
    }

    fn lock(&self) -> MutexGuard<'_, FakeInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds an empty folder at `root`.
    pub fn add_dir(&self, root: &Path) {
        self.lock().dirs.entry(root.to_path_buf()).or_default();
    }

    /// Writes (or replaces) a plain file `name` in `root` modified at
    /// `modified_ms`.
    pub fn put(&self, root: &Path, name: &str, bytes: &[u8], modified_ms: i64) {
        self.put_with(root, name, bytes, modified_ms, |_| {});
    }

    fn put_with(
        &self,
        root: &Path,
        name: &str,
        bytes: &[u8],
        modified_ms: i64,
        f: impl FnOnce(&mut FakeFile),
    ) {
        let mut file = FakeFile {
            bytes: bytes.to_vec(),
            modified_ms,
            hidden: false,
            online_only: false,
            is_file: true,
            links: 1,
        };
        f(&mut file);
        let mut i = self.lock();
        let dir = i.dirs.entry(root.to_path_buf()).or_default();
        dir.files.retain(|(n, _)| n != name);
        dir.files.push((name.to_string(), file));
    }

    /// A hidden (or system) file.
    pub fn put_hidden(&self, root: &Path, name: &str, bytes: &[u8]) {
        self.put_with(root, name, bytes, 1, |f| f.hidden = true);
    }

    /// A cloud placeholder (content online only).
    pub fn put_online_only(&self, root: &Path, name: &str, size: usize) {
        self.put_with(root, name, &vec![0; size], 1, |f| f.online_only = true);
    }

    /// A link (or junction, or folder) named `name`: never a plain file.
    pub fn put_link(&self, root: &Path, name: &str) {
        self.put_with(root, name, b"%PDF-elsewhere", 1, |f| f.is_file = false);
    }

    /// A file with `links` hard links.
    pub fn put_hard_linked(&self, root: &Path, name: &str, bytes: &[u8], links: u64) {
        self.put_with(root, name, bytes, 1, |f| f.links = links);
    }

    /// Removes a file.
    pub fn remove(&self, root: &Path, name: &str) {
        if let Some(d) = self.lock().dirs.get_mut(root) {
            d.files.retain(|(n, _)| n != name);
        }
    }

    /// The names of `root`'s files.
    #[must_use]
    pub fn names(&self, root: &Path) -> Vec<String> {
        self.lock()
            .dirs
            .get(root)
            .map(|d| d.files.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default()
    }

    /// Every call on `root` fails with the raw OS error `code`, `times`
    /// times (`u32::MAX`: until [`FakeFs::heal`]).
    pub fn fail(&self, root: &Path, code: i32, times: u32) {
        self.lock().dirs.entry(root.to_path_buf()).or_default().fail = Some((code, times));
    }

    /// Calls on `root` work again.
    pub fn heal(&self, root: &Path) {
        if let Some(d) = self.lock().dirs.get_mut(root) {
            d.fail = None;
            d.delay = Duration::ZERO;
        }
    }

    /// Every call on `root` takes `delay` first (a slow or hung share).
    pub fn delay(&self, root: &Path, delay: Duration) {
        self.lock()
            .dirs
            .entry(root.to_path_buf())
            .or_default()
            .delay = delay;
    }

    /// What `root` resolves to (default: itself).
    pub fn set_canonical(&self, root: &Path, to: &Path) {
        self.lock()
            .dirs
            .entry(root.to_path_buf())
            .or_default()
            .canonical = Some(to.to_path_buf());
    }

    /// Nothing can be created in `root` (or, `false`, it can again).
    pub fn set_read_only(&self, root: &Path, read_only: bool) {
        self.lock()
            .dirs
            .entry(root.to_path_buf())
            .or_default()
            .read_only = read_only;
    }

    /// The file at `path` is held open for writing (or not, `false`).
    pub fn set_locked(&self, path: &Path, locked: bool) {
        let mut i = self.lock();
        if locked {
            i.locked.insert(path.to_path_buf());
        } else {
            i.locked.remove(path);
        }
    }

    /// Where moved files went, in order (they leave the folder).
    #[must_use]
    pub fn handled(&self) -> Vec<PathBuf> {
        self.lock().handled.clone()
    }

    /// How many listings were made.
    #[must_use]
    pub fn lists(&self) -> usize {
        self.lock().lists
    }

    /// Waits the folder's delay, then fails when an error is injected.
    fn gate(&self, root: &Path) -> io::Result<()> {
        let delay = self
            .lock()
            .dirs
            .get(root)
            .map_or(Duration::ZERO, |d| d.delay);
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        let mut i = self.lock();
        let Some(d) = i.dirs.get_mut(root) else {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        };
        if let Some((code, times)) = d.fail {
            if times != u32::MAX {
                d.fail = (times > 1).then(|| (code, times - 1));
            }
            return Err(io::Error::from_raw_os_error(code));
        }
        Ok(())
    }

    fn split(path: &Path) -> Option<(PathBuf, String)> {
        Some((
            path.parent()?.to_path_buf(),
            path.file_name()?.to_str()?.to_string(),
        ))
    }

    fn file(&self, path: &Path) -> Result<FakeFile, FileError> {
        let (root, name) = FakeFs::split(path).ok_or(FileError::NotAFile)?;
        self.gate(&root).map_err(|_| FileError::Io)?;
        self.lock()
            .dirs
            .get(&root)
            .and_then(|d| d.files.iter().find(|(n, _)| *n == name))
            .map(|(_, f)| f.clone())
            .ok_or(FileError::Gone)
    }
}

impl FolderFs for FakeFs {
    fn root_is_dir(&self, root: &Path) -> io::Result<bool> {
        self.gate(root)?;
        Ok(true)
    }

    fn canonicalize(&self, root: &Path) -> io::Result<PathBuf> {
        self.gate(root)?;
        Ok(self
            .lock()
            .dirs
            .get(root)
            .and_then(|d| d.canonical.clone())
            .unwrap_or_else(|| root.to_path_buf()))
    }

    fn list(&self, root: &Path, cap: usize) -> io::Result<RawListing> {
        self.gate(root)?;
        let mut i = self.lock();
        i.lists += 1;
        let d = i
            .dirs
            .get(root)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let mut out = RawListing::default();
        for (name, f) in &d.files {
            if out.entries.len() >= cap {
                out.truncated = true;
                break;
            }
            out.entries.push(EntryInfo {
                name: Some(name.clone()),
                path: root.join(name),
                is_file: f.is_file,
                size: f.bytes.len() as u64,
                modified_ms: f.modified_ms,
                hidden: f.hidden,
                online_only: f.online_only,
                links: f.links,
            });
        }
        Ok(out)
    }

    fn openable(&self, path: &Path) -> bool {
        !self.lock().locked.contains(path) && self.file(path).is_ok()
    }

    fn read(&self, path: &Path) -> Result<ReadFile, FileError> {
        let f = self.file(path)?;
        if !f.is_file || f.links > 1 {
            return Err(FileError::NotAFile);
        }
        if f.online_only {
            return Err(FileError::OnlineOnly);
        }
        if self.lock().locked.contains(path) {
            return Err(FileError::Unstable);
        }
        let size = f.bytes.len() as u64;
        if size > super::MAX_FILE_BYTES {
            return Err(FileError::TooLarge);
        }
        if size == 0 {
            return Err(FileError::Empty);
        }
        super::scan::check_bytes(&f.bytes)?;
        Ok(ReadFile {
            sha256: sha256_hex(&f.bytes),
            size,
            modified_ms: f.modified_ms,
            bytes: f.bytes,
        })
    }

    fn write_probe(&self, root: &Path) -> io::Result<()> {
        self.gate(root)?;
        if self.lock().dirs.get(root).is_some_and(|d| d.read_only) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(())
    }

    fn move_handled(
        &self,
        root: &Path,
        path: &Path,
        sha256: &str,
        to: HandledDir,
    ) -> Result<PathBuf, MoveFailed> {
        if path.parent() != Some(root) {
            return Err(FileError::NotAFile.into());
        }
        let f = self.file(path)?;
        if !f.is_file || f.links > 1 {
            return Err(FileError::NotAFile.into());
        }
        if sha256_hex(&f.bytes) != sha256 {
            return Err(FileError::Changed.into());
        }
        let mut i = self.lock();
        let Some(d) = i.dirs.get_mut(root) else {
            return Err(FileError::Gone.into());
        };
        if d.read_only {
            return Err(MoveFailed {
                error: FileError::Io,
                denied: true,
            });
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        d.files.retain(|(n, _)| *n != name);
        let target = to.dir_in(root).join(&name);
        i.handled.push(target.clone());
        Ok(target)
    }
}

/// SHA-256 of a file's bytes, read in 64 KiB pieces.
pub(crate) fn file_sha256(path: &Path) -> Result<String, FileError> {
    use sha2::{Digest as _, Sha256};
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
    Ok(super::hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_injects_errors_once_or_for_ever() {
        let fs = FakeFs::new();
        let root = Path::new("/share/bills");
        fs.put(root, "a.pdf", b"%PDF-1", 1);
        fs.fail(root, 53, 2);
        assert_eq!(
            fs.list(root, 10).err().and_then(|e| e.raw_os_error()),
            Some(53)
        );
        assert!(fs.list(root, 10).is_err());
        assert_eq!(fs.list(root, 10).expect("healed").entries.len(), 1);
        fs.fail(root, 5, u32::MAX);
        for _ in 0..5 {
            assert!(fs.root_is_dir(root).is_err());
        }
        fs.heal(root);
        assert!(fs.root_is_dir(root).expect("ok"));
        assert!(fs.list(Path::new("/nope"), 10).is_err());
    }

    #[test]
    fn fake_caps_and_reads() {
        let fs = FakeFs::new();
        let root = Path::new("/r");
        for n in 0..5 {
            fs.put(root, &format!("{n}.pdf"), b"%PDF-1", 1);
        }
        let l = fs.list(root, 3).expect("list");
        assert_eq!(l.entries.len(), 3);
        assert!(l.truncated);
        let p = root.join("0.pdf");
        assert_eq!(fs.read(&p).map(|r| r.size), Ok(6));
        fs.set_locked(&p, true);
        assert!(!fs.openable(&p));
        assert_eq!(fs.read(&p), Err(FileError::Unstable));
        fs.put_link(root, "l.pdf");
        assert_eq!(fs.read(&root.join("l.pdf")), Err(FileError::NotAFile));
        fs.put_hard_linked(root, "h.pdf", b"%PDF-1", 2);
        assert_eq!(fs.read(&root.join("h.pdf")), Err(FileError::NotAFile));
        fs.put_online_only(root, "o.pdf", 10);
        assert_eq!(fs.read(&root.join("o.pdf")), Err(FileError::OnlineOnly));
        assert_eq!(fs.read(&root.join("gone.pdf")), Err(FileError::Gone));
        let e = fs.list(root, 100).expect("list");
        assert!(!format!("{:?}", e.entries[0]).contains(".pdf"));
    }

    #[test]
    fn real_listing_does_not_follow_entries() {
        let dir = tempfile::tempdir().expect("tmp");
        fs::write(dir.path().join("a.pdf"), b"%PDF-1").expect("write");
        fs::create_dir(dir.path().join("sub")).expect("dir");
        let l = RealFs.list(dir.path(), 100).expect("list");
        assert_eq!(l.entries.len(), 2);
        let sub = l
            .entries
            .iter()
            .find(|e| e.name.as_deref() == Some("sub"))
            .expect("sub");
        assert!(!sub.is_file);
        let a = l
            .entries
            .iter()
            .find(|e| e.name.as_deref() == Some("a.pdf"))
            .expect("a");
        assert!(a.is_file && a.size == 6 && a.links == 1 && !a.hidden && !a.online_only);
        assert!(RealFs.root_is_dir(dir.path()).expect("dir"));
        assert!(!RealFs.root_is_dir(&dir.path().join("a.pdf")).expect("file"));
        let capped = RealFs.list(dir.path(), 1).expect("list");
        assert!(capped.truncated && capped.entries.len() == 1);
    }

    /// A file held open for writing by another program can't be opened
    /// share-read on Windows: it stays settling.
    #[cfg(windows)]
    #[test]
    fn a_file_open_for_writing_is_not_openable() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("scan.pdf");
        let held = fs::File::create(&p).expect("create");
        assert!(!RealFs.openable(&p), "a writer still holds it");
        drop(held);
        assert!(RealFs.openable(&p));
    }
}
