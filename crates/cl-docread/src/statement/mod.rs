//! Reading a vendor statement (A06, ADD-02 steps 1, 2 and 8): the header
//! (vendor, account number, statement date, period, opening and closing
//! balance), the line table with each line's signed amount and running
//! balance, the rows left out of the lines, and the checks.
//!
//! Like the bill rules: no I/O, no clock, no floats; every amount is one
//! page word read by [`crate::amount::parse`] (plus a `CR` or trailing
//! minus that marks a credit); checks only point at values.
//!
//! Statement jobs (`JobHint::statement`) always read this way. Without the
//! hint the pipeline reads a statement only when it is plainly one (a
//! "Statement" title, a period, a balance/charges/credits column and three
//! dated rows or more; see [`is_plainly_a_statement`]).

pub mod checks;
pub mod header;
pub mod infer;
pub mod table;

#[cfg(test)]
mod tests;

use crate::layout::Layout;
use crate::model::{Check, CheckCode, Header, PageWords, RuleId, StatementRead};
use cl_core::CalendarDate;

/// Dated rows a document needs before it is read as a statement without
/// the hint.
pub const MIN_ROWS_UNHINTED: usize = 3;

/// What the statement rules found.
#[derive(Debug, Clone, Default)]
pub struct Statement {
    pub read: StatementRead,
    pub checks: Vec<Check>,
    /// A table header had a balance, charges, credits or payments column.
    pub statement_columns: bool,
}

/// Read a statement from the pages. `base` is the bill rules' header: its
/// vendor and account number are used as they are.
#[must_use]
pub fn read(layout: &Layout, pages: &[PageWords], base: &Header, today: CalendarDate) -> Statement {
    let t = table::read(layout, pages, today);
    let h = header::read(layout, pages, &t.in_table, today);
    let read = StatementRead {
        vendor: base.vendor.clone(),
        account_no: base.account_number.clone(),
        statement_date: h.statement_date,
        period_from: h.period_from,
        period_to: h.period_to,
        opening: h.opening.or(t.forward),
        closing: h.closing.or(t.total),
        lines: t.lines,
        left_out: t.left_out,
    };
    let mut checks = checks::run(&read);
    if t.capped {
        checks.insert(
            0,
            Check {
                code: CheckCode::StmtLinesCapped,
                rule: RuleId::new("statement.cap"),
                field: None,
                line: None,
                expected: None,
                found: None,
                difference: None,
                spot: None,
            },
        );
        checks.truncate(crate::model::MAX_CHECKS);
    }
    Statement {
        read,
        checks,
        statement_columns: t.statement_columns,
    }
}

/// Whether an unhinted document is plainly a statement: titled one, with
/// a period, a balance/charges/credits/payments column and at least
/// [`MIN_ROWS_UNHINTED`] dated lines. Never decided by AI.
#[must_use]
pub fn is_plainly_a_statement(titled: bool, s: &Statement) -> bool {
    titled
        && s.statement_columns
        && s.read.period_from.is_some()
        && s.read.lines.len() >= MIN_ROWS_UNHINTED
}
