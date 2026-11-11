//! Budgets and Budget vs. Actual against the real engine (P28 WP4, U-203
//! `budgets-exact-v1`, AC4).
//!
//! The hand fixture `fixtures/budgets/bva_hand.json` (a copy of the
//! engine's, every expected value worked out by hand) is posted into a
//! company of its own, so no other test's activity reaches its rows or
//! totals and its fiscal year 2026 budget is the company's only one. The
//! fixture's entry for "another company" is left out: the expected values
//! exclude it by design. Its unpaid invoice is posted as an accrual-only
//! entry, which is what the invoice is to the P&L (income on accrual, none
//! on cash).
//!
//! Every Budget vs. Actual cell is read through `Reports::run` (the table the window,
//! the exports and the goldens see) and compared by value with the
//! fixture.

use std::collections::HashMap;
use std::str::FromStr;

use cl_api::resources::reports::budgets::{
    BudgetLineWrite, BudgetRefusal, NewBudget, PERIODS, budget_refusal, parse_percent,
};
use cl_api::{
    AccountType, ApiError, Capabilities, JeLine, NewJournalEntry, Session, Side, TagFields,
    TagGroupFields,
};
use cl_core::Money;
use cl_core::ids::{AccountId, TagGroupId, TagId};
use cl_core::report::{
    Basis, Cell, DateRange, ReportDates, ReportRequest, ReportTable, RowKind, Source,
};
use rust_decimal::Decimal;
use serde_json::Value;

use crate::common::{account_fields, key, m, post};
use crate::support::{Harness, unique};

fn fixture() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/budgets/bva_hand.json");
    serde_json::from_str(&std::fs::read_to_string(p).expect("bva_hand.json")).expect("json")
}

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("decimal")
}

fn cal(s: &str) -> cl_core::CalendarDate {
    cl_core::CalendarDate::parse_wire(s).expect("date")
}

/// The company's books: fixture account number to id, tag key to id, and
/// the tag group ids by name.
struct Books {
    acct: HashMap<String, AccountId>,
    tags: HashMap<String, TagId>,
    groups: HashMap<String, TagGroupId>,
}

fn account_type(t: &str) -> AccountType {
    match t {
        "asset" => AccountType::Asset,
        "revenue" => AccountType::Revenue,
        "cogs" => AccountType::Cogs,
        "expense" => AccountType::Expense,
        "other_revenue" => AccountType::OtherRevenue,
        other => panic!("fixture account type {other}"),
    }
}

fn books(h: &Harness, s: &Session, fx: &Value) -> Books {
    let mut acct = HashMap::new();
    for a in fx["accounts"].as_array().expect("accounts") {
        let number = a["number"].as_str().expect("number").to_string();
        let mut f = account_fields(
            account_type(a["type"].as_str().expect("type")),
            a["name"].as_str().expect("name"),
        );
        // Unique names (two fixture accounts share "Rent"); the chart's
        // own numbers stay free.
        f.name = unique(&format!("{} {number}", f.name));
        if let Some(p) = a["parent"].as_str() {
            f.parent_id = Some(acct[p]);
        }
        let id = h
            .run(s.accounts().create(&f, key()))
            .unwrap_or_else(|e| panic!("account {number}: {e}"))
            .record
            .id;
        acct.insert(number, id);
    }
    let mut tags = HashMap::new();
    let mut groups = HashMap::new();
    for g in fx["tagGroups"].as_array().expect("groups") {
        let name = g["name"].as_str().expect("name");
        let gid = h
            .run(s.tags().create_group(
                &TagGroupFields {
                    name: unique(name),
                    description: None,
                    is_single_select: false,
                },
                key(),
            ))
            .expect("tag group")
            .id;
        groups.insert(name.to_string(), gid);
        for t in g["tags"].as_array().expect("tags") {
            let tid = h
                .run(s.tags().create(
                    &TagFields {
                        name: unique(t["name"].as_str().expect("name")),
                        color: None,
                        group_id: Some(gid),
                        description: None,
                        tag_number: None,
                    },
                    key(),
                ))
                .expect("tag")
                .record
                .id;
            tags.insert(t["key"].as_str().expect("key").to_string(), tid);
        }
    }
    let b = Books { acct, tags, groups };
    for e in fx["entries"].as_array().expect("entries") {
        if e["company"].as_str() == Some("other") {
            continue;
        }
        let basis = if e["txnType"].as_str() == Some("invoice") {
            cl_api::Basis::Accrual
        } else {
            cl_api::Basis::Both
        };
        let lines = e["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|l| {
                let side = match (l["debit"].as_str(), l["credit"].as_str()) {
                    (Some(x), _) => Side::Debit(m(x)),
                    (None, Some(x)) => Side::Credit(m(x)),
                    _ => panic!("line without an amount"),
                };
                JeLine {
                    account: b.acct[l["account"].as_str().expect("account")],
                    side,
                    description: None,
                    tag: l["tag"].as_str().map(|t| b.tags[t]),
                    contact: None,
                }
            })
            .collect();
        let entry = NewJournalEntry::new(
            cal(e["date"].as_str().expect("date")),
            Some(unique("bva fixture")),
            basis,
            lines,
        )
        .expect("balanced");
        post(h, s, &entry);
    }
    b
}

fn run(h: &Harness, s: &Session, r: &ReportRequest) -> ReportTable {
    for _ in 0..10 {
        match h.run(s.reports().run(r)) {
            Ok(t) => return t,
            Err(ApiError::RateLimited { retry_after }) => {
                std::thread::sleep(retry_after.unwrap_or(std::time::Duration::from_secs(15)));
            }
            Err(e) => panic!("BvA: {e}"),
        }
    }
    panic!("BvA: still rate limited")
}

fn bva_req(fx: &Value, budget: &str, basis: Basis, split: Option<TagGroupId>) -> ReportRequest {
    let ranges = fx["request"]["periodRanges"]
        .as_array()
        .expect("ranges")
        .iter()
        .map(|r| {
            let (a, b) = r.as_str().expect("range").split_once("..").expect("..");
            DateRange {
                from: cal(a),
                to: cal(b),
            }
        })
        .collect();
    let mut r = ReportRequest::new(Source::BudgetVsActual, ReportDates::Ranges(ranges));
    r.budget_id = Some(budget.to_string());
    r.first_period = fx["request"]["firstPeriod"]
        .as_u64()
        .and_then(|p| u8::try_from(p).ok());
    r.basis = Some(basis);
    r.tag_split = split;
    r
}

/// Compares one row's cells with the fixture's arrays (four columns of
/// four cells each: actual, budget, over, %).
fn expect_cells(cells: &[Cell], exp: &Value, label: &str) {
    let arr = |k: &str| exp.get(k).and_then(Value::as_array);
    let n = arr("actual").expect("actual").len();
    assert_eq!(cells.len(), n * 4, "{label}: cell count");
    for i in 0..n {
        let at = |j: usize| &cells[i * 4 + j];
        let want = |k: &str| arr(k).map(|a| a[i].clone());
        assert_eq!(
            *at(0),
            Cell::Amount(d(want("actual").expect("a").as_str().expect("s"))),
            "{label} actual[{i}]"
        );
        match want("budget") {
            Some(Value::String(b)) => {
                assert_eq!(*at(1), Cell::Amount(d(&b)), "{label} budget[{i}]");
            }
            Some(Value::Null) => assert_eq!(*at(1), Cell::Empty, "{label} budget[{i}] none"),
            _ => {}
        }
        if let Some(Value::String(o)) = want("over") {
            assert_eq!(*at(2), Cell::Amount(d(&o)), "{label} over[{i}]");
        }
        match want("pct") {
            Some(Value::String(p)) => {
                assert_eq!(*at(3), Cell::Percent(Some(d(&p))), "{label} pct[{i}]");
            }
            Some(Value::Null) => assert_eq!(*at(3), Cell::Percent(None), "{label} pct[{i}] —"),
            _ => {}
        }
    }
}

fn account_row<'t>(
    t: &'t ReportTable,
    kind: RowKind,
    last: &str,
) -> &'t cl_core::report::ReportRow {
    t.rows
        .iter()
        .find(|r| r.kind == kind && r.path.0.last().map(String::as_str) == Some(last))
        .unwrap_or_else(|| panic!("{kind:?} row {last}"))
}

/// Compares a total row with the fixture; a section with no rows has no
/// total row, and then every expected value is zero.
fn expect_total(t: &ReportTable, caption: &str, exp: &Value, label: &str) {
    match t.rows.iter().find(|r| r.caption == caption) {
        Some(r) => expect_cells(&r.cells, exp, label),
        None => assert!(
            ["actual", "budget"].iter().all(|k| exp[*k]
                .as_array()
                .expect("values")
                .iter()
                .all(|v| v.as_str().is_some_and(|s| d(s).is_zero()))),
            "{label}: no row, but the fixture has amounts"
        ),
    }
}

const TOTALS: [(&str, &str); 7] = [
    ("totalRevenue", "@reports.pl.total_income"),
    ("totalCogs", "@reports.pl.total_cogs"),
    ("grossProfit", "@reports.pl.gross_profit"),
    ("totalExpenses", "@reports.pl.total_expenses"),
    ("totalOtherRevenue", "@reports.pl.total_other_income"),
    ("totalOtherExpenses", "@reports.pl.total_other_expenses"),
    ("netIncome", "@reports.pl.net_income"),
];

fn lines_of(fx: &Value, b: &Books) -> Vec<BudgetLineWrite> {
    fx["budget"]["lines"]
        .as_object()
        .expect("lines")
        .iter()
        .map(|(num, cells)| {
            let mut periods = [Money::ZERO; PERIODS];
            for (i, c) in cells.as_array().expect("cells").iter().enumerate() {
                periods[i] = m(c.as_str().expect("cell"));
            }
            BudgetLineWrite {
                account_id: b.acct[num],
                periods,
            }
        })
        .collect()
}

/// AC4: Budget vs. Actual equals the hand fixture, on accrual and cash
/// basis and under a tag split; plus the version, refusals and the
/// engine-side fill, adjust and copy.
#[allow(clippy::too_many_lines, reason = "one fixture, every view of it")]
pub fn hand_fixture_and_writes(h: &Harness) {
    if !h.require_cap(Capabilities::BUDGETS_EXACT, "U-203") {
        return;
    }
    let fx = fixture();
    // A company of its own (switched back by the next `s3()`).
    let s = h.s3();
    let made = h
        .run(s.create_company(&cl_api::NewCompany {
            company_name: unique("BvA Co"),
            business_type: Some("default".into()),
            entity_type: None,
            system_accounts_only: None,
        }))
        .unwrap_or_else(|e| panic!("create company: {e}"));
    h.run(s.open_company(made.tenant_id))
        .unwrap_or_else(|e| panic!("open company: {e}"));
    let b = books(h, &s, &fx);
    let budgets = s.budgets();
    let created = h
        .run(budgets.create(
            &NewBudget {
                name: fx["budget"]["name"].as_str().expect("name").into(),
                fiscal_year: 2026,
            },
            key(),
        ))
        .expect("create budget");
    assert_eq!(
        created.fiscal_year_start,
        Some(cal(fx["fiscalYearStart"].as_str().expect("fy"))),
        "a January fiscal year"
    );
    let saved = h
        .run(budgets.save_lines(&created.id, created.version, &lines_of(&fx, &b), key()))
        .expect("save lines");
    assert!(
        saved.budget.version > created.version,
        "a write bumps the version"
    );
    // The engine's line total: 4000 = 1000 + 1000 + 1500 + 9999.
    let l4000 = saved
        .lines
        .iter()
        .find(|l| l.account_id == b.acct["4000"])
        .expect("4000 line");
    assert_eq!(l4000.total, d("13499"));

    // Accrual.
    let t = run(h, &s, &bva_req(&fx, &created.id, Basis::Accrual, None));
    let acc = &fx["expected"]["accrual"];
    for (num, exp) in acc["rows"].as_object().expect("rows") {
        let r = account_row(&t, RowKind::Account, &b.acct[num].to_string());
        expect_cells(&r.cells, exp, &format!("accrual {num}"));
    }
    for (num, exp) in acc["rollups"].as_object().expect("rollups") {
        let r = account_row(&t, RowKind::AccountTotal, &format!("{}~total", b.acct[num]));
        expect_cells(&r.cells, exp, &format!("rollup {num}"));
    }
    for (child, parent) in acc["parents"].as_object().expect("parents") {
        let r = account_row(&t, RowKind::Account, &b.acct[child].to_string());
        let p = b.acct[parent.as_str().expect("p")].to_string();
        assert!(r.path.0.contains(&p), "{child} sits under {parent}");
    }
    for (key_, caption) in TOTALS {
        expect_total(&t, caption, &acc["totals"][key_], key_);
    }
    // Exactly the fixture's accounts: budgeted or with activity.
    assert_eq!(
        t.rows.iter().filter(|r| r.kind == RowKind::Account).count(),
        acc["rows"].as_object().expect("rows").len()
    );
    // Only the Actual cells drill.
    let sales = account_row(&t, RowKind::Account, &b.acct["4000"].to_string());
    let drill = sales.drill.as_ref().expect("drill");
    assert!(drill.for_cell(&t.columns[0]).is_some());
    assert!(drill.for_cell(&t.columns[1]).is_none());

    // Cash: the accrual-only invoice is not income.
    let t = run(h, &s, &bva_req(&fx, &created.id, Basis::Cash, None));
    let cash = &fx["expected"]["cash"];
    for (num, exp) in cash["rows"].as_object().expect("rows") {
        let r = account_row(&t, RowKind::Account, &b.acct[num].to_string());
        expect_cells(&r.cells, exp, &format!("cash {num}"));
    }
    for (key_, caption) in TOTALS {
        if let Some(exp) = cash["totals"].get(key_) {
            expect_total(&t, caption, exp, &format!("cash {key_}"));
        }
    }

    // Tag split by Region: the budget is on the account total only.
    let split = &fx["expected"]["tagSplit"];
    let group = b.groups[split["group"].as_str().expect("group")];
    let t = run(
        h,
        &s,
        &bva_req(&fx, &created.id, Basis::Accrual, Some(group)),
    );
    for exp in split["rows"].as_array().expect("rows") {
        let id = b.acct[exp["account"].as_str().expect("account")].to_string();
        let r = match exp["rowKind"].as_str().expect("kind") {
            "account" => account_row(&t, RowKind::Account, &id),
            "account_total" => account_row(&t, RowKind::AccountTotal, &format!("{id}~split")),
            _ => {
                let tag = b.tags[exp["tag"].as_str().expect("tag")].to_string();
                account_row(&t, RowKind::TagRow, &format!("t:{tag}"))
            }
        };
        expect_cells(&r.cells, exp, &format!("split {id} {}", exp["rowKind"]));
    }

    // A write at the old version: 409, Reload.
    let stale = h
        .run(budgets.save_lines(&created.id, created.version, &lines_of(&fx, &b), key()))
        .expect_err("stale");
    assert_eq!(budget_refusal(&stale), Some(BudgetRefusal::Stale));
    // A second budget for the year: refused, and not as a stale write.
    let taken = h
        .run(budgets.create(
            &NewBudget {
                name: unique("again"),
                fiscal_year: 2026,
            },
            key(),
        ))
        .expect_err("year taken");
    assert_eq!(budget_refusal(&taken), Some(BudgetRefusal::YearTaken));
    // A Balance Sheet account is not a budget line.
    let mut bad = lines_of(&fx, &b);
    bad.truncate(1);
    bad[0].account_id = b.acct["1000"];
    let e = h
        .run(budgets.save_lines(&created.id, saved.budget.version, &bad, key()))
        .expect_err("not a P&L account");
    assert_eq!(budget_refusal(&e), Some(BudgetRefusal::LineInvalid));

    // FY2027: fill from 2026 (calendar months, cent rounding), adjust and
    // copy, all computed by the engine.
    let next = h
        .run(budgets.create(
            &NewBudget {
                name: unique("FY2027"),
                fiscal_year: 2027,
            },
            key(),
        ))
        .expect("FY2027");
    let filled = h
        .run(budgets.fill_from_actuals(&next.id, next.version, Some("accrual"), key()))
        .expect("fill");
    let f4000 = filled
        .lines
        .iter()
        .find(|l| l.account_id == b.acct["4000"])
        .expect("4000 filled");
    // January 2026: 1234.56 + the 300.00 invoice; April: 5000.00.
    assert_eq!(f4000.periods[0], d("1534.56"));
    assert_eq!(f4000.periods[3], d("5000"));
    let adjusted = h
        .run(budgets.adjust_by_percent(
            &next.id,
            filled.budget.version,
            parse_percent("10").expect("10"),
            key(),
        ))
        .expect("adjust");
    let a4000 = adjusted
        .lines
        .iter()
        .find(|l| l.account_id == b.acct["4000"])
        .expect("4000 adjusted");
    // 1534.56 x 1.1 = 1688.016, to the cent half away from zero.
    assert_eq!(a4000.periods[0], d("1688.02"));
    let copied = h
        .run(budgets.copy_from(&next.id, adjusted.budget.version, &created.id, key()))
        .expect("copy");
    let c4000 = copied
        .lines
        .iter()
        .find(|l| l.account_id == b.acct["4000"])
        .expect("4000 copied");
    assert_eq!(c4000.periods[3], d("9999"));
    assert!(
        h.run(budgets.list())
            .expect("list")
            .iter()
            .any(|x| x.id == next.id),
        "listed"
    );
    h.run(budgets.delete(&next.id)).expect("delete");
}
