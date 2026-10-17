//! The control file: where the app publishes the loopback port and the random
//! token for one run, readable by the current user only.
//!
//! Safety rules (docs/ARCHITECTURE.md, Q-16): the file is created fresh with
//! owner-only permissions; a symlink, a file owned by someone else, a file
//! other users can read or write, or a file with extra hard links is refused,
//! both when the app writes it and when a client reads it.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Largest control file a client will read.
const MAX_BYTES: u64 = 4096;
/// Token length in random bytes (hex-encoded in the file).
const TOKEN_BYTES: usize = 32;

/// What the control file holds.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlInfo {
    /// File format version.
    pub version: u32,
    /// Loopback port the server listens on.
    pub port: u16,
    /// Random secret every request must carry.
    pub token: String,
    /// Process id of the app (informational).
    pub pid: u32,
}

impl std::fmt::Debug for ControlInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The token is a credential: never print it.
        f.debug_struct("ControlInfo")
            .field("version", &self.version)
            .field("port", &self.port)
            .field("token", &"<redacted>")
            .field("pid", &self.pid)
            .finish()
    }
}

/// Why a control file was refused or could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ControlFileError {
    /// The file or its folder failed a safety rule.
    #[error("the control file is not safe to use: {0}")]
    Unsafe(String),
    /// The file could not be read or written.
    #[error("the control file could not be accessed: {0}")]
    Io(#[from] std::io::Error),
    /// The file is not a control file.
    #[error("the control file is not valid")]
    Malformed,
    /// The operating system gave no random numbers.
    #[error("no secure random numbers are available")]
    Random,
}

/// What a safety check needs to know about a file.
#[derive(Debug, Clone, Copy)]
pub struct FileFacts {
    /// A symlink or reparse point.
    pub is_link: bool,
    /// A regular file.
    pub is_regular: bool,
    /// Owned by the current user.
    pub owned_by_me: bool,
    /// Readable or writable by group or others (Unix mode bits).
    pub others_have_access: bool,
    /// Number of hard links.
    pub links: u64,
}

/// The safety rules as a pure function (unit-tested on every platform).
///
/// # Errors
/// [`ControlFileError::Unsafe`] naming the first rule broken.
pub fn evaluate(facts: FileFacts) -> Result<(), ControlFileError> {
    let bad = |why: &str| Err(ControlFileError::Unsafe(why.to_string()));
    if facts.is_link {
        return bad("it is a symbolic link");
    }
    if !facts.is_regular {
        return bad("it is not a regular file");
    }
    if !facts.owned_by_me {
        return bad("it is owned by another user");
    }
    if facts.others_have_access {
        return bad("other users can read or change it");
    }
    if facts.links > 1 {
        return bad("it has more than one hard link");
    }
    Ok(())
}

#[cfg(unix)]
fn facts_of(link_meta: &Metadata, meta: &Metadata) -> FileFacts {
    use std::os::unix::fs::MetadataExt;
    FileFacts {
        is_link: link_meta.file_type().is_symlink(),
        is_regular: meta.is_file(),
        owned_by_me: meta.uid() == rustix::process::geteuid().as_raw(),
        others_have_access: meta.mode() & 0o077 != 0,
        links: meta.nlink(),
    }
}

#[cfg(windows)]
fn facts_of(link_meta: &Metadata, meta: &Metadata) -> FileFacts {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    // Owner and ACL cannot be read without unsafe code; the folder rule in
    // `check_parent` stands in (the file must live under %LOCALAPPDATA%,
    // which is per user, never roaming and not synced by OneDrive).
    FileFacts {
        is_link: link_meta.file_type().is_symlink()
            || link_meta.file_attributes() & REPARSE_POINT != 0,
        is_regular: meta.is_file(),
        owned_by_me: true,
        others_have_access: false,
        links: 1,
    }
}

#[cfg(not(any(unix, windows)))]
fn facts_of(link_meta: &Metadata, meta: &Metadata) -> FileFacts {
    FileFacts {
        is_link: link_meta.file_type().is_symlink(),
        is_regular: meta.is_file(),
        owned_by_me: false,
        others_have_access: true,
        links: 1,
    }
}

/// The folder rule. Unix: nobody else may be able to replace the file
/// (no group or world write unless sticky). Windows: under the local (non-roaming) app data folder.
fn check_parent(path: &Path) -> Result<(), ControlFileError> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::env::current_dir()?,
    };
    // A symlinked folder (for example /tmp on macOS) is resolved first.
    let real = fs::canonicalize(&parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = fs::metadata(&real)?.mode();
        if mode & 0o022 != 0 && mode & 0o1000 == 0 {
            return Err(ControlFileError::Unsafe(
                "its folder can be changed by other users".into(),
            ));
        }
    }
    #[cfg(windows)]
    {
        let profile = std::env::var_os("LOCALAPPDATA")
            .map(std::path::PathBuf::from)
            .and_then(|p| fs::canonicalize(p).ok())
            .ok_or_else(|| {
                ControlFileError::Unsafe("the local app data folder is unknown".into())
            })?;
        if !real.starts_with(&profile) {
            return Err(ControlFileError::Unsafe(
                "it must be inside your local app data folder (never a roaming or synced folder)"
                    .into(),
            ));
        }
    }
    let _ = real;
    Ok(())
}

/// Check an existing file without trusting it.
fn check_existing(path: &Path) -> Result<(), ControlFileError> {
    let link_meta = fs::symlink_metadata(path)?;
    let meta = fs::metadata(path).unwrap_or_else(|_| link_meta.clone());
    evaluate(facts_of(&link_meta, &meta))
}

/// A fresh random token (hex).
///
/// # Errors
/// [`ControlFileError::Random`] if the OS has no random source.
pub fn generate_token() -> Result<String, ControlFileError> {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|_| ControlFileError::Random)?;
    let mut out = String::with_capacity(TOKEN_BYTES * 2);
    for b in bytes {
        out.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    Ok(out)
}

/// Compare two tokens without stopping at the first difference.
#[must_use]
pub fn tokens_equal(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    diff == 0
}

/// Write the control file. A leftover file from an earlier run is replaced
/// only if it passes every safety rule; anything else is refused untouched.
///
/// # Errors
/// [`ControlFileError::Unsafe`] for a refused path, `Io` for disk errors.
pub fn write_new(path: &Path, info: &ControlInfo) -> Result<(), ControlFileError> {
    check_parent(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            check_existing(path)?;
            fs::remove_file(path)?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let mut opts = OpenOptions::new();
    // `create_new` never follows a symlink and never reuses a file.
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path)?;
    let text = serde_json::to_string(info).map_err(|_| ControlFileError::Malformed)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    // Re-check what we created (also catches a race that swapped it).
    check_existing(path)
}

/// Read and verify a control file (the client side).
///
/// # Errors
/// [`ControlFileError::Unsafe`] when a rule is broken, `Malformed` for bad
/// content.
pub fn read_checked(path: &Path) -> Result<ControlInfo, ControlFileError> {
    check_parent(path)?;
    check_existing(path)?;
    let mut file = File::open(path)?;
    // The opened file must be the one that was checked.
    let link_meta = fs::symlink_metadata(path)?;
    let meta = file.metadata()?;
    evaluate(facts_of(&link_meta, &meta))?;
    if meta.len() > MAX_BYTES {
        return Err(ControlFileError::Malformed);
    }
    let mut text = String::new();
    (&mut file).take(MAX_BYTES).read_to_string(&mut text)?;
    let info: ControlInfo = serde_json::from_str(&text).map_err(|_| ControlFileError::Malformed)?;
    if info.version != 1 || info.port == 0 || info.token.len() < 32 {
        return Err(ControlFileError::Malformed);
    }
    Ok(info)
}

/// Remove the control file if it still holds `token` (ours).
pub fn remove_if_ours(path: &Path, token: &str) {
    if let Ok(info) = read_checked(path)
        && tokens_equal(&info.token, token)
    {
        let _ = fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_facts() -> FileFacts {
        FileFacts {
            is_link: false,
            is_regular: true,
            owned_by_me: true,
            others_have_access: false,
            links: 1,
        }
    }

    fn info() -> ControlInfo {
        ControlInfo {
            version: 1,
            port: 4000,
            token: generate_token().unwrap(),
            pid: 1,
        }
    }

    #[test]
    fn each_rule_is_enforced() {
        assert!(evaluate(ok_facts()).is_ok());
        for f in [
            FileFacts {
                is_link: true,
                ..ok_facts()
            },
            FileFacts {
                is_regular: false,
                ..ok_facts()
            },
            FileFacts {
                owned_by_me: false,
                ..ok_facts()
            },
            FileFacts {
                others_have_access: true,
                ..ok_facts()
            },
            FileFacts {
                links: 2,
                ..ok_facts()
            },
        ] {
            assert!(
                matches!(evaluate(f), Err(ControlFileError::Unsafe(_))),
                "{f:?}"
            );
        }
    }

    #[test]
    fn tokens_are_random_hex_and_compare_exactly() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(tokens_equal(&a, &a));
        assert!(!tokens_equal(&a, &b));
        assert!(!tokens_equal(&a, &a[..63]));
    }

    #[test]
    fn debug_never_prints_the_token() {
        let i = info();
        assert!(!format!("{i:?}").contains(&i.token));
    }

    #[test]
    fn write_then_read_round_trips_and_replaces_a_stale_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        let first = info();
        write_new(&path, &first).unwrap();
        assert_eq!(read_checked(&path).unwrap(), first);
        let second = info();
        write_new(&path, &second).unwrap();
        assert_eq!(read_checked(&path).unwrap(), second);
        remove_if_ours(&path, &first.token);
        assert!(path.exists(), "a different token must not remove the file");
        remove_if_ours(&path, &second.token);
        assert!(!path.exists());
    }

    #[test]
    fn garbage_and_oversized_files_are_malformed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        write_new(&path, &info()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert!(matches!(
            read_checked(&path),
            Err(ControlFileError::Malformed)
        ));
        fs::write(&path, vec![b' '; 5000]).unwrap();
        assert!(matches!(
            read_checked(&path),
            Err(ControlFileError::Malformed)
        ));
        // Wrong version or a short token.
        fs::write(&path, r#"{"version":2,"port":1,"token":"x","pid":1}"#).unwrap();
        assert!(matches!(
            read_checked(&path),
            Err(ControlFileError::Malformed)
        ));
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::{PermissionsExt, symlink};

        #[test]
        fn file_is_created_owner_only() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("c.json");
            write_new(&path, &info()).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        #[test]
        fn world_readable_file_is_refused_by_client_and_writer() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("c.json");
            write_new(&path, &info()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(
                read_checked(&path),
                Err(ControlFileError::Unsafe(_))
            ));
            assert!(matches!(
                write_new(&path, &info()),
                Err(ControlFileError::Unsafe(_))
            ));
            assert!(path.exists(), "an unsafe file is left alone");
        }

        #[test]
        fn symlink_is_refused_and_its_target_untouched() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("victim");
            fs::write(&target, "keep").unwrap();
            let link = dir.path().join("c.json");
            symlink(&target, &link).unwrap();
            assert!(matches!(
                write_new(&link, &info()),
                Err(ControlFileError::Unsafe(_))
            ));
            assert!(matches!(
                read_checked(&link),
                Err(ControlFileError::Unsafe(_))
            ));
            assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        }

        #[test]
        fn hard_linked_file_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("c.json");
            write_new(&path, &info()).unwrap();
            fs::hard_link(&path, dir.path().join("other")).unwrap();
            assert!(matches!(
                read_checked(&path),
                Err(ControlFileError::Unsafe(_))
            ));
        }

        #[test]
        fn group_writable_folder_is_refused() {
            let dir = tempfile::tempdir().unwrap();
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o777)).unwrap();
            let r = write_new(&dir.path().join("c.json"), &info());
            assert!(matches!(r, Err(ControlFileError::Unsafe(_))));
        }
    }

    #[cfg(windows)]
    #[test]
    fn folder_outside_local_app_data_is_refused() {
        // The system folder is outside local app data.
        let r = write_new(Path::new("C:\\Windows\\Temp\\cl-control.json"), &info());
        assert!(matches!(r, Err(ControlFileError::Unsafe(_))), "{r:?}");
    }
}
