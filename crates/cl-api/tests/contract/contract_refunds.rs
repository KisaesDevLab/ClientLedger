//! Refunds of credit-memo credit against the real engine (P22 plan §3 D8,
//! §6.1 golden 3-4, §6.2).
//!
//! - `refund_clears_credit` (golden 3, Q-P22d): a refund check with Print
//!   later takes the whole credit, waits in the check print queue, moves
//!   the bank and the client exactly; voiding it gives the credit back.
//! - `partial_refund_apply_rest_and_over_refund` (golden 4): a cash
//!   refund replayed posts once; the rest applied; one cent more is 409
//!   `CREDIT_NOT_AVAILABLE`; a refunded memo can't be voided until its
//!   refund is.
//! - `refund_check_number_with_print_later_400`: both is 400 naming
//!   `checkNumber`.
//! - `refund_cross_company_404`.

use cl_api::{
    AccountType, ApiError, ApplyInvoice, ApplyRequest, Capabilities, CreditRefusal, Method,
    NewCreditMemo, NewRefund, PostingOptions, RefundCheck, RefundMethod, RefundPrintStatus,
    RefundRefusal, RefundSource, Session, credit_refusal, refund_refusal,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::common::{date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_credit_memos::{Seed, case1, create, restore_tax_rate, seed, set_tax_rate};
use crate::contract_receive_payment::{balance, client_balance, invoice_balance};
use crate::support::Harness;

fn caps(h: &Harness) -> bool {
    crate::contract_credit_memos::caps(h)
        && h.require_cap(Capabilities::REFUND_CREDIT_MEMO, "U-139")
        && h.require_cap(Capabilities::REFUND_CHECKS, "U-139")
}

fn bank(h: &Harness, s: &Session) -> AccountId {
    detail_account(h, s, AccountType::Asset, "bank")
}

fn refund_of(
    memo: TransactionId,
    c: ContactId,
    on: CalendarDate,
    from: AccountId,
    amount: &str,
    check: bool,
) -> NewRefund {
    NewRefund {
        customer_id: c,
        date: on,
        from_account: from,
        amount: m(amount),
        source: RefundSource::CreditMemo(memo),
        method: if check {
            RefundMethod::Check
        } else {
            RefundMethod::Cash
        },
        check: if check {
            RefundCheck::PrintLater
        } else {
            RefundCheck::NotACheck
        },
        ref_no: None,
        memo: None,
    }
}

fn remaining(h: &Harness, s: &Session, memo: TransactionId) -> Money {
    h.run(s.credit_memos().get(memo)).expect("memo").remaining
}

fn void(h: &Harness, s: &Session, id: TransactionId) -> Result<(), ApiError> {
    h.run(
        s.transactions()
            .void(id, "contract: refund", &PostingOptions::default(), key()),
    )
    .map(|_| ())
}

/// The golden 1 memo kept as credit (8.25 % tax): 399.75 to give back.
fn kept_memo(h: &Harness, s: &Session, name: &str) -> (Seed, TransactionId) {
    let old = set_tax_rate(h, s, "0.0825");
    let x = seed(h, s, name);
    let rec = create(h, s, &case1(h, s, &x));
    restore_tax_rate(h, s, &old);
    assert_eq!(rec.remaining, m("399.75"));
    (x, rec.id)
}

/// Golden 3 and Q-P22d: refund by check with Print later.
pub fn refund_clears_credit(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let (x, memo) = kept_memo(h, s, "Bluestem refund");
    let on = date(2026, 11, 3);
    let from = bank(h, s);
    let bank_before = balance(h, s, from);
    assert_eq!(client_balance(h, s, x.client, on), m("350.25"));
    let id = h
        .run(s.customer_refunds().create(
            &refund_of(memo, x.client, on, from, "399.75", true),
            &PostingOptions::default(),
            key(),
        ))
        .expect("refund");
    assert_eq!(remaining(h, s, memo), Money::ZERO);
    assert_eq!(client_balance(h, s, x.client, on), m("750.00"));
    assert_eq!(
        balance(h, s, from),
        bank_before.checked_sub(m("399.75")).expect("sub")
    );
    let credits = h.run(s.credit_memos().credits(x.client)).expect("credits");
    assert!(
        credits
            .iter()
            .all(|c| c.source_id != memo || c.remaining == Money::ZERO),
        "no credit left to offer"
    );
    // The refund reads back: a check waiting to print, unnumbered.
    let r = h.run(s.customer_refunds().get(id)).expect("get");
    assert_eq!(r.amount, m("399.75"));
    assert_eq!(r.customer_id, x.client);
    assert_eq!(r.method, Some(RefundMethod::Check));
    assert_eq!(r.print_status, Some(RefundPrintStatus::Queue));
    assert_eq!(r.check_number, None);
    let src = r.source.expect("its source");
    assert_eq!(src.id, memo);
    assert_eq!(src.kind, cl_api::CreditKind::CreditMemo);
    // The memo lists it.
    let got = h.run(s.credit_memos().get(memo)).expect("memo");
    let refunds: Vec<_> = got
        .live_refunds()
        .map(|r| (r.refund_id, r.amount))
        .collect();
    assert_eq!(refunds, vec![(id, m("399.75"))]);
    // It waits in the check print queue of that bank account.
    let queue = h.run(s.printing().check_queue(Some(from))).expect("queue");
    let queued = queue
        .iter()
        .find(|q| q.id == id)
        .expect("the refund check waits to print");
    assert_eq!(queued.amount, m("399.75"));
    // Voiding the refund gives the credit back exactly.
    void(h, s, id).expect("void the refund");
    assert_eq!(remaining(h, s, memo), m("399.75"));
    assert_eq!(client_balance(h, s, x.client, on), m("350.25"));
    assert_eq!(balance(h, s, from), bank_before);
}

/// Golden 4: a partial cash refund (replayed once), the rest applied, an
/// over-refund refused, and the void order.
pub fn partial_refund_apply_rest_and_over_refund(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let (x, memo) = kept_memo(h, s, "Bluestem partial");
    let on = date(2026, 11, 3);
    let from = bank(h, s);
    let req = refund_of(memo, x.client, on, from, "100.00", false);
    let k = cl_api::IdempotencyKey::new();
    let one = h
        .run(
            s.customer_refunds()
                .create(&req, &PostingOptions::default(), k.clone()),
        )
        .expect("refund");
    let two = h
        .run(
            s.customer_refunds()
                .create(&req, &PostingOptions::default(), k),
        )
        .expect("replayed refund");
    assert_eq!(one, two, "one refund");
    assert_eq!(remaining(h, s, memo), m("299.75"), "posted once");
    // Apply the rest to B later.
    let applied = h
        .run(s.credit_memos().apply(
            memo,
            &ApplyRequest {
                date: date(2026, 11, 4),
                applications: vec![ApplyInvoice {
                    invoice_id: x.b,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply the rest");
    assert_eq!(applied.remaining, Money::ZERO);
    assert_eq!(invoice_balance(h, s, x.b), m("200.25"));
    // One cent more: 409 CREDIT_NOT_AVAILABLE, nothing posts.
    let e = h
        .run(s.customer_refunds().create(
            &refund_of(memo, x.client, date(2026, 11, 5), from, "0.01", false),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("nothing left to refund");
    assert_eq!(
        refund_refusal(&e),
        Some(RefundRefusal::CreditNotAvailable),
        "{e:?}"
    );
    // A refund dated before the memo is refused.
    let e = h
        .run(s.customer_refunds().create(
            &refund_of(memo, x.client, date(2026, 11, 1), from, "0.01", false),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("before the memo");
    assert!(
        matches!(
            refund_refusal(&e),
            Some(RefundRefusal::DateBeforeSource | RefundRefusal::CreditNotAvailable)
        ),
        "{e:?}"
    );
    // A refunded memo can't be voided; void the refund first.
    let e = void(h, s, memo).expect_err("the memo was refunded");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::CreditRefunded),
        "{e:?}"
    );
    void(h, s, one).expect("void the refund");
    void(h, s, memo).expect("void the memo");
    assert_eq!(invoice_balance(h, s, x.b), m("500.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("750.00"));
}

/// A typed check number and Print later together are 400 naming
/// `checkNumber` (the queue numbers the check when it prints). The typed
/// request can't say both, so the body is sent raw.
pub fn refund_check_number_with_print_later_400(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = seed(h, s, "Bluestem both");
    let memo = create(
        h,
        s,
        &NewCreditMemo::new(
            x.client,
            date(2026, 11, 2),
            vec![cl_api::InvoiceLine {
                is_taxable: Some(false),
                ..cl_api::InvoiceLine::account(
                    x.income,
                    cl_core::Quantity::parse_wire("1").expect("qty"),
                    m("20.00"),
                )
            }],
        ),
    );
    let mut body = refund_of(
        memo.id,
        x.client,
        date(2026, 11, 3),
        bank(h, s),
        "20.00",
        true,
    )
    .body(&h.caps)
    .expect("body");
    body["printLater"] = serde_json::json!(true);
    body["checkNumber"] = serde_json::json!(5001);
    let e = h
        .run(s.raw_mapped(Method::Post, "/api/v1/customer-refunds", Some(&body)))
        .expect_err("both");
    assert_eq!(
        refund_refusal(&e),
        Some(RefundRefusal::CheckNumberWithPrintLater),
        "{e:?}"
    );
    match &e {
        ApiError::Validation(v) => assert_eq!(
            v.field().map(|f| f.as_str().to_string()),
            Some("checkNumber".into())
        ),
        other => panic!("a 400: {other:?}"),
    }
    assert_eq!(remaining(h, s, memo.id), m("20.00"), "nothing posted");
}

/// Another company's refund and memo are 404.
pub fn refund_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let (x, memo) = kept_memo(h, s, "Bluestem refund scope");
    let on = date(2026, 11, 3);
    let id = h
        .run(s.customer_refunds().create(
            &refund_of(memo, x.client, on, bank(h, s), "10.00", false),
            &PostingOptions::default(),
            key(),
        ))
        .expect("refund");
    let b = h.s3();
    let e = h
        .run(b.customer_refunds().get(id))
        .expect_err("another company's refund");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let b_bank = bank(h, &b);
    let e = h
        .run(b.customer_refunds().create(
            &refund_of(memo, x.client, on, b_bank, "10.00", false),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("refund another company's credit");
    // PR-3 item 9: exactly the engine's answer. Company B's own bank passes
    // the account check; the source memo is another company's, so it is
    // 404 (customer-refund.service.ts: "Credit memo not found").
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    assert_eq!(remaining(h, s, memo), m("389.75"));
}
