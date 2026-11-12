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

const PROFIT_LOSS: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":[],"compare":[],"headers":{}},"nodes":[{"type":"section","id":"income","caption":"Income","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["revenue"]}],"groupBy":[{"key":"account"}],"display":"summary","defaultExpanded":false,"showTotal":true,"totalCaption":"Total Income","hideZero":true},{"type":"section","id":"expenses","caption":"Expenses","source":"gl","measure":"amount","filters":[{"field":"account_type","op":"in","values":["expense"]}],"groupBy":[{"key":"account"}],"display":"expandable","defaultExpanded":false,"showTotal":true,"totalCaption":"Total Expenses","hideZero":true},{"type":"formula","id":"net_income","caption":"Net Income","terms":[{"nodeId":"income","sign":1},{"nodeId":"expenses","sign":-1}]}]}"#;

const LEDGER: &str = r#"{"schemaVersion":1,"kind":"builder","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":["date","txn_type","number","contact","memo","amount"],"compare":[],"headers":{}},"nodes":[{"type":"section","id":"ledger","caption":"Transaction Detail by Account","source":"gl","measure":"amount","polarity":"debit","filters":[],"groupBy":[{"key":"account"}],"display":"detail","defaultExpanded":true,"showTotal":false,"hideZero":true}]}"#;

const SALES_AND_PURCHASES: &str = r#"{"schemaVersion":1,"kind":"builder","title":"Sales and purchases","dates":{"mode":"range","preset":"this_year_to_date"},"columns":{"detailFields":["date","txn_type","number","amount"],"compare":[],"headers":{}},"nodes":[{"type":"text","id":"intro","text":"Sales by client, then purchases."},{"type":"section","id":"sales","caption":"Sales","source":"sales","measure":"amount","filters":[],"groupBy":[{"key":"contact"}],"display":"expandable","defaultExpanded":false,"showTotal":true,"hideZero":true},{"type":"blank","id":"gap"},{"type":"section","id":"purchases","caption":"Purchases","source":"purchases","measure":"amount","filters":[],"groupBy":[{"key":"contact"}],"display":"summary","defaultExpanded":false,"showTotal":true,"hideZero":true},{"type":"formula","id":"net","caption":"Sales less purchases","terms":[{"nodeId":"sales","sign":1},{"nodeId":"purchases","sign":-1}]}]}"#;

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

/// The engine's column list for `def` (no periods or compare in the demo).
fn columns(def: &Value) -> Vec<Value> {
    let mut cols: Vec<Value> = def["columns"]["detailFields"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(
                    |f| json!({"key": format!("f:{f}"), "caption": f, "kind": "field", "field": f}),
                )
                .collect()
        })
        .unwrap_or_default();
    cols.push(json!({"key": "total", "caption": "Total", "kind": "total"}));
    cols
}

fn exact(d: Decimal) -> String {
    let mut d = d;
    d.rescale(4);
    d.to_string()
}

/// A section's lines: (path value, caption, posts) in caption order.
type Lines<'a> = Vec<(String, String, Vec<&'a Post>)>;

struct Section<'a> {
    lines: Lines<'a>,
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

fn natural_credit(section: &Value) -> bool {
    section["source"] == "sales"
        || section["filters"].as_array().is_some_and(|f| {
            f.iter().any(|x| {
                x["field"] == "account_type"
                    && x["values"]
                        .as_array()
                        .is_some_and(|v| v.iter().any(|t| t == "revenue" || t == "other_revenue"))
            })
        })
}

fn section_lines<'a>(
    books: &'a Books,
    section: &Value,
    from: CalendarDate,
    to: CalendarDate,
) -> Section<'a> {
    if section["source"] == "purchases" {
        return Section {
            lines: Vec::new(),
            denied: true,
        };
    }
    let types: Option<BTreeSet<String>> = section["filters"].as_array().and_then(|f| {
        f.iter()
            .find(|x| x["field"] == "account_type" && x["op"] == "in")
            .and_then(|x| x["values"].as_array())
            .map(|v| {
                v.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
    });
    let sales = section["source"] == "sales";
    let key = section["groupBy"][0]["key"].as_str().unwrap_or("account");
    let mut lines: BTreeMap<String, (String, Vec<&Post>)> = BTreeMap::new();
    for p in &books.posts {
        if p.date < from || p.date > to {
            continue;
        }
        let t = account_type_of(books, p);
        if sales && t != "revenue" {
            continue;
        }
        if types.as_ref().is_some_and(|ts| !ts.contains(&t)) {
            continue;
        }
        let (value, caption) = if key == "contact" {
            match &p.contact {
                Some(c) => (c.clone(), c.clone()),
                None => ("none".into(), "No name".into()),
            }
        } else {
            (p.account.to_string(), account_caption(books, p))
        };
        lines
            .entry(caption.clone())
            .or_insert_with(|| (format!("{key}:{value}"), Vec::new()))
            .1
            .push(p);
    }
    Section {
        lines: lines
            .into_iter()
            .map(|(caption, (path, posts))| (path, caption, posts))
            .collect(),
        denied: false,
    }
}

/// The engine's open rule (Q-A07r).
fn is_open(req: &RunRequest, section: &Value, path: &[String]) -> bool {
    let p = path.to_vec();
    req.expand_all
        || req.open.iter().any(|o| o.0 == p)
        || (section["defaultExpanded"] == true && !req.collapsed.iter().any(|c| c.0 == p))
}

fn txn_label(t: &str) -> String {
    let mut s = t.replace('_', " ");
    if let Some(f) = s.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    s
}

/// One detail row's cells.
fn detail_cells(cols: &[Value], section: &Value, p: &Post) -> Vec<Value> {
    let v = value(section, p);
    cols.iter()
        .map(|c| match c["field"].as_str() {
            Some("date") => json!(p.date.to_wire()),
            Some("txn_type") => json!(txn_label(&p.txn_type)),
            Some("number") => json!(p.number),
            Some("contact") => p.contact.as_ref().map_or(Value::Null, |c| json!(c)),
            Some("memo") => json!(p.memo),
            Some("amount") | None => json!(exact(v)),
            Some(_) => Value::Null,
        })
        .collect()
}

fn detail_row(cols: &[Value], section: &Value, path: &[String], p: &Post) -> Value {
    json!({
        "kind": "detail", "nodeId": section["id"], "level": 2, "path": path,
        "caption": "", "cells": detail_cells(cols, section, p),
        "fields": {"txnId": p.txn.to_string(), "txnType": p.txn_type, "lineId": format!("{}", p.line)},
        "drill": {"type": "transaction", "txnId": p.txn.to_string(), "txnType": p.txn_type},
        "expandable": false, "expanded": false
    })
}

fn amounts(cols: &[Value], total: Option<Decimal>) -> Vec<Value> {
    cols.iter()
        .map(|c| match (c["kind"].as_str(), total) {
            (Some("total"), Some(t)) => json!(exact(t)),
            _ => Value::Null,
        })
        .collect()
}

fn span(dates: &RunDates) -> (CalendarDate, CalendarDate) {
    match (dates.from, dates.to, dates.as_of) {
        (Some(f), Some(t), _) => (f, t),
        (_, _, Some(d)) => (CalendarDate::min(), d),
        _ => (CalendarDate::min(), CalendarDate::max()),
    }
}

/// The demo's `/report-builder/run`.
///
/// # Errors
/// 503 `REPORT_TIMEOUT` for the slow starter; an adapter error if the
/// demo's JSON were off contract.
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
    let (from, to) = span(&req.reference.dates);
    let cols = columns(&def);
    let mut rows = Vec::new();
    let mut totals: BTreeMap<String, Option<Decimal>> = BTreeMap::new();
    let mut denied = Vec::new();
    for node in def["nodes"].as_array().into_iter().flatten() {
        let id = node["id"].as_str().unwrap_or_default().to_string();
        let np = vec![format!("n:{id}")];
        let layout = |kind: &str, caption: &str| {
            json!({"kind": kind, "nodeId": id, "level": 0, "path": np, "caption": caption,
                   "cells": amounts(&cols, None), "expandable": false, "expanded": false})
        };
        match node["type"].as_str() {
            Some("text") => rows.push(layout("text", node["text"].as_str().unwrap_or_default())),
            Some(k @ ("blank" | "page_break")) => rows.push(layout(k, "")),
            Some("formula") => {
                let mut sum = Some(Decimal::ZERO);
                for t in node["terms"].as_array().into_iter().flatten() {
                    let term = totals
                        .get(t["nodeId"].as_str().unwrap_or_default())
                        .copied()
                        .flatten();
                    let sign = if t["sign"] == -1 {
                        -Decimal::ONE
                    } else {
                        Decimal::ONE
                    };
                    sum = sum.zip(term).map(|(s, x)| s + sign * x);
                }
                totals.insert(id.clone(), sum);
                let mut r = layout("formula", node["caption"].as_str().unwrap_or_default());
                r["cells"] = json!(amounts(&cols, sum));
                r["drill"] = Value::Null;
                rows.push(r);
            }
            Some("section") => {
                let s = section_lines(&books, node, from, to);
                if s.denied {
                    denied.push(id.clone());
                    totals.insert(id.clone(), None);
                    rows.push(layout(
                        "denied",
                        "Your role can't see this part of the report",
                    ));
                    continue;
                }
                rows.push(layout(
                    "header",
                    node["caption"].as_str().unwrap_or_default(),
                ));
                let display = node["display"].as_str().unwrap_or("summary");
                let mut total = Decimal::ZERO;
                for (seg, caption, posts) in &s.lines {
                    let line_total: Decimal = posts.iter().map(|p| value(node, p)).sum();

                    total += line_total;
                    let path = vec![np[0].clone(), seg.clone()];
                    let open = display == "detail"
                        || (display == "expandable" && is_open(req, node, &path));
                    rows.push(json!({
                        "kind": "line", "nodeId": id, "level": 1, "path": path, "caption": caption,
                        "cells": amounts(&cols, Some(line_total)),
                        "drill": {"type": "detail", "nodeId": id, "path": path},
                        "expandable": display == "expandable", "expanded": open
                    }));
                    if open {
                        for p in posts.iter().take(DEMO_PAGE) {
                            rows.push(detail_row(&cols, node, &path, p));
                        }
                        if posts.len() > DEMO_PAGE {
                            rows.push(json!({
                                "kind": "more", "nodeId": id, "level": 2, "path": path,
                                "caption": "More rows", "cells": amounts(&cols, None),
                                "cursor": DEMO_PAGE.to_string(), "expandable": false, "expanded": false
                            }));
                        }
                    }
                }
                totals.insert(id.clone(), Some(total));
                if node["showTotal"] == true {
                    rows.push(json!({
                        "kind": "total", "nodeId": id, "level": 0, "path": np,
                        "caption": node["totalCaption"].as_str().unwrap_or("Total"),
                        "cells": amounts(&cols, Some(total)),
                        "drill": {"type": "detail", "nodeId": id, "path": np},
                        "expandable": false, "expanded": false
                    }));
                }
            }
            _ => {}
        }
    }
    let notes: Vec<&str> = if rows.iter().all(|r| {
        r["cells"]
            .as_array()
            .is_none_or(|c| c.iter().all(Value::is_null))
    }) {
        vec!["No activity for these dates"]
    } else {
        Vec::new()
    };
    let dates = match req.reference.dates.as_of {
        Some(a) => json!({"asOf": a.to_wire()}),
        None => json!({"from": from.to_wire(), "to": to.to_wire()}),
    };
    let v = json!({
        "columns": cols, "rows": rows,
        "meta": {"basis": req.reference.basis.map_or("accrual", cl_core::report::Basis::as_str),
                 "dates": dates, "notes": notes, "truncated": false, "hiddenNotice": null, "denied": denied},
        "fingerprint": "d".repeat(64),
        "stamp": stamp(&books),
    });
    read_run(&v, req.reference.adjusting)
}

/// The demo's `/report-builder/run/detail`: the path's posts from the
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
    let node = def["nodes"]
        .as_array()
        .and_then(|n| n.iter().find(|x| x["id"] == req.node_id.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    let (from, to) = span(&req.reference.dates);
    let s = section_lines(&books, &node, from, to);
    if s.denied {
        return Err(engine_refusal(
            403,
            &json!({"error": {"message": "Not allowed.", "code": "PERMISSION_DENIED"}}),
        ));
    }
    let cols = self::columns(&def);
    let posts: Vec<&Post> = s
        .lines
        .iter()
        .filter(|(seg, _, _)| req.path.0.len() == 1 || req.path.0.get(1) == Some(seg))
        .flat_map(|(_, _, p)| p.iter().copied())
        .collect();
    let start: usize = req
        .cursor
        .as_deref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let page: Vec<Value> = posts
        .iter()
        .skip(start)
        .take(DEMO_PAGE)
        .map(|p| {
            let mut r = detail_row(&cols, &node, &req.path.0, p);
            r["level"] = json!(0);
            r
        })
        .collect();
    let next = (start + DEMO_PAGE < posts.len()).then(|| (start + DEMO_PAGE).to_string());
    let total: Decimal = posts.iter().map(|p| value(&node, p)).sum();

    let v = json!({"rows": page, "cursor": next, "subtotal": amounts(&cols, Some(total))});
    read_detail(&v, columns, layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_demo_starters_are_valid_definitions() {
        for s in starters().expect("starters") {
            let text = serde_json::to_string(&s.definition).expect("json");
            let def =
                cl_model::report_def::parse(&text).unwrap_or_else(|e| panic!("{}: {e:?}", s.key));
            cl_model::report_def::check(&def).unwrap_or_else(|e| panic!("{}: {e:?}", s.key));
        }
    }
}
