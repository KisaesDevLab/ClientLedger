//! Date field keys and date warnings (DESIGN.md "Date fields", Q-97).
//!
//! Pure functions over [`CalendarDate`]; "today" and the first weekday are
//! passed in (cl-core has no clock).

use cl_core::{CalendarDate, Weekday};

/// What a typed character does to a date field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateKey {
    /// `+`: next day.
    NextDay,
    /// `-`: previous day.
    PrevDay,
    /// `T`: today.
    Today,
    /// `M`: first day of the month.
    MonthStart,
    /// `H`: last day of the month.
    MonthEnd,
    /// `Y`: first day of the year.
    YearStart,
    /// `R`: last day of the year.
    YearEnd,
    /// `W`: first day of the week.
    WeekStart,
    /// `K`: last day of the week.
    WeekEnd,
}

impl DateKey {
    /// Map a typed character (case-insensitive; `=` counts as `+` because
    /// `+` needs Shift on most keyboards).
    #[must_use]
    pub fn from_char(c: char) -> Option<DateKey> {
        match c.to_ascii_lowercase() {
            '+' | '=' => Some(DateKey::NextDay),
            '-' | '_' => Some(DateKey::PrevDay),
            't' => Some(DateKey::Today),
            'm' => Some(DateKey::MonthStart),
            'h' => Some(DateKey::MonthEnd),
            'y' => Some(DateKey::YearStart),
            'r' => Some(DateKey::YearEnd),
            'w' => Some(DateKey::WeekStart),
            'k' => Some(DateKey::WeekEnd),
            _ => None,
        }
    }
}

/// Apply a date key. `None` when the result would leave the supported range
/// (the field keeps its value).
#[must_use]
pub fn apply_date_key(
    key: DateKey,
    current: CalendarDate,
    today: CalendarDate,
    first_weekday: Weekday,
) -> Option<CalendarDate> {
    match key {
        DateKey::NextDay => current.add_days(1).ok(),
        DateKey::PrevDay => current.add_days(-1).ok(),
        DateKey::Today => Some(today),
        DateKey::MonthStart => Some(current.start_of_month()),
        DateKey::MonthEnd => Some(current.end_of_month()),
        DateKey::YearStart => Some(current.start_of_year()),
        DateKey::YearEnd => Some(current.end_of_year()),
        DateKey::WeekStart => current.week_start(first_weekday).ok(),
        DateKey::WeekEnd => current
            .week_start(first_weekday)
            .ok()
            .and_then(|s| s.add_days(6).ok()),
    }
}

/// Thresholds for "that date looks wrong" (Q-97: more than 90 days back or
/// more than 30 days ahead; configurable in Preferences, P09).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateWarnRules {
    /// Warn when the date is more than this many days before today.
    pub past_days: u32,
    /// Warn when the date is more than this many days after today.
    pub future_days: u32,
}

impl Default for DateWarnRules {
    fn default() -> Self {
        DateWarnRules {
            past_days: 90,
            future_days: 30,
        }
    }
}

/// A date the owner should double-check before saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateWarning {
    /// Further back than the limit.
    TooFarPast {
        /// How many days before today.
        days: u32,
    },
    /// Further ahead than the limit.
    TooFarFuture {
        /// How many days after today.
        days: u32,
    },
}

impl DateWarnRules {
    /// The warning for `date`, if any. Exactly at the limit is fine.
    #[must_use]
    pub fn check(&self, date: CalendarDate, today: CalendarDate) -> Option<DateWarning> {
        let diff = today.days_until(date);
        if diff < 0 {
            let days = u32::try_from(diff.unsigned_abs()).unwrap_or(u32::MAX);
            (days > self.past_days).then_some(DateWarning::TooFarPast { days })
        } else {
            let days = u32::try_from(diff).unwrap_or(u32::MAX);
            (days > self.future_days).then_some(DateWarning::TooFarFuture { days })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, day).unwrap()
    }

    #[test]
    fn character_mapping() {
        assert_eq!(DateKey::from_char('+'), Some(DateKey::NextDay));
        assert_eq!(DateKey::from_char('='), Some(DateKey::NextDay));
        assert_eq!(DateKey::from_char('-'), Some(DateKey::PrevDay));
        assert_eq!(DateKey::from_char('T'), Some(DateKey::Today));
        assert_eq!(DateKey::from_char('k'), Some(DateKey::WeekEnd));
        assert_eq!(DateKey::from_char('x'), None);
        assert_eq!(DateKey::from_char('5'), None);
    }

    #[test]
    fn keys_move_the_date() {
        let today = d(2026, 11, 4); // a Wednesday
        let cur = d(2026, 11, 4);
        let sun = Weekday::Sun;
        let go = |k| apply_date_key(k, cur, today, sun).unwrap();
        assert_eq!(go(DateKey::NextDay), d(2026, 11, 5));
        assert_eq!(go(DateKey::PrevDay), d(2026, 11, 3));
        assert_eq!(go(DateKey::Today), today);
        assert_eq!(go(DateKey::MonthStart), d(2026, 11, 1));
        assert_eq!(go(DateKey::MonthEnd), d(2026, 11, 30));
        assert_eq!(go(DateKey::YearStart), d(2026, 1, 1));
        assert_eq!(go(DateKey::YearEnd), d(2026, 12, 31));
        assert_eq!(go(DateKey::WeekStart), d(2026, 11, 1));
        assert_eq!(go(DateKey::WeekEnd), d(2026, 11, 7));
    }

    #[test]
    fn today_ignores_the_current_value_and_week_respects_first_day() {
        let today = d(2026, 11, 4);
        assert_eq!(
            apply_date_key(DateKey::Today, d(2019, 3, 3), today, Weekday::Sun),
            Some(today)
        );
        assert_eq!(
            apply_date_key(DateKey::WeekStart, today, today, Weekday::Mon),
            Some(d(2026, 11, 2))
        );
        assert_eq!(
            apply_date_key(DateKey::WeekEnd, today, today, Weekday::Mon),
            Some(d(2026, 11, 8))
        );
    }

    #[test]
    fn month_and_year_edges() {
        let today = d(2026, 11, 4);
        let leap = d(2028, 2, 10);
        assert_eq!(
            apply_date_key(DateKey::MonthEnd, leap, today, Weekday::Sun),
            Some(d(2028, 2, 29))
        );
        assert_eq!(
            apply_date_key(DateKey::NextDay, d(2026, 12, 31), today, Weekday::Sun),
            Some(d(2027, 1, 1))
        );
        // The supported range ends 2199-12-31: stepping past it is refused.
        assert_eq!(
            apply_date_key(DateKey::NextDay, CalendarDate::max(), today, Weekday::Sun),
            None
        );
    }

    #[test]
    fn warnings_use_the_q97_defaults() {
        let rules = DateWarnRules::default();
        let today = d(2026, 11, 4);
        assert_eq!(rules.check(today, today), None);
        // Exactly 90 days back and 30 days ahead are fine.
        assert_eq!(rules.check(today.add_days(-90).unwrap(), today), None);
        assert_eq!(rules.check(today.add_days(30).unwrap(), today), None);
        assert_eq!(
            rules.check(today.add_days(-91).unwrap(), today),
            Some(DateWarning::TooFarPast { days: 91 })
        );
        assert_eq!(
            rules.check(today.add_days(31).unwrap(), today),
            Some(DateWarning::TooFarFuture { days: 31 })
        );
    }

    #[test]
    fn warnings_are_configurable() {
        let rules = DateWarnRules {
            past_days: 7,
            future_days: 0,
        };
        let today = d(2026, 11, 4);
        assert!(rules.check(today.add_days(-8).unwrap(), today).is_some());
        assert!(rules.check(today.add_days(1).unwrap(), today).is_some());
        assert!(rules.check(today.add_days(-7).unwrap(), today).is_none());
    }
}
