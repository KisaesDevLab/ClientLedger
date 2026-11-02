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
    let p = preview(&mut state, &new, None).expect("preview");
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
/// after a void — under the engine's rules (U-128): only source, account
/// and markup on the line; the markup posts on its own line; an edit that
/// drops the cost must release it.
#[test]
#[allow(clippy::too_many_lines, reason = "one story, step by step")]
fn a_billable_cost_is_invoiced_once_with_its_markup() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let offered = billable::costs(&mut state, client);
    assert_eq!(offered.items.len(), 1);
    assert_eq!(offered.total_count, 1);
    assert_eq!(offered.total_amount.to_wire(), "842.1500");
    let src = offered.items[0].source_line_id;
    let account = items[0].fields.income_account_id;
    // B2: item, quantity and price are the cost's; the account is required.
    let typed = InvoiceLine {
        billable_source_line_id: Some(src),
        ..line_for(&items, "842.15", false, "1")
    };
    let e = preview(
        &mut state,
        &NewInvoice::new(client, DemoState::today(), vec![typed]),
        None,
    )
    .expect_err("read-only fields");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::BillableFieldsReadOnly)
    );
    let no_account = InvoiceLine {
        account_id: None,
        ..InvoiceLine::billable(src, account)
    };
    let e = preview(
        &mut state,
        &NewInvoice::new(client, DemoState::today(), vec![no_account]),
        None,
    )
    .expect_err("account");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::BillableAccountRequired)
    );
    let line = InvoiceLine {
        markup_percent: SignedPercent::parse("10").ok(),
        ..InvoiceLine::billable(src, account)
    };
    let new = NewInvoice::new(client, DemoState::today(), vec![line]);
    let p = preview(&mut state, &new, None).expect("preview");
    assert_eq!(p.lines[0].amount.to_wire(), "926.3700");
    assert_eq!(
        p.lines[0].unit_price_source,
        Some(cl_api::PriceSource::BillableCost)
    );
    assert_eq!(
        p.lines[0].markup_amount.map(Money::to_wire).as_deref(),
        Some("84.2200")
    );
    assert!(p.lines[0].error.is_none());
    let saved = create(&mut state, &new, None).expect("create");
    let r = &saved.record;
    assert_eq!(r.lines.len(), 1, "cost and markup read as one row");
    assert_eq!(r.lines[0].amount.to_wire(), "926.3700");
    assert_eq!(r.billable_costs.len(), 1);
    assert_eq!(r.billable_sources(), vec![src]);
    assert_eq!(billable::costs(&mut state, client).items.len(), 0);
    // Another invoice can't bill it: the preview says which; a save is 409.
    let p = preview(&mut state, &new, None).expect("preview");
    assert_eq!(
        p.lines[0]
            .error
            .as_ref()
            .and_then(|e| e.invoice_number.clone()),
        r.txn.txn_number.clone()
    );
    let e = create(&mut state, &new, None).expect_err("twice");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::BillableAlreadyBilled)
    );
    // An edit that drops the cost without releasing it: 409.
    let plain = NewInvoice::new(
        client,
        DemoState::today(),
        vec![line_for(&items, "10.00", false, "1")],
    );
    let id = r.txn.id;
    let e = update(&mut state, id, r.txn.updated_at, &plain, None).expect_err("missing");
    assert_eq!(
        cl_api::invoice_refusal(&e),
        Some(cl_api::InvoiceRefusal::BillableLinesMissing)
    );
    // Resending it unchanged keeps the money exactly.
    let again = update(&mut state, id, r.txn.updated_at, &new, None).expect("kept");
    assert_eq!(again.record.txn.total, r.txn.total);
    let released = NewInvoice {
        release_billable_source_line_ids: vec![src],
        ..plain
    };
    let after =
        update(&mut state, id, again.record.txn.updated_at, &released, None).expect("released");
    assert_eq!(after.record.billable_costs, Vec::new());
    assert_eq!(billable::costs(&mut state, client).items.len(), 1);
    // Billed again, then voided: back again.
    let saved = create(&mut state, &new, None).expect("billed again");
    void(&mut state, saved.record.txn.id, None).expect("void");
    assert_eq!(billable::costs(&mut state, client).items.len(), 1);
}

/// B3: a subtotal and a percent discount read back without a quantity and
/// post the same total when resent as read.
#[test]
fn subtotal_and_discount_lines_round_trip() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let sub = items
        .iter()
        .find(|i| i.fields.item_type == cl_api::ItemType::Subtotal);
    let disc = items.iter().find(|i| {
        i.fields.item_type == cl_api::ItemType::Discount && i.fields.discount_percent.is_some()
    });
    let (Some(sub), Some(disc)) = (sub, disc) else {
        return; // the demo list has no such items
    };
    let new = NewInvoice::new(
        client,
        DemoState::today(),
        vec![
            line_for(&items, "65.00", true, "18"),
            InvoiceLine::item(sub.id, Quantity::parse_wire("1").expect("q")),
            InvoiceLine::item(disc.id, Quantity::parse_wire("1").expect("q")),
        ],
    );
    let saved = create(&mut state, &new, None).expect("create");
    let r = &saved.record;
    assert_eq!(r.lines.len(), 3);
    assert_eq!(r.lines[1].quantity, None);
    assert_eq!(r.lines[2].quantity, None);
    assert!(r.lines[2].discount_percent.is_some());
    // Resent as read: no quantity, the stored percent.
    let resend = NewInvoice::new(
        client,
        DemoState::today(),
        r.lines
            .iter()
            .map(|l| InvoiceLine {
                account_id: if l.item.is_some() {
                    None
                } else {
                    Some(l.account)
                },
                item_id: l.item,
                quantity: l.quantity,
                unit_price: l.unit_price,
                discount_percent: l.discount_percent,
                is_taxable: Some(l.is_taxable),
                ..InvoiceLine::billable(src_none(), l.account)
            })
            .map(|l| InvoiceLine {
                billable_source_line_id: None,
                ..l
            })
            .collect(),
    );
    let again = update(&mut state, r.txn.id, r.txn.updated_at, &resend, None).expect("edit");
    assert_eq!(again.record.txn.total, r.txn.total);
}

fn src_none() -> JournalLineId {
    JournalLineId::parse(billable::STORY_COST_LINE).expect("id")
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
    let p = preview(&mut state, &new, None).expect("preview");
    let base = item.fields.unit_price.expect("price").as_decimal();
    let want = price_levels::level_price(base, SignedPercent::parse("-7.5").expect("p"));
    assert_eq!(p.lines[0].unit_price.map(Money::as_decimal), Some(want));
    assert_eq!(
        p.lines[0].unit_price_source,
        Some(cl_api::PriceSource::PriceLevelPercent)
    );
    let unknown = NewInvoice::new(
        client,
        DemoState::today(),
        vec![InvoiceLine::item(
            ItemId::parse("d6000000-0000-4000-8000-000000000001").expect("id"),
            Quantity::parse_wire("1").expect("q"),
        )],
    );
    assert!(preview(&mut state, &unknown, None).is_err());
}

/// Engine PR #202 review: discounts stacked under a subtotal compound; a %
/// of a group at or below zero is zero; a positive document never has
/// negative tax.
#[test]
fn stacked_discounts_compound_and_tax_never_goes_negative() {
    let mut state = DemoState::default();
    let (client, items) = story(&mut state);
    let find = |t: cl_api::ItemType| {
        items
            .iter()
            .find(|i| {
                i.fields.item_type == t
                    && (t != cl_api::ItemType::Discount || i.fields.discount_percent.is_some())
            })
            .map(|i| i.id)
    };
    let (Some(sub), Some(disc)) = (
        find(cl_api::ItemType::Subtotal),
        find(cl_api::ItemType::Discount),
    ) else {
        return;
    };
    let pct = items
        .iter()
        .find(|i| i.id == disc)
        .and_then(|i| i.fields.discount_percent)
        .expect("percent")
        .as_decimal();
    let one = || Quantity::parse_wire("1").expect("q");
    let new = NewInvoice::new(
        client,
        DemoState::today(),
        vec![
            line_for(&items, "100.00", true, "1"),
            InvoiceLine::item(sub, one()),
            InvoiceLine::item(disc, one()),
            InvoiceLine::item(disc, one()),
        ],
    );
    let p = preview(&mut state, &new, None).expect("preview");
    let h = Decimal::ONE_HUNDRED;
    let first = -round(h * pct / h, 2);
    let second = -round((h + first) * pct / h, 2);
    assert_eq!(p.lines[2].amount.as_decimal(), first);
    assert_eq!(p.lines[3].amount.as_decimal(), second);
    // A taxable line of -50 on a positive invoice: the base floors at 0.
    let doc = compute(
        &[
            SalesLineIn {
                quantity: Decimal::ONE,
                unit_price: Decimal::from(100),
                taxable: false,
                tax_rate: None,
            },
            SalesLineIn {
                quantity: Decimal::ONE,
                unit_price: Decimal::from(-50),
                taxable: true,
                tax_rate: None,
            },
        ],
        Decimal::from_str("0.081").expect("rate"),
    )
    .expect("doc");
    assert_eq!(doc.tax_total, Decimal::ZERO);
    let neg = compute(
        &[SalesLineIn {
            quantity: Decimal::ONE,
            unit_price: Decimal::from(-50),
            taxable: true,
            tax_rate: None,
        }],
        Decimal::from_str("0.081").expect("rate"),
    )
    .expect("doc");
    assert_eq!(neg.tax_total, Decimal::from_str("-4.05").expect("d"));
}

/// Prev/Next lists newest first as the engine does (date, then when made,
/// descending), not by id: the seeded 1091/1095 and P22's 1201 to 1321
/// carry their numbers in their ids, so an id order put them ahead of an
/// invoice made today (`ui-check create_invoice`, Prev after Save & New).
#[test]
fn the_list_puts_an_invoice_made_today_before_the_seeded_ones() {
    let mut state = DemoState::new();
    // The seeds first: they build the contacts with the story clients.
    super::super::receivables::seed(&mut state);
    super::super::credit_memos::seed(&mut state);
    let (client, items) = story(&mut state);
    let seeded = list(&state);
    assert!(seeded.len() >= 2, "the seeded invoices are listed");
    let new = NewInvoice::new(
        client,
        DemoState::today(),
        vec![line_for(&items, "10.00", false, "1")],
    );
    let first = create(&mut state, &new, None)
        .expect("create")
        .record
        .txn
        .id;
    let second = create(&mut state, &new, None)
        .expect("create")
        .record
        .txn
        .id;
    let v = list(&state);
    assert_eq!(v[..2], [second, first]);
    assert_eq!(v[2..], seeded[..]);
    // Older dates follow newer ones whatever the order they were made in.
    let dates: Vec<String> = v
        .iter()
        .map(|id| state.invoices.stored[id].record["txnDate"].to_string())
        .collect();
    assert!(dates.windows(2).all(|w| w[0] >= w[1]), "{dates:?}");
}
