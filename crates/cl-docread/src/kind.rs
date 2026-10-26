//! Document kind (WP-A, step 9; plan D3 `kind.rs`): PAID, change given,
//! paid = total, or card last 4 → a receipt; a receipt with card last 4 →
//! card charge; "Check #" → check; "Cash" → cash expense; otherwise a bill;
//! conflicting signs → `Unknown` (the pipeline adds a `KindUnclear`
//! check). The card's digits are reported in the header, never an account.
//!
//! Internal to the rules.

use crate::fields::Signals;
use crate::model::{Confidence, DocKind, Header};
use crate::protocol::JobHint;
use cl_core::intake::IntakeKind;

/// Decide the kind from the header and the payment signs.
#[must_use]
pub fn classify(header: &Header, signals: &Signals, hint: JobHint) -> (DocKind, Confidence) {
    let card = signals.card.is_some() || signals.card_brand;
    // With the card's digits printed, a "Check #" is a restaurant's bill
    // number, not a payment by check.
    let check = signals.check.is_some() && signals.card.is_none();
    let cash = signals.cash.is_some();
    let methods = usize::from(card) + usize::from(check) + usize::from(cash);
    let paid_equals_total = matches!(
        (&header.amount_paid, &header.total),
        (Some(p), Some(t)) if p.value == t.value
    );
    let receipt = signals.paid_stamp.is_some() || signals.change || paid_equals_total || card;
    let (kind, bp): (DocKind, u16) = if methods > 1 {
        (DocKind::Unknown, 0)
    } else if card {
        (
            DocKind::CardCharge,
            if signals.card.is_some() { 9_000 } else { 7_000 },
        )
    } else if check {
        (DocKind::Check, 8_500)
    } else if cash {
        (DocKind::CashExpense, 8_000)
    } else if receipt {
        // Paid, but how is not printed.
        (DocKind::Unknown, 0)
    } else if signals.looks_like_bill {
        (DocKind::Bill, 8_500)
    } else {
        (DocKind::Bill, 6_000)
    };
    let bp = match (hint.kind, kind) {
        (Some(IntakeKind::Bills), DocKind::Bill) => bp.max(9_000),
        (Some(IntakeKind::Receipts), DocKind::Bill) => bp.min(5_000),
        _ => bp,
    };
    (kind, Confidence::saturating(u32::from(bp)))
}

#[cfg(test)]
#[allow(
    clippy::field_reassign_with_default,
    reason = "tests build headers field by field"
)]
mod tests {
    use super::*;
    use crate::model::{Bbox, Found, Last4, RuleId, Spot};
    use cl_core::Money;

    fn spot() -> Spot {
        Spot {
            page: 0,
            bbox: Bbox::PAGE,
        }
    }

    fn found<T>(value: T) -> Found<T> {
        Found {
            value,
            spot: spot(),
            rule: RuleId::new("t"),
            confidence: Confidence::HIGH,
        }
    }

    fn kind(h: &Header, s: &Signals) -> DocKind {
        classify(h, s, JobHint::default()).0
    }

    #[test]
    fn the_kind_table() {
        let h = Header::default();
        let card = Signals {
            card: Some(found(Last4::parse("4821").unwrap())),
            ..Signals::default()
        };
        assert_eq!(kind(&h, &card), DocKind::CardCharge);
        let check = Signals {
            check: Some(found("1042".to_string())),
            ..Signals::default()
        };
        assert_eq!(kind(&h, &check), DocKind::Check);
        let cash = Signals {
            cash: Some(spot()),
            change: true,
            ..Signals::default()
        };
        assert_eq!(kind(&h, &cash), DocKind::CashExpense);
        assert_eq!(kind(&h, &Signals::default()), DocKind::Bill);
        let stamp = Signals {
            paid_stamp: Some(spot()),
            ..Signals::default()
        };
        assert_eq!(
            kind(&h, &stamp),
            DocKind::Unknown,
            "paid, method not printed"
        );
        let both = Signals {
            cash: Some(spot()),
            ..card.clone()
        };
        assert_eq!(kind(&h, &both), DocKind::Unknown, "conflict");
        let restaurant = Signals {
            check: Some(found("1629".to_string())),
            ..card.clone()
        };
        assert_eq!(
            kind(&h, &restaurant),
            DocKind::CardCharge,
            "a check number beside card digits is the bill's"
        );
        let mut paid = Header::default();
        paid.total = Some(found(Money::parse_wire("12.40").unwrap()));
        paid.amount_paid = Some(found(Money::parse_wire("12.40").unwrap()));
        assert_eq!(
            kind(&paid, &Signals::default()),
            DocKind::Unknown,
            "receipt, no method"
        );
    }

    #[test]
    fn hints_move_confidence_not_kind() {
        let h = Header::default();
        let s = Signals::default();
        let bills = classify(
            &h,
            &s,
            JobHint {
                kind: Some(IntakeKind::Bills),
            },
        );
        assert_eq!(bills, (DocKind::Bill, Confidence::saturating(9_000)));
        let receipts = classify(
            &h,
            &s,
            JobHint {
                kind: Some(IntakeKind::Receipts),
            },
        );
        assert_eq!(receipts.0, DocKind::Bill);
        assert!(receipts.1 < Confidence::MEDIUM);
    }
}
