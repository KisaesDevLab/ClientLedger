//! Year-end (Q-48): the engine rolls net income into retained earnings by
//! itself (virtually, on every report), so there is no closing entry to
//! post. What the app does is prompt the owner to set a closing date after
//! a fiscal year ends, once per year end.

use cl_core::{CalendarDate, FiscalYearSpec};

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
    closing_date: Option<CalendarDate>,
    last_prompted_for: Option<CalendarDate>,
) -> Option<YearEndPrompt> {
    let spec = FiscalYearSpec::Calendar {
        start_month: fiscal_year_start_month.clamp(1, 12),
    };
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
        let p = year_end_prompt(d(2026, 1, 5), 1, None, None).expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2025, 12, 31));
        assert_eq!(p.suggested_closing_date, d(2025, 12, 31));
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, Some(d(2025, 12, 31)), None),
            None,
            "already closed through the year end"
        );
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, Some(d(2025, 9, 30)), None)
                .map(|p| p.fiscal_year_ended),
            Some(d(2025, 12, 31)),
            "closed only through September"
        );
        assert_eq!(
            year_end_prompt(d(2026, 1, 5), 1, None, Some(d(2025, 12, 31))),
            None,
            "already asked"
        );
        // A July fiscal year: on 2026-08-01 the year that ended 2026-06-30
        // is the one to prompt about.
        let p = year_end_prompt(d(2026, 8, 1), 7, None, None).expect("prompt");
        assert_eq!(p.fiscal_year_ended, d(2026, 6, 30));
        assert_eq!(
            year_end_prompt(d(2026, 8, 1), 7, Some(d(2026, 6, 30)), None),
            None
        );
    }
}
