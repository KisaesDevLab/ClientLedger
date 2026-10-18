//! Property tests for exact amount entry and the calculator (P10 AC3, plan
//! §6): every accepted value equals the exact integer result, inexact results
//! are refused with the true neighbours, zero has no sign, nothing panics,
//! and errors never echo the typed digits.

// Ok/Err arms side by side read better than `if let … else` in these properties.
#![allow(clippy::single_match_else)]

use cl_core::{Money, MoneyStyle, NegativeStyle, Quantity, UserInput};
use cl_model::amount_entry::{
    Amount, AmountEntryError, AmountKind, AmountRules, Preview, Sign, parse, preview,
};
use proptest::prelude::*;

const MAX_UNITS: i128 = 9_999_999_999_999_999_999;
const MAX_CENTS: i64 = 99_999_999_999_999_999;

const MINUS: MoneyStyle = MoneyStyle {
    negative: NegativeStyle::Minus,
    symbol: false,
};
const PARENS: MoneyStyle = MoneyStyle {
    negative: NegativeStyle::Parens,
    symbol: true,
};

fn full() -> AmountRules {
    AmountRules {
        input: UserInput::FULL,
        ..AmountRules::MONEY
    }
}

fn cents(c: i64) -> Money {
    Money::from_cents(c).unwrap()
}

fn units(r: &Result<cl_model::amount_entry::Parsed, AmountEntryError>) -> i128 {
    r.as_ref().unwrap().value.to_ten_thousandths()
}

fn style() -> impl Strategy<Value = MoneyStyle> {
    prop_oneof![Just(MINUS), Just(PARENS)]
}

/// Text that reaches every branch of the tokenizer.
fn calculator_text() -> impl Strategy<Value = String> {
    "[0-9.,$()+*/xX\u{d7}\u{f7} \u{2212}\u{a0}-]{0,40}"
}

/// Every digit run in owner-facing error text is a decimal-place limit
/// (2 or 4), never something the owner typed.
fn no_typed_digits(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|run| !run.is_empty())
        .all(|run| run == "2" || run == "4")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn plain_round_trip_in_cents(c in -MAX_CENTS..=MAX_CENTS, st in style()) {
        let m = cents(c);
        let p = parse(&m.format(st), AmountRules::MONEY).unwrap();
        prop_assert_eq!(p.value, Amount::Money(m));
        prop_assert!(!p.calculated);
    }

    #[test]
    fn plain_round_trip_in_ten_thousandths(u in -MAX_UNITS..=MAX_UNITS, st in style()) {
        let m = Money::from_ten_thousandths(u).unwrap();
        prop_assert_eq!(parse(&m.format(st), full()).unwrap().value, Amount::Money(m));
    }

    #[test]
    fn addition_is_checked_add(a in -MAX_CENTS..=MAX_CENTS, b in -MAX_CENTS..=MAX_CENTS, st in style()) {
        let (ma, mb) = (cents(a), cents(b));
        let r = parse(&format!("{}+{}", ma.format(st), mb.format(st)), AmountRules::MONEY);
        match ma.checked_add(mb) {
            Ok(v) => {
                prop_assert_eq!(r.as_ref().unwrap().value, Amount::Money(v));
                prop_assert!(r.unwrap().calculated);
            }
            Err(_) => prop_assert_eq!(r, Err(AmountEntryError::TooLarge)),
        }
    }

    #[test]
    fn subtraction_is_checked_sub(a in -MAX_CENTS..=MAX_CENTS, b in -MAX_CENTS..=MAX_CENTS, st in style()) {
        let (ma, mb) = (cents(a), cents(b));
        let r = parse(&format!("{} - {}", ma.format(st), mb.format(st)), AmountRules::MONEY);
        match ma.checked_sub(mb) {
            Ok(v) => prop_assert_eq!(r.unwrap().value, Amount::Money(v)),
            Err(_) => prop_assert_eq!(r, Err(AmountEntryError::TooLarge)),
        }
    }

    #[test]
    fn multiplication_by_a_whole_number(a in -MAX_CENTS..=MAX_CENTS, n in -1_000i64..=1_000) {
        let ma = cents(a);
        let r = parse(&format!("{}*{n}", ma.format(MINUS)), AmountRules::MONEY);
        let want = ma
            .to_ten_thousandths()
            .checked_mul(i128::from(n))
            .and_then(|u| Money::from_ten_thousandths(u).ok());
        match want {
            Some(v) => prop_assert_eq!(r.unwrap().value, Amount::Money(v)),
            None => prop_assert_eq!(r, Err(AmountEntryError::TooLarge)),
        }
    }

    #[test]
    fn division_is_exact_or_refused_with_true_neighbours(
        a in -MAX_CENTS..=MAX_CENTS,
        n in prop_oneof![1i64..=1_000, -1_000i64..=-1],
    ) {
        let au = cents(a).to_ten_thousandths();
        let n = i128::from(n);
        let r = parse(&format!("{}/{n}", cents(a).format(MINUS)), AmountRules::MONEY);
        let exact = au % n == 0 && (au / n) % 100 == 0;
        match r {
            Ok(p) => {
                prop_assert!(exact);
                prop_assert_eq!(p.value.to_ten_thousandths() * n, au);
            }
            Err(AmountEntryError::NotExact(nb)) => {
                prop_assert!(!exact);
                let (lo, hi) = (nb.below().to_ten_thousandths(), nb.above().to_ten_thousandths());
                prop_assert_eq!(hi - lo, 100, "neighbours are one cent apart");
                prop_assert_eq!(lo % 100, 0);
                // lo <= a/n <= hi, compared without dividing.
                let (lo_n, hi_n) = if n > 0 { (lo * n, hi * n) } else { (hi * n, lo * n) };
                prop_assert!(lo_n <= au && au <= hi_n, "{lo} {hi} {au} {n}");
                prop_assert_eq!(format!("{nb:?}"), "Neighbours(..)");
            }
            Err(e) => prop_assert!(false, "unexpected {e:?}"),
        }
    }

    #[test]
    fn multiplication_binds_tighter(
        a in -1_000_000_000i64..=1_000_000_000,
        b in -1_000_000_000i64..=1_000_000_000,
        c in -1_000i64..=1_000,
    ) {
        let text = format!("{}+{}*{c}", cents(a).format(MINUS), cents(b).format(MINUS));
        let want = i128::from(a) * 100 + i128::from(b) * 100 * i128::from(c);
        prop_assert_eq!(units(&parse(&text, AmountRules::MONEY)), want);
        let left = format!("{}*{c}+{}", cents(b).format(MINUS), cents(a).format(MINUS));
        prop_assert_eq!(units(&parse(&left, AmountRules::MONEY)), want);
    }

    #[test]
    fn zero_results_are_unsigned(a in -MAX_CENTS..=MAX_CENTS, st in style()) {
        let f = cents(a).format(st);
        let abs = cents(a.abs()).format(st);
        for text in [format!("{f}-{f}"), format!("0*{f}"), format!("{f}*0"), format!("-{abs}+{abs}")] {
            let p = parse(&text, AmountRules::MONEY).unwrap();
            prop_assert_eq!(p.value.as_money().unwrap().to_wire(), "0.0000", "{}", text);
        }
    }

    #[test]
    fn unicode_minus_and_nbsp_are_normalized(a in -MAX_CENTS..=MAX_CENTS, b in 0i64..=MAX_CENTS) {
        let fa = cents(a).format(MINUS);
        let fb = cents(b).format(MINUS);
        let plain = parse(&format!("{fa}-{fb}"), AmountRules::MONEY);
        let fancy = format!("\u{a0}{}\u{2212}{fb}\u{a0} ", fa.replace('-', "\u{2212}"));
        prop_assert_eq!(parse(&fancy, AmountRules::MONEY), plain);
    }

    #[test]
    fn non_negative_columns_refuse_negatives(a in -MAX_CENTS..=MAX_CENTS) {
        let rules = AmountRules { sign: Sign::NonNegative, ..AmountRules::MONEY };
        let r = parse(&cents(a).format(MINUS), rules);
        if a < 0 {
            prop_assert_eq!(r, Err(AmountEntryError::Negative));
        } else {
            prop_assert_eq!(r.unwrap().value, Amount::Money(cents(a)));
        }
    }

    #[test]
    fn quantities_multiply_exactly(q in -999_999_999_999_i64..=999_999_999_999, n in -50i64..=50) {
        let rules = AmountRules { kind: AmountKind::Quantity, input: UserInput::FULL, sign: Sign::Any };
        let qv = Quantity::from_ten_thousandths(i128::from(q)).unwrap();
        let r = parse(&format!("{}*{n}", qv.format(MINUS)), rules);
        match Quantity::from_ten_thousandths(i128::from(q) * i128::from(n)) {
            Ok(v) => prop_assert_eq!(r.unwrap().value, Amount::Quantity(v)),
            Err(_) => prop_assert_eq!(r, Err(AmountEntryError::TooLarge)),
        }
    }

    #[test]
    fn trailing_equals_and_sign_order(a in -MAX_CENTS..=MAX_CENTS, b in -MAX_CENTS..=MAX_CENTS) {
        let (fa, fb) = (cents(a).format(MINUS), cents(b).format(MINUS));
        let sum = format!("{fa}+{fb}");
        prop_assert_eq!(parse(&format!("{sum}="), AmountRules::MONEY), parse(&sum, AmountRules::MONEY));
        let abs = cents(a.abs()).format(MINUS);
        prop_assert_eq!(
            parse(&format!("$-{abs}"), AmountRules::MONEY),
            parse(&format!("-${abs}"), AmountRules::MONEY)
        );
    }

    #[test]
    fn arbitrary_text_never_panics(s in "\\PC{0,80}") {
        let _ = parse(&s, AmountRules::MONEY);
        let _ = parse(&s, full());
        let _ = preview(&s, AmountRules::MONEY);
    }

    #[test]
    fn calculator_text_never_panics_and_errors_hide_digits(s in calculator_text()) {
        for rules in [AmountRules::MONEY, full()] {
            match parse(&s, rules) {
                Ok(p) => {
                    // Accepted values honour the field's decimals.
                    let step = if rules.input == UserInput::CENTS { 100 } else { 1 };
                    prop_assert_eq!(p.value.to_ten_thousandths() % step, 0);
                }
                Err(e) => {
                    prop_assert!(no_typed_digits(&e.to_string()), "{}", e);
                    let debug = format!("{e:?}");
                    prop_assert!(!debug.contains(|c: char| c.is_ascii_digit()), "{}", debug);
                    prop_assert!(e.key().as_str().starts_with("grid."));
                }
            }
            if let Preview::Error(e) = preview(&s, rules) {
                let debug = format!("{e:?}");
                prop_assert!(!debug.contains(|c: char| c.is_ascii_digit()), "{}", debug);
            }
        }
    }
}

#[test]
fn max_plus_a_cent_is_too_large() {
    let max = Money::parse_wire("999999999999999.99").unwrap();
    let text = format!("{}+0.01", max.format(MINUS));
    assert_eq!(
        parse(&text, AmountRules::MONEY),
        Err(AmountEntryError::TooLarge)
    );
    assert_eq!(
        parse(&format!("-{}-0.01", max.format(MINUS)), AmountRules::MONEY),
        Err(AmountEntryError::TooLarge)
    );
}

#[test]
fn rejection_keys() {
    let table = [
        ("(5+3)*2", "grid.calc.no_grouping"),
        ("5/0", "grid.calc.divide_by_zero"),
        ("400+", "grid.calc.incomplete"),
        (
            "1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1+1",
            "grid.calc.too_long",
        ),
        ("100/3", "grid.calc.not_exact"),
        ("1 234", "grid.amount.ambiguous"),
        ("1.234", "grid.amount.too_many_decimals"),
        ("abc", "grid.amount.invalid"),
    ];
    for (text, key) in table {
        assert_eq!(
            parse(text, AmountRules::MONEY).unwrap_err().key().as_str(),
            key,
            "{text}"
        );
    }
}

#[test]
fn the_digit_check_itself_works() {
    assert!(no_typed_digits("too many decimal places (at most 2)"));
    assert!(!no_typed_digits("33.33"));
    assert!(!no_typed_digits("5"));
}
