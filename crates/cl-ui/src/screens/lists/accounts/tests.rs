//! Chart of Accounts screen tests (headless, demo link).

use std::sync::Arc;

use cl_api::{ApiError, ConflictReason, UserText};
use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::forms::SaveIntent;
use cl_model::lists::accounts::{AccountKind, DraftProblem};
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::{
    ChartScreen, F_CARD, F_DETAIL, F_NAME, F_NUMBER, F_PARENT, F_SENSITIVE, FormPanel, Panel,
};
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

#[test]
fn credit_normal_accounts_show_their_stored_balance_flipped() {
    let r = Rig::new(true);
    let chart = r.s.model.chart();
    // The engine stores debits minus credits for every type (the demo
    // does too); the chart shows each account's normal balance positive.
    for (number, flipped) in [(1010, false), (2050, true), (4000, true), (6100, false)] {
        let stored =
            r.s.accounts
                .iter()
                .find(|a| a.id == account_id(number))
                .and_then(|a| a.balance)
                .expect("stored");
        let shown = chart.entry(account_id(number)).and_then(|e| e.balance);
        let want = if flipped {
            stored.checked_neg().expect("neg")
        } else {
            stored
        };
        assert_eq!(shown, Some(want), "{number}");
        assert!(!want.is_negative(), "{number} reads positive");
    }
}

/// The demo chart shows Payments Clearing's balance as the demo ledger
/// keeps it: a grouped deposit (P14) takes it to 0.00.
#[test]
fn demo_chart_shows_the_payments_clearing_balance_from_the_ledger() {
    use crate::company::demo::deposits;
    fn clearing_balance(s: &ChartScreen) -> Option<cl_core::Money> {
        s.accounts
            .iter()
            .find(|a| a.system_tag.as_deref() == Some("payments_clearing"))
            .and_then(|a| a.balance)
    }
    let mut r = Rig::new(true);
    let m = |s: &str| cl_core::Money::parse_wire(s).expect("m");
    assert_eq!(clearing_balance(&r.s), Some(m("2092.50")));
    let date = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    {
        let mut state = r.link.demo_state().expect("demo");
        let waiting = deposits::pending(&mut state);
        let clearing = deposits::clearing_account(&mut state).expect("10150");
        let mut d = cl_api::DepositDraft::plain(account_id(1010), date, None, Vec::new());
        d.payments = waiting
            .iter()
            .map(|p| cl_api::GroupedPayment {
                source: p.source,
                kind: cl_api::DepositSource::Payment,
                account: clearing,
                amount: p.amount,
                contact: p.contact,
                memo: None,
            })
            .collect();
        deposits::create(&mut state, &d, None).expect("deposit");
    }
    let id = TenantId::parse(TENANT).expect("id");
    r.s = ChartScreen::new(Arc::clone(&r.link), Some((id, "Ozark".into())), date);
    r.frames(3);
    assert_eq!(clearing_balance(&r.s), Some(cl_core::Money::ZERO));
}

/// A new credit card account (2050 is selected, so the form is a
/// Liability) with `digits` in "Card ends in".
fn new_card(r: &mut Rig, name: &str, digits: &str) {
    use cl_model::lists::accounts::CardEnding;
    r.select(2050);
    r.s.command(cl_model::CommandId::new("list.new"));
    r.frames(1);
    assert_eq!(r.form().form.current().kind, AccountKind::Liability);
    r.form()
        .form
        .edit(F_DETAIL, |d| d.detail = "credit_card".into());
    r.form().form.edit(F_NAME, |d| d.name = name.into());
    let digits = digits.to_string();
    r.form()
        .form
        .edit(F_CARD, |d| d.card_last4 = CardEnding(digits));
    r.frames(1);
}

fn card_of(r: &Rig, id: cl_core::ids::AccountId) -> Option<String> {
    r.link
        .demo_state()
        .expect("demo")
        .chart
        .as_ref()
        .and_then(|c| c.iter().find(|a| a.id == id))
        .and_then(|a| a.card_last4)
        .map(|c| c.as_str().to_string())
}

/// A03: "Card ends in" shows on a credit card account only while the engine
/// stores the digits (the demo's stand-in for `document-drafts-v1`).
#[test]
fn card_ends_in_shows_only_on_credit_cards_with_the_capability() {
    let mut r = Rig::new(true);
    new_card(&mut r, "Fuel Card", "");
    assert_eq!(r.s.state()["card_digits_on"], false);
    assert_eq!(r.s.state()["card_field"], false, "the demo engine lacks it");
    assert!(r.s.command(cl_model::CommandId::new("dev.demo_card_digits")));
    r.frames(1);
    assert_eq!(r.s.state()["card_digits_on"], true);
    assert_eq!(r.s.state()["card_field"], true);
    // Not on another liability.
    r.form()
        .form
        .edit(F_DETAIL, |d| d.detail = "long_term_liability".into());
    r.frames(1);
    assert_eq!(r.s.state()["card_field"], false);
    // Off again.
    assert!(r.s.command(cl_model::CommandId::new("dev.demo_card_digits")));
    assert_eq!(r.s.state()["card_digits_on"], false);
}

/// A03 UI review 6: the "exactly 4 digits" warning goes once the typed
/// value is right (or empty), and stays while it is still wrong.
#[test]
fn the_card_warning_clears_once_the_value_is_valid() {
    use DraftProblem as P;
    use cl_model::lists::accounts::CardEnding;
    let typed = |text: &str| {
        let mut problems = vec![P::CardDigits, P::NameMissing];
        super::card_typed(&mut problems, &CardEnding(text.into()));
        problems
    };
    assert_eq!(typed("4821"), vec![P::NameMissing]);
    assert_eq!(typed(""), vec![P::NameMissing]);
    assert_eq!(typed("48a"), vec![P::CardDigits, P::NameMissing]);
    assert_eq!(typed("482"), vec![P::CardDigits, P::NameMissing]);
}

/// A03: the digits save with the account, are refused unless four digits,
/// clear when emptied or when the account stops being a credit card, and
/// never reach the control-channel state.
#[test]
fn card_ends_in_saves_refuses_and_clears() {
    let mut r = Rig::new(true);
    r.s.command(cl_model::CommandId::new("dev.demo_card_digits"));
    new_card(&mut r, "Fuel Card", "48a1");
    assert!(!r.s.save_form(SaveIntent::SaveAndClose));
    assert_eq!(r.s.state()["panel"], "new");
    assert_eq!(r.s.state()["problem"], "card_digits");
    assert_eq!(
        r.form().focus,
        Some(cl_model::lists::accounts::DraftField::Card)
    );
    r.form()
        .form
        .edit(F_CARD, |d| d.card_last4.0 = "4821".into());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    let st = r.s.state();
    assert_eq!(st["panel"], "none");
    assert_eq!(st["selected_card_set"], true);
    assert!(st["warning"].is_null(), "{st}");
    assert!(!st.to_string().contains("4821"), "{st}");
    let id = r.s.selected().expect("selected");
    assert_eq!(card_of(&r, id).as_deref(), Some("4821"));

    // Edit: the digits load into the form; emptied, they clear.
    r.s.open_edit(Some(id));
    r.frames(1);
    assert_eq!(r.form().form.current().card_last4.as_str(), "4821");
    r.form()
        .form
        .edit(F_CARD, |d| d.card_last4.0 = "1234".into());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(card_of(&r, id).as_deref(), Some("1234"));
    r.s.open_edit(Some(id));
    r.frames(1);
    r.form().form.edit(F_CARD, |d| d.card_last4.0.clear());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(card_of(&r, id), None);
    assert_eq!(r.s.state()["selected_card_set"], false);

    // No longer a credit card: the engine (and the demo) drop the digits.
    r.s.open_edit(Some(id));
    r.frames(1);
    r.form()
        .form
        .edit(F_CARD, |d| d.card_last4.0 = "5555".into());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(card_of(&r, id).as_deref(), Some("5555"));
    r.s.open_edit(Some(id));
    r.frames(1);
    r.form()
        .form
        .edit(F_DETAIL, |d| d.detail = "other_current_liability".into());
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(card_of(&r, id), None);
}

/// A03: without the capability nothing about the digits is sent or kept,
/// even if a form somehow carries them.
#[test]
fn without_the_capability_card_digits_are_not_saved() {
    let mut r = Rig::new(true);
    new_card(&mut r, "Fuel Card", "4821");
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    let id = r.s.selected().expect("selected");
    assert_eq!(card_of(&r, id), None);
    assert_eq!(r.s.state()["selected_card_set"], false);
}
