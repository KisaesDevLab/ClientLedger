//! Tags screen tests (headless, demo link).

use std::sync::Arc;

use cl_api::{ApiError, ConflictReason, UserText};
use cl_core::ids::TenantId;
use cl_model::CommandId;
use cl_model::forms::SaveIntent;
use cl_model::lists::accounts::AccountKind;
use cl_model::lists::tags::{TagField, TagProblem};
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::{Panel, TagsScreen};
use crate::company::CompanyLink;
use crate::company::demo::lists::{group_id, tag_id};
use crate::shell::Shell;
use crate::windows::Screen;

const TENANT: &str = "d0000000-0000-4000-8000-000000000001";

struct Rig {
    _dir: tempfile::TempDir,
    ctx: egui::Context,
    link: Arc<CompanyLink>,
    s: TagsScreen,
}

impl Rig {
    fn new() -> Rig {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let id = TenantId::parse(TENANT).expect("id");
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let s = TagsScreen::new(Arc::clone(&link), Some((id, "Ozark".into())));
        let mut rig = Rig {
            _dir: dir,
            ctx,
            link,
            s,
        };
        rig.frames(3);
        rig
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 820.0))),
                ..RawInput::default()
            };
            let s = &mut self.s;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
            });
            out.textures_delta.clear();
        }
    }

    fn state(&self, key: &str) -> serde_json::Value {
        self.s.state()[key].clone()
    }

    fn cmd(&mut self, id: &'static str) -> bool {
        let r = self.s.command(CommandId::new(id));
        self.frames(2);
        r
    }

    fn select(&mut self, n: u32) {
        self.s.place_cursor(Some(tag_id(n)));
        self.frames(3);
        assert_eq!(self.s.selected(), Some(tag_id(n)));
    }

    fn tag_form(&mut self) -> &mut super::TagForm {
        match &mut self.s.panel {
            Panel::Tag(f) => f,
            _ => panic!("no tag form"),
        }
    }

    fn group_form(&mut self) -> &mut super::GroupForm {
        match &mut self.s.panel {
            Panel::Group(f) => f,
            _ => panic!("no group form"),
        }
    }
}

#[test]
fn loads_tags_by_group_with_usage_of_the_selected_tag() {
    let mut r = Rig::new();
    assert_eq!(r.state("screen"), "tags");
    assert_eq!(r.state("rows"), 9);
    assert_eq!(r.state("total"), 10);
    assert_eq!(r.state("groups"), 2);
    assert_eq!(r.s.title(), "Tags");
    // Route first (its tags by name), then Crew, then ungrouped.
    let names: Vec<String> =
        r.s.model
            .rows()
            .iter()
            .map(|x| x.cells[1].clone())
            .collect();
    assert_eq!(
        names,
        [
            "Lake Area",
            "North Route",
            "South Route",
            "Crew 1",
            "Crew 2",
            "Crew A",
            "Install",
            "Storm Cleanup",
            "Warranty Work"
        ]
    );
    r.frames(2);
    assert_eq!(r.state("usage_loaded"), true);
    assert_eq!(r.state("usage_lines"), 12);
    assert_eq!(r.state("selected_group"), "Route");
    assert_eq!(r.state("merge_available"), false, "used on lines (U-41)");
}

#[test]
fn create_rename_and_delete_an_unused_tag() {
    let mut r = Rig::new();
    r.select(4);
    assert!(r.cmd("list.new"));
    assert_eq!(r.state("panel"), "new_tag");
    assert_eq!(
        r.tag_form().form.current().group,
        Some(group_id(2)),
        "a new tag starts in the selected tag's group"
    );
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "Crew 3".into());
    assert!(r.s.dirty());
    assert!(r.s.save_panel(SaveIntent::SaveAndClose));
    r.frames(3);
    assert_eq!(r.state("panel"), "none");
    assert_eq!(r.state("note"), "saved");
    assert_eq!(r.state("rows"), 10);
    assert_eq!(r.state("selected_group"), "Crew");
    assert_eq!(r.state("usage_total"), 0);

    assert!(r.cmd("edit.edit_transaction"));
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "Crew Three".into());
    assert!(r.cmd("form.save_close"));
    let names: Vec<String> = r
        .link
        .demo_state()
        .expect("demo")
        .tags_mut()
        .iter()
        .map(|t| t.fields.name.clone())
        .collect();
    assert!(names.contains(&"Crew Three".to_string()));

    assert!(r.cmd("form.delete_item"));
    assert_eq!(r.state("note"), "deleted");
    assert_eq!(r.state("rows"), 9);
}

#[test]
fn a_used_tag_is_not_deleted_and_names_stay_unique() {
    let mut r = Rig::new();
    r.select(7);
    assert_eq!(r.state("usage_total"), 3);
    r.cmd("form.delete_item");
    assert_eq!(r.state("warning"), "in_use");
    r.cmd("list.new");
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "spring promo 2025".into());
    r.tag_form()
        .form
        .edit(super::F_COLOR, |d| d.color = "green".into());
    assert!(!r.s.save_panel(SaveIntent::SaveAndNew));
    assert_eq!(
        r.tag_form().problems,
        vec![TagProblem::NameTaken, TagProblem::ColorInvalid]
    );
    assert_eq!(r.state("problem"), "name_taken");
    // Esc reverts, then closes.
    assert!(r.cmd("form.revert"));
    assert_eq!(r.state("dirty"), false);
    assert!(r.cmd("form.revert"));
    assert_eq!(r.state("panel"), "none");
}

#[test]
fn merge_only_for_header_only_tags() {
    let mut r = Rig::new();
    r.select(3);
    r.cmd("list.merge");
    assert_eq!(r.state("warning"), "merge_lines_later");
    assert_eq!(r.state("panel"), "none");
    r.select(7);
    assert_eq!(r.state("merge_available"), true);
    r.cmd("list.merge");
    assert_eq!(r.state("panel"), "merge");
    // The first target is the first active tag other than the source.
    assert!(r.cmd("form.save_close"));
    r.frames(3);
    assert_eq!(r.state("note"), "merged");
    assert_eq!(r.state("panel"), "none");
    assert_eq!(r.state("total"), 9);
    assert!(
        r.link
            .demo_state()
            .expect("demo")
            .tags_gone
            .contains(&tag_id(7))
    );
}

/// U-47 (Q-P11t): with `merge-closing-date-v1` a tag used on lines can be
/// merged, and a closed-period refusal asks for the closing-date password.
#[test]
fn line_tags_merge_with_the_closing_password() {
    let mut r = Rig::new();
    r.s.caps.merge_lines = true;
    r.s.caps.merge_closing = true;
    r.select(3);
    assert_eq!(r.state("merge_available"), true, "lines move too");
    r.cmd("list.merge");
    assert_eq!(r.state("panel"), "merge");
    r.s.merge_failed(ApiError::ClosedPeriod(cl_api::ClosedPeriod {
        lock_date: cl_core::CalendarDate::from_ymd(2026, 3, 31).ok(),
        can_override: true,
        password_required: Some(true),
        message: UserText::default(),
    }));
    r.frames(2);
    assert_eq!(r.state("merge_closing")["closing"], "asking");
    assert_eq!(r.state("merge_closing")["closing_verb"], "merge");
    assert_eq!(r.state("error"), false);
    // Alt+S waits under the dialog.
    assert!(r.cmd("form.save_close"));
    assert_eq!(r.state("panel"), "merge");
    let Panel::Merge(m) = &mut r.s.panel else {
        panic!("merge panel");
    };
    m.closing.password = crate::screens::company::form::SecretText::from("pw");
    r.s.submit_merge_password();
    r.frames(3);
    assert_eq!(r.state("note"), "merged");
    assert_eq!(r.state("panel"), "none");
    assert!(
        r.link
            .demo_state()
            .expect("demo")
            .tags_gone
            .contains(&tag_id(3))
    );
}

#[test]
fn groups_with_one_per_line_and_required_account_types() {
    let mut r = Rig::new();
    r.s.open_group(None);
    r.frames(2);
    assert_eq!(r.state("panel"), "new_group");
    r.group_form()
        .form
        .edit(super::F_NAME, |d| d.name = "Project Type".into());
    r.group_form()
        .form
        .edit(super::F_SINGLE, |d| d.single = true);
    r.group_form().form.edit(super::F_REQUIRED, |d| {
        d.required_for.insert(AccountKind::Income);
    });
    assert!(r.cmd("form.save_close"));
    assert_eq!(r.state("note"), "group_saved");
    assert_eq!(r.state("groups"), 3);
    assert_eq!(r.state("required_groups"), 1);
    let (id, kinds) = {
        let d = r.link.demo_state().expect("demo");
        let (id, kinds) = d.required_tag_groups.iter().next().expect("kept");
        (*id, kinds.clone())
    };
    assert!(kinds.contains("revenue"));
    let g = r.s.list.group(id).expect("listed");
    assert!(g.single && g.required_for.contains(&AccountKind::Income));

    // A used group name is refused.
    r.s.open_group(None);
    r.group_form()
        .form
        .edit(super::F_NAME, |d| d.name = "project type".into());
    assert!(!r.s.save_panel(SaveIntent::SaveAndClose));
    assert_eq!(r.group_form().problems, vec![TagProblem::GroupNameTaken]);
    r.cmd("form.revert");
    r.cmd("form.revert");

    // Deleting the group keeps its tags (none here) and drops the rule.
    r.s.open_group(Some(id));
    r.s.delete_group();
    r.frames(3);
    assert_eq!(r.state("note"), "group_deleted");
    assert_eq!(r.state("groups"), 2);
    assert_eq!(r.state("required_groups"), 0);

    // Deleting Crew leaves Crew 1 and Crew 2 ungrouped.
    r.s.open_group(Some(group_id(2)));
    r.s.delete_group();
    r.frames(3);
    let crew = r.s.list.tag(tag_id(4)).expect("kept");
    assert_eq!(crew.group, None);
}

#[test]
fn make_inactive_and_active_again() {
    let mut r = Rig::new();
    r.select(6);
    r.cmd("list.toggle_active");
    assert_eq!(r.state("note"), "made_inactive");
    assert_eq!(r.state("rows"), 8);
    r.cmd("list.show_inactive");
    assert_eq!(r.s.selected(), Some(tag_id(6)));
    assert_eq!(r.state("selected_active"), false);
    r.cmd("list.toggle_active");
    assert_eq!(r.state("note"), "made_active");
    r.cmd("list.show_inactive");
    assert_eq!(r.state("rows"), 9);
}

#[test]
fn warnings_and_other_commands() {
    let mut r = Rig::new();
    r.s.open_edit_tag(None);
    assert_eq!(r.state("warning"), "select_first");
    r.cmd("list.new");
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "x".into());
    r.s.open_group(None);
    assert_eq!(r.state("warning"), "finish_first");
    assert!(r.cmd("edit.undo"));
    assert!(r.cmd("edit.redo"));
    assert!(r.cmd("edit.find"));
    assert!(!r.cmd("no.such.command"));
    // An engine conflict on save reads as a used name.
    r.tag_form().save_failed(ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: None,
        message: UserText::default(),
        details: cl_api::ConflictDetails::default(),
    });
    assert_eq!(r.tag_form().problems, vec![TagProblem::NameTaken]);
    r.tag_form().save_failed(ApiError::NotFound { code: None });
    assert!(r.tag_form().error.is_some());
}

/// Q-270: the Number column, number-then-name order, type-ahead by number
/// and the row label `10 · Lake Area`.
#[test]
fn numbers_show_first_and_sort_and_find() {
    let mut r = Rig::new();
    assert_eq!(r.state("numbers"), true);
    let numbers: Vec<String> =
        r.s.model
            .rows()
            .iter()
            .take(4)
            .map(|x| x.cells[0].clone())
            .collect();
    assert_eq!(numbers, ["10", "20", "30", ""]);
    assert_eq!(r.s.model.rows()[0].label, "10 · Lake Area, Route");
    let lake = r.s.find_typed("10").expect("found by number");
    assert_eq!(r.s.model.id_at(lake), Some(tag_id(3)));
    let south = r.s.find_typed("3").expect("number prefix");
    assert_eq!(r.s.model.id_at(south), Some(tag_id(2)));
    let storm = r.s.find_typed("storm").expect("found by name");
    assert_eq!(r.s.model.id_at(storm), Some(tag_id(7)));
    assert_eq!(r.s.find_typed(" "), None);
    r.s.filter.search = "20".into();
    r.s.refilter();
    assert_eq!(r.state("rows"), 1);
    // The compact list (a panel open) keeps the Number column.
    r.cmd("list.new");
    assert_eq!(r.s.model.rows()[0].cells.len(), 4);
}

#[test]
fn a_number_is_saved_cleared_and_a_used_one_refused() {
    let mut r = Rig::new();
    r.select(6);
    r.cmd("list.new");
    r.tag_form()
        .form
        .edit(super::F_NUMBER, |d| d.number = " 20 ".into());
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "Hill Route".into());
    assert!(!r.s.save_panel(SaveIntent::SaveAndClose));
    assert_eq!(r.tag_form().problems, vec![TagProblem::NumberTaken]);
    assert_eq!(r.state("problem"), "number_taken");
    r.tag_form()
        .form
        .edit(super::F_NUMBER, |d| d.number = "15".into());
    assert!(r.s.save_panel(SaveIntent::SaveAndClose));
    r.frames(3);
    assert_eq!(r.state("note"), "saved");
    assert_eq!(r.state("selected_label"), "15 · Hill Route");
    let saved = |r: &Rig, name: &str| {
        r.link
            .demo_state()
            .expect("demo")
            .tags_mut()
            .iter()
            .find(|t| t.fields.name == name)
            .and_then(|t| t.fields.tag_number.clone())
    };
    assert_eq!(saved(&r, "Hill Route").as_deref(), Some("15"));
    // Ungrouped, numbered: first of the ungrouped tags.
    let ids: Vec<_> = r.s.model.rows().iter().map(|x| x.id).collect();
    let at = ids.iter().position(|id| Some(*id) == r.s.selected());
    assert_eq!(at, Some(6), "after the routes and crews");
    // Clearing the number.
    r.cmd("edit.edit_transaction");
    r.tag_form()
        .form
        .edit(super::F_NUMBER, |d| d.number = String::new());
    assert!(r.cmd("form.save_close"));
    assert_eq!(saved(&r, "Hill Route"), None);
    assert_eq!(r.state("selected_label"), "Hill Route");

    // Another window took the number first: the engine's 409 reads the same.
    r.cmd("edit.edit_transaction");
    r.tag_form()
        .form
        .edit(super::F_NUMBER, |d| d.number = "40".into());
    r.tag_form().save_failed(ApiError::Conflict {
        reason: ConflictReason::TagNumberTaken,
        current_version: None,
        code: None,
        message: UserText::default(),
        details: cl_api::ConflictDetails::default(),
    });
    assert_eq!(r.tag_form().problems, vec![TagProblem::NumberTaken]);
    assert_eq!(r.tag_form().focus, Some(TagField::Number));
    assert!(r.tag_form().error.is_none());
    assert_eq!(
        crate::i18n::t_with(TagProblem::NumberTaken.key().as_str(), &[("n", "40")]),
        "Another tag already uses number 40."
    );
}

#[test]
fn without_tag_numbers_the_column_and_field_are_hidden() {
    let mut r = Rig::new();
    r.s.caps.numbers = false;
    (r.s.grid, r.s.model) = super::new_grid(false, false);
    r.s.apply();
    r.frames(2);
    assert_eq!(r.state("numbers"), false);
    assert_eq!(r.s.model.rows()[0].cells.len(), 4);
    assert_eq!(r.s.model.rows()[0].cells[0], "Lake Area");
    r.select(6);
    r.cmd("list.new");
    r.tag_form()
        .form
        .edit(super::F_NAME, |d| d.name = "Plain".into());
    assert!(r.s.save_panel(SaveIntent::SaveAndClose));
    r.frames(2);
    let mut d = r.link.demo_state().expect("demo");
    let plain = d
        .tags_mut()
        .iter()
        .find(|t| t.fields.name == "Plain")
        .cloned()
        .expect("saved");
    assert_eq!(plain.fields.tag_number, None, "no number sent");
}
