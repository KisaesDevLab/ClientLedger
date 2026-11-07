//! Adjust Quantity/Value on Hand over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen() -> (tempfile::TempDir, AdjustScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    assert!(super::super::demo_reset(&link));
    let s = AdjustScreen::new(link, DemoState::today());
    (dir, s)
}

fn part(n: u32) -> cl_core::ids::ItemId {
    crate::company::demo::lists::item_id(crate::company::demo::stock::FIRST_PART + n)
}

#[test]
fn a_quantity_adjustment_posts_at_average_cost() {
    let (_d, mut s) = screen();
    assert_eq!(s.state()["parts"], 3);
    assert_eq!(s.state()["account"], true, "starts on cost of goods sold");
    s.draft.rows[0].item = Some(part(0));
    s.draft.rows[0].quantity = "-4".into();
    assert!(s.save(SaveIntent::SaveAndNew));
    assert_eq!(s.state()["saved"], 1);
    assert_eq!(s.state()["lines"], 0, "a fresh form");
    let on_hand = s
        .link
        .demo_state()
        .expect("demo")
        .inventory
        .stock(part(0))
        .expect("stock")
        .quantity_on_hand;
    assert_eq!(on_hand, cl_core::Quantity::parse_wire("116").ok());
}

#[test]
fn a_closed_date_is_refused_before_anything_is_sent() {
    let (_d, mut s) = screen();
    s.date = DateInput::new(CalendarDate::from_ymd(2026, 9, 30).expect("d"));
    s.draft.rows[0].item = Some(part(0));
    s.draft.rows[0].quantity = "1".into();
    assert!(!s.save(SaveIntent::SaveAndNew));
    assert_eq!(s.state()["problems"][0], "closed_period");
    assert_eq!(s.state()["saved"], 0);
}

#[test]
fn every_widget_has_a_name() {
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        let (_d, mut s) = screen();
        s.draft.rows[0].item = Some(part(1));
        let unnamed = crate::screens::sales_tax::unnamed_in(&mut s, theme);
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }
}
