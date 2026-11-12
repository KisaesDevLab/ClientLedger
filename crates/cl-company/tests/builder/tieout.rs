//! A07 WP3: every Report Builder starter ties out to its standard report on
//! the golden company (plan §6, review A07 #6). Each starter runs through
//! the window's adapter and units with the same dates, ranges and basis as
//! the standard report (the basis the standard request resolved to), and:
//!
//! - summary starters: every line equals the standard row of the same
//!   account, client, vendor or item in every value column, compared as
//!   the union of both sides (absent is zero, all-zero rows dropped: the
//!   builder hides zero lines), so a line missing on either side fails;
//! - section totals and formulas equal the standard subtotals and totals
//!   (P&L, Balance Sheet), else the last total equals the grand total;
//! - the Balance Sheet's prior years' earnings row is required and folds
//!   into the chart's designated Retained Earnings account only; the
//!   current year's row is required whenever net income isn't zero;
//! - detail starters (ledger, check register, agings, open items): each
//!   group's total equals the standard's, and the open amounts or debits
//!   and credits of the detail lines equal the standard's line for line.
//!
//! Every starter that names a standard report (`tiesTo`) must be tied
//! here: a new starter fails until it is.
//!
//! The test sums lines to compare them with the engine's figures; the app
//! never does.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::resources::report_builder::{
    DetailRequest, RunDates, RunPurpose, RunRef, RunRequest, Starter,
};
use cl_api::{AccountFilter, Session};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_core::report::{
    Basis, Cell, ColumnKind, DateRange, Drill, ReportDates, ReportTable, RowKind,
};
use cl_model::reports::{DateChoice, ReportOptions, find};
use rust_decimal::Decimal;

use super::{d, run};

type Lines = BTreeMap<String, Vec<Decimal>>;

fn amount(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(x) => *x,
        Cell::Empty => Decimal::ZERO,
        other => panic!("not an amount: {other:?}"),
    }
}

/// An entity's key on either side: an account's number (`4000 · Name`,
/// `4000 Name`), "none" for a missing client or item, else the caption.
fn key(caption: &str) -> String {
    let c = caption
        .strip_prefix("@reports.row.total_of ")
        .unwrap_or(caption);
    let c = c
        .strip_prefix('@')
        .filter(|r| r.starts_with('@'))
        .unwrap_or(c);
    match c {
        "No name"
        | "No item"
        | "No account"
        | "@reports.row.no_item"
        | "@reports.row.no_name"
        | "@reports.builder.none.contact"
        | "@reports.builder.none.item" => {
            return "none".into();
        }
        _ => {}
    }
    if c.ends_with("(Prior Years)") {
        return "prior_years".into();
    }
    if let Some((n, _)) = c.split_once(" \u{b7} ") {
        return n.to_string();
    }
    match c.split_once(' ') {
        Some((n, _)) if !n.is_empty() && n.chars().all(|x| x.is_ascii_digit()) => n.to_string(),
        _ => c.to_string(),
    }
}

/// The builder's value columns: amounts that aren't detail fields.
fn builder_cols(t: &ReportTable) -> Vec<usize> {
    t.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == ColumnKind::Amount && !c.key.starts_with("f:"))
        .map(|(i, _)| i)
        .collect()
}

/// The standard report's amount columns.
fn std_cols(t: &ReportTable) -> Vec<usize> {
    t.columns
        .iter()
        .enumerate()
        .filter(|(_, c)| c.kind == ColumnKind::Amount)
        .map(|(i, _)| i)
        .collect()
}

fn col(t: &ReportTable, key: &str) -> usize {
    t.columns
        .iter()
        .position(|c| c.key == key)
        .unwrap_or_else(|| panic!("column {key}"))
}

fn values(r: &cl_core::report::ReportRow, cols: &[usize]) -> Vec<Decimal> {
    cols.iter().map(|&i| amount(&r.cells[i])).collect()
}

fn add(m: &mut Lines, k: String, v: Vec<Decimal>) {
    let e = m.entry(k).or_insert_with(|| vec![Decimal::ZERO; v.len()]);
    for (a, b) in e.iter_mut().zip(v) {
        *a += b;
    }
}

fn drop_zero(mut m: Lines) -> Lines {
    m.retain(|_, v| v.iter().any(|x| !x.is_zero()));
    m
}

/// The builder's lines (one group-by: every `line` row).
fn builder_lines(t: &ReportTable, cols: &[usize]) -> Lines {
    let mut m = Lines::new();
    for r in t.rows.iter().filter(|r| r.kind == RowKind::Group) {
        add(&mut m, key(&r.caption), values(r, cols));
    }
    drop_zero(m)
}

/// The standard report's entity rows (accounts or list rows).
fn std_lines(t: &ReportTable, cols: &[usize]) -> Lines {
    let mut m = Lines::new();
    for r in t
        .rows
        .iter()
        .filter(|r| matches!(r.kind, RowKind::Account | RowKind::Detail))
    {
        add(&mut m, key(&r.caption), values(r, cols));
    }
    drop_zero(m)
}

/// The builder row for section `node`'s total, or formula `node`.
fn builder_total(t: &ReportTable, node: &str, cols: &[usize]) -> Option<Vec<Decimal>> {
    t.rows
        .iter()
        .find(|r| {
            (r.kind == RowKind::Total && r.path.0 == [format!("n:{node}")])
                || (r.kind == RowKind::Formula && r.path.0 == [format!("n:{node}")])
        })
        .map(|r| values(r, cols))
}

fn std_total(t: &ReportTable, path: &str, cols: &[usize]) -> Option<Vec<Decimal>> {
    t.rows
        .iter()
        .find(|r| r.path.0.join("/") == path)
        .map(|r| values(r, cols))
}

fn zeros(n: usize) -> Vec<Decimal> {
    vec![Decimal::ZERO; n]
}

async fn builder_run(
    s: &Session,
    st: &Starter,
    dates: RunDates,
    periods: Vec<DateRange>,
    basis: Basis,
) -> ReportTable {
    let req = RunRequest {
        reference: RunRef {
            definition: Some(st.definition.clone()),
            definition_id: None,
            version: None,
            dates,
            period_ranges: periods,
            compare_ranges: Vec::new(),
            fiscal_year_starts: vec![d(2025, 1, 1), d(2026, 1, 1)],
            basis: Some(basis),
            adjusting: None,
        },
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: true,
        purpose: RunPurpose::Screen,
    };
    let r = s
        .report_builder()
        .run(&req)
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", st.key));
    assert!(!r.truncated, "{}: every detail line is in the run", st.key);
    let text = serde_json::to_string(&st.definition).expect("json");
    let def = cl_model::report_def::parse(&text).expect("a starter parses");
    let mut t = r.table;
    cl_model::report_def::apply_units(&def, &mut t);
    t
}

fn starter<'a>(list: &'a [Starter], key: &str) -> &'a Starter {
    list.iter()
        .find(|s| s.key == key)
        .unwrap_or_else(|| panic!("starter {key}"))
}

/// The standard report `key` with `set` applied, and the basis it ran on.
async fn standard(
    s: &Session,
    key: &str,
    set: impl FnOnce(&mut ReportOptions),
) -> (cl_core::report::ReportRequest, ReportTable) {
    let mut o = ReportOptions::for_spec(find(key).unwrap_or_else(|| panic!("catalog {key}")));
    set(&mut o);
    run(s, key, &o).await
}

fn dates_of(req: &cl_core::report::ReportRequest) -> (RunDates, Vec<DateRange>) {
    match &req.dates {
        ReportDates::Range(r) => (RunDates::range(*r), Vec::new()),
        ReportDates::AsOf(a) => (RunDates::as_of(*a), Vec::new()),
        ReportDates::Ranges(v) => {
            let (first, last) = (v[0], v[v.len() - 1]);
            (
                RunDates::range(DateRange {
                    from: first.from,
                    to: last.to,
                }),
                if v.len() > 1 { v.clone() } else { Vec::new() },
            )
        }
        ReportDates::None => panic!("a dated report"),
    }
}

/// A summary starter against its standard report: lines both ways, then
/// the totals named in `totals` (builder node, standard path), else the
/// last total against the grand row.
async fn summary(
    s: &Session,
    st: &Starter,
    std_key: &str,
    set: impl FnOnce(&mut ReportOptions),
    totals: &[(&str, &str)],
    what: &str,
) {
    // `profit_loss/income,other_income`: only those sections of the
    // standard report (Revenue by Category ties to the P&L, `tiesTo`).
    let (std_key, sections) = match std_key.split_once('/') {
        Some((k, s)) => (k, s.split(',').collect::<Vec<_>>()),
        None => (std_key, Vec::new()),
    };
    let (req, mut std) = standard(s, std_key, set).await;
    let basis = req.basis.unwrap_or(Basis::Accrual);
    let (dates, periods) = dates_of(&req);
    let t = builder_run(s, st, dates, periods, basis).await;
    let (bc, sc) = (builder_cols(&t), std_cols(&std));
    // A % of income column is a percent: only amounts are compared.
    assert_eq!(bc.len(), sc.len(), "{what}: value columns line up");
    if !sections.is_empty() {
        let mut want = zeros(sc.len());
        for sec in &sections {
            if let Some(v) = std_total(&std, &format!("{sec}~total"), &sc) {
                for (a, b) in want.iter_mut().zip(v) {
                    *a += b;
                }
            }
        }
        let last = t
            .rows
            .iter()
            .rfind(|r| r.kind == RowKind::Total)
            .unwrap_or_else(|| panic!("{what}: a total"));
        assert_eq!(values(last, &bc), want, "{what}: total of {sections:?}");
        std.rows.retain(|r| {
            r.path
                .0
                .first()
                .is_some_and(|p| sections.iter().any(|s| p.split('/').next() == Some(*s)))
        });
    }
    let got = builder_lines(&t, &bc);
    assert!(!got.is_empty(), "{what}: builder lines");
    assert_eq!(got, std_lines(&std, &sc), "{what}: lines, both ways");
    if totals.is_empty() && sections.is_empty() {
        let last = t
            .rows
            .iter()
            .rfind(|r| r.kind == RowKind::Total)
            .unwrap_or_else(|| panic!("{what}: a total"));
        let grand = std
            .rows
            .iter()
            .rfind(|r| r.kind == RowKind::Grand)
            .unwrap_or_else(|| panic!("{what}: a grand total"));
        assert_eq!(values(last, &bc), values(grand, &sc), "{what}: total");
    }
    for (node, path) in totals {
        let n = bc.len();
        assert_eq!(
            builder_total(&t, node, &bc).unwrap_or_else(|| zeros(n)),
            std_total(&std, path, &sc).unwrap_or_else(|| zeros(n)),
            "{what}: {node} = {path}"
        );
    }
}

const PL_TOTALS: &[(&str, &str)] = &[
    ("income", "income~total"),
    ("cogs", "cogs~total"),
    ("gross_profit", "gross_profit"),
    ("expenses", "expenses~total"),
    ("net_operating_income", "operating_income"),
    ("other_income", "other_income~total"),
    ("other_expenses", "other_expenses~total"),
    ("net_income", "net_income"),
];

/// The Balance Sheet: every account, the prior years' row folded into the
/// designated Retained Earnings account (required), the year's net income
/// (required when not zero), the section totals and Total L&E.
async fn balance_sheet(s: &Session, st: &Starter, at: CalendarDate, basis: Basis, re: &str) {
    let what = format!("balance sheet {at} {basis:?}");
    let (_, std) = standard(s, "balance_sheet", |o| {
        o.dates = DateChoice::AsOf(at);
        o.basis = Some(basis);
    })
    .await;
    let t = builder_run(s, st, RunDates::as_of(at), Vec::new(), basis).await;
    let (bc, sc) = (builder_cols(&t), std_cols(&std));
    assert_eq!(bc.len(), 1, "{what}: one value column");
    let earnings = |which: &str| {
        t.rows
            .iter()
            .find(|r| r.kind == RowKind::Earnings && r.path.0.last().is_some_and(|p| p == which))
            .map(|r| amount(&r.cells[bc[0]]))
    };
    let prior =
        earnings("earnings:prior").unwrap_or_else(|| panic!("{what}: the prior years' row"));
    let mut got = Lines::new();
    for r in t.rows.iter().filter(|r| r.kind == RowKind::Group) {
        add(&mut got, key(&r.caption), values(r, &bc));
    }
    // The engine doesn't fold: the chart's Retained Earnings account is
    // its own line plus the prior years' row, no other account.
    add(&mut got, re.to_string(), vec![prior]);
    let mut want = std_lines(&std, &sc);
    let net = want
        .remove("@reports.bs.net_income")
        .map_or(Decimal::ZERO, |v| v[0]);
    match earnings("earnings:current") {
        Some(c) => add(&mut got, "net_income".into(), vec![c]),
        None => assert!(net.is_zero(), "{what}: the year's net income row"),
    }
    if !net.is_zero() {
        want.insert("net_income".into(), vec![net]);
    }
    assert_eq!(drop_zero(got), want, "{what}: accounts, both ways");
    for (node, path) in [
        ("assets", "assets~total"),
        ("liabilities", "liabilities~total"),
        ("equity", "equity~total"),
        ("liabilities_equity", "liabilities_and_equity"),
    ] {
        assert_eq!(
            builder_total(&t, node, &bc).unwrap_or_else(|| zeros(1)),
            std_total(&std, path, &sc).unwrap_or_else(|| panic!("{what}: {path}")),
            "{what}: {node}"
        );
    }
}

/// The Trial Balance: debits and credits per account, the prior years'
/// row, the totals.
async fn trial_balance(s: &Session, st: &Starter, at: CalendarDate, basis: Basis) {
    let what = format!("trial balance {at} {basis:?}");
    let (_, std) = standard(s, "trial_balance", |o| {
        o.dates = DateChoice::AsOf(at);
        o.basis = Some(basis);
    })
    .await;
    let t = builder_run(s, st, RunDates::as_of(at), Vec::new(), basis).await;
    let (bc, sc) = (builder_cols(&t), std_cols(&std));
    assert_eq!(bc.len(), 2, "{what}: debit and credit");
    let net = |v: Vec<Decimal>| vec![v[0] - v[1]];
    let mut got = Lines::new();
    for r in t
        .rows
        .iter()
        .filter(|r| matches!(r.kind, RowKind::Group | RowKind::Earnings))
    {
        let k = if r.kind == RowKind::Earnings {
            "prior_years".to_string()
        } else {
            key(&r.caption)
        };
        add(&mut got, k, net(values(r, &bc)));
    }
    let mut want = Lines::new();
    for r in std.rows.iter().filter(|r| r.kind == RowKind::Account) {
        add(&mut want, key(&r.caption), net(values(r, &sc)));
    }
    assert_eq!(
        drop_zero(got),
        drop_zero(want),
        "{what}: accounts, both ways"
    );
    let grand = std.rows.last().expect("grand");
    assert_eq!(
        builder_total(&t, "accounts", &bc).expect("the total"),
        values(grand, &sc),
        "{what}: totals"
    );
}

/// The ledger: each account's net and the sums of its lines' debits and
/// credits equal the General Ledger's account totals.
async fn ledger(s: &Session, st: &Starter, from: CalendarDate, to: CalendarDate, basis: Basis) {
    let what = format!("ledger {from} {basis:?}");
    let (req, std) = standard(s, "general_ledger", |o| {
        o.dates = DateChoice::Fixed { from, to };
        o.basis = Some(basis);
    })
    .await;
    let basis = req.basis.unwrap_or(basis);
    let t = builder_run(
        s,
        st,
        RunDates::range(DateRange { from, to }),
        Vec::new(),
        basis,
    )
    .await;
    let (total, debit, credit) = (col(&t, "total"), col(&t, "f:debit"), col(&t, "f:credit"));
    let mut got = Lines::new();
    let mut line = String::new();
    for r in &t.rows {
        match r.kind {
            RowKind::Group => {
                line = key(&r.caption);
                add(
                    &mut got,
                    line.clone(),
                    vec![amount(&r.cells[total]), Decimal::ZERO, Decimal::ZERO],
                );
            }
            RowKind::Detail => add(
                &mut got,
                line.clone(),
                vec![
                    Decimal::ZERO,
                    amount(&r.cells[debit]),
                    amount(&r.cells[credit]),
                ],
            ),
            _ => {}
        }
    }
    let (sd, sc) = (col(&std, "debit"), col(&std, "credit"));
    let mut want = Lines::new();
    for r in std.rows.iter().filter(|r| r.kind == RowKind::AccountTotal) {
        let (dr, cr) = (amount(&r.cells[sd]), amount(&r.cells[sc]));
        add(&mut want, key(&r.caption), vec![dr - cr, dr, cr]);
    }
    assert_eq!(
        drop_zero(got),
        drop_zero(want),
        "{what}: accounts, both ways"
    );
}

/// The check register of `bank`: its line's net, deposits and payments.
async fn check_register(s: &Session, st: &Starter, bank: AccountId, number: &str) {
    let (from, to) = (d(2026, 10, 1), d(2026, 12, 31));
    let (req, std) = standard(s, "check_register", |o| {
        o.dates = DateChoice::Fixed { from, to };
        o.filters.account_ids = vec![bank];
    })
    .await;
    let basis = req.basis.unwrap_or(Basis::Accrual);
    let t = builder_run(
        s,
        st,
        RunDates::range(DateRange { from, to }),
        Vec::new(),
        basis,
    )
    .await;
    let (total, debit, credit) = (col(&t, "total"), col(&t, "f:debit"), col(&t, "f:credit"));
    let mut in_line = false;
    let (mut net, mut dr, mut cr) = (None, Decimal::ZERO, Decimal::ZERO);
    for r in &t.rows {
        match r.kind {
            RowKind::Group => {
                in_line = key(&r.caption) == number;
                if in_line {
                    net = Some(amount(&r.cells[total]));
                }
            }
            RowKind::Detail if in_line => {
                dr += amount(&r.cells[debit]);
                cr += amount(&r.cells[credit]);
            }
            _ => {}
        }
    }
    let grand = std.rows.last().expect("grand");
    let (dep, pay) = (
        amount(&grand.cells[col(&std, "deposit")]),
        amount(&grand.cells[col(&std, "payment")]),
    );
    assert_eq!(net, Some(dep - pay), "check register {number}: net");
    assert_eq!((dr, cr), (dep, pay), "check register {number}: lines");
}

/// An open-items detail starter: the total and the open amounts line for
/// line.
async fn open_items(s: &Session, st: &Starter, std_key: &str, at: CalendarDate) {
    let what = format!("{} {at}", st.key);
    let (_, std) = standard(s, std_key, |o| o.dates = DateChoice::AsOf(at)).await;
    let t = builder_run(s, st, RunDates::as_of(at), Vec::new(), Basis::Accrual).await;
    let total = col(&t, "total");
    let open = col(&std, "open");
    let mut got: Vec<Decimal> = t
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Detail)
        .map(|r| amount(&r.cells[total]))
        .filter(|x| !x.is_zero())
        .collect();
    let mut want: Vec<Decimal> = std
        .rows
        .iter()
        .filter(|r| r.kind == RowKind::Detail)
        .map(|r| amount(&r.cells[open]))
        .filter(|x| !x.is_zero())
        .collect();
    got.sort();
    want.sort();
    assert_eq!(got, want, "{what}: open items, line for line");
    let grand = std.rows.last().expect("grand");
    let node = t
        .rows
        .iter()
        .find(|r| r.kind == RowKind::Total)
        .unwrap_or_else(|| panic!("{what}: a total"));
    assert_eq!(
        amount(&node.cells[total]),
        amount(&grand.cells[open]),
        "{what}: total"
    );
}

/// An open-items starter grouped by name: each name's open amount and the
/// total equal the balance summary's (zero names hidden on both sides).
async fn open_by_name(s: &Session, st: &Starter, std_key: &str, at: CalendarDate) {
    let what = format!("{} {at}", st.key);
    let (_, std) = standard(s, std_key, |o| o.dates = DateChoice::AsOf(at)).await;
    let t = builder_run(s, st, RunDates::as_of(at), Vec::new(), Basis::Accrual).await;
    let total = col(&t, "total");
    let balance = col(&std, "balance");
    let got = builder_lines(&t, &[total]);
    assert!(!got.is_empty(), "{what}: names");
    assert_eq!(
        got,
        std_lines(&std, &[balance]),
        "{what}: by name, both ways"
    );
    let node = t
        .rows
        .iter()
        .find(|r| r.kind == RowKind::Total)
        .unwrap_or_else(|| panic!("{what}: a total"));
    let grand = std
        .rows
        .iter()
        .rfind(|r| r.kind == RowKind::Grand)
        .unwrap_or_else(|| panic!("{what}: a grand total"));
    assert_eq!(
        amount(&node.cells[total]),
        amount(&grand.cells[balance]),
        "{what}: total"
    );
}

/// The detail of `drill` paged to the end (waiting out the rate limit):
/// its rows and its subtotal (equal on every page).
async fn page_all(
    s: &Session,
    reference: &RunRef,
    run: &cl_api::resources::report_builder::BuilderRun,
    drill: &Drill,
) -> (Vec<cl_core::report::ReportRow>, Vec<Cell>) {
    let Drill::BuilderDetail {
        node_id,
        path,
        range,
    } = drill.clone()
    else {
        panic!("not a builder drill: {drill:?}");
    };
    let mut req = DetailRequest {
        reference: reference.clone(),
        node_id,
        path,
        range,
        cursor: None,
        fingerprint: run.fingerprint.clone(),
        stamp: run.stamp,
    };
    let (mut rows, mut subtotal) = (Vec::new(), None);
    loop {
        let page = loop {
            match s
                .report_builder()
                .detail(&req, &run.table.columns, &run.layout)
                .await
            {
                Err(cl_api::ApiError::RateLimited { retry_after }) => {
                    tokio::time::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15)))
                        .await;
                }
                other => break other.unwrap_or_else(|e| panic!("drill {:?}: {e}", req.path)),
            }
        };
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

/// Every starter's section Totals and first lines drill to detail whose
/// subtotal is the row and whose lines sum to it, in every value column
/// (review #6, U-119e). The sum is the test's, never the app's.
async fn drills_sum_to_their_cells(s: &Session, list: &[Starter]) {
    for st in list {
        let as_of = st.definition["dates"]["mode"] == "asOf";
        let periods: Vec<DateRange> = if !as_of && st.definition["columns"]["periods"] == "month" {
            [(10, 31), (11, 30), (12, 31)]
                .iter()
                .map(|&(m, last)| DateRange {
                    from: d(2026, m, 1),
                    to: d(2026, m, last),
                })
                .collect()
        } else {
            Vec::new()
        };
        let reference = RunRef {
            definition: Some(st.definition.clone()),
            definition_id: None,
            version: None,
            dates: if as_of {
                RunDates::as_of(d(2026, 12, 31))
            } else {
                RunDates::range(DateRange {
                    from: d(2026, 10, 1),
                    to: d(2026, 12, 31),
                })
            },
            period_ranges: periods,
            compare_ranges: Vec::new(),
            fiscal_year_starts: vec![d(2025, 1, 1), d(2026, 1, 1)],
            basis: Some(Basis::Accrual),
            adjusting: None,
        };
        let req = RunRequest {
            reference: reference.clone(),
            open: Vec::new(),
            collapsed: Vec::new(),
            expand_all: false,
            purpose: RunPurpose::Screen,
        };
        let run = s
            .report_builder()
            .run(&req)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", st.key));
        let cols = builder_cols(&run.table);
        let mut drilled = 0;
        let mut lines = 0;
        for row in &run.table.rows {
            let take = match row.kind {
                RowKind::Total => true,
                RowKind::Group if lines < 2 => {
                    lines += 1;
                    true
                }
                _ => false,
            };
            let Some(drill @ Drill::BuilderDetail { .. }) = row.drill.as_ref().filter(|_| take)
            else {
                continue;
            };
            let (rows, subtotal) = page_all(s, &reference, &run, drill).await;
            let sum = |c: usize| -> Decimal {
                rows.iter()
                    .filter(|x| x.kind == RowKind::Detail)
                    .map(|x| amount(&x.cells[c]))
                    .sum()
            };
            let mut seen = Vec::new();
            for &c in &cols {
                let key = &run.table.columns[c].key;
                assert_eq!(
                    subtotal[c], row.cells[c],
                    "{}: {} subtotal ({key})",
                    st.key, row.caption
                );
                // A Trial Balance line shows its net on one side; its lines
                // carry both: debits less credits is the comparison.
                if let Some(base) = key.strip_suffix("debit") {
                    let credit = cols
                        .iter()
                        .copied()
                        .find(|&k| run.table.columns[k].key == format!("{base}credit"))
                        .unwrap_or_else(|| panic!("{key} has its credit column"));
                    seen.push(credit);
                    assert_eq!(
                        sum(c) - sum(credit),
                        amount(&row.cells[c]) - amount(&row.cells[credit]),
                        "{}: {} lines net ({key})",
                        st.key,
                        row.caption
                    );
                } else if !seen.contains(&c) {
                    assert_eq!(
                        sum(c),
                        amount(&row.cells[c]),
                        "{}: {} lines sum ({key})",
                        st.key,
                        row.caption
                    );
                }
            }
            drilled += 1;
        }
        // The ledger and the check register have no Total; the Trial
        // Balance's Total has no drill; every starter has drillable lines.
        assert!(drilled > 0, "{}: something drills", st.key);
    }
}
pub(super) async fn starters_tie_out(s: &Session, bank: AccountId) {
    let list = s.report_builder().starters().await.expect("starters");
    let chart = s
        .accounts()
        .list_all(&AccountFilter::default())
        .await
        .expect("accounts");
    let number = |id: AccountId| {
        chart
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.fields.account_number.clone())
            .expect("a numbered account")
    };
    let re = chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("retained_earnings"))
        .and_then(|a| a.fields.account_number.clone())
        .expect("the chart's Retained Earnings account");
    let mut tied: BTreeSet<&str> = BTreeSet::new();
    let ranges = [
        (d(2026, 10, 1), d(2026, 10, 31)),
        (d(2026, 10, 1), d(2026, 12, 31)),
        (d(2026, 1, 1), d(2026, 12, 31)),
    ];
    for basis in [Basis::Accrual, Basis::Cash] {
        for (from, to) in ranges {
            let fixed = |o: &mut ReportOptions| {
                o.dates = DateChoice::Fixed { from, to };
                o.basis = Some(basis);
            };
            let what = format!("{from}..{to} {basis:?}");
            for (key, std_key, totals) in [
                ("profit_loss", "profit_loss", PL_TOTALS),
                ("profit_loss_pct_income", "profit_loss", PL_TOTALS),
                (
                    "revenue_by_category",
                    "profit_loss/income,other_income",
                    &[][..],
                ),
                ("expenses_by_category", "expenses_by_category", &[][..]),
            ] {
                let st = starter(&list, key);
                summary(s, st, std_key, fixed, totals, &format!("{key} {what}")).await;
                tied.insert(st.key.as_str());
            }
        }
        let st = starter(&list, "profit_loss_by_month");
        summary(
            s,
            st,
            "profit_loss_by_month",
            |o| {
                o.dates = DateChoice::Fixed {
                    from: d(2026, 10, 1),
                    to: d(2026, 12, 31),
                };
                o.basis = Some(basis);
            },
            PL_TOTALS,
            &format!("profit_loss_by_month Q4 {basis:?}"),
        )
        .await;
        tied.insert(st.key.as_str());
        for at in [d(2026, 10, 31), d(2026, 11, 30), d(2026, 12, 31)] {
            let st = starter(&list, "balance_sheet");
            balance_sheet(s, st, at, basis, &re).await;
            tied.insert(st.key.as_str());
            let st = starter(&list, "trial_balance");
            trial_balance(s, st, at, basis).await;
            tied.insert(st.key.as_str());
        }
        let st = starter(&list, "transaction_detail_by_account");
        ledger(s, st, d(2026, 11, 1), d(2026, 11, 30), basis).await;
        tied.insert(st.key.as_str());
    }
    let q4 = |o: &mut ReportOptions| {
        o.dates = DateChoice::Fixed {
            from: d(2026, 10, 1),
            to: d(2026, 12, 31),
        };
    };
    for key in ["sales_by_client", "sales_by_item"] {
        let st = starter(&list, key);
        summary(s, st, key, q4, &[], &format!("{key} Q4")).await;
        tied.insert(st.key.as_str());
    }
    let st = starter(&list, "check_register");
    check_register(s, st, bank, &number(bank)).await;
    tied.insert(st.key.as_str());
    for at in [d(2026, 10, 31), d(2026, 11, 30), d(2026, 12, 31)] {
        for key in [
            "ar_aging_summary",
            "ap_aging_summary",
            "customer_balance_summary",
            "vendor_balance_summary",
            "bank_balances",
        ] {
            let st = starter(&list, key);
            summary(
                s,
                st,
                key,
                |o| o.dates = DateChoice::AsOf(at),
                &[],
                &format!("{key} {at}"),
            )
            .await;
            tied.insert(st.key.as_str());
        }
        // Open Invoices and Unpaid Bills list every open A/R or A/P item,
        // as the agings do; the standard lists list invoice and bill
        // documents only (on the golden company most open items are
        // journal entries), so they tie by name (Q-A07u(8)).
        for key in ["ar_aging_detail", "ap_aging_detail"] {
            let st = starter(&list, key);
            open_items(s, st, key, at).await;
            tied.insert(st.key.as_str());
        }
        // Grouped by name with zero names hidden: each name's open amount
        // is the balance summary's.
        for (key, std_key) in [
            ("open_invoices", "customer_balance_summary"),
            ("unpaid_bills", "vendor_balance_summary"),
        ] {
            let st = starter(&list, key);
            open_by_name(s, st, std_key, at).await;
            tied.insert(st.key.as_str());
        }
    }
    drills_sum_to_their_cells(s, &list).await;
    let untied: Vec<&str> = list
        .iter()
        .filter(|s| s.ties_to.is_some() && !tied.contains(s.key.as_str()))
        .map(|s| s.key.as_str())
        .collect();
    assert!(untied.is_empty(), "starters with no tie-out: {untied:?}");
}
