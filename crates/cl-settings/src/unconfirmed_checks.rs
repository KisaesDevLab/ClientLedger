//! Check batches printed but not yet confirmed (Q-P19n): saved on this
//! computer when the engine records the numbers, so Home's Needs-attention
//! reminder survives a restart, and cleared when the owner answers "Did all
//! checks print correctly?". Stored in `unconfirmed-checks.json` in the app
//! data folder; ids and check numbers only (no payees or amounts).

use crate::store::{read_capped, write_atomic};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// The file name.
pub const UNCONFIRMED_CHECKS_FILE: &str = "unconfirmed-checks.json";
/// Batches kept at most (the oldest beyond are dropped).
pub const MAX_BATCHES: usize = 200;

/// One check and the number it printed with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnconfirmedCheck {
    /// The engine's check id.
    pub check_id: String,
    /// The number recorded with the engine.
    pub number: u64,
}

/// One printed batch waiting for an answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnconfirmedBatch {
    /// The company (engine id).
    pub company_id: String,
    /// The bank account the checks are drawn on.
    pub bank_account_id: String,
    /// When it printed (RFC 3339, from the caller).
    pub printed_at: String,
    /// The checks, in print order.
    pub checks: Vec<UnconfirmedCheck>,
}

/// Every unanswered batch.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UnconfirmedChecks {
    /// Oldest first.
    pub batches: Vec<UnconfirmedBatch>,
}

impl UnconfirmedChecks {
    /// Parse stored text (defaults when unreadable; empty batches dropped).
    #[must_use]
    pub fn parse(text: &str) -> UnconfirmedChecks {
        let mut u: UnconfirmedChecks = serde_json::from_str(text).unwrap_or_default();
        u.normalize();
        u
    }

    fn normalize(&mut self) {
        self.batches
            .retain(|b| !b.checks.is_empty() && !b.company_id.is_empty());
        let extra = self.batches.len().saturating_sub(MAX_BATCHES);
        self.batches.drain(..extra);
    }

    /// Text to store.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Save a batch the engine just recorded.
    pub fn add(&mut self, batch: UnconfirmedBatch) {
        self.batches.push(batch);
        self.normalize();
    }

    /// The company's unanswered batches, oldest first.
    pub fn for_company<'a>(
        &'a self,
        company_id: &'a str,
    ) -> impl Iterator<Item = &'a UnconfirmedBatch> + 'a {
        self.batches
            .iter()
            .filter(move |b| b.company_id == company_id)
    }

    /// True while Home should show the reminder for the company.
    #[must_use]
    pub fn needs_attention(&self, company_id: &str) -> bool {
        self.for_company(company_id).next().is_some()
    }

    /// The owner answered for these checks (confirmed or requeued): forget
    /// them; batches left empty go.
    pub fn resolve(&mut self, company_id: &str, check_ids: &[&str]) {
        for b in self
            .batches
            .iter_mut()
            .filter(|b| b.company_id == company_id)
        {
            b.checks
                .retain(|c| !check_ids.contains(&c.check_id.as_str()));
        }
        self.normalize();
    }
}

/// Where the file lives.
#[derive(Debug, Clone)]
pub struct UnconfirmedChecksStore {
    path: PathBuf,
}

impl UnconfirmedChecksStore {
    /// The store in `dir` (created on first save).
    #[must_use]
    pub fn in_dir(dir: &Path) -> UnconfirmedChecksStore {
        UnconfirmedChecksStore {
            path: dir.join(UNCONFIRMED_CHECKS_FILE),
        }
    }

    /// The file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load (empty when missing or unreadable).
    #[must_use]
    pub fn load(&self) -> UnconfirmedChecks {
        match read_capped(&self.path) {
            Ok(Some(text)) => UnconfirmedChecks::parse(&text),
            _ => UnconfirmedChecks::default(),
        }
    }

    /// Save atomically.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, value: &UnconfirmedChecks) -> io::Result<()> {
        write_atomic(&self.path, &value.to_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(company: &str, ids: &[(&str, u64)]) -> UnconfirmedBatch {
        UnconfirmedBatch {
            company_id: company.into(),
            bank_account_id: "bank-1".into(),
            printed_at: "2026-10-29T10:00:00Z".into(),
            checks: ids
                .iter()
                .map(|(id, n)| UnconfirmedCheck {
                    check_id: (*id).into(),
                    number: *n,
                })
                .collect(),
        }
    }

    #[test]
    fn batches_drive_the_reminder_until_every_check_is_answered() {
        let mut u = UnconfirmedChecks::default();
        assert!(!u.needs_attention("co1"));
        u.add(batch("co1", &[("c1", 1001), ("c2", 1002)]));
        u.add(batch("co2", &[("c9", 5)]));
        assert!(u.needs_attention("co1") && u.needs_attention("co2"));
        assert_eq!(u.for_company("co1").count(), 1);
        u.resolve("co1", &["c1"]);
        assert!(u.needs_attention("co1"));
        u.resolve("co1", &["c2"]);
        assert!(!u.needs_attention("co1"));
        assert!(u.needs_attention("co2"));
        u.resolve("co1", &["c9"]); // another company's check is untouched
        assert!(u.needs_attention("co2"));
    }

    #[test]
    fn the_store_survives_a_restart_and_drops_junk() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = UnconfirmedChecksStore::in_dir(dir.path());
        assert_eq!(store.load(), UnconfirmedChecks::default());
        let mut u = UnconfirmedChecks::default();
        u.add(batch("co1", &[("c1", 1001)]));
        store.save(&u).expect("save");
        assert_eq!(store.load(), u);
        assert!(store.path().ends_with(UNCONFIRMED_CHECKS_FILE));
        assert_eq!(
            UnconfirmedChecks::parse("garbage"),
            UnconfirmedChecks::default()
        );
        let empty = UnconfirmedChecks::parse(
            r#"{"batches":[{"company_id":"co1","bank_account_id":"b","printed_at":"x","checks":[]}]}"#,
        );
        assert_eq!(empty.batches.len(), 0);
        let mut many = UnconfirmedChecks::default();
        for i in 0..(MAX_BATCHES + 3) {
            many.add(batch("co1", &[("c", u64::try_from(i).unwrap())]));
        }
        assert_eq!(many.batches.len(), MAX_BATCHES);
        assert_eq!(many.batches[0].checks[0].number, 3);
    }
}
