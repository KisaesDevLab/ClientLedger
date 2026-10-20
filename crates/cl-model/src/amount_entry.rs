//! Exact amount entry and the calculator (Q-12, P10 plan §2.5, Q-P10c).
//!
//! The owner types an amount (`1,234.56`, `-12`, `(12.00)`, `$5`) or a
//! calculation (`400+44.25`, `100/3`, `12 x 3`, `400+44.25=`). The text is
//! first tried as a plain amount with [`Money::parse_user`] (or the
//! [`Quantity`]/[`Rate`] equivalent), so every P05 rule is kept (`$-5` is
//! -5.00). Only when that fails and an operator follows a number is it a
//! calculation (P10 plan Rev 2 §2.6). Calculations are evaluated with exact rational
//! numbers over whole ten-thousandths; only the final result is rounded,
//! once, to the field's smallest unit (a cent for ordinary money fields),
//! half away from zero: `100/3` is 33.33, `33.335*1` is 33.34 and
//! `-33.335*1` is -33.34 (owner decision 2026-10-18, Q-P10x). A plain typed
//! money amount follows the same rule (Q-232, [`UserInput::MONEY`]): `12.345`
//! in a cents field is 12.35, with no hint (Q-237). Quantity and rate fields
//! still refuse extra decimals.
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
    /// An ordinary money field: cents (more decimals round, Q-232), any sign.
    pub const MONEY: AmountRules = AmountRules {
        kind: AmountKind::Money,
        input: UserInput::MONEY,
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

/// Why typed text is not accepted as an amount. Nothing is guessed; each
/// case asks the owner to type it again. `Debug` is written by
/// hand and prints the variant only, never a digit; `Display` never contains
/// the typed digits.
#[derive(Clone, PartialEq, Eq, thiserror::Error)]
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
    /// The exact arithmetic ran out of room although the result is not
    /// large (for example a long chain of thirds and sevenths).
    #[error("the calculation is too complex")]
    TooComplex,
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
            AmountEntryError::TooComplex => TextKey("grid.calc.too_complex"),
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
            AmountEntryError::Negative => Some(TextKey("grid.amount.no_negative.fix")),
            _ => None,
        }
    }
}

fn input_name(e: MoneyInputError) -> &'static str {
    match e {
        MoneyInputError::Empty => "Empty",
        MoneyInputError::Invalid => "Invalid",
        MoneyInputError::AmbiguousSeparator => "AmbiguousSeparator",
        MoneyInputError::TooManyDecimals { .. } => "TooManyDecimals",
        MoneyInputError::TooLarge => "TooLarge",
        MoneyInputError::SignConflict => "SignConflict",
    }
}

/// The variant only: no amounts, no digits, so the error is safe in any log.
impl fmt::Debug for AmountEntryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            AmountEntryError::Input(e) => return write!(f, "Input({})", input_name(*e)),
            AmountEntryError::NoGrouping => "NoGrouping",
            AmountEntryError::DivideByZero => "DivideByZero",
            AmountEntryError::Incomplete => "Incomplete",
            AmountEntryError::TooLong => "TooLong",
            AmountEntryError::TooLarge => "TooLarge",
            AmountEntryError::TooComplex => "TooComplex",
            AmountEntryError::Negative => "Negative",
        };
        f.write_str(name)
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

/// Why pasted text is not put into an amount or date cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PasteError {
    /// The clipboard holds more than one value (a tab or line break inside,
    /// as when cells are copied from a spreadsheet).
    #[error("paste one value at a time")]
    SeveralValues,
}

impl PasteError {
    /// The i18n key of the owner message ("Paste one amount at a time").
    #[must_use]
    pub fn key(self) -> TextKey {
        TextKey("grid.paste.one_value")
    }
}

/// Pasted text for one amount or date cell (Q-P10j rev): one trailing line
/// break is dropped (spreadsheets add one); anything still holding a tab or
/// line break is refused, never split or cut short. The result is inserted at
/// the caret and parsed on commit like typed text.
///
/// # Errors
/// [`PasteError::SeveralValues`].
pub fn single_paste(text: &str) -> Result<&str, PasteError> {
    let one = text
        .strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .or_else(|| text.strip_suffix('\r'))
        .unwrap_or(text);
    if one.contains(['\t', '\r', '\n']) {
        Err(PasteError::SeveralValues)
    } else {
        Ok(one)
    }
}

/// Is `c` an operand character next to which `x`/`X`/`×` means "times"?
fn ends_operand(c: char) -> bool {
    c.is_ascii_digit() || c == '.' || c == ')'
}

fn starts_operand(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '$' | '(' | '-')
}

/// Normalize typed text before parsing: trim Unicode whitespace (NBSP and
/// U+202F included) at both ends, drop one trailing `=` ("evaluate"), U+2212
/// MINUS SIGN → `-`, `÷` → `/`, and `×`, `x` or `X` between two operands →
/// `*`. Nothing else is rewritten; internal
/// spaces inside a number stay ambiguous (`1 234` is refused).
#[must_use]
pub fn normalize(text: &str) -> String {
    let t = text.trim();
    let t = t.strip_suffix('=').map_or(t, str::trim_end);
    let chars: Vec<char> = t.chars().collect();
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

/// An `i128` step overflowed. [`Ratio::apply`] decides whether that means
/// the result is too large or only that the exact fractions grew too big.
fn over<T>(v: Option<T>) -> CalcResult<T> {
    v.ok_or(AmountEntryError::TooComplex)
}

/// The largest magnitude any amount type holds, in units (Money's).
const MAX_UNITS: u128 = 9_999_999_999_999_999_999;

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

    /// Whole units, rounded toward zero (an estimate for overflow triage).
    fn magnitude(self) -> u128 {
        self.n.unsigned_abs() / self.d.unsigned_abs()
    }

    fn apply(self, op: Op, o: Ratio) -> CalcResult<Ratio> {
        let r = match op {
            Op::Add => self.add(o),
            Op::Sub => self.sub(o),
            Op::Mul => self.mul(o),
            Op::Div => self.div(o),
        };
        match r {
            Err(AmountEntryError::TooComplex) => {
                // Estimate the step's size from whole units: beyond every
                // type's range means too large, otherwise too complex.
                let (a, b) = (self.magnitude(), o.magnitude());
                let per = UNITS.unsigned_abs();
                let size = match op {
                    Op::Add | Op::Sub => a.saturating_add(b),
                    Op::Mul => a.saturating_mul(b) / per,
                    Op::Div => a.saturating_mul(per).checked_div(b).unwrap_or(u128::MAX),
                };
                Err(if size > MAX_UNITS {
                    AmountEntryError::TooLarge
                } else {
                    AmountEntryError::TooComplex
                })
            }
            other => other,
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

/// Turn an exact result into a value of the field's type: rounded once to
/// the field's smallest unit, half away from zero (Q-P10x). Exact integer
/// arithmetic; no floating point.
fn finish(r: Ratio, rules: AmountRules) -> CalcResult<Amount> {
    let max_scale = rules.input.max_scale().min(SCALE);
    let step = pow10(SCALE - max_scale);
    // The result in field units is r.n / (r.d * step).
    let den = over(r.d.checked_mul(step))?;
    let q = r.n / den; // toward zero
    let rem = (r.n % den).unsigned_abs();
    // 2 * rem fits in u128 (rem < den <= i128::MAX).
    let rounded = if rem != 0 && rem * 2 >= den.unsigned_abs() {
        q + r.n.signum()
    } else {
        q
    };
    let units = rounded
        .checked_mul(step)
        .ok_or(AmountEntryError::TooLarge)?;
    let value = Amount::from_units(rules.kind, units).ok_or(AmountEntryError::TooLarge)?;
    check_sign(value, rules.sign)
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
/// refusal; calculations are refused when unfinished, too long, too complex,
/// grouped with parentheses, divided by zero or too large. A calculation's
/// result is rounded to the field's unit, half away from zero (Q-P10x).
pub fn parse(text: &str, rules: AmountRules) -> Result<Parsed, AmountEntryError> {
    let t = normalize(text);
    if t.chars().count() > MAX_CHARS {
        return Err(AmountEntryError::TooLong);
    }
    // Plain amount first (Q-P05t: `$-5` and `-$5` are both -5.00).
    let plain_err = match parse_plain(&t, rules) {
        Ok(value) => {
            return Ok(Parsed {
                value,
                calculated: false,
            });
        }
        Err(e) => e,
    };
    if !is_expression(&t) {
        return Err(plain_err);
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
    if !is_expression(&t) || parse_plain(&t, rules).is_ok() {
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
        assert_eq!(wire("1.234"), "1.2300", "typed money rounds (Q-232)");
        assert!(!money("1.234").unwrap().calculated);
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
    fn results_round_to_the_cent_half_away_from_zero() {
        assert_eq!(wire("100/3"), "33.3300");
        assert_eq!(wire("-100/3"), "-33.3300");
        assert_eq!(wire("200/3"), "66.6700");
        assert_eq!(wire("33.335*1"), "33.3400", "a half rounds away from zero");
        assert_eq!(wire("-33.335*1"), "-33.3400");
        assert_eq!(wire("33.3349*1"), "33.3300");
        assert_eq!(wire("0.005*1"), "0.0100");
        assert_eq!(wire("-0.004*1"), "0.0000", "zero has no sign");
        assert_eq!(wire("1234567/3"), "411522.3300");
        assert!(money("100/3").unwrap().calculated);
        // A plain typed amount rounds the same way (Q-232).
        assert_eq!(wire("12.345"), "12.3500");
        assert_eq!(wire("-1.005"), "-1.0100");
        assert_eq!(wire("(1.005)"), "-1.0100");
        assert_eq!(wire("1.0049999"), "1.0000");
        assert_eq!(wire("$1,234.565"), "1234.5700");
        assert_eq!(preview("12.345", AmountRules::MONEY), Preview::None);
        // Quantity and rate fields still refuse extra decimals.
        let qty = AmountRules {
            kind: AmountKind::Quantity,
            input: UserInput::FULL,
            sign: Sign::Any,
        };
        assert_eq!(
            parse("1.23456", qty),
            Err(AmountEntryError::Input(MoneyInputError::TooManyDecimals {
                max: 4
            }))
        );
        // A debit column still refuses a negative after rounding.
        let debit = AmountRules {
            sign: Sign::NonNegative,
            ..AmountRules::MONEY
        };
        assert_eq!(parse("-1.005", debit), Err(AmountEntryError::Negative));
        assert_eq!(
            parse("-0.004", debit).unwrap().value.to_ten_thousandths(),
            0
        );
        // A four-decimal field rounds to its own unit.
        let full = AmountRules {
            input: UserInput::FULL,
            ..AmountRules::MONEY
        };
        assert_eq!(
            parse("1.005*2", full).unwrap().value.to_ten_thousandths(),
            2_0100
        );
        assert_eq!(parse("1/3", full).unwrap().value.to_ten_thousandths(), 3333);
        assert_eq!(parse("2/3", full).unwrap().value.to_ten_thousandths(), 6667);
        // Rounding past the edge of the range is still too large.
        assert_eq!(
            money("999,999,999,999,999.99+0.009"),
            Err(AmountEntryError::TooLarge)
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
        assert_eq!(normalize("\u{202f}12\u{202f}"), "12");
        assert_eq!(normalize("400+44.25 = "), "400+44.25");
        assert_eq!(normalize("=="), "=");
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
        assert_eq!(
            preview("100/3", r),
            Preview::Value(Amount::Money(Money::parse_wire("33.33").unwrap()))
        );
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
            AmountEntryError::TooComplex,
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
            ("grid.calc.too_complex", None),
        ];
        for (e, (key, fix)) in errors.iter().zip(table) {
            assert_eq!(e.key(), TextKey(key), "{e:?}");
            assert_eq!(e.fix_key(), fix.map(TextKey), "{e:?}");
        }
    }

    #[test]
    fn plain_parse_comes_first() {
        // Q-P05t: both sign orders are plain amounts.
        assert_eq!(wire("$-5"), "-5.0000");
        assert!(!money("$-5").unwrap().calculated);
        assert_eq!(money("$-5"), money("-$5"));
        assert_eq!(wire("5--3"), "8.0000");
        assert_eq!(wire("5*-3"), "-15.0000");
        assert_eq!(wire("(5)*2"), "-10.0000");
        assert_eq!(
            money("(-3)"),
            Err(AmountEntryError::Input(MoneyInputError::SignConflict))
        );
        assert_eq!(
            preview("(5)*2", AmountRules::MONEY),
            Preview::Value(Amount::Money(Money::parse_wire("-10").unwrap()))
        );
        assert_eq!(preview("$-5", AmountRules::MONEY), Preview::None);
    }

    #[test]
    fn trailing_equals_evaluates() {
        assert_eq!(wire("400+44.25="), "444.2500");
        assert!(money("400+44.25=").unwrap().calculated);
        assert_eq!(wire("12="), "12.0000");
        assert_eq!(
            money("="),
            Err(AmountEntryError::Input(MoneyInputError::Empty))
        );
        assert_eq!(
            preview("400+44.25=", AmountRules::MONEY),
            Preview::Value(Amount::Money(Money::parse_wire("444.25").unwrap()))
        );
    }

    #[test]
    fn too_complex_is_not_too_large() {
        // Sixteen fractions over primes near 1000: the exact denominator
        // outgrows i128 although the sum is tiny.
        let primes = [
            997, 991, 983, 977, 971, 967, 953, 947, 941, 937, 929, 919, 911, 907, 887, 883,
        ];
        let text = primes
            .iter()
            .map(|p| format!("1/{p}"))
            .collect::<Vec<_>>()
            .join("+");
        assert_eq!(money(&text), Err(AmountEntryError::TooComplex));
        assert_eq!(
            money("999,999,999,999,999*999,999,999,999,999*999,999,999"),
            Err(AmountEntryError::TooLarge)
        );
        assert_eq!(
            money(&text).unwrap_err().key(),
            TextKey("grid.calc.too_complex")
        );
    }

    #[test]
    fn debug_never_prints_digits() {
        assert_eq!(
            format!(
                "{:?}",
                AmountEntryError::Input(MoneyInputError::TooManyDecimals { max: 2 })
            ),
            "Input(TooManyDecimals)"
        );
        for e in [
            AmountEntryError::NoGrouping,
            AmountEntryError::DivideByZero,
            AmountEntryError::Incomplete,
            AmountEntryError::TooLong,
            AmountEntryError::TooLarge,
            AmountEntryError::TooComplex,
            AmountEntryError::Negative,
        ] {
            assert!(!format!("{e:?}").contains(|c: char| c.is_ascii_digit()));
        }
    }

    #[test]
    fn pastes_hold_one_value() {
        assert_eq!(single_paste("12.50"), Ok("12.50"));
        assert_eq!(single_paste("12.50\r\n"), Ok("12.50"));
        assert_eq!(single_paste("12.50\n"), Ok("12.50"));
        assert_eq!(single_paste("12.50\r"), Ok("12.50"));
        assert_eq!(single_paste(" 1,234.56 "), Ok(" 1,234.56 "));
        assert_eq!(single_paste("12.50\t3"), Err(PasteError::SeveralValues));
        assert_eq!(single_paste("1\r\n2\r\n"), Err(PasteError::SeveralValues));
        assert_eq!(
            single_paste("12.50\r\n\r\n"),
            Err(PasteError::SeveralValues)
        );
        assert_eq!(
            PasteError::SeveralValues.key(),
            TextKey("grid.paste.one_value")
        );
    }
}
