//! Check batches printed but not yet confirmed (Q-P19n): saved on this
//! computer when the engine records the numbers, so Home's Needs-attention
//! reminder survives a restart, and cleared when the owner answers "Did all
//! checks print correctly?". Stored in `unconfirmed-checks.json` in the app
//! data folder; ids and check numbers only (no payees or amounts).
//!
//! PR-2 B2: the batch is saved as an **intent** (the checks, the numbers
//! planned and the run's key) *before* `POST /checks/print` is sent, so a
//! lost reply or a crash leaves a record to read back; it becomes a printed
//! batch once the engine's numbers are known.

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

/// Where a saved batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchState {
    /// The engine recorded the numbers; waiting for "Did all checks print
    /// correctly?" (files from before PR-2 are all this).
    #[default]
    Printed,
    /// About to be sent, or sent without an answer: the numbers are planned,
    /// not known to be recorded. Read back before anything else (B2).
    Intent,
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
    /// Printed, or only intended (B2).
    #[serde(default)]
    pub state: BatchState,
    /// The print run's key (its `Idempotency-Key`), set on an intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<String>,
    /// The check stock sent (`CheckFormat` wire name), so a resend under
    /// the key is the identical request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
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

    /// The company's printed batches (not intents), oldest first.
    pub fn printed_for<'a>(
        &'a self,
        company_id: &'a str,
    ) -> impl Iterator<Item = &'a UnconfirmedBatch> + 'a {
        self.for_company(company_id)
            .filter(|b| b.state == BatchState::Printed)
    }

    /// The company's intents (to read back), oldest first.
    pub fn intents_for<'a>(
        &'a self,
        company_id: &'a str,
    ) -> impl Iterator<Item = &'a UnconfirmedBatch> + 'a {
        self.for_company(company_id)
            .filter(|b| b.state == BatchState::Intent)
    }

    /// The engine recorded the intent `key`'s numbers for `recorded` (check
    /// ids): it becomes a printed batch of those checks; the others leave it.
    /// False when there is no such intent.
    pub fn promote(&mut self, company_id: &str, key: &str, recorded: &[&str]) -> bool {
        let Some(b) = self.batches.iter_mut().find(|b| {
            b.company_id == company_id
                && b.state == BatchState::Intent
                && b.client_key.as_deref() == Some(key)
        }) else {
            return false;
        };
        b.checks.retain(|c| recorded.contains(&c.check_id.as_str()));
        b.state = BatchState::Printed;
        self.normalize();
        true
    }

    /// Nothing of the intent `key` was recorded: forget it.
    pub fn drop_intent(&mut self, company_id: &str, key: &str) {
        self.batches.retain(|b| {
            !(b.company_id == company_id
                && b.state == BatchState::Intent
                && b.client_key.as_deref() == Some(key))
        });
    }

    /// True while Home should show the reminder for the company (an intent
    /// counts: Print Forms reads it back when opened).
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
            state: BatchState::Printed,
            client_key: None,
            format: None,
        }
    }

    /// B2: an intent is saved before the print is sent; promoted to the
    /// checks the engine recorded, or dropped when none were; it counts for
    /// the reminder and an old file (no state) reads as printed.
    #[test]
    fn an_intent_is_promoted_or_dropped_by_its_key() {
        let mut u = UnconfirmedChecks::default();
        let mut i = batch("co1", &[("c1", 1001), ("c2", 1002)]);
        i.state = BatchState::Intent;
        i.client_key = Some("k1".into());
        u.add(i.clone());
        assert!(u.needs_attention("co1"));
        assert_eq!(u.intents_for("co1").count(), 1);
        assert_eq!(u.printed_for("co1").count(), 0);
        assert!(!u.promote("co1", "other", &["c1"]));
        assert!(u.promote("co1", "k1", &["c1"]), "partial");
        let b = u.printed_for("co1").next().expect("printed");
        assert_eq!(b.checks.len(), 1);
        assert_eq!(b.checks[0].number, 1001);
        assert_eq!(u.intents_for("co1").count(), 0);
        u.add(i);
        u.drop_intent("co1", "k1");
        assert_eq!(u.intents_for("co1").count(), 0);
        assert_eq!(u.printed_for("co1").count(), 1, "the printed one stays");
        let old = UnconfirmedChecks::parse(
            r#"{"batches":[{"company_id":"co1","bank_account_id":"b","printed_at":"x","checks":[{"check_id":"c","number":5}]}]}"#,
        );
        assert_eq!(old.batches[0].state, BatchState::Printed);
        let text = u.to_text();
        assert_eq!(UnconfirmedChecks::parse(&text), u);
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
