//! [`UnitFactor`]: how many base units one unit of a unit set holds (P24
//! plan §2.4 U-161, §4 "Base quantity"; Q-P24i, Q-P24v).
//!
//! - Exact: a whole number of millionths in an `i128`, above zero, at most
//!   six decimals and at most 1,000,000 (the engine's `unitFactorSchema`).
//!   No float constructor, accessor or conversion.
//! - Wire form: a JSON **string** `^\d{1,7}(\.\d{1,6})?$`; sent with
//!   exactly six decimals (`1.333333`).
//! - [`UnitFactor::base_quantity`] is the one conversion rule:
//!   `round4(quantity × factor)`, half away from zero (Q-P24v). Money never
//!   converts: a line's amount stays `round2(quantity × unit price)` in the
//!   unit entered.

use std::fmt;

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::money::{MoneyError, Quantity};

/// Decimal places of a factor.
pub const UNIT_FACTOR_SCALE: u32 = 6;

/// Millionths in one.
const PER_WHOLE: i128 = 1_000_000;

/// The largest factor (1,000,000 base units per unit).
const MAX_MILLIONTHS: i128 = 1_000_000 * PER_WHOLE;

/// Base units per unit (above zero, at most six decimals, at most
/// 1,000,000). Deliberately no `Display` and no float conversion.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitFactor(i128);

impl UnitFactor {
    /// One (the base unit's own factor).
    pub const ONE: UnitFactor = UnitFactor(PER_WHOLE);

    /// Exact value from millionths (`1_333_333` is 1.333333).
    ///
    /// # Errors
    /// [`MoneyError::OutOfRange`] for zero or less, or above 1,000,000.
    pub const fn from_millionths(units: i128) -> Result<UnitFactor, MoneyError> {
        if units <= 0 || units > MAX_MILLIONTHS {
            return Err(MoneyError::OutOfRange);
        }
        Ok(UnitFactor(units))
    }

    /// The exact value in millionths.
    #[must_use]
    pub const fn to_millionths(self) -> i128 {
        self.0
    }

    /// Parse the engine's text (`^\d{1,7}(\.\d{1,6})?$`).
    ///
    /// # Errors
    /// [`MoneyError::Format`]; [`MoneyError::TooPrecise`] for seven or more
    /// decimals; [`MoneyError::OutOfRange`] for zero or above 1,000,000.
    pub fn parse_wire(s: &str) -> Result<UnitFactor, MoneyError> {
        let (int, frac) = match s.split_once('.') {
            Some((i, f)) => (i, Some(f)),
            None => (s, None),
        };
        let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
        if !digits(int) || int.len() > 7 {
            return Err(MoneyError::Format);
        }
        let frac = match frac {
            Some(f) if !digits(f) => return Err(MoneyError::Format),
            Some(f) if f.len() > UNIT_FACTOR_SCALE as usize => return Err(MoneyError::TooPrecise),
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
        let pad = UNIT_FACTOR_SCALE - u32::try_from(frac.len()).unwrap_or(UNIT_FACTOR_SCALE);
        UnitFactor::from_millionths(units * PER_WHOLE + f * 10_i128.pow(pad))
    }

    /// What the owner typed: surrounding spaces and thousands commas are
    /// allowed; otherwise as [`UnitFactor::parse_wire`].
    ///
    /// # Errors
    /// As [`UnitFactor::parse_wire`].
    pub fn parse_user(s: &str) -> Result<UnitFactor, MoneyError> {
        let t: String = s.trim().chars().filter(|c| *c != ',').collect();
        UnitFactor::parse_wire(&t)
    }

    /// Canonical text for the engine: exactly six decimals (`12.000000`).
    #[must_use]
    pub fn to_wire(self) -> String {
        let abs = self.0.unsigned_abs();
        let per = PER_WHOLE.unsigned_abs();
        format!("{}.{:06}", abs / per, abs % per)
    }

    /// Display text: no trailing zeros (`12`, `1.333333`).
    #[must_use]
    pub fn format(self) -> String {
        let w = self.to_wire();
        let t = w.trim_end_matches('0');
        t.strip_suffix('.').unwrap_or(t).to_string()
    }

    /// The exact value as a `Decimal`.
    #[must_use]
    pub fn as_decimal(self) -> Decimal {
        Decimal::try_from_i128_with_scale(self.0, UNIT_FACTOR_SCALE)
            .map_or(Decimal::ZERO, |d| d.normalize())
    }

    /// `round4(quantity × self)`, half away from zero (Q-P24v): the base
    /// units a line in this unit moves. The engine refuses a non-zero line
    /// whose base quantity rounds to zero (`UNIT_QUANTITY_TOO_SMALL`).
    ///
    /// # Errors
    /// [`MoneyError::Overflow`] when the result leaves the quantity range.
    pub fn base_quantity(self, quantity: Quantity) -> Result<Quantity, MoneyError> {
        let v = quantity
            .as_decimal()
            .checked_mul(self.as_decimal())
            .ok_or(MoneyError::Overflow)?
            .round_dp_with_strategy(4, RoundingStrategy::MidpointAwayFromZero);
        Quantity::try_from(v).map_err(|_| MoneyError::Overflow)
    }
}

/// Prints the factor itself (`UnitFactor(12.000000)`).
impl fmt::Debug for UnitFactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "UnitFactor({})", self.to_wire())
    }
}

impl Serialize for UnitFactor {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_wire())
    }
}

/// Accepts only a JSON string in the engine grammar.
impl<'de> Deserialize<'de> for UnitFactor {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<UnitFactor, D::Error> {
        let s = String::deserialize(d)?;
        UnitFactor::parse_wire(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &str) -> UnitFactor {
        UnitFactor::parse_wire(s).expect("factor")
    }
    fn q(s: &str) -> Quantity {
        Quantity::parse_wire(s).expect("qty")
    }

    #[test]
    fn the_grammar_is_the_engines() {
        assert_eq!(f("12").to_wire(), "12.000000");
        assert_eq!(f("1.333333").format(), "1.333333");
        assert_eq!(f("12.500000").format(), "12.5");
        assert_eq!(f("1000000").to_wire(), "1000000.000000");
        assert_eq!(UnitFactor::parse_wire("0"), Err(MoneyError::OutOfRange));
        assert_eq!(
            UnitFactor::parse_wire("1000000.000001"),
            Err(MoneyError::OutOfRange)
        );
        assert_eq!(
            UnitFactor::parse_wire("1.0000001"),
            Err(MoneyError::TooPrecise)
        );
        assert_eq!(UnitFactor::parse_wire("-1"), Err(MoneyError::Format));
        assert_eq!(UnitFactor::parse_wire("1e3"), Err(MoneyError::Format));
        assert_eq!(UnitFactor::parse_wire(".5"), Err(MoneyError::Format));
        assert_eq!(
            UnitFactor::parse_user(" 1,000 ").map(UnitFactor::format),
            Ok("1000".into())
        );
        assert!(
            serde_json::from_str::<UnitFactor>("12").is_err(),
            "a JSON number"
        );
        assert_eq!(
            serde_json::to_string(&f("6")).expect("json"),
            "\"6.000000\""
        );
    }

    #[test]
    fn base_quantities_are_golden_case_u() {
        // case = 12, half-case = 6, litre = 1.333333, ml = 0.001333.
        assert_eq!(
            f("12").base_quantity(q("2")).map(Quantity::to_wire),
            Ok("24.0000".into())
        );
        assert_eq!(
            f("6").base_quantity(q("1")).map(Quantity::to_wire),
            Ok("6.0000".into())
        );
        // round4(0.99999975) = 1.0000 (half away from zero).
        assert_eq!(
            f("1.333333")
                .base_quantity(q("0.75"))
                .map(Quantity::to_wire),
            Ok("1.0000".into())
        );
        // round4(0.0000001333) = 0: the engine's UNIT_QUANTITY_TOO_SMALL.
        assert_eq!(f("0.001333").base_quantity(q("0.0001")), Ok(Quantity::ZERO));
        // Midpoints round away from zero, negatives included.
        assert_eq!(
            f("0.5").base_quantity(q("0.0001")).map(Quantity::to_wire),
            Ok("0.0001".into())
        );
        assert_eq!(
            f("0.5").base_quantity(q("-0.0001")).map(Quantity::to_wire),
            Ok("-0.0001".into())
        );
        assert_eq!(
            f("1000000").base_quantity(q("99999999")),
            Err(MoneyError::Overflow)
        );
    }
}
