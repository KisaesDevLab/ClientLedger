#![allow(clippy::many_single_char_names, reason = "short test names")]
//! The demo `.clreport` routes follow the engine's rules (U-220 parity).

use cl_api::resources::reports::definitions::{
    Choices, DefinitionWrite, ExportWhat, TransferReport, choice_key,
};
use serde_json::{Value, json};

use super::*;
use crate::company::demo::report_store;

/// A builder section over `accounts`, grouped by tag group `tg`.
pub(crate) fn builder(accounts: &[String], tg: &str) -> Value {
    json!({
        "schemaVersion": 1, "kind": "builder", "title": "Shop costs",
        "dates": {"mode": "range"},
        "columns": {"detailFields": [], "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "s1", "caption": "Shop costs", "source": "gl",
            "measure": "amount", "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": accounts},
                        {"field": "amount", "op": "gte", "value": "1250.5000"}],
            "groupBy": [{"key": "tag", "tagGroupId": tg}], "display": "summary",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    })
}

fn ids(d: &mut DemoState) -> (Vec<String>, String) {
    let accounts: Vec<String> = rows(d, "account")
        .into_iter()
        .filter(|r| r.account_type == Some("expense"))
        .take(2)
        .map(|r| r.id)
        .collect();
    let tg = tag_groups(d)
        .first()
        .map(|g| g.0.clone())
        .expect("tag group");
    (accounts, tg)
}

fn memorize(d: &mut DemoState, name: &str, kind: &str, def: Value) -> String {
    let w = DefinitionWrite {
        name: name.into(),
        group_id: None,
        kind: kind.into(),
        definition: def,
        origin: None,
    };
    report_store::save_definition(d, None, &w).expect("save").id
}

fn exported(d: &mut DemoState) -> (Vec<TransferReport>, Value) {
    let (accounts, tg) = ids(d);
    let def = builder(&accounts, &tg);
    assert!(
        cl_model::report_def::parse(&def.to_string()).is_ok(),
        "a valid definition"
    );
    let id = memorize(d, "Shop costs", "builder", def.clone());
    (export(d, &ExportWhat::Ids(vec![id])).expect("export"), def)
}

#[test]
fn export_tokenizes_every_id_and_names_it() {
    let mut d = DemoState::new();
    let (out, _) = exported(&mut d);
    let r = &out[0];
    assert_eq!(
        r.definition["nodes"][0]["filters"][0]["ids"],
        json!(["$ref:0", "$ref:1"])
    );
    assert_eq!(
        r.definition["nodes"][0]["groupBy"][0]["tagGroupId"],
        "$ref:2"
    );
    assert_eq!(r.definition["nodes"][0]["filters"][1]["value"], "1250.5000");
    assert_eq!(r.references.len(), 3);
    assert_eq!(r.references[0]["kind"], "account");
    assert_eq!(r.references[2]["kind"], "tagGroup");
    let text = serde_json::to_string(&out).expect("json");
    assert!(!cl_model::report_def::clreport::has_uuid(&text), "no ids");
    // An unknown report is a 404, as the engine.
    assert!(export(&mut d, &ExportWhat::Ids(vec!["nope".into()])).is_err());
}

#[test]
fn the_same_company_gets_its_definition_back_ready() {
    let mut d = DemoState::new();
    let (out, def) = exported(&mut d);
    let back = preview(&mut d, &out, &Choices::new()).expect("preview");
    assert!(back[0].ready, "{:?}", back[0]);
    let mut want = def;
    want["title"] = json!("Shop costs");
    assert_eq!(back[0].definition, want);
    assert!(back[0].name_taken.as_ref().is_some_and(|t| t.can_edit));
}

#[test]
fn unmatched_references_take_a_pick_or_a_leave_out() {
    let mut d = DemoState::new();
    let (mut out, _) = exported(&mut d);
    // The second account under another number: unresolved, the same-name
    // account its candidate; the tag group under another name: no match.
    out[0].references[1]["number"] = json!("99999");
    out[0].references[2]["name"] = json!("Not a group here");
    let first = preview(&mut d, &out, &Choices::new()).expect("preview");
    assert!(!first[0].ready);
    assert_eq!(first[0].unresolved.len(), 2);
    assert_eq!(first[0].unresolved[0].candidates.len(), 1);
    assert_eq!(first[0].unresolved[1].candidates.len(), 0);

    let mut c = Choices::new();
    let pick = first[0].unresolved[0].candidates[0].id.clone();
    c.insert(choice_key(0, "$ref:1"), Some(pick));
    c.insert(choice_key(0, "$ref:2"), None);
    let done = preview(&mut d, &out, &c).expect("preview");
    assert!(done[0].ready, "{:?}", done[0]);
    assert_eq!(done[0].dropped.len(), 1);
    assert_eq!(done[0].dropped[0].effect, "group_by_removed");
    assert_eq!(done[0].definition["nodes"][0]["groupBy"], json!([]));

    // Leaving both accounts out empties the filter, which goes.
    c.insert(choice_key(0, "$ref:0"), None);
    c.insert(choice_key(0, "$ref:1"), None);
    let wide = preview(&mut d, &out, &c).expect("preview");
    let filters = wide[0].definition["nodes"][0]["filters"].clone();
    assert_eq!(filters.as_array().map(Vec::len), Some(1), "{filters}");
    assert_eq!(filters[0]["field"], "amount");
    assert!(wide[0].dropped.iter().any(|x| x.effect == "filter_removed"));
}

#[test]
fn standard_leave_outs_and_the_budget_rule() {
    let mut d = DemoState::new();
    let budget = report_store::budgets(&mut d)[0].id.clone();
    let tg = tag_groups(&d)[0].0.clone();
    let (accounts, _) = ids(&mut d);
    let def = json!({"schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
        "options": {"dates": {"preset": "last_month"}, "columns": "tag_group",
                    "columnTagGroupId": tg, "budgetId": budget,
                    "tagSplit": {"tagGroupId": tg},
                    "filters": {"accountIds": [accounts[0]]}}});
    let id = memorize(&mut d, "Split P&L", "standard", def);
    let out = export(&mut d, &ExportWhat::Ids(vec![id])).expect("export");
    let tokens = &out[0].definition["options"];
    assert_eq!(
        tokens["tagSplit"]["tagGroupId"], tokens["columnTagGroupId"],
        "one reference"
    );
    let mut c = Choices::new();
    for n in 0..out[0].references.len() {
        c.insert(choice_key(0, &format!("$ref:{n}")), None);
    }
    let p = preview(&mut d, &out, &c).expect("preview");
    let o = &p[0].definition["options"];
    assert_eq!(o["tagSplit"], Value::Null);
    assert_eq!(o["columns"], "total");
    assert!(o.get("columnTagGroupId").is_none());
    assert_eq!(
        o["filters"]["accountIds"],
        json!([]),
        "a standard list may end empty"
    );
    // The budget can't be left out: it stays unresolved.
    assert!(!p[0].ready);
    assert_eq!(p[0].unresolved.len(), 1);
    assert_eq!(p[0].unresolved[0].reference["kind"], "budget");
}

#[test]
fn bad_input_is_refused_as_the_engine_does() {
    let mut d = DemoState::new();
    let (mut out, _) = exported(&mut d);
    let mut c = Choices::new();
    c.insert(choice_key(3, "$ref:0"), None);
    assert!(
        preview(&mut d, &out, &c).is_err(),
        "a report not in the file"
    );
    let mut c = Choices::new();
    c.insert(choice_key(0, "$ref:0"), Some("not-a-row".into()));
    assert!(preview(&mut d, &out, &c).is_err(), "not this company's row");
    out[0].definition["schemaVersion"] = json!(2);
    let e = preview(&mut d, &out, &Choices::new()).expect_err("newer");
    assert!(matches!(
        cl_api::resources::reports::definitions::definition_refusal(&e),
        Some(cl_api::resources::reports::definitions::DefinitionRefusal::Invalid { code: Some(c), .. })
            if c == "SCHEMA_NEWER"
    ));
    out[0].definition["schemaVersion"] = json!(1);
    out[0].definition["nodes"][0]["filters"][0]["ids"] =
        json!(["0b7c6c1e-3f7a-4d7e-9b1a-2f6a4c8e9d10"]);
    assert!(preview(&mut d, &out, &Choices::new()).is_err(), "a raw id");
}
