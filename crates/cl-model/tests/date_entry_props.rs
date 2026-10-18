//! Property tests for date entry (P10 plan §6): display and parse round-trip
//! in both orders, and the date keys keep their invariants on every day the
//! books support.

use cl_core::{CalendarDate, Weekday};
use cl_model::date_entry::{DateOrder, apply_key, format, parse};
use cl_model::date_rules::DateKey;
use proptest::prelude::*;

/// Days from 1900-01-01 to 2199-12-31.
const SPAN: i64 = 109_572;

fn day(offset: i64) -> CalendarDate {
    CalendarDate::min().add_days(offset).unwrap()
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).unwrap()
}

fn go(c: char, d: CalendarDate, first: Weekday) -> Option<CalendarDate> {
    apply_key(
        DateKey::from_char(c).unwrap(),
        &format(d, DateOrder::Mdy),
        None,
        DateOrder::Mdy,
        today(),
        first,
    )
}

fn weekday() -> impl Strategy<Value = Weekday> {
    prop_oneof![Just(Weekday::Sun), Just(Weekday::Mon), Just(Weekday::Sat)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3_000))]

    #[test]
    fn format_then_parse_is_identity(o in 0..=SPAN) {
        let d = day(o);
        for order in [DateOrder::Mdy, DateOrder::Ymd] {
            prop_assert_eq!(parse(&format(d, order), order, today()), Ok(d));
        }
    }

    #[test]
    fn next_then_previous_day_returns(o in 0..SPAN) {
        let d = day(o);
        let next = go('+', d, Weekday::Sun).unwrap();
        prop_assert_eq!(d.days_until(next), 1);
        prop_assert_eq!(go('-', next, Weekday::Sun), Some(d));
    }

    #[test]
    fn month_and_year_keys_bracket_the_day(o in 0..=SPAN) {
        let d = day(o);
        let (m, h) = (go('m', d, Weekday::Sun).unwrap(), go('h', d, Weekday::Sun).unwrap());
        prop_assert!(m <= d && d <= h);
        prop_assert_eq!(m.day(), 1);
        prop_assert_eq!(h.add_days(1).map_or(1, CalendarDate::day), 1, "H is the month's last day");
        let (y, r) = (go('y', d, Weekday::Sun).unwrap(), go('r', d, Weekday::Sun).unwrap());
        prop_assert!(y <= d && d <= r);
        prop_assert_eq!((y.month(), y.day(), r.month(), r.day()), (1, 1, 12, 31));
    }

    #[test]
    fn week_keys_bracket_the_day(o in 7..SPAN - 7, first in weekday()) {
        let d = day(o);
        let (w, k) = (go('w', d, first).unwrap(), go('k', d, first).unwrap());
        prop_assert!(w <= d && d <= k);
        prop_assert_eq!(w.days_until(k), 6);
        prop_assert_eq!(w.weekday(), first);
    }

    #[test]
    fn arbitrary_text_never_panics(s in "\\PC{0,30}", o in 0..=SPAN) {
        let _ = parse(&s, DateOrder::Mdy, day(o));
        let _ = parse(&s, DateOrder::Ymd, day(o));
    }

    #[test]
    fn date_shaped_text_never_panics(s in "[0-9/ -]{0,14}") {
        if let Ok(d) = parse(&s, DateOrder::Mdy, today()) {
            prop_assert!(CalendarDate::min() <= d && d <= CalendarDate::max());
        }
    }
}
