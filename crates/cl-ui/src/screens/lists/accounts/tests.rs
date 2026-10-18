//! Chart of Accounts screen tests (headless, demo link).

use std::sync::Arc;

use cl_api::{ApiError, ConflictReason, UserText};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::forms::SaveIntent;
use cl_model::lists::accounts::{AccountKind, DraftProblem};
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::{ChartScreen, F_NAME, F_NUMBER, F_PARENT, F_SENSITIVE, FormPanel, Panel};
use crate::company::CompanyLink;
use crate::company::demo::account_id;
use crate::shell::Shell;
use crate::windows::Screen;

const TENANT: &str = "d0000000-0000-4000-8000-000000000001";

struct Rig {
    _dir: tempfile::TempDir,
    ctx: egui::Context,
    link: Arc<CompanyLink>,
    s: ChartScreen,
}

impl Rig {
    fn new(numbers: bool) -> Rig {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let id = TenantId::parse(TENANT).expect("id");
        link.store()
            .update(|l| {
                l.upsert(id, "Ozark");
                if let Some(rec) = l.get_mut(id) {
                    rec.prefs.account_numbers = numbers;
                }
            })
            .expect("store");
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let s = ChartScreen::new(Arc::clone(&link), Some((id, "Ozark".into())), today);
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

    fn select(&mut self, number: u32) {
        self.s.want_cursor = None;
        self.s.place_cursor(Some(account_id(number)));
        self.frames(2);
        assert_eq!(self.s.selected(), Some(account_id(number)), "{number}");
    }

    fn form(&mut self) -> &mut FormPanel {
        match &mut self.s.panel {
            Panel::Form(f) => f,
            _ => panic!("no form open"),
        }
    }
}

#[test]
fn the_chart_loads_in_hierarchy_order_with_inactive_hidden() {
    let mut r = Rig::new(true);
    let st = r.s.state();
    assert_eq!(st["loaded"], true);
    assert_eq!(st["numbers"], true);
    let total = st["total"].as_u64().expect("total");
    // Answering Service is inactive and hidden until asked for.
    assert_eq!(st["rows"].as_u64(), Some(total - 1));
    assert_eq!(st["merge_available"], false);
    // Plates and Permits sits two levels down (Vehicle Expenses >
    // Trailer Registration > Plates and Permits).
    r.select(6721);
    assert_eq!(r.s.state()["selected_depth"], 2);
    assert_eq!(
        r.s.model.chart().full_name(account_id(6721)),
        Some("Vehicle Expenses:Trailer Registration:Plates and Permits")
    );
    r.s.command(cl_model::CommandId::new("list.show_inactive"));
    r.frames(1);
    assert_eq!(r.s.state()["show_inactive"], true);
    assert_eq!(r.s.state()["rows"].as_u64(), Some(total));
}

#[test]
fn balances_are_the_engine_figures_never_a_rollup() {
    let r = Rig::new(true);
    let chart = r.s.model.chart();
    let parent = chart.entry(account_id(6700)).expect("parent");
    let account =
        r.s.accounts
            .iter()
            .find(|a| a.id == account_id(6700))
            .expect("account");
    // Exactly what the engine (here: the demo) reported for the account
    // itself; the sub-accounts' zero balances are not added in.
    assert_eq!(parent.balance, account.balance);
    let cell = cl_grid::RowModel::cell(
        &r.s.model,
        cl_grid::RowModel::index_of(&r.s.model, r.s.model.key_of(account_id(6700)).expect("key"))
            .expect("index"),
        super::model::COL_BALANCE,
    );
    match (cell, account.balance) {
        (cl_grid::CellValue::Money(m), Some(b)) => assert_eq!(m, b),
        (cl_grid::CellValue::Empty, None) => {}
        other => panic!("unexpected balance cell {other:?}"),
    }
}

#[test]
fn the_number_preference_hides_the_number_column_and_field() {
    let mut r = Rig::new(false);
    assert_eq!(r.s.state()["numbers"], false);
    assert!(
        r.s.grid
            .spec()
            .columns
            .iter()
            .all(|c| c.id != super::model::COL_NUMBER)
    );
    let on = Rig::new(true);
    assert!(
        on.s.grid
            .spec()
            .columns
            .iter()
            .any(|c| c.id == super::model::COL_NUMBER)
    );
    r.s.command(cl_model::CommandId::new("list.new"));
    r.frames(2);
    assert_eq!(r.s.state()["panel"], "new");
}

#[test]
fn a_new_sub_account_is_saved_and_selected() {
    let mut r = Rig::new(true);
    r.select(6700);
    let before = r.s.model.chart().len();
    r.s.command(cl_model::CommandId::new("list.new"));
    r.frames(1);
    assert_eq!(r.s.state()["panel"], "new");
    // The new account takes the selected account's type.
    assert_eq!(r.form().form.current().kind, AccountKind::Expense);
    r.form().form.edit(F_NAME, |d| d.name = "Tolls".into());
    r.form().form.edit(F_NUMBER, |d| d.number = "6730".into());
    r.form()
        .form
        .edit(F_PARENT, |d| d.parent = Some(account_id(6700)));
    r.form().form.edit(F_SENSITIVE, |d| d.sensitive = true);
    assert!(r.s.dirty());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    let st = r.s.state();
    assert_eq!(st["panel"], "none");
    assert_eq!(st["note"], "saved");
    assert_eq!(r.s.model.chart().len(), before + 1);
    assert_eq!(st["selected_depth"], 1);
    assert_eq!(st["selected_sensitive"], true);
    // The sensitive mark is kept on this computer (demo state here).
    let id = r.s.selected().expect("selected");
    assert!(r.link.demo_state().expect("demo").sensitive.contains(&id));
}

#[test]
fn a_used_number_is_refused_before_and_by_the_engine() {
    let mut r = Rig::new(true);
    r.s.open_new();
    r.form()
        .form
        .edit(F_NAME, |d| d.name = "Second checking".into());
    r.form().form.edit(F_NUMBER, |d| d.number = "1010".into());
    assert!(!r.s.save_form(SaveIntent::SaveAndClose));
    assert_eq!(r.form().problems, vec![DraftProblem::NumberTaken]);
    assert_eq!(r.s.state()["problems"][0], "number_taken");

    // The engine's 409 ACCOUNT_NUMBER_EXISTS (another window took it) reads
    // the same, on the number field.
    r.form().form.edit(F_NUMBER, |d| d.number = "1011".into());
    let p = r.form();
    p.problems.clear();
    let _ = p.form.begin_save(SaveIntent::SaveAndClose);
    p.save_failed(ApiError::Conflict {
        reason: ConflictReason::AlreadyExists,
        current_version: None,
        code: cl_api::ErrorCode::parse("ACCOUNT_NUMBER_EXISTS"),
        message: UserText::default(),
    });
    assert_eq!(p.problems, vec![DraftProblem::NumberTaken]);
    assert!(p.error.is_none());
    assert_eq!(p.focus, Some(cl_model::lists::accounts::DraftField::Number));

    // Any other conflict is shown as the engine's error.
    let _ = p.form.begin_save(SaveIntent::SaveAndClose);
    p.problems.clear();
    p.save_failed(ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: None,
        message: UserText::default(),
    });
    assert_eq!(p.problems, Vec::new());
    assert!(p.error.is_some());
}

#[test]
fn edit_changes_the_name_and_esc_reverts_then_closes() {
    let mut r = Rig::new(true);
    r.select(6100);
    r.s.command(cl_model::CommandId::new("edit.edit_transaction"));
    r.frames(1);
    assert_eq!(r.s.state()["panel"], "edit");
    r.form()
        .form
        .edit(F_NAME, |d| d.name = "Fuel and Oil".into());
    assert!(r.s.dirty());
    // Esc: revert first, then close the panel, then nothing for the window.
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    assert!(!r.s.dirty());
    assert_eq!(r.s.state()["panel"], "edit");
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    assert_eq!(r.s.state()["panel"], "none");
    assert!(!r.s.command(cl_model::CommandId::new("form.revert")));

    r.s.open_edit(Some(account_id(6100)));
    r.form()
        .form
        .edit(F_NAME, |d| d.name = "Fuel and Oil".into());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(
        r.s.model
            .chart()
            .entry(account_id(6100))
            .map(|e| e.name.as_str()),
        Some("Fuel and Oil")
    );
}

#[test]
fn deactivate_and_reactivate_follow_the_engine_rules() {
    let mut r = Rig::new(true);
    // A system account the books need.
    r.select(1200);
    r.s.command(cl_model::CommandId::new("list.toggle_active"));
    assert_eq!(r.s.state()["warning"], "cant_deactivate_system");

    // An account with a balance (the engine's figure, read not computed).
    r.select(1010);
    assert!(
        r.s.accounts
            .iter()
            .find(|a| a.id == account_id(1010))
            .and_then(|a| a.balance)
            .is_some_and(|b| !b.is_zero())
    );
    r.s.toggle_active();
    assert_eq!(r.s.state()["warning"], "cant_deactivate_balance");

    // A zero-balance account goes inactive, and comes back.
    r.select(6721);
    r.s.toggle_active();
    r.frames(2);
    assert_eq!(r.s.state()["note"], "made_inactive");
    assert!(r.s.model.key_of(account_id(6721)).is_none());
    r.s.set_show_inactive(true);
    r.select(6721);
    assert_eq!(r.s.state()["selected_active"], false);
    r.s.toggle_active();
    r.frames(2);
    assert_eq!(r.s.state()["note"], "made_active");
    assert_eq!(r.s.state()["selected_active"], true);
}

#[test]
fn quick_report_lists_lines_and_shows_the_engine_balance() {
    let mut r = Rig::new(true);
    r.select(1010);
    r.s.command(cl_model::CommandId::new("list.quick_report"));
    r.frames(2);
    let st = r.s.state();
    assert_eq!(st["panel"], "report");
    assert_eq!(st["report_loaded"], true);
    let year = st["report_lines"].as_u64().expect("lines");
    assert!(year > 0);
    if let Panel::Report(p) = &mut r.s.panel {
        p.all_dates = true;
    }
    r.s.run_report();
    r.frames(2);
    assert!(r.s.state()["report_lines"].as_u64().expect("lines") >= year);
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    assert_eq!(r.s.state()["panel"], "none");
}

#[test]
fn the_state_carries_no_names_or_amounts() {
    let mut r = Rig::new(true);
    r.select(1010);
    r.s.open_report();
    r.frames(2);
    let text = r.s.state().to_string();
    for word in ["Checking", "Ozark", "1010", "$", "."] {
        assert!(!text.contains(word), "{word} in {text}");
    }
}

#[test]
fn find_and_typing_move_to_matching_accounts() {
    let mut r = Rig::new(true);
    let key = r.s.model.find_prefix("veh").expect("vehicles");
    assert!(r.s.model.account_at(key).is_some());
    assert!(r.s.model.find_prefix("67").is_some());
    r.s.filter.search = "fuel".into();
    r.s.model.apply_filter(&r.s.filter);
    assert!(r.s.model.shown() < r.s.model.chart().len());
    // Esc clears the search before the window closes.
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    assert_eq!(r.s.state()["searching"], false);
}

#[test]
fn make_builds_only_the_chart() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    assert!(
        crate::screens::lists::make(cl_model::CommandId::new("nav.home"), &link, None, today)
            .is_none()
    );
    let s = crate::screens::lists::make(cl_model::CommandId::new(super::KIND), &link, None, today)
        .expect("screen");
    assert_eq!(s.title(), "Chart of Accounts");
}

#[test]
fn the_chart_goes_compact_beside_the_form_and_keeps_the_account() {
    let mut r = Rig::new(true);
    let has_detail = |r: &Rig| {
        r.s.grid
            .spec()
            .columns
            .iter()
            .any(|c| c.id == super::model::COL_DETAIL)
    };
    r.select(6150);
    assert!(has_detail(&r));
    r.s.open_edit(r.s.selected());
    r.frames(2);
    assert!(!has_detail(&r));
    assert_eq!(r.s.selected(), Some(account_id(6150)));
    // The form keeps the keyboard, not the compact chart.
    assert!(!r.s.grid.has_focus());
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    r.frames(2);
    assert!(has_detail(&r));
    assert_eq!(r.s.selected(), Some(account_id(6150)));
    assert_eq!(r.s.state()["focused"], true);
}

#[test]
fn include_inactive_returns_to_the_account_just_hidden() {
    let mut r = Rig::new(true);
    r.select(6721);
    r.s.toggle_active();
    r.frames(2);
    assert_ne!(r.s.selected(), Some(account_id(6721)));
    r.s.set_show_inactive(true);
    r.frames(2);
    assert_eq!(r.s.selected(), Some(account_id(6721)));
}

#[test]
fn esc_revert_puts_the_focus_back_in_the_form() {
    let mut r = Rig::new(true);
    r.s.open_edit(Some(account_id(6100)));
    r.frames(1);
    r.form()
        .form
        .edit(F_NAME, |d| d.name = "Fuel and Oil".into());
    r.form().focus = None;
    assert!(r.s.command(cl_model::CommandId::new("form.revert")));
    assert_eq!(
        r.form().focus,
        Some(cl_model::lists::accounts::DraftField::Name)
    );
}
