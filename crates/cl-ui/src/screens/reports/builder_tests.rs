//! A07 fix pass: Report Builder window tests on the demo company (review
//! A07 #3, #4, #7, #8, #10, #15, #17; security S-L2, S-L7).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::report::{Drill, ReportTable, RowKind, RowPath};
use cl_model::CommandId;
use cl_model::reports::find;
use eframe::egui;

use super::window::{ReportWindow, RunState};
use super::*;
use crate::company::CompanyLink;
use crate::windows::{OpenPayload, Screen};

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn window(link: &Arc<CompanyLink>) -> ReportWindow {
    let spec = find("profit_loss").expect("p&l");
    ReportWindow::new(Arc::clone(link), CommandId::new(KIND), None, spec, today())
}

fn starter(link: &Arc<CompanyLink>, key: &str) -> ReportWindow {
    let mut w = window(link);
    w.poll();
    assert!(w.open_with(OpenPayload::BuilderStarter(key.into())));
    w.poll();
    assert_eq!(w.state, RunState::Shown, "{key}: {:?}", w.problem);
    w
}

fn count(t: &ReportTable, k: RowKind) -> usize {
    t.rows.iter().filter(|r| r.kind == k).count()
}

/// The detail tab of the first line with a `more` row in the ledger
/// starter (a line with more than one demo page of posts).
fn ledger_line_tab(link: &Arc<CompanyLink>) -> (ReportWindow, ReportWindow) {
    let w = starter(link, "transaction_detail_by_account");
    let t = w.table.as_ref().expect("table");
    let more = t
        .rows
        .iter()
        .find(|r| r.kind == RowKind::More)
        .expect("a demo line with more than five posts");
    let path = more.path.clone();
    let group = t
        .rows
        .iter()
        .find(|r| r.kind == RowKind::Group && r.path == path)
        .expect("its line");
    let Some(Drill::BuilderDetail { node_id, path, .. }) = group.drill.clone() else {
        panic!("a line drills to its detail");
    };
    let open = super::builder_run::detail_open_for_test(&w, &node_id, &path);
    let mut tab = window(link);
    tab.poll();
    assert!(tab.open_with(OpenPayload::BuilderDetail(Box::new(open))));
    tab.poll();
    assert_eq!(tab.state, RunState::Shown, "{:?}", tab.error);
    (w, tab)
}

/// #7: an expand by keyboard (no input after it) asks for a frame when
/// the debounce ends, and the window stays busy until the run is sent.
#[test]
fn an_expand_asks_for_the_frame_that_sends_its_run() {
    let (_d, link) = demo();
    let mut w = starter(&link, "sales_and_purchases");
    let opened = w
        .table
        .as_ref()
        .expect("table")
        .rows
        .iter()
        .find(|r| r.kind == RowKind::Group && r.expandable)
        .expect("a client line")
        .path
        .clone();
    let ctx = egui::Context::default();
    let mut out = ctx.run_ui(
        egui::RawInput {
            time: Some(1.0),
            ..Default::default()
        },
        |ui| super::builder_run::toggle(&mut w, &opened, ui),
    );
    out.textures_delta.clear();
    let delay = out
        .viewport_output
        .get(&egui::ViewportId::ROOT)
        .map(|v| v.repaint_delay)
        .expect("root viewport");
    assert!(
        delay <= std::time::Duration::from_millis(150),
        "a repaint is asked for (got {delay:?})"
    );
    assert!(
        w.builder.as_ref().expect("builder").busy(),
        "busy while the debounce runs"
    );
    // The frame after the debounce sends the run, with no input.
    let mut out = ctx.run_ui(
        egui::RawInput {
            time: Some(1.2),
            ..Default::default()
        },
        |ui| super::builder_run::tick(&mut w, ui),
    );
    out.textures_delta.clear();
    w.poll();
    assert!(
        count(w.table.as_ref().expect("table"), RowKind::Detail) > 0,
        "the expand ran"
    );
}

/// #17, #10: Load more pages a line's rows in place (the `more` row is
/// required here), and a file made while rows are missing says so.
#[test]
fn load_more_adds_rows_and_a_file_says_rows_were_not_loaded() {
    let (_d, link) = demo();
    let mut w = starter(&link, "transaction_detail_by_account");
    let t = w.table.clone().expect("table");
    let more = t
        .rows
        .iter()
        .find(|r| r.kind == RowKind::More)
        .cloned()
        .expect("a demo line with more than five posts");
    let snap = super::export::snapshot(&w).expect("exportable");
    let missing: Vec<&str> = snap
        .table()
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::More)
        .map(|r| r.caption.as_str())
        .collect();
    assert_ne!(missing.len(), 0, "a more row");
    assert!(
        missing.iter().all(|c| *c == "@reports.builder.not_loaded"),
        "{missing:?}"
    );
    let (details, mores) = (count(&t, RowKind::Detail), count(&t, RowKind::More));
    super::drill::open(&mut w, more.drill.as_ref().expect("page"));
    w.poll();
    let s = w.state_json_for_test();
    let after = w.table.as_ref().expect("table");
    assert!(count(after, RowKind::Detail) > details, "rows were added");
    assert!(
        count(after, RowKind::More) <= mores,
        "the more row moved or went"
    );
    assert_eq!(
        s["builder"]["detail_rows"],
        serde_json::json!(count(after, RowKind::Detail))
    );
}

/// #3, S-L1: a builder report can't be memorized from the toolbar or the
/// dialog, never marks the tab dirty, and the demo refuses a kind change.
#[test]
fn a_builder_report_is_never_memorized_as_a_standard_one() {
    let (_d, link) = demo();
    let mut w = starter(&link, "profit_loss");
    super::memorized::open_dialog(&mut w);
    assert!(!w.memo.dialog_open(), "the dialog doesn't open");
    assert!(w.command(CommandId::new("reports.memorize")));
    assert!(!w.memo.dialog_open());
    // A memorized builder report: no dirty dot, and a forced save is
    // refused before anything is sent.
    let body = crate::company::demo::report_builder::starters()
        .expect("starters")
        .into_iter()
        .find(|s| s.key == "profit_loss")
        .expect("p&l")
        .definition;
    let write = cl_api::resources::reports::definitions::DefinitionWrite {
        name: "My P&L".into(),
        group_id: None,
        kind: "builder".into(),
        definition: body,
    };
    let row = {
        let mut st = link.demo_state().expect("demo");
        crate::company::demo::report_store::save_definition(&mut st, None, &write).expect("saved")
    };
    super::memorized::open_row(&mut w, &row);
    w.poll();
    assert!(w.builder.is_some());
    let mut o = w.options.clone();
    o.dates = cl_model::reports::DateChoice::Preset(cl_model::reports::DatePreset::LastYear);
    w.set_options(o);
    assert!(!super::memorized::changed(&w), "never dirty");
    w.memo.dialog = Some(super::memorized::Dialog::named("My P&L"));
    super::memorized::save(&mut w, super::memorized::SaveAs::Changes);
    assert_eq!(
        w.memo.dialog.as_ref().and_then(|d| d.problem),
        Some("reports.memorize.invalid")
    );
    // The demo store, like the engine, refuses to turn it standard.
    let standard = cl_api::resources::reports::definitions::DefinitionWrite {
        name: "My P&L".into(),
        group_id: None,
        kind: "standard".into(),
        definition: serde_json::to_value(cl_model::reports::StandardDefinition::new(
            "profit_loss",
            cl_model::reports::ReportOptions::for_spec(find("profit_loss").expect("p&l")),
        ))
        .expect("json"),
    };
    let mut st = link.demo_state().expect("demo");
    assert!(
        crate::company::demo::report_store::save_definition(
            &mut st,
            Some((row.id.clone(), row.version)),
            &standard
        )
        .is_err()
    );
}

/// #4, #15: after the books change, a detail tab's Refresh runs the
/// report again for a fresh stamp and recovers; a page asked before the
/// Refresh is dropped.
#[test]
fn a_detail_tab_recovers_after_the_books_change() {
    let (_d, link) = demo();
    let (_w, mut tab) = ledger_line_tab(&link);
    let first = count(tab.table.as_ref().expect("table"), RowKind::Detail);
    // Ask the next page, then Refresh before it lands.
    let more = tab
        .table
        .as_ref()
        .expect("table")
        .rows
        .iter()
        .find(|r| r.kind == RowKind::More)
        .cloned()
        .expect("the tab pages");
    super::drill::open(&mut tab, more.drill.as_ref().expect("page"));
    // The books change (a void), as any posting would.
    {
        let mut st = link.demo_state().expect("demo");
        let ids: Vec<_> = st
            .ledger
            .transactions()
            .iter()
            .map(crate::company::demo::register::DemoLedger::id_of)
            .collect();
        assert!(
            ids.into_iter().any(|id| st.ledger.void(id).is_ok()),
            "something voids"
        );
    }
    tab.run();
    tab.poll();
    tab.poll();
    let s = tab.state_json_for_test();
    assert_eq!(tab.state, RunState::Shown, "{:?}", tab.error);
    assert_eq!(s["builder"]["banner"], serde_json::Value::Null, "{s}");
    assert_eq!(s["builder"]["detail"]["pages"], 1, "page one again: {s}");
    assert!(count(tab.table.as_ref().expect("table"), RowKind::Detail) <= first);
}

/// #8: a memorized builder report saved by a newer version says to
/// update, and one that can't be read says so: never a blank window.
#[test]
fn an_unreadable_memorized_builder_report_says_why() {
    let (_d, link) = demo();
    let mut w = window(&link);
    w.poll();
    let row =
        |definition: serde_json::Value| cl_api::resources::reports::definitions::DefinitionRow {
            id: "r1".into(),
            group_id: None,
            name: "From the future".into(),
            kind: "builder".into(),
            definition,
            version: 1,
            created_by: None,
            created_by_name: None,
            updated_at: None,
            can_edit: true,
        };
    super::memorized::open_row(
        &mut w,
        &row(serde_json::json!({"schemaVersion": 2, "kind": "builder", "nodes": []})),
    );
    w.poll();
    assert_eq!(w.state, RunState::Failed);
    assert_eq!(w.problem, Some("reports.builder.newer"));
    super::memorized::open_row(
        &mut w,
        &row(serde_json::json!({"schemaVersion": 1, "kind": "builder", "nodes": "x"})),
    );
    w.poll();
    assert_eq!(w.state, RunState::Failed);
    assert_eq!(w.problem, Some("reports.builder.unreadable"));
    // Refresh says it again.
    w.run();
    assert_eq!(w.problem, Some("reports.builder.unreadable"));
}

/// S-L7: a detail tab of a section the role can't see says so, with no
/// "Try again".
#[test]
fn a_denied_detail_tab_says_so() {
    let (_d, link) = demo();
    let w = starter(&link, "sales_and_purchases");
    let open =
        super::builder_run::detail_open_for_test(&w, "purchases", &RowPath::of(["n:purchases"]));
    let mut tab = window(&link);
    tab.poll();
    tab.open_with(OpenPayload::BuilderDetail(Box::new(open)));
    tab.poll();
    assert_eq!(tab.problem, Some("reports.builder.denied"));
    assert!(tab.error.is_none(), "no Retry");
    assert_eq!(
        tab.state_json_for_test()["builder"]["banner"],
        serde_json::Value::Null
    );
}

/// S-L2: `@@` is text an adapter marked as never a key.
#[test]
fn a_literal_caption_shows_one_at() {
    assert_eq!(
        super::label("@@reports.builder.denied"),
        "@reports.builder.denied"
    );
    assert_eq!(
        super::label("@reports.builder.denied"),
        crate::i18n::t("reports.builder.denied")
    );
}

/// #16: the demo refuses a drill path that isn't the section or a prefix
/// of its group-bys, as the engine does (400 `INVALID_PARAM`).
#[test]
fn the_demo_refuses_a_path_the_engine_refuses() {
    let (_d, link) = demo();
    let w = starter(&link, "profit_loss");
    let open =
        super::builder_run::detail_open_for_test(&w, "income", &RowPath::of(["n:income", "tag:x"]));
    let mut tab = window(&link);
    tab.poll();
    tab.open_with(OpenPayload::BuilderDetail(Box::new(open)));
    tab.poll();
    assert_eq!(tab.state, RunState::Failed);
    assert!(tab.error.is_some());
}

/// #2: an aging definition offers no Columns choice (the engine refuses
/// period and compare ranges with aging buckets).
#[test]
fn an_aging_definition_has_no_columns_choice() {
    let (_d, link) = demo();
    let mut w = window(&link);
    w.poll();
    let definition = serde_json::json!({"schemaVersion": 1, "kind": "builder",
        "dates": {"mode": "asOf", "preset": "today"},
        "columns": {"detailFields": [], "compare": [], "headers": {}, "agingBuckets": true},
        "nodes": [{"type": "section", "id": "receivables", "caption": "Accounts Receivable",
            "source": "ar_open", "measure": "open_balance", "filters": [],
            "groupBy": [{"key": "contact"}], "display": "summary", "defaultExpanded": false,
            "showTotal": true, "totalCaption": "Total", "hideZero": true}]});
    let st = cl_api::resources::report_builder::Starter {
        key: "aging".into(),
        title: "Aging".into(),
        ties_to: None,
        definition,
    };
    super::builder_run::show_starter(&mut w, &st);
    assert_eq!(w.spec.columns, &[cl_model::reports::ColumnBy::Total]);
    assert_eq!(w.options.columns, cl_model::reports::ColumnBy::Total);
}

/// A07 QA-7, QA-11: every switch (standard to standard, standard to a
/// starter, a new detail tab) starts a new view: no "Saved ..." line from
/// the report before, scroll back at the top.
#[test]
fn a_new_report_starts_clean_and_at_the_top() {
    let (_d, link) = demo();
    let mut w = window(&link);
    w.poll();
    let first = w.view;
    super::export::set_message_for_test(&mut w, "Saved pl.csv.");
    let spec = find("balance_sheet").expect("bs");
    w.show_report(spec, cl_model::reports::ReportOptions::for_spec(spec));
    assert!(w.export.message().is_none(), "standard to standard");
    assert_ne!(w.view, first);
    w.poll();
    super::export::set_message_for_test(&mut w, "Saved bs.csv.");
    w.notice = Some("a notice".into());
    let before = w.view;
    assert!(w.open_with(OpenPayload::BuilderStarter("profit_loss".into())));
    w.poll();
    assert!(w.export.message().is_none(), "standard to a starter");
    assert!(w.notice.is_none());
    assert_ne!(w.view, before);
    assert_eq!(w.state_json_for_test()["message"], false);
    // Two windows never share a view (their scroll areas are their own).
    let (a, b) = ledger_line_tab(&link);
    assert_ne!(a.view, b.view);
}

/// A07 QA-7: an export that finishes after another report is shown still
/// writes its file, but doesn't say "Saved ..." over the new report.
#[test]
fn a_late_export_says_nothing_over_the_next_report() {
    let (_d, link) = demo();
    let dir = tempfile::tempdir().expect("tmp");
    let mut w = window(&link);
    w.poll();
    let path = dir.path().join("pl.csv");
    super::export::pending_for_test(
        &mut w,
        b"a
"
        .to_vec(),
        path.clone(),
    );
    assert!(w.open_with(OpenPayload::BuilderStarter("profit_loss".into())));
    w.poll();
    super::export::poll(&mut w);
    assert!(path.exists(), "the file is written");
    assert!(w.export.message().is_none(), "{:?}", w.export.message());
    // On the same report it is said.
    let path = dir.path().join("b.csv");
    super::export::pending_for_test(
        &mut w,
        b"a
"
        .to_vec(),
        path.clone(),
    );
    super::export::poll(&mut w);
    assert!(w.export.message().is_some_and(|m| m.contains("b.csv")));
}

/// A07 QA-9: the Collapse button says what it does, on a builder run and
/// on a standard report.
#[test]
fn the_collapse_button_says_what_it_does() {
    let (_d, link) = demo();
    let mut w = starter(&link, "sales_and_purchases");
    let before = w.collapse_label();
    w.toggle_collapse_all();
    w.poll();
    assert_ne!(w.collapse_label(), before, "builder");
    w.toggle_collapse_all();
    w.poll();
    assert_eq!(w.collapse_label(), before, "builder, back");
    let spec = find("sales_tax_liability").expect("spec");
    w.show_report(spec, cl_model::reports::ReportOptions::for_spec(spec));
    w.poll();
    assert_eq!(w.collapse_label(), "reports.tool.collapse");
    w.toggle_collapse_all();
    w.poll();
    assert_eq!(w.collapse_label(), "reports.tool.expand_all");
    assert_eq!(w.state_json_for_test()["collapse_label"], "expand_all");
    assert!(crate::i18n::has("reports.tool.expand_all"));
}
