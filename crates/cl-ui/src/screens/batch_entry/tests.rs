use std::sync::Arc;

use super::*;

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn screen() -> (BulkEntryScreen, Arc<CompanyLink>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = BulkEntryScreen::new(Arc::clone(&link), None, day(11, 4));
    (s, link, dir)
}

#[test]
fn opens_its_own_kind_with_one_checks_section_on_the_bank() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let cmd = cl_model::commands::find(KIND).expect("command").id;
    let mut s = make(cmd, &link, None, day(11, 4)).expect("screen");
    assert_eq!(s.title(), "Bulk Entry");
    assert_eq!(s.scope(), Scope::Form);
    assert!(s.command(cmd));
    let v = s.state();
    assert_eq!(v["screen"], "batch_entry");
    assert_eq!(v["can_save"], false);
    assert_eq!(v["blocker"], "empty");
    assert_eq!(v["sections"], 1);
    assert_eq!(v["section_kind"], "checks");
    assert_eq!(v["section_list"][0]["has_account"], true);
    assert!(!s.dirty());
    let other = cl_model::commands::find("vendors.write_check")
        .expect("write check")
        .id;
    assert!(make(other, &link, None, day(11, 4)).is_none());
    let (s, _, _) = screen();
    assert!(s.books.loaded);
}
