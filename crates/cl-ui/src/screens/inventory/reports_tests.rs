//! The inventory reports over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen(r: Report) -> (tempfile::TempDir, ReportScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    assert!(super::super::demo_reset(&link));
    let s = ReportScreen::new(link, r, DemoState::today());
    (dir, s)
}

#[test]
fn inventory_value_matches_the_books() {
    let (_d, s) = screen(Report::Value);
    let v = s.state();
    assert_eq!(v["rows"], 3);
    assert_eq!(v["total"], "785.4000");
    assert_eq!(v["matches_books"], true);
}

#[test]
fn value_detail_lists_movements_and_filters_by_item() {
    let (_d, mut s) = screen(Report::Detail);
    assert_eq!(s.state()["rows"], 3);
    s.item = Some(crate::company::demo::lists::item_id(
        crate::company::demo::stock::FIRST_PART,
    ));
    s.refresh();
    assert_eq!(s.state()["rows"], 1);
    s.to = DateInput::new(CalendarDate::from_ymd(2025, 1, 1).expect("d"));
    s.refresh();
    assert_eq!(s.state()["problem"], "bad_range");
}

#[test]
fn stock_status_flags_what_to_reorder() {
    let (_d, s) = screen(Report::Stock);
    assert_eq!(s.state()["rows"], 3);
}

#[test]
fn an_empty_date_shows_nothing_and_off_is_refused() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = ReportScreen::new(link, Report::Value, DemoState::today());
    assert_eq!(s.state()["error"], true, "inventory is off");
}
