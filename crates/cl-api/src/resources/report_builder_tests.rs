//! Adapter tests on the engine's own samples (PR-C, bc41f48d:
//! `shared/src/report-builder/fixtures/run`, copied to
//! `tests/fixtures/report_builder/`). Each sample is `{request, response}`;
//! the requests are descriptions, so the wire shape of a request is pinned
//! here and proved live by the contract test.

use super::*;
use serde_json::json;

fn sample(name: &str) -> Value {
    let text = match name {
        "profit_loss_accrual" => {
            include_str!("../../tests/fixtures/report_builder/profit_loss_accrual.json")
        }
        "profit_loss_cash" => {
            include_str!("../../tests/fixtures/report_builder/profit_loss_cash.json")
        }
        "balance_sheet_accrual" => {
            include_str!("../../tests/fixtures/report_builder/balance_sheet_accrual.json")
        }
        "balance_sheet_cash" => {
            include_str!("../../tests/fixtures/report_builder/balance_sheet_cash.json")
        }
        "transaction_detail_by_account" => {
            include_str!("../../tests/fixtures/report_builder/transaction_detail_by_account.json")
        }
        "run_detail_page" => {
            include_str!("../../tests/fixtures/report_builder/run_detail_page.json")
        }
        "expandable_open" => {
            include_str!("../../tests/fixtures/report_builder/expandable_open.json")
        }
        "profit_loss_compare_prior_year" => {
            include_str!("../../tests/fixtures/report_builder/profit_loss_compare_prior_year.json")
        }
        "denied_section" => include_str!("../../tests/fixtures/report_builder/denied_section.json"),
        other => panic!("no sample {other}"),
    };
    let v: Value = serde_json::from_str(text).expect("fixture");
    v["response"].clone()
}

const RUNS: [&str; 8] = [
    "profit_loss_accrual",
    "profit_loss_cash",
    "balance_sheet_accrual",
    "balance_sheet_cash",
    "transaction_detail_by_account",
    "expandable_open",
    "profit_loss_compare_prior_year",
    "denied_section",
];

fn run(name: &str) -> BuilderRun {
    read_run(&sample(name), None).expect(name)
}

fn d(s: &str) -> Decimal {
    Decimal::from_str_exact(s).expect("decimal")
}

fn day(y: i32, m: u32, dd: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, dd).expect("day")
}

const ZERO_ID: &str = "00000000-0000-4000-8000-000000000000";

#[test]
fn every_engine_sample_reads() {
    for name in RUNS {
        let r = run(name);
        assert_eq!(r.fingerprint.len(), 64, "{name}");
        assert!(r.stamp > 0, "{name}");
        assert!(!r.truncated, "{name}");
        for row in &r.table.rows {
            assert_eq!(row.cells.len(), r.table.columns.len(), "{name}");
        }
    }
}

#[test]
fn a_profit_and_loss_keeps_the_exact_strings() {
    let r = run("profit_loss_accrual");
    let t = &r.table;
    // U-119f: the starter's 7 field columns have no value in a summary run
    // and aren't shown; a drill tab keeps them (wire 0..7, total at 7).
    assert_eq!(t.columns.len(), 1);
    assert_eq!(t.columns[0].key, "total");
    assert_eq!(r.layout.shown, [7]);
    assert_eq!(r.detail_columns.len(), 8);
    assert_eq!(r.detail_layout.shown, [0, 1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(t.columns[0].kind, ColumnKind::Amount);
    // The total drills every column: no range is sent (U-119e).
    assert_eq!(t.columns[0].range, None);
    assert_eq!(t.meta.basis, Some(Basis::Accrual));
    let kinds: Vec<RowKind> = t.rows.iter().take(6).map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        [
            RowKind::Header,
            RowKind::Group,
            RowKind::Total,
            RowKind::Header,
            RowKind::Total,
            RowKind::Formula
        ]
    );
    // 43.3300 stays four places, exactly.
    assert_eq!(t.rows[7].cells[0], Cell::Amount(d("43.3300")));
    assert_eq!(t.rows[15].caption, "Net Income");
    assert_eq!(t.rows[15].cells[0], Cell::Amount(d("111.6700")));
    assert_eq!(t.rows[0].cells[0], Cell::Empty, "a header has no amount");
    // A line and a total drill to their own node and path; formulas don't.
    assert_eq!(
        t.rows[1].drill,
        Some(Drill::BuilderDetail {
            node_id: "income".into(),
            path: RowPath::of(["n:income", &format!("account:{ZERO_ID}")]),
            range: None,
        })
    );
    assert_eq!(
        t.rows[2].drill,
        Some(Drill::BuilderDetail {
            node_id: "income".into(),
            path: RowPath::of(["n:income"]),
            range: None,
        })
    );
    assert_eq!(t.rows[5].drill, None);
    // The total column's cell drills every column's lines (no range).
    assert!(matches!(
        t.rows[1]
            .drill
            .as_ref()
            .and_then(|x| x.for_cell(&t.columns[0])),
        Some(Drill::BuilderDetail { range: None, .. })
    ));
    assert_eq!(run("profit_loss_cash").table.meta.basis, Some(Basis::Cash));
}

#[test]
fn a_balance_sheet_as_of_column_and_earnings_rows() {
    let v = sample("balance_sheet_accrual");
    let r = read_run(&v, Some(Adjusting::Exclude)).expect("bs");
    let t = &r.table;
    // The total column drills every column: no range (U-119e).
    assert_eq!(t.columns[0].range, None);
    assert_eq!(t.meta.dates, Some(ReportDates::AsOf(day(2026, 12, 31))));
    assert_eq!(t.rows[8].kind, RowKind::Earnings);
    assert_eq!(t.rows[8].caption, "Prior years' net income");
    assert_eq!(t.rows[8].cells[0], Cell::Amount(d("500.0000")));
    // Prior years start at 0001-01-01 on the wire: from the first day kept.
    assert_eq!(
        t.rows[8].drill,
        Some(Drill::ProfitLoss {
            from: CalendarDate::min(),
            to: day(2025, 12, 31),
            basis: Some(Basis::Accrual),
            adjusting: Some(Adjusting::Exclude),
            tag_ids: Vec::new(),
        })
    );
    assert!(matches!(
        t.rows[9].drill,
        Some(Drill::ProfitLoss { from, .. }) if from == day(2026, 1, 1)
    ));
    // A tag-filtered section's earnings drill names its tags (U-119e).
    let tag = "33333333-3333-4333-8333-333333333333";
    let mut tagged = v.clone();
    tagged["rows"][8]["drill"]["tagIds"] = json!([tag]);
    let r = read_run(&tagged, None).expect("tagged");
    assert!(matches!(
        &r.table.rows[8].drill,
        Some(Drill::ProfitLoss { tag_ids, .. })
            if tag_ids.len() == 1 && tag_ids[0].to_string() == tag
    ));
    tagged["rows"][8]["drill"]["tagIds"] = json!(["not a tag"]);
    assert!(matches!(
        read_run(&tagged, None),
        Err(ApiError::ReportShape { .. })
    ));
    assert_eq!(t.rows[11].kind, RowKind::Formula);
    assert_eq!(t.rows[11].cells[0], Cell::Amount(d("656.6700")));
    assert_eq!(
        run("balance_sheet_cash").table.meta.basis,
        Some(Basis::Cash)
    );
}

#[test]
fn compare_columns_change_and_percent() {
    let r = run("profit_loss_compare_prior_year");
    let t = &r.table;
    let keys: Vec<&str> = t.columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "total",
            "total:c0",
            "total:c0:change",
            "total:c0:pct_change"
        ]
    );
    assert_eq!(
        t.columns[1].range,
        Some(DateRange {
            from: day(2025, 1, 1),
            to: day(2025, 12, 31)
        })
    );
    assert_eq!(t.rows[1].cells[2], Cell::Amount(d("-300.0000")));
    assert_eq!(t.rows[1].cells[3], Cell::Percent(Some(d("-60.0"))));
    // A zero base has no percent.
    assert_eq!(t.rows[7].cells[3], Cell::Percent(None));
    let drill = t.rows[1].drill.clone().expect("drill");
    // Compare and change cells don't drill (plan §4).
    assert_eq!(drill.for_cell(&t.columns[1]), None);
    assert_eq!(drill.for_cell(&t.columns[2]), None);
    // An as-of period column drills from the beginning (U-119e).
    let as_of = column(&json!({"key": "p1", "caption": "Mar", "kind": "period",
                                "range": {"from": null, "to": "2026-03-31"}}))
    .expect("column")
    .expect("shown");
    assert!(matches!(
        drill.for_cell(&as_of),
        Some(Drill::BuilderDetail { range: Some(r), .. })
            if r.from == CalendarDate::min() && r.to == day(2026, 3, 31)
    ));
    let ranged = column(&json!({"key": "p1", "caption": "Mar", "kind": "period",
                                 "range": {"from": "2026-03-01", "to": "2026-03-31"}}))
    .expect("column")
    .expect("shown");
    assert!(drill.for_cell(&ranged).is_some());
    assert_eq!(drill.for_cell(&t.columns[3]), None);
}

#[test]
fn a_hidden_compare_column_keeps_its_wire_place() {
    let mut v = sample("profit_loss_compare_prior_year");
    // `amount: false` with a change: the engine sends the prior column as
    // `hidden` with null cells. The 7 field columns (wire 0..7) have no
    // value in a summary run, so they aren't shown either (U-119f).
    v["columns"][8]["kind"] = json!("hidden");
    for row in v["rows"].as_array_mut().expect("rows") {
        row["cells"][8] = Value::Null;
    }
    let r = read_run(&v, None).expect("run");
    assert_eq!(r.layout.wire, 11);
    assert_eq!(r.layout.shown, [7, 9, 10]);
    // A drill tab keeps the field columns.
    assert_eq!(r.detail_layout.shown, [0, 1, 2, 3, 4, 5, 6, 7, 9, 10]);
    assert_eq!(r.detail_columns.len(), 10);
    let t = &r.table;
    assert_eq!(t.columns.len(), 3);
    assert_eq!(t.columns[1].key, "total:c0:change");
    assert_eq!(t.rows[1].cells[1], Cell::Amount(d("-300.0000")));
    // A detail page carries the hidden cell too.
    let page = json!({"rows": [], "cursor": null,
                      "subtotal": [null, null, null, null, null, null, null,
                                   "1.0000", null, "2.0000", "3.0"]});
    let p = read_detail(&page, &t.columns, &r.layout).expect("page");
    assert_eq!(
        p.subtotal,
        [
            Cell::Amount(d("1.0000")),
            Cell::Amount(d("2.0000")),
            Cell::Percent(Some(d("3.0")))
        ]
    );
}

#[test]
fn a_detail_section_types_its_field_columns() {
    let r = run("transaction_detail_by_account");
    let t = &r.table;
    // Num and Description have no value in this run: not shown (U-119f);
    // a drill tab keeps all ten.
    let keys: Vec<&str> = t.columns.iter().map(|c| c.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "f:date",
            "f:txn_type",
            "f:contact",
            "f:memo",
            "f:debit",
            "f:credit",
            "f:running_balance",
            "total"
        ]
    );
    assert_eq!(r.detail_columns.len(), 10);
    let kinds: Vec<ColumnKind> = t.columns.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        [
            ColumnKind::Date,
            ColumnKind::Text,
            ColumnKind::Text,
            ColumnKind::Text,
            ColumnKind::Amount,
            ColumnKind::Amount,
            ColumnKind::Amount,
            ColumnKind::Amount,
        ]
    );
    let opening = &t.rows[2];
    assert_eq!(opening.kind, RowKind::Earnings);
    assert_eq!(opening.caption, "@reports.builder.opening");
    assert_eq!(opening.cells[6], Cell::Amount(d("500.0000")));
    let detail = &t.rows[3];
    assert_eq!(detail.kind, RowKind::Detail);
    assert_eq!(detail.cells[0], Cell::Date(day(2026, 3, 1)));
    assert_eq!(detail.cells[1], Cell::Text("Cash Sale".into()));
    assert_eq!(detail.cells[2], Cell::Text("Rivera Remodel".into()));
    assert_eq!(detail.cells[6], Cell::Amount(d("520.0000")));
    assert_eq!(detail.cells[7], Cell::Amount(d("20.0000")));
    assert!(matches!(
        &detail.drill,
        Some(Drill::Transaction { txn_type, .. }) if txn_type == "cash_sale"
    ));
    // Detail rows share their line's path (the window places them under
    // it). The lines are always open and can't close, so none is in the
    // open set (#11: Expand all stays on offer).
    assert_eq!(detail.path, t.rows[1].path);
    assert!(!t.rows[1].expandable);
    assert_eq!(r.expanded, Vec::<RowPath>::new());
}

/// S-L2: a caption, header, note or text cell from the engine or the owner
/// is never read as an i18n key, even when it looks like one.
#[test]
fn passed_through_text_that_looks_like_a_key_stays_text() {
    let mut v = sample("transaction_detail_by_account");
    v["rows"][1]["caption"] = json!("@reports.builder.denied");
    v["columns"][1]["caption"] = json!("@reports.col.total");
    v["rows"][3]["cells"][1] = json!("@reports.row.total");
    v["meta"]["notes"] = json!(["@reports.builder.no_activity"]);
    let t = read_run(&v, None).expect("run").table;
    assert_eq!(t.rows[1].caption, "@@reports.builder.denied");
    assert_eq!(t.columns[1].caption, "@@reports.col.total");
    assert_eq!(t.rows[3].cells[1], Cell::Text("@@reports.row.total".into()));
    assert_eq!(
        t.meta.note.as_deref(),
        Some("@@reports.builder.no_activity")
    );
}

/// S-L5: the denied row shows nothing whatever it carries; a figure in a
/// denied section's other rows is off contract.
#[test]
fn a_denied_section_never_shows_a_figure() {
    let mut v = sample("denied_section");
    v["rows"][3]["cells"][7] = json!("12.0000");
    v["rows"][3]["drill"] = json!({"type": "detail", "nodeId": "purch", "path": ["n:purch"]});
    let t = read_run(&v, None).expect("run").table;
    assert_eq!(t.rows[3].cells, [Cell::Empty]);
    assert_eq!(t.rows[3].drill, None);
    let mut v = sample("denied_section");
    let mut leak = v["rows"][2].clone();
    leak["nodeId"] = json!("purch");
    leak["kind"] = json!("subtotal");
    leak["path"] = json!(["n:purch"]);
    leak["cells"] = json!([null, null, null, null, null, null, null, "5.0000"]);
    v["rows"].as_array_mut().expect("rows").push(leak);
    assert_eq!(
        read_run(&v, None).expect_err("a leak"),
        ApiError::ReportShape {
            what: "denied section"
        }
    );
}

#[test]
fn the_expanded_rows_are_the_open_set() {
    let r = run("expandable_open");
    assert_eq!(
        r.expanded,
        [RowPath::of(["n:sales", &format!("contact:{ZERO_ID}")])]
    );
    assert!(r.table.rows[1].expandable);
    assert_eq!(r.table.rows[2].level, 2);
}

#[test]
fn a_denied_section_and_the_formula_over_it() {
    let r = run("denied_section");
    assert_eq!(r.denied, ["purch"]);
    let t = &r.table;
    assert_eq!(t.rows[3].kind, RowKind::Denied);
    assert_eq!(t.rows[3].caption, "@reports.builder.denied");
    assert_eq!(t.rows[3].drill, None);
    // The formula that needs it has no amount (the window shows the dash).
    assert_eq!(t.rows[4].kind, RowKind::Formula);
    assert_eq!(t.rows[4].cells, [Cell::Empty]);
}

#[test]
fn a_detail_page_reads_against_the_runs_columns() {
    let r = run("transaction_detail_by_account");
    // A drill tab reads against every column (the empty ones too).
    let p = read_detail(
        &sample("run_detail_page"),
        &r.detail_columns,
        &r.detail_layout,
    )
    .expect("page");
    assert_eq!(p.rows.len(), 2);
    assert_eq!(p.rows[0].kind, RowKind::Earnings, "the opening row");
    assert_eq!(p.rows[0].level, 0);
    assert!(matches!(
        &p.rows[1].drill,
        Some(Drill::Transaction { txn_type, .. }) if txn_type == "bill"
    ));
    assert_eq!(p.cursor, None);
    assert_eq!(p.subtotal[9], Cell::Amount(d("45.0000")));
    let mut next = sample("run_detail_page");
    next["cursor"] = json!("eyJrIjoibGluZSJ9");
    assert_eq!(
        read_detail(&next, &r.detail_columns, &r.detail_layout)
            .expect("page")
            .cursor
            .as_deref(),
        Some("eyJrIjoibGluZSJ9")
    );
}

#[test]
fn a_more_row_pages_with_its_cursor_or_from_the_start() {
    let mut v = sample("expandable_open");
    let more = |cursor: Value| {
        json!({"kind": "more", "nodeId": "sales", "level": 2,
               "path": ["n:sales", "contact:x"], "caption": "More rows",
               "cells": [null, null, null, null, null, null, null, null], "cursor": cursor,
               "expandable": false, "expanded": false})
    };
    let rows = v["rows"].as_array_mut().expect("rows");
    rows.push(more(json!("abc")));
    rows.push(more(Value::Null));
    v["meta"]["truncated"] = json!(true);
    let r = read_run(&v, None).expect("run");
    assert!(r.truncated);
    let n = r.table.rows.len();
    assert_eq!(r.table.rows[n - 2].caption, "@reports.builder.more");
    assert_eq!(
        r.table.rows[n - 2].drill,
        Some(Drill::BuilderPage {
            node_id: "sales".into(),
            path: RowPath::of(["n:sales", "contact:x"]),
            cursor: Some("abc".into()),
        })
    );
    assert!(matches!(
        &r.table.rows[n - 1].drill,
        Some(Drill::BuilderPage { cursor: None, .. })
    ));
}

#[test]
fn the_no_activity_note_is_worded_by_the_window() {
    let mut v = sample("profit_loss_accrual");
    v["meta"]["notes"] = json!(["No activity for these dates"]);
    let r = read_run(&v, None).expect("run");
    assert_eq!(
        r.table.meta.note.as_deref(),
        Some("@reports.builder.no_activity")
    );
}

#[test]
fn anything_off_contract_is_a_shape_error() {
    let cases: Vec<(&str, Value)> = vec![
        ("/rows/1/cells/0", json!(200)),
        ("/rows/1/cells/0", json!("1,200.00")),
        ("/rows/1/cells/0", json!("1e3")),
        ("/rows/1/kind", json!("chart")),
        ("/rows/1/drill/type", json!("teleport")),
        ("/rows/1/drill/path", json!([])),
        ("/rows/1/level", json!(-1)),
        ("/columns/0/kind", json!("money")),
        ("/stamp", json!("9")),
        ("/meta/truncated", json!({"shown": 1, "total": 2})),
        ("/meta/basis", json!("modified")),
    ];
    for (ptr, bad) in cases {
        let mut v = sample("profit_loss_accrual");
        *v.pointer_mut(ptr).expect(ptr) = bad;
        assert!(
            matches!(read_run(&v, None), Err(ApiError::ReportShape { .. })),
            "{ptr}"
        );
    }
    let mut v = sample("profit_loss_accrual");
    v["rows"][0]["cells"] = json!([]);
    assert!(
        matches!(read_run(&v, None), Err(ApiError::ReportShape { .. })),
        "one cell per column, layout rows too"
    );
    let mut v = sample("transaction_detail_by_account");
    v["columns"][0]["field"] = json!("mood");
    assert!(matches!(
        read_run(&v, None),
        Err(ApiError::ReportShape { .. })
    ));
    let r = run("transaction_detail_by_account");
    let mut v = sample("run_detail_page");
    v["subtotal"] = json!(["45.0000"]);
    assert!(matches!(
        read_detail(&v, &r.table.columns, &r.layout),
        Err(ApiError::ReportShape { .. })
    ));
    let mut v = sample("run_detail_page");
    v["rows"] = json!(vec![v["rows"][1].clone(); PAGE_ROWS + 1]);
    assert!(matches!(
        read_detail(&v, &r.table.columns, &r.layout),
        Err(ApiError::ReportShape { .. })
    ));
}

fn reference() -> RunRef {
    RunRef {
        definition: Some(json!({"schemaVersion": 1, "kind": "builder"})),
        definition_id: None,
        version: None,
        dates: RunDates::range(DateRange {
            from: day(2026, 10, 1),
            to: day(2026, 11, 30),
        }),
        period_ranges: Vec::new(),
        compare_ranges: Vec::new(),
        fiscal_year_starts: vec![day(2026, 1, 1)],
        basis: Some(Basis::Cash),
        adjusting: None,
    }
}

fn request() -> RunRequest {
    RunRequest {
        reference: reference(),
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: false,
        purpose: RunPurpose::Screen,
    }
}

#[test]
fn the_request_wire_shape_is_pinned() {
    assert_eq!(
        serde_json::to_value(request()).expect("json"),
        json!({
            "definition": {"schemaVersion": 1, "kind": "builder"},
            "dates": {"from": "2026-10-01", "to": "2026-11-30"},
            "fiscalYearStarts": ["2026-01-01"],
            "basis": "cash",
            "open": [],
            "collapsed": [],
            "purpose": "screen"
        })
    );
    let mut all = request();
    all.expand_all = true;
    all.collapsed = vec![RowPath::of(["n:a", "account:x"])];
    let v = serde_json::to_value(&all).expect("json");
    assert_eq!(v["expandAll"], json!(true));
    assert_eq!(v["collapsed"], json!([["n:a", "account:x"]]));
    let detail = DetailRequest {
        reference: reference(),
        node_id: "income".into(),
        path: RowPath::of(["n:income", "account:none"]),
        range: None,
        cursor: Some("c".into()),
        fingerprint: "f".repeat(64),
        stamp: 41,
    };
    let v = serde_json::to_value(&detail).expect("json");
    assert_eq!(v["path"], json!(["n:income", "account:none"]));
    assert_eq!(v["nodeId"], "income");
    assert_eq!(v["stamp"], json!(41), "the stamp is a number");
    assert!(v.get("open").is_none() && v.get("purpose").is_none());
    let mut as_of = reference();
    as_of.dates = RunDates::as_of(day(2026, 12, 31));
    assert_eq!(
        serde_json::to_value(&as_of).expect("json")["dates"],
        json!({"asOf": "2026-12-31"})
    );
}

#[test]
fn requests_over_the_caps_are_refused_before_sending() {
    let ok = RunRequest {
        open: vec![RowPath::of(["n:a"])],
        ..request()
    };
    check_run(&ok).expect("ok");
    let range = DateRange {
        from: day(2026, 1, 1),
        to: day(2026, 1, 31),
    };
    let mut periods = ok.clone();
    periods.reference.period_ranges = vec![range; MAX_PERIODS];
    check_run(&periods).expect("36 periods");
    periods.reference.period_ranges.push(range);
    let mut one_compare = ok.clone();
    one_compare.reference.compare_ranges = vec![vec![range]];
    check_run(&one_compare).expect("a compare range without periods");
    let mut compare = ok.clone();
    compare.reference.period_ranges = vec![range; 2];
    compare.reference.compare_ranges = vec![vec![range; 1]];
    let mut deep = ok.clone();
    deep.open = vec![RowPath::of(["a", "b", "c", "d", "e"])];
    let mut empty_path = ok.clone();
    empty_path.collapsed = vec![RowPath::default()];
    let mut many = ok.clone();
    many.open = vec![RowPath::of(["a"]); MAX_OPEN_PATHS + 1];
    let mut many_closed = ok.clone();
    many_closed.collapsed = vec![RowPath::of(["a"]); MAX_OPEN_PATHS + 1];
    let mut both = ok.clone();
    both.reference.definition_id = Some("d1".into());
    let mut no_years = ok.clone();
    no_years.reference.fiscal_year_starts.clear();
    let mut years = ok.clone();
    years.reference.fiscal_year_starts = vec![day(2026, 1, 1); MAX_FISCAL_YEAR_STARTS + 1];
    for bad in [
        periods,
        compare,
        deep,
        empty_path,
        many,
        many_closed,
        both,
        no_years,
        years,
    ] {
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
        r(400, err("CURSOR_INVALID", json!({}))),
        Some(BuilderRefusal::ParamsChanged)
    );
    assert_eq!(
        r(503, err("REPORT_TIMEOUT", json!({}))),
        Some(BuilderRefusal::Timeout)
    );
    assert_eq!(
        r(422, err("REPORT_TOO_LARGE", json!({"limit": 100_000}))),
        Some(BuilderRefusal::TooLarge)
    );
    assert!(matches!(
        r(400, err("DEFINITION_INVALID", json!({"path": "definition.nodes.0.polarity", "code": "MIXED_POLARITY"}))),
        Some(BuilderRefusal::Invalid { path: Some(p), code: Some(c) })
            if p == "definition.nodes.0.polarity" && c == "MIXED_POLARITY"
    ));
    // S-L3: an input key in the engine's path is scrubbed.
    assert!(matches!(
        r(400, err("DEFINITION_INVALID", json!({"path": "definition.columns.headers.Rivera Remodel", "code": "SCHEMA"}))),
        Some(BuilderRefusal::Invalid { path: Some(p), .. })
            if p == "definition.columns.headers.<key>"
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
        r(403, err("PERMISSION_DENIED", json!({}))),
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
    for v in [json!([one.clone()]), json!({"starters": [one.clone()]})] {
        let s = read_starters(&v).expect("starters");
        assert_eq!(s[0].key, "profit_loss");
    }
    assert!(read_starters(&json!([{"key": "x"}])).is_err());
}

#[test]
fn a_row_without_a_path_is_off_contract() {
    // U-119e: open-item detail rows carry their line's path, so a row
    // without one, detail or not, is a shape error (#12).
    let mut v = sample("transaction_detail_by_account");
    v["rows"][3]["path"] = json!([]);
    assert!(matches!(
        read_run(&v, None),
        Err(ApiError::ReportShape { .. })
    ));
    let mut v = sample("transaction_detail_by_account");
    v["rows"][1]["path"] = json!([]);
    assert!(matches!(
        read_run(&v, None),
        Err(ApiError::ReportShape { .. })
    ));
}

/// U-119e: a period column's range goes back as sent; an as-of column's
/// as `from: null`; the total column sends none.
#[test]
fn a_column_drill_sends_the_columns_own_range() {
    let as_of = DateRange {
        from: CalendarDate::min(),
        to: day(2026, 6, 30),
    };
    let mut req = DetailRequest {
        reference: reference(),
        node_id: "assets".into(),
        path: RowPath::of(["n:assets"]),
        range: Some(as_of),
        cursor: None,
        fingerprint: "f".repeat(64),
        stamp: 1,
    };
    let v = serde_json::to_value(&req).expect("json");
    assert_eq!(v["range"], json!({"from": null, "to": "2026-06-30"}));
    req.range = Some(DateRange {
        from: day(2026, 3, 1),
        to: day(2026, 3, 31),
    });
    let v = serde_json::to_value(&req).expect("json");
    assert_eq!(
        v["range"],
        json!({"from": "2026-03-01", "to": "2026-03-31"})
    );
    req.range = None;
    assert!(
        serde_json::to_value(&req)
            .expect("json")
            .get("range")
            .is_none()
    );
    // The run's columns: an as-of balance column keeps its date; the total
    // drills every column.
    let r = run("balance_sheet_accrual");
    for c in &r.table.columns {
        if c.key == "total" {
            assert_eq!(c.range, None);
        }
    }
}

/// U-217: a preview run says its cut in `meta.preview`; screen runs don't.
#[test]
fn a_preview_run_reads_its_cut() {
    let mut v = sample("transaction_detail_by_account");
    assert_eq!(read_run(&v, None).expect("screen").preview_rows, None);
    v["meta"]["preview"] = serde_json::json!({"detailRowsPerLine": 50});
    let r = read_run(&v, None).expect("preview");
    assert_eq!(r.preview_rows, Some(PREVIEW_DETAIL_ROWS));
    v["meta"]["preview"] = serde_json::json!({"detailRowsPerLine": "50"});
    assert!(read_run(&v, None).is_err(), "off contract");
    assert_eq!(
        serde_json::to_value(RunPurpose::Preview).expect("json"),
        serde_json::json!("preview")
    );
}
