//! QIF bank, card and cash registers (P25 step 3).
//!
//! - `!Type:Bank`, `!Type:CCard` and `!Type:Cash` are read; `!Type:Invst`
//!   and other account types (`Oth A`, `Oth L`, `Invoice`) are refused
//!   with their line; lists (`Cat`, `Class`, `Memorized`, `Security`,
//!   `Prices`) are skipped.
//! - `!Account` blocks name the account of the register that follows; each
//!   register is a section.
//! - A missing final `^` is tolerated; dates may be padded (`1/ 5/26`).
//! - Dates are US order by default; the whole column is judged at once
//!   (Q-195), and two-digit years follow Q-P25g using the injected today.

use crate::csv::dates::{DateOrder, judge_column, parse_date};
use crate::detect::FileFormat;
use crate::error::ImportError;
use crate::normalize::{
    AccountKind, AmountProblem, Balance, Declared, ParsedFile, ProblemKind, RowParts, Section,
    SkipReason, SkippedRow, decode, parse_amount_detail,
};
use cl_core::CalendarDate;

#[derive(Debug, Default)]
struct Record {
    line: usize,
    date: Option<String>,
    amount: Option<String>,
    payee: Option<String>,
    memo: Option<String>,
    number: Option<String>,
    category: Option<String>,
}

impl Record {
    fn is_empty(&self) -> bool {
        self.date.is_none()
            && self.amount.is_none()
            && self.payee.is_none()
            && self.memo.is_none()
            && self.number.is_none()
    }
}

struct Register {
    kind: AccountKind,
    name: Option<String>,
    records: Vec<Record>,
}

enum Mode {
    /// Before any header, or inside a skipped list.
    Skip,
    /// Inside an `!Account` block.
    Account,
    /// Inside a register.
    Register,
}

/// Reads a QIF file.
///
/// # Errors
/// [`ImportError::Investment`] or [`ImportError::UnsupportedQifType`] for
/// registers that cannot be imported, [`ImportError::NoStatement`] when the
/// file has no bank, card or cash register.
pub fn parse(
    bytes: &[u8],
    today: CalendarDate,
    order: Option<DateOrder>,
) -> Result<ParsedFile, ImportError> {
    if bytes.len() > crate::MAX_FILE_BYTES {
        return Err(ImportError::TooLarge {
            limit_bytes: crate::MAX_FILE_BYTES,
        });
    }
    let decoded = decode(bytes, Declared::Nothing)?;
    let registers = read_registers(&decoded.text)?;
    let mut sections = Vec::with_capacity(registers.len());
    for (index, reg) in registers.into_iter().enumerate() {
        sections.push(section(index, reg, today, order)?);
    }
    Ok(ParsedFile {
        format: FileFormat::Qif,
        encoding: decoded.encoding,
        encoding_note: None,
        institution: None,
        sections,
        csv_mapping: None,
        csv_headers: None,
    })
}

fn read_registers(text: &str) -> Result<Vec<Register>, ImportError> {
    let mut registers: Vec<Register> = Vec::new();
    let mut mode = Mode::Skip;
    let mut account_name: Option<String> = None;
    let mut pending_name: Option<String> = None;
    let mut rec = Record::default();

    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim_end_matches('\r');
        if l.trim().is_empty() {
            continue;
        }
        if let Some(header) = l.strip_prefix('!') {
            flush(&mut registers, &mut rec);
            let upper = header.trim().to_ascii_uppercase();
            if upper == "ACCOUNT" {
                mode = Mode::Account;
                account_name = None;
                continue;
            }
            if upper.starts_with("OPTION") || upper.starts_with("CLEAR") {
                continue;
            }
            let Some(ty) = upper.strip_prefix("TYPE:").map(str::trim) else {
                return Err(ImportError::UnsupportedQifType { line });
            };
            let kind = match ty {
                "BANK" => Some(AccountKind::Checking),
                "CCARD" => Some(AccountKind::CreditCard),
                "CASH" => Some(AccountKind::Cash),
                "INVST" => return Err(ImportError::Investment { line }),
                "CAT" | "CLASS" | "MEMORIZED" | "SECURITY" | "PRICES" | "BUDGET" | "TAG" => None,
                _ => return Err(ImportError::UnsupportedQifType { line }),
            };
            if let Some(kind) = kind {
                registers.push(Register {
                    kind,
                    name: pending_name.take(),
                    records: Vec::new(),
                });
                mode = Mode::Register;
            } else {
                mode = Mode::Skip;
            }
            continue;
        }
        match mode {
            Mode::Skip => {}
            Mode::Account => {
                if l.starts_with('^') {
                    pending_name = account_name.take();
                    mode = Mode::Skip;
                } else if let Some(n) = l.strip_prefix('N') {
                    account_name = Some(n.trim().chars().take(100).collect());
                }
            }
            Mode::Register => {
                if l.starts_with('^') {
                    flush(&mut registers, &mut rec);
                    continue;
                }
                if rec.is_empty() {
                    rec.line = line;
                }
                let split = l.chars().next().map_or(0, char::len_utf8);
                let (code, value) = l.split_at(split);
                let value = value.to_owned();
                match code {
                    "D" => rec.date = Some(value),
                    "T" => rec.amount = Some(value),
                    "U" => {
                        if rec.amount.is_none() {
                            rec.amount = Some(value);
                        }
                    }
                    "P" => rec.payee = Some(value),
                    "M" => rec.memo = Some(value),
                    "N" => rec.number = Some(value),
                    "L" => rec.category = Some(value),
                    // Category, cleared flag, address, splits: not needed.
                    _ => {}
                }
            }
        }
    }
    flush(&mut registers, &mut rec);
    if registers.is_empty() {
        return Err(ImportError::NoStatement);
    }
    Ok(registers)
}

fn section(
    index: usize,
    reg: Register,
    today: CalendarDate,
    order: Option<DateOrder>,
) -> Result<Section, ImportError> {
    let column = judge_column(
        reg.records
            .iter()
            .filter_map(|r| r.date.as_deref().map(|d| (r.line, d))),
        order.unwrap_or_default(),
        today,
    );
    let mut section = Section::new(index, reg.kind);
    section.account_name = reg.name;
    for r in &reg.records {
        if is_opening_balance(r) {
            if section.opening_balance.is_none()
                && let (Some(Ok(amount)), Some(Ok(date))) = (
                    r.amount.as_deref().map(parse_amount_detail),
                    r.date
                        .as_deref()
                        .map(|d| parse_date(d, column.order, today)),
                )
            {
                section.opening_balance = Some(Balance {
                    amount: amount.value,
                    date: Some(date),
                });
            }
            section.skipped.push(SkippedRow {
                line: r.line,
                reason: SkipReason::OpeningBalance,
            });
            continue;
        }
        let (description, memo) = match (&r.payee, &r.memo) {
            (Some(p), m) if !p.trim().is_empty() => (p.as_str(), m.as_deref()),
            (_, Some(m)) => (m.as_str(), None),
            _ => ("", None),
        };
        section.push_parts(&RowParts {
            line: r.line,
            date: r
                .date
                .as_deref()
                .map_or(Err(ProblemKind::MissingDate), |d| {
                    parse_date(d, column.order, today)
                }),
            amount: r
                .amount
                .as_deref()
                .map_or(Err(AmountProblem::Missing), parse_amount_detail),
            description,
            memo,
            check: r.number.as_deref(),
            fitid: None,
            running_balance: None,
            trn_type: None,
        });
    }
    section.check_row_limit()?;
    section.dates = Some(column);
    section.fill_range_from_rows();
    crate::sign::analyze(&mut section);
    Ok(section)
}

/// Quicken's first record: payee "Opening Balance" and a category in
/// brackets (a transfer to the account itself).
fn is_opening_balance(r: &Record) -> bool {
    let payee = r.payee.as_deref().map(str::trim);
    let category = r.category.as_deref().map(str::trim);
    payee.is_some_and(|p| p.eq_ignore_ascii_case("Opening Balance"))
        && category.is_some_and(|c| c.starts_with('[') && c.ends_with(']'))
}

/// Ends the current record (at `^`, a header, or the end of the file).
fn flush(registers: &mut [Register], rec: &mut Record) {
    let r = std::mem::take(rec);
    if r.is_empty() {
        return;
    }
    if let Some(reg) = registers.last_mut() {
        reg.records.push(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 7).unwrap()
    }

    #[test]
    fn reads_bank_register_without_final_caret() {
        let q = b"!Type:Cat\nNFood\n^\n!Type:Bank\nD1/ 5/26\nT-12.50\nPCoffee\nN1001\n^\nD1/ 6'26\nU100.00\nT100.00\nMDeposit\n";
        let f = parse(q, today(), None).unwrap();
        let s = &f.sections[0];
        assert_eq!(s.rows.len(), 2);
        assert_eq!(s.rows[0].line, 5);
        assert_eq!(s.rows[0].check_number, Some(1001));
        assert_eq!(s.rows[1].description, "Deposit");
        assert_eq!(s.rows[1].date, CalendarDate::from_ymd(2026, 1, 6).unwrap());
    }

    #[test]
    fn refuses_investment_and_other_types() {
        assert_eq!(
            parse(b"!Type:Bank\n^\n!Type:Invst\n", today(), None).unwrap_err(),
            ImportError::Investment { line: 3 }
        );
        assert_eq!(
            parse(b"\n!Type:Oth A\n", today(), None).unwrap_err(),
            ImportError::UnsupportedQifType { line: 2 }
        );
        assert_eq!(
            parse(b"!Type:Cat\nNx\n^\n", today(), None).unwrap_err(),
            ImportError::NoStatement
        );
    }
}
