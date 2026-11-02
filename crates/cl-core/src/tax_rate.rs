//! [`TaxRate`]: a sales tax rate with up to six decimal places (P23 plan
//! §3 WP-1, engine U-148 `numeric(7,6)`; Q-P23j keeps [`crate::Rate`] for
//! everything else).
//!
//! - The engine stores rates as a fraction (`0.088750` is 8.875 %). Engines
//!   before U-148 send four places (`0.0810`), later ones six; both decode
//!   to the same value, so every decoder can switch before the pin bump.
//! - Exact: the value is a whole number of millionths. There is no float
//!   constructor, accessor or conversion.
//! - The wire form accepted is a JSON **string** `^-?\d(\.\d{1,6})?$`; a
//!   JSON number is refused. The decoder is lenient on range (one integer
//!   digit, as the old `decimal(5,4)` column) so a stored legacy value
//!   never makes a whole record unreadable; [`TaxRate::is_setup_valid`] is
//!   the `0 ≤ r < 1` rule of a setup screen (plan §2.0).
//! - Owners type and see percents: [`TaxRate::parse_percent`] (at most
//!   four decimals of a percent) and [`TaxRate::format_percent`]
//!   (`8.100%`). This module is the only place a percent becomes a
//!   fraction.

use std::fmt;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::money::{MoneyError, Rate};

/// Decimal places of a tax rate fraction (engine `numeric(7,6)`).
pub const TAX_RATE_SCALE: u32 = 6;

/// Most decimal places of a typed percent (six places of the fraction).
pub const PERCENT_DECIMALS: u32 = 4;

/// Millionths in one whole unit.
const PER_WHOLE: i64 = 1_000_000;

/// Millionths in one percent.
const PER_PERCENT: i64 = 10_000;

/// Why typed percent text is not a tax rate. Messages never contain the
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TaxRateInputError {
    /// Nothing typed.
    #[error("no rate entered")]
    Empty,
    /// Not a percent (letters, two points, a sign, thousands commas).
    #[error("not a percent")]
    Invalid,
    /// More than four decimals of a percent.
    #[error("too many decimal places (at most {max})")]
    TooManyDecimals {
        /// The allowed number of decimal places.
        max: u32,
    },
    /// 100 % or more.
    #[error("the rate must be below 100%")]
    TooLarge,
}

/// A sales tax rate as a fraction with at most six decimals (`0.08875` is
/// 8.875 %).
///
/// Deliberately no arithmetic, no `Display` and no float conversion: the
/// engine computes every tax amount (CLAUDE.md non-negotiable 6, plan §4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct TaxRate(i64);

impl TaxRate {
    /// Zero.
    pub const ZERO: TaxRate = TaxRate(0);
    /// The largest value the decoder takes (`9.999999`, one integer digit).
    pub const MAX: TaxRate = TaxRate(9_999_999);
    /// The smallest value the decoder takes.
    pub const MIN: TaxRate = TaxRate(-9_999_999);

    /// Exact value from millionths (`88_750` is 0.08875).
    ///
    /// # Errors
    /// [`MoneyError::OutOfRange`] beyond one integer digit.
    pub const fn from_millionths(units: i64) -> Result<TaxRate, MoneyError> {
        if units > Self::MAX.0 || units < Self::MIN.0 {
            return Err(MoneyError::OutOfRange);
        }
        Ok(TaxRate(units))
    }

    /// The exact value in millionths.
    #[must_use]
    pub const fn to_millionths(self) -> i64 {
        self.0
    }

    /// Parse text received from the engine: `^-?\d(\.\d{1,6})?$`, so both
    /// the four-place form of engines before U-148 and the six-place form
    /// after it. `-0` becomes zero.
    ///
    /// # Errors
    /// [`MoneyError::Format`] for anything else (`+`, exponents, spaces,
    /// two integer digits, an empty fraction); [`MoneyError::TooPrecise`]
    /// for seven or more decimals.
    pub fn parse_wire(s: &str) -> Result<TaxRate, MoneyError> {
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (body, None),
        };
        if int.len() != 1 || !all_digits(int) {
            return Err(MoneyError::Format);
        }
        let frac = match frac {
            Some(f) if f.is_empty() || !all_digits(f) => return Err(MoneyError::Format),
            Some(f) if f.len() > TAX_RATE_SCALE as usize => return Err(MoneyError::TooPrecise),
            Some(f) => f,
            None => "",
        };
        let units = digits_to_millionths(int, frac);
        TaxRate::from_millionths(if neg { -units } else { units })
    }

    /// Canonical text for the engine: at least four decimals, six only
    /// when the fifth or sixth is not zero (`0.0825`, `0.08875` →
    /// `0.088750`). Engines before U-148 store `decimal(5,4)` and take the
    /// four-place form unchanged; later engines take either.
    #[must_use]
    pub fn to_wire(self) -> String {
        if self.0 % 100 == 0 {
            fixed(self.0 / 100, 4)
        } else {
            fixed(self.0, 6)
        }
    }

    /// Always six decimals (`0.082500`), the engine's U-148 output form.
    #[must_use]
    pub fn to_wire6(self) -> String {
        fixed(self.0, 6)
    }

    /// The owner's percent text (`8.25`, `8.875%`, ` 7 `): digits with an
    /// optional point and at most four decimals, an optional `%`, zero or
    /// more and below 100.
    ///
    /// # Errors
    /// See [`TaxRateInputError`]. Nothing is rounded.
    pub fn parse_percent(s: &str) -> Result<TaxRate, TaxRateInputError> {
        let t = s.trim();
        let t = t.strip_suffix('%').unwrap_or(t).trim_end();
        if t.is_empty() {
            return Err(TaxRateInputError::Empty);
        }
        let (int, frac) = match t.split_once('.') {
            Some((i, f)) => (i, f),
            None => (t, ""),
        };
        if !all_digits(int) || !all_digits(frac) || (int.is_empty() && frac.is_empty()) {
            return Err(TaxRateInputError::Invalid);
        }
        if frac.len() > PERCENT_DECIMALS as usize {
            return Err(TaxRateInputError::TooManyDecimals {
                max: PERCENT_DECIMALS,
            });
        }
        let int = int.trim_start_matches('0');
        if int.len() > 2 {
            return Err(TaxRateInputError::TooLarge);
        }
        let mut whole: i64 = 0;
        for b in int.bytes() {
            whole = whole * 10 + i64::from(b - b'0');
        }
        let mut f: i64 = 0;
        for b in frac.bytes() {
            f = f * 10 + i64::from(b - b'0');
        }
        let pad = PERCENT_DECIMALS - u32::try_from(frac.len()).unwrap_or(PERCENT_DECIMALS);
        let units = whole * PER_PERCENT + f * 10_i64.pow(pad);
        if units >= PER_WHOLE {
            return Err(TaxRateInputError::TooLarge);
        }
        Ok(TaxRate(units))
    }

    /// The percent for owners, with three to four decimals and a `%`
    /// (`8.100%`, `8.875%`, `4.2255%`, `0.000%`).
    #[must_use]
    pub fn format_percent(self) -> String {
        format!("{}%", percent_text(self.0, 3))
    }

    /// The percent as an editable field shows it: no `%`, trailing zeros
    /// removed (`8.1`, `7`, `8.875`).
    #[must_use]
    pub fn percent_input(self) -> String {
        percent_text(self.0, 0)
    }

    /// The setup rule (plan §2.0): zero or more and below 1 (100 %).
    #[must_use]
    pub const fn is_setup_valid(self) -> bool {
        self.0 >= 0 && self.0 < PER_WHOLE
    }

    /// True for zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// The exact value as a `Decimal` (read-only; trailing zeros stripped).
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        Decimal::new(self.0, TAX_RATE_SCALE).normalize()
    }

    /// The same rate as a four-place [`Rate`], when it has no fifth or
    /// sixth decimal (for callers that still hold the old type).
    #[must_use]
    pub fn to_rate(self) -> Option<Rate> {
        if self.0 % 100 != 0 {
            return None;
        }
        Rate::from_ten_thousandths(i128::from(self.0 / 100)).ok()
    }
}

/// Every four-place rate is a tax rate (exact).
impl From<Rate> for TaxRate {
    fn from(r: Rate) -> TaxRate {
        // A Rate holds at most ±9.9999, so its millionths fit easily.
        let units = r.to_ten_thousandths() * 100;
        TaxRate(i64::try_from(units).unwrap_or(0))
    }
}

impl TryFrom<Decimal> for TaxRate {
    type Error = MoneyError;

    /// Trailing zeros are stripped first; a non-zero seventh decimal is
    /// refused, never rounded.
    fn try_from(d: Decimal) -> Result<TaxRate, MoneyError> {
        let mut n = d.normalize();
        if n.scale() > TAX_RATE_SCALE {
            return Err(MoneyError::TooPrecise);
        }
        n.rescale(TAX_RATE_SCALE);
        let units = i64::try_from(n.mantissa()).map_err(|_| MoneyError::OutOfRange)?;
        TaxRate::from_millionths(units)
    }
}

/// A rate alone is not owner data (`TaxRate(0.0825)`).
impl fmt::Debug for TaxRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TaxRate({})", self.to_wire())
    }
}

impl Serialize for TaxRate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

/// Accepts only a JSON string in the engine grammar; a JSON number is an
/// `invalid type` error.
impl<'de> Deserialize<'de> for TaxRate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<TaxRate, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = TaxRate;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a decimal string (TaxRate)")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TaxRate, E> {
                TaxRate::parse_wire(v).map_err(E::custom)
            }
        }
        d.deserialize_str(V)
    }
}

fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// One integer digit and at most six fraction digits (checked by the
/// caller).
fn digits_to_millionths(int: &str, frac: &str) -> i64 {
    let mut units: i64 = 0;
    for b in int.bytes() {
        units = units * 10 + i64::from(b - b'0');
    }
    let mut f: i64 = 0;
    for b in frac.bytes() {
        f = f * 10 + i64::from(b - b'0');
    }
    let pad = TAX_RATE_SCALE - u32::try_from(frac.len()).unwrap_or(TAX_RATE_SCALE);
    units * PER_WHOLE + f * 10_i64.pow(pad)
}

/// `units` with exactly `places` decimals (`units` already scaled to them).
fn fixed(units: i64, places: u32) -> String {
    let per = 10_i64.pow(places);
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    let per = per.unsigned_abs();
    format!(
        "{sign}{}.{:0width$}",
        abs / per,
        abs % per,
        width = places as usize
    )
}

/// The percent (millionths / 10 000) with at least `min_decimals` and at
/// most four decimals, trailing zeros beyond the minimum removed.
fn percent_text(units: i64, min_decimals: usize) -> String {
    let sign = if units < 0 { "-" } else { "" };
    let abs = units.unsigned_abs();
    let per = PER_PERCENT.unsigned_abs();
    let mut frac = format!("{:04}", abs % per);
    while frac.len() > min_decimals && frac.ends_with('0') {
        frac.pop();
    }
    if frac.is_empty() {
        format!("{sign}{}", abs / per)
    } else {
        format!("{sign}{}.{frac}", abs / per)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> TaxRate {
        TaxRate::parse_wire(s).expect("rate")
    }

    #[test]
    fn four_and_six_place_wire_forms_decode_alike() {
        assert_eq!(r("0.0810"), r("0.081000"));
        assert_eq!(r("0.0810"), r("0.081"));
        assert_eq!(r("0.088750").to_millionths(), 88_750);
        assert_eq!(r("0.0000"), TaxRate::ZERO);
        assert_eq!(r("-0.0000"), TaxRate::ZERO);
        assert_eq!(r("0").to_millionths(), 0);
        assert_eq!(r("1.5").to_millionths(), 1_500_000);
    }

    #[test]
    fn wire_grammar_is_strict() {
        for bad in [
            "", ".5", "0.", "00.1", "10.0", "+0.1", "0.1e2", " 0.1", "0,1", "NaN", "--0.1",
        ] {
            assert_eq!(TaxRate::parse_wire(bad), Err(MoneyError::Format), "{bad:?}");
        }
        assert_eq!(
            TaxRate::parse_wire("0.0887501"),
            Err(MoneyError::TooPrecise)
        );
    }

    #[test]
    fn to_wire_keeps_four_places_when_they_suffice() {
        assert_eq!(r("0.0825").to_wire(), "0.0825");
        assert_eq!(r("0.082500").to_wire(), "0.0825");
        assert_eq!(r("0.08875").to_wire(), "0.088750");
        assert_eq!(r("0.011").to_wire(), "0.0110");
        assert_eq!(TaxRate::ZERO.to_wire(), "0.0000");
        assert_eq!(r("0.08875").to_wire6(), "0.088750");
        assert_eq!(r("0.0825").to_wire6(), "0.082500");
        assert_eq!(r("-0.000001").to_wire(), "-0.000001");
    }

    #[test]
    fn percents_parse_exactly() {
        let p = |s| TaxRate::parse_percent(s);
        assert_eq!(p("8.875"), Ok(r("0.08875")));
        assert_eq!(p("8.875%"), Ok(r("0.08875")));
        assert_eq!(p(" 8.25 % "), Ok(r("0.0825")));
        assert_eq!(p("7"), Ok(r("0.07")));
        assert_eq!(p(".5"), Ok(r("0.005")));
        assert_eq!(p("5."), Ok(r("0.05")));
        assert_eq!(p("0"), Ok(TaxRate::ZERO));
        assert_eq!(p("4.2255"), Ok(r("0.042255")));
        assert_eq!(p("99.9999"), Ok(r("0.999999")));
        assert_eq!(p("007.5"), Ok(r("0.075")));
        assert_eq!(p(""), Err(TaxRateInputError::Empty));
        assert_eq!(p(" % "), Err(TaxRateInputError::Empty));
        assert_eq!(
            p("8.87501"),
            Err(TaxRateInputError::TooManyDecimals { max: 4 })
        );
        assert_eq!(p("100"), Err(TaxRateInputError::TooLarge));
        assert_eq!(p("250"), Err(TaxRateInputError::TooLarge));
        for bad in ["-1", "+1", "8,5", "8.2.5", "abc", ".", "1e2", "8 %%"] {
            assert_eq!(p(bad), Err(TaxRateInputError::Invalid), "{bad:?}");
        }
    }

    #[test]
    fn percents_format_with_three_to_four_decimals() {
        assert_eq!(r("0.081").format_percent(), "8.100%");
        assert_eq!(r("0.08875").format_percent(), "8.875%");
        assert_eq!(r("0.04225").format_percent(), "4.225%");
        assert_eq!(r("0.042255").format_percent(), "4.2255%");
        assert_eq!(TaxRate::ZERO.format_percent(), "0.000%");
        assert_eq!(r("0.081").percent_input(), "8.1");
        assert_eq!(r("0.07").percent_input(), "7");
        assert_eq!(r("0.08875").percent_input(), "8.875");
        assert_eq!(TaxRate::ZERO.percent_input(), "0");
    }

    #[test]
    fn setup_rule_is_zero_to_below_one() {
        assert!(TaxRate::ZERO.is_setup_valid());
        assert!(r("0.999999").is_setup_valid());
        assert!(!r("1").is_setup_valid());
        assert!(!r("-0.01").is_setup_valid());
    }

    #[test]
    fn json_takes_strings_only() {
        let t: TaxRate = serde_json::from_str("\"0.088750\"").expect("string");
        assert_eq!(t, r("0.08875"));
        assert!(serde_json::from_str::<TaxRate>("0.08875").is_err());
        assert!(serde_json::from_str::<TaxRate>("\"0.0887501\"").is_err());
        assert_eq!(
            serde_json::to_string(&r("0.08875")).expect("json"),
            "\"0.088750\""
        );
        assert_eq!(format!("{:?}", r("0.0825")), "TaxRate(0.0825)");
    }

    #[test]
    fn conversions_are_exact() {
        let old = Rate::parse_wire("0.0825").expect("rate");
        assert_eq!(TaxRate::from(old), r("0.0825"));
        assert_eq!(r("0.0825").to_rate(), Some(old));
        assert_eq!(r("0.08875").to_rate(), None);
        assert_eq!(
            TaxRate::try_from(Decimal::new(8_875_000, 8)),
            Ok(r("0.08875"))
        );
        assert_eq!(
            TaxRate::try_from(Decimal::new(1, 7)),
            Err(MoneyError::TooPrecise)
        );
        assert_eq!(r("0.08875").as_decimal(), Decimal::new(8875, 5));
        assert_eq!(
            TaxRate::from_millionths(10_000_000),
            Err(MoneyError::OutOfRange)
        );
    }
}
