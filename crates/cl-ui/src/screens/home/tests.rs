//! Home on the demo engine (both P16 capabilities simulated) and on hand-made
//! answers (neither): lane wiring, chips, exact balances, dashes, and
//! attention counts that are exactly the engine's answers.

use super::*;
use crate::company::demo::DemoState;
use crate::shell::{Shell, ShellOptions};
use cl_api::Account;
use cl_core::Money;
use cl_model::home::{AttentionItem, AttentionKind, LaneFooter};
use eframe::egui::accesskit::Role;

fn counted(items: &[AttentionItem], key: &str) -> Option<u64> {
    items
        .iter()
        .find(|i| i.kind.key() == key)
        .and_then(|i| i.count)
}

/// The mockup's Needs attention on the demo, at every width.
#[test]
fn the_demo_needs_attention_shows_the_mockup() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    for width in [1440.0, 960.0, 480.0] {
        let nodes = tree(&mut h, width);
        let has = |role: Role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
        for name in [
            "Needs attention",
            "37 bank transactions to review",
            "Reconcile not finished for Operating Checking",
            "2 invoices past due",
            "1 payroll import not posted yet",
            "Backed up today 8:12 AM",
        ] {
            assert!(has(Role::Label, name), "{name} at {width}");
        }
        assert!(nodes.iter().any(|(r, n)| *r == Role::Label
            && n.starts_with("To-dos, reorder alerts and the weekly health check")));
        for link_name in ["Review", "Resume", "Open aging", "Pay bills", "Import"] {
            assert!(has(Role::Link, link_name), "{link_name}");
        }
    }
    let st = h.state();
    assert_eq!(st["attention"]["bank_waiting"], 37);
    assert_eq!(st["attention"]["payroll_unposted"], 1);
    assert_eq!(st["attention"]["todos"], Value::Null);
    assert_eq!(st["backup_due"], false);
    assert_eq!(
        st["attention_available"],
        json!([
            "bank_waiting",
            "reconcile_unfinished",
            "invoices_overdue",
            "bills_due",
            "payroll_unposted",
            "backup"
        ])
    );
    let text = st.to_string();
    for figure in ["84210", "48912", "7471", "Operating"] {
        assert!(!text.contains(figure), "{figure} in {text}");
    }
    // A link opens its window.
    let _ = link.drain();
    h.open("vendors.pay_bills");
    assert_eq!(
        link.drain(),
        vec![CompanyEvent::OpenWindow(CommandId::new(
            "vendors.pay_bills"
        ))]
    );
}

#[test]
fn attention_counts_are_exactly_the_engine_answers() {
    let op = crate::company::demo::account_id(1010);
    let card = crate::company::demo::account_id(2050);
    let mut summary = demo::summary(&mut DemoState::new(), DemoState::today(), 7);
    if let Some(r) = summary.receivables.as_mut() {
        r.overdue_count = 4;
    }
    let data = HomeData {
        counts: Slot::ready(Counts {
            feed: Ok(5),
            bills: Ok(3),
            payroll: Ok(0),
            clients: Ok(64),
            vendors: Ok(38),
        }),
        reconcile: Slot::ready(ReconcileState {
            in_progress: vec![op, card],
            last_completed: None,
        }),
        summary: Slot::ready(summary),
        ..HomeData::default()
    };
    let items = attention::items(&data, 14, true, false);
    assert_eq!(counted(&items, "bank_waiting"), Some(5));
    assert_eq!(counted(&items, "bills_due"), Some(3));
    assert_eq!(counted(&items, "payroll_unposted"), Some(0));
    assert_eq!(counted(&items, "reconcile_unfinished"), Some(2));
    assert_eq!(counted(&items, "invoices_overdue"), Some(4));
    assert_eq!(counted(&items, "backup"), Some(0));
    for later in ["todos", "reorder", "health_check"] {
        let i = items.iter().find(|i| i.kind.key() == later).expect("item");
        assert_eq!((i.count, i.available), (None, false), "{later}");
    }
    assert!(items.contains(&AttentionItem::counted(
        AttentionKind::BillsDue { days: 14 },
        3
    )));

    // Before U-76 overdue invoices can't be counted: a dash, not a zero.
    let items = attention::items(&data, 14, false, true);
    let inv = items
        .iter()
        .find(|i| i.kind == AttentionKind::InvoicesOverdue)
        .expect("invoices");
    assert_eq!((inv.count, inv.available), (None, false));
    assert_eq!(counted(&items, "backup"), Some(1), "a backup is due");

    // One failed query is one unknown count; the rest still show.
    let data = HomeData {
        counts: Slot::ready(Counts {
            feed: Err(ApiError::Internal),
            bills: Ok(1),
            payroll: Ok(2),
            clients: Ok(0),
            vendors: Ok(0),
        }),
        reconcile: Slot::failed(ApiError::Internal),
        ..HomeData::default()
    };
    let items = attention::items(&data, 7, true, false);
    let bank = items
        .iter()
        .find(|i| i.kind == AttentionKind::BankWaiting)
        .expect("bank");
    assert_eq!((bank.count, bank.available), (None, true));
    assert_eq!(counted(&items, "reconcile_unfinished"), None);
    assert_eq!(counted(&items, "bills_due"), Some(1));
    assert_eq!(counted(&items, "payroll_unposted"), Some(2));
}

/// With `dashboard-exact-v1` and a window of 1 to 90 days the bills line is
/// the payables panel's overdue plus due-soon count (voided bills out on the
/// engine); for another window, or without the panel, the list's count.
#[test]
fn the_bills_line_takes_the_payables_panel_for_its_window() {
    let mut summary = demo::summary(&mut DemoState::new(), DemoState::today(), 7);
    if let Some(p) = summary.payables.as_mut() {
        p.overdue_count = 2;
        p.due_soon_count = 5;
    }
    let data = HomeData {
        counts: Slot::ready(Counts {
            feed: Ok(0),
            bills: Ok(9),
            payroll: Ok(0),
            clients: Ok(0),
            vendors: Ok(0),
        }),
        summary: Slot::ready(summary),
        ..HomeData::default()
    };
    assert_eq!(attention::payables_due(&data, 7, true), Some(7));
    let items = attention::items(&data, 7, true, false);
    assert_eq!(counted(&items, "bills_due"), Some(7), "overdue + due soon");
    // Another window than the panel was read for: the list.
    assert_eq!(attention::payables_due(&data, 14, true), None);
    assert_eq!(
        counted(&attention::items(&data, 14, true, false), "bills_due"),
        Some(9)
    );
    // Past the engine's 90 days, and without the capability: the list.
    assert_eq!(attention::payables_due(&data, 120, true), None);
    assert_eq!(attention::payables_due(&data, 7, false), None);
    assert_eq!(
        counted(&attention::items(&data, 7, false, false), "bills_due"),
        Some(9)
    );
    // The list's count failed (past its page cap): a dash, not a number.
    let data = HomeData {
        counts: Slot::ready(Counts {
            feed: Ok(0),
            bills: Err(ApiError::Local(cl_api::LocalValidation::PageLimit)),
            payroll: Ok(0),
            clients: Ok(0),
            vendors: Ok(0),
        }),
        ..HomeData::default()
    };
    assert_eq!(
        counted(&attention::items(&data, 7, false, false), "bills_due"),
        None
    );
}

/// Home follows the shell's date: when the shell's day moves on (midnight
/// with Home open) Home reads everything again for the new day; outside a
/// shell it keeps the date it was opened with.
#[test]
fn home_follows_the_shells_date() {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let opened = DemoState::today();
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    let frame = |ctx: &egui::Context, h: &mut HomeScreen| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| h.ui(ui));
        });
        out.textures_delta.clear();
    };
    frame(&ctx, &mut h);
    assert_eq!(h.today, opened, "no shell: the date Home opened with");
    let months = |h: &HomeScreen| {
        h.data
            .chart
            .value
            .as_ref()
            .map(|m| m.last().map(|m| m.range.end))
    };
    let before = months(&h);
    // The shell's day moves to the first of next month.
    let next = date(2026, 12, 1);
    crate::grid_env::publish(
        &ctx,
        egui::ViewportId::ROOT,
        crate::grid_env::ShellFrame::new(next, &crate::prefs::UiPrefs::default(), false),
    );
    frame(&ctx, &mut h);
    assert_eq!(h.today, next);
    for _ in 0..3 {
        frame(&ctx, &mut h);
    }
    assert_ne!(months(&h), before, "the chart read again for the new day");
    assert_eq!(
        h.data
            .chart
            .value
            .as_ref()
            .and_then(|m| m.last())
            .map(|m| m.range.end),
        Some(date(2026, 11, 30)),
        "the last whole month: November"
    );
    // The same day again: no new read.
    assert!(!h.follow_shell_date(&ctx));
}

/// "1 day", never "1 days"; "Open aging" explains itself on hover; a past
/// backup shows in its own instant's offset.
#[test]
fn day_counts_aging_tip_and_backup_offsets() {
    use chrono::Offset as _;
    assert_eq!(attention::days_text(1), "1 day");
    assert_eq!(attention::days_text(14), "14 days");
    let mut summary = demo::summary(&mut DemoState::new(), DemoState::today(), 1);
    if let Some(p) = summary.payables.as_mut() {
        p.overdue_count = 1;
        p.due_soon_count = 0;
    }
    let (_d, link) = demo_link();
    let mut h = home(&link);
    h.local.bills_due_days = 1;
    h.data.summary = Slot::ready(summary);
    let lines = attention::lines(&h, &h.attention());
    assert!(
        lines
            .iter()
            .any(|l| l.text == "1 bill overdue or due within 1 day"),
        "{lines:#?}"
    );
    let aging = lines
        .iter()
        .find(|l| l.link.as_ref().is_some_and(|(t, _)| t == "Open aging"))
        .expect("aging link");
    assert!(
        aging
            .link_tip
            .as_deref()
            .is_some_and(|t| t.contains("past due"))
    );
    assert!(
        lines
            .iter()
            .filter(|l| l.link.as_ref().is_some_and(|(t, _)| t != "Open aging"))
            .all(|l| l.link_tip.is_none())
    );

    // A summer backup seen in winter: its own (summer) offset.
    let summer = chrono::FixedOffset::west_opt(5 * 3600).expect("CDT");
    let at = Timestamp::from_unix_millis(
        chrono::DateTime::parse_from_rfc3339("2026-07-01T03:30:00Z")
            .expect("ts")
            .timestamp_millis(),
    )
    .expect("ts");
    assert_eq!(
        attention::when_text(at, Zone::Fixed(summer), date(2026, 11, 4)),
        "06/30/2026"
    );
    assert_eq!(
        attention::when_text(at, Zone::Fixed(summer), date(2026, 6, 30)),
        "today 10:30 PM"
    );
    let local = Zone::Local.at(at);
    let expected = at.as_utc().with_timezone(&chrono::Local);
    assert_eq!(
        local.offset().local_minus_utc(),
        expected.offset().fix().local_minus_utc()
    );
}

/// The lines Home draws: a dash for what can't be counted (overdue invoices
/// before U-76, a failed query), one muted line for what arrives later,
/// nothing for a zero, "Nothing needs attention" when all is clear.
#[test]
fn attention_lines_show_dashes_and_the_later_note() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    h.exact = false;
    h.data = HomeData {
        accounts: Slot::ready(demo::accounts(&mut DemoState::new())),
        counts: Slot::ready(Counts {
            feed: Err(ApiError::Internal),
            bills: Ok(0),
            payroll: Ok(0),
            clients: Ok(0),
            vendors: Ok(0),
        }),
        reconcile: Slot::ready(ReconcileState::default()),
        ..HomeData::default()
    };
    let lines = attention::lines(&h, &h.attention());
    let dash = |text: &str| {
        lines
            .iter()
            .find(|l| l.text == text)
            .unwrap_or_else(|| panic!("{text}: {lines:#?}"))
            .clone()
    };
    let invoices = dash("Invoices past due");
    assert!(invoices.dash.is_some() && invoices.link.is_none());
    assert!(dash("Bank transactions to review").dash.is_some());
    assert!(
        !lines.iter().any(|l| l.text.contains("bills overdue")),
        "a zero shows nothing"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.muted && l.text.starts_with("To-dos, reorder alerts"))
    );
    assert!(
        !h.attention().iter().any(AttentionItem::needs_attention),
        "all clear"
    );
    for kind in AttentionKind::all(7) {
        if kind.supported() && kind != AttentionKind::Backup {
            assert!(
                crate::i18n::has(&format!("home.attention.{}.name", kind.key())),
                "{}",
                kind.key()
            );
        }
    }
}

#[test]
fn the_backup_reminder_follows_the_last_backup() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let line_of = |h: &HomeScreen| {
        attention::lines(h, &h.attention())
            .into_iter()
            .last()
            .expect("the backup line")
    };
    let l = line_of(&h);
    assert_eq!(l.text, "Backed up today 8:12 AM");
    assert!(l.muted && l.link.is_none());

    h.local.last_backup = None;
    let l = line_of(&h);
    assert_eq!(l.text, "This company hasn't been backed up yet");
    assert_eq!(l.link.map(|(_, opens)| opens), Some("file.backup"));

    let eight_days =
        Timestamp::from_unix_millis(h.local.now.unix_millis() - 8 * 86_400_000).expect("ts");
    h.local.last_backup = Some(eight_days);
    assert!(h.backup_due());
    let l = line_of(&h);
    assert_eq!(l.text, "Not backed up since 10/27/2026");
    assert_eq!(
        l.link.map(|(text, _)| text),
        Some("Back up now".to_string())
    );
}

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

// ----------------------------------------------------------------- WP8

fn date(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

fn pl(income: &str, expenses: &str, net: &str) -> cl_api::ProfitLoss {
    serde_json::from_value(json!({
        "totalIncome": income, "totalExpenses": expenses, "netIncome": net,
    }))
    .expect("profit and loss")
}

/// Standard months are calendar months; a 4-4-5 year's periods are whole
/// weeks named by their middle day. Six whole periods before today's.
#[test]
fn chart_months_follow_the_fiscal_calendar() {
    use cl_settings::FiscalCalendar;
    let today = date(2026, 11, 4);
    let std = charts::months(charts::spec(Some(1), FiscalCalendar::Standard), today);
    let ranges: Vec<(u32, String, String)> = std
        .iter()
        .map(|m| (m.named, m.start.to_wire(), m.end.to_wire()))
        .collect();
    assert_eq!(ranges.len(), charts::MONTHS);
    assert_eq!(ranges[0], (5, "2026-05-01".into(), "2026-05-31".into()));
    assert_eq!(ranges[5], (10, "2026-10-01".into(), "2026-10-31".into()));

    let retail = charts::months(charts::spec(Some(1), FiscalCalendar::Retail445), today);
    let names: Vec<u32> = retail.iter().map(|m| m.named).collect();
    assert_eq!(names, [5, 6, 7, 8, 9, 10]);
    assert_eq!(
        (retail[0].start.to_wire(), retail[0].end.to_wire()),
        ("2026-04-26".into(), "2026-05-23".into())
    );
    assert_eq!(
        (retail[5].start.to_wire(), retail[5].end.to_wire()),
        ("2026-09-27".into(), "2026-10-24".into()),
        "the period holding 11/04 starts 10/25"
    );
    for w in retail.windows(2) {
        assert_eq!(w[0].end.add_days(1).expect("day"), w[1].start, "contiguous");
    }
    for m in &retail {
        let days = m.start.days_until(m.end) + 1;
        assert!(days == 28 || days == 35, "{days}");
    }
    // A July fiscal start still shows calendar months.
    let july = charts::months(charts::spec(Some(7), FiscalCalendar::Standard), today);
    assert_eq!(july.last().map(|m| m.named), Some(10));
    assert_eq!(charts::basis(Some("cash")), cl_api::ReportBasis::Cash);
    assert_eq!(charts::basis(None), cl_api::ReportBasis::Accrual);
}

/// The figure is the report's net income, even when it is not income minus
/// expenses; a failed month is missing (a dash), not a zero.
#[test]
fn the_chart_shows_the_engines_net_income_and_missing_months() {
    let m = |n: u32| charts::Month {
        named: n,
        start: date(2026, n, 1),
        end: date(2026, n, 28),
    };
    let months = vec![
        charts::MonthPl {
            range: m(9),
            pl: Err(ApiError::Internal),
        },
        charts::MonthPl {
            range: m(10),
            pl: Ok(pl("21400", "7534.86", "13865.1")),
        },
    ];
    let style = cl_core::MoneyStyle::default();
    let v = charts::views(&months, style);
    assert!(v[0].missing && v[0].income.is_none() && v[0].income_cents == 0);
    assert_eq!(
        v[1].net.as_deref(),
        Some("13,865.10"),
        "the engine's figure"
    );
    assert_ne!(
        v[1].net.as_deref(),
        Some("13,865.14"),
        "not 21,400 - 7,534.86"
    );
    assert_eq!(
        (v[1].income_cents, v[1].expense_cents),
        (2_140_000, 753_486)
    );
    assert_eq!(
        charts::accessible(&v[1]),
        "October: income 21,400.00, expenses 7,534.86"
    );
    assert_eq!(charts::accessible(&v[0]), "September: couldn't load");
    let tokens = crate::theme::Tokens::LIGHT;
    let g = charts::groups(&v, &tokens);
    assert_eq!(g[0].bars, None);
    assert_eq!(g[1].bars.as_ref().map(Vec::len), Some(2));
    assert_eq!(g[1].label, "Oct");
    assert!(!charts::empty(&v));
    let zero = vec![charts::MonthPl {
        range: m(10),
        pl: Ok(pl("0", "0", "0")),
    }];
    assert!(charts::empty(&charts::views(&zero, style)), "a new company");
    let neg: cl_api::ReportAmount = serde_json::from_str("-12.345").expect("amount");
    assert_eq!(charts::cents(&neg), Some(-1235));
}

/// The demo's chart on Home: six months, each group a named button,
/// October's profit, and a click drills into Profit & Loss with the range.
#[test]
fn the_demo_chart_shows_the_mockup_and_drills_into_the_report() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let nodes = tree(&mut h, 1440.0);
    let has = |role: Role, name: &str| nodes.iter().any(|(r, n)| *r == role && n == name);
    assert!(has(Role::Label, "Income and expenses \u{b7} last 6 months"));
    assert!(has(Role::Label, "October profit"));
    assert!(has(Role::Label, "$13,865.14"));
    assert!(has(Role::Label, "Income") && has(Role::Label, "Expenses"));
    let groups: Vec<&String> = nodes
        .iter()
        .filter(|(r, n)| *r == Role::Button && n.contains(": income "))
        .map(|(_, n)| n)
        .collect();
    assert_eq!(groups.len(), 6, "{groups:?}");
    assert!(groups.iter().any(|n| n.starts_with("October: income ")));
    let st = h.state();
    assert_eq!(st["chart"]["months"], 6);
    assert_eq!(st["chart"]["missing"], 0);
    assert_eq!(
        st["chart"]["labels"],
        json!(["May", "Jun", "Jul", "Aug", "Sep", "Oct"])
    );
    assert!(!st.to_string().contains("13865"), "no amounts in the state");
    let _ = link.drain();
    let oct = charts::Month {
        named: 10,
        start: date(2026, 10, 1),
        end: date(2026, 10, 31),
    };
    h.drill(oct);
    assert_eq!(
        link.drain(),
        vec![CompanyEvent::OpenWindow(CommandId::new(
            "reports.profit_loss"
        ))]
    );
    assert_eq!(h.state()["chart"]["drill"], "2026-10-01..2026-10-31");
}

/// The checklist ticks from the reads alone: the profile, the chart, any
/// transaction, a bank file, the mail server and the last backup.
#[test]
fn the_setup_checklist_ticks_from_the_reads() {
    use cl_model::home::ChecklistItem;
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let c = checklist::checklist(&h).expect("read");
    assert_eq!(
        (c.done_count(), c.total()),
        (5, 6),
        "no opening-balance step (Q-P16j)"
    );
    assert_eq!(c.first_open(), Some(ChecklistItem::Email));
    assert!(!c.is_done(ChecklistItem::OpeningBalances));

    // A new company: nothing done, the list opens by itself.
    h.data.checklist = Slot::ready(checklist::Facts::default());
    h.data.accounts = Slot::ready(Vec::new());
    h.local.last_backup = None;
    let c = checklist::checklist(&h).expect("read");
    assert_eq!(c.done_count(), 0);
    assert!(h.checklist_expanded(), "prominent for a new company");
    assert_eq!(h.state()["checklist"]["expanded"], true);

    // Everything done: the checklist hides itself.
    h.data.checklist = Slot::ready(checklist::Facts {
        company_info: true,
        transaction: true,
        bank_file: true,
        email: true,
    });
    h.data.accounts = Slot::ready(demo::accounts(&mut DemoState::new()));
    h.local.last_backup = Some(demo::last_backup());
    assert!(checklist::checklist(&h).expect("read").complete());
    assert!(h.checklist_shown().is_none());
    assert_eq!(h.state()["checklist"]["visible"], false);

    // Not read yet: nothing shown.
    h.data.checklist = Slot::default();
    assert!(checklist::checklist(&h).is_none());

    let mut p = cl_api::CompanyProfileUpdate {
        business_name: "Ozark Ridge".into(),
        ..cl_api::CompanyProfileUpdate::default()
    };
    assert!(!checklist::profile_filled(&p), "no address yet");
    p.address_line1 = Some("12 Main".into());
    assert!(checklist::profile_filled(&p));
    p.business_name = "  ".into();
    assert!(!checklist::profile_filled(&p));
}

/// The demo's title row: "Setup checklist 5 of 6 done · Set up email for
/// invoices"; the summary opens the full list, a link opens its window, and
/// Hide checklist is kept in the company's preferences.
#[test]
fn the_setup_checklist_opens_hides_and_stays_hidden() {
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let nodes = tree(&mut h, 1440.0);
    let has = |nodes: &[(Role, String)], role: Role, name: &str| {
        nodes.iter().any(|(r, n)| *r == role && n == name)
    };
    assert!(has(&nodes, Role::Button, "Setup checklist 5 of 6 done"));
    assert!(has(&nodes, Role::Link, "Set up email for invoices"));
    assert!(
        !has(&nodes, Role::Button, "Hide checklist"),
        "closed at 5 of 6"
    );
    let st = h.state();
    assert_eq!(st["checklist"]["done"], 5);
    assert_eq!(st["checklist"]["total"], 6);
    assert_eq!(st["checklist"]["first_open"], "email");

    h.checklist_action(checklist::Action::Toggle);
    let nodes = tree(&mut h, 1440.0);
    assert!(has(&nodes, Role::Button, "Hide checklist"));
    assert!(has(&nodes, Role::Label, "Add a bank account (done)"));
    assert!(has(&nodes, Role::Label, "Set up email for invoices"));
    // Narrow: the line moves under the title and still shows.
    let nodes = tree(&mut h, 480.0);
    assert!(has(&nodes, Role::Link, "Set up email for invoices"));

    let _ = link.drain();
    h.checklist_action(checklist::Action::Open("edit.preferences"));
    assert_eq!(
        link.drain(),
        vec![CompanyEvent::OpenWindow(CommandId::new("edit.preferences"))]
    );

    h.checklist_action(checklist::Action::Dismiss);
    assert!(h.checklist_shown().is_none());
    let (id, _) = tenant(&link).expect("demo company");
    let saved = link.store().load().expect("store");
    assert!(
        saved
            .get(id)
            .expect("record")
            .prefs
            .setup_checklist_dismissed,
        "kept in CompanyPrefs"
    );
    let again = home(&link);
    assert!(again.local.checklist_dismissed);
    assert!(again.checklist_shown().is_none());
    assert_eq!(again.state()["checklist"]["dismissed"], true);
}

/// Ko-fi (Q-278): the address is the constant, drawing Home never opens
/// it, and one click records exactly one URL (the test sink stands in for
/// the browser).
#[test]
fn the_kofi_button_opens_the_page_only_when_clicked() {
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, vec2};
    assert_eq!(support::KOFI_URL, "https://ko-fi.com/I3D3227TTP");
    assert!(!support::KOFI_URL.contains('?'), "no tracking parameters");
    let (_d, link) = demo_link();
    let mut h = home(&link);
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let sink = crate::links::LinkSink::install(&ctx);
    let frame = |h: &mut HomeScreen, events: Vec<Event>| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 1400.0))),
            events,
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| h.ui(ui));
        });
        out.textures_delta.clear();
        out
    };
    let mut button = None;
    for _ in 0..5 {
        let out = frame(&mut h, Vec::new());
        assert!(
            !out.platform_output
                .commands
                .iter()
                .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_))),
            "no browser without a click"
        );
        if let Some(u) = out.platform_output.accesskit_update {
            for (_, n) in u.nodes {
                if n.role() == Role::Button && n.label() == Some("Support ClientLedger on Ko-fi") {
                    button = n.bounds();
                }
            }
        }
    }
    assert!(sink.opened().is_empty(), "nothing opened by drawing");
    let b = button.expect("the Ko-fi button is drawn and named");
    #[allow(clippy::cast_possible_truncation)]
    let at = Pos2::new(
        f64::midpoint(b.x0, b.x1) as f32,
        f64::midpoint(b.y0, b.y1) as f32,
    );
    let press = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame(&mut h, vec![Event::PointerMoved(at)]);
    frame(&mut h, vec![press(true)]);
    let out = frame(&mut h, vec![press(false)]);
    assert!(
        !out.platform_output
            .commands
            .iter()
            .any(|c| matches!(c, egui::OutputCommand::OpenUrl(_))),
        "the sink, not the browser"
    );
    frame(&mut h, Vec::new());
    assert_eq!(sink.opened(), [support::KOFI_URL], "exactly one");
}
