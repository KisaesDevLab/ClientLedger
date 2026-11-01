//! Sales receipts against the real engine (P22 plan §3 D8, §6.1 golden
//! 7, Q-P20u).
//!
//! - `sales_receipt_items_tax`: one item 2 x 49.99 taxable at 8.25 % is
//!   99.98 + 8.25 = 108.23, Dr Payments Clearing 108.23; the preview's
//!   figures are the saved ones; an edit with the read version saves, the
//!   old version is 409 STALE.
//! - `negative_receipt_needs_bank`: a -20.00 return to Payments Clearing
//!   is 400 `NEGATIVE_RECEIPT_NEEDS_BANK`; to a bank account it posts.
//! - `sales_receipt_cross_company_404`.

use cl_api::{
    AccountType, ApiError, Capabilities, ConflictReason, CreditRefusal, CreditWriteOptions,
    InvoiceLine, ItemFields, NewSalesReceipt, PaymentMethod, PostingOptions, Session,
    credit_refusal,
};
use cl_core::ids::{AccountId, ContactId};
use cl_core::{Money, Quantity};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_credit_memos::{restore_tax_rate, set_tax_rate};
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{balance, clearing};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::SALES_RECEIPT_LINES, "U-143")
        && h.require_cap(Capabilities::IDEMPOTENCY_CREDIT_MEMO, "U-137")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

fn receipt(c: ContactId, to: AccountId, lines: Vec<InvoiceLine>) -> NewSalesReceipt {
    let mut r = NewSalesReceipt::new(date(2026, 11, 4), lines);
    r.contact_id = Some(c);
    r.deposit_to_account_id = Some(to);
    r.payment_method = Some(PaymentMethod::Cash);
    r
}

fn income(h: &Harness, s: &Session) -> AccountId {
    account(h, s, AccountType::Revenue, "Counter sales").id
}

/// Golden 7: an item with tax into Payments Clearing.
pub fn sales_receipt_items_tax(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let old_rate = set_tax_rate(h, s, "0.0825");
    let c = customer(h, s, "Walk-in");
    let inc = income(h, s);
    let leash = h
        .run(s.items().create(
            &ItemFields {
                name: unique("Leash"),
                description: None,
                unit_price: Some(m("49.99")),
                income_account_id: inc,
                is_taxable: true,
                item_type: cl_api::ItemType::Service,
                discount_percent: None,
            },
            key(),
        ))
        .expect("item")
        .record
        .id;
    let uf = clearing(h, s);
    let uf_before = balance(h, s, uf);
    let r = receipt(c, uf, vec![InvoiceLine::item(leash, q("2"))]);
    let p = h
        .run(s.sales_receipts().preview(&r, None))
        .expect("preview");
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.figures.subtotal, m("99.98"));
    assert_eq!(p.figures.tax_total, m("8.25"), "round2(8.24835)");
    assert_eq!(p.figures.total, m("108.23"));
    let rec = h
        .run(
            s.sales_receipts()
                .create(&r, &PostingOptions::default(), key()),
        )
        .expect("create");
    assert_eq!(rec.subtotal, p.figures.subtotal);
    assert_eq!(rec.tax_amount, p.figures.tax_total);
    assert_eq!(rec.total, m("108.23"));
    assert_eq!(rec.deposit_to_account_id, Some(uf));
    assert_eq!(rec.customer_id, Some(c));
    assert!(!rec.deposited);
    assert_eq!(rec.lines.len(), 1);
    assert_eq!(rec.lines[0].amount, m("99.98"));
    assert_eq!(
        balance(h, s, uf),
        uf_before.checked_add(m("108.23")).expect("add"),
        "Dr Payments Clearing 108.23"
    );
    // It waits to be deposited at exactly 108.23.
    let waiting = h.run(s.payments().pending_deposits()).expect("pending");
    let mine = waiting
        .items
        .iter()
        .find(|w| w.source == rec.id)
        .expect("the receipt waits to be deposited");
    assert_eq!(mine.exact(), Some(m("108.23")));
    // GET reads it back; an edit with the read version saves; the old
    // version is stale.
    let got = h.run(s.sales_receipts().get(rec.id)).expect("get");
    assert_eq!(got.total, rec.total);
    assert_eq!(got.updated_at, rec.updated_at);
    let mut edit = r.clone();
    edit.memo = Some("Counter sale".into());
    let opts = CreditWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: Some(rec.updated_at),
    };
    let first = h
        .run(s.sales_receipts().update(rec.id, &edit, &opts, key()))
        .expect("edit");
    assert_eq!(first.total, m("108.23"));
    assert_ne!(first.updated_at, rec.updated_at);
    let e = h
        .run(s.sales_receipts().update(rec.id, &edit, &opts, key()))
        .expect_err("stale");
    assert!(
        matches!(
            e,
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                ..
            }
        ),
        "{e:?}"
    );
    restore_tax_rate(h, s, &old_rate);
}

/// A return (negative receipt) must come out of a bank account.
pub fn negative_receipt_needs_bank(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Return");
    let inc = income(h, s);
    let line = InvoiceLine {
        is_taxable: Some(false),
        ..InvoiceLine::account(inc, q("1"), m("-20.00"))
    };
    let uf = clearing(h, s);
    let to_clearing = receipt(c, uf, vec![line.clone()]);
    let p = h.run(s.sales_receipts().preview(&to_clearing, None));
    if let Ok(p) = &p {
        assert_eq!(p.figures.total, m("-20.00"));
        assert!(
            p.problems
                .iter()
                .any(|x| x.code == "NEGATIVE_RECEIPT_NEEDS_BANK"),
            "{:?}",
            p.problems
        );
    }
    let e = h
        .run(
            s.sales_receipts()
                .create(&to_clearing, &PostingOptions::default(), key()),
        )
        .expect_err("a return into Payments Clearing");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::NegativeReceiptNeedsBank),
        "{e:?}"
    );
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let before = balance(h, s, bank);
    let rec = h
        .run(s.sales_receipts().create(
            &receipt(c, bank, vec![line]),
            &PostingOptions::default(),
            key(),
        ))
        .expect("a return from the bank");
    assert_eq!(rec.total, m("-20.00"));
    assert_eq!(
        balance(h, s, bank),
        before.checked_sub(m("20.00")).expect("sub")
    );
    assert_ne!(rec.total, Money::ZERO);
}

/// Another company's receipt is 404.
pub fn sales_receipt_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Receipt scope");
    let inc = income(h, s);
    let rec = h
        .run(s.sales_receipts().create(
            &receipt(
                c,
                clearing(h, s),
                vec![InvoiceLine {
                    is_taxable: Some(false),
                    ..InvoiceLine::account(inc, q("1"), m("12.00"))
                }],
            ),
            &PostingOptions::default(),
            key(),
        ))
        .expect("create");
    let e = h
        .run(h.s3().sales_receipts().get(rec.id))
        .expect_err("another company's receipt");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
}
