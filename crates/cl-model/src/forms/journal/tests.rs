use proptest::prelude::*;

use super::*;
use crate::rowkey::RowKeyGen;

fn account(n: u64) -> AccountId {
    AccountId::parse(&format!("00000000-0000-0000-0000-{n:012}")).expect("id")
}

fn contact(n: u64) -> ContactId {
    ContactId::parse(&format!("00000000-0000-0000-0001-{n:012}")).expect("id")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, d).expect("date")
}

const AR: u64 = 1100;
const AP: u64 = 2000;

fn full_ctx() -> JournalContext {
    JournalContext {
        receivable: BTreeSet::from([account(AR)]),
        payable: BTreeSet::from([account(AP)]),
        names: BTreeMap::from([
            (
                contact(1),
                NameKind {
                    customer: true,
                    vendor: false,
                },
            ),
            (
                contact(2),
                NameKind {
                    customer: false,
                    vendor: true,
                },
            ),
        ]),
        names_supported: true,
        number_supported: true,
        adjusting_supported: true,
        reversing_supported: true,
    }
}

/// A form with `(account, debit, credit)` lines plus the blank entry line.
fn form(lines: &[(u64, &str, &str)]) -> JournalForm {
    let mut keys = RowKeyGen::new();
    let mut seeds = Vec::new();
    for (a, d, c) in lines {
        let mut l = FormLine::blank(keys.next_key());
        l.account = Some(account(*a));
        if !d.is_empty() {
            set_debit(&mut l, Some(money(d)));
        }
        if !c.is_empty() {
            set_credit(&mut l, Some(money(c)));
        }
        seeds.push(l);
    }
    JournalForm {
        date: Some(day(15)),
        lines: FormLines::from_lines(seeds, &mut keys),
        ..JournalForm::default()
    }
}

#[test]
fn typing_in_one_side_clears_the_other() {
    let mut l = FormLine::blank(RowKey(1));
    set_debit(&mut l, Some(money("1200.00")));
    assert_eq!(debit_of(&l), Some(money("1200.00")));
    assert_eq!(credit_of(&l), None);
    set_credit(&mut l, Some(money("5.00")));
    assert_eq!(debit_of(&l), None, "credit replaces the debit");
    assert_eq!(credit_of(&l), Some(money("5.00")));
    assert_eq!(l.amount, Some(money("-5.00")), "signed inside the module");
    clear_debit(&mut l);
    assert_eq!(
        credit_of(&l),
        Some(money("5.00")),
        "clearing debit keeps credit"
    );
    clear_credit(&mut l);
    assert_eq!(l.amount, None);
    set_debit(&mut l, Some(Money::ZERO));
    assert_eq!(l.amount, None, "zero is neither side");
    set_credit(&mut l, Some(money("-3")));
    assert_eq!(l.amount, None, "a negative entry is not a side");
}

#[test]
fn totals_and_difference_are_exact() {
    let f = form(&[(6100, "1200.00", ""), (1010, "", "1100.00")]);
    let t = totals(&f.lines).expect("totals");
    assert_eq!(t.debits, money("1200.00"));
    assert_eq!(t.credits, money("1100.00"));
    assert_eq!(t.difference, money("100.00"));
    assert!(!t.balanced());
    assert_eq!(
        balance_suggestion(&f.lines),
        Some(JeSide::Credit(money("100.00")))
    );
    let more_credit = form(&[(6100, "0.01", ""), (1010, "", "0.03")]);
    assert_eq!(
        balance_suggestion(&more_credit.lines),
        Some(JeSide::Debit(money("0.02")))
    );
    let even = form(&[(6100, "10", ""), (1010, "", "10")]);
    assert_eq!(balance_suggestion(&even.lines), None);
    assert!(!format!("{t:?}").contains("1200"), "redacted");
}

#[test]
fn totals_overflow_is_a_problem_not_a_rounded_figure() {
    let mut f = form(&[(1, "1", ""), (2, "1", ""), (3, "", "1")]);
    f.lines.lines[0].amount = Some(Money::MAX);
    f.lines.lines[1].amount = Some(Money::MAX);
    assert_eq!(totals(&f.lines), None);
    assert_eq!(balance_suggestion(&f.lines), None);
    let e = check(&f, &full_ctx()).expect_err("too large");
    assert!(e.contains(&JournalProblem::TooLarge), "{e:?}");
    assert_eq!(save_blocker(&f), Some(JournalProblem::TooLarge));
}

#[test]
fn apply_balance_fills_the_entry_line() {
    let mut f = form(&[(6100, "1200.00", ""), (1010, "", "1100.00")]);
    let blank = f.lines.lines[2].key;
    assert!(apply_balance(&mut f.lines, blank));
    assert_eq!(credit_of(&f.lines.lines[2]), Some(money("100.00")));
    assert!(totals(&f.lines).expect("t").balanced());
    // Re-balancing the same line ignores its own amount.
    f.lines.lines[0].amount = Some(money("1300.00"));
    assert!(apply_balance(&mut f.lines, blank));
    assert_eq!(credit_of(&f.lines.lines[2]), Some(money("200.00")));
    assert!(apply_balance(&mut f.lines, blank), "same amount again");
    assert_eq!(credit_of(&f.lines.lines[2]), Some(money("200.00")));
    // The others balance on their own: nothing to put on this line.
    let mut even = form(&[(6100, "10", ""), (1010, "", "10"), (6200, "3", "")]);
    let third = even.lines.lines[2].key;
    assert!(!apply_balance(&mut even.lines, third));
    assert_eq!(
        debit_of(&even.lines.lines[2]),
        Some(money("3")),
        "unchanged"
    );
    assert!(!apply_balance(&mut f.lines, RowKey(999)));
}

#[test]
fn unbalanced_entries_cannot_check() {
    let f = form(&[(6100, "1200.00", ""), (1010, "", "1100.00")]);
    let e = check(&f, &full_ctx()).expect_err("unbalanced");
    assert_eq!(
        e,
        vec![JournalProblem::Unbalanced {
            difference: money("100.00")
        }]
    );
    assert_eq!(
        save_blocker(&f),
        Some(JournalProblem::Unbalanced {
            difference: money("100.00")
        })
    );
    let ok = form(&[(6100, "1200.00", ""), (1010, "", "1200.00")]);
    let c = check(&ok, &full_ctx()).expect("balanced");
    assert_eq!(c.lines.len(), 2);
    assert_eq!(c.lines[0].side, JeSide::Debit(money("1200.00")));
    assert_eq!(c.lines[1].side, JeSide::Credit(money("1200.00")));
    assert_eq!(save_blocker(&ok), None);
}

#[test]
fn every_header_problem_is_reported() {
    let mut f = form(&[(6100, "5", ""), (1010, "", "5")]);
    f.date = None;
    f.number = "1".repeat(21);
    f.reverse_on = Some(day(15));
    let e = check(&f, &full_ctx()).expect_err("problems");
    assert_eq!(
        e,
        vec![JournalProblem::DateMissing, JournalProblem::NumberTooLong]
    );
    f.date = Some(day(15));
    f.number = format!("  {}  ", "1".repeat(20));
    let e = check(&f, &full_ctx()).expect_err("reverse");
    assert_eq!(e, vec![JournalProblem::ReverseNotAfter]);
    f.reverse_on = Some(day(14));
    assert_eq!(
        check(&f, &full_ctx()).expect_err("before"),
        vec![JournalProblem::ReverseNotAfter]
    );
    f.reverse_on = Some(day(16));
    f.memo = "  accrual ".into();
    f.adjusting = true;
    let c = check(&f, &full_ctx()).expect("ok");
    assert_eq!(c.number.as_deref(), Some("11111111111111111111"));
    assert_eq!(c.memo.as_deref(), Some("accrual"));
    assert!(c.adjusting);
    assert_eq!(c.reverse_on, Some(day(16)));
    // Each P15 field without its capability needs an engine update.
    let old = JournalContext {
        names_supported: false,
        number_supported: false,
        adjusting_supported: false,
        reversing_supported: false,
        ..full_ctx()
    };
    assert_eq!(
        check(&f, &old).expect_err("old engine"),
        vec![
            JournalProblem::NumberNeedsUpdate,
            JournalProblem::AdjustingNeedsUpdate,
            JournalProblem::ReversingNeedsUpdate
        ]
    );
}

#[test]
fn line_problems_name_the_line() {
    let mut f = form(&[(6100, "5", ""), (1010, "", "5")]);
    let k0 = f.lines.lines[0].key;
    f.lines.lines[0].account = None;
    let e = check(&f, &full_ctx()).expect_err("no account");
    assert_eq!(e, vec![JournalProblem::LineAccountMissing(k0)]);
    assert_eq!(e[0].field(), JournalField::LineAccount(k0));
    let mut g = form(&[(6100, "", ""), (1010, "", "5")]);
    g.lines.lines[0].memo = "x".into();
    let k = g.lines.lines[0].key;
    let e = check(&g, &full_ctx()).expect_err("no amount");
    assert!(e.contains(&JournalProblem::LineAmountMissing(k)), "{e:?}");
    let one = form(&[(6100, "5", "")]);
    let e = check(&one, &full_ctx()).expect_err("one line");
    assert!(e.contains(&JournalProblem::TooFewLines), "{e:?}");
    assert_eq!(save_blocker(&one), Some(JournalProblem::TooFewLines));
    let huge = form(&[(6100, "1000000000000", ""), (1010, "", "1000000000000")]);
    let e = check(&huge, &full_ctx()).expect_err("too large");
    assert_eq!(e.len(), 2, "{e:?}");
    assert!(matches!(e[0], JournalProblem::LineAmountTooLarge(_)));
}

#[test]
#[allow(clippy::many_single_char_names, reason = "short names per case")]
fn ar_ap_rule_q206() {
    let ctx = full_ctx();
    // A/R without a name.
    let mut f = form(&[(AR, "50", ""), (4000, "", "50")]);
    let k = f.lines.lines[0].key;
    assert_eq!(
        check(&f, &ctx).expect_err("name"),
        vec![JournalProblem::ArApNameMissing(k)]
    );
    // A/R with a vendor: needs a customer.
    f.lines.lines[0].customer = Some(contact(2));
    assert_eq!(
        check(&f, &ctx).expect_err("kind"),
        vec![JournalProblem::ArNeedsCustomer(k)]
    );
    f.lines.lines[0].customer = Some(contact(1));
    let c = check(&f, &ctx).expect("customer");
    assert_eq!(c.lines[0].contact, Some(contact(1)));
    // A/P with a customer: needs a vendor; an unknown name is not checked.
    let mut p = form(&[(6100, "50", ""), (AP, "", "50")]);
    let kp = p.lines.lines[1].key;
    p.lines.lines[1].customer = Some(contact(1));
    assert_eq!(
        check(&p, &ctx).expect_err("kind"),
        vec![JournalProblem::ApNeedsVendor(kp)]
    );
    p.lines.lines[1].customer = Some(contact(77));
    assert!(check(&p, &ctx).is_ok());
    // Any number of named A/R and A/P lines (Q-206, owner 2026-10-25).
    let mut two = form(&[(AR, "50", ""), (AP, "", "50")]);
    two.lines.lines[0].customer = Some(contact(1));
    two.lines.lines[1].customer = Some(contact(2));
    assert!(check(&two, &ctx).is_ok());
    let mut many = form(&[(AR, "30", ""), (AR, "20", ""), (4000, "", "50")]);
    many.lines.lines[0].customer = Some(contact(1));
    many.lines.lines[1].customer = Some(contact(1));
    let c = check(&many, &ctx).expect("two named A/R lines");
    assert_eq!(c.lines.len(), 3);
    // ... but every one of them still needs a name.
    many.lines.lines[1].customer = None;
    let k2 = many.lines.lines[1].key;
    assert_eq!(
        check(&many, &ctx).expect_err("second unnamed"),
        vec![JournalProblem::ArApNameMissing(k2)]
    );
    // Without je-names-v1: A/R and A/P lines and names need an update.
    let old = JournalContext {
        names_supported: false,
        ..full_ctx()
    };
    let mut n = form(&[(AR, "50", ""), (4000, "", "50")]);
    n.lines.lines[1].customer = Some(contact(1));
    let k1 = n.lines.lines[1].key;
    assert_eq!(
        check(&n, &old).expect_err("old"),
        vec![
            JournalProblem::ArApNeedsUpdate(k),
            JournalProblem::NameNeedsUpdate(k1)
        ]
    );
}

#[test]
fn problem_keys_and_names_are_stable() {
    let all = [
        JournalProblem::DateMissing,
        JournalProblem::NumberTooLong,
        JournalProblem::NumberNeedsUpdate,
        JournalProblem::AdjustingNeedsUpdate,
        JournalProblem::ReversingNeedsUpdate,
        JournalProblem::ReverseNotAfter,
        JournalProblem::TooFewLines,
        JournalProblem::TooManyLines,
        JournalProblem::LineAccountMissing(RowKey(1)),
        JournalProblem::LineAmountMissing(RowKey(1)),
        JournalProblem::LineAmountTooLarge(RowKey(1)),
        JournalProblem::TooLarge,
        JournalProblem::Unbalanced {
            difference: Money::ZERO,
        },
        JournalProblem::ArApNameMissing(RowKey(1)),
        JournalProblem::ArNeedsCustomer(RowKey(1)),
        JournalProblem::ApNeedsVendor(RowKey(1)),
        JournalProblem::ArApTwoLines,
        JournalProblem::ArApNeedsUpdate(RowKey(1)),
        JournalProblem::NameNeedsUpdate(RowKey(1)),
    ];
    let keys: BTreeSet<&str> = all.iter().map(|p| p.key().0).collect();
    assert_eq!(keys.len(), all.len());
    for p in all {
        assert_eq!(p.key().0, format!("journal.problem.{}", p.name()));
        let _ = p.field();
    }
}

#[test]
fn debug_hides_owner_data() {
    let mut f = form(&[(6100, "1286.40", ""), (1010, "", "1286.40")]);
    f.memo = "secret memo".into();
    f.number = "9876".into();
    let c = check(&f, &full_ctx()).expect("ok");
    let s = format!("{f:?} {c:?} {:?}", c.lines);
    assert!(
        !s.contains("1286") && !s.contains("secret") && !s.contains("9876"),
        "{s}"
    );
}

#[test]
fn saved_lines_load_back_signed() {
    let l = line_from_side(
        RowKey(4),
        account(1010),
        JeSide::Credit(money("12.34")),
        Some("m"),
        Some(contact(2)),
        None,
    );
    assert_eq!(credit_of(&l), Some(money("12.34")));
    assert_eq!(l.memo, "m");
    assert_eq!(l.customer, Some(contact(2)));
}

/// Up to 12 lines: `(debit?, cents)`, amounts 0.00..=99,999.99 (zero is
/// "neither side").
fn arb_lines() -> impl Strategy<Value = Vec<(bool, i64)>> {
    prop::collection::vec((any::<bool>(), 0_i64..10_000_000), 0..12)
}

fn form_of(lines: &[(bool, i64)]) -> JournalForm {
    let mut keys = RowKeyGen::new();
    let seeds = lines
        .iter()
        .enumerate()
        .map(|(i, (debit, cents))| {
            let mut l = FormLine::blank(keys.next_key());
            l.account = Some(account(u64::try_from(i).unwrap_or(0) + 5000));
            let m = Money::from_cents(*cents).expect("cents");
            if *debit {
                set_debit(&mut l, Some(m));
            } else {
                set_credit(&mut l, Some(m));
            }
            // A zero amount leaves the line blank but for its account, so
            // it is still a used line with no side.
            l
        })
        .collect();
    JournalForm {
        date: Some(day(15)),
        lines: FormLines::from_lines(seeds, &mut keys),
        ..JournalForm::default()
    }
}

fn sums(lines: &[(bool, i64)]) -> (i64, i64) {
    lines.iter().fold((0, 0), |(d, c), (debit, cents)| {
        if *debit {
            (d + cents, c)
        } else {
            (d, c + cents)
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    /// Acceptance 1: check() is Ok ⇔ Σd = Σc ∧ ≥ 2 lines ∧ each line one
    /// positive side.
    #[test]
    fn unbalanced_never_checks(lines in arb_lines()) {
        let f = form_of(&lines);
        let (d, c) = sums(&lines);
        let each_positive = lines.iter().all(|(_, cents)| *cents > 0);
        let expect_ok = d == c && lines.len() >= 2 && each_positive;
        let got = check(&f, &full_ctx());
        prop_assert_eq!(got.is_ok(), expect_ok, "{:?}", got);
        if let Ok(j) = &got {
            let debits = Money::sum(j.lines.iter().filter_map(|l| match l.side {
                JeSide::Debit(m) => Some(m),
                JeSide::Credit(_) => None,
            })).expect("sum");
            let credits = Money::sum(j.lines.iter().filter_map(|l| match l.side {
                JeSide::Credit(m) => Some(m),
                JeSide::Debit(_) => None,
            })).expect("sum");
            prop_assert_eq!(debits, credits);
            prop_assert!(j.lines.iter().all(|l| l.side.amount().is_positive()));
        }
    }

    /// Σd ≠ Σc ⇒ Unbalanced with difference = Σd − Σc, exactly.
    #[test]
    fn unbalanced_reports_the_exact_difference(lines in arb_lines()) {
        let (d, c) = sums(&lines);
        prop_assume!(d != c);
        let f = form_of(&lines);
        let want = Money::from_cents(d - c).expect("cents");
        let e = check(&f, &full_ctx()).expect_err("unbalanced");
        prop_assert!(e.contains(&JournalProblem::Unbalanced { difference: want }), "{:?}", e);
    }

    /// Applying the balance suggestion on the entry line always balances.
    #[test]
    fn balance_suggestion_always_balances(lines in arb_lines()) {
        let mut f = form_of(&lines);
        let entry = f.lines.lines.last().map(|l| l.key).expect("entry line");
        let before = balance_suggestion(&f.lines);
        let applied = apply_balance(&mut f.lines, entry);
        prop_assert_eq!(applied, before.is_some());
        prop_assert!(totals(&f.lines).expect("totals").balanced());
    }

    /// signed ↔ side round trip is lossless.
    #[test]
    fn signed_side_round_trip(units in 1_i128..=9_999_999_999_999_900, debit in any::<bool>()) {
        let m = Money::from_ten_thousandths(units).expect("money");
        let side = if debit { JeSide::Debit(m) } else { JeSide::Credit(m) };
        prop_assert_eq!(side_of(side.signed()), Some(side));
    }
}
