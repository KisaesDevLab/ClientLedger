//! Write Check (P13 WP1): the check record, editing a saved check, the
//! checks of a bank account and the next check numbers.
//!
//! - [`CheckRecord`] is `GET /transactions/{id}` read as a check: the
//!   ledger [`Transaction`] plus the check's own fields (number, print
//!   status, the name and address printed on it, the printed memo) and its
//!   header tags. Amounts are exact `Money`, as on every transaction read.
//! - Its version fingerprint is the [`Transaction`] editable fields plus
//!   the check fields. The print status is hashed as its kind (`numbered`
//!   for hand-written and printed, `queue`): a hand-written check never
//!   becomes printed without a new number, and the engine keeps a printed
//!   check printed when an edit keeps its number, so the kind is exactly
//!   what an edit decides. Header tags are not hashed: its `PUT /checks`
//!   answer does not carry them. An edit always sends them (`tagIds`,
//!   possibly empty) and the engine replaces the check's header tags with
//!   exactly that set.
//! - [`Checks::update_check`] (`PUT /checks/{id}`, `check-edit-v1`) is
//!   refused locally without the capability, guarded by the fingerprint
//!   (`Conflict { Stale }`, nothing sent) and reconciled after a lost reply
//!   by reading the check again. An edit always names the check number or
//!   Print later, so the record it leaves is known in advance.
//! - The checks list sends its amounts as the engine's numeric text: they
//!   become [`RegisterAmount`]s (exact for a decimal string, display-only
//!   for a JSON number) with no path into a posting. An edit always starts
//!   from [`Checks::get`].
//! - `Debug` never prints names, addresses, memos or amounts.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, TagId, TransactionId};
use cl_core::{CalendarDate, Timestamp};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::bank_postings::{BankPosting, CheckDraft, saved_any};
use crate::resources::operations::{CHECK_PUT, CHECK_SETTINGS, CHECKS_LIST, TXN_GET};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::reconcile::row_fp;
use crate::resources::register::{RegisterAmount, amount};
use crate::resources::transactions::{Transaction, TxnStatus};
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, fingerprint_of, get_versioned,
    update_guarded_reply,
};
use crate::session::Session;
use crate::wire::{Capabilities, PageRequest};

/// Where a check is in its printing life (`printStatus`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PrintStatus {
    /// Numbered by hand (or typed): already written.
    HandWritten,
    /// Waiting to be printed; no number yet ("To print").
    Queue,
    /// Printed, with the number it printed with.
    Printed,
    /// A status this version does not know (shown, never sent back).
    Other(String),
}

impl PrintStatus {
    /// From the engine's text.
    #[must_use]
    pub fn from_wire(s: &str) -> PrintStatus {
        match s {
            "hand_written" => PrintStatus::HandWritten,
            "queue" => PrintStatus::Queue,
            "printed" => PrintStatus::Printed,
            other => PrintStatus::Other(other.to_string()),
        }
    }

    /// The engine's text.
    #[must_use]
    pub fn as_wire(&self) -> &str {
        match self {
            PrintStatus::HandWritten => "hand_written",
            PrintStatus::Queue => "queue",
            PrintStatus::Printed => "printed",
            PrintStatus::Other(s) => s,
        }
    }

    /// The kind an edit decides (hashed in the fingerprint): `numbered`
    /// for hand-written and printed checks, otherwise the status itself.
    fn kind(&self) -> &str {
        match self {
            PrintStatus::HandWritten | PrintStatus::Printed => KIND_NUMBERED,
            other => other.as_wire(),
        }
    }
}

const KIND_NUMBERED: &str = "numbered";
const KIND_QUEUE: &str = "queue";

impl Serialize for PrintStatus {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_wire())
    }
}

/// A saved check: the ledger record plus the check's own fields.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckRecord {
    /// The ledger record (an `expense`, or a `bill_payment` for a check
    /// that pays bills; those cannot be edited here).
    pub txn: Transaction,
    /// The check number (`None` while it waits to be printed).
    pub check_number: Option<u32>,
    /// Hand-written, to print, or printed.
    pub print_status: PrintStatus,
    /// The name printed on the check (empty when the engine has none).
    pub payee_name: String,
    /// The address printed under the name.
    pub payee_address: Option<String>,
    /// The memo printed on the check.
    pub printed_memo: Option<String>,
    /// Header tags (empty when the read does not carry them).
    pub tag_ids: Vec<TagId>,
}

impl fmt::Debug for CheckRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckRecord")
            .field("id", &self.txn.id)
            .field("txn_type", &self.txn.txn_type)
            .field("date", &self.txn.txn_date)
            .field("status", &self.txn.status)
            .field("check_number", &self.check_number)
            .field("print_status", &self.print_status)
            .field("tag_ids", &self.tag_ids)
            .field("lines", &self.txn.lines.len())
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckWire {
    #[serde(default)]
    check_number: Option<u32>,
    #[serde(default)]
    print_status: Option<String>,
    #[serde(default)]
    payee_name_on_check: Option<String>,
    #[serde(default)]
    payee_address: Option<String>,
    #[serde(default)]
    printed_memo: Option<String>,
    #[serde(default)]
    tags: Option<Vec<TagRef>>,
}

#[derive(Deserialize)]
struct TagRef {
    id: TagId,
}

/// Reads the same object twice: once as the [`Transaction`], once for the
/// check fields. A record without a print status is not a check.
impl<'de> Deserialize<'de> for CheckRecord {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let txn = Transaction::deserialize(&v).map_err(|_| de::Error::custom("transaction"))?;
        let w = CheckWire::deserialize(&v).map_err(|_| de::Error::custom("check fields"))?;
        let print_status = w
            .print_status
            .as_deref()
            .map(PrintStatus::from_wire)
            .ok_or_else(|| de::Error::custom("not a check"))?;
        Ok(CheckRecord {
            txn,
            check_number: w.check_number,
            print_status,
            payee_name: w.payee_name_on_check.unwrap_or_default(),
            payee_address: w.payee_address,
            printed_memo: w.printed_memo,
            tag_ids: w
                .tags
                .unwrap_or_default()
                .into_iter()
                .map(|t| t.id)
                .collect(),
        })
    }
}

/// Adds the check fields to a transaction's editable JSON.
fn with_check_fields(
    mut txn: Value,
    number: Option<u32>,
    kind: &str,
    payee_name: &str,
    payee_address: Option<&str>,
    printed_memo: Option<&str>,
) -> Value {
    let empty_is_null = |s: Option<&str>| s.filter(|v| !v.is_empty()).map(str::to_string);
    if let Value::Object(map) = &mut txn {
        map.insert("checkNumber".into(), json!(number));
        map.insert("printStatus".into(), Value::String(kind.to_string()));
        map.insert(
            "payeeNameOnCheck".into(),
            Value::String(payee_name.to_string()),
        );
        map.insert("payeeAddress".into(), json!(empty_is_null(payee_address)));
        map.insert("printedMemo".into(), json!(empty_is_null(printed_memo)));
    }
    txn
}

impl EditableFields for CheckRecord {
    const EDITABLE: &'static [&'static str] = &[
        "txnType",
        "txnDate",
        "memo",
        "basis",
        "status",
        "contactId",
        "lines.accountId",
        "lines.debit",
        "lines.credit",
        "lines.description",
        "lines.tagId",
        "lines.contactId",
        "lines.isBillable",
        "lines.lineOrder",
        "lines.sourceTransactionId",
        "checkNumber",
        "printStatus",
        "payeeNameOnCheck",
        "payeeAddress",
        "printedMemo",
    ];

    fn editable(&self) -> Value {
        with_check_fields(
            self.txn.editable(),
            self.check_number,
            self.print_status.kind(),
            &self.payee_name,
            self.payee_address.as_deref(),
            self.printed_memo.as_deref(),
        )
    }
}

/// The fingerprint the check has once `PUT /checks/{id}` stored `d`
/// (`posting` is `d`'s posting) with the owner's `printed` choice.
/// `Reprint` puts a printed check back in the print queue without a number
/// (the choice is only sent after the engine said the check was printed
/// and its printed face changes); `KeepNumber` keeps it numbered.
fn requested_fingerprint(
    posting: &BankPosting,
    d: &CheckDraft,
    printed: Option<PrintedCheckChange>,
) -> [u8; 32] {
    let (number, kind) = if d.print_later || printed == Some(PrintedCheckChange::Reprint) {
        (None, KIND_QUEUE)
    } else {
        (d.check_number, KIND_NUMBERED)
    };
    fingerprint_of(&with_check_fields(
        posting.editable_value(false),
        number,
        kind,
        &d.payee_name,
        d.payee_address.as_deref(),
        d.printed_memo.as_deref(),
    ))
}

/// One row of the checks list. The amount is the engine's figure as sent,
/// for display only.
#[derive(Clone)]
pub struct CheckSummary {
    /// The check.
    pub id: TransactionId,
    /// Its number (`None` while it waits to be printed).
    pub number: Option<u32>,
    /// Its date, display only: the engine's list sends a time stamp of
    /// midnight in the engine's own time zone; the nearest calendar day is
    /// taken. Edits read the exact date from [`Checks::get`].
    pub date: Option<CalendarDate>,
    /// The name printed on the check, else the payee's name (owner data).
    pub payee: Option<String>,
    /// The check amount as the engine sent it.
    pub amount: Option<RegisterAmount>,
    /// Hand-written, to print, or printed.
    pub print_status: PrintStatus,
    /// Posted or void.
    pub status: TxnStatus,
}

impl fmt::Debug for CheckSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckSummary")
            .field("id", &self.id)
            .field("number", &self.number)
            .field("date", &self.date)
            .field("print_status", &self.print_status)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRow {
    id: TransactionId,
    #[serde(default)]
    txn_date: Option<String>,
    #[serde(default)]
    amount: Option<Box<RawValue>>,
    status: TxnStatus,
    #[serde(default)]
    check_number: Option<u32>,
    #[serde(default)]
    print_status: Option<String>,
    #[serde(default)]
    payee_name_on_check: Option<String>,
    #[serde(default)]
    contact_name: Option<String>,
}

const HALF_DAY_MS: i64 = 12 * 60 * 60 * 1000;

/// A list date: `YYYY-MM-DD`, or the time stamp of a local midnight, read
/// as the nearest calendar day (exact for engine time zones from UTC−11:59
/// to UTC+12:00).
fn list_date(s: &str) -> Option<CalendarDate> {
    if let Ok(d) = CalendarDate::parse_wire(s) {
        return Some(d);
    }
    let t = Timestamp::parse_wire(s).ok()?;
    let shifted = Timestamp::from_unix_millis(t.unix_millis().checked_add(HALF_DAY_MS)?).ok()?;
    CalendarDate::from_naive(shifted.as_utc().date_naive()).ok()
}

/// Parses `{data: [...]}` of `GET /checks`.
///
/// # Errors
/// `Decode` when the body or a row is not that shape.
pub fn checks_from_json(body: &[u8]) -> Result<Vec<CheckSummary>, ApiError> {
    #[derive(Deserialize)]
    struct W {
        data: Vec<ListRow>,
    }
    const WHAT: &str = "checks";
    let w: W = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    w.data
        .into_iter()
        .map(|r| {
            let amount = match r.amount.as_deref() {
                None => None,
                Some(raw) if raw.get() == "null" => None,
                Some(raw) => Some(amount(raw, WHAT)?),
            };
            Ok(CheckSummary {
                id: r.id,
                number: r.check_number,
                date: r.txn_date.as_deref().and_then(list_date),
                payee: r
                    .payee_name_on_check
                    .filter(|n| !n.is_empty())
                    .or(r.contact_name),
                amount,
                print_status: PrintStatus::from_wire(r.print_status.as_deref().unwrap_or("")),
                status: r.status,
            })
        })
        .collect()
}

/// The next check numbers the engine keeps (`GET /checks/settings`). The
/// bank details in the same settings are never read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckSettings {
    /// The next number per bank account.
    pub next_numbers: BTreeMap<AccountId, u32>,
    /// The company-wide next number of older engines.
    pub legacy_next: Option<u32>,
}

impl CheckSettings {
    /// The engine's next number for `bank` (its own counter, else the
    /// company-wide one; `None` when neither is set: the engine then starts
    /// at 1001).
    #[must_use]
    pub fn next_for(&self, bank: AccountId) -> Option<u32> {
        self.next_numbers.get(&bank).copied().or(self.legacy_next)
    }
}

fn positive_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n >= 1)
}

/// Parses `{settings: {...}}` of `GET /checks/settings`.
///
/// # Errors
/// `Decode` when the body is not an object with `settings`.
pub fn check_settings_from_json(body: &[u8]) -> Result<CheckSettings, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "check settings",
    })?;
    let settings = v
        .get("settings")
        .filter(|s| s.is_object())
        .ok_or(ApiError::Decode {
            what: "check settings",
        })?;
    let next_numbers = settings
        .get("nextCheckNumbers")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| Some((AccountId::parse(k).ok()?, positive_u32(v)?)))
                .collect()
        })
        .unwrap_or_default();
    Ok(CheckSettings {
        next_numbers,
        legacy_next: settings.get("nextCheckNumber").and_then(positive_u32),
    })
}

/// What happens to a check that was already printed when an edit changes
/// what is printed on it (`printedCheckChange`, `check-edit-v1`). Without
/// one, the engine refuses such an edit with 409 `CHECK_PRINTED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrintedCheckChange {
    /// Print it again: it goes back to the print queue without a number.
    Reprint,
    /// Keep its number as a handwritten check.
    KeepNumber,
}

impl PrintedCheckChange {
    /// The wire value (`reprint`, `keep_number`).
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            PrintedCheckChange::Reprint => "reprint",
            PrintedCheckChange::KeepNumber => "keep_number",
        }
    }
}

/// The engine code of the 409 for an edit that changes a printed check
/// without saying what to do with it.
pub const CHECK_PRINTED: &str = "CHECK_PRINTED";

/// True for the engine's refusal to change a printed check without a
/// [`PrintedCheckChange`] (409 `CHECK_PRINTED`).
#[must_use]
pub fn is_check_printed(e: &ApiError) -> bool {
    matches!(e, ApiError::Conflict { code: Some(c), .. } if c.as_str() == CHECK_PRINTED)
}

/// The engine code of the 409 for an edit (or a void) that changes or
/// removes a line already billed to a customer.
pub const LINE_BILLED: &str = "LINE_BILLED";

/// True for the engine's refusal to change a billed line (409
/// `LINE_BILLED`).
#[must_use]
pub fn is_line_billed(e: &ApiError) -> bool {
    matches!(e, ApiError::Conflict { code: Some(c), .. } if c.as_str() == LINE_BILLED)
}

/// True for the engine's 409 for a check number already used on the bank
/// account: the only 409 it sends without a code.
#[must_use]
pub fn is_number_used(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Conflict {
            reason: ConflictReason::Unspecified,
            code: None,
            ..
        }
    )
}

impl Versioned<CheckRecord> {
    /// The version the ledger record has, for an edit through
    /// `PUT /transactions/{id}` ([`crate::Transactions::update_bank_posting`],
    /// the path without `check-edit-v1`): that edit re-reads the plain
    /// [`Transaction`], whose fingerprint leaves out the check fields. The
    /// engine's own version (`Server`) is the same for both reads.
    #[must_use]
    pub fn transaction_version(&self) -> RecordVersion {
        match self.version {
            RecordVersion::Server(v) => RecordVersion::Server(v),
            RecordVersion::Fingerprint(_) => {
                RecordVersion::Fingerprint(EditableFields::fingerprint(&self.record.txn))
            }
        }
    }
}

/// Options of a check edit: the posting options (closing-date password)
/// and, for a printed check, what to do with the printed one.
#[derive(Clone, Debug, Default)]
pub struct CheckEditOptions {
    /// Closing-date password and the like.
    pub posting: PostingOptions,
    /// Reprint, or keep the number as handwritten (printed checks only;
    /// the engine ignores it otherwise).
    pub printed_check_change: Option<PrintedCheckChange>,
}

impl From<&PostingOptions> for CheckEditOptions {
    fn from(posting: &PostingOptions) -> CheckEditOptions {
        CheckEditOptions {
            posting: posting.clone(),
            printed_check_change: None,
        }
    }
}

impl From<PostingOptions> for CheckEditOptions {
    fn from(posting: PostingOptions) -> CheckEditOptions {
        CheckEditOptions {
            posting,
            printed_check_change: None,
        }
    }
}

/// Check operations of a session (`session.checks()`).
#[derive(Clone, Copy, Debug)]
pub struct Checks<'a>(pub(crate) &'a Session);

impl Session {
    /// Checks of the open company.
    #[must_use]
    pub fn checks(&self) -> Checks<'_> {
        Checks(self)
    }
}

impl Checks<'_> {
    /// One check with its version, to fill the Write Check form.
    ///
    /// # Errors
    /// `NotFound`; `Decode` when the transaction is not a check; any
    /// [`ApiError`].
    pub async fn get(&self, id: TransactionId) -> Result<Versioned<CheckRecord>, ApiError> {
        get_versioned(
            self.0,
            &TXN_GET,
            format!("/api/v1/transactions/{id}"),
            "transaction",
        )
        .await
    }

    /// Checks drawn on `bank` (all banks when `None`) dated within
    /// `start..=end` (either end open), newest first.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn list(
        &self,
        bank: Option<AccountId>,
        start: Option<CalendarDate>,
        end: Option<CalendarDate>,
        page: PageRequest,
    ) -> Result<Vec<CheckSummary>, ApiError> {
        let mut call = Call::at(Method::Get, CHECKS_LIST.route());
        if let Some(b) = bank {
            call.query.push(("bank_account_id".into(), b.to_string()));
        }
        if let Some(d) = start {
            call.query.push(("start_date".into(), d.to_wire()));
        }
        if let Some(d) = end {
            call.query.push(("end_date".into(), d.to_wire()));
        }
        call.query.extend(page.query());
        let reply = self.0.send(&CHECKS_LIST, call, None).await?;
        checks_from_json(&reply.body)
    }

    /// The engine's next check numbers.
    ///
    /// # Errors
    /// `Decode`; any [`ApiError`].
    pub async fn settings(&self) -> Result<CheckSettings, ApiError> {
        let call = Call::at(Method::Get, CHECK_SETTINGS.route());
        let reply = self.0.send(&CHECK_SETTINGS, call, None).await?;
        check_settings_from_json(&reply.body)
    }

    /// Replaces a saved check if it is still at `expected`: lines, amount,
    /// date, payee and memo, and the check's own number, printed name,
    /// address, printed memo and Print later. The header tags become
    /// exactly the posting's (`tagIds` is always sent; empty clears them).
    /// The engine checks the closing date for the old and the new
    /// date (`closingPassword` rides in `opts`).
    ///
    /// # Errors
    /// Nothing is sent for: `Local(CheckEditUnavailable)` without
    /// `check-edit-v1`; `Local(InvalidValue)` for a posting that is not a
    /// check, or one with neither a number nor Print later;
    /// `Local(ClosedPeriodOverrideUnavailable)`; `Conflict { Stale }` (the
    /// check changed since it was read). From the engine: `Validation`
    /// (code `CHECK_NOT_EDITABLE` for a check that pays bills; a void
    /// check; a reconciled bank line), `Conflict` (the number is used on
    /// the bank account), `NotFound`, `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `ClosingPasswordRateLimited`,
    /// `OutcomeUnknown`, or any [`ApiError`]. A printed check whose
    /// printed face changes without `opts.printed_check_change` is refused
    /// with `Conflict` code `CHECK_PRINTED` ([`is_check_printed`]); send it
    /// again with the owner's choice. A change to a line already billed to
    /// a customer is refused with `Conflict` code `LINE_BILLED`
    /// ([`is_line_billed`]); a used number is the code-less `Conflict`
    /// ([`is_number_used`]).
    pub async fn update_check(
        &self,
        id: TransactionId,
        expected: &RecordVersion,
        posting: &BankPosting,
        opts: impl Into<CheckEditOptions>,
        key: IdempotencyKey,
    ) -> Result<Versioned<CheckRecord>, ApiError> {
        let s = self.0;
        let opts: CheckEditOptions = opts.into();
        if !s.client().capabilities().has(Capabilities::CHECK_EDIT) {
            return Err(ApiError::Local(LocalValidation::CheckEditUnavailable));
        }
        let (Some(d), Some(mut value)) = (posting.check_draft(), posting.check_put_value()) else {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        };
        if !d.print_later && d.check_number.is_none() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        if let (Some(change), Value::Object(map)) = (opts.printed_check_change, &mut value) {
            map.insert("printedCheckChange".into(), json!(change.as_wire()));
        }
        let body = posting_body(s, value, &opts.posting)?;
        let u = GuardedUpdate {
            get: &TXN_GET,
            put: &CHECK_PUT,
            path: format!("/api/v1/transactions/{id}"),
            key: "transaction",
            body,
            idempotency_key: key,
            requested: requested_fingerprint(posting, d, opts.printed_check_change),
            fp: row_fp::<CheckRecord>,
            stamp_covered: true,
        };
        let reply = update_guarded_reply::<CheckRecord>(
            s,
            expected,
            u,
            Some(format!("/api/v1/checks/{id}")),
        )
        .await?;
        // A reconciled answer is the `{transaction}` read.
        saved_any(&reply.body, &["check", "transaction"])
    }
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
