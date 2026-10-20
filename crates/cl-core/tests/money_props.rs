//! Property tests P1–P11 for the exact money types (P05 plan §7, AC2).

use cl_core::money::{
    AllocUnit, Money, MoneyError, MoneyInputError, MoneyStyle, NegativeStyle, Quantity, Rate,
    UserInput,
};
use proptest::prelude::*;
use rust_decimal::Decimal;
use std::str::FromStr;

const MAX_UNITS: i128 = 9_999_999_999_999_999_999;

/// Units across the whole range, with the edges and small values weighted in.
fn units() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => -MAX_UNITS..=MAX_UNITS,
        3 => -10_000_000i128..=10_000_000,
        1 => Just(0i128),
        1 => Just(MAX_UNITS),
        1 => Just(-MAX_UNITS),
        1 => Just(1i128),
        1 => Just(-1i128),
    ]
}

fn money() -> impl Strategy<Value = Money> {
    units().prop_map(|u| Money::from_ten_thousandths(u).unwrap())
}

/// Values small enough that sums of up to 64 of them cannot overflow.
fn small_money() -> impl Strategy<Value = Money> {
    (-1_000_000_000_000_000i128..=1_000_000_000_000_000)
        .prop_map(|u| Money::from_ten_thousandths(u).unwrap())
}

fn style() -> impl Strategy<Value = MoneyStyle> {
    (any::<bool>(), any::<bool>()).prop_map(|(parens, symbol)| MoneyStyle {
        negative: if parens {
            NegativeStyle::Parens
        } else {
            NegativeStyle::Minus
        },
        symbol,
    })
}

/// `-?\d+\.\d{4}` with no superfluous leading zero and never `-0.0000`.
fn is_canonical_wire(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let Some((int, frac)) = body.split_once('.') else {
        return false;
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    digits(int)
        && digits(frac)
        && frac.len() == 4
        && (int == "0" || !int.starts_with('0'))
        && s != "-0.0000"
}

fn wire_of_all(values: &[Money]) -> bool {
    values.iter().all(|v| {
        let w = v.to_wire();
        w != "-0.0000" && (w.starts_with('-') == v.is_negative())
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    /// P1: wire and JSON round trips; the canonical form is stable.
    #[test]
    fn p1_wire_round_trip(u in units()) {
        let m = Money::from_ten_thousandths(u).unwrap();
        let w = m.to_wire();
        prop_assert!(is_canonical_wire(&w), "{}", w);
        let back = Money::parse_wire(&w).unwrap();
        prop_assert_eq!(back, m);
        prop_assert_eq!(back.to_wire(), w.clone());
        prop_assert_eq!(m.to_ten_thousandths(), u);
        let json = serde_json::to_string(&m).unwrap();
        prop_assert_eq!(&json, &format!("\"{w}\""));
        prop_assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), m);
        prop_assert_eq!(Decimal::from_str(&w).unwrap(), m.as_decimal());
    }

    /// P1 (other types): Quantity and Rate round trip over their ranges.
    #[test]
    fn p1_quantity_and_rate_round_trip(q in -999_999_999_999i128..=999_999_999_999, r in -99_999i128..=99_999) {
        let q = Quantity::from_ten_thousandths(q).unwrap();
        prop_assert_eq!(Quantity::parse_wire(&q.to_wire()).unwrap(), q);
        let r = Rate::from_ten_thousandths(r).unwrap();
        prop_assert_eq!(Rate::parse_wire(&r.to_wire()).unwrap(), r);
    }

    /// P2: every string in the grammar is accepted with the value an
    /// independent parser (`rust_decimal`) reads.
    #[test]
    fn p2_grammar_accepts(s in "-?[0-9]{1,15}(\\.[0-9]{1,4})?") {
        let m = Money::parse_wire(&s).unwrap();
        prop_assert_eq!(m.as_decimal(), Decimal::from_str(&s).unwrap().normalize());
        prop_assert!(m.to_wire() != "-0.0000");
    }

    /// P2: arbitrary text over a hostile alphabet is accepted only when it is
    /// in the grammar, and then means what `rust_decimal` says.
    #[test]
    fn p2_grammar_fuzz(s in "[-+0-9.eE, $_]{0,22}") {
        let in_grammar = {
            let body = s.strip_prefix('-').unwrap_or(&s);
            let (int, frac) = match body.split_once('.') {
                Some((i, f)) => (i, Some(f)),
                None => (body, None),
            };
            let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
            digits(int) && int.len() <= 15 && frac.is_none_or(|f| digits(f) && f.len() <= 4)
        };
        match Money::parse_wire(&s) {
            Ok(m) => {
                prop_assert!(in_grammar, "accepted {:?}", s);
                prop_assert_eq!(m.as_decimal(), Decimal::from_str(&s).unwrap().normalize());
            }
            Err(_) => prop_assert!(!in_grammar, "rejected {:?}", s),
        }
    }

    /// P2: a valid string with one forbidden change is refused.
    #[test]
    fn p2_mutations_rejected(s in "[0-9]{1,12}\\.[0-9]{1,4}", d in 1u8..=9) {
        for bad in [
            format!("+{s}"),
            format!("{s}e1"),
            format!(" {s}"),
            format!("{s} "),
            format!("{s}{}", "0".repeat(5 - s.split_once('.').unwrap().1.len()) + &d.to_string()),
            s.replacen('.', ",", 1),
            format!("1,{s}"),
            format!("--{s}"),
        ] {
            prop_assert!(Money::parse_wire(&bad).is_err(), "accepted {:?}", bad);
        }
    }

    /// P3: owner text produced by `format` parses back to the same value.
    #[test]
    fn p3_format_parse_round_trip(m in money(), st in style()) {
        let text = m.format(st);
        prop_assert_eq!(Money::parse_user(&text, UserInput::FULL), Ok(m), "{}", text);
        if m.to_ten_thousandths() % 100 == 0 {
            prop_assert_eq!(Money::parse_user(&text, UserInput::CENTS), Ok(m), "{}", text);
        }
    }

    /// P4: no drift. Sums are exact, order does not matter, and
    /// `(a + b) - b == a`.
    #[test]
    fn p4_no_drift(
        (values, shuffled) in prop::collection::vec(small_money(), 0..64)
            .prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle())),
        a in small_money(),
        b in small_money(),
    ) {
        let total = Money::sum(values.iter().copied()).unwrap();
        let exact: i128 = values.iter().map(|v| v.to_ten_thousandths()).sum();
        prop_assert_eq!(total.to_ten_thousandths(), exact);
        prop_assert_eq!(Money::sum(shuffled).unwrap(), total);
        prop_assert_eq!(a.checked_add(b).unwrap().checked_sub(b).unwrap(), a);
        prop_assert_eq!(a.checked_add(b).unwrap(), b.checked_add(a).unwrap());
    }

    /// P5: allocation parts sum exactly and stay within one unit of the
    /// exact share.
    #[test]
    fn p5_allocation_is_exact(
        u in -1_000_000_000_000i128..=1_000_000_000_000,
        weights in prop::collection::vec(0u64..1000, 1..20),
        cents in any::<bool>(),
    ) {
        prop_assume!(weights.iter().any(|w| *w > 0));
        let (unit, step) = if cents { (AllocUnit::Cent, 100i128) } else { (AllocUnit::TenThousandth, 1) };
        let total = Money::from_ten_thousandths(u - u % step).unwrap();
        let parts = total.allocate(&weights, unit).unwrap();
        prop_assert_eq!(parts.len(), weights.len());
        prop_assert_eq!(Money::sum(parts.iter().copied()).unwrap(), total);
        prop_assert_eq!(total.allocate(&weights, unit).unwrap(), parts.clone());
        let wsum: i128 = weights.iter().map(|w| i128::from(*w)).sum();
        for (p, w) in parts.iter().zip(&weights) {
            prop_assert_eq!(p.to_ten_thousandths() % step, 0);
            // |part * W - total * w| < step * W  (within one unit)
            let diff = p.to_ten_thousandths() * wsum - total.to_ten_thousandths() * i128::from(*w);
            prop_assert!(diff.abs() < step * wsum);
            if *w == 0 {
                prop_assert!(p.is_zero());
            }
        }
        prop_assert!(wire_of_all(&parts));
    }

    /// P6: ambiguous or over-precise owner input is refused, never guessed.
    #[test]
    fn p6_ambiguous_input_rejected(
        a in "[1-9][0-9]{0,2}",
        b in "[0-9]{1,2}|[0-9]{4,5}",
        c in "[0-9]{3}",
        d in "[0-9]{2}",
        extra in "[0-9]{3,4}",
        many in "[0-9]{5,8}",
    ) {
        use MoneyInputError as E;
        let cents = |s: &str| Money::parse_user(s, UserInput::CENTS);
        prop_assert_eq!(cents(&format!("{a},{b}")), Err(E::AmbiguousSeparator));
        prop_assert_eq!(cents(&format!("{a}.{c},{d}")), Err(E::AmbiguousSeparator));
        prop_assert_eq!(cents(&format!("{a} {c}")), Err(E::AmbiguousSeparator));
        prop_assert_eq!(cents(&format!("{a}.{c}.{d}")), Err(E::AmbiguousSeparator));
        prop_assert_eq!(cents(&format!("{a}.{extra}")), Err(E::TooManyDecimals { max: 2 }));
        prop_assert_eq!(
            Money::parse_user(&format!("{a}.{many}"), UserInput::FULL),
            Err(E::TooManyDecimals { max: 4 })
        );
        prop_assert_eq!(cents(&format!("(-{a})")), Err(E::SignConflict));
    }

    /// P7: a JSON number is refused on every money type.
    #[test]
    fn p7_json_number_rejected(m in money()) {
        let w = m.to_wire();
        prop_assert!(serde_json::from_str::<Money>(&w).is_err());
        let int = m.to_ten_thousandths() / 10_000;
        prop_assert!(serde_json::from_str::<Money>(&int.to_string()).is_err());
        prop_assert!(serde_json::from_str::<Quantity>("1").is_err());
        prop_assert!(serde_json::from_str::<Rate>("0.05").is_err());
    }

    /// P8: leaving the range is an error, never a wrap or a clamp.
    #[test]
    fn p8_overflow_is_an_error(p in 1i128..=MAX_UNITS, a in money(), b in money()) {
        let p = Money::from_ten_thousandths(p).unwrap();
        prop_assert_eq!(Money::MAX.checked_add(p), Err(MoneyError::Overflow));
        prop_assert_eq!(Money::MIN.checked_sub(p), Err(MoneyError::Overflow));
        prop_assert_eq!(
            Money::from_ten_thousandths(MAX_UNITS + p.to_ten_thousandths()),
            Err(MoneyError::OutOfRange)
        );
        let exact = a.to_ten_thousandths() + b.to_ten_thousandths();
        match a.checked_add(b) {
            Ok(s) => prop_assert_eq!(s.to_ten_thousandths(), exact),
            Err(e) => {
                prop_assert_eq!(e, MoneyError::Overflow);
                prop_assert!(exact.abs() > MAX_UNITS);
            }
        }
    }

    /// P9: display never rounds; an independent parser reads back the value.
    #[test]
    fn p9_display_never_rounds(m in money(), st in style()) {
        let text = m.format(st);
        let neg = text.starts_with('-') || text.starts_with('(');
        let plain: String = text.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
        let mut d = Decimal::from_str(&plain).unwrap();
        if neg {
            d = -d;
        }
        prop_assert_eq!(d, m.as_decimal(), "{}", text);
        let decimals = plain.split_once('.').map_or(0, |(_, f)| f.len());
        prop_assert!((2..=4).contains(&decimals));
    }

    /// P10: zero is never negative on the wire, whatever produced it.
    #[test]
    fn p10_zero_never_negative(a in money(), b in small_money(), q in -100_000i128..=100_000) {
        prop_assert_eq!(a.checked_sub(a).unwrap().to_wire(), "0.0000");
        prop_assert_eq!(Money::ZERO.checked_neg(), Ok(Money::ZERO));
        let q = Quantity::from_ten_thousandths(q).unwrap();
        let mut results = vec![a.abs(), a.checked_neg().unwrap(), b.checked_sub(b).unwrap()];
        results.extend(b.mul_round(q, 2).ok());
        results.extend(b.mul_round(q, 4).ok());
        if let Ok(parts) = b.split(3, AllocUnit::TenThousandth) {
            results.extend(parts);
        }
        prop_assert!(wire_of_all(&results));
        let neg_zero = Decimal::from_parts(0, 0, 0, true, 3);
        prop_assert_eq!(Money::try_from(neg_zero).unwrap().to_wire(), "0.0000");
    }

    /// P11: `TryFrom<Decimal>` strips trailing zeros before the scale check
    /// and refuses a non-zero fifth decimal.
    #[test]
    fn p11_try_from_decimal(u in units(), k in 0u32..=9, d in 1i128..=9) {
        let m = Money::from_ten_thousandths(u).unwrap();
        let padded = Decimal::from_i128_with_scale(u * 10i128.pow(k), 4 + k);
        prop_assert_eq!(Money::try_from(padded), Ok(m));
        let sign = if u < 0 { -1 } else { 1 };
        let fifth = Decimal::from_i128_with_scale(u * 10 + sign * d, 5);
        prop_assert_eq!(Money::try_from(fifth), Err(MoneyError::TooPrecise));
    }

    /// Ordering agrees with the exact value.
    #[test]
    fn ordering_is_consistent(a in money(), b in money()) {
        prop_assert_eq!(a.cmp(&b), a.to_ten_thousandths().cmp(&b.to_ten_thousandths()));
        prop_assert_eq!(a.cmp(&b), a.as_decimal().cmp(&b.as_decimal()));
        prop_assert_eq!(a == b, a.to_wire() == b.to_wire());
    }
}

proptest! {
    /// Q-232: a typed money amount with any number of decimals rounds to the
    /// cent, half away from zero, exactly as `rust_decimal`'s
    /// `MidpointAwayFromZero` would (the calculator's rule, Q-P10x).
    #[test]
    fn typed_money_rounds_to_the_cent(
        int in 0u64..1_000_000_000_000,
        frac in "[0-9]{0,14}",
        negative in any::<bool>(),
        parens in any::<bool>(),
    ) {
        use rust_decimal::RoundingStrategy;
        let body = if frac.is_empty() { int.to_string() } else { format!("{int}.{frac}") };
        let text = match (negative, parens) {
            (false, _) => body.clone(),
            (true, false) => format!("-{body}"),
            (true, true) => format!("({body})"),
        };
        let got = Money::parse_user(&text, UserInput::MONEY).unwrap().as_decimal();
        let mut exact = Decimal::from_str(&body).unwrap();
        if negative {
            exact = -exact;
        }
        // Scale at most two, and never more than half a cent away.
        prop_assert!(got.scale() <= 2, "{} -> {}", text, got);
        prop_assert!((got - exact).abs() <= Decimal::new(5, 3), "{} -> {}", text, got);
        let want = exact.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
        prop_assert_eq!(got, want, "{}", text);
    }
}
