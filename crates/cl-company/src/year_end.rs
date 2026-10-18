//! Year-end (Q-48): the engine rolls net income into retained earnings by
//! itself (virtually, on every report), so there is no closing entry to
//! post. What the app does is prompt the owner to set a closing date after
//! a fiscal year ends, once per year end.

use cl_core::{CalendarDate, FiscalYearSpec, RetailPattern, Weekday, YearEndRule};
use cl_settings::FiscalCalendar;

/// The fiscal year spec for a company's settings. Retail calendars end on
/// the last Saturday of the month before `fiscal_year_start_month`
/// (Q-P08j; the weekday rule becomes a preference with U-25).
#[must_use]
pub fn fiscal_spec(fiscal_year_start_month: u8, calendar: FiscalCalendar) -> FiscalYearSpec {
    let start_month = fiscal_year_start_month.clamp(1, 12);
    let pattern = match calendar {
        FiscalCalendar::Standard => return FiscalYearSpec::Calendar { start_month },
        FiscalCalendar::Retail445 => RetailPattern::P445,
        FiscalCalendar::Retail454 => RetailPattern::P454,
        FiscalCalendar::Retail544 => RetailPattern::P544,
    };
    FiscalYearSpec::Retail {
        pattern,
        year_end_month: if start_month == 1 {
            12
        } else {
            start_month - 1
        },
        end_rule: YearEndRule::LastWeekday(Weekday::Sat),
    }
}

/// A prompt to show after a fiscal year ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct YearEndPrompt {
    /// The last day of the fiscal year that just ended.
    pub fiscal_year_ended: CalendarDate,
    /// The closing date to suggest (the same day).
    pub suggested_closing_date: CalendarDate,
}

/// Whether to prompt today. `closing_date` is the company's current
/// closing date; `last_prompted_for` is the year end the owner was last
/// asked about (dismissed or set), kept locally.
#[must_use]
pub fn year_end_prompt(
    today: CalendarDate,
    fiscal_year_start_month: u8,
    calendar: FiscalCalendar,
    closing_date: Option<CalendarDate>,
    last_prompted_for: Option<CalendarDate>,
) -> Option<YearEndPrompt> {
    let spec = fiscal_spec(fiscal_year_start_month, calendar);
    let current = spec.year_containing(today).ok()?;
    let ended = current.start.add_days(-1).ok()?;
    if closing_date.is_some_and(|c| c >= ended) {
        return None;
    }
    if last_prompted_for == Some(ended) {
        return None;
    }
    Some(YearEndPrompt {
        fiscal_year_ended: ended,
        suggested_closing_date: ended,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).expect("date")
    }

    #[test]
    fn prompts_once_after_the_year_end_until_the_books_are_closed() {
        let std = FiscalCalendar::Standard;
        let p = year_end_prompt(d(2026, 1, 5), 1, std, None, None).expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2025, 12, 31));
        assert_eq!(p.suggested_closing_date, d(2025, 12, 31));
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, std, Some(d(2025, 12, 31)), None),
            None,
            "already closed through the year end"
        );
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, std, Some(d(2025, 9, 30)), None)
                .map(|p| p.fiscal_year_ended),
            Some(d(2025, 12, 31)),
            "closed only through September"
        );
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, std, None, Some(d(2025, 12, 31))),
            None,
            "already asked"
        );
        // A July fiscal year: on 2026-08-01 the year that ended 2026-06-30
        // is the one to prompt about.
        let p = year_end_prompt(d(2026, 8, 1), 7, std, None, None).expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2026, 6, 30));
        assert_eq!(
            year_end_prompt(d(2026, 8, 1), 7, std, Some(d(2026, 6, 30)), None),
            None
        );
    }

    #[test]
    fn retail_calendars_end_on_the_last_saturday() {
        // A 4-4-5 year starting in February ends on the last Saturday of
        // January: 2026-01-31 is a Saturday.
        let p = year_end_prompt(d(2026, 2, 10), 2, FiscalCalendar::Retail445, None, None)
            .expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2026, 1, 31));
        assert_eq!(p.fiscal_year_ended.weekday(), Weekday::Sat);
        // Standard months for the same start give January 31 too, but a
        // calendar-year 4-5-4 company ends on the last Saturday of December.
        let p = year_end_prompt(d(2026, 1, 5), 1, FiscalCalendar::Retail454, None, None)
            .expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2025, 12, 27));
    }
}
