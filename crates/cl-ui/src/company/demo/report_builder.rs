//! Report Builder runs in the demo (A07 WP2): a small stand-in for the
//! engine's `/report-builder/run` and `/run/detail` over the demo books
//! ([`super::reports::Books`]). It answers with engine-shaped JSON that goes
//! through the same adapter as the live engine
//! ([`cl_api::resources::report_builder::read_run`]), so the window can't
//! tell the two apart.
//!
//! It plays only what the demo's starters use (Q-A07t): `gl` and `sales`
//! sections grouped by one key (`account` or `contact`), summary,
//! expandable and detail displays with the engine's open rule, formulas,
//! detail rows paged five at a time behind a `more` row, and two demo
//! triggers: the demo's role can't see `purchases` sections (a denied
//! row), and the "Slow report" starter answers 503 `REPORT_TIMEOUT`.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::resources::report_builder::{
    BuilderRun, ColumnLayout, DetailPage, DetailRequest, RunDates, RunRequest, Starter,
    read_detail, read_run,
};
use cl_api::{ApiError, engine_refusal};
use cl_core::CalendarDate;
use cl_core::report::ColumnHead;
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::DemoState;
use super::reports::{Books, Post};

/// Detail rows a demo line shows before its `more` row.
const DEMO_PAGE: usize = 5;

/// The key of the starter that answers "took too long".
pub const SLOW: &str = "demo_slow";

const PROFIT_LOSS: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":["date","txn_type","contact","memo"],"compare":[],"headers":{}},"nodes":[{"type":"section","id":"income","caption":"Income","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["revenue"]}],"groupBy":[{"key":"account"}],"display":"summary","defaultExpanded":false,"showTotal":true,"totalCaption":"Total Income","hideZero":true},{"type":"section","id":"expenses","caption":"Expenses","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["expense"]}],"groupBy":[{"key":"account"}],"display":"expandable","defaultExpanded":false,"showTotal":true,"totalCaption":"Total Expenses","hideZero":true},{"type":"formula","id":"net_income","caption":"Net Income","terms":[{"nodeId":"income","sign":1},{"nodeId":"expenses","sign":-1}]}]}"#;

/// Transaction Detail by Account as the engine's starters v2 (U-119f):
/// one natural-sign section per account type.
const LEDGER: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":["date","txn_type","number","contact","memo","debit","credit"],"compare":[],"headers":{}},"nodes":[{"type":"section","id":"assets","caption":"Assets","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["asset"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"liabilities","caption":"Liabilities","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["liability"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"equity","caption":"Equity","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["equity"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"income","caption":"Income","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["revenue"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"cogs","caption":"Cost of Goods Sold","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["cogs"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"expenses","caption":"Expenses","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["expense"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"other_income","caption":"Other Income","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["other_revenue"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true},{"type":"section","id":"other_expenses","caption":"Other Expenses","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["other_expense"]}],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true}]}"#;

const SALES_AND_PURCHASES: &str = r#"{"schemaVersion":1,"kind":"builder","title":"Sales and purchases","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":["date","txn_type","number","amount"],"compare":[],"headers":{}},"nodes":[{"type":"text","id":"intro","text":"Sales by client, then purchases."},{"type":"section","id":"sales","caption":"Sales","source":"sales","measure":"amount","filters":[],"groupBy":[{"key":"contact"}],"display":"expandable","defaultExpanded":false,"showTotal":true,"hideZero":true},{"type":"blank","id":"gap"},{"type":"section","id":"purchases","caption":"Purchases","source":"purchases","measure":"amount","filters":[],"groupBy":[{"key":"contact"}],"display":"summary","defaultExpanded":false,"showTotal":true,"hideZero":true},{"type":"formula","id":"net","caption":"Sales less purchases","terms":[{"nodeId":"sales","sign":1},{"nodeId":"purchases","sign":-1}]}]}"#;

/// A/R Aging Summary as a builder report (A08 demo parity): open A/R by
/// client with aging columns, as of a date.
const AR_AGING: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"asOf","preset":"today"},"columns":{"detailFields":[],"agingBuckets":true,"compare":[],"headers":{}},"nodes":[{"type":"section","id":"ar","caption":"Accounts receivable","source":"ar_open","measure":"open_balance","filters":[],"groupBy":[{"key":"contact"}],"display":"summary","defaultExpanded":false,"showTotal":true,"totalCaption":"Total","hideZero":true}]}"#;

const SLOW_DEF: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":[],"compare":[],"headers":{}},"nodes":[{"type":"section","id":"all","caption":"Everything","source":"gl","measure":"amount","polarity":"debit","filters":[],"groupBy":[{"key":"account"}],"display":"summary","defaultExpanded":false,"showTotal":true,"hideZero":true}]}"#;

/// The demo's starters (`GET /report-builder/starters`).
///
/// # Errors
/// Never in practice (the texts are fixed); a parse failure is `Decode`.
pub fn starters() -> Result<Vec<Starter>, ApiError> {
    [
        (
            "profit_loss",
            "Profit & Loss",
            Some("profit-loss"),
            PROFIT_LOSS,
        ),
        (
            "transaction_detail_by_account",
            "Transaction Detail by Account",
            Some("general-ledger"),
            LEDGER,
        ),
        (
            "ar_aging_summary",
            "A/R Aging Summary",
            Some("ar-aging-summary"),
            AR_AGING,
        ),
        (
            "sales_and_purchases",
            "Sales and purchases",
            None,
            SALES_AND_PURCHASES,
        ),
        (SLOW, "Slow report", None, SLOW_DEF),
    ]
    .into_iter()
    .map(|(key, title, ties, text)| {
        Ok(Starter {
            key: key.into(),
            title: title.into(),
            ties_to: ties.map(str::to_string),
            definition: serde_json::from_str(text).map_err(|_| ApiError::Decode {
                what: "demo starter",
            })?,
        })
    })
    .collect()
}

/// A company-unknown id the demo plays as stale (an account or name that
/// was deleted): a filter naming it is refused `NOT_IN_COMPANY` (A08).
pub const STALE_ID: &str = "00000000-0000-4000-8000-00000000dead";

/// Detail rows a run shows under a `detail` display line before its `more`
/// row (an expandable line shows [`DEMO_PAGE`]): more than the editor
/// preview's 50, so the preview's cut is seen on one line (A08).
const DEMO_DETAIL_RUN: usize = 60;

/// The aging buckets, in column order, with the engine's captions.
const BUCKETS: [(&str, &str); 5] = [
    ("current", "Current"),
    ("d1_30", "1 - 30"),
    ("d31_60", "31 - 60"),
    ("d61_90", "61 - 90"),
    ("over_90", "Over 90"),
];

/// What a column shows (the engine's `specs`).
#[derive(Clone, Debug)]
enum Spec {
    /// A detail field (detail rows only).
    Field(String),
    /// A value slot.
    Amount(usize),
    /// The sum of value slots.
    Sum(Vec<usize>),
    /// Column `a` less column `b`.
    Change(usize, usize),
    /// (`a` - `b`) / |`b`| as a percent.
    PctChange(usize, usize),
    /// Column `of` as a percent of the row's base.
    Percent(usize),
}

/// One value slot: its dates (`from` `None`: as of `to`).
#[derive(Clone, Copy, Debug)]
struct Slot {
    from: Option<CalendarDate>,
    to: CalendarDate,
}

impl Slot {
    fn holds(self, d: CalendarDate) -> bool {
        d <= self.to && self.from.is_none_or(|f| d >= f)
    }
}

/// The run's columns and value slots.
struct Plan {
    cols: Vec<Value>,
    specs: Vec<Spec>,
    slots: Vec<Slot>,
    /// Aging columns: the values are the five buckets as of `slots[0]`.
    buckets: bool,
}

impl Plan {
    fn width(&self) -> usize {
        if self.buckets {
            BUCKETS.len()
        } else {
            self.slots.len()
        }
    }
}

type Vals = Vec<Decimal>;

fn invalid_param(field: &str) -> ApiError {
    engine_refusal(
        400,
        &json!({"error": {"message": "That report can't run with these dates.", "code": "INVALID_PARAM", "details": {"field": field}}}),
    )
}

fn prior_of(base: &str, s: Slot) -> Slot {
    let back = |d: CalendarDate, days: i64| d.add_days(-days).unwrap_or(d);
    if base == "prior_year" {
        let y = |d: CalendarDate| d.add_months(-12).unwrap_or(d);
        return Slot {
            from: s.from.map(y),
            to: y(s.to),
        };
    }
    // The engine's own rule without compareRanges: the same days before.
    match s.from {
        Some(f) => {
            let len = f.days_until(s.to) + 1;
            Slot {
                from: Some(back(f, len)),
                to: back(f, 1),
            }
        }
        None => Slot {
            from: None,
            to: back(s.to.start_of_month(), 1),
        },
    }
}

/// The engine's columns for `def` run with `r` (periods, compare, % of,
/// aging, renamed headers).
fn plan(def: &Value, r: &cl_api::resources::report_builder::RunRef) -> Result<Plan, ApiError> {
    let cols_def = &def["columns"];
    let headers = &cols_def["headers"];
    let caption = |key: &str, fallback: &str| -> String {
        headers[key].as_str().unwrap_or(fallback).to_string()
    };
    let mut cols: Vec<Value> = Vec::new();
    let mut specs: Vec<Spec> = Vec::new();
    for f in cols_def["detailFields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let key = format!("f:{f}");
        cols.push(json!({"key": key, "caption": caption(&key, f), "kind": "field", "field": f}));
        specs.push(Spec::Field(f.to_string()));
    }
    let as_of = r.dates.as_of;
    let (from, to) = span(&r.dates);
    let whole = Slot {
        from: as_of.is_none().then_some(from),
        to,
    };
    if cols_def["agingBuckets"] == true {
        let one = r.period_ranges.len() <= 1 && r.period_ranges.first().is_none_or(|p| p.to == to);
        if !r.compare_ranges.is_empty() {
            return Err(invalid_param("compareRanges"));
        }
        if !one {
            return Err(invalid_param("periodRanges"));
        }
        for (i, (b, cap)) in BUCKETS.iter().enumerate() {
            let key = format!("bucket:{b}");
            cols.push(json!({"key": key, "caption": caption(&key, cap), "kind": "bucket"}));
            specs.push(Spec::Amount(i));
        }
        cols.push(json!({"key": "total", "caption": caption("total", "Total"), "kind": "total"}));
        specs.push(Spec::Sum((0..BUCKETS.len()).collect()));
        return Ok(Plan {
            cols,
            specs,
            slots: vec![whole],
            buckets: true,
        });
    }
    let base: Vec<Slot> = if r.period_ranges.is_empty() {
        vec![whole]
    } else {
        r.period_ranges
            .iter()
            .map(|p| Slot {
                from: as_of.is_none().then_some(p.from),
                to: p.to,
            })
            .collect()
    };
    // As the engine: each period starts after the one before ends (an
    // as-of run's slots are days, so their ends ascend).
    for w in base.windows(2) {
        let overlaps = match w[1].from {
            Some(from) => from <= w[0].to,
            None => w[1].to <= w[0].to,
        };
        if overlaps {
            return Err(invalid_param("periodRanges"));
        }
    }
    let compare: Vec<&Value> = cols_def["compare"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    for given in &r.compare_ranges {
        if given.len() != base.len() {
            return Err(invalid_param("compareRanges"));
        }
    }
    let percent = cols_def.get("percentOf").is_some_and(|p| !p.is_null());
    let mut slots = base.clone();
    let multi = base.len() > 1;
    let mut base_cols = Vec::new();
    for (i, s) in base.iter().enumerate() {
        let key = if multi {
            format!("p{i}")
        } else {
            "total".into()
        };
        let range = json!({"from": s.from.map(CalendarDate::to_wire), "to": s.to.to_wire()});
        let cur = cols.len();
        if multi {
            let fallback = format!(
                "{} - {}",
                s.from.map_or_else(String::new, CalendarDate::to_wire),
                s.to.to_wire()
            );
            cols.push(json!({"key": key, "caption": caption(&key, &fallback), "kind": "period", "range": range}));
        } else {
            cols.push(json!({"key": key, "caption": caption(&key, "Total"), "kind": "total"}));
        }
        specs.push(Spec::Amount(i));
        base_cols.push(cur);
        for (ci, c) in compare.iter().enumerate() {
            let b = c["base"].as_str().unwrap_or("prior_period");
            let prior_slot = match r.compare_ranges.get(ci) {
                Some(given) => Slot {
                    from: as_of.is_none().then_some(given[i].from),
                    to: given[i].to,
                },
                None => prior_of(b, *s),
            };
            slots.push(prior_slot);
            let pk = format!("{key}:c{ci}");
            let cap = if b == "prior_year" {
                "Prior Year"
            } else {
                "Prior Period"
            };
            let prior = cols.len();
            let kind = if c["amount"] == true {
                "prior"
            } else {
                "hidden"
            };
            cols.push(json!({"key": pk, "caption": caption(&pk, cap), "kind": kind}));
            specs.push(Spec::Amount(slots.len() - 1));
            if c["dollarChange"] == true {
                let k = format!("{pk}:change");
                cols.push(json!({"key": k, "caption": caption(&k, "$ Change"), "kind": "change"}));
                specs.push(Spec::Change(cur, prior));
            }
            if c["percentChange"] == true {
                let k = format!("{pk}:pct_change");
                cols.push(
                    json!({"key": k, "caption": caption(&k, "% Change"), "kind": "pct_change"}),
                );
                specs.push(Spec::PctChange(cur, prior));
            }
        }
        if percent {
            let k = format!("{key}:pct");
            cols.push(json!({"key": k, "caption": caption(&k, "%"), "kind": "percent"}));
            specs.push(Spec::Percent(cur));
        }
    }
    if multi && as_of.is_none() {
        let tot = cols.len();
        cols.push(json!({"key": "total", "caption": caption("total", "Total"), "kind": "total"}));
        specs.push(Spec::Sum((0..base.len()).collect()));
        if percent {
            cols.push(json!({"key": "total:pct", "caption": caption("total:pct", "%"), "kind": "percent"}));
            specs.push(Spec::Percent(tot));
        }
    }
    Ok(Plan {
        cols,
        specs,
        slots,
        buckets: false,
    })
}

fn exact(d: Decimal) -> String {
    let mut d = d;
    d.rescale(4);
    d.to_string()
}

/// The amount behind each column (percent bases), the engine's `decCells`.
fn dec_cells(plan: &Plan, vals: Option<&Vals>) -> Vec<Option<Decimal>> {
    plan.specs
        .iter()
        .map(|s| {
            let v = vals?;
            match s {
                Spec::Amount(i) => v.get(*i).copied(),
                Spec::Sum(list) => Some(list.iter().filter_map(|i| v.get(*i)).sum()),
                _ => None,
            }
        })
        .collect()
}

/// A row's cells from its values (the engine's `cellsOf`); `fields` fills
/// a detail row's field columns.
fn cells_of(
    plan: &Plan,
    vals: Option<&Vals>,
    base: Option<&[Option<Decimal>]>,
    fields: Option<&Detail>,
) -> Vec<Value> {
    let mut out: Vec<Option<Decimal>> = Vec::with_capacity(plan.specs.len());
    let mut cells: Vec<Value> = Vec::with_capacity(plan.specs.len());
    for s in &plan.specs {
        let Some(v) = vals else {
            out.push(None);
            cells.push(Value::Null);
            continue;
        };
        let (d, cell) = match s {
            Spec::Field(f) => (None, fields.map_or(Value::Null, |x| x.field(f))),
            Spec::Amount(i) => {
                let x = v.get(*i).copied().unwrap_or_default();
                (Some(x), json!(exact(x)))
            }
            Spec::Sum(list) => {
                let x: Decimal = list.iter().filter_map(|i| v.get(*i)).sum();
                (Some(x), json!(exact(x)))
            }
            Spec::Change(a, b) => {
                let x = out[*a].unwrap_or_default() - out[*b].unwrap_or_default();
                (Some(x), json!(exact(x)))
            }
            Spec::PctChange(a, b) => {
                let (a, b) = (out[*a].unwrap_or_default(), out[*b].unwrap_or_default());
                (
                    None,
                    super::reports::percent1(a - b, b.abs()).map_or(Value::Null, Value::from),
                )
            }
            Spec::Percent(of) => {
                let b = base.and_then(|b| b.get(*of).copied().flatten());
                (
                    None,
                    b.and_then(|b| super::reports::percent1(out[*of].unwrap_or_default(), b))
                        .map_or(Value::Null, Value::from),
                )
            }
        };
        out.push(d);
        cells.push(cell);
    }
    for (c, col) in cells.iter_mut().zip(&plan.cols) {
        if col["kind"] == "hidden" {
            *c = Value::Null;
        }
    }
    cells
}

/// One transaction line (or open item) under a group-by line.
struct Detail {
    date: CalendarDate,
    txn_type: String,
    number: String,
    contact: Option<String>,
    memo: String,
    debit: Decimal,
    credit: Decimal,
    amount: Decimal,
    txn: String,
    line: String,
    vals: Vals,
}

impl Detail {
    fn field(&self, f: &str) -> Value {
        match f {
            "date" => json!(self.date.to_wire()),
            "txn_type" => json!(txn_label(&self.txn_type)),
            "number" => json!(self.number),
            "contact" => self.contact.as_ref().map_or(Value::Null, |c| json!(c)),
            "memo" => json!(self.memo),
            "debit" if !self.debit.is_zero() => json!(exact(self.debit)),
            "credit" if !self.credit.is_zero() => json!(exact(self.credit)),
            "amount" => json!(exact(self.amount)),
            _ => Value::Null,
        }
    }
}

/// One group-by line: its path segment, caption, values and details.
struct Line {
    seg: String,
    caption: String,
    vals: Vals,
    details: Vec<Detail>,
}

struct Section {
    lines: Vec<Line>,
    denied: bool,
}

fn account_type_of(books: &Books, p: &Post) -> String {
    books
        .chart
        .iter()
        .find(|a| a.id == p.account)
        .and_then(|a| serde_json::to_value(a.fields.account_type).ok())
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn account_caption(books: &Books, p: &Post) -> String {
    books
        .chart
        .iter()
        .find(|a| a.id == p.account)
        .map(|a| match &a.fields.account_number {
            Some(n) => format!("{n} {}", a.fields.name),
            None => a.fields.name.clone(),
        })
        .unwrap_or_default()
}

/// A post's value in a section: its polarity, else the natural side of
/// the section's accounts (sales and income: credits).
fn value(section: &Value, p: &Post) -> Decimal {
    match section["polarity"].as_str() {
        Some("debit") => p.debit - p.credit,
        Some("credit") => p.credit - p.debit,
        _ if natural_credit(section) => p.credit - p.debit,
        _ => p.debit - p.credit,
    }
}

/// The demo's ledger stamp: moves whenever a posted line changes.
fn stamp(books: &Books) -> u64 {
    // FNV-1a over every line's id and amounts.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in &books.posts {
        for b in format!("{}|{}|{}|{}", p.txn, p.line, p.debit, p.credit).bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h >> 12
}

/// The demo's run fingerprint: a hash of everything in the reference
/// (definition, dates, ranges, basis, lens), not the open lists or the
/// purpose, as 64 hex digits like the engine's. A detail request with
/// another one is refused 409 `RUN_CHANGED {reason:'params'}`.
fn fingerprint(r: &cl_api::resources::report_builder::RunRef) -> String {
    let text = serde_json::to_string(r).unwrap_or_default();
    let mut out = String::with_capacity(64);
    for seed in 0..4u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
        for b in text.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{h:016x}"));
    }
    out
}

fn natural_credit(section: &Value) -> bool {
    section["source"] == "sales"
        || section["filters"].as_array().is_some_and(|f| {
            f.iter().any(|x| {
                x["field"] == "account_type"
                    && x["values"].as_array().is_some_and(|v| {
                        v.iter().any(|t| {
                            matches!(
                                t.as_str(),
                                Some("revenue" | "other_revenue" | "liability" | "equity")
                            )
                        })
                    })
            })
        })
}

/// The values of a filter (`values` or `ids`) as a set.
fn filter_set(f: &Value) -> BTreeSet<String> {
    f["values"]
        .as_array()
        .or_else(|| f["ids"].as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The section keeps `x` under the filter on `field` (in / not in).
fn passes(section: &Value, field: &str, x: &str) -> bool {
    section["filters"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["field"] == field)
        .all(|f| {
            let set = filter_set(f);
            match f["op"].as_str() {
                Some("in") => set.contains(x),
                Some("not_in") => !set.contains(x),
                _ => true,
            }
        })
}

fn group_lines(map: BTreeMap<String, Line>) -> Vec<Line> {
    map.into_values().collect()
}

/// A section's lines over the plan's slots.
fn section_lines(d: &mut DemoState, books: &Books, section: &Value, plan: &Plan) -> Section {
    match section["source"].as_str() {
        Some("purchases") => Section {
            lines: Vec::new(),
            denied: true,
        },
        Some("ar_open") => Section {
            lines: open_lines(d, section, plan),
            denied: false,
        },
        _ => Section {
            lines: post_lines(books, section, plan),
            denied: false,
        },
    }
}

fn post_lines(books: &Books, section: &Value, plan: &Plan) -> Vec<Line> {
    let sales = section["source"] == "sales";
    let key = section["groupBy"][0]["key"].as_str().unwrap_or("account");
    let n = plan.width();
    let mut lines: BTreeMap<String, Line> = BTreeMap::new();
    for p in &books.posts {
        let hits: Vec<usize> = (0..plan.slots.len())
            .filter(|&i| plan.slots[i].holds(p.date))
            .collect();
        if hits.is_empty() {
            continue;
        }
        let t = account_type_of(books, p);
        if sales && t != "revenue" {
            continue;
        }
        if !passes(section, "account_type", &t)
            || !passes(section, "account", &p.account.to_string())
        {
            continue;
        }
        let (seg, caption) = match key {
            "contact" => match &p.contact {
                Some(c) => (format!("contact:{c}"), c.clone()),
                None => ("contact:none".into(), "No name".into()),
            },
            "txn_type" => (
                format!("txn_type:{}", builder_type(p)),
                txn_label(builder_type(p)),
            ),
            _ => (format!("account:{}", p.account), account_caption(books, p)),
        };
        let v = value(section, p);
        let mut vals = vec![Decimal::ZERO; n];
        for i in hits {
            vals[i] = v;
        }
        let line = lines.entry(caption.clone()).or_insert_with(|| Line {
            seg,
            caption,
            vals: vec![Decimal::ZERO; n],
            details: Vec::new(),
        });
        for (a, b) in line.vals.iter_mut().zip(&vals) {
            *a += *b;
        }
        line.details.push(Detail {
            date: p.date,
            txn_type: builder_type(p).to_string(),
            number: p.number.clone(),
            contact: p.contact.clone(),
            memo: p.memo.clone(),
            debit: p.debit,
            credit: p.credit,
            amount: v,
            txn: p.txn.to_string(),
            line: p.line.to_string(),
            vals,
        });
    }
    group_lines(lines)
}

/// `ar_open`: every client's open A/R items as of each slot's end, by
/// client (the engine's open-items source; aging columns bucket them).
fn open_lines(d: &mut DemoState, section: &Value, plan: &Plan) -> Vec<Line> {
    super::credit_memos::seed(d);
    let names: BTreeMap<String, String> = d
        .contacts_mut()
        .iter()
        .map(|c| (c.id.to_string(), c.fields.display_name.clone()))
        .collect();
    let original = section["measure"] == "original_amount";
    let n = plan.width();
    let mut lines: BTreeMap<String, Line> = BTreeMap::new();
    for (si, slot) in plan.slots.iter().enumerate() {
        let as_of = slot.to;
        for item in super::statements::ar_open_items(d, as_of, None) {
            let client = item.client.to_string();
            if !passes(section, "contact", &client) {
                continue;
            }
            let i = if plan.buckets {
                match item.days(as_of) {
                    x if x <= 0 => 0,
                    1..=30 => 1,
                    31..=60 => 2,
                    61..=90 => 3,
                    _ => 4,
                }
            } else {
                si
            };
            let v = if original { item.original } else { item.open };
            let caption = names
                .get(&client)
                .cloned()
                .unwrap_or_else(|| "No name".into());
            let mut vals = vec![Decimal::ZERO; n];
            vals[i] = v;
            let line = lines.entry(caption.clone()).or_insert_with(|| Line {
                seg: format!("contact:{client}"),
                caption: caption.clone(),
                vals: vec![Decimal::ZERO; n],
                details: Vec::new(),
            });
            line.vals[i] += v;
            line.details.push(Detail {
                date: item.date,
                txn_type: item.txn_type.to_string(),
                number: item.number.clone(),
                contact: Some(caption),
                memo: String::new(),
                debit: Decimal::ZERO,
                credit: Decimal::ZERO,
                amount: v,
                txn: item.txn.to_string(),
                line: "0".into(),
                vals,
            });
        }
    }
    group_lines(lines)
}

/// The engine's open rule (Q-A07r).
fn is_open(req: &RunRequest, section: &Value, path: &[String]) -> bool {
    let p = path.to_vec();
    req.expand_all
        || req.open.iter().any(|o| o.0 == p)
        || (section["defaultExpanded"] == true && !req.collapsed.iter().any(|c| c.0 == p))
}

/// The builder's type of `p`: a check is `check`, as the engine's
/// builder sends it (run.ts `is_check`, A07 QA-2).
fn builder_type(p: &Post) -> &str {
    if p.is_check && p.txn_type == "expense" {
        "check"
    } else {
        &p.txn_type
    }
}

fn txn_label(t: &str) -> String {
    let mut s = t.replace('_', " ");
    if let Some(f) = s.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    s
}

fn detail_row(plan: &Plan, section: &Value, path: &[String], x: &Detail) -> Value {
    json!({
        "kind": "detail", "nodeId": section["id"], "level": 2, "path": path,
        "caption": "", "cells": cells_of(plan, Some(&x.vals), None, Some(x)),
        "fields": {"txnId": x.txn, "txnType": x.txn_type, "lineId": x.line},
        "drill": {"type": "transaction", "txnId": x.txn, "txnType": x.txn_type},
        "expandable": false, "expanded": false
    })
}

fn span(dates: &RunDates) -> (CalendarDate, CalendarDate) {
    match (dates.from, dates.to, dates.as_of) {
        (Some(f), Some(t), _) => (f, t),
        (_, _, Some(d)) => (CalendarDate::min(), d),
        _ => (CalendarDate::min(), CalendarDate::max()),
    }
}

/// The engine's checks on a definition before it runs: its rules
/// (`DEFINITION_INVALID {path, code}`, the client's own validation, which
/// matches the engine's), then the ids it names (`NOT_IN_COMPANY`).
fn check_definition(books: &Books, def: &Value) -> Result<(), ApiError> {
    let refuse = |path: &str, code: &str| {
        engine_refusal(
            400,
            &json!({"error": {"message": "The report definition breaks a rule.", "code": "DEFINITION_INVALID",
                              "details": {"path": path, "code": code}}}),
        )
    };
    let text = serde_json::to_string(def).unwrap_or_default();
    let parsed = match cl_model::report_def::parse(&text) {
        Ok(p) => p,
        Err(cl_model::report_def::DefinitionError::Invalid { path, code }) => {
            return Err(refuse(&path, code.as_str()));
        }
        Err(_) => return Err(refuse("definition", "SCHEMA")),
    };
    let accounts: BTreeSet<String> = books.chart.iter().map(|a| a.id.to_string()).collect();
    for v in cl_model::report_def::outline::walk(&parsed.nodes) {
        let cl_model::report_def::Node::Section(s) = v.node else {
            continue;
        };
        for (j, f) in s.filters.iter().enumerate() {
            let stale = f
                .ids
                .iter()
                .flatten()
                .any(|id| id == STALE_ID || (f.field == "account" && !accounts.contains(id)));
            if stale {
                return Err(refuse(
                    &format!("{}.filters.{j}.ids", v.path),
                    "NOT_IN_COMPANY",
                ));
            }
        }
    }
    Ok(())
}

/// What a rendered node adds up to in its normal sign (`None`: denied):
/// formulas and named percent bases read these (U-218).
type Totals = BTreeMap<String, Option<Vals>>;

/// True when a section shows its figures with the other sign than its
/// natural one (a polarity set against its accounts' normal side, as the
/// demo tells it: one side per section). Its normal-sign total is then the
/// shown total negated (U-218).
fn shown_flipped(section: &Value) -> bool {
    match section["polarity"].as_str() {
        Some("debit") => natural_credit(section),
        Some("credit") => !natural_credit(section),
        _ => false,
    }
}

/// The normal-sign total of a section from its shown total.
fn normal_of(section: &Value, shown: Option<&Vals>) -> Option<Vals> {
    let flip = shown_flipped(section);
    shown.map(|v| v.iter().map(|x| if flip { -*x } else { *x }).collect())
}

struct Render<'a> {
    req: &'a RunRequest,
    plan: &'a Plan,
    books: &'a Books,
    percent_of: Option<String>,
    /// A preview run (U-217): every line cut at 50 transactions.
    preview: bool,
    rows: Vec<Value>,
    totals: Totals,
    denied: Vec<String>,
}

fn add_vals(a: &mut Vals, b: &Vals) {
    for (x, y) in a.iter_mut().zip(b) {
        *x += *y;
    }
}

impl Render<'_> {
    fn layout(&self, kind: &str, id: &str, level: usize, caption: &str) -> Value {
        json!({"kind": kind, "nodeId": id, "level": level, "path": [format!("n:{id}")], "caption": caption,
               "cells": self.plan.cols.iter().map(|_| Value::Null).collect::<Vec<_>>(),
               "expandable": false, "expanded": false})
    }

    /// The percent bases: the section's own total or the named node's.
    fn base(&self, section: Option<&Vals>) -> Option<Vec<Option<Decimal>>> {
        let p = self.percent_of.as_deref()?;
        let vals = if p == "section" {
            section.cloned()
        } else {
            self.totals.get(p).cloned().flatten()
        };
        Some(dec_cells(self.plan, vals.as_ref()))
    }

    /// Renders `nodes`; returns what the sections among them add up to,
    /// as shown (a group's subtotal row) and in the normal sign (what a
    /// formula over the group reads, U-218).
    fn nodes(
        &mut self,
        d: &mut DemoState,
        nodes: &[Value],
        level: usize,
    ) -> (Option<Vals>, Option<Vals>) {
        let mut sum = Some(vec![Decimal::ZERO; self.plan.width()]);
        let mut normal = Some(vec![Decimal::ZERO; self.plan.width()]);
        for node in nodes {
            let id = node["id"].as_str().unwrap_or_default().to_string();
            match node["type"].as_str() {
                Some("text") => {
                    let r = self.layout(
                        "text",
                        &id,
                        level,
                        node["text"].as_str().unwrap_or_default(),
                    );
                    self.rows.push(r);
                }
                Some(k @ ("blank" | "page_break")) => {
                    let r = self.layout(k, &id, level, "");
                    self.rows.push(r);
                }
                Some("formula") => {
                    let mut acc = Some(vec![Decimal::ZERO; self.plan.width()]);
                    for t in node["terms"].as_array().into_iter().flatten() {
                        let term = self
                            .totals
                            .get(t["nodeId"].as_str().unwrap_or_default())
                            .cloned()
                            .flatten();
                        acc = match (acc, term) {
                            (Some(mut a), Some(x)) => {
                                for (p, q) in a.iter_mut().zip(&x) {
                                    if t["sign"] == -1 {
                                        *p -= *q;
                                    } else {
                                        *p += *q;
                                    }
                                }
                                Some(a)
                            }
                            _ => None,
                        };
                    }
                    let base = self.base(None);
                    let mut r = self.layout(
                        "formula",
                        &id,
                        level,
                        node["caption"].as_str().unwrap_or_default(),
                    );
                    r["cells"] = json!(cells_of(self.plan, acc.as_ref(), base.as_deref(), None));
                    r["drill"] = Value::Null;
                    self.rows.push(r);
                    self.totals.insert(id, acc);
                }
                Some("group") => {
                    let r = self.layout(
                        "group",
                        &id,
                        level,
                        node["caption"].as_str().unwrap_or_default(),
                    );
                    self.rows.push(r);
                    let children: Vec<Value> =
                        node["children"].as_array().cloned().unwrap_or_default();
                    let (total, total_normal) = self.nodes(d, &children, level + 1);
                    if node["showTotal"] == true {
                        let base = self.base(None);
                        let caption = node["totalCaption"].as_str().unwrap_or("Total");
                        let mut r = self.layout("subtotal", &id, level, caption);
                        r["cells"] =
                            json!(cells_of(self.plan, total.as_ref(), base.as_deref(), None));
                        self.rows.push(r);
                    }
                    if let (Some(s), Some(t)) = (sum.as_mut(), total.as_ref()) {
                        add_vals(s, t);
                    }
                    if let (Some(s), Some(t)) = (normal.as_mut(), total_normal.as_ref()) {
                        add_vals(s, t);
                    }
                    self.totals.insert(id, total_normal);
                }
                Some("section") => {
                    let total = self.section(d, node, &id, level);
                    let total_normal = normal_of(node, total.as_ref());
                    match (sum.as_mut(), total.as_ref()) {
                        (Some(s), Some(t)) => add_vals(s, t),
                        _ => sum = None,
                    }
                    match (normal.as_mut(), total_normal.as_ref()) {
                        (Some(s), Some(t)) => add_vals(s, t),
                        _ => normal = None,
                    }
                    self.totals.insert(id, total_normal);
                }
                _ => {}
            }
        }
        (sum, normal)
    }

    fn section(&mut self, d: &mut DemoState, node: &Value, id: &str, level: usize) -> Option<Vals> {
        let np = vec![format!("n:{id}")];
        let s = section_lines(d, self.books, node, self.plan);
        if s.denied {
            self.denied.push(id.to_string());
            let r = self.layout(
                "denied",
                id,
                level,
                "Your role can't see this part of the report",
            );
            self.rows.push(r);
            return None;
        }
        let r = self.layout(
            "header",
            id,
            level,
            node["caption"].as_str().unwrap_or_default(),
        );
        self.rows.push(r);
        let display = node["display"].as_str().unwrap_or("summary");
        let shown: Vec<&Line> = s
            .lines
            .iter()
            .filter(|l| node["hideZero"] != true || l.vals.iter().any(|v| !v.is_zero()))
            .collect();
        let mut total = vec![Decimal::ZERO; self.plan.width()];
        for l in &shown {
            add_vals(&mut total, &l.vals);
        }
        let base = self.base(Some(&total));
        for l in shown {
            let path = vec![np[0].clone(), l.seg.clone()];
            let open =
                display == "detail" || (display == "expandable" && is_open(self.req, node, &path));
            self.rows.push(json!({
                "kind": "line", "nodeId": id, "level": level + 1, "path": path, "caption": l.caption,
                "cells": cells_of(self.plan, Some(&l.vals), base.as_deref(), None),
                "drill": {"type": "detail", "nodeId": id, "path": path},
                "expandable": display == "expandable", "expanded": open
            }));
            if open {
                let cap = if self.preview {
                    cl_api::resources::report_builder::PREVIEW_DETAIL_ROWS
                } else if display == "detail" {
                    DEMO_DETAIL_RUN
                } else {
                    DEMO_PAGE
                };
                for x in l.details.iter().take(cap) {
                    self.rows.push(detail_row(self.plan, node, &path, x));
                }
                if l.details.len() > cap {
                    self.rows.push(json!({
                        "kind": "more", "nodeId": id, "level": level + 2, "path": path,
                        "caption": "More rows",
                        "cells": self.plan.cols.iter().map(|_| Value::Null).collect::<Vec<_>>(),
                        "cursor": cap.to_string(), "expandable": false, "expanded": false
                    }));
                }
            }
        }
        if node["showTotal"] == true {
            self.rows.push(json!({
                "kind": "total", "nodeId": id, "level": level, "path": np,
                "caption": node["totalCaption"].as_str().unwrap_or("Total"),
                "cells": cells_of(self.plan, Some(&total), base.as_deref(), None),
                "drill": {"type": "detail", "nodeId": id, "path": np},
                "expandable": false, "expanded": false
            }));
        }
        Some(total)
    }
}

/// The demo's `/report-builder/run`.
///
/// # Errors
/// 503 `REPORT_TIMEOUT` for the slow starter; 400 `DEFINITION_INVALID`
/// for a broken rule or a stale id; 400 `INVALID_PARAM` for ranges the
/// engine refuses; an adapter error if the demo's JSON were off contract.
pub fn run(d: &mut DemoState, req: &RunRequest, slow: bool) -> Result<BuilderRun, ApiError> {
    if slow {
        return Err(engine_refusal(
            503,
            &json!({"error": {"message": "The report took too long.", "code": "REPORT_TIMEOUT"}}),
        ));
    }
    let def = req
        .reference
        .definition
        .clone()
        .ok_or(ApiError::Local(cl_api::LocalValidation::Unsupported))?;
    let books = Books::load(d);
    check_definition(&books, &def)?;
    let plan = plan(&def, &req.reference)?;
    let top: Vec<Value> = def["nodes"].as_array().cloned().unwrap_or_default();
    let percent_of = def["columns"]["percentOf"].as_str().map(str::to_string);
    let mut r = Render {
        req,
        plan: &plan,
        books: &books,
        percent_of,
        preview: req.purpose == cl_api::resources::report_builder::RunPurpose::Preview,
        rows: Vec::new(),
        totals: Totals::new(),
        denied: Vec::new(),
    };
    r.nodes(d, &top, 0);
    let Render {
        rows,
        denied,
        preview,
        ..
    } = r;
    let cut = preview && rows.iter().any(|x| x["kind"] == "more");
    let notes: Vec<&str> = if rows.iter().all(|r| {
        r["cells"]
            .as_array()
            .is_none_or(|c| c.iter().all(Value::is_null))
    }) {
        vec!["No activity for these dates"]
    } else {
        Vec::new()
    };
    let (from, to) = span(&req.reference.dates);
    let dates = match req.reference.dates.as_of {
        Some(a) => json!({"asOf": a.to_wire()}),
        None => json!({"from": from.to_wire(), "to": to.to_wire()}),
    };
    let v = json!({
        "columns": plan.cols, "rows": rows,
        "meta": {"basis": req.reference.basis.map_or("accrual", cl_core::report::Basis::as_str),
                 "dates": dates, "notes": notes, "truncated": cut, "hiddenNotice": null, "denied": denied,
                 "preview": if preview { json!({"detailRowsPerLine": cl_api::resources::report_builder::PREVIEW_DETAIL_ROWS}) } else { Value::Null }},
        "fingerprint": fingerprint(&req.reference),
        "stamp": stamp(&books),
    });
    read_run(&v, req.reference.adjusting)
}

/// The section `node_id` anywhere in the tree.
fn find_node(nodes: &[Value], node_id: &str) -> Option<Value> {
    for n in nodes {
        if n["id"] == node_id {
            return Some(n.clone());
        }
        if let Some(found) = n["children"].as_array().and_then(|c| find_node(c, node_id)) {
            return Some(found);
        }
    }
    None
}

/// The demo's `/report-builder/run/detail`: the path's lines from the
/// cursor (an offset), [`DEMO_PAGE`] at a time.
///
/// # Errors
/// 409 `RUN_CHANGED {reason:'books'}` when the demo books moved since the
/// run; 403 for a denied section.
pub fn detail(
    d: &mut DemoState,
    req: &DetailRequest,
    columns: &[ColumnHead],
    layout: &ColumnLayout,
) -> Result<DetailPage, ApiError> {
    if req.fingerprint != fingerprint(&req.reference) {
        return Err(engine_refusal(
            409,
            &json!({"error": {"message": "The report changed.", "code": "RUN_CHANGED", "details": {"reason": "params"}}}),
        ));
    }
    let books = Books::load(d);
    if req.stamp != stamp(&books) {
        return Err(engine_refusal(
            409,
            &json!({"error": {"message": "The books changed.", "code": "RUN_CHANGED", "details": {"reason": "books"}}}),
        ));
    }
    let def = req
        .reference
        .definition
        .clone()
        .ok_or(ApiError::Local(cl_api::LocalValidation::Unsupported))?;
    let nodes: Vec<Value> = def["nodes"].as_array().cloned().unwrap_or_default();
    let node = find_node(&nodes, &req.node_id).unwrap_or(Value::Null);
    let plan = plan(&def, &req.reference)?;
    // The engine's path rule (U-119e): the section (its Total) or a prefix
    // of its group-bys; and a range must be one of the period columns'.
    let keys: Vec<&str> = node["groupBy"]
        .as_array()
        .map(|g| g.iter().filter_map(|k| k["key"].as_str()).collect())
        .unwrap_or_default();
    let path_ok = req
        .path
        .0
        .first()
        .is_some_and(|p| *p == format!("n:{}", req.node_id))
        && req.path.0.len() <= keys.len() + 1
        && req.path.0[1..]
            .iter()
            .zip(&keys)
            .all(|(seg, k)| seg.split_once(':').is_some_and(|(sk, _)| sk == *k));
    let range_ok = req.range.is_none_or(|r| {
        req.reference.period_ranges.len() > 1
            && req
                .reference
                .period_ranges
                .iter()
                .any(|p| p.to == r.to && (r.from == CalendarDate::min() || p.from == r.from))
    });
    if !path_ok || !range_ok {
        return Err(engine_refusal(
            400,
            &json!({"error": {"message": "That part of the report can't be opened.", "code": "INVALID_PARAM"}}),
        ));
    }
    let s = section_lines(d, &books, &node, &plan);
    if s.denied {
        return Err(engine_refusal(
            403,
            &json!({"error": {"message": "Not allowed.", "code": "PERMISSION_DENIED"}}),
        ));
    }
    let lines: Vec<&Line> = s
        .lines
        .iter()
        .filter(|l| req.path.0.len() == 1 || req.path.0.get(1) == Some(&l.seg))
        .collect();
    let details: Vec<&Detail> = lines
        .iter()
        .flat_map(|l| l.details.iter())
        // One period column: only its lines (U-119e).
        .filter(|x| {
            req.range.is_none_or(|r| {
                x.date <= r.to && (r.from == CalendarDate::min() || x.date >= r.from)
            })
        })
        .collect();
    let start: usize = req
        .cursor
        .as_deref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let page: Vec<Value> = details
        .iter()
        .skip(start)
        .take(DEMO_PAGE)
        .map(|x| {
            let mut r = detail_row(&plan, &node, &req.path.0, x);
            r["level"] = json!(0);
            r
        })
        .collect();
    let next = (start + DEMO_PAGE < details.len()).then(|| (start + DEMO_PAGE).to_string());
    let mut total = vec![Decimal::ZERO; plan.width()];
    for l in &lines {
        add_vals(&mut total, &l.vals);
    }
    let v = json!({"rows": page, "cursor": next, "subtotal": cells_of(&plan, Some(&total), None, None)});
    read_detail(&v, columns, layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A07 QA-2 parity: a demo check is `check` in a builder row, as the
    /// engine sends it; other expenses stay `expense`.
    #[test]
    fn checks_are_named_check_in_builder_rows() {
        let d = DemoState::default();
        let books = Books::load(&d);
        let check = books
            .posts
            .iter()
            .find(|p| p.is_check)
            .expect("a demo check");
        assert_eq!(builder_type(check), "check");
        let plan = Plan {
            cols: Vec::new(),
            specs: Vec::new(),
            slots: vec![Slot {
                from: None,
                to: CalendarDate::max(),
            }],
            buckets: false,
        };
        let section = json!({"source": "gl", "polarity": "debit", "filters": [], "groupBy": [{"key": "account"}]});
        let lines = post_lines(&books, &section, &plan);
        let x = lines
            .iter()
            .flat_map(|l| &l.details)
            .find(|x| x.txn == check.txn.to_string())
            .expect("the check's line");
        let row = detail_row(&plan, &section, &[], x);
        assert_eq!(row["drill"]["txnType"], "check");
        assert!(
            books
                .posts
                .iter()
                .any(|p| !p.is_check && builder_type(p) == p.txn_type)
        );
    }

    #[test]
    fn the_demo_starters_are_valid_definitions() {
        for s in starters().expect("starters") {
            let text = serde_json::to_string(&s.definition).expect("json");
            let def =
                cl_model::report_def::parse(&text).unwrap_or_else(|e| panic!("{}: {e:?}", s.key));
            cl_model::report_def::check(&def).unwrap_or_else(|e| panic!("{}: {e:?}", s.key));
        }
    }

    // ---- A08 WP-B4: demo parity ------------------------------------------------

    use cl_api::resources::report_builder::{BuilderRefusal, RunPurpose, RunRef, builder_refusal};
    use cl_core::report::{Cell, DateRange, ReportTable, RowKind};

    fn day(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn range(from: CalendarDate, to: CalendarDate) -> RunDates {
        RunDates::range(DateRange { from, to })
    }

    fn req(def: &Value, dates: RunDates, compare: Vec<Vec<DateRange>>) -> RunRequest {
        RunRequest {
            reference: RunRef {
                definition: Some(def.clone()),
                definition_id: None,
                version: None,
                dates,
                period_ranges: Vec::new(),
                compare_ranges: compare,
                fiscal_year_starts: vec![day(2025, 1, 1), day(2026, 1, 1)],
                basis: None,
                adjusting: None,
            },
            open: Vec::new(),
            collapsed: Vec::new(),
            expand_all: false,
            purpose: RunPurpose::Screen,
        }
    }

    fn section(id: &str, caption: &str, types: &[&str]) -> Value {
        json!({"type": "section", "id": id, "caption": caption, "source": "gl", "measure": "amount",
               "filters": [{"field": "account_type", "op": "in", "values": types}],
               "groupBy": [{"key": "account"}], "display": "summary", "defaultExpanded": false,
               "showTotal": true, "hideZero": true})
    }

    fn gross_profit(columns: &Value) -> Value {
        json!({"schemaVersion": 1, "kind": "builder", "dates": {"mode": "range"},
               "columns": columns,
               "nodes": [section("income", "Income", &["revenue"]),
                         section("cogs", "Cost of goods sold", &["cogs"]),
                         {"type": "formula", "id": "gp", "caption": "Gross profit",
                          "terms": [{"nodeId": "income", "sign": 1}, {"nodeId": "cogs", "sign": -1}]}]})
    }

    fn plain_columns() -> Value {
        json!({"detailFields": [], "compare": [], "headers": {}})
    }

    fn cell(t: &ReportTable, kind: RowKind, node: &str, key: &str) -> Cell {
        let c = t
            .columns
            .iter()
            .position(|c| c.key == key)
            .unwrap_or_else(|| panic!("column {key}"));
        t.rows
            .iter()
            .find(|r| r.kind == kind && r.path.0 == [format!("n:{node}")])
            .unwrap_or_else(|| panic!("{node} {kind:?}"))
            .cells[c]
            .clone()
    }

    fn amount(c: &Cell) -> Decimal {
        match c {
            Cell::Amount(d) => *d,
            other => panic!("not an amount: {other:?}"),
        }
    }

    fn october() -> RunDates {
        range(day(2026, 10, 1), day(2026, 10, 31))
    }

    /// UI review H2: the builder's P&L sections tie to the standard Profit &
    /// Loss on the same books (October 2026), and an A/R run that seeds the
    /// demo's credit memo moves both the same way.
    #[test]
    fn a_builder_pnl_ties_to_the_standard_profit_and_loss() {
        let mut d = DemoState::new();
        let def = gross_profit(&plain_columns());
        let standard = |d: &DemoState, caption: &str| {
            let req = cl_core::report::ReportRequest::new(
                cl_core::report::Source::ProfitLoss,
                cl_core::report::ReportDates::Range(DateRange {
                    from: day(2026, 10, 1),
                    to: day(2026, 10, 31),
                }),
            );
            let pl = super::super::reports::statement(d, &req).expect("p&l");
            let row = pl
                .rows
                .iter()
                .find(|r| r.caption == caption)
                .unwrap_or_else(|| panic!("{caption}"));
            amount(&row.cells[0])
        };
        for seeded in [false, true] {
            if seeded {
                super::super::credit_memos::seed(&mut d);
            }
            let t = run(&mut d, &req(&def, october(), Vec::new()), false)
                .expect("run")
                .table;
            assert_eq!(
                amount(&cell(&t, RowKind::Total, "income", "total")),
                standard(&d, "@reports.pl.total_income"),
                "seeded {seeded}"
            );
            assert_eq!(
                amount(&cell(&t, RowKind::Formula, "gp", "total")),
                standard(&d, "@reports.pl.gross_profit"),
                "seeded {seeded}"
            );
        }
    }
    /// U-218 parity: a formula adds its terms in their normal sign, so
    /// flipping "Show as positive" on COGS changes the COGS rows only;
    /// Gross profit and a % of Gross profit base are unchanged, and the
    /// flipped section's own rows are negated.
    #[test]
    fn a_flipped_section_leaves_its_formula_alone() {
        let mut d = DemoState::new();
        let mut cols = plain_columns();
        cols["percentOf"] = json!("gp");
        let natural = gross_profit(&cols);
        let mut flipped = natural.clone();
        flipped["nodes"][1]["polarity"] = json!("credit");
        let t1 = run(&mut d, &req(&natural, october(), Vec::new()), false)
            .expect("run")
            .table;
        let t2 = run(&mut d, &req(&flipped, october(), Vec::new()), false)
            .expect("run")
            .table;
        let gp = |t: &ReportTable| amount(&cell(t, RowKind::Formula, "gp", "total"));
        assert_eq!(gp(&t1), gp(&t2));
        let cost = |t: &ReportTable| amount(&cell(t, RowKind::Total, "cogs", "total"));
        assert_ne!(cost(&t1), Decimal::ZERO, "the demo has cost of goods");
        assert_eq!(cost(&t2), -cost(&t1));
        // % of Gross profit: the base is the same, so the income row's
        // percent is unchanged.
        let pct = |t: &ReportTable| cell(t, RowKind::Total, "income", "total:pct");
        assert_eq!(pct(&t1), pct(&t2));
    }

    /// R16: overlapping periods are refused as the engine refuses them
    /// (a period starting on or before the end of the one before).
    #[test]
    fn overlapping_periods_are_refused() {
        let mut d = DemoState::default();
        let def = gross_profit(&plain_columns());
        let mut r = req(&def, range(day(2026, 10, 1), day(2026, 11, 30)), Vec::new());
        r.reference.period_ranges = vec![
            DateRange {
                from: day(2026, 10, 1),
                to: day(2026, 10, 31),
            },
            DateRange {
                from: day(2026, 10, 15),
                to: day(2026, 11, 30),
            },
        ];
        assert!(run(&mut d, &r, false).is_err());
        r.reference.period_ranges[1].from = day(2026, 11, 1);
        assert!(run(&mut d, &r, false).is_ok());
    }

    #[test]
    fn sections_by_account_type_and_a_formula_over_them() {
        let mut d = DemoState::default();
        let def = gross_profit(&plain_columns());
        let t = run(&mut d, &req(&def, october(), Vec::new()), false)
            .expect("run")
            .table;
        let income = amount(&cell(&t, RowKind::Total, "income", "total"));
        let cogs = amount(&cell(&t, RowKind::Total, "cogs", "total"));
        assert!(!income.is_zero() && !cogs.is_zero());
        assert_eq!(
            amount(&cell(&t, RowKind::Formula, "gp", "total")),
            income - cogs
        );
    }

    #[test]
    fn a_prior_period_compare_uses_the_ranges_sent() {
        let mut d = DemoState::default();
        let mut cols = plain_columns();
        cols["compare"] = json!([{"base": "prior_period", "amount": true, "dollarChange": true, "percentChange": true}]);
        let def = gross_profit(&cols);
        let sept = DateRange {
            from: day(2026, 9, 1),
            to: day(2026, 9, 30),
        };
        let t = run(&mut d, &req(&def, october(), vec![vec![sept]]), false)
            .expect("run")
            .table;
        let s = run(
            &mut d,
            &req(
                &gross_profit(&plain_columns()),
                range(sept.from, sept.to),
                Vec::new(),
            ),
            false,
        )
        .expect("september")
        .table;
        for (kind, node) in [(RowKind::Total, "income"), (RowKind::Formula, "gp")] {
            let prior = amount(&cell(&t, kind, node, "total:c0"));
            assert_eq!(prior, amount(&cell(&s, kind, node, "total")), "{node}");
            assert_eq!(
                amount(&cell(&t, kind, node, "total:c0:change")),
                amount(&cell(&t, kind, node, "total")) - prior
            );
        }
        assert!(matches!(
            cell(&t, RowKind::Total, "income", "total:c0:pct_change"),
            Cell::Percent(_)
        ));
        // A mismatched compare array is refused, as by the engine.
        let bad = req(&def, october(), vec![vec![sept, sept]]);
        assert!(run(&mut d, &bad, false).is_err());
    }

    #[test]
    fn percent_of_the_section_and_renamed_headers() {
        let mut d = DemoState::default();
        let mut cols = plain_columns();
        cols["percentOf"] = json!("section");
        cols["headers"] = json!({"total": "Oct 2026", "total:pct": "Share"});
        let t = run(
            &mut d,
            &req(&gross_profit(&cols), october(), Vec::new()),
            false,
        )
        .expect("run")
        .table;
        let total = t.columns.iter().find(|c| c.key == "total").expect("total");
        assert_eq!(total.caption, "Oct 2026");
        let pct = t.columns.iter().find(|c| c.key == "total:pct").expect("%");
        assert_eq!(pct.caption, "Share");
        assert_eq!(
            cell(&t, RowKind::Total, "income", "total:pct"),
            Cell::Percent(Some(Decimal::ONE_HUNDRED))
        );
    }

    #[test]
    fn ar_open_by_client_with_aging_columns_as_of_a_date() {
        let mut d = DemoState::default();
        let def = json!({"schemaVersion": 1, "kind": "builder", "dates": {"mode": "asOf"},
            "columns": {"detailFields": [], "agingBuckets": true, "compare": [], "headers": {}},
            "nodes": [{"type": "section", "id": "ar", "caption": "Who owes me", "source": "ar_open",
                       "measure": "open_balance", "filters": [], "groupBy": [{"key": "contact"}],
                       "display": "summary", "defaultExpanded": false, "showTotal": true, "hideZero": true}]});
        let at = day(2026, 10, 31);
        let t = run(&mut d, &req(&def, RunDates::as_of(at), Vec::new()), false)
            .expect("run")
            .table;
        let keys: Vec<&str> = t.columns.iter().map(|c| c.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "bucket:current",
                "bucket:d1_30",
                "bucket:d31_60",
                "bucket:d61_90",
                "bucket:over_90",
                "total"
            ]
        );
        assert!(
            t.rows.iter().any(|r| r.kind == RowKind::Group),
            "a client line"
        );
        let want = super::super::statements::ar_aging(&mut d, at).expect("aging");
        assert_eq!(
            amount(&cell(&t, RowKind::Total, "ar", "total")),
            want.total_exact.as_decimal()
        );
        // Aging columns take no compare ranges (the engine's rule).
        let bad = req(
            &def,
            RunDates::as_of(at),
            vec![vec![DateRange { from: at, to: at }]],
        );
        assert!(run(&mut d, &bad, false).is_err());
    }

    #[test]
    fn a_detail_line_shows_more_than_fifty_transactions() {
        let mut d = DemoState::default();
        let mut s = section("bank", "Bank", &["asset"]);
        s["display"] = json!("detail");
        s["defaultExpanded"] = json!(true);
        s["polarity"] = json!("debit");
        let def = json!({"schemaVersion": 1, "kind": "builder", "dates": {"mode": "range"},
            "columns": {"detailFields": ["date", "memo", "amount"], "compare": [], "headers": {}},
            "nodes": [s]});
        let t = run(
            &mut d,
            &req(&def, range(day(2026, 1, 1), day(2026, 12, 31)), Vec::new()),
            false,
        )
        .expect("run")
        .table;
        let mut most = 0;
        let mut run_len = 0;
        for r in &t.rows {
            if r.kind == RowKind::Detail {
                run_len += 1;
                most = most.max(run_len);
            } else {
                run_len = 0;
            }
        }
        assert!(most > 50, "one line shows {most} transactions");
    }

    #[test]
    fn a_forward_reference_and_a_stale_id_are_refused_with_their_path() {
        let mut d = DemoState::default();
        let mut def = gross_profit(&plain_columns());
        let nodes = def["nodes"].as_array_mut().expect("nodes");
        let formula = nodes.remove(2);
        nodes.insert(0, formula);
        let e = run(&mut d, &req(&def, october(), Vec::new()), false).expect_err("forward");
        assert!(
            matches!(builder_refusal(&e), Some(BuilderRefusal::Invalid { code: Some(c), .. }) if c == "FORMULA_FORWARD_REF"),
            "{:?}",
            builder_refusal(&e)
        );
        let mut def = gross_profit(&plain_columns());
        def["nodes"][0]["filters"]
            .as_array_mut()
            .expect("filters")
            .push(json!({"field": "account", "op": "in", "ids": [STALE_ID]}));
        let e = run(&mut d, &req(&def, october(), Vec::new()), false).expect_err("stale");
        assert_eq!(
            builder_refusal(&e),
            Some(BuilderRefusal::Invalid {
                path: Some("definition.nodes.0.filters.1.ids".into()),
                code: Some("NOT_IN_COMPANY".into()),
            })
        );
    }

    #[test]
    fn builder_rows_save_with_versions_names_and_authors() {
        use super::super::report_store::{author_only, save_definition};
        use cl_api::resources::reports::definitions::{
            DefinitionRefusal, DefinitionWrite, definition_refusal,
        };
        let mut d = DemoState::default();
        let w = DefinitionWrite {
            name: "Gross profit".into(),
            group_id: None,
            kind: "builder".into(),
            definition: gross_profit(&plain_columns()),
            origin: None,
        };
        let row = save_definition(&mut d, None, &w).expect("create");
        assert!(row.can_edit);
        assert_eq!(row.version, 1);
        let next = save_definition(&mut d, Some((row.id.clone(), 1)), &w).expect("update");
        assert_eq!(next.version, 2);
        let stale = save_definition(&mut d, Some((row.id.clone(), 1)), &w).expect_err("stale");
        assert_eq!(definition_refusal(&stale), Some(DefinitionRefusal::Stale));
        let taken = save_definition(&mut d, None, &w).expect_err("taken");
        assert_eq!(
            definition_refusal(&taken),
            Some(DefinitionRefusal::NameTaken)
        );
        let mut bad = w.clone();
        bad.name = "Broken".into();
        let nodes = bad.definition["nodes"].as_array_mut().expect("nodes");
        let formula = nodes.remove(2);
        nodes.insert(0, formula);
        let e = save_definition(&mut d, None, &bad).expect_err("invalid");
        assert!(matches!(
            definition_refusal(&e),
            Some(DefinitionRefusal::Invalid { path: Some(_), code: Some(c) }) if c == "FORMULA_FORWARD_REF"
        ));
        author_only(&mut d, &row.id);
        let e = save_definition(&mut d, Some((row.id.clone(), 2)), &w).expect_err("author");
        assert_eq!(definition_refusal(&e), Some(DefinitionRefusal::NotAuthor));
        let gone = save_definition(&mut d, Some(("nope".into(), 1)), &w).expect_err("404");
        assert!(matches!(gone, ApiError::NotFound { .. }));
    }
}
