//! Relative date presets (plan WP1 step 3): this or last month, quarter
//! or year on the company's fiscal calendar (calendar months or 4-4-5),
//! year to date, as of today and the end of last month. A "month" is the
//! fiscal period, so a 4-4-5 company gets its own periods.

use cl_core::fiscal::{FiscalError, FiscalYearSpec, Period};
use cl_core::report::DateRange;
use cl_core::{CalendarDate, FiscalYear};
use serde::{Deserialize, Serialize};

/// A date choice relative to today.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatePreset {
    /// Today (an as-of date) or today alone (a range).
    Today,
    /// The fiscal period containing today.
    ThisMonth,
    /// The fiscal period before it.
    LastMonth,
    /// The fiscal quarter containing today.
    ThisQuarter,
    /// The quarter before it.
    LastQuarter,
    /// The fiscal year containing today.
    ThisYear,
    /// The year before it.
    LastYear,
    /// The fiscal year's first day to today.
    YearToDate,
    /// The last day of last month (as of).
    EndOfLastMonth,
}

impl DatePreset {
    /// Every preset, in menu order.
    pub const ALL: [DatePreset; 9] = [
        DatePreset::Today,
        DatePreset::ThisMonth,
        DatePreset::LastMonth,
        DatePreset::ThisQuarter,
        DatePreset::LastQuarter,
        DatePreset::ThisYear,
        DatePreset::LastYear,
        DatePreset::YearToDate,
        DatePreset::EndOfLastMonth,
    ];

    /// Its i18n key.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            DatePreset::Today => "reports.preset.today",
            DatePreset::ThisMonth => "reports.preset.this_month",
            DatePreset::LastMonth => "reports.preset.last_month",
            DatePreset::ThisQuarter => "reports.preset.this_quarter",
            DatePreset::LastQuarter => "reports.preset.last_quarter",
            DatePreset::ThisYear => "reports.preset.this_year",
            DatePreset::LastYear => "reports.preset.last_year",
            DatePreset::YearToDate => "reports.preset.year_to_date",
            DatePreset::EndOfLastMonth => "reports.preset.end_of_last_month",
        }
    }

    /// The dates on `fiscal` when today is `today`. An as-of report takes
    /// the range's last day.
    ///
    /// # Errors
    /// A fiscal calendar that can't be computed near the date limits.
    pub fn range(
        self,
        fiscal: &FiscalYearSpec,
        today: CalendarDate,
    ) -> Result<DateRange, FiscalError> {
        let r = |p: Period| DateRange {
            from: p.start,
            to: p.end,
        };
        let y = |fy: FiscalYear| DateRange {
            from: fy.start,
            to: fy.end,
        };
        Ok(match self {
            DatePreset::Today => DateRange {
                from: today,
                to: today,
            },
            DatePreset::ThisMonth => r(fiscal.period_containing(today)?),
            DatePreset::LastMonth => {
                let this = fiscal.period_containing(today)?;
                r(fiscal.period_containing(this.start.add_days(-1)?)?)
            }
            DatePreset::EndOfLastMonth => {
                let this = fiscal.period_containing(today)?;
                let last = this.start.add_days(-1)?;
                DateRange {
                    from: last,
                    to: last,
                }
            }
            DatePreset::ThisQuarter => r(quarter_containing(*fiscal, today)?),
            DatePreset::LastQuarter => {
                let this = quarter_containing(*fiscal, today)?;
                r(quarter_containing(*fiscal, this.start.add_days(-1)?)?)
            }
            DatePreset::ThisYear => y(fiscal.year_containing(today)?),
            DatePreset::LastYear => {
                let this = fiscal.year_containing(today)?;
                y(fiscal.year_containing(this.start.add_days(-1)?)?)
            }
            DatePreset::YearToDate => DateRange {
                from: fiscal.year_containing(today)?.start,
                to: today,
            },
        })
    }
}

/// The fiscal quarter that contains `date`.
fn quarter_containing(fiscal: FiscalYearSpec, date: CalendarDate) -> Result<Period, FiscalError> {
    let fy = fiscal.year_containing(date)?;
    fiscal
        .quarters(&fy)?
        .into_iter()
        .find(|q| q.contains(date))
        .ok_or(FiscalError::OutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_core::Weekday;
    use cl_core::fiscal::{RetailPattern, YearEndRule};

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn range(
        p: DatePreset,
        f: FiscalYearSpec,
        today: CalendarDate,
    ) -> (CalendarDate, CalendarDate) {
        let r = p.range(&f, today).expect("range");
        (r.from, r.to)
    }

    #[test]
    fn calendar_year_presets() {
        let f = FiscalYearSpec::CALENDAR_YEAR;
        let today = d(2026, 11, 4);
        assert_eq!(range(DatePreset::Today, f, today), (today, today));
        assert_eq!(
            range(DatePreset::ThisMonth, f, today),
            (d(2026, 11, 1), d(2026, 11, 30))
        );
        assert_eq!(
            range(DatePreset::LastMonth, f, today),
            (d(2026, 10, 1), d(2026, 10, 31))
        );
        assert_eq!(
            range(DatePreset::ThisQuarter, f, today),
            (d(2026, 10, 1), d(2026, 12, 31))
        );
        assert_eq!(
            range(DatePreset::LastQuarter, f, today),
            (d(2026, 7, 1), d(2026, 9, 30))
        );
        assert_eq!(
            range(DatePreset::ThisYear, f, today),
            (d(2026, 1, 1), d(2026, 12, 31))
        );
        assert_eq!(
            range(DatePreset::LastYear, f, today),
            (d(2025, 1, 1), d(2025, 12, 31))
        );
        assert_eq!(
            range(DatePreset::YearToDate, f, today),
            (d(2026, 1, 1), today)
        );
        assert_eq!(
            range(DatePreset::EndOfLastMonth, f, today),
            (d(2026, 10, 31), d(2026, 10, 31))
        );
        // January: last month and last quarter cross the year.
        let jan = d(2027, 1, 15);
        assert_eq!(
            range(DatePreset::LastMonth, f, jan),
            (d(2026, 12, 1), d(2026, 12, 31))
        );
        assert_eq!(
            range(DatePreset::LastQuarter, f, jan),
            (d(2026, 10, 1), d(2026, 12, 31))
        );
    }

    #[test]
    fn a_july_fiscal_year() {
        let f = FiscalYearSpec::Calendar { start_month: 7 };
        let today = d(2026, 11, 4);
        assert_eq!(
            range(DatePreset::ThisYear, f, today),
            (d(2026, 7, 1), d(2027, 6, 30))
        );
        assert_eq!(
            range(DatePreset::ThisQuarter, f, today),
            (d(2026, 10, 1), d(2026, 12, 31))
        );
        assert_eq!(
            range(DatePreset::YearToDate, f, today),
            (d(2026, 7, 1), today)
        );
    }

    #[test]
    fn four_four_five_periods() {
        // FY 2026 ends on the last Saturday of December 2026 (Dec 26).
        let f = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 12,
            end_rule: YearEndRule::LastWeekday(Weekday::Sat),
        };
        let fy = f.year(2026).expect("fy");
        let periods = f.periods(&fy).expect("periods");
        let today = periods[1].start.add_days(3).expect("day"); // in period 2
        let this = DatePreset::ThisMonth.range(&f, today).expect("this");
        assert_eq!((this.from, this.to), (periods[1].start, periods[1].end));
        assert_eq!(this.from.days_until(this.to) + 1, 28, "a 4-week period");
        let last = DatePreset::LastMonth.range(&f, today).expect("last");
        assert_eq!((last.from, last.to), (periods[0].start, periods[0].end));
        let q = DatePreset::ThisQuarter.range(&f, today).expect("q");
        assert_eq!((q.from, q.to), (periods[0].start, periods[2].end));
        assert_eq!(q.from.days_until(q.to) + 1, 91, "4 + 4 + 5 weeks");
        let year = DatePreset::ThisYear.range(&f, today).expect("year");
        assert_eq!((year.from, year.to), (fy.start, fy.end));
    }
}
