//! Bank file import settings kept on this computer (P25).
//!
//! - **The link key** (Q-P25e, Q-P25u, non-negotiable 11). A bank file names
//!   its account by number. The desktop sends the engine only a mask and
//!   `HMAC-SHA256(key, …)` of the number, so a later file of the same account
//!   links itself. The key is 32 random bytes kept in the OS keychain under
//!   `bank-link-<linkId>`, where `linkId` is a random id in the company's
//!   [`crate::CompanyFlags`] (it survives a restore on this computer). It is
//!   never written to a settings file, a log, a backup or a transfer file.
//!   Another computer has another key: there the first file of an account is
//!   linked by its mask once, then by the hash.
//! - **CSV column profiles** (Q-P25f, Q-P25w): the local fallback for a bank
//!   CSV's column mapping, after the mapping the engine stored with the
//!   account's latest batch. Stored as opaque JSON under a caller-chosen key
//!   (the company, the account and the header's shape), so this crate never
//!   needs the reader's types. No amounts, names or descriptions are stored.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use cl_core::Secret;
use serde::{Deserialize, Serialize};

use crate::secrets::{SecretError, SecretStore};
use crate::store::{read_capped, write_atomic};

/// Keychain entry name of a company's link key.
#[must_use]
pub fn link_key_name(link_id: uuid::Uuid) -> String {
    format!("bank-link-{}", link_id.simple())
}

/// A fresh link id for a company that has none yet.
#[must_use]
pub fn new_link_id() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}

/// Errors of the link key.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkKeyError {
    /// The keychain refused.
    #[error(transparent)]
    Secret(#[from] SecretError),
    /// The system gave no random bytes.
    #[error("no randomness available for the bank link key")]
    NoRandom,
}

/// The company's link key (64 lower-case hex characters for 32 bytes),
/// made and saved on first use.
///
/// # Errors
/// [`LinkKeyError`] when the keychain refuses or no randomness is
/// available. The caller then links by mask only.
pub fn link_key(store: &dyn SecretStore, link_id: uuid::Uuid) -> Result<Secret, LinkKeyError> {
    let name = link_key_name(link_id);
    if let Some(k) = store.get(&name)? {
        if is_key(k.expose()) {
            return Ok(k);
        }
        tracing::warn!("the bank link key was unreadable; making a new one");
    }
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| LinkKeyError::NoRandom)?;
    let hex = bytes.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    });
    let key = Secret::new(hex);
    store.set(&name, &key)?;
    Ok(key)
}

fn is_key(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// One saved CSV column mapping.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CsvProfile {
    /// The caller's key (company, account and header shape).
    pub key: String,
    /// The mapping, as the reader serializes it.
    pub mapping: serde_json::Value,
}

/// The file's content: newest last, capped at [`MAX_PROFILES`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CsvProfiles {
    /// Format version.
    pub version: u32,
    /// The profiles.
    pub profiles: Vec<CsvProfile>,
}

/// Profiles remembered.
pub const MAX_PROFILES: usize = 200;
/// The file in the app data folder.
pub const CSV_PROFILES_FILE: &str = "bank-csv-profiles.json";

impl CsvProfiles {
    /// Parses the file text; a damaged file is empty.
    #[must_use]
    pub fn parse(text: &str) -> CsvProfiles {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut v = self.clone();
        v.version = 1;
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
    }

    /// The mapping saved under `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.profiles
            .iter()
            .rev()
            .find(|p| p.key == key)
            .map(|p| &p.mapping)
    }

    /// Saves `mapping` under `key` (replacing an older one; the oldest go
    /// past the cap).
    pub fn put(&mut self, key: &str, mapping: serde_json::Value) {
        self.profiles.retain(|p| p.key != key);
        self.profiles.push(CsvProfile {
            key: key.to_string(),
            mapping,
        });
        let over = self.profiles.len().saturating_sub(MAX_PROFILES);
        self.profiles.drain(..over);
    }
}

/// Where the profiles live.
#[derive(Debug, Clone)]
pub struct CsvProfilesStore {
    path: PathBuf,
}

impl CsvProfilesStore {
    /// The store in `dir` (created on first save).
    #[must_use]
    pub fn in_dir(dir: &Path) -> CsvProfilesStore {
        CsvProfilesStore {
            path: dir.join(CSV_PROFILES_FILE),
        }
    }

    /// Load (empty when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> CsvProfiles {
        match read_capped(&self.path) {
            Ok(Some(text)) => CsvProfiles::parse(&text),
            _ => CsvProfiles::default(),
        }
    }

    /// Save atomically.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, value: &CsvProfiles) -> io::Result<()> {
        write_atomic(&self.path, &value.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemorySecrets;

    #[test]
    fn the_link_key_is_made_once_and_kept_in_the_keychain() {
        let store = MemorySecrets::default();
        let id = new_link_id();
        let name = link_key_name(id);
        assert!(name.starts_with("bank-link-") && name.len() == 42, "{name}");
        let k1 = link_key(&store, id).expect("made");
        assert!(is_key(k1.expose()));
        let k2 = link_key(&store, id).expect("kept");
        assert_eq!(k1.expose(), k2.expose());
        let other = link_key(&store, new_link_id()).expect("other company");
        assert_ne!(k1.expose(), other.expose());
        assert_ne!(new_link_id(), new_link_id());
    }

    #[test]
    fn the_link_id_is_an_optional_company_flag() {
        let old: crate::CompanyFlags =
            serde_json::from_str(r#"{"sales_tax_enabled":true}"#).expect("old flags");
        assert_eq!(old.bank_link_id, None);
        assert!(
            !serde_json::to_string(&old)
                .expect("json")
                .contains("bank_link_id")
        );
        let id = new_link_id();
        let with = crate::CompanyFlags {
            bank_link_id: Some(id),
            ..old
        };
        let back: crate::CompanyFlags =
            serde_json::from_str(&serde_json::to_string(&with).expect("json")).expect("back");
        assert_eq!(back.bank_link_id, Some(id));
    }

    #[test]
    fn a_damaged_key_is_replaced() {
        let store = MemorySecrets::default();
        let id = new_link_id();
        store
            .set(&link_key_name(id), &Secret::new("not hex"))
            .expect("set");
        assert!(is_key(link_key(&store, id).expect("new").expose()));
    }

    #[test]
    fn profiles_round_trip_newest_wins_and_are_capped() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = CsvProfilesStore::in_dir(dir.path());
        let mut p = store.load();
        assert_eq!(p.profiles, Vec::new());
        p.put("co/acct/h1", serde_json::json!({"date": 0}));
        p.put("co/acct/h1", serde_json::json!({"date": 1}));
        store.save(&p).expect("save");
        let p = store.load();
        assert_eq!(p.profiles.len(), 1);
        assert_eq!(p.get("co/acct/h1"), Some(&serde_json::json!({"date": 1})));
        assert_eq!(p.get("nope"), None);
        let mut many = CsvProfiles::default();
        for i in 0..=MAX_PROFILES {
            many.put(&i.to_string(), serde_json::Value::Null);
        }
        assert_eq!(many.profiles.len(), MAX_PROFILES);
        assert!(many.get("0").is_none(), "the oldest went");
        assert_eq!(CsvProfiles::parse("{broken"), CsvProfiles::default());
    }
}
