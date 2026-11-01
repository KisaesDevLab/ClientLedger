//! PR-3 checkpoint fixes against the real engine (engine PRs #214, #215;
//! U-144 to U-146). Each test is PENDING by its capability.
//!
//! - `sales_doc_email_no_smtp_and_outbox_kind` (U-144): a credit memo and
//!   a sales receipt email on company B (no mail server) is
//!   `SMTP_NOT_CONFIGURED` (or offline); email-later is an Outbox row of
//!   kind `credit_memo` / `sales_receipt` naming the document.
//! - `invoice_client_guard_409` (U-146 B1): changing the client of an
//!   invoice with a live payment is 409 `INVOICE_HAS_PAYMENTS` with
//!   `{payments: 1, credits: 0}` and nothing changes; an edit down to
//!   exactly the amount paid makes it paid (N2).

use cl_api::{
    AccountType, Capabilities, EmailMessage, InvoiceLine, InvoiceRefusal, InvoiceStatus,
    NewCreditMemo, NewInvoice, NewSalesReceipt, OutboxKind, PaymentMethod, PostingOptions,
    invoice_refusal,
};
use cl_core::{Money, Quantity};

use crate::common::{account, date, key, m};
use crate::contract_credit_memos::create;
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{clearing, invoice, receive_auto};
use crate::support::Harness;

fn one() -> Quantity {
    Quantity::parse_wire("1").expect("qty")
}

fn line(a: cl_core::ids::AccountId, amount: &str) -> InvoiceLine {
    InvoiceLine {
        is_taxable: Some(false),
        ..InvoiceLine::account(a, one(), m(amount))
    }
}

fn message() -> EmailMessage {
    EmailMessage {
        to: vec!["ap@example.test".into()],
        subject: "Your document".into(),
        body: "Attached.".into(),
        ..EmailMessage::default()
    }
}

/// U-144 on company B, which has no mail server.
pub fn sales_doc_email_no_smtp_and_outbox_kind(h: &Harness) {
    if !h.require_cap(Capabilities::SALES_DOC_EMAIL, "U-144")
        || !h.require_cap(Capabilities::CREDIT_MEMO_EDIT, "U-138")
        || !h.require_cap(Capabilities::SALES_RECEIPT_LINES, "U-143")
    {
        return;
    }
    let s = h.s3();
    let c = customer(h, &s, "Emailed credit");
    let income = account(h, &s, AccountType::Revenue, "Email sales").id;
    let memo = create(
        h,
        &s,
        &NewCreditMemo::new(c, date(2026, 11, 5), vec![line(income, "25.00")]),
    );
    let mut r = NewSalesReceipt::new(date(2026, 11, 5), vec![line(income, "30.00")]);
    r.contact_id = Some(c);
    r.deposit_to_account_id = Some(clearing(h, &s));
    r.payment_method = Some(PaymentMethod::Cash);
    let receipt = h
        .run(
            s.sales_receipts()
                .create(&r, &PostingOptions::default(), key()),
        )
        .expect("receipt");
    let pdf = b"%PDF-1.4\n%%EOF\n";
    let no_smtp = |e: &cl_api::ApiError| {
        matches!(
            invoice_refusal(e),
            Some(InvoiceRefusal::SmtpNotConfigured | InvoiceRefusal::Offline)
        )
    };
    let e = h
        .run(
            s.credit_memos()
                .email(memo.id, &message(), "CreditMemo-1.pdf", pdf, None, key()),
        )
        .expect_err("no SMTP");
    assert!(no_smtp(&e), "{e:?}");
    let e = h
        .run(s.sales_receipts().email(
            receipt.id,
            &message(),
            "SalesReceipt-1.pdf",
            pdf,
            None,
            key(),
        ))
        .expect_err("no SMTP");
    assert!(no_smtp(&e), "{e:?}");
    // A failed send records nothing.
    let read = h.run(s.credit_memos().get(memo.id)).expect("memo");
    assert!(read.send_history.is_empty());
    let read = h.run(s.sales_receipts().get(receipt.id)).expect("receipt");
    assert!(read.send_history.is_empty());
    // Email later: Outbox rows of each kind; the queue lists them.
    let q1 = h
        .run(s.credit_memos().email_later(memo.id, &message(), key()))
        .expect("outbox");
    assert_eq!(q1.kind, OutboxKind::CreditMemo);
    assert_eq!(q1.transaction_id, Some(memo.id));
    let q2 = h
        .run(
            s.sales_receipts()
                .email_later(receipt.id, &message(), key()),
        )
        .expect("outbox");
    assert_eq!(q2.kind, OutboxKind::SalesReceipt);
    assert_eq!(q2.transaction_id, Some(receipt.id));
    let listed = h.run(s.invoices().outbox()).expect("outbox list");
    for q in [&q1, &q2] {
        let row = listed.iter().find(|i| i.id == q.id).expect("listed");
        assert_eq!(row.kind, q.kind);
    }
    for q in [q1, q2] {
        h.run(s.invoices().remove_from_outbox(q.id))
            .expect("removed");
    }
}

/// U-146 B1 and N2.
pub fn invoice_client_guard_409(h: &Harness) {
    if !h.require_cap(Capabilities::INVOICE_CLIENT_GUARD, "U-146")
        || !h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
    {
        return;
    }
    let s = &h.s1;
    let a = customer(h, s, "Guard A");
    let b = customer(h, s, "Guard B");
    let income = account(h, s, AccountType::Revenue, "Guard sales").id;
    let on = date(2026, 11, 2);
    let id = invoice(h, s, a, income, on, "100.00");
    receive_auto(h, s, a, date(2026, 11, 3), "40.00");
    let read = h.run(s.invoices().get(id)).expect("invoice").record;
    assert_eq!(read.amount_paid, Some(m("40.00")));
    let moved = NewInvoice::new(b, on, vec![line(income, "100.00")]);
    let e = h
        .run(
            s.invoices()
                .update(id, &read, &moved, &PostingOptions::default(), key()),
        )
        .expect_err("a paid invoice keeps its client");
    assert_eq!(
        invoice_refusal(&e),
        Some(InvoiceRefusal::HasPayments),
        "{e:?}"
    );
    let cl_api::ApiError::Conflict { details, .. } = &e else {
        panic!("409: {e:?}");
    };
    assert_eq!(details.count("payments"), Some(1));
    assert_eq!(details.count("credits"), Some(0));
    let after = h.run(s.invoices().get(id)).expect("unchanged").record;
    assert_eq!(after.txn.contact_id, Some(a));
    assert_eq!(after.balance_due, Some(m("60.00")));
    // N2: the same client, the total down to exactly what was paid: paid.
    let down = NewInvoice::new(a, on, vec![line(income, "40.00")]);
    let saved = h
        .run(
            s.invoices()
                .update(id, &after, &down, &PostingOptions::default(), key()),
        )
        .expect("same-client edit");
    assert_eq!(saved.invoice.record.balance_due, Some(Money::ZERO));
    assert_eq!(
        saved.invoice.record.invoice_status,
        Some(InvoiceStatus::Paid)
    );
}
