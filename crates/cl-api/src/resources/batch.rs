//! Bulk Entry (P15A, `/api/v1/batch/*`): the request and reply types.
//!
//! WP0 scaffold: types only. The calls (`validate`, `save`,
//! `suggest_account`, `parse_csv`), the body builders and the engine error
//! mapping arrive in lane A (`target/plans/P15A.md` §3 WP-A1).
//!
//! - A Bulk Entry window holds sections; each section is one
//!   [`BatchGroup`] (one [`BatchKind`] and one context account), and a
//!   whole batch goes to the engine in one call (`batch-mixed-v1`).
//! - Amounts are [`BatchAmount`]s: whole cents, positive, exact
//!   ([`Money`]), sent as `"123.45"` strings. cl-model rounds what the
//!   owner typed (Q-232) before a row is built, so a sub-cent value here
//!   is a bug and is refused by the type, never rounded.
//! - The closing-date password rides on each locked row as a [`Secret`]
//!   (`batch-closing-v1`): body only, never logged.
//! - Names, memos, amounts and passwords never appear in `Debug`: ids,
//!   row numbers and counts only.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, BatchId, ContactId, TagId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde::{Serialize, Serializer};

use crate::errors::{ApiError, LocalValidation, UserText};
use crate::session::Session;
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
    /// 1-based position in the group, captured at send time; the engine
    /// names failing rows by it.
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
    /// date (`batch-closing-v1`); body only.
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
    /// The row's 1-based `row_number` in its group.
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

/// Bulk Entry calls of a session (`session.batch()`); lane A adds them.
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
}

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
