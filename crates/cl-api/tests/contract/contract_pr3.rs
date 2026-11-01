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
//! - `ar_aging_ties_to_balance_sheet` (review missing test 4): on 10/31,
//!   11/12 and 11/30 the A/R aging total (exact total, client rows and
//!   buckets) equals the Balance Sheet's A/R after every step of a
//!   payment, a memo kept then applied later, an unapply, a bounce, an
//!   invoice voided after its bounce (B2), a bounce voided and a refund.

use cl_api::{
    AccountType, ApplicationRequest, ApplyInvoice, ApplyRequest, BounceRequest, Capabilities,
    CreditWriteOptions, EmailMessage, InvoiceLine, InvoiceRefusal, InvoiceStatus, NewCreditMemo,
    NewInvoice, NewRefund, NewSalesReceipt, OutboxKind, PaymentMethod, PaymentRecord,
    PostingOptions, RefundCheck, RefundMethod, RefundSource, ReportBasis, Session, invoice_refusal,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity};

use crate::common::{account, date, key, m};
use crate::contract_bank_postings::detail_account;
use crate::contract_credit_memos::create;
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{
    clearing, invoice, invoice_balance, preview, receive_auto, request, save_of, system_account,
};
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
    assert_eq!(read.send_history.len(), 0);
    let read = h.run(s.sales_receipts().get(receipt.id)).expect("receipt");
    assert_eq!(read.send_history.len(), 0);
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

/// The three dates the A/R aging is tied on: before, between and after the
/// steps below.
fn tie_dates() -> [CalendarDate; 3] {
    [date(2026, 10, 31), date(2026, 11, 12), date(2026, 11, 30)]
}

/// A report call; the engine allows 30 report requests a minute per user
/// (`expensiveOpLimiter`), so a 429 waits it out once.
fn report<T, F>(h: &Harness, call: impl Fn() -> F) -> T
where
    F: std::future::Future<Output = Result<T, cl_api::ApiError>>,
{
    match h.run(call()) {
        Err(cl_api::ApiError::RateLimited { retry_after }) => {
            std::thread::sleep(
                retry_after.unwrap_or(std::time::Duration::from_secs(60))
                    + std::time::Duration::from_secs(1),
            );
            h.run(call()).unwrap_or_else(|e| panic!("report: {e}"))
        }
        r => r.unwrap_or_else(|e| panic!("report: {e}")),
    }
}

/// On every tie date: the A/R aging's exact total, the sum of its client
/// rows and the sum of its buckets all equal the Balance Sheet's A/R
/// (accrual). The suite's other A/R in this company counts too, which is
/// the point: the two reports must agree whatever is on the books.
fn tie(h: &Harness, s: &Session, step: &str) {
    let ar = system_account(h, s, "accounts_receivable");
    for on in tie_dates() {
        let bs = report(h, || async move {
            s.reports().balance_sheet(on, ReportBasis::Accrual).await
        });
        let gl = bs
            .assets
            .iter()
            .find(|e| e.account_id == Some(ar))
            .map_or(Money::ZERO, |e| {
                let shown = e.balance.display(4).expect("a number");
                Money::parse_wire(&shown).expect("money")
            });
        let aging = report(h, || async move { s.reports().ar_aging_summary(on).await });
        assert_eq!(aging.total_exact, gl, "{step}: aging total on {on:?}");
        let rows = Money::sum(aging.customers.iter().map(|r| r.total)).expect("sum");
        assert_eq!(rows, gl, "{step}: client rows on {on:?}");
        let b = &aging.buckets_exact;
        let buckets = Money::sum([
            b.current,
            b.days_1_30,
            b.days_31_60,
            b.days_61_90,
            b.over_90,
        ])
        .expect("sum");
        assert_eq!(buckets, gl, "{step}: buckets on {on:?}");
    }
}

/// Receives `amount` on `on` applied to `inv` only, into Undeposited Funds.
fn pay_on(
    h: &Harness,
    s: &Session,
    c: ContactId,
    on: CalendarDate,
    inv: TransactionId,
    amount: &str,
) -> PaymentRecord {
    let uf = clearing(h, s);
    let mut r = request(c, on, Some(amount), Some(uf));
    r.applications = Some(vec![ApplicationRequest {
        invoice_id: inv,
        amount: Some(m(amount)),
        discount_amount: None,
        write_off_amount: None,
    }]);
    let p = preview(h, s, &r);
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    h.run(
        s.customer_payments()
            .receive(&save_of(&p, c, on, uf), &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("receive: {e}"))
}

/// Bounces a check that was never deposited (no fee); answers the
/// returned-payment entry.
fn bounce(h: &Harness, s: &Session, pay: TransactionId, on: CalendarDate) -> TransactionId {
    let read = h.run(s.customer_payments().get(pay)).expect("payment");
    let req = BounceRequest {
        date: on,
        bank_fee_amount: None,
        bank_fee_account_id: None,
        bank_fee_bank_account_id: None,
        memo: None,
        expected_updated_at: read.updated_at,
    };
    h.run(
        s.customer_payments()
            .bounce(pay, &req, &PostingOptions::default(), key()),
    )
    .expect("bounce")
    .return_id
}

fn void(h: &Harness, s: &Session, id: TransactionId) {
    h.run(
        s.transactions()
            .void(id, "contract: aging tie", &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("void: {e}"));
}

/// PR-3 review missing test 4: A/R aging = Balance Sheet A/R on three
/// dates through apply later, unapply, bounce, bounce void and refund.
/// Tied at four checkpoints, not after every step: 24 report requests
/// stay under the engine's 30 a minute.
pub fn ar_aging_ties_to_balance_sheet(h: &Harness) {
    if !crate::contract_credit_memos::caps(h)
        || !h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
        || !h.require_cap(Capabilities::PAYMENT_BOUNCE, "U-132")
        || !h.require_cap(Capabilities::REFUND_CREDIT_MEMO, "U-139")
        || !h.require_cap(Capabilities::AR_AGING_EXACT, "U-142")
        || !h.require_cap(Capabilities::AR_OPEN_ITEMS_ASOF, "U-142")
    {
        return;
    }
    let s = &h.s1;
    let c = customer(h, s, "Aging tie");
    let income = account(h, s, AccountType::Revenue, "Aging tie sales").id;
    let i1 = invoice(h, s, c, income, date(2026, 10, 1), "300.00");
    let i2 = invoice(h, s, c, income, date(2026, 10, 10), "200.00");

    // A payment on 10/20: 250.00 to the first invoice.
    let p1 = pay_on(h, s, c, date(2026, 10, 20), i1, "250.00");
    assert_eq!(invoice_balance(h, s, i1), m("50.00"));

    // A credit memo kept as credit on 10/25, applied later on 11/05.
    let memo = create(
        h,
        s,
        &NewCreditMemo::new(c, date(2026, 10, 25), vec![line(income, "120.00")]),
    );
    assert_eq!(memo.remaining, m("120.00"), "kept as credit");
    let applied = h
        .run(s.credit_memos().apply(
            memo.id,
            &ApplyRequest {
                date: date(2026, 11, 5),
                applications: vec![ApplyInvoice {
                    invoice_id: i2,
                    amount: Some(m("80.00")),
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply later");
    assert_eq!(applied.remaining, m("40.00"));
    assert_eq!(invoice_balance(h, s, i2), m("120.00"));
    tie(h, s, "a payment and a memo applied later");

    // Unapply: the credit comes back whole.
    let read = h.run(s.credit_memos().get(memo.id)).expect("memo");
    let opts = CreditWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: Some(read.updated_at),
    };
    let restored = h
        .run(s.credit_memos().unapply(memo.id, i2, &opts, key()))
        .expect("unapply");
    assert_eq!(restored, m("120.00"));
    assert_eq!(invoice_balance(h, s, i2), m("200.00"));

    // A second check pays the second invoice on 11/03 and bounces on
    // 11/08; the invoice is then voided (B2: allowed, its payment is gone).
    let p2 = pay_on(h, s, c, date(2026, 11, 3), i2, "200.00");
    assert_eq!(invoice_balance(h, s, i2), Money::ZERO);
    bounce(h, s, p2.id, date(2026, 11, 8));
    assert_eq!(invoice_balance(h, s, i2), m("200.00"));
    tie(h, s, "an unapply and a bounce");
    void(h, s, i2);
    tie(h, s, "invoice voided after its bounce");

    // The first check bounces on 11/09; voiding the bounce re-applies it.
    let returned = bounce(h, s, p1.id, date(2026, 11, 9));
    assert_eq!(invoice_balance(h, s, i1), m("300.00"));
    void(h, s, returned);
    assert_eq!(invoice_balance(h, s, i1), m("50.00"));

    // The memo's credit goes back as cash on 11/15.
    let bank = detail_account(h, s, AccountType::Asset, "bank");
    h.run(s.customer_refunds().create(
        &NewRefund {
            customer_id: c,
            date: date(2026, 11, 15),
            from_account: bank,
            amount: m("120.00"),
            source: RefundSource::CreditMemo(memo.id),
            method: RefundMethod::Cash,
            check: RefundCheck::NotACheck,
            ref_no: None,
            memo: None,
        },
        &PostingOptions::default(),
        key(),
    ))
    .expect("refund");
    tie(h, s, "a bounce voided and a refund");
}
