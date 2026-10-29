//! Amount text for printed documents: figures with thousands separators and
//! the written amount on a check. Exact: `rust_decimal` in, strings out;
//! nothing is ever rounded — a value a check cannot show is refused.

use crate::PrintError;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive as _;

/// Largest check amount, 9,999,999,999.99: the most the amount box holds at
/// full size (16 characters of Plex Mono 10 pt in 36 mm; owner review S1).
pub const MAX_CHECK_DOLLARS: u64 = 9_999_999_999;

/// `1234.5` → `1,234.50`; `-3` → `-3.00`; `0.125` → `0.125`. At least two
/// decimals, more only when they are not zero (a value is never rounded).
#[must_use]
pub fn format_amount(v: Decimal) -> String {
    let n = v.normalize();
    let text = if n.scale() < 2 {
        format!("{n:.2}")
    } else {
        n.to_string()
    };
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// A quantity or rate: the value as given, trailing zeros removed, with
/// thousands separators (`2.50` → `2.5`, `1000` → `1,000`).
#[must_use]
pub fn format_quantity(v: Decimal) -> String {
    let n = v.normalize();
    let text = n.to_string();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.as_str()),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if frac.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{frac}")
    }
}

/// Dollars and cents of a check amount.
///
/// # Errors
/// [`PrintError::NegativeAmount`] below zero, [`PrintError::SubCent`] for a
/// fraction of a cent, [`PrintError::AmountTooLarge`] above
/// [`MAX_CHECK_DOLLARS`].
pub fn dollars_cents(v: Decimal) -> Result<(u64, u8), PrintError> {
    if v.is_sign_negative() && !v.is_zero() {
        return Err(PrintError::NegativeAmount);
    }
    let cents = v
        .checked_mul(Decimal::ONE_HUNDRED)
        .ok_or(PrintError::AmountTooLarge)?;
    if cents.fract() != Decimal::ZERO {
        return Err(PrintError::SubCent);
    }
    let cents = cents.to_u128().ok_or(PrintError::AmountTooLarge)?;
    let dollars = u64::try_from(cents / 100).map_err(|_| PrintError::AmountTooLarge)?;
    if dollars > MAX_CHECK_DOLLARS {
        return Err(PrintError::AmountTooLarge);
    }
    // cents % 100 < 100 always fits.
    let c = u8::try_from(cents % 100).unwrap_or(0);
    Ok((dollars, c))
}

/// The amount in figures as a check shows it: always two decimals
/// (`1,234.50`).
///
/// # Errors
/// As [`dollars_cents`].
pub fn check_figures(v: Decimal) -> Result<String, PrintError> {
    let (d, c) = dollars_cents(v)?;
    let mut s = format_amount(Decimal::from(d));
    // format_amount of a whole number ends in ".00"; replace the cents.
    s.truncate(s.len() - 2);
    Ok(format!("{s}{c:02}"))
}

/// The written amount in capitals (Q-P19u): `1234.56` → `ONE THOUSAND TWO
/// HUNDRED THIRTY-FOUR AND 56/100`. The check layout adds the asterisk
/// fill.
///
/// # Errors
/// As [`dollars_cents`].
pub fn amount_in_words(v: Decimal) -> Result<String, PrintError> {
    let (dollars, cents) = dollars_cents(v)?;
    let words = number_words(dollars).to_uppercase();
    Ok(format!("{words} AND {cents:02}/100"))
}

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
const SCALES: [(u64, &str); 3] = [
    (1_000_000_000, "billion"),
    (1_000_000, "million"),
    (1_000, "thousand"),
];

fn number_words(n: u64) -> String {
    if n == 0 {
        return ONES[0].to_string();
    }
    let mut parts = Vec::new();
    let mut rest = n;
    for (scale, name) in SCALES {
        if rest >= scale {
            parts.push(format!("{} {name}", below_thousand(rest / scale)));
            rest %= scale;
        }
    }
    if rest > 0 {
        parts.push(below_thousand(rest));
    }
    parts.join(" ")
}

/// 1..=999 (larger leading groups only reach 999 because the maximum check
/// is below a trillion).
fn below_thousand(n: u64) -> String {
    let idx = |v: u64| usize::try_from(v).unwrap_or(0);
    let mut parts = Vec::new();
    let h = n / 100;
    let r = n % 100;
    if h > 0 {
        parts.push(format!("{} hundred", ONES[idx(h).min(19)]));
    }
    if r >= 20 {
        let t = TENS[idx(r / 10)];
        if r.is_multiple_of(10) {
            parts.push(t.to_string());
        } else {
            parts.push(format!("{t}-{}", ONES[idx(r % 10)]));
        }
    } else if r > 0 {
        parts.push(ONES[idx(r)].to_string());
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).unwrap()
    }

    #[test]
    fn words_for_the_required_amounts() {
        assert_eq!(amount_in_words(d("0.00")).unwrap(), "ZERO AND 00/100");
        assert_eq!(amount_in_words(d("0.05")).unwrap(), "ZERO AND 05/100");
        assert_eq!(
            amount_in_words(d("1000000.00")).unwrap(),
            "ONE MILLION AND 00/100"
        );
        assert_eq!(
            amount_in_words(d("19999999.99")).unwrap(),
            "NINETEEN MILLION NINE HUNDRED NINETY-NINE THOUSAND NINE HUNDRED NINETY-NINE AND 99/100"
        );
        assert_eq!(
            amount_in_words(d("1234.56")).unwrap(),
            "ONE THOUSAND TWO HUNDRED THIRTY-FOUR AND 56/100"
        );
        assert_eq!(
            amount_in_words(d("1234.5600")).unwrap(),
            "ONE THOUSAND TWO HUNDRED THIRTY-FOUR AND 56/100"
        );
        assert_eq!(amount_in_words(d("40.1")).unwrap(), "FORTY AND 10/100");
        assert_eq!(
            amount_in_words(d("115")).unwrap(),
            "ONE HUNDRED FIFTEEN AND 00/100"
        );
        assert_eq!(
            amount_in_words(d("2000017001.01")).unwrap(),
            "TWO BILLION SEVENTEEN THOUSAND ONE AND 01/100"
        );
        assert_eq!(
            amount_in_words(d("9999999999.99")).unwrap(),
            "NINE BILLION NINE HUNDRED NINETY-NINE MILLION NINE HUNDRED NINETY-NINE THOUSAND NINE HUNDRED NINETY-NINE AND 99/100"
        );
        assert_eq!(
            check_figures(d("9999999999.99")).unwrap(),
            "9,999,999,999.99"
        );
    }

    #[test]
    fn negative_sub_cent_and_huge_amounts_are_refused() {
        assert_eq!(amount_in_words(d("-0.01")), Err(PrintError::NegativeAmount));
        assert_eq!(
            amount_in_words(d("-1234.56")),
            Err(PrintError::NegativeAmount)
        );
        assert_eq!(amount_in_words(d("1.005")), Err(PrintError::SubCent));
        assert_eq!(
            amount_in_words(d("10000000000.00")),
            Err(PrintError::AmountTooLarge)
        );
        assert_eq!(check_figures(d("-5")), Err(PrintError::NegativeAmount));
        // Negative zero is zero.
        assert_eq!(amount_in_words(d("-0.00")).unwrap(), "ZERO AND 00/100");
    }

    #[test]
    fn figures_always_show_cents() {
        assert_eq!(check_figures(d("1234.5")).unwrap(), "1,234.50");
        assert_eq!(check_figures(d("0")).unwrap(), "0.00");
        assert_eq!(check_figures(d("19999999.99")).unwrap(), "19,999,999.99");
        assert_eq!(check_figures(d("100.0000")).unwrap(), "100.00");
    }

    #[test]
    fn amounts_and_quantities_format_exactly() {
        assert_eq!(format_amount(d("1234567.891")), "1,234,567.891");
        assert_eq!(format_amount(d("-3")), "-3.00");
        assert_eq!(format_amount(d("999.5000")), "999.50");
        assert_eq!(format_amount(d("0")), "0.00");
        assert_eq!(format_quantity(d("2.50")), "2.5");
        assert_eq!(format_quantity(d("12000")), "12,000");
        assert_eq!(format_quantity(d("-0.125")), "-0.125");
    }
}
