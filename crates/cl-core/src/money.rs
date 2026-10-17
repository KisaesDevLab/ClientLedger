//! Exact amounts: [`Money`], [`Quantity`] and [`Rate`].
//!
//! Each type is a fixed-scale decimal over [`rust_decimal::Decimal`] that
//! mirrors one engine column type:
//!
//! | Type | Engine column | Integer digits | Decimals |
//! |---|---|---|---|
//! | [`Money`] | `decimal(19,4)` | 15 | ≤ 4 |
//! | [`Quantity`] | `decimal(12,4)` | 8 | ≤ 4 |
//! | [`Rate`] | `decimal(5,4)` | 1 | ≤ 4 |
//!
//! Rules (CLAUDE.md non-negotiable 6, P05 plan §2.1 and §5):
//! - No floating point anywhere: there is no float constructor, accessor or
//!   conversion, and the crate denies the float lints.
//! - The wire form sent to the engine is canonical: `-?\d{1,N}\.\d{4}`.
//! - The wire form accepted from the engine is `^-?\d{1,N}(\.\d{1,4})?$` and
//!   arrives only as a JSON **string**; a JSON number is refused.
//! - Zero never carries a sign: every constructor and every arithmetic result
//!   is normalized, so `to_wire()` never emits `-0.0000`.
//! - Arithmetic is checked: leaving the column's range is an error, never a
//!   wrap or a silent rounding.
//! - Owner input ([`Money::parse_user`]) is separate from the wire grammar and
//!   refuses anything ambiguous instead of guessing or rounding.

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Decimal places of every engine money, quantity and rate column.
pub const SCALE: u32 = 4;

/// Ten-thousandth units in one whole unit.
const UNITS_PER_WHOLE: i128 = 10_000;

/// Why a value is not a valid amount, or why arithmetic on it failed.
///
/// Messages never contain the value itself (no amounts in logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// Not in the engine's decimal string format (sign, digits, one point).
    #[error("not a decimal string in the engine format")]
    Format,
    /// More than four decimal places with a non-zero digit beyond the fourth.
    #[error("more than 4 decimal places")]
    TooPrecise,
    /// Outside the column's range.
    #[error("value is outside the allowed range")]
    OutOfRange,
    /// An arithmetic result left the column's range.
    #[error("arithmetic result is outside the allowed range")]
    Overflow,
    /// Allocation weights are empty or all zero, or a split into zero parts.
    #[error("allocation needs at least one non-zero weight")]
    BadWeights,
    /// The total cannot be split in the chosen unit (for example a sub-cent
    /// total split in cents).
    #[error("total is not a whole number of the allocation unit")]
    NotWholeUnit,
    /// A rounding scale other than 2 or 4 was requested.
    #[error("rounding scale must be 2 or 4")]
    BadScale,
}

/// Why owner input is not accepted as an amount. Nothing is ever rounded or
/// guessed: each case asks the owner to type it again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyInputError {
    /// Nothing typed.
    #[error("no amount entered")]
    Empty,
    /// Characters that are not part of an amount, or no digits at all.
    #[error("not an amount")]
    Invalid,
    /// Separators that could mean different things (`1,23`, `1.234,56`,
    /// `1 234`, two decimal points).
    #[error("ambiguous separators")]
    AmbiguousSeparator,
    /// More decimal places than allowed here.
    #[error("too many decimal places (at most {max})")]
    TooManyDecimals {
        /// The allowed number of decimal places.
        max: u32,
    },
    /// More integer digits than the column holds.
    #[error("amount is too large")]
    TooLarge,
    /// Both a minus sign and parentheses.
    #[error("both a minus sign and parentheses")]
    SignConflict,
}

/// Rules for owner input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserInput {
    max_scale: u32,
}

impl UserInput {
    /// Up to two decimals (ordinary money fields).
    pub const CENTS: UserInput = UserInput { max_scale: 2 };
    /// Up to four decimals (unit prices, quantities, rates).
    pub const FULL: UserInput = UserInput { max_scale: SCALE };

    /// The most decimal places accepted.
    #[must_use]
    pub const fn max_scale(self) -> u32 {
        self.max_scale
    }
}

/// How a negative amount is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NegativeStyle {
    /// `-1,234.50` (default).
    #[default]
    Minus,
    /// `(1,234.50)`.
    Parens,
}

/// Display options. Thousands separators are always shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoneyStyle {
    /// Minus sign or parentheses.
    pub negative: NegativeStyle,
    /// Prefix `$`.
    pub symbol: bool,
}

/// The unit an allocation splits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocUnit {
    /// Whole cents (the total must be a whole number of cents).
    Cent,
    /// Ten-thousandths (the engine's smallest unit).
    TenThousandth,
}

const fn pow10(n: u32) -> i128 {
    let mut v: i128 = 1;
    let mut i = 0;
    while i < n {
        v *= 10;
        i += 1;
    }
    v
}

/// The one place a `Decimal` is built from units: clears the sign of zero and
/// strips trailing zeros, so every stored value is canonical (S5).
fn decimal_from_units(units: i128) -> Result<Decimal, MoneyError> {
    let d = Decimal::try_from_i128_with_scale(units, SCALE).map_err(|_| MoneyError::OutOfRange)?;
    Ok(normalize(d))
}

/// Strip trailing zeros and clear the sign of a zero.
fn normalize(d: Decimal) -> Decimal {
    let mut n = d.normalize();
    if n.is_zero() {
        n.set_sign_positive(true);
    }
    n
}

/// Units of 0.0001 in a value whose scale is at most 4 (exact).
fn units_of(d: Decimal) -> i128 {
    let mut x = d;
    x.rescale(SCALE);
    x.mantissa()
}

/// Strict engine grammar `^-?\d{1,N}(\.\d{1,4})?$` to ten-thousandth units.
fn parse_wire_units(s: &str, int_digits: u32) -> Result<i128, MoneyError> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    if int.is_empty() || int.len() > int_digits as usize || !all_digits(int) {
        return Err(MoneyError::Format);
    }
    let frac = match frac {
        Some(f) if f.is_empty() || !all_digits(f) => return Err(MoneyError::Format),
        Some(f) if f.len() > SCALE as usize => return Err(MoneyError::TooPrecise),
        Some(f) => f,
        None => "",
    };
    let units = digits_to_units(int, frac);
    Ok(if neg { -units } else { units })
}

fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// `int` and `frac` are ASCII digits; `int` has at most 15 significant digits
/// and `frac` at most 4, so the result fits easily in an `i128`.
fn digits_to_units(int: &str, frac: &str) -> i128 {
    let mut units: i128 = 0;
    for b in int.bytes() {
        units = units * 10 + i128::from(b - b'0');
    }
    let mut f: i128 = 0;
    for b in frac.bytes() {
        f = f * 10 + i128::from(b - b'0');
    }
    // `frac.len() <= 4` is checked by every caller.
    let pad = SCALE.saturating_sub(u32::try_from(frac.len()).unwrap_or(SCALE));
    units * UNITS_PER_WHOLE + f * pow10(pad)
}

/// Owner input to ten-thousandth units (see [`Money::parse_user`]).
fn parse_user_units(s: &str, rules: UserInput, int_digits: u32) -> Result<i128, MoneyInputError> {
    let t = s.trim();
    if t.is_empty() {
        return Err(MoneyInputError::Empty);
    }
    let (paren, t) = if let Some(inner) = t.strip_prefix('(') {
        (
            true,
            inner.strip_suffix(')').ok_or(MoneyInputError::Invalid)?,
        )
    } else {
        (false, t)
    };
    // A minus sign and a dollar sign, in either order, each at most once.
    let mut neg = false;
    let mut dollar = false;
    let mut rest = t;
    loop {
        if let Some(r) = rest.strip_prefix('-') {
            if neg {
                return Err(MoneyInputError::Invalid);
            }
            neg = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix('$') {
            if dollar {
                return Err(MoneyInputError::Invalid);
            }
            dollar = true;
            rest = r;
        } else {
            break;
        }
    }
    if neg && paren {
        return Err(MoneyInputError::SignConflict);
    }
    if rest.is_empty() {
        return Err(MoneyInputError::Invalid);
    }
    let mut dots = 0;
    for c in rest.chars() {
        match c {
            '0'..='9' | ',' => {}
            '.' => dots += 1,
            c if c.is_whitespace() => return Err(MoneyInputError::AmbiguousSeparator),
            _ => return Err(MoneyInputError::Invalid),
        }
    }
    if dots > 1 {
        return Err(MoneyInputError::AmbiguousSeparator);
    }
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, f),
        None => (rest, ""),
    };
    if frac.contains(',') {
        return Err(MoneyInputError::AmbiguousSeparator);
    }
    let int_plain: String = if int.contains(',') {
        let mut groups = int.split(',');
        let first = groups.next().unwrap_or_default();
        // `0,123` is a European decimal comma, not a thousands group.
        if first.is_empty()
            || first.len() > 3
            || first.starts_with('0')
            || groups.clone().any(|g| g.len() != 3)
        {
            return Err(MoneyInputError::AmbiguousSeparator);
        }
        int.chars().filter(|c| *c != ',').collect()
    } else {
        int.to_string()
    };
    if int_plain.is_empty() && frac.is_empty() {
        return Err(MoneyInputError::Invalid);
    }
    let max = rules.max_scale.min(SCALE);
    if frac.len() > max as usize {
        return Err(MoneyInputError::TooManyDecimals { max });
    }
    let significant = int_plain.trim_start_matches('0');
    if significant.len() > int_digits as usize {
        return Err(MoneyInputError::TooLarge);
    }
    let units = digits_to_units(significant, frac);
    Ok(if neg || paren { -units } else { units })
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

/// Display without rounding: at least `min_decimals`, more only when those
/// digits are non-zero.
fn format_units(units: i128, min_decimals: usize, style: MoneyStyle) -> String {
    let abs = units.unsigned_abs();
    let per = UNITS_PER_WHOLE.unsigned_abs();
    let whole = abs / per;
    let mut frac = format!("{:04}", abs % per);
    while frac.len() > min_decimals && frac.ends_with('0') {
        frac.pop();
    }
    let mut body = String::new();
    if style.symbol {
        body.push('$');
    }
    body.push_str(&group_thousands(whole));
    if !frac.is_empty() {
        body.push('.');
        body.push_str(&frac);
    }
    if units < 0 {
        match style.negative {
            NegativeStyle::Minus => format!("-{body}"),
            NegativeStyle::Parens => format!("({body})"),
        }
    } else {
        body
    }
}

/// Canonical wire text: exactly four decimals.
fn wire_from_units(units: i128) -> String {
    let abs = units.unsigned_abs();
    let per = UNITS_PER_WHOLE.unsigned_abs();
    let sign = if units < 0 { "-" } else { "" };
    format!("{sign}{}.{:04}", abs / per, abs % per)
}

/// `a × b` where both are in ten-thousandths, rounded half away from zero to
/// `scale` (2 or 4) decimals, returned in ten-thousandths.
fn mul_round_units(a: i128, b: i128, scale: u32) -> Result<i128, MoneyError> {
    if scale != 2 && scale != SCALE {
        return Err(MoneyError::BadScale);
    }
    let prod = a.checked_mul(b).ok_or(MoneyError::Overflow)?; // scale 8
    let div = pow10(2 * SCALE - scale);
    let mut q = prod / div;
    let r = prod % div;
    if r.unsigned_abs() * 2 >= div.unsigned_abs() {
        q += prod.signum();
    }
    q.checked_mul(pow10(SCALE - scale))
        .ok_or(MoneyError::Overflow)
}

struct WireVisitor(&'static str);

impl serde::de::Visitor<'_> for WireVisitor {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a decimal string ({})", self.0)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<String, E> {
        Ok(v.to_string())
    }
}

macro_rules! fixed_decimal {
    (
        $(#[$doc:meta])*
        $name:ident, int_digits = $digits:expr, max = ($lo:expr, $mid:expr),
        max_units = $max_units:expr, min_display_decimals = $mind:expr
    ) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
        pub struct $name(Decimal);

        impl $name {
            /// Zero.
            pub const ZERO: $name = $name(Decimal::ZERO);
            /// The largest value the engine column holds.
            pub const MAX: $name = $name(Decimal::from_parts($lo, $mid, 0, false, SCALE));
            /// The smallest (most negative) value the engine column holds.
            pub const MIN: $name = $name(Decimal::from_parts($lo, $mid, 0, true, SCALE));
            /// Integer digits the engine column holds.
            pub const INTEGER_DIGITS: u32 = $digits;
            const MAX_UNITS: i128 = $max_units;

            /// Exact value from ten-thousandth units (`12_5000` is 12.5).
            ///
            /// # Errors
            /// [`MoneyError::OutOfRange`] outside the column's range.
            pub fn from_ten_thousandths(units: i128) -> Result<Self, MoneyError> {
                if units.unsigned_abs() > Self::MAX_UNITS.unsigned_abs() {
                    return Err(MoneyError::OutOfRange);
                }
                decimal_from_units(units).map(Self)
            }

            /// The exact value in ten-thousandth units.
            #[must_use]
            pub fn to_ten_thousandths(self) -> i128 {
                units_of(self.0)
            }

            /// Parse text received from the engine (strict grammar
            /// `^-?\d{1,N}(\.\d{1,4})?$`). `-0` becomes zero.
            ///
            /// # Errors
            /// [`MoneyError::Format`] for anything else (`+`, exponents,
            /// whitespace, commas, `NaN`, `Infinity`, too many digits);
            /// [`MoneyError::TooPrecise`] for five or more decimals.
            pub fn parse_wire(s: &str) -> Result<Self, MoneyError> {
                Self::from_ten_thousandths(parse_wire_units(s, Self::INTEGER_DIGITS)?)
            }

            /// Canonical text for the engine: optional `-`, digits, and
            /// exactly four decimals (`12.5000`). Never `-0.0000`.
            #[must_use]
            pub fn to_wire(self) -> String {
                wire_from_units(self.to_ten_thousandths())
            }

            /// Parse what the owner typed.
            ///
            /// Accepted: surrounding whitespace, an optional `$`, a leading
            /// `-` **or** wrapping parentheses, thousands commas in correct
            /// groups of three, and `.5` or `5.`.
            ///
            /// # Errors
            /// See [`MoneyInputError`]: ambiguous separators, more decimals
            /// than `rules` allow, too many integer digits, empty input or a
            /// bare sign. Nothing is ever rounded.
            pub fn parse_user(s: &str, rules: UserInput) -> Result<Self, MoneyInputError> {
                let units = parse_user_units(s, rules, Self::INTEGER_DIGITS)?;
                Self::from_ten_thousandths(units).map_err(|_| MoneyInputError::TooLarge)
            }

            /// Display text with thousands separators. Shows at least the
            /// type's minimum decimals and more only when those digits are
            /// non-zero, so a value is never rounded or hidden.
            #[must_use]
            pub fn format(self, style: MoneyStyle) -> String {
                format_units(self.to_ten_thousandths(), $mind, style)
            }

            /// `self + other`.
            ///
            /// # Errors
            /// [`MoneyError::Overflow`] when the result leaves the range.
            pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
                let u = self
                    .to_ten_thousandths()
                    .checked_add(other.to_ten_thousandths())
                    .ok_or(MoneyError::Overflow)?;
                Self::from_ten_thousandths(u).map_err(|_| MoneyError::Overflow)
            }

            /// `self - other`.
            ///
            /// # Errors
            /// [`MoneyError::Overflow`] when the result leaves the range.
            pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
                let u = self
                    .to_ten_thousandths()
                    .checked_sub(other.to_ten_thousandths())
                    .ok_or(MoneyError::Overflow)?;
                Self::from_ten_thousandths(u).map_err(|_| MoneyError::Overflow)
            }

            /// `-self`. The range is symmetric, so this only fails if the
            /// invariant was broken; zero stays unsigned.
            ///
            /// # Errors
            /// [`MoneyError::Overflow`] (not expected).
            pub fn checked_neg(self) -> Result<Self, MoneyError> {
                Self::from_ten_thousandths(-self.to_ten_thousandths())
                    .map_err(|_| MoneyError::Overflow)
            }

            /// The absolute value (cannot overflow: the range is symmetric).
            #[must_use]
            pub fn abs(self) -> Self {
                Self(normalize(self.0.abs()))
            }

            /// The exact sum of `values`.
            ///
            /// # Errors
            /// [`MoneyError::Overflow`] when a partial sum leaves the range.
            pub fn sum<I: IntoIterator<Item = Self>>(values: I) -> Result<Self, MoneyError> {
                values
                    .into_iter()
                    .try_fold(Self::ZERO, Self::checked_add)
            }

            /// True for zero.
            #[must_use]
            pub fn is_zero(self) -> bool {
                self.0.is_zero()
            }

            /// True below zero.
            #[must_use]
            pub fn is_negative(self) -> bool {
                self.0.is_sign_negative() && !self.0.is_zero()
            }

            /// True above zero.
            #[must_use]
            pub fn is_positive(self) -> bool {
                !self.0.is_sign_negative() && !self.0.is_zero()
            }

            /// `-1`, `0` or `1`.
            #[must_use]
            pub fn signum(self) -> i8 {
                if self.is_zero() {
                    0
                } else if self.is_negative() {
                    -1
                } else {
                    1
                }
            }

            /// The exact value as a `Decimal` (read-only view; scale ≤ 4,
            /// trailing zeros stripped).
            #[must_use]
            pub fn as_decimal(self) -> Decimal {
                self.0
            }
        }

        impl TryFrom<Decimal> for $name {
            type Error = MoneyError;

            /// Trailing zeros are stripped first, so `1.50000` is accepted as
            /// 1.5; a non-zero fifth decimal is refused, never rounded.
            fn try_from(d: Decimal) -> Result<Self, MoneyError> {
                let n = normalize(d);
                if n.scale() > SCALE {
                    return Err(MoneyError::TooPrecise);
                }
                let max = Decimal::from_parts($lo, $mid, 0, false, SCALE);
                if n.abs() > max {
                    return Err(MoneyError::OutOfRange);
                }
                Ok(Self(n))
            }
        }

        /// Prints the amount itself (`Money(12.3400)`): owner data. Never
        /// log a value with `?value` at info level or above.
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.to_wire())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&self.to_wire())
            }
        }

        /// Accepts only a JSON string in the engine grammar; a JSON number
        /// is an `invalid type` error.
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = d.deserialize_str(WireVisitor(stringify!($name)))?;
                Self::parse_wire(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

fixed_decimal!(
    /// An amount of money: engine `decimal(19,4)`, at most 15 integer digits
    /// and 4 decimals.
    ///
    /// There is deliberately no float constructor or accessor, no `Mul` or
    /// `Div`, and no `Display` (use [`Money::format`] for owners and
    /// [`Money::to_wire`] for the engine).
    Money,
    int_digits = 15,
    max = (0x89E7_FFFF, 0x8AC7_2304),
    max_units = 9_999_999_999_999_999_999,
    min_display_decimals = 2
);

fixed_decimal!(
    /// A quantity: engine `decimal(12,4)`, at most 8 integer digits.
    Quantity,
    int_digits = 8,
    max = (0xD4A5_0FFF, 0xE8),
    max_units = 999_999_999_999,
    min_display_decimals = 0
);

fixed_decimal!(
    /// A rate such as a sales tax rate: engine `decimal(5,4)`, one integer
    /// digit (`0.0825` is 8.25%).
    Rate,
    int_digits = 1,
    max = (0x0001_869F, 0),
    max_units = 99_999,
    min_display_decimals = 0
);

impl Money {
    /// Exact value from whole cents.
    ///
    /// # Errors
    /// [`MoneyError::OutOfRange`] outside the column's range.
    pub fn from_cents(cents: i64) -> Result<Money, MoneyError> {
        let units = i128::from(cents)
            .checked_mul(100)
            .ok_or(MoneyError::OutOfRange)?;
        Money::from_ten_thousandths(units)
    }

    /// `self × quantity`, rounded half away from zero (the `PostgreSQL`
    /// `numeric` rule) to `scale` decimals (2 or 4). For previews only; the
    /// engine's own result always replaces it.
    ///
    /// # Errors
    /// [`MoneyError::BadScale`] for another scale; [`MoneyError::Overflow`]
    /// when the result leaves the range.
    pub fn mul_round(self, quantity: Quantity, scale: u32) -> Result<Money, MoneyError> {
        let u = mul_round_units(
            self.to_ten_thousandths(),
            quantity.to_ten_thousandths(),
            scale,
        )?;
        Money::from_ten_thousandths(u).map_err(|_| MoneyError::Overflow)
    }

    /// `self × rate`, rounded like [`Money::mul_round`] (sales tax previews).
    ///
    /// # Errors
    /// As [`Money::mul_round`].
    pub fn mul_rate_round(self, rate: Rate, scale: u32) -> Result<Money, MoneyError> {
        let u = mul_round_units(self.to_ten_thousandths(), rate.to_ten_thousandths(), scale)?;
        Money::from_ten_thousandths(u).map_err(|_| MoneyError::Overflow)
    }

    /// Split `self` in proportion to `weights` (largest remainder method).
    ///
    /// - The parts sum **exactly** to `self`.
    /// - Each part is within one `unit` of its exact share.
    /// - Ties go to the lower index; a zero weight gets zero.
    /// - A negative total gives the negated split of its absolute value.
    ///
    /// # Errors
    /// [`MoneyError::BadWeights`] when the weights are empty or all zero;
    /// [`MoneyError::NotWholeUnit`] when `self` is not a whole number of
    /// `unit` (a sub-cent total split in cents).
    pub fn allocate(self, weights: &[u64], unit: AllocUnit) -> Result<Vec<Money>, MoneyError> {
        let step: i128 = match unit {
            AllocUnit::Cent => 100,
            AllocUnit::TenThousandth => 1,
        };
        let units = self.to_ten_thousandths();
        if units % step != 0 {
            return Err(MoneyError::NotWholeUnit);
        }
        let total = (units / step).unsigned_abs();
        let wsum = weights
            .iter()
            .try_fold(0u128, |acc, w| acc.checked_add(u128::from(*w)))
            .ok_or(MoneyError::Overflow)?;
        if wsum == 0 {
            return Err(MoneyError::BadWeights);
        }
        let mut parts = Vec::with_capacity(weights.len());
        let mut order = Vec::with_capacity(weights.len());
        let mut floor_sum: u128 = 0;
        for (i, w) in weights.iter().enumerate() {
            // total < 2^64 and w < 2^64, so the product fits in a u128.
            let p = total
                .checked_mul(u128::from(*w))
                .ok_or(MoneyError::Overflow)?;
            parts.push(p / wsum);
            order.push((p % wsum, i));
            floor_sum += p / wsum;
        }
        let leftover = usize::try_from(total - floor_sum).map_err(|_| MoneyError::Overflow)?;
        order.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, i) in order.into_iter().take(leftover) {
            if let Some(p) = parts.get_mut(i) {
                *p += 1;
            }
        }
        parts
            .into_iter()
            .map(|p| {
                let u = i128::try_from(p).map_err(|_| MoneyError::Overflow)? * step;
                Money::from_ten_thousandths(if units < 0 { -u } else { u })
            })
            .collect()
    }

    /// Split `self` into `n` parts as equal as possible (see
    /// [`Money::allocate`]; earlier parts get the extra units).
    ///
    /// # Errors
    /// [`MoneyError::BadWeights`] for `n == 0`; otherwise as
    /// [`Money::allocate`].
    pub fn split(self, n: usize, unit: AllocUnit) -> Result<Vec<Money>, MoneyError> {
        self.allocate(&vec![1; n], unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).unwrap()
    }

    #[test]
    fn bounds_are_the_engine_columns() {
        assert_eq!(Money::MAX.to_wire(), "999999999999999.9999");
        assert_eq!(Money::MIN.to_wire(), "-999999999999999.9999");
        assert_eq!(Quantity::MAX.to_wire(), "99999999.9999");
        assert_eq!(Rate::MAX.to_wire(), "9.9999");
        assert_eq!(Rate::MIN.to_wire(), "-9.9999");
        assert_eq!(Money::MAX.to_ten_thousandths(), Money::MAX_UNITS);
        assert_eq!(Quantity::MAX.to_ten_thousandths(), Quantity::MAX_UNITS);
        assert_eq!(Rate::MAX.to_ten_thousandths(), Rate::MAX_UNITS);
        assert_eq!(Money::ZERO.to_wire(), "0.0000");
        assert_eq!(Money::default(), Money::ZERO);
    }

    #[test]
    fn wire_accepts_the_grammar() {
        assert_eq!(m("12.5").to_wire(), "12.5000");
        assert_eq!(m("12").to_wire(), "12.0000");
        assert_eq!(m("-0.0001").to_wire(), "-0.0001");
        assert_eq!(m("0007.10").to_wire(), "7.1000");
        assert_eq!(m("-0").to_wire(), "0.0000");
        assert_eq!(m("-0.0000").to_wire(), "0.0000");
        assert_eq!(m("999999999999999.9999"), Money::MAX);
        assert_eq!(Quantity::parse_wire("99999999.9999"), Ok(Quantity::MAX));
        assert_eq!(Rate::parse_wire("0.0825").unwrap().to_wire(), "0.0825");
    }

    #[test]
    fn wire_rejects_everything_else() {
        for s in [
            "",
            "-",
            ".5",
            "5.",
            "+5",
            "1e3",
            "1E3",
            "NaN",
            "Infinity",
            "-Infinity",
            " 1",
            "1 ",
            "1,000",
            "1.2.3",
            "--1",
            "0x10",
            "1_000",
            "١",
            "1000000000000000",
            "$1",
        ] {
            assert_eq!(Money::parse_wire(s), Err(MoneyError::Format), "{s:?}");
        }
        assert_eq!(Money::parse_wire("1.23456"), Err(MoneyError::TooPrecise));
        assert_eq!(Rate::parse_wire("10.0"), Err(MoneyError::Format));
        assert_eq!(Quantity::parse_wire("123456789"), Err(MoneyError::Format));
    }

    #[test]
    fn minor_unit_constructors() {
        assert_eq!(Money::from_cents(1250).unwrap().to_wire(), "12.5000");
        assert_eq!(Money::from_cents(-1).unwrap().to_wire(), "-0.0100");
        assert_eq!(Money::from_cents(i64::MAX), Err(MoneyError::OutOfRange));
        assert_eq!(Money::from_ten_thousandths(1).unwrap().to_wire(), "0.0001");
        assert_eq!(
            Money::from_ten_thousandths(Money::MAX_UNITS + 1),
            Err(MoneyError::OutOfRange)
        );
        assert_eq!(
            Money::from_ten_thousandths(i128::MIN),
            Err(MoneyError::OutOfRange)
        );
    }

    #[test]
    fn zero_is_never_negative() {
        let a = m("12.3456");
        assert_eq!(a.checked_sub(a).unwrap().to_wire(), "0.0000");
        assert_eq!(Money::ZERO.checked_neg(), Ok(Money::ZERO));
        assert_eq!(Money::ZERO.checked_neg().unwrap().to_wire(), "0.0000");
        let neg_zero = Decimal::from_parts(0, 0, 0, true, 2);
        let z = Money::try_from(neg_zero).unwrap();
        assert_eq!(z.to_wire(), "0.0000");
        assert!(!z.is_negative());
        assert_eq!(z.signum(), 0);
        let tiny = m("-0.0001").mul_round(Quantity::parse_wire("0.0001").unwrap(), 4);
        assert_eq!(tiny.unwrap().to_wire(), "0.0000");
        assert_eq!(
            m("-0.0049")
                .mul_round(Quantity::parse_wire("1").unwrap(), 2)
                .unwrap()
                .to_wire(),
            "0.0000"
        );
    }

    #[test]
    fn try_from_decimal_strips_trailing_zeros() {
        let d = Decimal::from_i128_with_scale(150_000, 5); // 1.50000
        assert_eq!(Money::try_from(d).unwrap().to_wire(), "1.5000");
        let d = Decimal::from_i128_with_scale(100_001, 5); // 1.00001
        assert_eq!(Money::try_from(d), Err(MoneyError::TooPrecise));
        let big = Decimal::from_i128_with_scale(10i128.pow(15), 0);
        assert_eq!(Money::try_from(big), Err(MoneyError::OutOfRange));
        assert_eq!(Money::try_from(Money::MIN.as_decimal()), Ok(Money::MIN));
    }

    #[test]
    fn checked_arithmetic() {
        assert_eq!(m("1.25").checked_add(m("2.5")).unwrap().to_wire(), "3.7500");
        assert_eq!(m("1").checked_sub(m("2.5")).unwrap().to_wire(), "-1.5000");
        assert_eq!(
            Money::MAX.checked_add(m("0.0001")),
            Err(MoneyError::Overflow)
        );
        assert_eq!(
            Money::MIN.checked_sub(m("0.0001")),
            Err(MoneyError::Overflow)
        );
        assert_eq!(Money::MIN.checked_neg(), Ok(Money::MAX));
        assert_eq!(Money::MIN.abs(), Money::MAX);
        assert_eq!(Money::sum([m("0.1"), m("0.2")]).unwrap(), m("0.3"));
        assert_eq!(
            Money::sum([Money::MAX, Money::MAX]),
            Err(MoneyError::Overflow)
        );
        assert_eq!(Money::sum([]), Ok(Money::ZERO));
        assert_eq!(m("-3").signum(), -1);
        assert_eq!(m("3").signum(), 1);
        assert!(m("3").is_positive() && !m("0").is_positive() && !m("-3").is_positive());
    }

    #[test]
    fn mul_round_is_half_away_from_zero() {
        let q = |s| Quantity::parse_wire(s).unwrap();
        assert_eq!(m("0.125").mul_round(q("1"), 2).unwrap().to_wire(), "0.1300");
        assert_eq!(
            m("-0.125").mul_round(q("1"), 2).unwrap().to_wire(),
            "-0.1300"
        );
        assert_eq!(
            m("0.1249").mul_round(q("1"), 2).unwrap().to_wire(),
            "0.1200"
        );
        assert_eq!(
            m("19.99").mul_round(q("3"), 2).unwrap().to_wire(),
            "59.9700"
        );
        assert_eq!(
            m("1.0001").mul_round(q("0.5"), 4).unwrap().to_wire(),
            "0.5001"
        );
        assert_eq!(m("1").mul_round(q("1"), 3), Err(MoneyError::BadScale));
        assert_eq!(Money::MAX.mul_round(q("2"), 2), Err(MoneyError::Overflow));
        let tax = Rate::parse_wire("0.0825").unwrap();
        assert_eq!(m("100").mul_rate_round(tax, 2).unwrap().to_wire(), "8.2500");
        assert_eq!(
            m("10.10").mul_rate_round(tax, 2).unwrap().to_wire(),
            "0.8300"
        );
    }

    #[test]
    fn allocation_examples() {
        let parts = m("100").allocate(&[1, 1, 1], AllocUnit::Cent).unwrap();
        let w: Vec<String> = parts.iter().map(|p| p.to_wire()).collect();
        assert_eq!(w, ["33.3400", "33.3300", "33.3300"]);
        let parts = m("-100").split(3, AllocUnit::Cent).unwrap();
        let w: Vec<String> = parts.iter().map(|p| p.to_wire()).collect();
        assert_eq!(w, ["-33.3400", "-33.3300", "-33.3300"]);
        let parts = m("1")
            .allocate(&[0, 3, 1], AllocUnit::TenThousandth)
            .unwrap();
        let w: Vec<String> = parts.iter().map(|p| p.to_wire()).collect();
        assert_eq!(w, ["0.0000", "0.7500", "0.2500"]);
        assert_eq!(
            m("1").allocate(&[], AllocUnit::Cent),
            Err(MoneyError::BadWeights)
        );
        assert_eq!(
            m("1").allocate(&[0, 0], AllocUnit::Cent),
            Err(MoneyError::BadWeights)
        );
        assert_eq!(
            m("1").split(0, AllocUnit::Cent),
            Err(MoneyError::BadWeights)
        );
        assert_eq!(
            m("0.001").split(2, AllocUnit::Cent),
            Err(MoneyError::NotWholeUnit)
        );
        assert_eq!(
            Money::MAX
                .allocate(&[u64::MAX, u64::MAX], AllocUnit::TenThousandth)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn user_input_accepts_owner_forms() {
        let p = |s| Money::parse_user(s, UserInput::CENTS).map(Money::to_wire);
        assert_eq!(p("1,234.56"), Ok("1234.5600".into()));
        assert_eq!(p("  $1,234.5 "), Ok("1234.5000".into()));
        assert_eq!(p("-$12"), Ok("-12.0000".into()));
        assert_eq!(p("$-12"), Ok("-12.0000".into()));
        assert_eq!(p("(12.50)"), Ok("-12.5000".into()));
        assert_eq!(p("($1,000)"), Ok("-1000.0000".into()));
        assert_eq!(p(".5"), Ok("0.5000".into()));
        assert_eq!(p("5."), Ok("5.0000".into()));
        assert_eq!(p("-0"), Ok("0.0000".into()));
        assert_eq!(p("000123"), Ok("123.0000".into()));
        assert_eq!(
            Money::parse_user("0.0001", UserInput::FULL).map(Money::to_wire),
            Ok("0.0001".into())
        );
        assert_eq!(UserInput::FULL.max_scale(), 4);
    }

    #[test]
    fn user_input_rejects_ambiguity_and_never_rounds() {
        use MoneyInputError as E;
        let p = |s| Money::parse_user(s, UserInput::CENTS);
        assert_eq!(p(""), Err(E::Empty));
        assert_eq!(p("   "), Err(E::Empty));
        for s in [
            "-", "$", "-$", ".", "abc", "1e3", "+5", "(5", "5)", "--5", "$$5", "-(5)", "()",
        ] {
            assert_eq!(p(s), Err(E::Invalid), "{s:?}");
        }
        for s in [
            "1,23",
            "1.234,56",
            "1 234",
            "1.2.3",
            ",123",
            "1,2345",
            "12,34,567",
            "1,000.00,0",
            "0,123",
            "0,123.45",
            "-0,500",
            "01,234",
        ] {
            assert_eq!(p(s), Err(E::AmbiguousSeparator), "{s:?}");
        }
        assert_eq!(p("1.234"), Err(E::TooManyDecimals { max: 2 }));
        assert_eq!(
            Money::parse_user("1.23456", UserInput::FULL),
            Err(E::TooManyDecimals { max: 4 })
        );
        assert_eq!(p("(-5)"), Err(E::SignConflict));
        assert_eq!(p("1000000000000000"), Err(E::TooLarge));
        assert_eq!(
            p("999,999,999,999,999.99").map(Money::to_wire),
            Ok("999999999999999.9900".into())
        );
        assert_eq!(Rate::parse_user("10", UserInput::FULL), Err(E::TooLarge));
    }

    #[test]
    fn format_shows_every_digit() {
        let plain = MoneyStyle::default();
        let parens = MoneyStyle {
            negative: NegativeStyle::Parens,
            symbol: true,
        };
        assert_eq!(m("1234.5").format(plain), "1,234.50");
        assert_eq!(m("1234567.1234").format(plain), "1,234,567.1234");
        assert_eq!(m("0.001").format(plain), "0.001");
        assert_eq!(m("-12").format(plain), "-12.00");
        assert_eq!(m("-12").format(parens), "($12.00)");
        assert_eq!(m("999").format(parens), "$999.00");
        assert_eq!(Money::MIN.format(plain), "-999,999,999,999,999.9999");
        assert_eq!(Quantity::parse_wire("2").unwrap().format(plain), "2");
        assert_eq!(
            Quantity::parse_wire("1000.5").unwrap().format(plain),
            "1,000.5"
        );
        assert_eq!(Rate::parse_wire("0.0825").unwrap().format(plain), "0.0825");
    }

    #[test]
    fn serde_is_strings_only() {
        let a = m("12.5");
        assert_eq!(serde_json::to_string(&a).unwrap(), "\"12.5000\"");
        assert_eq!(serde_json::from_str::<Money>("\"12.50\"").unwrap(), a);
        let err = serde_json::from_str::<Money>("12.5")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid type"), "{err}");
        assert!(serde_json::from_str::<Money>("12").is_err());
        assert!(serde_json::from_str::<Money>("\"1e3\"").is_err());
        assert!(serde_json::from_str::<Money>("null").is_err());
        assert!(serde_json::from_str::<Rate>("\"0.08250\"").is_err());
        assert_eq!(format!("{a:?}"), "Money(12.5000)");
    }

    #[test]
    fn ordering_and_hash_follow_the_value() {
        use std::collections::HashSet;
        assert!(m("-1") < m("0.0001"));
        assert!(m("1.5") > m("1.4999"));
        let set: HashSet<Money> = [m("1.5"), m("1.50"), m("1.5000")].into_iter().collect();
        assert_eq!(set.len(), 1);
        assert_eq!(m("1.5").as_decimal().scale(), 1);
    }
}
