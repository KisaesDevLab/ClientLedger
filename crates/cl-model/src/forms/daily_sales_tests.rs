//! The daily-sales twin, its problems, the layout rules and the
//! over/short properties (512 cases).

use proptest::prelude::*;

use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn acct(n: u8) -> Option<AccountId> {
    AccountId::parse(&format!("4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).ok()
}

fn lid(n: u8) -> DailyLineId {
    DailyLineId::parse(&format!("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
}

fn saved(n: u8, section: DailySection, label: &str, account: Option<AccountId>) -> LayoutLine {
    LayoutLine {
        id: Some(lid(n)),
        ..LayoutLine::new(section, label, account)
    }
}

/// The plan's "Plant sale counter".
fn counter() -> DailyLayout {
    DailyLayout {
        name: "Plant sale counter".into(),
        lines: vec![
            saved(1, DailySection::Sales, "Plant sales", acct(1)),
            saved(2, DailySection::Tax, "Sales tax collected", acct(2)),
            saved(3, DailySection::Payment, "Cash", acct(3)),
            saved(4, DailySection::Payment, "Cards", acct(3)),
            saved(5, DailySection::Payout, "Paid-outs", acct(4)),
            saved(6, DailySection::Sales, "Dept 2", None),
        ],
    }
}

fn day(values: &[(u8, &str)]) -> DailySalesForm {
    DailySalesForm {
        template: DailyTemplateId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").ok(),
        date: CalendarDate::from_ymd(2026, 11, 4).ok(),
        tag: None,
        notes: "  canary  ".into(),
        values: values.iter().map(|(n, a)| (lid(*n), Some(m(a)))).collect(),
    }
}

#[test]
fn the_plan_day_is_2_35_short_and_0_65_over() {
    let l = counter();
    let t = totals(
        &day(&[(1, "1000.00"), (2, "80.00"), (3, "500.00"), (4, "577.65")]),
        &l,
    )
    .expect("totals");
    assert_eq!(t.over_short, OverShort::Short(m("2.35")));
    assert_eq!(t.signed_over_short, m("-2.35"));
    assert_eq!((t.debits, t.credits), (m("1077.65"), m("1080")));
    assert_eq!(
        (t.sales, t.tax, t.payments),
        (m("1000"), m("80"), m("1077.65"))
    );
    let t = totals(
        &day(&[(1, "1000.00"), (2, "80.00"), (3, "503.00"), (4, "577.65")]),
        &l,
    )
    .expect("totals");
    assert_eq!(t.over_short, OverShort::Over(m("0.65")));
    let even = totals(&day(&[(1, "10"), (3, "10")]), &l).expect("totals");
    assert_eq!(even.over_short, OverShort::Even);
}

#[test]
fn the_twin_skips_what_the_engine_skips() {
    let mut l = counter();
    l.lines[4].active = false; // a removed paid-out line
    let mut f = day(&[(1, "100"), (3, "90"), (5, "10"), (6, "7")]);
    f.values.insert(lid(99), Some(m("1000"))); // a line not on the layout
    f.values.insert(lid(4), None); // blank
    f.values.insert(lid(2), Some(Money::ZERO));
    let t = totals(&f, &l).expect("totals");
    assert_eq!(
        t.signed_over_short,
        m("-10"),
        "inactive, unknown, blank and zero skipped"
    );
    assert_eq!(t.unmapped, vec![5], "Dept 2 has an amount and no account");
    assert_eq!(t.sales, m("100"), "an unmapped line adds to no total");
}

#[test]
fn check_refuses_and_passes() {
    let l = counter();
    let ok = check(
        &day(&[(1, "1000"), (2, "80"), (3, "500"), (4, "577.65")]),
        &l,
    )
    .expect("checked");
    assert_eq!(ok.expected_over_short, m("-2.35"));
    assert_eq!(ok.values.len(), 4);
    assert_eq!(ok.notes.as_deref(), Some("canary"));
    assert!(!format!("{ok:?} {:?}", day(&[])).contains("canary"));

    assert_eq!(
        check(&DailySalesForm::default(), &l).expect_err("empty"),
        vec![
            DailyProblem::TemplateMissing,
            DailyProblem::DateMissing,
            DailyProblem::NoAmounts
        ]
    );
    assert_eq!(
        check(&day(&[(1, "-1.00"), (3, "5")]), &l).expect_err("negative"),
        vec![DailyProblem::NegativeAmount { index: 0 }]
    );
    assert_eq!(
        check(&day(&[(6, "5"), (3, "5")]), &l).expect_err("unmapped"),
        vec![DailyProblem::UnmappedWithAmount { index: 5 }]
    );
    assert_eq!(
        check(&day(&[(1, "0")]), &l).expect_err("zero"),
        vec![DailyProblem::NoAmounts]
    );
    let mut long = day(&[(1, "1")]);
    long.notes = "x".repeat(NOTES_MAX + 1);
    assert_eq!(
        check(&long, &l).expect_err("notes"),
        vec![DailyProblem::NotesTooLong]
    );
    let big = DailyLayout {
        name: String::new(),
        lines: (0..=200u8)
            .map(|n| saved(n, DailySection::Payment, "Cash", acct(3)))
            .collect(),
    };
    let f = DailySalesForm {
        values: (0..=200u8).map(|n| (lid(n), Some(m("1")))).collect(),
        ..day(&[])
    };
    assert_eq!(
        check(&f, &big).expect_err("many"),
        vec![DailyProblem::TooManyValues]
    );
    let huge = day(&[(3, "999999999999999.99"), (4, "999999999999999.99")]);
    assert_eq!(
        check(&huge, &l).expect_err("huge"),
        vec![DailyProblem::TooLarge]
    );
}

#[test]
fn sides_are_fixed_by_section() {
    for s in DailySection::ALL {
        let want = match s {
            DailySection::Sales | DailySection::Tax | DailySection::Tips | DailySection::Other => {
                DailySide::Credit
            }
            _ => DailySide::Debit,
        };
        assert_eq!(side_for(s), want);
        assert_eq!(LayoutLine::new(s, "x", None).side, want);
    }
}

#[test]
fn layout_problems_and_the_tax_line() {
    assert_eq!(counter().problems(), Vec::new());
    let mut l = DailyLayout {
        name: "Blank".into(),
        lines: vec![LayoutLine::new(DailySection::Sales, " ", None)],
    };
    assert_eq!(
        l.problems(),
        vec![
            LayoutProblem::EmptyLabel { index: 0 },
            LayoutProblem::NoTender
        ]
    );
    l.lines[0].label = "x".repeat(LABEL_MAX + 1);
    l.lines[0].active = false;
    assert_eq!(
        l.problems(),
        vec![
            LayoutProblem::LabelTooLong { index: 0 },
            LayoutProblem::NoSalesLine,
            LayoutProblem::NoTender
        ]
    );
    l.lines = vec![LayoutLine::new(DailySection::Payment, "Cash", None); MAX_LAYOUT_LINES + 1];
    assert!(l.problems().contains(&LayoutProblem::TooManyLines));

    // Q-260: the manual tax line goes after the sales lines, once.
    let mut c = counter();
    assert!(!c.ensure_tax_line(acct(2)));
    c.lines.remove(1);
    assert!(c.ensure_tax_line(acct(2)));
    let (at, tax) = c.in_section(DailySection::Tax).next().expect("tax line");
    assert_eq!(at, 5, "after the last sales line (Dept 2)");
    assert_eq!(tax.label, TAX_LINE_LABEL);
    assert_eq!(tax.account, acct(2));
    assert_eq!(tax.side, DailySide::Credit);
    assert_eq!(tax.id, None);
    assert!(!c.ensure_tax_line(acct(2)));
    let mut empty = DailyLayout::default();
    assert!(empty.ensure_tax_line(None));
    assert_eq!(empty.in_section(DailySection::Tax).count(), 1);
}

fn cents() -> impl Strategy<Value = Option<Money>> {
    prop_oneof![
        1 => Just(None),
        1 => Just(Some(Money::ZERO)),
        6 => (0i64..100_000_000_000).prop_map(|c| Money::from_cents(c).ok()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn over_short_is_debits_minus_credits(
        vals in proptest::collection::vec(cents(), 6),
        active in proptest::collection::vec(any::<bool>(), 6),
    ) {
        let mut l = counter();
        for (line, a) in l.lines.iter_mut().zip(&active) {
            line.active = *a;
        }
        let f = DailySalesForm {
            values: vals.iter().enumerate().map(|(i, v)| (lid(u8::try_from(i + 1).expect("n")), *v)).collect(),
            ..day(&[])
        };
        let t = totals(&f, &l).expect("in range");
        // Σ debit-side − Σ credit-side over active, mapped lines.
        let mut want = Money::ZERO;
        for (line, v) in l.lines.iter().zip(&vals) {
            let Some(v) = v else { continue };
            if !line.active || line.account.is_none() {
                continue;
            }
            want = match line.side {
                DailySide::Debit => want.checked_add(*v),
                DailySide::Credit => want.checked_sub(*v),
            }.expect("range");
        }
        prop_assert_eq!(t.signed_over_short, want);
        // The sign.
        match t.over_short {
            OverShort::Over(x) => prop_assert!(x.is_positive() && x == want),
            OverShort::Short(x) => prop_assert!(x.is_positive() && x.checked_neg().ok() == Some(want)),
            OverShort::Even => prop_assert!(want.is_zero()),
        }
        // Once the engine adds its over/short line, the entry balances.
        let (dr, cr) = match t.over_short {
            OverShort::Short(x) => (t.debits.checked_add(x).expect("dr"), t.credits),
            OverShort::Over(x) => (t.debits, t.credits.checked_add(x).expect("cr")),
            OverShort::Even => (t.debits, t.credits),
        };
        prop_assert_eq!(dr, cr);
        // check() gates on the same figure.
        if let Ok(c) = check(&f, &l) {
            prop_assert_eq!(c.expected_over_short, want);
            prop_assert!(c.values.iter().all(|(_, v)| v.is_positive()));
        }
    }
}
