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
    };
    req.tag_split = tag_split;
    req.fiscal_year_start = fiscal_year_start;
    req.budget_id.clone_from(&opts.budget_id);
    Ok(req)
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
}
