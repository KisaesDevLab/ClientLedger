//! Home on the demo engine (both P16 capabilities simulated) and on hand-made
//! answers (neither): lane wiring, chips and the lane figures.

use super::*;
use crate::company::demo::DemoState;
use crate::shell::{Shell, ShellOptions};
use cl_api::Account;
use cl_core::Money;
use cl_model::home::LaneFooter;
use eframe::egui::accesskit::Role;

fn demo_link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn tenant(link: &CompanyLink) -> Option<(TenantId, String)> {
    link.demo_state()
        .and_then(|s| s.companies.first().map(|c| (c.tenant_id, c.name.clone())))
}

fn home(link: &Arc<CompanyLink>) -> HomeScreen {
    HomeScreen::new(Arc::clone(link), tenant(link), DemoState::today())
}

/// Draws Home a few frames at `width`; every accessible (role, name).
fn tree(s: &mut HomeScreen, width: f32) -> Vec<(Role, String)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut all = std::collections::HashMap::new();
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(t) = out.platform_output.accesskit_update {
            all.extend(t.nodes);
        }
    }
    all.into_values()
        .map(|n| {
            let name = n
                .label()
                .or(if n.role() == Role::Label {
                    n.value()
                } else {
                    None
                })
                .unwrap_or_default()
                .to_string();
            (n.role(), name)
        })
        .collect()
}

#[test]
fn every_lane_action_opens_its_form_or_a_tracked_placeholder() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let _ = link.drain();
    for lane in &LANES {
        for id in lane.actions {
            let cmd = cl_model::commands::find(id).expect("a registry command");
            h.open(id);
            assert_eq!(link.drain(), vec![CompanyEvent::OpenWindow(cmd.id)], "{id}");
            assert_eq!(h.last_opened, Some(cmd.id));
        }
    }
    // The shell opens each one: the real screen, or the placeholder of a
    // command that names the phase that builds it.
    let mut shell = Shell::new(ShellOptions {
        demo: true,
        ..ShellOptions::default()
    });
    let shell_link = Arc::clone(shell.company_link().expect("demo link"));
    for lane in &LANES {
        for id in lane.actions {
            let cmd = cl_model::commands::find(id).expect("command");
            shell_link.push(CompanyEvent::OpenWindow(cmd.id));
            shell.dispatch_now(CommandId::new(KIND));
            let w = shell.windows.active().expect("a window");
            assert_eq!(w.kind, cmd.id, "{id} opened");
            let placeholder = w.screen.state() == Value::Null;
            assert_eq!(placeholder, cmd.arrives.is_some(), "{id}");
            if let Some(p) = cmd.arrives {
                assert!(p.starts_with('P'), "{id} names its phase");
                assert_eq!(
                    crate::windows::Placeholder::for_command(cmd.id).arrives(),
                    Some(p)
                );
            }
        }
    }
}

#[test]
fn home_is_a_built_screen_and_the_demo_opens_it() {
    let cmd = cl_model::commands::find(KIND).expect("nav.home");
    assert!(cmd.opens && cmd.arrives.is_none(), "Home is built (P16)");
    let shell = Shell::new(ShellOptions {
        demo: true,
        ..ShellOptions::default()
    });
    let w = shell.windows.find_kind(cmd.id).expect("Home is open");
    let st = shell.windows.get(w).expect("window").screen.state();
    assert_eq!(st["screen"], "home");
    let (_d, link) = demo_link();
    assert!(make(cmd.id, &link, None, DemoState::today()).is_some());
    let other = cl_model::commands::find("vendors.write_check")
        .expect("wc")
        .id;
    assert!(make(other, &link, None, DemoState::today()).is_none());
}

#[test]
fn the_demo_lanes_show_the_mockup() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    assert!(h.exact, "the demo simulates dashboard-exact-v1");
    for width in [1440.0, 960.0, 480.0] {
        let nodes = tree(&mut h, width);
        let has = |role: Role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
        assert!(has(Role::Label, "Home"), "{width}");
        for lane in &LANES {
            let title = crate::i18n::t(&format!("home.lane.{}", lane.id.key())).to_uppercase();
            assert!(has(Role::Label, &title), "{title}");
            for id in lane.actions {
                let label = lanes::action_label(id);
                assert!(has(Role::Button, &label), "{label} at {width}: {nodes:#?}");
            }
        }
        assert!(has(Role::Button, "Review Bank Feed"));
        assert!(has(Role::Button, "Refresh"));
        for name in [
            "Clients owe you",
            "$48,912.40",
            "Bills due in 7 days",
            "$7,471.90",
            "Operating Checking \u{b7} 09/30/2026",
        ] {
            assert!(has(Role::Label, name), "{name} at {width}");
        }
    }
    let st = h.state();
    assert_eq!(
        st["lanes"],
        json!(["money_in", "money_out", "banking", "close_report"])
    );
    assert_eq!(st["lane_columns"], 1, "the last draw was 480 wide");
    assert_eq!(
        st["footers"],
        json!({"clients_owe": true, "bills_due": true, "last_reconciled": true, "sales_tax_owed": false})
    );
    let text = st.to_string();
    for figure in ["48912", "48,912", "7471", "Operating"] {
        assert!(!text.contains(figure), "{figure} in {text}");
    }
}

/// An engine without `dashboard-exact-v1`: dashes for the dashboard figures;
/// the reconciliation footer still reads.
#[test]
fn without_the_dashboard_capability_lane_figures_are_dashes() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    h.exact = false;
    h.data = HomeData {
        accounts: Slot::ready(demo::accounts(&mut DemoState::new())),
        reconcile: Slot::ready(demo::reconcile(&[crate::company::demo::account_id(1010)])),
        ..HomeData::default()
    };
    let style = cl_core::MoneyStyle::default();
    let owe = lanes::footer_view(&h, LaneFooter::ClientsOwe, style);
    assert_eq!(owe.value, None);
    assert_eq!(
        owe.tip.as_deref(),
        Some(crate::i18n::t("home.footer.later.tip").as_str())
    );
    assert_eq!(
        lanes::footer_view(&h, LaneFooter::BillsDue, style).value,
        None
    );
    assert_eq!(
        lanes::footer_view(&h, LaneFooter::SalesTaxOwed, style).value,
        None
    );
    assert_eq!(
        lanes::footer_view(&h, LaneFooter::LastReconciled, style)
            .value
            .as_deref(),
        Some("Operating Checking \u{b7} 09/30/2026")
    );
    h.data.reconcile = Slot::failed(ApiError::Internal);
    let last = lanes::footer_view(&h, LaneFooter::LastReconciled, style);
    assert_eq!(last.value, None);
    assert!(last.tip.is_some(), "a failed read says so");
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

/// The demo's balances: the card shows the demo engine's figures, exactly.
#[test]
fn the_demo_balances_follow_the_demo_engine() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let rows = balances::rows(&h.data, h.exact).expect("rows");
    assert_eq!(
        rows.first().map(|r| r.label.as_str()),
        Some("1010 \u{b7} Operating Checking")
    );
    assert_eq!(rows.first().and_then(|r| r.books), Some(m("84210.55")));
    assert!(rows.iter().all(|r| r.books.is_some()));
    let nodes = tree(&mut h, 1440.0);
    let has = |role: Role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
    for name in [
        "Account balances",
        "Account",
        "In your books",
        "Bank file",
        "1010 \u{b7} Operating Checking",
        "84,210.55",
    ] {
        assert!(has(Role::Label, name), "{name}");
    }
    let st = h.state();
    assert_eq!(st["balances_source"], "cash_position");
    assert_eq!(st["balances_rows"], json!(rows.len()));
    assert!(!st.to_string().contains("84210"), "no amounts in the state");
}

/// Without `dashboard-exact-v1` the balances are the chart's own stored
/// balances: strings, read exactly, banks first, never summed here.
#[test]
fn without_the_dashboard_capability_balances_stay_exact() {
    let account = |id: u32, number: &str, name: &str, detail: &str, kind: &str, balance: &str| {
        serde_json::from_value::<Account>(json!({
            "id": crate::company::demo::account_id(id).to_string(),
            "name": name,
            "accountNumber": number,
            "accountType": kind,
            "detailType": detail,
            "isActive": true,
            "balance": balance,
        }))
        .expect("account")
    };
    let accounts = vec![
        account(
            2050,
            "2050",
            "Business Card",
            "credit_card",
            "liability",
            "-6388.21",
        ),
        account(
            1020,
            "1020",
            "Payroll Checking",
            "bank",
            "asset",
            "12004.00",
        ),
        account(
            1010,
            "1010",
            "Operating Checking",
            "bank",
            "asset",
            "84210.55",
        ),
        account(6100, "6100", "Fuel", "fuel", "expense", "99.00"),
    ];
    // A JSON number is never read as money.
    assert!(
        serde_json::from_value::<Account>(json!({
            "id": crate::company::demo::account_id(1).to_string(),
            "name": "X", "accountType": "asset", "isActive": true, "balance": 84210.55,
        }))
        .is_err()
    );
    let data = HomeData {
        accounts: Slot::ready(accounts),
        ..HomeData::default()
    };
    let rows = balances::rows(&data, false).expect("rows");
    let shown: Vec<(&str, Option<Money>)> =
        rows.iter().map(|r| (r.label.as_str(), r.books)).collect();
    assert_eq!(
        shown,
        [
            ("1010 \u{b7} Operating Checking", Some(m("84210.55"))),
            ("1020 \u{b7} Payroll Checking", Some(m("12004.00"))),
            ("2050 \u{b7} Business Card", Some(m("-6388.21"))),
        ],
        "banks first, then cards; the engine's figure as stored"
    );
    assert_eq!(balances::source(&data, false), "accounts");
    // With the capability but no cash position yet, the chart's figures.
    assert_eq!(balances::source(&data, true), "accounts");

    // Nothing read yet: no rows (loading); an empty chart has none.
    assert_eq!(balances::rows(&HomeData::default(), false), None);
    let empty = HomeData {
        accounts: Slot::ready(Vec::new()),
        ..HomeData::default()
    };
    assert_eq!(balances::rows(&empty, false), Some(Vec::new()));
    let failed = HomeData {
        accounts: Slot::failed(ApiError::Internal),
        ..HomeData::default()
    };
    assert_eq!(balances::rows(&failed, false), None);
    assert_eq!(balances::source(&failed, false), "none");
}

#[test]
fn lane_chips_come_from_the_registry_and_the_engine() {
    let (_d, link) = demo_link();
    let h = home(&link);
    assert_eq!(
        lanes::trailing(&h, "vendors.write_check"),
        lanes::Trailing::Chip("Ctrl W".into())
    );
    assert_eq!(
        lanes::trailing(&h, "banking.feed"),
        lanes::Trailing::Badge(37)
    );
    assert_eq!(
        lanes::trailing(&h, "reports.profit_loss"),
        lanes::Trailing::Note {
            text: "Oct".into(),
            mono: false
        }
    );
    assert_eq!(
        lanes::trailing(&h, "company.closing_date"),
        lanes::Trailing::Note {
            text: "09/30".into(),
            mono: true
        }
    );
    assert_eq!(
        lanes::trailing(&h, "company.send_to_accountant"),
        lanes::Trailing::Nothing
    );
    assert_eq!(lanes::action_label("banking.feed"), "Review Bank Feed");
    assert_eq!(lanes::action_label("vendors.write_check"), "Write Check");
    assert_eq!(
        (
            lanes::columns(1200.0),
            lanes::columns(900.0),
            lanes::columns(480.0)
        ),
        (4, 2, 1)
    );
    let d = |y, mo, day| CalendarDate::from_ymd(y, mo, day).expect("date");
    assert_eq!(lanes::last_month(d(2026, 11, 4)), 10);
    assert_eq!(lanes::last_month(d(2027, 1, 15)), 12);
    for month in 1..=12 {
        assert_eq!(lanes::month_short(month).len(), 3, "{month}");
    }
    for lane in &LANES {
        assert!(crate::i18n::has(&format!("home.lane.{}", lane.id.key())));
    }
}

#[test]
fn a_change_elsewhere_reads_home_again_and_feeds_the_rail() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let rail = |events: Vec<CompanyEvent>| {
        events
            .into_iter()
            .filter_map(|e| match e {
                CompanyEvent::RailCounts(c) => Some(c),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let first = rail(link.drain());
    assert_eq!(first.len(), 1, "read on open");
    assert_eq!(first[0].bank_feed, Some(37));
    assert!(first[0].clients.is_some() && first[0].vendors.is_some());
    let _ = tree(&mut h, 1440.0);
    assert!(
        rail(link.drain()).is_empty(),
        "no timer: drawing reads nothing"
    );
    link.bus().publish(crate::data_bus::Change::Ledger(None));
    let _ = tree(&mut h, 1440.0);
    assert_eq!(rail(link.drain()).len(), 1, "a save elsewhere reads again");
}
