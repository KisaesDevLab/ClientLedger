#![allow(clippy::many_single_char_names)]

use proptest::prelude::*;

use super::*;
use crate::forms::lines::FormLine;
use crate::rowkey::RowKeyGen;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn uuid(n: u64) -> String {
    format!("00000000-0000-0000-0000-{n:012}")
}

fn account(n: u64) -> AccountId {
    AccountId::parse(&uuid(n)).expect("id")
}

fn txn(n: u64) -> TransactionId {
    TransactionId::parse(&uuid(n)).expect("id")
}

const BANK: u64 = 1010;
const CLEARING: u64 = 10150;
const CAPS: DepositContext<'static> = DepositContext {
    grouping: true,
    cashback: true,
    control_accounts: &[],
};

fn picked(n: u64, amount: &str) -> PickedPayment {
    PickedPayment {
        source: txn(n),
        amount: money(amount),
        contact: None,
    }
}

fn line(keys: &mut RowKeyGen, acct: u64, amount: &str) -> FormLine {
    FormLine {
        account: Some(account(acct)),
        amount: Some(money(amount)),
        ..FormLine::blank(keys.next_key())
    }
}

fn form(keys: &mut RowKeyGen) -> DepositForm {
    DepositForm {
        account: Some(account(BANK)),
        date: CalendarDate::from_ymd(2026, 11, 4).ok(),
        ..DepositForm::default()
    }
    .with_entry(keys)
}

impl DepositForm {
    fn with_entry(mut self, keys: &mut RowKeyGen) -> DepositForm {
        self.lines.ensure_entry(keys);
        self
    }
}

#[test]
fn the_story_groups_two_payments_into_one_deposit() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    assert!(f.toggle(picked(1, "1250.00")));
    assert!(f.toggle(picked(2, "842.50")));
    let t = f.totals().expect("totals");
    assert_eq!(t.payments, money("2092.50"));
    assert_eq!(t.total, money("2092.50"));
    let c = check(&f, &CAPS).expect("ok");
    assert_eq!(c.total, money("2092.50"));
    assert_eq!(c.payments.len(), 2);
    assert_eq!(
        bank_match(c.total, money("2092.50")),
        Some(BankMatch::Matches)
    );
    assert_eq!(
        bank_match(c.total, money("2100.00")),
        Some(BankMatch::Differs(money("7.50")))
    );
    // Unticking takes it out again; a stale pick is dropped.
    assert!(!f.toggle(picked(2, "842.50")));
    f.toggle(picked(2, "842.50"));
    assert_eq!(f.keep_only(&[txn(1)]), 1);
    assert_eq!(f.picked.len(), 1);
    let shown = format!("{f:?} {c:?}");
    assert!(
        !shown.contains("1250") && !shown.contains("2092"),
        "{shown}"
    );
}

#[test]
fn a_fee_line_becomes_a_deduction_and_cash_back_comes_off() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    f.lines.lines = vec![
        line(&mut keys, 4100, "300.00"),
        line(&mut keys, 6250, "-12.50"),
    ];
    f.lines.ensure_entry(&mut keys);
    f.cash_back = CashBackForm {
        account: Some(account(3100)),
        amount: Some(money("50.00")),
        memo: " Petty cash ".into(),
    };
    let c = check(&f, &CAPS).expect("ok");
    assert_eq!(c.total, money("237.50"));
    assert_eq!(c.lines.len(), 1);
    assert_eq!(c.deductions.len(), 1);
    assert_eq!(
        c.deductions[0].amount,
        money("12.50"),
        "sent as a positive fee"
    );
    let cb = c.cash_back.expect("cash back");
    assert_eq!(cb.memo.as_deref(), Some("Petty cash"));
    // 400.00 cash back leaves nothing: refused as the cash back's problem.
    f.cash_back.amount = Some(money("400.00"));
    assert_eq!(
        check(&f, &CAPS).err(),
        Some(vec![DepositProblem::CashBackTooLarge])
    );
    // Exactly the subtotal is still nothing to deposit.
    f.cash_back.amount = Some(money("287.50"));
    assert_eq!(
        check(&f, &CAPS).err(),
        Some(vec![DepositProblem::CashBackTooLarge])
    );
    f.cash_back.amount = Some(money("287.49"));
    assert_eq!(check(&f, &CAPS).map(|c| c.total).ok(), Some(money("0.01")));
}

#[test]
fn every_problem_is_reported() {
    let mut keys = RowKeyGen::new();
    let mut f = DepositForm::default().with_entry(&mut keys);
    let p = check(&f, &CAPS).expect_err("empty");
    assert_eq!(
        p,
        [
            DepositProblem::AccountMissing,
            DepositProblem::DateMissing,
            DepositProblem::NothingToDeposit
        ]
    );
    f = form(&mut keys);
    let a = line(&mut keys, BANK, "5.00");
    let b = FormLine {
        account: None,
        ..line(&mut keys, 1, "5.00")
    };
    let c = FormLine {
        amount: Some(Money::ZERO),
        ..line(&mut keys, 4000, "0")
    };
    let d = line(&mut keys, 6250, "-1.00");
    let keys_of = [a.key, b.key, c.key, d.key];
    f.lines.lines = vec![a, b, c, d];
    f.picked.push(picked(9, "10.00"));
    f.cash_back = CashBackForm {
        account: Some(account(CLEARING)),
        amount: None,
        memo: String::new(),
    };
    let none = DepositContext::default();
    let p = check(&f, &none).expect_err("old engine");
    assert!(p.contains(&DepositProblem::GroupingNeedsUpdate));
    assert!(p.contains(&DepositProblem::LineToDepositAccount(keys_of[0])));
    assert!(p.contains(&DepositProblem::LineAccountMissing(keys_of[1])));
    assert!(p.contains(&DepositProblem::LineAmountZero(keys_of[2])));
    assert!(p.contains(&DepositProblem::NegativeNeedsUpdate(keys_of[3])));
    assert!(p.contains(&DepositProblem::CashBackNeedsUpdate));
    let control = [account(CLEARING)];
    let ctx = DepositContext {
        control_accounts: &control,
        ..CAPS
    };
    let p = check(&f, &ctx).expect_err("cash back");
    assert!(p.contains(&DepositProblem::CashBackToControlAccount));
    assert!(p.contains(&DepositProblem::CashBackAmountMissing));
    f.cash_back.account = Some(account(BANK));
    assert!(
        check(&f, &ctx)
            .expect_err("own")
            .contains(&DepositProblem::CashBackToDepositAccount)
    );
    f.cash_back.account = None;
    f.cash_back.amount = Some(money("1"));
    assert!(
        check(&f, &ctx)
            .expect_err("no account")
            .contains(&DepositProblem::CashBackAccountMissing)
    );
    // Only fees: the total is not positive.
    let mut g = form(&mut keys);
    g.lines.lines = vec![line(&mut keys, 6250, "-3.00")];
    assert_eq!(
        check(&g, &CAPS).err(),
        Some(vec![DepositProblem::TotalNotPositive])
    );
    for p in [
        DepositProblem::CashBackTooLarge,
        DepositProblem::LineAmountZero(keys_of[0]),
    ] {
        assert!(p.key().0.starts_with("deposit.problem."));
        assert_ne!(p.name(), "");
    }
    assert_eq!(
        DepositProblem::LineAmountZero(keys_of[2]).field(),
        DepositField::LineAmount(keys_of[2])
    );
}

#[test]
fn too_many_parts_is_refused() {
    let mut keys = RowKeyGen::new();
    let mut f = form(&mut keys);
    for n in 0..=MAX_PARTS as u64 {
        f.picked.push(picked(n + 1, "1.00"));
    }
    assert!(
        check(&f, &CAPS)
            .expect_err("many")
            .contains(&DepositProblem::TooManyParts)
    );
}

#[test]
fn the_summary_shows_the_engines_total() {
    let s = DepositSummary {
        date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        account: "1010 Operating".into(),
        memo: String::new(),
        items: vec![SummaryRow {
            label: "Hollister".into(),
            detail: "5521".into(),
            check_no: "5521".into(),
            method: "Check".into(),
            amount: money("1250.00"),
        }],
        other: vec![SummaryRow {
            label: "Card fees".into(),
            detail: String::new(),
            check_no: String::new(),
            method: String::new(),
            amount: money("-12.50"),
        }],
        cash_back: Some(money("50.00")),
        total: money("1187.50"),
    };
    assert!(s.parts_agree());
    let off = DepositSummary {
        total: money("1187.51"),
        ..s.clone()
    };
    assert!(!off.parts_agree());
    assert!(!format!("{s:?}").contains("Hollister"));
}

fn arb_cents() -> impl Strategy<Value = i64> {
    1_i64..=5_000_000
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// `check()` passes iff the exact total is above zero (every part is
    /// valid by construction), and the checked parts add up to it:
    /// total == Σpicked + Σlines − cash back, and the deductions are the
    /// negative lines made positive.
    #[test]
    fn ok_iff_the_total_is_positive(
        picks in prop::collection::vec(arb_cents(), 0..6),
        lines in prop::collection::vec((arb_cents(), any::<bool>()), 0..6),
        cash in prop::option::of(arb_cents()),
    ) {
        let mut keys = RowKeyGen::new();
        let mut f = form(&mut keys);
        for (i, c) in picks.iter().enumerate() {
            f.picked.push(PickedPayment {
                source: txn(i as u64 + 1),
                amount: Money::from_cents(*c).expect("m"),
                contact: None,
            });
        }
        f.lines.lines = lines
            .iter()
            .map(|(c, neg)| FormLine {
                account: Some(account(4000)),
                amount: Some(Money::from_cents(if *neg { -c } else { *c }).expect("m")),
                ..FormLine::blank(keys.next_key())
            })
            .collect();
        f.lines.ensure_entry(&mut keys);
        if let Some(c) = cash {
            f.cash_back = CashBackForm {
                account: Some(account(3100)),
                amount: Some(Money::from_cents(c).expect("m")),
                memo: String::new(),
            };
        }
        let cents: i64 = picks.iter().sum::<i64>()
            + lines.iter().map(|(c, n)| if *n { -c } else { *c }).sum::<i64>()
            - cash.unwrap_or(0);
        let expected = Money::from_cents(cents).expect("m");
        let anything = !picks.is_empty() || !lines.is_empty();
        let r = check(&f, &CAPS);
        prop_assert_eq!(r.is_ok(), anything && cents > 0);
        if anything {
            prop_assert_eq!(f.totals().map(|t| t.total), Some(expected));
        }
        if let Ok(c) = r {
            prop_assert_eq!(c.total, expected);
            let neg: i64 = lines.iter().filter(|(_, n)| *n).map(|(c, _)| c).sum();
            let ded = Money::sum(c.deductions.iter().map(|l| l.amount)).expect("sum");
            prop_assert_eq!(ded, Money::from_cents(neg).expect("m"));
            prop_assert!(c.lines.iter().chain(&c.deductions).all(|l| l.amount.is_positive()));
            let back = Money::sum(c.payments.iter().map(|p| p.amount))
                .and_then(|s| s.checked_add(Money::sum(c.lines.iter().map(|l| l.amount))?))
                .and_then(|s| s.checked_sub(ded))
                .and_then(|s| s.checked_sub(c.cash_back.as_ref().map_or(Money::ZERO, |b| b.amount)))
                .expect("sum");
            prop_assert_eq!(back, c.total);
        }
    }
}
