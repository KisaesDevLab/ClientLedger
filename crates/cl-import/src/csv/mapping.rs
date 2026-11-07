//! Bank CSV files: the table, the column mapping, and applying it (P25
//! step 4, Q-195, Q-P25w, review S9).
//!
//! Amount layouts:
//! - one signed amount column;
//! - debit and credit columns: the absolute value is used and the column
//!   decides the direction (a debit column holding `-45.00` is money out);
//! - an amount column plus a DR/CR indicator column.
//!
//! The mapping is serializable: the engine stores it with the batch
//! (`csvMapping`) and `cl-settings` keeps local profiles as opaque JSON.

use super::dates::{DateColumn, DateOrder, judge_column, parse_date};
use super::{Row, parse_with};
use crate::ParseOptions;
use crate::detect::FileFormat;
use crate::error::ImportError;
use crate::normalize::{
    AccountKind, AmountProblem, Declared, ParsedAmount, ParsedFile, ProblemKind, RowParts, Section,
    TextEncoding, decode, parse_amount, parse_amount_detail,
};
use cl_core::CalendarDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The current mapping format version.
pub const MAPPING_VERSION: u32 = 1;

/// A bank CSV file, read into rows but not yet mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvTable {
    /// `,`, `;` or tab.
    pub delimiter: char,
    /// How the bytes were decoded.
    pub encoding: TextEncoding,
    /// The header row, when the file has one.
    pub header: Option<Row>,
    /// The data rows (after any header and preamble lines).
    pub rows: Vec<Row>,
    /// Lines left out before the header (or the first data row):
    /// "Account: …", "Statement period …" (review DS4). Listed as skipped.
    pub preamble: Vec<usize>,
}

impl CsvTable {
    /// The most columns any row has.
    #[must_use]
    pub fn width(&self) -> usize {
        self.header
            .iter()
            .chain(&self.rows)
            .map(|r| r.fields.len())
            .max()
            .unwrap_or(0)
    }

    /// Column names for the mapping step: the header's, or "Column N".
    #[must_use]
    pub fn column_names(&self) -> Vec<String> {
        let width = self.width();
        (0..width)
            .map(|i| {
                self.header
                    .as_ref()
                    .and_then(|h| h.fields.get(i))
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("Column {}", i + 1))
            })
            .collect()
    }
}

/// Which columns hold what (0-based column indexes).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsvMapping {
    /// [`MAPPING_VERSION`].
    pub version: u32,
    /// Whether the first table row is a header.
    pub has_header: bool,
    /// The date column.
    pub date: usize,
    /// How its dates are written.
    pub date_order: DateOrder,
    /// Where the amount comes from.
    pub amount: AmountColumns,
    /// Description columns, joined with a space (often one).
    pub description: Vec<usize>,
    /// A memo column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo: Option<usize>,
    /// A check number column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_number: Option<usize>,
    /// A running balance column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<usize>,
    /// The header names the mapping was made for (trimmed), to tell
    /// whether it fits another file. Empty for files without a header.
    #[serde(default)]
    pub headers: Vec<String>,
}

/// Where a CSV row's amount comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "layout", rename_all = "camelCase")]
pub enum AmountColumns {
    /// One signed amount column.
    Signed {
        /// The column.
        column: usize,
    },
    /// Separate money-out and money-in columns; each holds a size, and the
    /// column decides the direction.
    DebitCredit {
        /// Money out.
        debit: usize,
        /// Money in.
        credit: usize,
    },
    /// An amount column plus a column saying DR/CR (Debit/Credit,
    /// Withdrawal/Deposit, D/C, W).
    Indicator {
        /// The amount (its size is used).
        amount: usize,
        /// The direction.
        indicator: usize,
    },
}

impl AmountColumns {
    fn columns(self) -> Vec<usize> {
        match self {
            Self::Signed { column } => vec![column],
            Self::DebitCredit { debit, credit } => vec![debit, credit],
            Self::Indicator { amount, indicator } => vec![amount, indicator],
        }
    }
}

impl CsvMapping {
    /// Whether the mapping can be applied to `table`: every column exists
    /// and, when both have headers, the names match (case-insensitive).
    #[must_use]
    pub fn fits(&self, table: &CsvTable) -> bool {
        let width = table.width();
        if self.columns().iter().any(|c| *c >= width) {
            return false;
        }
        if self.has_header != table.header.is_some() {
            return false;
        }
        match (&table.header, self.headers.is_empty()) {
            (Some(h), false) => {
                h.fields.len() == self.headers.len()
                    && h.fields
                        .iter()
                        .zip(&self.headers)
                        .all(|(a, b)| a.trim().eq_ignore_ascii_case(b.trim()))
            }
            _ => true,
        }
    }

    fn columns(&self) -> Vec<usize> {
        let mut c = vec![self.date];
        c.extend(self.amount.columns());
        c.extend(&self.description);
        c.extend(self.memo);
        c.extend(self.check_number);
        c.extend(self.balance);
        c
    }
}

/// Reads a CSV file into a table: decoding (BOM, UTF-8, else
/// Windows-1252), the delimiter, preamble lines, and the header.
///
/// # Errors
/// [`ImportError::Csv`] when the reader refuses the text, or
/// [`ImportError::BadText`] for broken UTF-16.
pub fn read_table(bytes: &[u8]) -> Result<CsvTable, ImportError> {
    if bytes.len() > crate::MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit_bytes: crate::MAX_FILE_BYTES,
        });
    }
    let decoded = decode(bytes, Declared::Nothing)?;
    let delimiter = sniff_bank_separator(&decoded.text);
    let rows = parse_with(&decoded.text, delimiter)?;
    let start = table_start(&rows);
    let preamble = rows[..start].iter().map(|r| r.line).collect();
    let mut rows: Vec<Row> = rows.into_iter().skip(start).collect();
    let header = if rows.first().is_some_and(looks_like_header) {
        Some(rows.remove(0))
    } else {
        None
    };
    Ok(CsvTable {
        delimiter,
        encoding: decoded.encoding,
        header,
        rows,
        preamble,
    })
}

/// Where the table starts. Preamble lines ("Account: …", "Statement
/// period …") are shorter than the table, but so is a data row whose empty
/// trailing memo the exporter dropped (review DS4). The table starts at
/// the first row as wide as most rows, or earlier at the first row that
/// reads like the data (a date and an amount in the same columns as the
/// first full data row), with the header just before it.
fn table_start(rows: &[Row]) -> usize {
    let width = modal_width(rows);
    let full = rows
        .iter()
        .position(|r| r.fields.len() == width)
        .unwrap_or(0);
    let Some(shape) = rows[full..]
        .iter()
        .find(|r| !looks_like_header(r))
        .and_then(data_shape)
    else {
        return full;
    };
    match rows[..full]
        .iter()
        .position(|r| data_shape(r) == Some(shape))
    {
        Some(0) => 0,
        // A header is at least as wide as the data under it; a shorter
        // line before the data ("Account: 1234") is preamble.
        Some(i)
            if looks_like_header(&rows[i - 1])
                && rows[i - 1].fields.len() >= rows[i].fields.len() =>
        {
            i - 1
        }
        Some(i) => i,
        None => full,
    }
}

/// The first column holding a date and the first other column holding an
/// amount, when a row has both.
fn data_shape(row: &Row) -> Option<(usize, usize)> {
    let probe = probe_date();
    let filled = |f: &String| !f.trim().is_empty();
    let date = row
        .fields
        .iter()
        .position(|f| filled(f) && parse_date(f, DateOrder::Mdy, probe).is_ok())?;
    let amount = row
        .fields
        .iter()
        .enumerate()
        .position(|(i, f)| i != date && filled(f) && parse_amount(f).is_ok())?;
    Some((date, amount))
}

/// The separator of a bank CSV: sniffed on whichever of the first 20
/// non-blank lines holds the most separators, so preamble lines
/// ("Account: …", "Statement period") do not decide it.
#[must_use]
pub fn sniff_bank_separator(text: &str) -> char {
    let count = |l: &str| l.matches([',', ';', '\t']).count();
    let line = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .fold("", |best, l| if count(l) > count(best) { l } else { best });
    super::sniff_separator(line)
}

fn modal_width(rows: &[Row]) -> usize {
    let mut counts: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    for r in rows {
        *counts.entry(r.fields.len()).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(w, n)| (*n, *w))
        .map_or(0, |(w, _)| w)
}

/// A header has a letter in some cell and no cell that reads as a date or
/// an amount.
fn looks_like_header(row: &Row) -> bool {
    let probe = probe_date();
    row.fields
        .iter()
        .any(|f| f.chars().any(char::is_alphabetic))
        && !row.fields.iter().any(|f| {
            !f.trim().is_empty()
                && (parse_date(f, DateOrder::Mdy, probe).is_ok() || parse_amount(f).is_ok())
        })
}

/// A fixed "today" for header and column sniffing (only two-digit years
/// depend on it, and sniffing only asks "is it a date at all").
fn probe_date() -> CalendarDate {
    CalendarDate::min()
}

/// Header words, upper-cased, for each role.
const DATE_WORDS: [&str; 6] = [
    "POSTING DATE",
    "POSTED DATE",
    "TRANSACTION DATE",
    "TRANS DATE",
    "DATE",
    "POSTED",
];
const AMOUNT_WORDS: [&str; 3] = ["AMOUNT", "TRANSACTION AMOUNT", "AMT"];
const DEBIT_WORDS: [&str; 6] = [
    "DEBIT",
    "DEBITS",
    "WITHDRAWAL",
    "WITHDRAWALS",
    "MONEY OUT",
    "PAYMENTS",
];
const CREDIT_WORDS: [&str; 6] = [
    "CREDIT", "CREDITS", "DEPOSIT", "DEPOSITS", "MONEY IN", "RECEIPTS",
];
const DESC_WORDS: [&str; 6] = [
    "DESCRIPTION",
    "PAYEE",
    "NAME",
    "DETAILS",
    "TRANSACTION",
    "MERCHANT",
];
const MEMO_WORDS: [&str; 3] = ["MEMO", "NOTE", "NOTES"];
const CHECK_WORDS: [&str; 6] = [
    "CHECK NUMBER",
    "CHECK NO",
    "CHECK #",
    "CHECK",
    "CHK",
    "CHECKNUM",
];
const BALANCE_WORDS: [&str; 3] = ["BALANCE", "RUNNING BALANCE", "RUNNING BAL"];
const INDICATOR_WORDS: [&str; 6] = [
    "CR/DR",
    "DR/CR",
    "DEBIT/CREDIT",
    "CREDIT/DEBIT",
    "TYPE",
    "TRANSACTION TYPE",
];

fn header_key(s: &str) -> String {
    s.trim()
        .trim_end_matches(['.', ':'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase()
}

/// Guesses a mapping from the header names, falling back to the values.
/// `None` when no date or amount column can be found.
#[must_use]
pub fn guess(table: &CsvTable) -> Option<CsvMapping> {
    let names: Vec<String> = table
        .header
        .as_ref()
        .map(|h| h.fields.iter().map(|f| header_key(f)).collect())
        .unwrap_or_default();
    let find = |words: &[&str], taken: &[usize]| -> Option<usize> {
        words.iter().find_map(|w| {
            names
                .iter()
                .enumerate()
                .find(|(i, n)| n.as_str() == *w && !taken.contains(i))
                .map(|(i, _)| i)
        })
    };
    let mut taken = Vec::new();
    let date = find(&DATE_WORDS, &taken).or_else(|| {
        column_where(table, |f| {
            parse_date(f, DateOrder::Mdy, probe_date()).is_ok()
        })
    })?;
    taken.push(date);
    let amount = if let Some(a) = find(&AMOUNT_WORDS, &taken) {
        taken.push(a);
        match find(&INDICATOR_WORDS, &taken) {
            Some(ind) if column_is_indicator(table, ind) => {
                taken.push(ind);
                AmountColumns::Indicator {
                    amount: a,
                    indicator: ind,
                }
            }
            _ => AmountColumns::Signed { column: a },
        }
    } else if let (Some(d), Some(c)) = (find(&DEBIT_WORDS, &taken), find(&CREDIT_WORDS, &taken)) {
        taken.extend([d, c]);
        AmountColumns::DebitCredit {
            debit: d,
            credit: c,
        }
    } else {
        let a = amount_column(table, &taken, &names)?;
        taken.push(a);
        AmountColumns::Signed { column: a }
    };
    let balance = find(&BALANCE_WORDS, &taken);
    taken.extend(balance);
    let check_number = find(&CHECK_WORDS, &taken);
    taken.extend(check_number);
    let memo = find(&MEMO_WORDS, &taken);
    taken.extend(memo);
    let description = match find(&DESC_WORDS, &taken) {
        Some(d) => vec![d],
        None => (0..table.width())
            .find(|i| !taken.contains(i) && column_has_text(table, *i))
            .into_iter()
            .collect(),
    };
    Some(CsvMapping {
        version: MAPPING_VERSION,
        has_header: table.header.is_some(),
        date,
        date_order: DateOrder::Mdy,
        amount,
        description,
        memo,
        check_number,
        balance,
        headers: table
            .header
            .as_ref()
            .map(|h| h.fields.iter().map(|f| f.trim().to_owned()).collect())
            .unwrap_or_default(),
    })
}

/// The first column whose non-blank values all pass `test` (at least one).
fn column_where(table: &CsvTable, test: impl Fn(&str) -> bool) -> Option<usize> {
    (0..table.width()).find(|&i| {
        let mut any = false;
        let all = table.rows.iter().take(50).all(|r| match r.fields.get(i) {
            Some(f) if !f.trim().is_empty() => {
                any = true;
                test(f)
            }
            _ => true,
        });
        all && any
    })
}

/// The amount column by its values: every filled value reads as an
/// amount, it is filled on most rows (a check number column is mostly
/// blank) and it is not named like a check number or a balance; one with a
/// decimal point is preferred (review note on mapping.rs).
fn amount_column(table: &CsvTable, taken: &[usize], names: &[String]) -> Option<usize> {
    let sample: Vec<&Row> = table.rows.iter().take(50).collect();
    let candidates: Vec<(usize, bool)> = (0..table.width())
        .filter(|i| !taken.contains(i))
        .filter(|i| {
            names.get(*i).is_none_or(|n| {
                !CHECK_WORDS.contains(&n.as_str()) && !BALANCE_WORDS.contains(&n.as_str())
            })
        })
        .filter_map(|i| {
            let filled: Vec<&str> = sample
                .iter()
                .filter_map(|r| r.fields.get(i).map(String::as_str))
                .filter(|f| !f.trim().is_empty())
                .collect();
            let ok = !filled.is_empty()
                && filled.len() * 2 > sample.len()
                && filled.iter().all(|f| parse_amount(f).is_ok());
            ok.then(|| (i, filled.iter().any(|f| f.contains('.'))))
        })
        .collect();
    candidates
        .iter()
        .find(|(_, point)| *point)
        .or_else(|| candidates.first())
        .map(|(i, _)| *i)
}

fn column_has_text(table: &CsvTable, i: usize) -> bool {
    table.rows.iter().take(50).any(|r| {
        r.fields
            .get(i)
            .is_some_and(|f| f.chars().any(char::is_alphabetic))
    })
}

fn column_is_indicator(table: &CsvTable, i: usize) -> bool {
    let mut any = false;
    let all = table.rows.iter().take(50).all(|r| match r.fields.get(i) {
        Some(f) if !f.trim().is_empty() => {
            any = true;
            indicator(f).is_some()
        }
        _ => true,
    });
    all && any
}

/// `Some(true)` for money out, `Some(false)` for money in.
fn indicator(text: &str) -> Option<bool> {
    match text.trim().to_ascii_uppercase().as_str() {
        "DR" | "D" | "DEBIT" | "W" | "WITHDRAWAL" | "DB" => Some(true),
        "CR" | "C" | "CREDIT" | "DEPOSIT" | "DEP" => Some(false),
        _ => None,
    }
}

/// Applies a mapping to a table, giving one section (Q-P25i: a CSV is one
/// account). Errors cite each record's first line.
///
/// # Errors
/// [`ImportError::MappingDoesNotFit`] when a mapped column does not exist,
/// [`ImportError::TooManyRows`] over the row limit.
pub fn apply(
    table: &CsvTable,
    mapping: &CsvMapping,
    today: CalendarDate,
) -> Result<Section, ImportError> {
    let width = table.width();
    if mapping.columns().iter().any(|c| *c >= width) {
        return Err(ImportError::MappingDoesNotFit);
    }
    let mut section = Section::new(0, AccountKind::Unknown);
    let dates = judge_column(
        table
            .rows
            .iter()
            .filter_map(|r| r.fields.get(mapping.date).map(|f| (r.line, f.as_str()))),
        mapping.date_order,
        today,
    );
    let needed = mapping.columns().into_iter().max().unwrap_or(0) + 1;
    for row in &table.rows {
        let get = |i: usize| row.fields.get(i).map_or("", String::as_str);
        if row.fields.len() < needed && !trailing_blank_ok(row, mapping) {
            section.problems.push(crate::Problem {
                line: row.line,
                kind: ProblemKind::MissingColumns,
            });
            continue;
        }
        let amount = match amount(row, mapping.amount) {
            Ok(a) => Ok(a),
            Err(Direction::Problem(p)) => Err(p),
            Err(Direction::Unclear) => {
                section.problems.push(crate::Problem {
                    line: row.line,
                    kind: ProblemKind::UnclearDirection,
                });
                continue;
            }
        };
        let description: Vec<&str> = mapping
            .description
            .iter()
            .map(|&i| get(i).trim())
            .filter(|s| !s.is_empty())
            .collect();
        let description = description.join(" ");
        section.push_parts(&RowParts {
            line: row.line,
            date: parse_date(get(mapping.date), mapping.date_order, today),
            amount,
            description: &description,
            memo: mapping.memo.map(get),
            check: mapping.check_number.map(get),
            fitid: None,
            running_balance: mapping.balance.and_then(|i| parse_amount(get(i)).ok()),
            trn_type: None,
        });
    }
    section.check_row_limit()?;
    section.dates = Some(dates);
    section.fill_range_from_rows();
    crate::sign::analyze(&mut section);
    Ok(section)
}

/// A short row is fine when only optional trailing columns are missing
/// (some exporters drop empty trailing fields).
fn trailing_blank_ok(row: &Row, mapping: &CsvMapping) -> bool {
    let mut required = vec![mapping.date];
    required.extend(mapping.amount.columns());
    required.iter().all(|c| *c < row.fields.len())
}

enum Direction {
    Problem(AmountProblem),
    Unclear,
}

fn amount(row: &Row, layout: AmountColumns) -> Result<ParsedAmount, Direction> {
    let get = |i: usize| row.fields.get(i).map_or("", String::as_str);
    let blank = |s: &str| s.trim().is_empty();
    let with = |a: ParsedAmount, value: Decimal| ParsedAmount {
        value,
        rounded_from: a.rounded_from,
    };
    match layout {
        AmountColumns::Signed { column } => {
            parse_amount_detail(get(column)).map_err(Direction::Problem)
        }
        AmountColumns::DebitCredit { debit, credit } => {
            let d = (!blank(get(debit)))
                .then(|| parse_amount_detail(get(debit)))
                .transpose()
                .map_err(Direction::Problem)?;
            let c = (!blank(get(credit)))
                .then(|| parse_amount_detail(get(credit)))
                .transpose()
                .map_err(Direction::Problem)?;
            let d = d.filter(|v| !v.value.is_zero());
            let c = c.filter(|v| !v.value.is_zero());
            match (d, c) {
                (Some(_), Some(_)) => Err(Direction::Unclear),
                (Some(d), None) => {
                    let v = -d.value.abs();
                    Ok(with(d, v))
                }
                (None, Some(c)) => {
                    let v = c.value.abs();
                    Ok(with(c, v))
                }
                // Both blank or zero: a zero row, skipped like any other.
                (None, None) if !blank(get(debit)) || !blank(get(credit)) => Ok(ParsedAmount {
                    value: Decimal::ZERO,
                    rounded_from: None,
                }),
                (None, None) => Err(Direction::Problem(AmountProblem::Missing)),
            }
        }
        AmountColumns::Indicator {
            amount,
            indicator: ind,
        } => {
            let a = parse_amount_detail(get(amount)).map_err(Direction::Problem)?;
            match indicator(get(ind)) {
                Some(true) => {
                    let v = -a.value.abs();
                    Ok(with(a, v))
                }
                Some(false) => {
                    let v = a.value.abs();
                    Ok(with(a, v))
                }
                None => Err(Direction::Unclear),
            }
        }
    }
}

/// Reads a bank CSV file with the given or guessed mapping.
///
/// # Errors
/// As [`read_table`] and [`apply`], plus [`ImportError::CsvNeedsMapping`]
/// when no mapping was given and none can be guessed.
pub fn parse(bytes: &[u8], options: &ParseOptions) -> Result<ParsedFile, ImportError> {
    let table = read_table(bytes)?;
    // A remembered mapping is used only when it fits this file's columns
    // (review DB2): reordered columns with the same count, or swapped
    // debit and credit headers, would otherwise be read with the old
    // columns. A mapping the owner just chose is always applied.
    let given = options
        .csv_mapping
        .as_ref()
        .filter(|m| !options.csv_mapping_remembered || m.fits(&table));
    let mut mapping = match given {
        Some(m) => m.clone(),
        None => guess(&table).ok_or(ImportError::CsvNeedsMapping)?,
    };
    if let Some(order) = options.date_order {
        mapping.date_order = order;
    }
    let mut section = apply(&table, &mapping, options.today)?;
    section.skipped.splice(
        0..0,
        table
            .preamble
            .iter()
            .map(|&line| crate::normalize::SkippedRow {
                line,
                reason: crate::normalize::SkipReason::Preamble,
            }),
    );
    Ok(ParsedFile {
        format: FileFormat::Csv,
        encoding: table.encoding,
        encoding_note: None,
        institution: None,
        sections: vec![section],
        csv_headers: Some(table.column_names()),
        csv_mapping: Some(mapping),
    })
}

/// The date verdict for one column under one order (for the mapping step).
#[must_use]
pub fn date_column(
    table: &CsvTable,
    column: usize,
    order: DateOrder,
    today: CalendarDate,
) -> DateColumn {
    judge_column(
        table
            .rows
            .iter()
            .filter_map(|r| r.fields.get(column).map(|f| (r.line, f.as_str()))),
        order,
        today,
    )
}
