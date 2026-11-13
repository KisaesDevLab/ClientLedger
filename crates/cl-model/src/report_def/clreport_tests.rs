use serde_json::{Value, json};

use super::*;

fn builder_def() -> Value {
    json!({
        "schemaVersion": 1, "kind": "builder", "title": "Shop costs",
        "nodes": [{"type": "section", "id": "s1", "caption": "Supplies",
                   "filters": [{"field": "account", "op": "in", "ids": ["$ref:0"]},
                               {"field": "amount", "op": "gte", "value": "1250.5000"}],
                   "groupBy": [{"key": "tag", "tagGroupId": "$ref:1"}]}]
    })
}

fn standard_def() -> Value {
    json!({"schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
           "options": {"filters": {"accountIds": ["$ref:0"]}, "budgetId": null}})
}

fn file() -> ReportFile {
    ReportFile::new(
        "ClientLedger 0.9.0",
        "2026-11-12T21:00:00Z",
        vec![
            FileReport {
                name: "Shop costs".into(),
                group: Some("For the bank".into()),
                kind: "builder".into(),
                definition: builder_def(),
                references: vec![
                    json!({"kind": "account", "number": "6100", "name": "Shop supplies", "accountType": "expense"}),
                    json!({"kind": "tagGroup", "name": "Region"}),
                ],
            },
            FileReport {
                name: "Monthly P&L".into(),
                group: None,
                kind: "standard".into(),
                definition: standard_def(),
                references: vec![
                    json!({"kind": "account", "name": "Sales", "accountType": "revenue"}),
                ],
            },
        ],
    )
}

fn bytes_of(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("json")
}

#[test]
fn the_envelope_round_trips_through_write_and_read() {
    let f = file();
    let bytes = write(&f).expect("write");
    assert_eq!(read(&bytes).expect("read"), f);
    // A byte-order mark from an editor is fine.
    let mut bom = "\u{feff}".as_bytes().to_vec();
    bom.extend_from_slice(&bytes);
    assert_eq!(read(&bom).expect("bom"), f);
}

#[test]
fn an_amount_filter_value_is_kept_exactly_as_typed() {
    let bytes = write(&file()).expect("write");
    let back = read(&bytes).expect("read");
    let value = &back.reports[0].definition["nodes"][0]["filters"][1]["value"];
    assert_eq!(value, &Value::String("1250.5000".into()));
    let text = String::from_utf8(bytes).expect("utf8");
    assert!(text.contains("\"1250.5000\""), "trailing zeros kept");
}

#[test]
fn a_newer_file_or_definition_is_refused_as_newer() {
    let mut v = serde_json::to_value(file()).expect("v");
    v["schemaVersion"] = json!(2);
    // A key this build doesn't know yet doesn't hide the version.
    v["somethingNew"] = json!(true);
    assert_eq!(read(&bytes_of(&v)), Err(FileError::Newer));

    for i in 0..2 {
        let mut v = serde_json::to_value(file()).expect("v");
        v["reports"][i]["definition"]["schemaVersion"] = json!(2);
        v["reports"][i]["definition"]["newField"] = json!(1);
        assert_eq!(read(&bytes_of(&v)), Err(FileError::Newer), "report {i}");
    }
}

#[test]
fn malformed_files_are_refused() {
    let not = Err(FileError::NotReportFile);
    assert_eq!(read(b"not json"), not);
    assert_eq!(read(&[0xff, 0xfe, 0x00]), not);
    assert_eq!(read(b"[]"), not);
    let mut v = serde_json::to_value(file()).expect("v");
    v["format"] = json!("something-else");
    assert_eq!(read(&bytes_of(&v)), not);
    let mut v = serde_json::to_value(file()).expect("v");
    v["extra"] = json!(1);
    assert_eq!(read(&bytes_of(&v)), not, "unknown key");
    let mut v = serde_json::to_value(file()).expect("v");
    v["reports"][0]["kind"] = json!("standard");
    assert_eq!(
        read(&bytes_of(&v)),
        not,
        "kind differs from the definition's"
    );
    let mut v = serde_json::to_value(file()).expect("v");
    v["reports"][0]["name"] = json!("   ");
    assert_eq!(read(&bytes_of(&v)), not, "blank name");
    let mut v = serde_json::to_value(file()).expect("v");
    v["reports"][0]["kind"] = json!("chart");
    v["reports"][0]["definition"]["kind"] = json!("chart");
    assert_eq!(read(&bytes_of(&v)), not, "unknown kind");
    let mut v = serde_json::to_value(file()).expect("v");
    v["reports"] = json!([]);
    assert_eq!(read(&bytes_of(&v)), Err(FileError::Empty));
}

#[test]
fn deep_nesting_is_refused() {
    let mut deep = json!("x");
    for _ in 0..70 {
        deep = json!([deep]);
    }
    let mut v = serde_json::to_value(file()).expect("v");
    v["reports"][0]["definition"]["nodes"][0]["deep"] = deep;
    assert_eq!(read(&bytes_of(&v)), Err(FileError::NotReportFile));
}

#[test]
fn oversize_files_definitions_and_counts_are_refused() {
    // The file, by size, before it is parsed.
    let big = vec![b' '; MAX_FILE_BYTES + 1];
    assert_eq!(read(&big), Err(FileError::TooLarge));
    // A standard definition over 64 KB.
    let mut f = file();
    f.reports[1].definition["options"]["pad"] = json!("x".repeat(64 * 1024));
    assert_eq!(write(&f), Err(FileError::TooLarge));
    let v = serde_json::to_value(&f).expect("v");
    assert_eq!(read(&bytes_of(&v)), Err(FileError::TooLarge));
    // A builder definition may be larger (256 KB).
    let mut f = file();
    f.reports[0].definition["pad"] = json!("x".repeat(100 * 1024));
    assert!(write(&f).is_ok());
    // More than 200 reports.
    let mut f = file();
    let one = f.reports[1].clone();
    f.reports = vec![one; MAX_REPORTS + 1];
    assert_eq!(write(&f), Err(FileError::TooLarge));
    let v = serde_json::to_value(&f).expect("v");
    assert_eq!(read(&bytes_of(&v)), Err(FileError::TooLarge));
}

#[test]
fn a_file_that_would_be_over_2_mb_is_not_written() {
    // Twelve builder definitions of 200 KB each: every one fits, the file
    // doesn't, and a file a reader would refuse is never written.
    let mut f = file();
    let mut big = f.reports[0].clone();
    big.definition["pad"] = json!("x".repeat(200 * 1024));
    f.reports = vec![big; 12];
    assert_eq!(write(&f), Err(FileError::TooLarge));
}

#[test]
fn a_written_file_holds_no_ids_and_no_figures() {
    let text = String::from_utf8(write(&file()).expect("write")).expect("utf8");
    assert!(!has_uuid(&text));
    for k in ["\"cells\"", "\"rows\"", "\"totals\"", "\"balances\""] {
        assert!(!text.contains(k), "{k}");
    }
    // An id left in a definition is refused.
    let mut f = file();
    f.reports[0].definition["nodes"][0]["filters"][0]["ids"] =
        json!(["0b7c6c1e-3f7a-4d7e-9b1a-2f6a4c8e9d10"]);
    assert_eq!(write(&f), Err(FileError::CompanyData));
    // So are run figures.
    let mut f = file();
    f.reports[1].definition["rows"] = json!([{"cells": ["12.00"]}]);
    assert_eq!(write(&f), Err(FileError::CompanyData));
    // And an id in a reference.
    let mut f = file();
    f.reports[1].references[0]["id"] = json!("0B7C6C1E-3F7A-4D7E-9B1A-2F6A4C8E9D10");
    assert_eq!(write(&f), Err(FileError::CompanyData));
}

#[test]
fn uuid_shapes_are_found_inside_text() {
    assert!(has_uuid("0b7c6c1e-3f7a-4d7e-9b1a-2f6a4c8e9d10"));
    assert!(has_uuid("id=0b7c6c1e-3f7a-4d7e-9b1a-2f6a4c8e9d10;"));
    assert!(!has_uuid("$ref:12"));
    assert!(!has_uuid("0b7c6c1e-3f7a-4d7e-9b1a-2f6a4c8e9d1"));
    assert!(!has_uuid("0b7c6c1e_3f7a-4d7e-9b1a-2f6a4c8e9d10"));
    assert!(!has_uuid("Sales report for 2026"));
}

#[test]
fn free_names_count_up_from_two() {
    let taken = ["Shop costs (2)".to_lowercase(), "shop costs (3)".into()];
    assert_eq!(
        free_name("Shop costs", |n| taken.contains(&n.to_lowercase())),
        "Shop costs (4)"
    );
    assert_eq!(free_name(" P&L ", |_| false), "P&L (2)");
}

#[test]
fn references_read_for_display() {
    let a = Reference::of(&json!({"kind": "account", "number": "6100", "name": "Shop supplies"}))
        .expect("account");
    assert_eq!(a.label().as_deref(), Some("6100 \u{b7} Shop supplies"));
    let t = Reference::of(&json!({"kind": "tag", "name": "West", "tagGroupName": "Region"}))
        .expect("tag");
    assert_eq!(t.label().as_deref(), Some("Region: West"));
    let b = Reference::of(&json!({"kind": "budget", "name": "Plan", "fiscalYear": 2026}))
        .expect("budget");
    assert_eq!(b.label().as_deref(), Some("Plan (2026)"));
    let gone = Reference::of(&json!({"kind": "tag", "missing": true})).expect("missing");
    assert!(gone.missing);
    assert_eq!(gone.label(), None);
    assert!(Reference::of(&json!({"kind": "planet"})).is_none());
    // Debug names no row.
    assert!(!format!("{a:?}").contains("Shop"));
}

/// A09 S-L1: a group name over the engine's 100 characters is refused
/// locally (a report name may have 200).
#[test]
fn a_group_name_over_100_characters_is_refused() {
    let mut f = file();
    f.reports[0].group = Some("g".repeat(MAX_GROUP_CHARS));
    f.reports[0].name = "n".repeat(MAX_NAME_CHARS);
    assert!(write(&f).is_ok());
    f.reports[0].group = Some("g".repeat(MAX_GROUP_CHARS + 1));
    assert_eq!(write(&f), Err(FileError::NotReportFile));
}

/// A09 S-L4: the references of the whole file are capped, so a file of
/// many reports can't ask the engine (and the review) for 200 × 5,000.
#[test]
fn the_references_of_a_whole_file_are_capped() {
    let mut f = file();
    let r = f.reports[1].clone();
    f.reports = (0..3)
        .map(|n| {
            let mut x = r.clone();
            x.name = format!("Report {n}");
            x.references =
                vec![json!({"kind": "account", "name": "Sales"}); MAX_FILE_REFERENCES / 2];
            x
        })
        .collect();
    let bytes = serde_json::to_vec(&f).expect("json");
    assert_eq!(read(&bytes), Err(FileError::TooLarge));
    assert_eq!(write(&f), Err(FileError::TooLarge), "export refuses too");
    f.reports.truncate(2);
    assert!(read(&serde_json::to_vec(&f).expect("json")).is_ok());
}
