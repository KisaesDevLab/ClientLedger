//! Exact amount entry and the calculator (Q-12, P10 plan §2.5, Q-P10c).
//!
//! The owner types an amount (`1,234.56`, `-12`, `(12.00)`, `$5`) or a
//! calculation (`400+44.25`, `100/3`, `12 x 3`). Plain amounts go straight
//! to [`Money::parse_user`] (or the [`Quantity`]/[`Rate`] equivalent), so
//! every P05 refusal is kept. Calculations are evaluated with exact rational
//! numbers over whole ten-thousandths; a result that is not a whole number of
//! the field's smallest unit (a cent for ordinary money fields) is **refused,
//! never rounded**, and the two nearest values are offered instead.
//!
//! Grammar (standard precedence, left-associative):
//!
//! ```text
//! expr    := term (('+'|'-') term)*
//! term    := factor (('*'|'/') factor)*
//! factor  := '-'? operand
//! operand := number | '(' number ')'
//! ```
//!
//! `number` is anything [`Money::parse_user`] accepts with
//! [`UserInput::FULL`]. Parentheses only mean the accounting negative around
//! one number; grouping parentheses are refused.
//!
//! No floating point anywhere (this crate is in the `nofloat` scan). Error
//! values never carry the typed digits, so they are safe to log.

use crate::text::TextKey;
use cl_core::{Money, MoneyInputError, MoneyStyle, Quantity, Rate, UserInput};
use std::fmt;

/// Ten-thousandth units in one whole unit (cl-core's fixed scale).
const UNITS: i128 = 10_000;
/// cl-core's fixed number of decimals.
const SCALE: u32 = 4;
/// The longest calculation accepted, in characters.
pub const MAX_CHARS: usize = 256;
/// The most operators (`+ - * /` between operands) in one calculation.
pub const MAX_OPERATORS: usize = 32;

/// Which exact type a field holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AmountKind {
    /// [`Money`] (engine `decimal(19,4)`).
    Money,
    /// [`Quantity`] (engine `decimal(12,4)`).
    Quantity,
    /// [`Rate`] (engine `decimal(5,4)`).
    Rate,
}

/// Whether a field accepts negative values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sign {
    /// Any sign.
    Any,
    /// Zero or more (debit/credit and payment/deposit columns: a negative
    /// belongs in the other column).
    NonNegative,
}

/// The rules for one amount field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmountRules {
    /// The exact type.
    pub kind: AmountKind,
    /// The most decimals the owner may type (and the unit a calculation's
    /// result must be a whole number of).
    pub input: UserInput,
    /// Sign rule.
    pub sign: Sign,
}

impl AmountRules {
    /// An ordinary money field: cents, any sign.
    pub const MONEY: AmountRules = AmountRules {
        kind: AmountKind::Money,
        input: UserInput::CENTS,
        sign: Sign::Any,
    };
}

/// An exact value of one of the three amount types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Amount {
    /// Money.
    Money(Money),
    /// A quantity.
    Quantity(Quantity),
    /// A rate.
    Rate(Rate),
}

impl Amount {
    /// The exact value in ten-thousandth units.
    #[must_use]
    pub fn to_ten_thousandths(self) -> i128 {
        match self {
            Amount::Money(m) => m.to_ten_thousandths(),
            Amount::Quantity(q) => q.to_ten_thousandths(),
            Amount::Rate(r) => r.to_ten_thousandths(),
        }
    }

    /// Display text (exact; never rounded).
    #[must_use]
    pub fn format(self, style: MoneyStyle) -> String {
        match self {
            Amount::Money(m) => m.format(style),
            Amount::Quantity(q) => q.format(style),
            Amount::Rate(r) => r.format(style),
        }
    }

    /// Below zero.
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.to_ten_thousandths() < 0
    }

    /// The money value, when this is money.
    #[must_use]
    pub fn as_money(self) -> Option<Money> {
        match self {
            Amount::Money(m) => Some(m),
            _ => None,
        }
    }

    fn from_units(kind: AmountKind, units: i128) -> Option<Amount> {
        match kind {
            AmountKind::Money => Money::from_ten_thousandths(units).ok().map(Amount::Money),
            AmountKind::Quantity => Quantity::from_ten_thousandths(units)
                .ok()
                .map(Amount::Quantity),
            AmountKind::Rate => Rate::from_ten_thousandths(units).ok().map(Amount::Rate),
        }
    }
}

/// The two values nearest an inexact calculation result, and its value cut
/// short for the caption ("100 / 3 is 33.333…; type 33.33 or 33.34").
///
/// These are owner data: `Debug` prints none of them, so an error that
/// reaches a log line carries no amounts. Show them only on screen.
#[derive(Clone, PartialEq, Eq)]
pub struct Neighbours {
    below: Amount,
    above: Amount,
    approx: String,
}

impl Neighbours {
    /// The nearest allowed value below the result.
    #[must_use]
    pub fn below(&self) -> Amount {
        self.below
    }

    /// The nearest allowed value above the result.
    #[must_use]
    pub fn above(&self) -> Amount {
        self.above
    }

    /// The result truncated to one decimal more than the field allows,
    /// followed by `…` (`33.333…`). Never used as a value.
    #[must_use]
    pub fn approx(&self) -> &str {
        &self.approx
    }
}

impl fmt::Debug for Neighbours {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Neighbours(..)")
    }
}

/// Why typed text is not accepted as an amount. Nothing is ever rounded or
/// guessed; each case asks the owner to type it again. `Debug` and `Display`
/// never contain the typed digits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AmountEntryError {
    /// An amount (or one number in a calculation) was refused by the P05
    /// owner-input rules.
    #[error("{0}")]
    Input(MoneyInputError),
    /// Parentheses used for grouping (`(5+3)*2`, nested or around an
    /// expression). Parentheses only mark one negative number.
    #[error("parentheses only mark a negative number")]
    NoGrouping,
    /// Division by zero.
    #[error("cannot divide by zero")]
    DivideByZero,
    /// The calculation is not finished (`400+`).
    #[error("the calculation is not finished")]
    Incomplete,
    /// Too many characters or operators.
    #[error("the calculation is too long")]
    TooLong,
    /// The result (or a step of the calculation) is outside the field's range.
    #[error("the result is too large for this field")]
    TooLarge,
    /// The result is not a whole number of the field's smallest unit.
    #[error("the result is not a whole number of cents")]
    NotExact(Neighbours),
    /// A negative value in a column that accepts only zero or more.
    #[error("negative amounts go in the other column")]
    Negative,
}

impl AmountEntryError {
    /// The i18n key of the owner message.
    #[must_use]
    pub fn key(&self) -> TextKey {
        match self {
            AmountEntryError::Input(e) => match e {
                MoneyInputError::Empty => TextKey("grid.amount.empty"),
                MoneyInputError::Invalid => TextKey("grid.amount.invalid"),
                MoneyInputError::AmbiguousSeparator => TextKey("grid.amount.ambiguous"),
                MoneyInputError::TooManyDecimals { .. } => TextKey("grid.amount.too_many_decimals"),
                MoneyInputError::TooLarge => TextKey("grid.amount.too_large"),
                MoneyInputError::SignConflict => TextKey("grid.amount.sign_conflict"),
            },
            AmountEntryError::NoGrouping => TextKey("grid.calc.no_grouping"),
            AmountEntryError::DivideByZero => TextKey("grid.calc.divide_by_zero"),
            AmountEntryError::Incomplete => TextKey("grid.calc.incomplete"),
            AmountEntryError::TooLong => TextKey("grid.calc.too_long"),
            AmountEntryError::TooLarge => TextKey("grid.amount.too_large"),
            AmountEntryError::NotExact(_) => TextKey("grid.calc.not_exact"),
            AmountEntryError::Negative => TextKey("grid.amount.no_negative"),
        }
    }

    /// The i18n key of the suggested fix, when there is one.
    #[must_use]
    pub fn fix_key(&self) -> Option<TextKey> {
        match self {
            AmountEntryError::Input(MoneyInputError::AmbiguousSeparator) => {
                Some(TextKey("grid.amount.ambiguous.fix"))
            }
            AmountEntryError::Input(MoneyInputError::TooManyDecimals { .. }) => {
                Some(TextKey("grid.amount.too_many_decimals.fix"))
            }
            AmountEntryError::NoGrouping => Some(TextKey("grid.calc.no_grouping.fix")),
            AmountEntryError::NotExact(_) => Some(TextKey("grid.calc.not_exact.fix")),
            AmountEntryError::Negative => Some(TextKey("grid.amount.no_negative.fix")),
            _ => None,
        }
    }
}

impl From<MoneyInputError> for AmountEntryError {
    fn from(e: MoneyInputError) -> Self {
        AmountEntryError::Input(e)
    }
}

/// An accepted entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Parsed {
    /// The exact value.
    pub value: Amount,
    /// `true` when a calculation was evaluated (the grid reports
    /// `Source::Calculator`), `false` for a plain amount.
    pub calculated: bool,
}

/// What the live caption under an amount cell shows while the owner types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// Nothing: a plain amount, empty text or an unfinished calculation.
    None,
    /// `= 444.25`.
    Value(Amount),
    /// The calculation cannot be accepted as typed.
    Error(AmountEntryError),
}

/// The first field of pasted text: up to the first tab or line break,
/// trimmed (Excel copies end in `\r\n`; Q-P10j).
#[must_use]
pub fn first_pasted_field(text: &str) -> &str {
    text.split(['\t', '\r', '\n']).next().unwrap_or("").trim()
}

/// Is `c` an operand character next to which `x`/`X`/`×` means "times"?
fn ends_operand(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ')'
}

fn starts_operand(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '$' | '(' | '-')
}

/// Normalize typed text before parsing: trim Unicode whitespace (NBSP
/// included) at both ends, U+2212 MINUS SIGN → `-`, `÷` → `/`, and `×`, `x`
/// or `X` between two operands → `*`. Nothing else is rewritten; internal
/// spaces inside a number stay ambiguous (`1 234` is refused).
#[must_use]
pub fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    let mut out = String::with_capacity(chars.len());
    for (i, &c) in chars.iter().enumerate() {
        let mapped = match c {
            '\u{2212}' => '-',
            '÷' => '/',
            'x' | 'X' | '×' => {
                let prev = chars[..i].iter().rev().find(|p| !p.is_whitespace());
                let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
                if prev.is_some_and(|p| ends_operand(*p))
                    && next.is_some_and(|n| starts_operand(*n))
                {
                    '*'
                } else {
                    c
                }
            }
            other => other,
        };
        out.push(mapped);
    }
    out
}

/// Does normalized text contain a calculation (an operator after the first
/// number started)? A `-` before the first digit is the sign of a plain
/// amount (`-5`, `$-5`, `-$5`).
fn is_expression(t: &str) -> bool {
    let mut seen_number = false;
    for c in t.chars() {
        match c {
            '+' | '*' | '/' => return true,
            '-' if seen_number => return true,
            '0'..='9' | '.' => seen_number = true,
            _ => {}
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
}

/// An exact rational number of ten-thousandth units: `n / d`, `d > 0`,
/// reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ratio {
    n: i128,
    d: i128,
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.unsigned_abs(), b.unsigned_abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    // Both inputs fit in i128 and the gcd is at most the smaller magnitude;
    // gcd(i128::MIN, 0) would be 2^127, which callers never produce because
    // every n and d here came from checked arithmetic.
    i128::try_from(a).unwrap_or(1)
}

type CalcResult<T> = Result<T, AmountEntryError>;

fn over<T>(v: Option<T>) -> CalcResult<T> {
    v.ok_or(AmountEntryError::TooLarge)
}

impl Ratio {
    fn units(n: i128) -> Ratio {
        Ratio { n, d: 1 }
    }

    fn new(n: i128, d: i128) -> CalcResult<Ratio> {
        if d == 0 {
            return Err(AmountEntryError::DivideByZero);
        }
        let (n, d) = if d < 0 {
            (over(n.checked_neg())?, over(d.checked_neg())?)
        } else {
            (n, d)
        };
        let g = gcd(n, d);
        Ok(Ratio { n: n / g, d: d / g })
    }

    fn neg(self) -> CalcResult<Ratio> {
        Ok(Ratio {
            n: over(self.n.checked_neg())?,
            d: self.d,
        })
    }

    fn add(self, o: Ratio) -> CalcResult<Ratio> {
        let g = gcd(self.d, o.d);
        let den = over((self.d / g).checked_mul(o.d))?;
        let left = over(self.n.checked_mul(o.d / g))?;
        let right = over(o.n.checked_mul(self.d / g))?;
        Ratio::new(over(left.checked_add(right))?, den)
    }

    fn sub(self, o: Ratio) -> CalcResult<Ratio> {
        self.add(o.neg()?)
    }

    /// `x * y` in units: `(a/b)·(c/d) / 10⁴`.
    fn mul(self, o: Ratio) -> CalcResult<Ratio> {
        let g1 = gcd(self.n, o.d);
        let g2 = gcd(o.n, self.d);
        let n = over((self.n / g1).checked_mul(o.n / g2))?;
        let d = over(
            (self.d / g2)
                .checked_mul(o.d / g1)
                .and_then(|d| d.checked_mul(UNITS)),
        )?;
        Ratio::new(n, d)
    }

    /// `x / y` in units: `(a/b) / (c/d) · 10⁴`.
    fn div(self, o: Ratio) -> CalcResult<Ratio> {
        if o.n == 0 {
            return Err(AmountEntryError::DivideByZero);
        }
        let g1 = gcd(self.n, o.n);
        let g2 = gcd(self.d, o.d);
        let n = over(
            (self.n / g1)
                .checked_mul(o.d / g2)
                .and_then(|n| n.checked_mul(UNITS)),
        )?;
        let d = over((self.d / g2).checked_mul(o.n / g1))?;
        Ratio::new(n, d)
    }

    fn apply(self, op: Op, o: Ratio) -> CalcResult<Ratio> {
        match op {
            Op::Add => self.add(o),
            Op::Sub => self.sub(o),
            Op::Mul => self.mul(o),
            Op::Div => self.div(o),
        }
    }
}

/// One factor: an optional unary minus and an operand's units.
fn operand_units(text: &str) -> CalcResult<i128> {
    Ok(Money::parse_user(text, UserInput::FULL)?.to_ten_thousandths())
}

/// Split normalized text into factors and the operators between them.
fn tokenize(t: &str) -> CalcResult<(Vec<Ratio>, Vec<Op>)> {
    let chars: Vec<char> = t.chars().collect();
    let mut factors = Vec::new();
    let mut ops = Vec::new();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    loop {
        // Operand, with an optional unary minus.
        skip_ws(&mut i);
        let mut negative = false;
        if i < chars.len() && chars[i] == '-' {
            negative = true;
            i += 1;
            skip_ws(&mut i);
        }
        if i >= chars.len() {
            return Err(AmountEntryError::Incomplete);
        }
        let text: String = if chars[i] == '(' {
            let close = chars[i + 1..].iter().position(|c| *c == ')');
            let Some(len) = close else {
                // Still typing, or a grouping attempt: refuse grouping if an
                // operator or another parenthesis follows.
                let rest: String = chars[i + 1..].iter().collect();
                return Err(
                    if rest.contains(['(', '+', '*', '/']) || is_expression(&rest) {
                        AmountEntryError::NoGrouping
                    } else {
                        AmountEntryError::Incomplete
                    },
                );
            };
            let inner: String = chars[i + 1..i + 1 + len].iter().collect();
            if inner.contains(['(', '+', '*', '/']) || is_expression(&inner) {
                return Err(AmountEntryError::NoGrouping);
            }
            i += len + 2;
            format!("({inner})")
        } else {
            let start = i;
            while i < chars.len() && !matches!(chars[i], '+' | '-' | '*' | '/' | '(' | ')') {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            let run = run.trim().to_string();
            if run.is_empty() {
                // Two operators in a row (`5+*3`) or a stray parenthesis.
                return Err(AmountEntryError::Input(MoneyInputError::Invalid));
            }
            run
        };
        let mut r = Ratio::units(operand_units(&text)?);
        if negative {
            r = r.neg()?;
        }
        factors.push(r);
        // Operator, or the end.
        skip_ws(&mut i);
        if i >= chars.len() {
            return Ok((factors, ops));
        }
        let op = match chars[i] {
            '+' => Op::Add,
            '-' => Op::Sub,
            '*' => Op::Mul,
            '/' => Op::Div,
            '(' | ')' => return Err(AmountEntryError::NoGrouping),
            _ => return Err(AmountEntryError::Input(MoneyInputError::Invalid)),
        };
        ops.push(op);
        if ops.len() > MAX_OPERATORS {
            return Err(AmountEntryError::TooLong);
        }
        i += 1;
    }
}

/// Evaluate with `*` and `/` before `+` and `-`, left to right.
fn evaluate(factors: &[Ratio], ops: &[Op]) -> CalcResult<Ratio> {
    let mut terms: Vec<Ratio> = Vec::with_capacity(factors.len());
    let mut signs: Vec<Op> = Vec::new();
    let mut acc = factors[0];
    for (op, f) in ops.iter().zip(&factors[1..]) {
        match op {
            Op::Mul | Op::Div => acc = acc.apply(*op, *f)?,
            Op::Add | Op::Sub => {
                terms.push(acc);
                signs.push(*op);
                acc = *f;
            }
        }
    }
    terms.push(acc);
    let mut total = terms[0];
    for (op, t) in signs.iter().zip(&terms[1..]) {
        total = total.apply(*op, *t)?;
    }
    Ok(total)
}

const fn pow10(n: u32) -> i128 {
    let mut v: i128 = 1;
    let mut k = 0;
    while k < n {
        v *= 10;
        k += 1;
    }
    v
}

/// Floor division for a positive divisor.
fn floor_div(n: i128, d: i128) -> i128 {
    let q = n / d;
    if n % d != 0 && n < 0 { q - 1 } else { q }
}

/// `1234567` → `1,234,567`.
fn group(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The value `r` (in units) truncated toward zero to `decimals` places,
/// with thousands separators and a trailing `…`.
fn approx_text(r: Ratio, decimals: u32) -> String {
    let neg = r.n < 0;
    let n = r.n.unsigned_abs();
    let d = r.d.unsigned_abs();
    let per = UNITS.unsigned_abs();
    let whole_units = n / d;
    let mut rem = n % d;
    let whole = whole_units / per;
    let mut frac = format!("{:04}", whole_units % per);
    let mut k = SCALE;
    while k < decimals {
        let Some(r10) = rem.checked_mul(10) else {
            break;
        };
        let digit = r10 / d;
        rem = r10 % d;
        frac.push(char::from_digit(u32::try_from(digit).unwrap_or(0), 10).unwrap_or('0'));
        k += 1;
    }
    frac.truncate(decimals as usize);
    let sign = if neg { "-" } else { "" };
    format!("{sign}{}.{frac}…", group(&whole.to_string()))
}

/// Turn an exact result into a value of the field's type, or refuse it.
fn finish(r: Ratio, rules: AmountRules) -> CalcResult<Amount> {
    let max_scale = rules.input.max_scale().min(SCALE);
    let step = pow10(SCALE - max_scale);
    let floor_units = floor_div(r.n, r.d);
    let exact_units = (r.n % r.d == 0).then_some(floor_units);
    if let Some(u) = exact_units
        && u % step == 0
    {
        let value = Amount::from_units(rules.kind, u).ok_or(AmountEntryError::TooLarge)?;
        return check_sign(value, rules.sign);
    }
    // Not a whole number of the field's unit: offer the two nearest values.
    // When either neighbour is outside the type's range (the result is at or
    // beyond the edge), the honest answer is "too large".
    let lo = floor_div(floor_units, step) * step;
    let hi = over(lo.checked_add(step))?;
    let below = Amount::from_units(rules.kind, lo);
    let above = Amount::from_units(rules.kind, hi);
    match (below, above) {
        (Some(below), Some(above)) => {
            if rules.sign == Sign::NonNegative && r.n < 0 {
                return Err(AmountEntryError::Negative);
            }
            Err(AmountEntryError::NotExact(Neighbours {
                below,
                above,
                approx: approx_text(r, max_scale + 1),
            }))
        }
        _ => Err(AmountEntryError::TooLarge),
    }
}

fn check_sign(value: Amount, sign: Sign) -> CalcResult<Amount> {
    if sign == Sign::NonNegative && value.is_negative() {
        Err(AmountEntryError::Negative)
    } else {
        Ok(value)
    }
}

fn parse_plain(t: &str, rules: AmountRules) -> CalcResult<Amount> {
    let value = match rules.kind {
        AmountKind::Money => Amount::Money(Money::parse_user(t, rules.input)?),
        AmountKind::Quantity => Amount::Quantity(Quantity::parse_user(t, rules.input)?),
        AmountKind::Rate => Amount::Rate(Rate::parse_user(t, rules.input)?),
    };
    check_sign(value, rules.sign)
}

/// Parse what the owner typed into an amount field: a plain amount or a
/// calculation.
///
/// # Errors
/// See [`AmountEntryError`]. Plain amounts keep every [`MoneyInputError`]
/// refusal; calculations are refused when unfinished, too long, grouped with
/// parentheses, divided by zero, too large or not exact.
pub fn parse(text: &str, rules: AmountRules) -> Result<Parsed, AmountEntryError> {
    let t = normalize(text);
    if t.chars().count() > MAX_CHARS {
        return Err(AmountEntryError::TooLong);
    }
    if !is_expression(&t) {
        return Ok(Parsed {
            value: parse_plain(&t, rules)?,
            calculated: false,
        });
    }
    let (factors, ops) = tokenize(&t)?;
    let r = evaluate(&factors, &ops)?;
    Ok(Parsed {
        value: finish(r, rules)?,
        calculated: !ops.is_empty(),
    })
}

/// The live caption while typing: the result of a calculation, its error,
/// or nothing for plain amounts and unfinished calculations.
#[must_use]
pub fn preview(text: &str, rules: AmountRules) -> Preview {
    let t = normalize(text);
    if !is_expression(&t) {
        return Preview::None;
    }
    match parse(text, rules) {
        Ok(p) => Preview::Value(p.value),
        Err(AmountEntryError::Incomplete) => Preview::None,
        Err(e) => Preview::Error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn money(text: &str) -> Result<Parsed, AmountEntryError> {
        parse(text, AmountRules::MONEY)
    }

    fn wire(text: &str) -> String {
        money(text).unwrap().value.as_money().unwrap().to_wire()
    }

    #[test]
    fn plain_amounts_use_the_p05_rules() {
        assert_eq!(wire("1,234.56"), "1234.5600");
        assert_eq!(wire("$-5"), "-5.0000");
        assert_eq!(wire("-$5"), "-5.0000");
        assert_eq!(wire("(12.00)"), "-12.0000");
        assert!(!money("12").unwrap().calculated);
        assert_eq!(
            money("1 234"),
            Err(AmountEntryError::Input(MoneyInputError::AmbiguousSeparator))
        );
        assert_eq!(
            money("1.234,56"),
            Err(AmountEntryError::Input(MoneyInputError::AmbiguousSeparator))
        );
        assert_eq!(
            money("1.234"),
            Err(AmountEntryError::Input(MoneyInputError::TooManyDecimals {
                max: 2
            }))
        );
        assert_eq!(
            money(""),
            Err(AmountEntryError::Input(MoneyInputError::Empty))
        );
        assert_eq!(
            money("abc"),
            Err(AmountEntryError::Input(MoneyInputError::Invalid))
        );
    }

    #[test]
    fn calculations_are_exact() {
        assert_eq!(wire("400+44.25"), "444.2500");
        assert!(money("400+44.25").unwrap().calculated);
        assert_eq!(wire("400 + 44.25"), "444.2500");
        assert_eq!(wire("2+3*4"), "14.0000");
        assert_eq!(wire("10-2-3"), "5.0000");
        assert_eq!(wire("100/4/5"), "5.0000");
        assert_eq!(wire("12 x 3"), "36.0000");
        assert_eq!(wire("12×3"), "36.0000");
        assert_eq!(wire("12÷3"), "4.0000");
        assert_eq!(wire("5*-3"), "-15.0000");
        assert_eq!(wire("5--3"), "8.0000");
        assert_eq!(wire("(5)+1"), "-4.0000");
        assert_eq!(wire("$1,000.00+$5"), "1005.0000");
        assert_eq!(wire("1.005*2"), "2.0100");
        // 1/3 + 2/3 is exactly 1: rationals, no intermediate rounding.
        assert_eq!(wire("1/3+2/3"), "1.0000");
    }

    #[test]
    fn inexact_results_are_refused_with_neighbours() {
        let Err(AmountEntryError::NotExact(n)) = money("100/3") else {
            panic!("expected NotExact");
        };
        assert_eq!(n.below().as_money().unwrap().to_wire(), "33.3300");
        assert_eq!(n.above().as_money().unwrap().to_wire(), "33.3400");
        assert_eq!(n.approx(), "33.333…");
        let Err(AmountEntryError::NotExact(n)) = money("-100/3") else {
            panic!("expected NotExact");
        };
        assert_eq!(n.below().as_money().unwrap().to_wire(), "-33.3400");
        assert_eq!(n.above().as_money().unwrap().to_wire(), "-33.3300");
        assert_eq!(n.approx(), "-33.333…");
        assert_eq!(format!("{n:?}"), "Neighbours(..)");
        // A four-decimal field accepts what a cents field refuses.
        let full = AmountRules {
            input: UserInput::FULL,
            ..AmountRules::MONEY
        };
        assert_eq!(
            parse("1.005*2", full).unwrap().value.to_ten_thousandths(),
            2_0100
        );
        let Err(AmountEntryError::NotExact(n)) = parse("1/3", full) else {
            panic!("expected NotExact");
        };
        assert_eq!(n.approx(), "0.33333…");
        assert_eq!(
            parse("1234567/3", AmountRules::MONEY).unwrap_err().key(),
            TextKey("grid.calc.not_exact")
        );
    }

    #[test]
    fn calculator_refusals() {
        assert_eq!(money("(5+3)*2"), Err(AmountEntryError::NoGrouping));
        assert_eq!(money("2*(5+3)"), Err(AmountEntryError::NoGrouping));
        assert_eq!(money("((5))+1"), Err(AmountEntryError::NoGrouping));
        assert_eq!(money("5(3)+1"), Err(AmountEntryError::NoGrouping));
        assert_eq!(money("5/0"), Err(AmountEntryError::DivideByZero));
        assert_eq!(money("5/(0)"), Err(AmountEntryError::DivideByZero));
        assert_eq!(money("5/0.00"), Err(AmountEntryError::DivideByZero));
        assert_eq!(money("400+"), Err(AmountEntryError::Incomplete));
        assert_eq!(money("400*-"), Err(AmountEntryError::Incomplete));
        assert_eq!(money("5+(3"), Err(AmountEntryError::Incomplete));
        assert_eq!(
            money("5+*3"),
            Err(AmountEntryError::Input(MoneyInputError::Invalid))
        );
        assert_eq!(
            money("5+1,23"),
            Err(AmountEntryError::Input(MoneyInputError::AmbiguousSeparator))
        );
        assert_eq!(
            money(&"1+".repeat(33).add_one()),
            Err(AmountEntryError::TooLong)
        );
        assert_eq!(wire(&"1+".repeat(32).add_one()), "33.0000");
        assert_eq!(money(&"1".repeat(257)), Err(AmountEntryError::TooLong));
    }

    trait AddOne {
        fn add_one(self) -> String;
    }
    impl AddOne for String {
        fn add_one(mut self) -> String {
            self.push('1');
            self
        }
    }

    #[test]
    fn range_and_sign() {
        assert_eq!(
            money("999,999,999,999,999.99+0.01"),
            Err(AmountEntryError::TooLarge)
        );
        assert_eq!(
            money("999,999,999,999,999*999,999,999,999,999*999,999,999"),
            Err(AmountEntryError::TooLarge)
        );
        let debit = AmountRules {
            sign: Sign::NonNegative,
            ..AmountRules::MONEY
        };
        assert_eq!(parse("-5", debit), Err(AmountEntryError::Negative));
        assert_eq!(parse("5-6", debit), Err(AmountEntryError::Negative));
        assert_eq!(parse("-1/3", debit), Err(AmountEntryError::Negative));
        assert_eq!(
            parse("-5", debit).unwrap_err().fix_key(),
            Some(TextKey("grid.amount.no_negative.fix"))
        );
        assert!(parse("5-5", debit).is_ok());
    }

    #[test]
    fn zero_never_has_a_sign() {
        for t in ["-0", "(0)", "5-5", "-0.00", "0*-3", "-5+5", "(0)/7"] {
            assert_eq!(wire(t), "0.0000", "{t}");
        }
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("\u{a0} 12 \u{a0}"), "12");
        assert_eq!(wire("\u{2212}5"), "-5.0000");
        assert_eq!(wire("10\u{2212}2"), "8.0000");
        assert_eq!(wire("\u{a0}400+44.25\u{a0}"), "444.2500");
        assert_eq!(
            money("1\u{a0}234"),
            Err(AmountEntryError::Input(MoneyInputError::AmbiguousSeparator))
        );
        assert_eq!(normalize("tax"), "tax");
        assert_eq!(normalize("5x"), "5x");
        assert_eq!(normalize("5 X 2"), "5 * 2");
        assert_eq!(first_pasted_field(" 12.50\t3\r\n"), "12.50");
        assert_eq!(first_pasted_field("12.50\r\n"), "12.50");
        assert_eq!(first_pasted_field(""), "");
    }

    #[test]
    fn quantities_and_rates() {
        let qty = AmountRules {
            kind: AmountKind::Quantity,
            input: UserInput::FULL,
            sign: Sign::Any,
        };
        let rate = AmountRules {
            kind: AmountKind::Rate,
            ..qty
        };
        assert_eq!(
            parse("1.5*3", qty).unwrap().value,
            Amount::Quantity(Quantity::parse_wire("4.5").unwrap())
        );
        assert_eq!(
            parse("8.25/100", rate).unwrap().value,
            Amount::Rate(Rate::parse_wire("0.0825").unwrap())
        );
        assert_eq!(
            parse("10", rate),
            Err(AmountEntryError::Input(MoneyInputError::TooLarge))
        );
        assert_eq!(parse("5+5", rate), Err(AmountEntryError::TooLarge));
        assert_eq!(
            parse("123456789", qty),
            Err(AmountEntryError::Input(MoneyInputError::TooLarge))
        );
    }

    #[test]
    fn preview_states() {
        let r = AmountRules::MONEY;
        assert_eq!(preview("12", r), Preview::None);
        assert_eq!(preview("", r), Preview::None);
        assert_eq!(preview("400+", r), Preview::None);
        assert_eq!(
            preview("400+44.25", r),
            Preview::Value(Amount::Money(Money::parse_wire("444.25").unwrap()))
        );
        assert!(matches!(
            preview("100/3", r),
            Preview::Error(AmountEntryError::NotExact(_))
        ));
        assert_eq!(
            preview("1/0", r),
            Preview::Error(AmountEntryError::DivideByZero)
        );
    }

    #[test]
    fn every_error_has_a_grid_key() {
        let errors = [
            AmountEntryError::Input(MoneyInputError::Empty),
            AmountEntryError::Input(MoneyInputError::Invalid),
            AmountEntryError::Input(MoneyInputError::AmbiguousSeparator),
            AmountEntryError::Input(MoneyInputError::TooManyDecimals { max: 2 }),
            AmountEntryError::Input(MoneyInputError::TooLarge),
            AmountEntryError::Input(MoneyInputError::SignConflict),
            AmountEntryError::NoGrouping,
            AmountEntryError::DivideByZero,
            AmountEntryError::Incomplete,
            AmountEntryError::TooLong,
            AmountEntryError::TooLarge,
            AmountEntryError::Negative,
        ];
        let table = [
            ("grid.amount.empty", None),
            ("grid.amount.invalid", None),
            ("grid.amount.ambiguous", Some("grid.amount.ambiguous.fix")),
            (
                "grid.amount.too_many_decimals",
                Some("grid.amount.too_many_decimals.fix"),
            ),
            ("grid.amount.too_large", None),
            ("grid.amount.sign_conflict", None),
            ("grid.calc.no_grouping", Some("grid.calc.no_grouping.fix")),
            ("grid.calc.divide_by_zero", None),
            ("grid.calc.incomplete", None),
            ("grid.calc.too_long", None),
            ("grid.amount.too_large", None),
            (
                "grid.amount.no_negative",
                Some("grid.amount.no_negative.fix"),
            ),
        ];
        for (e, (key, fix)) in errors.iter().zip(table) {
            assert_eq!(e.key(), TextKey(key), "{e:?}");
            assert_eq!(e.fix_key(), fix.map(TextKey), "{e:?}");
        }
    }
}
