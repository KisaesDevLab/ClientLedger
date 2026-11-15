use serde_json::json;

use super::*;

const ACCT: &str = "11111111-1111-4111-8111-111111111111";
const ACCT2: &str = "22222222-2222-4222-8222-222222222222";
const GROUP: &str = "33333333-3333-4333-8333-333333333333";

fn req() -> DraftRequest {
    DraftRequest {
        description: "Rent by crew this year, bills over $500".into(),
        date_presets: vec!["this_year".into(), "last_month".into()],
    }
}

fn section(filters: &serde_json::Value, group_by: &serde_json::Value) -> serde_json::Value {
    json!({"type": "section", "id": "rowa", "caption": "Rent", "source": "gl",
           "measure": "amount", "filters": filters, "groupBy": group_by,
           "display": "summary", "defaultExpanded": false, "showTotal": true,
           "hideZero": false})
}

fn answer() -> serde_json::Value {
    json!({
        "definition": {"schemaVersion": 1, "kind": "builder",
            "dates": {"mode": "range", "preset": "this_year"},
            "columns": {"detailFields": [], "compare": [], "headers": {}},
            "nodes": [
                section(&json!([{"field": "account", "op": "in", "ids": ["$ref:0", ACCT]}]),
                        &json!([{"key": "tag", "tagGroupId": "$ref:1"}])),
                {"type": "formula", "id": "rowb", "caption": "Net", "terms": [{"nodeId": "rowa", "sign": -1}]}
            ]},
        "savable": false,
        "unresolved": [
            {"ref": "$ref:0", "kind": "account", "name": "Rent", "reason": "ambiguous",
             "sites": [{"nodeId": "rowa", "path": "definition.nodes.0.filters.0.ids.0"}],
             "candidates": [{"id": ACCT, "label": "6300 \u{b7} Rent"}, {"id": ACCT2, "label": "6310 \u{b7} Rent - storage"}]},
            {"ref": "$ref:1", "kind": "tagGroup", "name": null, "reason": "not_found",
             "sites": [{"nodeId": "rowa", "path": "definition.nodes.0.groupBy.0.tagGroupId"}]}
        ],
        "notes": [{"code": "pick_unresolved", "message": "x"},
                  {"code": "amount_filter_left_out", "message": "x", "nodeId": "rowa"},
                  {"code": "something_newer", "message": "x"}],
        "reader": "cloud", "provider": "anthropic", "model": "m"
    })
}

#[test]
fn a_draft_with_placeholders_is_taken() {
    let d = validate(&req(), &answer()).expect("draft");
    assert!(!d.savable);
    assert_eq!(d.unresolved.len(), 2);
    assert_eq!(d.unresolved[0].kind, DraftKind::Account);
    assert_eq!(d.unresolved[0].candidates.len(), 2);
    assert_eq!(d.unresolved[1].kind, DraftKind::TagGroup);
    assert_eq!(d.unresolved[1].name, None);
    assert_eq!(d.unresolved[1].sites[0].node_id, "rowa");
    // An unknown code is left out; the engine's words are never kept.
    assert_eq!(
        d.notes,
        vec![
            DraftNote {
                code: NoteCode::PickUnresolved,
                node_id: None
            },
            DraftNote {
                code: NoteCode::AmountFilterLeftOut,
                node_id: Some("rowa".into())
            },
        ]
    );
    assert_eq!(d.provider.as_deref(), Some("anthropic"));
}

#[test]
fn a_clean_draft_is_savable() {
    let mut v = answer();
    v["definition"]["nodes"][0] = section(
        &json!([{"field": "account", "op": "in", "ids": [ACCT]}]),
        &json!([{"key": "tag", "tagGroupId": GROUP}]),
    );
    v["unresolved"] = json!([]);
    v["savable"] = json!(true);
    let d = validate(&req(), &v).expect("draft");
    assert!(d.savable);
    assert_eq!(d.unresolved, vec![]);
}

/// Non-negotiable 4: no number other than the structure's, and no amount
/// filter of any shape.
#[test]
fn numbers_and_amount_filters_are_refused() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("/definition/nodes/0/caption", json!("Rent over 500")),
        ("/definition/nodes/0/caption", json!(500)),
        ("/definition/nodes/1/terms/0/sign", json!(2)),
        ("/definition/schemaVersion", json!(2)),
        ("/definition/dates/preset", json!("last_decade")),
        (
            "/definition/nodes/0/filters",
            json!([{"field": "amount", "op": "gte", "value": "500"}]),
        ),
        (
            "/definition/nodes/0/filters",
            json!([{"field": "account", "op": "in", "ids": ["$ref:0", ACCT], "value": "5"}]),
        ),
        (
            "/definition/nodes/0/filters",
            json!([{"field": "account", "op": "in", "ids": ["$ref:0", "not-an-id-42"]}]),
        ),
    ];
    for (at, bad_value) in cases {
        let mut v = answer();
        *v.pointer_mut(at).expect(at) = bad_value.clone();
        assert!(validate(&req(), &v).is_err(), "{at} = {bad_value}");
    }
}

#[test]
fn placeholders_and_the_unresolved_list_must_agree() {
    // A token in the definition that isn't listed.
    let mut v = answer();
    v["unresolved"].as_array_mut().expect("a").pop();
    assert!(validate(&req(), &v).is_err());
    // Listed, but not in the definition.
    let mut v = answer();
    v["definition"]["nodes"][0]["groupBy"] = json!([]);
    assert!(validate(&req(), &v).is_err());
    // `savable` that disagrees.
    let mut v = answer();
    v["savable"] = json!(true);
    assert!(validate(&req(), &v).is_err());
    // A candidate id that isn't one.
    let mut v = answer();
    v["unresolved"][0]["candidates"][0]["id"] = json!("x");
    assert!(validate(&req(), &v).is_err());
    // A name with a digit.
    let mut v = answer();
    v["unresolved"][0]["name"] = json!("Rent 500");
    assert!(validate(&req(), &v).is_err());
}

#[test]
fn structural_digits_pass() {
    let mut v = answer();
    v["definition"]["columns"]["headers"] = json!({"d1_30": "Recent"});
    v["definition"]["nodes"][0]["filters"] = json!([{"field": "account", "op": "in", "ids": ["$ref:0", ACCT]},
               {"field": "aging", "op": "in", "values": ["d31_60", "over_90"]}]);
    assert!(validate(&req(), &v).is_ok());
}

#[test]
fn the_request_sends_only_the_description_and_presets() {
    let v = serde_json::to_value(req()).expect("json");
    assert_eq!(
        v.as_object().map(|m| m.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["datePresets".to_string(), "description".to_string()])
    );
    assert!(!format!("{:?}", req()).contains("Rent"));
}

#[test]
fn the_description_cap_is_the_engine_s() {
    assert!(!description_ok("   "));
    assert!(description_ok(&"a".repeat(MAX_DESCRIPTION_CHARS)));
    assert!(!description_ok(&"a".repeat(MAX_DESCRIPTION_CHARS + 1)));
}

#[test]
fn refusals_are_recognised() {
    let consent = crate::engine_refusal(
        403,
        &json!({"error": {"message": "x", "code": "AI_CONSENT_REQUIRED", "details": {"reason": "ai_disabled", "task": "report_summary"}}}),
    );
    assert_eq!(draft_refusal(&consent), Some(DraftRefusal::ConsentRequired));
    let unmapped = crate::engine_refusal(
        422,
        &json!({"error": {"message": "x", "code": "DRAFT_UNMAPPED", "details": {"field": "definition", "reason": "no_report"}}}),
    );
    assert_eq!(
        draft_refusal(&unmapped),
        Some(DraftRefusal::Unmapped {
            reason: Some("no_report".into())
        })
    );
    let down = crate::engine_refusal(
        503,
        &json!({"error": {"message": "x", "code": "AI_UNAVAILABLE", "details": {"provider": "ollama", "reason": "offline"}}}),
    );
    assert_eq!(draft_refusal(&down), Some(DraftRefusal::Unavailable));
    let invalid = crate::engine_refusal(
        400,
        &json!({"error": {"message": "x", "code": "VALIDATION_ERROR"}}),
    );
    assert_eq!(draft_refusal(&invalid), Some(DraftRefusal::Invalid));
}
