//! Bulk Entry (P15A, `/api/v1/batch/*`): validate, save, account
//! suggestions and CSV parsing (engine contract:
//! `target/plans/P15A-engine-shapes.md`, PR #163, U-67–U-74).
//!
//! - A Bulk Entry window holds sections; each section is one
//!   [`BatchGroup`] (one [`BatchKind`] and one context account), and a
//!   whole batch goes to the engine in one call (`batch-mixed-v1`,
//!   `groups[]`). Row numbers are unique across the whole call; the engine
//!   names refused rows by them and cl-api maps each back to its group.
//! - Amounts are [`BatchAmount`]s: whole cents, positive, exact
//!   ([`Money`]), sent as `"123.45"` strings, never JSON numbers. cl-model
//!   rounds what the owner typed (Q-232) before a row is built, so a
//!   sub-cent value here is a bug and is refused by the type, never
//!   rounded. `parse-csv` amounts come back as strings
//!   (`batch-exact-v1`); a JSON number there is refused as
//!   [`ApiError::InexactMoney`].
//! - The closing-date password is set on each locked row as a [`Secret`];
//!   the save sends it once as the body's `closingPassword`
//!   (`batch-closing-v1`, one check per request). Body only, never logged;
//!   `overrideConfirmed` is never sent (NN8).
//! - Save all needs `batch-atomic-v1`, `batch-exact-v1`,
//!   `batch-idempotent-v1` and `batch-mixed-v1`; without them nothing is
//!   sent ([`LocalValidation::BatchUnavailable`]), so nothing posts
//!   through the engine's float path. Validate also works on an older
//!   engine for a single section (its one-type shape).
//! - Names, memos, amounts and passwords never appear in `Debug`: ids,
//!   row numbers and counts only.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, BatchId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde::{Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation, UserText};
use crate::idempotency::IdempotencyKey;
use crate::resources::contacts::{SuggestSource, SuggestedAccount};
use crate::resources::operations::{
    BATCH_PARSE_CSV, BATCH_SAVE, BATCH_SUGGEST_ACCOUNT, BATCH_VALIDATE,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// The largest file `POST /batch/parse-csv` accepts (the engine's upload
/// limit, Q-P15Ag).
pub const PARSE_CSV_MAX_BYTES: usize = 10 * 1024 * 1024;

/// The most rows the engine takes in one call.
pub const ENGINE_MAX_ROWS: usize = 10_000;

// ---------------------------------------------------------------- kinds

/// What a section's rows become (Q-P15Ab, Q-263).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchKind {
    /// Checks from a bank account (`batch-check-v1`).
    Check,
    /// Expenses (no check number) from a bank account.
    Expense,
    /// Card charges on a credit card account.
    #[serde(rename = "credit_card_charge")]
    CardCharge,
    /// Card credits (refunds) on a credit card account.
    #[serde(rename = "credit_card_credit")]
    CardCredit,
    /// Deposits into a bank account.
    Deposit,
    /// Journal entries (no context account).
    JournalEntry,
}

impl BatchKind {
    /// Every kind, in the order the Type picker lists them.
    pub const ALL: [BatchKind; 6] = [
        BatchKind::Check,
        BatchKind::Expense,
        BatchKind::CardCharge,
        BatchKind::CardCredit,
        BatchKind::Deposit,
        BatchKind::JournalEntry,
    ];

    /// The engine's `txn_type`.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            BatchKind::Check => "check",
            BatchKind::Expense => "expense",
            BatchKind::CardCharge => "credit_card_charge",
            BatchKind::CardCredit => "credit_card_credit",
            BatchKind::Deposit => "deposit",
            BatchKind::JournalEntry => "journal_entry",
        }
    }

    /// The kind with this `txn_type`.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<BatchKind> {
        BatchKind::ALL.into_iter().find(|k| k.wire() == s)
    }

    /// The capability the engine must advertise for this kind, beyond the
    /// ones every save needs.
    #[must_use]
    pub const fn capability(self) -> Option<&'static str> {
        match self {
            BatchKind::Check => Some(Capabilities::BATCH_CHECK),
            _ => None,
        }
    }

    /// True when rows post against a bank or card account picked for the
    /// section (every kind but journal entries).
    #[must_use]
    pub const fn has_context(self) -> bool {
        !matches!(self, BatchKind::JournalEntry)
    }
}

/// Which `batch-*-v1` capabilities the engine advertises (P15A §2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // one flag per capability
pub struct BatchCaps {
    /// `batch-atomic-v1`: one transaction, failing rows named.
    pub atomic: bool,
    /// `batch-check-v1`: check rows.
    pub check: bool,
    /// `batch-mixed-v1`: several groups in one call.
    pub mixed: bool,
    /// `batch-exact-v1`: exact string amounts.
    pub exact: bool,
    /// `batch-idempotent-v1`: `Idempotency-Key` replays.
    pub idempotent: bool,
    /// `batch-closing-v1`: a row's closing-date password.
    pub closing: bool,
}

impl BatchCaps {
    /// Reads the flags from the engine's capabilities.
    #[must_use]
    pub fn of(caps: &Capabilities) -> BatchCaps {
        BatchCaps {
            atomic: caps.has(Capabilities::BATCH_ATOMIC),
            check: caps.has(Capabilities::BATCH_CHECK),
            mixed: caps.has(Capabilities::BATCH_MIXED),
            exact: caps.has(Capabilities::BATCH_EXACT),
            idempotent: caps.has(Capabilities::BATCH_IDEMPOTENT),
            closing: caps.has(Capabilities::BATCH_CLOSING),
        }
    }

    /// True when Save all may post at all (Q-P15Ac): atomic, exact and
    /// idempotent. Without them nothing posts through the engine's float
    /// path.
    #[must_use]
    pub const fn can_save(self) -> bool {
        self.atomic && self.exact && self.idempotent
    }

    /// True when the engine takes rows of `kind`.
    #[must_use]
    pub const fn offers(self, kind: BatchKind) -> bool {
        match kind {
            BatchKind::Check => self.check,
            _ => true,
        }
    }
}

// --------------------------------------------------------------- amounts

/// A Bulk Entry amount: exact, positive, whole cents.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BatchAmount(Money);

impl BatchAmount {
    /// Wraps an amount cl-model already rounded to the cent.
    ///
    /// # Errors
    /// `NonPositiveAmount` for zero or less; `NotWholeCents` when digits
    /// remain below the cent (a 2.505 is a bug upstream, never rounded
    /// here).
    pub fn new(amount: Money) -> Result<BatchAmount, LocalValidation> {
        if !amount.is_positive() {
            return Err(LocalValidation::NonPositiveAmount);
        }
        if amount.to_ten_thousandths() % 100 != 0 {
            return Err(LocalValidation::NotWholeCents);
        }
        Ok(BatchAmount(amount))
    }

    /// The amount.
    #[must_use]
    pub const fn money(self) -> Money {
        self.0
    }

    /// The engine text: digits, a point and two decimals (`"1234.50"`).
    #[must_use]
    pub fn to_wire(self) -> String {
        // Whole cents and positive (checked in `new`).
        let cents = self.0.to_ten_thousandths() / 100;
        format!("{}.{:02}", cents / 100, cents % 100)
    }
}

impl TryFrom<Money> for BatchAmount {
    type Error = LocalValidation;
    fn try_from(m: Money) -> Result<BatchAmount, LocalValidation> {
        BatchAmount::new(m)
    }
}

/// Owner data: the value is not printed.
impl fmt::Debug for BatchAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BatchAmount(..)")
    }
}

impl Serialize for BatchAmount {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

// ------------------------------------------------------------------ rows

/// One row the owner entered or pasted, as sent to the engine.
///
/// A picked name or account goes as its id; text the owner typed that is
/// not (yet) in a list goes as the name, for the engine to resolve or, with
/// "Add new names to the list", to create (Q-P15Ad).
#[derive(Clone, PartialEq, Eq)]
pub struct BatchRowIn {
    /// 1-based, unique across the whole call, captured at send time; the
    /// engine names failing rows by it.
    pub row_number: u32,
    /// The transaction date.
    pub date: CalendarDate,
    /// Check number, Ref no. or journal Entry No.
    pub number: Option<String>,
    /// A check row's Print later mark (`batch-check-v1`).
    pub print_later: bool,
    /// The name, when picked from the list.
    pub contact: Option<ContactId>,
    /// The name as typed, when not picked.
    pub contact_name: Option<String>,
    /// The line account, when picked.
    pub account: Option<AccountId>,
    /// The line account as typed (name or number), when not picked.
    pub account_name: Option<String>,
    /// The amount (every kind but journal entries).
    pub amount: Option<BatchAmount>,
    /// A journal entry row's debit.
    pub debit: Option<BatchAmount>,
    /// A journal entry row's credit.
    pub credit: Option<BatchAmount>,
    /// The memo.
    pub memo: Option<String>,
    /// One tag per line (Q-233).
    pub tag: Option<TagId>,
    /// The closing-date password on a row dated on or before the closing
    /// date (`batch-closing-v1`). The save sends it once, as the body's
    /// `closingPassword`; every row that carries one must carry the same.
    pub closing_password: Option<Secret>,
}

impl BatchRowIn {
    /// An empty row `row_number` dated `date`.
    #[must_use]
    pub fn new(row_number: u32, date: CalendarDate) -> BatchRowIn {
        BatchRowIn {
            row_number,
            date,
            number: None,
            print_later: false,
            contact: None,
            contact_name: None,
            account: None,
            account_name: None,
            amount: None,
            debit: None,
            credit: None,
            memo: None,
            tag: None,
            closing_password: None,
        }
    }
}

/// Ids and flags only: no name, memo, amount or password.
impl fmt::Debug for BatchRowIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchRowIn")
            .field("row_number", &self.row_number)
            .field("contact", &self.contact)
            .field("account", &self.account)
            .field("tag", &self.tag)
            .field("print_later", &self.print_later)
            .field("closing_password", &self.closing_password.is_some())
            .finish_non_exhaustive()
    }
}

/// One section of a batch: rows of one kind on one context account.
#[derive(Clone, PartialEq, Eq)]
pub struct BatchGroup {
    /// What the rows become.
    pub kind: BatchKind,
    /// The bank or card account (`None` for journal entries).
    pub context: Option<AccountId>,
    /// The rows, in send order.
    pub rows: Vec<BatchRowIn>,
}

/// The kind, account and row count only.
impl fmt::Debug for BatchGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchGroup")
            .field("kind", &self.kind)
            .field("context", &self.context)
            .field("rows", &self.rows.len())
            .finish()
    }
}

// -------------------------------------------------------------- problems

/// The cell a problem is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BatchField {
    /// Date.
    Date,
    /// Check number, Ref no. or Entry No.
    Number,
    /// Pay to, Payee, Received from or Name.
    Contact,
    /// The line account.
    Account,
    /// Amount.
    Amount,
    /// Debit.
    Debit,
    /// Credit.
    Credit,
    /// Memo.
    Memo,
    /// Tag.
    Tag,
    /// The row as a whole (or a field the app does not show).
    Row,
}

/// What kind of problem the engine (or the demo) found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ProblemKind {
    /// A name or account that is not in the lists ("Not in your lists").
    NotInLists,
    /// An inactive or deleted account, name or tag.
    Inactive,
    /// A required cell is empty.
    Required,
    /// A value the engine refused (bad date, bad amount, …).
    Invalid,
    /// The date is on or before the closing date.
    ClosedPeriod,
    /// A journal entry group whose debits and credits differ.
    Unbalanced,
    /// Anything else (the engine's message says what).
    Other,
}

/// One problem on one cell of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldProblem {
    /// The cell.
    pub field: BatchField,
    /// What is wrong.
    pub kind: ProblemKind,
    /// The engine's message for the owner (display only; never logged).
    pub message: UserText,
}

impl FieldProblem {
    /// A problem with `message` for the owner (empty when the app supplies
    /// its own text for `kind`).
    #[must_use]
    pub fn new(field: BatchField, kind: ProblemKind, message: impl Into<String>) -> FieldProblem {
        FieldProblem {
            field,
            kind,
            message: UserText::new(message),
        }
    }
}

/// A refused row of a save ([`ApiError::BatchRows`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchRowProblem {
    /// 0-based index of the group in the request.
    pub group: u32,
    /// The row's 1-based `row_number` (unique across the call).
    pub row_number: u32,
    /// What is wrong.
    pub problem: FieldProblem,
}

impl BatchRowProblem {
    /// The error a refused save returns.
    #[must_use]
    pub fn into_error(problems: Vec<BatchRowProblem>) -> ApiError {
        ApiError::BatchRows(problems)
    }
}

// --------------------------------------------------------------- replies

/// One row's result from `POST /batch/validate`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowCheck {
    /// 0-based index of the group in the request.
    pub group: u32,
    /// The row's 1-based `row_number`.
    pub row_number: u32,
    /// True when the row can be saved as it is (a warning still counts).
    pub ok: bool,
    /// The name the engine resolved.
    pub contact: Option<ContactId>,
    /// The account the engine resolved.
    pub account: Option<AccountId>,
    /// Problems, if any.
    pub problems: Vec<FieldProblem>,
    /// True when the name is not in the lists and would be added (with
    /// "Add new names to the list").
    pub new_contact: bool,
}

/// `POST /batch/validate`: one result per row sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchValidation {
    /// The rows, in request order.
    pub rows: Vec<RowCheck>,
}

/// One transaction a save posted. A journal entry's rows share an id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SavedTxn {
    /// 0-based index of the group in the request.
    pub group: u32,
    /// The row's 1-based `row_number`.
    pub row_number: u32,
    /// The posted transaction.
    pub id: TransactionId,
}

/// `POST /batch/save`: everything posted, in one engine transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BatchSaved {
    /// The engine's batch id (`batch-atomic-v1`).
    pub batch_id: Option<BatchId>,
    /// The posted rows.
    pub saved: Vec<SavedTxn>,
    /// How many names were added to the lists.
    pub created_contacts: u32,
}

// Account suggestions reuse `contacts::{SuggestedAccount, SuggestSource}`:
// `GET /batch/suggest-account` runs the same engine lookup as
// `GET /contacts/{id}/suggest-account` (default, then most recent).

// ------------------------------------------------------------------- CSV

/// A field a CSV or pasted column can fill (`column_mapping` keys).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvField {
    /// Date.
    Date,
    /// Check number, Ref no. or Entry No.
    RefNo,
    /// The name.
    Payee,
    /// The line account.
    Account,
    /// Memo.
    Memo,
    /// Amount.
    Amount,
    /// Debit.
    Debit,
    /// Credit.
    Credit,
}

impl CsvField {
    /// Every field.
    pub const ALL: [CsvField; 8] = [
        CsvField::Date,
        CsvField::RefNo,
        CsvField::Payee,
        CsvField::Account,
        CsvField::Memo,
        CsvField::Amount,
        CsvField::Debit,
        CsvField::Credit,
    ];
}

/// Field → 0-based column index, always sent explicitly to `parse-csv`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ColumnMapping(pub BTreeMap<CsvField, u16>);

/// One row `POST /batch/parse-csv` read, as text (`batch-exact-v1`
/// returns amounts as strings; cl-model re-parses them with the owner's
/// money rules, Q-232).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ParsedRow {
    /// The engine's 1-based row number.
    pub row_number: u32,
    /// Date text (the engine normalizes it when it can).
    pub date: Option<String>,
    /// Check number, Ref no. or Entry No.
    pub ref_no: Option<String>,
    /// The name.
    pub contact_name: Option<String>,
    /// The account name or number.
    pub account_name: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// Amount text.
    pub amount: Option<String>,
    /// Debit text.
    pub debit: Option<String>,
    /// Credit text.
    pub credit: Option<String>,
}

/// The row number only.
impl fmt::Debug for ParsedRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedRow")
            .field("row_number", &self.row_number)
            .finish_non_exhaustive()
    }
}

/// `POST /batch/parse-csv`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedRows {
    /// The rows, in file order.
    pub rows: Vec<ParsedRow>,
}

// ----------------------------------------------------------------- calls

/// The most groups (sections) one call takes.
pub const ENGINE_MAX_GROUPS: usize = 50;

/// The name of the file part `parse-csv` receives.
const CSV_FILE_NAME: &str = "batch.csv";

/// One row as the engine reads it (camelCase keys; empty values left out).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RowWire<'a> {
    row_number: u32,
    date: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ref_no: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contact_id: Option<ContactId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contact_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<AccountId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memo: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<BatchAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    debit: Option<BatchAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    credit: Option<BatchAmount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag_id: Option<TagId>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    print_later: bool,
}

/// One group (`snake_case` keys, as the engine's batch body has them).
#[derive(Serialize)]
struct GroupWire<'a> {
    txn_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_account_id: Option<AccountId>,
    rows: Vec<RowWire<'a>>,
}

/// `POST /batch/validate` with `groups[]`.
#[derive(Serialize)]
struct ValidateWire<'a> {
    groups: Vec<GroupWire<'a>>,
}

/// `POST /batch/save` with `groups[]`.
#[derive(Serialize)]
struct SaveWire<'a> {
    groups: Vec<GroupWire<'a>>,
    auto_create_contacts: bool,
    #[serde(rename = "closingPassword", skip_serializing_if = "Option::is_none")]
    closing_password: Option<&'a str>,
}

/// Text worth sending: `None` for an empty or blank value.
fn text(s: Option<&String>) -> Option<&str> {
    s.map(String::as_str).filter(|t| !t.trim().is_empty())
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn unavailable() -> ApiError {
    ApiError::Local(LocalValidation::BatchUnavailable)
}

/// Checks one row against its kind and builds its wire form. An id wins
/// over typed text, so the name is not sent when an id is.
fn row_wire(kind: BatchKind, row: &BatchRowIn) -> Result<RowWire<'_>, ApiError> {
    let journal = kind == BatchKind::JournalEntry;
    if row.row_number == 0
        || (journal && row.amount.is_some())
        || (!journal && (row.debit.is_some() || row.credit.is_some()))
        || (row.print_later && kind != BatchKind::Check)
    {
        return Err(invalid());
    }
    Ok(RowWire {
        row_number: row.row_number,
        date: row.date.to_wire(),
        ref_no: text(row.number.as_ref()),
        contact_id: row.contact,
        contact_name: if row.contact.is_some() {
            None
        } else {
            text(row.contact_name.as_ref())
        },
        account_id: row.account,
        account_name: if row.account.is_some() {
            None
        } else {
            text(row.account_name.as_ref())
        },
        memo: text(row.memo.as_ref()),
        amount: row.amount,
        debit: row.debit,
        credit: row.credit,
        tag_id: row.tag,
        print_later: row.print_later,
    })
}

/// The checked groups of a call and each row number's group index.
struct Prepared<'a> {
    groups: Vec<GroupWire<'a>>,
    group_of: BTreeMap<u32, u32>,
}

/// Checks the shape of a batch (nothing is sent when it fails): 1 to 50
/// non-empty groups, at most 10,000 rows, row numbers from 1 and unique
/// across the call, a context account exactly for the kinds that have one,
/// money fields that match the kind, and the kinds the engine offers.
fn prepare(groups: &[BatchGroup], caps: BatchCaps) -> Result<Prepared<'_>, ApiError> {
    let total: usize = groups.iter().map(|g| g.rows.len()).sum();
    if groups.is_empty() || groups.len() > ENGINE_MAX_GROUPS || total > ENGINE_MAX_ROWS {
        return Err(invalid());
    }
    let mut group_of = BTreeMap::new();
    let mut wire = Vec::with_capacity(groups.len());
    for (index, g) in groups.iter().enumerate() {
        if !caps.offers(g.kind) {
            return Err(unavailable());
        }
        if g.rows.is_empty() || g.kind.has_context() != g.context.is_some() {
            return Err(invalid());
        }
        // At most 50 groups (checked above).
        let index = u32::try_from(index).map_err(|_| invalid())?;
        let mut rows = Vec::with_capacity(g.rows.len());
        for r in &g.rows {
            if group_of.insert(r.row_number, index).is_some() {
                return Err(invalid());
            }
            rows.push(row_wire(g.kind, r)?);
        }
        wire.push(GroupWire {
            txn_type: g.kind.wire(),
            context_account_id: g.context,
            rows,
        });
    }
    Ok(Prepared {
        groups: wire,
        group_of,
    })
}

/// The one closing-date password of a save: the rows that carry one must
/// all carry the same.
fn closing_password(groups: &[BatchGroup]) -> Result<Option<&Secret>, ApiError> {
    let mut found: Option<&Secret> = None;
    for pw in groups
        .iter()
        .flat_map(|g| &g.rows)
        .filter_map(|r| r.closing_password.as_ref())
    {
        match found {
            Some(seen) if seen.expose() != pw.expose() => return Err(invalid()),
            _ => found = Some(pw),
        }
    }
    Ok(found)
}

/// The cell an engine error `field` is about.
fn field_of(field: &str) -> BatchField {
    match field {
        "date" => BatchField::Date,
        "ref_no" | "refNo" | "print_later" | "printLater" | "entry" => BatchField::Number,
        "contact_name" | "contactName" | "contact_id" | "contactId" => BatchField::Contact,
        "account_name" | "accountName" | "account_id" | "accountId" => BatchField::Account,
        "amount" => BatchField::Amount,
        "debit" => BatchField::Debit,
        "credit" => BatchField::Credit,
        "memo" | "description" => BatchField::Memo,
        "tag" | "tag_id" | "tagId" => BatchField::Tag,
        _ => BatchField::Row,
    }
}

/// What kind of problem an engine row error is: by its code first, then
/// (engines without codes) by its message.
fn kind_of(code: Option<&str>, message: &str) -> ProblemKind {
    match code {
        Some("TXN_UNBALANCED") => return ProblemKind::Unbalanced,
        Some("TB_PERIOD_LOCKED") => return ProblemKind::ClosedPeriod,
        Some("JE_AR_AP_NAME_REQUIRED") => return ProblemKind::Required,
        Some("CHECK_NUMBER_DUPLICATE" | "VALIDATION_ERROR" | "JE_AR_AP_ONE_PER_ENTRY") => {
            return ProblemKind::Invalid;
        }
        _ => {}
    }
    let m = message.to_ascii_lowercase();
    if m.contains("not found") {
        ProblemKind::NotInLists
    } else if m.contains("inactive") {
        ProblemKind::Inactive
    } else if m.contains("does not balance") {
        ProblemKind::Unbalanced
    } else if m.contains("required") {
        ProblemKind::Required
    } else {
        ProblemKind::Other
    }
}

/// One engine row error (`{field, message, code?}`).
#[derive(serde::Deserialize)]
struct ErrorWire {
    #[serde(default)]
    field: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

impl ErrorWire {
    fn problem(&self) -> FieldProblem {
        let message = self.message.as_deref().unwrap_or("");
        FieldProblem::new(
            field_of(self.field.as_deref().unwrap_or("row")),
            kind_of(self.code.as_deref(), message),
            message,
        )
    }
}

/// `Row 12: …` → `…` (the engine prefixes a posting failure's message).
fn strip_row_prefix(message: &str) -> &str {
    message
        .strip_prefix("Row ")
        .and_then(|rest| {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            (digits > 0).then(|| rest[digits..].strip_prefix(": "))?
        })
        .unwrap_or(message)
}

/// The refused rows of a failed save (`details.rowNumbers` and
/// `details.rows`). Groups are filled in by the caller, which knows them
/// (0 until then). `None` when the details name no rows.
pub(crate) fn rows_from_details(
    details: &serde_json::Value,
    message: &str,
    code: Option<&str>,
) -> Option<Vec<BatchRowProblem>> {
    let numbers: Vec<u32> = details
        .get("rowNumbers")?
        .as_array()?
        .iter()
        .filter_map(|n| n.as_u64().and_then(|n| u32::try_from(n).ok()))
        .collect();
    if numbers.is_empty() {
        return None;
    }
    let mut by_row: BTreeMap<u32, Vec<FieldProblem>> = BTreeMap::new();
    for row in details
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(n) = row
            .get("rowNumber")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
        else {
            continue;
        };
        let errors: Vec<ErrorWire> = row
            .get("errors")
            .cloned()
            .and_then(|e| serde_json::from_value(e).ok())
            .unwrap_or_default();
        by_row
            .entry(n)
            .or_default()
            .extend(errors.iter().map(ErrorWire::problem));
    }
    let fallback = strip_row_prefix(message);
    let mut out = Vec::new();
    for n in numbers {
        let problems = by_row
            .remove(&n)
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| {
                vec![FieldProblem::new(
                    BatchField::Row,
                    kind_of(code, fallback),
                    fallback,
                )]
            });
        out.extend(problems.into_iter().map(|problem| BatchRowProblem {
            group: 0,
            row_number: n,
            problem,
        }));
    }
    Some(out)
}

/// Puts each refused row in its group.
fn with_groups(e: ApiError, group_of: &BTreeMap<u32, u32>) -> ApiError {
    match e {
        ApiError::BatchRows(mut rows) => {
            for r in &mut rows {
                if let Some(g) = group_of.get(&r.row_number) {
                    r.group = *g;
                }
            }
            ApiError::BatchRows(rows)
        }
        other => other,
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateReply {
    rows: Vec<ValidateRowWire>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateRowWire {
    row_number: u32,
    #[serde(default)]
    group_index: Option<u32>,
    status: String,
    #[serde(default)]
    resolved_contact_id: Option<ContactId>,
    #[serde(default)]
    resolved_account_id: Option<AccountId>,
    #[serde(default)]
    errors: Vec<ErrorWire>,
    #[serde(default)]
    new_contact: Option<serde::de::IgnoredAny>,
}

fn validation_from(reply: ValidateReply, group_of: &BTreeMap<u32, u32>) -> BatchValidation {
    BatchValidation {
        rows: reply
            .rows
            .into_iter()
            .map(|r| {
                let problems: Vec<FieldProblem> = r.errors.iter().map(ErrorWire::problem).collect();
                RowCheck {
                    group: r
                        .group_index
                        .or_else(|| group_of.get(&r.row_number).copied())
                        .unwrap_or(0),
                    row_number: r.row_number,
                    ok: problems.is_empty() && matches!(r.status.as_str(), "valid" | "warning"),
                    contact: r.resolved_contact_id,
                    account: r.resolved_account_id,
                    problems,
                    new_contact: r.new_contact.is_some(),
                }
            })
            .collect(),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveReply {
    #[serde(default)]
    batch_id: Option<BatchId>,
    #[serde(default)]
    created_contacts: Vec<serde::de::IgnoredAny>,
    #[serde(default)]
    transactions: Vec<SavedWire>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedWire {
    row_number: u32,
    id: TransactionId,
}

fn saved_from(reply: SaveReply, group_of: &BTreeMap<u32, u32>) -> BatchSaved {
    BatchSaved {
        batch_id: reply.batch_id,
        saved: reply
            .transactions
            .into_iter()
            .map(|t| SavedTxn {
                group: group_of.get(&t.row_number).copied().unwrap_or(0),
                row_number: t.row_number,
                id: t.id,
            })
            .collect(),
        created_contacts: u32::try_from(reply.created_contacts.len()).unwrap_or(u32::MAX),
    }
}

/// An amount cell of `parse-csv`: a string (kept exactly), absent or
/// null; a JSON number may have been rounded and is refused.
fn csv_amount(v: Option<&serde_json::Value>) -> Result<Option<String>, ApiError> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok((!s.trim().is_empty()).then(|| s.clone())),
        Some(serde_json::Value::Number(_)) => Err(ApiError::InexactMoney {
            what: "parse-csv amounts",
        }),
        Some(_) => Err(ApiError::Decode {
            what: "parse-csv rows",
        }),
    }
}

fn csv_text(row: &serde_json::Value, key: &str) -> Result<Option<String>, ApiError> {
    match row.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok((!s.is_empty()).then(|| s.clone())),
        Some(_) => Err(ApiError::Decode {
            what: "parse-csv rows",
        }),
    }
}

/// Reads a `parse-csv` reply: `{rows: [...]}` with text cells and string
/// amounts.
pub(crate) fn parsed_from_json(body: &[u8]) -> Result<ParsedRows, ApiError> {
    let bad = || ApiError::Decode {
        what: "parse-csv rows",
    };
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|_| bad())?;
    let rows = v
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(bad)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let row_number = r
            .get("rowNumber")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(bad)?;
        out.push(ParsedRow {
            row_number,
            date: csv_text(r, "date")?,
            ref_no: csv_text(r, "refNo")?,
            contact_name: csv_text(r, "contactName")?,
            account_name: csv_text(r, "accountName")?,
            memo: csv_text(r, "memo")?,
            amount: csv_amount(r.get("amount"))?,
            debit: csv_amount(r.get("debit"))?,
            credit: csv_amount(r.get("credit"))?,
        });
    }
    Ok(ParsedRows { rows: out })
}

/// `multipart/form-data`: `txn_type`, the explicit `column_mapping` (a
/// JSON string) and the file part `batch.csv`. The boundary is random and
/// checked not to occur in the file.
fn csv_multipart(txn_type: &str, mapping: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = loop {
        let b = format!("cl-{}", uuid::Uuid::new_v4().simple());
        if !bytes.windows(b.len()).any(|w| w == b.as_bytes()) {
            break b;
        }
    };
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (field, value) in [("txn_type", txn_type), ("column_mapping", mapping)] {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{CSV_FILE_NAME}\"\r\nContent-Type: text/csv\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// True when a save was refused because its idempotency key was already
/// used with a different body (409 `IDEMPOTENCY_KEY_REUSED`): a new key is
/// needed only if the batch changed.
#[must_use]
pub fn is_key_reused(e: &ApiError) -> bool {
    matches!(
        e,
        ApiError::Conflict { code: Some(c), .. } if c.as_str() == "IDEMPOTENCY_KEY_REUSED"
    )
}

/// Bulk Entry calls of a session (`session.batch()`).
#[derive(Clone, Copy, Debug)]
pub struct Batch<'a>(pub(crate) &'a Session);

impl Session {
    /// Bulk Entry for the open company.
    #[must_use]
    pub fn batch(&self) -> Batch<'_> {
        Batch(self)
    }
}

impl Batch<'_> {
    /// The engine's `batch-*-v1` capabilities.
    #[must_use]
    pub fn caps(self) -> BatchCaps {
        BatchCaps::of(&self.0.client().capabilities())
    }

    /// Checks rows without posting anything: names and accounts are
    /// resolved and each row's problems reported. Closing-date passwords
    /// on the rows are not sent.
    ///
    /// With `batch-mixed-v1` the whole batch goes as `groups[]`; an older
    /// engine takes one section at a time (its one-type shape).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a malformed batch and
    /// `Local(BatchUnavailable)` for what the engine does not offer
    /// (nothing sent); any [`ApiError`].
    pub async fn validate(self, groups: &[BatchGroup]) -> Result<BatchValidation, ApiError> {
        let caps = self.caps();
        let prepared = prepare(groups, caps)?;
        let body = if caps.mixed {
            json_body(&ValidateWire {
                groups: prepared.groups,
            })?
        } else {
            let mut groups = prepared.groups;
            let (Some(one), true) = (groups.pop(), groups.is_empty()) else {
                return Err(unavailable());
            };
            json_body(&one)?
        };
        let call = Call::at(Method::Post, BATCH_VALIDATE.route()).json_body(body);
        let reply = self.0.send(&BATCH_VALIDATE, call, None).await?;
        let wire: ValidateReply = decode(&reply, "batch validation")?;
        Ok(validation_from(wire, &prepared.group_of))
    }

    /// Posts every row in one engine transaction: all rows are saved or
    /// none. `key` is kept for retries of the same batch (a replay returns
    /// the first result and posts nothing again); `add_new_names` adds
    /// names that are not in the lists.
    ///
    /// # Errors
    /// `Local(..)` with nothing sent: `BatchUnavailable` (the engine lacks
    /// an atomic, exact, idempotent grouped save or a kind),
    /// `InvalidValue` (a malformed batch, or two different passwords),
    /// `ClosedPeriodOverrideUnavailable` (a password without
    /// `batch-closing-v1`). From the engine: `BatchRows` (refused rows, in
    /// their groups), `ClosedPeriod`, `ClosingPasswordRejected`,
    /// `ClosingPasswordRateLimited`, `Conflict` (see [`is_key_reused`]);
    /// any [`ApiError`].
    pub async fn save(
        self,
        groups: &[BatchGroup],
        key: IdempotencyKey,
        add_new_names: bool,
    ) -> Result<BatchSaved, ApiError> {
        let caps = self.caps();
        if !(caps.can_save() && caps.mixed) {
            return Err(unavailable());
        }
        let prepared = prepare(groups, caps)?;
        let password = closing_password(groups)?;
        if password.is_some() && !caps.closing {
            return Err(ApiError::Local(
                LocalValidation::ClosedPeriodOverrideUnavailable,
            ));
        }
        let body = json_body(&SaveWire {
            groups: prepared.groups,
            auto_create_contacts: add_new_names,
            closing_password: password.map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Post, BATCH_SAVE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self
            .0
            .send(&BATCH_SAVE, call, None)
            .await
            .map_err(|e| with_groups(e, &prepared.group_of))?;
        let wire: SaveReply = decode(&reply, "batch save").map_err(saved_unreadable)?;
        Ok(saved_from(wire, &prepared.group_of))
    }

    /// The account to suggest for a name: its default, else the account of
    /// its most recent transaction. Works offline (no AI).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn suggest_account(
        self,
        contact: ContactId,
    ) -> Result<Option<SuggestedAccount>, ApiError> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct W {
            #[serde(default)]
            account_id: Option<AccountId>,
            #[serde(default)]
            source: Option<String>,
        }
        let mut call = Call::at(Method::Get, BATCH_SUGGEST_ACCOUNT.route());
        call.query = vec![("contact_id".into(), contact.to_string())];
        let reply = self.0.send(&BATCH_SUGGEST_ACCOUNT, call, None).await?;
        let w: W = decode(&reply, "suggested account")?;
        let source = match w.source.as_deref() {
            Some("default") => SuggestSource::Default,
            Some("recent") => SuggestSource::Recent,
            // No suggestion, or a source this version does not know.
            _ => return Ok(None),
        };
        Ok(w.account_id
            .map(|account| SuggestedAccount { account, source }))
    }

    /// Reads a CSV file or a pasted block (tabs are detected by the
    /// engine) into rows of text with the explicit `mapping`. The first
    /// line is the header; amounts come back as exact strings for cl-model
    /// to parse with the owner's money rules.
    ///
    /// # Errors
    /// `Local(BatchFileSize)` (empty or over 10 MB) or
    /// `Local(InvalidValue)` (an empty mapping), nothing sent;
    /// `InexactMoney` when the engine sends amounts as numbers (an engine
    /// without `batch-exact-v1`); `Validation` (`CSV_EMPTY`,
    /// `CSV_MALFORMED`, `CSV_TOO_MANY_ROWS`); any [`ApiError`].
    pub async fn parse_csv(
        self,
        kind: BatchKind,
        bytes: &[u8],
        mapping: &ColumnMapping,
    ) -> Result<ParsedRows, ApiError> {
        if bytes.is_empty() || bytes.len() > PARSE_CSV_MAX_BYTES {
            return Err(ApiError::Local(LocalValidation::BatchFileSize));
        }
        if mapping.0.is_empty() {
            return Err(invalid());
        }
        // The kind only steers the engine's own column guessing, which the
        // explicit mapping replaces; an engine without check rows reads a
        // check file as expenses.
        let txn_type = if self.caps().offers(kind) {
            kind.wire()
        } else {
            BatchKind::Expense.wire()
        };
        let mapping = serde_json::to_string(mapping).map_err(|_| invalid())?;
        let (boundary, body) = csv_multipart(txn_type, &mapping, bytes);
        let call = Call::at(Method::Post, BATCH_PARSE_CSV.route()).multipart_body(&boundary, body);
        let reply = self.0.send(&BATCH_PARSE_CSV, call, None).await?;
        parsed_from_json(&reply.body)
    }
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod call_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn money(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }

    #[test]
    fn amounts_are_whole_positive_cents_sent_as_strings() {
        let a = BatchAmount::new(money("1234.5")).expect("ok");
        assert_eq!(a.to_wire(), "1234.50");
        assert_eq!(serde_json::to_string(&a).expect("json"), "\"1234.50\"");
        assert_eq!(
            BatchAmount::new(money("0.01")).expect("ok").to_wire(),
            "0.01"
        );
        assert_eq!(
            BatchAmount::new(money("2.505")),
            Err(LocalValidation::NotWholeCents)
        );
        assert_eq!(
            BatchAmount::new(money("0")),
            Err(LocalValidation::NonPositiveAmount)
        );
        assert_eq!(
            BatchAmount::try_from(money("-5")),
            Err(LocalValidation::NonPositiveAmount)
        );
        assert_eq!(format!("{a:?}"), "BatchAmount(..)");
    }

    #[test]
    fn kinds_round_trip_their_wire_names() {
        for k in BatchKind::ALL {
            assert_eq!(BatchKind::from_wire(k.wire()), Some(k));
            assert_eq!(
                serde_json::to_string(&k).expect("json"),
                format!("\"{}\"", k.wire())
            );
        }
        assert_eq!(BatchKind::from_wire("invoice"), None);
        assert!(!BatchKind::JournalEntry.has_context());
        assert!(BatchKind::Deposit.has_context());
    }

    #[test]
    fn caps_gate_saving_and_checks() {
        let none = BatchCaps::of(&Capabilities::default());
        assert!(!none.can_save());
        assert!(!none.offers(BatchKind::Check));
        assert!(none.offers(BatchKind::Expense));
        let all: Capabilities = [
            Capabilities::BATCH_ATOMIC,
            Capabilities::BATCH_CHECK,
            Capabilities::BATCH_MIXED,
            Capabilities::BATCH_EXACT,
            Capabilities::BATCH_IDEMPOTENT,
            Capabilities::BATCH_CLOSING,
        ]
        .into_iter()
        .collect();
        let caps = BatchCaps::of(&all);
        assert!(caps.can_save() && caps.offers(BatchKind::Check) && caps.mixed);
        let no_exact = BatchCaps {
            exact: false,
            ..caps
        };
        assert!(!no_exact.can_save(), "no float money (non-negotiable 6)");
    }

    #[test]
    fn debug_shows_no_owner_data() {
        let date = CalendarDate::from_ymd(2026, 10, 21).expect("date");
        let mut row = BatchRowIn::new(3, date);
        row.contact_name = Some("Secret Vendor".into());
        row.memo = Some("private memo".into());
        row.amount = Some(BatchAmount::new(money("987.65")).expect("ok"));
        row.closing_password = Some(Secret::new("hunter2"));
        let group = BatchGroup {
            kind: BatchKind::Check,
            context: None,
            rows: vec![row.clone()],
        };
        let parsed = ParsedRow {
            row_number: 1,
            contact_name: Some("Secret Vendor".into()),
            amount: Some("987.65".into()),
            ..ParsedRow::default()
        };
        let problem = FieldProblem::new(
            BatchField::Contact,
            ProblemKind::NotInLists,
            "Secret Vendor",
        );
        for text in [
            format!("{row:?}"),
            format!("{group:?}"),
            format!("{parsed:?}"),
            format!("{problem:?}"),
        ] {
            for leak in ["Secret Vendor", "private memo", "987", "hunter2"] {
                assert!(!text.contains(leak), "{text}");
            }
        }
    }

    #[test]
    fn mapping_is_a_field_to_column_object() {
        let mut m = ColumnMapping::default();
        m.0.insert(CsvField::Date, 0);
        m.0.insert(CsvField::RefNo, 1);
        m.0.insert(CsvField::Amount, 4);
        assert_eq!(
            serde_json::to_string(&m).expect("json"),
            r#"{"date":0,"ref_no":1,"amount":4}"#
        );
        assert_eq!(CsvField::ALL.len(), 8);
    }

    #[test]
    fn refused_rows_are_their_own_error() {
        let e = BatchRowProblem::into_error(vec![BatchRowProblem {
            group: 0,
            row_number: 37,
            problem: FieldProblem::new(BatchField::Account, ProblemKind::Inactive, ""),
        }]);
        assert!(matches!(&e, ApiError::BatchRows(p) if p[0].row_number == 37));
        assert_eq!(e.to_string(), "1 bulk entry rows refused");
    }
}
