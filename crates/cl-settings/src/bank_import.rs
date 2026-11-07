//! Bank file import settings kept on this computer (P25).
//!
//! - **CSV column profiles** (Q-P25f, Q-P25w): the local fallback for a bank
//!   CSV's column mapping, after the mapping the engine stored with the
//!   account's latest batch. Stored as opaque JSON under a caller-chosen key
//!   (the company, the account and the header's shape), so this crate never
//!   needs the reader's types. No amounts, names or descriptions are stored.
//!
//! The account-link key is the engine's (Q-P25u, owner 2026-11-07): it
//! travels with the company, so nothing about it is kept here.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{read_capped, write_atomic};

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

    #[test]
    fn an_old_link_id_flag_is_ignored() {
        // Q-P25u: the link key moved to the engine; records written before
        // still read.
        let old: crate::CompanyFlags = serde_json::from_str(
            r#"{"sales_tax_enabled":true,"bank_link_id":"7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03"}"#,
        )
        .expect("old flags");
        assert!(old.sales_tax_enabled);
        assert!(
            !serde_json::to_string(&old)
                .expect("json")
                .contains("bank_link_id")
        );
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
