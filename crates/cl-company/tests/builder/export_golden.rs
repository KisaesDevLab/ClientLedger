//! A09 AC1/AC2 (S2, Q-A09h): a Report Builder P&L exported from the golden
//! company in three states (collapsed; Expenses opened; every line opened),
//! each a fresh `purpose: 'export'` run with every `more` row filled
//! through `run/detail`, written as the desktop writes the file
//! (`present` + `cl_print::report_export::to_csv`) under
//! `fixtures/golden/report_builder/`.
//!
//! Cross-check first: every opened line's detail rows sum to the line in
//! every value column, and no `more` row is left. The sums are the test's
//! (compared as `Decimal`); the app never adds figures up.

use std::collections::HashMap;
use std::path::Path;

use cl_api::Session;
use cl_api::resources::report_builder::{
    BuilderRun, DetailRequest, RunDates, RunPurpose, RunRef, RunRequest,
};
use cl_core::report::{Basis, Cell, ColumnKind, DateRange, Drill, ReportTable, RowKind, RowPath};
use cl_model::reports::{ReportOptions, find};
use rust_decimal::Decimal;

use super::{bless, d, stable_ids};

fn amount(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(x) => *x,
        Cell::Empty => Decimal::ZERO,
        other => panic!("not an amount: {other:?}"),
    }
}

/// The value columns (not the detail field columns).
fn value_cols(t: &ReportTable) -> Vec<usize> {
    t.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == ColumnKind::Amount && !c.key.starts_with("f:"))
        .map(|(i, _)| i)
        .collect()
}

/// The P&L starter with every section a line can be opened in.
fn definition(s_def: &serde_json::Value) -> serde_json::Value {
    let mut def = s_def.clone();
    for n in def["nodes"].as_array_mut().into_iter().flatten() {
        if n["type"] == "section" {
            n["display"] = serde_json::json!("expandable");
        }
    }
    def
}

fn reference(def: &serde_json::Value) -> RunRef {
    RunRef {
        definition: Some(def.clone()),
        definition_id: None,
        version: None,
        dates: RunDates::range(DateRange {
            from: d(2026, 10, 1),
            to: d(2026, 10, 31),
        }),
        period_ranges: Vec::new(),
        compare_ranges: Vec::new(),
        fiscal_year_starts: vec![d(2025, 1, 1), d(2026, 1, 1)],
        basis: Some(Basis::Accrual),
        adjusting: None,
    }
}

/// An export run with its `more` rows filled, as the desktop does it.
async fn export(s: &Session, req: &RunRequest) -> BuilderRun {
    let rb = s.report_builder();
    let mut run = loop {
        match rb.run(req).await {
            Err(cl_api::ApiError::RateLimited { retry_after }) => {
                tokio::time::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15))).await;
            }
            other => break other.expect("export run"),
        }
    };
    loop {
        let next = run
            .table
            .rows
            .iter()
            .find_map(|r| match (&r.kind, &r.drill) {
                (
                    RowKind::More,
                    Some(Drill::BuilderPage {
                        node_id,
                        path,
                        cursor: Some(c),
                    }),
                ) => Some((node_id.clone(), path.clone(), c.clone())),
                _ => None,
            });
        let Some((node_id, path, cursor)) = next else {
            break;
        };
        let dr = DetailRequest {
            reference: req.reference.clone(),
            node_id,
            path: path.clone(),
            range: None,
            cursor: Some(cursor),
            fingerprint: run.fingerprint.clone(),
            stamp: run.stamp,
        };
        let page = loop {
            match rb.detail(&dr, &run.table.columns, &run.layout).await {
                Err(cl_api::ApiError::RateLimited { retry_after }) => {
                    tokio::time::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15)))
                        .await;
                }
                other => break other.expect("detail page"),
            }
        };
        // The desktop's splice: the page's rows where the `more` row was.
        let at = run
            .table
            .rows
            .iter()
            .position(|r| r.kind == RowKind::More && r.path == path)
            .expect("the more row");
        let more = run.table.rows.remove(at);
        let mut rows: Vec<_> = page
            .rows
            .into_iter()
            .map(|mut r| {
                r.level = more.level;
                r
            })
            .collect();
        if let Some(c) = page.cursor {
            let mut m = more;
            if let Some(Drill::BuilderPage { cursor, .. }) = m.drill.as_mut() {
                *cursor = Some(c);
            }
            rows.push(m);
        }
        run.table.rows.splice(at..at, rows);
    }
    run
}

/// Every opened line's detail rows sum to it; no `more` row is left.
fn opened_lines_tie(name: &str, run: &BuilderRun) {
    let t = &run.table;
    assert!(
        t.rows.iter().all(|r| r.kind != RowKind::More),
        "{name}: every page is in"
    );
    let cols = value_cols(t);
    let mut checked = 0;
    for line in t
        .rows
        .iter()
        .filter(|r| r.expandable && run.expanded.contains(&r.path))
    {
        let details: Vec<_> = t
            .rows
            .iter()
            .filter(|r| r.kind == RowKind::Detail && r.path == line.path)
            .collect();
        for &c in &cols {
            let sum: Decimal = details.iter().map(|r| amount(&r.cells[c])).sum();
            assert_eq!(
                sum,
                amount(&line.cells[c]),
                "{name}: {} detail sums to its line ({})",
                line.caption,
                t.columns[c].key
            );
        }
        checked += 1;
    }
    if name != "pl_collapsed" {
        assert!(checked > 0, "{name}: a line is open");
    }
}

/// The three exports, cross-checked, compared with (or written as) their
/// goldens. Returns what differs.
pub(super) async fn export_goldens(s: &Session, names: &HashMap<String, String>) -> Vec<String> {
    let list = s.report_builder().starters().await.expect("starters");
    let pl = list
        .iter()
        .find(|x| x.key == "profit_loss")
        .expect("the P&L starter");
    let def = definition(&pl.definition);
    let base = |open: Vec<RowPath>, expand_all: bool| RunRequest {
        reference: reference(&def),
        open,
        collapsed: Vec::new(),
        expand_all,
        purpose: RunPurpose::Export,
    };
    let collapsed = export(s, &base(Vec::new(), false)).await;
    let expenses: Vec<RowPath> = collapsed
        .table
        .rows
        .iter()
        .filter(|r| r.expandable && r.path.0.first().is_some_and(|p| p == "n:expenses"))
        .map(|r| r.path.clone())
        .collect();
    assert!(!expenses.is_empty(), "October has expense lines");
    let opened = export(s, &base(expenses, false)).await;
    let all = export(s, &base(Vec::new(), true)).await;
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/golden/report_builder");
    let options = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
    let mut mismatched = Vec::new();
    for (file, run) in [
        ("pl_collapsed", &collapsed),
        ("pl_expenses_open", &opened),
        ("pl_all_open", &all),
    ] {
        opened_lines_tie(file, run);
        let rows: Vec<usize> = (0..run.table.rows.len()).collect();
        let table = cl_model::reports::present(&run.table, &rows, &options);
        let rows: Vec<usize> = (0..table.rows.len()).collect();
        let head = vec![
            "Profit and Loss".to_string(),
            "10/01/2026 to 10/31/2026".into(),
        ];
        let csv = cl_print::report_export::to_csv(&table, &rows, &head, &|k: &str| k.to_string());
        let csv = stable_ids(&csv.replace("\r\n", "\n"), names);
        let path = dir.join(format!("{file}.csv"));
        if bless() {
            std::fs::create_dir_all(&dir).expect("golden dir");
            std::fs::write(&path, &csv).expect("write golden");
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want.replace("\r\n", "\n") == csv => {}
            Ok(_) => {
                let out = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/golden-reports-actual/report_builder");
                let _ = std::fs::create_dir_all(&out);
                let _ = std::fs::write(out.join(format!("{file}.csv")), &csv);
                mismatched.push(format!("report_builder/{file} differs"));
            }
            Err(_) => mismatched.push(format!("report_builder/{file} is missing")),
        }
    }
    mismatched
}
