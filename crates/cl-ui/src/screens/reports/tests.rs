//! Report window tests (P28 WP1) on the demo company.

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::report::{AgingBucket, Cell, Drill, RowKind, Source, TagScope};
use cl_model::CommandId;
use cl_model::reports::{DateChoice, ReportOptions, find};
use eframe::egui;

use super::drill::{Target, target};
use super::window::{ReportWindow, RunState, sort_within_groups};
use super::*;
use crate::company::CompanyLink;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn window(link: &Arc<CompanyLink>) -> ReportWindow {
    let spec = find("profit_loss").expect("p&l");
    ReportWindow::new(Arc::clone(link), CommandId::new(KIND), None, spec, today())
}

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

#[test]
fn every_reports_command_opens_the_window() {
    let (_d, link) = demo();
    for kind in [
        KIND,
        "reports.profit_loss",
        "reports.balance_sheet",
        "reports.ar_aging",
        "reports.trial_balance",
    ] {
        let cmd = cl_model::commands::find(kind).expect("command");
        assert!(cmd.opens && cmd.arrives.is_none(), "{kind} is built");
        let s = make(cmd.id, &link, None, today()).expect("window");
        assert_eq!(s.state()["screen"], "reports");
    }
    let other = cl_model::commands::find("vendors.write_check")
        .expect("cmd")
        .id;
    assert!(make(other, &link, None, today()).is_none());
}

/// Q-220: a report this engine can't run says so and runs nothing (the
/// demo plays the P&L since WP5, not the Journal).
#[test]
fn a_report_the_engine_cant_run_yet_says_so() {
    let (_d, link) = demo();
    let mut w = window(&link);
    w.poll();
    assert_eq!(w.state, RunState::Shown, "the demo P&L runs");
    let spec = find("journal").expect("spec");
    w.show_report(spec, ReportOptions::for_spec(spec));
    assert_eq!(w.state, RunState::Unavailable);
    let st = w.state_json_for_test();
    assert_eq!(st["available"], false);
    assert!(crate::i18n::has("reports.needs_update"));
}

#[test]
fn the_exact_reports_run_on_the_demo() {
    let (_d, link) = demo();
    let mut w = window(&link);
    for key in [
        "ar_aging_summary",
        "sales_tax_liability",
        "inventory_valuation",
        "inventory_stock_status",
    ] {
        let spec = find(key).expect("spec");
        assert!(available(&link, spec), "{key}");
        w.show_report(spec, ReportOptions::for_spec(spec));
        w.poll();
        // The demo may switch inventory off: a refusal is a shown error,
        // never a hang. A/R aging and sales tax always run.
        if matches!(key, "ar_aging_summary" | "sales_tax_liability") {
            assert_eq!(w.state, RunState::Shown, "{key}");
        }
        assert!(
            matches!(w.state, RunState::Shown | RunState::Failed),
            "{key}: {:?}",
            w.state
        );
        if w.state == RunState::Shown {
            let t = w.table.as_ref().expect("table");
            assert!(!t.columns.is_empty(), "{key}");
            assert!(
                t.rows.iter().any(|r| r.kind == RowKind::Grand) || key == "inventory_stock_status"
            );
        }
    }
}

#[test]
fn collapse_and_options_change_without_running_again() {
    let (_d, link) = demo();
    let mut w = window(&link);
    let spec = find("sales_tax_liability").expect("spec");
    w.show_report(spec, ReportOptions::for_spec(spec));
    w.poll();
    assert_eq!(w.state, RunState::Shown);
    let all = w.shown_rows().len();
    let expandable: Vec<_> = w
        .table
        .as_ref()
        .map(|t| {
            t.rows
                .iter()
                .filter(|r| r.expandable)
                .map(|r| r.path.clone())
                .collect()
        })
        .unwrap_or_default();
    if let Some(p) = expandable.first() {
        w.toggle_collapse(p);
        assert_eq!(w.state, RunState::Shown, "collapse is local");
        assert!(w.shown_rows().len() < all);
        w.toggle_collapse(p);
        assert_eq!(w.shown_rows().len(), all);
    }
    // A date change runs again.
    let mut next = w.options.clone();
    next.dates = DateChoice::Preset(cl_model::reports::DatePreset::ThisYear);
    w.set_options(next);
    w.poll();
    assert_eq!(w.state, RunState::Shown);
}

#[test]
fn exports_read_the_rows_on_screen() {
    let (_d, link) = demo();
    let mut w = window(&link);
    let spec = find("ar_aging_summary").expect("spec");
    w.show_report(spec, ReportOptions::for_spec(spec));
    w.poll();
    assert_eq!(w.state, RunState::Shown);
    // A/R Aging Detail isn't read by this build yet: its cells draw no link.
    let t = w.table.as_ref().expect("table");
    let d = t.rows[0].drill.as_ref().expect("drill").at(&t.columns[0]);
    assert!(!super::drill::live(&link, Source::ArAgingSummary, &d));
    let csv = String::from_utf8(super::export::bytes(&w, super::export::Kind::Csv).expect("csv"))
        .expect("utf8");
    // Header lines first (QA-6), then the heading row.
    assert!(csv.starts_with("A/R Aging"), "{csv}");
    assert!(
        csv.contains(
            "

,Current,"
        ),
        "{csv}"
    );
    assert!(csv.contains("Total"));
    let xlsx = super::export::bytes(&w, super::export::Kind::Xlsx).expect("xlsx");
    assert_eq!(&xlsx[..2], b"PK");
    let pdf = super::export::bytes(&w, super::export::Kind::Pdf).expect("pdf");
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn drills_open_forms_and_report_tabs() {
    let id: cl_core::ids::TransactionId =
        "11111111-1111-4111-8111-111111111111".parse().expect("id");
    assert_eq!(
        target(
            Source::InventoryValuationDetail,
            &Drill::Transaction {
                id,
                txn_type: "bill".into()
            }
        ),
        Target::Form("vendors.enter_bill", id)
    );
    assert!(matches!(
        target(
            Source::InventoryValuationDetail,
            &Drill::Transaction {
                id,
                txn_type: "inventory_adjustment".into()
            }
        ),
        Target::Nothing(_)
    ));
    let as_of = today();
    let Target::Report(open) = target(
        Source::ArAgingSummary,
        &Drill::Aging {
            contact: None,
            bucket: AgingBucket::Over90,
            as_of,
        },
    ) else {
        panic!("a report")
    };
    assert_eq!(open.key, "ar_aging_detail");
    assert_eq!(
        open.options.expect("options").dates,
        DateChoice::AsOf(as_of)
    );
    let lines = |n: usize, tag| Drill::Lines {
        account_ids: (0..n)
            .map(|i| {
                format!("00000000-0000-4000-8000-{i:012}")
                    .parse()
                    .expect("id")
            })
            .collect(),
        from: Some(as_of),
        to: as_of,
        contact: None,
        tag,
        basis: None,
        adjusting: None,
    };
    let Target::Report(open) = target(Source::ProfitLoss, &lines(2, TagScope::Any)) else {
        panic!("a report")
    };
    assert_eq!(open.key, "transaction_detail_by_account");
    assert_eq!(open.options.expect("o").filters.account_ids.len(), 2);
    assert_eq!(
        target(Source::ProfitLoss, &lines(501, TagScope::Any)),
        Target::Nothing("reports.drill.narrow")
    );
}

#[test]
fn sorting_keeps_totals_in_place() {
    use cl_core::report::{ColumnHead, ColumnKind, ReportMeta, ReportRow, ReportTable, RowPath};
    let row = |kind, caption: &str, v: i64| ReportRow {
        kind,
        level: 1,
        path: RowPath::of([caption]),
        caption: caption.into(),
        cells: vec![Cell::Amount(rust_decimal::Decimal::from(v))],
        drill: None,
        expandable: false,
    };
    let t = ReportTable {
        columns: vec![ColumnHead {
            key: "total".into(),
            caption: "Total".into(),
            kind: ColumnKind::Amount,
            range: None,
            drill: None,
        }],
        rows: vec![
            row(RowKind::Header, "h", 0),
            row(RowKind::Detail, "a", 5),
            row(RowKind::Detail, "b", 9),
            row(RowKind::Detail, "c", 1),
            row(RowKind::Total, "t", 15),
            row(RowKind::Detail, "d", 2),
        ],
        meta: ReportMeta::default(),
    };
    let mut rows: Vec<usize> = (0..t.rows.len()).collect();
    sort_within_groups(&t, &mut rows, 0, true);
    assert_eq!(rows, vec![0, 2, 1, 3, 4, 5]);
}

#[test]
fn a_drilled_report_opens_on_its_options() {
    let (_d, link) = demo();
    let mut w = window(&link);
    let spec = find("inventory_valuation").expect("spec");
    let mut o = ReportOptions::for_spec(spec);
    o.dates = DateChoice::AsOf(CalendarDate::from_ymd(2026, 10, 31).expect("d"));
    let taken = crate::windows::Screen::open_with(
        &mut w,
        crate::windows::OpenPayload::Report(Box::new(ReportOpen {
            key: spec.key.into(),
            options: Some(o.clone()),
        })),
    );
    assert!(taken);
    assert_eq!(w.spec.key, "inventory_valuation");
    assert_eq!(w.options, o);
}

/// Every interactive widget has an accessible name (control channel and
/// screen readers).
#[test]
fn every_widget_is_named() {
    let (_d, link) = demo();
    let mut w = window(&link);
    let spec = find("sales_tax_liability").expect("spec");
    w.show_report(spec, ReportOptions::for_spec(spec));
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 940.0),
            )),
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| crate::windows::Screen::ui(&mut w, ui));
        });
        out.textures_delta.clear();
        nodes.extend(
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default(),
        );
    }
    assert_ne!(nodes.len(), 0);
    let unnamed = crate::screens::unnamed_widgets(&nodes);
    assert!(unnamed.is_empty(), "{unnamed:?}");
}

/// AC5: CSV, Excel and PDF carry the owner's tag-row labels and the
/// account-total choice, not the engine's raw rows.
#[test]
fn exports_carry_the_tag_labels_and_the_total_choice() {
    use cl_core::report::{ReportMeta, ReportRow, ReportTable, RowPath};
    use cl_model::reports::{TagLabel, TagSplit};
    let (_d, link) = demo();
    let mut w = window(&link);
    let row = |kind, path: &[&str], caption: &str| ReportRow {
        kind,
        level: 1,
        path: RowPath::of(path.iter().copied()),
        caption: caption.into(),
        cells: vec![Cell::Amount(rust_decimal::Decimal::new(1234, 2))],
        drill: None,
        expandable: false,
    };
    w.table = Some(ReportTable {
        columns: vec![cl_core::report::ColumnHead {
            key: "total".into(),
            caption: "@reports.col.total".into(),
            kind: cl_core::report::ColumnKind::Amount,
            range: None,
            drill: None,
        }],
        rows: vec![
            row(RowKind::Account, &["expenses", "a"], "6100 \u{b7} Repairs"),
            row(
                RowKind::TagRow,
                &["expenses", "a", "t:1"],
                "10 \u{b7} Lake Area",
            ),
            row(
                RowKind::AccountTotal,
                &["expenses", "a~split"],
                "@reports.row.total_of Repairs",
            ),
        ],
        meta: ReportMeta::default(),
    });
    let mut split = TagSplit {
        tag_group_id: cl_core::ids::TagGroupId::parse("22222222-2222-4222-8222-222222222222")
            .expect("g"),
        show_account_total: false,
        label: TagLabel::Prepend,
        separator: "/".into(),
    };
    w.options.tag_split = Some(split.clone());
    let csv = |w: &ReportWindow| {
        String::from_utf8(super::export::bytes(w, super::export::Kind::Csv).expect("csv"))
            .expect("utf8")
    };
    let text = csv(&w);
    assert!(text.contains("10/6100 Repairs - Lake Area"), "{text}");
    assert!(!text.contains("Total Repairs"), "{text}");
    split.show_account_total = true;
    split.label = TagLabel::None;
    w.options.tag_split = Some(split);
    let text = csv(&w);
    assert!(text.contains("Repairs - Lake Area"), "{text}");
    assert!(!text.contains("10/6100"), "{text}");
    assert!(text.contains("Total Repairs"), "{text}");
}

/// B3, Q-P28ah: every filter a report offers reaches its route (it changes
/// the query, or the list's narrowing), and none of them makes the window
/// say "Needs an engine update" on an engine with every capability. So no
/// filter is shown and then ignored, and Ask (which offers the same
/// filters) never maps to a dead end.
#[test]
fn every_offered_filter_changes_what_runs() {
    use cl_api::resources::reports::{list_run, run};
    use cl_model::reports::{AmountFilter, CATALOG, FilterKind, resolve};
    let fiscal = cl_core::fiscal::FiscalYearSpec::CALENDAR_YEAR;
    let acc = cl_core::ids::AccountId::parse("11111111-1111-4111-8111-111111111111").expect("id");
    let con = cl_core::ids::ContactId::parse("22222222-2222-4222-8222-222222222222").expect("id");
    let tag = cl_core::ids::TagId::parse("33333333-3333-4333-8333-333333333333").expect("id");
    let item = cl_core::ids::ItemId::parse("44444444-4444-4444-8444-444444444444").expect("id");
    let query = |req: &cl_core::report::ReportRequest| {
        run::exact_query(req)
            .or_else(|_| list_run::query(req))
            .unwrap_or_default()
    };
    for spec in CATALOG {
        let mut base = ReportOptions::for_spec(spec);
        base.column_tag_group_id = Some(
            cl_core::ids::TagGroupId::parse("55555555-5555-4555-8555-555555555555").expect("id"),
        );
        // The pickers a report waits for.
        if spec.filters.contains(&FilterKind::Vendor) {
            base.filters.contact_ids = vec![con];
        }
        if spec.filters.contains(&FilterKind::BankAccount) {
            base.filters.account_ids = vec![acc];
        }
        for k in spec.filters {
            let mut o = base.clone();
            match k {
                FilterKind::Vendor | FilterKind::BankAccount | FilterKind::Budget => continue,
                FilterKind::Accounts => o.filters.account_ids = vec![acc],
                FilterKind::Contacts => o.filters.contact_ids = vec![con],
                FilterKind::Tags => o.filters.tag_ids = vec![tag],
                FilterKind::Items => o.filters.item_ids = vec![item],
                FilterKind::Amount => {
                    o.filters.amount = Some(AmountFilter {
                        min: Some("10.0000".into()),
                        max: None,
                    });
                }
                FilterKind::Cleared => o.filters.cleared = Some(cl_core::report::Cleared::Cleared),
                FilterKind::TxnTypes => o.filters.txn_types = vec!["expense".into()],
                FilterKind::Adjusting => {
                    o.filters.adjusting = Some(cl_core::report::Adjusting::Exclude);
                }
            }
            let req = resolve(spec, &o, &fiscal, today()).expect(spec.key);
            assert_eq!(
                run::missing_capability(&req, |_| true),
                None,
                "{} {k:?} needs no engine update",
                spec.key
            );
            let before = resolve(spec, &base, &fiscal, today()).expect(spec.key);
            let changed = query(&req) != query(&before)
                // Item filters go to the inventory route's own argument,
                // the aging details narrow their rows by name.
                || (*k == FilterKind::Items && spec.source == Source::InventoryValuationDetail)
                || (*k == FilterKind::Contacts
                    && matches!(spec.source, Source::ArAgingDetail | Source::ApAgingDetail));
            assert!(changed, "{} {k:?} reaches the route", spec.key);
        }
    }
}

/// S3: a payee or memo that starts with `@` shows as typed; app keys still
/// translate.
#[test]
fn owner_text_starting_with_at_is_not_a_key() {
    assert_eq!(label("@Home Depot"), "@Home Depot");
    assert_eq!(label("@home"), "@home");
    assert_eq!(label("Plain"), "Plain");
    assert_ne!(label("@reports.empty"), "@reports.empty");
}

/// QA-7: an aging hides a name whose every column is zero (a test, not a
/// sum); totals and headers always stay.
#[test]
fn all_zero_rows_are_names_with_nothing_open() {
    use cl_core::report::{ReportRow, RowPath};
    let row = |kind, cells| ReportRow {
        kind,
        level: 0,
        path: RowPath::of(["r"]),
        caption: "Branson Hills Inn".into(),
        cells,
        drill: None,
        expandable: false,
    };
    let z = || Cell::Amount(rust_decimal::Decimal::ZERO);
    assert!(super::window::all_zero(&row(
        RowKind::Detail,
        vec![z(), z()]
    )));
    assert!(!super::window::all_zero(&row(
        RowKind::Detail,
        vec![z(), Cell::Amount(rust_decimal::Decimal::new(-567_827, 2))]
    )));
    assert!(!super::window::all_zero(&row(
        RowKind::Grand,
        vec![z(), z()]
    )));
    assert!(!super::window::all_zero(&row(
        RowKind::Detail,
        vec![Cell::Text("x".into())]
    )));
}
