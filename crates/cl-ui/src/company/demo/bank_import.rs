//! The demo's bank feed for Import Bank File (P25, Q-P25m): the real
//! parsers run in the wizard; this keeps the imported rows in memory and
//! classifies a batch with the Rust copy of the engine's keys v1
//! (`cl_import::dedupe`), so re-importing a file adds nothing here too.
//!
//! The demo runs the engine's exact rules and its look-alike rule: a bank
//! id seen before is a duplicate only with the same amount and a date
//! within 5 days, otherwise the bank reused the id and it is ignored
//! (review DS7); the same row seen before; a bank id repeated in one file;
//! look-alikes (Q-P25c). Changed rows and the balance check are the
//! engine's ("not available in demo"). The account link is keyed like the
//! engine's (Q-P25u: the hash is made here, standing in for the engine, and
//! only the hash and mask are kept). A commit's preparation finishes after
//! a short delay, like the engine's (Q-P25v).

use std::collections::BTreeSet;

use cl_api::{
    BankIdentity, BankImportCounts, BankImportOutcome, BankImportRefusal, BankImportRequest,
    BankLookupBy, BankLookupVia, ConnectionMatch, ImportBatchId, ImportPostProcessing,
    ImportRowReason, ImportRowResult, ImportRowStatus, ImportUndoOutcome,
};

/// How long the demo's preparation of a commit takes.
const DEMO_PREPARING: std::time::Duration = std::time::Duration::from_millis(1200);

/// The demo's stand-in for the engine's keyed hash of an account number
/// (the engine keeps the real key, Q-P25u).
fn demo_hash(id: &BankIdentity) -> String {
    let text = format!(
        "demo-link-v1|{}|{}",
        id.bank_id.as_deref().unwrap_or_default(),
        id.acct_id
    );
    crate::screens::import_bank_file::flow::sha256_hex(text.as_bytes())
}

/// The mask the engine stores (the reader's masking rule).
fn demo_mask(id: &BankIdentity) -> String {
    cl_import::normalize::AccountId::new(id.bank_id.as_deref(), &id.acct_id)
        .map(|a| a.mask())
        .unwrap_or_default()
}
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
    /// For the look-alike rule (Q-P25c).
    date: cl_core::CalendarDate,
    amount: rust_decimal::Decimal,
    description: String,
    fitid: Option<String>,
    check_number: Option<u32>,
}

/// Q-P25c (owner 2026-11-07): `row` looks like `item` when the amounts
/// are equal, the dates are within 3 days, the bank ids and check numbers
/// do not differ, and the descriptions are equal or one starts the other
/// (the shorter at least 8 characters).
fn looks_alike(item: &Item, row: &cl_api::BankImportRow) -> bool {
    if item.amount != row.amount {
        return false;
    }
    let days = item.date.days_until(row.date).abs();
    if days > 3 {
        return false;
    }
    let a = cl_import::dedupe::norm(&item.description);
    let b = cl_import::dedupe::norm(&row.description);
    if let (Some(x), Some(y)) = (item.fitid.as_deref(), row.fitid.as_deref())
        && x.trim() != y.trim()
        && !(days == 0 && a == b)
    {
        return false;
    }
    if let (Some(x), Some(y)) = (item.check_number, row.check_number)
        && x != y
    {
        return false;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    a == b || (short.chars().count() >= 8 && long.starts_with(short.as_str()))
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
    /// When its preparation is done (Q-P25v).
    ready_at: std::time::Instant,
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
    /// Empties the feed (`dev.demo_bank_import_reset`).
    pub fn reset(&mut self) {
        *self = DemoBankFeed::default();
    }

    /// Items waiting in the feed (all accounts).
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.items.iter().filter(|i| i.pending).count()
    }

    /// Previews or commits `req` at `now`.
    ///
    /// # Errors
    /// A commit of a file whose account is linked to another account,
    /// without `relink_identity`, is refused like the engine's 409.
    pub fn import(
        &mut self,
        req: &BankImportRequest,
        commit: bool,
        now: Timestamp,
    ) -> Result<BankImportOutcome, BankImportRefusal> {
        let identity = req
            .bank_identity
            .as_ref()
            .map(|id| (demo_hash(id), demo_mask(id)));
        let linked_elsewhere = identity.as_ref().and_then(|(hash, _)| {
            self.identities
                .iter()
                .find(|i| &i.hash == hash && i.account != req.account)
                .map(|i| cl_api::LinkedElsewhere {
                    account_name: String::new(),
                    mask: i.mask.clone(),
                })
        });
        if commit
            && !req.relink_identity
            && let Some(l) = linked_elsewhere.clone()
        {
            return Err(BankImportRefusal::LinkedElsewhere(l));
        }
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
            // Review DS7: a bank id seen before is the same transaction
            // only with the same amount and a date within 5 days; else the
            // bank reused the id and it is ignored (the row key decides).
            let seen = fitid_key.as_ref().and_then(|fk| {
                mine.iter()
                    .position(|i| &i.dedupe_key == fk)
                    .filter(|p| !used.contains(p))
            });
            let by_fitid = seen.filter(|p| {
                mine.get(*p)
                    .is_some_and(|i| i.amount == r.amount && i.date.days_until(r.date).abs() <= 5)
            });
            let reused = seen.is_some() && by_fitid.is_none();
            let fitid_key = if reused { None } else { fitid_key };
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
            } else if let Some(p) = mine
                .iter()
                .enumerate()
                .position(|(i, it)| !used.contains(&i) && looks_alike(it, r))
            {
                (
                    ImportRowStatus::PossibleDuplicate,
                    Some(ImportRowReason::SimilarExisting),
                    Some(p),
                )
            } else {
                let reason = if reused {
                    Some(ImportRowReason::BankIdReused)
                } else {
                    r.fitid
                        .as_deref()
                        .is_some_and(|f| seen_fitids.get(f.trim()).copied().unwrap_or(0) > 1)
                        .then_some(ImportRowReason::RepeatedBankId)
                };
                (ImportRowStatus::New, reason, None)
            };
            if let Some(p) = existing {
                used.insert(p);
            }
            match status {
                ImportRowStatus::Duplicate => counts.duplicate += 1,
                ImportRowStatus::PossibleDuplicate => counts.possible_duplicate += 1,
                ImportRowStatus::Changed => counts.changed += 1,
                ImportRowStatus::New => counts.new += 1,
            }
            // Q-P25c: look-alikes are added with a flag, like new rows.
            let insert = matches!(
                status,
                ImportRowStatus::New | ImportRowStatus::PossibleDuplicate
            ) && r.include != Some(false);
            if insert && commit {
                to_insert.push(Item {
                    account: req.account,
                    batch,
                    dedupe_key: fitid_key.unwrap_or_else(|| k.row_key.clone()),
                    row_key: k.row_key.clone(),
                    imported_at: now,
                    pending: true,
                    date: r.date,
                    amount: r.amount,
                    description: r.description.clone(),
                    fitid: r.fitid.clone(),
                    check_number: r.check_number,
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
                ready_at: std::time::Instant::now() + DEMO_PREPARING,
            });
            if let Some((hash, mask)) = identity {
                self.identities.retain(|i| i.hash != hash);
                self.identities.push(Identity {
                    hash,
                    mask,
                    account: req.account,
                });
            }
        }
        Ok(BankImportOutcome {
            batch: commit.then_some(batch),
            same_file_imported_at: same_file,
            identity_linked_elsewhere: if commit { None } else { linked_elsewhere },
            inserted_items: Vec::new(),
            post_processing: commit.then_some(ImportPostProcessing::Scheduled),
            rows,
            counts,
            inserted_total: total,
            balance_check: None,
            replayed: false,
        })
    }

    /// Where a batch's preparation is (Q-P25v): done once its short delay
    /// has passed.
    #[must_use]
    pub fn post_processing(&self, id: ImportBatchId) -> Option<ImportPostProcessing> {
        self.batches.iter().find(|b| b.id == id).map(|b| {
            if std::time::Instant::now() >= b.ready_at {
                ImportPostProcessing::Done
            } else {
                ImportPostProcessing::Scheduled
            }
        })
    }

    /// Undoes a batch: its waiting rows leave the feed.
    ///
    /// # Errors
    /// `AlreadyUndone`, or `StillPreparing` while the batch is prepared.
    pub fn undo(&mut self, id: ImportBatchId) -> Result<ImportUndoOutcome, BankImportRefusal> {
        let b = self
            .batches
            .iter_mut()
            .find(|b| b.id == id && !b.undone)
            .ok_or(BankImportRefusal::AlreadyUndone)?;
        if std::time::Instant::now() < b.ready_at {
            return Err(BankImportRefusal::StillPreparing);
        }
        b.undone = true;
        let before = self.items.len();
        self.items.retain(|i| !(i.batch == id && i.pending));
        let removed = u32::try_from(before - self.items.len()).unwrap_or(u32::MAX);
        Ok(ImportUndoOutcome {
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
        let hash = match by {
            BankLookupBy::Account(id) => Some(demo_hash(id)),
            BankLookupBy::Mask(_) => None,
        };
        self.identities
            .iter()
            .filter(|i| match by {
                BankLookupBy::Account(_) => Some(&i.hash) == hash.as_ref(),
                BankLookupBy::Mask(m) => &i.mask == m,
            })
            .map(|i| ConnectionMatch {
                account: i.account,
                account_name: String::new(),
                mask: Some(i.mask.clone()),
                via: match by {
                    BankLookupBy::Account(_) => BankLookupVia::Hash,
                    BankLookupBy::Mask(_) => BankLookupVia::Mask,
                },
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_api::{BankFileFormat, BankImportRow};
    use cl_core::CalendarDate;

    fn account(n: u8) -> AccountId {
        format!("00000000-0000-4000-8000-0000000000{n:02}")
            .parse()
            .expect("id")
    }

    fn row(line: u32, day: u32, amount: &str, text: &str, fitid: &str) -> BankImportRow {
        BankImportRow {
            line,
            date: CalendarDate::from_ymd(2026, 11, day).expect("date"),
            amount: amount.parse().expect("amount"),
            description: text.into(),
            memo: None,
            check_number: None,
            fitid: Some(fitid.into()),
            include: None,
            changed_action: None,
        }
    }

    fn req(rows: Vec<BankImportRow>, sha: char) -> BankImportRequest {
        BankImportRequest {
            account: account(1),
            format: BankFileFormat::Qfx,
            file_name: "f.qfx".into(),
            source_file_sha256: sha.to_string().repeat(64),
            section_index: 0,
            section_start: None,
            section_end: None,
            amounts_flipped: false,
            csv_mapping: None,
            relink_identity: false,
            bank_identity: Some(BankIdentity {
                bank_id: None,
                acct_id: "000012342208".into(),
            }),
            file_ending_balance: None,
            file_ending_date: None,
            rows,
        }
    }

    /// Review DS7 (engine fixture 23): a bank that numbers each download
    /// from 1 reuses ids for other transactions; December's rows are new.
    #[test]
    fn per_download_bank_ids_are_not_duplicates() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let nov = (1..=3)
            .map(|i| {
                row(
                    i,
                    10 + i,
                    &format!("{i}0.00"),
                    &format!("NOV {i}"),
                    &i.to_string(),
                )
            })
            .collect();
        f.import(&req(nov, 'e'), true, now).expect("nov");
        let dec = (1..=3)
            .map(|i| {
                row(
                    i,
                    20 + i,
                    &format!("{i}1.00"),
                    &format!("DEC {i}"),
                    &i.to_string(),
                )
            })
            .collect();
        let o = f.import(&req(dec, 'f'), true, now).expect("dec");
        assert_eq!((o.counts.new, o.counts.inserted), (3, 3));
        assert!(
            o.rows
                .iter()
                .all(|r| r.reason == Some(ImportRowReason::BankIdReused))
        );
        // A date moved 2 days with the same amount is still the same.
        let shifted = vec![row(1, 13, "10.00", "NOV 1", "1")];
        let o = f.import(&req(shifted, 'a'), false, now).expect("shift");
        assert_eq!(o.counts.duplicate, 1);
    }

    /// The link is kept as a hash and a mask only; a file linked to
    /// another account is refused on commit unless relinked; preparation
    /// takes a moment and undo waits for it.
    #[test]
    fn links_relinks_and_prepares() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let r = req(vec![row(1, 2, "5.00", "SHOP", "A")], 'b');
        let o = f.import(&r, true, now).expect("first");
        assert_eq!(o.post_processing, Some(ImportPostProcessing::Scheduled));
        let batch = o.batch.expect("batch");
        assert_eq!(f.undo(batch), Err(BankImportRefusal::StillPreparing));
        assert!(
            !format!("{f:?}").contains("12342208"),
            "no account number kept"
        );
        let id = r.bank_identity.clone().expect("identity");
        assert_eq!(
            f.lookup(&BankLookupBy::Account(id))[0].via,
            BankLookupVia::Hash
        );
        let mut other = req(vec![row(1, 3, "6.00", "SHOP 2", "B")], 'c');
        other.account = account(2);
        assert!(
            f.import(&other, false, now)
                .expect("preview")
                .identity_linked_elsewhere
                .is_some()
        );
        assert!(matches!(
            f.import(&other, true, now),
            Err(BankImportRefusal::LinkedElsewhere(_))
        ));
        other.relink_identity = true;
        assert!(f.import(&other, true, now).is_ok());
        std::thread::sleep(DEMO_PREPARING);
        assert_eq!(f.post_processing(batch), Some(ImportPostProcessing::Done));
        assert!(f.undo(batch).is_ok());
    }
}
