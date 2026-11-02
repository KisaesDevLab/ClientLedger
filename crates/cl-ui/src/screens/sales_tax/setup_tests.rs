//! Tests of Sales Tax Setup over the demo engine.

use super::*;

fn screen() -> (tempfile::TempDir, SetupScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = SetupScreen::new(link);
    (dir, s)
}

#[test]
fn the_seed_lists_taxes_groups_agencies_and_codes() {
    let (_d, s) = screen();
    let v = s.state();
    assert_eq!(v["screen"], "sales_tax_setup");
    assert_eq!(v["items"], 4);
    assert_eq!(v["groups"], 1);
    assert_eq!(v["agencies"], 2);
    assert_eq!(v["codes"], 3);
    assert_eq!(v["legacy"], false);
    let monett = s.items.iter().find(|i| i.name == "Monett").expect("group");
    assert_eq!(monett.rate.format_percent(), "8.100%");
}

#[test]
fn a_tax_saves_with_its_agency_and_a_duplicate_name_is_refused() {
    let (_d, mut s) = screen();
    s.new_entry();
    if let Some(Editor::Item { ed, .. }) = s.editor.as_mut() {
        ed.name = "Branson".into();
        ed.percent = "7.25".into();
    }
    s.save();
    assert_eq!(s.problem, Some("sales_tax.problem.agency_required"));
    if let Some(Editor::Item { ed, .. }) = s.editor.as_mut() {
        ed.agency = Some(crate::company::demo::sales_tax::agency_id(1));
    }
    s.save();
    assert_eq!(s.saved, 1, "{:?}", s.problem);
    assert!(s.editor.is_none());
    let b = s.items.iter().find(|i| i.name == "Branson").expect("saved");
    assert_eq!(b.rate.to_wire6(), "0.072500");
    s.new_entry();
    if let Some(Editor::Item { ed, .. }) = s.editor.as_mut() {
        ed.name = "branson".into();
        ed.percent = "1".into();
        ed.agency = Some(crate::company::demo::sales_tax::agency_id(1));
    }
    s.save();
    assert_eq!(s.problem, Some("sales_tax.refusal.name_in_use"));
}

#[test]
fn a_group_needs_a_member_and_a_member_in_use_stays_active() {
    let (_d, mut s) = screen();
    s.new_group();
    if let Some(Editor::Group { ed, .. }) = s.editor.as_mut() {
        ed.name = "Ozark County".into();
    }
    s.save();
    assert_eq!(s.problem, Some("sales_tax.problem.group_empty"));
    if let Some(Editor::Group { ed, .. }) = s.editor.as_mut() {
        ed.add(crate::company::demo::sales_tax::item_id(3));
    }
    s.save();
    assert_eq!(s.saved, 1);
    assert_eq!(s.state()["groups"], 2);
    // State is in Monett: it can't be made inactive.
    let state = crate::company::demo::sales_tax::item_id(1);
    s.open_item(state);
    s.toggle_active();
    assert_eq!(s.problem, Some("sales_tax.refusal.item_in_group"));
}

#[test]
fn codes_and_agencies_deactivate_never_delete() {
    let (_d, mut s) = screen();
    s.tab = Tab::Codes;
    s.new_entry();
    if let Some(Editor::Code { ed, .. }) = s.editor.as_mut() {
        ed.code = "Out".into();
        ed.name = "Out of state".into();
    }
    s.save();
    assert_eq!(s.state()["codes"], 4);
    let out = s.codes.iter().find(|c| c.code == "Out").expect("code").id;
    s.open_code(out);
    s.toggle_active();
    assert_eq!(s.state()["codes"], 3);
    s.show_inactive = true;
    s.refresh();
    assert_eq!(s.state()["codes"], 4);
    // System codes are read only.
    let tax = s.codes.iter().find(|c| c.code == "Tax").expect("Tax").id;
    s.open_code(tax);
    assert_eq!(s.state()["editing"]["system"], true);
    s.tab = Tab::Agencies;
    s.new_entry();
    s.save();
    assert_eq!(s.problem, Some("sales_tax.setup.pick_vendor"));
    let vendor = s.vendors.first().map(|v| v.id).expect("a vendor");
    if let Some(Editor::Agency { vendor: v }) = s.editor.as_mut() {
        *v = Some(vendor);
    }
    s.save();
    assert_eq!(s.state()["agencies"], 3);
    for k in [
        "sales_tax.setup.title",
        "sales_tax.setup.legacy_banner",
        "sales_tax.setup.in_use_note",
        "sales_tax.setup.empty_items",
        "sales_tax.problem.group_empty",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
}

#[test]
fn ctrl_n_and_the_list_commands_work_in_the_window() {
    let (_d, mut s) = screen();
    assert!(s.command(CommandId::new("list.new")));
    assert_eq!(s.state()["editing"]["kind"], "item");
    assert!(s.command(CommandId::new("list.show_inactive")));
    assert!(s.show_inactive);
    assert!(!s.command(CommandId::new("form.save_close")));
    assert_eq!(s.scope(), Scope::Window);
}
