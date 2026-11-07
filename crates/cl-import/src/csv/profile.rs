//! Local CSV profiles (Q-P25f, Q-P25w): a mapping remembered for files
//! with the same header, kept by `cl-settings` as opaque JSON. The
//! account's latest batch mapping comes first; a profile is the fallback.

use super::mapping::{CsvMapping, CsvTable};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A remembered mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvProfile {
    /// [`fingerprint`] of the header it was made for.
    pub fingerprint: String,
    /// The mapping.
    pub mapping: CsvMapping,
}

/// A stable id for a header row: lowercase hex SHA-256 of the trimmed,
/// upper-cased names joined by `|`, plus the column count. Files without
/// a header get the column count only (`cols:N`).
#[must_use]
pub fn fingerprint(table: &CsvTable) -> String {
    match &table.header {
        Some(h) => {
            let joined: Vec<String> = h
                .fields
                .iter()
                .map(|f| f.trim().to_ascii_uppercase())
                .collect();
            let text = format!("v1|{}|{}", h.fields.len(), joined.join("|"));
            crate::dedupe::hex(&Sha256::digest(text.as_bytes()))
        }
        None => format!("cols:{}", table.width()),
    }
}

/// The first profile that fits `table`.
#[must_use]
pub fn find<'a>(profiles: &'a [CsvProfile], table: &CsvTable) -> Option<&'a CsvProfile> {
    let fp = fingerprint(table);
    profiles
        .iter()
        .find(|p| p.fingerprint == fp && p.mapping.fits(table))
}
