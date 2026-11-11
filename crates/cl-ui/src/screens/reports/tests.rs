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

/// Q-220: the P&L needs `reports-exact-v1`; the window says so and runs
/// nothing.
#[test]
fn a_report_the_engine_cant_run_yet_says_so() {
    let (_d, link) = demo();
    let w = window(&link);
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
    assert!(csv.starts_with(",Current,"), "{csv}");
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
