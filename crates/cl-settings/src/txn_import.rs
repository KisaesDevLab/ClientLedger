//! Saved column mappings of spreadsheet transaction imports (P31 D5,
//! Q-125), kept on this computer.
//!
//! Same pattern as the bank CSV profiles ([`crate::bank_import`]): one
//! JSON file in the app data folder, newest last, capped. A mapping is
//! filed under the company and the file's header set, and holds only the
//! field → header names the owner chose: never cells, amounts or names.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::{read_capped, write_atomic};

/// One saved mapping.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxnMappingEntry {
    /// The company (its id as text).
    pub company: String,
    /// The header set (`cl_model::imports::csv_txn::header_signature`).
    pub signature: String,
    /// Field (engine column name) → normalized header.
    pub fields: BTreeMap<String, String>,
}

/// The file's content: newest last, capped at [`MAX_TXN_MAPPINGS`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TxnMappings {
    /// Format version.
    pub version: u32,
    /// The mappings.
    pub entries: Vec<TxnMappingEntry>,
}

/// Mappings remembered across all companies.
pub const MAX_TXN_MAPPINGS: usize = 200;
/// The file in the app data folder.
pub const TXN_MAPPINGS_FILE: &str = "txn-csv-mappings.json";

impl TxnMappings {
    /// Parses the file text; a damaged file is empty.
    #[must_use]
    pub fn parse(text: &str) -> TxnMappings {
        serde_json::from_str(text).unwrap_or_default()
    }

    /// JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut v = self.clone();
        v.version = 1;
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
    }

    /// The mapping saved for this company and header set.
    #[must_use]
    pub fn get(&self, company: &str, signature: &str) -> Option<&BTreeMap<String, String>> {
        self.entries
            .iter()
            .rev()
            .find(|e| e.company == company && e.signature == signature)
            .map(|e| &e.fields)
    }

    /// Saves a mapping (replacing an older one for the same company and
    /// header set; the oldest go past the cap).
    pub fn put(&mut self, company: &str, signature: &str, fields: BTreeMap<String, String>) {
        self.entries
            .retain(|e| !(e.company == company && e.signature == signature));
        self.entries.push(TxnMappingEntry {
            company: company.to_string(),
            signature: signature.to_string(),
            fields,
        });
        let over = self.entries.len().saturating_sub(MAX_TXN_MAPPINGS);
        self.entries.drain(..over);
    }
}

/// Where the mappings live.
#[derive(Debug, Clone)]
pub struct TxnMappingsStore {
    path: PathBuf,
}

impl TxnMappingsStore {
    /// The store in `dir` (created on first save).
    #[must_use]
    pub fn in_dir(dir: &Path) -> TxnMappingsStore {
        TxnMappingsStore {
            path: dir.join(TXN_MAPPINGS_FILE),
        }
    }

    /// Load (empty when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> TxnMappings {
        match read_capped(&self.path) {
            Ok(Some(text)) => TxnMappings::parse(&text),
            _ => TxnMappings::default(),
        }
    }

    /// Save atomically.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, value: &TxnMappings) -> io::Result<()> {
        write_atomic(&self.path, &value.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(h: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("date".to_string(), h.to_string())])
    }

    #[test]
    fn txn_mappings_round_trip_per_company_newest_wins_and_capped() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = TxnMappingsStore::in_dir(dir.path());
        let mut m = store.load();
        assert_eq!(m.entries, Vec::new());
        m.put("co-a", "date\u{1f}type", fields("date"));
        m.put("co-a", "date\u{1f}type", fields("txn date"));
        m.put("co-b", "date\u{1f}type", fields("posted"));
        store.save(&m).expect("save");
        let m = store.load();
        assert_eq!(m.entries.len(), 2);
        assert_eq!(m.get("co-a", "date\u{1f}type"), Some(&fields("txn date")));
        assert_eq!(m.get("co-b", "date\u{1f}type"), Some(&fields("posted")));
        assert_eq!(m.get("co-c", "date\u{1f}type"), None);
        let mut many = TxnMappings::default();
        for i in 0..=MAX_TXN_MAPPINGS {
            many.put("co", &i.to_string(), BTreeMap::new());
        }
        assert_eq!(many.entries.len(), MAX_TXN_MAPPINGS);
        assert!(many.get("co", "0").is_none(), "the oldest went");
        assert_eq!(TxnMappings::parse("{broken"), TxnMappings::default());
    }
}
