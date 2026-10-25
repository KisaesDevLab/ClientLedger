//! Matching a vendor statement against the books (P17, plan §3 WP3, D4,
//! Q-P17q).
//!
//! The owner types or pastes the statement's lines ([`parse_paste`]); the
//! screen (`cl-ui` `screens/bill/statement_match.rs`, WP6) reads the books
//! side from the engine (posted bills, vendor credits and bill payments) as
//! [`BooksItem`]s, and [`match_lines`] pairs them:
//!
//! 1. **By reference:** equal ref ([`crate::forms::bill::ref_matches`]),
//!    compatible kind and the **exact** amount. Statement lines are taken in
//!    a canonical order (by value); a line with exactly one unmatched
//!    candidate takes it. A line with several (the same ref and amount
//!    entered twice in the books) is listed as [`Ambiguous`] with them,
//!    never picked by date.
//! 2. **By amount and date:** what is left pairs on the exact amount and a
//!    compatible kind with the dates at most `date_slack_days` apart, but
//!    only when the candidate is unique **on both sides**. Anything else
//!    with a candidate is listed as [`Ambiguous`] with its candidates;
//!    nothing is guessed.
//!
//! The result is deterministic, never pairs two lines with one item (or one
//! line with two items) and does not depend on the order of the inputs.
//! Amounts are compared as exact [`Money`]; there is no tolerance and no
//! running balance (that waits for U-14).

use std::cmp::Ordering;
use std::fmt;

use cl_core::ids::TransactionId;
use cl_core::money::UserInput;
use cl_core::{CalendarDate, Money};

use crate::date_entry::{self, DateOrder};
use crate::forms::bill::ref_matches;
use crate::text::TextKey;

/// The date slack of the second pass (Q-P17q).
pub const DEFAULT_DATE_SLACK_DAYS: i64 = 7;

/// What a line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum StatementKind {
    /// A bill or invoice: the vendor charged.
    Charge,
    /// A vendor credit (credit memo).
    Credit,
    /// A payment to the vendor.
    Payment,
    /// A negative statement line pasted without a kind: a credit or a
    /// payment (it matches either).
    CreditOrPayment,
}

impl StatementKind {
    /// True when a line of kind `self` can pair with `other`.
    #[must_use]
    pub fn compatible(self, other: StatementKind) -> bool {
        use StatementKind::{Credit, CreditOrPayment, Payment};
        self == other
            || matches!(
                (self, other),
                (CreditOrPayment, Credit | Payment) | (Credit | Payment, CreditOrPayment)
            )
    }

    /// The kind named in a pasted cell (`invoice`, `credit memo`,
    /// `payment`...), ignoring case and surrounding space.
    #[must_use]
    pub fn parse(word: &str) -> Option<StatementKind> {
        match word.trim().to_lowercase().as_str() {
            "charge" | "bill" | "invoice" | "inv" => Some(StatementKind::Charge),
            "credit" | "credit memo" | "cm" | "vendor credit" => Some(StatementKind::Credit),
            "payment" | "pmt" | "paid" => Some(StatementKind::Payment),
            _ => None,
        }
    }
}

/// One line of the vendor's statement. `amount` is the size of the line
/// (more than zero); the kind says which way it goes. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatementLine {
    /// Date on the statement.
    pub date: CalendarDate,
    /// The vendor's reference (invoice, credit memo or payment number).
    pub reference: String,
    /// Charge, credit or payment.
    pub kind: StatementKind,
    /// The amount, more than zero.
    pub amount: Money,
}

impl fmt::Debug for StatementLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementLine")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// One posted bill, vendor credit or bill payment from the books (the
/// engine's figures as sent; ids are unique). `Debug` is redacted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct BooksItem {
    /// Transaction id.
    pub id: TransactionId,
    /// [`StatementKind::Charge`] for a bill, `Credit` for a vendor credit,
    /// `Payment` for a bill payment.
    pub kind: StatementKind,
    /// Date.
    pub date: CalendarDate,
    /// Ref. no. (vendor invoice number, credit ref or payment ref).
    pub reference: String,
    /// The total, more than zero.
    pub amount: Money,
}

impl fmt::Debug for BooksItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BooksItem")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Why a pasted row was not read. No variant carries the row's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PasteProblem {
    /// Not 3 or 4 columns (date, ref, amount, optional kind).
    Columns,
    /// The date is not a date.
    Date,
    /// The amount is not an amount (at most two decimals).
    Amount,
    /// The amount is zero.
    ZeroAmount,
    /// The kind is not charge, credit or payment.
    Kind,
}

impl PasteProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            PasteProblem::Columns => TextKey("statement.paste.columns"),
            PasteProblem::Date => TextKey("statement.paste.date"),
            PasteProblem::Amount => TextKey("statement.paste.amount"),
            PasteProblem::ZeroAmount => TextKey("statement.paste.zero"),
            PasteProblem::Kind => TextKey("statement.paste.kind"),
        }
    }
}

/// A pasted row that was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadRow {
    /// Line number in the pasted text (from 1).
    pub line: usize,
    /// Why.
    pub problem: PasteProblem,
}

/// A pasted row that was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastedRow {
    /// Line number in the pasted text (from 1).
    pub line: usize,
    /// The statement line.
    pub entry: StatementLine,
}

/// The result of [`parse_paste`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedPaste {
    /// Rows read, in paste order.
    pub rows: Vec<PastedRow>,
    /// Rows not read, in paste order.
    pub bad: Vec<BadRow>,
    /// The first line was a column heading and was skipped.
    pub header_skipped: bool,
}

impl ParsedPaste {
    /// The statement lines, in paste order.
    #[must_use]
    pub fn lines(&self) -> Vec<StatementLine> {
        self.rows.iter().map(|r| r.entry.clone()).collect()
    }
}

/// Splits a comma-separated line, honouring double quotes (`"1,330.00"`;
/// `""` is a quote inside quotes).
fn split_csv(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells
}

fn cells(line: &str) -> Vec<String> {
    if line.contains('\t') {
        line.split('\t').map(str::to_string).collect()
    } else {
        split_csv(line)
    }
}

fn parse_row(
    cells: &[String],
    order: DateOrder,
    today: CalendarDate,
) -> Result<StatementLine, PasteProblem> {
    let (date, reference, amount, kind) = match cells {
        [d, r, a] => (d, r, a, None),
        [d, r, a, k] => (d, r, a, Some(k)),
        _ => return Err(PasteProblem::Columns),
    };
    let date = date_entry::parse(date, order, today).map_err(|_| PasteProblem::Date)?;
    let amount = Money::parse_user(amount, UserInput::CENTS).map_err(|_| PasteProblem::Amount)?;
    if amount.is_zero() {
        return Err(PasteProblem::ZeroAmount);
    }
    let kind = match kind.map(|k| k.trim()).filter(|k| !k.is_empty()) {
        Some(k) => StatementKind::parse(k).ok_or(PasteProblem::Kind)?,
        None if amount.is_positive() => StatementKind::Charge,
        None => StatementKind::CreditOrPayment,
    };
    Ok(StatementLine {
        date,
        reference: reference.trim().to_string(),
        kind,
        amount: amount.abs(),
    })
}

/// Reads pasted statement lines: tab-separated (a spreadsheet copy) or
/// comma-separated with optional double quotes. Each row is date, ref,
/// amount and an optional kind (`invoice`/`bill`/`charge`,
/// `credit`/`credit memo`, `payment`). Without a kind the sign decides: a
/// positive amount is a charge, a negative one (`-75.00` or `(75.00)`) a
/// credit or payment. Amounts keep their size; the kind carries the
/// direction. Blank lines are skipped; a first line that has neither a date
/// nor an amount is taken as the column headings. Bad rows are reported by
/// line number and never guessed.
#[must_use]
pub fn parse_paste(text: &str, order: DateOrder, today: CalendarDate) -> ParsedPaste {
    let mut out = ParsedPaste::default();
    let mut first = true;
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let cs = cells(line);
        let is_first = std::mem::replace(&mut first, false);
        match parse_row(&cs, order, today) {
            Ok(entry) => out.rows.push(PastedRow {
                line: line_no,
                entry,
            }),
            Err(problem) => {
                let heading = is_first
                    && cs.len() >= 3
                    && date_entry::parse(&cs[0], order, today).is_err()
                    && Money::parse_user(&cs[2], UserInput::CENTS).is_err();
                if heading {
                    out.header_skipped = true;
                } else {
                    out.bad.push(BadRow {
                        line: line_no,
                        problem,
                    });
                }
            }
        }
    }
    out
}

/// Which pass paired a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MatchPass {
    /// Equal ref, kind and exact amount.
    Reference,
    /// Exact amount and kind, dates within the slack, unique on both sides.
    AmountAndDate,
}

/// A statement line paired with a books item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchPair {
    /// Index into the statement lines passed to [`match_lines`].
    pub statement: usize,
    /// The books item.
    pub books: TransactionId,
    /// Which pass paired them.
    pub pass: MatchPass,
}

/// A statement line with more than one possible books item (or whose item
/// is also another line's candidate): listed, never auto-picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ambiguous {
    /// Index into the statement lines.
    pub statement: usize,
    /// The possible books items, by date then id.
    pub candidates: Vec<TransactionId>,
}

/// The outcome of [`match_lines`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MatchResult {
    /// Paired lines, by statement index.
    pub pairs: Vec<MatchPair>,
    /// Statement lines with no books item at all ("On the statement, not in
    /// your books"), by index.
    pub missing_in_books: Vec<usize>,
    /// Books items no statement line can pair with ("In your books, not on
    /// the statement"), by date then id.
    pub missing_on_statement: Vec<TransactionId>,
    /// Lines left for the owner to decide, by statement index.
    pub ambiguous: Vec<Ambiguous>,
}

fn same_amount_and_kind(s: &StatementLine, b: &BooksItem) -> bool {
    s.amount == b.amount && s.kind.compatible(b.kind)
}

fn books_order(a: &BooksItem, b: &BooksItem) -> Ordering {
    (a.date, a.id).cmp(&(b.date, b.id))
}

/// Pass 1: equal ref + kind + exact amount, statement lines in canonical
/// order. One unmatched candidate pairs; several (the same ref and amount
/// twice in the books) are returned as a tie for the owner, never picked by
/// date.
fn pass_one(
    stmt: &[StatementLine],
    books: &[BooksItem],
    s_order: &[usize],
    b_order: &[usize],
    s_taken: &mut [bool],
    b_taken: &mut [bool],
    pairs: &mut Vec<MatchPair>,
) -> Vec<(usize, Vec<usize>)> {
    let mut ties = Vec::new();
    for &si in s_order {
        let s = &stmt[si];
        if s.reference.trim().is_empty() {
            continue;
        }
        let cands: Vec<usize> = b_order
            .iter()
            .copied()
            .filter(|&bi| {
                let b = &books[bi];
                !b_taken[bi]
                    && same_amount_and_kind(s, b)
                    && ref_matches(&s.reference, &b.reference)
            })
            .collect();
        match cands.as_slice() {
            [] => {}
            [bi] => {
                s_taken[si] = true;
                b_taken[*bi] = true;
                pairs.push(MatchPair {
                    statement: si,
                    books: books[*bi].id,
                    pass: MatchPass::Reference,
                });
            }
            _ => {
                s_taken[si] = true;
                ties.push((si, cands));
            }
        }
    }
    ties
}

/// The reference ties of pass 1 (statement index, candidate book indexes):
/// their candidates less any item paired since (by a later reference line
/// or by pass 2) go to `ambiguous`; a tie whose items were all paired
/// elsewhere has nothing left in the books. Returns which books items are
/// still a tie's candidate.
fn add_reference_ties(
    reference_ties: Vec<(usize, Vec<usize>)>,
    books: &[BooksItem],
    pairs: &[MatchPair],
    ambiguous: &mut Vec<Ambiguous>,
    missing_in_books: &mut Vec<usize>,
) -> Vec<bool> {
    let paired: std::collections::BTreeSet<TransactionId> = pairs.iter().map(|p| p.books).collect();
    let mut tied = vec![false; books.len()];
    for (si, cands) in reference_ties {
        let left: Vec<usize> = cands
            .into_iter()
            .filter(|&bi| !paired.contains(&books[bi].id))
            .collect();
        if left.is_empty() {
            missing_in_books.push(si);
            continue;
        }
        for &bi in &left {
            tied[bi] = true;
        }
        ambiguous.push(Ambiguous {
            statement: si,
            candidates: left.iter().map(|&bi| books[bi].id).collect(),
        });
    }
    tied
}

/// Pairs statement lines with books items (see the module docs).
/// `date_slack_days` below zero is treated as zero.
#[must_use]
pub fn match_lines(
    stmt: &[StatementLine],
    books: &[BooksItem],
    date_slack_days: i64,
) -> MatchResult {
    let slack = date_slack_days.max(0);
    // Canonical orders: by value, so the result does not depend on the
    // input order (equal statement lines are interchangeable).
    let mut s_order: Vec<usize> = (0..stmt.len()).collect();
    s_order.sort_by(|&a, &b| stmt[a].cmp(&stmt[b]).then(a.cmp(&b)));
    let mut b_order: Vec<usize> = (0..books.len()).collect();
    b_order.sort_by(|&a, &b| books_order(&books[a], &books[b]));

    let mut s_taken = vec![false; stmt.len()];
    let mut b_taken = vec![false; books.len()];
    let mut pairs = Vec::new();

    let ref_ambiguous = pass_one(
        stmt,
        books,
        &s_order,
        &b_order,
        &mut s_taken,
        &mut b_taken,
        &mut pairs,
    );

    // Pass 2: exact amount + kind within the slack, unique on both sides.
    let near = |si: usize, bi: usize| {
        let (s, b) = (&stmt[si], &books[bi]);
        same_amount_and_kind(s, b) && s.date.days_until(b.date).abs() <= slack
    };
    let s_cands: Vec<Vec<usize>> = (0..stmt.len())
        .map(|si| {
            if s_taken[si] {
                return Vec::new();
            }
            b_order
                .iter()
                .copied()
                .filter(|&bi| !b_taken[bi] && near(si, bi))
                .collect()
        })
        .collect();
    let b_cands: Vec<usize> = (0..books.len())
        .map(|bi| {
            if b_taken[bi] {
                return 0;
            }
            (0..stmt.len())
                .filter(|&si| !s_taken[si] && near(si, bi))
                .count()
        })
        .collect();
    let mut ambiguous = Vec::new();
    let mut missing_in_books = Vec::new();
    for si in 0..stmt.len() {
        if s_taken[si] {
            continue;
        }
        match s_cands[si].as_slice() {
            [] => missing_in_books.push(si),
            [bi] if b_cands[*bi] == 1 => {
                pairs.push(MatchPair {
                    statement: si,
                    books: books[*bi].id,
                    pass: MatchPass::AmountAndDate,
                });
            }
            cands => ambiguous.push(Ambiguous {
                statement: si,
                candidates: cands.iter().map(|&bi| books[bi].id).collect(),
            }),
        }
    }
    let tied = add_reference_ties(
        ref_ambiguous,
        books,
        &pairs,
        &mut ambiguous,
        &mut missing_in_books,
    );
    // Pass-2 pairs are disjoint (each item had exactly one candidate line),
    // and every unpaired item with a candidate line is in that line's
    // ambiguous list, so only items without any candidate are missing.
    let missing_on_statement = b_order
        .iter()
        .copied()
        .filter(|&bi| !b_taken[bi] && b_cands[bi] == 0 && !tied[bi])
        .map(|bi| books[bi].id)
        .collect();
    pairs.sort_by_key(|p| p.statement);
    ambiguous.sort_by_key(|a| a.statement);
    missing_in_books.sort_unstable();
    MatchResult {
        pairs,
        missing_in_books,
        missing_on_statement,
        ambiguous,
    }
}

#[cfg(test)]
mod tests;
