//! The demo's bank feed for Import Bank File (P25, Q-P25m): the real
//! parsers run in the wizard; this keeps the imported rows in memory and
//! classifies a batch with the Rust copy of the engine's keys v1
//! (`cl_import::dedupe`), so re-importing a file adds nothing here too.
//!
//! Only the exact rules run (a bank id seen before, the same row seen
//! before, a bank id repeated in one file). Possible duplicates, changed
//! rows and the balance check are the engine's; the demo has none ("not
//! available in demo").

use std::collections::BTreeSet;

use cl_api::{
    BankImportCounts, BankImportOutcome, BankImportRequest, BankLookupBy, BankLookupVia,
    ConnectionMatch, ImportBatchId, ImportPostProcessing, ImportRowReason, ImportRowResult,
    ImportRowStatus, ImportUndoOutcome,
};
use cl_core::ids::AccountId;
use cl_core::{Money, Timestamp};

/// One row in the demo feed.
#[derive(Clone, Debug)]
struct Item {
    account: AccountId,
    batch: ImportBatchId,
    dedupe_key: String,
    row_key: String,
    imported_at: Timestamp,
    /// Still waiting (undo removes the waiting ones).
    pending: bool,
}

/// One import.
#[derive(Clone, Debug)]
struct Batch {
    id: ImportBatchId,
    account: AccountId,
    format: cl_api::BankFileFormat,
    flipped: bool,
    mapping: Option<serde_json::Value>,
    sha256: String,
    at: Timestamp,
    undone: bool,
    inserted: u32,
}

/// A linked file identity.
#[derive(Clone, Debug)]
struct Identity {
    hash: String,
    mask: String,
    account: AccountId,
}

/// The demo feed.
#[derive(Clone, Debug, Default)]
pub struct DemoBankFeed {
    items: Vec<Item>,
    batches: Vec<Batch>,
    identities: Vec<Identity>,
}

fn batch_id(n: usize) -> ImportBatchId {
    ImportBatchId::parse(&format!("d0000000-0000-4000-8000-b{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

impl DemoBankFeed {
    /// Items waiting in the feed (all accounts).
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.items.iter().filter(|i| i.pending).count()
    }

    /// Previews or commits `req` at `now`.
    pub fn import(
        &mut self,
        req: &BankImportRequest,
        commit: bool,
        now: Timestamp,
    ) -> BankImportOutcome {
        let keyed = cl_import::dedupe::key_rows(
            req.rows
                .iter()
                .map(|r| (r.date, r.amount, r.description.as_str())),
        );
        let mut seen_fitids: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for r in &req.rows {
            if let Some(f) = r.fitid.as_deref() {
                *seen_fitids.entry(f.trim()).or_default() += 1;
            }
        }
        let mine: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.account == req.account)
            .collect();
        let mut rows = Vec::with_capacity(req.rows.len());
        let mut counts = BankImportCounts {
            in_file: u32::try_from(req.rows.len()).unwrap_or(u32::MAX),
            ..BankImportCounts::default()
        };
        let mut to_insert = Vec::new();
        let mut total = Money::ZERO;
        let batch = batch_id(self.batches.len() + 1);
        let mut used: BTreeSet<usize> = BTreeSet::new();
        for (r, k) in req.rows.iter().zip(&keyed) {
            let fitid = r
                .fitid
                .as_deref()
                .map(str::trim)
                .filter(|f| !f.is_empty() && seen_fitids.get(f).copied() == Some(1));
            let fitid_key = fitid.map(cl_import::dedupe::fitid_key);
            let by_fitid = fitid_key.as_ref().and_then(|fk| {
                mine.iter()
                    .position(|i| &i.dedupe_key == fk)
                    .filter(|p| !used.contains(p))
            });
            let by_row = || {
                mine.iter()
                    .position(|i| i.row_key == k.row_key || i.dedupe_key == k.row_key)
                    .filter(|p| !used.contains(p))
            };
            let (status, reason, existing) = if let Some(p) = by_fitid {
                (
                    ImportRowStatus::Duplicate,
                    Some(ImportRowReason::SameBankId),
                    Some(p),
                )
            } else if let Some(p) = by_row() {
                (
                    ImportRowStatus::Duplicate,
                    Some(ImportRowReason::SameRow),
                    Some(p),
                )
            } else {
                let reason = r
                    .fitid
                    .as_deref()
                    .is_some_and(|f| seen_fitids.get(f.trim()).copied().unwrap_or(0) > 1)
                    .then_some(ImportRowReason::RepeatedBankId);
                (ImportRowStatus::New, reason, None)
            };
            if let Some(p) = existing {
                used.insert(p);
                counts.duplicate += 1;
            } else {
                counts.new += 1;
            }
            let insert = status == ImportRowStatus::New && r.include != Some(false);
            if insert && commit {
                to_insert.push(Item {
                    account: req.account,
                    batch,
                    dedupe_key: fitid_key.unwrap_or_else(|| k.row_key.clone()),
                    row_key: k.row_key.clone(),
                    imported_at: now,
                    pending: true,
                });
                if let Some(m) = cl_api::wire_decimal(r.amount)
                    .ok()
                    .and_then(|s| Money::parse_wire(&s).ok())
                {
                    total = total.checked_add(m).unwrap_or(total);
                }
                counts.inserted += 1;
            }
            let existing_item = existing.and_then(|p| mine.get(p));
            rows.push(ImportRowResult {
                line: r.line,
                status,
                reason,
                existing_item: None,
                existing_imported_at: existing_item.map(|i| i.imported_at),
                existing_pending: existing_item.map(|i| i.pending),
                closed_period: false,
                inserted: insert && commit,
                replaced: false,
            });
        }
        let same_file = self
            .batches
            .iter()
            .rev()
            .find(|b| b.account == req.account && b.sha256 == req.source_file_sha256 && !b.undone)
            .map(|b| b.at);
        let linked_elsewhere = req.bank_identity.as_ref().and_then(|id| {
            self.identities
                .iter()
                .find(|i| i.hash == id.account_key_hash && i.account != req.account)
                .map(|i| cl_api::LinkedElsewhere {
                    account_name: String::new(),
                    mask: i.mask.clone(),
                })
        });
        if commit {
            self.items.extend(to_insert);
            self.batches.push(Batch {
                id: batch,
                account: req.account,
                format: req.format,
                flipped: req.amounts_flipped,
                mapping: req.csv_mapping.clone(),
                sha256: req.source_file_sha256.clone(),
                at: now,
                undone: false,
                inserted: counts.inserted,
            });
            if let Some(id) = &req.bank_identity {
                self.identities.retain(|i| i.hash != id.account_key_hash);
                self.identities.push(Identity {
                    hash: id.account_key_hash.clone(),
                    mask: id.mask.clone(),
                    account: req.account,
                });
            }
        }
        BankImportOutcome {
            batch: commit.then_some(batch),
            same_file_imported_at: same_file,
            identity_linked_elsewhere: if commit { None } else { linked_elsewhere },
            inserted_items: Vec::new(),
            post_processing: commit.then_some(ImportPostProcessing::Done),
            rows,
            counts,
            inserted_total: total,
            balance_check: None,
            replayed: false,
        }
    }

    /// Undoes a batch: its waiting rows leave the feed.
    pub fn undo(&mut self, id: ImportBatchId) -> Option<ImportUndoOutcome> {
        let b = self.batches.iter_mut().find(|b| b.id == id && !b.undone)?;
        b.undone = true;
        let before = self.items.len();
        self.items.retain(|i| !(i.batch == id && i.pending));
        let removed = u32::try_from(before - self.items.len()).unwrap_or(u32::MAX);
        Some(ImportUndoOutcome {
            batch: id,
            removed,
            kept: 0,
        })
    }

    /// The latest import of `account` in `format`: its flip and mapping.
    #[must_use]
    pub fn latest(
        &self,
        account: AccountId,
        format: cl_api::BankFileFormat,
    ) -> Option<(bool, Option<serde_json::Value>)> {
        self.batches
            .iter()
            .rev()
            .find(|b| b.account == account && b.format == format && !b.undone && b.inserted > 0)
            .map(|b| (b.flipped, b.mapping.clone()))
    }

    /// The accounts a file identity was linked to.
    #[must_use]
    pub fn lookup(&self, by: &BankLookupBy) -> Vec<ConnectionMatch> {
        self.identities
            .iter()
            .filter(|i| match by {
                BankLookupBy::Hash(h) => &i.hash == h,
                BankLookupBy::Mask(m) => &i.mask == m,
            })
            .map(|i| ConnectionMatch {
                account: i.account,
                account_name: String::new(),
                mask: Some(i.mask.clone()),
                via: match by {
                    BankLookupBy::Hash(_) => BankLookupVia::Hash,
                    BankLookupBy::Mask(_) => BankLookupVia::Mask,
                },
            })
            .collect()
    }
}
