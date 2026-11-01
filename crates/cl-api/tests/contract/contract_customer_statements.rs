//! Customer statements against the real engine (P22 plan §3 D8, §6.1
//! golden 5, §6.2, Q-177, Q-P22c, Q-P22n).
//!
//! - `statement_aging_equals_ar_aging` (golden 5): as of 11/30 the
//!   statement's buckets equal the client's A/R aging row and its amount
//!   due equals the client's A/R balance (300.25); a later payment leaves
//!   the 11/30 figures alone; balance forward from 11/01 opens at 750.00;
//!   the company-wide aging total is the A/R account's balance.
//! - `customer_statement_run_send_history`: a numbered run replayed with
//!   its key issues once; the snapshot doesn't move after a payment; Mark
//!   as sent replayed records one event; an email waits in the Outbox.
//! - `statement_cross_company_404`: another company's statement is 404,
//!   for email too, before any SMTP problem.

use cl_api::{
    ApiError, Capabilities, EmailMessage, NewStatementRun, PageRequest, SendMethod, SendRecord,
    Session, StatementFormat, StatementListQuery, StatementQuery, statement_refusal,
};
use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};

use crate::common::{date, key, m};
use crate::contract_credit_memos::{Seed, case1, create, restore_tax_rate, seed, set_tax_rate};
use crate::contract_receive_payment::{
    balance, clearing, client_balance, preview, receive_auto, request, save_of, system_account,
};
use crate::support::Harness;

fn caps(h: &Harness) -> bool {
    crate::contract_credit_memos::caps(h)
        && h.require_cap(Capabilities::CUSTOMER_STATEMENT, "U-112")
        && h.require_cap(Capabilities::CUSTOMER_STATEMENT_SEND, "U-140")
        && h.require_cap(Capabilities::IDEMPOTENCY_CUSTOMER_STATEMENTS, "U-140")
        && h.require_cap(Capabilities::AR_AGING_EXACT, "U-142")
        && h.require_cap(Capabilities::AR_OPEN_ITEMS_ASOF, "U-142")
}

fn nov30() -> CalendarDate {
    date(2026, 11, 30)
}

/// A payment of `amount` on `on` applied to nothing: an open credit.
fn unapplied_payment(h: &Harness, s: &Session, c: ContactId, on: CalendarDate, amount: &str) {
    let uf = clearing(h, s);
    let mut r = request(c, on, Some(amount), Some(uf));
    r.applications = Some(Vec::new());
    let p = preview(h, s, &r);
    assert_eq!(p.totals.unapplied, m(amount), "{:?}", p.problems);
    h.run(s.customer_payments().receive(
        &save_of(&p, c, on, uf),
        &cl_api::PostingOptions::default(),
        key(),
    ))
    .unwrap_or_else(|e| panic!("receive: {e}"));
}

/// Golden 5's books: the golden 1 memo kept as credit and a 50.00
/// payment dated 11/30 applied to nothing.
fn golden5(h: &Harness, s: &Session, name: &str) -> Seed {
    let old = set_tax_rate(h, s, "0.0825");
    let x = seed(h, s, name);
    let memo = create(h, s, &case1(h, s, &x));
    restore_tax_rate(h, s, &old);
    assert_eq!(memo.remaining, m("399.75"));
    unapplied_payment(h, s, x.client, nov30(), "50.00");
    x
}

fn statement(
    h: &Harness,
    s: &Session,
    c: ContactId,
    as_of: CalendarDate,
    format: StatementFormat,
    start: Option<CalendarDate>,
) -> cl_api::CustomerStatement {
    h.run(s.customer_statements().statement(&StatementQuery {
        contact: c,
        as_of,
        format,
        start,
    }))
    .unwrap_or_else(|e| panic!("statement: {e}"))
}

/// Q-177 and golden 5: statement aging = A/R aging = the A/R balance.
pub fn statement_aging_equals_ar_aging(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = golden5(h, s, "Bluestem aging");
    let st = statement(h, s, x.client, nov30(), StatementFormat::OpenItem, None);
    let b = st.aging.buckets;
    assert_eq!(b.current, m("-50.00"), "the 11/30 payment credit");
    assert_eq!(b.days_1_30, m("100.25"), "B 500.00 and the memo -399.75");
    assert_eq!(b.days_31_60, m("250.00"), "A, 46 days");
    assert_eq!(b.days_61_90, Money::ZERO);
    assert_eq!(b.over_90, Money::ZERO);
    assert_eq!(st.aging.total, m("300.25"));
    assert_eq!(st.amount_due, m("300.25"));
    assert!(!st.has_difference(), "the statement ties to the ledger");
    // = the client's A/R balance as of 11/30.
    assert_eq!(client_balance(h, s, x.client, nov30()), m("300.25"));
    // = the client's row of the A/R aging summary, bucket for bucket.
    let summary = h
        .run(s.reports().ar_aging_summary(nov30()))
        .expect("aging summary");
    let row = summary.row(Some(x.client)).expect("the client's row");
    assert!(row.buckets == st.aging.buckets, "same buckets");
    assert_eq!(row.total, st.aging.total);
    // The statement list agrees.
    let list = h
        .run(s.customer_statements().statement_list(
            &StatementListQuery {
                as_of: nov30(),
                min_days_overdue: 0,
                open_only: true,
            },
            PageRequest::first_max(),
        ))
        .expect("statement list");
    let listed = list
        .data
        .iter()
        .find(|r| r.contact_id == x.client)
        .expect("listed");
    assert_eq!(listed.amount_due, m("300.25"));
    // A payment dated 12/03 leaves the 11/30 figures unchanged.
    receive_auto(h, s, x.client, date(2026, 12, 3), "100.00");
    let again = statement(h, s, x.client, nov30(), StatementFormat::OpenItem, None);
    assert!(again.aging == st.aging, "as of 11/30, unchanged");
    assert_eq!(again.amount_due, m("300.25"));
    // Balance forward from 11/01: opening 750.00, the memo and the
    // payment, closing 300.25.
    let bf = statement(
        h,
        s,
        x.client,
        nov30(),
        StatementFormat::BalanceForward,
        Some(date(2026, 11, 1)),
    );
    assert_eq!(bf.opening_balance, Some(m("750.00")));
    let amounts: Vec<_> = bf.lines.iter().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![m("-399.75"), m("-50.00")]);
    assert_eq!(bf.lines.last().map(|l| l.balance), Some(m("300.25")));
    assert_eq!(bf.amount_due, m("300.25"));
    // Company-wide: the aging total is the A/R account's balance, as of a
    // date past everything the suite posts.
    let far = date(2030, 12, 31);
    let all = h
        .run(s.reports().ar_aging_summary(far))
        .expect("aging summary");
    let ar = system_account(h, s, "accounts_receivable");
    assert_eq!(all.total_exact, balance(h, s, ar), "aging total = GL");
}

/// Q-P22c, Q-P22n: numbered snapshots, Mark as sent, the Outbox.
pub fn customer_statement_run_send_history(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = golden5(h, s, "Bluestem run");
    let run = NewStatementRun {
        as_of: nov30(),
        format: StatementFormat::OpenItem,
        start: None,
        contact_ids: vec![x.client],
    };
    let k = cl_api::IdempotencyKey::new();
    let one = h
        .run(s.customer_statements().create_run(&run, k.clone()))
        .expect("run");
    let two = h
        .run(s.customer_statements().create_run(&run, k))
        .expect("replayed run");
    assert_eq!(one.run_id, two.run_id, "issued once");
    assert_eq!(one.statements.len(), 1);
    let issued = &one.statements[0];
    assert_eq!(two.statements[0].statement_id, issued.statement_id);
    assert_eq!(issued.contact_id, x.client);
    assert_eq!(issued.amount_due, m("300.25"));
    assert!(!issued.number.is_empty(), "numbered (Q-P22n)");
    // A new run gets the next number.
    let next = h
        .run(s.customer_statements().create_run(&run, key()))
        .expect("second run");
    assert_ne!(next.statements[0].number, issued.number);
    // The snapshot: the statement as issued, unchanged by a later payment.
    let rec = h
        .run(s.customer_statements().get(issued.statement_id))
        .expect("get");
    assert_eq!(rec.number, issued.number);
    assert_eq!(rec.amount_due, m("300.25"));
    assert_eq!(rec.snapshot.amount_due, m("300.25"));
    assert_eq!(rec.send_history.len(), 0);
    receive_auto(h, s, x.client, date(2026, 12, 1), "100.00");
    let rec = h
        .run(s.customer_statements().get(issued.statement_id))
        .expect("get");
    assert_eq!(rec.snapshot.amount_due, m("300.25"), "a snapshot");
    // Mark as sent (printed), replayed with its key: one event.
    let sent = SendRecord {
        method: SendMethod::Printed,
        recipients: Vec::new(),
        sent_at: None,
    };
    let k = cl_api::IdempotencyKey::new();
    h.run(
        s.customer_statements()
            .mark_sent(issued.statement_id, &sent, k.clone()),
    )
    .expect("mark sent");
    h.run(
        s.customer_statements()
            .mark_sent(issued.statement_id, &sent, k),
    )
    .expect("replayed mark sent");
    let rec = h
        .run(s.customer_statements().get(issued.statement_id))
        .expect("get");
    let methods: Vec<_> = rec.send_history.iter().map(|e| e.method).collect();
    assert_eq!(methods, vec![SendMethod::Printed], "recorded once");
    let history = h
        .run(s.customer_statements().history(x.client))
        .expect("history");
    let mine = history
        .iter()
        .find(|r| r.id == issued.statement_id)
        .expect("in the client's history");
    assert!(mine.last_sent_at.is_some(), "its last send");
    assert_eq!(mine.last_sent_at, rec.send_history[0].sent_at);
    // Email later: it waits in the Outbox.
    let msg = EmailMessage {
        to: vec!["billing@example.test".into()],
        subject: "Statement".into(),
        body: "Your statement".into(),
        ..EmailMessage::default()
    };
    let queued = h
        .run(
            s.customer_statements()
                .email_later(issued.statement_id, &msg, key()),
        )
        .expect("outbox");
    h.run(s.invoices().remove_from_outbox(queued.id))
        .expect("removed from the Outbox");
}

/// Another company's statement and client are 404, for email too, before
/// any SMTP answer.
pub fn statement_cross_company_404(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let x = golden5(h, s, "Bluestem stmt scope");
    let run = h
        .run(s.customer_statements().create_run(
            &NewStatementRun {
                as_of: nov30(),
                format: StatementFormat::OpenItem,
                start: None,
                contact_ids: vec![x.client],
            },
            key(),
        ))
        .expect("run");
    let id = run.statements[0].statement_id;
    let b = h.s3();
    let e = h
        .run(b.customer_statements().get(id))
        .expect_err("another company's statement");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let msg = EmailMessage {
        to: vec!["billing@example.test".into()],
        subject: "Statement".into(),
        body: "Hello".into(),
        ..EmailMessage::default()
    };
    let e = h
        .run(b.customer_statements().email(
            id,
            &msg,
            "Statement.pdf",
            b"%PDF-1.4\n%%EOF\n",
            None,
            key(),
        ))
        .expect_err("email another company's statement");
    assert!(
        matches!(e, ApiError::NotFound { .. }),
        "404 before SMTP: {e:?} ({:?})",
        statement_refusal(&e)
    );
    let e = h
        .run(b.customer_statements().mark_sent(
            id,
            &SendRecord {
                method: SendMethod::Printed,
                recipients: Vec::new(),
                sent_at: None,
            },
            key(),
        ))
        .expect_err("mark another company's statement");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.customer_statements().statement(&StatementQuery {
            contact: x.client,
            as_of: nov30(),
            format: StatementFormat::OpenItem,
            start: None,
        }))
        .expect_err("another company's client");
    assert!(matches!(e, ApiError::NotFound { .. }), "{e:?}");
    let e = h
        .run(b.customer_statements().create_run(
            &NewStatementRun {
                as_of: nov30(),
                format: StatementFormat::OpenItem,
                start: None,
                contact_ids: vec![x.client],
            },
            key(),
        ))
        .expect_err("a run for another company's client");
    assert!(
        matches!(e, ApiError::NotFound { .. } | ApiError::Validation(_)),
        "{e:?}"
    );
}
