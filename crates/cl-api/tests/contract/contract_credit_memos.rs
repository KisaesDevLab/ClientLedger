//! Credit memos against the real engine (P22 plan §3 D8, §6.1, §6.2).
//! Every figure is the engine's; the tests compare them with the golden
//! cases (`fixtures/receivables/golden.json`). Test code may add and
//! subtract to state an expectation.
//!
//! - `credit_memo_reduces_ar_exactly` (AC1, golden 1-2): items and tax at
//!   8.25 %, applied at save oldest first (listed oldest invoice first
//!   whatever order they were sent in), A/R down by exactly 399.75.
//! - `credit_memo_keep_then_apply_later` (Q-P22b, golden 4): kept as
//!   credit, `/apply` later, `/unapply`, a replayed apply posts once, a
//!   void reopens the invoice; a PUT carrying applications is 400.
//! - `credit_date_rules`: an apply dated before the memo is refused.
//! - `credit_memo_stale_409`, `credit_memo_closed_period_password`,
//!   `credit_memo_cross_company_404`.

use cl_api::{
    AccountType, ApiError, ApplyInvoice, ApplyRequest, Capabilities, ConflictReason,
    CreditMemoApplication, CreditMemoPreview, CreditMemoRecord, CreditRefusal, CreditWriteOptions,
    InvoiceLine, ItemFields, Method, NewCreditMemo, PostingOptions, Session, credit_refusal,
};
use cl_core::ids::{AccountId, ContactId, ItemId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity};
use serde_json::json;

use crate::common::{account, date, key, m};
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{balance, client_balance, invoice_balance, system_account};
use crate::support::{Harness, unique};

pub(crate) fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::CREDIT_MEMO_APPLY, "U-137")
        && h.require_cap(Capabilities::IDEMPOTENCY_CREDIT_MEMO, "U-137")
        && h.require_cap(Capabilities::CREDIT_MEMO_EDIT, "U-138")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

/// Sets the company's sales tax rate and answers the one it replaces, so
/// the test can put it back (other tests set the rate they need too).
pub(crate) fn set_tax_rate(h: &Harness, s: &Session, rate: &str) -> serde_json::Value {
    let before = h
        .run(s.raw_mapped(Method::Get, "/api/v1/company/settings", None))
        .expect("settings");
    let old = before
        .pointer("/settings/defaultSalesTaxRate")
        .or_else(|| before.get("defaultSalesTaxRate"))
        .cloned()
        .unwrap_or(json!("0.0810"));
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"defaultSalesTaxRate": rate})),
    ))
    .expect("set the tax rate");
    old
}

pub(crate) fn restore_tax_rate(h: &Harness, s: &Session, old: &serde_json::Value) {
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"defaultSalesTaxRate": old})),
    ))
    .expect("restore the tax rate");
}

/// Golden seed: a client with invoice A 250.00 (09/15, due 10/15) and
/// invoice B 500.00 (10/20, due 11/19), untaxed.
pub(crate) struct Seed {
    pub client: ContactId,
    pub income: AccountId,
    pub a: TransactionId,
    pub b: TransactionId,
}

pub(crate) fn dated_invoice(
    h: &Harness,
    s: &Session,
    c: ContactId,
    income: AccountId,
    on: CalendarDate,
    due: CalendarDate,
    amount: &str,
) -> TransactionId {
    let line = InvoiceLine {
        is_taxable: Some(false),
        ..InvoiceLine::account(income, q("1"), m(amount))
    };
    let mut new = cl_api::NewInvoice::new(c, on, vec![line]);
    new.due_date = Some(due);
    h.run(s.invoices().create(&new, &PostingOptions::default(), key()))
        .unwrap_or_else(|e| panic!("create invoice: {e}"))
        .invoice
        .record
        .txn
        .id
}

pub(crate) fn seed(h: &Harness, s: &Session, name: &str) -> Seed {
    let client = customer(h, s, name);
    let income = account(h, s, AccountType::Revenue, "Veterinary").id;
    let a = dated_invoice(
        h,
        s,
        client,
        income,
        date(2026, 9, 15),
        date(2026, 10, 15),
        "250.00",
    );
    let b = dated_invoice(
        h,
        s,
        client,
        income,
        date(2026, 10, 20),
        date(2026, 11, 19),
        "500.00",
    );
    Seed {
        client,
        income,
        a,
        b,
    }
}

fn item(h: &Harness, s: &Session, income: AccountId, name: &str, price: &str, tax: bool) -> ItemId {
    h.run(s.items().create(
        &ItemFields {
            name: unique(name),
            description: None,
            unit_price: Some(m(price)),
            income_account_id: income,
            is_taxable: tax,
            item_type: cl_api::ItemType::Service,
            discount_percent: None,
        },
        key(),
    ))
    .expect("item")
    .record
    .id
}

/// Golden case 1's memo with items: 2 x 150.00 taxable + 1 x 75.00 not,
/// dated 11/02.
pub(crate) fn case1(h: &Harness, s: &Session, x: &Seed) -> NewCreditMemo {
    let exam = item(h, s, x.income, "Exam", "150.00", true);
    let boarding = item(h, s, x.income, "Boarding", "75.00", false);
    NewCreditMemo::new(
        x.client,
        date(2026, 11, 2),
        vec![
            InvoiceLine::item(exam, q("2")),
            InvoiceLine::item(boarding, q("1")),
        ],
    )
}

pub(crate) fn preview(h: &Harness, s: &Session, memo: &NewCreditMemo) -> CreditMemoPreview {
    h.run(s.credit_memos().preview(memo, None))
        .unwrap_or_else(|e| panic!("credit memo preview: {e}"))
}

pub(crate) fn create(h: &Harness, s: &Session, memo: &NewCreditMemo) -> CreditMemoRecord {
    h.run(
        s.credit_memos()
            .create(memo, &PostingOptions::default(), key()),
    )
    .unwrap_or_else(|e| panic!("create credit memo: {e}"))
}

/// The preview's suggested applications with their balance tokens, copied
/// unchanged (what the form does).
fn applications_of(p: &CreditMemoPreview) -> Vec<CreditMemoApplication> {
    p.suggested_applications
        .iter()
        .map(|a| CreditMemoApplication {
            invoice_id: a.invoice_id,
            amount: a.amount,
            expected_balance_due: p
                .open_invoice(a.invoice_id)
                .expect("open invoice")
                .balance_due_exact
                .clone(),
        })
        .collect()
}

fn opts(updated_at: cl_core::Timestamp) -> CreditWriteOptions {
    CreditWriteOptions {
        posting: PostingOptions::default(),
        expected_updated_at: Some(updated_at),
    }
}

/// AC1, golden 1-2: the memo's figures, apply at save oldest first, and
/// A/R down by exactly the memo's total.
pub fn credit_memo_reduces_ar_exactly(h: &Harness) {
    if !caps(h) || !h.require_cap(Capabilities::AR_OPEN_ITEMS_ASOF, "U-142") {
        return;
    }
    let s = &h.s1;
    let old_rate = set_tax_rate(h, s, "0.0825");
    let x = seed(h, s, "Bluestem exact");
    let on = date(2026, 11, 2);
    assert_eq!(client_balance(h, s, x.client, on), m("750.00"));
    let ar = system_account(h, s, "accounts_receivable");
    let ar_before = balance(h, s, ar);
    let mut memo = case1(h, s, &x);
    let p = preview(h, s, &memo);
    assert!(p.problems.is_empty(), "{:?}", p.problems);
    assert_eq!(p.figures.subtotal, m("375.00"));
    assert_eq!(p.figures.tax_total, m("24.75"), "round2(300.00 x 8.25 %)");
    assert_eq!(p.figures.total, m("399.75"));
    let suggested: Vec<_> = p
        .suggested_applications
        .iter()
        .map(|a| (a.invoice_id, a.amount))
        .collect();
    assert_eq!(
        suggested,
        vec![(x.a, m("250.00")), (x.b, m("149.75"))],
        "oldest first"
    );
    assert_eq!(p.remaining_credit, Money::ZERO);
    assert_eq!(p.figures.contact_balance_after, Some(m("350.25")));
    // Sent newest first: the engine lists them oldest invoice first.
    memo.applications = applications_of(&p);
    memo.applications.reverse();
    let rec = create(h, s, &memo);
    assert_eq!(rec.total, m("399.75"));
    assert_eq!(rec.subtotal, m("375.00"));
    assert_eq!(rec.tax_amount, m("24.75"));
    assert_eq!(rec.remaining, Money::ZERO);
    let applied: Vec<_> = rec
        .applications
        .iter()
        .map(|a| (a.invoice_id, a.amount, a.applied_date, a.voided))
        .collect();
    assert_eq!(
        applied,
        vec![
            (x.a, m("250.00"), None, false),
            (x.b, m("149.75"), None, false)
        ]
    );
    assert_eq!(invoice_balance(h, s, x.a), Money::ZERO);
    assert_eq!(invoice_balance(h, s, x.b), m("350.25"));
    assert_eq!(client_balance(h, s, x.client, on), m("350.25"));
    // A/R moves by exactly 399.75.
    assert_eq!(
        balance(h, s, ar),
        ar_before.checked_sub(m("399.75")).expect("sub")
    );
    // The entry: Cr A/R 399.75; Dr income 375.00 and sales tax 24.75.
    let t = h.run(s.transactions().get(rec.id)).expect("get").record;
    let cr: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| (l.account_id, l.credit))
        .collect();
    assert_eq!(cr, vec![(ar, m("399.75"))]);
    let income: Money = t
        .lines
        .iter()
        .filter(|l| l.account_id == x.income)
        .fold(Money::ZERO, |acc, l| acc.checked_add(l.debit).expect("add"));
    assert_eq!(income, m("375.00"));
    let other_dr: Vec<_> = t
        .lines
        .iter()
        .filter(|l| l.debit.is_positive() && l.account_id != x.income)
        .map(|l| l.debit)
        .collect();
    assert_eq!(other_dr, vec![m("24.75")], "sales tax payable");
    // GET reads it back the same.
    let got = h.run(s.credit_memos().get(rec.id)).expect("get");
    assert_eq!(got.total, rec.total);
    assert_eq!(got.updated_at, rec.updated_at);
    assert_eq!(got.applications.len(), 2);
    restore_tax_rate(h, s, &old_rate);
}

/// Q-P22b and golden 4: kept as credit, applied later, unapplied, a
/// replayed apply posts once, a PUT with applications is 400, a void
/// reopens what it settled.
pub fn credit_memo_keep_then_apply_later(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let old_rate = set_tax_rate(h, s, "0.0825");
    let x = seed(h, s, "Bluestem keep");
    let memo = case1(h, s, &x);
    let rec = create(h, s, &memo);
    assert_eq!(rec.remaining, m("399.75"), "kept as credit");
    assert!(rec.applications.is_empty());
    assert_eq!(invoice_balance(h, s, x.a), m("250.00"));
    // The credit is listed for the client.
    let credits = h.run(s.credit_memos().credits(x.client)).expect("credits");
    let mine = credits
        .iter()
        .find(|c| c.source_id == rec.id)
        .expect("the memo is an open credit");
    assert_eq!(mine.kind, cl_api::CreditKind::CreditMemo);
    assert_eq!(mine.remaining, m("399.75"));
    assert_eq!(mine.total, m("399.75"));
    // A PUT carrying applications: 400 (a saved memo's credit is used
    // through /apply).
    let mut put = memo.clone();
    put.applications = vec![CreditMemoApplication {
        invoice_id: x.b,
        amount: m("1.00"),
        expected_balance_due: h
            .run(s.credit_memos().preview(&memo, Some(rec.id)))
            .expect("preview")
            .open_invoice(x.b)
            .expect("open")
            .balance_due_exact
            .clone(),
    }];
    let e = h
        .run(
            s.credit_memos()
                .update(rec.id, &put, &opts(rec.updated_at), key()),
        )
        .expect_err("applications on a PUT");
    assert!(matches!(e, ApiError::Validation(_)), "{e:?}");
    // Apply 200.00 to B later; a replay posts once.
    let req = ApplyRequest {
        date: date(2026, 11, 5),
        applications: vec![ApplyInvoice {
            invoice_id: x.b,
            amount: Some(m("200.00")),
            expected_balance_due: None,
        }],
        expected_updated_at: None,
    };
    let k = cl_api::IdempotencyKey::new();
    let one = h
        .run(
            s.credit_memos()
                .apply(rec.id, &req, &PostingOptions::default(), k.clone()),
        )
        .expect("apply");
    assert_eq!(one.remaining, m("199.75"));
    assert_eq!(one.applied.len(), 1);
    assert_eq!(one.applied[0].amount, m("200.00"));
    h.run(
        s.credit_memos()
            .apply(rec.id, &req, &PostingOptions::default(), k),
    )
    .expect("replayed apply");
    assert_eq!(invoice_balance(h, s, x.b), m("300.00"), "applied once");
    let got = h.run(s.credit_memos().get(rec.id)).expect("get");
    assert_eq!(got.remaining, m("199.75"));
    let live: Vec<_> = got
        .applications
        .iter()
        .filter(|a| !a.voided)
        .map(|a| (a.invoice_id, a.amount, a.applied_date))
        .collect();
    assert_eq!(live, vec![(x.b, m("200.00"), Some(date(2026, 11, 5)))]);
    // Unapply: the credit comes back exactly.
    let remaining = h
        .run(
            s.credit_memos()
                .unapply(rec.id, x.b, &opts(got.updated_at), key()),
        )
        .expect("unapply");
    assert_eq!(remaining, m("399.75"));
    assert_eq!(invoice_balance(h, s, x.b), m("500.00"));
    // Fill: amount None takes min(remaining, balance), oldest given first.
    let fill = h
        .run(s.credit_memos().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 6),
                applications: vec![
                    ApplyInvoice {
                        invoice_id: x.a,
                        amount: None,
                        expected_balance_due: None,
                    },
                    ApplyInvoice {
                        invoice_id: x.b,
                        amount: None,
                        expected_balance_due: None,
                    },
                ],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply all");
    assert_eq!(fill.remaining, Money::ZERO);
    assert_eq!(invoice_balance(h, s, x.a), Money::ZERO);
    assert_eq!(invoice_balance(h, s, x.b), m("350.25"));
    // Nothing left: one cent more is 409 CREDIT_NOT_AVAILABLE.
    let e = h
        .run(s.credit_memos().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 6),
                applications: vec![ApplyInvoice {
                    invoice_id: x.b,
                    amount: Some(m("0.01")),
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("no credit left");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::CreditNotAvailable),
        "{e:?}"
    );
    // Void the memo: both invoices reopen, the client owes 750.00 again.
    h.run(s.transactions().void(
        rec.id,
        "contract: entered in error",
        &PostingOptions::default(),
        key(),
    ))
    .expect("void the memo");
    assert_eq!(invoice_balance(h, s, x.a), m("250.00"));
    assert_eq!(invoice_balance(h, s, x.b), m("500.00"));
    assert_eq!(
        client_balance(h, s, x.client, date(2026, 11, 6)),
        m("750.00")
    );
    let credits = h.run(s.credit_memos().credits(x.client)).expect("credits");
    assert!(
        credits.iter().all(|c| c.source_id != rec.id),
        "a void memo is no credit"
    );
    restore_tax_rate(h, s, &old_rate);
}

/// B1b: an apply dated before the memo is refused; nothing moves.
pub fn credit_date_rules(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = seed(h, s, "Bluestem dates");
    let memo = NewCreditMemo::new(
        x.client,
        date(2026, 11, 10),
        vec![InvoiceLine {
            is_taxable: Some(false),
            ..InvoiceLine::account(x.income, q("1"), m("40.00"))
        }],
    );
    let rec = create(h, s, &memo);
    let e = h
        .run(s.credit_memos().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 9),
                applications: vec![ApplyInvoice {
                    invoice_id: x.a,
                    amount: Some(m("40.00")),
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("dated before the memo");
    assert_eq!(
        credit_refusal(&e),
        Some(CreditRefusal::ApplyDateBeforeSource),
        "{e:?}"
    );
    assert_eq!(invoice_balance(h, s, x.a), m("250.00"));
}

/// An edit sent with an old `expectedUpdatedAt` is 409 STALE.
pub fn credit_memo_stale_409(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = seed(h, s, "Bluestem stale");
    let mut memo = NewCreditMemo::new(
        x.client,
        date(2026, 11, 3),
        vec![InvoiceLine {
            is_taxable: Some(false),
            ..InvoiceLine::account(x.income, q("1"), m("60.00"))
        }],
    );
    let rec = create(h, s, &memo);
    memo.memo = Some("first edit".into());
    let first = h
        .run(
            s.credit_memos()
                .update(rec.id, &memo, &opts(rec.updated_at), key()),
        )
        .expect("first edit");
    assert_ne!(first.updated_at, rec.updated_at);
    assert_eq!(first.total, m("60.00"));
    memo.memo = Some("second edit".into());
    let e = h
        .run(
            s.credit_memos()
                .update(rec.id, &memo, &opts(rec.updated_at), key()),
        )
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
    assert_eq!(credit_refusal(&e), Some(CreditRefusal::Stale));
}

/// A memo dated on or before the closing date needs the closing-date
/// password (tenant B). No wrong-password probe: failed attempts count
/// toward the owner's limiter budget across the whole suite.
pub fn credit_memo_closed_period_password(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = h.s3();
    let x = seed(h, &s, "Bluestem closed");
    // A memo dated 10/25, saved before the books close on 10/31.
    let open = create(
        h,
        &s,
        &NewCreditMemo::new(
            x.client,
            date(2026, 10, 25),
            vec![InvoiceLine {
                is_taxable: Some(false),
                ..InvoiceLine::account(x.income, q("1"), m("25.00"))
            }],
        ),
    );
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2026-10-31"})),
    ))
    .expect("set the closing date");
    let closed = NewCreditMemo::new(
        x.client,
        date(2026, 10, 30),
        vec![InvoiceLine {
            is_taxable: Some(false),
            ..InvoiceLine::account(x.income, q("1"), m("25.00"))
        }],
    );
    let e = h
        .run(
            s.credit_memos()
                .create(&closed, &PostingOptions::default(), key()),
        )
        .expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    // An apply dated in the closed period (after the memo's own date) is
    // refused too: the apply's date is checked.
    let e = h
        .run(s.credit_memos().apply(
            open.id,
            &ApplyRequest {
                date: date(2026, 10, 30),
                applications: vec![ApplyInvoice {
                    invoice_id: x.a,
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
    // Voiding the closed-period memo needs the password as well.
    let e = h
        .run(s.transactions().void(
            open.id,
            "contract: closed",
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("void of a closed-period memo");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    // An apply dated after the closing date: no password needed.
    let ok = h
        .run(s.credit_memos().apply(
            open.id,
            &ApplyRequest {
                date: date(2026, 11, 3),
                applications: vec![ApplyInvoice {
                    invoice_id: x.a,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("apply after the closing date");
    assert_eq!(ok.remaining, Money::ZERO);
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
    assert_eq!(invoice_balance(h, &s, x.a), m("225.00"));
}

/// Another company's memo and client are 404 (company scope).
pub fn credit_memo_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = seed(h, s, "Bluestem scoped");
    let rec = create(
        h,
        s,
        &NewCreditMemo::new(
            x.client,
            date(2026, 11, 4),
            vec![InvoiceLine {
                is_taxable: Some(false),
                ..InvoiceLine::account(x.income, q("1"), m("10.00"))
            }],
        ),
    );
    let b = h.s3();
    let e = h
        .run(b.credit_memos().get(rec.id))
        .expect_err("another company's memo");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.credit_memos().apply(
            rec.id,
            &ApplyRequest {
                date: date(2026, 11, 4),
                applications: vec![ApplyInvoice {
                    invoice_id: x.a,
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("apply across companies");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.credit_memos().credits(x.client))
        .expect_err("another company's client");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    assert_eq!(invoice_balance(h, s, x.a), m("250.00"));
}
