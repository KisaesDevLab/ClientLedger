//! Dates in QIF and CSV files (Q-195, Q-P25g).
//!
//! US month/day is the default. A column is judged as a whole: when some
//! value cannot be month/day but can be day/month (`13/02`), the column is
//! flagged so the wizard can offer day/month; a value that is no date in
//! the chosen order (`02/30`) is a row problem. ISO dates (`2026-11-03`)
//! and 8-digit `20261103` read the same in every order.
//!
//! Two-digit years: `'YY` (Quicken's apostrophe) is 20YY; `/YY` is 20YY
//! when that is at most next year, otherwise 19YY (Q-P25g).

use crate::normalize::ProblemKind;
use cl_core::CalendarDate;
use serde::{Deserialize, Serialize};

/// The order of the day, month and year in a date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    /// Month/day/year (US, the default, Q-195).
    #[default]
    Mdy,
    /// Day/month/year.
    Dmy,
    /// Year/month/day.
    Ymd,
}

/// The verdict on a whole date column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateColumn {
    /// The order used to read the column.
    pub order: DateOrder,
    /// The first line whose date cannot be month/day but can be day/month
    /// (`13/02`): the wizard offers to read the column as day/month.
    /// `None` when nothing points that way.
    pub day_first_line: Option<usize>,
    /// Every value reads both ways and at least one reads differently
    /// (`03/04`): the default order was used without evidence.
    pub ambiguous: bool,
}

/// The parts of a date as written, before an order is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parts {
    /// Year known to be first (ISO, 8 digits, a 4-digit first part).
    Ymd(i32, u32, u32),
    /// Three numbers `a/b/year` in an order to be chosen.
    Ab(u32, u32, i32),
    /// A month written as a word, so the order is known.
    Named { month: u32, day: u32, year: i32 },
}

/// Reads a date in the given order.
///
/// # Errors
/// [`ProblemKind::MissingDate`] for blank text, [`ProblemKind::BadDate`]
/// when it is not a date in any order, [`ProblemKind::ImpossibleDate`]
/// when it reads but the day or month does not exist in this order.
pub fn parse_date(
    text: &str,
    order: DateOrder,
    today: CalendarDate,
) -> Result<CalendarDate, ProblemKind> {
    let parts = split(text, today)?;
    build(parts, order)
}

fn build(parts: Parts, order: DateOrder) -> Result<CalendarDate, ProblemKind> {
    let (y, m, d) = match (parts, order) {
        (
            Parts::Ymd(y, m, d)
            | Parts::Named {
                month: m,
                day: d,
                year: y,
            },
            _,
        ) => (y, m, d),
        (Parts::Ab(a, b, y), DateOrder::Mdy | DateOrder::Ymd) => (y, a, b),
        (Parts::Ab(a, b, y), DateOrder::Dmy) => (y, b, a),
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || !(1900..=2199).contains(&y) {
        return Err(ProblemKind::ImpossibleDate);
    }
    CalendarDate::from_ymd(y, m, d).map_err(|_| ProblemKind::ImpossibleDate)
}

/// Judges a whole column of `(line, text)` values (Q-195). `preferred` is
/// the order to use (the mapping's, or month/day).
#[must_use]
pub fn judge_column<'a, I>(values: I, preferred: DateOrder, today: CalendarDate) -> DateColumn
where
    I: IntoIterator<Item = (usize, &'a str)>,
{
    let mut day_first_line = None;
    let mut ambiguous_seen = false;
    let mut all_both = true;
    for (line, text) in values {
        let Ok(parts) = split(text, today) else {
            continue;
        };
        let Parts::Ab(..) = parts else {
            continue;
        };
        let mdy = build(parts, DateOrder::Mdy);
        let dmy = build(parts, DateOrder::Dmy);
        match (mdy, dmy) {
            (Err(_), Ok(_)) => {
                all_both = false;
                if day_first_line.is_none() {
                    day_first_line = Some(line);
                }
            }
            (Ok(a), Ok(b)) if a != b => ambiguous_seen = true,
            (Ok(_), Ok(_)) => {}
            _ => all_both = false,
        }
    }
    DateColumn {
        order: preferred,
        day_first_line: if preferred == DateOrder::Dmy {
            None
        } else {
            day_first_line
        },
        ambiguous: all_both && ambiguous_seen,
    }
}

const MONTHS: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// Splits a date into its parts. Accepts `/`, `-`, `.`, `'` and spaces as
/// separators (Quicken pads with spaces: `1/ 5/26`), 8 digits as
/// `YYYYMMDD`, and English month names (`Nov 3, 2026`, `03-Nov-2026`).
#[allow(clippy::many_single_char_names)] // d/m/y parts read best as letters
fn split(text: &str, today: CalendarDate) -> Result<Parts, ProblemKind> {
    let t = text.trim();
    if t.is_empty() {
        return Err(ProblemKind::MissingDate);
    }
    // Long values are not dates; stop before doing any work on them.
    if t.len() > 40 {
        return Err(ProblemKind::BadDate);
    }
    // A time after the date (`11/03/2026 00:00:00`, `2026-11-03T10:00`).
    let t = t.split(['T', ' ']).find(|s| !s.is_empty()).filter(|s| {
        // Keep spaces when they are padding inside the date (`1/ 5/26`).
        s.contains(['/', '-', '.']) && s.matches(['/', '-', '.', '\'']).count() >= 2
    });
    let t = match t {
        Some(s) => s.to_owned(),
        None => text.trim().to_owned(),
    };
    if t.len() == 8 && t.bytes().all(|b| b.is_ascii_digit()) {
        let n = |r: std::ops::Range<usize>| t[r].parse::<u32>().map_err(|_| ProblemKind::BadDate);
        let y = i32::try_from(n(0..4)?).map_err(|_| ProblemKind::BadDate)?;
        return Ok(Parts::Ymd(y, n(4..6)?, n(6..8)?));
    }
    // Tokens: runs of digits or letters; remember an apostrophe before the
    // last number (Quicken's 2000s marker).
    let mut tokens: Vec<(String, bool)> = Vec::new();
    let mut cur = String::new();
    let mut apostrophe = false;
    for c in t.chars() {
        if c.is_ascii_alphanumeric() {
            if !cur.is_empty()
                && cur.chars().last().is_some_and(|l| l.is_ascii_digit()) != c.is_ascii_digit()
            {
                tokens.push((std::mem::take(&mut cur), false));
            }
            cur.push(c);
        } else if matches!(c, '/' | '-' | '.' | '\'' | ' ' | ',') {
            if !cur.is_empty() {
                tokens.push((std::mem::take(&mut cur), apostrophe));
                apostrophe = false;
            }
            if c == '\'' {
                apostrophe = true;
            }
        } else {
            return Err(ProblemKind::BadDate);
        }
    }
    if !cur.is_empty() {
        tokens.push((cur, apostrophe));
    }
    if tokens.len() != 3 {
        return Err(ProblemKind::BadDate);
    }
    let month_name = |s: &str| {
        let u = s.to_ascii_uppercase();
        MONTHS
            .iter()
            .position(|m| u.len() >= 3 && u.starts_with(m))
            .and_then(|i| u32::try_from(i + 1).ok())
    };
    let num = |s: &str| -> Result<u32, ProblemKind> {
        if s.is_empty() || s.len() > 4 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ProblemKind::BadDate);
        }
        s.parse::<u32>().map_err(|_| ProblemKind::BadDate)
    };
    let year = |s: &str, apos: bool| -> Result<i32, ProblemKind> {
        let v = i32::try_from(num(s)?).map_err(|_| ProblemKind::BadDate)?;
        match s.len() {
            4 => Ok(v),
            1 | 2 => {
                let full = 2000 + v;
                if apos || full <= today.year() + 1 {
                    Ok(full)
                } else {
                    Ok(1900 + v)
                }
            }
            _ => Err(ProblemKind::BadDate),
        }
    };
    let (a, b, c) = (&tokens[0], &tokens[1], &tokens[2]);
    // Month names.
    if let Some(m) = month_name(&a.0) {
        return Ok(Parts::Named {
            month: m,
            day: num(&b.0)?,
            year: year(&c.0, c.1)?,
        });
    }
    if let Some(m) = month_name(&b.0) {
        return Ok(Parts::Named {
            month: m,
            day: num(&a.0)?,
            year: year(&c.0, c.1)?,
        });
    }
    if a.0.len() == 4 {
        let y = i32::try_from(num(&a.0)?).map_err(|_| ProblemKind::BadDate)?;
        return Ok(Parts::Ymd(y, num(&b.0)?, num(&c.0)?));
    }
    if a.0.len() > 2 || b.0.len() > 2 {
        return Err(ProblemKind::BadDate);
    }
    Ok(Parts::Ab(num(&a.0)?, num(&b.0)?, year(&c.0, c.1)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).unwrap()
    }
    fn today() -> CalendarDate {
        d(2026, 11, 7)
    }

    #[test]
    fn reads_common_forms() {
        let p = |s| parse_date(s, DateOrder::Mdy, today());
        assert_eq!(p("11/03/2026"), Ok(d(2026, 11, 3)));
        assert_eq!(p("1/ 5/26"), Ok(d(2026, 1, 5)));
        assert_eq!(p(" 1/ 5'26"), Ok(d(2026, 1, 5)));
        assert_eq!(p("2026-11-03"), Ok(d(2026, 11, 3)));
        assert_eq!(p("20261103"), Ok(d(2026, 11, 3)));
        assert_eq!(p("11-03-2026"), Ok(d(2026, 11, 3)));
        assert_eq!(p("Nov 3, 2026"), Ok(d(2026, 11, 3)));
        assert_eq!(p("03-Nov-2026"), Ok(d(2026, 11, 3)));
        assert_eq!(p("11/03/2026 00:00:00"), Ok(d(2026, 11, 3)));
        assert_eq!(p("2026-11-03T10:15:00"), Ok(d(2026, 11, 3)));
        assert_eq!(p(""), Err(ProblemKind::MissingDate));
        assert_eq!(p("soon"), Err(ProblemKind::BadDate));
        assert_eq!(p(&"1".repeat(1_000_000)), Err(ProblemKind::BadDate));
    }

    #[test]
    fn two_digit_years_follow_q_p25g() {
        let p = |s| parse_date(s, DateOrder::Mdy, today());
        assert_eq!(p("1/5/27"), Ok(d(2027, 1, 5)), "next year is still 20YY");
        assert_eq!(p("1/5/28"), Ok(d(1928, 1, 5)), "two years ahead is 19YY");
        assert_eq!(p("1/5'28"), Ok(d(2028, 1, 5)), "apostrophe is always 20YY");
        assert_eq!(p("1/5/99"), Ok(d(1999, 1, 5)));
    }

    #[test]
    fn orders_and_impossible_dates() {
        assert_eq!(
            parse_date("03/04/2026", DateOrder::Mdy, today()),
            Ok(d(2026, 3, 4))
        );
        assert_eq!(
            parse_date("03/04/2026", DateOrder::Dmy, today()),
            Ok(d(2026, 4, 3))
        );
        assert_eq!(
            parse_date("13/02/2026", DateOrder::Mdy, today()),
            Err(ProblemKind::ImpossibleDate)
        );
        assert_eq!(
            parse_date("13/02/2026", DateOrder::Dmy, today()),
            Ok(d(2026, 2, 13))
        );
        assert_eq!(
            parse_date("02/30/2026", DateOrder::Mdy, today()),
            Err(ProblemKind::ImpossibleDate)
        );
    }

    #[test]
    fn column_verdicts() {
        let ambiguous = judge_column(
            [(2, "03/04/2026"), (3, "05/06/2026")],
            DateOrder::Mdy,
            today(),
        );
        assert_eq!(
            ambiguous,
            DateColumn {
                order: DateOrder::Mdy,
                day_first_line: None,
                ambiguous: true
            }
        );
        let flagged = judge_column(
            [(2, "03/04/2026"), (3, "13/02/2026")],
            DateOrder::Mdy,
            today(),
        );
        assert_eq!(flagged.day_first_line, Some(3));
        assert!(!flagged.ambiguous);
        let bad = judge_column(
            [(2, "02/30/2026"), (3, "01/15/2026")],
            DateOrder::Mdy,
            today(),
        );
        assert_eq!(bad.day_first_line, None);
        let iso = judge_column([(2, "2026-03-04")], DateOrder::Mdy, today());
        assert!(!iso.ambiguous);
    }
}
