//! The physical count worksheet over the demo engine.

use super::*;
use crate::company::demo::DemoState;

fn screen() -> (tempfile::TempDir, CountScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    assert!(super::super::demo_reset(&link));
    let s = CountScreen::new(link, DemoState::today());
    (dir, s)
}

#[test]
fn a_stale_sheet_reloads_keeps_counts_and_marks_rows() {
    let (_d, mut s) = screen();
    assert_eq!(s.state()["rows"], 3);
    assert_eq!(s.state()["can_post"], false, "Post waits for a count");
    let first = s
        .sheet
        .rows
        .iter()
        .position(|r| r.name == "Hardwood Mulch Bag")
        .expect("row");
    s.sheet.rows[first].counted = "110".into();
    assert!(s.command(CommandId::new("dev.demo_inventory_sale")));
    assert!(s.post());
    let v = s.state();
    assert_eq!(v["posted"], 0);
    assert_eq!(v["changed"], 1);
    assert_eq!(v["counted"], 1, "the typed count stays");
    assert_eq!(v["notice"], true);
    assert!(s.post());
    assert_eq!(s.state()["posted"], 1);
}

#[test]
fn the_count_sheet_prints() {
    let (_d, mut s) = screen();
    s.print_on_hand = true;
    s.print();
    assert_eq!(s.state()["printing"], false, "the demo makes it at once");
    assert_eq!(s.state()["error"], false);
}
