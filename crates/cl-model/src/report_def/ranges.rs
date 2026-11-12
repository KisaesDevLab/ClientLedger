//! The period and compare ranges a Report Builder run sends (ADD-03 A08,
//! plan §3.5, Q-A07u(2)): `periodRanges` from the P28 period splitting and
//! one `compareRanges` array per compare entry, parallel to them, computed
//! here rather than by the engine's same-length shift (the period before
//! October is September 1-30).
//!
//! "The period before" is the standard reports' rule (Q-PR6a, reused from
//! [`crate::reports::resolve`] so a Customized report compares with the
//! same dates): each period column with its unit's previous period; one
//! range with the previous month, fiscal period, quarter or year it is (or
//! the same part of it: month to date compares with the same days of the
//! month before); custom dates with the same number of days just before;
//! as of a date with the end of the previous whole period. "The same
//! period last year" maps each date to the same fiscal period one fiscal
//! year back, by period number (4-4-5 and 52/53-week aware), keeping a
//! period's end on the period's end (February 29 to February 28).
//!
//! No egui, no I/O, no amounts.

use cl_core::CalendarDate;
use cl_core::fiscal::{FiscalError, FiscalYearSpec};
use cl_core::report::{DateRange, ReportDates};

use super::schema::{BuilderDefinition, CompareBase};
use crate::reports::resolve::{ResolveError, prior_ranges, resolve};
use crate::reports::{ColumnBy, DateChoice, DateMode, ReportOptions, ReportSpec};

/// The engine's cap on `fiscalYearStarts` (`run.ts`, ≤ 60).
pub const MAX_FISCAL_YEAR_STARTS: usize = 60;

/// A run's dates: a range, or one as-of day.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunSpan {
    /// From and to.
    Range(DateRange),
    /// As of the day.
    AsOf(CalendarDate),
}

/// Everything date-shaped a run sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRanges {
    /// The run's dates.
    pub dates: RunSpan,
    /// Period columns (empty for one column: the engine then uses `dates`).
    pub period_ranges: Vec<DateRange>,
    /// One array per compare set, each as long as the base columns (one
    /// when `period_ranges` is empty).
    pub compare_ranges: Vec<Vec<DateRange>>,
    /// The starts of every fiscal year the run touches, from the year
    /// before the earliest date (prior years' earnings) through the last.
    pub fiscal_year_starts: Vec<CalendarDate>,
}

/// The ranges for `def` run under `spec` with `opts` (the window's dates,
/// basis and period columns), on `fiscal`, when today is `today`.
///
/// # Errors
/// [`ResolveError`] when the dates can't be resolved or fall outside the
/// fiscal calendar (a compare range included).
pub fn run_ranges(
    def: &BuilderDefinition,
    spec: &ReportSpec,
    opts: &ReportOptions,
    fiscal: &FiscalYearSpec,
    today: CalendarDate,
) -> Result<RunRanges, ResolveError> {
    let req = resolve(spec, opts, fiscal, today)?;
    let as_of = spec.dates == DateMode::AsOf;
    let (dates, period_ranges) = match &req.dates {
        ReportDates::Range(r) => (RunSpan::Range(*r), Vec::new()),
        ReportDates::AsOf(d) => (RunSpan::AsOf(*d), Vec::new()),
        ReportDates::Ranges(v) => {
            let (Some(first), Some(last)) = (v.first(), v.last()) else {
                return Err(FiscalError::OutOfRange.into());
            };
            let dates = if as_of {
                RunSpan::AsOf(last.to)
            } else {
                RunSpan::Range(DateRange {
                    from: first.from,
                    to: last.to,
                })
            };
            (dates, if v.len() > 1 { v.clone() } else { Vec::new() })
        }
        ReportDates::None => return Err(FiscalError::OutOfRange.into()),
    };
    // The engine's base columns: the periods, else the dates.
    let base: Vec<DateRange> = if period_ranges.is_empty() {
        vec![match dates {
            RunSpan::Range(r) => r,
            RunSpan::AsOf(d) => DateRange { from: d, to: d },
        }]
    } else {
        period_ranges.clone()
    };
    // Aging columns age everything as of the report date: the engine
    // refuses compare ranges with them (`run.ts`, BUCKETS_PERIODS).
    let compare_ranges = if def.columns.aging_buckets == Some(true) {
        Vec::new()
    } else {
        let columns = if spec.columns.contains(&opts.columns) {
            opts.columns
        } else {
            spec.default_columns()
        };
        let span = match opts.dates {
            DateChoice::Preset(p) => p.range(fiscal, today)?,
            DateChoice::Fixed { from, to } => DateRange { from, to },
            DateChoice::AsOf(d) => DateRange { from: d, to: d },
        };
        // Calendar-month columns go back twelve calendar months (on a
        // 52/53-week year their months aren't fiscal periods).
        let months = columns == ColumnBy::Month && !period_ranges.is_empty();
        // A prior period the desktop can't place (a date outside the
        // fiscal calendar): no compare ranges are sent at all, and the
        // engine's own rule applies to every compare (R9; the arrays are
        // one per compare entry, so one can't be left out alone).
        let prior = def
            .columns
            .compare
            .iter()
            .any(|c| c.base == CompareBase::PriorPeriod)
            .then(|| prior_ranges(&req.dates, columns, span, *fiscal, as_of));
        if matches!(prior, Some(None)) {
            Vec::new()
        } else {
            def.columns
                .compare
                .iter()
                .map(|c| match c.base {
                    CompareBase::PriorPeriod => Ok(prior.clone().flatten().unwrap_or_default()),
                    CompareBase::PriorYear => base
                        .iter()
                        .map(|r| {
                            Ok(DateRange {
                                from: year_back(r.from, as_of, months, *fiscal)?,
                                to: year_back(r.to, true, months, *fiscal)?,
                            })
                        })
                        .collect::<Result<Vec<_>, FiscalError>>()
                        .map_err(ResolveError::from),
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    if compare_ranges.iter().any(|c| c.len() != base.len()) {
        return Err(FiscalError::OutOfRange.into());
    }
    let fiscal_year_starts = year_starts(dates, &base, &compare_ranges, *fiscal)?;
    Ok(RunRanges {
        dates,
        period_ranges,
        compare_ranges,
        fiscal_year_starts,
    })
}

/// `d` in the same fiscal period one fiscal year back (the same calendar
/// month a year back when `months`): the same number of days into the
/// period (cut at its end), or the period's end when `d` ends its period
/// and `end` is set.
fn year_back(
    d: CalendarDate,
    end: bool,
    months: bool,
    fiscal: FiscalYearSpec,
) -> Result<CalendarDate, FiscalError> {
    if months {
        let back = d.add_months(-12).map_err(|_| FiscalError::OutOfRange)?;
        return Ok(if end && d == d.end_of_month() {
            back.end_of_month()
        } else {
            back
        });
    }
    let p = fiscal.period_containing(d)?;
    let fy = fiscal.year_containing(d)?;
    let before = fy.start.add_days(-1).map_err(|_| FiscalError::OutOfRange)?;
    let prev = fiscal.year_containing(before)?;
    let periods = fiscal.periods(&prev)?;
    let q = periods
        .get(usize::from(p.number).saturating_sub(1))
        .ok_or(FiscalError::OutOfRange)?;
    if end && d == p.end {
        return Ok(q.end);
    }
    let at = q
        .start
        .add_days(p.start.days_until(d))
        .map_err(|_| FiscalError::OutOfRange)?;
    Ok(at.min(q.end))
}

/// The starts of the fiscal years from the one before the earliest date
/// the run names through the one holding the latest (the engine refuses a
/// date with no start on or before it).
fn year_starts(
    dates: RunSpan,
    base: &[DateRange],
    compare: &[Vec<DateRange>],
    fiscal: FiscalYearSpec,
) -> Result<Vec<CalendarDate>, FiscalError> {
    let mut days: Vec<CalendarDate> = match dates {
        RunSpan::Range(r) => vec![r.from, r.to],
        RunSpan::AsOf(d) => vec![d],
    };
    for r in base.iter().chain(compare.iter().flatten()) {
        days.push(r.from);
        days.push(r.to);
    }
    let (Some(first), Some(last)) = (days.iter().min(), days.iter().max()) else {
        return Err(FiscalError::OutOfRange);
    };
    let first = fiscal.year_containing(*first)?;
    let mut year = first
        .start
        .add_days(-1)
        .ok()
        .and_then(|d| fiscal.year_containing(d).ok())
        .unwrap_or(first);
    let mut starts = Vec::new();
    loop {
        starts.push(year.start);
        if year.end >= *last || starts.len() >= MAX_FISCAL_YEAR_STARTS {
            break;
        }
        match year
            .end
            .add_days(1)
            .ok()
            .and_then(|d| fiscal.year_containing(d).ok())
        {
            Some(next) => year = next,
            None => break,
        }
    }
    Ok(starts)
}

#[cfg(test)]
#[path = "ranges_tests.rs"]
mod tests;
