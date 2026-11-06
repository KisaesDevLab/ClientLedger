//! Tests of the demo sales tax engine, held to the non-engineOnly cases
//! 1-6 of `fixtures/sales_tax/golden.json` (the WP-2 part of the chain;
//! cases 7-10 and 12-14 are WP-3's).

use super::*;
use cl_api::{
    ContactFields, ContactType, InvoiceLine, Item, ItemFields, NewCreditMemo, NewInvoice,
    NewSalesReceipt,
};
use cl_core::{Money, Quantity};
use serde_json::Value;

pub(crate) fn golden() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sales_tax/golden.json");
    serde_json::from_slice(&std::fs::read(path).expect("golden file")).expect("json")
}

pub(crate) fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("decimal")
}

pub(crate) fn day(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

/// The golden seed in a fresh demo company: its items (by tax code), its
/// clients (defaults and codes); the demo's own setup is the seed's.
pub(crate) struct Seed {
    pub(crate) state: DemoState,
    pub(crate) items: BTreeMap<String, cl_core::ids::ItemId>,
    pub(crate) clients: BTreeMap<String, ContactId>,
}

pub(crate) fn seed(g: &Value) -> Seed {
    let mut state = DemoState::new();
    let _ = agencies(&mut state, true);
    let by_code = |st: &DemoState, c: &str| {
        st.sales_tax
            .codes
            .iter()
            .find(|x| x.code == c)
            .map(|x| x.id)
    };
    let by_tax = |st: &DemoState, n: &str| {
        st.sales_tax
            .items
            .iter()
            .find(|i| i.name == n)
            .map(|i| i.id)
    };
    let s = &g["seeds"]["monett"];
    let mut items = BTreeMap::new();
    for (k, it) in s["items"].as_array().expect("items").iter().enumerate() {
        let name = it["name"].as_str().expect("name");
        let id = super::super::lists::item_id(900 + u32::try_from(k).expect("k"));
        let code = by_code(&state, it["taxCode"].as_str().expect("code"));
        state.items_mut().push(Item {
            stock: cl_api::ItemStock::default(),
            id,
            fields: ItemFields {
                purchase: cl_api::ItemPurchase::default(),
                name: name.into(),
                description: None,
                unit_price: None,
                income_account_id: super::super::account_id(4000),
                is_taxable: false,
                item_type: cl_api::ItemType::Service,
                discount_percent: None,
                tax_code_id: code,
            },
            is_active: true,
            default_tag_id: None,
            created_at: None,
            updated_at: None,
        });
        items.insert(name.to_string(), id);
    }
    let mut clients = BTreeMap::new();
    for c in s["clients"].as_array().expect("clients") {
        let name = c["name"].as_str().expect("name");
        let mut f = ContactFields::named(ContactType::Customer, name);
        f.default_sales_tax_id = c["salesTax"].as_str().and_then(|n| by_tax(&state, n));
        f.tax_code_id = c["taxCode"].as_str().and_then(|n| by_code(&state, n));
        let contacts = state.contacts_mut();
        let id =
            super::super::contacts::contact_id(800 + u32::try_from(contacts.len()).expect("n"));
        contacts.push(cl_api::Contact {
            id,
            fields: f,
            is_active: true,
            opening_balance: None,
            opening_balance_date: None,
            created_at: None,
            updated_at: None,
        });
        clients.insert(name.to_string(), id);
    }
    Seed {
        state,
        items,
        clients,
    }
}

pub(crate) fn lines(s: &Seed, req: &Value) -> Vec<InvoiceLine> {
    req["lines"]
        .as_array()
        .expect("lines")
        .iter()
        .map(|l| {
            let mut x = InvoiceLine::item(
                s.items[l["item"].as_str().expect("item")],
                Quantity::parse_wire(l["quantity"].as_str().expect("q")).expect("q"),
            );
            x.unit_price = Some(Money::parse_wire(l["unitPrice"].as_str().expect("p")).expect("p"));
            x
        })
        .collect()
}

pub(crate) fn same(got: Decimal, want: &Value, what: &str) {
    if let Some(w) = want.as_str() {
        assert_eq!(got, d(w), "{what}");
    }
}

fn check_tax(got: Option<&cl_api::DocSalesTax>, want: &Value, what: &str) {
    if want.is_null() {
        return;
    }
    let got = got.unwrap_or_else(|| panic!("{what}: no salesTax"));
    assert_eq!(got.name, want["name"].as_str().expect("name"), "{what}");
    assert_eq!(
        got.rate.to_wire6(),
        want["rate"].as_str().expect("rate"),
        "{what}"
    );
    for (m, w) in got
        .members
        .iter()
        .zip(want["members"].as_array().expect("members"))
    {
        assert_eq!(m.name, w["name"].as_str().expect("name"), "{what}");
        assert_eq!(
            m.rate.to_wire6(),
            w["rate"].as_str().expect("rate"),
            "{what}"
        );
        same(m.tax.expect("tax").as_decimal(), &w["tax"], what);
    }
    assert_eq!(
        got.members.len(),
        want["members"].as_array().map_or(0, Vec::len)
    );
}

fn check_preview(p: &cl_api::InvoicePreview, want: &Value, what: &str) {
    check_tax(p.sales_tax.as_ref(), &want["salesTax"], what);
    same(p.tax_total.as_decimal(), &want["taxTotal"], what);
    same(p.total.as_decimal(), &want["total"], what);
    same(p.subtotal.as_decimal(), &want["subtotal"], what);
    if let Some(b) = p.taxable_base {
        same(b.as_decimal(), &want["taxableBase"], what);
    }
    if let Some(ls) = want["lines"].as_array() {
        for (l, w) in p.lines.iter().zip(ls) {
            same(l.amount.as_decimal(), &w["amount"], what);
            same(
                l.tax_amount.expect("tax").as_decimal(),
                &w["taxAmount"],
                what,
            );
            assert_eq!(
                l.tax_code.as_ref().map(|c| c.code.as_str()),
                w["taxCode"].as_str(),
                "{what}"
            );
        }
    }
}

fn liability(s: &Seed, req: &Value, want: &Value) {
    let (cols, unassigned) = liability_accrual(
        &s.state,
        day(req["startDate"].as_str().expect("start")),
        day(req["endDate"].as_str().expect("end")),
    );
    let mut total = unassigned;
    for w in want["items"].as_array().expect("items") {
        let name = w["item"].as_str().expect("item");
        let id = s
            .state
            .sales_tax
            .items
            .iter()
            .find(|i| i.name == name)
            .map(|i| i.id)
            .expect("item");
        let c = cols.iter().find(|c| c.item == id).expect("column");
        same(c.taxable_sales, &w["taxableSales"], name);
        same(c.collected, &w["collected"], name);
        same(c.owed_at_end, &w["owedAtEnd"], name);
        for (code, amount) in w["nonTaxable"].as_object().expect("non") {
            same(c.non_taxable[code], amount, name);
        }
    }
    total += cols.iter().map(|c| c.collected).sum::<Decimal>();
    same(total, &want["total"], "total");
    same(unassigned, &want["unassigned"], "unassigned");
}

/// Runs one WP-2 step of the golden chain (cases 1-6).
pub(crate) fn run_step(s: &mut Seed, id: i64, step: &Value) {
    let req = &step["request"];
    let want = &step["expect"];
    let what = format!("case {id} {}", step["op"]);
    let client = req["client"].as_str().map(|c| s.clients[c]);
    let date = req["date"].as_str().map(day);
    match step["op"].as_str().expect("op") {
        "invoice" => {
            let new = NewInvoice::new(client.expect("c"), date.expect("d"), lines(s, req));
            let p = super::super::invoices::preview(&mut s.state, &new, None).expect("preview");
            check_preview(&p, want, &what);
            let saved = super::super::invoices::create(&mut s.state, &new, None).expect("posted");
            check_tax(saved.record.sales_tax.as_ref(), &want["salesTax"], &what);
        }
        "creditMemo" => {
            let m = NewCreditMemo::new(client.expect("c"), date.expect("d"), lines(s, req));
            let r = super::super::credit_memos::create_memo(&mut s.state, &m, None).expect("memo");
            check_tax(r.sales_tax.as_ref(), &want["salesTax"], &what);
            same(r.tax_amount.as_decimal(), &want["taxTotal"], &what);
            same(r.total.as_decimal(), &want["total"], &what);
        }
        "salesReceipt" => {
            let mut r = NewSalesReceipt::new(date.expect("d"), lines(s, req));
            r.contact_id = client;
            let rec = super::super::credit_memos::save_receipt(&mut s.state, &r, None, None)
                .expect("receipt");
            check_tax(rec.sales_tax.as_ref(), &want["salesTax"], &what);
            same(rec.tax_amount.as_decimal(), &want["taxTotal"], &what);
            same(rec.total.as_decimal(), &want["total"], &what);
        }
        "dailySalesPreview" | "dailySales" => {
            let item = s
                .state
                .sales_tax
                .items
                .iter()
                .find(|i| Some(i.name.as_str()) == req["salesTax"].as_str())
                .map(|i| i.id)
                .expect("item");
            let sales = d(req["sales"].as_str().expect("sales"));
            let typed = d(req["tax"].as_str().expect("tax"));
            let expected = expected_daily_tax(&s.state, item, sales);
            same(expected, &want["expectedTax"], &what);
            let differs = expected != typed;
            assert_eq!(
                differs,
                want["problems"]
                    .as_array()
                    .is_some_and(|p| p.iter().any(|x| x == "TAX_DIFFERS")),
                "{what}"
            );
            if step["op"] == "dailySales" {
                let txn = TransactionId::parse("d1900000-0000-4000-8000-000000000001").expect("id");
                post_daily(
                    &mut s.state,
                    txn,
                    date.expect("d"),
                    Some(item),
                    sales,
                    typed,
                );
            }
        }
        "liability" => liability(s, req, want),
        other => panic!("unexpected op {other} in case {id}"),
    }
}

#[test]
fn the_demo_engine_runs_golden_cases_1_to_6() {
    let g = golden();
    let mut s = seed(&g);
    let mut ran = 0;
    for id in 1..=6 {
        let case = g["cases"]
            .as_array()
            .expect("cases")
            .iter()
            .find(|c| c["id"] == id)
            .expect("case");
        for step in case["steps"].as_array().expect("steps") {
            run_step(&mut s, id, step);
            ran += 1;
        }
    }
    assert_eq!(ran, 10, "every step of cases 1-6 ran");
}

#[test]
fn a_voided_document_drops_its_rows_and_owed_follows() {
    let g = golden();
    let mut s = seed(&g);
    let new = NewInvoice::new(
        s.clients["Ozark Outfitters"],
        day("2026-11-02"),
        lines(
            &s,
            &json!({"lines": [{"item": "Plants", "quantity": "1", "unitPrice": "1000.00"}]}),
        ),
    );
    let saved = super::super::invoices::create(&mut s.state, &new, None).expect("posted");
    let o = owed(&mut s.state, day("2026-11-30"));
    assert_eq!(o["total"], "88.75");
    super::super::invoices::void(&mut s.state, saved.record.txn.id, None).expect("void");
    assert_eq!(owed(&mut s.state, day("2026-11-30"))["total"], "0.00");
}

#[test]
fn setup_refuses_duplicates_system_codes_and_deactivating_used_members() {
    let mut st = DemoState::new();
    let all = items(&mut st, true);
    assert_eq!(all.len(), 4);
    let monett = all.iter().find(|i| i.name == "Monett").expect("group");
    assert_eq!(monett.rate.to_wire6(), "0.081000");
    assert_eq!(monett.members.len(), 2);
    assert_eq!(agencies(&mut st, false).len(), 2);
    let code = |e: &ApiError| cl_api::sales_tax_refusal(e);
    // A member of an active group can't be made inactive.
    let e = set_item_active(&mut st, item_id(STATE), false).expect_err("in group");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::ItemInGroup));
    // Names are unique, case-insensitively.
    let draft = SalesTaxItemDraft {
        kind: SalesTaxKind::Item,
        name: "springfield".into(),
        description: None,
        rate: TaxRate::parse_percent("7").ok(),
        agency_id: Some(agency_id(1)),
        member_ids: Vec::new(),
    };
    let e = save_item(&mut st, None, &draft, None).expect_err("taken");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::NameInUse));
    let made = save_item(
        &mut st,
        None,
        &SalesTaxItemDraft {
            name: "Branson".into(),
            ..draft
        },
        None,
    )
    .expect("made");
    assert_eq!(made.rate.to_wire6(), "0.070000");
    // A stale edit is refused.
    let stale = cl_core::Timestamp::parse_wire("2020-01-01T00:00:00.000Z").ok();
    let e = save_item(
        &mut st,
        Some(made.id),
        &SalesTaxItemDraft {
            kind: SalesTaxKind::Item,
            name: "Branson".into(),
            description: None,
            rate: TaxRate::parse_percent("7.5").ok(),
            agency_id: Some(agency_id(1)),
            member_ids: Vec::new(),
        },
        stale,
    )
    .expect_err("stale");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::Stale));
    // Tax and Non are built in.
    let e = deactivate_code(&mut st, code_id(TAX)).expect_err("system");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::SystemCode));
    let e = save_code(
        &mut st,
        None,
        &TaxCodeDraft {
            code: "res".into(),
            name: "x".into(),
            taxable: false,
        },
    )
    .expect_err("taken");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::CodeInUse));
    // Deactivated, never deleted: still listed with inactive ones.
    deactivate_code(&mut st, code_id(RES)).expect("deactivated");
    assert_eq!(codes(&mut st, false).len(), 2);
    assert_eq!(codes(&mut st, true).len(), 3);
    // An agency with active taxes stays.
    let e = set_agency_active(&mut st, agency_id(2), false).expect_err("in use");
    assert_eq!(code(&e), Some(cl_api::SalesTaxRefusal::AgencyInUse));
}

#[test]
fn tax_off_charges_nothing_and_refuses_a_requested_tax() {
    let mut st = DemoState::new();
    let client = super::super::contacts::contact_id(1);
    assert!(resolve(&mut st, None, client).expect("ok").is_some());
    save_settings(
        &mut st,
        &cl_api::SalesTaxSettingsUpdate {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .expect("saved");
    assert!(resolve(&mut st, None, client).expect("ok").is_none());
    let e = resolve(&mut st, Some(item_id(MONETT)), client).expect_err("off");
    assert_eq!(
        cl_api::sales_tax_refusal(&e),
        Some(cl_api::SalesTaxRefusal::SalesTaxOff)
    );
    assert_eq!(settings_json(&st)["salesTaxEnabled"], false);
}
