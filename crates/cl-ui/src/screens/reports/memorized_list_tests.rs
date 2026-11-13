//! The Memorized Reports list on the demo engine (A09 S1): grouping,
//! Q-310 gating, rename (a builder title follows), Memorize a copy, 409 and
//! 403, groups, and Open.

use super::*;
use crate::company::demo::report_store;

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    if let Some(mut s) = link.demo_state() {
        s.open = s.companies.first().map(|c| c.tenant_id);
    }
    (dir, link)
}

fn list(link: &Arc<CompanyLink>) -> MemorizedList {
    MemorizedList::new(Arc::clone(link), None)
}

fn row<'a>(s: &'a MemorizedList, name: &str) -> &'a DefinitionRow {
    s.rows.iter().find(|r| r.name == name).expect(name)
}

fn select(s: &mut MemorizedList, name: &str) {
    let id = row(s, name).id.clone();
    s.selected = Some(Selected::Report(id));
}

fn pl_builder() -> Value {
    crate::company::demo::report_builder::starters()
        .expect("starters")
        .into_iter()
        .find(|s| s.key == "profit_loss")
        .expect("p&l")
        .definition
}

/// A builder row memorized in the demo store.
fn add_builder(link: &CompanyLink, name: &str) -> DefinitionRow {
    let mut d = link.demo_state().expect("demo");
    report_store::save_definition(
        &mut d,
        None,
        &DefinitionWrite {
            name: name.into(),
            group_id: None,
            kind: "builder".into(),
            definition: titled(&pl_builder(), name),
            origin: None,
        },
    )
    .expect("saved")
}

#[test]
fn a_copy_takes_the_next_free_name_ignoring_case() {
    assert_eq!(copy_name("Sales", &["Sales"]), "Sales (copy)");
    assert_eq!(
        copy_name("Sales", &["Sales", "sales (COPY)"]),
        "Sales (copy 2)"
    );
    assert_eq!(
        copy_name("Sales", &["Sales (copy)", "Sales (copy 2)"]),
        "Sales (copy 3)"
    );
    // Kept within the engine's 200 characters.
    let long = "x".repeat(MAX_NAME);
    let c = copy_name(&long, &[long.as_str()]);
    assert_eq!(c.chars().count(), MAX_NAME);
    assert!(c.ends_with(" (copy)"));
}

#[test]
fn the_list_shows_every_report_with_its_author_and_report() {
    let (_d, link) = demo();
    add_builder(&link, "Builder P&L");
    let s = list(&link);
    assert!(s.loaded);
    assert_eq!(s.rows.len(), 3);
    let st = s.state();
    assert_eq!(st["screen"], "memorized_reports");
    assert_eq!(st["shown"], 3);
    assert_eq!(report_title(row(&s, "Builder P&L")), "Report Builder");
    assert_eq!(
        report_title(row(&s, "Monthly P&L for the bank")),
        "Profit & Loss"
    );
    assert_eq!(
        row(&s, "Balance sheet for the board")
            .created_by_name
            .as_deref(),
        Some("Dana")
    );
    assert_eq!(s.scope(), Scope::ReportList);
}

#[test]
fn search_narrows_by_every_word() {
    let (_d, link) = demo();
    let mut s = list(&link);
    s.search = "BOARD balance".into();
    assert_eq!(s.state()["shown"], 1);
    s.search = "nothing like it".into();
    assert_eq!(s.state()["shown"], 0);
}

/// Q-310: another user's report offers Open and Memorize a copy only; the
/// reason shows for the selected row (keyboard users can't hover).
#[test]
fn a_report_someone_else_memorized_is_gated() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Balance sheet for the board");
    let gate = s.gate().expect("gated");
    assert!(gate.contains("Dana"), "{gate}");
    assert_eq!(s.state()["gated"], true);
    // F2, Ctrl+Delete and Move ask nothing; the reason shows instead.
    assert!(s.command(CommandId::new("reports.memorized_rename")));
    assert!(s.edit.is_none());
    assert!(s.command(CommandId::new("reports.memorized_delete")));
    assert!(s.edit.is_none());
    s.ask_move();
    assert!(s.edit.is_none());
    assert_eq!(s.notice.as_deref(), Some(gate.as_str()));
    // Mine is not gated.
    select(&mut s, "Monthly P&L for the bank");
    assert!(s.gate().is_none());
}

/// 403 `NOT_AUTHOR` (the engine is the enforcer): the gate's words and a
/// reload.
#[test]
fn the_engines_not_author_refusal_says_the_same() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Balance sheet for the board");
    let id = row(&s, "Balance sheet for the board").id.clone();
    // As if the list had read canEdit wrong.
    s.edit = Some(Edit::Rename {
        id,
        name: "Mine now".into(),
    });
    s.confirm();
    s.poll();
    assert!(s.edit.is_none());
    assert!(s.notice.as_deref().is_some_and(|n| n.contains("Dana")));
    assert!(s.rows.iter().all(|r| r.name != "Mine now"));
}

#[test]
fn renaming_a_standard_report_keeps_its_settings() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Monthly P&L for the bank");
    let before = row(&s, "Monthly P&L for the bank").clone();
    assert!(s.command(CommandId::new("reports.memorized_rename")));
    assert_eq!(s.state()["edit"], "rename");
    assert_eq!(s.scope(), Scope::Window, "the card has the keys");
    if let Some(Edit::Rename { name, .. }) = s.edit.as_mut() {
        *name = "  P&L for the bank  ".into();
    }
    s.confirm();
    s.poll();
    assert_eq!(s.last_action, Some("renamed"));
    let after = row(&s, "P&L for the bank");
    assert_eq!(after.definition, before.definition);
    assert_eq!(after.version, before.version + 1);
}

/// Q-A08r: a builder report's title follows its name.
#[test]
fn renaming_a_builder_report_renames_its_title() {
    let (_d, link) = demo();
    add_builder(&link, "Builder P&L");
    let mut s = list(&link);
    select(&mut s, "Builder P&L");
    s.ask_rename();
    if let Some(Edit::Rename { name, .. }) = s.edit.as_mut() {
        *name = "Owner P&L".into();
    }
    s.confirm();
    s.poll();
    assert_eq!(row(&s, "Owner P&L").definition["title"], "Owner P&L");
}

#[test]
fn a_taken_name_stays_on_the_card() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Monthly P&L for the bank");
    s.ask_rename();
    if let Some(Edit::Rename { name, .. }) = s.edit.as_mut() {
        *name = "balance sheet for the BOARD".into();
    }
    s.confirm();
    s.poll();
    assert_eq!(s.state()["edit"], "rename");
    assert_eq!(s.problem, Some("reports.memorize.name_taken"));
}

/// 409 `STALE_VERSION`: the list says so and reloads.
#[test]
fn a_report_changed_elsewhere_reloads_and_asks_to_try_again() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Monthly P&L for the bank");
    let id = row(&s, "Monthly P&L for the bank").id.clone();
    {
        let mut d = link.demo_state().expect("demo");
        let r = report_store::definition(&mut d, &id).expect("row");
        report_store::patch_definition(
            &mut d,
            &id,
            r.version,
            &DefinitionPatch {
                name: Some("Changed in another window".into()),
                ..DefinitionPatch::default()
            },
        )
        .expect("changed");
    }
    s.ask_rename();
    if let Some(Edit::Rename { name, .. }) = s.edit.as_mut() {
        *name = "Mine".into();
    }
    s.confirm();
    s.poll();
    assert!(s.edit.is_none());
    assert_eq!(
        s.notice.as_deref(),
        Some(t("reports.memorized.stale").as_str())
    );
    assert!(s.rows.iter().any(|r| r.name == "Changed in another window"));
}

/// Memorize a copy of someone else's report: mine, same group, selected,
/// and other report windows are told.
#[test]
fn memorize_a_copy_makes_one_of_mine() {
    let (_d, link) = demo();
    let mut s = list(&link);
    let mut other = link.bus().subscribe();
    let _ = link.bus().take(&mut other);
    select(&mut s, "Balance sheet for the board");
    s.memorize_copy();
    s.poll();
    let copy = row(&s, "Balance sheet for the board (copy)").clone();
    assert!(copy.can_edit);
    assert_eq!(
        copy.definition,
        row(&s, "Balance sheet for the board").definition
    );
    assert_eq!(s.selected, Some(Selected::Report(copy.id)));
    assert!(link.bus().take(&mut other).reports);
    // A second copy takes the next name.
    select(&mut s, "Balance sheet for the board");
    s.memorize_copy();
    s.poll();
    row(&s, "Balance sheet for the board (copy 2)");
}

#[test]
fn a_builder_copy_has_its_own_title() {
    let (_d, link) = demo();
    add_builder(&link, "Builder P&L");
    let mut s = list(&link);
    select(&mut s, "Builder P&L");
    s.memorize_copy();
    s.poll();
    assert_eq!(
        row(&s, "Builder P&L (copy)").definition["title"],
        "Builder P&L (copy)"
    );
}

#[test]
fn groups_are_added_renamed_and_deleted_and_reports_move() {
    let (_d, link) = demo();
    let mut s = list(&link);
    assert!(s.command(CommandId::new("list.new")));
    if let Some(Edit::NewGroup { name }) = s.edit.as_mut() {
        *name = "For the bank".into();
    }
    s.confirm();
    s.poll();
    assert_eq!(s.groups.len(), 1);
    let gid = s.groups[0].id.clone();
    assert_eq!(s.selected, Some(Selected::Group(gid.clone())));
    // Move a report into it.
    select(&mut s, "Monthly P&L for the bank");
    s.ask_move();
    if let Some(Edit::Move { group, .. }) = s.edit.as_mut() {
        *group = Some(gid.clone());
    }
    s.confirm();
    s.poll();
    assert_eq!(
        row(&s, "Monthly P&L for the bank").group_id.as_deref(),
        Some(gid.as_str())
    );
    // The arrows step through the group heading and its report.
    s.selected = None;
    s.step(true);
    assert_eq!(s.selected, Some(Selected::Group(gid.clone())));
    s.step(true);
    assert_eq!(
        s.selected_row().map(|r| r.name.as_str()),
        Some("Monthly P&L for the bank")
    );
    // Rename the group (F2 on its heading).
    s.selected = Some(Selected::Group(gid.clone()));
    s.command(CommandId::new("reports.memorized_rename"));
    assert_eq!(s.state()["edit"], "rename_group");
    if let Some(Edit::RenameGroup { name, .. }) = s.edit.as_mut() {
        *name = "Bank".into();
    }
    s.confirm();
    s.poll();
    assert_eq!(s.groups[0].name, "Bank");
    // Delete it: asked first, then its report moves to no group.
    s.command(CommandId::new("reports.memorized_delete"));
    assert_eq!(s.state()["edit"], "delete_group");
    s.confirm();
    s.poll();
    assert_eq!(s.groups, Vec::<DefinitionGroup>::new());
    assert!(row(&s, "Monthly P&L for the bank").group_id.is_none());
}

#[test]
fn delete_asks_first_and_esc_cancels() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Monthly P&L for the bank");
    s.command(CommandId::new("reports.memorized_delete"));
    assert_eq!(s.state()["edit"], "delete");
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.edit.is_none());
    assert_eq!(s.rows.len(), 2);
    s.command(CommandId::new("list.delete"));
    s.confirm();
    s.poll();
    assert_eq!(s.last_action, Some("deleted"));
    assert!(s.rows.iter().all(|r| r.name != "Monthly P&L for the bank"));
}

/// Open (Enter) shows the report as memorized: Ctrl+M there offers Save
/// changes.
#[test]
fn open_shows_the_report_as_memorized() {
    let (_d, link) = demo();
    let mut s = list(&link);
    select(&mut s, "Monthly P&L for the bank");
    let _ = link.drain();
    assert!(s.command(CommandId::new("reports.memorized_open")));
    let payload = link
        .drain()
        .into_iter()
        .find_map(|e| match e {
            CompanyEvent::OpenWith(kind, p) if kind.as_str() == super::super::KIND => Some(p),
            _ => None,
        })
        .expect("opened");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let spec = cl_model::reports::find("profit_loss").expect("p&l");
    let mut w = super::super::ReportWindow::new(
        Arc::clone(&link),
        CommandId::new(super::super::KIND),
        None,
        spec,
        today,
    );
    assert!(w.open_with(*payload));
    assert_eq!(
        w.memo.current.as_ref().map(|c| c.name.as_str()),
        Some("Monthly P&L for the bank")
    );
    assert!(!super::super::memorized::changed(&w));
}

/// Edit in Report Builder opens the editor on a builder row as memorized.
#[test]
fn edit_in_report_builder_opens_the_row() {
    let (_d, link) = demo();
    add_builder(&link, "Builder P&L");
    let mut s = list(&link);
    select(&mut s, "Builder P&L");
    let _ = link.drain();
    s.edit_selected();
    let opened = link.drain().into_iter().any(|e| {
        matches!(e, CompanyEvent::OpenWith(kind, _) if kind.as_str() == super::super::builder::KIND)
    });
    assert!(opened);
}

#[test]
fn the_window_is_made_from_its_command() {
    let (_d, link) = demo();
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let w = super::super::make(CommandId::new(KIND), &link, None, today).expect("made");
    assert_eq!(w.title(), "Memorized Reports");
    assert_eq!(w.state()["screen"], "memorized_reports");
}
