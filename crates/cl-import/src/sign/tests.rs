use super::*;
use crate::normalize::{AccountKind, BankRow, Section};
use cl_core::CalendarDate;
use proptest::prelude::*;
use std::str::FromStr as _;

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn row(line: usize, amount: &str, balance: Option<&str>, ty: Option<&str>) -> BankRow {
    BankRow {
        line,
        date: CalendarDate::from_ymd(2026, 11, u32::try_from(line).unwrap()).unwrap(),
        file_amount: dec(amount),
        rounded_from: None,
        description: String::new(),
        memo: None,
        check_number: None,
        fitid: None,
        running_balance: balance.map(dec),
        trn_type: ty.map(str::to_owned),
    }
}

fn section(kind: AccountKind, rows: Vec<BankRow>) -> Section {
    let mut s = Section::new(0, kind);
    s.rows = rows;
    analyze(&mut s);
    s
}

proptest! {
    #[test]
    fn display_and_wire_are_involutions(cents in -99_999_999_999_999i64..=99_999_999_999_999i64, flipped: bool) {
        let a = Decimal::new(cents, 2);
        prop_assert_eq!(to_display(to_display(a, flipped), flipped), a);
        prop_assert_eq!(to_feed_amount(a, flipped), -to_display(a, flipped));
    }
}

#[test]
fn both_card_conventions_send_the_same_wire_amounts() {
    // Issuer A: charges negative (OFX convention); issuer B: charges positive.
    let a = [dec("-45.00"), dec("120.00")];
    let b = [dec("45.00"), dec("-120.00")];
    let wire_a: Vec<_> = a.iter().map(|x| to_feed_amount(*x, false)).collect();
    let wire_b: Vec<_> = b.iter().map(|x| to_feed_amount(*x, true)).collect();
    assert_eq!(wire_a, wire_b);
    assert_eq!(
        wire_a[0],
        dec("45.00"),
        "a charge is money out: positive on the wire"
    );
}

#[test]
fn card_check() {
    let pos = section(
        AccountKind::CreditCard,
        vec![
            row(1, "45", None, None),
            row(2, "12", None, None),
            row(3, "-100", None, None),
        ],
    );
    assert!(pos.sign.suggest_flip);
    assert_eq!(pos.sign.reason, Some(SignReason::CardChargesPositive));
    let neg = section(
        AccountKind::CreditCard,
        vec![row(1, "-45", None, None), row(2, "100", None, None)],
    );
    assert!(!neg.sign.suggest_flip);
}

#[test]
fn bank_debit_types_rule() {
    let s = section(
        AccountKind::Checking,
        vec![
            row(1, "45", None, Some("DEBIT")),
            row(2, "12", None, Some("POS")),
            row(3, "9", None, Some("CHECK")),
            row(4, "8", None, Some("FEE")),
            row(5, "-1", None, Some("ATM")),
            row(6, "500", None, Some("CREDIT")),
        ],
    );
    assert!(s.sign.suggest_flip, "4 of 5 debit types positive is 80%");
    let s = section(
        AccountKind::Checking,
        vec![
            row(1, "45", None, Some("DEBIT")),
            row(2, "12", None, Some("POS")),
            row(3, "-9", None, Some("CHECK")),
            row(4, "-8", None, Some("FEE")),
        ],
    );
    assert!(!s.sign.suggest_flip);
}

#[test]
fn continuity_settles_order_and_sign() {
    // Oldest first, normal sign: 100 → 55 → 155.
    let s = section(
        AccountKind::Unknown,
        vec![
            row(1, "-10", Some("100"), None),
            row(2, "-45", Some("55"), None),
            row(3, "100", Some("155"), None),
        ],
    );
    assert_eq!(
        s.sign.continuity,
        Continuity::Continuous {
            order: RowOrder::OldestFirst,
            flipped: false
        }
    );
    assert_eq!(s.ledger_balance.map(|b| b.amount), Some(dec("155")));
    // Newest first.
    let s = section(
        AccountKind::Unknown,
        vec![
            row(3, "100", Some("155"), None),
            row(2, "-45", Some("55"), None),
            row(1, "-10", Some("100"), None),
        ],
    );
    assert_eq!(
        s.sign.continuity,
        Continuity::Continuous {
            order: RowOrder::NewestFirst,
            flipped: false
        }
    );
    assert_eq!(
        s.ledger_balance.and_then(|b| b.date),
        CalendarDate::from_ymd(2026, 11, 3).ok()
    );
    // Flipped sign (money out positive): the check detects it.
    let s = section(
        AccountKind::Unknown,
        vec![
            row(1, "10", Some("100"), None),
            row(2, "45", Some("55"), None),
            row(3, "-100", Some("155"), None),
        ],
    );
    assert_eq!(
        s.sign.continuity,
        Continuity::Continuous {
            order: RowOrder::OldestFirst,
            flipped: true
        }
    );
    assert!(s.sign.suggest_flip);
    // Stored in the file's sign; the display sign is the bank's balance.
    let bal = s.ledger_balance.unwrap().amount;
    assert_eq!(to_display(bal, true), dec("155"));
    // A gap.
    let s = section(
        AccountKind::Unknown,
        vec![
            row(1, "-10", Some("100"), None),
            row(2, "-45", Some("55"), None),
            row(3, "100", Some("150"), None),
        ],
    );
    assert_eq!(s.sign.continuity, Continuity::Broken { line: 3 });
    assert_eq!(s.ledger_balance, None);
}

/// Review DB1: a card file whose balance column is the amount owed (it
/// grows with charges, which are positive) still asks to flip, though the
/// amounts follow the balance unflipped. The ending balance is kept.
#[test]
fn a_card_balance_column_never_hides_the_card_rule() {
    let rows = vec![
        row(1, "40", Some("40"), None),
        row(2, "25", Some("65"), None),
        row(3, "-15", Some("50"), None),
    ];
    let s = section(AccountKind::CreditCard, rows.clone());
    assert!(matches!(s.sign.continuity, Continuity::Continuous { .. }));
    assert!(s.sign.suggest_flip);
    assert_eq!(s.sign.reason, Some(SignReason::CardChargesPositive));
    assert_eq!(s.ledger_balance.map(|b| b.amount), Some(dec("50")));
    // The same rows read as a plain CSV, then checked as a card.
    let plain = section(AccountKind::Unknown, rows);
    assert!(!plain.sign.suggest_flip);
    let as_card = check_as(&plain, AccountKind::CreditCard);
    assert!(as_card.suggest_flip);
}

/// Equal amounts fit oldest-first and newest-first flipped alike: the
/// dates choose; with no date order the balance settles nothing.
#[test]
fn equal_amounts_are_decided_by_the_dates() {
    let s = section(
        AccountKind::Unknown,
        vec![
            row(1, "-10", Some("100"), None),
            row(2, "-10", Some("90"), None),
            row(3, "-10", Some("80"), None),
        ],
    );
    assert_eq!(
        s.sign.continuity,
        Continuity::Continuous {
            order: RowOrder::OldestFirst,
            flipped: false
        }
    );
    assert!(!s.sign.suggest_flip);
    let mut same_day = vec![
        row(1, "-10", Some("100"), None),
        row(2, "-10", Some("90"), None),
    ];
    for r in &mut same_day {
        r.date = CalendarDate::from_ymd(2026, 11, 5).unwrap();
    }
    let s = section(AccountKind::Unknown, same_day);
    assert_eq!(s.sign.continuity, Continuity::NotAvailable);
    assert!(!s.sign.suggest_flip);
}
