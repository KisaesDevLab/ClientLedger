//! Client and Vendor center tests (headless, demo link).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::{ContactId, TenantId};
use cl_model::CommandId;
use cl_model::forms::SaveIntent;
use cl_model::lists::contacts::{ContactProblem, Role, Roles, TaxIdKind, Terms, TermsChoice};
use eframe::egui::{self, Pos2, RawInput, Rect, vec2};

use super::form::FormPanel;
use super::{CenterScreen, Panel, Tab};
use crate::company::CompanyLink;
use crate::company::demo::contacts::{BOTH, INACTIVE};
use crate::shell::Shell;
use crate::windows::Screen;

const TENANT: &str = "d0000000-0000-4000-8000-000000000001";

struct Rig {
    _dir: tempfile::TempDir,
    ctx: egui::Context,
    link: Arc<CompanyLink>,
    s: CenterScreen,
}

impl Rig {
    fn new(role: Role) -> Rig {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let ctx = egui::Context::default();
        Shell::install(&ctx);
        cl_grid::sequencer::install(&ctx);
        let s = Rig::screen(&link, role);
        let mut rig = Rig {
            _dir: dir,
            ctx,
            link,
            s,
        };
        rig.frames(3);
        rig
    }

    fn screen(link: &Arc<CompanyLink>, role: Role) -> CenterScreen {
        let id = TenantId::parse(TENANT).expect("id");
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        CenterScreen::new(role, Arc::clone(link), Some((id, "Ozark".into())), today)
    }

    /// The other center over the same demo company.
    fn other(&self, role: Role) -> CenterScreen {
        Rig::screen(&self.link, role)
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
                ..RawInput::default()
            };
            let s = &mut self.s;
            let mut out = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
            });
            out.textures_delta.clear();
        }
    }

    fn id(&self, name: &str) -> ContactId {
        self.s
            .contacts
            .iter()
            .find(|c| c.fields.display_name == name)
            .map_or_else(|| panic!("{name}"), |c| c.id)
    }

    fn select(&mut self, name: &str) {
        let id = self.id(name);
        self.s.place_cursor(Some(id));
        self.frames(2);
        assert_eq!(self.s.selected(), Some(id), "{name}");
    }

    fn form(&mut self) -> &mut FormPanel {
        match &mut self.s.panel {
            Panel::Form(f) => f,
            _ => panic!("no form open"),
        }
    }

    fn cmd(&mut self, id: &'static str) -> bool {
        let done = self.s.command(CommandId::new(id));
        self.frames(1);
        done
    }
}

fn names(s: &CenterScreen) -> Vec<String> {
    (0..cl_grid::RowModel::row_count(&s.model))
        .map(|i| cl_grid::RowModel::row_label(&s.model, i))
        .collect()
}

#[test]
fn a_contact_with_both_roles_is_in_both_centers() {
    let mut r = Rig::new(Role::Client);
    assert_eq!(r.s.state()["screen"], "client_center");
    let mut vendors = r.other(Role::Vendor);
    let mut out = r.ctx.run_ui(RawInput::default(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| vendors.ui(ui));
    });
    out.textures_delta.clear();
    let both = format!("{BOTH}, Customer and vendor");
    assert!(names(&r.s).contains(&both), "client center");
    assert!(names(&vendors).contains(&both), "vendor center");
    assert!(!names(&r.s).iter().any(|n| n.starts_with("Galena Gravel")));
    assert!(
        names(&vendors)
            .iter()
            .any(|n| n.starts_with("Galena Gravel"))
    );
    r.select(BOTH);
    assert_eq!(r.s.state()["selected_both"], true);
}

#[test]
fn inactive_contacts_are_hidden_until_asked_for() {
    let mut r = Rig::new(Role::Client);
    let rows = r.s.state()["rows"].as_u64().expect("rows");
    let total = r.s.state()["total"].as_u64().expect("total");
    assert_eq!(rows + 1, total);
    assert!(!names(&r.s).iter().any(|n| n.starts_with(INACTIVE)));
    r.cmd("list.show_inactive");
    assert_eq!(r.s.state()["rows"].as_u64(), Some(total));
    assert!(names(&r.s).contains(&format!("{INACTIVE}, Inactive")));
}

#[test]
fn open_balances_are_the_report_figures() {
    let mut r = Rig::new(Role::Client);
    assert_eq!(r.s.state()["balances_loaded"], true);
    let demo = r
        .link
        .demo_state()
        .map(|d| crate::company::demo::contacts::customer_balances(&d));
    let summary = demo.expect("demo");
    let riverbend = r.id("Riverbend HOA");
    let style = r.s.parts.frame.money_style;
    let want = summary
        .balance_of(riverbend)
        .and_then(|a| super::model::report_text(a, style));
    assert!(want.is_some(), "Riverbend is in the report");
    assert_eq!(r.s.model.balance_text(riverbend), want);
    // Someone the report leaves out is "not in the report", never a zero.
    let listed: Vec<ContactId> = summary.data.iter().map(|x| x.id).collect();
    let quiet =
        r.s.contacts
            .iter()
            .find(|c| c.is_active && !listed.contains(&c.id))
            .map(|c| c.id);
    if let Some(q) = quiet {
        assert!(matches!(
            r.s.model.balance(q),
            super::model::Balance::NotInReport
        ));
        assert_eq!(r.s.model.balance_text(q), None);
    }
    r.select("Riverbend HOA");
    assert_eq!(r.s.state()["detail_loaded"], true);
    assert_eq!(r.s.state()["history_rows"], 10);
}

#[test]
fn new_client_saves_and_a_used_name_is_refused() {
    let mut r = Rig::new(Role::Client);
    assert!(r.cmd("list.new"));
    assert_eq!(r.s.state()["panel"], "new");
    r.form().form.edit(1, |d| d.name = "riverbend hoa".into());
    assert!(!r.s.save_form(SaveIntent::SaveAndClose));
    assert_eq!(r.form().problems, [ContactProblem::NameTaken]);
    r.form().form.edit(1, |d| {
        d.name = "Table Rock Marina".into();
        d.email = "dock@trm.example".into();
        d.terms = TermsChoice::Fixed(Terms::Net15);
    });
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    assert_eq!(r.s.state()["panel"], "none");
    assert_eq!(r.s.state()["note"], "saved");
    let id = r.id("Table Rock Marina");
    assert_eq!(r.s.selected(), Some(id));
    let saved = r.s.contacts.iter().find(|c| c.id == id).expect("saved");
    assert_eq!(
        saved.fields.default_payment_terms.as_deref(),
        Some("net_15")
    );
    assert_eq!(saved.fields.contact_type, cl_api::ContactType::Customer);
    assert!(saved.fields.tax_id.is_none());
}

#[test]
fn edit_reverts_with_esc_then_closes() {
    let mut r = Rig::new(Role::Client);
    r.select("Riverbend HOA");
    r.cmd("edit.edit_transaction");
    assert_eq!(r.s.state()["panel"], "edit");
    r.form().form.edit(1, |d| d.name.push_str(" East"));
    assert_eq!(r.s.state()["dirty"], true);
    r.cmd("form.revert");
    assert_eq!(r.s.state()["dirty"], false);
    assert_eq!(r.s.state()["panel"], "edit");
    r.cmd("form.revert");
    assert_eq!(r.s.state()["panel"], "none");
    // Esc with nothing open returns false (the shell closes the window).
    assert!(!r.s.command(CommandId::new("form.revert")));
}

#[test]
fn make_inactive_and_active_again() {
    let mut r = Rig::new(Role::Vendor);
    r.select("Galena Gravel");
    r.cmd("list.toggle_active");
    r.frames(1);
    assert_eq!(r.s.state()["note"], "made_inactive");
    assert!(!names(&r.s).iter().any(|n| n.starts_with("Galena Gravel")));
    r.cmd("list.show_inactive");
    assert_eq!(r.s.selected(), Some(r.id("Galena Gravel")));
    assert_eq!(r.s.state()["selected_active"], false);
    r.cmd("list.toggle_active");
    r.frames(1);
    assert_eq!(r.s.state()["note"], "made_active");
    assert_eq!(r.s.state()["selected_active"], true);
}

#[test]
fn vendor_fields_tax_id_masked_and_1099_profile() {
    let mut r = Rig::new(Role::Vendor);
    r.select("Crane Creek Tree Service");
    assert_eq!(r.s.state()["profile_loaded"], true);
    assert_eq!(r.s.state()["w9_on_file"], true);
    r.cmd("edit.edit_transaction");
    // The saved tax ID never comes back into the field.
    assert_eq!(r.form().form.current().tax_id, "");
    assert_eq!(r.form().form.current().tax_id_kind, Some(TaxIdKind::Ein));
    r.form().form.edit(36, |d| d.backup_withholding = true);
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    let id = r.id("Crane Creek Tree Service");
    let kept = r.s.contacts.iter().find(|c| c.id == id).expect("vendor");
    assert_eq!(kept.fields.tax_id.as_deref(), Some("00-0004521"), "kept");
    let p = r
        .link
        .demo_state()
        .map(|d| crate::company::demo::contacts::profile(&d, id));
    assert!(p.is_some_and(|p| p.backup_withholding));
    let state = r.s.state().to_string();
    assert!(!state.contains("4521"), "no tax ID in the state");

    // Removing the tax ID on file.
    r.cmd("edit.edit_transaction");
    r.form().clear_tax_id_for_test();
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    let gone = r.s.contacts.iter().find(|c| c.id == id).expect("vendor");
    assert_eq!(gone.fields.tax_id, None);
}

/// B1 canary: a typed tax ID never shows in the state, the accessibility
/// tree, or `Debug` output.
#[test]
fn a_typed_tax_id_never_leaks() {
    const CANARY: &str = "98-7654321";
    let mut r = Rig::new(Role::Vendor);
    r.select("Crane Creek Tree Service");
    r.cmd("edit.edit_transaction");
    r.form().focus_tax_id_for_test();
    r.frames(2);
    r.ctx.enable_accesskit();
    let mut texts = Vec::new();
    for events in [
        vec![egui::Event::Text(CANARY.into())],
        Vec::new(),
        Vec::new(),
    ] {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 900.0))),
            events,
            ..RawInput::default()
        };
        let s = &mut r.s;
        let mut out = r.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(tree) = out.platform_output.accesskit_update {
            for (_, node) in tree.nodes {
                texts.extend(node.value().map(str::to_string));
                texts.extend(node.label().map(str::to_string));
            }
        }
    }
    assert_eq!(
        r.form().form.current().tax_id,
        CANARY,
        "typed into the field"
    );
    assert!(!texts.is_empty(), "the tree was read");
    let digits = "7654321";
    assert!(
        !texts
            .join(
                "
"
            )
            .contains(digits),
        "accessibility tree"
    );
    assert!(!r.s.state().to_string().contains(digits), "state");
    let draft = r.form().form.current().clone();
    assert!(!format!("{draft:?}").contains(digits), "draft Debug");
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(2);
    let id = r.id("Crane Creek Tree Service");
    let saved = r.s.contacts.iter().find(|c| c.id == id).expect("vendor");
    assert_eq!(saved.fields.tax_id.as_deref(), Some(CANARY));
    assert!(!format!("{saved:?}").contains(digits), "contact Debug");
}

#[test]
fn a_new_vendor_gets_a_default_expense_account() {
    let mut r = Rig::new(Role::Vendor);
    r.cmd("list.new");
    let fuel = crate::company::demo::account_id(6100);
    let checking = crate::company::demo::account_id(1010);
    r.form().form.edit(1, |d| {
        d.name = "Kirbyville Propane".into();
        d.expense_account = Some(checking);
    });
    assert!(!r.s.save_form(SaveIntent::SaveAndNew));
    assert_eq!(r.form().problems, [ContactProblem::ExpenseAccountInvalid]);
    r.form().form.edit(31, |d| d.expense_account = Some(fuel));
    assert!(r.s.save_form(SaveIntent::SaveAndNew));
    r.frames(1);
    assert_eq!(r.s.state()["panel"], "new", "Save & New opens a blank form");
    let id = r.id("Kirbyville Propane");
    let v = r.s.contacts.iter().find(|c| c.id == id).expect("vendor");
    assert_eq!(v.fields.default_expense_account_id, Some(fuel));
    assert_eq!(v.fields.contact_type, cl_api::ContactType::Vendor);
}

#[test]
fn a_role_change_moves_a_contact_between_centers() {
    let mut r = Rig::new(Role::Client);
    r.select("Table Rock Dental");
    r.cmd("edit.edit_transaction");
    r.form().form.edit(7, |d| d.roles = Roles::Both);
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(r.s.state()["selected_both"], true);
    r.cmd("edit.edit_transaction");
    r.form().form.edit(7, |d| d.roles = Roles::Vendor);
    assert!(r.s.save_form(SaveIntent::SaveAndClose));
    r.frames(1);
    assert_eq!(r.s.state()["note"], "saved_elsewhere");
    assert!(
        !names(&r.s)
            .iter()
            .any(|n| n.starts_with("Table Rock Dental"))
    );
}

#[test]
fn merge_moves_history_and_makes_the_duplicate_inactive() {
    let mut r = Rig::new(Role::Vendor);
    let source = r.id("Galena Gravel");
    r.select("Galena Gravel");
    let moved = r.s.state()["history_total"].as_u64().expect("total");
    r.cmd("list.merge");
    assert_eq!(r.s.state()["panel"], "merge");
    let Panel::Merge(m) = &mut r.s.panel else {
        panic!("merge panel");
    };
    let target_pos = m
        .targets
        .iter()
        .position(|(_, n)| n == "Bear Creek Hauling")
        .expect("target");
    m.choice = target_pos;
    let target = m.targets[target_pos].0;
    r.s.confirm_merge();
    r.frames(3);
    assert_eq!(r.s.state()["panel"], "none");
    assert_eq!(r.s.state()["note"], "done");
    assert_eq!(r.s.selected(), Some(target));
    let before_target = r
        .link
        .demo_state()
        .map(|d| crate::company::demo::contacts::history(&d, target, 0).1)
        .expect("demo");
    assert!(before_target >= moved);
    assert_eq!(r.s.state()["history_total"].as_u64(), Some(before_target));
    let gone =
        r.s.contacts
            .iter()
            .find(|c| c.id == source)
            .expect("source");
    assert!(!gone.is_active);
}

fn closed(required: bool) -> cl_api::ApiError {
    cl_api::ApiError::ClosedPeriod(cl_api::ClosedPeriod {
        lock_date: CalendarDate::from_ymd(2026, 3, 31).ok(),
        can_override: true,
        password_required: Some(required),
        message: cl_api::UserText::default(),
    })
}

/// U-47: with `merge-closing-date-v1` the confirmation drops the
/// closed-period warning, a 423 opens the closing-date password dialog
/// (Esc closes only the dialog), and the password sends the same merge.
#[test]
fn closed_period_merge_asks_for_the_closing_password() {
    let mut r = Rig::new(Role::Vendor);
    r.s.merge_closing = true;
    let source = r.id("Galena Gravel");
    r.select("Galena Gravel");
    r.cmd("list.merge");
    assert_eq!(r.s.state()["panel"], "merge");
    assert_eq!(r.s.state()["merge_closing"]["closing"], "idle");
    r.s.merge_failed(closed(true));
    r.frames(2);
    let st = r.s.state();
    assert_eq!(st["merge_closing"]["closing"], "asking");
    assert_eq!(st["merge_closing"]["closing_verb"], "merge");
    assert_eq!(st["merge_closing"]["closing_supported"], true);
    assert_eq!(st["error"], false, "the dialog takes the refusal");
    // Esc cancels the dialog, not the merge panel.
    assert!(r.cmd("form.revert"));
    assert_eq!(r.s.state()["panel"], "merge");
    assert_eq!(r.s.state()["merge_closing"]["closing"], "cancelled");
    // Again, now with the password: the demo merges.
    r.s.merge_failed(closed(true));
    let Panel::Merge(m) = &mut r.s.panel else {
        panic!("merge panel");
    };
    m.closing.password = crate::screens::company::form::SecretText::from("pw");
    r.s.submit_merge_password();
    r.frames(3);
    assert_eq!(r.s.state()["panel"], "none");
    assert_eq!(r.s.state()["note"], "done");
    assert!(r.s.contacts.iter().any(|c| c.id == source && !c.is_active));
}

/// Without the capability the engine's refusal is only explained (the
/// password can't be sent) and the confirmation keeps its warning.
#[test]
fn closed_period_merge_without_the_capability_explains() {
    let mut r = Rig::new(Role::Vendor);
    r.select("Galena Gravel");
    r.cmd("list.merge");
    r.s.merge_failed(closed(true));
    r.frames(2);
    assert_eq!(r.s.state()["merge_closing"]["closing"], "unavailable");
    assert_eq!(r.s.state()["merge_closing"]["closing_supported"], false);
}

/// B2: merging a 1099 vendor says so, and a client-and-vendor contact is
/// offered only client-and-vendor targets.
#[test]
fn merge_mentions_1099_and_keeps_both_roles() {
    let mut r = Rig::new(Role::Vendor);
    r.select("Crane Creek Tree Service");
    r.cmd("list.merge");
    assert_eq!(r.s.state()["panel"], "merge");
    assert_eq!(r.s.state()["merge_source_1099"], true);
    r.cmd("edit.cancel");
    r.s.panel = Panel::None;
    r.select(BOTH);
    r.cmd("list.merge");
    let roles: Vec<Roles> = if let Panel::Merge(m) = &r.s.panel {
        m.targets
            .iter()
            .filter_map(|(id, _)| r.s.model.list().entry(*id).map(|e| e.roles))
            .collect()
    } else {
        assert_eq!(r.s.state()["warning"], "nothing");
        Vec::new()
    };
    assert!(roles.iter().all(|x| *x == Roles::Both), "{roles:?}");
}

#[test]
fn quick_report_loads_every_transaction_and_tabs_say_what_waits() {
    let mut r = Rig::new(Role::Client);
    r.select("Riverbend HOA");
    r.cmd("list.quick_report");
    assert_eq!(r.s.state()["tab"], "transactions");
    assert_eq!(r.s.state()["history_all"], true);
    let total = r.s.state()["history_total"].as_u64().expect("total");
    assert_eq!(r.s.state()["history_rows"].as_u64(), Some(total));
    r.s.tab = Tab::Jobs;
    r.frames(1);
    assert_eq!(r.s.state()["jobs_available"], false);
    r.cmd("form.revert");
    assert_eq!(r.s.state()["tab"], "details");
    assert_eq!(Tab::of(Role::Vendor).len(), 3, "no jobs tab for vendors");
}

#[test]
fn every_key_the_centers_use_has_english_text() {
    use cl_model::lists::contacts::Terms as T;
    for r in [Roles::Client, Roles::Vendor, Roles::Both] {
        assert!(crate::i18n::has(r.label().as_str()));
    }
    for x in T::ALL {
        assert!(crate::i18n::has(x.label().as_str()));
    }
    let sources = [
        include_str!("../center.rs"),
        include_str!("form.rs"),
        include_str!("model.rs"),
    ];
    for src in sources {
        for part in src.split('"').skip(1).step_by(2) {
            let is_key = (part.starts_with("center.") || part.starts_with("orphan.contact"))
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_' || c.is_ascii_digit())
                && part.contains('.')
                && !part.ends_with('.');
            if is_key && part != "center.rs" {
                assert!(crate::i18n::has(part), "{part} has no text");
            }
        }
    }
}

/// S5: report figures are shown as text, laid out like money, without
/// becoming `Money`.
#[test]
fn report_figures_are_shown_as_text() {
    use super::model::{report_is_zero, report_text};
    use cl_core::{MoneyStyle, NegativeStyle};
    let a = |json: &str| -> cl_api::ReportAmount { serde_json::from_str(json).expect("amount") };
    let plain = MoneyStyle::default();
    let dollars = MoneyStyle {
        negative: NegativeStyle::Parens,
        symbol: true,
    };
    let minus = MoneyStyle {
        negative: NegativeStyle::Minus,
        symbol: false,
    };
    assert_eq!(
        report_text(&a("1234567.891"), minus).as_deref(),
        Some("1,234,567.89")
    );
    assert_eq!(
        report_text(&a("\"-1234.5\""), dollars).as_deref(),
        Some("($1,234.50)")
    );
    assert_eq!(report_text(&a("-0.004"), minus).as_deref(), Some("0.00"));
    assert_eq!(report_text(&a("999"), minus).as_deref(), Some("999.00"));
    assert_eq!(
        report_text(&a("0.30000000000000004"), minus).as_deref(),
        Some("0.30")
    );
    assert!(report_text(&a("\"1e\""), plain).is_none(), "not a number");
    assert!(report_is_zero(&a("-0.001")));
    assert!(!report_is_zero(&a("0.01")));
}

/// S6: with 20,000 contacts, filtering and rebuilding the grid model stays
/// quick (fastest of five runs, so one slow run of a busy machine does not
/// fail it).
#[test]
fn twenty_thousand_contacts_filter_and_rebuild_quickly() {
    let contacts: Vec<cl_api::Contact> = (0..20_000u32)
        .map(|n| {
            let kind = ["customer", "vendor", "both"][(n % 3) as usize];
            serde_json::from_value(serde_json::json!({
                "id": format!("c0000000-0000-4000-8000-{n:012}"),
                "contactType": kind,
                "displayName": format!("Contact {n:05}"),
                "email": format!("c{n}@example.test"),
                "isActive": n % 7 != 0,
            }))
            .expect("contact")
        })
        .collect();
    let mut model = super::model::ListModel::new(Role::Client);
    let all = cl_model::lists::contacts::ContactFilter::default();
    let search = cl_model::lists::contacts::ContactFilter {
        show_inactive: false,
        search: "contact 19".into(),
    };
    let ((), load) = fastest_of(5, || model.set_contacts(&contacts, &all));
    assert!(load.as_millis() < 250, "loading took {load:?}");
    let (shown, took) = fastest_of(5, || {
        model.apply_filter(&search);
        let n = model.shown();
        model.apply_filter(&all);
        n
    });
    assert!(shown > 0 && model.shown() > shown);
    assert!(took.as_millis() < 100, "filter and rebuild took {took:?}");
}

/// The fastest of `runs` timings of `f`, with its last answer.
fn fastest_of<T>(runs: usize, mut f: impl FnMut() -> T) -> (T, std::time::Duration) {
    let mut best = std::time::Duration::MAX;
    let mut out = None;
    for _ in 0..runs.max(1) {
        let start = std::time::Instant::now();
        out = Some(f());
        best = best.min(start.elapsed());
    }
    (out.expect("ran once"), best)
}

#[test]
fn toggle_details_hides_the_details_and_a_new_window_remembers() {
    let mut r = Rig::new(Role::Vendor);
    let handle = crate::grid_env::WidthsHandle::default();
    crate::grid_env::publish_widths(&r.ctx, &handle);
    r.frames(1);
    assert_eq!(r.s.state()["details_collapsed"], false);
    assert!(r.s.command(CommandId::new("window.toggle_details")));
    r.frames(1);
    assert_eq!(r.s.state()["details_collapsed"], true);
    assert_eq!(
        handle.lock().expect("lock").panels.get("vendors.center"),
        Some(true)
    );
    // A form shows the panel; the command leaves the choice alone.
    assert!(r.s.command(CommandId::new("list.new")));
    r.frames(1);
    assert_eq!(r.s.state()["details_collapsed"], false);
    assert!(r.s.command(CommandId::new("window.toggle_details")));
    assert_eq!(r.s.state()["details_remembered"], true);
    // A new Vendors window reads the choice; Customers & Jobs has its own.
    r.s = r.other(Role::Vendor);
    r.frames(2);
    assert_eq!(r.s.state()["details_collapsed"], true);
    r.s = r.other(Role::Client);
    r.frames(2);
    assert_eq!(r.s.state()["details_collapsed"], false);
}
