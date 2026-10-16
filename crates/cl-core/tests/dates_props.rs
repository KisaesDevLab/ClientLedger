//! Property tests for calendar dates and timestamps (P05 plan §7, AC2).

use cl_core::dates::{CalendarDate, Timestamp, Weekday};
use proptest::prelude::*;

/// Days from 1900-01-01 to 2199-12-31.
const SPAN: i64 = 109_572;

fn date() -> impl Strategy<Value = CalendarDate> {
    (0..=SPAN).prop_map(|n| CalendarDate::min().add_days(n).unwrap())
}

fn weekday() -> impl Strategy<Value = Weekday> {
    (0u8..7).prop_map(|n| Weekday::try_from(n).unwrap())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn wire_round_trip(d in date()) {
        let w = d.to_wire();
        prop_assert_eq!(w.len(), 10);
        prop_assert_eq!(CalendarDate::parse_wire(&w), Ok(d));
        let json = serde_json::to_string(&d).unwrap();
        prop_assert_eq!(serde_json::from_str::<CalendarDate>(&json).unwrap(), d);
    }

    #[test]
    fn day_arithmetic_is_consistent(d in date(), n in -40_000i64..=40_000) {
        if let Ok(e) = d.add_days(n) {
            prop_assert_eq!(d.days_until(e), n);
            prop_assert_eq!(e.add_days(-n), Ok(d));
            prop_assert_eq!(d < e, n > 0);
        }
    }

    #[test]
    fn month_and_quarter_bounds(d in date()) {
        let (sm, em) = (d.start_of_month(), d.end_of_month());
        prop_assert!(sm <= d && d <= em);
        prop_assert_eq!(sm.day(), 1);
        prop_assert_eq!((sm.year(), sm.month()), (em.year(), em.month()));
        if let Ok(next) = em.add_days(1) {
            prop_assert_eq!(next.day(), 1);
        }
        let (sq, eq) = (d.start_of_quarter(), d.end_of_quarter());
        prop_assert!(sq <= d && d <= eq);
        prop_assert_eq!(sq.month() % 3, 1);
        prop_assert_eq!(eq.month() % 3, 0);
        prop_assert!(d.start_of_year() <= sq && eq <= d.end_of_year());
    }

    #[test]
    fn week_start_is_the_previous_first_day(d in date(), first in weekday()) {
        if let Ok(ws) = d.week_start(first) {
            prop_assert_eq!(ws.weekday(), first);
            let back = ws.days_until(d);
            prop_assert!((0..7).contains(&back));
        }
    }

    #[test]
    fn timestamp_millis_round_trip(ms in -2_208_988_800_000i64..=7_258_118_399_999) {
        let t = Timestamp::from_unix_millis(ms).unwrap();
        prop_assert_eq!(t.unix_millis(), ms);
        let w = t.to_wire();
        prop_assert!(w.ends_with('Z'));
        prop_assert_eq!(w.len(), 24, "{}", w);
        prop_assert_eq!(Timestamp::parse_wire(&w), Ok(t));
    }
}
