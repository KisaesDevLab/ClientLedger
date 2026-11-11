//! The demo engine's report store (P28 WP5): memorized reports, budgets
//! with Budget vs. Actual, report email (nothing leaves the computer) and a
//! stub "ask a question" mapper. Every answer goes through the live
//! adapters and checks (`bva::table`, `ask::validate`), so the demo window
//! runs the same code as a live one. The stub mapper picks a report and
//! dates only; it never sees or sets an amount (non-negotiable 4).
//! Synthetic.

use std::collections::BTreeMap;

use cl_api::resources::reports::ask::{AskMapping, AskRequest, validate};
use cl_api::resources::reports::budgets::{
    Budget, BudgetLine, BudgetLineWrite, BudgetLines, BudgetTotals, NewBudget, PERIODS,
    PeriodTotals,
};
use cl_api::resources::reports::definitions::{DefinitionGroup, DefinitionRow, DefinitionWrite};
use cl_api::resources::reports::email::{EmailSent, ReportEmail};
use cl_api::{Account, AccountType, ApiError, LocalValidation};
use cl_core::ids::AccountId;
use cl_core::report::{Basis, DateRange, ReportDates, ReportRequest, ReportTable};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::DemoState;
use super::reports::{Books, percent1};

/// One budget and its lines (account → the twelve period amounts).
#[derive(Clone, Debug)]
pub struct DemoBudget {
    /// The budget.
    pub budget: Budget,
    /// Its lines.
    pub lines: BTreeMap<AccountId, [Money; PERIODS]>,
}

/// What the demo engine keeps for the report window.
#[derive(Clone, Debug, Default)]
pub struct DemoReportStore {
    /// The memorized reports.
    pub definitions: Vec<DefinitionRow>,
    /// Their groups.
    pub groups: Vec<DefinitionGroup>,
    /// The budgets.
    pub budgets: Vec<DemoBudget>,
    /// Report emails "sent" (the demo sends nothing).
    pub emails_sent: u32,
    seeded: bool,
    next: u32,
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn not_found() -> ApiError {
    ApiError::Local(LocalValidation::Unsupported)
}

/// The engine's refusal `code` (the live mapping reads it).
fn refusal(status: u16, code: &str) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code}}),
    )
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or(Money::ZERO)
}

/// The sample budget's monthly amounts by account number (round figures
/// near the sample's own months).
const SEED_BUDGET: [(&str, &str); 8] = [
    ("4000", "85000.00"),
    ("5200", "30000.00"),
    ("6100", "2200.00"),
    ("6300", "4350.00"),
    ("6350", "4000.00"),
    ("6550", "12000.00"),
    ("6560", "1100.00"),
    ("6700", "2500.00"),
];

impl DemoReportStore {
    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("demo-{prefix}-{}", self.next)
    }

    /// Seeds the mockup's memorized report and the sample budget once.
    fn seed(&mut self, chart: &[Account]) {
        if std::mem::replace(&mut self.seeded, true) {
            return;
        }
        if let Some(spec) = cl_model::reports::find("profit_loss") {
            let mut o = cl_model::reports::ReportOptions::for_spec(spec);
            o.columns = cl_model::reports::ColumnBy::Month;
            o.dates =
                cl_model::reports::DateChoice::Preset(cl_model::reports::DatePreset::YearToDate);
            let body =
                serde_json::to_value(cl_model::reports::StandardDefinition::new("profit_loss", o))
                    .unwrap_or(Value::Null);
            let id = self.id("report");
            self.definitions.push(DefinitionRow {
                id,
                group_id: None,
                name: "Monthly P&L for the bank".into(),
                kind: "standard".into(),
                definition: body,
                version: 1,
                created_by: None,
                created_by_name: Some("Kurt".into()),
                updated_at: Some(DemoState::now()),
            });
        }
        let mut lines = BTreeMap::new();
        for (number, amount) in SEED_BUDGET {
            if let Some(a) = chart
                .iter()
                .find(|a| a.fields.account_number.as_deref() == Some(number))
            {
                lines.insert(a.id, [money(amount); PERIODS]);
            }
        }
        let id = self.id("budget");
        self.budgets.push(DemoBudget {
            budget: Budget {
                id,
                name: "2026 budget".into(),
                fiscal_year: 2026,
                fiscal_year_start: CalendarDate::from_ymd(2026, 1, 1).ok(),
                tag_id: None,
                version: 1,
            },
            lines,
        });
    }

    fn budget(&self, id: &str) -> Result<&DemoBudget, ApiError> {
        self.budgets
            .iter()
            .find(|b| b.budget.id == id)
            .ok_or_else(not_found)
    }

    fn budget_mut(&mut self, id: &str, version: u64) -> Result<&mut DemoBudget, ApiError> {
        let b = self
            .budgets
            .iter_mut()
            .find(|b| b.budget.id == id)
            .ok_or_else(not_found)?;
        if b.budget.version != version {
            return Err(refusal(409, "STALE_VERSION"));
        }
        Ok(b)
    }
}

fn store(d: &mut DemoState) -> &mut DemoReportStore {
    let chart = d.chart.clone().unwrap_or_else(super::chart);
    d.report_store.seed(&chart);
    &mut d.report_store
}

// --- Memorized reports -------------------------------------------------------

/// The memorized reports and their groups.
pub fn definitions(d: &mut DemoState) -> (Vec<DefinitionRow>, Vec<DefinitionGroup>) {
    let s = store(d);
    (s.definitions.clone(), s.groups.clone())
}

/// One memorized report.
///
/// # Errors
/// `Unsupported` for an unknown id.
pub fn definition(d: &mut DemoState, id: &str) -> Result<DefinitionRow, ApiError> {
    store(d)
        .definitions
        .iter()
        .find(|r| r.id == id)
        .cloned()
        .ok_or_else(not_found)
}

/// Saves a memorized report: new (`target` is `None`) or over `target`'s
/// version.
///
/// # Errors
/// `InvalidValue` for an empty name; `Conflict(Stale)` for another
/// version; the engine's name-taken refusal as `Conflict` too.
pub fn save_definition(
    d: &mut DemoState,
    target: Option<(String, u64)>,
    w: &DefinitionWrite,
) -> Result<DefinitionRow, ApiError> {
    let s = store(d);
    let name = w.name.trim();
    if name.is_empty() {
        return Err(invalid());
    }
    let taken = s.definitions.iter().any(|r| {
        r.name.eq_ignore_ascii_case(name) && target.as_ref().is_none_or(|(id, _)| *id != r.id)
    });
    if taken {
        return Err(refusal(409, "NAME_TAKEN"));
    }
    if let Some((id, version)) = target {
        let r = s
            .definitions
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(not_found)?;
        if r.version != version {
            return Err(refusal(409, "STALE_VERSION"));
        }
        r.name = name.to_string();
        r.group_id.clone_from(&w.group_id);
        r.definition = w.definition.clone();
        r.version += 1;
        r.updated_at = Some(DemoState::now());
        Ok(r.clone())
    } else {
        let id = s.id("report");
        let row = DefinitionRow {
            id,
            group_id: w.group_id.clone(),
            name: name.to_string(),
            kind: w.kind.clone(),
            definition: w.definition.clone(),
            version: 1,
            created_by: None,
            created_by_name: Some("Kurt".into()),
            updated_at: Some(DemoState::now()),
        };
        s.definitions.push(row.clone());
        Ok(row)
    }
}

// --- Budgets -----------------------------------------------------------------

/// The budgets.
pub fn budgets(d: &mut DemoState) -> Vec<Budget> {
    store(d).budgets.iter().map(|b| b.budget.clone()).collect()
}

/// A new budget for `n.fiscal_year`.
///
/// # Errors
/// `InvalidValue` for an empty name; `Conflict` when the year has one.
pub fn create_budget(d: &mut DemoState, n: &NewBudget) -> Result<Budget, ApiError> {
    let s = store(d);
    if n.name.trim().is_empty() {
        return Err(invalid());
    }
    if s.budgets
        .iter()
        .any(|b| b.budget.fiscal_year == n.fiscal_year)
    {
        return Err(refusal(409, "BUDGET_YEAR_TAKEN"));
    }
    let id = s.id("budget");
    let budget = Budget {
        id,
        name: n.name.trim().to_string(),
        fiscal_year: n.fiscal_year,
        fiscal_year_start: CalendarDate::from_ymd(n.fiscal_year, 1, 1).ok(),
        tag_id: None,
        version: 1,
    };
    s.budgets.push(DemoBudget {
        budget: budget.clone(),
        lines: BTreeMap::new(),
    });
    Ok(budget)
}

fn sum(xs: impl IntoIterator<Item = Decimal>) -> Decimal {
    xs.into_iter().sum()
}

fn lines_of(b: &DemoBudget, chart: &[Account]) -> BudgetLines {
    let mut lines = Vec::new();
    let mut by_type: BTreeMap<&'static str, [Decimal; PERIODS]> = BTreeMap::new();
    for (id, periods) in &b.lines {
        let Some(a) = chart.iter().find(|a| a.id == *id) else {
            continue;
        };
        let p: [Decimal; PERIODS] = periods.map(Money::as_decimal);
        let kind = a.fields.account_type.as_str();
        let slot = by_type.entry(kind).or_insert([Decimal::ZERO; PERIODS]);
        for (s, v) in slot.iter_mut().zip(p) {
            *s += v;
        }
        lines.push(BudgetLine {
            account_id: a.id,
            account_name: a.fields.name.clone(),
            account_number: a.fields.account_number.clone(),
            account_type: kind.to_string(),
            periods: p,
            total: sum(p),
        });
    }
    lines.sort_by(|x, y| x.account_number.cmp(&y.account_number));
    let totals = |k: &str| {
        let p = by_type.get(k).copied().unwrap_or([Decimal::ZERO; PERIODS]);
        PeriodTotals {
            periods: p.to_vec(),
            total: sum(p),
        }
    };
    let (rev, cogs, exp, orev, oexp) = (
        totals("revenue"),
        totals("cogs"),
        totals("expense"),
        totals("other_revenue"),
        totals("other_expense"),
    );
    let net: Vec<Decimal> = (0..PERIODS)
        .map(|i| {
            let at = |t: &PeriodTotals| t.periods.get(i).copied().unwrap_or_default();
            at(&rev) + at(&orev) - at(&cogs) - at(&exp) - at(&oexp)
        })
        .collect();
    BudgetLines {
        budget: b.budget.clone(),
        lines,
        totals: BudgetTotals {
            revenue: rev,
            cogs,
            expenses: exp,
            other_revenue: orev,
            other_expenses: oexp,
            net_income: PeriodTotals {
                total: sum(net.iter().copied()),
                periods: net,
            },
        },
    }
}

/// Budget `id`'s lines.
///
/// # Errors
/// `Unsupported` for an unknown id.
pub fn budget_lines(d: &mut DemoState, id: &str) -> Result<BudgetLines, ApiError> {
    let chart = d.chart.clone().unwrap_or_else(super::chart);
    let s = store(d);
    Ok(lines_of(s.budget(id)?, &chart))
}

/// Saves rows of budget `id` at `version`.
///
/// # Errors
/// `Conflict(Stale)` for another version; `InvalidValue` for an account
/// that isn't a P&L account.
pub fn save_budget_lines(
    d: &mut DemoState,
    id: &str,
    version: u64,
    writes: &[BudgetLineWrite],
) -> Result<BudgetLines, ApiError> {
    let chart = d.chart.clone().unwrap_or_else(super::chart);
    let s = store(d);
    let b = s.budget_mut(id, version)?;
    for w in writes {
        let pl = chart.iter().any(|a| {
            a.id == w.account_id
                && matches!(
                    a.fields.account_type,
                    AccountType::Revenue
                        | AccountType::Cogs
                        | AccountType::Expense
                        | AccountType::OtherRevenue
                        | AccountType::OtherExpense
                )
        });
        if !pl {
            return Err(invalid());
        }
    }
    for w in writes {
        if w.periods.iter().all(|m| m.is_zero()) {
            b.lines.remove(&w.account_id);
        } else {
            b.lines.insert(w.account_id, w.periods);
        }
    }
    b.budget.version += 1;
    Ok(lines_of(b, &chart))
}

/// What a whole-budget action does.
pub enum WholeBudget<'a> {
    /// Last year's actuals, month by month.
    Fill,
    /// Every cell by a percent.
    Adjust(Decimal),
    /// Another budget's cells.
    Copy(&'a str),
}

/// Fills, adjusts or copies budget `id` at `version`.
///
/// # Errors
/// `Conflict(Stale)`; `Unsupported` for an unknown budget.
#[allow(
    clippy::many_single_char_names,
    reason = "the demo engine's short names: d state, s store, b budget, a account, i period"
)]
pub fn whole_budget(
    d: &mut DemoState,
    id: &str,
    version: u64,
    what: &WholeBudget<'_>,
) -> Result<BudgetLines, ApiError> {
    let chart = d.chart.clone().unwrap_or_else(super::chart);
    let books = Books::load(d);
    let s = store(d);
    let source = match what {
        WholeBudget::Copy(from) => Some(s.budget(from)?.lines.clone()),
        _ => None,
    };
    let b = s.budget_mut(id, version)?;
    match what {
        WholeBudget::Fill => {
            // The twelve calendar months before the budget's year (Q-P28ac).
            let year = b.budget.fiscal_year - 1;
            b.lines.clear();
            for a in chart.iter().filter(|a| {
                matches!(
                    a.fields.account_type,
                    AccountType::Revenue
                        | AccountType::Cogs
                        | AccountType::Expense
                        | AccountType::OtherRevenue
                        | AccountType::OtherExpense
                )
            }) {
                let mut p = [Money::ZERO; PERIODS];
                for (m, slot) in (1u32..).zip(p.iter_mut()) {
                    let Ok(from) = CalendarDate::from_ymd(year, m, 1) else {
                        continue;
                    };
                    let v = books.natural_total(a, from, from.end_of_month());
                    *slot = Money::parse_wire(&v.round_dp(2).to_string()).unwrap_or(Money::ZERO);
                }
                if p.iter().any(|m| !m.is_zero()) {
                    b.lines.insert(a.id, p);
                }
            }
        }
        WholeBudget::Adjust(pct) => {
            let factor = Decimal::ONE + *pct / Decimal::ONE_HUNDRED;
            for p in b.lines.values_mut() {
                for m in p.iter_mut() {
                    let v = (m.as_decimal() * factor).round_dp_with_strategy(
                        2,
                        rust_decimal::RoundingStrategy::MidpointAwayFromZero,
                    );
                    *m = Money::parse_wire(&v.to_string()).map_err(|_| invalid())?;
                }
            }
        }
        WholeBudget::Copy(_) => {
            b.lines = source.unwrap_or_default();
        }
    }
    b.budget.version += 1;
    Ok(lines_of(b, &chart))
}

/// Budget vs. Actual for `req` (whole fiscal periods of the budget's year),
/// through the live adapter.
///
/// # Errors
/// `InvalidValue` without a budget or period ranges; `Unsupported` for the
/// tag split (the demo doesn't play it).
#[allow(
    clippy::many_single_char_names,
    reason = "the demo engine's short names: d state, s store, b budget, a account, i period"
)]
pub fn budget_vs_actual(d: &mut DemoState, req: &ReportRequest) -> Result<ReportTable, ApiError> {
    let id = req.budget_id.clone().ok_or_else(invalid)?;
    if req.tag_split.is_some() {
        return Err(not_found());
    }
    let ranges: Vec<DateRange> = match &req.dates {
        ReportDates::Ranges(v) => v.clone(),
        ReportDates::Range(r) => vec![*r],
        _ => return Err(invalid()),
    };
    let (Some(first), Some(last)) = (ranges.first().copied(), ranges.last().copied()) else {
        return Err(invalid());
    };
    let books = Books::load(d);
    let s = store(d);
    let b = s.budget(&id)?.clone();
    let first_period = usize::from(req.first_period.unwrap_or(1).max(1));
    let budget_of = |a: AccountId, i: usize| -> Decimal {
        b.lines
            .get(&a)
            .and_then(|p| p.get(first_period - 1 + i))
            .map_or(Decimal::ZERO, |m| m.as_decimal())
    };
    let n = ranges.len();
    // Per account: (actual, budget) for each range, then the total.
    let pair = |a: &Account| -> Vec<(Decimal, Decimal)> {
        let mut v: Vec<(Decimal, Decimal)> = ranges
            .iter()
            .enumerate()
            .map(|(i, r)| (books.natural_total(a, r.from, r.to), budget_of(a.id, i)))
            .collect();
        let t = (sum(v.iter().map(|x| x.0)), sum(v.iter().map(|x| x.1)));
        v.push(t);
        v
    };
    let cell = |(actual, budget): (Decimal, Decimal)| {
        json!({"actualExact": format!("{actual:.4}"), "budgetExact": format!("{budget:.4}"),
            "overExact": format!("{:.4}", actual - budget),
            "pctOfBudgetExact": percent1(actual, budget)})
    };
    let mut columns: Vec<Value> = ranges
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({"key": format!("p{i}"), "caption": super::reports::caption_of(*r), "kind": "period",
                "periodIndex": first_period + i, "from": r.from.to_wire(), "to": r.to.to_wire(),
                "drill": {"start_date": r.from.to_wire(), "end_date": r.to.to_wire()}})
        })
        .collect();
    columns.push(
        json!({"key": "total", "caption": "Total", "kind": "total", "periodIndex": null,
        "from": first.from.to_wire(), "to": last.to.to_wire(),
        "drill": {"start_date": first.from.to_wire(), "end_date": last.to.to_wire()}}),
    );
    let mut out = serde_json::Map::new();
    let mut totals: BTreeMap<&str, Vec<(Decimal, Decimal)>> = BTreeMap::new();
    let zero = vec![(Decimal::ZERO, Decimal::ZERO); n + 1];
    let add = |a: &[(Decimal, Decimal)], b: &[(Decimal, Decimal)]| -> Vec<(Decimal, Decimal)> {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x.0 + y.0, x.1 + y.1))
            .collect()
    };
    for (key, kind) in [
        ("revenue", AccountType::Revenue),
        ("cogs", AccountType::Cogs),
        ("expenses", AccountType::Expense),
        ("otherRevenue", AccountType::OtherRevenue),
        ("otherExpenses", AccountType::OtherExpense),
    ] {
        let own: BTreeMap<AccountId, Vec<(Decimal, Decimal)>> = books
            .chart
            .iter()
            .filter(|a| a.fields.account_type == kind)
            .map(|a| (a.id, pair(a)))
            .filter(|(_, v)| v.iter().any(|(x, y)| !x.is_zero() || !y.is_zero()))
            .collect();
        let mut total = zero.clone();
        for v in own.values() {
            total = add(&total, v);
        }
        let placed = books.tree(&[kind], &|a| own.contains_key(&a.id));
        let rows: Vec<Value> = placed
            .iter()
            .map(|(a, depth, kids)| {
                let mine = own.get(&a.id).cloned().unwrap_or_else(|| zero.clone());
                let mut e = json!({"rowKind": "account", "accountId": a.id.to_string(),
                    "name": a.fields.name, "accountNumber": a.fields.account_number,
                    "parentAccountId": a.fields.parent_id.map(|p| p.to_string()), "depth": depth,
                    "cells": mine.iter().copied().map(cell).collect::<Vec<_>>(),
                    "drill": {"account_ids": a.id.to_string()}});
                if *kids {
                    let mut r = zero.clone();
                    for id in books.subtree(a.id) {
                        if let Some(v) = own.get(&id) {
                            r = add(&r, v);
                        }
                    }
                    e["rollupCells"] = json!(r.into_iter().map(cell).collect::<Vec<_>>());
                }
                e
            })
            .collect();
        totals.insert(key, total);
        out.insert(key.into(), Value::Array(rows));
    }
    let section_total = |k: &str| totals.get(k).cloned().unwrap_or_else(|| zero.clone());
    let combine = |f: &dyn Fn(usize, bool) -> Decimal| -> Value {
        json!(
            (0..=n)
                .map(|i| cell((f(i, true), f(i, false))))
                .collect::<Vec<_>>()
        )
    };
    let at = |v: &[(Decimal, Decimal)], i: usize, actual: bool| {
        v.get(i)
            .map_or(Decimal::ZERO, |x| if actual { x.0 } else { x.1 })
    };
    let (rev, cogs, exp, orev, oexp) = (
        section_total("revenue"),
        section_total("cogs"),
        section_total("expenses"),
        section_total("otherRevenue"),
        section_total("otherExpenses"),
    );
    let has_cogs = out
        .get("cogs")
        .and_then(Value::as_array)
        .is_some_and(|v| !v.is_empty());
    let cells = |v: &[(Decimal, Decimal)]| json!(v.iter().copied().map(cell).collect::<Vec<_>>());
    out.insert(
        "totalsExact".into(),
        json!({
            "totalRevenue": cells(&rev), "totalCogs": cells(&cogs), "totalExpenses": cells(&exp),
            "totalOtherRevenue": cells(&orev), "totalOtherExpenses": cells(&oexp),
            "grossProfit": if has_cogs {
                combine(&|i, x| at(&rev, i, x) - at(&cogs, i, x))
            } else { Value::Null },
            "netIncome": combine(&|i, x| at(&rev, i, x) + at(&orev, i, x) - at(&cogs, i, x)
                - at(&exp, i, x) - at(&oexp, i, x)),
        }),
    );
    let basis = req.basis.unwrap_or(Basis::Accrual).as_str();
    out.insert("title".into(), json!("Budget vs. Actual"));
    out.insert("layout".into(), json!("budget_vs_actual"));
    out.insert(
        "budget".into(),
        json!({"id": b.budget.id, "version": b.budget.version, "tagId": null}),
    );
    out.insert("columns".into(), Value::Array(columns));
    out.insert(
        "drillTarget".into(),
        json!({"path": "/reports/general-ledger", "params": {"basis": basis}}),
    );
    out.insert("meta".into(), json!({"exact": true, "basis": basis}));
    cl_api::resources::reports::adapters::bva::table(&Value::Object(out), req)
}

// --- Email, ask ----------------------------------------------------------------

/// "Sends" a report email: the demo has no mail server and nothing leaves
/// the computer; it counts the send and answers as the engine would.
///
/// # Errors
/// `InvalidValue` without a recipient or file.
pub fn email(d: &mut DemoState, m: &ReportEmail) -> Result<EmailSent, ApiError> {
    if m.to.is_empty() || m.file.is_empty() {
        return Err(invalid());
    }
    let s = store(d);
    s.emails_sent += 1;
    let count = u32::try_from(m.to.len() + m.cc.len()).unwrap_or(u32::MAX);
    serde_json::from_value(json!({"sent": true, "recipientCount": count}))
        .map_err(|_| ApiError::Internal)
}

/// The demo's stub mapper: picks a report and dates from a few words of
/// the question, then runs the live checks on its answer (`validate`), so
/// it can only choose what the request offered. No amounts.
///
/// # Errors
/// [`ApiError::ReportShape`] (the window says it couldn't map the
/// question) when no rule matches or the answer fails the checks.
pub fn ask(req: &AskRequest) -> Result<AskMapping, ApiError> {
    let q = req.question.to_lowercase();
    let offered = |k: &str| req.reports.iter().any(|r| r.key == k);
    let preset = |p: &str| {
        req.date_presets
            .iter()
            .any(|x| x == p)
            .then(|| p.to_string())
    };
    let answer = if (q.contains("owe") || q.contains("aging") || q.contains("overdue"))
        && offered("ar_aging_summary")
    {
        json!({"reportKey": "ar_aging_summary", "reader": "demo"})
    } else if q.contains("balance sheet") && offered("balance_sheet") {
        json!({"reportKey": "balance_sheet", "reader": "demo"})
    } else if offered("profit_loss")
        && (q.contains("compare")
            || q.contains("profit")
            || q.contains("year so far")
            || q.contains("make")
            || q.contains("october"))
    {
        let mut extras = Vec::new();
        if q.contains("compare") || q.contains("year so far") || q.contains("ytd") {
            extras.push("pct_of_income");
            extras.push("ytd");
        }
        json!({"reportKey": "profit_loss", "reader": "demo",
            "dates": preset("last_month").map(|p| json!({"preset": p})),
            "basis": "accrual", "extras": extras})
    } else {
        return Err(ApiError::ReportShape { what: "ask" });
    };
    validate(req, &answer)
}

#[cfg(test)]
#[path = "report_store_tests.rs"]
mod tests;
