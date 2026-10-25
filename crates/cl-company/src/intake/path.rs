//! Intake folder paths (A01 plan D1, D2): which paths may be used
//! ([`check_path_syntax`], no I/O), how a confirmed root is pinned
//! ([`canonical_key`]), and what the system's errors on a folder mean
//! ([`classify`]).

use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::fs::{FolderFs, RealFs};
use super::{Health, hex};

/// The longest path the engine stores (characters).
pub const MAX_PATH_CHARS: usize = 1024;

/// Why a typed folder path can't be used. Never carries the path itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum PathProblem {
    /// Nothing typed.
    #[error("no folder was entered")]
    Empty,
    /// Over [`MAX_PATH_CHARS`] characters.
    #[error("the folder path is too long")]
    TooLong,
    /// A control character or NUL.
    #[error("the folder path has a character that can't be used")]
    ControlCharacter,
    /// Not a full path for this system (a drive path or network share on
    /// Windows, a path from `/` elsewhere).
    #[error("the folder path is not a full path")]
    NotAbsolute,
    /// A device path (`\\?\`, `\\.\`).
    #[error("device paths can't be used")]
    Device,
    /// A web address (`…://…`).
    #[error("web addresses can't be used")]
    Url,
    /// A `WebDAV` form (`@SSL`, `@port`, `DavWWWRoot`).
    #[error("web folders can't be used")]
    WebDav,
    /// A `..` part.
    #[error("the folder path can't contain ..")]
    DotDot,
    /// Inside the app's own data, database or backup folders.
    #[error("the app's own folders can't be used")]
    AppFolder,
    /// The root of the system drive (`C:\`, `/`).
    #[error("the root of the system drive can't be used")]
    DriveRoot,
}

impl PathProblem {
    /// A short name for messages and the control channel.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            PathProblem::Empty => "empty",
            PathProblem::TooLong => "too_long",
            PathProblem::ControlCharacter => "control_character",
            PathProblem::NotAbsolute => "not_absolute",
            PathProblem::Device => "device",
            PathProblem::Url => "url",
            PathProblem::WebDav => "webdav",
            PathProblem::DotDot => "dot_dot",
            PathProblem::AppFolder => "app_folder",
            PathProblem::DriveRoot => "drive_root",
        }
    }
}

/// `text` with `/` as `\` (Windows forms are checked this way on every
/// system, so `//?/x` is refused like `\\?\x`).
fn backslashed(text: &str) -> String {
    text.replace('/', "\\")
}

/// A comparison key: separators as this system writes them, no trailing
/// separator, case folded on Windows.
fn compare_key(text: &str) -> String {
    if cfg!(windows) {
        backslashed(text).trim_end_matches('\\').to_lowercase()
    } else {
        text.trim_end_matches('/').to_string()
    }
}

fn is_drive_form(bs: &str) -> bool {
    let b = bs.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// `\\host\share…` with a host and a share.
fn is_unc_form(bs: &str) -> bool {
    let Some(rest) = bs.strip_prefix(r"\\") else {
        return false;
    };
    if rest.starts_with(['\\', '?', '.']) {
        return false;
    }
    let mut parts = rest.split('\\');
    let host = parts.next().unwrap_or("");
    let share = parts.next().unwrap_or("");
    !host.is_empty() && !share.is_empty()
}

/// The system drive's letter (`SystemDrive`, `C` when unset).
#[cfg(windows)]
fn system_drive() -> u8 {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|d| d.bytes().next())
        .filter(u8::is_ascii_alphabetic)
        .unwrap_or(b'C')
        .to_ascii_lowercase()
}

/// Checks a typed folder path without touching the disk (safe on the UI
/// thread): the engine's rules (D1) plus not inside `app_dirs` (the app
/// data, `PostgreSQL` data and backup folders) and not the system drive's
/// root. Surrounding spaces are trimmed; the trimmed text is returned as
/// the path to store.
///
/// Accepted: `C:\…` and `\\host\share\…` on Windows (either slash),
/// `/…` elsewhere.
///
/// # Errors
/// [`PathProblem`].
pub fn check_path_syntax(text: &str, app_dirs: &[&Path]) -> Result<PathBuf, PathProblem> {
    let t = text.trim();
    if t.is_empty() {
        return Err(PathProblem::Empty);
    }
    if t.chars().count() > MAX_PATH_CHARS {
        return Err(PathProblem::TooLong);
    }
    if t.chars().any(char::is_control) {
        return Err(PathProblem::ControlCharacter);
    }
    let bs = backslashed(t);
    // `scheme://` (either slash); `C://x` is a drive path with a doubled
    // slash, not a scheme.
    if bs.match_indices(r":\\").any(|(at, _)| at != 1) {
        return Err(PathProblem::Url);
    }
    if bs.starts_with(r"\\?\") || bs.starts_with(r"\\.\") {
        return Err(PathProblem::Device);
    }
    let lower = bs.to_lowercase();
    if lower.contains("davwwwroot") {
        return Err(PathProblem::WebDav);
    }
    if let Some(rest) = bs.strip_prefix(r"\\")
        && rest
            .split('\\')
            .next()
            .is_some_and(|host| host.contains('@'))
    {
        return Err(PathProblem::WebDav);
    }
    if bs.split('\\').any(|part| part == "..") {
        return Err(PathProblem::DotDot);
    }
    #[cfg(windows)]
    {
        if !is_drive_form(&bs) && !is_unc_form(&bs) {
            return Err(PathProblem::NotAbsolute);
        }
        let b = bs.trim_end_matches('\\').as_bytes();
        if b.len() == 2 && b[0].to_ascii_lowercase() == system_drive() {
            return Err(PathProblem::DriveRoot);
        }
    }
    #[cfg(not(windows))]
    {
        if !t.starts_with('/') || is_drive_form(&bs) || is_unc_form(&bs) {
            return Err(PathProblem::NotAbsolute);
        }
        if t.trim_end_matches('/').is_empty() {
            return Err(PathProblem::DriveRoot);
        }
    }
    let key = compare_key(t);
    let sep = if cfg!(windows) { '\\' } else { '/' };
    for dir in app_dirs {
        let Some(d) = dir.to_str().map(compare_key) else {
            continue;
        };
        if d.is_empty() {
            continue;
        }
        if key == d || key.starts_with(&format!("{d}{sep}")) {
            return Err(PathProblem::AppFolder);
        }
    }
    Ok(PathBuf::from(t))
}

/// The pin of a resolved root: a hash of its canonical path (verbatim
/// prefixes removed, case folded on Windows), so a re-mapped drive letter
/// or a swapped junction is noticed without keeping the path.
pub(crate) fn key_of(canonical: &Path) -> String {
    let text = canonical.to_string_lossy();
    let text = if cfg!(windows) {
        let t = if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = text.strip_prefix(r"\\?\") {
            rest.to_string()
        } else {
            text.into_owned()
        };
        t.to_lowercase()
    } else {
        text.into_owned()
    };
    hex(&Sha256::digest(
        format!("clientledger-intake-root:{text}").as_bytes(),
    ))
}

/// [`canonical_key`] through `fs`.
///
/// # Errors
/// The root's [`Health`].
pub fn canonical_key_with(fs: &dyn FolderFs, root: &Path) -> Result<String, Health> {
    fs.canonicalize(root)
        .map(|c| key_of(&c))
        .map_err(|e| classify(&e, true))
}

/// The pin of `root` (follows links): stored at confirm time and checked
/// by the watcher every minute. **Blocking**: call it on a background
/// thread (the probe does).
///
/// # Errors
/// The root's [`Health`] (`Missing`, `Unreachable`, …).
pub fn canonical_key(root: &Path) -> Result<String, Health> {
    canonical_key_with(&RealFs, root)
}

/// What a failed look at a folder (or, with `on_root == false`, at
/// something inside it) means for the owner (D2.6). Network failures are
/// told apart from a missing folder so "Can't reach the network folder"
/// isn't shown as "Can't find the folder".
#[must_use]
pub fn classify(e: &io::Error, on_root: bool) -> Health {
    #[cfg(windows)]
    if let Some(code) = e.raw_os_error() {
        match code {
            // ERROR_BAD_NETPATH, ERROR_BAD_NET_NAME, ERROR_NETWORK_UNREACHABLE,
            // ERROR_HOST_UNREACHABLE, ERROR_SEM_TIMEOUT, ERROR_NETNAME_DELETED.
            53 | 67 | 1231 | 1232 | 121 | 64 => return Health::Unreachable,
            // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND.
            2 | 3 if on_root => return Health::Missing,
            // ERROR_ACCESS_DENIED.
            5 => return Health::PermissionDenied,
            _ => {}
        }
    }
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        // EHOSTDOWN has no `ErrorKind` of its own.
        #[cfg(target_os = "linux")]
        const EHOSTDOWN: i32 = 112;
        #[cfg(not(target_os = "linux"))]
        const EHOSTDOWN: i32 = 64;
        if code == EHOSTDOWN {
            return Health::Unreachable;
        }
    }
    // `NotConnected`, `HostUnreachable`, `NetworkDown`, `TimedOut`,
    // `StaleNetworkFileHandle` and anything else unknown: the folder can't
    // be reached right now.
    match e.kind() {
        io::ErrorKind::NotFound if on_root => Health::Missing,
        io::ErrorKind::NotADirectory if on_root => Health::NotAFolder,
        io::ErrorKind::PermissionDenied => Health::PermissionDenied,
        _ => Health::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn check(text: &str) -> Result<PathBuf, PathProblem> {
        check_path_syntax(text, &[])
    }

    #[test]
    fn refusals_on_every_system() {
        for (text, problem) in [
            ("", PathProblem::Empty),
            ("   ", PathProblem::Empty),
            ("relative\\bills", PathProblem::NotAbsolute),
            ("bills", PathProblem::NotAbsolute),
            (r"\\?\C:\x", PathProblem::Device),
            ("//?/C:/x", PathProblem::Device),
            (r"\\.\PhysicalDrive0", PathProblem::Device),
            ("https://x.example/bills", PathProblem::Url),
            ("file:///c:/x", PathProblem::Url),
            (r"\\server@SSL\DavWWWRoot\bills", PathProblem::WebDav),
            (r"\\server@8080\bills", PathProblem::WebDav),
            (r"\\server\DavWWWRoot\bills", PathProblem::WebDav),
            ("/srv/davwwwroot/x", PathProblem::WebDav),
            ("/home/me/../etc", PathProblem::DotDot),
            (r"C:\Scans\..\Windows", PathProblem::DotDot),
            ("/home/me/a\u{0}b", PathProblem::ControlCharacter),
            ("C:\\a\tb", PathProblem::ControlCharacter),
        ] {
            assert_eq!(check(text), Err(problem), "{text:?}");
        }
        let long = format!("/{}", "a".repeat(MAX_PATH_CHARS));
        assert_eq!(check(&long), Err(PathProblem::TooLong));
        let names: std::collections::HashSet<_> = [
            PathProblem::Empty,
            PathProblem::TooLong,
            PathProblem::ControlCharacter,
            PathProblem::NotAbsolute,
            PathProblem::Device,
            PathProblem::Url,
            PathProblem::WebDav,
            PathProblem::DotDot,
            PathProblem::AppFolder,
            PathProblem::DriveRoot,
        ]
        .iter()
        .map(|p| p.as_str())
        .collect();
        assert_eq!(names.len(), 10);
    }

    #[cfg(windows)]
    #[test]
    fn windows_forms() {
        for ok in [
            r"C:\Scans\Bills",
            "C:/Scans/Bills",
            r"S:\",
            r"\\fileserver\Scans\Smith",
            "//fileserver/Scans",
            r"  D:\Receipts  ",
        ] {
            assert!(check(ok).is_ok(), "{ok}");
        }
        assert_eq!(
            check(r"  D:\Receipts ").expect("ok"),
            PathBuf::from(r"D:\Receipts")
        );
        for (text, problem) in [
            ("/home/me", PathProblem::NotAbsolute),
            (r"\Scans", PathProblem::NotAbsolute),
            ("C:Scans", PathProblem::NotAbsolute),
            (r"\\server", PathProblem::NotAbsolute),
            (r"\\server\", PathProblem::NotAbsolute),
            (r"C:\", PathProblem::DriveRoot),
            ("c:/", PathProblem::DriveRoot),
            ("C:", PathProblem::NotAbsolute),
        ] {
            assert_eq!(check(text), Err(problem), "{text}");
        }
        let app = Path::new(r"C:\Users\Owner\AppData\Roaming\ClientLedger");
        let pg = Path::new(r"D:\CL\pgdata");
        for inside in [
            r"C:\Users\Owner\AppData\Roaming\ClientLedger",
            r"c:\users\owner\appdata\roaming\clientledger\backups\",
            "C:/Users/Owner/AppData/Roaming/ClientLedger/x",
            r"D:\CL\pgdata\base",
        ] {
            assert_eq!(
                check_path_syntax(inside, &[app, pg]),
                Err(PathProblem::AppFolder),
                "{inside}"
            );
        }
        assert!(check_path_syntax(r"C:\Users\Owner\AppData\Roaming\ClientLedger2", &[app]).is_ok());
        assert!(
            check_path_syntax(r"D:\CL", &[app, pg]).is_ok(),
            "a parent is fine"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_forms() {
        assert!(check("/home/owner/Receipts").is_ok());
        assert!(check("/Volumes/Scans/Bills/").is_ok());
        for (text, problem) in [
            (r"C:\Scans", PathProblem::NotAbsolute),
            (r"\\server\share", PathProblem::NotAbsolute),
            ("/", PathProblem::DriveRoot),
            ("//", PathProblem::DriveRoot),
        ] {
            assert_eq!(check(text), Err(problem), "{text}");
        }
        let app = Path::new("/home/owner/.local/share/clientledger");
        assert_eq!(
            check_path_syntax("/home/owner/.local/share/clientledger/backups", &[app]),
            Err(PathProblem::AppFolder)
        );
        assert!(check_path_syntax("/home/owner/.local/share/clientledger-x", &[app]).is_ok());
    }

    #[test]
    fn problems_never_echo_the_path() {
        let e = check("https://secret-vendor.example").expect_err("url");
        let shown = format!("{e} {e:?}");
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn canonical_keys_pin_the_resolved_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let a = canonical_key(dir.path()).expect("key");
        assert_eq!(a.len(), 64);
        assert_eq!(canonical_key(dir.path()).expect("again"), a);
        let other = tempfile::tempdir().expect("tmp");
        assert_ne!(canonical_key(other.path()).expect("key"), a);
        assert_eq!(
            canonical_key(&dir.path().join("missing")),
            Err(Health::Missing)
        );
        // Through a file system double: the key follows what the root
        // resolves to.
        let fs = super::super::fs::FakeFs::new();
        let root = Path::new("/Users/o/Dropbox");
        fs.add_dir(root);
        let before = canonical_key_with(&fs, root).expect("key");
        fs.set_canonical(root, Path::new("/Users/o/Library/CloudStorage/Dropbox"));
        assert_ne!(canonical_key_with(&fs, root).expect("key"), before);
        fs.fail(root, if cfg!(windows) { 53 } else { 107 }, 1);
        assert_eq!(canonical_key_with(&fs, root), Err(Health::Unreachable));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_prefixes_do_not_change_the_key() {
        assert_eq!(
            key_of(Path::new(r"\\?\C:\Scans")),
            key_of(Path::new(r"c:\scans"))
        );
        assert_eq!(
            key_of(Path::new(r"\\?\UNC\server\share\x")),
            key_of(Path::new(r"\\SERVER\share\x"))
        );
    }

    proptest! {
        #[test]
        fn never_panics(s in ".{0,300}") {
            let _ = check(&s);
        }

        /// The refusals that are about the form, not the system (device,
        /// URL, WebDAV, `..`), don't change when `/` and `\` swap.
        #[test]
        fn refusal_is_stable_under_slash_swaps(s in "[a-zA-Z:/\\\\?.@ ]{0,24}") {
            let swapped: String = s
                .chars()
                .map(|c| match c {
                    '/' => '\\',
                    '\\' => '/',
                    c => c,
                })
                .collect();
            let form = |r: Result<PathBuf, PathProblem>| {
                matches!(
                    r,
                    Err(PathProblem::Device
                        | PathProblem::Url
                        | PathProblem::WebDav
                        | PathProblem::DotDot)
                )
            };
            if cfg!(windows) {
                prop_assert_eq!(check(&s).is_err(), check(&swapped).is_err());
            }
            prop_assert_eq!(form(check(&s)), form(check(&swapped)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_codes() {
        for (code, health) in [
            (53, Health::Unreachable),
            (67, Health::Unreachable),
            (1231, Health::Unreachable),
            (1232, Health::Unreachable),
            (121, Health::Unreachable),
            (64, Health::Unreachable),
            (3, Health::Missing),
            (2, Health::Missing),
            (5, Health::PermissionDenied),
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
        // Not found below the root is not a missing folder.
        assert_eq!(
            classify(&io::Error::from_raw_os_error(2), false),
            Health::Unreachable
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_codes() {
        for (code, health) in [
            (107, Health::Unreachable), // ENOTCONN
            (112, Health::Unreachable), // EHOSTDOWN
            (113, Health::Unreachable), // EHOSTUNREACH
            (110, Health::Unreachable), // ETIMEDOUT
            (116, Health::Unreachable), // ESTALE
            (2, Health::Missing),       // ENOENT
            (13, Health::PermissionDenied),
            (20, Health::NotAFolder), // ENOTDIR
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_codes() {
        for (code, health) in [
            (57, Health::Unreachable), // ENOTCONN
            (64, Health::Unreachable), // EHOSTDOWN
            (65, Health::Unreachable), // EHOSTUNREACH
            (60, Health::Unreachable), // ETIMEDOUT
            (70, Health::Unreachable), // ESTALE
            (2, Health::Missing),
            (13, Health::PermissionDenied),
        ] {
            let e = io::Error::from_raw_os_error(code);
            assert_eq!(classify(&e, true), health, "code {code}");
        }
    }

    #[test]
    fn kinds() {
        for (kind, health) in [
            (io::ErrorKind::NotFound, Health::Missing),
            (io::ErrorKind::PermissionDenied, Health::PermissionDenied),
            (io::ErrorKind::TimedOut, Health::Unreachable),
            (io::ErrorKind::NotConnected, Health::Unreachable),
            (io::ErrorKind::Other, Health::Unreachable),
        ] {
            assert_eq!(classify(&io::Error::from(kind), true), health, "{kind:?}");
        }
    }
}
