//! A statement's checks (A06 step 4). They are flagged, never adjusted:
//! every check only points at a value; nothing here writes one.
//!
//! - the lines add up to closing − opening (opening 0 when not printed),
//! - each running balance is the one before plus the line (the first line
//!   starts from the opening balance when printed),
//! - each line's date falls within the period,
//! - no two lines share date, reference and amount.
//!
//! The whole-statement check comes first so the per-line ones can't push
//! it past the cap on checks.

use crate::model::{Check, CheckCode, CheckValue, MAX_CHECKS, RuleId, StatementRead};
use cl_core::Money;
use std::collections::HashSet;

fn check(c: &CodeAt) -> Check {
    let code = c;
    Check {
        code: code.code,
        rule: RuleId::new(code.rule),
        field: None,
        line: code.line,
        expected: code.expected.map(CheckValue::Money),
        found: code.found.map(CheckValue::Money),
        difference: match (code.found, code.expected) {
            (Some(f), Some(e)) => f.checked_sub(e).ok(),
            _ => None,
        },
        spot: code.spot,
    }
}

struct CodeAt {
    code: CheckCode,
    rule: &'static str,
    line: Option<u16>,
    expected: Option<Money>,
    found: Option<Money>,
    spot: Option<crate::model::Spot>,
}

fn index(i: usize) -> Option<u16> {
    u16::try_from(i).ok()
}

/// The checks on a statement reading; the reading is not changed.
#[must_use]
pub fn run(st: &StatementRead) -> Vec<Check> {
    let mut out = Vec::new();
    if let Some(closing) = &st.closing
        && !st.lines.is_empty()
    {
        let opening = st.opening.as_ref().map_or(Money::ZERO, |o| o.value);
        let lines = Money::sum(st.lines.iter().map(|l| l.amount.value)).ok();
        let expected = closing.value.checked_sub(opening).ok();
        if lines.is_none() || lines != expected {
            out.push(check(&CodeAt {
                code: CheckCode::StmtSum,
                rule: "statement.sum",
                line: None,
                expected,
                found: lines,
                spot: Some(closing.spot),
            }));
        }
    }
    let mut prev = st.opening.as_ref().map(|o| o.value);
    for (i, l) in st.lines.iter().enumerate() {
        if let Some(r) = &l.running {
            if let Some(p) = prev {
                let expected = p.checked_add(l.amount.value).ok();
                if expected != Some(r.value) {
                    out.push(check(&CodeAt {
                        code: CheckCode::StmtRunning,
                        rule: "statement.running",
                        line: index(i),
                        expected,
                        found: Some(r.value),
                        spot: Some(r.spot),
                    }));
                }
            }
            prev = Some(r.value);
        } else {
            prev = prev.and_then(|p| p.checked_add(l.amount.value).ok());
        }
    }
    let from = st.period_from.as_ref().map(|f| f.value);
    let to = st.period_to.as_ref().map(|f| f.value);
    for (i, l) in st.lines.iter().enumerate() {
        let d = l.date.value;
        if from.is_some_and(|f| d < f) || to.is_some_and(|t| d > t) {
            out.push(Check {
                code: CheckCode::StmtDateOutsidePeriod,
                rule: RuleId::new("statement.period"),
                field: None,
                line: index(i),
                expected: None,
                found: Some(CheckValue::Date(d)),
                difference: None,
                spot: Some(l.date.spot),
            });
        }
    }
    let mut seen = HashSet::new();
    for (i, l) in st.lines.iter().enumerate() {
        let key = (
            l.date.value,
            l.reference.as_ref().map(|r| r.value.to_lowercase()),
            l.amount.value,
        );
        if !seen.insert(key) {
            out.push(check(&CodeAt {
                code: CheckCode::StmtDuplicateLine,
                rule: "statement.duplicate",
                line: index(i),
                expected: None,
                found: None,
                spot: Some(l.amount.spot),
            }));
        }
    }
    out.truncate(MAX_CHECKS);
    out
}
