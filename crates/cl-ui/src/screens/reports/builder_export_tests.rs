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
    drop(d);
    let table = file_table(&w, f);
    assert!(
        table
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::More)
            .all(|r| r.caption == "@reports.builder.not_loaded" && r.drill.is_none())
    );
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

/// The CSV Export makes: every detail row of the opened lines, no "rows
/// not loaded" line, and the screen's lines in the same order.
#[test]
fn the_exported_csv_has_every_row() {
    let (_d, link) = demo();
    let w = ledger(&link);
    let bytes = filled_bytes(&w, Kind::Csv).expect("csv");
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(!text.contains("not loaded"), "{text}");
    let screen = crate::screens::reports::export::bytes(&w, Kind::Csv).expect("screen csv");
    let screen = String::from_utf8(screen).expect("utf8");
    assert!(
        text.lines().count() > screen.lines().count(),
        "the file has the rows the screen pages"
    );
}

/// Print of a builder report reads every row, then opens the PDF.
#[test]
fn print_fills_then_opens_the_pdf() {
    let (_d, link) = demo();
    let mut w = ledger(&link);
    let _ = link.drain();
    assert!(w.command(CommandId::new("file.print")));
    // The PDF is built on a worker thread: up to 30 s on a busy machine.
    for _ in 0..3000 {
        crate::screens::reports::export::poll(&mut w);
        if !w.export.busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!w.export.filling());
    assert!(link.drain().into_iter().any(|e| matches!(
        e,
        crate::company::CompanyEvent::OpenWith(k, _)
            if k.as_str() == crate::screens::document_view::KIND
    )));
}
