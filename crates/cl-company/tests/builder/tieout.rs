//! A07 WP3: the Report Builder starters tie out to the standard reports on
//! the golden company (plan §6). The builder Profit & Loss and Balance
//! Sheet, each account line read through the window's adapter and units,
//! equal the standard reports' account rows, exactly, on both bases; the
//! builder net income equals the standard one.

use std::collections::BTreeMap;

use cl_api::Session;
use cl_api::resources::report_builder::{RunDates, RunPurpose, RunRef, RunRequest, Starter};
use cl_core::CalendarDate;
use cl_core::report::{Basis, Cell, DateRange, ReportTable, RowKind};
use cl_model::reports::{DateChoice, ReportOptions, find};
use rust_decimal::Decimal;

use super::{by_number, d, net_income, run};

fn amount(c: &Cell) -> Decimal {
    match c {
        Cell::Amount(x) => *x,
        other => panic!("not an amount: {other:?}"),
    }
}

/// The builder's account lines by number (`<number> <name>`).
fn lines(t: &ReportTable) -> BTreeMap<String, Decimal> {
    t.rows
        .iter()
        .filter(|r| r.kind == RowKind::Group)
        .map(|r| {
            // `<number> <name>`, or the name alone (as the standard
            // report's key) for an account without a number.
            let n = match r.caption.split_once(' ') {
                Some((num, _)) if num.chars().all(|c| c.is_ascii_digit()) => num.to_string(),
                _ => r.caption.clone(),
            };
            (n, amount(&r.cells[0]))
        })
        .collect()
}

async fn builder_run(s: &Session, st: &Starter, dates: RunDates, basis: Basis) -> ReportTable {
    let req = RunRequest {
        reference: RunRef {
            definition: Some(st.definition.clone()),
            definition_id: None,
            version: None,
            dates,
            period_ranges: Vec::new(),
            compare_ranges: Vec::new(),
            fiscal_year_starts: vec![d(2025, 1, 1), d(2026, 1, 1)],
            basis: Some(basis),
            adjusting: None,
        },
        open: Vec::new(),
        collapsed: Vec::new(),
        expand_all: false,
        purpose: RunPurpose::Screen,
    };
    let r = s
        .report_builder()
        .run(&req)
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", st.key));
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

pub(super) async fn starters_tie_out(s: &Session) {
    let list = s.report_builder().starters().await.expect("starters");
    let pl = starter(&list, "profit_loss");
    let bs = starter(&list, "balance_sheet");
    for basis in [Basis::Accrual, Basis::Cash] {
        for (from, to) in [
            (d(2026, 10, 1), d(2026, 10, 31)),
            (d(2026, 1, 1), d(2026, 12, 31)),
        ] {
            let t = builder_run(s, pl, RunDates::range(DateRange { from, to }), basis).await;
            let mut o = ReportOptions::for_spec(find("profit_loss").expect("p&l"));
            o.dates = DateChoice::Fixed { from, to };
            o.basis = Some(basis);
            let (_, std) = run(s, "profit_loss", &o).await;
            let want = by_number(&std, 0);
            let got = lines(&t);
            assert!(!got.is_empty(), "{basis:?} {from}: builder P&L lines");
            for (n, v) in &got {
                assert_eq!(
                    want.get(n),
                    Some(v),
                    "{basis:?} {from}..{to}: builder P&L account {n}"
                );
            }
            let net = t
                .rows
                .iter()
                .rfind(|r| r.kind == RowKind::Formula)
                .expect("net income");
            assert_eq!(
                amount(&net.cells[0]),
                net_income(s, from, to, basis).await,
                "{basis:?} {from}..{to}: net income"
            );
        }
        let at: CalendarDate = d(2026, 12, 31);
        let t = builder_run(s, bs, RunDates::as_of(at), basis).await;
        let mut o = ReportOptions::for_spec(find("balance_sheet").expect("bs"));
        o.dates = DateChoice::AsOf(at);
        o.basis = Some(basis);
        let (_, std) = run(s, "balance_sheet", &o).await;
        let want = by_number(&std, 0);
        let earnings = |which: &str| {
            t.rows
                .iter()
                .find(|r| {
                    r.kind == RowKind::Earnings && r.path.0.last().is_some_and(|p| p == which)
                })
                .map_or(Decimal::ZERO, |r| amount(&r.cells[0]))
        };
        let prior = earnings("earnings:prior");
        // The builder doesn't fold prior years' net income into the
        // Retained Earnings account (engine PR-C): that one account is its
        // own line plus the prior-years row; every other account is equal.
        let mut folded = 0;
        for (n, v) in lines(&t) {
            let w = *want
                .get(&n)
                .unwrap_or_else(|| panic!("{basis:?}: standard BS has account {n}"));
            if w != v {
                assert_eq!(w, v + prior, "{basis:?}: builder BS account {n}");
                folded += 1;
            }
        }
        assert!(folded <= 1, "{basis:?}: only Retained Earnings folds");
        // The two earnings rows are the year's and the prior years' net
        // income (engine PR-C: not folded into a Retained Earnings row).
        let current = t.rows.iter().find(|r| {
            r.kind == RowKind::Earnings && r.path.0.last().is_some_and(|p| p == "earnings:current")
        });
        if let Some(c) = current {
            assert_eq!(
                amount(&c.cells[0]),
                net_income(s, d(2026, 1, 1), at, basis).await,
                "{basis:?}: net income this year"
            );
        }
    }
}
