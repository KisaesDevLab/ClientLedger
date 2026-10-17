//! Property tests for fiscal calendars (P05 plan §7: periods contiguous and
//! covering, 52/53-week years, year boundaries).

use cl_core::dates::{CalendarDate, Weekday};
use cl_core::fiscal::{FiscalYearSpec, RetailPattern, YearEndRule};
use proptest::prelude::*;

fn spec() -> impl Strategy<Value = FiscalYearSpec> {
    let weekday = (0u8..7).prop_map(|n| Weekday::try_from(n).unwrap());
    let pattern = prop_oneof![
        Just(RetailPattern::P445),
        Just(RetailPattern::P454),
        Just(RetailPattern::P544)
    ];
    let rule = (weekday, any::<bool>()).prop_map(|(w, last)| {
        if last {
            YearEndRule::LastWeekday(w)
        } else {
            YearEndRule::NearestToMonthEnd(w)
        }
    });
    prop_oneof![
        (1u8..=12).prop_map(|start_month| FiscalYearSpec::Calendar { start_month }),
        (pattern, 1u8..=12, rule).prop_map(|(pattern, year_end_month, end_rule)| {
            FiscalYearSpec::Retail {
                pattern,
                year_end_month,
                end_rule,
            }
        }),
    ]
}

/// Dates away from the ends of the supported range (1902 .. 2197).
fn date() -> impl Strategy<Value = CalendarDate> {
    (800i64..=108_000).prop_map(|n| CalendarDate::min().add_days(n).unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn year_contains_the_date_and_neighbours_touch(s in spec(), d in date()) {
        let fy = s.year_containing(d).unwrap();
        prop_assert!(fy.contains(d));
        prop_assert_eq!(s.year(fy.label).unwrap(), fy);
        let next = s.year(fy.label + 1).unwrap();
        prop_assert_eq!(fy.end.add_days(1).unwrap(), next.start);
        let prev = s.year(fy.label - 1).unwrap();
        prop_assert_eq!(prev.end.add_days(1).unwrap(), fy.start);
        match s {
            FiscalYearSpec::Calendar { .. } => {
                prop_assert!(fy.days() == 365 || fy.days() == 366);
                prop_assert_eq!(fy.end.year(), fy.label);
            }
            FiscalYearSpec::Retail { end_rule, .. } => {
                prop_assert!(fy.days() == 364 || fy.days() == 371, "{}", fy.days());
                let w = match end_rule {
                    YearEndRule::LastWeekday(w) | YearEndRule::NearestToMonthEnd(w) => w,
                };
                prop_assert_eq!(fy.end.weekday(), w);
            }
        }
    }

    #[test]
    fn periods_are_contiguous_and_cover_the_year(s in spec(), d in date()) {
        let fy = s.year_containing(d).unwrap();
        let p = s.periods(&fy).unwrap();
        prop_assert_eq!(p[0].start, fy.start);
        prop_assert_eq!(p[11].end, fy.end);
        for (i, w) in p.windows(2).enumerate() {
            prop_assert_eq!(w[0].end.add_days(1).unwrap(), w[1].start);
            prop_assert!(w[0].start <= w[0].end);
            prop_assert_eq!(usize::from(w[0].number), i + 1);
        }
        let hits: Vec<_> = p.iter().filter(|x| x.contains(d)).collect();
        prop_assert_eq!(hits.len(), 1);
        prop_assert_eq!(*hits[0], s.period_containing(d).unwrap());
        if let FiscalYearSpec::Retail { pattern, .. } = s {
            let weeks = pattern.weeks();
            for (period, w) in p.iter().zip(weeks) {
                let len = period.start.days_until(period.end) + 1;
                let expected = 7 * i64::from(w) + if period.number == 12 { fy.days() - 364 } else { 0 };
                prop_assert_eq!(len, expected);
            }
        }
        let q = s.quarters(&fy).unwrap();
        prop_assert_eq!(q[0].start, fy.start);
        prop_assert_eq!(q[3].end, fy.end);
        for w in q.windows(2) {
            prop_assert_eq!(w[0].end.add_days(1).unwrap(), w[1].start);
        }
    }

    /// Over 400 consecutive retail years, 53-week years occur and every year
    /// is 52 or 53 weeks (the sum matches the elapsed days).
    #[test]
    fn retail_week_years_add_up(s in spec().prop_filter("retail", |s| matches!(s, FiscalYearSpec::Retail { .. }))) {
        let mut long = 0;
        let first = s.year(1905).unwrap();
        let mut last = first;
        for label in 1905..2195 {
            let fy = s.year(label).unwrap();
            if fy.days() == 371 {
                long += 1;
            }
            last = fy;
        }
        prop_assert!(long > 40 && long < 60, "{}", long);
        prop_assert_eq!(first.start.days_until(last.end) + 1, 7 * (52 * 290 + long));
    }
}
