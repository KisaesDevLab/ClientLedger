//! The demo engine double against the engine's golden file (U-122), the
//! preview, price levels, billable costs and the store.

use super::super::{billable, price_levels};
use super::*;
use cl_api::{InvoiceLine, SignedPercent};
use cl_core::Quantity;
use cl_core::ids::{ContactId, ItemId};

const GOLDEN: &str = include_str!("../../../../../fixtures/invoices/golden.json");

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("decimal")
}

fn s2(x: Decimal) -> String {
    format!("{:.2}", round(x, 2))
}

/// Every golden case: line amounts, rates, line taxes and totals to the
/// exact string; every error case refused.
#[test]
fn the_demo_engine_computes_every_golden_case() {
    let g: Value = serde_json::from_str(GOLDEN).expect("golden json");
    let cases = g["cases"].as_array().expect("cases");
    assert!(cases.len() >= 10);
    for c in cases {
        let name = c["name"].as_str().expect("name");
        let rate = parse_rate(c["defaultTaxRate"].as_str().expect("rate")).expect(name);
        let lines: Vec<SalesLineIn> = c["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|l| SalesLineIn {
                quantity: parse_quantity(l["quantity"].as_str().expect("q")).expect(name),
                unit_price: parse_price(l["unitPrice"].as_str().expect("p")).expect(name),
                taxable: l["isTaxable"].as_bool().expect("taxable"),
                tax_rate: l["taxRate"].as_str().map(|r| parse_rate(r).expect(name)),
            })
            .collect();
        let out = compute(&lines, rate).expect(name);
        let e = &c["expect"];
        let want = e["lines"].as_array().expect("expected lines");
        assert_eq!(out.lines.len(), want.len(), "{name}");
        for (i, (o, x)) in out.lines.iter().zip(want).enumerate() {
            assert_eq!(s2(o.amount), x["amount"], "{name} line {i} amount");
            assert_eq!(
                format!("{:.4}", o.tax_rate),
                x["taxRate"],
                "{name} line {i} rate"
            );
            assert_eq!(s2(o.tax_amount), x["taxAmount"], "{name} line {i} tax");
        }
        let line_taxes: Decimal = out.lines.iter().map(|l| l.tax_amount).sum();
        assert_eq!(
            line_taxes, out.tax_total,
            "{name}: line taxes sum to the tax"
        );
        assert_eq!(s2(out.subtotal), e["subtotal"], "{name}");
        assert_eq!(s2(out.taxable_base), e["taxableBase"], "{name}");
        assert_eq!(s2(out.tax_total), e["taxTotal"], "{name}");
        assert_eq!(s2(out.total), e["total"], "{name}");
    }
    for c in g["errors"].as_array().expect("errors") {
        let name = c["name"].as_str().expect("name");
        let refused = c["lines"].as_array().expect("lines").iter().any(|l| {
            parse_quantity(l["quantity"].as_str().unwrap_or("")).is_err()
                || parse_price(l["unitPrice"].as_str().unwrap_or("")).is_err()
                || l["taxRate"]
                    .as_str()
                    .is_some_and(|r| parse_rate(r).is_err())
        });
        assert!(refused, "{name} must be refused");
    }
}

#[test]
fn price_levels_markup_and_discounts_round_half_away_from_zero() {
    let p = SignedPercent::parse("-7.5").expect("p");
    assert_eq!(s2(price_levels::level_price(d("32.50"), p)), "30.06");
    assert_eq!(
        s2(price_levels::discount_of(d("1068.00"), d("10"))),
        "-106.80"
    );
    let f = billable::figures(d("842.15"), d("1"), Some(d("10")));
    assert_eq!(
        (s2(f.markup), s2(f.amount)),
        ("84.22".into(), "926.37".into())
    );
    // Q-P20z: a negative cost gets no markup.
    let f = billable::figures(d("-50.00"), d("1"), Some(d("10")));
    assert_eq!(
        (s2(f.markup), s2(f.amount)),
        ("0.00".into(), "-50.00".into())
    );
    // Q-P20w: one line, rate = amount ÷ quantity (shown only).
    let f = billable::figures(d("100.00"), d("3"), Some(d("10")));
    assert_eq!(f.amount, d("110.00"));
    assert_eq!(f.rate, d("36.6667"));
}

fn story(state: &mut DemoState) -> (ContactId, Vec<cl_api::Item>) {
    let contacts = state
        .contacts
        .get_or_insert_with(super::super::contacts::contacts)
        .clone();
    let client = contacts
        .iter()
        .find(|c| c.fields.display_name == super::super::bills::STORY_CUSTOMER)
        .expect("story client")
        .id;
    let items = state
        .items
        .get_or_insert_with(super::super::lists::items)
        .clone();
    (client, items)
}

fn line_for(items: &[cl_api::Item], price: &str, taxable: bool, qty: &str) -> InvoiceLine {
    let account = items[0].fields.income_account_id;
    InvoiceLine {
        is_taxable: Some(taxable),
        ..InvoiceLine::account(
            account,
            Quantity::parse_wire(qty).expect("q"),
            Money::parse_wire(price).expect("m"),
        )
    }
}

/// AC1: the mockup's lines total 3,080.15 + 154.72 = 3,234.87, and what
/// posts is what the preview said.
#[test]
fn the_mockup_invoice_previews_and_posts_3234_87() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let new = NewInvoice::new(
        client,
        DemoState::today(),
        vec![
            line_for(&items, "65.00", false, "18"),
            line_for(&items, "32.50", true, "24"),
            line_for(&items, "48.00", true, "6"),
            line_for(&items, "842.15", true, "1"),
        ],
    );
    let p = preview(&mut state, &new).expect("preview");
    assert_eq!(p.subtotal.to_wire(), "3080.1500");
    assert_eq!(p.tax_total.to_wire(), "154.7200");
    assert_eq!(p.total.to_wire(), "3234.8700");
    assert!(p.due_date.is_some());
    let saved = create(&mut state, &new, None).expect("create");
    let r = &saved.record;
    assert_eq!(
        r.txn.total.map(Money::to_wire).as_deref(),
        Some("3234.8700")
    );
    assert_eq!(
        r.tax_amount.map(Money::to_wire).as_deref(),
        Some("154.7200")
    );
    let taxes: Vec<String> = r
        .lines
        .iter()
        .map(|l| l.tax_amount.map(Money::to_wire).unwrap_or_default())
        .collect();
    assert_eq!(taxes, ["0.0000", "63.1800", "23.3300", "68.2100"]);
    assert_eq!(unsent(&state), vec![r.txn.id]);
    let sent = record_sent(
        &mut state,
        r.txn.id,
        &SendRecord {
            method: SendMethod::Printed,
            recipients: vec![],
            sent_at: None,
        },
    )
    .expect("sent");
    assert!(sent.record.was_sent());
    assert_eq!(unsent(&state), Vec::new());
    assert_eq!(
        get(&state, r.txn.id)
            .expect("get")
            .record
            .send_history
            .len(),
        1
    );
}

/// AC2: the story cost is offered, invoiced once with its markup, and back
/// after a void.
#[test]
fn a_billable_cost_is_invoiced_once_with_its_markup() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let page = cl_api::PageRequest::new(50, 0).expect("page");
    let offered = billable::costs(&mut state, client, page);
    assert_eq!(offered.data.len(), 1);
    let src = offered.data[0].line_id;
    let line = InvoiceLine {
        billable_source_line_id: Some(src),
        markup_percent: SignedPercent::parse("10").ok(),
        ..line_for(&items, "842.15", false, "1")
    };
    let new = NewInvoice::new(client, DemoState::today(), vec![line]);
    let p = preview(&mut state, &new).expect("preview");
    assert_eq!(p.lines[0].amount.to_wire(), "926.3700");
    let saved = create(&mut state, &new, None).expect("create");
    assert_eq!(billable::costs(&mut state, client, page).data.len(), 0);
    let e = create(&mut state, &new, None).expect_err("twice");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::BillableAlreadyBilled)
    );
    void(&mut state, saved.record.txn.id, None).expect("void");
    assert_eq!(billable::costs(&mut state, client, page).data.len(), 1);
}

#[test]
fn a_price_level_prices_an_unpriced_item_line() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let item = items
        .iter()
        .find(|i| i.fields.unit_price.is_some())
        .expect("a priced item");
    let level: cl_api::PriceLevel = serde_json::from_value(json!({
        "id": "d5000000-0000-4000-8000-000000000001", "name": "Builders", "percent": "-7.5",
    }))
    .expect("level");
    state.price_levels.clients.insert(client, level.id);
    state.price_levels.levels.push(level);
    let line = InvoiceLine::item(item.id, Quantity::parse_wire("1").expect("q"));
    let new = NewInvoice::new(client, DemoState::today(), vec![line]);
    let p = preview(&mut state, &new).expect("preview");
    let base = item.fields.unit_price.expect("price").as_decimal();
    let want = price_levels::level_price(base, SignedPercent::parse("-7.5").expect("p"));
    assert_eq!(p.lines[0].unit_price.map(Money::as_decimal), Some(want));
    assert_eq!(
        p.lines[0].unit_price_source,
        Some(cl_api::PriceSource::PriceLevel)
    );
    let unknown = NewInvoice::new(
        client,
        DemoState::today(),
        vec![InvoiceLine::item(
            ItemId::parse("d6000000-0000-4000-8000-000000000001").expect("id"),
            Quantity::parse_wire("1").expect("q"),
        )],
    );
    assert!(preview(&mut state, &unknown).is_err());
}
