//! Options to a concrete request (plan §0 step 2): presets become dates on
//! the company's fiscal calendar, period columns become explicit ranges
//! (4-4-5 aware, never a count), and the options the report doesn't offer
//! are dropped.

use cl_core::fiscal::{FiscalError, FiscalYearSpec};
use cl_core::report::{Compare, DateRange, ReportDates, ReportFilters, ReportRequest};
use cl_core::{CalendarDate, Money};

use super::options::{ColumnBy, DateChoice, ReportOptions};
use super::spec::{BasisMode, DateMode, ReportSpec};

/// At most this many period columns (U-201).
pub const MAX_COLUMNS: usize = 36;

/// Why options can't run.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// The fiscal calendar can't be computed for these dates.
    #[error("the dates are outside the fiscal calendar")]
    Fiscal(#[from] FiscalError),
    /// From is after To.
    #[error("the start date is after the end date")]
    Backwards,
    /// More than [`MAX_COLUMNS`] period columns.
    #[error("too many columns")]
    TooManyColumns,
    /// Columns by tag group with no group chosen.
    #[error("choose a tag group")]
    NoTagGroup,
    /// Columns by project (U-35, not built).
    #[error("project columns aren't available yet")]
    ProjectsUnavailable,
    /// Tag split with columns by tag or client (Q-P28h).
    #[error("tag split can't be used with columns by tag or client")]
    TagSplitWithCompare,
    /// Tag split with a tag filter (the engine takes one or the other).
    #[error("tag split can't be used with a tag filter")]
    TagSplitWithTagFilter,
    /// An amount filter that isn't an amount.
    #[error("the amount filter isn't an amount")]
    BadAmount,
}

/// The request for `spec` with `opts`, on `fiscal`, when today is `today`.
///
/// # Errors
/// [`ResolveError`].
#[allow(clippy::too_many_lines, reason = "one pass over the options, in order")]
pub fn resolve(
    spec: &ReportSpec,
    opts: &ReportOptions,
    fiscal: &FiscalYearSpec,
    today: CalendarDate,
) -> Result<ReportRequest, ResolveError> {
    let span = match opts.dates {
        DateChoice::Preset(p) => p.range(fiscal, today)?,
        DateChoice::Fixed { from, to } => DateRange { from, to },
        DateChoice::AsOf(d) => DateRange { from: d, to: d },
    };
    if span.from > span.to {
        return Err(ResolveError::Backwards);
    }
    let columns = if spec.columns.contains(&opts.columns) {
        opts.columns
    } else {
        spec.default_columns()
    };
    let periods = matches!(
        columns,
        ColumnBy::Month | ColumnBy::FiscalPeriod | ColumnBy::Quarter | ColumnBy::Year
    );
    let dates = match spec.dates {
        DateMode::None => ReportDates::None,
        DateMode::Year => {
            let y = span.to.year();
            ReportDates::Range(DateRange {
                from: CalendarDate::from_ymd(y, 1, 1).map_err(|_| FiscalError::OutOfRange)?,
                to: CalendarDate::from_ymd(y, 12, 31).map_err(|_| FiscalError::OutOfRange)?,
            })
        }
        DateMode::AsOf if periods => {
            // Balances at the end of each period from the fiscal year's
            // start to the as-of date.
            let start = fiscal.year_containing(span.to)?.start;
            ReportDates::Ranges(split(
                columns,
                DateRange {
                    from: start,
                    to: span.to,
                },
                *fiscal,
            )?)
        }
        DateMode::AsOf => ReportDates::AsOf(span.to),
        DateMode::Range if periods => ReportDates::Ranges(split(columns, span, *fiscal)?),
        DateMode::Range => ReportDates::Range(span),
    };
    // Budget vs. Actual: whole fiscal periods of one fiscal year, each a
    // budget period (custom dates snap out, Q-P28n).
    let (dates, first_period) = if spec.source == cl_core::report::Source::BudgetVsActual {
        let (ranges, first) = budget_periods(span, *fiscal)?;
        (ReportDates::Ranges(ranges), Some(first))
    } else {
        (dates, None)
    };
    let compare = match columns {
        ColumnBy::Client => Compare::ByContact,
        ColumnBy::TagGroup => Compare::ByTag {
            tag_group_id: opts.column_tag_group_id.ok_or(ResolveError::NoTagGroup)?,
        },
        ColumnBy::Project => return Err(ResolveError::ProjectsUnavailable),
        _ => Compare::None,
    };
    let tag_split = if spec.tag_split {
        opts.tag_split.as_ref().map(|t| t.tag_group_id)
    } else {
        None
    };
    if tag_split.is_some() && compare != Compare::None {
        return Err(ResolveError::TagSplitWithCompare);
    }
    if tag_split.is_some()
        && (!opts.filters.tag_ids.is_empty() || opts.filters.untagged_in_group.is_some())
    {
        return Err(ResolveError::TagSplitWithTagFilter);
    }
    let extras: std::collections::BTreeSet<_> = opts
        .extras
        .iter()
        .copied()
        .filter(|e| spec.extras.contains(e))
        .collect();
    // YTD columns start the fiscal year; so does a balance report's net
    // income drill and the Trial Balance's income and expense lines.
    let fiscal_year_start = if extras.contains(&cl_core::report::Extra::Ytd)
        || matches!(
            spec.source,
            cl_core::report::Source::BalanceSheet | cl_core::report::Source::TrialBalance
        ) {
        Some(fiscal.year_containing(span.to)?.start)
    } else {
        None
    };
    let f = &opts.filters;
    let amount = |s: &Option<String>| -> Result<_, ResolveError> {
        s.as_deref()
            .map(|s| {
                Money::parse_wire(s.trim())
                    .map(|m| m.abs().as_decimal())
                    .map_err(|_| ResolveError::BadAmount)
            })
            .transpose()
    };
    let (amount_min, amount_max) = match &f.amount {
        Some(a) => (amount(&a.min)?, amount(&a.max)?),
        None => (None, None),
    };
    let mut req = ReportRequest::new(spec.source, dates);
    req.basis = match spec.basis {
        BasisMode::Toggle => Some(opts.basis.unwrap_or(cl_core::report::Basis::Accrual)),
        BasisMode::NotApplicable => None,
    };
    req.columns = compare;
    req.extras = extras;
    req.filters = ReportFilters {
        account_ids: f.account_ids.clone(),
        contact_ids: f.contact_ids.clone(),
        tag_ids: f.tag_ids.clone(),
        item_ids: f.item_ids.clone(),
        amount_min,
        amount_max,
        cleared: f.cleared,
        txn_types: f.txn_types.clone(),
        adjusting: f.adjusting,
        untagged_in_group: f.untagged_in_group,
        bucket: f
            .bucket
            .filter(|b| *b != cl_core::report::AgingBucket::Total),
        no_contact: f.no_contact,
    };
    req.tag_split = tag_split;
    req.fiscal_year_start = fiscal_year_start;
    req.budget_id.clone_from(&opts.budget_id);
    req.first_period = first_period;
    // "Prior period" is the previous month, fiscal period, quarter or year
    // (B3), not the engine's default of the same number of days before.
    if req.extras.contains(&cl_core::report::Extra::PriorPeriod)
        && !req.extras.contains(&cl_core::report::Extra::PriorYear)
    {
        let balance = spec.dates == DateMode::AsOf;
        req.prior_ranges = prior_ranges(&req.dates, columns, span, *fiscal, balance);
    }
    Ok(req)
}

/// A calendar unit a span or column can be a whole (or a part) of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    /// A calendar month (columns by month).
    Month,
    /// A fiscal period (a "month" on the fiscal calendar, 4-4-5 aware).
    FiscalPeriod,
    /// A fiscal quarter.
    Quarter,
    /// A fiscal year.
    Year,
}

impl Unit {
    /// The unit's period containing `at`.
    fn around(self, at: CalendarDate, fiscal: FiscalYearSpec) -> Result<DateRange, FiscalError> {
        Ok(match self {
            Unit::Month => DateRange {
                from: at.start_of_month(),
                to: at.end_of_month(),
            },
            Unit::FiscalPeriod => {
                let p = fiscal.period_containing(at)?;
                DateRange {
                    from: p.start,
                    to: p.end,
                }
            }
            Unit::Quarter => {
                let fy = fiscal.year_containing(at)?;
                let q = fiscal
                    .quarters(&fy)?
                    .into_iter()
                    .find(|q| q.contains(at))
                    .ok_or(FiscalError::OutOfRange)?;
                DateRange {
                    from: q.start,
                    to: q.end,
                }
            }
            Unit::Year => {
                let fy = fiscal.year_containing(at)?;
                DateRange {
                    from: fy.start,
                    to: fy.end,
                }
            }
        })
    }

    /// The unit's period before the one containing `at`.
    fn before(self, at: CalendarDate, fiscal: FiscalYearSpec) -> Result<DateRange, FiscalError> {
        let this = self.around(at, fiscal)?;
        let prev_day = this
            .from
            .add_days(-1)
            .map_err(|_| FiscalError::OutOfRange)?;
        self.around(prev_day, fiscal)
    }

    /// The unit of a column cut by `by`.
    const fn of_columns(by: ColumnBy) -> Option<Unit> {
        match by {
            ColumnBy::Month => Some(Unit::Month),
            ColumnBy::FiscalPeriod => Some(Unit::FiscalPeriod),
            ColumnBy::Quarter => Some(Unit::Quarter),
            ColumnBy::Year => Some(Unit::Year),
            _ => None,
        }
    }

    /// The smallest unit `span` starts and stays inside of (a whole month,
    /// "month to date", a whole quarter, year to date, ...), or `None` for
    /// custom dates.
    fn of_span(span: DateRange, fiscal: FiscalYearSpec) -> Option<Unit> {
        [Unit::FiscalPeriod, Unit::Month, Unit::Quarter, Unit::Year]
            .into_iter()
            .find(|u| {
                u.around(span.from, fiscal)
                    .is_ok_and(|p| p.from == span.from && span.to <= p.to)
            })
    }
}

/// The same days `r` covers in its unit's period, one period back:
/// a whole period gives the whole previous period; a part (the first
/// column of custom dates, "month to date") gives the same part, cut at
/// the previous period's end.
fn shifted(unit: Unit, r: DateRange, fiscal: FiscalYearSpec) -> Result<DateRange, FiscalError> {
    let this = unit.around(r.from, fiscal)?;
    if r.to > this.to {
        return Err(FiscalError::OutOfRange);
    }
    let prev = unit.before(r.from, fiscal)?;
    let at = |offset: i64| -> Result<CalendarDate, FiscalError> {
        Ok(prev
            .from
            .add_days(offset)
            .map_err(|_| FiscalError::OutOfRange)?
            .min(prev.to))
    };
    let from = at(this.from.days_until(r.from))?;
    let to = if r.to == this.to {
        prev.to
    } else {
        at(this.from.days_until(r.to))?
    };
    Ok(DateRange { from, to })
}

/// The same number of days just before `r` (custom dates: the engine's
/// own rule, sent explicitly).
fn days_before(r: DateRange) -> Option<DateRange> {
    let len = r.from.days_until(r.to) + 1;
    Some(DateRange {
        from: r.from.add_days(-len).ok()?,
        to: r.from.add_days(-1).ok()?,
    })
}

/// "Prior period" ranges for `dates` (Q-PR6a): each period column
/// compares with its unit's previous period (the same part of it when the
/// column is cut by the dates); a single range with the previous instance
/// of the month, fiscal period, quarter or year it is (or is the start
/// of), else the same number of days before. A balance sheet compares each
/// date with the end of the previous whole period. `None` when a date
/// falls outside the calendar: the engine's own rule then applies.
fn prior_ranges(
    dates: &ReportDates,
    columns: ColumnBy,
    span: DateRange,
    fiscal: FiscalYearSpec,
    balance: bool,
) -> Option<Vec<DateRange>> {
    match dates {
        ReportDates::Ranges(v) => {
            let unit = Unit::of_columns(columns)?;
            v.iter()
                .map(|r| {
                    if balance {
                        unit.before(r.from, fiscal).ok()
                    } else {
                        shifted(unit, *r, fiscal).ok()
                    }
                })
                .collect()
        }
        ReportDates::Range(r) => Some(vec![match Unit::of_span(*r, fiscal) {
            Some(u) => shifted(u, *r, fiscal).ok()?,
            None => days_before(*r)?,
        }]),
        ReportDates::AsOf(at) => {
            // The dates' unit when they were a whole period (a preset),
            // else the fiscal period the date falls in.
            let unit = Unit::of_span(span, fiscal)
                .filter(|_| span.from != span.to)
                .unwrap_or(Unit::FiscalPeriod);
            Some(vec![unit.before(*at, fiscal).ok()?])
        }
        ReportDates::None => None,
    }
}

/// The fiscal periods span touches, within the fiscal year of its first
/// day, and the number (1-12) of the first: Budget vs. Actual's columns.
fn budget_periods(
    span: DateRange,
    fiscal: FiscalYearSpec,
) -> Result<(Vec<DateRange>, u8), ResolveError> {
    let fy = fiscal.year_containing(span.from)?;
    let to = span.to.min(fy.end);
    let chosen: Vec<_> = fiscal
        .periods(&fy)?
        .into_iter()
        .filter(|p| p.end >= span.from && p.start <= to)
        .collect();
    let first = chosen.first().map_or(1, |p| p.number);
    Ok((
        chosen
            .into_iter()
            .map(|p| DateRange {
                from: p.start,
                to: p.end,
            })
            .collect(),
        first,
    ))
}

/// `span` cut into `by` columns, each clipped to `span`.
fn split(
    by: ColumnBy,
    span: DateRange,
    fiscal: FiscalYearSpec,
) -> Result<Vec<DateRange>, ResolveError> {
    let mut out = Vec::new();
    let mut at = span.from;
    while at <= span.to {
        let (start, end) = match by {
            ColumnBy::Month => (at.start_of_month(), at.end_of_month()),
            ColumnBy::FiscalPeriod => {
                let p = fiscal.period_containing(at)?;
                (p.start, p.end)
            }
            ColumnBy::Quarter => {
                let fy = fiscal.year_containing(at)?;
                let q = fiscal
                    .quarters(&fy)?
                    .into_iter()
                    .find(|q| q.contains(at))
                    .ok_or(FiscalError::OutOfRange)?;
                (q.start, q.end)
            }
            _ => {
                let fy = fiscal.year_containing(at)?;
                (fy.start, fy.end)
            }
        };
        out.push(DateRange {
            from: start.max(span.from),
            to: end.min(span.to),
        });
        if out.len() > MAX_COLUMNS {
            return Err(ResolveError::TooManyColumns);
        }
        match end.add_days(1) {
            Ok(next) => at = next,
            Err(_) => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reports::options::{AmountFilter, TagSplit};
    use crate::reports::presets::DatePreset;
    use crate::reports::{ReportOptions, find};
    use cl_core::Weekday;
    use cl_core::fiscal::{RetailPattern, YearEndRule};
    use cl_core::report::{Basis, Extra, Source};

    fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    const CAL: FiscalYearSpec = FiscalYearSpec::CALENDAR_YEAR;
    const G: &str = "33333333-3333-4333-8333-333333333333";

    #[test]
    fn the_mockup_pl_resolves_to_october() {
        let spec = find("profit_loss").expect("p&l");
        let mut o = ReportOptions::for_spec(spec);
        o.extras.insert(Extra::PctOfIncome);
        o.extras.insert(Extra::Ytd);
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("resolve");
        assert_eq!(r.source, Source::ProfitLoss);
        assert_eq!(
            r.dates,
            ReportDates::Range(DateRange {
                from: d(2026, 10, 1),
                to: d(2026, 10, 31)
            })
        );
        assert_eq!(r.basis, Some(Basis::Accrual));
        assert_eq!(r.fiscal_year_start, Some(d(2026, 1, 1)));
        assert_eq!(r.extras.len(), 2);
    }

    #[test]
    fn month_columns_are_explicit_ranges() {
        let spec = find("profit_loss_by_month").expect("by month");
        let mut o = ReportOptions::for_spec(spec);
        o.dates = DateChoice::Fixed {
            from: d(2026, 1, 15),
            to: d(2026, 3, 10),
        };
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("resolve");
        let ReportDates::Ranges(v) = r.dates else {
            panic!("ranges")
        };
        assert_eq!(
            v,
            vec![
                DateRange {
                    from: d(2026, 1, 15),
                    to: d(2026, 1, 31)
                },
                DateRange {
                    from: d(2026, 2, 1),
                    to: d(2026, 2, 28)
                },
                DateRange {
                    from: d(2026, 3, 1),
                    to: d(2026, 3, 10)
                },
            ]
        );
    }

    #[test]
    fn four_four_five_fiscal_period_columns() {
        let f = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 12,
            end_rule: YearEndRule::LastWeekday(Weekday::Sat),
        };
        let spec = find("profit_loss_by_fiscal_period").expect("spec");
        let o = ReportOptions::for_spec(spec); // this fiscal year
        let fy = f.year(2026).expect("fy");
        let r = resolve(spec, &o, &f, fy.start.add_days(40).expect("d")).expect("resolve");
        let ReportDates::Ranges(v) = r.dates else {
            panic!("ranges")
        };
        let periods = f.periods(&fy).expect("periods");
        assert_eq!(v.len(), 12);
        for (col, p) in v.iter().zip(periods) {
            assert_eq!((col.from, col.to), (p.start, p.end));
        }
        assert_eq!(v[2].from.days_until(v[2].to) + 1, 35, "the 5-week period");
    }

    #[test]
    fn more_than_36_columns_are_refused() {
        let spec = find("profit_loss_by_month").expect("by month");
        let mut o = ReportOptions::for_spec(spec);
        o.dates = DateChoice::Fixed {
            from: d(2020, 1, 1),
            to: d(2026, 12, 31),
        };
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::TooManyColumns)
        );
    }

    #[test]
    fn balances_are_as_of_and_open_items_have_no_basis() {
        let bs = find("balance_sheet").expect("bs");
        let r = resolve(bs, &ReportOptions::for_spec(bs), &CAL, d(2026, 11, 4)).expect("bs");
        assert_eq!(r.dates, ReportDates::AsOf(d(2026, 11, 4)));
        let ar = find("ar_aging_summary").expect("ar");
        let mut o = ReportOptions::for_spec(ar);
        o.basis = Some(Basis::Cash);
        let r = resolve(ar, &o, &CAL, d(2026, 11, 4)).expect("ar");
        assert_eq!(r.basis, None);
        assert_eq!(r.dates, ReportDates::AsOf(d(2026, 11, 4)));
        // A balance sheet by month: one as-of per month end so far.
        let mut o = ReportOptions::for_spec(bs);
        o.columns = ColumnBy::Month;
        let r = resolve(bs, &o, &CAL, d(2026, 3, 15)).expect("bs");
        let ReportDates::Ranges(v) = r.dates else {
            panic!("ranges")
        };
        assert_eq!(
            v.iter().map(|r| r.to).collect::<Vec<_>>(),
            vec![d(2026, 1, 31), d(2026, 2, 28), d(2026, 3, 15)]
        );
    }

    #[test]
    fn year_reports_take_the_calendar_year() {
        let spec = find("vendor_1099_summary").expect("1099");
        let r = resolve(spec, &ReportOptions::for_spec(spec), &CAL, d(2027, 1, 20)).expect("r");
        assert_eq!(
            r.dates,
            ReportDates::Range(DateRange {
                from: d(2026, 1, 1),
                to: d(2026, 12, 31)
            })
        );
    }

    #[test]
    fn columns_by_tag_and_tag_split_rules() {
        let spec = find("profit_loss").expect("p&l");
        let mut o = ReportOptions::for_spec(spec);
        o.columns = ColumnBy::TagGroup;
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::NoTagGroup)
        );
        o.column_tag_group_id = Some(G.parse().expect("id"));
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("by tag");
        assert!(matches!(r.columns, Compare::ByTag { .. }));
        o.tag_split = Some(TagSplit {
            tag_group_id: G.parse().expect("id"),
            show_account_total: false,
            label: crate::reports::TagLabel::Append,
            separator: "-".into(),
        });
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::TagSplitWithCompare)
        );
        o.columns = ColumnBy::Total;
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("split");
        assert!(r.tag_split.is_some());
        o.columns = ColumnBy::Project;
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::ProjectsUnavailable)
        );
    }

    #[test]
    fn amount_filters_are_exact_and_absolute() {
        let spec = find("general_ledger").expect("gl");
        let mut o = ReportOptions::for_spec(spec);
        o.filters.amount = Some(AmountFilter {
            min: Some("-100.10".into()),
            max: Some("2500".into()),
        });
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("gl");
        assert_eq!(
            r.filters.amount_min.map(|d| d.to_string()),
            Some("100.1".into())
        );
        assert_eq!(
            r.filters.amount_max.map(|d| d.to_string()),
            Some("2500".into())
        );
        o.filters.amount = Some(AmountFilter {
            min: Some("1e3".into()),
            max: None,
        });
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::BadAmount)
        );
    }

    #[test]
    fn backwards_dates_are_refused() {
        let spec = find("profit_loss").expect("p&l");
        let mut o = ReportOptions::for_spec(spec);
        o.dates = DateChoice::Fixed {
            from: d(2026, 2, 1),
            to: d(2026, 1, 1),
        };
        assert_eq!(
            resolve(spec, &o, &CAL, d(2026, 11, 4)),
            Err(ResolveError::Backwards)
        );
        let _ = DatePreset::ALL;
    }

    #[test]
    fn budget_vs_actual_runs_whole_fiscal_periods_of_one_year() {
        let spec = find("budget_vs_actual").expect("bva");
        let mut o = ReportOptions::for_spec(spec);
        assert_eq!(
            spec.missing_pick_in(&o),
            Some(crate::reports::FilterKind::Budget),
            "a budget first"
        );
        o.budget_id = Some("b1".into());
        assert_eq!(spec.missing_pick_in(&o), None);
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("this year");
        let ReportDates::Ranges(v) = &r.dates else {
            panic!("{:?}", r.dates)
        };
        assert_eq!(v.len(), 12);
        assert_eq!(r.first_period, Some(1));
        assert_eq!(r.budget_id.as_deref(), Some("b1"));
        // Custom dates snap out to whole periods and stop at the year's end.
        o.dates = DateChoice::Fixed {
            from: d(2026, 2, 10),
            to: d(2027, 1, 5),
        };
        let r = resolve(spec, &o, &CAL, d(2026, 11, 4)).expect("custom");
        let ReportDates::Ranges(v) = &r.dates else {
            panic!("{:?}", r.dates)
        };
        assert_eq!(r.first_period, Some(2));
        assert_eq!(v.first().map(|x| x.from), Some(d(2026, 2, 1)));
        assert_eq!(v.last().map(|x| x.to), Some(d(2026, 12, 31)));
        // 4-4-5: the periods are the retail calendar's.
        let f = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 12,
            end_rule: YearEndRule::LastWeekday(Weekday::Sat),
        };
        o.dates = DateChoice::Preset(DatePreset::ThisYear);
        let r = resolve(spec, &o, &f, d(2026, 11, 4)).expect("4-4-5");
        let ReportDates::Ranges(v) = &r.dates else {
            panic!("{:?}", r.dates)
        };
        assert_eq!(v.len(), 12);
        assert_eq!(v[0].from.days_until(v[0].to) + 1, 28, "four weeks");
    }

    fn r(from: CalendarDate, to: CalendarDate) -> DateRange {
        DateRange { from, to }
    }

    fn prior_of(
        key: &str,
        f: impl FnOnce(&mut ReportOptions),
        fiscal: &FiscalYearSpec,
    ) -> ReportRequest {
        let spec = find(key).expect("spec");
        let mut o = ReportOptions::for_spec(spec);
        o.extras.insert(Extra::PriorPeriod);
        f(&mut o);
        resolve(spec, &o, fiscal, d(2026, 12, 4)).expect("resolve")
    }

    /// B3/T4: "Prior period" is the previous period on the calendar, not
    /// the same number of days before.
    #[test]
    fn prior_period_is_the_previous_month_quarter_or_year() {
        // Last month (November) compares with all of October, Oct 1 too.
        let req = prior_of(
            "profit_loss",
            |o| o.dates = DateChoice::Preset(DatePreset::LastMonth),
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![r(d(2026, 10, 1), d(2026, 10, 31))])
        );
        // December (31 days) with November (30).
        let req = prior_of(
            "profit_loss",
            |o| o.dates = DateChoice::Preset(DatePreset::ThisMonth),
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![r(d(2026, 11, 1), d(2026, 11, 30))])
        );
        // A quarter with the quarter before; year to date with the same
        // days last year.
        let req = prior_of(
            "profit_loss",
            |o| {
                o.dates = DateChoice::Fixed {
                    from: d(2026, 10, 1),
                    to: d(2026, 12, 31),
                }
            },
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![r(d(2026, 7, 1), d(2026, 9, 30))])
        );
        let req = prior_of(
            "profit_loss",
            |o| {
                o.dates = DateChoice::Fixed {
                    from: d(2026, 1, 1),
                    to: d(2026, 11, 30),
                }
            },
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![r(d(2025, 1, 1), d(2025, 11, 30))])
        );
        // Custom dates keep the day count, sent explicitly.
        let req = prior_of(
            "profit_loss",
            |o| {
                o.dates = DateChoice::Fixed {
                    from: d(2026, 10, 2),
                    to: d(2026, 10, 31),
                }
            },
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![r(d(2026, 9, 2), d(2026, 10, 1))])
        );
        // The prior year wins: no prior ranges.
        let req = prior_of(
            "profit_loss",
            |o| {
                o.extras.insert(Extra::PriorYear);
            },
            &CAL,
        );
        assert_eq!(req.prior_ranges, None);
    }

    #[test]
    fn prior_period_columns_compare_with_their_own_previous_period() {
        let req = prior_of(
            "profit_loss_by_month",
            |o| {
                o.columns = ColumnBy::Month;
                o.dates = DateChoice::Fixed {
                    from: d(2026, 1, 15),
                    to: d(2026, 3, 31),
                };
            },
            &CAL,
        );
        assert_eq!(
            req.prior_ranges,
            Some(vec![
                r(d(2025, 12, 15), d(2025, 12, 31)),
                r(d(2026, 1, 1), d(2026, 1, 31)),
                r(d(2026, 2, 1), d(2026, 2, 28)),
            ])
        );
        // 4-4-5: each fiscal period with the one before (a 5-week period's
        // prior is the 4-week period before it, not 35 days).
        let f = FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: 12,
            end_rule: YearEndRule::LastWeekday(Weekday::Sat),
        };
        let req = prior_of(
            "profit_loss_by_fiscal_period",
            |o| o.columns = ColumnBy::FiscalPeriod,
            &f,
        );
        let ReportDates::Ranges(cols) = &req.dates else {
            panic!("ranges")
        };
        let prior = req.prior_ranges.expect("prior");
        assert_eq!(prior.len(), cols.len());
        for (i, p) in prior.iter().enumerate().skip(1) {
            assert_eq!(*p, cols[i - 1], "period {i}");
        }
        assert_eq!(prior[2].from.days_until(prior[2].to) + 1, 28);
    }

    #[test]
    fn a_balance_sheet_compares_with_the_previous_period_end() {
        // One date: the end of the fiscal period before it.
        let req = prior_of(
            "balance_sheet",
            |o| o.dates = DateChoice::AsOf(d(2026, 11, 15)),
            &CAL,
        );
        assert_eq!(req.dates, ReportDates::AsOf(d(2026, 11, 15)));
        assert_eq!(
            req.prior_ranges
                .as_ref()
                .and_then(|p| p.first())
                .map(|p| p.to),
            Some(d(2026, 10, 31))
        );
        // A quarter preset: the quarter before.
        let req = prior_of(
            "balance_sheet",
            |o| o.dates = DateChoice::Preset(DatePreset::LastQuarter),
            &CAL,
        );
        assert_eq!(
            req.prior_ranges
                .as_ref()
                .and_then(|p| p.first())
                .map(|p| p.to),
            Some(d(2026, 6, 30))
        );
        // By month: each month end with the month end before.
        let req = prior_of(
            "balance_sheet",
            |o| {
                o.columns = ColumnBy::Month;
                o.dates = DateChoice::AsOf(d(2026, 3, 15));
            },
            &CAL,
        );
        assert_eq!(
            req.prior_ranges
                .expect("prior")
                .iter()
                .map(|p| p.to)
                .collect::<Vec<_>>(),
            vec![d(2025, 12, 31), d(2026, 1, 31), d(2026, 2, 28)]
        );
    }
}
