//! Cell text in and out of the editor (P10 plan §2.5-2.7).
//!
//! [`cell_text`] turns a model value into the editor's starting text;
//! [`parse_buffer`] turns the edited text back into a [`CellInput`] using the
//! exact `cl-model` entry rules. Nothing here rounds: display uses
//! `Money::format` and the calculator refuses inexact results.

use cl_model::TextKey;
use cl_model::amount_entry::{self, Amount, AmountEntryError};
use cl_model::date_entry;

use super::keys::Formats;
use super::model::{CellInput, CellKind, CellProblem, CellValue, ListId, Lookups, Source};

/// The editor text for a value (empty for checkboxes, choices, chips,
/// marks and suggestions, which have no text buffer).
#[must_use]
pub fn cell_text(value: &CellValue<'_>, fmt: Formats) -> String {
    let style = cl_core::MoneyStyle {
        negative: fmt.money.negative,
        symbol: false,
    };
    match value {
        CellValue::Text(t) => t.to_string(),
        CellValue::Money(m) => m.format(style),
        CellValue::Quantity(q) => q.format(style),
        CellValue::Rate(r) => r.format(style),
        CellValue::Date(d) => date_entry::format(*d, fmt.date_order),
        CellValue::Ref { display, .. } => display.to_string(),
        CellValue::Empty
        | CellValue::Loading
        | CellValue::Bool(_)
        | CellValue::Choice(_)
        | CellValue::Tags(_)
        | CellValue::Mark(_)
        | CellValue::Suggested { .. } => String::new(),
    }
}

/// An amount error as a cell problem (keys only).
#[must_use]
pub fn amount_problem(e: &AmountEntryError) -> CellProblem {
    CellProblem {
        key: e.key(),
        fix: e.fix_key(),
        col: None,
    }
}

/// The key shown under a picker cell whose text matched nothing.
pub const NOT_IN_LIST: TextKey = TextKey("grid.picker.not_in_list");
/// Text longer than the column allows.
pub const TEXT_TOO_LONG: TextKey = TextKey("grid.text.too_long");
/// Tag text that matched no tag.
pub const TAG_NOT_IN_LIST: TextKey = TextKey("grid.tags.not_in_list");

/// What a picker's text resolves to on Tab or commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerResolution {
    /// An entry (from the highlight or an exact match).
    Entry(u64),
    /// Nothing typed.
    Empty,
    /// Text that matches nothing (kept, flagged).
    Unresolved(String),
}

/// Resolve picker text without a highlighted popup item: an exact name or
/// number match, else unresolved.
#[must_use]
pub fn resolve_picker(list: ListId, text: &str, lookups: &dyn Lookups) -> PickerResolution {
    let t = text.trim();
    if t.is_empty() {
        return PickerResolution::Empty;
    }
    match lookups.exact(list, t) {
        Some(s) => PickerResolution::Entry(s.id),
        None => PickerResolution::Unresolved(t.to_string()),
    }
}

/// Parse an edited text buffer for a text, memo, amount or date cell.
///
/// # Errors
/// The owner's text is not accepted; the problem carries keys only.
pub fn parse_buffer(
    kind: CellKind,
    text: &str,
    fmt: Formats,
) -> Result<(CellInput, Source), CellProblem> {
    match kind {
        CellKind::Text { max_chars } | CellKind::Memo { max_chars, .. } => {
            if text.chars().count() > max_chars as usize {
                return Err(CellProblem::new(TEXT_TOO_LONG));
            }
            if text.trim().is_empty() {
                Ok((CellInput::Clear, Source::Typed))
            } else {
                Ok((CellInput::Text(text.to_string()), Source::Typed))
            }
        }
        CellKind::Amount(rules) => {
            if text.trim().is_empty() {
                return Ok((CellInput::Clear, Source::Typed));
            }
            let p = amount_entry::parse(text, rules).map_err(|e| amount_problem(&e))?;
            let input = match p.value {
                Amount::Money(m) => CellInput::Money(m),
                Amount::Quantity(q) => CellInput::Quantity(q),
                Amount::Rate(r) => CellInput::Rate(r),
            };
            let source = if p.calculated {
                Source::Calculator
            } else {
                Source::Typed
            };
            Ok((input, source))
        }
        CellKind::Date => {
            if text.trim().is_empty() {
                return Ok((CellInput::Clear, Source::Typed));
            }
            date_entry::parse(text, fmt.date_order, fmt.today)
                .map(|d| (CellInput::Date(d), Source::Typed))
                .map_err(|e| CellProblem::new(e.key()))
        }
        // Pickers and tags resolve through lookups; checkboxes, dropdowns,
        // marks and display cells have no text buffer.
        CellKind::Picker { .. }
        | CellKind::Tags { .. }
        | CellKind::Checkbox
        | CellKind::Dropdown { .. }
        | CellKind::Mark
        | CellKind::Display => Ok((CellInput::Clear, Source::Typed)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{LookupResult, Suggestion};
    use cl_core::{CalendarDate, Money, MoneyStyle, NegativeStyle, Weekday};
    use cl_model::amount_entry::AmountRules;
    use cl_model::date_entry::DateOrder;
    use std::borrow::Cow;

    fn fmt() -> Formats {
        Formats {
            today: CalendarDate::from_ymd(2026, 11, 4).unwrap(),
            first_weekday: Weekday::Sun,
            date_order: DateOrder::Mdy,
            money: MoneyStyle {
                negative: NegativeStyle::Parens,
                symbol: true,
            },
            enter_mode: cl_model::forms::EnterMode::SaveAndNew,
        }
    }

    #[test]
    fn text_of_values() {
        let f = fmt();
        let m = Money::parse_wire("-1234.5").unwrap();
        assert_eq!(
            cell_text(&CellValue::Money(m), f),
            "(1,234.50)",
            "no symbol in cells"
        );
        assert_eq!(
            cell_text(
                &CellValue::Date(CalendarDate::from_ymd(2026, 1, 5).unwrap()),
                f
            ),
            "01/05/2026"
        );
        assert_eq!(
            cell_text(
                &CellValue::Ref {
                    id: 1,
                    display: Cow::Borrowed("Fuel")
                },
                f
            ),
            "Fuel"
        );
        assert_eq!(
            cell_text(
                &CellValue::Suggested {
                    id: 1,
                    display: Cow::Borrowed("Fuel")
                },
                f
            ),
            ""
        );
    }

    #[test]
    fn parse_each_kind() {
        let f = fmt();
        let amount = CellKind::Amount(AmountRules::MONEY);
        assert_eq!(
            parse_buffer(amount, "400+44.25", f),
            Ok((
                CellInput::Money(Money::parse_wire("444.25").unwrap()),
                Source::Calculator
            ))
        );
        assert_eq!(
            parse_buffer(amount, "  ", f),
            Ok((CellInput::Clear, Source::Typed))
        );
        assert_eq!(
            parse_buffer(amount, "100/3", f),
            Ok((
                CellInput::Money(Money::parse_wire("33.33").unwrap()),
                Source::Calculator
            ))
        );
        assert_eq!(
            parse_buffer(CellKind::Date, "1/5/99", f),
            Ok((
                CellInput::Date(CalendarDate::from_ymd(2099, 1, 5).unwrap()),
                Source::Typed
            ))
        );
        assert_eq!(
            parse_buffer(CellKind::Date, "2/30", f).unwrap_err().key,
            TextKey("grid.date.no_such_day")
        );
        let text = CellKind::Text { max_chars: 3 };
        assert_eq!(
            parse_buffer(text, "abcd", f).unwrap_err().key,
            TEXT_TOO_LONG
        );
        assert_eq!(
            parse_buffer(text, "ab", f),
            Ok((CellInput::Text("ab".into()), Source::Typed))
        );
        assert_eq!(
            parse_buffer(text, "", f),
            Ok((CellInput::Clear, Source::Typed))
        );
        assert_eq!(
            parse_buffer(CellKind::Checkbox, "x", f),
            Ok((CellInput::Clear, Source::Typed))
        );
    }

    struct One;
    impl Lookups for One {
        fn suggest(&self, _l: ListId, _q: &str, _n: usize) -> LookupResult {
            LookupResult::Loading
        }
        fn exact(&self, _l: ListId, q: &str) -> Option<Suggestion> {
            (q.eq_ignore_ascii_case("fuel")).then(|| Suggestion {
                id: 7,
                label: "Fuel".into(),
                group: None,
            })
        }
        fn display(&self, _l: ListId, _id: u64) -> Option<Cow<'_, str>> {
            None
        }
        fn all(&self, _l: ListId) -> LookupResult {
            LookupResult::Loading
        }
    }

    #[test]
    fn picker_resolution() {
        let l = ListId(1);
        assert_eq!(
            resolve_picker(l, " FUEL ", &One),
            PickerResolution::Entry(7)
        );
        assert_eq!(resolve_picker(l, "", &One), PickerResolution::Empty);
        assert_eq!(
            resolve_picker(l, "Gas", &One),
            PickerResolution::Unresolved("Gas".into())
        );
    }
}
