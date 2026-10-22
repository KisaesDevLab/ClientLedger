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

#[test]
fn entry_rows_take_the_next_numbers_in_section_order() {
    let (mut s, _link, _dir) = screen();
    let bank = s.batch().sections[0].context.expect("bank");
    s.grid.adapter_mut().highest.insert(bank, 500);
    s.renumber_entry_rows();
    let last = |s: &BulkEntryScreen, i: usize| {
        s.batch().sections[i]
            .rows
            .last()
            .map(|r| r.number.clone())
            .unwrap_or_default()
    };
    assert_eq!(last(&s, 0), "501");
    assert!(!s.dirty(), "part of the blank window");
    let rev = s.grid.form().revision();
    s.renumber_entry_rows();
    assert_eq!(s.grid.form().revision(), rev, "nothing changed: no edit");
    // A second Checks section on the same bank: the same numbers as the
    // earlier copy-per-section way gave.
    s.add_section(SectionKind::Checks);
    assert_eq!(s.batch().sections[1].context, Some(bank));
    let want = reference_numbers(s.batch(), bank, 500);
    s.renumber_entry_rows();
    assert_eq!(vec![last(&s, 0), last(&s, 1)], want);
    // A typed check above the entry row moves both on.
    s.grid.edit_step(|b| {
        let mut r = cl_model::forms::batch::RowDraft::new(cl_grid::RowKey(9_999));
        r.number = "700".into();
        r.memo = "typed".into();
        let at = b.sections[0].rows.len() - 1;
        b.sections[0].rows.insert(at, r);
    });
    let want = reference_numbers(s.batch(), bank, 500);
    s.renumber_entry_rows();
    assert_eq!(vec![last(&s, 0), last(&s, 1)], want);
    assert!(want.iter().all(|n| n.parse::<u32>().is_ok_and(|n| n > 700)));
}

/// The entry rows' numbers as the copy-per-section way worked them out.
fn reference_numbers(
    b: &cl_model::forms::batch::BulkBatch,
    bank: cl_core::ids::AccountId,
    highest: u64,
) -> Vec<String> {
    let mut b = b.clone();
    for i in 0..b.sections.len() {
        let mut probe = b.clone();
        if let Some(r) = probe.sections[i].rows.last_mut() {
            r.number.clear();
        }
        let n = probe
            .next_check_number(bank, Some(highest))
            .unwrap_or_default();
        if let Some(r) = b.sections[i].rows.last_mut() {
            r.number = n;
        }
    }
    b.sections
        .iter()
        .map(|s| s.rows.last().map(|r| r.number.clone()).unwrap_or_default())
        .collect()
}
