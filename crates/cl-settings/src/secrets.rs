//! Keychain-backed secrets (the backup passphrase in P08; AI keys and the
//! SMTP password in P09). A secret is addressed by a short lowercase name
//! and scoped to the app data folder, so two installs on one computer never
//! share an entry. Values are [`Secret`]s and are never written to the
//! settings files or logs (non-negotiable 11).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use cl_core::Secret;
use sha2::{Digest, Sha256};

/// Keychain service name shown in the OS credential manager.
const SERVICE: &str = "ClientLedger";

/// Errors of a secret store.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    /// The name is not a valid secret name.
    #[error("invalid secret name")]
    InvalidName,
    /// The keychain refused (message from the OS, never the value).
    #[error("the keychain could not be used: {0}")]
    Keychain(String),
}

/// Somewhere named secrets can be kept.
pub trait SecretStore: Send + Sync {
    /// The secret, or `None` when nothing was saved under `name`.
    ///
    /// # Errors
    /// `InvalidName`, `Keychain`.
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError>;

    /// Saves or replaces a secret.
    ///
    /// # Errors
    /// `InvalidName`, `Keychain`.
    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError>;

    /// Removes a secret (a missing one is not an error).
    ///
    /// # Errors
    /// `InvalidName`, `Keychain`.
    fn delete(&self, name: &str) -> Result<(), SecretError>;

    /// Short description for logs (`os-keychain`, `memory`).
    fn kind(&self) -> &'static str;

    /// True when a non-empty secret is saved under `name` (the value is
    /// read and dropped at once; callers that only need presence use this
    /// so the value never sits in their own state).
    ///
    /// # Errors
    /// `InvalidName`, `Keychain`.
    fn has(&self, name: &str) -> Result<bool, SecretError> {
        Ok(self.get(name)?.is_some_and(|s| !s.is_empty()))
    }
}

/// Secret names: lowercase letters, digits and dashes, 1-40 characters.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Stable short tag for a path's text: first 8 bytes of SHA-256 of the
/// lowercased text. Mirrors `cl_pg::keychain` (Q-K1); the crates may not
/// depend on each other, so the same test vectors guard both copies.
fn tag_of(text: &str) -> String {
    let digest = Sha256::digest(text.to_lowercase().as_bytes());
    digest[..8].iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// A path without the Windows verbatim prefix: `\\?\C:\x` → `C:\x`,
/// `\\?\UNC\server\share` → `\\server\share`.
fn strip_verbatim(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// The verbatim form Windows `canonicalize` returns for a normalized path.
fn verbatim(normalized: &str) -> String {
    if let Some(rest) = normalized.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{rest}")
    } else {
        format!(r"\\?\{normalized}")
    }
}

/// Canonicalize the deepest existing ancestor and re-append the rest.
fn resolve(path: &Path) -> PathBuf {
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        if let Ok(real) = cur.canonicalize() {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        let Some(name) = cur.file_name().map(std::ffi::OsStr::to_os_string) else {
            return path.to_path_buf();
        };
        rest.push(name);
        if !cur.pop() {
            return path.to_path_buf();
        }
    }
}

/// The tag of the app data folder, the same whether or not the folder exists
/// yet (Q-K1), plus the tags earlier versions may have used for it.
fn dir_tags(dir: &Path) -> (String, Vec<String>) {
    let absolute = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let normalized = strip_verbatim(&resolve(&absolute).to_string_lossy());
    let current = tag_of(&normalized);
    let mut legacy = Vec::new();
    let mut add = |text: &str| {
        let t = tag_of(text);
        if t != current && !legacy.contains(&t) {
            legacy.push(t);
        }
    };
    add(&dir
        .canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy());
    if cfg!(windows) {
        add(&verbatim(&normalized));
    }
    add(&absolute.to_string_lossy());
    add(&dir.to_string_lossy());
    (current, legacy)
}

/// The operating-system keychain.
#[derive(Debug, Clone)]
pub struct KeyringSecrets {
    tag: String,
    /// Tags earlier versions used for the same folder; read only when the
    /// current name has nothing (Q-K1).
    legacy_tags: Vec<String>,
}

impl KeyringSecrets {
    /// Entries scoped to the app data folder `root`.
    #[must_use]
    pub fn for_root(root: &Path) -> KeyringSecrets {
        let (tag, legacy_tags) = dir_tags(root);
        KeyringSecrets { tag, legacy_tags }
    }

    fn entry(name: &str, tag: &str) -> Result<keyring::Entry, SecretError> {
        if !valid_name(name) {
            return Err(SecretError::InvalidName);
        }
        keyring::Entry::new(SERVICE, &format!("{name}-{tag}"))
            .map_err(|e| SecretError::Keychain(e.to_string()))
    }

    fn get_tagged(name: &str, tag: &str) -> Result<Option<Secret>, SecretError> {
        match Self::entry(name, tag)?.get_password() {
            Ok(p) => Ok(Some(Secret::new(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Keychain(e.to_string())),
        }
    }

    fn set_tagged(name: &str, tag: &str, value: &Secret) -> Result<(), SecretError> {
        Self::entry(name, tag)?
            .set_password(value.expose())
            .map_err(|e| SecretError::Keychain(e.to_string()))
    }

    fn delete_tagged(name: &str, tag: &str) -> Result<(), SecretError> {
        match Self::entry(name, tag)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Keychain(e.to_string())),
        }
    }
}

/// Read `name` under `tag`; when nothing is there, try each legacy tag and
/// copy the first hit to `tag` (the old entry is kept until the next
/// [`set_and_retire`]; nothing is invented). A failed copy is logged and the
/// secret is still returned. A legacy name that cannot be read is logged and
/// skipped.
fn lookup_with_legacy(
    name: &str,
    tag: &str,
    legacy: &[String],
    get: impl Fn(&str, &str) -> Result<Option<Secret>, SecretError>,
    set: impl Fn(&str, &str, &Secret) -> Result<(), SecretError>,
) -> Result<Option<Secret>, SecretError> {
    if let Some(s) = get(name, tag)? {
        return Ok(Some(s));
    }
    for old in legacy {
        let s = match get(name, old) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(_) => {
                tracing::warn!(
                    from_tag = old.as_str(),
                    "could not read a secret under an earlier name; skipped it"
                );
                continue;
            }
        };
        if set(name, tag, &s).is_ok() {
            tracing::info!(
                from_tag = old.as_str(),
                to_tag = tag,
                "found a saved secret under an earlier name; copied it to the current name"
            );
        } else {
            tracing::warn!(
                from_tag = old.as_str(),
                to_tag = tag,
                "found a saved secret under an earlier name but could not copy it"
            );
        }
        return Ok(Some(s));
    }
    Ok(None)
}

/// Save `value` as `name` under `tag`, then remove it under every legacy tag:
/// once the current name holds the new value they only hold superseded ones.
/// A failed removal is logged; the save still counts.
fn set_and_retire(
    name: &str,
    tag: &str,
    legacy: &[String],
    value: &Secret,
    set: impl Fn(&str, &str, &Secret) -> Result<(), SecretError>,
    delete: impl Fn(&str, &str) -> Result<(), SecretError>,
) -> Result<(), SecretError> {
    set(name, tag, value)?;
    for old in legacy {
        if delete(name, old).is_err() {
            tracing::warn!(
                from_tag = old.as_str(),
                "could not remove a superseded secret under an earlier name"
            );
        }
    }
    Ok(())
}

/// Remove `name` under the legacy tags first and `tag` last; stop at the
/// first error.
fn delete_all(
    name: &str,
    tag: &str,
    legacy: &[String],
    delete: impl Fn(&str, &str) -> Result<(), SecretError>,
) -> Result<(), SecretError> {
    for old in legacy {
        delete(name, old)?;
    }
    delete(name, tag)
}

impl SecretStore for KeyringSecrets {
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        lookup_with_legacy(
            name,
            &self.tag,
            &self.legacy_tags,
            Self::get_tagged,
            Self::set_tagged,
        )
    }

    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
        set_and_retire(
            name,
            &self.tag,
            &self.legacy_tags,
            value,
            Self::set_tagged,
            Self::delete_tagged,
        )
    }

    /// Removes the secret under its current and every earlier name, so a
    /// removed key never comes back through the legacy lookup. Earlier names
    /// go first and the current name last.
    ///
    /// The earlier names are the ones this folder's path computes to (as
    /// given, absolute, canonical, and the Windows `\\?\` form). An entry
    /// saved through a different spelling of the same folder (another
    /// symlink, a mapped drive letter, an 8.3 short name) is not reached.
    fn delete(&self, name: &str) -> Result<(), SecretError> {
        delete_all(name, &self.tag, &self.legacy_tags, Self::delete_tagged)
    }

    fn kind(&self) -> &'static str {
        "os-keychain"
    }
}

/// In-memory store for tests and the demo (nothing persists).
#[derive(Default)]
pub struct MemorySecrets {
    map: Mutex<HashMap<String, Secret>>,
}

impl MemorySecrets {
    /// Empty.
    #[must_use]
    pub fn new() -> MemorySecrets {
        MemorySecrets::default()
    }
}

impl std::fmt::Debug for MemorySecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemorySecrets").finish_non_exhaustive()
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        if !valid_name(name) {
            return Err(SecretError::InvalidName);
        }
        Ok(self
            .map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
        if !valid_name(name) {
            return Err(SecretError::InvalidName);
        }
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_string(), value.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        if !valid_name(name) {
            return Err(SecretError::InvalidName);
        }
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "memory"
    }
}

/// Name of the backup passphrase entry (Q-P02i). Earlier passphrases are
/// kept under `backup-passphrase-<n>` when the owner changes it, so older
/// files stay readable.
pub const BACKUP_PASSPHRASE: &str = "backup-passphrase";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip_and_names() {
        let s = MemorySecrets::new();
        assert_eq!(s.get(BACKUP_PASSPHRASE).expect("get"), None);
        s.set(BACKUP_PASSPHRASE, &Secret::new("pw-canary"))
            .expect("set");
        assert_eq!(
            s.get(BACKUP_PASSPHRASE)
                .expect("get")
                .map(|v| v.expose().to_string()),
            Some("pw-canary".into())
        );
        assert!(!format!("{s:?}").contains("canary"));
        assert!(s.has(BACKUP_PASSPHRASE).expect("has"));
        s.delete(BACKUP_PASSPHRASE).expect("delete");
        assert_eq!(s.get(BACKUP_PASSPHRASE).expect("get"), None);
        assert!(!s.has(BACKUP_PASSPHRASE).expect("has"));
        assert_eq!(
            s.get("Bad Name").expect_err("name"),
            SecretError::InvalidName
        );
        assert_eq!(s.kind(), "memory");
    }

    /// A made-up Windows data folder (not anyone's real path).
    const SAMPLE_DIR: &str = r"C:\Users\alex.sample\AppData\Local\ClientLedger\pgdata";

    #[test]
    fn tags_match_the_cl_pg_vectors() {
        assert_eq!(tag_of(SAMPLE_DIR), "ff61f0171e214704");
        assert_eq!(
            tag_of(&strip_verbatim(&format!(r"\\?\{SAMPLE_DIR}"))),
            "ff61f0171e214704"
        );
        assert_eq!(tag_of(&verbatim(SAMPLE_DIR)), "1d2f0ad9f7eec027");
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
        assert_eq!(verbatim(r"\\srv\share"), r"\\?\UNC\srv\share");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-settings-sec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("scratch");
        d
    }

    #[test]
    fn the_tag_is_the_same_before_and_after_the_folder_exists() {
        let d = scratch("tag");
        let root = d.join("ClientLedger");
        // The name the old code wrote first-run secrets under: the folder did
        // not exist yet, so it hashed the path as given.
        let first_run = tag_of(&root.to_string_lossy());
        let before = KeyringSecrets::for_root(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let after = KeyringSecrets::for_root(&root);
        assert_eq!(before.tag, after.tag);
        assert!(
            first_run == after.tag || after.legacy_tags.contains(&first_run),
            "first-run tag lost"
        );
        let old = tag_of(&root.canonicalize().expect("canon").to_string_lossy());
        assert!(old == after.tag || after.legacy_tags.contains(&old));
        let _ = std::fs::remove_dir_all(d);
    }

    type Map = Mutex<HashMap<String, String>>;

    fn map_get(m: &Map) -> impl Fn(&str, &str) -> Result<Option<Secret>, SecretError> + Copy + '_ {
        move |n, t| {
            Ok(m.lock()
                .expect("lock")
                .get(&format!("{n}-{t}"))
                .map(|v| Secret::new(v.clone())))
        }
    }

    fn map_set(m: &Map) -> impl Fn(&str, &str, &Secret) -> Result<(), SecretError> + Copy + '_ {
        move |n, t, s| {
            m.lock()
                .expect("lock")
                .insert(format!("{n}-{t}"), s.expose().to_string());
            Ok(())
        }
    }

    fn map_delete(m: &Map) -> impl Fn(&str, &str) -> Result<(), SecretError> + Copy + '_ {
        move |n, t| {
            m.lock().expect("lock").remove(&format!("{n}-{t}"));
            Ok(())
        }
    }

    #[test]
    fn a_secret_under_a_legacy_tag_is_found_and_copied() {
        let m: Map = Mutex::default();
        let (get, set) = (map_get(&m), map_set(&m));
        let legacy = vec!["old1".to_string(), "old2".to_string()];
        assert_eq!(
            lookup_with_legacy("ai-key", "new", &legacy, get, set).expect("get"),
            None
        );
        assert!(m.lock().expect("lock").is_empty(), "nothing invented");
        m.lock()
            .expect("lock")
            .insert("ai-key-old2".into(), "k".into());
        assert_eq!(
            lookup_with_legacy("ai-key", "new", &legacy, get, set)
                .expect("get")
                .map(|s| s.expose().to_string()),
            Some("k".into())
        );
        let map = m.lock().expect("lock");
        assert_eq!(map.get("ai-key-new").map(String::as_str), Some("k"));
        assert_eq!(
            map.get("ai-key-old2").map(String::as_str),
            Some("k"),
            "kept"
        );
    }

    #[test]
    fn an_unreadable_legacy_tag_is_skipped() {
        let m: Map = Mutex::default();
        m.lock()
            .expect("lock")
            .insert("ai-key-old2".into(), "k".into());
        let legacy = vec!["old1".to_string(), "old2".to_string()];
        let get = |n: &str, t: &str| {
            if t == "old1" {
                Err(SecretError::Keychain("denied".into()))
            } else {
                map_get(&m)(n, t)
            }
        };
        let got = lookup_with_legacy("ai-key", "new", &legacy, get, map_set(&m)).expect("get");
        assert_eq!(got.map(|s| s.expose().to_string()), Some("k".into()));
        let failing = |_: &str, _: &str| Err(SecretError::Keychain("locked".into()));
        assert!(lookup_with_legacy("ai-key", "new", &legacy, failing, map_set(&m)).is_err());
    }

    #[test]
    fn a_set_retires_the_legacy_tags() {
        let m: Map = Mutex::default();
        let legacy = vec!["old1".to_string(), "old2".to_string()];
        m.lock()
            .expect("lock")
            .insert("ai-key-old2".into(), "k".into());
        m.lock()
            .expect("lock")
            .insert("smtp-other".into(), "x".into());
        lookup_with_legacy("ai-key", "new", &legacy, map_get(&m), map_set(&m)).expect("get");
        assert!(
            m.lock().expect("lock").contains_key("ai-key-old2"),
            "kept on read"
        );
        set_and_retire(
            "ai-key",
            "new",
            &legacy,
            &Secret::new("fresh"),
            map_set(&m),
            map_delete(&m),
        )
        .expect("set");
        let map = m.lock().expect("lock");
        assert_eq!(map.get("ai-key-new").map(String::as_str), Some("fresh"));
        assert!(!map.contains_key("ai-key-old2"));
        assert_eq!(map.len(), 2, "other names untouched");
    }

    #[test]
    fn a_failed_set_retires_nothing_and_a_failed_retire_still_saves() {
        let m: Map = Mutex::default();
        m.lock()
            .expect("lock")
            .insert("ai-key-old".into(), "k".into());
        let legacy = vec!["old".to_string()];
        let err = set_and_retire(
            "ai-key",
            "new",
            &legacy,
            &Secret::new("fresh"),
            |_, _, _| Err(SecretError::Keychain("locked".into())),
            map_delete(&m),
        );
        assert!(err.is_err());
        assert!(m.lock().expect("lock").contains_key("ai-key-old"));
        set_and_retire(
            "ai-key",
            "new",
            &legacy,
            &Secret::new("fresh"),
            map_set(&m),
            |_, _| Err(SecretError::Keychain("denied".into())),
        )
        .expect("set still counts");
        assert_eq!(
            m.lock()
                .expect("lock")
                .get("ai-key-new")
                .map(String::as_str),
            Some("fresh")
        );
    }

    #[test]
    fn delete_removes_legacy_tags_first_and_the_current_tag_last() {
        let order = Mutex::new(Vec::<String>::new());
        let legacy = vec!["old1".to_string(), "old2".to_string()];
        delete_all("ai-key", "new", &legacy, |_, t| {
            order.lock().expect("lock").push(t.to_string());
            Ok(())
        })
        .expect("delete");
        assert_eq!(*order.lock().expect("lock"), ["old1", "old2", "new"]);
        order.lock().expect("lock").clear();
        let err = delete_all("ai-key", "new", &legacy, |_, t| {
            order.lock().expect("lock").push(t.to_string());
            if t == "old1" {
                Err(SecretError::Keychain("denied".into()))
            } else {
                Ok(())
            }
        });
        assert!(err.is_err());
        assert_eq!(*order.lock().expect("lock"), ["old1"]);
    }

    #[test]
    fn keyring_entries_are_scoped_to_the_folder() {
        let a = KeyringSecrets::for_root(Path::new("/a/one"));
        let b = KeyringSecrets::for_root(Path::new("/a/two"));
        assert_ne!(a.tag, b.tag);
        assert_eq!(a.kind(), "os-keychain");
        assert_eq!(
            a.get("Bad Name").expect_err("name"),
            SecretError::InvalidName
        );
    }
}
