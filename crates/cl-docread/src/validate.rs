//! Reconciliation checks (WP-A, step 10; plan D3 `validate.rs`): lines =
//! subtotal; subtotal + tax (+ labelled fees) = total; total − paid =
//! balance; due date = date + terms. Each problem becomes a `Check` with
//! expected, found and difference (`found − expected`). Checks never change
//! a value (AC3): the signature takes the reading by shared reference.
//!
//! With lines but no subtotal, and no tax or fees, the lines are checked
//! against the total. A missing "amount paid" counts as zero when a balance
//! is printed. Arithmetic that would overflow skips the check.
//!
//! Internal to the rules.

use crate::lines;
use crate::model::{Check, CheckCode, CheckValue, FieldName, Found, Header, Line, RuleId};
use cl_core::Money;

/// Check the reading's numbers against each other.
#[must_use]
pub fn check(header: &Header, lines: &[Line]) -> Vec<Check> {
    let mut out = Vec::new();
    // Lines = subtotal (or total when nothing sits between them).
    if !lines.is_empty()
        && let Some(sum) = lines::sum(lines)
    {
        let target = match (&header.subtotal, &header.total) {
            (Some(s), _) => Some((s, FieldName::Subtotal)),
            (None, Some(t)) if header.tax.is_none() && header.fees.is_empty() => {
                Some((t, FieldName::Total))
            }
            _ => None,
        };
        if let Some((f, field)) = target {
            out.extend(money_check(
                CheckCode::LinesVsSubtotal,
                "validate.lines",
                field,
                sum,
                f,
            ));
        }
    }
    // Subtotal + tax + fees = total.
    if let (Some(sub), Some(total)) = (&header.subtotal, &header.total) {
        let parts = std::iter::once(sub.value)
            .chain(header.tax.iter().map(|t| t.value))
            .chain(header.fees.iter().map(|f| f.value));
        if let Ok(expected) = Money::sum(parts) {
            out.extend(money_check(
                CheckCode::SubtotalTaxVsTotal,
                "validate.total",
                FieldName::Total,
                expected,
                total,
            ));
        }
    }
    // Total − paid = balance.
    if let (Some(total), Some(balance)) = (&header.total, &header.balance_due) {
        let paid = header.amount_paid.as_ref().map_or(Money::ZERO, |p| p.value);
        if let Ok(expected) = total.value.checked_sub(paid) {
            out.extend(money_check(
                CheckCode::TotalPaidVsBalance,
                "validate.balance",
                FieldName::BalanceDue,
                expected,
                balance,
            ));
        }
    }
    // Due = date + terms.
    if let (Some(date), Some(terms), Some(due)) = (&header.date, &header.terms, &header.due_date)
        && let Some(days) = terms.value.days
        && let Ok(expected) = date.value.add_days(i64::from(days))
        && expected != due.value
    {
        out.push(Check {
            code: CheckCode::DueDateVsTerms,
            rule: RuleId::new("validate.due-date"),
            field: Some(FieldName::DueDate),
            line: None,
            expected: Some(CheckValue::Date(expected)),
            found: Some(CheckValue::Date(due.value)),
            difference: None,
            spot: Some(due.spot),
        });
    }
    out
}

fn money_check(
    code: CheckCode,
    rule: &'static str,
    field: FieldName,
    expected: Money,
    found: &Found<Money>,
) -> Option<Check> {
    if expected == found.value {
        return None;
    }
    Some(Check {
        code,
        rule: RuleId::new(rule),
        field: Some(field),
        line: None,
        expected: Some(CheckValue::Money(expected)),
        found: Some(CheckValue::Money(found.value)),
        difference: found.value.checked_sub(expected).ok(),
        spot: Some(found.spot),
    })
}

#[cfg(test)]
#[allow(
    clippy::field_reassign_with_default,
    reason = "tests build headers field by field"
)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Confidence, Spot, Terms};
    use cl_core::CalendarDate;
    use proptest::prelude::*;

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: Spot {
                page: 0,
                bbox: Bbox::PAGE,
            },
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    fn m(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    fn line(a: &str) -> Line {
        Line {
            description: None,
            quantity: None,
            unit_price: None,
            amount: found(m(a)),
        }
    }

    fn codes(h: &Header, lines: &[Line]) -> Vec<CheckCode> {
        check(h, lines).into_iter().map(|c| c.code).collect()
    }

    #[test]
    fn consistent_documents_raise_nothing() {
        let mut h = Header::default();
        h.subtotal = Some(found(m("304.00")));
        h.tax = Some(found(m("25.08")));
        h.fees = vec![found(m("15"))];
        h.total = Some(found(m("344.08")));
        h.amount_paid = Some(found(m("44.08")));
        h.balance_due = Some(found(m("300")));
        h.date = Some(found(CalendarDate::from_ymd(2026, 7, 14).unwrap()));
        h.terms = Some(found(Terms {
            text: "Net 30".into(),
            days: Some(30),
        }));
        h.due_date = Some(found(CalendarDate::from_ymd(2026, 8, 13).unwrap()));
        assert_eq!(codes(&h, &[line("294"), line("10")]), []);
    }

    #[test]
    fn each_mismatch_is_reported_with_its_difference() {
        let mut h = Header::default();
        h.subtotal = Some(found(m("300.00")));
        h.tax = Some(found(m("25.08")));
        h.total = Some(found(m("326.08")));
        h.balance_due = Some(found(m("300")));
        h.date = Some(found(CalendarDate::from_ymd(2026, 7, 14).unwrap()));
        h.terms = Some(found(Terms {
            text: "Net 30".into(),
            days: Some(30),
        }));
        h.due_date = Some(found(CalendarDate::from_ymd(2026, 8, 14).unwrap()));
        let checks = check(&h, &[line("294"), line("10")]);
        let got: Vec<_> = checks.iter().map(|c| (c.code, c.difference)).collect();
        assert_eq!(
            got,
            [
                (CheckCode::LinesVsSubtotal, Some(m("-4"))),
                (CheckCode::SubtotalTaxVsTotal, Some(m("1"))),
                (CheckCode::TotalPaidVsBalance, Some(m("-26.08"))),
                (CheckCode::DueDateVsTerms, None),
            ]
        );
    }

    #[test]
    fn lines_against_total_when_nothing_between() {
        let mut h = Header::default();
        h.total = Some(found(m("12")));
        assert_eq!(codes(&h, &[line("10")]), [CheckCode::LinesVsSubtotal]);
        h.tax = Some(found(m("2")));
        assert_eq!(codes(&h, &[line("10")]), []);
    }

    proptest! {
        /// AC3: whatever the numbers, a mismatch produces a check and the
        /// header is untouched (checked by value equality).
        #[test]
        fn mismatches_always_flagged(sub in 0_i64..10_000_000, tax in 0_i64..1_000_000, delta in -500_i64..500) {
            let mut h = Header::default();
            h.subtotal = Some(found(Money::from_cents(sub).unwrap()));
            h.tax = Some(found(Money::from_cents(tax).unwrap()));
            h.total = Some(found(Money::from_cents(sub + tax + delta).unwrap()));
            let before = h.clone();
            let c = check(&h, &[]);
            prop_assert_eq!(&h, &before);
            prop_assert_eq!(c.iter().any(|c| c.code == CheckCode::SubtotalTaxVsTotal), delta != 0);
        }
    }
}
