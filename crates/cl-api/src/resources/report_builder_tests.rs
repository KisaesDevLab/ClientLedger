//! Adapter tests on the hand samples (plan §7 WP1); PR-C's engine samples
//! replace them when pinned. These prove the reading below, not the
//! engine's contract: the contract tests in `tests/contract/` do that.

use super::*;
use serde_json::json;

fn sample(name: &str) -> Value {
    let text = match name {
        "run" => include_str!("../../tests/fixtures/report_builder/run_sample.json"),
        _ => include_str!("../../tests/fixtures/report_builder/detail_sample.json"),
    };
    serde_json::from_str(text).expect("fixture")
}

/// The columns of the run the detail sample pages.
fn detail_columns() -> Vec<ColumnHead> {
    columns(&json!({"columns": [
        {"key": "date", "caption": "Date", "kind": "date"},
        {"key": "amount", "caption": "Amount", "kind": "amount"}
    ]}))
    .expect("columns")
}

fn d(s: &str) -> Decimal {
    Decimal::from_str_exact(s).expect("decimal")
}

#[test]
fn a_run_keeps_the_exact_strings_and_maps_every_kind() {
    let run = read_run(&sample("run")).expect("run");
    assert_eq!(run.fingerprint, "fp-1");
    assert_eq!(run.stamp, "41");
    assert_eq!(run.denied, ["bills"]);
    assert_eq!(
        run.expanded,
        [
            RowPath::of(["n:income"]),
            RowPath::of(["n:income", "account:7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60"])
        ]
    );
    let t = &run.table;
    assert_eq!(t.columns.len(), 6);
    let kinds: Vec<RowKind> = t.rows.iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            RowKind::Header,
            RowKind::Group,
            RowKind::Detail,
            RowKind::More,
            RowKind::Group,
            RowKind::Subtotal,
            RowKind::Denied,
            RowKind::Formula,
            RowKind::Earnings,
            RowKind::Blank,
            RowKind::Text,
        ]
    );
    // Sub-cent amounts stay exact: 800.005 is not rounded.
    assert_eq!(t.rows[1].cells[3], Cell::Amount(d("800.005")));
    assert_eq!(t.rows[1].cells[5], Cell::Percent(Some(d("100.0"))));
    assert_eq!(t.rows[2].cells[5], Cell::Percent(None));
    // Detail fields fill the text and date columns.
    assert_eq!(
        t.rows[2].cells[0],
        Cell::Date(CalendarDate::from_ymd(2026, 10, 4).expect("d"))
    );
    assert_eq!(t.rows[2].cells[1], Cell::Text("1001".into()));
    // Layout rows get empty cells, one per column.
    assert!(
        t.rows[0]
            .cells
            .iter()
            .all(|c| matches!(c, Cell::Empty | Cell::Percent(None)))
    );
    // A cell that depends on a denied section shows the dash.
    assert_eq!(t.rows[7].cells[2], Cell::Text("\u{2014}".into()));
    assert_eq!(t.rows[6].caption, "@reports.builder.denied");
    assert_eq!(t.rows[3].caption, "@reports.builder.more");
    assert_eq!(t.meta.basis, Some(Basis::Accrual));
    assert!(t.rows[1].expandable);
}

#[test]
fn drills_are_builder_detail_pages_forms_and_profit_and_loss() {
    let run = read_run(&sample("run")).expect("run");
    let t = &run.table;
    let sub = t.rows[5].drill.as_ref().expect("subtotal drill");
    assert_eq!(
        sub,
        &Drill::BuilderDetail {
            node_id: "income".into(),
            path: RowPath::of(["n:income", "total"]),
            range: None,
        }
    );
    // A period column drills with its own dates; the percent column not at all.
    assert!(matches!(
        sub.for_cell(&t.columns[2]),
        Some(Drill::BuilderDetail { range: Some(r), .. }) if r.from == CalendarDate::from_ymd(2026, 10, 1).expect("d")
    ));
    assert_eq!(sub.for_cell(&t.columns[5]), None);
    assert!(matches!(
        &t.rows[2].drill,
        Some(Drill::Transaction { txn_type, .. }) if txn_type == "invoice"
    ));
    assert_eq!(
        t.rows[3].drill,
        Some(Drill::BuilderPage {
            node_id: "income".into(),
            path: RowPath::of([
                "n:income",
                "account:7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60",
                "more"
            ]),
            cursor: "eyJrIjoiMSJ9".into(),
        })
    );
    assert!(matches!(t.rows[8].drill, Some(Drill::ProfitLoss { .. })));
    // The section's lens and tags travel with the earnings drill.
    let mut v = sample("run");
    v["rows"][8]["drill"]["adjusting"] = json!("exclude");
    v["rows"][8]["drill"]["tagIds"] = json!(["33333333-3333-4333-8333-333333333333"]);
    let t = read_run(&v).expect("run").table;
    assert!(matches!(
        &t.rows[8].drill,
        Some(Drill::ProfitLoss { adjusting: Some(Adjusting::Exclude), tag_ids, .. }) if tag_ids.len() == 1
    ));
    assert_eq!(t.rows[7].drill, None, "formulas don't drill");
    assert_eq!(t.rows[6].drill, None, "denied rows don't drill");
}

#[test]
fn a_detail_page_echoes_the_subtotal() {
    let p = read_detail(&sample("detail"), "bank", &detail_columns()).expect("page");
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[0].kind, RowKind::Earnings, "the opening row");
    assert_eq!(p.cursor.as_deref(), Some("eyJrIjoiMiJ9"));
    assert_eq!(p.subtotal[1], Cell::Amount(d("125.1000")));
    let mut last = sample("detail");
    last["cursor"] = Value::Null;
    assert_eq!(
        read_detail(&last, "bank", &detail_columns())
            .expect("page")
            .cursor,
        None
    );
}

#[test]
fn anything_off_contract_is_a_shape_error() {
    let cases: Vec<(&str, Value)> = vec![
        ("/rows/1/cells/2", json!(1200)),
        ("/rows/1/cells/2", json!("1,200.00")),
        ("/rows/1/cells/2", json!("1e3")),
        ("/rows/1/kind", json!("chart")),
        ("/rows/1/drill/kind", json!("teleport")),
        ("/rows/3/cursor", Value::Null),
        ("/columns/2/kind", json!("money")),
        ("/rows/1/level", json!(-1)),
    ];
    for (ptr, bad) in cases {
        let mut v = sample("run");
        *v.pointer_mut(ptr).expect(ptr) = bad;
        assert!(
            matches!(read_run(&v), Err(ApiError::ReportShape { .. })),
            "{ptr}"
        );
    }
    let mut v = sample("run");
    v["rows"][1]["cells"].as_array_mut().expect("arr").pop();
    assert!(matches!(read_run(&v), Err(ApiError::ReportShape { .. })));
    let mut v = sample("detail");
    v["subtotal"] = json!(["125.10"]);
    assert!(matches!(
        read_detail(&v, "bank", &detail_columns()),
        Err(ApiError::ReportShape { .. })
    ));
    let mut v = sample("detail");
    v["rows"] = json!(vec![v["rows"][1].clone(); PAGE_ROWS + 1]);
    assert!(matches!(
        read_detail(&v, "bank", &detail_columns()),
        Err(ApiError::ReportShape { .. })
    ));
}

fn reference() -> RunRef {
    RunRef {
        definition: Some(json!({"schemaVersion": 1, "kind": "builder"})),
        definition_id: None,
        version: None,
        dates: RunDates::range(DateRange {
            from: CalendarDate::from_ymd(2026, 10, 1).expect("d"),
            to: CalendarDate::from_ymd(2026, 11, 30).expect("d"),
        }),
        period_ranges: Vec::new(),
        compare_ranges: Vec::new(),
        fiscal_year_starts: vec![CalendarDate::from_ymd(2026, 1, 1).expect("d")],
        basis: Some(Basis::Cash),
        adjusting: None,
    }
}

#[test]
fn the_request_wire_shape_is_pinned() {
    let req = RunRequest {
        reference: reference(),
        open: None,
        open_all: false,
        purpose: RunPurpose::Screen,
    };
    assert_eq!(
        serde_json::to_value(&req).expect("json"),
        json!({
            "definition": {"schemaVersion": 1, "kind": "builder"},
            "dates": {"from": "2026-10-01", "to": "2026-11-30"},
            "fiscalYearStarts": ["2026-01-01"],
            "basis": "cash",
            "open": null,
            "purpose": "screen"
        })
    );
    let detail = DetailRequest {
        reference: reference(),
        node_id: "income".into(),
        path: RowPath::of(["n:income", "account:none"]),
        range: None,
        cursor: Some("c".into()),
        fingerprint: "fp".into(),
        stamp: "41".into(),
    };
    let v = serde_json::to_value(&detail).expect("json");
    assert_eq!(v["path"], json!(["n:income", "account:none"]));
    assert_eq!(v["nodeId"], "income");
    assert!(v.get("open").is_none() && v.get("purpose").is_none());
}

#[test]
fn requests_over_the_caps_are_refused_before_sending() {
    let ok = RunRequest {
        reference: reference(),
        open: Some(vec![RowPath::of(["n:a"])]),
        open_all: false,
        purpose: RunPurpose::Screen,
    };
    check_run(&ok).expect("ok");
    let range = DateRange {
        from: CalendarDate::from_ymd(2026, 1, 1).expect("d"),
        to: CalendarDate::from_ymd(2026, 1, 31).expect("d"),
    };
    let mut periods = ok.clone();
    periods.reference.period_ranges = vec![range; MAX_PERIODS];
    check_run(&periods).expect("36 periods");
    periods.reference.period_ranges.push(range);
    let mut compare = ok.clone();
    compare.reference.period_ranges = vec![range; 2];
    compare.reference.compare_ranges = vec![vec![range; 1]];
    let mut deep = ok.clone();
    deep.open = Some(vec![RowPath::of(["a", "b", "c", "d", "e"])]);
    let mut many = ok.clone();
    many.open = Some(vec![RowPath::of(["a"]); MAX_OPEN_PATHS + 1]);
    let mut both = ok.clone();
    both.reference.definition_id = Some("d1".into());
    for bad in [periods, compare, deep, many, both] {
        assert!(matches!(
            check_run(&bad),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
    }
}

#[test]
fn refusals_are_recognised() {
    let r = |status: u16, body: Value| builder_refusal(&crate::engine_refusal(status, &body));
    let err = |code: &str, details: Value| json!({"error": {"message": "x", "code": code, "details": details}});
    assert_eq!(
        r(409, err("RUN_CHANGED", json!({"reason": "books"}))),
        Some(BuilderRefusal::BooksChanged)
    );
    assert_eq!(
        r(409, err("RUN_CHANGED", json!({"reason": "params"}))),
        Some(BuilderRefusal::ParamsChanged)
    );
    assert_eq!(
        r(503, err("REPORT_TIMEOUT", json!({}))),
        Some(BuilderRefusal::Timeout)
    );
    assert_eq!(
        r(422, err("REPORT_TOO_LARGE", json!({}))),
        Some(BuilderRefusal::TooLarge)
    );
    assert!(matches!(
        // The PR-A body: `details: {path, code}`.
        r(400, err("DEFINITION_INVALID", json!({"path": "definition.nodes.0.polarity", "code": "MIXED_POLARITY"}))),
        Some(BuilderRefusal::Invalid { path: Some(p), code: Some(c) })
            if p == "definition.nodes.0.polarity" && c == "MIXED_POLARITY"
    ));
    assert_eq!(
        r(409, err("STALE_VERSION", json!({"current": 4}))),
        Some(BuilderRefusal::StaleDefinition)
    );
    assert_eq!(
        r(404, err("NOT_FOUND", json!({}))),
        Some(BuilderRefusal::Deleted)
    );
    assert_eq!(
        r(403, err("FORBIDDEN", json!({}))),
        Some(BuilderRefusal::Denied)
    );
    assert_eq!(
        builder_refusal(&ApiError::PayloadTooLarge),
        Some(BuilderRefusal::DefinitionTooLarge)
    );
    assert_eq!(builder_refusal(&ApiError::Timeout), None);
}

#[test]
fn starters_read_bare_or_wrapped() {
    let one = json!({"key": "profit_loss", "title": "Profit & Loss",
                     "definition": {"schemaVersion": 1, "kind": "builder"}});
    for v in [json!([one.clone()]), json!({"data": [one.clone()]})] {
        let s = read_starters(&v).expect("starters");
        assert_eq!(s[0].key, "profit_loss");
    }
    assert!(read_starters(&json!([{"key": "x"}])).is_err());
}
