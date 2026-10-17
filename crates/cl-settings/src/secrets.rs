//! Keychain-backed secrets (the backup passphrase in P08; AI keys and the
//! SMTP password in P09). A secret is addressed by a short lowercase name
//! and scoped to the app data folder, so two installs on one computer never
//! share an entry. Values are [`Secret`]s and are never written to the
//! settings files or logs (non-negotiable 11).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
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
}

/// Secret names: lowercase letters, digits and dashes, 1-40 characters.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Stable short tag of the app data folder (first 8 bytes of SHA-256).
fn dir_tag(dir: &Path) -> String {
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let digest = Sha256::digest(canon.to_string_lossy().to_lowercase().as_bytes());
    digest[..8].iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// The operating-system keychain.
#[derive(Debug, Clone)]
pub struct KeyringSecrets {
    tag: String,
}

impl KeyringSecrets {
    /// Entries scoped to the app data folder `root`.
    #[must_use]
    pub fn for_root(root: &Path) -> KeyringSecrets {
        KeyringSecrets { tag: dir_tag(root) }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, SecretError> {
        if !valid_name(name) {
            return Err(SecretError::InvalidName);
        }
        keyring::Entry::new(SERVICE, &format!("{name}-{}", self.tag))
            .map_err(|e| SecretError::Keychain(e.to_string()))
    }
}

impl SecretStore for KeyringSecrets {
    fn get(&self, name: &str) -> Result<Option<Secret>, SecretError> {
        match self.entry(name)?.get_password() {
            Ok(p) => Ok(Some(Secret::new(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::Keychain(e.to_string())),
        }
    }

    fn set(&self, name: &str, value: &Secret) -> Result<(), SecretError> {
        self.entry(name)?
            .set_password(value.expose())
            .map_err(|e| SecretError::Keychain(e.to_string()))
    }

    fn delete(&self, name: &str) -> Result<(), SecretError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(SecretError::Keychain(e.to_string())),
        }
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
        s.delete(BACKUP_PASSPHRASE).expect("delete");
        assert_eq!(s.get(BACKUP_PASSPHRASE).expect("get"), None);
        assert_eq!(
            s.get("Bad Name").expect_err("name"),
            SecretError::InvalidName
        );
        assert_eq!(s.kind(), "memory");
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
