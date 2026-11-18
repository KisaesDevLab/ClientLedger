//! Spreadsheet transaction import (P31 D5, Q-125): invoices, bills and
//! journal entries from a CSV the owner maps.
//!
//! The screen splits the file into cells (`cl_import::csv`); this module
//! decides everything else before anything is sent:
//!
//! - which file column fills which field ([`guess_mapping`], the saved
//!   mapping by header set: [`mapping_to_saved`], [`mapping_from_saved`]);
//! - how rows group into documents ([`build`]): by the Group column when it
//!   is mapped, else by type, date, number and name;
//! - every amount, exactly ([`parse_amount`], the engine's
//!   `parseImportAmount` table, plan §3 / E0): `rust_decimal`, the entered
//!   scale kept, never rounded, never a float;
//! - the row problems, in codes and row numbers the screen words for the
//!   owner ([`ProblemKind`]): required fields, unreadable amounts and dates,
//!   a group that mixes documents, a journal entry that doesn't balance;
//! - the canonical CSV the engine's `transactions` kind reads (E4,
//!   [`TxnPlan::canonical_csv`]), with a map from its rows back to the
//!   owner's ([`TxnPlan::source_row`]).
//!
//! No sales tax is worked out for spreadsheet invoices (Q-P31i): the lines
//! post as entered.
//!
//! `Debug` of anything holding cells shows counts and row numbers only:
//! never names, memos or amounts.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::CalendarDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::forms::batch::{normalize_header, signature};

/// The most data rows one file may have (the engine's cap).
pub const MAX_ROWS: usize = 50_000;
/// The most decimal places an amount may have (numeric(19,4)).
pub const MAX_PLACES: u32 = 4;
/// The most digits before the decimal point (numeric(19,4)).
pub const MAX_WHOLE_DIGITS: usize = 15;

/// A field of the engine's `transactions` kind (E4), in its column order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxnField {
    /// Rows with the same value make one document.
    GroupId,
    /// Invoice, bill or journal entry.
    Type,
    /// The document date.
    Date,
    /// Invoice, bill or entry number.
    Number,
    /// Customer or vendor.
    Name,
    /// When an invoice or bill is due.
    DueDate,
    /// The document memo.
    Memo,
    /// The line's account.
    LineAccount,
    /// The line's product or service.
    LineItem,
    /// The line's description.
    LineDescription,
    /// One signed amount (invoice and bill lines; a positive journal line is
    /// a debit).
    LineAmount,
    /// A journal line's debit.
    LineDebit,
    /// A journal line's credit.
    LineCredit,
    /// The line's tag.
    LineTag,
}

impl TxnField {
    /// Every field, in the canonical column order.
    pub const ALL: [TxnField; 14] = [
        TxnField::GroupId,
        TxnField::Type,
        TxnField::Date,
        TxnField::Number,
        TxnField::Name,
        TxnField::DueDate,
        TxnField::Memo,
        TxnField::LineAccount,
        TxnField::LineItem,
        TxnField::LineDescription,
        TxnField::LineAmount,
        TxnField::LineDebit,
        TxnField::LineCredit,
        TxnField::LineTag,
    ];

    /// The engine's column name.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            TxnField::GroupId => "group_id",
            TxnField::Type => "type",
            TxnField::Date => "date",
            TxnField::Number => "number",
            TxnField::Name => "name",
            TxnField::DueDate => "due_date",
            TxnField::Memo => "memo",
            TxnField::LineAccount => "line_account",
            TxnField::LineItem => "line_item",
            TxnField::LineDescription => "line_description",
            TxnField::LineAmount => "line_amount",
            TxnField::LineDebit => "line_debit",
            TxnField::LineCredit => "line_credit",
            TxnField::LineTag => "line_tag",
        }
    }

    /// The field an engine column name names.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<TxnField> {
        TxnField::ALL.into_iter().find(|f| f.wire() == s)
    }

    /// A document-level field (the same on every row of a group).
    #[must_use]
    pub const fn is_header(self) -> bool {
        matches!(
            self,
            TxnField::Type
                | TxnField::Date
                | TxnField::Number
                | TxnField::Name
                | TxnField::DueDate
                | TxnField::Memo
        )
    }
}

/// Field → 0-based file column.
pub type TxnMapping = BTreeMap<TxnField, usize>;

/// The field a header names, if any.
fn alias(header: &str) -> Option<TxnField> {
    let h = normalize_header(header);
    let f = match h.as_str() {
        "group" | "group id" | "group_id" | "txn id" | "transaction id" | "entry id" | "doc id"
        | "document id" | "trans no" | "trans" => TxnField::GroupId,
        "type" | "txn type" | "transaction type" | "doc type" | "document type" => TxnField::Type,
        "date" | "txn date" | "transaction date" | "invoice date" | "bill date" | "entry date" => {
            TxnField::Date
        }
        "no" | "num" | "number" | "ref" | "ref no" | "reference" | "invoice no"
        | "invoice number" | "bill no" | "bill number" | "entry no" | "doc no" => TxnField::Number,
        "name" | "customer" | "vendor" | "payee" | "customer name" | "vendor name" | "contact" => {
            TxnField::Name
        }
        "due" | "due date" | "due_date" => TxnField::DueDate,
        "memo" | "notes" | "note" | "message" => TxnField::Memo,
        "account" | "line account" | "line_account" | "gl account" | "account name"
        | "category" | "split" => TxnField::LineAccount,
        "item" | "line item" | "line_item" | "product" | "service" | "product/service" => {
            TxnField::LineItem
        }
        "description" | "line description" | "line_description" | "line memo" | "details" => {
            TxnField::LineDescription
        }
        "amount" | "line amount" | "line_amount" | "total" => TxnField::LineAmount,
        "debit" | "debits" | "dr" | "line debit" | "line_debit" | "debit amount" => {
            TxnField::LineDebit
        }
        "credit" | "credits" | "cr" | "line credit" | "line_credit" | "credit amount" => {
            TxnField::LineCredit
        }
        "tag" | "class" | "line tag" | "line_tag" => TxnField::LineTag,
        _ => return None,
    };
    Some(f)
}

/// The mapping the headers suggest: each field takes the first column whose
/// header names it.
#[must_use]
pub fn guess_mapping(headers: &[&str]) -> TxnMapping {
    let mut out = TxnMapping::new();
    for (i, h) in headers.iter().enumerate() {
        if let Some(f) = alias(h) {
            out.entry(f).or_insert(i);
        }
    }
    out
}

/// The header set a saved mapping is filed under: normalized, sorted,
/// without repeats (a reordered file still finds it).
#[must_use]
pub fn header_signature(headers: &[&str]) -> String {
    signature(headers).join("\u{1f}")
}

/// A mapping as saved: field (engine name) → the normalized header it
/// came from. Holds header text only, never cells.
#[must_use]
pub fn mapping_to_saved(headers: &[&str], mapping: &TxnMapping) -> BTreeMap<String, String> {
    mapping
        .iter()
        .filter_map(|(f, col)| {
            let h = normalize_header(headers.get(*col)?);
            (!h.is_empty()).then(|| (f.wire().to_string(), h))
        })
        .collect()
}

/// A saved mapping resolved against this file's headers (fields whose
/// header is gone are left out).
#[must_use]
pub fn mapping_from_saved(headers: &[&str], saved: &BTreeMap<String, String>) -> TxnMapping {
    let normalized: Vec<String> = headers.iter().map(|h| normalize_header(h)).collect();
    saved
        .iter()
        .filter_map(|(f, name)| {
            let f = TxnField::from_wire(f)?;
            let i = normalized.iter().position(|h| h == name)?;
            Some((f, i))
        })
        .collect()
}

/// A column the mapping still needs before rows can be checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingColumn {
    /// Type.
    Type,
    /// Date.
    Date,
    /// Account or item.
    AccountOrItem,
    /// Amount, debit or credit.
    Amount,
}

/// What the mapping lacks (empty when rows can be checked). One file column
/// mapped to two fields is allowed (the owner's choice).
#[must_use]
pub fn mapping_gaps(mapping: &TxnMapping) -> Vec<MissingColumn> {
    let has = |f| mapping.contains_key(&f);
    let mut out = Vec::new();
    if !has(TxnField::Type) {
        out.push(MissingColumn::Type);
    }
    if !has(TxnField::Date) {
        out.push(MissingColumn::Date);
    }
    if !has(TxnField::LineAccount) && !has(TxnField::LineItem) {
        out.push(MissingColumn::AccountOrItem);
    }
    if !has(TxnField::LineAmount) && !has(TxnField::LineDebit) && !has(TxnField::LineCredit) {
        out.push(MissingColumn::Amount);
    }
    out
}

// ---- Amounts (plan §3, the engine's E0 table) ---------------------------

/// Why an amount cell can't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AmountProblem {
    /// Not a number the table accepts (`1e5`, `-(5)`, `1.234,56`, text).
    Invalid,
    /// More than four decimal places.
    TooManyPlaces,
    /// Too large for the books (more than 15 digits before the point).
    TooLarge,
}

/// Reads an amount exactly, as the engine's `parseImportAmount` does
/// (E0). Blank is `Ok(None)`; the caller decides whether blank is allowed.
///
/// Accepted: `$`, `,` thousands separators, `(x)` for negative, a leading
/// or trailing `-`, `$-5`, U+2212 for minus, non-breaking and other spaces
/// (stripped); in a signed column (`signed`), a trailing `CR` (negative) or
/// `DR` (positive). Rejected: exponents, more than four decimal places,
/// `-(5)`, a comma after the decimal point (`1.234,56`).
///
/// The entered scale is kept (`1,234.50` → `1234.50`).
///
/// # Errors
/// [`AmountProblem`] when the cell isn't an amount.
pub fn parse_amount(text: &str, signed: bool) -> Result<Option<Decimal>, AmountProblem> {
    let mut s: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '\u{a0}' && *c != '\u{202f}')
        .map(|c| if c == '\u{2212}' { '-' } else { c })
        .collect();
    if s.is_empty() {
        return Ok(None);
    }
    let mut negative = false;
    if signed {
        let upper = s.to_ascii_uppercase();
        if upper.ends_with("CR") {
            negative = true;
            s.truncate(s.len() - 2);
        } else if upper.ends_with("DR") {
            s.truncate(s.len() - 2);
        }
    }
    let mut signs = 0;
    if s.starts_with('(') && s.ends_with(')') && s.len() >= 2 {
        negative = !negative;
        signs += 1;
        s = s[1..s.len() - 1].to_string();
    }
    // `$` and one minus, in either order, in front; or one minus behind.
    let mut body = s.as_str();
    let mut dollar = false;
    loop {
        if let Some(rest) = body.strip_prefix('$').filter(|_| !dollar) {
            dollar = true;
            body = rest;
        } else if let Some(rest) = body.strip_prefix('-') {
            negative = !negative;
            signs += 1;
            body = rest;
        } else {
            break;
        }
    }
    if let Some(rest) = body.strip_suffix('-') {
        negative = !negative;
        signs += 1;
        body = rest;
    }
    if signs > 1 {
        return Err(AmountProblem::Invalid);
    }
    let (whole, frac) = match body.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (body, None),
    };
    let whole_digits: String = whole.chars().filter(|c| *c != ',').collect();
    let frac_ok = frac.is_none_or(|f| f.chars().all(|c| c.is_ascii_digit()));
    if whole.starts_with(',')
        || whole.ends_with(',')
        || whole.contains(",,")
        || !whole_digits.chars().all(|c| c.is_ascii_digit())
        || !frac_ok
        || (whole_digits.is_empty() && frac.is_none_or(str::is_empty))
    {
        return Err(AmountProblem::Invalid);
    }
    let frac = frac.unwrap_or("");
    if frac.len() > MAX_PLACES as usize {
        return Err(AmountProblem::TooManyPlaces);
    }
    let whole_digits = whole_digits.trim_start_matches('0');
    if whole_digits.len() > MAX_WHOLE_DIGITS {
        return Err(AmountProblem::TooLarge);
    }
    let canonical = format!(
        "{}{}{}{}",
        if negative { "-" } else { "" },
        if whole_digits.is_empty() {
            "0"
        } else {
            whole_digits
        },
        if frac.is_empty() { "" } else { "." },
        frac
    );
    let d: Decimal = canonical.parse().map_err(|_| AmountProblem::Invalid)?;
    // `-0.00` reads as 0.00.
    Ok(Some(if d.is_zero() { d.abs() } else { d }))
}

/// Canonical amount text: `-1234.50` (the entered scale kept).
#[must_use]
pub fn canonical_amount(d: Decimal) -> String {
    d.to_string()
}

// ---- Dates ----------------------------------------------------------------

/// Reads a full date: `2026-03-31` or `3/31/2026` (a four-digit year; no
/// date keys and nothing that depends on today).
#[must_use]
pub fn parse_date(text: &str) -> Option<CalendarDate> {
    let t = text.trim();
    if let Ok(d) = CalendarDate::parse_wire(t) {
        return Some(d);
    }
    let parts: Vec<&str> = t.split('/').collect();
    let [m, d, y] = parts.as_slice() else {
        return None;
    };
    let digits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_digit())
    };
    if !digits(m, 2) || !digits(d, 2) || y.len() != 4 || !digits(y, 4) {
        return None;
    }
    CalendarDate::from_ymd(y.parse().ok()?, m.parse().ok()?, d.parse().ok()?).ok()
}

// ---- Documents --------------------------------------------------------------

/// The kind of document a group makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocType {
    /// A customer invoice.
    Invoice,
    /// A vendor bill.
    Bill,
    /// A journal entry.
    JournalEntry,
}

impl DocType {
    /// The engine's type name.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            DocType::Invoice => "invoice",
            DocType::Bill => "bill",
            DocType::JournalEntry => "journal_entry",
        }
    }

    /// Reads a Type cell (`Invoice`, `Bill`, `Journal Entry`, `JE`,
    /// `General Journal`).
    #[must_use]
    pub fn parse(text: &str) -> Option<DocType> {
        let t: String = text
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c == '_' || c == '-' { ' ' } else { c })
            .collect();
        match t.split_whitespace().collect::<Vec<_>>().join(" ").as_str() {
            "invoice" | "inv" => Some(DocType::Invoice),
            "bill" => Some(DocType::Bill),
            "journal entry" | "journal" | "je" | "general journal" => Some(DocType::JournalEntry),
            _ => None,
        }
    }
}

/// A line's amount, as the file gave it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LineAmount {
    /// One signed amount (an invoice or bill line; a journal line where
    /// positive is a debit).
    Amount(Decimal),
    /// A journal debit.
    Debit(Decimal),
    /// A journal credit.
    Credit(Decimal),
}

impl LineAmount {
    /// Debit minus credit (a signed journal amount is a debit).
    #[must_use]
    pub fn signed(self) -> Decimal {
        match self {
            LineAmount::Amount(d) | LineAmount::Debit(d) => d,
            LineAmount::Credit(d) => -d,
        }
    }
}

/// One line of a document.
#[derive(Clone, PartialEq, Eq)]
pub struct TxnLine {
    /// The owner's row number (1 is the header row).
    pub row: usize,
    /// Account name or number.
    pub account: Option<String>,
    /// Product or service.
    pub item: Option<String>,
    /// Line description.
    pub description: Option<String>,
    /// The amount.
    pub amount: LineAmount,
    /// Tag.
    pub tag: Option<String>,
}

/// One document the file makes.
#[derive(Clone, PartialEq, Eq)]
pub struct TxnDocument {
    /// The group value (the file's, or `G1`, `G2`… when there is no Group
    /// column).
    pub group: String,
    /// Kind.
    pub doc_type: DocType,
    /// Date.
    pub date: CalendarDate,
    /// Number.
    pub number: Option<String>,
    /// Customer or vendor.
    pub name: Option<String>,
    /// Due date.
    pub due_date: Option<CalendarDate>,
    /// Memo.
    pub memo: Option<String>,
    /// Lines, in file order.
    pub lines: Vec<TxnLine>,
}

impl TxnDocument {
    /// The document's total: the sum of an invoice's or bill's lines, or a
    /// journal entry's debits. Exact.
    #[must_use]
    pub fn total(&self) -> Decimal {
        match self.doc_type {
            DocType::Invoice | DocType::Bill => self.lines.iter().map(|l| l.amount.signed()).sum(),
            DocType::JournalEntry => self
                .lines
                .iter()
                .map(|l| l.amount.signed())
                .filter(Decimal::is_sign_positive)
                .sum(),
        }
    }

    /// Debits minus credits of a journal entry (zero when balanced).
    #[must_use]
    pub fn imbalance(&self) -> Decimal {
        self.lines.iter().map(|l| l.amount.signed()).sum()
    }

    /// The first row of the document.
    #[must_use]
    pub fn first_row(&self) -> usize {
        self.lines.first().map_or(0, |l| l.row)
    }
}

impl fmt::Debug for TxnDocument {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxnDocument")
            .field("doc_type", &self.doc_type)
            .field("first_row", &self.first_row())
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// What is wrong with a row (or the file, row 0). The screen words each
/// one for the owner; the engine's own message is never shown.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// A required cell is blank (the field says which).
    Missing,
    /// The Type isn't invoice, bill or journal entry.
    TypeUnknown,
    /// The date isn't a full date.
    DateInvalid,
    /// The amount can't be read.
    AmountInvalid,
    /// The amount has more than four decimal places.
    AmountTooPrecise,
    /// The amount is too large.
    AmountTooLarge,
    /// A debit or credit is negative.
    AmountNegative,
    /// The line has no amount.
    AmountMissing,
    /// The line has both a debit and a credit.
    DebitAndCredit,
    /// A debit or credit on an invoice or bill line (they take Amount).
    DebitCreditOnDocument,
    /// The row's document cells differ from its group's first row.
    MixedGroup,
    /// The journal entry's debits and credits differ by this much.
    Unbalanced(Decimal),
    /// An invoice or bill whose total isn't above zero.
    TotalNotPositive,
    /// The due date is before the document date.
    DueBeforeDate,
    /// The file has no rows.
    NoRows,
    /// The file has more rows than one import takes.
    TooManyRows,
}

impl ProblemKind {
    /// A stable code (logs and i18n keys; never amounts).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            ProblemKind::Missing => "missing",
            ProblemKind::TypeUnknown => "type_unknown",
            ProblemKind::DateInvalid => "date_invalid",
            ProblemKind::AmountInvalid => "amount_invalid",
            ProblemKind::AmountTooPrecise => "amount_too_precise",
            ProblemKind::AmountTooLarge => "amount_too_large",
            ProblemKind::AmountNegative => "amount_negative",
            ProblemKind::AmountMissing => "amount_missing",
            ProblemKind::DebitAndCredit => "debit_and_credit",
            ProblemKind::DebitCreditOnDocument => "debit_credit_on_document",
            ProblemKind::MixedGroup => "mixed_group",
            ProblemKind::Unbalanced(_) => "unbalanced",
            ProblemKind::TotalNotPositive => "total_not_positive",
            ProblemKind::DueBeforeDate => "due_before_date",
            ProblemKind::NoRows => "no_rows",
            ProblemKind::TooManyRows => "too_many_rows",
        }
    }
}

/// The code only: an imbalance is an amount.
impl fmt::Debug for ProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// One problem found before import.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowProblem {
    /// The owner's row number (1 is the header row; 0 the whole file).
    pub row: usize,
    /// The field, when one cell is at fault.
    pub field: Option<TxnField>,
    /// What is wrong.
    pub kind: ProblemKind,
}

/// The file read into documents, with every problem found.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TxnPlan {
    /// Documents, in order of their first row.
    pub documents: Vec<TxnDocument>,
    /// Problems, by row.
    pub problems: Vec<RowProblem>,
    /// Data rows read (blank rows skipped).
    pub rows: usize,
}

impl fmt::Debug for TxnPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TxnPlan")
            .field("documents", &self.documents.len())
            .field("problems", &self.problems)
            .field("rows", &self.rows)
            .finish()
    }
}

/// How many of each kind the file makes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocCounts {
    /// Invoices.
    pub invoices: usize,
    /// Bills.
    pub bills: usize,
    /// Journal entries.
    pub journal_entries: usize,
}

struct RowCells<'a> {
    row: usize,
    cells: &'a [String],
    mapping: &'a TxnMapping,
}

impl RowCells<'_> {
    fn get(&self, f: TxnField) -> Option<&str> {
        let c = self.cells.get(*self.mapping.get(&f)?)?.trim();
        (!c.is_empty()).then_some(c)
    }
}

fn amount_problem(p: AmountProblem) -> ProblemKind {
    match p {
        AmountProblem::Invalid => ProblemKind::AmountInvalid,
        AmountProblem::TooManyPlaces => ProblemKind::AmountTooPrecise,
        AmountProblem::TooLarge => ProblemKind::AmountTooLarge,
    }
}

/// A group's document cells, from its first row with each one filled.
#[derive(Default)]
struct Header {
    doc_type: Option<String>,
    date: Option<String>,
    number: Option<String>,
    name: Option<String>,
    due_date: Option<String>,
    memo: Option<String>,
}

impl Header {
    fn slot(&mut self, f: TxnField) -> Option<&mut Option<String>> {
        Some(match f {
            TxnField::Type => &mut self.doc_type,
            TxnField::Date => &mut self.date,
            TxnField::Number => &mut self.number,
            TxnField::Name => &mut self.name,
            TxnField::DueDate => &mut self.due_date,
            TxnField::Memo => &mut self.memo,
            _ => return None,
        })
    }
}

struct Group {
    key: String,
    header: Header,
    first_row: usize,
    rows: Vec<usize>,
}

/// Groups the rows and checks them (plan §2b D5). `rows` are the file's
/// data rows, `first_row` the owner's number of the first one (2 under a
/// header row). Blank rows are skipped.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build(rows: &[Vec<String>], first_row: usize, mapping: &TxnMapping) -> TxnPlan {
    let mut problems = Vec::new();
    let filled: Vec<RowCells<'_>> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.iter().any(|c| !c.trim().is_empty()))
        .map(|(i, r)| RowCells {
            row: first_row + i,
            cells: r,
            mapping,
        })
        .collect();
    if filled.is_empty() {
        problems.push(RowProblem {
            row: 0,
            field: None,
            kind: ProblemKind::NoRows,
        });
        return TxnPlan {
            problems,
            ..TxnPlan::default()
        };
    }
    if filled.len() > MAX_ROWS {
        problems.push(RowProblem {
            row: 0,
            field: None,
            kind: ProblemKind::TooManyRows,
        });
        return TxnPlan {
            problems,
            rows: filled.len(),
            ..TxnPlan::default()
        };
    }
    let by_group = mapping.contains_key(&TxnField::GroupId);
    // Group the rows (first appearance order).
    let mut groups: Vec<Group> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut row_group: Vec<Option<usize>> = Vec::with_capacity(filled.len());
    for r in &filled {
        let key = if by_group {
            let Some(g) = r.get(TxnField::GroupId) else {
                problems.push(RowProblem {
                    row: r.row,
                    field: Some(TxnField::GroupId),
                    kind: ProblemKind::Missing,
                });
                row_group.push(None);
                continue;
            };
            g.to_string()
        } else {
            [
                TxnField::Type,
                TxnField::Date,
                TxnField::Number,
                TxnField::Name,
            ]
            .iter()
            .map(|f| r.get(*f).unwrap_or("").to_lowercase())
            .collect::<Vec<_>>()
            .join("\u{1f}")
        };
        let gi = *index.entry(key.clone()).or_insert_with(|| {
            groups.push(Group {
                key: key.clone(),
                header: Header::default(),
                first_row: r.row,
                rows: Vec::new(),
            });
            groups.len() - 1
        });
        let g = &mut groups[gi];
        g.rows.push(r.row);
        // Document cells: the first filled value wins; a later different
        // one mixes two documents in one group.
        for f in TxnField::ALL.into_iter().filter(|f| f.is_header()) {
            let Some(v) = r.get(f) else { continue };
            let Some(slot) = g.header.slot(f) else {
                continue;
            };
            match slot {
                None => *slot = Some(v.to_string()),
                Some(prev) if !same_cell(f, prev, v) => problems.push(RowProblem {
                    row: r.row,
                    field: Some(f),
                    kind: ProblemKind::MixedGroup,
                }),
                Some(_) => {}
            }
        }
        row_group.push(Some(gi));
    }
    // Read each group's document.
    let mut documents: Vec<Option<TxnDocument>> = Vec::with_capacity(groups.len());
    for (n, g) in groups.iter().enumerate() {
        let mut bad = false;
        let mut need = |f: TxnField, v: &Option<String>, problems: &mut Vec<RowProblem>| {
            if v.is_none() {
                problems.push(RowProblem {
                    row: g.first_row,
                    field: Some(f),
                    kind: ProblemKind::Missing,
                });
                bad = true;
            }
        };
        need(TxnField::Type, &g.header.doc_type, &mut problems);
        need(TxnField::Date, &g.header.date, &mut problems);
        let doc_type = g.header.doc_type.as_deref().and_then(|t| {
            let d = DocType::parse(t);
            if d.is_none() {
                problems.push(RowProblem {
                    row: g.first_row,
                    field: Some(TxnField::Type),
                    kind: ProblemKind::TypeUnknown,
                });
            }
            d
        });
        let date = g.header.date.as_deref().and_then(|t| {
            let d = parse_date(t);
            if d.is_none() {
                problems.push(RowProblem {
                    row: g.first_row,
                    field: Some(TxnField::Date),
                    kind: ProblemKind::DateInvalid,
                });
            }
            d
        });
        let due_date = g.header.due_date.as_deref().and_then(|t| {
            let d = parse_date(t);
            if d.is_none() {
                problems.push(RowProblem {
                    row: g.first_row,
                    field: Some(TxnField::DueDate),
                    kind: ProblemKind::DateInvalid,
                });
            }
            d
        });
        if matches!(doc_type, Some(DocType::Invoice | DocType::Bill)) {
            need(TxnField::Name, &g.header.name, &mut problems);
        }
        let (Some(doc_type), Some(date)) = (doc_type, date) else {
            documents.push(None);
            continue;
        };
        if let Some(due) = due_date
            && due < date
        {
            problems.push(RowProblem {
                row: g.first_row,
                field: Some(TxnField::DueDate),
                kind: ProblemKind::DueBeforeDate,
            });
        }
        documents.push((!bad).then(|| TxnDocument {
            group: if by_group {
                g.key.clone()
            } else {
                format!("G{}", n + 1)
            },
            doc_type,
            date,
            number: g.header.number.clone(),
            name: g.header.name.clone(),
            due_date: due_date.filter(|_| doc_type != DocType::JournalEntry),
            memo: g.header.memo.clone(),
            lines: Vec::new(),
        }));
    }
    // Lines.
    let mut line_bad = vec![false; groups.len()];
    for (r, gi) in filled.iter().zip(&row_group) {
        let Some(gi) = *gi else { continue };
        let Some(doc) = documents[gi].as_mut() else {
            continue;
        };
        match read_line(r, doc.doc_type) {
            Ok(line) => doc.lines.push(line),
            Err(mut p) => {
                line_bad[gi] = true;
                problems.append(&mut p);
            }
        }
    }
    // Whole-document checks.
    for (gi, doc) in documents.iter().enumerate() {
        let Some(doc) = doc else { continue };
        if line_bad[gi] {
            continue;
        }
        match doc.doc_type {
            DocType::JournalEntry => {
                let diff = doc.imbalance();
                if !diff.is_zero() {
                    problems.push(RowProblem {
                        row: doc.first_row(),
                        field: None,
                        kind: ProblemKind::Unbalanced(diff),
                    });
                }
            }
            DocType::Invoice | DocType::Bill => {
                if !doc.total().is_sign_positive() || doc.total().is_zero() {
                    problems.push(RowProblem {
                        row: doc.first_row(),
                        field: Some(TxnField::LineAmount),
                        kind: ProblemKind::TotalNotPositive,
                    });
                }
            }
        }
    }
    problems.sort_by_key(|p| (p.row, p.field));
    problems.dedup();
    TxnPlan {
        documents: documents.into_iter().flatten().collect(),
        problems,
        rows: filled.len(),
    }
}

/// Two cells of one document field name the same thing.
fn same_cell(field: TxnField, first: &str, other: &str) -> bool {
    match field {
        TxnField::Date | TxnField::DueDate => match (parse_date(first), parse_date(other)) {
            (Some(x), Some(y)) => x == y,
            _ => first == other,
        },
        TxnField::Type => match (DocType::parse(first), DocType::parse(other)) {
            (Some(x), Some(y)) => x == y,
            _ => first.eq_ignore_ascii_case(other),
        },
        _ => first == other,
    }
}

fn read_line(r: &RowCells<'_>, doc_type: DocType) -> Result<TxnLine, Vec<RowProblem>> {
    let mut problems = Vec::new();
    let mut push = |field, kind| {
        problems.push(RowProblem {
            row: r.row,
            field: Some(field),
            kind,
        });
    };
    let account = r.get(TxnField::LineAccount).map(str::to_string);
    let item = r.get(TxnField::LineItem).map(str::to_string);
    match doc_type {
        DocType::Invoice if account.is_none() && item.is_none() => {
            push(TxnField::LineItem, ProblemKind::Missing);
        }
        DocType::Bill if account.is_none() && item.is_none() => {
            push(TxnField::LineAccount, ProblemKind::Missing);
        }
        DocType::JournalEntry if account.is_none() => {
            push(TxnField::LineAccount, ProblemKind::Missing);
        }
        _ => {}
    }
    let mut read = |f: TxnField, signed: bool| match r.get(f) {
        None => None,
        Some(t) => match parse_amount(t, signed) {
            Ok(v) => v,
            Err(p) => {
                push(f, amount_problem(p));
                None
            }
        },
    };
    let amount = read(TxnField::LineAmount, true);
    let debit = read(TxnField::LineDebit, false);
    let credit = read(TxnField::LineCredit, false);
    // A cell that didn't read is already a problem; "missing" would repeat it.
    let unreadable = [
        TxnField::LineAmount,
        TxnField::LineDebit,
        TxnField::LineCredit,
    ]
    .iter()
    .any(|f| r.get(*f).is_some())
        && amount.is_none()
        && debit.is_none()
        && credit.is_none();
    for (f, v) in [(TxnField::LineDebit, debit), (TxnField::LineCredit, credit)] {
        if v.is_some_and(|d| d.is_sign_negative() && !d.is_zero()) {
            push(f, ProblemKind::AmountNegative);
        }
    }
    let line_amount = match (doc_type, amount, debit, credit) {
        (_, Some(a), None, None) => Some(LineAmount::Amount(a)),
        (DocType::Invoice | DocType::Bill, _, Some(_), _)
        | (DocType::Invoice | DocType::Bill, _, _, Some(_)) => {
            push(
                if debit.is_some() {
                    TxnField::LineDebit
                } else {
                    TxnField::LineCredit
                },
                ProblemKind::DebitCreditOnDocument,
            );
            None
        }
        (DocType::JournalEntry, None, Some(d), None) => Some(LineAmount::Debit(d)),
        (DocType::JournalEntry, None, None, Some(c)) => Some(LineAmount::Credit(c)),
        // A blank partner of the other column (`0` or `0.00`) is no amount.
        (DocType::JournalEntry, None, Some(d), Some(c)) if c.is_zero() => {
            Some(LineAmount::Debit(d))
        }
        (DocType::JournalEntry, None, Some(d), Some(c)) if d.is_zero() => {
            Some(LineAmount::Credit(c))
        }
        (DocType::JournalEntry, _, Some(_), Some(_))
        | (DocType::JournalEntry, Some(_), Some(_), _)
        | (DocType::JournalEntry, Some(_), _, Some(_)) => {
            push(TxnField::LineDebit, ProblemKind::DebitAndCredit);
            None
        }
        (_, None, None, None) => {
            if !unreadable {
                push(TxnField::LineAmount, ProblemKind::AmountMissing);
            }
            None
        }
    };
    match line_amount {
        Some(amount) if problems.is_empty() => Ok(TxnLine {
            row: r.row,
            account,
            item,
            description: r.get(TxnField::LineDescription).map(str::to_string),
            amount,
            tag: r.get(TxnField::LineTag).map(str::to_string),
        }),
        _ => Err(problems),
    }
}

impl TxnPlan {
    /// Nothing stands in the way of importing.
    #[must_use]
    pub fn can_import(&self) -> bool {
        self.problems.is_empty() && !self.documents.is_empty()
    }

    /// How many of each kind.
    #[must_use]
    pub fn counts(&self) -> DocCounts {
        let mut c = DocCounts::default();
        for d in &self.documents {
            match d.doc_type {
                DocType::Invoice => c.invoices += 1,
                DocType::Bill => c.bills += 1,
                DocType::JournalEntry => c.journal_entries += 1,
            }
        }
        c
    }

    /// The rows of the canonical file, header first (E4 column order):
    /// ISO dates, canonical amounts; invoice and bill lines in
    /// `line_amount`, journal lines in `line_debit` or `line_credit` (a
    /// signed journal amount goes to the side its sign says). Every line
    /// has one amount cell filled.
    #[must_use]
    pub fn canonical_rows(&self) -> Vec<Vec<String>> {
        let mut out = vec![
            TxnField::ALL
                .iter()
                .map(|f| f.wire().to_string())
                .collect::<Vec<_>>(),
        ];
        for d in &self.documents {
            for l in &d.lines {
                let (amount, debit, credit) = match (d.doc_type, l.amount) {
                    (DocType::Invoice | DocType::Bill, a) => {
                        (canonical_amount(a.signed()), String::new(), String::new())
                    }
                    (DocType::JournalEntry, a) => {
                        let s = a.signed();
                        if s.is_sign_negative() && !s.is_zero() {
                            (String::new(), String::new(), canonical_amount(-s))
                        } else {
                            (String::new(), canonical_amount(s), String::new())
                        }
                    }
                };
                let o = |v: &Option<String>| v.clone().unwrap_or_default();
                out.push(vec![
                    d.group.clone(),
                    d.doc_type.wire().to_string(),
                    d.date.to_wire(),
                    o(&d.number),
                    o(&d.name),
                    d.due_date.map(CalendarDate::to_wire).unwrap_or_default(),
                    o(&d.memo),
                    o(&l.account),
                    o(&l.item),
                    o(&l.description),
                    amount,
                    debit,
                    credit,
                    o(&l.tag),
                ]);
            }
        }
        out
    }

    /// The canonical CSV text (CRLF line ends, RFC 4180 quoting).
    #[must_use]
    pub fn canonical_csv(&self) -> String {
        let mut s = String::new();
        for row in self.canonical_rows() {
            let cells: Vec<String> = row.iter().map(|c| csv_cell(c)).collect();
            s.push_str(&cells.join(","));
            s.push_str("\r\n");
        }
        s
    }

    /// The owner's row for the engine's row number in the canonical file
    /// (the engine counts the header as row 1; 0 is the whole file).
    #[must_use]
    pub fn source_row(&self, engine_row: usize) -> Option<usize> {
        let k = engine_row.checked_sub(2)?;
        self.documents
            .iter()
            .flat_map(|d| d.lines.iter().map(|l| l.row))
            .nth(k)
    }
}

/// One CSV cell, quoted when it holds a comma, quote or line break.
#[must_use]
pub fn csv_cell(c: &str) -> String {
    if c.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", c.replace('"', "\"\""))
    } else {
        c.to_string()
    }
}

#[cfg(test)]
#[path = "csv_txn_tests.rs"]
mod tests;
