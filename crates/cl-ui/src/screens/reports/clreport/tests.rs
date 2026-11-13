#![allow(clippy::many_single_char_names, reason = "short test names")]
//! `.clreport` export and import on the demo engine (A09 WP-A): the review's
//! state machine, single versus several reports, Cancel writes nothing,
//! Rename / Replace by the rules, and Replace's verified backup copy.

use std::sync::Arc;

use cl_api::resources::reports::definitions::{
    DefinitionPatch, DefinitionWrite, NameTaken, PreviewReport, TransferReport,
};
use serde_json::{Value, json};

use super::*;
use crate::company::CompanyLink;
use crate::company::demo::report_store;

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

/// A valid builder definition over two demo expense accounts and a tag
/// group.
fn builder_def(link: &CompanyLink) -> Value {
    let mut d = link.demo_state().expect("demo");
    let accounts: Vec<String> = crate::company::demo::report_transfer::pick_list(&mut d, "account")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let tg = crate::company::demo::report_transfer::pick_list(&mut d, "tagGroup")[0]
        .0
        .clone();
    drop(d);
    let chart = link
        .demo_state()
        .and_then(|d| d.chart.clone())
        .unwrap_or_else(crate::company::demo::chart);
    let expenses: Vec<String> = chart
        .iter()
        .filter(|a| a.fields.account_type == cl_api::AccountType::Expense)
        .map(|a| a.id.to_string())
        .filter(|id| accounts.contains(id))
        .take(2)
        .collect();
    json!({
        "schemaVersion": 1, "kind": "builder", "title": "Shop costs",
        "dates": {"mode": "range"},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "s1", "caption": "Shop costs", "source": "gl",
            "measure": "amount", "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": expenses},
                        {"field": "amount", "op": "gte", "value": "1250.5000"}],
            "groupBy": [{"key": "tag", "tagGroupId": tg}], "display": "summary",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    })
}

fn memorize(
    link: &CompanyLink,
    name: &str,
    kind: &str,
    def: Value,
    group: Option<String>,
) -> DefinitionRow {
    let w = DefinitionWrite {
        name: name.into(),
        group_id: group,
        kind: kind.into(),
        definition: def,
        origin: None,
    };
    let mut d = link.demo_state().expect("demo");
    report_store::save_definition(&mut d, None, &w).expect("save")
}

fn count(link: &CompanyLink) -> usize {
    let mut d = link.demo_state().expect("demo");
    report_store::definitions(&mut d).0.len()
}

fn pump(w: &mut ReportFileWindow) {
    for _ in 0..50 {
        w.poll_export();
        w.poll_import();
        if !matches!(w.stage, Stage::Reading(_) | Stage::Starting(..)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    // A preview the demo answered at once.
    w.poll_import();
}

/// Exports `target` to `path` through the window.
fn export_to(link: &Arc<CompanyLink>, target: Target, path: &Path) {
    let mut w = ReportFileWindow::new(Arc::clone(link));
    assert!(w.open_with(OpenPayload::ReportFile(Box::new(Start::Export(target)))));
    w.answer_save(Some(path.to_path_buf()));
    w.poll_export();
    assert_eq!(w.stage_name(), "done", "{:?}", w.message);
    assert_eq!(w.last, Some("exported"));
}

fn import(link: &Arc<CompanyLink>, path: &Path) -> ReportFileWindow {
    let mut w = ReportFileWindow::new(Arc::clone(link));
    assert!(w.open_with(OpenPayload::ReportFile(Box::new(Start::Import))));
    w.read_path(path.to_path_buf());
    pump(&mut w);
    w
}

// ------------------------------------------------------------ the review

fn preview(name: &str, ready: bool, taken: Option<(bool, &str)>) -> PreviewReport {
    PreviewReport {
        name: name.into(),
        kind: "builder".into(),
        ready,
        definition: json!({}),
        unresolved: Vec::new(),
        dropped: Vec::new(),
        group_id: None,
        name_taken: taken.map(|(can_edit, kind)| NameTaken {
            id: "d1".into(),
            version: 3,
            kind: kind.into(),
            can_edit,
        }),
        problem: None,
    }
}

fn review_of(names: &[&str]) -> Review {
    let file: Vec<_> = names
        .iter()
        .map(|n| cl_model::report_def::clreport::FileReport {
            name: (*n).into(),
            group: None,
            kind: "builder".into(),
            definition: json!({"kind": "builder", "schemaVersion": 1}),
            references: vec![json!({"kind": "budget", "name": "Plan", "fiscalYear": 2026})],
        })
        .collect();
    Review::new(&file, vec!["Shop costs".into(), "Shop costs (2)".into()])
}

#[test]
fn continue_waits_for_every_report_and_every_clash() {
    let mut r = review_of(&["Shop costs", "Fuel"]);
    assert!(!r.can_continue(), "no preview yet");
    r.take(vec![
        preview("Shop costs", true, Some((true, "builder"))),
        preview("Fuel", false, None),
    ]);
    assert!(!r.can_continue(), "Fuel isn't ready");
    // The clash defaults to Rename with the next free name.
    assert_eq!(r.clashes[0], Clash::Rename("Shop costs (3)".into()));
    r.take(vec![
        preview("Shop costs", true, Some((true, "builder"))),
        preview("Fuel", true, None),
    ]);
    assert!(r.can_continue());
    // A Rename to a taken name, or to nothing, stops it.
    r.set_clash(0, Clash::Rename("shop COSTS".into()));
    assert_eq!(r.clash_problem(0), Some("report_file.rename.taken"));
    assert!(!r.can_continue());
    r.set_clash(0, Clash::Rename("Fuel".into()));
    assert_eq!(
        r.clash_problem(0),
        Some("report_file.rename.taken"),
        "another report of the file"
    );
    r.set_clash(0, Clash::Rename("  ".into()));
    assert_eq!(r.clash_problem(0), Some("report_file.rename.empty"));
    r.set_clash(0, Clash::Replace);
    assert!(r.can_continue());
    let plan = r.plan().expect("plan");
    assert_eq!(plan[0].replace, Some(("d1".into(), 3)));
    assert_eq!(plan[1].replace, None);
    // A budget is never left out.
    assert!(!r.can_leave_out(0, "$ref:0"));
}

#[test]
fn replace_is_offered_only_to_the_author_and_for_the_same_kind() {
    let mut r = review_of(&["Shop costs"]);
    r.take(vec![preview("Shop costs", true, Some((false, "builder")))]);
    assert_eq!(
        Review::replace_refused(&r.reports[0]),
        Some(review::REPLACE_NOT_AUTHOR)
    );
    r.set_clash(0, Clash::Replace);
    assert!(
        matches!(r.clashes[0], Clash::Rename(_)),
        "refused: Rename stays"
    );
    r.take(vec![preview("Shop costs", true, Some((true, "standard")))]);
    assert_eq!(
        Review::replace_refused(&r.reports[0]),
        Some(review::REPLACE_OTHER_KIND)
    );
    r.set_clash(0, Clash::Replace);
    assert_ne!(r.clashes[0], Clash::Replace);
    r.take(vec![preview("Shop costs", true, Some((true, "builder")))]);
    r.set_clash(0, Clash::Replace);
    assert_eq!(r.clashes[0], Clash::Replace);
    // The other user's report now: a kept Replace falls back to Rename.
    r.take(vec![preview("Shop costs", true, Some((false, "builder")))]);
    assert!(matches!(r.clashes[0], Clash::Rename(_)));
    assert!(i18n_has_every_key());
}

fn i18n_has_every_key() -> bool {
    [
        review::REPLACE_NOT_AUTHOR,
        review::REPLACE_OTHER_KIND,
        "report_file.rename.taken",
    ]
    .iter()
    .all(|k| crate::i18n::has(k))
}

// ------------------------------------------------------------ export

#[test]
fn an_export_writes_a_file_with_no_ids_that_reads_back() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("Shop costs.clreport");
    export_to(
        &link,
        Target::Saved {
            id: row.id.clone(),
            name: row.name.clone(),
        },
        &path,
    );
    let bytes = std::fs::read(&path).expect("written");
    assert!(!cl_model::report_def::clreport::has_uuid(
        &String::from_utf8_lossy(&bytes)
    ));
    let file = clreport::read(&bytes).expect("reads back");
    assert_eq!(file.reports.len(), 1);
    assert_eq!(file.reports[0].name, "Shop costs");
    assert_eq!(
        file.reports[0].definition["nodes"][0]["filters"][1]["value"],
        "1250.5000"
    );
}

#[test]
fn a_cancelled_save_dialog_writes_nothing_and_closes() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let mut w = ReportFileWindow::new(Arc::clone(&link));
    w.open_with(OpenPayload::ReportFile(Box::new(Start::Export(
        Target::Report(Box::new(row)),
    ))));
    w.answer_save(None);
    w.poll_export();
    assert!(w.take_close_request());
    assert_eq!(
        std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|x| x == "clreport"))
            .count(),
        0
    );
}

#[test]
fn an_empty_group_or_no_engine_support_says_so() {
    let (_d, link) = demo();
    let group = DefinitionGroup {
        id: "g".into(),
        name: "Empty".into(),
        sort: 0,
        version: 1,
    };
    assert_eq!(
        start_export(
            &link,
            Target::Group {
                group,
                reports: Vec::new()
            }
        ),
        Some("report_file.export.empty_group")
    );
    assert_eq!(start_import(&link), None);
    assert!(matches!(
        link.drain().as_slice(),
        [CompanyEvent::OpenWith(k, p)] if k.as_str() == KIND
            && matches!(**p, OpenPayload::ReportFile(ref s) if **s == Start::Import)
    ));
}

// ------------------------------------------------------------ import

#[test]
fn one_builder_report_opens_unsaved_in_the_builder_and_cancel_writes_nothing() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("one.clreport");
    export_to(&link, Target::Report(Box::new(row)), &path);
    let before = count(&link);

    // Cancel at the review: nothing written, the window closes.
    let groups = |link: &CompanyLink| {
        let mut d = link.demo_state().expect("demo");
        report_store::definitions(&mut d).1
    };
    let groups_before = groups(&link);
    let mut w = import(&link, &path);
    assert_eq!(w.stage_name(), "review", "{:?}", w.message);
    w.cancel();
    assert!(w.take_close_request());
    assert_eq!(count(&link), before);
    assert_eq!(groups(&link), groups_before, "no group made");
    assert!(!dir.path().join("Backups").exists(), "no backup copy");

    // Continue: the builder opens on it, unsaved, with origin Imported.
    let mut w = import(&link, &path);
    let r = w.review().expect("review");
    assert!(r.reports[0].ready);
    assert!(r.reports[0].name_taken.is_some(), "the same name is here");
    assert_eq!(r.clashes[0], Clash::Rename("Shop costs (2)".into()));
    let _ = link.drain();
    w.finish(false);
    assert_eq!(count(&link), before, "nothing saved yet");
    let events = link.drain();
    let seed = events.iter().find_map(|e| match e {
        CompanyEvent::OpenWith(k, p) if k.as_str() == super::super::builder::KIND => match &**p {
            OpenPayload::BuilderEditor(s) => Some((**s).clone()),
            _ => None,
        },
        _ => None,
    });
    assert!(
        matches!(seed, Some(super::super::builder::BuilderSeed::Imported { ref name, .. }) if name == "Shop costs (2)"),
        "{seed:?}"
    );
    assert!(w.take_close_request());
}

#[test]
fn one_standard_report_opens_in_a_report_window() {
    let (dir, link) = demo();
    let def = {
        let mut d = link.demo_state().expect("demo");
        report_store::definitions(&mut d).0[0].definition.clone()
    };
    let row = memorize(&link, "Monthly for the owner", "standard", def, None);
    let path = dir.path().join("std.clreport");
    export_to(&link, Target::Report(Box::new(row)), &path);
    // In another company the name is free: delete it here first.
    {
        let mut d = link.demo_state().expect("demo");
        let (rows, _) = report_store::definitions(&mut d);
        let r = rows
            .iter()
            .find(|r| r.name == "Monthly for the owner")
            .expect("row");
        report_store::delete_definition(&mut d, &r.id, r.version).expect("delete");
    }
    let mut w = import(&link, &path);
    let _ = link.drain();
    w.finish(false);
    let events = link.drain();
    assert!(
        events.iter().any(|e| matches!(e,
            CompanyEvent::OpenWith(k, p) if k.as_str() == super::super::KIND
                && matches!(&**p, OpenPayload::ImportedReport(o) if o.name == "Monthly for the owner"))),
        "{events:?}"
    );
}

/// A file of two reports: one with an account the company lacks (picked)
/// and a tag group it lacks (left out), the other clashing (Rename); then
/// Memorize all creates the file's group and both reports.
#[test]
fn several_reports_are_reviewed_then_memorized_all() {
    let (dir, link) = demo();
    let def = builder_def(&link);
    let a = memorize(&link, "Shop costs", "builder", def.clone(), None);
    let b = memorize(&link, "Fuel costs", "builder", def, None);
    let path = dir.path().join("two.clreport");
    let group = DefinitionGroup {
        id: "g".into(),
        name: "x".into(),
        sort: 0,
        version: 1,
    };
    // Export the two by ids, then edit the file as another company's.
    let mut d = link.demo_state().expect("demo");
    let mut out = crate::company::demo::report_transfer::export(
        &mut d,
        &ExportWhat::Ids(vec![a.id.clone(), b.id.clone()]),
    )
    .expect("export");
    drop(d);
    let _ = group;
    out[0].name = "Shop costs elsewhere".into();
    out[0].group_name = Some("From the accountant".into());
    out[1].group_name = Some("From the accountant".into());
    out[0].references[1]["number"] = json!("99999");
    out[0].references[2]["name"] = json!("No such group");
    let bytes = file_bytes(&out).expect("bytes");
    std::fs::write(&path, bytes).expect("write");
    let before = count(&link);

    let mut w = import(&link, &path);
    let r = w.review().expect("review").clone();
    assert!(!r.can_continue());
    assert_eq!(r.reports[0].unresolved.len(), 2);
    assert_eq!(r.clashes[1], Clash::Rename("Fuel costs (2)".into()));
    let pick = r.reports[0].unresolved[0].candidates[0].id.clone();
    w.choose(0, "$ref:1", Some(pick));
    w.poll_import();
    w.choose(0, "$ref:2", None);
    w.poll_import();
    let r = w.review().expect("review");
    assert!(r.can_continue(), "{:?}", r.reports);
    assert_eq!(r.reports[0].dropped[0].effect, "group_by_removed");
    w.finish(false);
    for _ in 0..10 {
        w.poll_import();
    }
    assert_eq!(w.stage_name(), "done", "{:?}", w.message);
    assert_eq!(count(&link), before + 2);
    let mut d = link.demo_state().expect("demo");
    let (rows, groups) = report_store::definitions(&mut d);
    let g = groups
        .iter()
        .find(|g| g.name == "From the accountant")
        .expect("the file's group made");
    for name in ["Shop costs elsewhere", "Fuel costs (2)"] {
        let row = rows.iter().find(|r| r.name == name).expect(name);
        assert_eq!(row.group_id.as_deref(), Some(g.id.as_str()));
    }
}

/// The report a Replace overwrites is grouped and its body differs from
/// the file's: the copy holds the old body, the report keeps its group and
/// its own name (letter case too), and gets the file's body (#1, #9, #15).
#[test]
fn replace_asks_then_writes_a_verified_copy_before_overwriting() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("one.clreport");
    // The file: the same report under another letter case, a new caption.
    {
        let mut d = link.demo_state().expect("demo");
        let mut out = crate::company::demo::report_transfer::export(
            &mut d,
            &ExportWhat::Ids(vec![row.id.clone()]),
        )
        .expect("export");
        out[0].name = "SHOP COSTS".into();
        out[0].definition["title"] = json!("SHOP COSTS");
        out[0].definition["nodes"][0]["caption"] = json!("From the file");
        std::fs::write(&path, file_bytes(&out).expect("bytes")).expect("write");
    }
    // The existing report is grouped, and is moved into the group after
    // the export (its version is what the preview reads).
    let row = {
        let mut d = link.demo_state().expect("demo");
        let g = report_store::create_group(&mut d, "For the bank").expect("group");
        report_store::patch_definition(
            &mut d,
            &row.id,
            row.version,
            &DefinitionPatch {
                group_id: Some(Some(g.id)),
                ..DefinitionPatch::default()
            },
        )
        .expect("move")
    };
    let mut w = import(&link, &path);
    w.set_clash(0, Clash::Replace);
    w.finish(false);
    assert!(
        matches!(&w.stage, Stage::Review(rv) if rv.confirm),
        "the confirm comes first"
    );
    let copies = dir.path().join("Backups").join(BACKUP_SUBFOLDER);
    assert!(!copies.exists(), "nothing written before the yes");
    w.finish(true);
    for _ in 0..10 {
        w.poll_import();
    }
    // The copy is there and reads as the old report.
    let files: Vec<_> = std::fs::read_dir(&copies)
        .expect("copies")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(files.len(), 1);
    let copy = clreport::read(&std::fs::read(files[0].path()).expect("read")).expect("valid");
    assert_eq!(copy.reports[0].name, "Shop costs");
    assert_eq!(
        copy.reports[0].definition["nodes"][0]["caption"], "Shop costs",
        "the copy holds the old body"
    );
    assert_eq!(copy.reports[0].group.as_deref(), Some("For the bank"));
    // Then the report was overwritten (a new version), and it opens.
    let mut d = link.demo_state().expect("demo");
    let now = report_store::definition(&mut d, &row.id).expect("row");
    assert!(now.version > row.version);
    assert_eq!(now.definition["nodes"][0]["caption"], "From the file");
    assert_eq!(now.group_id, row.group_id, "still in its group");
    assert_eq!(now.name, "Shop costs", "its own name, as written");
    assert_eq!(now.definition["title"], "Shop costs", "Q-A08r");
    drop(d);
    assert_eq!(w.last, Some("replaced"));
    assert!(w.take_close_request());
}

#[test]
fn a_bad_file_says_why_and_offers_another() {
    let (dir, link) = demo();
    let path = dir.path().join("bad.clreport");
    std::fs::write(&path, b"{\"format\": \"something\"}").expect("write");
    let w = import(&link, &path);
    assert_eq!(w.stage_name(), "failed");
    assert_eq!(w.last, Some("report_file.error.not_report_file"));
    let big = dir.path().join("big.clreport");
    std::fs::write(&big, vec![b' '; clreport::MAX_FILE_BYTES + 1]).expect("write");
    let w = import(&link, &big);
    assert_eq!(w.last, Some("report_file.error.too_large"));
    let newer = dir.path().join("newer.clreport");
    std::fs::write(
        &newer,
        serde_json::to_vec(&json!({"format": "clientledger-report", "schemaVersion": 2}))
            .expect("j"),
    )
    .expect("write");
    let w = import(&link, &newer);
    assert_eq!(w.last, Some("report_file.error.newer"));
}

#[test]
fn transfer_reports_map_to_file_reports_and_back() {
    let t = TransferReport {
        name: "x".into(),
        group_name: Some("g".into()),
        kind: "standard".into(),
        definition: json!({}),
        references: vec![],
    };
    let f = review::file_report_of(&t);
    assert_eq!(f.group.as_deref(), Some("g"));
    assert_eq!(review::transfer_of(&f), t);
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/report_files")
        .join(name)
}

/// The scenario's file (`report_file.yaml`): the demo matches Fuel by
/// number, offers 6350 for a 6390 it lacks, finds no "Job site" group, and
/// already has "Monthly P&L for the bank".
#[test]
fn the_scenario_fixture_reviews_as_the_scenario_expects() {
    let (_d, link) = demo();
    let w = import(&link, &fixture("from_accountant.clreport"));
    let r = w.review().expect("review");
    assert_eq!(r.reports.len(), 2);
    assert_eq!(r.reports[0].unresolved.len(), 2);
    assert_eq!(r.reports[0].unresolved[0].candidates.len(), 1);
    assert!(
        r.reports[0].unresolved[0].candidates[0]
            .label
            .starts_with("6350")
    );
    assert_eq!(r.reports[0].unresolved[1].candidates.len(), 0);
    assert!(r.reports[1].ready);
    assert_eq!(
        r.clashes[1],
        Clash::Rename("Monthly P&L for the bank (2)".into())
    );
    let w = import(&link, &fixture("one_report.clreport"));
    let r = w.review().expect("review");
    assert!(r.can_continue(), "{:?}", r.reports);
}

#[test]
fn start_over_takes_back_a_leave_out_the_save_check_refuses() {
    let (_d, link) = demo();
    let mut w = import(&link, &fixture("from_accountant.clreport"));
    w.choose(0, "$ref:2", None);
    w.poll_import();
    assert!(w.review().expect("review").has_choices(0));
    assert_eq!(w.review().expect("review").reports[0].dropped.len(), 1);
    w.start_over(0);
    w.poll_import();
    let r = w.review().expect("review");
    assert!(!r.has_choices(0));
    assert_eq!(r.reports[0].dropped.len(), 0);
    assert_eq!(
        r.reports[0].unresolved.len(),
        2,
        "matched again by the engine"
    );
}

#[test]
fn a_standard_import_opens_with_the_import_origin_and_its_name() {
    let (_d, link) = demo();
    let def = {
        let mut d = link.demo_state().expect("demo");
        report_store::definitions(&mut d).0[0].definition.clone()
    };
    let spec = cl_model::reports::find("profit_loss").expect("spec");
    let mut w = super::super::ReportWindow::new(
        Arc::clone(&link),
        CommandId::new(super::super::KIND),
        None,
        spec,
        cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("day"),
    );
    assert!(
        w.open_with(OpenPayload::ImportedReport(Box::new(ImportedOpen {
            definition: def,
            name: "From the file".into(),
            group_id: Some("g1".into()),
        })))
    );
    assert_eq!(w.memo.origin.as_deref(), Some("import"));
    let d = w.memo.dialog.as_ref().expect("Memorize dialog");
    assert_eq!(
        (d.name.as_str(), d.group.as_deref()),
        ("From the file", Some("g1"))
    );
    // Another report in that window is not an import.
    assert!(
        w.open_with(OpenPayload::Report(Box::new(super::super::ReportOpen {
            key: "balance_sheet".into(),
            options: None,
        })))
    );
    assert_eq!(w.memo.origin, None);
}

#[test]
fn a_replace_whose_copy_cant_be_written_replaces_nothing() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("one.clreport");
    export_to(&link, Target::Report(Box::new(row.clone())), &path);
    // The backup folder can't be made: a file has its name.
    std::fs::write(dir.path().join("Backups"), b"x").expect("block");
    let mut w = import(&link, &path);
    w.set_clash(0, Clash::Replace);
    w.finish(false);
    w.finish(true);
    for _ in 0..10 {
        w.poll_import();
    }
    assert_eq!(w.stage_name(), "done");
    assert_eq!(w.last, Some("stopped"));
    assert!(
        w.message
            .as_deref()
            .is_some_and(|m| m.contains("backup folder"))
    );
    let mut d = link.demo_state().expect("demo");
    let now = report_store::definition(&mut d, &row.id).expect("row");
    assert_eq!(now.version, row.version, "not replaced");
}

/// The write check never refuses a real report: every builder starter and
/// every catalog report's default definition writes.
#[test]
fn every_starter_and_standard_report_writes_as_a_file() {
    let mut reports: Vec<TransferReport> = crate::company::demo::report_builder::starters()
        .expect("starters")
        .into_iter()
        .map(|s| TransferReport {
            name: s.key.clone(),
            group_name: None,
            kind: "builder".into(),
            definition: serde_json::to_value(&s.definition).expect("json"),
            references: Vec::new(),
        })
        .collect();
    for spec in cl_model::reports::CATALOG {
        reports.push(TransferReport {
            name: spec.key.into(),
            group_name: None,
            kind: "standard".into(),
            definition: serde_json::to_value(cl_model::reports::StandardDefinition::new(
                spec.key,
                cl_model::reports::ReportOptions::for_spec(spec),
            ))
            .expect("json"),
            references: Vec::new(),
        });
    }
    for r in &reports {
        assert!(file_bytes(std::slice::from_ref(r)).is_ok(), "{}", r.name);
    }
}

// ------------------------------------------------------------ fix pass

/// #6: a preview that fails after a choice keeps the review; the choice is
/// taken back, the card says why, and Try again asks once more.
#[test]
fn a_failed_preview_after_a_choice_keeps_the_review() {
    let (_d, link) = demo();
    let mut w = import(&link, &fixture("from_accountant.clreport"));
    let before = w.review().expect("review").clone();
    // An id this company doesn't have: the preview refuses it.
    w.choose(0, "$ref:1", Some("not-a-row".into()));
    w.poll_import();
    assert_eq!(w.stage_name(), "review", "{:?}", w.message);
    let Stage::Review(rv) = &w.stage else {
        unreachable!()
    };
    assert_eq!(rv.error, Some("report_file.error.engine"));
    assert!(rv.review.choices.is_empty(), "the choice was taken back");
    assert_eq!(rv.review.reports, before.reports, "the review stays");
    w.retry();
    w.poll_import();
    let Stage::Review(rv) = &w.stage else {
        unreachable!()
    };
    assert_eq!(rv.error, None);
}

/// Two memorized reports exported, then deleted (another company's file).
fn two_report_file(link: &Arc<CompanyLink>, path: &Path) {
    let def = builder_def(link);
    let a = memorize(link, "Shop costs", "builder", def.clone(), None);
    let b = memorize(link, "Fuel costs", "builder", def, None);
    let mut d = link.demo_state().expect("demo");
    let out = crate::company::demo::report_transfer::export(
        &mut d,
        &ExportWhat::Ids(vec![a.id.clone(), b.id.clone()]),
    )
    .expect("export");
    for r in [a, b] {
        let v = report_store::definition(&mut d, &r.id)
            .expect("row")
            .version;
        report_store::delete_definition(&mut d, &r.id, v).expect("delete");
    }
    drop(d);
    std::fs::write(path, file_bytes(&out).expect("bytes")).expect("write");
}

/// #7: a name taken meanwhile stops Memorize all after the first report;
/// the rest comes back to the review with its clash choices, and saving
/// again finishes without saving the first twice.
#[test]
fn a_name_taken_meanwhile_comes_back_to_the_review() {
    let (dir, link) = demo();
    let path = dir.path().join("two.clreport");
    two_report_file(&link, &path);
    let mut w = import(&link, &path);
    assert!(w.review().expect("review").can_continue());
    let before = count(&link);
    // Another window memorizes "Fuel costs" meanwhile.
    memorize(&link, "Fuel costs", "builder", builder_def(&link), None);
    w.finish(false);
    for _ in 0..10 {
        w.poll_import();
    }
    assert_eq!(w.last, Some("review_again"));
    let m = w.message.clone().unwrap_or_default();
    assert!(
        m.contains("Fuel costs") && m.contains("1 report memorized"),
        "{m}"
    );
    let r = w.review().expect("back in the review");
    assert_eq!(r.file.len(), 1, "the saved report left the review");
    assert_eq!(r.clashes[0], Clash::Rename("Fuel costs (2)".into()));
    w.finish(false);
    for _ in 0..10 {
        w.poll_import();
    }
    assert_eq!(w.last, Some("imported"), "{:?}", w.message);
    // The other window's report, Shop costs once, and Fuel costs (2).
    assert_eq!(count(&link), before + 3);
    let mut d = link.demo_state().expect("demo");
    let rows = report_store::definitions(&mut d).0;
    assert_eq!(rows.iter().filter(|r| r.name == "Shop costs").count(), 1);
    assert!(rows.iter().any(|r| r.name == "Fuel costs (2)"));
}

/// #7: a stop before anything was saved says nothing was memorized.
#[test]
fn a_stop_with_nothing_saved_says_so() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("one.clreport");
    export_to(&link, Target::Report(Box::new(row)), &path);
    std::fs::write(dir.path().join("Backups"), b"x").expect("block");
    let mut w = import(&link, &path);
    w.set_clash(0, Clash::Replace);
    w.finish(true);
    for _ in 0..10 {
        w.poll_import();
    }
    let m = w.message.clone().unwrap_or_default();
    assert!(m.contains("Nothing was memorized"), "{m}");
    assert!(!m.contains("before it were memorized"), "{m}");
}

/// #8: a standard report this build can't show fails in the Report File
/// window (which says so) instead of closing it with nothing opened.
#[test]
fn a_standard_report_that_cant_open_says_so_before_closing() {
    let (_d, link) = demo();
    let mut w = ReportFileWindow::new(Arc::clone(&link));
    let _ = link.drain();
    w.open_unsaved(&Planned {
        index: 0,
        name: "Odd".into(),
        group_id: None,
        new_group: None,
        kind: "standard".into(),
        definition: json!({"schemaVersion": 1, "kind": "standard", "reportKey": "no_such_report", "options": {}}),
        replace: None,
    });
    assert_eq!(w.stage_name(), "failed");
    assert_eq!(w.last, Some("report_file.error.invalid"));
    assert!(!w.take_close_request());
    assert!(link.drain().is_empty(), "nothing opened");
}

/// S-L2: a backup export that answers fewer reports than asked replaces
/// nothing.
#[test]
fn a_short_backup_answer_replaces_nothing() {
    let (_d, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let mut w = ReportFileWindow::new(Arc::clone(&link));
    let ids = vec![row.id.clone()];
    let what = ExportWhat::Ids(ids);
    let w2 = what.clone();
    let mut s = Saving {
        plan: Vec::new(),
        groups: BTreeMap::new(),
        backed_up: false,
        next: 0,
        step: Some(Step::Backup(
            engine::read(
                &link,
                None,
                move |d| crate::company::demo::report_transfer::export(d, &w2),
                move |s| async move { s.report_definitions().export(&what).await },
            ),
            2,
        )),
        saved: Vec::new(),
        single: false,
        review: Review::default(),
    };
    let mut outcome = Advance::Waiting;
    for _ in 0..50 {
        outcome = w.advance(&mut s);
        if !matches!(outcome, Advance::Waiting) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(matches!(
        outcome,
        Advance::Stop("report_file.backup.failed")
    ));
    assert!(!s.backed_up);
}

/// S-L3: a folder (or anything not a regular file) is refused without
/// being read.
#[test]
fn a_path_that_isnt_a_file_is_refused() {
    let dir = tempfile::tempdir().expect("tmp");
    assert_eq!(read_file(dir.path()), Err(FileError::NotReportFile));
}

/// S-L6: the engine's export (audited as a download) starts only after
/// the Save dialog gave a path; a cancelled dialog exports nothing.
#[test]
fn an_export_asks_for_the_path_before_exporting() {
    let (_d, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let mut w = ReportFileWindow::new(Arc::clone(&link));
    w.open_with(OpenPayload::ReportFile(Box::new(Start::Export(
        Target::Report(Box::new(row)),
    ))));
    w.poll_export();
    let Stage::Export(x) = &w.stage else {
        unreachable!()
    };
    assert!(
        x.job.is_none() && x.what.is_some(),
        "no export before a path"
    );
    w.answer_save(None);
    w.poll_export();
    let Stage::Export(x) = &w.stage else {
        unreachable!()
    };
    assert!(
        x.job.is_none() && x.built.is_none(),
        "cancelled: never exported"
    );
    assert!(w.take_close_request());
}

/// I2 and #14: two reports of a file can't Replace the same report; a
/// Replace that can't stand falls back to a free name, not the taken one.
#[test]
fn two_replaces_of_one_report_are_refused_and_a_fallback_is_free() {
    let mut r = review_of(&["Shop costs", "SHOP COSTS"]);
    r.take(vec![
        preview("Shop costs", true, Some((true, "builder"))),
        preview("SHOP COSTS", true, Some((true, "builder"))),
    ]);
    r.set_clash(0, Clash::Replace);
    r.set_clash(1, Clash::Replace);
    assert_eq!(r.clash_problem(1), Some(review::REPLACE_TWICE));
    assert!(!r.can_continue());
    assert!(crate::i18n::has(review::REPLACE_TWICE));
    r.take(vec![
        preview("Shop costs", true, Some((false, "builder"))),
        preview("SHOP COSTS", true, Some((true, "builder"))),
    ]);
    assert!(
        matches!(&r.clashes[0], Clash::Rename(n) if n != "Shop costs"),
        "{:?}",
        r.clash_problem(0)
    );
    assert_eq!(r.clash_problem(0), None, "the fallback name is free");
}

/// #15 and #1: the plan of a Replace keeps the existing name as written
/// and sends no group.
#[test]
fn a_replace_plan_keeps_the_existing_name_and_group() {
    let mut r = review_of(&["SHOP COSTS"]);
    let mut p = preview("SHOP COSTS", true, Some((true, "builder")));
    p.group_id = Some("g-file".into());
    r.take(vec![p]);
    r.set_clash(0, Clash::Replace);
    let plan = r.plan().expect("plan");
    assert_eq!(plan[0].name, "Shop costs");
    assert_eq!(
        (plan[0].group_id.as_deref(), plan[0].new_group.as_deref()),
        (None, None)
    );
}

/// S-L5: `Debug` of the review and its plan names nothing.
#[test]
fn review_debug_names_nothing() {
    let mut r = review_of(&["Secret payroll"]);
    r.take(vec![preview(
        "Secret payroll",
        true,
        Some((true, "builder")),
    )]);
    let plan = r.plan().expect("plan");
    let text = format!("{r:?} {plan:?}");
    assert!(!text.contains("Secret") && !text.contains("Shop"), "{text}");
}

/// S-L4: the review draws at most MAX_DRAWN_UNRESOLVED references; the
/// rest are counted.
#[test]
fn the_review_counts_what_it_doesnt_draw() {
    let mut r = review_of(&["A"]);
    let mut p = preview("A", false, None);
    p.unresolved = (0..MAX_DRAWN_UNRESOLVED + 7)
        .map(|n| {
            serde_json::from_value(
                json!({"ref": format!("$ref:{n}"), "reference": {"kind": "account", "name": "x"}}),
            )
            .expect("u")
        })
        .collect();
    r.take(vec![p]);
    assert_eq!(r.unresolved_total() - MAX_DRAWN_UNRESOLVED, 7);
}

/// Q-A09v: an account the file names by number says "under that number";
/// other references say "under that name".
#[test]
fn an_account_by_number_says_under_that_number() {
    use cl_model::report_def::clreport::Reference;
    let acct = Reference::of(&json!({"kind": "account", "number": "6287", "name": "Bank fees"}));
    assert_eq!(
        unresolved_key(acct.as_ref()),
        "report_file.card.unresolved_number"
    );
    let plain = Reference::of(&json!({"kind": "account", "name": "Bank fees"}));
    assert_eq!(
        unresolved_key(plain.as_ref()),
        "report_file.card.unresolved"
    );
    let tag = Reference::of(&json!({"kind": "tagGroup", "name": "Region"}));
    assert_eq!(unresolved_key(tag.as_ref()), "report_file.card.unresolved");
    assert!(crate::i18n::has("report_file.card.unresolved_number"));
}
