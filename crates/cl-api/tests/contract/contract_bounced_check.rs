//! Bounced checks against the real engine (P21 plan §6.2 AC4, golden case
//! 7; Q-P21i/k/u).
//!
//! - `bounced_check_restores_balances`: the mockup's 4,000.00 check is
//!   deposited with another client's 6,210.00 check as one 10,210.00
//!   deposit; the bounce with a 35.00 bank fee takes exactly 4,000.00 back
//!   out of the bank, reopens invoice 1091 to 4,385.00 and the client to
//!   5,645.00; the other check stays deposited; the payment has no credit
//!   left; a replay with the same key posts once; a new key is 409
//!   `PAYMENT_BOUNCED`; the deposit can't be voided while the bounce
//!   stands; the bank register shows the `returned_payment` entry.
//! - `undeposited_bounce_takes_it_from_clearing`: a check never deposited
//!   bounces out of Undeposited Funds and leaves the pending list; its fee
//!   comes out of the bank named for it.

use cl_api::{
    AccountType, BankPosting, BounceRequest, Capabilities, DepositCaps, DepositDraft, HeldKind,
    IdempotencyKey, PaymentRefusal, PostingOptions, Session, payment_refusal,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{
    balance, clearing, client_balance, invoice, invoice_balance, mockup_client, receive_auto,
};
use crate::support::Harness;

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
        && h.require_cap(Capabilities::PAYMENT_BOUNCE, "U-132")
        && h.require_cap(Capabilities::DEPOSIT_GROUPING, "U-52")
}

fn bounce_req(
    on: CalendarDate,
    fee: Option<(&str, AccountId)>,
    at: cl_core::Timestamp,
) -> BounceRequest {
    BounceRequest {
        date: on,
        bank_fee_amount: fee.map(|(f, _)| m(f)),
        bank_fee_account_id: fee.map(|(_, a)| a),
        bank_fee_bank_account_id: None,
        memo: None,
        expected_updated_at: at,
    }
}

fn pending_ids(h: &Harness, s: &Session) -> Vec<TransactionId> {
    h.run(s.payments().pending_deposits())
        .expect("pending")
        .items
        .iter()
        .map(|p| p.source)
        .collect()
}

/// AC4: the bounce restores every figure exactly (grouped deposit).
pub fn bounced_check_restores_balances(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 4);
    let x = mockup_client(h, s, "Bounce Neosho");
    let other_client = customer(h, s, "Other client");
    invoice(h, s, other_client, x.income, date(2026, 10, 20), "6210.00");
    let uf = clearing(h, s);
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let fees = account(h, s, AccountType::Expense, "Bank fees").id;
    let pay = receive_auto(h, s, x.client, on, "4000.00");
    let other = receive_auto(h, s, other_client, on, "6210.00");
    assert_eq!(invoice_balance(h, s, x.i1091), m("385.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("1645.00"));

    // One deposit of both checks.
    let pending = h.run(s.payments().pending_deposits()).expect("pending");
    let mut d = DepositDraft::plain(bank, date(2026, 11, 5), None, Vec::new());
    d.payments = pending
        .items
        .iter()
        .filter(|p| p.source == pay.id || p.source == other.id)
        .map(|p| p.grouped(uf).expect("exact"))
        .collect();
    assert_eq!(d.payments.len(), 2);
    assert_eq!(d.total(), Some(m("10210.00")));
    let posting = BankPosting::deposit_with(
        d,
        DepositCaps {
            grouping: true,
            cashback: h.caps.has(Capabilities::DEPOSIT_CASHBACK),
            vendor_refund: h.caps.has(Capabilities::VENDOR_REFUND),
        },
    )
    .expect("deposit");
    let deposit = h
        .run(
            s.transactions()
                .create_bank_posting(&posting, &PostingOptions::default(), key()),
        )
        .expect("grouped deposit")
        .record
        .id;
    assert_eq!(balance(h, s, bank), m("10210.00"));
    let read = h.run(s.customer_payments().get(pay.id)).expect("get");
    assert_eq!(read.deposited_in.map(|d| d.deposit_id), Some(deposit));

    // The bank returns the 4,000.00 check with a 35.00 fee.
    let k = IdempotencyKey::new();
    let req = bounce_req(date(2026, 11, 12), Some(("35.00", fees)), read.updated_at);
    let b = h
        .run(
            s.customer_payments()
                .bounce(pay.id, &req, &PostingOptions::default(), k.clone()),
        )
        .expect("bounce");
    assert_eq!(b.held_in.account_id, bank);
    assert_eq!(b.held_in.kind, HeldKind::Bank);
    assert!(b.fee_id.is_some());
    let reopened: Vec<_> = b
        .reopened
        .iter()
        .map(|r| (r.invoice_id, r.balance_due))
        .collect();
    assert_eq!(reopened, vec![(x.i1091, m("4385.00"))]);
    // Exactly: the bank keeps the other check less the fee.
    assert_eq!(balance(h, s, bank), m("6175.00"));
    assert_eq!(balance(h, s, fees), m("35.00"));
    assert_eq!(invoice_balance(h, s, x.i1091), m("4385.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("5645.00"));
    // The returned entry: Cr bank 4,000.00, Dr A/R 4,000.00.
    let t = h
        .run(s.transactions().get(b.return_id))
        .expect("get")
        .record;
    let to_bank: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.account_id == bank)
        .map(|l| (l.debit, l.credit))
        .collect();
    assert_eq!(to_bank, vec![(Money::ZERO, m("4000.00"))]);
    let dr: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.debit.is_positive())
        .map(|l| l.debit)
        .collect();
    assert_eq!(dr, vec![m("4000.00")]);
    // The bank register reads the "Bounced payment" entry (a transaction
    // type cl-api learned at this pin).
    let page = h
        .run(s.register().page(
            bank,
            &cl_api::RegisterQuery::new(CalendarDate::min(), CalendarDate::max()),
            1,
            50,
        ))
        .expect("the bank register reads the bounce");
    let returned: Vec<_> = page
        .lines
        .iter()
        .filter(|l| l.txn_type == cl_api::TxnType::ReturnedPayment)
        .map(|l| l.txn_id)
        .collect();
    assert_eq!(returned, vec![b.return_id]);
    // The other check stays deposited; the bounced one has no credit and
    // does not wait to be deposited again.
    let o = h.run(s.customer_payments().get(other.id)).expect("other");
    assert_eq!(o.deposited_in.map(|d| d.deposit_id), Some(deposit));
    assert!(o.bounce.is_none());
    let after = h.run(s.customer_payments().get(pay.id)).expect("get");
    assert!(after.is_bounced());
    assert_eq!(after.bounce.map(|r| r.return_id), Some(b.return_id));
    assert_eq!(after.unapplied_remaining, Money::ZERO);
    assert!(!pending_ids(h, s).contains(&pay.id));
    let p = crate::contract_receive_payment::preview(
        h,
        s,
        &crate::contract_receive_payment::request(x.client, on, None, None),
    );
    assert!(
        p.credits.iter().all(|c| c.source_payment_id != pay.id),
        "a bounced check is no credit"
    );

    // A replay with the same key posts once.
    let again = h
        .run(
            s.customer_payments()
                .bounce(pay.id, &req, &PostingOptions::default(), k),
        )
        .expect("replayed");
    assert_eq!(again.return_id, b.return_id);
    assert_eq!(balance(h, s, bank), m("6175.00"));
    // A new key: it bounced already.
    let e = h
        .run(s.customer_payments().bounce(
            pay.id,
            &bounce_req(date(2026, 11, 12), None, after.updated_at),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("bounced already");
    assert!(
        matches!(
            payment_refusal(&e),
            Some(PaymentRefusal::Bounced | PaymentRefusal::Stale)
        ),
        "{e:?}"
    );
    // The deposit can't be voided while the bounce stands.
    let e = h
        .run(s.transactions().void(
            deposit,
            "contract: release",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("the deposit holds a bounced check");
    assert_eq!(payment_refusal(&e), Some(PaymentRefusal::Bounced), "{e:?}");
    assert_eq!(balance(h, s, bank), m("6175.00"), "nothing changed");
}

/// A check never deposited bounces out of Undeposited Funds.
pub fn undeposited_bounce_takes_it_from_clearing(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 6);
    let x = mockup_client(h, s, "Bounce undeposited");
    let uf = clearing(h, s);
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let fees = account(h, s, AccountType::Expense, "Returned item fees").id;
    let uf_before = balance(h, s, uf);
    let pay = receive_auto(h, s, x.client, on, "4000.00");
    assert_eq!(
        balance(h, s, uf),
        uf_before.checked_add(m("4000.00")).expect("add")
    );
    assert!(pending_ids(h, s).contains(&pay.id));
    // A fee on a check that never reached a bank needs the bank it comes
    // out of.
    let e = h
        .run(s.customer_payments().bounce(
            pay.id,
            &bounce_req(date(2026, 11, 9), Some(("25.00", fees)), pay.updated_at),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("which bank pays the fee?");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::FeeBankRequired),
        "{e:?}"
    );
    let mut req = bounce_req(date(2026, 11, 9), Some(("25.00", fees)), pay.updated_at);
    req.bank_fee_bank_account_id = Some(bank);
    let b = h
        .run(
            s.customer_payments()
                .bounce(pay.id, &req, &PostingOptions::default(), key()),
        )
        .expect("bounce");
    assert_eq!(b.held_in.account_id, uf);
    assert_eq!(b.held_in.kind, HeldKind::PaymentsClearing);
    assert_eq!(balance(h, s, uf), uf_before, "Undeposited Funds as before");
    assert_eq!(balance(h, s, bank), m("-25.00"));
    assert_eq!(balance(h, s, fees), m("25.00"));
    assert_eq!(invoice_balance(h, s, x.i1091), m("4385.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("5645.00"));
    assert!(
        !pending_ids(h, s).contains(&pay.id),
        "a bounced check is not deposited"
    );
}
