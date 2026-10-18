//! The calculator caption under an amount cell (Q-12): `= 444.25` while a
//! calculation is typed, nothing for plain amounts or unfinished
//! calculations, and the plain-language error (with the two nearest values
//! for an inexact result). The evaluator is `cl_model::amount_entry`.

use cl_model::TextKey;
use cl_model::amount_entry::{AmountEntryError, AmountRules, Preview, preview};

/// The caption text, when there is one. `text` resolves i18n keys
/// (`grid.calc.preview` is "= {value}"; `grid.calc.not_exact.detail` is
/// "{approx}; type {below} or {above}").
#[must_use]
pub fn caption(
    buffer: &str,
    rules: AmountRules,
    style: cl_core::MoneyStyle,
    text: &dyn Fn(TextKey) -> String,
) -> Option<String> {
    let style = cl_core::MoneyStyle {
        symbol: false,
        ..style
    };
    match preview(buffer, rules) {
        Preview::None => None,
        Preview::Value(v) => {
            Some(text(TextKey("grid.calc.preview")).replace("{value}", &v.format(style)))
        }
        Preview::Error(e) => Some(error_text(&e, style, text)),
    }
}

/// An amount error as one caption line: the message, then the fix.
#[must_use]
pub fn error_text(
    e: &AmountEntryError,
    style: cl_core::MoneyStyle,
    text: &dyn Fn(TextKey) -> String,
) -> String {
    let mut s = text(e.key());
    if let AmountEntryError::NotExact(n) = e {
        let detail = text(TextKey("grid.calc.not_exact.detail"))
            .replace("{approx}", n.approx())
            .replace("{below}", &n.below().format(style))
            .replace("{above}", &n.above().format(style));
        s.push_str(": ");
        s.push_str(&detail);
    } else if let Some(fix) = e.fix_key() {
        s.push_str(". ");
        s.push_str(&text(fix));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(k: TextKey) -> String {
        match k.0 {
            "grid.calc.preview" => "= {value}".into(),
            "grid.calc.not_exact" => "Not a whole number of cents".into(),
            "grid.calc.not_exact.detail" => "{approx}; type {below} or {above}".into(),
            other => other.into(),
        }
    }

    #[test]
    fn captions() {
        let r = AmountRules::MONEY;
        let s = cl_core::MoneyStyle::default();
        assert_eq!(caption("400+44.25", r, s, &t).as_deref(), Some("= 444.25"));
        assert_eq!(caption("12", r, s, &t), None);
        assert_eq!(caption("400+", r, s, &t), None);
        assert_eq!(
            caption("100/3", r, s, &t).as_deref(),
            Some("Not a whole number of cents: 33.333…; type 33.33 or 33.34")
        );
        assert_eq!(
            caption("(5+3)*2", r, s, &t).as_deref(),
            Some("grid.calc.no_grouping. grid.calc.no_grouping.fix")
        );
    }
}
