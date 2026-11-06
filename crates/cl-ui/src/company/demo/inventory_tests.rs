//! The demo costing against `fixtures/inventory/golden.json` (byte-identical
//! to the engine's `packages/shared/fixtures/inventory-golden.json`): every
//! case not marked `engineOnly`.

use std::collections::BTreeMap;

use serde_json::Value;

use super::*;

const GOLDEN: &str = include_str!("../../../../../fixtures/inventory/golden.json");

fn item(name: &str) -> ItemId {
    let n = match name {
        "W" => 1,
        "X" => 2,
        _ => 3,
    };
    ItemId::parse(&format!("d0000000-0000-4000-8000-e000000000{n:02}")).expect("id")
}
fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
fn m(t: &str) -> Money {
    Money::parse_wire(t).expect("money")
}
fn q(t: &str) -> Quantity {
    Quantity::parse_wire(t).expect("qty")
}
fn d(t: &str) -> CalendarDate {
    CalendarDate::parse_wire(t).expect("date")
}

/// A document the cases post: its date, item, movement and linked sale.
#[derive(Clone)]
struct Doc {
    date: CalendarDate,
    item: ItemId,
    kind: MoveKind,
    return_of: Option<String>,
}

/// The demo ledger plus the cases' document table.
#[derive(Clone, Default)]
struct Book {
    inv: DemoInventory,
    docs: BTreeMap<String, Doc>,
}

fn kind_of(step: &Value) -> MoveKind {
    let qty = || q(s(step, "quantity").expect("quantity"));
    let amount = || m(s(step, "amount").expect("amount"));
    match s(step, "event").expect("event") {
        "bill" => MoveKind::Receipt {
            quantity: qty(),
            amount: amount(),
        },
        "vendor_credit" => MoveKind::VendorReturn {
            quantity: qty(),
            amount: amount(),
        },
        "invoice" => MoveKind::Sale { quantity: qty() },
        "credit_memo" => MoveKind::CustomerReturn { quantity: qty() },
        "adjust_qty" => MoveKind::AdjustQuantity {
            change: q(s(step, "quantityChange").expect("change")),
        },
        "adjust_value" => MoveKind::AdjustValue {
            change: m(s(step, "valueChange").expect("change")),
        },
        other => panic!("unknown event {other}"),
    }
}

fn with_quantity(k: MoveKind, quantity: Quantity) -> MoveKind {
    match k {
        MoveKind::Sale { .. } => MoveKind::Sale { quantity },
        MoveKind::CustomerReturn { .. } => MoveKind::CustomerReturn { quantity },
        MoveKind::Receipt { amount, .. } => MoveKind::Receipt { quantity, amount },
        MoveKind::VendorReturn { amount, .. } => MoveKind::VendorReturn { quantity, amount },
        other => other,
    }
}

fn line(doc: &Doc) -> Vec<DocLine> {
    vec![DocLine {
        item: doc.item,
        kind: doc.kind,
        fallback_cost: None,
        return_of: doc.return_of.clone(),
    }]
}

/// Runs one step; answers the warnings and, for a count, the count lines.
/// A refusal is the engine code (the step's `expect.code`).
fn run(
    book: &mut Book,
    step: &Value,
    suffix: &str,
    item_name: Option<&str>,
) -> Result<Vec<cl_api::NegativeStock>, &'static str> {
    let event = s(step, "event").expect("event");
    let doc = format!("{}{suffix}", s(step, "doc").unwrap_or("?"));
    let code = |e: DemoInventoryError| e.code();
    Ok(match event {
        "void" | "delete" => book.inv.remove(&doc).map_err(code)?,
        "edit" => {
            let mut e = book.docs.get(&doc).expect("doc").clone();
            if let Some(n) = s(step, "quantity") {
                e.kind = with_quantity(e.kind, q(n));
            }
            if let Some(t) = s(step, "date") {
                e.date = d(t);
            }
            let w = book.inv.edit(&doc, e.date, &line(&e)).map_err(code)?;
            book.docs.insert(doc, e);
            w
        }
        "count" => {
            let lines: Vec<cl_api::CountLine> = step["lines"]
                .as_array()
                .expect("lines")
                .iter()
                .map(|l| cl_api::CountLine {
                    item_id: item(s(l, "item").expect("item")),
                    sheet_on_hand: q(s(l, "sheetOnHand").expect("sheet")),
                    counted_quantity: q(s(l, "countedQuantity").expect("counted")),
                    unit_id: None,
                })
                .collect();
            let (_, w) = book
                .inv
                .post_count(&doc, d(s(step, "date").expect("date")), &lines)
                .map_err(code)?;
            w
        }
        _ => {
            let e = Doc {
                date: d(s(step, "date").expect("date")),
                item: item(item_name.or_else(|| s(step, "item")).expect("item")),
                kind: kind_of(step),
                return_of: s(step, "returnOf").map(|r| format!("{r}{suffix}")),
            };
            // As the demo routes do: the linked-return rules first.
            book.inv
                .check_write(&doc, e.date, &line(&e))
                .map_err(code)?;
            let w = book.inv.post(&doc, e.date, &line(&e)).map_err(code)?;
            book.docs.insert(doc, e);
            w
        }
    })
}

fn row<'a>(rows: &'a [Row], doc: &str, it: ItemId) -> &'a Row {
    rows.iter()
        .find(|r| r.doc == doc && r.item == it)
        .unwrap_or_else(|| panic!("no row for {doc}"))
}

fn check_row(r: &Row, e: &Value, ctx: &str) {
    if let Some(v) = s(e, "value") {
        assert_eq!(r.value, m(v), "{ctx} value");
    }
    if let Some(c) = s(e, "correction") {
        assert_eq!(
            r.correction.checked_neg().expect("neg"),
            m(c),
            "{ctx} correction"
        );
    }
    if let Some(c) = s(e, "residual") {
        assert_eq!(
            r.residual.checked_neg().expect("neg"),
            m(c),
            "{ctx} residual"
        );
    }
    if let Some(v) = s(e, "onHand") {
        assert_eq!(r.on_hand.to_wire(), v, "{ctx} onHand");
    }
    if let Some(v) = s(e, "assetValue") {
        assert_eq!(r.asset_value, m(v), "{ctx} assetValue");
    }
    if let Some(v) = s(e, "averageCost") {
        assert_eq!(
            r.last_cost.map(UnitCost::to_wire).as_deref(),
            Some(v),
            "{ctx} averageCost"
        );
    }
}

fn check_rows(book: &Book, expected: &Value, ctx: &str) {
    let rows = book.inv.rows().expect("rows");
    let w: Vec<&Row> = rows.iter().filter(|r| r.item == item("W")).collect();
    let list = expected.as_array().expect("rows");
    if list.len() < w.len() {
        // Only some rows listed (case R): each by its document, in order.
        let mut at = 0;
        for e in list {
            let doc = s(e, "doc").expect("doc");
            let i = w[at..]
                .iter()
                .position(|r| r.doc == doc)
                .unwrap_or_else(|| panic!("{ctx}: no row {doc} after the last"));
            at += i;
            check_row(w[at], e, &format!("{ctx} {doc}"));
        }
        return;
    }
    assert_eq!(w.len(), list.len(), "{ctx}: row count");
    for (r, e) in w.iter().zip(list) {
        assert_eq!(r.doc, s(e, "doc").expect("doc"), "{ctx}: position order");
        check_row(r, e, &format!("{ctx} {}", r.doc));
    }
}

fn check_warnings(w: &[cl_api::NegativeStock], expected: &Value, ctx: &str) {
    let list = expected.as_array().expect("warnings");
    assert_eq!(w.len(), list.len(), "{ctx}: warnings {w:?}");
    for (got, e) in w.iter().zip(list) {
        assert_eq!(s(e, "code"), Some("NEGATIVE_STOCK"));
        assert_eq!(got.item_id, item(s(e, "item").expect("item")), "{ctx}");
        assert_eq!(got.date, d(s(e, "date").expect("date")), "{ctx}");
        assert_eq!(
            got.on_hand.to_wire(),
            s(e, "onHand").expect("onHand"),
            "{ctx}"
        );
    }
}

fn case<'a>(all: &'a [Value], id: &str) -> &'a Value {
    all.iter()
        .find(|c| s(c, "id") == Some(id))
        .unwrap_or_else(|| panic!("case {id}"))
}

/// Builds `id`'s end state, checking every expectation on the way.
#[allow(
    clippy::many_single_char_names,
    reason = "the golden file's short keys"
)]
fn play(all: &[Value], id: &str) -> Book {
    play_to(all, id, None)
}

/// [`play`], stopping after step `limit` (a case built on the first steps
/// of another, `fromSteps`; the checks after the steps are left out then).
#[allow(
    clippy::many_single_char_names,
    reason = "the golden file's short keys"
)]
fn play_to(all: &[Value], id: &str, limit: Option<u64>) -> Book {
    let c = case(all, id);
    let mut book = match s(c, "from") {
        Some(from) => play_to(all, from, c["fromSteps"].as_u64()),
        None => Book::default(),
    };
    if c.get("setup").is_some() {
        // F: X and Y run case A's steps as W does.
        for name in ["X", "Y"] {
            for step in case(all, "A")["steps"].as_array().expect("steps") {
                run(&mut book, step, name, Some(name)).expect("setup step");
            }
        }
    }
    // Report cases (S) have no steps of their own.
    for step in c["steps"].as_array().map_or(&[][..], Vec::as_slice) {
        let n = step["n"].as_u64().expect("n");
        if limit.is_some_and(|l| n > l) {
            break;
        }
        let ctx = format!("case {id} step {n}");
        let refused = s(&step["expect"], "code");
        // Units (golden U): the demo's unit rules, then the step in base units.
        let (step, unit_refused) = match c.get("unitSet") {
            Some(set) => unit_step(set, step, &ctx),
            None => (step.clone(), None),
        };
        let step = &step;
        if let Some(e) =
            s(step, "event").filter(|e| *e == "item_unit_prices" || *e == "unit_factor_change")
        {
            let _ = e;
            assert_eq!(unit_refused, refused.map(str::to_string), "{ctx}");
            continue;
        }
        if let Some(got) = unit_refused {
            assert_eq!(Some(got.as_str()), refused, "{ctx} refusal");
            continue;
        }
        let w = match (run(&mut book, step, "", None), refused) {
            (Ok(w), None) => w,
            (Err(got), Some(want)) => {
                assert_eq!(got, want, "{ctx} refusal");
                continue;
            }
            (Ok(_), Some(want)) => panic!("{ctx}: expected {want}"),
            (Err(got), None) => panic!("{ctx}: refused {got}"),
        };
        let Some(e) = step.get("expect") else {
            if let Some(after) = c.get("rowsAfter").and_then(|r| r.get(n.to_string())) {
                check_rows(&book, after, &ctx);
            }
            continue;
        };
        if let Some(ws) = e.get("warnings") {
            check_warnings(&w, ws, &ctx);
        }
        let rows = book.inv.rows().expect("rows");
        if let Some(lines) = e.get("lines") {
            let doc = s(step, "doc").expect("doc");
            for l in lines.as_array().expect("lines") {
                let it = item(s(l, "item").expect("item"));
                let r = row(&rows, doc, it);
                assert_eq!(
                    r.on_hand.to_wire(),
                    s(l, "onHand").expect("on hand"),
                    "{ctx}"
                );
                assert_eq!(r.value, m(s(l, "value").expect("value")), "{ctx}");
                assert_eq!(r.asset_value, m(s(l, "assetValue").expect("v")), "{ctx}");
            }
            for none in e
                .get("noLine")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let it = item(none.as_str().expect("item"));
                assert!(
                    !rows.iter().any(|r| r.doc == doc && r.item == it),
                    "{ctx} no line"
                );
            }
        } else if let Some(doc) = s(step, "doc").filter(|_| e.get("value").is_some()) {
            check_row(row(&rows, doc, item("W")), e, &ctx);
        } else if e.get("onHand").is_some() {
            let last = rows.iter().rfind(|r| r.item == item("W")).expect("a row");
            check_row(last, e, &ctx);
        }
        if let Some(after) = c.get("rowsAfter").and_then(|r| r.get(n.to_string())) {
            check_rows(&book, after, &ctx);
        }
    }
    if limit.is_some() {
        return book;
    }
    if let Some(rows) = c.get("rows") {
        check_rows(&book, rows, &format!("case {id} rows"));
    }
    if let Some(other) = s(c, "rowsAs") {
        check_rows(
            &book,
            &case(all, other)["rows"],
            &format!("case {id} rows as {other}"),
        );
    }
    if let Some(f) = c.get("final").and_then(Value::as_object) {
        for (name, e) in f {
            let st = book.inv.stock(item(name)).expect("stock");
            let ctx = format!("case {id} final {name}");
            assert_eq!(
                st.quantity_on_hand.map(Quantity::to_wire).as_deref(),
                s(e, "onHand"),
                "{ctx}"
            );
            assert_eq!(st.asset_value, s(e, "assetValue").map(m), "{ctx}");
            if let Some(a) = s(e, "averageCost") {
                assert_eq!(
                    st.average_cost.map(UnitCost::to_wire).as_deref(),
                    Some(a),
                    "{ctx}"
                );
            }
        }
    }
    book
}

/// Golden U runs (the demo's unit sets, P24 WP-3 step 3).
const UNIT_CASES_PENDING: bool = false;

/// A golden U step through the demo's unit rules: a line in a unit becomes
/// its base quantity (`round4`, refused at zero); the unit-price and
/// factor-change events are checked here. Answers the step to run and any
/// refusal code.
fn unit_step(set: &Value, step: &Value, ctx: &str) -> (Value, Option<String>) {
    use crate::company::demo::units;
    use cl_core::UnitFactor;
    let factor_of = |name: &str| -> UnitFactor {
        if s(set, "base") == Some(name) {
            return UnitFactor::ONE;
        }
        set["units"]
            .as_array()
            .expect("units")
            .iter()
            .find(|u| s(u, "name") == Some(name))
            .and_then(|u| s(u, "factor"))
            .map_or_else(
                || panic!("{ctx}: no unit {name}"),
                |f| UnitFactor::parse_wire(f).expect("factor"),
            )
    };
    match s(step, "event").expect("event") {
        "item_unit_prices" => {
            let base = m(s(step, "basePrice").expect("base price"));
            for (unit, want) in step["expect"].as_object().expect("prices") {
                let got = units::unit_price(base, factor_of(unit)).expect("price");
                assert_eq!(got.to_wire(), want.as_str().expect("price"), "{ctx} {unit}");
            }
            return (step.clone(), None);
        }
        "unit_factor_change" => {
            // Every unit of the set has been on a posted line by now.
            let saved = cl_api::UnitSet {
                id: cl_core::ids::UnitSetId::parse("d0000000-0000-4000-8001-000000000001")
                    .expect("id"),
                name: "Bottle".into(),
                base_unit_name: s(set, "base").expect("base").into(),
                base_unit_abbreviation: None,
                is_active: true,
                units: vec![cl_api::Unit {
                    id: Some(
                        cl_core::ids::UnitId::parse("d0000000-0000-4000-8002-000000000001")
                            .expect("id"),
                    ),
                    name: s(step, "unit").expect("unit").into(),
                    abbreviation: None,
                    factor: factor_of(s(step, "unit").expect("unit")),
                }],
                in_use: true,
                updated_at: None,
            };
            let mut next = cl_api::NewUnitSet {
                name: saved.name.clone(),
                base_unit_name: saved.base_unit_name.clone(),
                base_unit_abbreviation: None,
                is_active: true,
                units: saved.units.clone(),
            };
            next.units[0].factor =
                UnitFactor::parse_wire(s(step, "factor").expect("factor")).expect("f");
            let r = units::check_change(&saved, &next, |_| true);
            return (step.clone(), r.err().map(|e| e.code().to_string()));
        }
        _ => {}
    }
    let Some(unit) = s(step, "unit") else {
        return (step.clone(), None);
    };
    let qty = q(s(step, "quantity").expect("quantity"));
    match units::base_quantity(factor_of(unit), qty) {
        Ok(base) => {
            if let Some(b) = s(&step["expect"], "baseQuantity") {
                assert_eq!(base.to_wire(), b, "{ctx} baseQuantity");
            }
            let mut out = step.clone();
            out["quantity"] = Value::String(base.to_wire());
            (out, None)
        }
        Err(e) => (step.clone(), Some(e.code().to_string())),
    }
}

/// Every demo case of the golden file passes (P24 §6.2: average cost
/// matches the reference, negative stock included).
#[test]
fn inventory_golden() {
    let g: Value = serde_json::from_str(GOLDEN).expect("golden json");
    let all = g["cases"].as_array().expect("cases");
    let mut ran = Vec::new();
    for c in all {
        if c.get("engineOnly").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        // Units of measure (golden U) come with the demo unit sets.
        if UNIT_CASES_PENDING && c.get("unitSet").is_some() {
            continue;
        }
        let id = s(c, "id").expect("id");
        play(all, id);
        ran.push(id.to_string());
    }
    for must in ["A", "B", "V", "X", "M", "D", "N", "E", "R", "U", "F"] {
        assert!(ran.iter().any(|r| r == must), "case {must} ran");
    }
}

/// Banker's rounding would give 0.00 for case N's sale (round2(-0.005));
/// the demo gives -0.01 relief, so the value rises by 0.01.
#[test]
fn rounding_is_half_away_from_zero_not_bankers() {
    let w = item("W");
    let mut inv = DemoInventory::default();
    let post = |inv: &mut DemoInventory, doc: &str, day: &str, kind| {
        inv.post(
            doc,
            d(day),
            &[DocLine {
                item: w,
                kind,
                fallback_cost: None,
                return_of: None,
            }],
        )
        .expect("post")
    };
    post(
        &mut inv,
        "N1",
        "2026-01-02",
        MoveKind::Receipt {
            quantity: q("2"),
            amount: m("10.00"),
        },
    );
    post(
        &mut inv,
        "N2",
        "2026-01-03",
        MoveKind::VendorReturn {
            quantity: q("1"),
            amount: m("10.01"),
        },
    );
    post(
        &mut inv,
        "N3",
        "2026-01-04",
        MoveKind::Sale { quantity: q("0.5") },
    );
    let rows = inv.rows().expect("rows");
    let n3 = rows.iter().find(|r| r.doc == "N3").expect("N3");
    assert_eq!(n3.value, m("0.01"));
    let bankers =
        Decimal::new(-5, 3).round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven);
    assert_eq!(
        bankers,
        Decimal::ZERO,
        "the strategy this test guards against"
    );
}

/// Entry order does not matter (I6): the backdated bill gives case B's
/// numbers whether it is entered first or last.
#[test]
fn the_result_depends_on_documents_not_entry_order() {
    let w = item("W");
    let docs: Vec<(&str, &str, MoveKind)> = vec![
        (
            "A1",
            "2026-01-02",
            MoveKind::Receipt {
                quantity: q("10"),
                amount: m("33.33"),
            },
        ),
        (
            "B1",
            "2026-01-03",
            MoveKind::Receipt {
                quantity: q("6"),
                amount: m("12.00"),
            },
        ),
        (
            "A2",
            "2026-01-05",
            MoveKind::Receipt {
                quantity: q("5"),
                amount: m("20.50"),
            },
        ),
        ("A3", "2026-01-08", MoveKind::Sale { quantity: q("4") }),
    ];
    let state = |order: &[usize]| {
        let mut inv = DemoInventory::default();
        for &i in order {
            let (doc, day, kind) = &docs[i];
            inv.post(
                doc,
                d(day),
                &[DocLine {
                    item: w,
                    kind: *kind,
                    fallback_cost: None,
                    return_of: None,
                }],
            )
            .expect("post");
        }
        inv.stock(w).expect("stock")
    };
    let a = state(&[0, 1, 2, 3]);
    let b = state(&[3, 2, 0, 1]);
    assert_eq!(a, b);
    assert_eq!(a.asset_value, Some(m("53.29")));
}

#[test]
fn closed_periods_no_stock_and_stale_counts_are_refused() {
    let w = item("W");
    let mut inv = DemoInventory::default();
    let line = |kind| {
        [DocLine {
            item: w,
            kind,
            fallback_cost: None,
            return_of: None,
        }]
    };
    // A value-only change with nothing on hand (Q-P24s).
    assert_eq!(
        inv.post(
            "V1",
            d("2026-01-02"),
            &line(MoveKind::AdjustValue { change: m("1.50") })
        ),
        Err(DemoInventoryError::NoStock)
    );
    assert!(inv.rows().expect("rows").is_empty(), "nothing changed");
    inv.post(
        "A1",
        d("2026-01-02"),
        &line(MoveKind::Receipt {
            quantity: q("10"),
            amount: m("33.33"),
        }),
    )
    .expect("bill");
    // Owner 2026-11-06: closed periods are refused for inventory, even with
    // the password; nothing is restated.
    inv.closing_date = Some(d("2026-01-10"));
    let closed = inv.post(
        "B1",
        d("2026-01-03"),
        &line(MoveKind::Receipt {
            quantity: q("6"),
            amount: m("12.00"),
        }),
    );
    assert_eq!(closed, Err(DemoInventoryError::PeriodClosed));
    assert_eq!(
        DemoInventoryError::PeriodClosed.code(),
        "INVENTORY_PERIOD_CLOSED"
    );
    assert_eq!(inv.remove("A1"), Err(DemoInventoryError::PeriodClosed));
    assert_eq!(
        inv.edit(
            "A1",
            d("2026-01-20"),
            &line(MoveKind::Receipt {
                quantity: q("1"),
                amount: m("1.00")
            })
        ),
        Err(DemoInventoryError::PeriodClosed),
        "moving a closed document out of the period is refused too"
    );
    // A count whose sheet is out of date.
    let stale = inv.post_count(
        "F1",
        d("2026-01-31"),
        &[cl_api::CountLine {
            item_id: w,
            sheet_on_hand: q("9"),
            counted_quantity: q("8"),
            unit_id: None,
        }],
    );
    assert_eq!(stale, Err(DemoInventoryError::Stale(vec![w])));
    let (lines, _) = inv
        .post_count(
            "F2",
            d("2026-01-31"),
            &[cl_api::CountLine {
                item_id: w,
                sheet_on_hand: q("10"),
                counted_quantity: q("10"),
                unit_id: None,
            }],
        )
        .expect("a matching count");
    assert!(lines.is_empty(), "no adjustment when every count matches");
    assert_eq!(inv.remove("nope"), Err(DemoInventoryError::NotFound));
}

/// Q-P24d: with nothing on hand and no average yet, a sale is costed at
/// the purchase cost snapshot.
#[test]
fn a_first_sale_uses_the_purchase_cost_snapshot() {
    let w = item("W");
    let mut inv = DemoInventory::default();
    let warn = inv
        .post(
            "S1",
            d("2026-01-02"),
            &[DocLine {
                item: w,
                kind: MoveKind::Sale { quantity: q("2") },
                fallback_cost: Some(UnitCost::parse_wire("2.505").expect("cost")),
                return_of: None,
            }],
        )
        .expect("sale");
    assert_eq!(warn.len(), 1);
    let r = &inv.rows().expect("rows")[0];
    assert_eq!(r.value, m("-5.01"));
    assert_eq!(r.on_hand, q("-2"));
}
