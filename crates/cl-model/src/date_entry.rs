//! Date text entry shared by grid cells and form fields (P10 plan §2.6,
//! Q-P10f).
//!
//! [`parse`] reads what the owner typed, [`format`] shows a date in the
//! owner's chosen order, and [`apply_key`] applies a date key (`+ - T M H Y
//! R W K`, see [`crate::date_rules`]) to the date being edited. "Today" and
//! the first day of the week are always passed in; nothing here reads a
//! clock.

use crate::date_rules::{DateKey, apply_date_key};
use crate::text::TextKey;
use cl_core::{CalendarDate, DateError, Weekday};

/// The first day of the week for US businesses (Q-41, Q-P10f). Used where
/// no preference exists yet.
pub const US_FIRST_WEEKDAY: Weekday = Weekday::Sun;

/// How dates are shown (from the owner's date-format preference).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum DateOrder {
    /// `MM/DD/YYYY` (default).
    #[default]
    Mdy,
    /// `YYYY-MM-DD`.
    Ymd,
}

/// Why typed text is not a date. No variant carries the typed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DateEntryError {
    /// Nothing typed.
    #[error("no date entered")]
    Empty,
    /// Not a date shape this field understands.
    #[error("not a date")]
    Invalid,
    /// The right shape but no such day (`2/29/2027`, `13/1/2026`).
    #[error("no such day")]
    NoSuchDay,
    /// Outside the years the books support (1900 to 2199).
    #[error("date is outside the supported years")]
    OutOfRange,
}

impl DateEntryError {
    /// The i18n key of the owner message.
    #[must_use]
    pub fn key(self) -> TextKey {
        match self {
            DateEntryError::Empty => TextKey("grid.date.empty"),
            DateEntryError::Invalid => TextKey("grid.date.invalid"),
            DateEntryError::NoSuchDay => TextKey("grid.date.no_such_day"),
            DateEntryError::OutOfRange => TextKey("grid.date.out_of_range"),
        }
    }
}

impl From<DateError> for DateEntryError {
    fn from(e: DateError) -> Self {
        match e {
            DateError::NoSuchDay => DateEntryError::NoSuchDay,
            DateError::OutOfRange => DateEntryError::OutOfRange,
            DateError::Format | DateError::TimestampFormat => DateEntryError::Invalid,
        }
    }
}

/// Display text: `MM/DD/YYYY` or `YYYY-MM-DD`.
#[must_use]
pub fn format(date: CalendarDate, order: DateOrder) -> String {
    match order {
        DateOrder::Mdy => format!("{:02}/{:02}/{}", date.month(), date.day(), date.year()),
        DateOrder::Ymd => date.to_wire(),
    }
}

/// A run of 1 to `max` ASCII digits.
fn number(part: &str, max: usize) -> Result<u32, DateEntryError> {
    if part.is_empty() || part.len() > max || !part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DateEntryError::Invalid);
    }
    part.parse().map_err(|_| DateEntryError::Invalid)
}

/// A typed year: four digits as typed, one or two digits mean 2000-2099.
fn year(part: &str) -> Result<i32, DateEntryError> {
    let y = match part.len() {
        1 | 2 => number(part, 2)? + 2000,
        4 => number(part, 4)?,
        _ => return Err(DateEntryError::Invalid),
    };
    i32::try_from(y).map_err(|_| DateEntryError::Invalid)
}

fn ymd(y: i32, m: u32, d: u32) -> Result<CalendarDate, DateEntryError> {
    Ok(CalendarDate::from_ymd(y, m, d)?)
}

/// Parse a typed date.
///
/// Accepted in either order: `M/D` (today's year), `M/D/YY` (2000 to 2099),
/// `M/D/YYYY` and ISO `YYYY-MM-DD` (also `YYYY-M-D`). With [`DateOrder::Ymd`]
/// also `YYYY/M/D`. Surrounding whitespace is ignored.
///
/// # Errors
/// [`DateEntryError::Empty`], [`DateEntryError::Invalid`] for any other
/// shape, [`DateEntryError::NoSuchDay`] and [`DateEntryError::OutOfRange`].
pub fn parse(
    text: &str,
    order: DateOrder,
    today: CalendarDate,
) -> Result<CalendarDate, DateEntryError> {
    let s = text.trim();
    if s.is_empty() {
        return Err(DateEntryError::Empty);
    }
    let dashes: Vec<&str> = s.split('-').collect();
    if dashes.len() == 3 {
        if dashes[0].len() != 4 {
            return Err(DateEntryError::Invalid);
        }
        return ymd(
            year(dashes[0])?,
            number(dashes[1], 2)?,
            number(dashes[2], 2)?,
        );
    }
    let parts: Vec<&str> = s.split('/').collect();
    match parts.as_slice() {
        [m, d] => ymd(today.year(), number(m, 2)?, number(d, 2)?),
        [y, m, d] if y.len() == 4 => {
            if order == DateOrder::Ymd {
                ymd(year(y)?, number(m, 2)?, number(d, 2)?)
            } else {
                Err(DateEntryError::Invalid)
            }
        }
        [m, d, y] => ymd(year(y)?, number(m, 2)?, number(d, 2)?),
        _ => Err(DateEntryError::Invalid),
    }
}

/// Apply a date key to the date being edited: the typed text when it is a
/// date, otherwise the field's current value, otherwise today. `None` when
/// the result would leave the supported range (the field keeps its value).
#[must_use]
pub fn apply_key(
    key: DateKey,
    text: &str,
    current: Option<CalendarDate>,
    order: DateOrder,
    today: CalendarDate,
    first_weekday: Weekday,
) -> Option<CalendarDate> {
    let base = parse(text, order, today).ok().or(current).unwrap_or(today);
    apply_date_key(key, base, today, first_weekday)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).unwrap()
    }

    const TODAY: (i32, u32, u32) = (2026, 11, 4);

    fn today() -> CalendarDate {
        d(TODAY.0, TODAY.1, TODAY.2)
    }

    fn mdy(s: &str) -> Result<CalendarDate, DateEntryError> {
        parse(s, DateOrder::Mdy, today())
    }

    #[test]
    fn us_shapes() {
        assert_eq!(mdy("11/04/2026"), Ok(d(2026, 11, 4)));
        assert_eq!(mdy("1/4/26"), Ok(d(2026, 1, 4)));
        assert_eq!(mdy("1/4/5"), Ok(d(2005, 1, 4)));
        assert_eq!(mdy("1/4/99"), Ok(d(2099, 1, 4)));
        assert_eq!(mdy(" 3/7 "), Ok(d(2026, 3, 7)), "M/D uses today's year");
        assert_eq!(mdy("2026-11-04"), Ok(d(2026, 11, 4)));
        assert_eq!(mdy("2026-1-4"), Ok(d(2026, 1, 4)));
    }

    #[test]
    fn ymd_order_also_takes_year_first_slashes() {
        let t = today();
        assert_eq!(parse("2026/11/4", DateOrder::Ymd, t), Ok(d(2026, 11, 4)));
        assert_eq!(parse("11/4/2026", DateOrder::Ymd, t), Ok(d(2026, 11, 4)));
        assert_eq!(mdy("2026/11/4"), Err(DateEntryError::Invalid));
    }

    #[test]
    fn refusals() {
        assert_eq!(mdy(""), Err(DateEntryError::Empty));
        assert_eq!(mdy("  "), Err(DateEntryError::Empty));
        assert_eq!(mdy("2/29/2027"), Err(DateEntryError::NoSuchDay));
        assert_eq!(mdy("13/40/2026"), Err(DateEntryError::NoSuchDay));
        assert_eq!(mdy("0/1/2026"), Err(DateEntryError::NoSuchDay));
        assert_eq!(mdy("1/1/2200"), Err(DateEntryError::OutOfRange));
        assert_eq!(mdy("12/31/1899"), Err(DateEntryError::OutOfRange));
        assert_eq!(mdy("1/1/202"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("1/1/20266"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("111/1/2026"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("1/x/2026"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("+1/1/2026"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("1/1/2026/1"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("26-11-04"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("tomorrow"), Err(DateEntryError::Invalid));
        assert_eq!(mdy("2/29/2028"), Ok(d(2028, 2, 29)));
    }

    #[test]
    fn errors_have_keys_and_no_values() {
        assert_eq!(
            DateEntryError::NoSuchDay.key(),
            TextKey("grid.date.no_such_day")
        );
        assert_eq!(
            DateEntryError::OutOfRange.key(),
            TextKey("grid.date.out_of_range")
        );
        assert_eq!(DateEntryError::Invalid.key(), TextKey("grid.date.invalid"));
        assert_eq!(DateEntryError::Empty.key(), TextKey("grid.date.empty"));
        assert_eq!(
            DateEntryError::from(DateError::TimestampFormat),
            DateEntryError::Invalid
        );
    }

    #[test]
    fn display_orders() {
        assert_eq!(format(d(2026, 1, 4), DateOrder::Mdy), "01/04/2026");
        assert_eq!(format(d(2026, 1, 4), DateOrder::Ymd), "2026-01-04");
        for order in [DateOrder::Mdy, DateOrder::Ymd] {
            let x = d(2031, 12, 9);
            assert_eq!(parse(&format(x, order), order, today()), Ok(x));
        }
    }

    fn key(c: char, text: &str, current: Option<CalendarDate>, first: Weekday) -> CalendarDate {
        let k = DateKey::from_char(c).unwrap();
        apply_key(k, text, current, DateOrder::Mdy, today(), first).unwrap()
    }

    #[test]
    fn keys_start_from_text_then_current_then_today() {
        let sun = US_FIRST_WEEKDAY;
        assert_eq!(
            key('+', "1/31/2026", Some(d(2020, 5, 5)), sun),
            d(2026, 2, 1)
        );
        assert_eq!(key('+', "garbage", Some(d(2020, 5, 5)), sun), d(2020, 5, 6));
        assert_eq!(key('+', "", None, sun), d(2026, 11, 5));
        assert_eq!(key('t', "1/1/2001", None, sun), today());
    }

    #[test]
    fn keys_at_month_and_year_edges() {
        let sun = US_FIRST_WEEKDAY;
        assert_eq!(key('-', "1/1/2026", None, sun), d(2025, 12, 31));
        assert_eq!(key('+', "12/31/2026", None, sun), d(2027, 1, 1));
        assert_eq!(key('m', "12/31/2026", None, sun), d(2026, 12, 1));
        assert_eq!(key('h', "12/1/2026", None, sun), d(2026, 12, 31));
        assert_eq!(key('h', "4/10/2026", None, sun), d(2026, 4, 30));
        assert_eq!(key('y', "7/4/2026", None, sun), d(2026, 1, 1));
        assert_eq!(key('r', "7/4/2026", None, sun), d(2026, 12, 31));
        assert_eq!(key('Y', "1/1/2026", None, sun), d(2026, 1, 1));
        assert_eq!(key('R', "12/31/2026", None, sun), d(2026, 12, 31));
    }

    #[test]
    fn leap_years() {
        let sun = US_FIRST_WEEKDAY;
        assert_eq!(key('+', "2/28/2028", None, sun), d(2028, 2, 29));
        assert_eq!(key('+', "2/29/2028", None, sun), d(2028, 3, 1));
        assert_eq!(key('+', "2/28/2027", None, sun), d(2027, 3, 1));
        assert_eq!(key('h', "2/10/2027", None, sun), d(2027, 2, 28));
        assert_eq!(key('h', "2/10/2028", None, sun), d(2028, 2, 29));
        assert_eq!(
            key('h', "2/10/2100", None, sun),
            d(2100, 2, 28),
            "2100 is no leap year"
        );
        assert_eq!(key('h', "2/10/2000", None, sun), d(2000, 2, 29));
        assert_eq!(key('-', "3/1/2028", None, sun), d(2028, 2, 29));
    }

    #[test]
    fn weeks_across_a_year_boundary() {
        // Thursday 1/1/2026: the Sunday week started 12/28/2025.
        assert_eq!(key('w', "1/1/2026", None, Weekday::Sun), d(2025, 12, 28));
        assert_eq!(key('k', "1/1/2026", None, Weekday::Sun), d(2026, 1, 3));
        assert_eq!(key('w', "1/1/2026", None, Weekday::Mon), d(2025, 12, 29));
        assert_eq!(key('k', "1/1/2026", None, Weekday::Mon), d(2026, 1, 4));
        // Wednesday 12/30/2026: the week ends in January.
        assert_eq!(key('k', "12/30/2026", None, Weekday::Sun), d(2027, 1, 2));
        assert_eq!(key('k', "12/30/2026", None, Weekday::Mon), d(2027, 1, 3));
        // On the first day itself, W stays and K is six days on.
        assert_eq!(key('w', "1/4/2026", None, Weekday::Sun), d(2026, 1, 4));
        assert_eq!(key('k', "1/4/2026", None, Weekday::Sun), d(2026, 1, 10));
    }

    #[test]
    fn keys_refuse_to_leave_the_range() {
        let k = DateKey::from_char('+').unwrap();
        assert_eq!(
            apply_key(
                k,
                "12/31/2199",
                None,
                DateOrder::Mdy,
                today(),
                US_FIRST_WEEKDAY
            ),
            None
        );
        let k = DateKey::from_char('-').unwrap();
        assert_eq!(
            apply_key(
                k,
                "1/1/1900",
                None,
                DateOrder::Mdy,
                today(),
                US_FIRST_WEEKDAY
            ),
            None
        );
    }
}
