//! Document dates (WP-A, step 8; plan D3 `dates.rs`, S6). Our own grammar
//! (the engine accepts ISO only): US month/day/year by default; a first
//! number over 12 means day-first; both 12 or less → US at medium
//! confidence plus a "could be day-first" check; two-digit years are 20yy;
//! month names, ordinals and ISO are read too.
//!
//! Accepted shapes (case-insensitive; a leading weekday name is skipped):
//!
//! | Shape | Example |
//! | --- | --- |
//! | ISO | `2026-07-14`, `2026/7/14` |
//! | numeric, same separator `/`, `-` or `.` | `07/14/2026`, `7-14-26`, `14.07.2026` |
//! | month name first | `July 14, 2026`, `Jul 14th 2026`, `Sept. 3 2026`, `Jul-14-2026` |
//! | day first | `14 July 2026`, `3rd Sep 2026`, `14-Jul-26` |
//!
//! **Consumers:** the rules and the AI gate; the signature of [`parse`] is
//! fixed.

use crate::model::Confidence;
use cl_core::CalendarDate;

/// A date read from text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedDate {
    pub date: CalendarDate,
    pub confidence: Confidence,
    /// Both numbers were 12 or less: the caller adds a
    /// `DateCouldBeDayFirst` check.
    pub could_be_day_first: bool,
}

/// Confidence of an unambiguous date.
const SURE: u16 = 9_500;
/// A day-first numeric date (first number over 12): unusual on US
/// documents.
const DAY_FIRST: u16 = 8_000;
/// Both numbers 12 or less, read month first (S6: medium).
const AMBIGUOUS: u16 = 7_000;
/// Taken off for a two-digit year.
const SHORT_YEAR_PENALTY: u16 = 500;
/// Cap for a date far from "today" (more than 10 years back or 2 ahead).
const IMPLAUSIBLE: u16 = 4_000;

/// The date printed in `text`, or `None`. `today` bounds two-digit years
/// and implausible dates.
#[must_use]
pub fn parse(text: &str, today: CalendarDate) -> Option<ParsedDate> {
    let text = text
        .trim()
        .trim_end_matches([',', ';', ':'])
        .trim_end_matches('.')
        .trim();
    if text.is_empty() || text.chars().count() > 40 {
        return None;
    }
    let (date, bp, ambiguous) = numeric(text).or_else(|| named(text))?;
    let mut bp = bp;
    if !plausible(date, today) {
        bp = bp.min(IMPLAUSIBLE);
    }
    Some(ParsedDate {
        date,
        confidence: Confidence::saturating(u32::from(bp)),
        could_be_day_first: ambiguous,
    })
}

fn plausible(date: CalendarDate, today: CalendarDate) -> bool {
    let low = today
        .add_months(-120)
        .unwrap_or_else(|_| CalendarDate::min());
    let high = today.add_months(24).unwrap_or_else(|_| CalendarDate::max());
    (low..=high).contains(&date)
}

/// `y-m-d` or `a/b/y` with one separator kind.
fn numeric(text: &str) -> Option<(CalendarDate, u16, bool)> {
    let sep = text.chars().find(|c| matches!(c, '/' | '-' | '.'))?;
    let parts: Vec<&str> = text.split(sep).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || p.len() > 4 || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let n = |p: &str| p.parse::<u32>().ok();
    if parts[0].len() == 4 {
        // ISO order.
        if parts[1].len() > 2 || parts[2].len() > 2 {
            return None;
        }
        let year = i32::try_from(n(parts[0])?).ok()?;
        let date = CalendarDate::from_ymd(year, n(parts[1])?, n(parts[2])?).ok()?;
        return Some((date, SURE, false));
    }
    if parts[0].len() > 2 || parts[1].len() > 2 {
        return None;
    }
    let (year, short) = year_of(parts[2])?;
    let (a, b) = (n(parts[0])?, n(parts[1])?);
    let (month, day, bp, ambiguous) = if a > 12 {
        (b, a, DAY_FIRST, false)
    } else if b > 12 || a == b {
        (a, b, SURE, false)
    } else {
        (a, b, AMBIGUOUS, true)
    };
    let date = CalendarDate::from_ymd(year, month, day).ok()?;
    Some((date, penalize(bp, short), ambiguous))
}

/// Dates with a month name.
fn named(text: &str) -> Option<(CalendarDate, u16, bool)> {
    let mut tokens: Vec<String> = text
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '-' | '/' | '.'))
        .filter(|t| !t.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if tokens.first().is_some_and(|t| weekday(t)) {
        tokens.remove(0);
    }
    if tokens.len() != 3 {
        return None;
    }
    let (month, day_tok) = if let Some(m) = month(&tokens[0]) {
        (m, &tokens[1])
    } else {
        (month(&tokens[1])?, &tokens[0])
    };
    let day = day_of(day_tok)?;
    let (year, short) = year_of(&tokens[2])?;
    let date = CalendarDate::from_ymd(year, month, day).ok()?;
    Some((date, penalize(SURE, short), false))
}

fn penalize(bp: u16, short_year: bool) -> u16 {
    if short_year {
        bp.saturating_sub(SHORT_YEAR_PENALTY)
    } else {
        bp
    }
}

/// A 4-digit year, or a 2-digit year as 20yy (`true` = short).
fn year_of(t: &str) -> Option<(i32, bool)> {
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let v = i32::try_from(t.parse::<u32>().ok()?).ok()?;
    match t.len() {
        2 => Some((2000 + v, true)),
        4 => Some((v, false)),
        _ => None,
    }
}

/// `14`, `14th`, `1st`, `2nd`, `3rd` (the suffix must fit the number).
fn day_of(t: &str) -> Option<u32> {
    let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || digits.len() > 2 {
        return None;
    }
    let suffix = &t[digits.len()..];
    let d: u32 = digits.parse().ok()?;
    let want = match (d % 100, d % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    (suffix.is_empty() || suffix == want).then_some(d)
}

fn month(t: &str) -> Option<u32> {
    const NAMES: [(&str, &str); 12] = [
        ("jan", "january"),
        ("feb", "february"),
        ("mar", "march"),
        ("apr", "april"),
        ("may", "may"),
        ("jun", "june"),
        ("jul", "july"),
        ("aug", "august"),
        ("sep", "september"),
        ("oct", "october"),
        ("nov", "november"),
        ("dec", "december"),
    ];
    let pos = NAMES
        .iter()
        .position(|(short, long)| t == *short || t == *long || (t == "sept" && *short == "sep"))?;
    u32::try_from(pos + 1).ok()
}

fn weekday(t: &str) -> bool {
    const DAYS: [&str; 7] = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    DAYS.iter()
        .any(|d| t == *d || (t.len() == 3 && d.starts_with(t)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 10, 26).unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, d).unwrap()
    }

    fn read(s: &str) -> Option<(CalendarDate, u16, bool)> {
        parse(s, today()).map(|p| (p.date, p.confidence.bp(), p.could_be_day_first))
    }

    #[test]
    fn the_grammar_table() {
        type Want = Option<(CalendarDate, u16, bool)>;
        let cases: [(&str, Want); 22] = [
            ("2026-07-14", Some((ymd(2026, 7, 14), SURE, false))),
            ("2026/7/4", Some((ymd(2026, 7, 4), SURE, false))),
            ("07/14/2026", Some((ymd(2026, 7, 14), SURE, false))),
            ("14/07/2026", Some((ymd(2026, 7, 14), DAY_FIRST, false))),
            ("7-14-26", Some((ymd(2026, 7, 14), SURE - 500, false))),
            ("14.07.2026", Some((ymd(2026, 7, 14), DAY_FIRST, false))),
            ("07/07/2026", Some((ymd(2026, 7, 7), SURE, false))),
            ("July 14, 2026", Some((ymd(2026, 7, 14), SURE, false))),
            ("Jul 14th 2026", Some((ymd(2026, 7, 14), SURE, false))),
            ("Sept. 3, 2026", Some((ymd(2026, 9, 3), SURE, false))),
            (
                "Tuesday, July 14, 2026",
                Some((ymd(2026, 7, 14), SURE, false)),
            ),
            ("14 July 2026", Some((ymd(2026, 7, 14), SURE, false))),
            ("3rd Sep 2026", Some((ymd(2026, 9, 3), SURE, false))),
            ("14-Jul-26", Some((ymd(2026, 7, 14), SURE - 500, false))),
            ("JUL-14-2026", Some((ymd(2026, 7, 14), SURE, false))),
            ("02/30/2026", None),
            ("13/13/2026", None),
            ("3th Sep 2026", None),
            ("July 2026", None),
            ("07/14/2026/1", None),
            ("07/14-2026", None),
            ("Net 30", None),
        ];
        for (text, want) in cases {
            assert_eq!(read(text), want, "{text}");
        }
    }

    #[test]
    fn upper_case_receipt_dates() {
        assert_eq!(read("MAR 04 2026").map(|r| r.0), Some(ymd(2026, 3, 4)));
        assert_eq!(read("MAY 22 2026").map(|r| r.0), Some(ymd(2026, 5, 22)));
        assert_eq!(read("06/06/26").map(|r| r.0), Some(ymd(2026, 6, 6)));
    }

    /// T12: both numbers 12 or less → US reading at medium confidence and
    /// a "could be day-first" flag.
    #[test]
    fn ambiguous_numeric_dates_are_medium_and_flagged() {
        let p = parse("07/08/2026", today()).unwrap();
        assert_eq!(p.date, ymd(2026, 7, 8));
        assert_eq!(
            p.confidence.bucket(),
            crate::model::ConfidenceBucket::Medium
        );
        assert!(p.could_be_day_first);
    }

    #[test]
    fn far_dates_are_low_but_kept() {
        let p = parse("01/15/2001", today()).unwrap();
        assert_eq!(p.date, ymd(2001, 1, 15));
        assert_eq!(p.confidence.bp(), IMPLAUSIBLE);
        let due = parse("12/31/2027", today()).unwrap();
        assert_eq!(due.confidence.bp(), SURE, "future due dates are fine");
    }

    proptest! {
        #[test]
        fn printed_dates_round_trip(days in 0_i64..20_000, form in 0_u8..4) {
            let d = ymd(2000, 1, 1).add_days(days).unwrap();
            let names = ["January","February","March","April","May","June","July",
                "August","September","October","November","December"];
            let name = names[usize::try_from(d.month() - 1).unwrap()];
            let text = match form {
                0 => d.to_wire(),
                1 => format!("{:02}/{:02}/{}", d.month(), d.day(), d.year()),
                2 => format!("{name} {}, {}", d.day(), d.year()),
                _ => format!("{} {name} {}", d.day(), d.year()),
            };
            let p = parse(&text, today()).unwrap();
            prop_assert_eq!(p.date, d);
            prop_assert_eq!(p.could_be_day_first, form == 1 && d.day() <= 12 && d.day() != d.month());
        }

        #[test]
        fn arbitrary_text_is_safe(s in "\\PC{0,48}") {
            let _ = parse(&s, today());
        }
    }
}
