//! The register's A/P void guard against the live engine (P17 `WP2b`, plan
//! review B1): a paid bill voided through its own route is refused and its
//! payment link stays; the bill payment's own void takes the link off and
//! the bill is owed again; then the bill's void goes through.
//!
//! The payment is made with a raw `POST /bill-payments` (Pay Bills arrives
//! in P18).

use cl_api::{
    AccountType, ApiError, BillLine, ContactFields, ContactType, Method, NewBill, NewContact,
    Session,
};
use cl_core::Money;
use cl_core::ids::TransactionId;
use serde_json::{Value, json};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::support::{Harness, unique};

fn bill_state(h: &Harness, s: &Session, id: TransactionId) -> Value {
    let reply = h
        .run(s.raw(Method::Get, &format!("/api/v1/bills/{id}"), &[], None))
        .expect("raw bill");
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.body["bill"].clone()
}

/// Void a paid bill through the guarded path: refused, the payment links
/// remain; void the payment through its own route, then the bill.
pub fn paid_bill_void_is_refused_and_links_remain(h: &Harness) {
    let s = &h.s1;
    let expense = account(h, s, AccountType::Expense, "Hay");
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    let vendor = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, unique("Feed Co")),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record;
    let bill = h
        .run(s.create_bill(
            &NewBill {
                contact_id: vendor.id,
                txn_date: date(2026, 10, 6),
                due_date: Some(date(2099, 11, 5)),
                vendor_invoice_number: Some(unique("INV")),
                memo: None,
                lines: vec![BillLine {
                    account_id: expense.id,
                    description: None,
                    amount: m("148.0000"),
                }],
            },
            key(),
        ))
        .expect("bill")
        .id;
    let paid = h
        .run(s.raw(
            Method::Post,
            "/api/v1/bill-payments",
            &[],
            Some(&json!({
                "bankAccountId": bank.to_string(),
                "txnDate": "2026-10-07",
                "method": "ach",
                "bills": [{ "billId": bill.to_string(), "amount": "148.00" }],
            })),
        ))
        .expect("raw pay");
    assert_eq!(paid.status, 201, "{}", paid.body);
    let payment = paid.body["payments"][0]["id"]
        .as_str()
        .and_then(|v| TransactionId::parse(v).ok())
        .expect("payment id");
    assert_eq!(bill_state(h, s, bill)["billStatus"], "paid");

    // The guarded path refuses the paid bill; nothing about it changes.
    let e = h
        .run(s.ap_voids().bill(bill, "Voided in the register", key()))
        .expect_err("a paid bill is not voided");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    let after = bill_state(h, s, bill);
    assert_eq!(after["status"], "posted", "{after}");
    assert_eq!(after["billStatus"], "paid", "{after}");
    let got = h
        .run(s.raw(
            Method::Get,
            &format!("/api/v1/bill-payments/{payment}"),
            &[],
            None,
        ))
        .expect("raw payment");
    let apps = got.body["payment"]["billApplications"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(apps.len(), 1, "the payment link remains: {}", got.body);
    assert_eq!(apps[0]["billId"], bill.to_string());

    // The payment's own void takes the link off: the bill is owed again.
    h.run(
        s.ap_voids()
            .bill_payment(payment, "Voided in the register", key()),
    )
    .expect("payment voided");
    let reopened = bill_state(h, s, bill);
    assert_eq!(reopened["billStatus"], "unpaid", "{reopened}");
    assert!(
        reopened["balanceDue"].is_string(),
        "money is a string: {reopened}"
    );
    assert_eq!(
        Money::parse_wire(reopened["balanceDue"].as_str().unwrap_or("")).ok(),
        Some(m("148.0000"))
    );
    let voided = h
        .run(s.transactions().get(payment))
        .expect("payment read")
        .record;
    assert_eq!(voided.status, cl_api::TxnStatus::Void);

    // Now the bill's own void goes through.
    h.run(s.ap_voids().bill(bill, "Voided in the register", key()))
        .expect("bill voided");
    assert_eq!(bill_state(h, s, bill)["status"], "void");
}
