//! Bulk Entry (P15A): sections, rows and local checks, with no egui and no
//! engine types.
//!
//! - A batch is a list of [`Section`]s; each section holds rows of one
//!   [`SectionKind`] on one context account (none for journal entries) and
//!   becomes one engine group (`target/plans/P15A.md` §1).
//! - [`local_problems`] finds what is plainly wrong before the engine sees
//!   a row: required cells, amounts that are not positive whole cents, a
//!   journal entry with both or neither side, journal entry groups (same
//!   date and Entry No., Q-P15Aj) that do not balance exactly, check
//!   numbers used twice in the batch, and dates in a closed period (a lock
//!   mark, not an error, when the engine takes the closing password).
//! - Blank rows are ignored everywhere ([`RowDraft::is_blank`]).
//!
//! Money is exact: every amount is a [`Money`] rounded to the cent when it
//! is typed or pasted ([`parse_amount`], Q-232). Sums use `i128`
//! ten-thousandths, so nothing overflows or rounds. Balances are never
//! derived here (non-negotiable 7).
//!
//! cl-model may not depend on cl-api (layering): the engine-facing values
//! are plain types ([`SectionKind::engine_kind`], [`OutGroup`], [`OutRow`])
//! that the screen maps 1:1 onto `cl_api::batch::{BatchGroup, BatchRowIn}`.

use std::collections::BTreeMap;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Serialize};

use crate::amount_entry::{self, AmountEntryError, AmountRules};
pub use crate::rowkey::{RowKey, RowKeyGen};

mod paste;
mod state;
pub use paste::{
    ImportedRow, MAX_REMEMBERED, Mapping, MappingMemory, ParsedCells, RememberedMapping,
    default_mapping, looks_like_header, mappable_fields, positional_mapping, rows_from_parsed,
};
pub use state::{
    Blocker, EngineChecks, EngineProblem, EngineProblemKind, EngineState, EngineVerdict, OutGroup,
    OutRow, Outgoing, SaveCaps, SectionTotals, SentRow, problems_by_row,
};

/// The most rows one batch holds (Q-P15Ag).
pub const MAX_ROWS: usize = 2_000;
/// The longest check number the engine takes (digits).
pub const MAX_CHECK_NUMBER_DIGITS: usize = 9;
/// The longest journal Entry No. the engine takes.
pub const MAX_ENTRY_NO_CHARS: usize = 20;
/// The longest Ref no. the engine takes.
pub const MAX_REF_CHARS: usize = 50;
/// The longest memo the engine takes.
pub const MAX_MEMO_CHARS: usize = 2_000;

/// Ten-thousandth units in one cent (cl-core's fixed scale is 4).
const UNITS_PER_CENT: i128 = 100;

// ------------------------------------------------------------------ kinds

/// A section's type (Q-P15Ab, Q-263).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Checks from a bank account.
    Checks,
    /// Expenses (no check number) from a bank account.
    Expenses,
    /// Charges on a credit card account.
    CardCharges,
    /// Credits (refunds) on a credit card account.
    CardCredits,
    /// Deposits into a bank account.
    Deposits,
    /// Journal entries (no context account).
    JournalEntries,
}

/// Which accounts a section's Account picker offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ContextKind {
    /// Bank accounts.
    Bank,
    /// Credit card accounts.
    Card,
}

impl SectionKind {
    /// Every kind, in the order the Type picker lists them.
    pub const ALL: [SectionKind; 6] = [
        SectionKind::Checks,
        SectionKind::Expenses,
        SectionKind::CardCharges,
        SectionKind::CardCredits,
        SectionKind::Deposits,
        SectionKind::JournalEntries,
    ];

    /// The grid columns, left to right (plan §3 WP-D1).
    #[must_use]
    pub const fn columns(self) -> &'static [Field] {
        match self {
            SectionKind::Checks => &[
                Field::Date,
                Field::Number,
                Field::PrintLater,
                Field::Contact,
                Field::Account,
                Field::Amount,
                Field::Memo,
                Field::Tag,
            ],
            SectionKind::Expenses
            | SectionKind::CardCharges
            | SectionKind::CardCredits
            | SectionKind::Deposits => &[
                Field::Date,
                Field::Number,
                Field::Contact,
                Field::Account,
                Field::Amount,
                Field::Memo,
                Field::Tag,
            ],
            SectionKind::JournalEntries => &[
                Field::Date,
                Field::Number,
                Field::Account,
                Field::Debit,
                Field::Credit,
                Field::Contact,
                Field::Memo,
                Field::Tag,
            ],
        }
    }

    /// The context account the section needs: a bank account for checks,
    /// expenses and deposits, a card for card charges and credits, none for
    /// journal entries.
    #[must_use]
    pub const fn needs_context(self) -> Option<ContextKind> {
        match self {
            SectionKind::Checks | SectionKind::Expenses | SectionKind::Deposits => {
                Some(ContextKind::Bank)
            }
            SectionKind::CardCharges | SectionKind::CardCredits => Some(ContextKind::Card),
            SectionKind::JournalEntries => None,
        }
    }

    /// The engine's `txn_type` (`cl_api::batch::BatchKind::from_wire`).
    #[must_use]
    pub const fn engine_kind(self) -> &'static str {
        match self {
            SectionKind::Checks => "check",
            SectionKind::Expenses => "expense",
            SectionKind::CardCharges => "credit_card_charge",
            SectionKind::CardCredits => "credit_card_credit",
            SectionKind::Deposits => "deposit",
            SectionKind::JournalEntries => "journal_entry",
        }
    }

    /// True for journal entries (Debit and Credit instead of Amount).
    #[must_use]
    pub const fn is_journal(self) -> bool {
        matches!(self, SectionKind::JournalEntries)
    }

    /// The longest text the Number cell takes.
    #[must_use]
    pub const fn max_number_chars(self) -> usize {
        match self {
            SectionKind::Checks => MAX_CHECK_NUMBER_DIGITS,
            SectionKind::JournalEntries => MAX_ENTRY_NO_CHARS,
            _ => MAX_REF_CHARS,
        }
    }

    const fn hash_tag(self) -> u8 {
        match self {
            SectionKind::Checks => 1,
            SectionKind::Expenses => 2,
            SectionKind::CardCharges => 3,
            SectionKind::CardCredits => 4,
            SectionKind::Deposits => 5,
            SectionKind::JournalEntries => 6,
        }
    }
}

/// A cell of a Bulk Entry row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// Date.
    Date,
    /// Check number, Ref no. or Entry No.
    Number,
    /// Print later (checks).
    PrintLater,
    /// Pay to, Payee, Received from or Name.
    Contact,
    /// The line account.
    Account,
    /// Amount.
    Amount,
    /// Debit (journal entries).
    Debit,
    /// Credit (journal entries).
    Credit,
    /// Memo.
    Memo,
    /// Tag (one per line, Q-233).
    Tag,
}

// ------------------------------------------------------------------- rows

/// One row as the owner typed or pasted it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RowDraft {
    /// The row's identity (stable across inserts, deletes and undo; the
    /// engine's row numbers are assigned at send time).
    pub key: RowKey,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Check number, Ref no. or Entry No.
    pub number: String,
    /// Print later (checks).
    pub print_later: bool,
    /// The name, when picked from the list.
    pub contact: Option<ContactId>,
    /// The name as typed.
    pub contact_text: String,
    /// The line account, when picked from the list.
    pub account: Option<AccountId>,
    /// The line account as typed.
    pub account_text: String,
    /// Amount (every kind but journal entries).
    pub amount: Option<Money>,
    /// Debit (journal entries).
    pub debit: Option<Money>,
    /// Credit (journal entries).
    pub credit: Option<Money>,
    /// Memo.
    pub memo: String,
    /// Tag.
    pub tag: Option<TagId>,
}

/// The key and which cells are filled only: no name, memo or amount.
impl std::fmt::Debug for RowDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowDraft")
            .field("key", &self.key)
            .field("date", &self.date.is_some())
            .field("contact", &self.contact)
            .field("account", &self.account)
            .field("amount", &self.amount.is_some())
            .field("debit", &self.debit.is_some())
            .field("credit", &self.credit.is_some())
            .finish_non_exhaustive()
    }
}

impl RowDraft {
    /// An empty row with `key`.
    #[must_use]
    pub fn new(key: RowKey) -> RowDraft {
        RowDraft {
            key,
            ..RowDraft::default()
        }
    }

    /// True when the owner has not entered anything that makes this a
    /// transaction: no name, account, amount, memo or tag. The date, the
    /// number and Print later alone do not count, because new rows get them
    /// filled in (today, the next check number, Q-242). Blank rows are never
    /// checked or sent.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        !self.has_contact()
            && !self.has_account()
            && self.amount.is_none()
            && self.debit.is_none()
            && self.credit.is_none()
            && self.memo.trim().is_empty()
            && self.tag.is_none()
    }

    /// A name is picked or typed.
    #[must_use]
    pub fn has_contact(&self) -> bool {
        self.contact.is_some() || !self.contact_text.trim().is_empty()
    }

    /// An account is picked or typed.
    #[must_use]
    pub fn has_account(&self) -> bool {
        self.account.is_some() || !self.account_text.trim().is_empty()
    }

    /// A hash of the row's content (everything but the key), stable across
    /// runs and platforms. An engine result counts only while it is
    /// unchanged.
    #[must_use]
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::new();
        self.feed(&mut h);
        h.finish()
    }

    fn feed(&self, h: &mut Fnv) {
        h.opt(self.date.map(CalendarDate::to_wire).as_deref());
        h.str(self.number.trim());
        h.byte(u8::from(self.print_later));
        h.opt(self.contact.map(|c| c.to_string()).as_deref());
        h.str(self.contact_text.trim());
        h.opt(self.account.map(|a| a.to_string()).as_deref());
        h.str(self.account_text.trim());
        h.money(self.amount);
        h.money(self.debit);
        h.money(self.credit);
        h.str(&self.memo);
        h.opt(self.tag.map(|t| t.to_string()).as_deref());
    }
}

/// Parse a typed or pasted amount with the money rules: a plain amount or
/// a calculation, rounded to the cent half away from zero (Q-232, silent
/// per Q-237). `Ok(None)` for an empty cell. A negative result is kept, so
/// the row shows [`Problem::NotPositive`].
///
/// # Errors
/// [`AmountEntryError`] when the text is not an amount.
pub fn parse_amount(text: &str) -> Result<Option<Money>, AmountEntryError> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let parsed = amount_entry::parse(text, AmountRules::MONEY)?;
    // `AmountRules::MONEY` is a money field, so the value is money.
    Ok(parsed.value.as_money())
}

// --------------------------------------------------------------- sections

/// One section: rows of one kind on one context account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// What the rows become.
    pub kind: SectionKind,
    /// The bank or card account (`None` for journal entries).
    pub context: Option<AccountId>,
    /// The rows, in grid order.
    pub rows: Vec<RowDraft>,
}

impl Section {
    /// An empty section.
    #[must_use]
    pub fn new(kind: SectionKind, context: Option<AccountId>) -> Section {
        Section {
            kind,
            context,
            rows: Vec::new(),
        }
    }

    /// The rows that are not blank, in grid order.
    pub fn filled(&self) -> impl Iterator<Item = &RowDraft> {
        self.rows.iter().filter(|r| !r.is_blank())
    }

    /// How many rows are not blank.
    #[must_use]
    pub fn filled_count(&self) -> usize {
        self.filled().count()
    }

    /// The journal entry groups: rows with a date and an Entry No., keyed by
    /// both (Q-P15Aj). Empty for other kinds.
    #[must_use]
    pub fn je_groups(&self) -> BTreeMap<JeGroupKey, Vec<&RowDraft>> {
        let mut groups: BTreeMap<JeGroupKey, Vec<&RowDraft>> = BTreeMap::new();
        if !self.kind.is_journal() {
            return groups;
        }
        for row in self.filled() {
            if let Some(key) = JeGroupKey::of(row) {
                groups.entry(key).or_default().push(row);
            }
        }
        groups
    }

    /// The key of the journal entry group `row` belongs to.
    #[must_use]
    pub fn je_group_of(&self, row: &RowDraft) -> Option<JeGroupKey> {
        if self.kind.is_journal() {
            JeGroupKey::of(row)
        } else {
            None
        }
    }

    /// The problems this section shows on its own (check numbers are
    /// compared within the section only; [`BulkBatch::local_problems`]
    /// compares across sections on the same account).
    #[must_use]
    pub fn local_problems(&self, closing: Option<CalendarDate>) -> Vec<(RowKey, Problem)> {
        local_problems(self, closing)
    }
}

/// A journal entry group: the rows with the same date and Entry No.
/// (Q-P15Aj).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JeGroupKey {
    /// The entry date.
    pub date: CalendarDate,
    /// The Entry No. (trimmed).
    pub number: String,
}

impl JeGroupKey {
    fn of(row: &RowDraft) -> Option<JeGroupKey> {
        let number = row.number.trim();
        match row.date {
            Some(date) if !number.is_empty() => Some(JeGroupKey {
                date,
                number: number.to_string(),
            }),
            _ => None,
        }
    }
}

// --------------------------------------------------------------- problems

/// What the app itself finds wrong with a row, before the engine checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A required cell is empty.
    Missing(Field),
    /// An amount is zero or negative.
    NotPositive,
    /// An amount has digits below the cent. Typed and pasted amounts are
    /// rounded (Q-232), so this only catches a value set some other way.
    SubCent(Field),
    /// A journal entry row has both a debit and a credit.
    BothDebitCredit,
    /// The check number is used twice in this batch on the same account.
    DuplicateNumber,
    /// The check number is not 1 to 9 digits (or is zero).
    BadCheckNumber,
    /// A check marked Print later has a number (the number is given when
    /// the check is printed).
    PrintLaterNumber,
    /// The text is longer than the engine takes.
    TooLong(Field),
    /// The line account is the section's own bank or card account.
    SameAsContext,
    /// A journal entry group's debits and credits differ by `difference`.
    JeUnbalanced {
        /// The group.
        group: JeGroupKey,
        /// Debits minus credits (exact).
        difference: Money,
    },
    /// The amounts are too large to add up.
    TooLarge,
    /// The date is on or before the closing date: a lock mark. The save asks
    /// for the closing-date password once (Q-P15Ae); it blocks the save only
    /// when the engine cannot take the password.
    Closed,
}

impl Problem {
    /// True for the closed-period lock mark, which is not an error by
    /// itself.
    #[must_use]
    pub const fn is_lock_mark(&self) -> bool {
        matches!(self, Problem::Closed)
    }

    /// The cell the problem is shown on (`None`: the row as a whole).
    #[must_use]
    pub const fn field(&self) -> Option<Field> {
        match self {
            Problem::Missing(f) | Problem::SubCent(f) | Problem::TooLong(f) => Some(*f),
            Problem::DuplicateNumber | Problem::BadCheckNumber | Problem::PrintLaterNumber => {
                Some(Field::Number)
            }
            Problem::SameAsContext => Some(Field::Account),
            Problem::Closed => Some(Field::Date),
            Problem::NotPositive
            | Problem::BothDebitCredit
            | Problem::JeUnbalanced { .. }
            | Problem::TooLarge => None,
        }
    }
}

/// True when `m` is a whole number of cents.
fn whole_cents(m: Money) -> bool {
    m.to_ten_thousandths() % UNITS_PER_CENT == 0
}

fn amount_problems(field: Field, m: Money, out: &mut Vec<Problem>) {
    if !m.is_positive() {
        out.push(Problem::NotPositive);
    } else if !whole_cents(m) {
        out.push(Problem::SubCent(field));
    }
}

/// A check number the engine takes: 1 to 9 digits, not zero.
fn valid_check_number(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= MAX_CHECK_NUMBER_DIGITS
        && n.bytes().all(|b| b.is_ascii_digit())
        && n.bytes().any(|b| b != b'0')
}

/// The problems of one row on its own (no group or duplicate checks).
fn row_problems(
    kind: SectionKind,
    context: Option<AccountId>,
    row: &RowDraft,
    closing: Option<CalendarDate>,
) -> Vec<Problem> {
    let mut out = Vec::new();
    match row.date {
        None => out.push(Problem::Missing(Field::Date)),
        Some(d) if closing.is_some_and(|c| d <= c) => out.push(Problem::Closed),
        Some(_) => {}
    }
    let number = row.number.trim();
    if number.chars().count() > kind.max_number_chars() && kind != SectionKind::Checks {
        out.push(Problem::TooLong(Field::Number));
    }
    if !row.has_account() {
        out.push(Problem::Missing(Field::Account));
    } else if row.account.is_some() && row.account == context {
        out.push(Problem::SameAsContext);
    }
    if kind.is_journal() {
        if number.is_empty() {
            out.push(Problem::Missing(Field::Number));
        }
        match (row.debit, row.credit) {
            (Some(_), Some(_)) => out.push(Problem::BothDebitCredit),
            (None, None) => out.push(Problem::Missing(Field::Debit)),
            _ => {}
        }
        if let Some(d) = row.debit {
            amount_problems(Field::Debit, d, &mut out);
        }
        if let Some(c) = row.credit {
            amount_problems(Field::Credit, c, &mut out);
        }
    } else {
        match row.amount {
            None => out.push(Problem::Missing(Field::Amount)),
            Some(m) => amount_problems(Field::Amount, m, &mut out),
        }
    }
    if kind == SectionKind::Checks {
        if row.print_later {
            if !number.is_empty() {
                out.push(Problem::PrintLaterNumber);
            }
            if !row.has_contact() {
                out.push(Problem::Missing(Field::Contact));
            }
        } else if !number.is_empty() && !valid_check_number(number) {
            out.push(Problem::BadCheckNumber);
        }
    }
    if row.memo.chars().count() > MAX_MEMO_CHARS {
        out.push(Problem::TooLong(Field::Memo));
    }
    // One NotPositive per row is enough.
    out.dedup();
    out
}

/// Debits minus credits of a journal entry group, exact.
fn je_difference(rows: &[&RowDraft]) -> i128 {
    rows.iter()
        .map(|r| {
            r.debit.map_or(0, Money::to_ten_thousandths)
                - r.credit.map_or(0, Money::to_ten_thousandths)
        })
        .sum()
}

/// The group problems of a journal entry section.
fn je_problems(section: &Section, out: &mut Vec<(RowKey, Problem)>) {
    for (group, rows) in section.je_groups() {
        let diff = je_difference(&rows);
        if diff == 0 {
            continue;
        }
        let problem = match Money::from_ten_thousandths(diff) {
            Ok(difference) => Problem::JeUnbalanced {
                group: group.clone(),
                difference,
            },
            Err(_) => Problem::TooLarge,
        };
        for r in rows {
            out.push((r.key, problem.clone()));
        }
    }
}

/// The comparison form of a check number: digits without leading zeros.
fn check_number_key(n: &str) -> Option<String> {
    let n = n.trim();
    valid_check_number(n).then(|| n.trim_start_matches('0').to_string())
}

/// Marks every numbered check row whose number appears more than once
/// among `rows` (all on one bank account).
fn duplicate_numbers<'a>(
    rows: impl IntoIterator<Item = &'a RowDraft>,
    out: &mut Vec<(RowKey, Problem)>,
) {
    let mut seen: BTreeMap<String, Vec<RowKey>> = BTreeMap::new();
    for r in rows {
        if r.print_later {
            continue;
        }
        if let Some(k) = check_number_key(&r.number) {
            seen.entry(k).or_default().push(r.key);
        }
    }
    for keys in seen.into_values().filter(|k| k.len() > 1) {
        out.extend(keys.into_iter().map(|k| (k, Problem::DuplicateNumber)));
    }
}

fn section_problems(section: &Section, closing: Option<CalendarDate>) -> Vec<(RowKey, Problem)> {
    let mut out = Vec::new();
    for row in section.filled() {
        out.extend(
            row_problems(section.kind, section.context, row, closing)
                .into_iter()
                .map(|p| (row.key, p)),
        );
    }
    if section.kind.is_journal() {
        je_problems(section, &mut out);
    }
    out
}

/// The problems the app finds in one section by itself: missing cells,
/// amounts that are not positive whole cents, journal entry rows with both
/// or neither side, unbalanced journal entry groups (every row of the group
/// gets the difference), duplicate check numbers within the section, and
/// the closed-period lock mark for dates on or before `closing`. Blank rows
/// are ignored.
#[must_use]
pub fn local_problems(section: &Section, closing: Option<CalendarDate>) -> Vec<(RowKey, Problem)> {
    let mut out = section_problems(section, closing);
    if section.kind == SectionKind::Checks {
        duplicate_numbers(section.filled(), &mut out);
    }
    out
}

// ------------------------------------------------------------------ batch

/// The whole Bulk Entry window's editable content: the value a
/// `FormState` holds (undo covers it; engine results live in
/// `EngineChecks`, outside undo).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BulkBatch {
    /// The sections, in strip order.
    pub sections: Vec<Section>,
    /// "Add new names to the list" (off by default, Q-P15Ad).
    pub add_new_names: bool,
}

impl BulkBatch {
    /// A batch with one section.
    #[must_use]
    pub fn new(first: Section) -> BulkBatch {
        BulkBatch {
            sections: vec![first],
            add_new_names: false,
        }
    }

    /// Every row that is not blank, with its section index.
    pub fn filled(&self) -> impl Iterator<Item = (usize, &RowDraft)> {
        self.sections
            .iter()
            .enumerate()
            .flat_map(|(i, s)| s.filled().map(move |r| (i, r)))
    }

    /// How many rows are not blank.
    #[must_use]
    pub fn filled_count(&self) -> usize {
        self.filled().count()
    }

    /// The row with `key` and its section index.
    #[must_use]
    pub fn find(&self, key: RowKey) -> Option<(usize, &RowDraft)> {
        self.sections
            .iter()
            .enumerate()
            .find_map(|(i, s)| s.rows.iter().find(|r| r.key == key).map(|r| (i, r)))
    }

    /// Every row key in the batch (blank rows too), for
    /// [`RowKeyGen::after`].
    pub fn keys(&self) -> impl Iterator<Item = RowKey> + '_ {
        self.sections
            .iter()
            .flat_map(|s| s.rows.iter().map(|r| r.key))
    }

    /// The problems the app finds, for every section: as
    /// [`local_problems`], with check numbers compared across all Checks
    /// sections on the same bank account.
    #[must_use]
    pub fn local_problems(&self, closing: Option<CalendarDate>) -> Vec<(RowKey, Problem)> {
        let mut out = Vec::new();
        let mut checks_by_account: BTreeMap<Option<AccountId>, Vec<&RowDraft>> = BTreeMap::new();
        for s in &self.sections {
            out.extend(section_problems(s, closing));
            if s.kind == SectionKind::Checks {
                checks_by_account
                    .entry(s.context)
                    .or_default()
                    .extend(s.filled());
            }
        }
        for rows in checks_by_account.into_values() {
            duplicate_numbers(rows, &mut out);
        }
        out
    }

    /// The rows dated on or before `closing` (they need the closing-date
    /// password, Q-P15Ae).
    #[must_use]
    pub fn locked_rows(&self, closing: Option<CalendarDate>) -> Vec<RowKey> {
        let Some(c) = closing else {
            return Vec::new();
        };
        self.filled()
            .filter(|(_, r)| r.date.is_some_and(|d| d <= c))
            .map(|(_, r)| r.key)
            .collect()
    }

    /// The next check number for a new row on `account` (Q-242): one more
    /// than the highest of `highest_in_books` and every check number in the
    /// batch's Checks sections on that account. `None` when there is
    /// nothing to count from.
    #[must_use]
    pub fn next_check_number(
        &self,
        account: AccountId,
        highest_in_books: Option<u64>,
    ) -> Option<String> {
        let in_batch = self
            .sections
            .iter()
            .filter(|s| s.kind == SectionKind::Checks && s.context == Some(account))
            .flat_map(|s| s.rows.iter())
            .filter_map(|r| check_number_key(&r.number).and_then(|k| k.parse::<u64>().ok()))
            .max();
        let highest = in_batch.max(highest_in_books)?;
        let next = highest.checked_add(1)?;
        let text = next.to_string();
        (text.len() <= MAX_CHECK_NUMBER_DIGITS).then_some(text)
    }
}

// ------------------------------------------------------------------- hash

/// 64-bit FNV-1a over a length-prefixed encoding: stable across runs and
/// platforms (unlike `std`'s hasher), so a row's hash means the same thing
/// for as long as the window is open and the batch hash can seed an
/// idempotency key.
struct Fnv(u64);

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Fnv {
        Fnv(Fnv::OFFSET)
    }

    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(Fnv::PRIME);
        }
    }

    fn byte(&mut self, b: u8) {
        self.bytes(&[b]);
    }

    fn str(&mut self, s: &str) {
        self.bytes(&(s.len() as u64).to_le_bytes());
        self.bytes(s.as_bytes());
    }

    fn opt(&mut self, s: Option<&str>) {
        match s {
            None => self.byte(0),
            Some(s) => {
                self.byte(1);
                self.str(s);
            }
        }
    }

    fn money(&mut self, m: Option<Money>) {
        match m {
            None => self.byte(0),
            Some(m) => {
                self.byte(1);
                self.bytes(&m.to_ten_thousandths().to_le_bytes());
            }
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "batch/state_tests.rs"]
mod state_tests;

#[cfg(test)]
#[path = "batch/paste_tests.rs"]
mod paste_tests;
