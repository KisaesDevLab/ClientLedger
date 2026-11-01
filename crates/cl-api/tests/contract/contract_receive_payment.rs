//! Receive Payment against the real engine (P21 plan §3 D9, §6.2). Every
//! figure is the engine's; the tests only compare them with the golden
//! cases (test code may add and subtract to state an expectation).
//!
//! - `mockup_payment_4000_leaves_385_open` (AC1): 4,000.00 on invoices of
//!   4,385.00 and 1,260.00 goes oldest first, 385.00 stays open, the client
//!   balance goes 5,645.00 → 1,645.00, the entry is Dr Undeposited Funds /
//!   Cr A/R 4,000.00 and the payment waits to be deposited.
//! - `write_off_closes_invoice_to_chosen_account` (AC2): 4,384.25 with
//!   0.75 written off to the owner's account closes the invoice; a void
//!   reopens it to 4,385.00 and takes the 0.75 back.
//! - `overpayment_credit_apply_refund` (Q-P21b): 5,700.00 leaves a 55.00
//!   credit; `/apply` puts 30.00 on a new invoice, a refund takes 25.00, and
//!   nothing is left to apply (409 `CREDIT_NOT_AVAILABLE`).
//! - `receive_and_apply_retry_same_key_posts_once`: a replayed receive,
//!   void and apply post once.
//! - `payment_stale_409`, `invoice_balance_changed_409`,
//!   `exact_overapply_refused`, `payment_closed_period_password`,
//!   `payment_cross_company_404`.

use cl_api::{
    AccountFilter, AccountType, ApiError, ApplyInvoice, ApplyRequest, Capabilities, ConflictReason,
    CreditRequest, IdempotencyKey, InvoiceLine, Method, NewCustomerPayment, NewCustomerRefund,
    NewInvoice, PaymentPreview, PaymentPreviewRequest, PaymentRefusal, PaymentWriteOptions,
    PostingOptions, SaveApplication, SaveCredit, Session, payment_refusal,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::contract_invoices::customer;
use crate::support::Harness;

/// The tenant's Payments Clearing (Undeposited Funds) account.
pub(crate) fn clearing(h: &Harness, s: &Session) -> AccountId {
    system_account(h, s, "payments_clearing")
}

pub(crate) fn system_account(h: &Harness, s: &Session, tag: &str) -> AccountId {
    h.run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .unwrap_or_else(|| panic!("the tenant's {tag} account"))
        .id
}

pub(crate) fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or(Money::ZERO)
}

/// A one-line, untaxed invoice of exactly `amount`.
pub(crate) fn invoice(
    h: &Harness,
    s: &Session,
    c: ContactId,
    income: AccountId,
    on: CalendarDate,
    amount: &str,
) -> TransactionId {
    let line = InvoiceLine {
        is_taxable: Some(false),
        ..InvoiceLine::account(income, Quantity::parse_wire("1").expect("qty"), m(amount))
    };
    h.run(s.invoices().create(
        &NewInvoice::new(c, on, vec![line]),
        &PostingOptions::default(),
        key(),
    ))
    .unwrap_or_else(|e| panic!("create invoice: {e}"))
    .invoice
    .record
    .txn
    .id
}

/// An invoice's balance as the engine stores it.
pub(crate) fn invoice_balance(h: &Harness, s: &Session, id: TransactionId) -> Money {
    h.run(s.invoices().get(id))
        .expect("invoice")
        .record
        .balance_due
        .unwrap_or(Money::ZERO)
}

/// A preview request: Auto (no applications) unless set later.
pub(crate) fn request(
    c: ContactId,
    on: CalendarDate,
    amount: Option<&str>,
    deposit_to: Option<AccountId>,
) -> PaymentPreviewRequest {
    PaymentPreviewRequest {
        customer_id: c,
        date: on,
        amount: amount.map(m),
        deposit_to,
        applications: None,
        credit_applications: Vec::new(),
        hold_as_deposit: false,
        editing_payment_id: None,
    }
}

pub(crate) fn preview(h: &Harness, s: &Session, r: &PaymentPreviewRequest) -> PaymentPreview {
    h.run(s.customer_payments().preview(r))
        .unwrap_or_else(|e| panic!("preview: {e}"))
}

/// The client's balance as the engine reports it (a preview's "before").
pub(crate) fn client_balance(h: &Harness, s: &Session, c: ContactId, on: CalendarDate) -> Money {
    preview(h, s, &request(c, on, None, None)).contact_balance_before
}

/// The save of a preview: its amounts and balance tokens copied unchanged
/// (what the form does, Q-P21s).
pub(crate) fn save_of(
    p: &PaymentPreview,
    c: ContactId,
    on: CalendarDate,
    deposit_to: AccountId,
) -> NewCustomerPayment {
    let token = |i: TransactionId| {
        p.invoice(i)
            .map(|r| r.balance_due_exact.clone())
            .expect("the invoice is in the preview")
    };
    NewCustomerPayment {
        customer_id: c,
        date: on,
        amount: p.amount,
        deposit_to,
        payment_method: Some("check".into()),
        ref_no: None,
        memo: None,
        applications: p
            .invoices
            .iter()
            .filter(|r| {
                r.payment.is_positive() || r.discount.is_positive() || r.write_off.is_positive()
            })
            .map(|r| SaveApplication {
                invoice_id: r.invoice_id,
                amount: r.payment,
                expected_balance_due: r.balance_due_exact.clone(),
                discount_amount: r.discount.is_positive().then_some(r.discount),
                write_off_amount: r.write_off.is_positive().then_some(r.write_off),
            })
            .collect(),
        credit_applications: p
            .credit_applications
            .iter()
            .map(|u| SaveCredit {
                source_payment_id: u.source_payment_id,
                invoice_id: u.invoice_id,
                amount: u.amount,
                expected_balance_due: token(u.invoice_id),
            })
            .collect(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: false,
        bank_feed_item_id: None,
    }
}

/// Previews and receives `amount` (Auto) into Undeposited Funds.
pub(crate) fn receive_auto(
    h: &Harness,
    s: &Session,
    c: ContactId,
    on: CalendarDate,
    amount: &str,
) -> cl_api::PaymentRecord {
    let uf = clearing(h, s);
    let p = preview(h, s, &request(c, on, Some(amount), Some(uf)));
    h.run(
        s.customer_payments()
            .receive(&save_of(&p, c, on, uf), &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("receive: {e}"))
}

/// The mockup's client: invoices 1091 (4,385.00) and 1095 (1,260.00).
pub(crate) struct Mockup {
    pub client: ContactId,
    pub income: AccountId,
    pub i1091: TransactionId,
    pub i1095: TransactionId,
}

pub(crate) fn mockup_client(h: &Harness, s: &Session, name: &str) -> Mockup {
    let client = customer(h, s, name);
    let income = account(h, s, AccountType::Revenue, "Dental services").id;
    let i1091 = invoice(h, s, client, income, date(2026, 10, 1), "4385.00");
    let i1095 = invoice(h, s, client, income, date(2026, 10, 15), "1260.00");
    Mockup {
        client,
        income,
        i1091,
        i1095,
    }
}

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
        && h.require_cap(Capabilities::IDEMPOTENCY_RECEIVE_PAYMENT, "U-132")
}

/// AC1: the mockup's payment, exactly.
pub fn mockup_payment_4000_leaves_385_open(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 4);
    let x = mockup_client(h, s, "Neosho Dental");
    let uf = clearing(h, s);
    let p = preview(h, s, &request(x.client, on, Some("4000.00"), Some(uf)));
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.amount_source, cl_api::AmountSource::Typed);
    let rows: Vec<_> = p
        .invoices
        .iter()
        .map(|r| (r.invoice_id, r.payment))
        .collect();
    assert_eq!(
        rows,
        vec![(x.i1091, m("4000.00")), (x.i1095, m("0.00"))],
        "oldest first"
    );
    assert_eq!(p.totals.amount_due, m("5645.00"));
    assert_eq!(p.totals.applied, m("4000.00"));
    assert_eq!(p.totals.discounts_and_credits, m("0.00"));
    assert_eq!(p.totals.unapplied, m("0.00"));
    assert_eq!(p.shortfalls.len(), 1);
    assert_eq!(p.shortfalls[0].invoice_id, x.i1091);
    assert_eq!(p.shortfalls[0].amount, m("385.00"));
    assert_eq!(p.contact_balance_before, m("5645.00"));
    assert_eq!(p.contact_balance_after, m("1645.00"));
    assert!(!p.hold_as_deposit_offered);
    // The next step counts this payment: the same preview to a bank
    // account (which never waits) gives the others alone.
    let bank = crate::contract_bank_postings::detail_account(h, s, AccountType::Asset, "bank");
    let others = preview(h, s, &request(x.client, on, Some("4000.00"), Some(bank)))
        .pending_deposits
        .expect("pending");
    let with = p.pending_deposits.clone().expect("pending");
    assert_eq!(with.count, others.count + 1);
    assert_eq!(
        with.total,
        others.total.checked_add(m("4000.00")).expect("add")
    );

    let rec = h
        .run(s.customer_payments().receive(
            &save_of(&p, x.client, on, uf),
            &PostingOptions::default(),
            key(),
        ))
        .expect("receive");
    assert_eq!(rec.amount, m("4000.00"));
    assert_eq!(rec.deposit_to, Some(uf));
    assert_eq!(rec.unapplied_remaining, Money::ZERO);
    let live: Vec<_> = rec
        .live_applications()
        .map(|a| (a.invoice_id, a.amount))
        .collect();
    assert_eq!(live, vec![(x.i1091, m("4000.00"))]);
    assert_eq!(invoice_balance(h, s, x.i1091), m("385.00"));
    assert_eq!(invoice_balance(h, s, x.i1095), m("1260.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("1645.00"));
    let summary = h
        .run(s.reports().customer_balance_summary())
        .expect("summary");
    assert_eq!(
        summary.balance_of(x.client).and_then(|a| a.display(2)),
        Some("1645.00".into())
    );
    // The entry: Dr Undeposited Funds 4,000.00 / Cr A/R 4,000.00.
    let t = h.run(s.transactions().get(rec.id)).expect("get").record;
    let dr: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.debit.is_positive())
        .map(|l| (l.account_id, l.debit))
        .collect();
    assert_eq!(dr, vec![(uf, m("4000.00"))]);
    let cr: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| l.credit)
        .collect();
    assert_eq!(cr, vec![m("4000.00")]);
    // It waits in Undeposited Funds at exactly 4,000.00.
    let waiting = h.run(s.payments().pending_deposits()).expect("pending");
    let mine = waiting
        .items
        .iter()
        .find(|w| w.source == rec.id)
        .expect("the payment waits to be deposited");
    assert_eq!(mine.exact(), Some(m("4000.00")));
    // GET reads it back the same.
    let got = h.run(s.customer_payments().get(rec.id)).expect("get");
    assert_eq!(got.updated_at, rec.updated_at);
    assert_eq!(got.applications.len(), 1);
}

/// AC2: a write-off closes the invoice to the account the owner chose.
pub fn write_off_closes_invoice_to_chosen_account(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::PAYMENT_SETTLE, "U-133") {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 5);
    let x = mockup_client(h, s, "Write off");
    let bad_debt = account(h, s, AccountType::Expense, "Small balances").id;
    let uf = clearing(h, s);
    let mut r = request(x.client, on, Some("4384.25"), Some(uf));
    r.applications = Some(vec![cl_api::ApplicationRequest {
        invoice_id: x.i1091,
        amount: None,
        discount_amount: None,
        write_off_amount: None,
    }]);
    let p = preview(h, s, &r);
    assert_eq!(p.shortfalls.len(), 1);
    assert_eq!(p.shortfalls[0].amount, m("0.75"));
    // Write the shortfall off: the engine's figure copied.
    r.applications = Some(vec![cl_api::ApplicationRequest {
        invoice_id: x.i1091,
        amount: None,
        discount_amount: None,
        write_off_amount: Some(p.shortfalls[0].amount),
    }]);
    let p = preview(h, s, &r);
    assert!(p.shortfalls.is_empty(), "nothing left open");
    assert_eq!(p.totals.write_offs, m("0.75"));
    assert_eq!(p.contact_balance_after, m("1260.00"));
    let mut save = save_of(&p, x.client, on, uf);
    save.write_off_account_id = Some(bad_debt);
    let rec = h
        .run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), key()),
        )
        .expect("receive");
    assert_eq!(rec.write_off_account_id, Some(bad_debt));
    assert_eq!(invoice_balance(h, s, x.i1091), Money::ZERO);
    assert_eq!(balance(h, s, bad_debt), m("0.75"));
    assert_eq!(client_balance(h, s, x.client, on), m("1260.00"));
    let t = h.run(s.transactions().get(rec.id)).expect("get").record;
    let to_bad_debt: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.account_id == bad_debt)
        .map(|l| l.debit)
        .collect();
    assert_eq!(to_bad_debt, vec![m("0.75")]);
    // A void reopens the invoice to its full 4,385.00 and takes the
    // write-off back.
    h.run(s.customer_payments().void(
        rec.id,
        Some("Entered twice"),
        &PaymentWriteOptions {
            posting: PostingOptions::default(),
            expected_updated_at: Some(rec.updated_at),
        },
        key(),
    ))
    .expect("void");
    assert_eq!(invoice_balance(h, s, x.i1091), m("4385.00"));
    assert_eq!(balance(h, s, bad_debt), Money::ZERO);
    assert_eq!(client_balance(h, s, x.client, on), m("5645.00"));
}

/// Q-P21b: an overpayment is a credit; `/apply` uses it on a later
/// invoice; a refund takes the rest; then nothing is left.
pub fn overpayment_credit_apply_refund(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::PAYMENT_SETTLE, "U-133") {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 6);
    let x = mockup_client(h, s, "Overpaid");
    let uf = clearing(h, s);
    let p = preview(h, s, &request(x.client, on, Some("5700.00"), Some(uf)));
    assert_eq!(p.totals.applied, m("5645.00"));
    assert_eq!(p.totals.unapplied, m("55.00"));
    assert_eq!(p.overpayment, Some(m("55.00")));
    assert_eq!(p.contact_balance_after, m("-55.00"));
    let rec = h
        .run(s.customer_payments().receive(
            &save_of(&p, x.client, on, uf),
            &PostingOptions::default(),
            key(),
        ))
        .expect("receive");
    assert_eq!(rec.unapplied_remaining, m("55.00"));
    // The next preview lists the credit.
    let later = invoice(h, s, x.client, x.income, date(2026, 11, 7), "300.00");
    let p2 = preview(h, s, &request(x.client, date(2026, 11, 7), None, None));
    let credit = p2
        .credits
        .iter()
        .find(|c| c.source_payment_id == rec.id)
        .expect("the credit is offered");
    assert_eq!(credit.remaining, m("55.00"));
    // Apply 30.00 of it to the new invoice (the token echoed unchanged).
    let token = p2
        .invoice(later)
        .map(|r| r.balance_due_exact.clone())
        .expect("row");
    let applied = h
        .run(s.customer_payments().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 7),
                applications: vec![ApplyInvoice {
                    invoice_id: later,
                    amount: Some(m("30.00")),
                    expected_balance_due: Some(token),
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply");
    assert_eq!(applied.remaining, m("25.00"));
    assert_eq!(applied.applied.len(), 1);
    assert_eq!(applied.applied[0].amount, m("30.00"));
    assert_eq!(invoice_balance(h, s, later), m("270.00"));
    // Refund the rest from a bank account.
    let bank = crate::contract_bank_postings::detail_account(h, s, AccountType::Asset, "bank");
    h.run(s.customer_payments().refund(
        &NewCustomerRefund {
            customer_id: x.client,
            date: date(2026, 11, 8),
            from_account: bank,
            amount: m("25.00"),
            source_payment: rec.id,
            method: Some("check".into()),
            ref_no: None,
            memo: None,
        },
        &PostingOptions::default(),
        key(),
    ))
    .expect("refund");
    let after = h.run(s.customer_payments().get(rec.id)).expect("get");
    assert_eq!(after.unapplied_remaining, Money::ZERO);
    // Nothing left: 409 CREDIT_NOT_AVAILABLE, and nothing changed.
    let e = h
        .run(s.customer_payments().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 8),
                applications: vec![ApplyInvoice {
                    invoice_id: later,
                    amount: Some(m("1.00")),
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("no credit left");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::CreditNotAvailable),
        "{e:?}"
    );
    assert_eq!(invoice_balance(h, s, later), m("270.00"));
    // A credit used inside a receipt: 0 received is refused (credits alone
    // go through /apply), and the preview says so.
    let mut r = request(x.client, date(2026, 11, 8), Some("0.00"), Some(uf));
    r.credit_applications = vec![CreditRequest {
        source_payment_id: rec.id,
        invoice_id: later,
        amount: None,
    }];
    let p3 = h.run(s.customer_payments().preview(&r));
    match p3 {
        Ok(p3) => assert!(
            p3.problems
                .iter()
                .any(|x| x.code == "AMOUNT_REQUIRED" || x.code == "CREDIT_NOT_AVAILABLE"),
            "{:?}",
            p3.problems
        ),
        Err(e) => assert!(
            matches!(e, ApiError::Conflict { .. } | ApiError::Validation(_)),
            "{e:?}"
        ),
    }
}

/// A replayed receive, apply and void post once.
pub fn receive_and_apply_retry_same_key_posts_once(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::PAYMENT_SETTLE, "U-133") {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 9);
    let x = mockup_client(h, s, "Replay");
    let uf = clearing(h, s);
    let p = preview(h, s, &request(x.client, on, Some("4500.00"), Some(uf)));
    let save = save_of(&p, x.client, on, uf);
    let k = IdempotencyKey::new();
    let one = h
        .run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), k.clone()),
        )
        .expect("receive");
    let two = h
        .run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), k),
        )
        .expect("replayed");
    assert_eq!(one.id, two.id, "one payment");
    assert_eq!(invoice_balance(h, s, x.i1091), Money::ZERO);
    assert_eq!(invoice_balance(h, s, x.i1095), m("1145.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("1145.00"));

    // Apply: an overpayment's credit, replayed.
    let y = mockup_client(h, s, "Replay apply");
    let over = receive_auto(h, s, y.client, on, "5700.00");
    assert_eq!(over.unapplied_remaining, m("55.00"));
    let later = invoice(h, s, y.client, y.income, on, "100.00");
    let req = ApplyRequest {
        date: on,
        applications: vec![ApplyInvoice {
            invoice_id: later,
            amount: None,
            expected_balance_due: None,
        }],
        expected_updated_at: None,
    };
    let k = IdempotencyKey::new();
    let first = h
        .run(
            s.customer_payments()
                .apply(over.id, &req, &PostingOptions::default(), k.clone()),
        )
        .expect("apply");
    assert_eq!(first.applied[0].amount, m("55.00"), "fills the credit");
    h.run(
        s.customer_payments()
            .apply(over.id, &req, &PostingOptions::default(), k),
    )
    .expect("replayed");
    assert_eq!(invoice_balance(h, s, later), m("45.00"), "applied once");
    let src = h.run(s.customer_payments().get(over.id)).expect("get");
    assert_eq!(src.unapplied_remaining, Money::ZERO);

    // Void, replayed.
    let k = IdempotencyKey::new();
    let opts = PaymentWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: Some(one.updated_at),
    };
    h.run(s.customer_payments().void(one.id, None, &opts, k.clone()))
        .expect("void");
    h.run(s.customer_payments().void(one.id, None, &opts, k))
        .expect("replayed void");
    assert_eq!(invoice_balance(h, s, x.i1091), m("4385.00"));
    assert_eq!(client_balance(h, s, x.client, on), m("5645.00"));
}

/// An edit sent with an old `expectedUpdatedAt` is 409 STALE.
pub fn payment_stale_409(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 10);
    let x = mockup_client(h, s, "Stale payment");
    let uf = clearing(h, s);
    let rec = receive_auto(h, s, x.client, on, "1000.00");
    let mut r = request(x.client, on, Some("1000.00"), Some(uf));
    r.editing_payment_id = Some(rec.id);
    r.applications = Some(vec![cl_api::ApplicationRequest {
        invoice_id: x.i1091,
        amount: Some(m("1000.00")),
        discount_amount: None,
        write_off_amount: None,
    }]);
    let p = preview(h, s, &r);
    let mut edit = save_of(&p, x.client, on, uf);
    edit.memo = Some("first edit".into());
    let opts = PaymentWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: Some(rec.updated_at),
    };
    let first = h
        .run(s.customer_payments().update(rec.id, &edit, &opts, key()))
        .expect("first edit");
    assert_ne!(first.updated_at, rec.updated_at);
    edit.memo = Some("second edit".into());
    let e = h
        .run(s.customer_payments().update(rec.id, &edit, &opts, key()))
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
    assert_eq!(payment_refusal(&e), Some(PaymentRefusal::Stale));
}

/// A save whose invoice balance moved after the preview is 409
/// `INVOICE_BALANCE_CHANGED`; nothing posts.
pub fn invoice_balance_changed_409(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 11);
    let x = mockup_client(h, s, "Moved balance");
    let uf = clearing(h, s);
    let p = preview(h, s, &request(x.client, on, Some("500.00"), Some(uf)));
    let stale_save = save_of(&p, x.client, on, uf);
    // Another payment lands on 1091 in between.
    receive_auto(h, s, x.client, on, "100.00");
    assert_eq!(invoice_balance(h, s, x.i1091), m("4285.00"));
    let e = h
        .run(
            s.customer_payments()
                .receive(&stale_save, &PostingOptions::default(), key()),
        )
        .expect_err("the balance moved");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::InvoiceBalanceChanged),
        "{e:?}"
    );
    assert_eq!(
        invoice_balance(h, s, x.i1091),
        m("4285.00"),
        "nothing posted"
    );
    // The token is the stored balance echoed byte for byte: a fresh
    // preview's save goes through.
    let p = preview(h, s, &request(x.client, on, Some("500.00"), Some(uf)));
    h.run(s.customer_payments().receive(
        &save_of(&p, x.client, on, uf),
        &PostingOptions::default(),
        key(),
    ))
    .expect("fresh save");
    assert_eq!(invoice_balance(h, s, x.i1091), m("3785.00"));
}

/// Exact amounts (Q-P21g): one cent over the balance is refused.
pub fn exact_overapply_refused(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 12);
    let x = mockup_client(h, s, "Exact");
    let uf = clearing(h, s);
    let p = preview(h, s, &request(x.client, on, Some("4385.01"), Some(uf)));
    let mut save = save_of(&p, x.client, on, uf);
    save.applications = vec![SaveApplication {
        invoice_id: x.i1091,
        amount: m("4385.01"),
        expected_balance_due: p
            .invoice(x.i1091)
            .map(|r| r.balance_due_exact.clone())
            .expect("row"),
        discount_amount: None,
        write_off_amount: None,
    }];
    let e = h
        .run(
            s.customer_payments()
                .receive(&save, &PostingOptions::default(), key()),
        )
        .expect_err("over by a cent");
    assert_eq!(
        payment_refusal(&e),
        Some(PaymentRefusal::Overapplied),
        "{e:?}"
    );
    assert_eq!(invoice_balance(h, s, x.i1091), m("4385.00"));
}

/// A payment dated on or before the closing date needs the closing-date
/// password (tenant B). No wrong-password probe: failed attempts count
/// toward the owner's limiter budget across the whole suite.
pub fn payment_closed_period_password(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::PAYMENT_BOUNCE, "U-132") {
        return;
    }
    let s = h.s3();
    let x = mockup_client(h, &s, "Closed payment");
    let uf = clearing(h, &s);
    // An open-period payment to bounce and to take credit from later.
    let open_on = date(2026, 10, 20);
    let over = receive_auto(h, &s, x.client, open_on, "5700.00");
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-10-31"})),
    ))
    .expect("set the closing date");
    let closed_on = date(2026, 10, 25);
    let p = preview(
        h,
        &s,
        &request(x.client, closed_on, Some("10.00"), Some(uf)),
    );
    let e = h
        .run(s.customer_payments().receive(
            &save_of(&p, x.client, closed_on, uf),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let e = h
        .run(s.customer_payments().apply(
            over.id,
            &ApplyRequest {
                date: closed_on,
                applications: vec![ApplyInvoice {
                    invoice_id: x.i1091,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("apply dated in the closed period");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let e = h
        .run(s.customer_payments().bounce(
            over.id,
            &cl_api::BounceRequest {
                date: closed_on,
                bank_fee_amount: None,
                bank_fee_account_id: None,
                bank_fee_bank_account_id: None,
                memo: None,
                expected_updated_at: over.updated_at,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("bounce dated in the closed period");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let e = h
        .run(s.customer_payments().void(
            over.id,
            None,
            &PaymentWriteOptions {
                posting: PostingOptions::default(),
                expected_updated_at: Some(over.updated_at),
            },
            key(),
        ))
        .expect_err("void of a closed-period payment");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    // A bounce dated after the closing date needs no password (only the
    // bounce date is checked, Q-P21u).
    let b = h
        .run(s.customer_payments().bounce(
            over.id,
            &cl_api::BounceRequest {
                date: date(2026, 11, 2),
                bank_fee_amount: None,
                bank_fee_account_id: None,
                bank_fee_bank_account_id: None,
                memo: None,
                expected_updated_at: over.updated_at,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("bounce after the closing date");
    assert_eq!(b.held_in.kind, cl_api::HeldKind::PaymentsClearing);
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
    assert_eq!(invoice_balance(h, &s, x.i1091), m("4385.00"));
}

/// Another company's payment and client are 404 (company scope).
pub fn payment_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let on = date(2026, 11, 13);
    let x = mockup_client(h, s, "Scoped");
    let rec = receive_auto(h, s, x.client, on, "100.00");
    let b = h.s3();
    let e = h
        .run(b.customer_payments().get(rec.id))
        .expect_err("another company's payment");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.customer_payments().open_invoices(x.client))
        .expect_err("another company's client");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(
            b.customer_payments()
                .void(rec.id, None, &PaymentWriteOptions::default(), key()),
        )
        .expect_err("void across companies");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    assert_eq!(invoice_balance(h, s, x.i1091), m("4285.00"));
}
