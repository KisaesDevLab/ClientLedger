use super::*;
use crate::report_def::{CompareSet, parse};
use crate::reports::{
    BUILDER_AS_OF, BUILDER_AS_OF_TOTAL, BUILDER_RANGE, ColumnBy, DatePreset, ReportOptions,
};
use cl_core::Weekday;
use cl_core::fiscal::{RetailPattern, YearEndRule};
use proptest::prelude::*;

const CAL: FiscalYearSpec = FiscalYearSpec::Calendar { start_month: 1 };
const RETAIL: FiscalYearSpec = FiscalYearSpec::Retail {
    pattern: RetailPattern::P445,
    year_end_month: 12,
    end_rule: YearEndRule::LastWeekday(Weekday::Sat),
};

fn d(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("date")
}

fn r(from: CalendarDate, to: CalendarDate) -> DateRange {
    DateRange { from, to }
}

fn def(as_of: bool, compare: &[CompareBase], aging: bool) -> BuilderDefinition {
    let (source, measure) = if aging {
        ("ar_open", "open_balance")
    } else if as_of {
        ("gl", "balance")
    } else {
        ("gl", "amount")
    };
    let text = serde_json::json!({
        "schemaVersion": 1, "kind": "builder",
        "dates": {"mode": if as_of { "asOf" } else { "range" }},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [{"type": "section", "id": "s1", "caption": "Income", "source": source,
                   "measure": measure, "filters": [], "polarity": if aging { serde_json::Value::Null } else { "debit".into() }, "groupBy": [], "display": "summary",
                   "defaultExpanded": false, "showTotal": true, "hideZero": false}]
    })
    .to_string();
    let mut def = parse(&text).expect("definition");
    def.columns.compare = compare
        .iter()
        .map(|b| CompareSet {
            base: *b,
            amount: true,
            dollar_change: true,
            percent_change: false,
        })
        .collect();
    if aging {
        def.columns.aging_buckets = Some(true);
    }
    def
}

fn opts(spec: &ReportSpec, dates: DateChoice, columns: ColumnBy) -> ReportOptions {
    let mut o = ReportOptions::for_spec(spec);
    o.dates = dates;
    o.columns = columns;
    o
}

#[test]
fn the_period_before_october_is_september_1_to_30() {
    let def = def(false, &[CompareBase::PriorPeriod], false);
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Fixed {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        },
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(
        got.dates,
        RunSpan::Range(r(d(2026, 10, 1), d(2026, 10, 31)))
    );
    assert_eq!(got.period_ranges, Vec::<DateRange>::new());
    // Not the engine's same-length shift (August 31 - September 30).
    assert_eq!(
        got.compare_ranges,
        vec![vec![r(d(2026, 9, 1), d(2026, 9, 30))]]
    );
    assert_eq!(got.fiscal_year_starts, vec![d(2025, 1, 1), d(2026, 1, 1)]);
}

#[test]
fn a_preset_month_compares_with_the_month_before() {
    let def = def(false, &[CompareBase::PriorPeriod], false);
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Preset(DatePreset::LastMonth),
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(
        got.compare_ranges,
        vec![vec![r(d(2026, 9, 1), d(2026, 9, 30))]]
    );
}

#[test]
fn custom_dates_compare_with_the_same_number_of_days_before() {
    let def = def(false, &[CompareBase::PriorPeriod], false);
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Fixed {
            from: d(2026, 10, 5),
            to: d(2026, 10, 14),
        },
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(
        got.compare_ranges,
        vec![vec![r(d(2026, 9, 25), d(2026, 10, 4))]]
    );
}

#[test]
fn month_columns_compare_each_with_its_own_month_before_and_last_year() {
    let def = def(
        false,
        &[CompareBase::PriorPeriod, CompareBase::PriorYear],
        false,
    );
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Fixed {
            from: d(2028, 1, 1),
            to: d(2028, 3, 31),
        },
        ColumnBy::Month,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2028, 4, 2)).expect("ranges");
    assert_eq!(
        got.period_ranges,
        vec![
            r(d(2028, 1, 1), d(2028, 1, 31)),
            r(d(2028, 2, 1), d(2028, 2, 29)),
            r(d(2028, 3, 1), d(2028, 3, 31)),
        ]
    );
    assert_eq!(
        got.compare_ranges[0],
        vec![
            r(d(2027, 12, 1), d(2027, 12, 31)),
            r(d(2028, 1, 1), d(2028, 1, 31)),
            r(d(2028, 2, 1), d(2028, 2, 29)),
        ]
    );
    // Leap February ends on February 28 a year back.
    assert_eq!(
        got.compare_ranges[1],
        vec![
            r(d(2027, 1, 1), d(2027, 1, 31)),
            r(d(2027, 2, 1), d(2027, 2, 28)),
            r(d(2027, 3, 1), d(2027, 3, 31)),
        ]
    );
    assert_eq!(
        got.fiscal_year_starts,
        vec![d(2026, 1, 1), d(2027, 1, 1), d(2028, 1, 1)]
    );
}

#[test]
fn prior_year_on_a_4_4_5_calendar_goes_by_period_number() {
    let def = def(false, &[CompareBase::PriorYear], false);
    let fy = RETAIL.year_containing(d(2026, 6, 1)).expect("fy");
    let prev = RETAIL
        .year_containing(fy.start.add_days(-1).expect("day"))
        .expect("prev");
    let periods = RETAIL.periods(&fy).expect("periods");
    let before = RETAIL.periods(&prev).expect("periods");
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Fixed {
            from: periods[3].start,
            to: periods[5].end,
        },
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &RETAIL, d(2026, 8, 1)).expect("ranges");
    assert_eq!(
        got.compare_ranges,
        vec![vec![r(before[3].start, before[5].end)]]
    );
    assert!(got.fiscal_year_starts.contains(&prev.start));
}

#[test]
fn an_as_of_balance_sheet_compares_with_a_year_before() {
    let def = def(true, &[CompareBase::PriorYear], false);
    let o = opts(
        &BUILDER_AS_OF,
        DateChoice::AsOf(d(2026, 10, 31)),
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_AS_OF, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(got.dates, RunSpan::AsOf(d(2026, 10, 31)));
    assert_eq!(got.period_ranges, Vec::<DateRange>::new());
    assert_eq!(got.compare_ranges.len(), 1);
    assert_eq!(got.compare_ranges[0].len(), 1);
    assert_eq!(got.compare_ranges[0][0].to, d(2025, 10, 31));
}

#[test]
fn an_as_of_date_compares_with_the_end_of_the_period_before() {
    let def = def(true, &[CompareBase::PriorPeriod], false);
    let o = opts(
        &BUILDER_AS_OF,
        DateChoice::Preset(DatePreset::EndOfLastMonth),
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_AS_OF, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(got.dates, RunSpan::AsOf(d(2026, 10, 31)));
    assert_eq!(got.compare_ranges[0][0].to, d(2026, 9, 30));
}

#[test]
fn as_of_month_columns_compare_each_month_end_with_the_one_before() {
    let def = def(
        true,
        &[CompareBase::PriorPeriod, CompareBase::PriorYear],
        false,
    );
    let o = opts(
        &BUILDER_AS_OF,
        DateChoice::AsOf(d(2026, 3, 31)),
        ColumnBy::Month,
    );
    let got = run_ranges(&def, &BUILDER_AS_OF, &o, &CAL, d(2026, 4, 2)).expect("ranges");
    assert_eq!(got.dates, RunSpan::AsOf(d(2026, 3, 31)));
    assert_eq!(got.period_ranges.len(), 3);
    let ends = |v: &[DateRange]| v.iter().map(|r| r.to).collect::<Vec<_>>();
    assert_eq!(
        ends(&got.compare_ranges[0]),
        vec![d(2025, 12, 31), d(2026, 1, 31), d(2026, 2, 28)]
    );
    assert_eq!(
        ends(&got.compare_ranges[1]),
        vec![d(2025, 1, 31), d(2025, 2, 28), d(2025, 3, 31)]
    );
}

#[test]
fn aging_columns_send_no_compare_ranges() {
    let def = def(true, &[], true);
    let o = opts(
        &BUILDER_AS_OF_TOTAL,
        DateChoice::AsOf(d(2026, 10, 31)),
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_AS_OF_TOTAL, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(got.compare_ranges, Vec::<Vec<DateRange>>::new());
    assert_eq!(got.period_ranges, Vec::<DateRange>::new());
}

#[test]
fn no_compare_sends_none() {
    let def = def(false, &[], false);
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Preset(DatePreset::ThisYear),
        ColumnBy::Quarter,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(got.compare_ranges, Vec::<Vec<DateRange>>::new());
    assert_eq!(got.period_ranges.len(), 4);
    assert_eq!(got.fiscal_year_starts, vec![d(2025, 1, 1), d(2026, 1, 1)]);
}

fn spec_strategy() -> impl Strategy<Value = FiscalYearSpec> {
    prop_oneof![
        (1u8..=12).prop_map(|m| FiscalYearSpec::Calendar { start_month: m }),
        (1u8..=12).prop_map(|m| FiscalYearSpec::Retail {
            pattern: RetailPattern::P445,
            year_end_month: m,
            end_rule: YearEndRule::NearestToMonthEnd(Weekday::Sat),
        }),
    ]
}

/// One random case: fiscal calendar, start offset, length, columns, as of,
/// compare bases.
type Case = (FiscalYearSpec, i64, i64, ColumnBy, bool, Vec<CompareBase>);

fn case_strategy() -> impl Strategy<Value = Case> {
    (
        spec_strategy(),
        0i64..2_000,
        0i64..400,
        prop_oneof![
            Just(ColumnBy::Total),
            Just(ColumnBy::Month),
            Just(ColumnBy::Quarter),
            Just(ColumnBy::Year)
        ],
        any::<bool>(),
        prop::collection::vec(
            prop_oneof![Just(CompareBase::PriorPeriod), Just(CompareBase::PriorYear)],
            0..=2,
        ),
    )
}

/// The engine's own checks on what is sent: every compare array is as long
/// as the base columns; periods and each compare array ascend without
/// overlap; every range runs forward; every date has a fiscal year start on
/// or before it, within 371 days. The only refusal allowed is the column
/// cap (R10), and most cases are accepted, so the checks really ran.
#[test]
fn ranges_are_what_the_engine_accepts() {
    use proptest::test_runner::{TestCaseError, TestRunner};
    let accepted = std::cell::Cell::new(0usize);
    let capped = std::cell::Cell::new(0usize);
    let mut runner = TestRunner::new(ProptestConfig::with_cases(256));
    let result = runner.run(
        &case_strategy(),
        |(fiscal, start, len, columns, as_of, bases)| {
            let from = d(2022, 1, 1).add_days(start).expect("from");
            let to = from.add_days(len).expect("to");
            let def = def(as_of, &bases, false);
            let spec = if as_of {
                &BUILDER_AS_OF
            } else {
                &BUILDER_RANGE
            };
            let dates = if as_of {
                DateChoice::AsOf(to)
            } else {
                DateChoice::Fixed { from, to }
            };
            let o = opts(spec, dates, columns);
            let got = match run_ranges(&def, spec, &o, &fiscal, to) {
                Ok(got) => got,
                Err(ResolveError::TooManyColumns) => {
                    capped.set(capped.get() + 1);
                    return Ok(());
                }
                Err(e) => return Err(TestCaseError::fail(format!("refused: {e:?}"))),
            };
            accepted.set(accepted.get() + 1);
            let base_len = got.period_ranges.len().max(1);
            // Empty only when a prior period can't be placed: the engine's
            // own rule then applies (R9).
            if got.compare_ranges.is_empty() {
                prop_assert!(bases.is_empty() || bases.contains(&CompareBase::PriorPeriod));
            } else {
                prop_assert_eq!(got.compare_ranges.len(), bases.len());
            }
            for c in &got.compare_ranges {
                prop_assert_eq!(c.len(), base_len);
            }
            for v in std::iter::once(&got.period_ranges).chain(got.compare_ranges.iter()) {
                for x in v {
                    prop_assert!(x.from <= x.to);
                }
                for w in v.windows(2) {
                    prop_assert!(w[0].to < w[1].from);
                }
            }
            let starts = &got.fiscal_year_starts;
            prop_assert!(!starts.is_empty() && starts.len() <= MAX_FISCAL_YEAR_STARTS);
            prop_assert!(starts.windows(2).all(|w| w[0] < w[1]));
            let mut days: Vec<CalendarDate> = match got.dates {
                RunSpan::Range(r) => vec![r.from, r.to],
                RunSpan::AsOf(d) => vec![d],
            };
            for x in got
                .period_ranges
                .iter()
                .chain(got.compare_ranges.iter().flatten())
            {
                days.push(x.from);
                days.push(x.to);
            }
            for day in days {
                let s = starts.iter().rev().find(|s| **s <= day);
                prop_assert!(
                    s.is_some_and(|s| s.days_until(day) <= 371),
                    "no start for {:?}",
                    day
                );
            }
            Ok(())
        },
    );
    assert!(result.is_ok(), "{result:?}");
    assert!(
        accepted.get() >= 200,
        "only {} of 256 cases ran the checks ({} capped)",
        accepted.get(),
        capped.get()
    );
}

/// R9: a prior period the desktop can't place sends no compare ranges (the
/// engine's rule), never a refusal. A one-day custom range on a 4-4-5
/// calendar at its first day still resolves, and whatever is sent is
/// parallel.
#[test]
fn an_unplaceable_prior_period_leaves_the_engines_rule() {
    let def = def(
        false,
        &[CompareBase::PriorPeriod, CompareBase::PriorYear],
        false,
    );
    let o = opts(
        &BUILDER_RANGE,
        DateChoice::Fixed {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        },
        ColumnBy::Total,
    );
    let got = run_ranges(&def, &BUILDER_RANGE, &o, &CAL, d(2026, 11, 12)).expect("ranges");
    assert_eq!(got.compare_ranges.len(), 2, "both placed");
    assert!(
        prior_ranges(
            &ReportDates::Ranges(vec![r(d(2026, 10, 1), d(2026, 10, 31))]),
            ColumnBy::Total,
            r(d(2026, 10, 1), d(2026, 10, 31)),
            CAL,
            false,
        )
        .is_none(),
        "Total columns over a split range have no unit: the engine's rule"
    );
}
/// R12: the contract tests (`cl-api` tests/contract, which can't depend on
/// cl-model) send the ranges in `fixtures/report_builder/compare_ranges.json`;
/// this checks they are exactly what the desktop computes for those runs.
#[test]
fn the_contract_tests_send_what_the_desktop_computes() {
    let year = 2034;
    let cases = [
        (
            "prior_period_october",
            def(false, &[CompareBase::PriorPeriod], false),
            &BUILDER_RANGE,
            DateChoice::Fixed {
                from: d(year, 10, 1),
                to: d(year, 10, 31),
            },
        ),
        (
            "prior_year_as_of",
            def(true, &[CompareBase::PriorYear], false),
            &BUILDER_AS_OF,
            DateChoice::AsOf(d(year + 1, 3, 31)),
        ),
    ];
    let mut computed = serde_json::Map::new();
    for (name, def, spec, dates) in cases {
        let o = opts(spec, dates, ColumnBy::Total);
        let got = run_ranges(&def, spec, &o, &CAL, d(year + 1, 4, 1)).expect("ranges");
        computed.insert(
            name.into(),
            serde_json::json!({
                "compareRanges": got.compare_ranges,
                "fiscalYearStarts": got.fiscal_year_starts,
            }),
        );
    }
    let computed = serde_json::Value::Object(computed);
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/report_builder/compare_ranges.json"
    ))
    .expect("fixture");
    assert_eq!(
        fixture,
        computed,
        "update the fixture to:\n{}",
        serde_json::to_string_pretty(&computed).unwrap_or_default()
    );
}
