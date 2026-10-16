//! Fiscal years, periods and quarters.
//!
//! Pure calendar maths. P08 stores the owner's choice and P28 uses the report
//! presets; the engine only knows `fiscalYearStartMonth` and reports on the
//! dates the UI sends.
//!
//! Two kinds of fiscal year:
//! - **Calendar months** starting in any month. Twelve monthly periods.
//! - **Retail 52/53-week** years (4-4-5, 4-5-4, 5-4-4) that end on a fixed
//!   weekday: either the last such weekday of the year-end month, or the one
//!   nearest the month's last day. Twelve periods of whole weeks; in a
//!   53-week year the extra week goes to period 12 (Q-P05q).
//!
//! A fiscal year's label is the calendar year of its year-end month (Q-P05q,
//! Q-P05w): a year running July 2026 – June 2027 is FY 2027.

use crate::dates::{CalendarDate, DateError, Weekday};

/// Why a fiscal calendar cannot be computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FiscalError {
    /// A month outside 1–12.
    #[error("fiscal month must be 1 to 12")]
    BadMonth,
    /// The fiscal year reaches outside the supported date range.
    #[error("fiscal year is outside the supported date range")]
    OutOfRange,
}

impl From<DateError> for FiscalError {
    fn from(_: DateError) -> Self {
        FiscalError::OutOfRange
    }
}

/// Weeks per period within each quarter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RetailPattern {
    /// 4, 4, 5 weeks.
    P445,
    /// 4, 5, 4 weeks.
    P454,
    /// 5, 4, 4 weeks.
    P544,
}

impl RetailPattern {
    /// Weeks in each of the twelve periods of a 52-week year.
    #[must_use]
    pub fn weeks(self) -> [u8; 12] {
        let q = match self {
            RetailPattern::P445 => [4, 4, 5],
            RetailPattern::P454 => [4, 5, 4],
            RetailPattern::P544 => [5, 4, 4],
        };
        [
            q[0], q[1], q[2], q[0], q[1], q[2], q[0], q[1], q[2], q[0], q[1], q[2],
        ]
    }
}

/// Where a retail fiscal year ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum YearEndRule {
    /// The last given weekday of the year-end month.
    LastWeekday(Weekday),
    /// The given weekday nearest the last day of the year-end month (may fall
    /// up to three days into the next month).
    NearestToMonthEnd(Weekday),
}

/// The owner's fiscal year setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FiscalYearSpec {
    /// Calendar months; the year starts on the first of `start_month`.
    Calendar {
        /// First month of the fiscal year, 1–12 (1 = calendar year).
        start_month: u8,
    },
    /// A 52/53-week year.
    Retail {
        /// Weeks per period.
        pattern: RetailPattern,
        /// The month the year ends in, 1–12.
        year_end_month: u8,
        /// The weekday rule for the last day.
        end_rule: YearEndRule,
    },
}

/// One fiscal year.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FiscalYear {
    /// The calendar year of the year-end month.
    pub label: i32,
    /// First day.
    pub start: CalendarDate,
    /// Last day.
    pub end: CalendarDate,
}

impl FiscalYear {
    /// Number of days.
    #[must_use]
    pub fn days(&self) -> i64 {
        self.start.days_until(self.end) + 1
    }

    /// True when `date` falls in this year.
    #[must_use]
    pub fn contains(&self, date: CalendarDate) -> bool {
        self.start <= date && date <= self.end
    }
}

/// A fiscal period (month-like) or quarter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Period {
    /// 1–12 for periods, 1–4 for quarters.
    pub number: u8,
    /// First day.
    pub start: CalendarDate,
    /// Last day.
    pub end: CalendarDate,
}

impl Period {
    /// True when `date` falls in this period.
    #[must_use]
    pub fn contains(&self, date: CalendarDate) -> bool {
        self.start <= date && date <= self.end
    }
}

fn month(m: u8) -> Result<u32, FiscalError> {
    if (1..=12).contains(&m) {
        Ok(u32::from(m))
    } else {
        Err(FiscalError::BadMonth)
    }
}

/// Days from `from` back to the previous (or same) `to` weekday, 0–6.
fn days_back(from: Weekday, to: Weekday) -> u32 {
    (7 + from.num_days_from_sunday() - to.num_days_from_sunday()) % 7
}

impl FiscalYearSpec {
    /// The calendar year (January start).
    pub const CALENDAR_YEAR: FiscalYearSpec = FiscalYearSpec::Calendar { start_month: 1 };

    /// The fiscal year with this label.
    ///
    /// # Errors
    /// [`FiscalError::BadMonth`] for an invalid spec;
    /// [`FiscalError::OutOfRange`] near the ends of the supported dates.
    pub fn year(&self, label: i32) -> Result<FiscalYear, FiscalError> {
        match *self {
            FiscalYearSpec::Calendar { start_month } => {
                let m = month(start_month)?;
                let start_year = if m == 1 { label } else { label - 1 };
                let start = CalendarDate::from_ymd(start_year, m, 1)?;
                let end = start.add_months(12)?.add_days(-1)?;
                Ok(FiscalYear { label, start, end })
            }
            FiscalYearSpec::Retail {
                year_end_month,
                end_rule,
                ..
            } => {
                let m = month(year_end_month)?;
                let end = retail_year_end(label, m, end_rule)?;
                let start = retail_year_end(label - 1, m, end_rule)?.add_days(1)?;
                Ok(FiscalYear { label, start, end })
            }
        }
    }

    /// The fiscal year that contains `date`.
    ///
    /// # Errors
    /// As [`FiscalYearSpec::year`].
    pub fn year_containing(&self, date: CalendarDate) -> Result<FiscalYear, FiscalError> {
        // The label is the date's year or one either side of it.
        for label in [date.year(), date.year() + 1, date.year() - 1] {
            let fy = self.year(label)?;
            if fy.contains(date) {
                return Ok(fy);
            }
        }
        Err(FiscalError::OutOfRange)
    }

    /// The twelve periods of `fy`, contiguous and covering it exactly.
    ///
    /// # Errors
    /// As [`FiscalYearSpec::year`].
    pub fn periods(&self, fy: &FiscalYear) -> Result<[Period; 12], FiscalError> {
        let mut out = Vec::with_capacity(12);
        match *self {
            FiscalYearSpec::Calendar { start_month } => {
                month(start_month)?;
                for i in 0..12u8 {
                    let start = fy.start.add_months(i32::from(i))?;
                    let end = if i == 11 {
                        fy.end
                    } else {
                        start.add_months(1)?.add_days(-1)?
                    };
                    out.push(Period {
                        number: i + 1,
                        start,
                        end,
                    });
                }
            }
            FiscalYearSpec::Retail {
                pattern,
                year_end_month,
                ..
            } => {
                month(year_end_month)?;
                let mut start = fy.start;
                for (i, weeks) in (0u8..).zip(pattern.weeks()) {
                    let end = if i == 11 {
                        // Period 12 takes the rest: 4 or 5 weeks, plus the
                        // 53rd week when there is one.
                        fy.end
                    } else {
                        start.add_days(7 * i64::from(weeks) - 1)?
                    };
                    out.push(Period {
                        number: i + 1,
                        start,
                        end,
                    });
                    start = end.add_days(1).unwrap_or(end);
                }
            }
        }
        <[Period; 12]>::try_from(out).map_err(|_| FiscalError::OutOfRange)
    }

    /// The four quarters of `fy` (three periods each).
    ///
    /// # Errors
    /// As [`FiscalYearSpec::year`].
    pub fn quarters(&self, fy: &FiscalYear) -> Result<[Period; 4], FiscalError> {
        let p = self.periods(fy)?;
        let q = |n: u8, a: usize, b: usize| Period {
            number: n,
            start: p[a].start,
            end: p[b].end,
        };
        Ok([q(1, 0, 2), q(2, 3, 5), q(3, 6, 8), q(4, 9, 11)])
    }

    /// The period that contains `date`.
    ///
    /// # Errors
    /// As [`FiscalYearSpec::year`].
    pub fn period_containing(&self, date: CalendarDate) -> Result<Period, FiscalError> {
        let fy = self.year_containing(date)?;
        self.periods(&fy)?
            .into_iter()
            .find(|p| p.contains(date))
            .ok_or(FiscalError::OutOfRange)
    }
}

/// The last day of the retail fiscal year whose year-end month is
/// `month` of `year`.
fn retail_year_end(year: i32, month: u32, rule: YearEndRule) -> Result<CalendarDate, FiscalError> {
    let last = CalendarDate::from_ymd(year, month, 1)?.end_of_month();
    let end = match rule {
        YearEndRule::LastWeekday(w) => last.add_days(-i64::from(days_back(last.weekday(), w)))?,
        YearEndRule::NearestToMonthEnd(w) => {
            let back = days_back(last.weekday(), w);
            if back <= 3 {
                last.add_days(-i64::from(back))?
            } else {
                last.add_days(i64::from(7 - back))?
            }
        }
    };
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> CalendarDate {
        CalendarDate::parse_wire(s).unwrap()
    }

    #[test]
    fn calendar_year_and_july_start() {
        let fy = FiscalYearSpec::CALENDAR_YEAR
            .year_containing(d("2026-10-15"))
            .unwrap();
        assert_eq!(
            (fy.label, fy.start, fy.end),
            (2026, d("2026-01-01"), d("2026-12-31"))
        );
        let july = FiscalYearSpec::Calendar { start_month: 7 };
        let fy = july.year_containing(d("2026-10-15")).unwrap();
        assert_eq!(
            (fy.label, fy.start, fy.end),
            (2027, d("2026-07-01"), d("2027-06-30"))
        );
        let fy = july.year_containing(d("2026-06-30")).unwrap();
        assert_eq!(fy.label, 2026);
        let p = july.periods(&fy).unwrap();
        assert_eq!((p[0].start, p[0].end), (d("2025-07-01"), d("2025-07-31")));
        assert_eq!((p[7].start, p[7].end), (d("2026-02-01"), d("2026-02-28")));
        assert_eq!(p[11].end, d("2026-06-30"));
        let q = july.quarters(&fy).unwrap();
        assert_eq!(
            (q[1].number, q[1].start, q[1].end),
            (2, d("2025-10-01"), d("2025-12-31"))
        );
        let p = july.period_containing(d("2026-02-14")).unwrap();
        assert_eq!(p.number, 8);
    }

    #[test]
    fn nrf_style_454_with_a_53_week_year() {
        // Saturday nearest January 31: the year ending 2024-02-03 has 53 weeks.
        let spec = FiscalYearSpec::Retail {
            pattern: RetailPattern::P454,
            year_end_month: 1,
            end_rule: YearEndRule::NearestToMonthEnd(Weekday::Sat),
        };
        let fy = spec.year_containing(d("2023-06-01")).unwrap();
        assert_eq!(
            (fy.label, fy.start, fy.end),
            (2024, d("2023-01-29"), d("2024-02-03"))
        );
        assert_eq!(fy.days(), 371);
        let p = spec.periods(&fy).unwrap();
        assert_eq!((p[0].start, p[0].end), (d("2023-01-29"), d("2023-02-25")));
        assert_eq!(p[1].start.days_until(p[1].end) + 1, 35);
        assert_eq!(p[11].start.days_until(p[11].end) + 1, 35); // 4 weeks + the 53rd
        let next = spec.year(2025).unwrap();
        assert_eq!(
            (next.start, next.end, next.days()),
            (d("2024-02-04"), d("2025-02-01"), 364)
        );
    }

    #[test]
    fn last_weekday_rule_and_445() {
        // Last Saturday of December.
        let spec = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 12,
            end_rule: YearEndRule::LastWeekday(Weekday::Sat),
        };
        let fy = spec.year(2026).unwrap();
        assert_eq!((fy.start, fy.end), (d("2025-12-28"), d("2026-12-26")));
        let p = spec.periods(&fy).unwrap();
        assert_eq!(p[2].start.days_until(p[2].end) + 1, 35);
        assert_eq!(p[11].start.days_until(p[11].end) + 1, 35);
        let q = spec.quarters(&fy).unwrap();
        assert_eq!(q[0].start.days_until(q[0].end) + 1, 91);
        // Nearest rule spilling into the next month keeps the nominal label.
        let near = FiscalYearSpec::Retail {
            pattern: RetailPattern::P544,
            year_end_month: 12,
            end_rule: YearEndRule::NearestToMonthEnd(Weekday::Sat),
        };
        let fy = near.year_containing(d("2027-01-01")).unwrap();
        assert_eq!((fy.label, fy.end), (2026, d("2027-01-02")));
        assert_eq!(RetailPattern::P544.weeks()[0], 5);
    }

    #[test]
    fn invalid_specs_and_range() {
        assert_eq!(
            FiscalYearSpec::Calendar { start_month: 13 }.year(2026),
            Err(FiscalError::BadMonth)
        );
        let bad = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 0,
            end_rule: YearEndRule::LastWeekday(Weekday::Sun),
        };
        assert_eq!(bad.year(2026), Err(FiscalError::BadMonth));
        assert_eq!(
            FiscalYearSpec::CALENDAR_YEAR.year(1899),
            Err(FiscalError::OutOfRange)
        );
        let july = FiscalYearSpec::Calendar { start_month: 7 };
        assert_eq!(
            july.year_containing(d("2199-12-31")),
            Err(FiscalError::OutOfRange)
        );
        assert!(!july.year(2026).unwrap().contains(d("2026-07-01")));
    }
}
