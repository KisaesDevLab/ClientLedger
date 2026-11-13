#![allow(clippy::many_single_char_names, reason = "short test names")]
//! `.clreport` export and import on the demo engine (A09 WP-A): the review's
//! state machine, single versus several reports, Cancel writes nothing,
//! Rename / Replace by the rules, and Replace's verified backup copy.

use std::sync::Arc;

use cl_api::resources::reports::definitions::{
    DefinitionWrite, NameTaken, PreviewReport, TransferReport,
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
    let mut w = import(&link, &path);
    assert_eq!(w.stage_name(), "review", "{:?}", w.message);
    w.cancel();
    assert!(w.take_close_request());
    assert_eq!(count(&link), before);

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

#[test]
fn replace_asks_then_writes_a_verified_copy_before_overwriting() {
    let (dir, link) = demo();
    let row = memorize(&link, "Shop costs", "builder", builder_def(&link), None);
    let path = dir.path().join("one.clreport");
    export_to(&link, Target::Report(Box::new(row.clone())), &path);
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
    // Then the report was overwritten (a new version), and it opens.
    let mut d = link.demo_state().expect("demo");
    let now = report_store::definition(&mut d, &row.id).expect("row");
    assert!(now.version > row.version);
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
