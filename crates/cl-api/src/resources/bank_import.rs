//! Bank file imports (P25, U-06 `bank-import-batch-v1`).
//!
//! The desktop parses the owner's bank file (`cl-import`) and sends one
//! account section as a batch. The engine computes everything else: the
//! dedupe keys, each row's status, the closed-period flag and the balance
//! check. This module only carries them.
//!
//! - [`BankImport::preview`]: `POST /banking/feed/import-batch` with
//!   `dryRun: true`. It changes nothing and must carry **no**
//!   `Idempotency-Key` (the engine answers 400 `IDEMPOTENCY_KEY_NOT_ALLOWED`),
//!   so it is sent with the call's `no_key` flag and retried like a read.
//! - [`BankImport::commit`]: the same route with `dryRun: false` and the
//!   key the screen made at the first Send (the engine answers 400
//!   `IDEMPOTENCY_KEY_REQUIRED` without one). A lost answer is replayed
//!   by the engine with `Idempotent-Replayed: true`, even when it is large.
//!   The Import time class gives it one attempt of up to 120 s.
//! - [`BankImport::batches`], [`BankImport::undo`] and
//!   [`BankImport::lookup`]: earlier imports, undoing one, and finding the
//!   connection a file's account was linked to.
//!
//! Money on the wire is an exact decimal string. Row amounts use the feed
//! sign (positive = money out); the file's ending balance uses the
//! holder's sign. Nothing here does arithmetic on amounts.
//!
//! `Debug` output names ids, line numbers and counts only: never amounts,
//! descriptions, masks or account numbers (no PII in logs).

use std::fmt;

use cl_core::ids::{AccountId, parse_uuid};
use cl_core::{CalendarDate, Money, Timestamp};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, require};
use crate::resources::feed::FeedItemId;
use crate::resources::operations::{
    BANK_CONNECTION_LOOKUP, FEED_IMPORT_BATCH, FEED_IMPORT_BATCH_UNDO, FEED_IMPORT_BATCHES,
};
use crate::session::Session;
use crate::wire::Capabilities;

/// The engine's row cap per batch (Q-P25d).
pub const MAX_ROWS: usize = 20_000;
/// The largest request body the desktop sends (Q-P25d): under the engine's
/// global 10 MB limit with room to spare. Over it the owner is asked to
/// download a shorter date range.
pub const MAX_BODY_BYTES: usize = 9_500_000;
/// Description limit (characters).
pub const MAX_DESCRIPTION: usize = 500;
/// Memo limit (characters).
pub const MAX_MEMO: usize = 1_000;
/// The largest check number sent as a number (9 digits).
pub const MAX_CHECK_NUMBER: u32 = 999_999_999;
/// The asset detail types a bank file goes into with
/// `bank-import-account-types-v1` (U-175; the engine's `BANK_DETAIL_TYPES`).
/// Without it only `bank`.
pub const BANK_REGISTER_DETAIL_TYPES: [&str; 5] =
    ["bank", "checking", "savings", "cash", "petty_cash"];

/// Whether an asset account with `detail` takes a bank file, given whether
/// the engine has `bank-import-account-types-v1`.
#[must_use]
pub fn takes_bank_file(detail: &str, all_bank_types: bool) -> bool {
    if all_bank_types {
        BANK_REGISTER_DETAIL_TYPES.contains(&detail)
    } else {
        detail == "bank"
    }
}

/// The kind of bank file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BankFileFormat {
    /// OFX 1.x (SGML) or 2.x (XML).
    Ofx,
    /// Quicken's OFX flavour.
    Qfx,
    /// Quicken Interchange Format.
    Qif,
    /// A bank CSV export.
    Csv,
}

impl BankFileFormat {
    /// The wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            BankFileFormat::Ofx => "ofx",
            BankFileFormat::Qfx => "qfx",
            BankFileFormat::Qif => "qif",
            BankFileFormat::Csv => "csv",
        }
    }

    fn parse(s: &str) -> Option<BankFileFormat> {
        match s {
            "ofx" => Some(BankFileFormat::Ofx),
            "qfx" => Some(BankFileFormat::Qfx),
            "qif" => Some(BankFileFormat::Qif),
            "csv" => Some(BankFileFormat::Csv),
            _ => None,
        }
    }
}

/// What to do with a row whose bank id exists with another amount.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChangedAction {
    /// Replace the waiting item (only while it is untouched; audited).
    Replace,
    /// Add the row as a new item.
    Add,
}

impl ChangedAction {
    const fn as_wire(self) -> &'static str {
        match self {
            ChangedAction::Replace => "replace",
            ChangedAction::Add => "add",
        }
    }
}

/// One row of the batch, as the file has it after the sign was settled.
#[derive(Clone, PartialEq, Eq)]
pub struct BankImportRow {
    /// Line (or entry) number in the file; echoed in results and errors.
    pub line: u32,
    /// The bank's date.
    pub date: CalendarDate,
    /// Feed sign: positive = money out. At most 2 decimals and 12 integer
    /// digits; never zero (zero rows are skipped before sending).
    pub amount: Decimal,
    /// The bank's description (OFX: NAME and MEMO assembled as the engine's
    /// golden file pins).
    pub description: String,
    /// Not part of any key.
    pub memo: Option<String>,
    /// Up to 9 digits.
    pub check_number: Option<u32>,
    /// The bank's transaction id (OFX FITID).
    pub fitid: Option<String>,
    /// `Some(false)` skips a new row; `Some(true)` adds a possible duplicate
    /// ("Add anyway").
    pub include: Option<bool>,
    /// For a changed row.
    pub changed_action: Option<ChangedAction>,
}

impl fmt::Debug for BankImportRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BankImportRow")
            .field("line", &self.line)
            .field("date", &self.date)
            .field("include", &self.include)
            .field("changed_action", &self.changed_action)
            .finish_non_exhaustive()
    }
}

/// The account a file names, sent only to the local engine (Q-P25u): the
/// engine keeps the company's link key, makes the keyed hash and the mask,
/// and stores only those. Never logged, shown or kept by the desktop
/// beyond the open file; `Debug` prints nothing of it.
#[derive(Clone, PartialEq, Eq)]
pub struct BankIdentity {
    /// The routing number (`BANKID`), when the file has one.
    pub bank_id: Option<String>,
    /// The account number (`ACCTID`), trimmed.
    pub acct_id: String,
}

impl BankIdentity {
    fn valid(&self) -> bool {
        !self.acct_id.trim().is_empty()
            && !too_long(&self.acct_id, MAX_ACCT_ID)
            && self
                .bank_id
                .as_deref()
                .is_none_or(|b| !b.trim().is_empty() && !too_long(b, MAX_ACCT_ID))
    }

    fn wire(&self) -> Value {
        let mut m = Map::new();
        if let Some(b) = &self.bank_id {
            m.insert("bankId".into(), json!(b));
        }
        m.insert("acctId".into(), json!(self.acct_id));
        Value::Object(m)
    }
}

impl fmt::Debug for BankIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BankIdentity(..)")
    }
}

/// The longest CSV mapping the engine keeps, in characters of JSON.
const MAX_CSV_MAPPING: usize = 16_384;

/// The longest `BANKID` or `ACCTID` sent (OFX allows 22 and 9; card
/// numbers and odd exports stay well under this).
const MAX_ACCT_ID: usize = 64;

/// One account section of a bank file, ready to send.
#[derive(Clone, PartialEq)]
pub struct BankImportRequest {
    /// The bank or card account in the books.
    pub account: AccountId,
    /// What the file is.
    pub format: BankFileFormat,
    /// Base file name only (no folders).
    pub file_name: String,
    /// SHA-256 of the file's bytes, lower-case hex.
    pub source_file_sha256: String,
    /// Which section of the file (0-based).
    pub section_index: u32,
    /// The section's first and last dates, when known.
    pub section_start: Option<CalendarDate>,
    /// See `section_start`.
    pub section_end: Option<CalendarDate>,
    /// The owner (or a sign check) flipped the file's signs.
    pub amounts_flipped: bool,
    /// A CSV's column mapping, stored with the batch (opaque here).
    pub csv_mapping: Option<Value>,
    /// Link this file's identity even though it is linked elsewhere.
    pub relink_identity: bool,
    /// The file's account identity.
    pub bank_identity: Option<BankIdentity>,
    /// The file's ending balance in the holder's sign (after any flip).
    pub file_ending_balance: Option<Decimal>,
    /// The date of that balance.
    pub file_ending_date: Option<CalendarDate>,
    /// The rows.
    pub rows: Vec<BankImportRow>,
}

impl fmt::Debug for BankImportRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BankImportRequest")
            .field("account", &self.account)
            .field("format", &self.format)
            .field("section_index", &self.section_index)
            .field("rows", &self.rows.len())
            .finish_non_exhaustive()
    }
}

/// An exact decimal as the engine reads it: at most 12 integer digits and
/// 2 decimals (`^-?\d{1,12}(\.\d{1,2})?$`), printed with 2 decimals.
///
/// # Errors
/// `NotWholeCents` for a third decimal; `InvalidValue` for 13 or more
/// integer digits.
pub fn wire_decimal(d: Decimal) -> Result<String, LocalValidation> {
    let n = d.normalize();
    if n.scale() > 2 {
        return Err(LocalValidation::NotWholeCents);
    }
    let limit = Decimal::from(1_000_000_000_000_i64);
    if n.abs() >= limit {
        return Err(LocalValidation::InvalidValue);
    }
    let mut s = n.to_string();
    match s.find('.') {
        None => s.push_str(".00"),
        Some(i) if s.len() - i == 2 => s.push('0'),
        Some(_) => {}
    }
    Ok(s)
}

fn too_long(s: &str, max: usize) -> bool {
    s.chars().count() > max
}

fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl BankImportRequest {
    /// The request body, as sent. A commit and its retries send the same
    /// bytes (built from the same request).
    ///
    /// # Errors
    /// A [`LocalValidation`] for anything the engine would refuse that the
    /// desktop can see first: too many rows, a zero or too-precise amount,
    /// a description or memo over its limit, a check number over 9 digits,
    /// a malformed hash or file name.
    pub fn body(&self, dry_run: bool) -> Result<Value, LocalValidation> {
        if self.rows.len() > MAX_ROWS {
            return Err(LocalValidation::TooManyLines);
        }
        if self.file_name.is_empty()
            || too_long(&self.file_name, 255)
            || self.file_name.contains(['/', '\\'])
            || !is_hex64(&self.source_file_sha256)
        {
            return Err(LocalValidation::InvalidValue);
        }
        let mut rows = Vec::with_capacity(self.rows.len());
        for r in &self.rows {
            if r.amount.is_zero() {
                return Err(LocalValidation::NonPositiveAmount);
            }
            if too_long(&r.description, MAX_DESCRIPTION)
                || r.memo.as_deref().is_some_and(|m| too_long(m, MAX_MEMO))
                || r.check_number.is_some_and(|n| n > MAX_CHECK_NUMBER)
                || r.fitid.as_deref().is_some_and(|f| too_long(f, 255))
                || r.line == 0
            {
                return Err(LocalValidation::InvalidValue);
            }
            let mut row = Map::new();
            row.insert("line".into(), json!(r.line));
            row.insert("date".into(), json!(r.date.to_wire()));
            row.insert("amount".into(), json!(wire_decimal(r.amount)?));
            row.insert("description".into(), json!(r.description));
            if let Some(m) = &r.memo {
                row.insert("memo".into(), json!(m));
            }
            if let Some(n) = r.check_number {
                row.insert("checkNumber".into(), json!(n));
            }
            if let Some(f) = &r.fitid {
                row.insert("fitid".into(), json!(f));
            }
            if let Some(i) = r.include {
                row.insert("include".into(), json!(i));
            }
            if let Some(a) = r.changed_action {
                row.insert("changedAction".into(), json!(a.as_wire()));
            }
            rows.push(Value::Object(row));
        }
        let mut body = Map::new();
        body.insert("accountId".into(), json!(self.account.to_string()));
        body.insert("format".into(), json!(self.format.as_wire()));
        body.insert("fileName".into(), json!(self.file_name));
        body.insert("sourceFileSha256".into(), json!(self.source_file_sha256));
        body.insert("sectionIndex".into(), json!(self.section_index));
        if let Some(d) = self.section_start {
            body.insert("sectionStart".into(), json!(d.to_wire()));
        }
        if let Some(d) = self.section_end {
            body.insert("sectionEnd".into(), json!(d.to_wire()));
        }
        body.insert("amountsFlipped".into(), json!(self.amounts_flipped));
        if let Some(m) = &self.csv_mapping {
            // The engine keeps at most 16,384 characters of it as JSON.
            if !m.is_object() || m.to_string().chars().count() > MAX_CSV_MAPPING {
                return Err(LocalValidation::InvalidValue);
            }
            body.insert("csvMapping".into(), m.clone());
        }
        body.insert("dryRun".into(), json!(dry_run));
        if self.relink_identity {
            body.insert("relinkIdentity".into(), json!(true));
        }
        if let Some(id) = &self.bank_identity {
            if !id.valid() {
                return Err(LocalValidation::InvalidValue);
            }
            body.insert("bankIdentity".into(), id.wire());
        }
        if let (Some(b), Some(d)) = (self.file_ending_balance, self.file_ending_date) {
            body.insert("fileEndingBalance".into(), json!(wire_decimal(b)?));
            body.insert("fileEndingDate".into(), json!(d.to_wire()));
        }
        body.insert("rows".into(), Value::Array(rows));
        Ok(Value::Object(body))
    }

    /// The body's bytes, refused locally over [`MAX_BODY_BYTES`].
    ///
    /// # Errors
    /// As [`BankImportRequest::body`]; `BatchFileSize` over the size limit.
    pub fn body_bytes(&self, dry_run: bool) -> Result<Vec<u8>, LocalValidation> {
        let bytes =
            serde_json::to_vec(&self.body(dry_run)?).map_err(|_| LocalValidation::InvalidValue)?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(LocalValidation::BatchFileSize);
        }
        Ok(bytes)
    }
}

/// A row's status as the engine judged it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportRowStatus {
    /// Not in the feed yet.
    New,
    /// Already imported (skipped).
    Duplicate,
    /// Looks like one already in the feed (skipped unless "Add anyway").
    PossibleDuplicate,
    /// The bank's id is in the feed with another amount.
    Changed,
}

/// Why a row has its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportRowReason {
    /// The same bank transaction id.
    SameBankId,
    /// The same date, amount and description.
    SameRow,
    /// The bank id repeats in this file, so it was ignored.
    RepeatedBankId,
    /// The bank reused an id for another transaction, so it was ignored.
    BankIdReused,
    /// A similar transaction is already in the feed.
    SimilarExisting,
}

/// One row's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRowResult {
    /// The row's line.
    pub line: u32,
    /// Its status.
    pub status: ImportRowStatus,
    /// Why, when the engine said.
    pub reason: Option<ImportRowReason>,
    /// The feed item it matched.
    pub existing_item: Option<FeedItemId>,
    /// When that item was imported.
    pub existing_imported_at: Option<Timestamp>,
    /// That item still waits in the feed (untouched).
    pub existing_pending: Option<bool>,
    /// Dated on or before the closing date.
    pub closed_period: bool,
    /// Added to the feed by this commit.
    pub inserted: bool,
    /// Replaced the waiting item (a changed row with "replace").
    pub replaced: bool,
    /// The matched item's amount in the feed sign (money out positive) and
    /// its date, for a changed row's "was / now" (Q-P25af). Read when the
    /// engine sends `existingAmount`/`existingDate`; an engine that doesn't
    /// (or a value that doesn't read) leaves them empty.
    pub existing_amount: Option<Money>,
    /// See `existing_amount`.
    pub existing_date: Option<CalendarDate>,
}

/// The counts of a preview or commit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BankImportCounts {
    /// Rows sent.
    pub in_file: u32,
    /// New rows.
    pub new: u32,
    /// Already imported.
    pub duplicate: u32,
    /// Possible duplicates.
    pub possible_duplicate: u32,
    /// Changed rows.
    pub changed: u32,
    /// Rows in a closed period.
    pub closed_period: u32,
    /// Rows added (commit only).
    pub inserted: u32,
}

/// The engine's balance check (display only).
#[derive(Clone, PartialEq, Eq)]
pub struct ImportBalanceCheck {
    /// The file's balance (holder's sign).
    pub file_balance: Money,
    /// Its date.
    pub file_date: CalendarDate,
    /// The books' balance on that date (owed, for a card).
    pub books_at_date: Money,
    /// Items waiting in the feed dated on or before it.
    pub pending_total: Money,
    /// New rows dated on or before it.
    pub new_total: Money,
    /// New and waiting rows that look already entered in the register.
    pub likely_in_books_count: u32,
    /// Their total.
    pub likely_in_books_total: Money,
    /// The books once every row is accepted.
    pub projected: Money,
    /// File minus projected (0.00 when they agree).
    pub difference: Money,
}

impl fmt::Debug for ImportBalanceCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportBalanceCheck")
            .field("file_date", &self.file_date)
            .field("likely_in_books_count", &self.likely_in_books_count)
            .finish_non_exhaustive()
    }
}

/// After-commit preparation of the new items (no AI, no posting).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportPostProcessing {
    /// Waiting or running.
    Scheduled,
    /// Finished.
    Done,
    /// Failed (the items are still in the feed).
    Failed,
    /// An undo stopped it (`bank-import-progress-v1`): the batch is undone
    /// and its suggestions were not all prepared.
    Cancelled,
}

impl ImportPostProcessing {
    fn parse(s: &str) -> Option<ImportPostProcessing> {
        match s {
            "scheduled" => Some(ImportPostProcessing::Scheduled),
            "done" => Some(ImportPostProcessing::Done),
            "failed" => Some(ImportPostProcessing::Failed),
            "cancelled" => Some(ImportPostProcessing::Cancelled),
            _ => None,
        }
    }
}

/// An earlier import of the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImportBatchId(uuid::Uuid);

impl ImportBatchId {
    /// Parses a batch id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<ImportBatchId, ApiError> {
        parse_uuid(s)
            .map(ImportBatchId)
            .map_err(|_| ApiError::Decode {
                what: "import batch id",
            })
    }
}

impl fmt::Display for ImportBatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The account a file's identity is linked to elsewhere.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkedElsewhere {
    /// That account's name in the books.
    pub account_name: String,
    /// The file's mask.
    pub mask: String,
}

impl fmt::Debug for LinkedElsewhere {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LinkedElsewhere(..)")
    }
}

/// A preview's or commit's answer.
#[derive(Clone, PartialEq)]
pub struct BankImportOutcome {
    /// The batch (commit only).
    pub batch: Option<ImportBatchId>,
    /// This same file was imported before, at this time.
    pub same_file_imported_at: Option<Timestamp>,
    /// The file's identity is linked to another account (preview only).
    pub identity_linked_elsewhere: Option<LinkedElsewhere>,
    /// The items added.
    pub inserted_items: Vec<FeedItemId>,
    /// Preparation of the added items (commit only).
    pub post_processing: Option<ImportPostProcessing>,
    /// Every row's answer, in the order sent.
    pub rows: Vec<ImportRowResult>,
    /// The counts.
    pub counts: BankImportCounts,
    /// The total of the rows added.
    pub inserted_total: Money,
    /// The balance check, when the file had a balance.
    pub balance_check: Option<ImportBalanceCheck>,
    /// The engine replayed an earlier answer for this key.
    pub replayed: bool,
}

impl fmt::Debug for BankImportOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BankImportOutcome")
            .field("batch", &self.batch)
            .field("counts", &self.counts)
            .field("post_processing", &self.post_processing)
            .field("replayed", &self.replayed)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRow {
    line: u32,
    status: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    existing_item_id: Option<String>,
    #[serde(default)]
    existing_imported_at: Option<String>,
    #[serde(default)]
    existing_pending: Option<bool>,
    #[serde(default)]
    closed_period: bool,
    #[serde(default)]
    inserted: bool,
    #[serde(default)]
    replaced: Option<bool>,
    #[serde(default)]
    existing_amount: Option<Value>,
    #[serde(default)]
    existing_date: Option<Value>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct WireCounts {
    in_file: u32,
    new: u32,
    duplicate: u32,
    possible_duplicate: u32,
    changed: u32,
    closed_period: u32,
    inserted: u32,
}

#[derive(Deserialize)]
struct WireLikely {
    count: u32,
    total: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBalance {
    file_balance: String,
    file_date: String,
    books_at_date: String,
    pending_total: String,
    new_total: String,
    likely_in_books: WireLikely,
    projected: String,
    difference: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLinked {
    account_name: String,
    mask: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireOutcome {
    #[serde(default)]
    batch_id: Option<String>,
    #[serde(default)]
    same_file_imported_at: Option<String>,
    #[serde(default)]
    identity_linked_elsewhere: Option<WireLinked>,
    #[serde(default)]
    inserted_item_ids: Vec<String>,
    #[serde(default)]
    post_processing: Option<String>,
    rows: Vec<WireRow>,
    #[serde(default)]
    counts: WireCounts,
}

const WHAT: &str = "bank import";

fn money(s: &str) -> Result<Money, ApiError> {
    Money::parse_wire(s).map_err(|_| ApiError::Decode { what: WHAT })
}

fn date(s: &str) -> Result<CalendarDate, ApiError> {
    CalendarDate::parse_wire(s.get(..10).unwrap_or(s)).map_err(|_| ApiError::Decode { what: WHAT })
}

fn status(s: &str) -> Result<ImportRowStatus, ApiError> {
    match s {
        "new" => Ok(ImportRowStatus::New),
        "duplicate" => Ok(ImportRowStatus::Duplicate),
        "possible_duplicate" => Ok(ImportRowStatus::PossibleDuplicate),
        "changed" => Ok(ImportRowStatus::Changed),
        _ => Err(ApiError::Decode { what: WHAT }),
    }
}

fn reason(s: &str) -> Option<ImportRowReason> {
    match s {
        "same_bank_id" => Some(ImportRowReason::SameBankId),
        "same_row" => Some(ImportRowReason::SameRow),
        "repeated_bank_id" => Some(ImportRowReason::RepeatedBankId),
        "bank_id_reused" => Some(ImportRowReason::BankIdReused),
        "similar_existing" => Some(ImportRowReason::SimilarExisting),
        _ => None,
    }
}

/// Reads a preview or commit answer.
///
/// # Errors
/// `Decode` when the answer can't be read (an amount that is not an exact
/// decimal string included).
pub fn outcome_from(body: &[u8], replayed: bool) -> Result<BankImportOutcome, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let inserted_total = v
        .get("insertedTotal")
        .and_then(Value::as_str)
        .ok_or(ApiError::Decode { what: WHAT })
        .and_then(money)?;
    let balance_check = match v.get("balanceCheck") {
        None | Some(Value::Null) => None,
        Some(b) => {
            let b: WireBalance =
                serde_json::from_value(b.clone()).map_err(|_| ApiError::Decode { what: WHAT })?;
            Some(ImportBalanceCheck {
                file_balance: money(&b.file_balance)?,
                file_date: date(&b.file_date)?,
                books_at_date: money(&b.books_at_date)?,
                pending_total: money(&b.pending_total)?,
                new_total: money(&b.new_total)?,
                likely_in_books_count: b.likely_in_books.count,
                likely_in_books_total: money(&b.likely_in_books.total)?,
                projected: money(&b.projected)?,
                difference: money(&b.difference)?,
            })
        }
    };
    let w: WireOutcome = serde_json::from_value(v).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut rows = Vec::with_capacity(w.rows.len());
    for r in w.rows {
        rows.push(ImportRowResult {
            line: r.line,
            status: status(&r.status)?,
            reason: r.reason.as_deref().and_then(reason),
            existing_item: r
                .existing_item_id
                .as_deref()
                .map(FeedItemId::parse)
                .transpose()?,
            existing_imported_at: r
                .existing_imported_at
                .as_deref()
                .and_then(|t| Timestamp::parse_wire(t).ok()),
            existing_pending: r.existing_pending,
            closed_period: r.closed_period,
            inserted: r.inserted,
            replaced: r.replaced.unwrap_or(false),
            existing_amount: r
                .existing_amount
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|a| Money::parse_wire(a).ok()),
            existing_date: r
                .existing_date
                .as_ref()
                .and_then(Value::as_str)
                .and_then(|d| date(d).ok()),
        });
    }
    Ok(BankImportOutcome {
        batch: w
            .batch_id
            .as_deref()
            .map(ImportBatchId::parse)
            .transpose()?,
        same_file_imported_at: w
            .same_file_imported_at
            .as_deref()
            .and_then(|t| Timestamp::parse_wire(t).ok()),
        identity_linked_elsewhere: w.identity_linked_elsewhere.map(|l| LinkedElsewhere {
            account_name: l.account_name,
            mask: l.mask,
        }),
        inserted_items: w
            .inserted_item_ids
            .iter()
            .map(|s| FeedItemId::parse(s))
            .collect::<Result<_, _>>()?,
        post_processing: w
            .post_processing
            .as_deref()
            .and_then(ImportPostProcessing::parse),
        rows,
        counts: BankImportCounts {
            in_file: w.counts.in_file,
            new: w.counts.new,
            duplicate: w.counts.duplicate,
            possible_duplicate: w.counts.possible_duplicate,
            changed: w.counts.changed,
            closed_period: w.counts.closed_period,
            inserted: w.counts.inserted,
        },
        inserted_total,
        balance_check,
        replayed,
    })
}

/// An earlier import (newest first in [`BankImport::batches`]).
#[derive(Clone, PartialEq)]
pub struct ImportBatchSummary {
    /// The batch.
    pub id: ImportBatchId,
    /// The account it went into.
    pub account: AccountId,
    /// The file's kind.
    pub format: Option<BankFileFormat>,
    /// The file's signs were flipped.
    pub amounts_flipped: bool,
    /// A CSV's column mapping (opaque).
    pub csv_mapping: Option<Value>,
    /// The file's ending balance and date, when it had one.
    pub file_ending_balance: Option<Money>,
    /// See `file_ending_balance`.
    pub file_ending_date: Option<CalendarDate>,
    /// Rows added.
    pub inserted: u32,
    /// Preparation of the added items.
    pub post_processing: Option<ImportPostProcessing>,
    /// When it was undone.
    pub undone_at: Option<Timestamp>,
    /// When it was imported.
    pub created_at: Option<Timestamp>,
    /// How far the preparation is: items prepared and items in all, when
    /// the engine says (Q-P25ad; an older engine doesn't, and a value
    /// that isn't a count is ignored).
    pub progress: Option<(u32, u32)>,
}

impl fmt::Debug for ImportBatchSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportBatchSummary")
            .field("id", &self.id)
            .field("format", &self.format)
            .field("inserted", &self.inserted)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBatch {
    id: String,
    account_id: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    amounts_flipped: bool,
    #[serde(default)]
    csv_mapping: Option<Value>,
    #[serde(default)]
    file_ending_balance: Option<String>,
    #[serde(default)]
    file_ending_date: Option<String>,
    #[serde(default)]
    counts: WireCounts,
    #[serde(default)]
    post_processing: Option<String>,
    #[serde(default)]
    undone_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    processed: Option<Value>,
    #[serde(default)]
    total: Option<Value>,
}

/// A preparation's progress (Q-P25ad), read tolerantly: both counts
/// present, whole and fitting, the total not zero, else none.
fn progress(processed: Option<&Value>, total: Option<&Value>) -> Option<(u32, u32)> {
    let count = |v: Option<&Value>| v?.as_u64().and_then(|n| u32::try_from(n).ok());
    let (done, all) = (count(processed)?, count(total)?);
    (all > 0).then_some((done.min(all), all))
}

/// Reads a batch list answer (`{batches: […]}`).
///
/// # Errors
/// `Decode` when it can't be read.
pub fn batches_from(body: &[u8]) -> Result<Vec<ImportBatchSummary>, ApiError> {
    #[derive(Deserialize)]
    struct W {
        batches: Vec<WireBatch>,
    }
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    w.batches
        .into_iter()
        .map(|b| {
            Ok(ImportBatchSummary {
                id: ImportBatchId::parse(&b.id)?,
                account: b
                    .account_id
                    .parse()
                    .map_err(|_| ApiError::Decode { what: WHAT })?,
                format: b.format.as_deref().and_then(BankFileFormat::parse),
                amounts_flipped: b.amounts_flipped,
                csv_mapping: b.csv_mapping.filter(|m| !m.is_null()),
                file_ending_balance: b.file_ending_balance.as_deref().map(money).transpose()?,
                file_ending_date: b.file_ending_date.as_deref().map(date).transpose()?,
                inserted: b.counts.inserted,
                post_processing: b
                    .post_processing
                    .as_deref()
                    .and_then(ImportPostProcessing::parse),
                undone_at: b
                    .undone_at
                    .as_deref()
                    .and_then(|t| Timestamp::parse_wire(t).ok()),
                created_at: b
                    .created_at
                    .as_deref()
                    .and_then(|t| Timestamp::parse_wire(t).ok()),
                progress: progress(b.processed.as_ref(), b.total.as_ref()),
            })
        })
        .collect()
}

/// An undo's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportUndoOutcome {
    /// The batch.
    pub batch: ImportBatchId,
    /// Rows taken out of the feed.
    pub removed: u32,
    /// Rows kept (already used).
    pub kept: u32,
    /// Waiting items a `changed` row had replaced, put back as they were.
    pub restored: u32,
}

/// Reads an undo answer.
///
/// # Errors
/// `Decode` when it can't be read.
pub fn undo_from(body: &[u8]) -> Result<ImportUndoOutcome, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct W {
        batch_id: String,
        #[serde(default)]
        removed: u32,
        #[serde(default)]
        kept: u32,
        #[serde(default)]
        restored: u32,
    }
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    Ok(ImportUndoOutcome {
        batch: ImportBatchId::parse(&w.batch_id)?,
        removed: w.removed,
        kept: w.kept,
        restored: w.restored,
    })
}

/// How a lookup found the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BankLookupVia {
    /// By the account number, through the engine's keyed hash saved at an
    /// earlier import (certain).
    Hash,
    /// By the masked number (a suggestion).
    Mask,
}

/// One account a file may belong to.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionMatch {
    /// The account in the books.
    pub account: AccountId,
    /// Its name.
    pub account_name: String,
    /// The mask that was linked.
    pub mask: Option<String>,
    /// How it was found.
    pub via: BankLookupVia,
}

impl fmt::Debug for ConnectionMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionMatch")
            .field("account", &self.account)
            .field("via", &self.via)
            .finish_non_exhaustive()
    }
}

/// Reads a lookup answer (`{matches: […]}`).
///
/// # Errors
/// `Decode` when it can't be read.
pub fn matches_from(body: &[u8]) -> Result<Vec<ConnectionMatch>, ApiError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct M {
        account_id: String,
        #[serde(default)]
        account_name: String,
        #[serde(default)]
        mask: Option<String>,
        via: String,
    }
    #[derive(Deserialize)]
    struct W {
        matches: Vec<M>,
    }
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    Ok(w.matches
        .into_iter()
        .filter_map(|m| {
            Some(ConnectionMatch {
                account: m.account_id.parse().ok()?,
                account_name: m.account_name,
                mask: m.mask,
                via: match m.via.as_str() {
                    "hash" => BankLookupVia::Hash,
                    "mask" => BankLookupVia::Mask,
                    _ => return None,
                },
            })
        })
        .collect())
}

/// What to look a file's account up by.
#[derive(Clone, PartialEq, Eq)]
pub enum BankLookupBy {
    /// The file's account number: the engine hashes it with the company's
    /// link key (Q-P25u).
    Account(BankIdentity),
    /// The mask text (a suggestion).
    Mask(String),
}

impl fmt::Debug for BankLookupBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BankLookupBy::Account(_) => "BankLookupBy::Account(..)",
            BankLookupBy::Mask(_) => "BankLookupBy::Mask(..)",
        })
    }
}

/// The import refusals a screen tells the owner about.
#[derive(Clone, PartialEq, Eq)]
pub enum BankImportRefusal {
    /// The account is not a bank or credit card account.
    NotBankOrCard,
    /// The account is not in this company.
    NotInCompany,
    /// A row the engine refused (line and field).
    InvalidRow {
        /// The row's line.
        line: Option<u64>,
        /// The field.
        field: Option<String>,
    },
    /// Over 20,000 rows.
    TooManyRows,
    /// The rows add up to too much.
    TotalTooLarge,
    /// The file's identity is linked to another account.
    LinkedElsewhere(LinkedElsewhere),
    /// Undo: some rows were already used (their lines).
    RowsAlreadyUsed(Vec<u64>),
    /// Undo: the import is still being prepared.
    StillPreparing,
    /// Undo: already undone.
    AlreadyUndone,
}

impl fmt::Debug for BankImportRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BankImportRefusal::NotBankOrCard => f.write_str("NotBankOrCard"),
            BankImportRefusal::NotInCompany => f.write_str("NotInCompany"),
            BankImportRefusal::InvalidRow { line, field } => f
                .debug_struct("InvalidRow")
                .field("line", line)
                .field("field", field)
                .finish(),
            BankImportRefusal::TooManyRows => f.write_str("TooManyRows"),
            BankImportRefusal::TotalTooLarge => f.write_str("TotalTooLarge"),
            BankImportRefusal::LinkedElsewhere(_) => f.write_str("LinkedElsewhere(..)"),
            BankImportRefusal::RowsAlreadyUsed(l) => write!(f, "RowsAlreadyUsed({})", l.len()),
            BankImportRefusal::StillPreparing => f.write_str("StillPreparing"),
            BankImportRefusal::AlreadyUndone => f.write_str("AlreadyUndone"),
        }
    }
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn bank_import_refusal(e: &ApiError) -> Option<BankImportRefusal> {
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        _ => None,
    };
    match error_code(e)? {
        "ACCOUNT_NOT_BANK_OR_CARD" => Some(BankImportRefusal::NotBankOrCard),
        "ACCOUNT_NOT_IN_COMPANY" => Some(BankImportRefusal::NotInCompany),
        "INVALID_ROW" => {
            let (line, field) = match e {
                ApiError::Validation(v) => (v.line(), v.field().map(|p| p.as_str().to_string())),
                _ => (None, None),
            };
            Some(BankImportRefusal::InvalidRow { line, field })
        }
        "TOO_MANY_ROWS" => Some(BankImportRefusal::TooManyRows),
        "BATCH_TOTAL_TOO_LARGE" => Some(BankImportRefusal::TotalTooLarge),
        "IDENTITY_LINKED_ELSEWHERE" => {
            let d = details?;
            Some(BankImportRefusal::LinkedElsewhere(LinkedElsewhere {
                account_name: d.text("accountName").unwrap_or_default().to_string(),
                mask: d.text("mask").unwrap_or_default().to_string(),
            }))
        }
        "ROWS_ALREADY_USED" => Some(BankImportRefusal::RowsAlreadyUsed(
            details.map(|d| d.numbers("lines")).unwrap_or_default(),
        )),
        "STILL_PREPARING" => Some(BankImportRefusal::StillPreparing),
        "ALREADY_UNDONE" => Some(BankImportRefusal::AlreadyUndone),
        _ => None,
    }
}

/// Bank file imports of a session (`session.bank_import()`).
#[derive(Clone, Copy, Debug)]
pub struct BankImport<'a>(pub(crate) &'a Session);

impl Session {
    /// Bank file imports of the open company (P25, U-06).
    #[must_use]
    pub fn bank_import(&self) -> BankImport<'_> {
        BankImport(self)
    }
}

impl BankImport<'_> {
    /// Previews a section: every row's status, the counts and the balance
    /// check. Changes nothing; sent without an `Idempotency-Key`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `bank-import-batch-v1`; a
    /// [`LocalValidation`] from [`BankImportRequest::body`] (nothing sent); the
    /// 422s [`bank_import_refusal`] reads; any [`ApiError`].
    pub async fn preview(&self, req: &BankImportRequest) -> Result<BankImportOutcome, ApiError> {
        require(self.0, Capabilities::BANK_IMPORT_BATCH)?;
        let bytes = req.body_bytes(true).map_err(ApiError::Local)?;
        let mut call = Call::at(Method::Post, FEED_IMPORT_BATCH.route()).json_body(bytes);
        call.no_key = true;
        let reply = self.0.send(&FEED_IMPORT_BATCH, call, None).await?;
        outcome_from(&reply.body, reply.replayed)
    }

    /// Commits a section under `key` (made at the first Send and reused for
    /// every retry of the same body; a changed body gets a new key).
    ///
    /// # Errors
    /// As [`BankImport::preview`]; 409 `identity_linked_elsewhere` unless
    /// `relink_identity`; `OutcomeUnknown { keyed: true }` when the answer
    /// was lost (send again with the same key).
    pub async fn commit(
        &self,
        req: &BankImportRequest,
        key: IdempotencyKey,
    ) -> Result<BankImportOutcome, ApiError> {
        require(self.0, Capabilities::BANK_IMPORT_BATCH)?;
        let bytes = req.body_bytes(false).map_err(ApiError::Local)?;
        let mut call = Call::at(Method::Post, FEED_IMPORT_BATCH.route()).json_body(bytes);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_IMPORT_BATCH, call, None).await?;
        outcome_from(&reply.body, reply.replayed)
    }

    /// Earlier imports, newest first, optionally of one account and format.
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability; any [`ApiError`].
    pub async fn batches(
        &self,
        account: Option<AccountId>,
        format: Option<BankFileFormat>,
    ) -> Result<Vec<ImportBatchSummary>, ApiError> {
        require(self.0, Capabilities::BANK_IMPORT_BATCH)?;
        let mut call = Call::at(Method::Get, FEED_IMPORT_BATCHES.route());
        if let Some(a) = account {
            call.query.push(("accountId".into(), a.to_string()));
        }
        if let Some(f) = format {
            call.query.push(("format".into(), f.as_wire().into()));
        }
        let reply = self.0.send(&FEED_IMPORT_BATCHES, call, None).await?;
        batches_from(&reply.body)
    }

    /// Undoes an import: its untouched waiting rows leave the feed. With
    /// `pending_only`, rows already used stay and the rest go. While the
    /// import is still being prepared, an engine with
    /// `bank-import-progress-v1` stops the preparation after its current
    /// chunk and then undoes (the batch then reports `Cancelled`); it waits
    /// up to 20 s for that, so the call uses the Import time class.
    ///
    /// # Errors
    /// The 409s [`bank_import_refusal`] reads (`rows_already_used`,
    /// `still_preparing` (could not stop within 20 s; nothing undone),
    /// `already_undone`); `NotFound`; any [`ApiError`].
    pub async fn undo(
        &self,
        batch: ImportBatchId,
        pending_only: bool,
        key: IdempotencyKey,
    ) -> Result<ImportUndoOutcome, ApiError> {
        require(self.0, Capabilities::BANK_IMPORT_BATCH)?;
        let mut call = Call::new(
            Method::Delete,
            FEED_IMPORT_BATCH_UNDO.route(),
            FEED_IMPORT_BATCH_UNDO
                .route()
                .replace("{id}", &batch.to_string()),
        );
        if pending_only {
            call.query.push(("pendingOnly".into(), "true".into()));
        }
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_IMPORT_BATCH_UNDO, call, None).await?;
        undo_from(&reply.body)
    }

    /// The accounts a file's identity was linked to before.
    ///
    /// # Errors
    /// `Local(Unsupported)` without the capability; `Local(InvalidValue)`
    /// for a blank or too long number or mask; any [`ApiError`].
    pub async fn lookup(&self, by: &BankLookupBy) -> Result<Vec<ConnectionMatch>, ApiError> {
        require(self.0, Capabilities::BANK_IMPORT_BATCH)?;
        let body = match by {
            BankLookupBy::Account(id) if id.valid() => id.wire(),
            BankLookupBy::Mask(m) if !m.is_empty() && !too_long(m, 10) => json!({ "mask": m }),
            _ => return Err(ApiError::Local(LocalValidation::InvalidValue)),
        };
        let bytes = serde_json::to_vec(&body)
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let mut call = Call::at(Method::Post, BANK_CONNECTION_LOOKUP.route()).json_body(bytes);
        call.no_key = true;
        let reply = self.0.send(&BANK_CONNECTION_LOOKUP, call, None).await?;
        matches_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "bank_import_tests.rs"]
mod tests;
