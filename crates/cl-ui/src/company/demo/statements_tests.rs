//! The demo open items, A/R aging and statements: golden case 5 of
//! `fixtures/receivables/golden.json` and the equalities statement aging =
//! A/R aging row = the client balance (P22 plan §6.2, §6.3).

use cl_api::{NewCustomerPayment, SendMethod, SendRecord};
use proptest::prelude::*;

use super::super::credit_memos::tests::{case1, client_balance, fresh, line};
use super::super::credit_memos::{BLUESTEM, create_memo};
use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("date")
}

fn pay(s: &mut DemoState, c: ContactId, date: &str, amount: &str) {
    let clearing = super::super::receivables::clearing(s).expect("clearing");
    let p = NewCustomerPayment {
        customer_id: c,
        date: d(date),
        amount: m(amount),
        deposit_to: clearing,
        payment_method: None,
        ref_no: None,
        memo: None,
        applications: Vec::new(),
        credit_applications: Vec::new(),
        write_off_account_id: None,
        discount_account_id: None,
        hold_as_deposit: false,
        bank_feed_item_id: None,
    };
    super::super::receivables::receive(s, &p, None, None).expect("receive");
}

fn q(c: ContactId, as_of: &str, format: StatementFormat, start: Option<&str>) -> StatementQuery {
    StatementQuery {
        contact: c,
        as_of: d(as_of),
        format,
        start: start.map(d),
    }
}

#[test]
fn golden_5_statement_aging_equals_ar_aging_and_the_balance() {
    let (mut s, c) = fresh();
    create_memo(&mut s, &case1(c), None).expect("memo");
    pay(&mut s, c, "2026-11-30", "50.00");
    assert_eq!(client_balance(&s, c), m("300.25"));
    let st = statement(&mut s, &q(c, "2026-11-30", StatementFormat::OpenItem, None)).expect("st");
    let a = &st.aging.buckets;
    assert_eq!(a.current, m("-50.00"));
    assert_eq!(a.days_1_30, m("100.25"));
    assert_eq!(a.days_31_60, m("250.00"));
    assert_eq!(a.days_61_90, m("0.00"));
    assert_eq!(a.over_90, m("0.00"));
    assert_eq!(st.aging.total, m("300.25"));
    assert_eq!(st.amount_due, m("300.25"));
    assert_eq!(st.lines.len(), 4);
    let aging = ar_aging(&mut s, d("2026-11-30")).expect("aging");
    let row = aging.row(Some(c)).expect("Bluestem row");
    assert_eq!(row.total, st.aging.total);
    assert_eq!(row.buckets, st.aging.buckets);
    // A later payment leaves the 11/30 figures alone.
    pay(&mut s, c, "2026-12-03", "100.00");
    let again =
        statement(&mut s, &q(c, "2026-11-30", StatementFormat::OpenItem, None)).expect("st");
    assert_eq!(again.aging, st.aging);
    // Balance forward from 11/01: 750.00, -399.75, -50.00 -> 300.25.
    let bf = statement(
        &mut s,
        &q(
            c,
            "2026-11-30",
            StatementFormat::BalanceForward,
            Some("2026-11-01"),
        ),
    )
    .expect("bf");
    assert_eq!(bf.opening_balance, Some(m("750.00")));
    let amounts: Vec<Money> = bf.lines.iter().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![m("-399.75"), m("-50.00")]);
    assert_eq!(bf.lines.last().expect("line").balance, m("300.25"));
    assert_eq!(bf.amount_due, m("300.25"));
}

#[test]
fn runs_issue_numbered_snapshots_without_ids_and_record_sends() {
    let (mut s, c) = fresh();
    create_memo(&mut s, &case1(c), None).expect("memo");
    let run = NewStatementRun {
        as_of: d("2026-11-30"),
        format: StatementFormat::OpenItem,
        start: None,
        contact_ids: vec![c],
    };
    let r1 = create_run(&mut s, &run, "k1").expect("run");
    let r2 = create_run(&mut s, &run, "k1").expect("replay");
    assert_eq!(r1, r2);
    let id = r1.statements[0].statement_id;
    let rec = get(&s, id).expect("issued");
    assert!(rec.snapshot.contact.id.is_none());
    assert!(rec.snapshot.lines.iter().all(|l| l.txn_id.is_none()));
    assert_eq!(rec.snapshot.lines.len(), 3);
    pay(&mut s, c, "2026-11-20", "10.00");
    assert_eq!(get(&s, id).expect("issued").snapshot, rec.snapshot);
    mark_sent(
        &mut s,
        id,
        &SendRecord {
            method: SendMethod::Printed,
            recipients: Vec::new(),
            sent_at: None,
        },
    )
    .expect("sent");
    let msg = cl_api::EmailMessage {
        to: vec!["office@bluestem-vet.example".into()],
        subject: "Statement".into(),
        body: "Attached.".into(),
        ..cl_api::EmailMessage::default()
    };
    let queued = email_later(&mut s, id, &msg).expect("outbox");
    assert_eq!(queued.kind, cl_api::OutboxKind::CustomerStatement);
    assert_eq!(queued.customer_statement_id, Some(id));
    assert!(
        super::super::invoices::outbox(&s)
            .iter()
            .any(|o| o.id == queued.id)
    );
    s.invoices.no_smtp = true;
    let e = email(&mut s, id, &msg, None, "e0").expect_err("no mail server");
    assert_eq!(
        cl_api::statement_refusal(&e),
        Some(cl_api::StatementRefusal::SmtpNotConfigured)
    );
    s.invoices.no_smtp = false;
    email(&mut s, id, &msg, Some(queued.id), "e1").expect("sent");
    email(&mut s, id, &msg, Some(queued.id), "e1").expect("replay sends once");
    // History rows carry only lastSentAt (as the engine); the statement keeps every send.
    let h = history(&s, c);
    assert!(h[0].last_sent_at.is_some());
    assert_eq!(get(&s, id).expect("statement").send_history.len(), 2);
    let list = statement_list(
        &mut s,
        &StatementListQuery {
            as_of: d("2026-11-30"),
            min_days_overdue: 0,
            open_only: true,
        },
    );
    let row = list
        .iter()
        .find(|r| r.contact_id == c)
        .expect("Bluestem listed");
    assert_eq!(row.name, BLUESTEM);
    assert!(row.has_email);
    assert_eq!(row.oldest_days_overdue, Some(46));
    assert_eq!(
        row.last_statement.as_ref().map(|l| l.date),
        Some(d("2026-11-30"))
    );
    let e = create_run(
        &mut s,
        &NewStatementRun {
            contact_ids: vec![c; 501],
            ..run
        },
        "k2",
    )
    .expect_err("too many");
    assert_eq!(
        cl_api::statement_refusal(&e),
        Some(cl_api::StatementRefusal::TooManyContacts)
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// For random memos, payments and dates: Σ open items = the client
    /// balance (as of a date after everything), the statement aging = the
    /// A/R aging row, and amount due = the aging total.
    #[test]
    fn statement_aging_equals_ar_aging_on_random_data(
        memo in 1i64..90_000,
        paid in 1i64..90_000,
        day in 1u32..28,
        as_of_day in 1u32..30,
    ) {
        let (mut s, c) = fresh();
        let price = Money::from_cents(memo).expect("m").to_wire();
        create_memo(&mut s, &cl_api::NewCreditMemo::new(c, CalendarDate::from_ymd(2026, 11, day).expect("d"), vec![line("1", &price, false)]), None).expect("memo");
        pay(&mut s, c, &format!("2026-11-{day:02}"), &Money::from_cents(paid).expect("m").to_wire());
        prop_assert_eq!(items_total(&s, d("2027-12-31"), c), amount(client_balance(&s, c)));
        let as_of = CalendarDate::from_ymd(2026, 11, as_of_day).expect("d");
        let st = statement(&mut s, &StatementQuery { contact: c, as_of, format: StatementFormat::OpenItem, start: None }).expect("st");
        let aging = ar_aging(&mut s, as_of).expect("aging");
        match aging.row(Some(c)) {
            Some(row) => {
                prop_assert_eq!(row.buckets, st.aging.buckets);
                prop_assert_eq!(row.total, st.aging.total);
            }
            None => prop_assert!(st.aging.total.is_zero()),
        }
        prop_assert_eq!(st.amount_due, st.aging.total);
        prop_assert_eq!(amount(st.aging.total), items_total(&s, as_of, c));
    }
}
