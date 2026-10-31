//! Billable costs against the real engine (P20 review B4, U-128):
//! `billable_consumed` — a cost marked billable on a check is listed,
//! invoiced (the invoice line sends only its source, income account and
//! markup), gone from the list, refused on a second invoice (409), back
//! after the line is released or the invoice voided; GET reads the cost
//! and markup as one line. PENDING(U-128) until `billable-costs-v1`.

use cl_api::{
    AccountType, ApiError, BankPosting, Capabilities, InvoiceLine, InvoiceRefusal,
    InvoiceWriteOptions, NewInvoice, PostingOptions, SignedPercent, invoice_refusal,
};
use cl_core::Money;

use crate::common::{account, date, key, m};
use crate::contract_write_check::{books, story};
use crate::support::Harness;

pub fn billable_consumed(h: &Harness) {
    if !h.require_cap(Capabilities::BILLABLE_COSTS, "U-128")
        || !h.require_cap(Capabilities::LINE_BILLABLE, "line-billable-v1")
        || !h.require_cap(Capabilities::INVOICE_EDIT, "U-124")
    {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    // A markup needs the company's markup account.
    let markup = account(h, s, AccountType::Revenue, "Markup income").id;
    h.run(s.set_billable_markup_account(Some(markup), key()))
        .expect("markup account");
    let p =
        BankPosting::check_with(story(&b, date(2026, 10, 20), None, true), true).expect("check");
    h.run(
        s.transactions()
            .create_bank_posting(&p, &PostingOptions::default(), key()),
    )
    .expect("billable check");
    let costs = h
        .run(s.invoices().all_billable_costs(b.project))
        .expect("costs");
    let cost = costs
        .items
        .iter()
        .find(|c| c.cost_amount == m("842.15"))
        .expect("the check's billable line")
        .clone();
    assert_eq!(costs.total_count, costs.items.len() as u64);
    let income = account(h, s, AccountType::Revenue, "Billed costs").id;
    let line = InvoiceLine {
        markup_percent: Some(SignedPercent::parse("10").expect("pct")),
        is_taxable: Some(false),
        ..InvoiceLine::billable(cost.source_line_id, income)
    };
    let new = NewInvoice::new(b.project, date(2026, 10, 25), vec![line]);
    let p = h.run(s.invoices().preview(&new, None)).expect("preview");
    assert_eq!(p.lines[0].amount, m("926.37"), "842.15 + 84.22");
    assert_eq!(p.lines[0].markup_amount, Some(m("84.22")));
    let saved = h
        .run(s.invoices().create(&new, &PostingOptions::default(), key()))
        .expect("invoice")
        .invoice;
    let r = h
        .run(s.invoices().get(saved.record.txn.id))
        .expect("get")
        .record;
    assert_eq!(r.lines.len(), 1, "cost and markup read as one line");
    assert_eq!(r.lines[0].amount, m("926.37"));
    assert_eq!(r.txn.total, Some(m("926.37")));
    let left = h
        .run(s.invoices().all_billable_costs(b.project))
        .expect("costs");
    assert!(
        left.items
            .iter()
            .all(|c| c.source_line_id != cost.source_line_id),
        "billed costs leave the list"
    );
    // A second invoice can't bill it.
    let e = h
        .run(s.invoices().create(&new, &PostingOptions::default(), key()))
        .expect_err("double claim");
    assert_eq!(
        invoice_refusal(&e),
        Some(InvoiceRefusal::BillableAlreadyBilled),
        "{e:?}"
    );
    // Item, quantity or price on a billable line: refused locally.
    let typed = InvoiceLine {
        quantity: Some(cl_core::Quantity::parse_wire("1").expect("q")),
        ..InvoiceLine::billable(cost.source_line_id, income)
    };
    let e = h
        .run(s.invoices().create(
            &NewInvoice::new(b.project, date(2026, 10, 25), vec![typed]),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("read-only fields");
    assert!(matches!(e, ApiError::Local(_)), "{e:?}");
    // An edit that drops the line without releasing it: 409; released: back.
    let plain = NewInvoice::new(
        b.project,
        date(2026, 10, 25),
        vec![InvoiceLine::account(
            income,
            cl_core::Quantity::parse_wire("1").expect("q"),
            m("10.00"),
        )],
    );
    let e = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &plain, &PostingOptions::default(), key()),
        )
        .expect_err("missing");
    assert_eq!(
        invoice_refusal(&e),
        Some(InvoiceRefusal::BillableLinesMissing),
        "{e:?}"
    );
    let released = NewInvoice {
        release_billable_source_line_ids: vec![cost.source_line_id],
        ..plain
    };
    let edited = h
        .run(
            s.invoices()
                .update(r.txn.id, &r, &released, &PostingOptions::default(), key()),
        )
        .expect("released")
        .invoice;
    let back = h
        .run(s.invoices().all_billable_costs(b.project))
        .expect("costs");
    assert!(
        back.items
            .iter()
            .any(|c| c.source_line_id == cost.source_line_id)
    );
    // Billed again, then the invoice voided: back again.
    let again = h
        .run(s.invoices().create(&new, &PostingOptions::default(), key()))
        .expect("billed again")
        .invoice;
    h.run(s.invoices().void(
        again.record.txn.id,
        "contract test",
        &InvoiceWriteOptions {
            expected_updated_at: again.record.txn.updated_at,
            ..InvoiceWriteOptions::default()
        },
        key(),
    ))
    .expect("void");
    let after = h
        .run(s.invoices().all_billable_costs(b.project))
        .expect("costs");
    assert!(
        after
            .items
            .iter()
            .any(|c| c.source_line_id == cost.source_line_id)
    );
    assert_ne!(edited.record.txn.total, Some(Money::ZERO));
}
