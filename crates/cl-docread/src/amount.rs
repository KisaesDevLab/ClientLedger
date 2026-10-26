//! Document amounts (WP-A, step 8; plan D3 `amount.rs`, §4 "Parsed"): an
//! exact port of the engine's `normalizeBillMoney`
//! (`ai-bill-ocr.service.ts:149-183`): `$`, `USD`, US thousands commas,
//! accounting parentheses; refuses ambiguous input such as `12,50`; never
//! rounds. Then `Money::parse_wire`.
//!
//! The **only** parser of document amounts. **Consumers:** the rules, the
//! AI gate, and the desktop (WP-G) — the signature of [`parse`] is fixed.
//!
//! One place where the result differs from the engine's string: five or
//! more decimals (`1.23456`) pass `normalizeBillMoney` but not the engine's
//! money columns; [`parse`] refuses them rather than round
//! (`Money::parse_wire` → `TooPrecise`). The shared case table lives in
//! `fixtures/docs/rule-cases.json` so the contract test can run it against
//! the live engine too (review N4).

use cl_core::{Money, Quantity};

/// The amount printed in `text`, or `None` when it is not exactly one
/// unambiguous amount.
#[must_use]
pub fn parse(text: &str) -> Option<Money> {
    Money::parse_wire(&normalize(text)?).ok()
}

/// A quantity printed in `text` (a line item's "Qty"), through the same
/// normalization as [`parse`], or `None`.
#[must_use]
pub fn parse_quantity(text: &str) -> Option<Quantity> {
    Quantity::parse_wire(&normalize(text)?).ok()
}

/// The engine's `normalizeBillMoney` for a string: a plain decimal string
/// (`"1234.56"`, `"-12.5"`) or `None`. Ported line by line, quirks
/// included (a minus sign before and after the currency sign cancel out).
#[must_use]
pub fn normalize(raw: &str) -> Option<String> {
    let mut s: String = js_trim(raw).to_string();
    let mut negative = false;
    // /^\(.*\)$/ — `.` does not match a line terminator.
    if s.len() >= 2
        && s.starts_with('(')
        && s.ends_with(')')
        && !s[1..s.len() - 1].chars().any(is_js_line_terminator)
    {
        negative = true;
        s = s[1..s.len() - 1].to_string();
    }
    s.retain(|c| !is_js_space(c));
    take_minus(&mut s, &mut negative);
    strip_currency(&mut s);
    take_minus(&mut s, &mut negative);
    if s.contains(',') {
        if !is_us_thousands(&s) {
            return None;
        }
        s.retain(|c| c != ',');
    }
    if s.starts_with('.') {
        s.insert(0, '0');
    }
    if s.ends_with('.') {
        s.pop();
    }
    if !is_plain_decimal(&s) {
        return None;
    }
    // /^0+(?=\d)/: leading zeros of the integer part.
    let zeros = s.bytes().take_while(|&b| b == b'0').count();
    if zeros > 0 && s.as_bytes().get(zeros).is_some_and(u8::is_ascii_digit) {
        s.drain(..zeros);
    } else if zeros > 1 {
        // "00.5" → "0.5": the lookahead keeps the last zero before a
        // non-digit.
        s.drain(..zeros - 1);
    }
    if negative && !is_zero_spelling(&s) {
        s.insert(0, '-');
    }
    Some(s)
}

fn take_minus(s: &mut String, negative: &mut bool) {
    if s.starts_with('-') {
        *negative = !*negative;
        s.remove(0);
    }
}

/// `/^(?:US\$|USD|\$)/i`.
fn strip_currency(s: &mut String) {
    let lower = s.get(..3).map(str::to_ascii_lowercase);
    if lower.as_deref() == Some("us$") || lower.as_deref() == Some("usd") {
        s.drain(..3);
    } else if s.starts_with('$') {
        s.remove(0);
    }
}

/// `/^\d{1,3}(,\d{3})+(\.\d+)?$/`.
fn is_us_thousands(s: &str) -> bool {
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if let Some(f) = frac
        && (f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let mut groups = int.split(',');
    let Some(first) = groups.next() else {
        return false;
    };
    if first.is_empty() || first.len() > 3 || !first.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut rest = 0;
    for g in groups {
        if g.len() != 3 || !g.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        rest += 1;
    }
    rest > 0
}

/// `/^-?\d+(\.\d+)?$/`.
fn is_plain_decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// `/^0(\.0+)?$/`.
fn is_zero_spelling(s: &str) -> bool {
    match s.split_once('.') {
        None => s == "0",
        Some((i, f)) => i == "0" && !f.is_empty() && f.bytes().all(|b| b == b'0'),
    }
}

/// JavaScript's line terminators.
fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// JavaScript's `\s` (`WhiteSpace` + `LineTerminator`): Unicode spaces and
/// the BOM, but not U+0085.
pub(crate) fn is_js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{FEFF}'
}

/// `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The shared case table (`fixtures/docs/rule-cases.json`, `money`):
    /// input → the engine's normalized string or null.
    pub(crate) fn case_table() -> Vec<(String, Option<String>)> {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/docs/rule-cases.json")).unwrap();
        json["money"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                (
                    c["in"].as_str().unwrap().to_string(),
                    c["out"].as_str().map(str::to_string),
                )
            })
            .collect()
    }

    #[test]
    fn normalize_matches_the_engine_case_table() {
        let cases = case_table();
        assert!(cases.len() >= 30);
        for (input, want) in cases {
            assert_eq!(normalize(&input), want, "input {input:?}");
        }
    }

    #[test]
    fn parse_refuses_what_the_money_column_cannot_hold() {
        assert_eq!(
            parse("$1,234.56"),
            Some(Money::parse_wire("1234.56").unwrap())
        );
        assert_eq!(parse("(12.50)"), Some(Money::parse_wire("-12.5").unwrap()));
        assert_eq!(parse("-0"), Some(Money::ZERO));
        assert_eq!(normalize("1.23456").as_deref(), Some("1.23456"));
        assert_eq!(parse("1.23456"), None, "never rounded");
        assert_eq!(parse("1234567890123456"), None, "too many digits");
        assert_eq!(parse("12,50"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("Total"), None);
        assert_eq!(parse("--5"), Some(Money::parse_wire("5").unwrap()));
        assert_eq!(normalize("(---5)").as_deref(), Some("--5"));
        assert_eq!(parse("(---5)"), None, "\"--5\" is not a wire amount");
    }

    #[test]
    fn quantities_share_the_normalization() {
        assert_eq!(parse_quantity("3.459"), Quantity::parse_wire("3.459").ok());
        assert_eq!(parse_quantity("1,200"), Quantity::parse_wire("1200").ok());
        assert_eq!(parse_quantity("123456789"), None, "8 integer digits");
        assert_eq!(parse_quantity("2 ea"), None);
    }

    proptest! {
        /// Any amount the engine column holds, printed as a document would
        /// print it, reads back exactly.
        #[test]
        fn printed_amounts_round_trip(
            units in -999_999_999_999_i64..=999_999_999_999,
            commas in any::<bool>(),
            dollar in any::<bool>(),
            parens in any::<bool>(),
        ) {
            let m = Money::from_cents(units).unwrap();
            let abs = m.abs();
            let mut text = abs.format(cl_core::MoneyStyle::default());
            if !commas {
                text.retain(|c| c != ',');
            }
            if dollar {
                text.insert(0, '$');
            }
            if m.is_negative() {
                text = if parens { format!("({text})") } else { format!("-{text}") };
            }
            prop_assert_eq!(parse(&text), Some(m));
        }

        /// Arbitrary text never panics and anything accepted is a plain
        /// decimal.
        #[test]
        fn arbitrary_text_is_safe(s in "\\PC{0,24}") {
            if let Some(n) = normalize(&s) {
                prop_assert!(is_plain_decimal(&n) || n.starts_with("--"), "{n}");
            }
        }
    }
}
