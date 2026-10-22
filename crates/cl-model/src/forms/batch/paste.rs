//! Bulk Entry paste and CSV mapping helpers (P15A WP-B3).
//!
//! Paste and CSV both go through the engine's `parse-csv` (Q-P15Af); the
//! screen reads only the header and a few rows locally for the mapping
//! preview. This module decides:
//!
//! - whether a first row is a header ([`looks_like_header`]);
//! - the default column mapping, from header aliases ([`default_mapping`])
//!   or, for a headerless paste, left to right from the cursor column
//!   ([`positional_mapping`]);
//! - the remembered mappings per company and type, keyed by the header set
//!   so a reordered file still maps ([`MappingMemory`]);
//! - how the engine's text cells become rows ([`rows_from_parsed`]):
//!   amounts are re-parsed with the owner's money rules (rounded to the
//!   cent, Q-232) and dates with the US date parser.
//!
//! Tags are never imported (Q-P15Af).

use std::collections::BTreeMap;
use std::fmt;

use cl_core::CalendarDate;
use serde::{Deserialize, Serialize};

use super::{Field, RowDraft, RowKeyGen, SectionKind, parse_amount_literal};
use crate::date_entry::{self, DateOrder};

/// Field → 0-based column index (the screen maps it onto
/// `cl_api::batch::ColumnMapping`: Number → `ref_no`, Contact → `payee`).
pub type Mapping = BTreeMap<Field, u16>;

/// The most remembered mappings kept per company (oldest dropped first).
pub const MAX_REMEMBERED: usize = 100;

/// The fields a pasted or CSV column can fill for `kind`: its columns but
/// Print later and Tag.
#[must_use]
pub fn mappable_fields(kind: SectionKind) -> Vec<Field> {
    kind.columns()
        .iter()
        .copied()
        .filter(|f| !matches!(f, Field::PrintLater | Field::Tag))
        .collect()
}

/// A leap year, so `2/29` reads as a date.
fn any_day() -> Option<CalendarDate> {
    CalendarDate::from_ymd(2000, 1, 1).ok()
}

fn is_date(cell: &str) -> bool {
    CalendarDate::parse_wire(cell.trim()).is_ok()
        || any_day().is_some_and(|today| {
            date_entry::parse(cell, DateOrder::Mdy, today).is_ok()
                || date_entry::parse(cell, DateOrder::Ymd, today).is_ok()
        })
}

fn is_amount(cell: &str) -> bool {
    matches!(parse_amount_literal(cell), Ok(Some(_)))
}

/// True when the first row of a paste is a header: it has text and no cell
/// reads as a date or an amount (plan §5; the owner can flip "First row is
/// headers" in the mapping panel).
#[must_use]
pub fn looks_like_header(cells: &[&str]) -> bool {
    let filled: Vec<&str> = cells
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .collect();
    !filled.is_empty() && !filled.iter().any(|c| is_date(c) || is_amount(c))
}

/// A header as compared: lowercase, single spaces, no trailing `.` or `:`,
/// `#` read as "no".
fn normalize_header(h: &str) -> String {
    let lower = h.trim().to_lowercase().replace('#', " no");
    let words: Vec<&str> = lower.split_whitespace().collect();
    words
        .join(" ")
        .trim_end_matches(['.', ':'])
        .replace(". ", " ")
        .trim()
        .to_string()
}

/// The field a header names, if any.
fn alias(header: &str) -> Option<Field> {
    let h = normalize_header(header);
    let field = match h.as_str() {
        "date" | "txn date" | "transaction date" | "trans date" | "posted" | "post date"
        | "posting date" | "posted date" => Field::Date,
        "no" | "num" | "number" | "check" | "check no" | "check number" | "chk no" | "ref"
        | "ref no" | "reference" | "reference no" | "entry" | "entry no" | "journal no"
        | "doc no" => Field::Number,
        "payee"
        | "pay to"
        | "pay to the order of"
        | "name"
        | "vendor"
        | "customer"
        | "received from"
        | "payer"
        | "merchant" => Field::Contact,
        "account" | "account name" | "category" | "expense account" | "from account" | "split"
        | "gl account" => Field::Account,
        "amount" | "total" | "payment" | "deposit" | "charge" => Field::Amount,
        "debit" | "debits" | "dr" | "debit amount" => Field::Debit,
        "credit" | "credits" | "cr" | "credit amount" => Field::Credit,
        "memo" | "description" | "notes" | "note" | "details" | "memo/description" => Field::Memo,
        _ => return None,
    };
    Some(field)
}

/// The mapping header aliases suggest for `kind`: each field takes the
/// first column whose header names it; fields the kind does not have are
/// left out.
#[must_use]
pub fn default_mapping(kind: SectionKind, headers: &[&str]) -> Mapping {
    let fields = mappable_fields(kind);
    let mut out = Mapping::new();
    for (i, h) in headers.iter().enumerate() {
        let Ok(col) = u16::try_from(i) else { break };
        if let Some(f) = alias(h).filter(|f| fields.contains(f)) {
            out.entry(f).or_insert(col);
        }
    }
    out
}

/// The mapping for a headerless paste: pasted column `j` fills grid column
/// `start_column + j` of `kind`; columns that land on Print later, Tag or
/// past the last column are not imported.
#[must_use]
pub fn positional_mapping(kind: SectionKind, start_column: usize, width: usize) -> Mapping {
    let cols = kind.columns();
    let mut out = Mapping::new();
    for j in 0..width {
        let (Some(f), Ok(col)) = (cols.get(start_column + j), u16::try_from(j)) else {
            continue;
        };
        if !matches!(f, Field::PrintLater | Field::Tag) {
            out.insert(*f, col);
        }
    }
    out
}

/// One remembered mapping.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RememberedMapping {
    /// The section type.
    pub kind: SectionKind,
    /// The header set: normalized, sorted, without duplicates.
    pub signature: Vec<String>,
    /// Field → the normalized header it came from.
    pub fields: BTreeMap<Field, String>,
}

/// Mappings the owner asked to remember ("Remember for this company"),
/// per type and header set (Q-P15Af). Stored with the company's
/// preferences by the screen.
#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MappingMemory {
    /// Oldest first.
    pub entries: Vec<RememberedMapping>,
}

/// Counts only: headers can be anything a file holds.
impl fmt::Debug for MappingMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MappingMemory")
            .field("entries", &self.entries.len())
            .finish()
    }
}

fn signature(headers: &[&str]) -> Vec<String> {
    let mut s: Vec<String> = headers
        .iter()
        .map(|h| normalize_header(h))
        .filter(|h| !h.is_empty())
        .collect();
    s.sort();
    s.dedup();
    s
}

impl MappingMemory {
    /// Remembers `mapping` for `kind` and these headers (replacing an older
    /// one for the same header set).
    pub fn remember(&mut self, kind: SectionKind, headers: &[&str], mapping: &Mapping) {
        let sig = signature(headers);
        let fields: BTreeMap<Field, String> = mapping
            .iter()
            .filter_map(|(f, col)| {
                headers
                    .get(usize::from(*col))
                    .map(|h| (*f, normalize_header(h)))
            })
            .filter(|(_, h)| !h.is_empty())
            .collect();
        self.entries
            .retain(|e| !(e.kind == kind && e.signature == sig));
        self.entries.push(RememberedMapping {
            kind,
            signature: sig,
            fields,
        });
        if self.entries.len() > MAX_REMEMBERED {
            let extra = self.entries.len() - MAX_REMEMBERED;
            self.entries.drain(..extra);
        }
    }

    /// The remembered mapping for `kind` and this header set, resolved to
    /// the columns' current positions.
    #[must_use]
    pub fn recall(&self, kind: SectionKind, headers: &[&str]) -> Option<Mapping> {
        let sig = signature(headers);
        let entry = self
            .entries
            .iter()
            .rev()
            .find(|e| e.kind == kind && e.signature == sig)?;
        let normalized: Vec<String> = headers.iter().map(|h| normalize_header(h)).collect();
        Some(
            entry
                .fields
                .iter()
                .filter_map(|(f, name)| {
                    let i = normalized.iter().position(|h| h == name)?;
                    u16::try_from(i).ok().map(|col| (*f, col))
                })
                .collect(),
        )
    }
}

/// One row the engine's `parse-csv` returned, as text (the screen copies
/// it from `cl_api::batch::ParsedRow`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ParsedCells {
    /// Date text.
    pub date: Option<String>,
    /// Check number, Ref no. or Entry No.
    pub number: Option<String>,
    /// The name.
    pub contact: Option<String>,
    /// The account name or number.
    pub account: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// Amount text.
    pub amount: Option<String>,
    /// Debit text.
    pub debit: Option<String>,
    /// Credit text.
    pub credit: Option<String>,
}

/// Nothing from the file.
impl fmt::Debug for ParsedCells {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParsedCells").finish_non_exhaustive()
    }
}

/// A row made from parsed text, with the cells that could not be read
/// (left empty, so the row shows them as missing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportedRow {
    /// The row.
    pub row: RowDraft,
    /// Cells whose text is not a date or an amount.
    pub unreadable: Vec<Field>,
}

fn text(cell: Option<&String>) -> String {
    cell.map(|s| s.trim().to_string()).unwrap_or_default()
}

fn read_date(
    cell: Option<&String>,
    order: DateOrder,
    today: CalendarDate,
    unreadable: &mut Vec<Field>,
) -> Option<CalendarDate> {
    let t = cell.map(|s| s.trim()).filter(|s| !s.is_empty())?;
    let parsed = CalendarDate::parse_wire(t)
        .ok()
        .or_else(|| date_entry::parse(t, order, today).ok());
    if parsed.is_none() {
        unreadable.push(Field::Date);
    }
    parsed
}

fn read_amount(
    field: Field,
    cell: Option<&String>,
    unreadable: &mut Vec<Field>,
) -> Option<cl_core::Money> {
    let t = cell?;
    parse_amount_literal(t).unwrap_or_else(|_| {
        unreadable.push(field);
        None
    })
}

/// Rows for a `kind` section from the engine's parsed text: names and
/// accounts as typed text (resolved by the engine's Validate), amounts
/// re-parsed literally with the money rules (`$`, `,`, `(x)`; rounded to
/// the cent, Q-232; a negative stays negative and shows as a problem;
/// never a calculation, so `100/4` is unreadable), dates as ISO
/// or the owner's typed date order. Journal entries take Debit and Credit;
/// other kinds take Amount. Empty records are dropped. New keys come from
/// `keys`.
#[must_use]
pub fn rows_from_parsed(
    kind: SectionKind,
    parsed: &[ParsedCells],
    order: DateOrder,
    today: CalendarDate,
    keys: &mut RowKeyGen,
) -> Vec<ImportedRow> {
    let journal = kind.is_journal();
    let mut out = Vec::new();
    for p in parsed {
        let mut unreadable = Vec::new();
        let date = read_date(p.date.as_ref(), order, today, &mut unreadable);
        let (amount, debit, credit) = if journal {
            (
                None,
                read_amount(Field::Debit, p.debit.as_ref(), &mut unreadable),
                read_amount(Field::Credit, p.credit.as_ref(), &mut unreadable),
            )
        } else {
            (
                read_amount(Field::Amount, p.amount.as_ref(), &mut unreadable),
                None,
                None,
            )
        };
        let row = RowDraft {
            key: super::RowKey::default(),
            date,
            number: text(p.number.as_ref()),
            print_later: false,
            contact: None,
            contact_text: text(p.contact.as_ref()),
            account: None,
            account_text: text(p.account.as_ref()),
            amount,
            debit,
            credit,
            memo: p.memo.clone().unwrap_or_default(),
            tag: None,
        };
        if row.is_blank() && row.date.is_none() && row.number.is_empty() && unreadable.is_empty() {
            continue;
        }
        out.push(ImportedRow {
            row: RowDraft {
                key: keys.next_key(),
                ..row
            },
            unreadable,
        });
    }
    out
}
