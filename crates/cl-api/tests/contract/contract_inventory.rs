//! Inventory at average cost against the live engine (P24 plan §6.1, §6.2):
//! the nine capabilities, the shared golden file byte-identical with the
//! engine's, cases A (reference sequence incl. negative stock then a
//! receipt), R (linked returns) and U (units of measure) replayed from that
//! file step by step, valuation = GL, the closed-period refusal, the
//! reorder report's purchase unit and the keyed unit-set create.
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
    ContactFields, ContactType, InventoryRefusal, InvoiceLine, InvoiceWriteOptions, ItemFields,
    ItemPurchase, ItemType, ItemUpdate, NewAdjustment, NewContact, NewCreditMemo, NewInvoice,
    NewUnitSet, PostingOptions, Session, Unit, UnitRefusal, inventory_refusal, unit_refusal,
};
use cl_core::ids::{AccountId, ContactId, ItemId, TransactionId, UnitId, UnitSetId};
use cl_core::{CalendarDate, Money, Quantity, UnitFactor};
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
        Err(_) => h.pending(
            "U-157",
            "no engine checkout to compare the golden file with",
        ),
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

/// What a replay knows: the golden documents posted, the parts and units.
#[derive(Default)]
struct Book {
    docs: HashMap<String, TransactionId>,
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
    let opts = PostingOptions::default();
    let mut warnings = Vec::new();
    let id = match event {
        "bill" => {
            let item = item.expect("item");
            let (unit, _) = unit_of(co, h, book, item, s);
            let amount = m(st(s, "amount").expect("amount"));
            let draft = BillDraft {
                vendor: co.vendor,
                date: date.expect("date"),
                vendor_invoice_number: None,
                terms: None,
                due_date: None,
                memo: None,
                amount_due: amount,
                lines: vec![BillLineDraft {
                    item: Some(item),
                    quantity: Some(q(st(s, "quantity").expect("qty"))),
                    unit_price: Some(m(st(s, "unitPrice").expect("price"))),
                    unit,
                    ..BillLineDraft::expense(co.asset, amount)
                }],
                received: true,
                draft_attachment: None,
            };
            Some(
                h.run(co.s.bills().create(&draft, &opts, key()))?
                    .record
                    .txn
                    .id,
            )
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
        "void" => {
            let doc = book.docs[st(s, "doc").expect("doc")];
            h.run(co.s.invoices().void(
                doc,
                "golden void",
                &InvoiceWriteOptions::default(),
                key(),
            ))?;
            None
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

/// Plays `id` (and the case it starts from) and checks every step's
/// expected state against the engine.
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

/// Case R: credit memos linked to the Jan 8 sale come back at that sale's
/// cost; a third link is refused; the sale can't be voided.
pub fn golden_r_linked_returns(h: &Harness) {
    if !all_caps(h) {
        return;
    }
    let co = fresh(h, "Inventory R");
    let mut book = Book::default();
    book.items.insert("W".into(), co.part(h, "W", None));
    play(&co, h, &golden(), "R", Some(10), &mut book);
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

/// Closed periods (owner 2026-11-06): with the books closed through Jan
/// 10, a part bought on Jan 3 and an adjustment on Jan 10 are refused
/// with `INVENTORY_PERIOD_CLOSED`; Jan 11 is fine. Tenant B's closing date
/// is set through the settings route (as the closed-period tests do) and
/// put back afterwards.
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
    let before = h.run(co.s.company_settings()).expect("settings").lock_date;
    let mut book = Book::default();
    book.items.insert("W".into(), co.part(h, "W", None));
    let all = golden();
    play(&co, h, &all, "A", Some(2), &mut book);
    let lock = |date: Option<CalendarDate>| {
        h.run(co.s.raw_mapped(
            cl_api::Method::Put,
            "/api/v1/company/settings",
            Some(&serde_json::json!({ "lockDate": date.map(CalendarDate::to_wire) })),
        ))
        .expect("set the closing date");
    };
    lock(Some(d("2026-01-10")));
    let bill = serde_json::json!({"event": "bill", "date": "2026-01-03", "item": "W",
        "quantity": "6", "unitPrice": "2.00", "amount": "12.00"});
    let refused_bill = step(&co, h, &mut book, &bill);
    let adjust = serde_json::json!({"event": "adjust_value", "date": "2026-01-10",
        "item": "W", "valueChange": "1.00"});
    let refused_adjust = step(&co, h, &mut book, &adjust);
    let open = serde_json::json!({"event": "adjust_value", "date": "2026-01-11",
        "item": "W", "valueChange": "1.00"});
    let after = step(&co, h, &mut book, &open);
    lock(before);
    let e = refused_bill.expect_err("closed");
    assert!(is_code(&e, "INVENTORY_PERIOD_CLOSED"), "{e}");
    let e = refused_adjust.expect_err("closed");
    assert!(is_code(&e, "INVENTORY_PERIOD_CLOSED"), "{e}");
    after.expect("after the closing date");
    let it = co.stock(h, book.items["W"]).stock;
    assert_eq!(it.asset_value, Some(m("54.83")));
}
