//! `TaxRate` properties (P23 plan §6.3): the wire form round-trips over the
//! whole six-place range, the four-place form of engines before U-148
//! decodes equal to its six-place padding, percents round-trip, and JSON
//! numbers are refused.

use cl_core::{Rate, TaxRate};
use proptest::prelude::*;

proptest! {
    #[test]
    fn wire_round_trips(u in -9_999_999_i64..=9_999_999) {
        let r = TaxRate::from_millionths(u).unwrap();
        prop_assert_eq!(TaxRate::parse_wire(&r.to_wire()).unwrap(), r);
        prop_assert_eq!(TaxRate::parse_wire(&r.to_wire6()).unwrap(), r);
        prop_assert_eq!(r.to_millionths(), u);
    }

    #[test]
    fn four_places_equal_their_six_place_padding(u in -99_999_i64..=99_999) {
        let four = Rate::from_ten_thousandths(i128::from(u)).unwrap().to_wire();
        let six = format!("{four}00");
        prop_assert_eq!(TaxRate::parse_wire(&four).unwrap(), TaxRate::parse_wire(&six).unwrap());
        prop_assert_eq!(TaxRate::parse_wire(&four).unwrap().to_wire(), four);
    }

    #[test]
    fn percents_round_trip(u in 0_i64..1_000_000) {
        let r = TaxRate::from_millionths(u).unwrap();
        prop_assert!(r.is_setup_valid());
        prop_assert_eq!(TaxRate::parse_percent(&r.format_percent()).unwrap(), r);
        prop_assert_eq!(TaxRate::parse_percent(&r.percent_input()).unwrap(), r);
    }

    #[test]
    fn json_numbers_are_refused(u in 0_i64..1_000_000) {
        let r = TaxRate::from_millionths(u).unwrap();
        prop_assert!(serde_json::from_str::<TaxRate>(&r.to_wire()).is_err());
        let s = serde_json::to_string(&r).unwrap();
        prop_assert_eq!(serde_json::from_str::<TaxRate>(&s).unwrap(), r);
    }
}
