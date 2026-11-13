//! Export and Print of a builder report on the demo engine (A09 S2,
//! Q-A09h): the export run asks for the screen's lists, every `more` row
//! with a cursor is filled, the cap leaves a "not loaded" line and a head
//! note, a failed page or Cancel writes nothing.

use std::sync::Arc;

use cl_api::resources::report_builder::RunPurpose;
use cl_core::CalendarDate;
use cl_model::CommandId;

use super::*;
use crate::company::CompanyLink;
use crate::screens::reports::export::{Kind, filled_bytes};
use crate::screens::reports::window::RunState;
use crate::windows::{OpenPayload, Screen};

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

/// The ledger starter: lines with more than one demo page of posts, all
/// open ("detail"), so its run has `more` rows with cursors.
fn ledger(link: &Arc<CompanyLink>) -> ReportWindow {
    let spec = cl_model::reports::find("profit_loss").expect("p&l");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let mut w = ReportWindow::new(
        Arc::clone(link),
        CommandId::new(crate::screens::reports::KIND),
        None,
        spec,
        today,
    );
    w.poll();
    assert!(w.open_with(OpenPayload::BuilderStarter(
        "transaction_detail_by_account".into()
    )));
    w.poll();
    assert_eq!(w.state, RunState::Shown);
    w
}

fn count(t: &ReportTable, k: RowKind) -> usize {
    t.rows.iter().filter(|r| r.kind == k).count()
}

#[test]
fn the_export_run_asks_for_the_screens_lists() {
    let (_d, link) = demo();
    let w = ledger(&link);
    let req = export_request(&w).expect("a builder report");
    assert_eq!(req.purpose, RunPurpose::Export);
    let (open, collapsed, all) = w.builder.as_ref().expect("b").state.shown_lists();
    assert_eq!(
        (req.open, req.collapsed, req.expand_all),
        (open, collapsed, all)
    );
}

/// Every `more` row with a cursor is paged in; the file has no `more` row
/// and more detail rows than the screen's first pages.
#[test]
fn every_more_row_is_filled() {
    let (_d, link) = demo();
    let w = ledger(&link);
    let screen = w.table.clone().expect("table");
    assert!(
        count(&screen, RowKind::More) > 0,
        "the demo pages its lines"
    );
    let req = export_request(&w).expect("req");
    let mut d = link.demo_state().expect("demo");
    let run = crate::company::demo::report_builder::run(&mut d, &req, false).expect("run");
    let first = count(&run.table, RowKind::Detail);
    let stop = AtomicBool::new(false);
    let f = fill_with(&req, run, MAX_EXPORT_ROWS, &stop, |dr, run| {
        crate::company::demo::report_builder::detail(&mut d, dr, &run.table.columns, &run.layout)
    })
    .expect("filled");
    assert!(!f.cut);
    assert_eq!(count(&f.run.table, RowKind::More), 0);
    assert!(count(&f.run.table, RowKind::Detail) > first);
}

/// At the cap a `more` row stays: the file says rows are missing.
#[test]
fn the_cap_leaves_a_not_loaded_line() {
    let (_d, link) = demo();
    let w = ledger(&link);
    let req = export_request(&w).expect("req");
    let mut d = link.demo_state().expect("demo");
    let run = crate::company::demo::report_builder::run(&mut d, &req, false).expect("run");
    let first = count(&run.table, RowKind::Detail);
    let stop = AtomicBool::new(false);
    let f = fill_with(&req, run, first + 1, &stop, |dr, run| {
        crate::company::demo::report_builder::detail(&mut d, dr, &run.table.columns, &run.layout)
    })
    .expect("filled");
    assert!(f.cut);
    assert_eq!(
        count(&f.run.table, RowKind::Detail),
        first + 1,
        "the page that passed the cap was trimmed to it (minor 4)"
    );
    drop(d);
    let table = file_table(&FileCtx::of(&w), f.clone());
    assert!(
        table
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::More)
            .all(|r| r.caption == "@reports.builder.not_loaded" && r.drill.is_none())
    );
    // The file's head says it is cut (#10).
    let csv = crate::screens::reports::export::filled_file(&w, f, Kind::Csv).expect("csv");
    let csv = String::from_utf8(csv).expect("utf8");
    let note = crate::i18n::t_with(
        "reports.export.cut",
        &[("n", &crate::i18n::count_text(MAX_EXPORT_ROWS as u64))],
    );
    let head: Vec<&str> = csv.lines().take(6).collect();
    assert!(head.iter().any(|l| l.contains(&note)), "{head:?}");
}

/// The export as the window runs it, with a page that fails: no file is
/// written and the message says why (#10).
#[test]
fn a_failed_page_writes_no_file_and_says_why() {
    let (dir, link) = demo();
    let mut w = ledger(&link);
    let path = dir.path().join("out.csv");
    crate::screens::reports::export::fill_for_test(&mut w, path.clone(), |_, _| {
        Err(ApiError::Local(cl_api::LocalValidation::InvalidValue))
    });
    for _ in 0..200 {
        crate::screens::reports::export::poll(&mut w);
        if !w.export.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!path.exists(), "nothing written");
    assert_eq!(
        w.export.message(),
        Some(crate::i18n::t("reports.export.fill_failed"))
    );
    assert_eq!(w.export.state()["ok"], false);
}

/// A page that fails fails the file; Cancel stops it.
#[test]
fn a_failed_page_or_cancel_makes_no_file() {
    let (_d, link) = demo();
    let w = ledger(&link);
    let req = export_request(&w).expect("req");
    let mut d = link.demo_state().expect("demo");
    let run = crate::company::demo::report_builder::run(&mut d, &req, false).expect("run");
    let stop = AtomicBool::new(false);
    let e = fill_with(&req, run.clone(), MAX_EXPORT_ROWS, &stop, |_, _| {
        Err(ApiError::Local(cl_api::LocalValidation::InvalidValue))
    });
    assert!(e.is_err());
    stop.store(true, Ordering::Relaxed);
    let e = fill_with(&req, run, MAX_EXPORT_ROWS, &stop, |_, _| {
        panic!("no page after Cancel")
    });
    assert!(matches!(e, Err(ApiError::Cancelled)));
}

/// The CSV Export makes: one line per row of the filled run (every detail
/// row of the opened lines, each once), no "rows not loaded" line, and the
/// screen's lines in the same order (#10).
#[test]
fn the_exported_csv_has_every_row() {
    let (_d, link) = demo();
    let w = ledger(&link);
    // The filled run, as the export fills it.
    let req = export_request(&w).expect("req");
    let mut d = link.demo_state().expect("demo");
    let run = crate::company::demo::report_builder::run(&mut d, &req, false).expect("run");
    let stop = AtomicBool::new(false);
    let f = fill_with(&req, run, MAX_EXPORT_ROWS, &stop, |dr, run| {
        crate::company::demo::report_builder::detail(&mut d, dr, &run.table.columns, &run.layout)
    })
    .expect("filled");
    drop(d);
    let table = file_table(&FileCtx::of(&w), f);
    let bytes = filled_bytes(&w, Kind::Csv).expect("csv");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(!text.contains("not loaded"), "{text}");
    // Every detail row is a line of its own, once.
    for r in table
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Detail && !r.caption.starts_with('@'))
    {
        assert!(
            text.contains(r.caption.as_str()),
            "{} is in the file",
            r.caption
        );
    }
    let screen = crate::screens::reports::export::bytes(&w, Kind::Csv).expect("screen csv");
    let screen = String::from_utf8(screen).expect("utf8");
    let shown = crate::screens::reports::export::snapshot(&w)
        .expect("snapshot")
        .table()
        .rows
        .len();
    let head = screen.lines().count() - shown;
    assert_eq!(
        text.lines().count(),
        head + table.rows.len(),
        "the head, then one line per row of the filled run"
    );
    // The screen's lines (not its pages) come in the same order.
    let lines: Vec<&str> = table
        .rows
        .iter()
        .filter(|r| r.kind != RowKind::Detail && r.kind != RowKind::More)
        .map(|r| r.caption.as_str())
        .filter(|c| !c.is_empty() && !c.starts_with('@'))
        .collect();
    let mut at = 0;
    for l in &lines {
        let found = text[at..].find(l).map(|i| i + at);
        assert!(found.is_some(), "{l} in order");
        at = found.unwrap_or(at);
    }
}

/// Print of a builder report reads every row, then opens the PDF.
#[test]
fn print_fills_then_opens_the_pdf() {
    let (_d, link) = demo();
    let mut w = ledger(&link);
    let _ = link.drain();
    assert!(w.command(CommandId::new("file.print")));
    // The pages are read and the PDF built on worker threads. The run alone
    // takes ~40 s in a debug build, so wait on a wall-clock deadline (a
    // loaded CI runner is several times slower), not a fixed poll count.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    while std::time::Instant::now() < deadline {
        crate::screens::reports::export::poll(&mut w);
        if !w.export.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!w.export.busy(), "the print finished");
    assert!(link.drain().into_iter().any(|e| matches!(
        e,
        crate::company::CompanyEvent::OpenWith(k, _)
            if k.as_str() == crate::screens::document_view::KIND
    )));
}

/// S-L8: a section the role can't see exports as its "denied" row with no
/// figures, and a formula that needs it says "—", in CSV, Excel and PDF
/// (Print builds the same PDF).
#[test]
fn a_denied_section_exports_no_figures() {
    let (_d, link) = demo();
    let spec = cl_model::reports::find("profit_loss").expect("p&l");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("d");
    let mut w = ReportWindow::new(
        Arc::clone(&link),
        CommandId::new(crate::screens::reports::KIND),
        None,
        spec,
        today,
    );
    w.poll();
    assert!(w.open_with(OpenPayload::BuilderStarter("sales_and_purchases".into())));
    w.poll();
    assert_eq!(w.state, RunState::Shown);
    let req = export_request(&w).expect("req");
    let mut d = link.demo_state().expect("demo");
    let run = crate::company::demo::report_builder::run(&mut d, &req, false).expect("run");
    let stop = AtomicBool::new(false);
    let f = fill_with(&req, run, MAX_EXPORT_ROWS, &stop, |dr, run| {
        crate::company::demo::report_builder::detail(&mut d, dr, &run.table.columns, &run.layout)
    })
    .expect("filled");
    drop(d);
    let table = file_table(&FileCtx::of(&w), f.clone());
    let denied: Vec<_> = table
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Denied)
        .collect();
    assert!(!denied.is_empty(), "the denied row is in the file");
    for r in &denied {
        assert!(
            r.cells
                .iter()
                .all(|c| !matches!(c, cl_core::report::Cell::Amount(_))),
            "no figures for the denied section"
        );
    }
    let formula = table
        .rows
        .iter()
        .find(|r| r.kind == RowKind::Formula)
        .expect("formula");
    assert_eq!(
        formula.cells.last(),
        Some(&cl_core::report::Cell::Text("\u{2014}".into()))
    );
    let csv = String::from_utf8(
        crate::screens::reports::export::filled_file(&w, f.clone(), Kind::Csv).expect("csv"),
    )
    .expect("utf8");
    assert!(csv.contains('\u{2014}'), "{csv}");
    assert!(crate::screens::reports::export::filled_file(&w, f.clone(), Kind::Xlsx).is_some());
    assert!(crate::screens::reports::export::filled_file(&w, f, Kind::Pdf).is_some());
}
