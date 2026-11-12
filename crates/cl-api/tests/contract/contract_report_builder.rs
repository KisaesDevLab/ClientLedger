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
