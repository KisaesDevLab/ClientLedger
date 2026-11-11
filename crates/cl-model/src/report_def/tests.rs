//! Schema and rule tests (plan §9 R-T9, R-T10, R-T14). The JSON fixtures
//! are hand-written from plan §3 until PR-A's shared fixtures land; the
//! `report_def_fixtures` integration test parses those too once present.

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

fn money_section(id: &str) -> Value {
    json!({"type": "section", "id": id, "caption": id, "source": "sales", "measure": "amount",
           "display": "summary"})
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
    // Ids sit only at the id paths: filters' ids and groupBy's tagGroupId.
    assert_eq!(
        s.filters[1].ids.as_deref(),
        Some(&["33333333-3333-4333-8333-333333333333".to_string()][..])
    );
    assert_eq!(
        s.group_by[1].tag_group_id.as_deref(),
        Some("44444444-4444-4444-8444-444444444444")
    );
    assert_eq!(
        s.filters[0].values.as_deref(),
        Some(&["income".to_string(), "other_income".to_string()][..])
    );
}

#[test]
fn a_newer_version_is_refused_before_strict_parsing() {
    let mut v = pnl();
    v["schemaVersion"] = json!(2);
    v["aFieldFromVersionTwo"] = json!(true);
    assert_eq!(parse_value(&v), Err(DefinitionError::Newer));
    let mut v = pnl();
    v["kind"] = json!("standard");
    assert_eq!(parse_value(&v), Err(DefinitionError::NotBuilder));
}

#[test]
fn unknown_fields_are_refused_at_every_depth() {
    let mut top = pnl();
    top["extra"] = json!(1);
    let mut cols = pnl();
    cols["columns"]["extra"] = json!(1);
    let mut node = pnl();
    node["nodes"][4]["children"][0]["extra"] = json!(1);
    let mut filter = pnl();
    filter["nodes"][0]["filters"][0]["extra"] = json!(1);
    let mut term = pnl();
    term["nodes"][2]["terms"][0]["extra"] = json!(1);
    let mut kind = pnl();
    kind["nodes"][3]["type"] = json!("chart");
    for v in [top, cols, node, filter, term, kind] {
        assert!(
            matches!(parse_value(&v), Err(DefinitionError::Malformed(_))),
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
            Err(DefinitionError::Malformed(m)) => assert!(m.contains("duplicate"), "{m}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn two_hundred_nodes_pass_and_two_hundred_and_one_fail_at_any_depth() {
    // 1 group + 199 blanks inside it = 200 nodes.
    let children: Vec<Value> = (0..199).map(|i| blank(&format!("b{i}"))).collect();
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    v["nodes"] = json!([{"type": "group", "id": "g", "caption": "G", "children": children}]);
    parse_value(&v).expect("200 nodes");
    v["nodes"][0]["children"]
        .as_array_mut()
        .expect("arr")
        .push(blank("b199"));
    assert_eq!(code_of(parse_value(&v)), ("nodes".into(), Code::Limit));
}

#[test]
fn twenty_four_detail_fields_pass_and_twenty_five_fail() {
    let mut v = pnl();
    v["columns"]["detailFields"] = json!(vec!["date"; 24]);
    parse_value(&v).expect("24");
    v["columns"]["detailFields"] = json!(vec!["date"; 25]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.detailFields".into(), Code::Limit)
    );
    v["columns"]["detailFields"] = json!(["shoe_size"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.detailFields.0".into(), Code::UnknownField)
    );
}

#[test]
fn the_byte_cap_counts_bytes_not_characters() {
    // 500 characters of three-byte text per row stays inside each text cap
    // but pushes the whole file past 256 KB.
    let text: String = "\u{20ac}".repeat(500);
    let rows: Vec<Value> = (0..190)
        .map(|i| json!({"type": "text", "id": format!("t{i}"), "text": text}))
        .collect();
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    v["nodes"] = json!(rows);
    let s = serde_json::to_string(&v).expect("text");
    assert!(s.chars().count() < MAX_BYTES && s.len() > MAX_BYTES);
    assert_eq!(parse(&s), Err(DefinitionError::TooLarge));
    v["nodes"] = json!(rows[..150]);
    parse_value(&v).expect("under the cap");
}

#[test]
fn a_uuid_in_owner_text_is_refused() {
    let cases = [
        ("/title", "title"),
        ("/nodes/0/caption", "nodes.0.caption"),
        ("/nodes/0/totalCaption", "nodes.0.totalCaption"),
        ("/nodes/7/text", "nodes.7.text"),
        ("/columns/headers/total", "columns.headers.total"),
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
        ("nodes.0.filters.1.ids.0".into(), Code::BadId)
    );
    let mut v = pnl();
    v["nodes"][0]["filters"][1]["ids"] = json!(vec![UUID; 501]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.0.filters.1.ids".into(), Code::Limit)
    );
    let mut v = pnl();
    v["nodes"][0]["groupBy"][1]["tagGroupId"] = json!("x");
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.0.groupBy.1.tagGroupId".into(), Code::BadId)
    );
}

#[test]
fn node_ids_follow_the_pattern_and_are_unique() {
    for bad in [
        "Income",
        "",
        "a b",
        &"a".repeat(33),
        "0123456789abcdef0123456789abcdef",
    ] {
        let mut v = pnl();
        v["nodes"][3]["id"] = json!(bad);
        assert_eq!(code_of(parse_value(&v)).1, Code::NodeId, "{bad:?}");
    }
    let mut v = pnl();
    v["nodes"][3]["id"] = json!("income");
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.3.id".into(), Code::DuplicateId)
    );
}

#[test]
fn formulas_take_only_earlier_money_rows() {
    // Forward.
    let mut v = pnl();
    v["nodes"][2]["terms"][1]["nodeId"] = json!("expenses");
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.2.terms.1".into(), Code::FormulaForwardRef)
    );
    // Itself, unknown, and a blank row.
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
        ("nodes.4.children.1.terms.0".into(), Code::FormulaForwardRef)
    );
    // Units.
    let mut v = pnl();
    v["nodes"][1]["source"] = json!("sales");
    v["nodes"][1]["measure"] = json!("count");
    v["nodes"][1]["filters"] = json!([]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.2.terms".into(), Code::FormulaUnits)
    );
    // Sign.
    let mut v = pnl();
    v["nodes"][2]["terms"][0]["sign"] = json!(2);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.2.terms.0.sign".into(), Code::BadSign)
    );
}

#[test]
fn percent_of_takes_a_money_node_or_section() {
    let mut v = pnl();
    v["columns"]["percentOf"] = json!("section");
    parse_value(&v).expect("section");
    v["columns"]["percentOf"] = json!("gross_profit");
    parse_value(&v).expect("a money formula");
    for bad in ["note", "nope"] {
        v["columns"]["percentOf"] = json!(bad);
        assert_eq!(
            code_of(parse_value(&v)),
            ("columns.percentOf".into(), Code::PercentBase)
        );
    }
    let mut v = pnl();
    v["nodes"].as_array_mut().expect("arr").push(json!(
        {"type": "section", "id": "n", "caption": "n", "source": "sales",
         "measure": "count", "display": "summary"}));
    v["columns"]["percentOf"] = json!("n");
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.percentOf".into(), Code::PercentBase)
    );
}

#[test]
fn earnings_buckets_running_balance_and_polarity_rules() {
    // Earnings with an account filter.
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["nodes"][0]["filters"] = json!([{"field": "account", "op": "in", "ids": [UUID]}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.0.earnings".into(), Code::EarningsFilter)
    );
    // Earnings on sales.
    let mut v = pnl();
    v["nodes"][1]["earnings"] = json!("trial_balance");
    assert_eq!(code_of(parse_value(&v)).1, Code::EarningsFilter);
    // Buckets with periods, and on an activity section.
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["columns"]["periods"] = json!("month");
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.agingBuckets".into(), Code::BucketsPeriods)
    );
    let mut v: Value = serde_json::from_str(AGING).expect("json");
    v["nodes"]
        .as_array_mut()
        .expect("arr")
        .push(money_section("s2"));
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.source".into(), Code::BucketsPeriods)
    );
    // A running balance grouped by account type last.
    let mut v: Value = serde_json::from_str(SHEET).expect("json");
    v["nodes"][0]["groupBy"] = json!([{"key": "account"}, {"key": "account_type"}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.0.groupBy".into(), Code::RunningBalance)
    );
    // Natural over income and expense accounts.
    let mut v = pnl();
    v["nodes"][1]["filters"][0]["values"] = json!(["cogs", "income"]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.polarity".into(), Code::MixedPolarity)
    );
    v["nodes"][1]["polarity"] = json!("debit");
    parse_value(&v).expect("an explicit side");
}

#[test]
fn greyed_choices_are_refused() {
    let mut v = pnl();
    v["basis"] = json!("cash");
    v["nodes"][1]["source"] = json!("sales");
    v["nodes"][1]["measure"] = json!("quantity");
    v["nodes"][1]["filters"] = json!([]);
    v["nodes"][2]["terms"] = json!([{"nodeId": "income", "sign": 1}]);
    v["nodes"][5]["terms"] = json!([{"nodeId": "gross_profit", "sign": 1}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.measure".into(), Code::Unavailable)
    );
    let mut v = pnl();
    v["nodes"][1]["groupBy"] = json!([{"key": "project"}]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.groupBy.0.key".into(), Code::Unavailable)
    );
}

#[test]
fn caps_on_filters_group_bys_terms_compare_and_text() {
    let mut v = pnl();
    v["nodes"][1]["filters"] = json!(vec![json!({"field": "tag", "op": "in", "ids": [UUID]}); 21]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.filters".into(), Code::Limit)
    );
    let mut v = pnl();
    v["nodes"][1]["groupBy"] = json!(vec![json!({"key": "account"}); 4]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.1.groupBy".into(), Code::Limit)
    );
    let mut v = pnl();
    v["nodes"][2]["terms"] = json!(vec![json!({"nodeId": "income", "sign": 1}); 21]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.2.terms".into(), Code::Limit)
    );
    let mut v = pnl();
    let c = v["columns"]["compare"][0].clone();
    v["columns"]["compare"] = json!([c.clone(), c.clone(), c]);
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.compare".into(), Code::Limit)
    );
    let mut v = pnl();
    v["nodes"][7]["text"] = json!("x".repeat(501));
    assert_eq!(
        code_of(parse_value(&v)),
        ("nodes.7.text".into(), Code::Limit)
    );
    let mut v = pnl();
    v["columns"]["headers"]["total"] = json!("x".repeat(81));
    assert_eq!(
        code_of(parse_value(&v)),
        ("columns.headers.total".into(), Code::Limit)
    );
}

#[test]
fn a_node_reached_twice_keeps_its_unit() {
    // A group holding a, b and the formula a + b; then a formula and
    // percentOf on the group. Each of a and b is reached twice.
    let mut v = pnl();
    v["nodes"] = json!([
        {"type": "group", "id": "g", "caption": "G", "children": [
            money_section("a"),
            money_section("b"),
            {"type": "formula", "id": "ab", "caption": "a+b",
             "terms": [{"nodeId": "a", "sign": 1}, {"nodeId": "b", "sign": 1}]}
        ]},
        {"type": "formula", "id": "twice", "caption": "x",
         "terms": [{"nodeId": "g", "sign": 1}, {"nodeId": "ab", "sign": -1}]}
    ]);
    v["columns"]["percentOf"] = json!("g");
    parse_value(&v).expect("valid");
}
