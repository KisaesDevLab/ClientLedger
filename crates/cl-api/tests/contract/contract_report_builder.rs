//! Report Builder runs against the real engine (A07, U-119c/d:
//! `report-builder-v1`, `report-builder-starters-v1`).
//!
//! - Every starter runs through the strict request schema and the adapter,
//!   collapsed and with Expand all, so each column and row kind a starter
//!   can produce is read (periods, buckets, percents, tb pairs, earnings).
//! - A detail round trip: the page echoes the line's figures, then a
//!   posting makes the same page `RUN_CHANGED {reason:'books'}` and another
//!   fingerprint `{reason:'params'}`.
//!
//! The postings live in 2034, a year no other contract test posts in.

use cl_api::resources::report_builder::{
    BuilderRefusal, BuilderRun, DetailRequest, RunDates, RunPurpose, RunRef, RunRequest,
    builder_refusal,
};
use cl_api::{AccountType, ApiError, Capabilities, Session};
use cl_core::CalendarDate;
use cl_core::report::{Cell, DateRange, Drill, RowKind};
use serde_json::{Value, json};

use crate::common::{account, date, je2, post};
use crate::support::Harness;

const YEAR: i32 = 2034;

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::REPORT_BUILDER, "U-119c")
        && h.require_cap(Capabilities::REPORT_BUILDER_STARTERS, "U-119d")
        && h.require_cap(Capabilities::REPORT_BUILDER_STARTERS_V2, "U-119f")
}

fn last_day(month: u32) -> CalendarDate {
    let (y, m) = if month == 12 {
        (YEAR + 1, 1)
    } else {
        (YEAR, month + 1)
    };
    date(y, m, 1).add_days(-1).expect("day")
}

/// The run's reference for `definition` over `YEAR`: the period ranges its
/// `columns.periods` asks for, resolved here as the window does.
fn reference(definition: &Value) -> RunRef {
    let as_of = definition["dates"]["mode"] == "asOf";
    let months: &[u32] = match definition["columns"]["periods"].as_str() {
        Some("month") => &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        Some("quarter") => &[1, 4, 7, 10],
        Some("year") => &[1],
        _ => &[],
    };
    let step = match definition["columns"]["periods"].as_str() {
        Some("quarter") => 3,
        Some("year") => 12,
        _ => 1,
    };
    let period_ranges = months
        .iter()
        .map(|&m| DateRange {
            from: date(YEAR, m, 1),
            to: last_day(m + step - 1),
        })
        .collect();
    RunRef {
        definition: Some(definition.clone()),
        definition_id: None,
        version: None,
        dates: if as_of {
            RunDates::as_of(date(YEAR, 12, 31))
        } else {
            RunDates::range(DateRange {
                from: date(YEAR, 1, 1),
                to: date(YEAR, 12, 31),
            })
        },
        period_ranges,
        compare_ranges: Vec::new(),
        fiscal_year_starts: vec![date(YEAR - 1, 1, 1), date(YEAR, 1, 1)],
        basis: None,
        adjusting: None,
    }
}

fn request(reference: RunRef, expand_all: bool) -> RunRequest {
    RunRequest {
        reference,
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all,
        purpose: RunPurpose::Screen,
    }
}

/// A run, waited out through the engine's report rate limit.
fn run(h: &Harness, s: &Session, req: &RunRequest) -> Result<BuilderRun, ApiError> {
    for _ in 0..10 {
        match h.run(s.report_builder().run(req)) {
            Err(ApiError::RateLimited { .. }) => {
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
            other => return other,
        }
    }
    h.run(s.report_builder().run(req))
}

pub fn every_starter_runs_through_the_adapter(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let starters = h.run(s.report_builder().starters()).expect("starters");
    assert_eq!(starters.len(), 20, "PR-D serves 20 starters");
    for st in &starters {
        assert!(
            !st.ties_to.as_deref().unwrap_or("x").is_empty(),
            "{}",
            st.key
        );
        for expand_all in [false, true] {
            let r = run(h, s, &request(reference(&st.definition), expand_all))
                .unwrap_or_else(|e| panic!("{} (expand all {expand_all}): {e}", st.key));
            assert_eq!(r.fingerprint.len(), 64, "{}", st.key);
            assert!(!r.table.columns.is_empty(), "{}", st.key);
            for row in &r.table.rows {
                assert_eq!(row.cells.len(), r.table.columns.len(), "{}", st.key);
            }
            // Expand all shows every expandable row open.
            if expand_all {
                for row in r.table.rows.iter().filter(|x| x.expandable) {
                    assert!(r.expanded.contains(&row.path), "{}", st.key);
                }
            }
        }
    }
}

fn ledger(accounts: &[String]) -> Value {
    json!({
        "schemaVersion": 1, "kind": "builder",
        "dates": {"mode": "range"},
        "columns": {"detailFields": ["date", "memo", "amount", "running_balance"],
                    "compare": [], "headers": {}},
        "nodes": [{
            "type": "section", "id": "ledger", "caption": "Ledger", "source": "gl",
            "measure": "amount", "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": accounts}],
            "groupBy": [{"key": "account"}], "display": "expandable",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    })
}

pub fn detail_pages_echo_and_go_stale(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbBank");
    let supplies = account(h, s, AccountType::Expense, "RbSupplies");
    for (d, amt) in [(3, "10.005"), (4, "20.0000"), (5, "0.1")] {
        post(
            h,
            s,
            &je2(date(YEAR, 3, d), "rb", supplies.id, bank.id, amt),
        );
    }
    let def = ledger(&[supplies.id.to_string(), bank.id.to_string()]);
    let reference = reference(&def);
    let r = run(h, s, &request(reference.clone(), false)).expect("run");
    // Collapsed: lines only, no detail.
    assert!(r.table.rows.iter().all(|x| x.kind != RowKind::Detail));
    let line = r
        .table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Group && x.caption.contains("RbSupplies"))
        .expect("the supplies line");
    let total = r
        .table
        .columns
        .iter()
        .position(|c| c.key == "total")
        .expect("total");
    assert_eq!(
        line.cells[total],
        Cell::Amount(rust_decimal::Decimal::from_str_exact("30.1050").expect("d"))
    );
    let Some(Drill::BuilderDetail { node_id, path, .. }) = line.drill.clone() else {
        panic!("a line drills to its detail: {:?}", line.drill);
    };
    let detail = DetailRequest {
        reference: reference.clone(),
        node_id,
        path,
        range: None,
        cursor: None,
        fingerprint: r.fingerprint.clone(),
        stamp: r.stamp,
    };
    let page = h
        .run(
            s.report_builder()
                .detail(&detail, &r.table.columns, &r.layout),
        )
        .expect("page");
    assert_eq!(page.cursor, None);
    assert_eq!(page.rows[0].kind, RowKind::Earnings, "the opening row");
    assert_eq!(
        page.rows
            .iter()
            .filter(|x| x.kind == RowKind::Detail)
            .count(),
        3
    );
    assert_eq!(page.subtotal[total], line.cells[total], "the echo");
    for row in page.rows.iter().filter(|x| x.kind == RowKind::Detail) {
        assert!(
            matches!(&row.drill, Some(Drill::Transaction { txn_type, .. }) if txn_type == "journal_entry")
        );
    }

    // Another fingerprint: params.
    let mut other = detail.clone();
    other.fingerprint = "0".repeat(64);
    let e = h
        .run(
            s.report_builder()
                .detail(&other, &r.table.columns, &r.layout),
        )
        .expect_err("params");
    assert_eq!(builder_refusal(&e), Some(BuilderRefusal::ParamsChanged));

    // A posting moves the stamp: books.
    post(
        h,
        s,
        &je2(date(YEAR, 3, 9), "rb", supplies.id, bank.id, "1.00"),
    );
    let e = h
        .run(
            s.report_builder()
                .detail(&detail, &r.table.columns, &r.layout),
        )
        .expect_err("books");
    assert_eq!(builder_refusal(&e), Some(BuilderRefusal::BooksChanged));
    let again = run(h, s, &request(reference, false)).expect("rerun");
    assert!(again.stamp > r.stamp);
}

/// Pages a drill to the end: its rows and the (equal on every page)
/// subtotal.
fn page_all(
    h: &Harness,
    s: &Session,
    mut req: DetailRequest,
    r: &BuilderRun,
) -> (Vec<cl_core::report::ReportRow>, Vec<Cell>) {
    let mut rows = Vec::new();
    let mut subtotal = None;
    loop {
        let page = h
            .run(s.report_builder().detail(&req, &r.table.columns, &r.layout))
            .unwrap_or_else(|e| panic!("drill {:?}: {e}", req.path));
        if let Some(sub) = &subtotal {
            assert_eq!(sub, &page.subtotal, "every page echoes one subtotal");
        }
        subtotal = Some(page.subtotal);
        rows.extend(page.rows);
        match page.cursor {
            Some(c) => req.cursor = Some(c),
            None => return (rows, subtotal.unwrap_or_default()),
        }
    }
}

fn sum(rows: &[cl_core::report::ReportRow], col: usize) -> rust_decimal::Decimal {
    rows.iter()
        .filter(|x| x.kind == RowKind::Detail)
        .map(|x| match x.cells[col] {
            Cell::Amount(a) => a,
            _ => rust_decimal::Decimal::ZERO,
        })
        .sum()
}

/// U-119e (`report-builder-drill-v1`): a section Total drills to every line
/// of the section and a period column, range or as-of, to that column's
/// lines; the subtotal shows that column and the total, and the lines sum
/// to the cell.
pub fn totals_and_columns_drill_to_their_lines(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::REPORT_BUILDER_DRILL, "U-119e") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbDrillBank");
    let fuel = account(h, s, AccountType::Expense, "RbDrillFuel");
    let tools = account(h, s, AccountType::Expense, "RbDrillTools");
    for (m, d, acct, amt) in [
        (3, 2, fuel.id, "12.3456"),
        (3, 9, tools.id, "40.00"),
        (4, 1, fuel.id, "7.01"),
        (6, 5, tools.id, "0.0050"),
    ] {
        post(h, s, &je2(date(YEAR, m, d), "rbd", acct, bank.id, amt));
    }
    let section = |measure: &str, ids: &[String], display: &str| {
        json!({
            "type": "section", "id": "sec", "caption": "Section", "source": "gl",
            "measure": measure, "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": ids}],
            "groupBy": [{"key": "account"}], "display": display,
            "defaultExpanded": false, "showTotal": true, "hideZero": true
        })
    };
    let def = |mode: &str, measure: &str, ids: &[String]| {
        json!({
            "schemaVersion": 1, "kind": "builder", "dates": {"mode": mode},
            "columns": {"detailFields": ["date", "amount"], "periods": "month",
                        "compare": [], "headers": {}},
            "nodes": [section(measure, ids, "summary")]
        })
    };
    // Range: monthly P&L-style columns over two expense accounts.
    let expenses = def(
        "range",
        "amount",
        &[fuel.id.to_string(), tools.id.to_string()],
    );
    let reference_r = reference(&expenses);
    let r = run(h, s, &request(reference_r.clone(), false)).expect("run");
    let total = r
        .table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Total)
        .expect("the section total");
    let Some(drill @ Drill::BuilderDetail { .. }) = total.drill.clone() else {
        panic!("the Total drills: {:?}", total.drill);
    };
    let cols = &r.table.columns;
    let tcol = cols.iter().position(|c| c.key == "total").expect("total");
    let detail = |d: &Drill| {
        let Drill::BuilderDetail {
            node_id,
            path,
            range,
        } = d.clone()
        else {
            panic!("{d:?}")
        };
        DetailRequest {
            reference: reference_r.clone(),
            node_id,
            path,
            range,
            cursor: None,
            fingerprint: r.fingerprint.clone(),
            stamp: r.stamp,
        }
    };
    // The Total (its total column has no range: every column's lines).
    let (rows, subtotal) = page_all(h, s, detail(&drill.at(&cols[tcol])), &r);
    assert_eq!(subtotal, total.cells, "the Total's subtotal is its row");
    assert_eq!(
        Cell::Amount(sum(&rows, tcol)),
        total.cells[tcol],
        "the lines sum to the Total"
    );
    // March only.
    let march = cols
        .iter()
        .position(|c| c.range.is_some_and(|x| x.from == date(YEAR, 3, 1)))
        .expect("the March column");
    let (rows, subtotal) = page_all(h, s, detail(&drill.at(&cols[march])), &r);
    assert_eq!(rows.iter().filter(|x| x.kind == RowKind::Detail).count(), 2);
    assert_eq!(subtotal[march], total.cells[march], "March's cell");
    assert_eq!(subtotal[tcol], total.cells[march], "the total holds March");
    assert_eq!(Cell::Amount(sum(&rows, march)), total.cells[march]);
    for (i, c) in subtotal.iter().enumerate() {
        if i != march && i != tcol && cols[i].kind == cl_core::report::ColumnKind::Amount {
            assert_eq!(*c, Cell::Empty, "{} is not March", cols[i].key);
        }
    }
    // A range that matches no column is refused.
    let mut bad = detail(&drill);
    bad.range = Some(DateRange {
        from: date(YEAR, 3, 2),
        to: date(YEAR, 3, 31),
    });
    let e = h
        .run(s.report_builder().detail(&bad, cols, &r.layout))
        .expect_err("no such column");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");

    // As of: month-end balances of the bank; the April column drills.
    let balances = def("asOf", "balance", &[bank.id.to_string()]);
    let reference_a = reference(&balances);
    let a = run(h, s, &request(reference_a.clone(), false)).expect("as-of run");
    let line = a
        .table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Group)
        .expect("the bank line");
    let april = a
        .table
        .columns
        .iter()
        .position(|c| c.range.is_some_and(|x| x.to == date(YEAR, 4, 30)))
        .expect("the April column");
    assert_eq!(
        a.table.columns[april].range.map(|x| x.from),
        Some(CalendarDate::min()),
        "an as-of column runs from the beginning"
    );
    let Some(Drill::BuilderDetail {
        node_id,
        path,
        range,
    }) = line.drill.as_ref().map(|d| d.at(&a.table.columns[april]))
    else {
        panic!("the bank line drills");
    };
    let req = DetailRequest {
        reference: reference_a,
        node_id,
        path,
        range,
        cursor: None,
        fingerprint: a.fingerprint.clone(),
        stamp: a.stamp,
    };
    let (rows, subtotal) = page_all(h, s, req, &a);
    assert_eq!(subtotal[april], line.cells[april], "April's balance");
    assert_eq!(
        rows.iter().filter(|x| x.kind == RowKind::Detail).count(),
        3,
        "the lines through April"
    );
    assert_eq!(Cell::Amount(sum(&rows, april)), line.cells[april]);
}

/// A one-section `gl` definition over `accounts`, grouped by account, with
/// one compare set on `base` (A08).
fn compared(accounts: &[String], as_of: bool, base: Option<&str>) -> Value {
    let compare: Vec<Value> = base
        .map(|b| json!({"base": b, "amount": true, "dollarChange": true, "percentChange": false}))
        .into_iter()
        .collect();
    json!({
        "schemaVersion": 1, "kind": "builder",
        "dates": {"mode": if as_of { "asOf" } else { "range" }},
        "columns": {"detailFields": [], "compare": compare, "headers": {}},
        "nodes": [{
            "type": "section", "id": "s1", "caption": "Accounts", "source": "gl",
            "measure": if as_of { "balance" } else { "amount" }, "polarity": "debit",
            "filters": [{"field": "account", "op": "in", "ids": accounts}],
            "groupBy": [{"key": "account"}], "display": "summary",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        }]
    })
}

fn plain(definition: Value, dates: RunDates, starts: Vec<CalendarDate>) -> RunRef {
    RunRef {
        definition: Some(definition),
        definition_id: None,
        version: None,
        dates,
        period_ranges: Vec::new(),
        compare_ranges: Vec::new(),
        fiscal_year_starts: starts,
        basis: None,
        adjusting: None,
    }
}

/// Every group and subtotal row's cell under `key`, by path.
fn cells_by_path(r: &BuilderRun, key: &str) -> Vec<(cl_core::report::RowPath, Cell)> {
    let col = r
        .table
        .columns
        .iter()
        .position(|c| c.key == key)
        .unwrap_or_else(|| panic!("column {key}"));
    r.table
        .rows
        .iter()
        .filter(|x| matches!(x.kind, RowKind::Group | RowKind::Subtotal))
        .map(|x| (x.path.clone(), x.cells[col].clone()))
        .collect()
}

/// The compare ranges and fiscal year starts the desktop sends for the run
/// `name` of `fixtures/report_builder/compare_ranges.json` (cl-model's
/// `ranges::run_ranges` output, checked there; cl-api can't depend on
/// cl-model).
fn desktop_ranges(name: &str) -> (Vec<Vec<DateRange>>, Vec<cl_core::CalendarDate>) {
    let all: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../fixtures/report_builder/compare_ranges.json"
    ))
    .expect("fixture");
    let case = &all[name];
    (
        serde_json::from_value(case["compareRanges"].clone()).expect("compare ranges"),
        serde_json::from_value(case["fiscalYearStarts"].clone()).expect("starts"),
    )
}

/// A08 (Q-A07u(2)): the desktop's `compareRanges` are what the engine
/// compares with. October's prior period is September 1-30 (not the
/// engine's same-length shift, August 31 - September 30): the compare
/// cells equal the totals of a separate run over September, string for
/// string, and an August 31 posting stays out of them.
pub fn prior_period_compare_ranges_are_used(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbCmpBank");
    let rent = account(h, s, AccountType::Expense, "RbCmpRent");
    for (m, d, amt) in [
        (8, 31, "7.00"),
        (9, 1, "100.25"),
        (9, 30, "0.0050"),
        (10, 15, "40.00"),
    ] {
        post(h, s, &je2(date(YEAR, m, d), "rb", rent.id, bank.id, amt));
    }
    let ids = [rent.id.to_string(), bank.id.to_string()];
    // What the desktop computes for this run (cl-model ranges::run_ranges,
    // checked against the fixture there, R12).
    let (compare_ranges, starts) = desktop_ranges("prior_period_october");
    let mut october = plain(
        compared(&ids, false, Some("prior_period")),
        RunDates::range(DateRange {
            from: date(YEAR, 10, 1),
            to: date(YEAR, 10, 31),
        }),
        starts.clone(),
    );
    october.compare_ranges = compare_ranges;
    let a = run(h, s, &request(october, false)).expect("october with compare");
    let september = plain(
        compared(&ids, false, None),
        RunDates::range(DateRange {
            from: date(YEAR, 9, 1),
            to: date(YEAR, 9, 30),
        }),
        starts,
    );
    let b = run(h, s, &request(september, false)).expect("september");
    let compare = cells_by_path(&a, "total:c0");
    let totals = cells_by_path(&b, "total");
    assert_ne!(compare.len(), 0);
    assert_eq!(compare, totals, "compare cells = September's totals");
    let rent_line = a
        .table
        .rows
        .iter()
        .find(|x| x.kind == RowKind::Group && x.caption.contains("RbCmpRent"))
        .expect("rent line");
    let col = a
        .table
        .columns
        .iter()
        .position(|c| c.key == "total:c0")
        .expect("compare column");
    assert_eq!(
        rent_line.cells[col],
        Cell::Amount(rust_decimal::Decimal::from_str_exact("100.2550").expect("d")),
        "August 31 is not in the period before October"
    );
}

/// A08: an as-of balance report compared with the same day a year before
/// equals a separate as-of run at that day.
pub fn prior_year_as_of_compare_ranges_are_used(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbYoyBank");
    let savings = account(h, s, AccountType::Asset, "RbYoySavings");
    for (m, d, amt) in [(2, 10, "250.00"), (3, 31, "0.0001"), (6, 1, "99.99")] {
        post(h, s, &je2(date(YEAR, m, d), "rb", savings.id, bank.id, amt));
    }
    let ids = [savings.id.to_string(), bank.id.to_string()];
    let (compare_ranges, starts) = desktop_ranges("prior_year_as_of");
    let mut now = plain(
        compared(&ids, true, Some("prior_year")),
        RunDates::as_of(date(YEAR + 1, 3, 31)),
        starts.clone(),
    );
    now.compare_ranges = compare_ranges;
    let a = run(h, s, &request(now, false)).expect("as of with compare");
    let then = plain(
        compared(&ids, true, None),
        RunDates::as_of(date(YEAR, 3, 31)),
        starts,
    );
    let b = run(h, s, &request(then, false)).expect("a year before");
    let compare = cells_by_path(&a, "total:c0");
    assert_ne!(compare.len(), 0);
    assert_eq!(compare, cells_by_path(&b, "total"));
}

/// A08 (Q-A08c, Q-310): a builder definition is memorized with `canEdit`,
/// changed at its version, refused when stale or named like another, and
/// its kind can't change.
pub fn builder_definitions_save_and_refuse(h: &Harness) {
    use cl_api::resources::reports::definitions::{
        DefinitionRefusal, DefinitionWrite, definition_refusal,
    };
    if !caps(h) || !h.require_cap(Capabilities::REPORT_DEFS_BUILDER, "U-119c") {
        return;
    }
    let s = &h.s1;
    let defs = s.report_definitions();
    let bank = account(h, s, AccountType::Asset, "RbSaveBank");
    let body = compared(&[bank.id.to_string()], false, Some("prior_period"));
    let name = crate::support::unique("Builder P&L");
    let w = DefinitionWrite {
        name: name.clone(),
        group_id: None,
        kind: "builder".into(),
        definition: body.clone(),
        origin: None,
    };
    let key = cl_api::IdempotencyKey::new();
    let row = h.run(defs.create(&w, key.clone())).expect("create");
    assert_eq!(row.kind, "builder");
    assert!(row.can_edit, "the author may change it");
    assert_eq!(row.definition, body);
    // The same press again (the same key) stores no second row.
    let replay = h.run(defs.create(&w, key)).expect("replay");
    assert_eq!(replay.id, row.id);

    let mut changed = w.clone();
    changed.definition["columns"]["compare"] = json!([]);
    let next = h
        .run(defs.update(&row.id, row.version, &changed))
        .expect("update");
    assert!(next.version > row.version);
    assert!(next.can_edit);
    let stale = h
        .run(defs.update(&row.id, row.version, &changed))
        .expect_err("stale");
    assert_eq!(definition_refusal(&stale), Some(DefinitionRefusal::Stale));

    // Another report with the same name: NAME_TAKEN.
    let again = h
        .run(defs.create(&w, cl_api::IdempotencyKey::new()))
        .expect_err("name taken");
    assert_eq!(
        definition_refusal(&again),
        Some(DefinitionRefusal::NameTaken)
    );

    // The kind can't change.
    let standard = DefinitionWrite {
        name,
        group_id: None,
        kind: "standard".into(),
        definition: json!({"schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
                           "options": {"dates": {"preset": "last_month"}}}),
        origin: None,
    };
    h.run(defs.update(&row.id, next.version, &standard))
        .expect_err("a kind change is refused");
    let read = h.run(defs.get(&row.id)).expect("get");
    assert_eq!(read.kind, "builder");
    assert!(read.can_edit);

    // An invalid builder body says where.
    let mut bad = w.clone();
    bad.name = crate::support::unique("Bad builder");
    bad.definition["nodes"][0]["polarity"] = json!("natural");
    bad.definition["nodes"][0]["filters"] = json!([]);
    let e = h
        .run(defs.create(&bad, cl_api::IdempotencyKey::new()))
        .expect_err("invalid");
    assert!(
        matches!(
            definition_refusal(&e),
            Some(DefinitionRefusal::Invalid {
                path: Some(_),
                code: Some(_)
            })
        ),
        "{:?}",
        definition_refusal(&e)
    );
    h.run(defs.delete(&row.id, read.version)).expect("delete");
}

/// U-217 (`report-builder-preview-v1`): a preview run is the screen run
/// cut at 50 transactions per line, its "more" row marking the cut, with
/// the same fingerprint and the same rows up to the cut (AC8).
pub fn a_preview_run_is_the_screen_run_cut_at_fifty(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::REPORT_BUILDER_PREVIEW, "U-217") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbPrevBank");
    let fuel = account(h, s, AccountType::Expense, "RbPrevFuel");
    for i in 0..55 {
        let day = date(YEAR, 5, 1 + (i % 28));
        post(h, s, &je2(day, "rb", fuel.id, bank.id, "1.25"));
    }
    let mut def = ledger(&[fuel.id.to_string()]);
    def["nodes"][0]["display"] = json!("detail");
    def["nodes"][0]["defaultExpanded"] = json!(true);
    def["columns"]["detailFields"] = json!(["date", "memo", "amount"]);
    let reference = reference(&def);
    let whole = run(h, s, &request(reference.clone(), false)).expect("screen run");
    let mut req = request(reference, false);
    req.purpose = RunPurpose::Preview;
    let cut = run(h, s, &req).expect("preview run");
    assert_eq!(cut.preview_rows, Some(50));
    assert_eq!(whole.preview_rows, None);
    assert!(cut.truncated);
    assert_eq!(
        cut.fingerprint, whole.fingerprint,
        "purpose is not a parameter"
    );
    assert_eq!(cut.table.columns, whole.table.columns);
    let more = cut
        .table
        .rows
        .iter()
        .position(|r| r.kind == RowKind::More)
        .expect("the cut line's more row");
    let details = cut.table.rows[..more]
        .iter()
        .filter(|r| r.kind == RowKind::Detail)
        .count();
    assert_eq!(details, 50);
    for i in 0..more {
        let (a, b) = (&cut.table.rows[i], &whole.table.rows[i]);
        assert_eq!(
            (a.kind, &a.path, &a.cells),
            (b.kind, &b.path, &b.cells),
            "row {i}"
        );
    }
    // The totals after the cut are the full run's too.
    let total = |r: &BuilderRun| {
        r.table
            .rows
            .iter()
            .rfind(|x| x.kind == RowKind::Total)
            .map(|x| x.cells.clone())
    };
    assert_eq!(total(&cut), total(&whole));
}

/// U-218 (`report-builder-formula-sign-v1`, Q-A08ai): a formula over an
/// Expenses section shown "Credits as positive" equals the same formula
/// with the natural sign; only the Expenses rows change sign (A09 E1).
pub fn a_flipped_section_leaves_its_formula_alone(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::REPORT_BUILDER_FORMULA_SIGN, "U-218") {
        return;
    }
    let s = &h.s1;
    let bank = account(h, s, AccountType::Asset, "RbSignBank");
    let sales = account(h, s, AccountType::Revenue, "RbSignSales");
    let rent = account(h, s, AccountType::Expense, "RbSignRent");
    post(
        h,
        s,
        &je2(date(YEAR, 6, 3), "rb", bank.id, sales.id, "900.00"),
    );
    post(
        h,
        s,
        &je2(date(YEAR, 6, 9), "rb", rent.id, bank.id, "350.25"),
    );
    let section = |id: &str, acct: String, polarity: Option<&str>| {
        json!({
            "type": "section", "id": id, "caption": id, "source": "gl",
            "measure": "amount", "polarity": polarity,
            "filters": [{"field": "account", "op": "in", "ids": [acct]}],
            "groupBy": [{"key": "account"}], "display": "summary",
            "defaultExpanded": false, "showTotal": true, "hideZero": false
        })
    };
    let def = |flip: bool| {
        json!({
            "schemaVersion": 1, "kind": "builder",
            "dates": {"mode": "range"},
            "columns": {"detailFields": [], "compare": [], "headers": {}},
            "nodes": [
                section("income", sales.id.to_string(), None),
                section("expenses", rent.id.to_string(), flip.then_some("credit")),
                {"type": "formula", "id": "net", "caption": "Net",
                 "terms": [{"nodeId": "income", "sign": 1}, {"nodeId": "expenses", "sign": -1}]}
            ]
        })
    };
    let total = |r: &BuilderRun, kind: RowKind, node: &str| {
        r.table
            .rows
            .iter()
            .find(|x| x.kind == kind && x.path.0.first().is_some_and(|p| p == &format!("n:{node}")))
            .map_or_else(|| panic!("{node} {kind:?}"), |x| x.cells[0].clone())
    };
    let natural = run(h, s, &request(reference(&def(false)), false)).expect("natural");
    let flipped = run(h, s, &request(reference(&def(true)), false)).expect("flipped");
    assert_eq!(
        total(&natural, RowKind::Formula, "net"),
        total(&flipped, RowKind::Formula, "net"),
        "the formula ignores display polarity"
    );
    let amount = |c: Cell| match c {
        Cell::Amount(d) => d,
        other => panic!("not an amount: {other:?}"),
    };
    assert_eq!(
        amount(total(&flipped, RowKind::Total, "expenses")),
        -amount(total(&natural, RowKind::Total, "expenses")),
        "the flipped section's own total shows the other sign"
    );
}
