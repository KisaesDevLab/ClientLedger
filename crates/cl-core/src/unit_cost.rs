//! [`UnitCost`]: an item's average or purchase cost per unit with up to six
//! decimal places (P24 plan §2.0, §3: engine `numeric(19,6)`, the average
//! cost `round6(T / Q)` the engine reports as a six-place string).
//!
//! - Exact: the value is a whole number of millionths in an `i128`. There is
//!   no float constructor, accessor or conversion.
//! - Signed: a negative average is possible while value and quantity have
//!   opposite signs (P24 §5 "negative value with positive quantity").
//! - The wire form accepted is a JSON **string** `^-?\d{1,13}(\.\d{1,6})?$`;
//!   a JSON number is refused. The wire form sent is canonical with exactly
//!   six decimals (`3.588667`).
//! - Rounding (`round6`, `round2`) is half away from zero on the exact
//!   rational, negatives included (plan §2.0 "Rounding").

use std::fmt;

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::money::{Money, MoneyError, MoneyStyle, NegativeStyle, Quantity};

/// Decimal places of a unit cost (engine `numeric(19,6)`).
pub const UNIT_COST_SCALE: u32 = 6;

/// Integer digits of a unit cost (`numeric(19,6)`).
pub const UNIT_COST_INTEGER_DIGITS: u32 = 13;

/// Millionths in one whole unit.
const PER_WHOLE: i128 = 1_000_000;

/// A cost per unit with at most six decimals.
///
/// Deliberately no `Display` (use [`UnitCost::format`] for owners and
/// [`UnitCost::to_wire`] for the engine) and no float conversion.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct UnitCost(i128);

impl UnitCost {
    /// Zero.
    pub const ZERO: UnitCost = UnitCost(0);
    /// The largest value the column holds (13 nines, six nines).
    pub const MAX: UnitCost = UnitCost(9_999_999_999_999_999_999);
    /// The smallest (most negative) value the column holds.
    pub const MIN: UnitCost = UnitCost(-9_999_999_999_999_999_999);

    /// Exact value from millionths (`3_588_667` is 3.588667).
    ///
    /// # Errors
    /// [`MoneyError::OutOfRange`] outside the column's range.
    pub const fn from_millionths(units: i128) -> Result<UnitCost, MoneyError> {
        if units > Self::MAX.0 || units < Self::MIN.0 {
            return Err(MoneyError::OutOfRange);
        }
        Ok(UnitCost(units))
    }

    /// The exact value in millionths.
    #[must_use]
    pub const fn to_millionths(self) -> i128 {
        self.0
    }

    /// Parse text received from the engine (`^-?\d{1,13}(\.\d{1,6})?$`).
    /// `-0` becomes zero.
    ///
    /// # Errors
    /// [`MoneyError::Format`] for anything else; [`MoneyError::TooPrecise`]
    /// for seven or more decimals.
    pub fn parse_wire(s: &str) -> Result<UnitCost, MoneyError> {
        let (neg, body) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (body, None),
        };
        if int.is_empty() || int.len() > UNIT_COST_INTEGER_DIGITS as usize || !all_digits(int) {
            return Err(MoneyError::Format);
        }
        let frac = match frac {
            Some(f) if f.is_empty() || !all_digits(f) => return Err(MoneyError::Format),
            Some(f) if f.len() > UNIT_COST_SCALE as usize => return Err(MoneyError::TooPrecise),
            Some(f) => f,
            None => "",
        };
        let mut units: i128 = 0;
        for b in int.bytes() {
            units = units * 10 + i128::from(b - b'0');
        }
        let mut f: i128 = 0;
        for b in frac.bytes() {
            f = f * 10 + i128::from(b - b'0');
        }
        // `frac.len() <= 6` was checked above.
        let pad = UNIT_COST_SCALE - u32::try_from(frac.len()).unwrap_or(UNIT_COST_SCALE);
        let units = units * PER_WHOLE + f * 10_i128.pow(pad);
        UnitCost::from_millionths(if neg { -units } else { units })
    }

    /// Canonical text for the engine: exactly six decimals (`3.588667`).
    /// Never `-0.000000`.
    #[must_use]
    pub fn to_wire(self) -> String {
        let abs = self.0.unsigned_abs();
        let per = PER_WHOLE.unsigned_abs();
        let sign = if self.0 < 0 { "-" } else { "" };
        format!("{sign}{}.{:06}", abs / per, abs % per)
    }

    /// Display text with thousands separators: at least two decimals, more
    /// only when they are not zero (`3.588667`, `3.80`), so nothing is
    /// rounded or hidden.
    #[must_use]
    pub fn format(self, style: MoneyStyle) -> String {
        let abs = self.0.unsigned_abs();
        let per = PER_WHOLE.unsigned_abs();
        let mut frac = format!("{:06}", abs % per);
        while frac.len() > 2 && frac.ends_with('0') {
            frac.pop();
        }
        let mut body = String::new();
        if style.symbol {
            body.push('$');
        }
        body.push_str(&group_thousands(abs / per));
        body.push('.');
        body.push_str(&frac);
        if self.0 < 0 {
            match style.negative {
                NegativeStyle::Minus => format!("-{body}"),
                NegativeStyle::Parens => format!("({body})"),
            }
        } else {
            body
        }
    }

    /// `round6(value / quantity)`, half away from zero: the average cost of
    /// `value` over `quantity` (P24 §2.2 `L`). `None` for a zero quantity.
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] when the result leaves the column's range.
    pub fn average(value: Money, quantity: Quantity) -> Result<Option<UnitCost>, MoneyError> {
        if quantity.is_zero() {
            return Ok(None);
        }
        let q = value
            .as_decimal()
            .checked_div(quantity.as_decimal())
            .ok_or(MoneyError::Overflow)?
            .round_dp_with_strategy(UNIT_COST_SCALE, RoundingStrategy::MidpointAwayFromZero);
        UnitCost::try_from(q)
            .map(Some)
            .map_err(|_| MoneyError::Overflow)
    }

    /// `round2(self × quantity)`, half away from zero: the value of
    /// `quantity` units at this cost, to the cent (P24 §2.2 excess and
    /// empty-stock inflows).
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] when the result leaves the money range.
    pub fn value_of(self, quantity: Quantity) -> Result<Money, MoneyError> {
        let v = self
            .as_decimal()
            .checked_mul(quantity.as_decimal())
            .ok_or(MoneyError::Overflow)?
            .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
        Money::try_from(v).map_err(|_| MoneyError::Overflow)
    }

    /// True for zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// True above zero.
    #[must_use]
    pub const fn is_positive(self) -> bool {
        self.0 > 0
    }

    /// True below zero.
    #[must_use]
    pub const fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// The exact value as a `Decimal` (read-only; trailing zeros stripped).
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        // |units| < 10^19 fits a Decimal's 96-bit mantissa.
        Decimal::try_from_i128_with_scale(self.0, UNIT_COST_SCALE).map_or(Decimal::ZERO, |d| {
            let mut n = d.normalize();
            if n.is_zero() {
                n.set_sign_positive(true);
            }
            n
        })
    }
}

/// Every money amount (four places) is a unit cost (six places) when it
/// fits 13 integer digits.
impl TryFrom<Money> for UnitCost {
    type Error = MoneyError;

    fn try_from(m: Money) -> Result<UnitCost, MoneyError> {
        UnitCost::from_millionths(m.to_ten_thousandths() * 100)
    }
}

impl TryFrom<Decimal> for UnitCost {
    type Error = MoneyError;

    /// Trailing zeros are stripped first; a non-zero seventh decimal is
    /// refused, never rounded.
    fn try_from(d: Decimal) -> Result<UnitCost, MoneyError> {
        let mut n = d.normalize();
        if n.scale() > UNIT_COST_SCALE {
            return Err(MoneyError::TooPrecise);
        }
        n.rescale(UNIT_COST_SCALE);
        UnitCost::from_millionths(n.mantissa())
    }
}

/// Prints the cost itself (`UnitCost(3.588667)`): owner data. Never log a
/// value with `?value` at info level or above.
impl fmt::Debug for UnitCost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UnitCost({})", self.to_wire())
    }
}

impl Serialize for UnitCost {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

/// Accepts only a JSON string in the engine grammar; a JSON number is an
/// `invalid type` error.
impl<'de> Deserialize<'de> for UnitCost {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<UnitCost, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = UnitCost;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a decimal string (UnitCost)")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<UnitCost, E> {
                UnitCost::parse_wire(v).map_err(E::custom)
            }
        }
        d.deserialize_str(V)
    }
}

fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// `1234567` → `1,234,567`.
fn group_thousands(whole: u128) -> String {
    let digits = whole.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> UnitCost {
        UnitCost::parse_wire(s).expect("cost")
    }
    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("money")
    }
    fn q(s: &str) -> Quantity {
        Quantity::parse_wire(s).expect("qty")
    }

    #[test]
    fn wire_grammar_and_canonical_form() {
        assert_eq!(c("3.588667").to_wire(), "3.588667");
        assert_eq!(c("3.8").to_wire(), "3.800000");
        assert_eq!(c("12").to_wire(), "12.000000");
        assert_eq!(c("-0.000001").to_wire(), "-0.000001");
        assert_eq!(c("-0").to_wire(), "0.000000");
        assert_eq!(c("9999999999999.999999"), UnitCost::MAX);
        assert_eq!(c("-9999999999999.999999"), UnitCost::MIN);
        for bad in [
            "",
            "-",
            ".5",
            "5.",
            "+5",
            "1e3",
            " 1",
            "1 ",
            "1,000",
            "NaN",
            "10000000000000",
        ] {
            assert_eq!(UnitCost::parse_wire(bad), Err(MoneyError::Format), "{bad}");
        }
        assert_eq!(
            UnitCost::parse_wire("1.0000001"),
            Err(MoneyError::TooPrecise)
        );
        assert_eq!(
            UnitCost::from_millionths(UnitCost::MAX.0 + 1),
            Err(MoneyError::OutOfRange)
        );
    }

    #[test]
    fn serde_takes_strings_only() {
        let v: UnitCost = serde_json::from_str("\"4.050000\"").expect("string");
        assert_eq!(v, c("4.05"));
        assert!(serde_json::from_str::<UnitCost>("4.05").is_err());
        assert_eq!(serde_json::to_string(&v).expect("json"), "\"4.050000\"");
        assert_eq!(format!("{v:?}"), "UnitCost(4.050000)");
    }

    #[test]
    fn format_keeps_every_digit() {
        let s = MoneyStyle::default();
        assert_eq!(c("3.588667").format(s), "3.588667");
        assert_eq!(c("3.8").format(s), "3.80");
        assert_eq!(c("1234.5").format(s), "1,234.50");
        assert_eq!(
            c("-0.5").format(MoneyStyle {
                negative: NegativeStyle::Parens,
                symbol: true
            }),
            "($0.50)"
        );
    }

    /// The reference sequence's averages (P24 §6.1 case A).
    #[test]
    fn average_is_round6_half_away_from_zero() {
        let avg = |v, n| UnitCost::average(m(v), q(n)).expect("ok").expect("some");
        assert_eq!(avg("33.33", "10").to_wire(), "3.333000");
        assert_eq!(avg("53.83", "15").to_wire(), "3.588667");
        assert_eq!(avg("39.48", "11").to_wire(), "3.589091");
        assert_eq!(avg("20.00", "6").to_wire(), "3.333333");
        assert_eq!(avg("-0.01", "1").to_wire(), "-0.010000");
        // 0.0000005 midpoints round away from zero.
        assert_eq!(avg("0.0005", "1000").to_wire(), "0.000001");
        assert_eq!(avg("-0.0005", "1000").to_wire(), "-0.000001");
        assert_eq!(UnitCost::average(m("1"), Quantity::ZERO), Ok(None));
    }

    #[test]
    fn value_of_is_round2_half_away_from_zero() {
        assert_eq!(c("3.589091").value_of(q("3")).expect("v"), m("10.77"));
        assert_eq!(c("3.588889").value_of(q("2")).expect("v"), m("7.18"));
        // 0.005 → 0.01 and -0.005 → -0.01 (banker's would give 0.00).
        assert_eq!(c("0.01").value_of(q("0.5")).expect("v"), m("0.01"));
        assert_eq!(c("-0.01").value_of(q("0.5")).expect("v"), m("-0.01"));
        assert_eq!(c("2.01").value_of(q("-1.5")).expect("v"), m("-3.02"));
    }

    #[test]
    fn conversions_are_exact() {
        assert_eq!(UnitCost::try_from(m("3.3333")).expect("m"), c("3.3333"));
        assert_eq!(
            UnitCost::try_from(Decimal::new(35_886_670, 7)).expect("d"),
            c("3.588667")
        );
        assert_eq!(
            UnitCost::try_from(Decimal::new(35_886_671, 7)),
            Err(MoneyError::TooPrecise)
        );
        assert_eq!(c("3.50").as_decimal(), Decimal::new(35, 1));
        assert_eq!(c("-0").as_decimal(), Decimal::ZERO);
        assert!(c("1").is_positive() && c("-1").is_negative() && UnitCost::ZERO.is_zero());
        assert_eq!(c("1").to_millionths(), 1_000_000);
    }

    /// Quantities are signed: adjustment deltas and running on-hand figures
    /// go below zero (P24 §3).
    #[test]
    fn quantities_are_signed_deltas() {
        let d = q("-3");
        assert!(d.is_negative());
        assert_eq!(d.to_wire(), "-3.0000");
        assert_eq!(q("-0.0001").to_wire(), "-0.0001");
        assert_eq!(q("7").checked_add(q("-10")).expect("sum"), q("-3"));
        assert_eq!(Quantity::MIN.to_wire(), "-99999999.9999");
    }
}
