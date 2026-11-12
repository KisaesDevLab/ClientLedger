//! Schema and rule tests (plan §9 R-T9, R-T10, R-T14) on the hand
//! fixtures; `tests/report_def_fixtures.rs` runs the engine's shared cases
//! (PR-A) once pinned.

use serde_json::{Value, json};

use super::*;

const PNL: &str = include_str!("../../tests/fixtures/report_def/profit_loss_gross_profit.json");
const AGING: &str = include_str!("../../tests/fixtures/report_def/ar_aging_summary.json");
const SHEET: &str = include_str!("../../tests/fixtures/report_def/balance_sheet.json");
const UUID: &str = "55555555-5555-4555-8555-555555555555";

fn pnl() -> Value {
    serde_json::from_str(PNL).expect("fixture")
}

fn parse_value(v: &Value) -> Result<BuilderDefinition, DefinitionError> {
    parse(&serde_json::to_string(v).expect("text"))
}

fn code_of(r: Result<BuilderDefinition, DefinitionError>) -> (String, Code) {
    match r {
        Err(DefinitionError::Invalid { path, code }) => (path, code),
        other => panic!("expected a rule error, got {other:?}"),
    }
}

fn blank(id: &str) -> Value {
    json!({"type": "blank", "id": id})
}

fn section(id: &str, source: &str, measure: &str) -> Value {
    json!({"type": "section", "id": id, "caption": id, "source": source, "measure": measure,
           "filters": [], "groupBy": [], "display": "summary",
           "defaultExpanded": false, "showTotal": true, "hideZero": false})
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "reads like the JSON it builds"
)]
fn group(id: &str, children: Vec<Value>) -> Value {
    json!({"type": "group", "id": id, "caption": id, "children": children, "showTotal": true})
}

#[test]
fn the_hand_fixtures_parse_and_round_trip() {
    for text in [PNL, AGING, SHEET] {
        let def = parse(text).expect("valid");
        // Back to the same JSON (the engine stores `JSON.stringify(parsed)`).
        let again: Value = serde_json::from_str(&to_text(&def).expect("text")).expect("json");
        let orig: Value = serde_json::from_str(text).expect("json");
        assert_eq!(again, orig);
    }
    let def = parse(PNL).expect("valid");
    assert_eq!(outline::count(&def.nodes), 9);
    let Node::Section(s) = &def.nodes[0] else {
        panic!("section")
    };
    assert_eq!(
        s.filters[1].ids.as_deref(),
        Some(&["33333333-3333-4333-8333-333333333333".to_string()][..])
    );
    assert_eq!(
        s.group_by[1].tag_group_id.as_deref(),
        Some("44444444-4444-4444-8444-444444444444")
    );
}

#[test]
fn a_newer_version_is_refused_before_anything_else() {
    let mut v = pnl();
    v["schemaVersion"] = json!(2);
    v["aFieldFromVersionTwo"] = json!(true);
    v["nodes"] = json!(vec![blank("x"); 300]);
    assert_eq!(parse_value(&v), Err(DefinitionError::Newer));
    let mut v = pnl();
    v["kind"] = json!("standard");
    assert_eq!(parse_value(&v), Err(DefinitionError::NotBuilder));
    let mut v = pnl();
    v["schemaVersion"] = json!(0);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.schemaVersion".into(), Code::Schema)
    );
}

#[test]
fn unknown_fields_and_missing_required_ones_are_refused_at_every_depth() {
    let mut cases = Vec::new();
    for ptr in [
        "",
        "/columns",
        "/nodes/4/children/0",
        "/nodes/0/filters/0",
        "/nodes/2/terms/0",
        "/columns/compare/0",
    ] {
        let mut v = pnl();
        v.pointer_mut(ptr).expect(ptr)["extra"] = json!(1);
        cases.push(v);
    }
    let mut kind = pnl();
    kind["nodes"][3]["type"] = json!("chart");
    cases.push(kind);
    // Booleans and lists the engine requires are required here too.
    for (ptr, key) in [
        ("/nodes/1", "hideZero"),
        ("/nodes/1", "filters"),
        ("/nodes/4", "children"),
        ("/columns", "headers"),
        ("/columns/compare/0", "amount"),
    ] {
        let mut v = pnl();
        v.pointer_mut(ptr)
            .and_then(Value::as_object_mut)
            .expect(ptr)
            .remove(key);
        cases.push(v);
    }
    let mut source = pnl();
    source["nodes"][1]["source"] = json!("inventory");
    cases.push(source);
    for v in cases {
        assert!(
            matches!(parse_value(&v), Err(DefinitionError::Malformed { .. })),
            "{v}"
        );
    }
}

#[test]
fn duplicate_keys_are_refused_at_every_depth() {
    let top = PNL.replacen("\"title\":", "\"title\": \"x\", \"title\":", 1);
    let nested = PNL.replacen(
        "\"caption\": \"Expenses\",",
        "\"caption\": \"Expenses\", \"caption\": \"Again\",",
        1,
    );
    let headers = PNL.replacen(
        "\"headers\": { \"total\": \"Year to date\" }",
        "\"headers\": { \"total\": \"A\", \"total\": \"B\" }",
        1,
    );
    for text in [top, nested, headers] {
        assert_ne!(text, PNL, "the fixture changed");
        match parse(&text) {
            Err(DefinitionError::Malformed {
                kind: MalformedKind::DuplicateKey,
                ..
            }) => {}
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn two_hundred_nodes_pass_and_two_hundred_and_one_fail() {
    let children: Vec<Value> = (0..199).map(|i| blank(&format!("b{i}"))).collect();
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    v["nodes"] = json!([group("g", children)]);
    parse_value(&v).expect("200 nodes");
    v["nodes"][0]["children"]
        .as_array_mut()
        .expect("arr")
        .push(blank("b199"));
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes".into(), Code::TooManyNodes)
    );
}

#[test]
fn groups_nest_eight_deep_and_no_deeper() {
    let nest = |depth: usize| {
        let mut n = blank("leaf");
        for d in (0..depth).rev() {
            n = group(&format!("g{d}"), vec![n]);
        }
        n
    };
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    v["nodes"] = json!([nest(7)]); // the leaf sits at depth 8
    parse_value(&v).expect("8 deep");
    v["nodes"] = json!([nest(8)]);
    let (path, code) = code_of(parse_value(&v));
    assert_eq!(code, Code::TooDeep);
    assert!(path.ends_with(".children.0"), "{path}");
}

#[test]
fn detail_fields_are_capped_known_and_used_once() {
    let mut v = pnl();
    // Every field but the running balance (the P&L has periods).
    let all: Vec<&str> = fields::DETAIL_FIELDS
        .iter()
        .copied()
        .filter(|f| *f != "running_balance")
        .collect();
    v["columns"]["detailFields"] = json!(all);
    parse_value(&v).expect("21 fields");
    v["columns"]["detailFields"] = json!(["date", "date"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.detailFields".into(), Code::Schema)
    );
    v["columns"]["detailFields"] = json!(["shoe_size"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.detailFields.0".into(), Code::Schema)
    );
}

#[test]
fn the_byte_cap_counts_compact_utf8_bytes() {
    // 500 three-byte characters per row (500 UTF-16 units, the text cap);
    // 190 rows pass 256 KB, 150 don't.
    let text: String = "\u{20ac}".repeat(500);
    let rows: Vec<Value> = (0..190)
        .map(|i| json!({"type": "text", "id": format!("t{i}"), "text": text}))
        .collect();
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    v["nodes"] = json!(rows);
    assert_eq!(parse_value(&v), Err(DefinitionError::TooLarge));
    v["nodes"] = json!(rows[..150]);
    // Pretty-printed it is larger than compact; only compact counts.
    let pretty = serde_json::to_string_pretty(&v).expect("text");
    parse(&pretty).expect("under the cap");
}

/// S-L4: text far over the cap is refused before it is parsed (a newer
/// version can't hide it either).
#[test]
fn text_four_times_the_cap_is_refused_unread() {
    let mut text = String::from(r#"{"schemaVersion":99,"kind":"builder","x":""#);
    text.push_str(&" ".repeat(4 * crate::report_def::MAX_BYTES));
    text.push_str(r#""}"#);
    assert_eq!(parse(&text), Err(DefinitionError::TooLarge));
}

/// S-L3: an input object key in an error path stays exact for the parity
/// test, and never reaches the message.
#[test]
fn an_error_message_never_shows_an_input_key() {
    let mut v = pnl();
    v["columns"]["headers"] = json!({"Rivera Remodel": "Owner text"});
    let e = parse_value(&v).expect_err("a bad header key");
    assert_eq!(
        code_of(Err(e.clone())),
        (
            "definition.columns.headers.Rivera Remodel".into(),
            Code::Schema
        )
    );
    let shown = e.to_string();
    assert!(!shown.contains("Rivera"), "{shown}");
    assert!(
        shown.contains("definition.columns.headers.<key>"),
        "{shown}"
    );
}

#[test]
fn lengths_count_utf16_units_and_edges_are_kept() {
    let mut v = pnl();
    v["nodes"][0]["caption"] = json!("\u{1F600}".repeat(100)); // 200 units
    parse_value(&v).expect("200 units");
    v["nodes"][0]["caption"] = json!("\u{1F600}".repeat(101)); // 202 units
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.caption".into(), Code::Schema)
    );
    v["nodes"][0]["caption"] = json!("Income ");
    assert_eq!(code_of(parse_value(&v)).1, Code::Schema);
    v["nodes"][0]["caption"] = json!("");
    assert_eq!(code_of(parse_value(&v)).1, Code::Schema);
}

#[test]
fn a_uuid_in_owner_text_is_refused() {
    let cases = [
        ("/title", "definition.title"),
        ("/nodes/0/caption", "definition.nodes.0.caption"),
        ("/nodes/0/totalCaption", "definition.nodes.0.totalCaption"),
        ("/nodes/7/text", "definition.nodes.7.text"),
        ("/columns/headers/total", "definition.columns.headers.total"),
    ];
    for (ptr, path) in cases {
        let mut v = pnl();
        *v.pointer_mut(ptr).expect(ptr) = json!(format!("See {UUID} here"));
        assert_eq!(code_of(parse_value(&v)), (path.into(), Code::UuidInText));
    }
    assert!(!validate::contains_uuid("Invoice 1001-22"));
}

#[test]
fn ids_are_checked_where_they_belong() {
    let mut v = pnl();
    v["nodes"][0]["filters"][1]["ids"] = json!(["not-a-uuid"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.filters.1.ids.0".into(), Code::Schema)
    );
    let mut v = pnl();
    v["nodes"][0]["filters"][1]["ids"] = json!(vec![UUID; 501]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.filters.1.ids".into(), Code::Schema)
    );
    // A tag group with, and only with, group by tag.
    let mut v = pnl();
    v["nodes"][0]["groupBy"][0]["tagGroupId"] = json!(UUID);
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.0.groupBy.0.tagGroupId".into(),
            Code::Schema
        )
    );
    // A uuid is not a node id.
    let mut v = pnl();
    v["nodes"][3]["id"] = json!(UUID);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.3.id".into(), Code::Schema)
    );
}

#[test]
fn node_ids_follow_the_pattern_are_unique_and_section_is_reserved() {
    for bad in ["Income", "", "a b", &"a".repeat(33)] {
        let mut v = pnl();
        v["nodes"][3]["id"] = json!(bad);
        assert_eq!(code_of(parse_value(&v)).1, Code::Schema, "{bad:?}");
    }
    for dup in ["income", "section"] {
        let mut v = pnl();
        v["nodes"][3]["id"] = json!(dup);
        assert_eq!(
            code_of(parse_value(&v)),
            ("definition.nodes.3.id".into(), Code::DuplicateId)
        );
    }
}

#[test]
fn filters_take_the_right_operator_and_values() {
    let cases = [
        // ids on a value field
        (
            json!({"field": "account_type", "op": "in", "ids": [UUID]}),
            "op",
        ),
        // a value outside the enumeration
        (
            json!({"field": "account_type", "op": "in", "values": ["income"]}),
            "values.0",
        ),
        // is_none on a field that is never empty
        (json!({"field": "account", "op": "is_none"}), "op"),
        // gte without a value
        (json!({"field": "amount", "op": "gte"}), "op"),
    ];
    for (filter, at) in cases {
        let mut v = pnl();
        v["nodes"][1]["filters"] =
            json!([{"field": "account_type", "op": "in", "values": ["cogs"]}, filter]);
        assert_eq!(
            code_of(parse_value(&v)),
            (format!("definition.nodes.1.filters.1.{at}"), Code::FilterOp)
        );
    }
    let mut v = pnl();
    v["nodes"][1]["filters"] = json!([
        {"field": "account_type", "op": "in", "values": ["cogs"]},
        {"field": "amount", "op": "gte", "value": "-12.5"},
        {"field": "contact", "op": "is_none"}
    ]);
    parse_value(&v).expect("valid filters");
    v["nodes"][1]["filters"][1]["value"] = json!("12.50001");
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.filters.1.value".into(), Code::Schema)
    );
}

#[test]
fn sources_offer_only_their_own_measures_groups_and_polarity() {
    let mut v = pnl();
    v["nodes"][1]["measure"] = json!("quantity");
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.measure".into(), Code::NotInSource)
    );
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["nodes"][0]["groupBy"] = json!([{"key": "tag", "tagGroupId": UUID}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.groupBy.0.key".into(), Code::NotInSource)
    );
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["nodes"][0]["polarity"] = json!("debit");
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.polarity".into(), Code::NotInSource)
    );
}

#[test]
fn formulas_take_only_earlier_rows_of_one_unit() {
    let mut v = pnl();
    v["nodes"][2]["terms"][1]["nodeId"] = json!("expenses");
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.2.terms.1.nodeId".into(),
            Code::FormulaForwardRef
        )
    );
    for target in ["net_income", "nope", "b1"] {
        let mut v = pnl();
        v["nodes"][5]["terms"][0]["nodeId"] = json!(target);
        assert_eq!(
            code_of(parse_value(&v)).1,
            Code::FormulaForwardRef,
            "{target}"
        );
    }
    // An ancestor: a formula inside the group it adds up.
    let mut v = pnl();
    v["nodes"][4]["children"]
        .as_array_mut()
        .expect("arr")
        .push(json!({"type": "formula", "id": "inner", "caption": "x",
                     "terms": [{"nodeId": "spend", "sign": 1}]}));
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.4.children.1.terms.0.nodeId".into(),
            Code::FormulaForwardRef
        )
    );
    // Money with a count.
    let mut v = pnl();
    v["columns"]
        .as_object_mut()
        .expect("obj")
        .remove("percentOf");
    v["nodes"][1] = section("cogs", "sales", "count");
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.2.terms".into(), Code::FormulaUnits)
    );
    // A trial-balance pair can't be a term.
    let mut v = pnl();
    v["columns"]
        .as_object_mut()
        .expect("obj")
        .remove("percentOf");
    v["nodes"][1] = section("cogs", "gl", "tb_net");
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.2.terms.1.nodeId".into(),
            Code::FormulaUnits
        )
    );
    // Sign and term count.
    let mut v = pnl();
    v["nodes"][2]["terms"][0]["sign"] = json!(2);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.2.terms.0.sign".into(), Code::Schema)
    );
    let mut v = pnl();
    v["nodes"][2]["terms"] = json!([]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.2.terms".into(), Code::Schema)
    );
}

#[test]
fn a_node_reached_twice_keeps_its_unit() {
    let mut v = pnl();
    v["nodes"] = json!([
        group("g", vec![
            section("a", "sales", "amount"),
            section("b", "sales", "amount"),
            json!({"type": "formula", "id": "ab", "caption": "a+b",
                   "terms": [{"nodeId": "a", "sign": 1}, {"nodeId": "b", "sign": 1}]})
        ]),
        {"type": "formula", "id": "twice", "caption": "x",
         "terms": [{"nodeId": "g", "sign": 1}, {"nodeId": "ab", "sign": -1}]}
    ]);
    v["columns"]["percentOf"] = json!("g");
    parse_value(&v).expect("valid");
}

#[test]
fn percent_of_takes_a_money_node_or_section_over_money_sections() {
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    parse_value(&v).expect("section");
    v["columns"]["percentOf"] = json!("gross_profit");
    parse_value(&v).expect("a money formula");
    for bad in ["note", "nope"] {
        v["columns"]["percentOf"] = json!(bad);
        assert_eq!(
            code_of(parse_value(&v)),
            ("definition.columns.percentOf".into(), Code::PercentBase)
        );
    }
    let mut v = pnl();
    v["nodes"]
        .as_array_mut()
        .expect("arr")
        .push(section("n", "sales", "count"));
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.8.measure".into(), Code::PercentBase)
    );
}

#[test]
fn earnings_buckets_running_balance_and_polarity_rules() {
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["nodes"][1]["children"][1]["filters"][1] = json!({"field": "contact", "op": "is_none"});
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.1.children.1.filters.1.field".into(),
            Code::EarningsFilter
        )
    );
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["nodes"][1]["children"][1]["earnings"] = json!("trial_balance");
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.nodes.1.children.1.earnings".into(),
            Code::EarningsFilter
        )
    );
    // Buckets with periods, and on an activity section.
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["columns"]["periods"] = json!("month");
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.columns.agingBuckets".into(),
            Code::BucketsPeriods
        )
    );
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["nodes"]
        .as_array_mut()
        .expect("arr")
        .push(section("s2", "sales", "amount"));
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.source".into(), Code::BucketsPeriods)
    );
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["columns"]["agingBuckets"] = json!(false);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.agingBuckets".into(), Code::Schema)
    );
    // A running balance needs the account innermost, and no periods.
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["columns"]["detailFields"] = json!(["date", "running_balance"]);
    parse_value(&v).expect("by account");
    v["nodes"][0]["groupBy"] = json!([{"key": "account"}, {"key": "contact"}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.0.groupBy".into(), Code::RunningBalance)
    );
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["columns"]["detailFields"] = json!(["running_balance"]);
    v["columns"]["periods"] = json!("month");
    assert_eq!(
        code_of(parse_value(&v)),
        (
            "definition.columns.detailFields".into(),
            Code::RunningBalance
        )
    );
    // Natural over income and expense accounts; an explicit side is fine.
    let mut v = pnl();
    v["nodes"][1]["filters"][0]["values"] = json!(["cogs", "revenue"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.polarity".into(), Code::MixedPolarity)
    );
    v["nodes"][1]["polarity"] = json!("debit");
    parse_value(&v).expect("an explicit side");
}

#[test]
fn caps_on_filters_group_bys_compare_text_and_headers() {
    let mut v = pnl();
    v["nodes"][1]["filters"] = json!(vec![json!({"field": "tag", "op": "in", "ids": [UUID]}); 21]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.filters".into(), Code::Schema)
    );
    let mut v = pnl();
    v["nodes"][1]["groupBy"] =
        json!([{"key": "account"}, {"key": "contact"}, {"key": "item"}, {"key": "month"}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.1.groupBy".into(), Code::Schema)
    );
    let mut v = pnl();
    let c = v["columns"]["compare"][0].clone();
    v["columns"]["compare"] = json!([c.clone(), c.clone(), c]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.compare".into(), Code::Schema)
    );
    let mut v = pnl();
    v["nodes"][7]["text"] = json!("x".repeat(501));
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.nodes.7.text".into(), Code::Schema)
    );
    let mut v = pnl();
    v["columns"]["headers"]["total"] = json!("x".repeat(81));
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.headers.total".into(), Code::Schema)
    );
    let mut v = pnl();
    v["columns"]["headers"]["Total"] = json!("x");
    assert_eq!(
        code_of(parse_value(&v)),
        ("definition.columns.headers.Total".into(), Code::Schema)
    );
}
