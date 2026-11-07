//! Inventory at average cost against the live engine (P24 plan §6.1, §6.2):
//! the nine capabilities, the shared golden file byte-identical with the
//! engine's, every case not marked `engineOnly` replayed from that file
//! step by step (A with valuation = GL, U with units of measure, the rest
//! with their movement rows), the closed-period refusals with and without
//! the closing-date password, the reorder report's purchase unit and the
//! keyed unit-set create.
//!
//! The cases run in tenant B (S3; a new company would spend the run's
//! rate-limited auth budget), each on its own part, so their histories
//! never mix. Tenant B is closed through January 2026 by the closed-period
//! tests, so every golden date moves one year later (2027): costing depends
//! on the order of the rows, not on the year.

use std::collections::HashMap;
use std::path::PathBuf;

use cl_api::{
    AccountFilter, AccountType, AdjustmentLine, ApiError, BillDraft, BillLineDraft, Capabilities,
    ClosedPeriodAuthorization, ContactFields, ContactType, CountLine, InventoryRefusal,
    InvoiceLine, ItemFields, ItemPurchase, ItemType, ItemUpdate, NewAdjustment, NewContact,
    NewCount, NewCreditMemo, NewInvoice, NewUnitSet, PostingOptions, Session, Unit, UnitRefusal,
    inventory_refusal, unit_refusal,
};
use cl_core::ids::{AccountId, ContactId, ItemId, TransactionId, UnitId, UnitSetId};
use cl_core::{CalendarDate, Money, Quantity, Secret, UnitFactor};
use serde_json::Value;

use crate::common::{account, key, m};
use crate::support::{Harness, unique};

const GOLDEN: &str = include_str!("../../../../fixtures/inventory/golden.json");

/// The capabilities P24 needs (plan §2.4), all advertised by the engine.
const CAPS: [&str; 8] = [
    Capabilities::ITEM_TYPES,
    Capabilities::INVENTORY_ACCOUNT,
    Capabilities::COMPANY_INVENTORY_FLAG,
    Capabilities::INVENTORY_COSTING,
    Capabilities::INVENTORY_ADJUST,
    Capabilities::INVENTORY_REPORTS,
    Capabilities::REORDER_ALERTS,
    Capabilities::UNITS_OF_MEASURE,
];

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

/// A golden date, one year later (see the module docs).
fn d(s: &str) -> CalendarDate {
    let g = CalendarDate::parse_wire(s).expect("date");
    CalendarDate::from_ymd(g.year() + 1, g.month(), g.day()).expect("date a year later")
}

fn st<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

fn all_caps(h: &Harness) -> bool {
    CAPS.iter().all(|c| h.require_cap(c, "U-157..U-161"))
}

/// The engine advertises every inventory capability, and the golden file
/// is byte-identical with the engine checkout's.
pub fn capabilities_and_golden_bytes(h: &Harness) {
    let caps = h.s1.client().capabilities();
    for c in CAPS {
        assert!(caps.has(c), "{c} advertised");
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let engine = root.join("engine/packages/shared/fixtures/inventory-golden.json");
    match std::fs::read(&engine) {
        // Byte-identical as committed: a checkout may turn LF into CRLF.
        Ok(bytes) => assert!(
            String::from_utf8_lossy(&bytes).replace("\r\n", "\n") == GOLDEN.replace("\r\n", "\n"),
            "fixtures/inventory/golden.json differs from {}",
            engine.display()
        ),
        // CI runs the contracts with the submodule checked out
        // (CL_API_REQUIRE=1): there a missing file fails (review D4).
        Err(e) => {
            assert!(
                std::env::var("CL_API_REQUIRE").as_deref() != Ok("1"),
                "CL_API_REQUIRE=1 but {} is unreadable ({:?}): the golden file is not compared",
                engine.display(),
                e.kind()
            );
            h.pending(
                "U-157",
                "no engine checkout to compare the golden file with",
            );
        }
    }
}

/// A fresh company with inventory on, opened on S3.
struct Co {
    s: Session,
    asset: AccountId,
    cogs: AccountId,
    income: AccountId,
    adjustment: AccountId,
    vendor: ContactId,
    customer: ContactId,
}

fn fresh(h: &Harness, _name: &str) -> Co {
    let s = h.s3();
    h.run(s.set_inventory_enabled(true, key()))
        .unwrap_or_else(|e| panic!("inventory on: {e}"));
    let accounts = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    let asset = accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("inventory_asset"))
        .expect("U-156 adopts or creates Inventory Asset")
        .id;
    let cogs = accounts
        .iter()
        .find(|a| a.is_active && a.fields.account_type == AccountType::Cogs)
        .expect("U-156 ensures a COGS account")
        .id;
    let income = account(h, &s, AccountType::Revenue, "Sales").id;
    let adjustment = account(h, &s, AccountType::Expense, "Inventory Shrinkage").id;
    let contact = |t: ContactType, n: &str| {
        h.run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(t, unique(n)),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .unwrap_or_else(|e| panic!("contact: {e}"))
        .record
        .id
    };
    let vendor = contact(ContactType::Vendor, "Prairie Mill Supply");
    let customer = contact(ContactType::Customer, "Bluestem Veterinary Clinic");
    Co {
        s,
        asset,
        cogs,
        income,
        adjustment,
        vendor,
        customer,
    }
}

impl Co {
    fn part(&self, h: &Harness, name: &str, set: Option<UnitSetId>) -> ItemId {
        h.run(self.s.items().create(
            &ItemFields {
                purchase: ItemPurchase {
                    asset_account_id: Some(self.asset),
                    cogs_account_id: Some(self.cogs),
                    unit_set_id: set,
                    ..ItemPurchase::default()
                },
                name: unique(name),
                description: None,
                unit_price: Some(m("10.00")),
                income_account_id: self.income,
                is_taxable: false,
                item_type: ItemType::InventoryPart,
                discount_percent: None,
                tax_code_id: None,
            },
            key(),
        ))
        .unwrap_or_else(|e| panic!("part: {e}"))
        .record
        .id
    }

    fn stock(&self, h: &Harness, item: ItemId) -> cl_api::Item {
        h.run(self.s.items().get(item)).expect("item").record
    }
}

/// What a replay knows: the golden documents posted (and the drafts an
/// edit starts from), the parts and units.
#[derive(Default)]
struct Book {
    docs: HashMap<String, TransactionId>,
    invoices: HashMap<String, NewInvoice>,
    bills: HashMap<String, BillDraft>,
    items: HashMap<String, ItemId>,
    units: HashMap<String, UnitId>,
    set: Option<UnitSetId>,
}

/// Whether `e` is the engine refusal `code` the golden expects.
fn is_code(e: &ApiError, code: &str) -> bool {
    let unit = unit_refusal(e);
    let inv = inventory_refusal(e);
    match code {
        "UNIT_NOT_IN_SET" => unit == Some(UnitRefusal::NotInSet),
        "UNIT_QUANTITY_TOO_SMALL" => unit == Some(UnitRefusal::QuantityTooSmall),
        "UNIT_IN_USE" => unit == Some(UnitRefusal::InUse),
        "RETURN_EXCEEDS_SALE" => inv == Some(InventoryRefusal::ReturnExceedsSale),
        "RETURN_BEFORE_SALE" => inv == Some(InventoryRefusal::ReturnBeforeSale),
        "SALE_HAS_RETURNS" => inv == Some(InventoryRefusal::SaleHasReturns),
        "INVENTORY_PERIOD_CLOSED" => inv == Some(InventoryRefusal::PeriodClosed),
        _ => false,
    }
}

fn case<'a>(all: &'a [Value], id: &str) -> &'a Value {
    all.iter()
        .find(|c| st(c, "id") == Some(id))
        .unwrap_or_else(|| panic!("golden case {id}"))
}

/// The line's unit and, on a sale in a unit, the engine's price in it.
fn unit_of(
    co: &Co,
    h: &Harness,
    book: &Book,
    item: ItemId,
    step: &Value,
) -> (Option<UnitId>, Option<Money>) {
    let Some(name) = st(step, "unit") else {
        return (None, None);
    };
    let unit = book.units.get(name).copied();
    let price = unit.and_then(|u| {
        co.stock(h, item)
            .stock
            .unit_prices
            .into_iter()
            .find(|p| p.unit_id == Some(u))
            .and_then(|p| p.unit_price)
    });
    (unit, price)
}

/// The step's closing-date password (case C's refusals carry one).
fn posting_opts(s: &Value) -> PostingOptions {
    PostingOptions {
        closed_period: st(s, "closingPassword")
            .map(|p| ClosedPeriodAuthorization::with_password(Secret::new(p))),
    }
}

/// A golden bill or vendor credit line of a part.
fn bill_line(co: &Co, item: ItemId, unit: Option<UnitId>, s: &Value) -> BillLineDraft {
    let amount = m(st(s, "amount").expect("amount"));
    BillLineDraft {
        item: Some(item),
        quantity: Some(q(st(s, "quantity").expect("qty"))),
        unit_price: Some(m(st(s, "unitPrice").expect("price"))),
        unit,
        ..BillLineDraft::expense(co.asset, amount)
    }
}

/// Edits a golden invoice or bill (a new quantity or date); answers the
/// save's `NEGATIVE_STOCK` warnings.
fn edit(
    co: &Co,
    h: &Harness,
    book: &mut Book,
    s: &Value,
    opts: &PostingOptions,
) -> Result<Vec<cl_api::NegativeStock>, ApiError> {
    let name = st(s, "doc").expect("doc");
    let doc = book.docs[name];
    let date = st(s, "date").map(d);
    if let Some(mut new) = book.invoices.get(name).cloned() {
        if let Some(n) = st(s, "quantity") {
            new.lines[0].quantity = Some(q(n));
        }
        if let Some(t) = date {
            new.txn_date = t;
        }
        let cur = h.run(co.s.invoices().get(doc))?;
        let saved = h.run(co.s.invoices().update(doc, &cur.record, &new, opts, key()))?;
        book.invoices.insert(name.to_string(), new);
        return Ok(saved.stock_warnings);
    }
    let mut draft = book.bills.get(name).cloned().expect("an edited bill");
    assert!(st(s, "quantity").is_none(), "golden bill edits move dates");
    if let Some(t) = date {
        draft.date = t;
    }
    let cur = h.run(co.s.bills().get(doc))?;
    let saved = h.run(co.s.bills().update(doc, &cur, &draft, opts, key()))?;
    book.bills.insert(name.to_string(), draft);
    Ok(saved.stock_warnings)
}

/// Case F's count; answers the adjustment it posted and its warnings.
fn count(
    co: &Co,
    h: &Harness,
    book: &Book,
    s: &Value,
    opts: &PostingOptions,
) -> Result<cl_api::CountResult, ApiError> {
    let lines: Vec<CountLine> = s["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| CountLine {
            item_id: book.items[st(l, "item").expect("item")],
            sheet_on_hand: q(st(l, "sheetOnHand").expect("sheet")),
            counted_quantity: q(st(l, "countedQuantity").expect("counted")),
            unit_id: None,
        })
        .collect();
    let c = NewCount {
        as_of: d(st(s, "date").expect("date")),
        adjustment_account_id: co.adjustment,
        memo: None,
        lines,
    };
    h.run(co.s.inventory().post_count(&c, opts, key()))
}

/// Runs one golden step; `Err` is the engine's refusal.
#[allow(clippy::too_many_lines, reason = "one arm per golden event")]
fn step(
    co: &Co,
    h: &Harness,
    book: &mut Book,
    s: &Value,
) -> Result<Vec<cl_api::NegativeStock>, ApiError> {
    let event = st(s, "event").expect("event");
    let item = st(s, "item").map(|n| book.items[n]);
    let date = st(s, "date").map(d);
    let opts = posting_opts(s);
    let mut warnings = Vec::new();
    let id = match event {
        "bill" => {
            let item = item.expect("item");
            let (unit, _) = unit_of(co, h, book, item, s);
            let draft = BillDraft {
                vendor: co.vendor,
                date: date.expect("date"),
                vendor_invoice_number: None,
                terms: None,
                due_date: None,
                memo: None,
                amount_due: m(st(s, "amount").expect("amount")),
                lines: vec![bill_line(co, item, unit, s)],
                received: true,
                draft_attachment: None,
            };
            let saved = h.run(co.s.bills().create(&draft, &opts, key()))?;
            warnings = saved.stock_warnings;
            if let Some(doc) = st(s, "doc") {
                book.bills.insert(doc.to_string(), draft);
            }
            Some(saved.record.txn.id)
        }
        "vendor_credit" => {
            let item = item.expect("item");
            let draft = cl_api::VendorCreditDraft {
                vendor: co.vendor,
                date: date.expect("date"),
                vendor_invoice_number: None,
                memo: None,
                amount: m(st(s, "amount").expect("amount")),
                lines: vec![bill_line(co, item, None, s)],
                draft_attachment: None,
            };
            let saved = h.run(co.s.vendor_credits().create(&draft, &opts, key()))?;
            warnings = saved.stock_warnings;
            Some(saved.record.txn.id)
        }
        "invoice" => {
            let item = item.expect("item");
            let (unit, price) = unit_of(co, h, book, item, s);
            let line = InvoiceLine {
                unit_id: unit,
                unit_price: price,
                ..InvoiceLine::item(item, q(st(s, "quantity").expect("qty")))
            };
            let new = NewInvoice::new(co.customer, date.expect("date"), vec![line]);
            let saved = h.run(co.s.invoices().create(&new, &opts, key()))?;
            warnings = saved.stock_warnings;
            if let Some(doc) = st(s, "doc") {
                book.invoices.insert(doc.to_string(), new);
            }
            Some(saved.invoice.record.txn.id)
        }
        "credit_memo" => {
            let sale = st(s, "returnOf").map(|doc| book.docs[doc]);
            let line = InvoiceLine {
                return_of: Some(sale),
                ..InvoiceLine::item(item.expect("item"), q(st(s, "quantity").expect("qty")))
            };
            let new = NewCreditMemo::new(co.customer, date.expect("date"), vec![line]);
            Some(h.run(co.s.credit_memos().create(&new, &opts, key()))?.id)
        }
        "adjust_qty" | "adjust_value" => {
            let a = NewAdjustment {
                date: date.expect("date"),
                adjustment_account_id: co.adjustment,
                memo: None,
                lines: vec![AdjustmentLine {
                    item_id: item.expect("item"),
                    quantity_change: st(s, "quantityChange").map(q),
                    unit_id: None,
                    value_change: st(s, "valueChange").map(m),
                }],
            };
            let saved = h.run(co.s.inventory().create_adjustment(&a, &opts, key()))?;
            warnings = saved.warnings;
            Some(saved.adjustment.id)
        }
        // Through the generic route, as the engine's golden test does: it
        // answers the walk's NEGATIVE_STOCK warnings for any document.
        "void" => {
            let doc = book.docs[st(s, "doc").expect("doc")];
            let v = h.run(co.s.transactions().void(doc, "golden void", &opts, key()))?;
            assert_eq!(v.record.status, cl_api::TxnStatus::Void);
            warnings = v.stock_warnings;
            None
        }
        "delete" => {
            let doc = book.docs[st(s, "doc").expect("doc")];
            let o = cl_api::DeleteOptions {
                reason: Some("golden delete".into()),
                closing_password: st(s, "closingPassword").map(Secret::new),
            };
            let v = h.run(co.s.transactions().delete(doc, &o, key()))?;
            if let Some(want) = st(&s["expect"], "status") {
                assert_eq!(want, "deleted");
                assert_eq!(v.record.status, cl_api::TxnStatus::Deleted);
            }
            warnings = v.stock_warnings;
            None
        }
        "edit" => {
            warnings = edit(co, h, book, s, &opts)?;
            None
        }
        "count" => {
            let got = count(co, h, book, s, &opts)?;
            check_count(co, h, book, &s["expect"], got.adjustment.as_ref());
            warnings = got.warnings;
            got.adjustment.map(|a| a.id)
        }
        "item_unit_prices" => {
            let w = book.items["W"];
            let got = h.run(co.s.items().get(w))?;
            let mut fields = got.record.fields.clone();
            fields.unit_price = st(s, "basePrice").map(m);
            let v = got.version;
            let upd = ItemUpdate {
                fields,
                is_active: true,
            };
            h.run(co.s.items().update(w, &v, &upd, key()))?;
            None
        }
        "unit_factor_change" => {
            let set = book.set.expect("set");
            let cur = h.run(co.s.unit_sets().get(set)).expect("set");
            let name = st(s, "unit").expect("unit");
            let factor = UnitFactor::parse_wire(st(s, "factor").expect("factor")).expect("factor");
            let n = NewUnitSet {
                name: cur.name.clone(),
                base_unit_name: cur.base_unit_name.clone(),
                base_unit_abbreviation: cur.base_unit_abbreviation.clone(),
                is_active: true,
                units: cur
                    .units
                    .iter()
                    .map(|u| Unit {
                        factor: if u.name == name { factor } else { u.factor },
                        ..u.clone()
                    })
                    .collect(),
            };
            let at = cur.updated_at.expect("a unit set reads its updatedAt");
            h.run(co.s.unit_sets().update(set, &n, at))?;
            None
        }
        other => panic!("golden event {other} has no runner"),
    };
    if let (Some(doc), Some(id)) = (st(s, "doc"), id) {
        book.docs.insert(doc.to_string(), id);
    }
    Ok(warnings)
}

/// Case F: the count's one adjustment has a line per differing part with
/// the golden quantity change and value; matching parts have none.
fn check_count(
    co: &Co,
    h: &Harness,
    book: &Book,
    expect: &Value,
    adj: Option<&cl_api::AdjustmentRecord>,
) {
    let lines = adj.map_or(&[][..], |a| a.lines.as_slice());
    for e in expect["lines"].as_array().map_or(&[][..], Vec::as_slice) {
        let name = st(e, "item").expect("item");
        let id = book.items[name];
        let l = lines
            .iter()
            .find(|l| l.item_id == id)
            .unwrap_or_else(|| panic!("count line for {name}"));
        assert_eq!(
            l.quantity_change.map(Quantity::to_wire).as_deref(),
            st(e, "quantityChange"),
            "count {name} quantity change"
        );
        assert_eq!(l.value, st(e, "value").map(m), "count {name} value");
        let it = co.stock(h, id).stock;
        assert_eq!(
            it.quantity_on_hand.map(Quantity::to_wire).as_deref(),
            st(e, "onHand"),
            "count {name} on hand"
        );
        assert_eq!(
            it.asset_value,
            st(e, "assetValue").map(m),
            "count {name} asset value"
        );
    }
    for name in expect["noLine"].as_array().map_or(&[][..], Vec::as_slice) {
        let id = book.items[name.as_str().expect("name")];
        assert!(
            lines.iter().all(|l| l.item_id != id),
            "count: no line for {name}"
        );
    }
}

/// The part's movement rows (valuation detail over the shifted year) in
/// position order match the golden `rows`: document order, value with its
/// correction, running on hand and value. `subset`: only the rows of the
/// documents listed (case R).
fn check_rows(co: &Co, h: &Harness, book: &Book, want: &Value, ctx: &str, subset: bool) {
    let item = book.items["W"];
    let name: HashMap<TransactionId, &str> =
        book.docs.iter().map(|(k, v)| (*v, k.as_str())).collect();
    let want = want.as_array().expect("rows");
    let listed: Vec<&str> = want.iter().filter_map(|r| st(r, "doc")).collect();
    let got: Vec<_> = h
        .run(
            co.s.inventory()
                .valuation_detail(d("2026-01-01"), d("2026-12-31"), Some(item)),
        )
        .expect("valuation detail")
        .into_iter()
        .filter(|r| r.item_id == item)
        .filter(|r| {
            !subset
                || name
                    .get(&r.transaction_id)
                    .is_some_and(|n| listed.contains(n))
        })
        .collect();
    let docs: Vec<&str> = got
        .iter()
        .map(|r| name.get(&r.transaction_id).copied().unwrap_or("?"))
        .collect();
    assert_eq!(docs, listed, "{ctx} row order");
    for (r, e) in got.iter().zip(want) {
        let doc = st(e, "doc").unwrap_or("?");
        // The report's value is the movement's value with its correction
        // (case S: A6's 38.00 and -0.63 read 37.37).
        let mut value = m(st(e, "value").expect("value"));
        if let Some(c) = st(e, "correction") {
            value = value.checked_add(m(c)).expect("sum");
        }
        assert_eq!(r.value, value, "{ctx} {doc} value");
        if let Some(n) = st(e, "onHand") {
            assert_eq!(r.running_quantity.to_wire(), n, "{ctx} {doc} on hand");
        }
        if let Some(v) = st(e, "assetValue") {
            assert_eq!(r.running_value, m(v), "{ctx} {doc} asset value");
        }
    }
}

/// The case's end state: `final` per part.
fn check_final(co: &Co, h: &Harness, book: &Book, c: &Value, ctx: &str) {
    let Some(f) = c.get("final").and_then(Value::as_object) else {
        return;
    };
    for (name, want) in f {
        let it = co.stock(h, book.items[name]).stock;
        if let Some(n) = st(want, "onHand") {
            assert_eq!(
                it.quantity_on_hand.map(Quantity::to_wire).as_deref(),
                Some(n),
                "{ctx} final {name} on hand"
            );
        }
        if let Some(v) = st(want, "assetValue") {
            assert_eq!(it.asset_value, Some(m(v)), "{ctx} final {name} value");
        }
        if let Some(a) = st(want, "averageCost") {
            assert_eq!(
                it.average_cost.map(cl_core::UnitCost::to_wire).as_deref(),
                Some(a),
                "{ctx} final {name} average"
            );
        }
    }
}

/// Plays `id` (and the case it starts from) and checks every step's
/// expected state against the engine, and `rowsAfter` where given.
fn play(co: &Co, h: &Harness, all: &[Value], id: &str, limit: Option<u64>, book: &mut Book) {
    let c = case(all, id);
    if let Some(from) = st(c, "from") {
        play(co, h, all, from, c["fromSteps"].as_u64(), book);
    }
    for s in c["steps"].as_array().map_or(&[][..], Vec::as_slice) {
        let n = s["n"].as_u64().expect("n");
        if limit.is_some_and(|l| n > l) {
            break;
        }
        let ctx = format!("case {id} step {n}");
        let expect = &s["expect"];
        let got = step(co, h, book, s);
        if let Some(code) = st(expect, "code") {
            match got {
                Err(e) => assert!(is_code(&e, code), "{ctx}: want {code}, got {e}"),
                Ok(_) => panic!("{ctx}: want {code}, the engine accepted it"),
            }
            continue;
        }
        let warnings = got.unwrap_or_else(|e| panic!("{ctx}: {e} {e:?}"));
        if let Some(after) = c.get("rowsAfter").and_then(|r| r.get(n.to_string())) {
            check_rows(co, h, book, after, &ctx, false);
        }
        // A step without `expect` states nothing about warnings (case M
        // step 10 sends A4 negative without saying so).
        if !expect.is_object() {
            continue;
        }
        let want_warn = expect["warnings"].as_array().map_or(0, Vec::len);
        assert_eq!(warnings.len(), want_warn, "{ctx} NEGATIVE_STOCK warnings");
        if let Some(prices) = expect
            .as_object()
            .filter(|_| st(s, "event") == Some("item_unit_prices"))
        {
            let it = co.stock(h, book.items["W"]);
            for (unit, want) in prices {
                let p = it
                    .stock
                    .unit_prices
                    .iter()
                    .find(|p| &p.name == unit)
                    .and_then(|p| p.unit_price)
                    .map(Money::to_wire);
                assert_eq!(p.as_deref(), want.as_str(), "{ctx} {unit} price");
            }
            continue;
        }
        let Some(item) = st(s, "item").map(|n| book.items[n]) else {
            continue;
        };
        let it = co.stock(h, item).stock;
        if let Some(want) = st(expect, "onHand") {
            assert_eq!(
                it.quantity_on_hand.map(Quantity::to_wire).as_deref(),
                Some(want),
                "{ctx} on hand"
            );
        }
        if let Some(want) = st(expect, "assetValue") {
            assert_eq!(it.asset_value, Some(m(want)), "{ctx} value");
        }
        if let Some(want) = st(expect, "averageCost") {
            assert_eq!(
                it.average_cost.map(cl_core::UnitCost::to_wire).as_deref(),
                Some(want),
                "{ctx} average"
            );
        }
    }
}

fn golden() -> Vec<Value> {
    let v: Value = serde_json::from_str(GOLDEN).expect("golden json");
    v["cases"].as_array().expect("cases").clone()
}

/// Case A: the reference sequence, negative stock then a receipt with its
/// correction; afterwards the valuation as of Jan 31 equals the GL.
pub fn golden_a_and_valuation_equals_gl(h: &Harness) {
    if !all_caps(h) {
        return;
    }
    let co = fresh(h, "Inventory A");
    let mut book = Book::default();
    book.items.insert("W".into(), co.part(h, "W", None));
    let all = golden();
    play(&co, h, &all, "A", None, &mut book);
    let v = h
        .run(co.s.inventory().valuation(d("2026-01-31")))
        .expect("valuation");
    let acct = v
        .accounts
        .iter()
        .find(|a| a.account_id == co.asset)
        .expect("the asset account's GL check");
    // The account holds every case's parts: W's row is case A's.
    let row = v
        .rows
        .iter()
        .find(|r| r.item_id == book.items["W"])
        .expect("W valued");
    assert_eq!(row.value, m("16.20"));
    assert_eq!(acct.gl_balance, acct.valuation, "valuation = GL");
    assert!(acct.difference.is_zero());
    let gl = h
        .run(co.s.accounts().get(co.asset))
        .expect("asset")
        .record
        .balance;
    assert_eq!(
        gl,
        Some(acct.gl_balance),
        "the account balance is the report's"
    );
    // Case S: the report rows as of earlier dates (U-160).
    let s = case(&all, "S");
    for (asof, want) in s["valuation"].as_object().expect("valuation") {
        let v = h
            .run(co.s.inventory().valuation(d(asof)))
            .expect("valuation");
        let row = v.rows.iter().find(|r| r.item_id == book.items["W"]);
        let value = row.map_or(Money::ZERO, |r| r.value);
        assert_eq!(Some(value), st(want, "value").map(m), "S valuation {asof}");
    }
}

/// Case U: units of measure: stock in base units, round4 base quantities,
/// unit prices from the base price, a too-small quantity and a used
/// factor refused; the unit-set create is keyed (a replay returns the same
/// set, not `DUPLICATE_NAME`); the reorder report names the purchase unit.
pub fn golden_u_units(h: &Harness) {
    if !all_caps(h) {
        return;
    }
    let co = fresh(h, "Inventory U");
    let all = golden();
    let spec = &case(&all, "U")["unitSet"];
    let n = NewUnitSet {
        name: unique(st(spec, "name").expect("name")),
        base_unit_name: st(spec, "base").expect("base").into(),
        base_unit_abbreviation: None,
        is_active: true,
        units: spec["units"]
            .as_array()
            .expect("units")
            .iter()
            .map(|u| Unit {
                id: None,
                name: st(u, "name").expect("name").into(),
                abbreviation: None,
                factor: UnitFactor::parse_wire(st(u, "factor").expect("factor")).expect("factor"),
            })
            .collect(),
    };
    let k = key();
    let set = h
        .run(co.s.unit_sets().create(&n, k.clone()))
        .expect("unit set");
    let again = h
        .run(co.s.unit_sets().create(&n, k))
        .expect("the same key replays the create");
    assert_eq!(again.id, set.id);
    let mut book = Book {
        set: Some(set.id),
        ..Book::default()
    };
    for u in &set.units {
        book.units.insert(u.name.clone(), u.id.expect("unit id"));
    }
    let w = co.part(h, "W", Some(set.id));
    book.items.insert("W".into(), w);
    play(&co, h, &all, "U", None, &mut book);

    // The count sheet counts a part with a set in its base unit.
    let sheet = h
        .run(co.s.inventory().count_sheet(d("2026-01-31")))
        .expect("count sheet");
    let row = sheet
        .rows
        .iter()
        .find(|r| r.item_id == w)
        .expect("W on the sheet");
    assert_eq!(row.unit_label.as_deref(), Some("bottle"));

    // Reorder: buy by the case; 30 bottles to reorder = 2.5 cases.
    let got = h.run(co.s.items().get(w)).expect("item");
    let mut fields = got.record.fields.clone();
    fields.purchase.reorder_point = Some(q("100"));
    fields.purchase.reorder_quantity = Some(q("30"));
    fields.purchase.default_purchase_unit_id = book.units.get("case").copied();
    let v = got.version;
    h.run(co.s.items().update(
        w,
        &v,
        &ItemUpdate {
            fields,
            is_active: true,
        },
        key(),
    ))
    .expect("reorder fields");
    let rows = h.run(co.s.inventory().reorder_report()).expect("reorder");
    let r = rows.iter().find(|r| r.item_id == w).expect("W to reorder");
    assert_eq!(
        r.purchase_unit.as_ref().map(|u| u.name.as_str()),
        Some("case")
    );
    assert_eq!(
        r.reorder_quantity_in_purchase_unit
            .map(Quantity::to_wire)
            .as_deref(),
        Some("2.5000")
    );
}

/// Every case of the golden file not marked `engineOnly` (review D3), each
/// on its own part: B, V, X, M, D, N, E, R and F replay here with their
/// `rows`, `rowsAs`, `rowsAfter` and `final`; A (with S) and U have their
/// own tests above.
pub fn golden_every_case(h: &Harness) {
    if !all_caps(h) {
        return;
    }
    let co = fresh(h, "Inventory golden");
    let all = golden();
    let mut ran = Vec::new();
    for c in &all {
        let id = st(c, "id").expect("id");
        if c["engineOnly"].as_bool() == Some(true) || ["A", "S", "U"].contains(&id) {
            continue;
        }
        let mut book = Book::default();
        book.items
            .insert("W".into(), co.part(h, &format!("W {id}"), None));
        // Case F counts X and Y too, each in W's state after case A.
        if c.get("setup").is_some() {
            for extra in ["X", "Y"] {
                let part = co.part(h, &format!("{extra} {id}"), None);
                let mut other = Book::default();
                other.items.insert("W".into(), part);
                play(&co, h, &all, "A", None, &mut other);
                book.items.insert(extra.into(), part);
            }
        }
        play(&co, h, &all, id, None, &mut book);
        let ctx = format!("case {id}");
        if let Some(rows) = c.get("rows") {
            // Case R lists only the rows its returns change (as the
            // engine's golden test reads it).
            check_rows(&co, h, &book, rows, &ctx, id == "R");
        }
        if let Some(other) = st(c, "rowsAs") {
            check_rows(&co, h, &book, &case(&all, other)["rows"], &ctx, false);
        }
        check_final(&co, h, &book, c, &ctx);
        ran.push(id);
    }
    for id in ["B", "V", "X", "M", "D", "N", "E", "R", "F"] {
        assert!(ran.contains(&id), "golden case {id} replayed");
    }
}

/// Puts tenant B's closing date (and the closing password, when the test
/// set one) back when the test ends, passed or failed.
struct Relock<'a> {
    h: &'a Harness,
    s: &'a Session,
    password: Option<Secret>,
    before: Option<CalendarDate>,
}

impl Relock<'_> {
    fn lock(&self, date: Option<CalendarDate>) {
        self.h
            .run(self.s.raw_mapped(
                cl_api::Method::Put,
                "/api/v1/company/settings",
                Some(&serde_json::json!({ "lockDate": date.map(CalendarDate::to_wire) })),
            ))
            .expect("set the closing date");
    }
}

impl Drop for Relock<'_> {
    fn drop(&mut self) {
        // With a password set, moving the closing date needs it: clear it
        // first.
        if let Some(pw) = self.password.take() {
            let _ = self
                .h
                .run(self.s.set_closing_password(Some(&pw), None, key()));
        }
        let _ = self.h.run(self.s.raw_mapped(
            cl_api::Method::Put,
            "/api/v1/company/settings",
            Some(&serde_json::json!({ "lockDate": self.before.map(CalendarDate::to_wire) })),
        ));
    }
}

/// Asserts `got` is the engine's `INVENTORY_PERIOD_CLOSED`.
fn refused<T>(got: Result<T, ApiError>, what: &str) {
    match got {
        Err(e) => assert!(is_code(&e, "INVENTORY_PERIOD_CLOSED"), "{what}: {e}"),
        Ok(_) => panic!("{what}: the engine accepted it"),
    }
}

/// Closed periods (owner 2026-11-06, Q-P24b; review D2): after case A with
/// the books closed through Jan 10, every inventory write that touches the
/// closed period is refused with `INVENTORY_PERIOD_CLOSED` and writes
/// nothing: a bill and an adjustment dated in it, the Jan 15 bill and the
/// Jan 20 adjustment moved into it, an open bill once the closing date
/// moves past it; then, with a closing-date password set, case C's
/// refusals sent with the right password. The closing date moved back
/// before the Jan 8 sale lets it be edited (case M step 10's rows); Jan 11
/// is open. Tenant B's closing date and password are put back afterwards.
#[allow(clippy::too_many_lines, reason = "one scenario, step by step")]
pub fn closed_period_refuses_inventory(h: &Harness) {
    if !all_caps(h) {
        return;
    }
    let co = fresh(h, "Inventory closed");
    if h.run(co.s.closing_password_status())
        .is_ok_and(|st| st.password_set)
    {
        h.pending(
            "U-159",
            "tenant B has a closing password; the lock date is not ours to move",
        );
        return;
    }
    let mut guard = Relock {
        h,
        s: &co.s,
        password: None,
        before: h.run(co.s.company_settings()).expect("settings").lock_date,
    };
    let mut book = Book::default();
    book.items.insert("W".into(), co.part(h, "W", None));
    let w = book.items["W"];
    let all = golden();
    play(&co, h, &all, "A", None, &mut book);
    guard.lock(Some(d("2026-01-10")));
    let closed_state = co.stock(h, w).stock;

    // Dated in the closed period.
    let bill_in = serde_json::json!({"event": "bill", "date": "2026-01-03", "item": "W",
        "quantity": "6", "unitPrice": "2.00", "amount": "12.00"});
    refused(step(&co, h, &mut book, &bill_in), "a bill dated Jan 3");
    let adjust_in = serde_json::json!({"event": "adjust_value", "date": "2026-01-10",
        "item": "W", "valueChange": "1.00"});
    refused(
        step(&co, h, &mut book, &adjust_in),
        "an adjustment dated Jan 10",
    );

    // Moved into it from an open date.
    let bill_moved = serde_json::json!({"event": "edit", "doc": "A6", "date": "2026-01-09"});
    refused(
        step(&co, h, &mut book, &bill_moved),
        "the Jan 15 bill moved to Jan 9",
    );
    let a7 = book.docs["A7"];
    let rec = h.run(co.s.inventory().adjustment(a7)).expect("A7");
    let moved = NewAdjustment {
        date: d("2026-01-09"),
        adjustment_account_id: co.adjustment,
        memo: None,
        lines: vec![AdjustmentLine {
            item_id: w,
            quantity_change: Some(q("-1")),
            unit_id: None,
            value_change: None,
        }],
    };
    refused(
        h.run(co.s.inventory().update_adjustment(
            a7,
            rec.updated_at.expect("an adjustment reads its updatedAt"),
            &moved,
            &PostingOptions::default(),
            key(),
        )),
        "the Jan 20 adjustment moved to Jan 9",
    );
    assert_eq!(co.stock(h, w).stock, closed_state, "nothing written");

    // The closing date moved over open documents (case C closingMoved).
    let c = case(&all, "C");
    guard.lock(Some(d("2026-01-20")));
    let over = serde_json::json!({"event": "edit", "doc": "A6", "date": "2026-01-16"});
    refused(
        step(&co, h, &mut book, &over),
        "the Jan 15 bill once the books close through Jan 20",
    );
    guard.lock(Some(d("2026-01-05")));
    let open = &c["closingMoved"][1]["then"];
    step(&co, h, &mut book, open).expect("the Jan 8 sale is open again");
    check_rows(
        &co,
        h,
        &book,
        &case(&all, "M")["rowsAfter"]["10"],
        "closing date moved back",
        false,
    );
    guard.lock(Some(d("2026-01-10")));

    // With a closing-date password, the right password changes nothing.
    // Three of case C's refusals (dated in, moved in, an adjustment): each
    // refused request counts against the engine's closing-password limiter
    // (10 failures in 15 minutes) that the closed-period tests share.
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        let pw = st(&c["steps"][0], "password").expect("the golden password");
        assert!(
            h.run(co.s.set_closing_password(None, Some(&Secret::new(pw)), key()))
                .expect("set the closing password")
        );
        guard.password = Some(Secret::new(pw));
        let before = co.stock(h, w).stock;
        let refusals = c["refusals"].as_array().expect("refusals");
        for id in ["C", "C4", "C7"] {
            let r = refusals
                .iter()
                .find(|r| st(r, "case") == Some(id))
                .unwrap_or_else(|| panic!("refusal {id}"));
            assert!(
                st(r, "closingPassword").is_some(),
                "{id} sends the password"
            );
            refused(
                step(&co, h, &mut book, r),
                &format!("case {id} with the password"),
            );
        }
        assert_eq!(co.stock(h, w).stock, before, "nothing written");
    }

    // After the closing date: fine.
    let after = serde_json::json!({"event": "bill", "date": "2026-01-11", "item": "W",
        "quantity": "6", "unitPrice": "2.00", "amount": "12.00"});
    step(&co, h, &mut book, &after).expect("a bill dated Jan 11");
    drop(guard);
}
