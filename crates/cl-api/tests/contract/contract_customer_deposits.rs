//! Customer deposits against the real engine (P21 plan §6.2 AC3, golden
//! case 6; U-135).
//!
//! - `customer_deposit_lifecycle`: 500.00 held (Dr Undeposited Funds / Cr
//!   Customer Deposits); applied to a 1,200.00 invoice it draws the
//!   liability back to where it was and leaves 700.00 open; the deposit
//!   can't be voided while the draw stands (409 `DEPOSIT_APPLIED`);
//!   voiding the draw puts the 500.00 back on hand.

use cl_api::{
    ApplyInvoice, ApplyRequest, Capabilities, PaymentListQuery, PaymentRefusal,
    PaymentWriteOptions, PostingOptions, payment_refusal,
};
use cl_core::Money;

use crate::common::{date, key, m};
use crate::contract_receive_payment::{
    balance, clearing, client_balance, invoice, invoice_balance, mockup_client, preview, request,
    save_of, system_account,
};
use crate::support::Harness;

/// AC3: the deposit liability clears on application.
pub fn customer_deposit_lifecycle(h: &Harness) {
    if !h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
        || !h.require_cap(Capabilities::CUSTOMER_DEPOSITS, "U-135")
    {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 3);
    // The mockup client: two open invoices, no payments yet.
    let x = mockup_client(h, s, "Deposit client");
    let uf = clearing(h, s);
    // The preview offers to hold an amount with nothing applied.
    let mut r = request(x.client, on, Some("500.00"), Some(uf));
    r.applications = Some(Vec::new());
    let p = preview(h, s, &r);
    assert!(p.hold_as_deposit_offered, "nothing applied: hold?");
    r.hold_as_deposit = true;
    let p = preview(h, s, &r);
    assert_eq!(
        p.contact_balance_after, p.contact_balance_before,
        "a hold leaves the client balance alone"
    );
    let mut save = save_of(&p, x.client, on, uf);
    save.hold_as_deposit = true;
    save.applications = Vec::new();
    // The liability's balance before (company-wide; compared as a change).
    let cd_before = h
        .run(s.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some("customer_deposits"))
        .map(|a| a.balance.unwrap_or(Money::ZERO));
    let held = h
        .run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), key()),
        )
        .expect("hold");
    assert!(held.holds_deposit);
    assert_eq!(held.deposit_remaining, m("500.00"));
    let cd = system_account(h, s, "customer_deposits");
    let cd_before = cd_before.unwrap_or(Money::ZERO);
    // The engine reports account balances debit-positive: a liability
    // that grows by 500.00 reads 500.00 lower.
    let cd_holding = |x: &str| cd_before.checked_sub(m(x)).expect("sub");
    assert_eq!(balance(h, s, cd), cd_holding("500.00"));
    // The entry: Dr Undeposited Funds 500.00 / Cr Customer Deposits 500.00.
    let t = h.run(s.transactions().get(held.id)).expect("get").record;
    let lines: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.debit.is_positive() || l.credit.is_positive())
        .map(|l| (l.account_id, l.debit, l.credit))
        .collect();
    assert!(lines.contains(&(uf, m("500.00"), Money::ZERO)), "{lines:?}");
    assert!(lines.contains(&(cd, Money::ZERO, m("500.00"))), "{lines:?}");
    let on_hand = h
        .run(s.customer_payments().deposits_on_hand(x.client))
        .expect("on hand");
    assert_eq!(on_hand.len(), 1);
    assert_eq!(on_hand[0].remaining, m("500.00"));
    let before = client_balance(h, s, x.client, on);

    // Apply it to a new 1,200.00 invoice: a draw.
    let inv = invoice(h, s, x.client, x.income, date(2026, 11, 10), "1200.00");
    let applied = h
        .run(s.customer_payments().apply(
            held.id,
            &ApplyRequest {
                date: date(2026, 11, 10),
                applications: vec![ApplyInvoice {
                    invoice_id: inv,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply the deposit");
    assert_eq!(applied.applied[0].amount, m("500.00"));
    assert_eq!(applied.remaining, Money::ZERO);
    assert_eq!(invoice_balance(h, s, inv), m("700.00"));
    assert_eq!(balance(h, s, cd), cd_before, "the liability cleared");
    assert_eq!(
        client_balance(h, s, x.client, date(2026, 11, 10)),
        before.checked_add(m("700.00")).expect("add"),
        "the new invoice less the deposit"
    );
    let on_hand = h
        .run(s.customer_payments().deposits_on_hand(x.client))
        .expect("on hand");
    assert_eq!(on_hand.len(), 0, "nothing left on hand");

    // The deposit can't be voided while its draw stands.
    let held_now = h.run(s.customer_payments().get(held.id)).expect("get");
    let e = h
        .run(s.customer_payments().void(
            held.id,
            None,
            &PaymentWriteOptions {
                posting: PostingOptions::default(),
                expected_updated_at: Some(held_now.updated_at),
            },
            key(),
        ))
        .expect_err("applied");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::DepositApplied),
        "{e:?}"
    );

    // Voiding the draw puts the deposit back on hand.
    let draw = h
        .run(s.customer_payments().list(
            &PaymentListQuery {
                contact: Some(x.client),
                ..PaymentListQuery::default()
            },
            cl_api::PageRequest::first_max(),
        ))
        .expect("list")
        .data
        .into_iter()
        .find(|p| p.id != held.id)
        .expect("the draw is a payment of the client");
    let draw = h.run(s.customer_payments().get(draw.id)).expect("draw");
    h.run(s.customer_payments().void(
        draw.id,
        None,
        &PaymentWriteOptions {
            posting: PostingOptions::default(),
            expected_updated_at: Some(draw.updated_at),
        },
        key(),
    ))
    .expect("void the draw");
    assert_eq!(invoice_balance(h, s, inv), m("1200.00"));
    assert_eq!(balance(h, s, cd), cd_holding("500.00"));
    let on_hand = h
        .run(s.customer_payments().deposits_on_hand(x.client))
        .expect("on hand");
    assert_eq!(on_hand.len(), 1);
    assert_eq!(on_hand[0].remaining, m("500.00"));
}
