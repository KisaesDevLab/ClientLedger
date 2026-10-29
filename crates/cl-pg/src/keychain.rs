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
    /// Names the same secret may have been saved under by earlier versions
    /// (Q-K1); read only when `account` has nothing.
    legacy_accounts: Vec<String>,
}

impl KeyringStore {
    /// Entry for the cluster stored at `data_dir`.
    #[must_use]
    pub fn for_data_dir(data_dir: &Path) -> Self {
        Self::named(data_dir, "postgres-superuser")
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
        Self::named(data_dir, purpose)
    }

    fn named(data_dir: &Path, prefix: &str) -> Self {
        let tags = DirTags::of(data_dir);
        KeyringStore {
            account: format!("{prefix}-{}", tags.current),
            legacy_accounts: tags
                .legacy
                .iter()
                .map(|t| format!("{prefix}-{t}"))
                .collect(),
        }
    }

    fn entry(account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(SERVICE, account).map_err(|e| PgError::SecretStore(e.to_string()))
    }

    fn get_account(account: &str) -> Result<Option<Secret>> {
        match Self::entry(account)?.get_password() {
            Ok(p) => Ok(Some(Secret::new(p))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(PgError::SecretStore(e.to_string())),
        }
    }

    fn set_account(account: &str, secret: &Secret) -> Result<()> {
        Self::entry(account)?
            .set_password(secret.expose())
            .map_err(|e| PgError::SecretStore(e.to_string()))
    }

    fn delete_account(account: &str) -> Result<()> {
        match Self::entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(PgError::SecretStore(e.to_string())),
        }
    }

    /// Remove the saved password (used when a cluster is deleted), under its
    /// current name and any earlier name, so a deleted password never comes
    /// back through the legacy lookup. Earlier names go first and the current
    /// name last, so a partial failure never leaves only a legacy copy.
    ///
    /// The earlier names are the ones this folder's path computes to (as
    /// given, absolute, canonical, and the Windows `\\?\` form). An entry
    /// saved through a different spelling of the same folder (another
    /// symlink, a mapped drive letter, an 8.3 short name) is not reached.
    ///
    /// # Errors
    /// Fails when the keychain refuses; a missing entry is not an error.
    pub fn delete(&self) -> Result<()> {
        delete_all(&self.account, &self.legacy_accounts, Self::delete_account)
    }
}

impl PasswordStore for KeyringStore {
    fn get(&self) -> Result<Option<Secret>> {
        lookup_with_legacy(
            &self.account,
            &self.legacy_accounts,
            Self::get_account,
            Self::set_account,
        )
    }

    fn set(&self, secret: &Secret) -> Result<()> {
        set_and_retire(
            &self.account,
            &self.legacy_accounts,
            secret,
            Self::set_account,
            Self::delete_account,
        )
    }

    fn kind(&self) -> &'static str {
        "os-keychain"
    }
}

/// Read `account`; when it has nothing, try each of `legacy` in turn and copy
/// the first hit to `account` (Q-K1). The legacy entry is kept (until the
/// next [`set_and_retire`]) and no new secret is ever made here. A failed
/// copy is logged and the secret is still returned, so a read that found the
/// secret never fails because of it. A legacy name that cannot be read is
/// logged and skipped.
fn lookup_with_legacy(
    account: &str,
    legacy: &[String],
    get: impl Fn(&str) -> Result<Option<Secret>>,
    set: impl Fn(&str, &Secret) -> Result<()>,
) -> Result<Option<Secret>> {
    if let Some(s) = get(account)? {
        return Ok(Some(s));
    }
    for old in legacy {
        let (from, to) = (tag_part(old), tag_part(account));
        let s = match get(old) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(_) => {
                tracing::warn!(
                    from_tag = from,
                    "could not read a secret under an earlier name; skipped it"
                );
                continue;
            }
        };
        if set(account, &s).is_ok() {
            tracing::info!(
                from_tag = from,
                to_tag = to,
                "found a saved secret under an earlier name; copied it to the current name"
            );
        } else {
            tracing::warn!(
                from_tag = from,
                to_tag = to,
                "found a saved secret under an earlier name but could not copy it"
            );
        }
        return Ok(Some(s));
    }
    Ok(None)
}

/// Save `secret` under `account`, then remove every `legacy` entry: once the
/// current name holds the new value they only hold superseded ones. A failed
/// removal is logged; the save still counts.
fn set_and_retire(
    account: &str,
    legacy: &[String],
    secret: &Secret,
    set: impl Fn(&str, &Secret) -> Result<()>,
    delete: impl Fn(&str) -> Result<()>,
) -> Result<()> {
    set(account, secret)?;
    for old in legacy {
        if delete(old).is_err() {
            tracing::warn!(
                from_tag = tag_part(old),
                "could not remove a superseded secret under an earlier name"
            );
        }
    }
    Ok(())
}

/// Remove `legacy` entries first and `account` last; stop at the first error.
fn delete_all(account: &str, legacy: &[String], delete: impl Fn(&str) -> Result<()>) -> Result<()> {
    for old in legacy {
        delete(old)?;
    }
    delete(account)
}

/// The trailing folder tag of an account name (for logs).
fn tag_part(account: &str) -> &str {
    account.rsplit('-').next().unwrap_or(account)
}

/// The tag a data folder's secrets are saved under now, plus the tags earlier
/// versions may have used for the same folder.
struct DirTags {
    current: String,
    legacy: Vec<String>,
}

impl DirTags {
    fn of(data_dir: &Path) -> Self {
        let absolute = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        let resolved = crate::paths::resolve(&absolute);
        let normalized = strip_verbatim(&resolved.to_string_lossy());
        let current = tag_of(&normalized);
        let mut legacy = Vec::new();
        let mut add = |text: &str| {
            let t = tag_of(text);
            if t != current && !legacy.contains(&t) {
                legacy.push(t);
            }
        };
        // What earlier versions computed: the canonical path when the folder
        // existed (with `\\?\` on Windows), the path as given otherwise.
        add(&data_dir
            .canonicalize()
            .unwrap_or_else(|_| data_dir.to_path_buf())
            .to_string_lossy());
        if cfg!(windows) {
            add(&verbatim(&normalized));
        }
        // First-run secrets on systems where a parent is a link (macOS /var).
        add(&absolute.to_string_lossy());
        add(&data_dir.to_string_lossy());
        DirTags { current, legacy }
    }
}

/// A path without the Windows verbatim prefix: `\\?\C:\x` → `C:\x`,
/// `\\?\UNC\server\share` → `\\server\share`. Other text is unchanged.
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

/// Stable short tag for a path's text: first 8 bytes of SHA-256 of the
/// lowercased text (lowercased everywhere, as earlier versions did).
fn tag_of(text: &str) -> String {
    let digest = Sha256::digest(text.to_lowercase().as_bytes());
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

    /// A made-up Windows data folder (not anyone's real path).
    const SAMPLE_DIR: &str = r"C:\Users\alex.sample\AppData\Local\ClientLedger\pgdata";

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(strip_verbatim(r"\\?\C:\a\pgdata"), r"C:\a\pgdata");
        assert_eq!(
            strip_verbatim(r"\\?\UNC\server\share\pgdata"),
            r"\\server\share\pgdata"
        );
        assert_eq!(strip_verbatim(r"C:\a\pgdata"), r"C:\a\pgdata");
        assert_eq!(strip_verbatim("/var/x/pgdata"), "/var/x/pgdata");
        assert_eq!(verbatim(r"C:\a"), r"\\?\C:\a");
        assert_eq!(verbatim(r"\\server\share"), r"\\?\UNC\server\share");
    }

    #[test]
    fn a_first_run_tag_is_the_new_tag() {
        // First run wrote under the plain path; later runs looked under the
        // canonical `\\?\` form. The new tag must equal the first-run one.
        assert_eq!(tag_of(SAMPLE_DIR), "ff61f0171e214704");
        assert_eq!(
            tag_of(&strip_verbatim(&format!(r"\\?\{SAMPLE_DIR}"))),
            "ff61f0171e214704"
        );
        assert_eq!(tag_of(&verbatim(SAMPLE_DIR)), "1d2f0ad9f7eec027");
    }

    #[test]
    fn the_tag_is_the_same_before_and_after_the_folder_exists() {
        let d = scratch("tag");
        let data = d.join("pgdata");
        // The name the old code wrote first-run secrets under: the folder did
        // not exist yet, so it hashed the path as given.
        let first_run = tag_of(&data.to_string_lossy());
        let before = DirTags::of(&data);
        fs::create_dir_all(&data).unwrap();
        let after = DirTags::of(&data);
        assert_eq!(before.current, after.current);
        // The exact bug: once the folder exists, that first-run name must still
        // be reachable (macOS /var -> /private/var, Windows `\\?\` and 8.3
        // short names, plain Linux paths).
        assert!(
            first_run == after.current || after.legacy.contains(&first_run),
            "first-run tag lost"
        );
        // Once it exists, the old computation (canonical path) is a legacy tag.
        let old = tag_of(&data.canonicalize().unwrap().to_string_lossy());
        assert!(old == after.current || after.legacy.contains(&old));
        assert!(!after.legacy.contains(&after.current));
        let _ = fs::remove_dir_all(d);
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_canonical_verbatim_tag_is_legacy() {
        let d = scratch("verb");
        let data = d.join("pgdata");
        fs::create_dir_all(&data).unwrap();
        let tags = DirTags::of(&data);
        let canon = data.canonicalize().unwrap();
        let canon = canon.to_string_lossy();
        assert!(canon.starts_with(r"\\?\"));
        assert_eq!(tags.current, tag_of(&strip_verbatim(&canon)));
        assert!(tags.legacy.contains(&tag_of(&canon)));
        let _ = fs::remove_dir_all(d);
    }

    type Map = Mutex<std::collections::HashMap<String, String>>;

    fn map_get(m: &Map) -> impl Fn(&str) -> Result<Option<Secret>> + '_ {
        |a| Ok(m.lock().unwrap().get(a).map(|v| Secret::new(v.clone())))
    }

    fn map_set(m: &Map) -> impl Fn(&str, &Secret) -> Result<()> + '_ {
        |a, s| {
            m.lock()
                .unwrap()
                .insert(a.to_string(), s.expose().to_string());
            Ok(())
        }
    }

    #[test]
    fn a_secret_under_the_legacy_canonical_name_is_found_and_copied() {
        let d = scratch("legacy");
        let data = d.join("pgdata");
        fs::create_dir_all(&data).unwrap();
        let store = KeyringStore::for_purpose(&data, "host-master");
        let old_name = format!("host-master-{}", tag_of(&verbatim(SAMPLE_DIR)));
        let legacy = vec![
            format!("host-master-{}", tag_of(SAMPLE_DIR)),
            old_name.clone(),
        ];
        let m: Map = Mutex::default();
        m.lock()
            .unwrap()
            .insert(old_name.clone(), "the-master".into());
        let got = lookup_with_legacy(&store.account, &legacy, map_get(&m), map_set(&m)).unwrap();
        assert_eq!(got, Some(Secret::new("the-master")));
        let map = m.lock().unwrap();
        assert_eq!(
            map.get(&store.account).map(String::as_str),
            Some("the-master")
        );
        assert_eq!(
            map.get(&old_name).map(String::as_str),
            Some("the-master"),
            "kept"
        );
        assert_eq!(map.len(), 2, "nothing else written");
        drop(map);
        // The real store lists the canonical form of its folder as legacy.
        let canon = tag_of(&data.canonicalize().unwrap().to_string_lossy());
        assert!(
            store.account.ends_with(&canon)
                || store
                    .legacy_accounts
                    .contains(&format!("host-master-{canon}"))
        );
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn the_current_name_wins_and_nothing_found_stays_nothing() {
        let m: Map = Mutex::default();
        let legacy = vec!["p-old".to_string()];
        assert_eq!(
            lookup_with_legacy("p-new", &legacy, map_get(&m), map_set(&m)).unwrap(),
            None
        );
        assert!(m.lock().unwrap().is_empty(), "no secret is ever invented");
        m.lock().unwrap().insert("p-new".into(), "new".into());
        m.lock().unwrap().insert("p-old".into(), "old".into());
        assert_eq!(
            lookup_with_legacy("p-new", &legacy, map_get(&m), map_set(&m)).unwrap(),
            Some(Secret::new("new"))
        );
        assert_eq!(
            m.lock().unwrap().get("p-old").map(String::as_str),
            Some("old")
        );
    }

    #[test]
    fn a_failed_copy_still_returns_the_secret() {
        let m: Map = Mutex::default();
        m.lock().unwrap().insert("p-old".into(), "old".into());
        let got = lookup_with_legacy("p-new", &["p-old".to_string()], map_get(&m), |_, _| {
            Err(PgError::SecretStore("locked".into()))
        })
        .unwrap();
        assert_eq!(got, Some(Secret::new("old")));
    }

    #[test]
    fn an_unreadable_legacy_name_is_skipped() {
        let m: Map = Mutex::default();
        m.lock().unwrap().insert("p-old2".into(), "old".into());
        let legacy = vec!["p-old1".to_string(), "p-old2".to_string()];
        let get = |a: &str| {
            if a == "p-old1" {
                Err(PgError::SecretStore("denied".into()))
            } else {
                map_get(&m)(a)
            }
        };
        let got = lookup_with_legacy("p-new", &legacy, get, map_set(&m)).unwrap();
        assert_eq!(got, Some(Secret::new("old")));
        // An error on the current name still fails the read.
        let failing = |_: &str| Err(PgError::SecretStore("locked".into()));
        assert!(lookup_with_legacy("p-new", &legacy, failing, map_set(&m)).is_err());
    }

    fn map_delete(m: &Map) -> impl Fn(&str) -> Result<()> + '_ {
        |a| {
            m.lock().unwrap().remove(a);
            Ok(())
        }
    }

    #[test]
    fn a_set_retires_the_legacy_names() {
        let m: Map = Mutex::default();
        let legacy = vec!["p-old1".to_string(), "p-old2".to_string()];
        m.lock().unwrap().insert("p-old2".into(), "old".into());
        // A read copies forward and keeps the legacy entry ...
        assert_eq!(
            lookup_with_legacy("p-new", &legacy, map_get(&m), map_set(&m)).unwrap(),
            Some(Secret::new("old"))
        );
        assert!(m.lock().unwrap().contains_key("p-old2"));
        // ... until the next set, which removes every legacy entry.
        set_and_retire(
            "p-new",
            &legacy,
            &Secret::new("fresh"),
            map_set(&m),
            map_delete(&m),
        )
        .unwrap();
        let map = m.lock().unwrap();
        assert_eq!(map.get("p-new").map(String::as_str), Some("fresh"));
        assert_eq!(map.len(), 1, "only the current name remains");
    }

    #[test]
    fn a_failed_set_retires_nothing_and_a_failed_retire_still_saves() {
        let m: Map = Mutex::default();
        m.lock().unwrap().insert("p-old".into(), "old".into());
        let legacy = vec!["p-old".to_string()];
        let err = set_and_retire(
            "p-new",
            &legacy,
            &Secret::new("fresh"),
            |_, _| Err(PgError::SecretStore("locked".into())),
            map_delete(&m),
        );
        assert!(err.is_err());
        assert!(m.lock().unwrap().contains_key("p-old"), "kept on failure");
        set_and_retire("p-new", &legacy, &Secret::new("fresh"), map_set(&m), |_| {
            Err(PgError::SecretStore("denied".into()))
        })
        .unwrap();
        assert_eq!(
            m.lock().unwrap().get("p-new").map(String::as_str),
            Some("fresh")
        );
    }

    #[test]
    fn delete_removes_legacy_names_first_and_the_current_name_last() {
        let order = Mutex::new(Vec::<String>::new());
        let legacy = vec!["p-old1".to_string(), "p-old2".to_string()];
        delete_all("p-new", &legacy, |a| {
            order.lock().unwrap().push(a.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(*order.lock().unwrap(), ["p-old1", "p-old2", "p-new"]);
        // A failure stops before the current name is touched.
        order.lock().unwrap().clear();
        let err = delete_all("p-new", &legacy, |a| {
            order.lock().unwrap().push(a.to_string());
            if a == "p-old1" {
                Err(PgError::SecretStore("denied".into()))
            } else {
                Ok(())
            }
        });
        assert!(err.is_err());
        assert_eq!(*order.lock().unwrap(), ["p-old1"]);
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
