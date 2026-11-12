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
