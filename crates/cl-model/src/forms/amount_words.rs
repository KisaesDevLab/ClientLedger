//! The amount in words on a check ("One thousand two hundred eighty-six
//! and 40/100"); the screen adds " dollars".
//!
//! Integer arithmetic on the exact ten-thousandth units of [`Money`]: whole
//! dollars and cents are split with `/` and `%`, never through a float
//! (non-negotiable 6).

use cl_core::Money;

/// The largest amount a check takes: 999,999,999,999.99 (twelve dollar
/// digits, the engine check limit).
pub const MAX_CHECK_UNITS: i128 = 9_999_999_999_999_900;

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

const SCALES: [&str; 4] = ["", "thousand", "million", "billion"];

/// Words for 1..=999, pushed onto `out`.
fn hundreds(n: u64, out: &mut Vec<String>) {
    let h = n / 100;
    let rest = n % 100;
    if h > 0 {
        out.push(format!("{} hundred", ONES[usize::try_from(h).unwrap_or(0)]));
    }
    if rest == 0 {
        return;
    }
    // `rest` < 100, so the indexes below are in range.
    let idx = |v: u64| usize::try_from(v).unwrap_or(0);
    if rest < 20 {
        out.push(ONES[idx(rest)].to_string());
    } else if rest.is_multiple_of(10) {
        out.push(TENS[idx(rest / 10)].to_string());
    } else {
        out.push(format!("{}-{}", TENS[idx(rest / 10)], ONES[idx(rest % 10)]));
    }
}

/// Words for whole dollars (`0` is "zero").
fn dollars_in_words(mut n: u64) -> String {
    if n == 0 {
        return ONES[0].to_string();
    }
    let mut groups = Vec::new();
    while n > 0 {
        groups.push(n % 1000);
        n /= 1000;
    }
    let mut out = Vec::new();
    for (i, g) in groups.iter().enumerate().rev() {
        if *g == 0 {
            continue;
        }
        hundreds(*g, &mut out);
        if let Some(scale) = SCALES.get(i).filter(|s| !s.is_empty()) {
            out.push((*scale).to_string());
        }
    }
    out.join(" ")
}

/// The written amount, e.g. "One thousand two hundred eighty-six and
/// 40/100". `None` for zero or less, for more than
/// [`MAX_CHECK_UNITS`], and for amounts with a third or fourth decimal
/// (a check is written in whole cents).
#[must_use]
pub fn amount_in_words(amount: Money) -> Option<String> {
    let units = amount.to_ten_thousandths();
    if units <= 0 || units > MAX_CHECK_UNITS || units % 100 != 0 {
        return None;
    }
    let dollars = u64::try_from(units / 10_000).ok()?;
    let cents = (units % 10_000) / 100;
    let words = dollars_in_words(dollars);
    let mut chars = words.chars();
    let first = chars.next()?.to_ascii_uppercase();
    Some(format!("{first}{} and {cents:02}/100", chars.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn words(s: &str) -> Option<String> {
        amount_in_words(Money::parse_wire(s).expect("money"))
    }

    #[test]
    fn table() {
        let cases = [
            ("0.01", "Zero and 01/100"),
            ("1.00", "One and 00/100"),
            ("19.99", "Nineteen and 99/100"),
            ("100", "One hundred and 00/100"),
            ("1286.40", "One thousand two hundred eighty-six and 40/100"),
            ("1000000.00", "One million and 00/100"),
            ("21.5", "Twenty-one and 50/100"),
            ("90", "Ninety and 00/100"),
            ("1001011", "One million one thousand eleven and 00/100"),
            (
                "999999999999.99",
                "Nine hundred ninety-nine billion nine hundred ninety-nine million \
                 nine hundred ninety-nine thousand nine hundred ninety-nine and 99/100",
            ),
        ];
        for (amount, want) in cases {
            assert_eq!(words(amount).as_deref(), Some(want), "{amount}");
        }
    }

    #[test]
    fn rejects() {
        assert_eq!(words("0"), None, "zero");
        assert_eq!(words("-5.00"), None, "negative");
        assert_eq!(words("1.2345"), None, "four decimals");
        assert_eq!(words("1.005"), None, "three decimals");
        assert_eq!(words("1000000000000.00"), None, "too large");
        assert_eq!(words("999999999999.999"), None);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn never_panics_and_cents_match(units in -20_000_000_000_000_000_i128..20_000_000_000_000_000_i128) {
            let Ok(m) = Money::from_ten_thousandths(units) else { return Ok(()); };
            match amount_in_words(m) {
                Some(w) => {
                    prop_assert!(units > 0 && units % 100 == 0 && units <= MAX_CHECK_UNITS);
                    let cents = (units / 100) % 100;
                    let suffix = format!(" and {cents:02}/100");
                    prop_assert!(w.ends_with(&suffix), "{}", w);
                    prop_assert!(w.chars().next().is_some_and(|c| c.is_ascii_uppercase()));
                }
                None => prop_assert!(units <= 0 || units % 100 != 0 || units > MAX_CHECK_UNITS),
            }
        }

        #[test]
        fn whole_cents_in_range_always_have_words(cents in 1_i64..99_999_999_999_999_i64) {
            let m = Money::from_cents(cents).expect("in range");
            prop_assert!(amount_in_words(m).is_some());
        }
    }
}
