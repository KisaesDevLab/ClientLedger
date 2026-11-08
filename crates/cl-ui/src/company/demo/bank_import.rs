//! The demo's bank feed for Import Bank File (P25, Q-P25m): the real
//! parsers run in the wizard; this keeps the imported rows in memory and
//! classifies a batch with the Rust copy of the engine's keys v1
//! (`cl_import::dedupe`), so re-importing a file adds nothing here too.
//!
//! The demo runs the engine's rules in its order (U-06): a bank id
//! repeated in one file is ignored; a bank id seen before is a duplicate
//! with the same amount and a date within 5 days (review DS7); then the
//! same row seen before; then the same bank id with another amount, a date
//! within 5 days and a matching description is a changed row (replace or
//! add as new); any other seen bank id was reused by the bank and is
//! ignored; then look-alikes (Q-P25c), compared only with items dated
//! inside the file's own date range, as the engine does. The balance check is the engine's
//! ("not available in demo"). The account link is keyed like the
//! engine's (Q-P25u: the hash is made here, standing in for the engine, and
//! only the hash and mask are kept). A commit's preparation finishes after
//! a short delay, like the engine's (Q-P25v), reporting how far it is
//! (Q-P25ad); an undo while it runs stops it (`Cancelled`, engine PR #234).

use std::collections::BTreeSet;

use cl_api::{
    BankIdentity, BankImportCounts, BankImportOutcome, BankImportRefusal, BankImportRequest,
    BankLookupBy, BankLookupVia, ChangedAction, ConnectionMatch, ImportBatchId, ImportBatchSummary,
    ImportPostProcessing, ImportRowReason, ImportRowResult, ImportRowStatus, ImportUndoOutcome,
};

/// How long the demo's preparation of a commit takes.
const DEMO_PREPARING: std::time::Duration = std::time::Duration::from_millis(800);
/// How long it takes after `dev.demo_bank_import_slow`, so a scenario can
/// undo or close the window while it runs.
const DEMO_PREPARING_SLOW: std::time::Duration = std::time::Duration::from_secs(60);

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

/// The mask the engine stores: the last 4 characters of the number.
fn demo_mask(id: &BankIdentity) -> String {
    let chars: Vec<char> = id.acct_id.trim().chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
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
    /// Replaced by a changed row: its values before (put back on undo).
    replaced: Option<(
        ImportBatchId,
        cl_core::CalendarDate,
        rust_decimal::Decimal,
        String,
    )>,
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
/// Q-P25c (owner confirmed 2026-11-07): like the engine, only items dated
/// inside the file's own date range are compared for look-alikes.
fn in_range(item: &Item, req: &BankImportRequest) -> bool {
    req.section_start.is_none_or(|start| item.date >= start)
        && req.section_end.is_none_or(|end| item.date <= end)
}

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

/// The engine's description match: equal, or one starts the other and
/// the shorter has at least 8 characters (normalized).
fn descriptions_match(a: &str, b: &str) -> bool {
    let (a, b) = (cl_import::dedupe::norm(a), cl_import::dedupe::norm(b));
    let (short, long) = if a.chars().count() <= b.chars().count() {
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
    /// When its preparation started and when it is done (Q-P25v).
    started_at: std::time::Instant,
    ready_at: std::time::Instant,
    /// An undo stopped the preparation (engine PR #234).
    cancelled: bool,
}

impl Batch {
    fn state(&self, now: std::time::Instant) -> ImportPostProcessing {
        if self.cancelled {
            ImportPostProcessing::Cancelled
        } else if now >= self.ready_at {
            ImportPostProcessing::Done
        } else {
            ImportPostProcessing::Scheduled
        }
    }

    /// Items prepared of all, like the engine's `processed`/`total`
    /// (counts of items; no money involved).
    fn progress(&self, now: std::time::Instant) -> Option<(u32, u32)> {
        let all = self.inserted;
        if all == 0 {
            return None;
        }
        let whole = self
            .ready_at
            .saturating_duration_since(self.started_at)
            .as_millis()
            .max(1);
        let gone = now
            .min(self.ready_at)
            .saturating_duration_since(self.started_at)
            .as_millis();
        let part = gone.saturating_mul(u128::from(all)) / whole;
        Some((u32::try_from(part).unwrap_or(all).min(all), all))
    }
}

/// A linked file identity.
#[derive(Clone, Debug)]
struct Identity {
    hash: String,
    mask: String,
    account: AccountId,
}

/// A waiting imported item as the Bank Feed review reads it (P26).
#[derive(Clone, Debug)]
pub struct WaitingRow {
    /// Stable while it waits.
    pub key: String,
    /// The bank account.
    pub account: AccountId,
    /// The bank's date.
    pub date: cl_core::CalendarDate,
    /// The amount in the feed sign (money out positive).
    pub amount: rust_decimal::Decimal,
    /// The bank's description.
    pub description: String,
    /// When it was imported.
    pub imported_at: Timestamp,
}

/// The demo feed.
#[derive(Clone, Debug, Default)]
pub struct DemoBankFeed {
    items: Vec<Item>,
    batches: Vec<Batch>,
    identities: Vec<Identity>,
    /// Commits are prepared slowly (`dev.demo_bank_import_slow`).
    slow: bool,
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

    /// Prepares the next commits slowly (`dev.demo_bank_import_slow`), so
    /// Undo and Close can be tried while it runs.
    pub fn set_slow(&mut self, slow: bool) {
        self.slow = slow;
    }

    /// Items waiting in the feed (all accounts).
    #[must_use]
    pub fn waiting(&self) -> usize {
        self.items.iter().filter(|i| i.pending).count()
    }

    /// The waiting items for the Bank Feed review (P26), each under a key
    /// that stays the same while it waits (its batch and row key).
    #[must_use]
    pub fn waiting_rows(&self) -> Vec<WaitingRow> {
        self.items
            .iter()
            .filter(|i| i.pending)
            .map(|i| WaitingRow {
                key: format!("{}/{}", i.batch, i.row_key),
                account: i.account,
                date: i.date,
                amount: i.amount,
                description: i.description.clone(),
                imported_at: i.imported_at,
            })
            .collect()
    }

    /// The review accepted or excluded the item under `key`: it no longer
    /// waits, so an import Undo leaves it alone (P26 plan §5).
    pub fn settle(&mut self, key: &str) {
        if let Some(i) = self
            .items
            .iter_mut()
            .find(|i| i.pending && format!("{}/{}", i.batch, i.row_key) == key)
        {
            i.pending = false;
        }
    }

    /// The last bank date of the latest import into `account` (not
    /// undone), when there was one.
    #[must_use]
    pub fn last_import(&self, account: AccountId) -> Option<cl_core::CalendarDate> {
        let batch = self
            .batches
            .iter()
            .rev()
            .find(|b| b.account == account && !b.undone && b.inserted > 0)?;
        self.items
            .iter()
            .filter(|i| i.batch == batch.id)
            .map(|i| i.date)
            .max()
    }

    /// Previews or commits `req` at `now`. A row dated on or before
    /// `lock` (the company's closing date) is flagged like the engine's
    /// (`closedPeriod`: `row.date <= lockDate`); it is still added.
    ///
    /// # Errors
    /// A commit of a file whose account is linked to another account,
    /// without `relink_identity`, is refused like the engine's 409.
    pub fn import(
        &mut self,
        req: &BankImportRequest,
        commit: bool,
        now: Timestamp,
        lock: Option<cl_core::CalendarDate>,
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
        let mut replacing: Vec<(String, cl_core::CalendarDate, rust_decimal::Decimal, String)> =
            Vec::new();
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
            let same_id_other = seen.filter(|_| by_fitid.is_none());
            let fitid_key = if same_id_other.is_some() {
                None
            } else {
                fitid_key
            };
            let by_row = || {
                mine.iter()
                    .position(|i| i.row_key == k.row_key || i.dedupe_key == k.row_key)
                    .filter(|p| !used.contains(p))
            };
            let (status, reason, existing) =
                if let Some(p) = by_fitid {
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
                } else if let Some(p) = same_id_other.filter(|p| {
                    mine.get(*p).is_some_and(|i| {
                        i.date.days_until(r.date).abs() <= 5
                            && descriptions_match(&i.description, &r.description)
                    })
                }) {
                    (ImportRowStatus::Changed, None, Some(p))
                } else if let Some(p) = mine.iter().enumerate().position(|(i, it)| {
                    !used.contains(&i) && in_range(it, req) && looks_alike(it, r)
                }) {
                    (
                        ImportRowStatus::PossibleDuplicate,
                        Some(ImportRowReason::SimilarExisting),
                        Some(p),
                    )
                } else {
                    let reason = if same_id_other.is_some() {
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
            // Q-P25c: look-alikes are added with a flag, like new rows; a
            // changed row is added only as new ("add").
            let insert = match status {
                ImportRowStatus::New | ImportRowStatus::PossibleDuplicate => {
                    r.include != Some(false)
                }
                ImportRowStatus::Changed => r.changed_action == Some(ChangedAction::Add),
                ImportRowStatus::Duplicate => false,
            };
            let existing_pending = existing.and_then(|p| mine.get(p)).map(|i| i.pending);
            let replace = status == ImportRowStatus::Changed
                && r.changed_action == Some(ChangedAction::Replace)
                && existing_pending == Some(true);
            if replace
                && commit
                && let Some(it) = existing.and_then(|p| mine.get(p))
            {
                replacing.push((
                    it.dedupe_key.clone(),
                    r.date,
                    r.amount,
                    r.description.clone(),
                ));
            }
            if insert && commit {
                to_insert.push(Item {
                    account: req.account,
                    batch,
                    dedupe_key: fitid_key.unwrap_or_else(|| k.row_key.clone()),
                    row_key: k.row_key.clone(),
                    imported_at: now,
                    pending: true,
                    replaced: None,
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
            let closed_period = lock.is_some_and(|l| r.date <= l);
            if closed_period {
                counts.closed_period += 1;
            }
            rows.push(ImportRowResult {
                line: r.line,
                status,
                reason,
                existing_item: None,
                existing_imported_at: existing_item.map(|i| i.imported_at),
                existing_pending: existing_item.map(|i| i.pending),
                closed_period,
                inserted: insert && commit,
                replaced: replace && commit,
                // Q-P25af: what a changed row was (the engine's follow-up).
                existing_amount: existing_item
                    .filter(|_| status == ImportRowStatus::Changed)
                    .and_then(|i| cl_api::wire_decimal(i.amount).ok())
                    .and_then(|a| Money::parse_wire(&a).ok()),
                existing_date: existing_item
                    .filter(|_| status == ImportRowStatus::Changed)
                    .map(|i| i.date),
            });
        }
        let same_file = self
            .batches
            .iter()
            .rev()
            .find(|b| b.account == req.account && b.sha256 == req.source_file_sha256 && !b.undone)
            .map(|b| b.at);
        if commit {
            for (key, date, amount, description) in replacing {
                if let Some(it) = self
                    .items
                    .iter_mut()
                    .find(|i| i.account == req.account && i.dedupe_key == key)
                {
                    let before = (
                        batch,
                        it.date,
                        it.amount,
                        std::mem::take(&mut it.description),
                    );
                    it.date = date;
                    it.amount = amount;
                    it.description = description;
                    it.replaced = Some(before);
                }
            }
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
                started_at: std::time::Instant::now(),
                ready_at: std::time::Instant::now()
                    + if self.slow {
                        DEMO_PREPARING_SLOW
                    } else {
                        DEMO_PREPARING
                    },
                cancelled: false,
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
    /// has passed, cancelled when an undo stopped it.
    #[must_use]
    pub fn post_processing(&self, id: ImportBatchId) -> Option<ImportPostProcessing> {
        let now = std::time::Instant::now();
        self.batches
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.state(now))
    }

    /// Items prepared of all (Q-P25ad), like the engine's batch list.
    #[must_use]
    pub fn progress(&self, id: ImportBatchId) -> Option<(u32, u32)> {
        let now = std::time::Instant::now();
        self.batches
            .iter()
            .find(|b| b.id == id)
            .and_then(|b| b.progress(now))
    }

    /// The imports, newest first, like `GET /banking/feed/import-batches`
    /// (the fields the wizard reads).
    #[must_use]
    pub fn summaries(&self) -> Vec<ImportBatchSummary> {
        let now = std::time::Instant::now();
        self.batches
            .iter()
            .rev()
            .map(|b| ImportBatchSummary {
                id: b.id,
                account: b.account,
                format: Some(b.format),
                amounts_flipped: b.flipped,
                csv_mapping: b.mapping.clone(),
                file_ending_balance: None,
                file_ending_date: None,
                inserted: b.inserted,
                post_processing: Some(b.state(now)),
                undone_at: b.undone.then_some(b.at),
                created_at: Some(b.at),
                progress: b.progress(now),
            })
            .collect()
    }

    /// Undoes a batch: its waiting rows leave the feed. While the batch is
    /// still prepared, the preparation stops first (`Cancelled`), as the
    /// engine's undo does (PR #234).
    ///
    /// # Errors
    /// `AlreadyUndone`.
    pub fn undo(&mut self, id: ImportBatchId) -> Result<ImportUndoOutcome, BankImportRefusal> {
        let b = self
            .batches
            .iter_mut()
            .find(|b| b.id == id && !b.undone)
            .ok_or(BankImportRefusal::AlreadyUndone)?;
        if std::time::Instant::now() < b.ready_at {
            b.cancelled = true;
        }
        b.undone = true;
        let before = self.items.len();
        self.items.retain(|i| !(i.batch == id && i.pending));
        let removed = u32::try_from(before - self.items.len()).unwrap_or(u32::MAX);
        let mut restored = 0;
        for it in &mut self.items {
            if let Some((b, date, amount, description)) = it.replaced.take() {
                if b == id {
                    it.date = date;
                    it.amount = amount;
                    it.description = description;
                    restored += 1;
                } else {
                    it.replaced = Some((b, date, amount, description));
                }
            }
        }
        Ok(ImportUndoOutcome {
            batch: id,
            removed,
            kept: 0,
            restored,
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

    /// The accounts a file identity was linked to: by the account
    /// number's hash, else (like the engine) by its last 4 characters.
    #[must_use]
    pub fn lookup(&self, by: &BankLookupBy) -> Vec<ConnectionMatch> {
        if let BankLookupBy::Account(id) = by {
            let hash = demo_hash(id);
            let found = self.matches(|i| i.hash == hash, BankLookupVia::Hash);
            if !found.is_empty() {
                return found;
            }
            let mask = demo_mask(id);
            return self.matches(|i| i.mask == mask, BankLookupVia::Mask);
        }
        match by {
            BankLookupBy::Mask(m) => self.matches(|i| &i.mask == m, BankLookupVia::Mask),
            BankLookupBy::Account(_) => Vec::new(),
        }
    }

    fn matches(
        &self,
        keep: impl Fn(&Identity) -> bool,
        via: BankLookupVia,
    ) -> Vec<ConnectionMatch> {
        self.identities
            .iter()
            .filter(|i| keep(i))
            .map(|i| ConnectionMatch {
                account: i.account,
                account_name: String::new(),
                mask: Some(i.mask.clone()),
                via,
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::many_single_char_names, reason = "short test names")]
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

    /// Q-P25c (owner confirmed 2026-11-07): a look-alike dated outside the
    /// file's own date range is not compared, so not flagged; inside, it is.
    #[test]
    fn look_alikes_are_compared_only_within_the_files_range() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let mut first = req(vec![row(1, 3, "84.17", "RIVERBEND FUEL #212", "R1")], 'a');
        first.section_start = CalendarDate::from_ymd(2026, 11, 3).ok();
        first.section_end = first.section_start;
        f.import(&first, true, now, None).expect("first");
        let mut csv = row(2, 4, "84.17", "RIVERBEND FUEL #212", "x");
        csv.fitid = None;
        let mut out = req(vec![csv.clone()], 'b');
        out.bank_identity = None;
        out.section_start = CalendarDate::from_ymd(2026, 11, 4).ok();
        out.section_end = out.section_start;
        let o = f.import(&out, false, now, None).expect("out of range");
        assert_eq!(
            o.rows[0].status,
            ImportRowStatus::New,
            "11/03 is before the file"
        );
        let mut inside = req(vec![csv], 'c');
        inside.bank_identity = None;
        inside.section_start = CalendarDate::from_ymd(2026, 11, 2).ok();
        inside.section_end = CalendarDate::from_ymd(2026, 11, 4).ok();
        let o = f.import(&inside, false, now, None).expect("in range");
        assert_eq!(o.rows[0].status, ImportRowStatus::PossibleDuplicate);
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
        f.import(&req(nov, 'e'), true, now, None).expect("nov");
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
        let o = f.import(&req(dec, 'f'), true, now, None).expect("dec");
        assert_eq!((o.counts.new, o.counts.inserted), (3, 3));
        assert!(
            o.rows
                .iter()
                .all(|r| r.reason == Some(ImportRowReason::BankIdReused))
        );
        // A date moved 2 days with the same amount is still the same.
        let shifted = vec![row(1, 13, "10.00", "NOV 1", "1")];
        let o = f
            .import(&req(shifted, 'a'), false, now, None)
            .expect("shift");
        assert_eq!(o.counts.duplicate, 1);
    }

    /// The engine's order: the same bank id with another amount, a date
    /// within 5 days and a matching description is changed; replace
    /// updates the waiting item and undo restores it; add inserts it.
    #[test]
    fn a_changed_row_replaces_and_undo_restores() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let first = f
            .import(
                &req(vec![row(1, 20, "80.00", "HARDWARE STORE 12", "CH")], 'a'),
                true,
                now,
                None,
            )
            .expect("first");
        std::thread::sleep(DEMO_PREPARING);
        let mut changed = row(1, 21, "85.00", "HARDWARE STORE 12", "CH");
        let p = f
            .import(&req(vec![changed.clone()], 'b'), false, now, None)
            .expect("p");
        assert_eq!(p.rows[0].status, ImportRowStatus::Changed);
        assert_eq!(p.rows[0].existing_pending, Some(true));
        changed.changed_action = Some(ChangedAction::Replace);
        let r = f
            .import(&req(vec![changed.clone()], 'b'), true, now, None)
            .expect("r");
        assert!(r.rows[0].replaced && !r.rows[0].inserted);
        assert_eq!(f.waiting(), 1);
        std::thread::sleep(DEMO_PREPARING);
        let u = f.undo(r.batch.expect("batch")).expect("undo");
        assert_eq!((u.removed, u.restored), (0, 1));
        assert!(
            f.items
                .iter()
                .any(|i| i.amount == "80.00".parse().expect("d"))
        );
        changed.changed_action = Some(ChangedAction::Add);
        let a = f
            .import(&req(vec![changed], 'c'), true, now, None)
            .expect("add");
        assert_eq!(a.counts.inserted, 1);
        assert_eq!(f.waiting(), 2);
        // Another description: the bank reused the id.
        let other = row(1, 22, "99.00", "SOMETHING ELSE", "CH");
        let o = f
            .import(&req(vec![other], 'd'), false, now, None)
            .expect("o");
        assert_eq!(o.rows[0].reason, Some(ImportRowReason::BankIdReused));
        let _ = first;
    }

    /// The engine's rule: a row dated on or before the closing date is
    /// flagged (and counted), still added; with no closing date none is.
    #[test]
    fn rows_on_or_before_the_closing_date_are_flagged() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let rows = vec![
            row(1, 9, "-1.00", "ON THE DAY", "C1"),
            row(2, 10, "-2.00", "AFTER", "C2"),
            row(3, 8, "-3.00", "BEFORE", "C3"),
        ];
        let lock = CalendarDate::from_ymd(2026, 11, 9).ok();
        let o = f
            .import(&req(rows.clone(), 'a'), false, now, lock)
            .expect("preview");
        let flags: Vec<bool> = o.rows.iter().map(|r| r.closed_period).collect();
        assert_eq!(flags, vec![true, false, true]);
        assert_eq!(o.counts.closed_period, 2);
        assert_eq!(o.counts.new, 3);
        let c = f
            .import(&req(rows.clone(), 'a'), true, now, lock)
            .expect("send");
        assert_eq!(c.counts.inserted, 3);
        let none = f.import(&req(rows, 'b'), false, now, None).expect("none");
        assert_eq!(none.counts.closed_period, 0);
    }

    /// The link is kept as a hash and a mask only; a file linked to
    /// another account is refused on commit unless relinked; preparation
    /// takes a moment and an undo while it runs stops it.
    #[test]
    fn links_relinks_and_prepares() {
        let mut f = DemoBankFeed::default();
        let now = crate::company::demo::DemoState::now();
        let r = req(vec![row(1, 2, "5.00", "SHOP", "A")], 'b');
        let o = f.import(&r, true, now, None).expect("first");
        assert_eq!(o.post_processing, Some(ImportPostProcessing::Scheduled));
        let batch = o.batch.expect("batch");
        assert_eq!(f.progress(batch).map(|(_, all)| all), Some(1));
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
            f.import(&other, false, now, None)
                .expect("preview")
                .identity_linked_elsewhere
                .is_some()
        );
        assert!(matches!(
            f.import(&other, true, now, None),
            Err(BankImportRefusal::LinkedElsewhere(_))
        ));
        other.relink_identity = true;
        assert!(f.import(&other, true, now, None).is_ok());
        std::thread::sleep(DEMO_PREPARING);
        assert_eq!(f.post_processing(batch), Some(ImportPostProcessing::Done));
        assert_eq!(f.progress(batch), Some((1, 1)));
        assert!(f.undo(batch).is_ok());
        assert_eq!(f.post_processing(batch), Some(ImportPostProcessing::Done));
        assert_eq!(f.undo(batch), Err(BankImportRefusal::AlreadyUndone));
    }

    /// PR #234: an undo while the preparation runs stops it and undoes;
    /// the batch then reads `Cancelled`, in the list too.
    #[test]
    fn undo_while_preparing_cancels() {
        let mut f = DemoBankFeed::default();
        f.set_slow(true);
        let now = crate::company::demo::DemoState::now();
        let r = req(vec![row(1, 2, "5.00", "SHOP", "A")], 'b');
        let batch = f
            .import(&r, true, now, None)
            .expect("send")
            .batch
            .expect("batch");
        assert_eq!(
            f.post_processing(batch),
            Some(ImportPostProcessing::Scheduled)
        );
        let listed = f.summaries();
        assert_eq!(
            listed[0].post_processing,
            Some(ImportPostProcessing::Scheduled)
        );
        assert!(listed[0].undone_at.is_none());
        let u = f.undo(batch).expect("undo");
        assert_eq!(u.removed, 1);
        assert_eq!(f.waiting(), 0);
        assert_eq!(
            f.post_processing(batch),
            Some(ImportPostProcessing::Cancelled)
        );
        assert!(f.summaries()[0].undone_at.is_some());
        f.reset();
        assert!(!f.slow);
    }
}
