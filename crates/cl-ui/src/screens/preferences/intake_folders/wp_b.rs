//! A01 WP-B stub. Stand-ins, with the exact signatures of A01 plan §2
//! ("Interface WP-A and WP-C code against"), for what WP-B builds in
//! `cl_company::intake` (`path.rs`, `probe.rs`). When WP-B merges, the
//! orchestrator deletes this file and points `use` lines at
//! `cl_company::intake::{check_path_syntax, canonical_key, probe_async,
//! PathProblem, ProbeReport, Health}` instead, mapping any variant or field
//! names that differ.
//!
//! The syntax check is real (a few string rules, no I/O), so the panel's
//! inline refusals work before WP-B lands. The probe is a minimal
//! background-thread version: reachable, is a folder, file counts, the
//! write test and the pinned key, under the 15 s deadline.
//!
//! Nothing here logs, and nothing names a path in `Debug`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// How long a probe may take before it reports "Not responding".
pub const PROBE_DEADLINE: Duration = Duration::from_secs(15);
/// Longest path the engine stores (characters).
pub const MAX_PATH_CHARS: usize = 1024;
/// Largest file taken in (Q-50), inclusive.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Most directory entries one probe reads.
pub const MAX_ENTRIES: usize = 10_000;

/// Why a typed folder path is refused (A01 D1/D2 path rules).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathProblem {
    /// Nothing typed.
    Empty,
    /// Over 1024 characters.
    TooLong,
    /// A control character or NUL.
    ControlChars,
    /// Not a drive (`C:\`), network (`\\server\share`) or Unix (`/`) path.
    NotAbsolute,
    /// A device path (`\\?\`, `\\.\`).
    Device,
    /// A web address (`https://…`).
    Url,
    /// A WebDAV form (`@SSL`, `@<port>`, `DavWWWRoot`).
    WebDav,
    /// A `..` segment.
    DotDot,
    /// Inside ClientLedger's own folders (app data, database, backups).
    AppFolder,
    /// A whole drive (`C:\`, `/`).
    DriveRoot,
}

/// A folder's health (A01 D6). The stub only produces the probe's subset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Health {
    /// Reachable and readable.
    Ok,
    /// Not confirmed on this computer.
    NotConfirmed,
    /// The folder is not there (drive unplugged, renamed, sync paused).
    Missing,
    /// The network folder can't be reached.
    Unreachable,
    /// No answer before the deadline.
    NotResponding,
    /// ClientLedger isn't allowed to read it.
    PermissionDenied,
    /// The path is a file, not a folder.
    NotAFolder,
    /// Handled files can't be moved.
    CantMove,
    /// The root now resolves somewhere else.
    Moved,
    /// Another company on this computer uses the folder.
    MappedByAnother,
    /// Scans are slow.
    Slow,
    /// More entries than one scan reads.
    TooMany,
}

/// What Test found.
#[derive(Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// [`Health::Ok`] when the folder can be used.
    pub health: Health,
    /// Files that would come in.
    pub files: usize,
    /// Files over the 10 MB limit (listed, never read).
    pub too_large: usize,
    /// HEIC/HEIF photos (listed, never read).
    pub heic: usize,
    /// Cloud files not on this computer yet.
    pub online_only: usize,
    /// Whether a test file could be made and removed (`None`: not checked,
    /// because the folder leaves originals where they are).
    pub writable: Option<bool>,
    /// The path looks like a cloud-sync folder.
    pub cloud: bool,
    /// The pinned root (`canonical_key`), when reachable.
    pub canonical_key: Option<String>,
}

impl std::fmt::Debug for ProbeReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeReport")
            .field("health", &self.health)
            .field("files", &self.files)
            .field("too_large", &self.too_large)
            .field("heic", &self.heic)
            .field("online_only", &self.online_only)
            .field("writable", &self.writable)
            .field("cloud", &self.cloud)
            .field(
                "canonical_key",
                &self.canonical_key.as_ref().map(|_| "<hash>"),
            )
            .finish()
    }
}

impl ProbeReport {
    fn failed(health: Health) -> ProbeReport {
        ProbeReport {
            health,
            files: 0,
            too_large: 0,
            heic: 0,
            online_only: 0,
            writable: None,
            cloud: false,
            canonical_key: None,
        }
    }
}

/// `path` with `/` as `\`, lower case, no trailing separator: how the
/// rules compare paths.
fn norm(path: &str) -> String {
    let mut s = path.replace('/', "\\").to_lowercase();
    while s.len() > 1 && s.ends_with('\\') {
        s.pop();
    }
    s
}

fn is_drive_form(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn is_unc_form(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 3 && b[0] == b'\\' && b[1] == b'\\' && !matches!(b[2], b'\\' | b'?' | b'.')
}

/// Checks a typed folder path against the engine's rules plus the app's
/// own: no I/O, safe on the UI thread.
///
/// # Errors
/// The first [`PathProblem`] found.
pub fn check_path_syntax(text: &str, app_dirs: &[&Path]) -> Result<PathBuf, PathProblem> {
    if text.is_empty() {
        return Err(PathProblem::Empty);
    }
    if text.chars().count() > MAX_PATH_CHARS {
        return Err(PathProblem::TooLong);
    }
    if text.chars().any(char::is_control) {
        return Err(PathProblem::ControlChars);
    }
    let lower = text.to_lowercase().replace('/', "\\");
    if lower.starts_with("\\\\?\\") || lower.starts_with("\\\\.\\") {
        return Err(PathProblem::Device);
    }
    if text.contains("://") {
        return Err(PathProblem::Url);
    }
    let host = lower
        .strip_prefix("\\\\")
        .and_then(|rest| rest.split('\\').next())
        .unwrap_or("");
    if lower.contains("davwwwroot") || lower.contains("@ssl") || host.contains('@') {
        return Err(PathProblem::WebDav);
    }
    if !(is_drive_form(text) || is_unc_form(text) || text.starts_with('/')) {
        return Err(PathProblem::NotAbsolute);
    }
    if text.split(['\\', '/']).any(|seg| seg == "..") {
        return Err(PathProblem::DotDot);
    }
    let n = norm(text);
    if n == "\\" || (n.len() == 2 && is_drive_form(&format!("{n}\\"))) {
        return Err(PathProblem::DriveRoot);
    }
    for dir in app_dirs {
        let Some(d) = dir.to_str().map(norm) else {
            continue;
        };
        if d.is_empty() {
            continue;
        }
        if n == d || n.starts_with(&format!("{d}\\")) {
            return Err(PathProblem::AppFolder);
        }
    }
    Ok(PathBuf::from(text))
}

fn classify(e: &std::io::Error) -> Health {
    match e.kind() {
        std::io::ErrorKind::NotFound => Health::Missing,
        std::io::ErrorKind::PermissionDenied => Health::PermissionDenied,
        _ => Health::Unreachable,
    }
}

/// The pinned root of `root`: a hash of where it really points. Blocking:
/// call it off the UI thread.
///
/// # Errors
/// The folder's [`Health`] when it can't be resolved.
pub fn canonical_key(root: &Path) -> Result<String, Health> {
    let real = std::fs::canonicalize(root).map_err(|e| classify(&e))?;
    let text = real.to_string_lossy();
    let text = if cfg!(windows) {
        text.to_lowercase()
    } else {
        text.into_owned()
    };
    Ok(cl_settings::path_hash(&format!("root:{text}")))
}

fn looks_cloud(root: &Path) -> bool {
    let s = root.to_string_lossy().to_lowercase();
    [
        "onedrive",
        "dropbox",
        "google drive",
        "googledrive",
        "icloud",
        "cloudstorage",
        "box sync",
    ]
    .iter()
    .any(|w| s.contains(w))
}

fn skipped_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    name.starts_with('.')
        || name.starts_with("~$")
        || lower == "thumbs.db"
        || lower == "desktop.ini"
        || [".part", ".crdownload", ".download", ".tmp"]
            .iter()
            .any(|ext| lower.ends_with(ext))
}

fn probe_now(root: &Path, check_write: bool) -> ProbeReport {
    let meta = match std::fs::metadata(root) {
        Ok(m) => m,
        Err(e) => return ProbeReport::failed(classify(&e)),
    };
    if !meta.is_dir() {
        return ProbeReport::failed(Health::NotAFolder);
    }
    let entries = match std::fs::read_dir(root) {
        Ok(r) => r,
        Err(e) => return ProbeReport::failed(classify(&e)),
    };
    let mut report = ProbeReport::failed(Health::Ok);
    for (n, entry) in entries.enumerate() {
        if n >= MAX_ENTRIES {
            report.health = Health::TooMany;
            break;
        }
        let Ok(entry) = entry else { continue };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if skipped_name(name) {
            continue;
        }
        let Ok(m) = entry.metadata() else { continue };
        if m.len() == 0 {
            continue;
        }
        report.files += 1;
        let lower = name.to_lowercase();
        if m.len() > MAX_FILE_BYTES {
            report.too_large += 1;
        } else if lower.ends_with(".heic") || lower.ends_with(".heif") {
            report.heic += 1;
        }
    }
    if check_write {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let probe = root.join(format!(".clientledger-write-test-{nanos:x}"));
        let ok = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .is_ok();
        report.writable = Some(ok && std::fs::remove_file(&probe).is_ok());
    }
    report.cloud = looks_cloud(root);
    report.canonical_key = canonical_key(root).ok();
    report
}

/// Tests `root` on its own thread; the answer arrives within
/// [`PROBE_DEADLINE`] (past it: [`Health::NotResponding`], and the stuck
/// thread is abandoned).
#[must_use]
pub fn probe_async(root: PathBuf, check_write: bool) -> Receiver<ProbeReport> {
    let (out_tx, out_rx) = mpsc::channel();
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("intake-probe".into())
        .spawn(move || {
            let _ = tx.send(probe_now(&root, check_write));
        });
    if spawned.is_err() {
        let _ = out_tx.send(ProbeReport::failed(Health::NotResponding));
        return out_rx;
    }
    let watchdog = std::thread::Builder::new()
        .name("intake-probe-deadline".into())
        .spawn(move || {
            let report = rx
                .recv_timeout(PROBE_DEADLINE)
                .unwrap_or_else(|_| ProbeReport::failed(Health::NotResponding));
            let _ = out_tx.send(report);
        });
    drop(watchdog);
    out_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(text: &str) -> Result<PathBuf, PathProblem> {
        check_path_syntax(
            text,
            &[Path::new(r"C:\Users\o\AppData\Roaming\ClientLedger")],
        )
    }

    #[test]
    fn path_rules_follow_the_engine() {
        for ok in [
            r"C:\Scans",
            "D:/Bills",
            r"\\fileserver\Scans\Smith",
            "/home/owner/Receipts",
            r"C:\Users\o\AppData\Roaming\ClientLedgerSister",
        ] {
            assert!(check(ok).is_ok(), "{ok}");
        }
        for (bad, why) in [
            ("", PathProblem::Empty),
            ("Receipts", PathProblem::NotAbsolute),
            (r"Scans\2026", PathProblem::NotAbsolute),
            (r"\Scans", PathProblem::NotAbsolute),
            (r"\\?\C:\x", PathProblem::Device),
            (r"\\.\PhysicalDrive0", PathProblem::Device),
            ("//?/C:/x", PathProblem::Device),
            ("https://x", PathProblem::Url),
            ("file:///C:/x", PathProblem::Url),
            (r"\\server@SSL\share", PathProblem::WebDav),
            (r"\\server@8080\share", PathProblem::WebDav),
            (r"\\server\DavWWWRoot\x", PathProblem::WebDav),
            (r"C:\Scans\..\Windows", PathProblem::DotDot),
            ("/home/../etc", PathProblem::DotDot),
            ("C:\\Scans\u{0}", PathProblem::ControlChars),
            ("C:\\Sc\nans", PathProblem::ControlChars),
            (r"C:\", PathProblem::DriveRoot),
            ("c:/", PathProblem::DriveRoot),
            ("/", PathProblem::DriveRoot),
            (
                r"C:\Users\o\AppData\Roaming\ClientLedger",
                PathProblem::AppFolder,
            ),
            (
                r"c:/users/o/appdata/roaming/clientledger/pg",
                PathProblem::AppFolder,
            ),
        ] {
            assert_eq!(check(bad), Err(why), "{bad:?}");
        }
        let long = format!("C:\\{}", "a".repeat(1022));
        assert_eq!(check(&long), Err(PathProblem::TooLong));
        assert!(check(&format!("C:\\{}", "a".repeat(1021))).is_ok());
    }

    #[test]
    fn a_refusal_is_the_same_with_either_slash() {
        for text in [
            r"C:\Scans\..\x",
            r"\\?\C:\x",
            r"C:\",
            r"C:\Users\o\AppData\Roaming\ClientLedger\x",
        ] {
            let flipped = text.replace('\\', "/");
            assert_eq!(check(text).err(), check(&flipped).err(), "{text}");
        }
    }

    #[test]
    fn probe_reports_files_and_a_missing_folder() {
        let dir = tempfile::tempdir().expect("tmp");
        std::fs::write(dir.path().join("bill.pdf"), b"%PDF-1.4").expect("w");
        std::fs::write(dir.path().join("photo.HEIC"), b"x").expect("w");
        std::fs::write(dir.path().join(".hidden.pdf"), b"x").expect("w");
        std::fs::write(dir.path().join("half.pdf.part"), b"x").expect("w");
        std::fs::write(dir.path().join("empty.pdf"), b"").expect("w");
        std::fs::create_dir(dir.path().join("Processed")).expect("d");
        let r = probe_async(dir.path().to_path_buf(), true)
            .recv_timeout(PROBE_DEADLINE + Duration::from_secs(5))
            .expect("report");
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.files, 2);
        assert_eq!(r.heic, 1);
        assert_eq!(r.writable, Some(true));
        assert!(r.canonical_key.is_some());
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("ls").count(),
            6,
            "the write test left nothing"
        );
        let dbg = format!("{r:?}");
        assert!(!dbg.contains(&*dir.path().to_string_lossy()), "{dbg}");
        let gone = probe_async(dir.path().join("nope"), false)
            .recv_timeout(PROBE_DEADLINE + Duration::from_secs(5))
            .expect("report");
        assert_eq!(gone.health, Health::Missing);
        let file = probe_async(dir.path().join("bill.pdf"), false)
            .recv_timeout(PROBE_DEADLINE + Duration::from_secs(5))
            .expect("report");
        assert_eq!(file.health, Health::NotAFolder);
        assert_eq!(
            canonical_key(dir.path()).ok(),
            r.canonical_key,
            "the key is stable"
        );
    }
}
