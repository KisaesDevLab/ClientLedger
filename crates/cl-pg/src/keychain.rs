//! Where the database superuser password lives.
//!
//! First choice is the operating system keychain (Windows Credential Manager,
//! macOS Keychain, Secret Service on Linux). When none is available (a
//! headless Linux box, a locked-down account) the password goes into an
//! encrypted file next to the data folder and the owner is told. The file is
//! never inside the data folder, so cluster backups do not contain it.

use crate::secret::hex;
use crate::{PgError, Result, Secret};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Keychain service name shown to the owner in their OS credential manager.
const SERVICE: &str = "ClientLedger";

/// Magic bytes and version at the start of an encrypted secret file.
const FILE_MAGIC: &[u8] = b"CLPGSEC1";

/// Somewhere a single password can be saved and read back.
pub trait PasswordStore: Send + Sync {
    /// The saved password, or `None` when nothing was saved yet.
    ///
    /// # Errors
    /// Fails when the store cannot be read.
    fn get(&self) -> Result<Option<Secret>>;

    /// Save (or replace) the password.
    ///
    /// # Errors
    /// Fails when the store cannot be written.
    fn set(&self, secret: &Secret) -> Result<()>;

    /// Short description for logs, such as `"os-keychain"`.
    fn kind(&self) -> &'static str;
}

/// OS keychain entry. The account name includes a hash of the data folder so
/// two data folders on one computer never share a password.
#[derive(Debug, Clone)]
pub struct KeyringStore {
    account: String,
}

impl KeyringStore {
    /// Entry for the cluster stored at `data_dir`.
    #[must_use]
    pub fn for_data_dir(data_dir: &Path) -> Self {
        KeyringStore {
            account: format!("postgres-superuser-{}", dir_tag(data_dir)),
        }
    }

    /// Entry for another secret that belongs to the cluster at `data_dir`,
    /// such as the engine's application-role password. `purpose` is a short
    /// lowercase name (letters, digits, dashes).
    ///
    /// # Panics
    /// Panics when `purpose` is not a valid name (a programming error).
    #[must_use]
    pub fn for_purpose(data_dir: &Path, purpose: &str) -> Self {
        assert!(valid_purpose(purpose), "invalid secret purpose");
        KeyringStore {
            account: format!("{purpose}-{}", dir_tag(data_dir)),
        }
    }

    fn entry(&self) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, &self.account).map_err(|e| PgError::SecretStore(e.to_string()))
    }

    /// Remove the saved password (used when a cluster is deleted).
    ///
    /// # Errors
    /// Fails when the keychain refuses; a missing entry is not an error.
    pub fn delete(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(PgError::SecretStore(e.to_string())),
        }
    }
}

impl PasswordStore for KeyringStore {
    fn get(&self) -> Result<Option<Secret>> {
        match self.entry()?.get_password() {
            Ok(p) => Ok(Some(Secret::new(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(PgError::SecretStore(e.to_string())),
        }
    }

    fn set(&self, secret: &Secret) -> Result<()> {
        self.entry()?
            .set_password(secret.expose())
            .map_err(|e| PgError::SecretStore(e.to_string()))
    }

    fn kind(&self) -> &'static str {
        "os-keychain"
    }
}

/// Stable short tag for a data directory path (first 8 bytes of SHA-256).
fn dir_tag(data_dir: &Path) -> String {
    let canon = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    let digest = Sha256::digest(canon.to_string_lossy().to_lowercase().as_bytes());
    hex(&digest[..8])
}

/// Encrypted file fallback.
///
/// The key is derived from this computer and user name (and the Linux machine
/// id when readable). That keeps the password unreadable if the file is copied
/// somewhere else, but it does not stop other programs running as the same user
/// on the same machine; the OS keychain is the real protection and is preferred
/// whenever it works (Q-P03c).
#[derive(Debug, Clone)]
pub struct EncryptedFileStore {
    path: PathBuf,
}

impl EncryptedFileStore {
    /// Store at `path`; it is created on the first `set`.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        EncryptedFileStore { path: path.into() }
    }

    /// Fallback file for another secret of the same cluster: a sibling named
    /// `<name>.<purpose>.secret`.
    ///
    /// # Panics
    /// Panics when `purpose` is not a valid name (a programming error).
    #[must_use]
    pub fn beside_for_purpose(data_dir: &Path, purpose: &str) -> Self {
        assert!(valid_purpose(purpose), "invalid secret purpose");
        let mut name = data_dir
            .file_name()
            .map_or_else(|| "pgdata".into(), std::ffi::OsStr::to_os_string);
        name.push(format!(".{purpose}.secret"));
        EncryptedFileStore::new(data_dir.with_file_name(name))
    }

    /// Default location: a sibling of the data directory, `<name>.secret`.
    #[must_use]
    pub fn beside(data_dir: &Path) -> Self {
        let mut name = data_dir
            .file_name()
            .map_or_else(|| "pgdata".into(), std::ffi::OsStr::to_os_string);
        name.push(".secret");
        EncryptedFileStore::new(data_dir.with_file_name(name))
    }

    /// Where the file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn cipher() -> ChaCha20Poly1305 {
        let mut h = Sha256::new();
        h.update(b"ClientLedger pg secret v1\0");
        for var in ["USERNAME", "USER", "COMPUTERNAME", "HOSTNAME"] {
            h.update(std::env::var(var).unwrap_or_default().as_bytes());
            h.update([0u8]);
        }
        if let Ok(id) = fs::read_to_string("/etc/machine-id") {
            h.update(id.trim().as_bytes());
        }
        let digest = h.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        ChaCha20Poly1305::new(&Key::from(key))
    }
}

impl PasswordStore for EncryptedFileStore {
    fn get(&self) -> Result<Option<Secret>> {
        let bytes = match fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(PgError::io("read saved database password", e)),
        };
        let header = FILE_MAGIC.len() + 12;
        if bytes.len() <= header || !bytes.starts_with(FILE_MAGIC) {
            return Err(PgError::SecretStore(
                "the saved database password file is damaged".into(),
            ));
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&bytes[FILE_MAGIC.len()..header]);
        let plain = Self::cipher()
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &bytes[header..],
                    aad: FILE_MAGIC,
                },
            )
            .map_err(|_| {
                PgError::SecretStore(
                    "the saved database password cannot be opened on this computer or account"
                        .into(),
                )
            })?;
        let text = String::from_utf8(plain)
            .map_err(|_| PgError::SecretStore("saved database password is not text".into()))?;
        Ok(Some(Secret::new(text)))
    }

    fn set(&self, secret: &Secret) -> Result<()> {
        let mut nonce = [0u8; 12];
        getrandom::fill(&mut nonce).map_err(|e| PgError::Random(e.to_string()))?;
        let sealed = Self::cipher()
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: secret.expose().as_bytes(),
                    aad: FILE_MAGIC,
                },
            )
            .map_err(|_| PgError::SecretStore("could not encrypt the database password".into()))?;
        let mut out = Vec::with_capacity(FILE_MAGIC.len() + 12 + sealed.len());
        out.extend_from_slice(FILE_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| PgError::io("create secret folder", e))?;
        }
        let tmp = self.path.with_extension("secret.tmp");
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| PgError::io("write saved database password", e))?;
        f.write_all(&out)
            .and_then(|()| f.sync_all())
            .map_err(|e| PgError::io("write saved database password", e))?;
        drop(f);
        fs::rename(&tmp, &self.path).map_err(|e| PgError::io("save database password", e))
    }

    fn kind(&self) -> &'static str {
        "encrypted-file"
    }
}

/// In-memory store for tests and tools; never persists anything.
#[derive(Debug, Default)]
pub struct MemoryStore {
    value: Mutex<Option<Secret>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl PasswordStore for MemoryStore {
    fn get(&self) -> Result<Option<Secret>> {
        Ok(self
            .value
            .lock()
            .map_err(|_| PgError::SecretStore("memory store poisoned".into()))?
            .clone())
    }

    fn set(&self, secret: &Secret) -> Result<()> {
        *self
            .value
            .lock()
            .map_err(|_| PgError::SecretStore("memory store poisoned".into()))? =
            Some(secret.clone());
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "memory"
    }
}

/// Something the owner should be told about where the password went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreNotice {
    /// The OS keychain did not work, so an encrypted file is used instead.
    KeychainUnavailable {
        /// Why the keychain failed (no secrets in it).
        reason: String,
        /// Where the fallback file is.
        file: PathBuf,
    },
}

impl std::fmt::Display for StoreNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreNotice::KeychainUnavailable { file, .. } => write!(
                f,
                "This computer's secure password storage is not available, so ClientLedger saved \
                 its database password in an encrypted file ({}). Keep that file with your \
                 company data folder; it is not part of your backups.",
                file.display()
            ),
        }
    }
}

/// The store chosen for a data folder plus an optional notice for the owner.
pub struct StoreChoice {
    /// Where to read and write the password.
    pub store: Box<dyn PasswordStore>,
    /// Present when the file fallback was chosen.
    pub notice: Option<StoreNotice>,
}

impl std::fmt::Debug for StoreChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreChoice")
            .field("store", &self.store.kind())
            .field("notice", &self.notice)
            .finish()
    }
}

/// Pick the OS keychain when it works (checked with a real write, read, and
/// delete of a probe entry), the encrypted file otherwise.
///
/// A cluster that already uses the file store keeps using it, so the choice is
/// stable between runs.
#[must_use]
pub fn default_store(data_dir: &Path) -> StoreChoice {
    choose_store(
        EncryptedFileStore::beside(data_dir),
        KeyringStore::for_data_dir(data_dir),
    )
}

/// [`default_store`] for another secret of the same cluster (Q-P04a). Same
/// rule: an existing fallback file wins, then the keychain if it works.
///
/// # Panics
/// Panics when `purpose` is not a valid name (a programming error).
#[must_use]
pub fn default_store_for(data_dir: &Path, purpose: &str) -> StoreChoice {
    choose_store(
        EncryptedFileStore::beside_for_purpose(data_dir, purpose),
        KeyringStore::for_purpose(data_dir, purpose),
    )
}

fn valid_purpose(purpose: &str) -> bool {
    !purpose.is_empty()
        && purpose.len() <= 40
        && purpose
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn choose_store(file: EncryptedFileStore, keyring: KeyringStore) -> StoreChoice {
    if file.path().exists() {
        return StoreChoice {
            store: Box::new(file),
            notice: None,
        };
    }
    match probe_keychain() {
        Ok(()) => StoreChoice {
            store: Box::new(keyring),
            notice: None,
        },
        Err(reason) => {
            tracing::warn!("OS keychain unavailable; using encrypted file store");
            let notice = StoreNotice::KeychainUnavailable {
                reason,
                file: file.path().to_path_buf(),
            };
            StoreChoice {
                store: Box::new(file),
                notice: Some(notice),
            }
        }
    }
}

fn probe_keychain() -> std::result::Result<(), String> {
    let account = format!("probe-{}", std::process::id());
    let entry = keyring::Entry::new(SERVICE, &account).map_err(|e| e.to_string())?;
    entry.set_password("probe").map_err(|e| e.to_string())?;
    let back = entry.get_password().map_err(|e| e.to_string())?;
    let _ = entry.delete_credential();
    if back == "probe" {
        Ok(())
    } else {
        Err("the keychain returned a different value than was saved".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-pg-key-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn file_store_round_trips_and_hides_the_value() {
        let d = scratch("rt");
        let store = EncryptedFileStore::new(d.join("pw.secret"));
        assert_eq!(store.get().unwrap(), None);
        let s = Secret::new("correct horse battery staple");
        store.set(&s).unwrap();
        assert_eq!(store.get().unwrap(), Some(s));
        let raw = fs::read(store.path()).unwrap();
        assert!(raw.starts_with(FILE_MAGIC));
        assert!(!String::from_utf8_lossy(&raw).contains("horse"));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn file_store_overwrites() {
        let d = scratch("ow");
        let store = EncryptedFileStore::new(d.join("pw.secret"));
        store.set(&Secret::new("one")).unwrap();
        store.set(&Secret::new("two")).unwrap();
        assert_eq!(store.get().unwrap(), Some(Secret::new("two")));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn tampering_is_detected() {
        let d = scratch("tamper");
        let store = EncryptedFileStore::new(d.join("pw.secret"));
        store.set(&Secret::new("secret")).unwrap();
        let mut raw = fs::read(store.path()).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        fs::write(store.path(), raw).unwrap();
        assert!(store.get().is_err());
        fs::write(store.path(), b"garbage").unwrap();
        assert!(store.get().is_err());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn sibling_path_is_next_to_the_data_dir() {
        let s = EncryptedFileStore::beside(Path::new("/x/y/pgdata"));
        assert_eq!(s.path(), Path::new("/x/y/pgdata.secret"));
    }

    #[test]
    fn memory_store_round_trips() {
        let m = MemoryStore::new();
        assert_eq!(m.get().unwrap(), None);
        m.set(&Secret::new("a")).unwrap();
        assert_eq!(m.get().unwrap(), Some(Secret::new("a")));
    }

    #[test]
    fn different_data_dirs_get_different_keychain_accounts() {
        let a = KeyringStore::for_data_dir(Path::new("/tmp/a-does-not-exist"));
        let b = KeyringStore::for_data_dir(Path::new("/tmp/b-does-not-exist"));
        assert_ne!(a.account, b.account);
    }

    #[test]
    fn purposes_get_their_own_accounts_and_files() {
        let data = Path::new("/tmp/c-does-not-exist/pgdata");
        let a = KeyringStore::for_purpose(data, "engine-app");
        assert_ne!(a.account, KeyringStore::for_data_dir(data).account);
        assert_ne!(
            a.account,
            KeyringStore::for_purpose(data, "other-thing").account
        );
        assert_eq!(
            EncryptedFileStore::beside_for_purpose(data, "engine-app").path(),
            Path::new("/tmp/c-does-not-exist/pgdata.engine-app.secret")
        );
    }

    #[test]
    #[should_panic(expected = "invalid secret purpose")]
    fn a_purpose_cannot_smuggle_path_characters() {
        let _ = EncryptedFileStore::beside_for_purpose(Path::new("/x/pgdata"), "../evil");
    }

    #[test]
    fn an_existing_fallback_file_keeps_being_used() {
        let d = scratch("sticky");
        let data = d.join("pgdata");
        EncryptedFileStore::beside(&data)
            .set(&Secret::new("x"))
            .unwrap();
        let choice = default_store(&data);
        assert_eq!(choice.store.kind(), "encrypted-file");
        assert!(choice.notice.is_none());
        let _ = fs::remove_dir_all(d);
    }
}
