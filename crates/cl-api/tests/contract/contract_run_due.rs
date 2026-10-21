//! The catch-up run against the real engine (P15 lane T, U-63
//! `recurring-run-due-v1`; plan §6 acceptance 2). PENDING until the engine
//! pin carries the capability.
//!
//! The run's `asOf` is the simulated clock (past dates; the engine refuses
//! a future one). Every check compares exact engine strings.

use cl_api::{
    AccountType, ApiError, Capabilities, CardChargeDraft, ClosedPeriodAuthorization, Frequency,
    IdempotencyKey, Method, NewTemplate, PageRequest, PostingOptions, RunDue, ScheduleEnd,
    ScheduleMode, Session, SkipReason, SplitLine, Template, TemplateDraft, TemplateSchedule,
    TransactionFilter, new_run_id,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, m};
use crate::support::{Harness, unique};

fn ready(h: &Harness) -> bool {
    h.require_cap(Capabilities::TEMPLATES, "U-62 templates-v1")
        && h.require_cap(Capabilities::RECURRING_RUN_DUE, "U-63 recurring-run-due-v1")
}

struct Card {
    card: AccountId,
    software: AccountId,
}

fn card(h: &Harness, s: &Session) -> Card {
    Card {
        card: account(h, s, AccountType::Liability, "Run-due card").id,
        software: account(h, s, AccountType::Expense, "Run-due software").id,
    }
}

fn auto(
    h: &Harness,
    s: &Session,
    c: &Card,
    mode: ScheduleMode,
    next: CalendarDate,
    end: ScheduleEnd,
) -> Template {
    h.run(s.templates().create(&NewTemplate {
        name: unique("Accounting software"),
        group: None,
        client_form: None,
        draft: TemplateDraft::CardCharge(CardChargeDraft {
            pay_from: c.card,
            date: next,
            number: None,
            payee: None,
            memo: None,
            lines: vec![SplitLine {
                account: c.software,
                amount: m("49.00"),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        }),
        schedule: Some(TemplateSchedule {
            mode,
            frequency: Frequency::Monthly,
            interval: 1,
            next_date: next,
            end,
        }),
    }))
    .unwrap_or_else(|e| panic!("template: {e}"))
}

/// Runs to the end of `more`, with one run id.
fn run(h: &Harness, s: &Session, as_of: CalendarDate) -> RunDue {
    let id = new_run_id();
    let mut last = h
        .run(s.recurring().run_due(as_of, id, 200))
        .unwrap_or_else(|e| panic!("run-due: {e}"));
    let mut posted = std::mem::take(&mut last.posted);
    while last.more {
        last = h.run(s.recurring().run_due(as_of, id, 200)).expect("more");
        posted.clone_from(&last.posted);
    }
    last.posted = posted;
    last
}

fn mine(r: &RunDue, t: &Template) -> Vec<CalendarDate> {
    let mut d: Vec<_> = r
        .posted
        .iter()
        .filter(|p| p.template == t.id)
        .map(|p| p.date)
        .collect();
    d.sort();
    d
}

/// The card's transactions (ids and dates).
fn card_txns(h: &Harness, s: &Session, card: AccountId) -> Vec<(TransactionId, CalendarDate)> {
    let page = h
        .run(s.transactions().list(
            &TransactionFilter {
                account_id: Some(card),
                ..TransactionFilter::default()
            },
            PageRequest::new(100, 0).expect("page"),
        ))
        .expect("list");
    page.data.into_iter().map(|r| (r.id, r.txn_date)).collect()
}

fn balance(h: &Harness, s: &Session, a: AccountId) -> Money {
    h.run(s.accounts().get(a))
        .expect("account")
        .record
        .balance
        .unwrap_or_default()
}

pub fn monthly_catch_up_exactly_once_across_restarts(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Auto,
        date(2026, 1, 31),
        ScheduleEnd::Never,
    );
    let before = balance(h, s, c.software);
    let first = run(h, s, date(2026, 4, 30));
    assert_eq!(
        mine(&first, &t),
        vec![
            date(2026, 1, 31),
            date(2026, 2, 28),
            date(2026, 3, 31),
            date(2026, 4, 30)
        ],
        "clamped to each month's end"
    );
    assert!(
        mine(&run(h, s, date(2026, 4, 30)), &t).is_empty(),
        "again: 0"
    );
    h.kill_engine_and_wait();
    let s = h.login();
    h.run(s.open_company(h.contract_co)).expect("open");
    assert!(
        mine(&run(h, &s, date(2026, 4, 30)), &t).is_empty(),
        "after a restart: 0"
    );
    assert_eq!(
        mine(&run(h, &s, date(2026, 6, 30)), &t),
        vec![date(2026, 5, 31), date(2026, 6, 30)]
    );
    let txns = card_txns(h, &s, c.card);
    let mut dates: Vec<_> = txns.iter().map(|(_, d)| *d).collect();
    dates.sort();
    dates.dedup();
    assert_eq!((txns.len(), dates.len()), (6, 6), "six, on distinct dates");
    let after = balance(h, &s, c.software);
    assert_eq!(
        after.checked_sub(before).expect("diff").to_wire(),
        "294.0000",
        "exactly 6 × 49.00"
    );
    let one = h.run(s.transactions().get(txns[0].0)).expect("txn");
    let raw = h
        .run(s.raw_mapped(
            Method::Get,
            &format!("/api/v1/transactions/{}", one.record.id),
            None,
        ))
        .expect("raw");
    assert!(
        raw.pointer("/transaction/recurringScheduleId")
            .is_some_and(|v| !v.is_null())
            || raw
                .pointer("/transaction/sourceId")
                .is_some_and(|v| v == &json!(t.id.to_string())),
        "the transaction carries its schedule"
    );
}

pub fn parallel_runs_post_once(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Auto,
        date(2025, 1, 15),
        ScheduleEnd::Never,
    );
    let as_of = date(2025, 4, 15);
    let calls: Vec<_> = (0..4)
        .map(|_| {
            let s = s.clone();
            async move { s.recurring().run_due(as_of, new_run_id(), 200).await }
        })
        .collect();
    let results = h.run(futures_join_all(calls));
    let total: usize = results
        .iter()
        .map(|r| r.as_ref().map_or(0, |r| mine(r, &t).len()))
        .sum();
    assert_eq!(total, 4, "four dates, each once");
    assert_eq!(card_txns(h, s, c.card).len(), 4);
}

async fn futures_join_all<F, T>(fs: Vec<F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = fs.into_iter().map(tokio::spawn).collect();
    let mut out = Vec::new();
    for h in handles {
        out.push(h.await.expect("join"));
    }
    out
}

pub fn lost_reply_same_run_id(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Auto,
        date(2025, 6, 10),
        ScheduleEnd::Never,
    );
    let id = new_run_id();
    let first = h
        .run(s.recurring().run_due(date(2025, 7, 10), id, 200))
        .expect("run");
    let again = h
        .run(s.recurring().run_due(date(2025, 7, 10), id, 200))
        .expect("replay");
    assert_eq!(mine(&first, &t).len(), 2);
    assert_eq!(
        mine(&again, &t),
        mine(&first, &t),
        "the same posts come back"
    );
    assert_eq!(card_txns(h, s, c.card).len(), 2, "posted once");
}

pub fn closed_period_skipped_then_entered_with_password(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = h.s3();
    let c = card(h, &s);
    let t = auto(
        h,
        &s,
        &c,
        ScheduleMode::Auto,
        date(2026, 1, 31),
        ScheduleEnd::Never,
    );
    let lock = |v: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": v })),
        ))
        .expect("closing date");
    };
    lock(json!("2026-02-28"));
    let r = run(h, &s, date(2026, 3, 31));
    assert_eq!(mine(&r, &t), vec![date(2026, 3, 31)]);
    let skipped: Vec<_> = r
        .skipped
        .iter()
        .filter(|k| k.template == t.id)
        .map(|k| (k.date, k.reason.clone()))
        .collect();
    assert_eq!(
        skipped,
        vec![
            (date(2026, 1, 31), SkipReason::ClosedPeriod),
            (date(2026, 2, 28), SkipReason::ClosedPeriod)
        ]
    );
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15 closing-password-v1") {
        let pw = Secret::new(unique("closing-run"));
        h.run(s.set_closing_password(None, Some(&pw), IdempotencyKey::new()))
            .expect("password");
        let e = h
            .run(s.templates().enter(
                t.id,
                Some(date(2026, 1, 31)),
                &PostingOptions::default(),
                IdempotencyKey::new(),
            ))
            .expect_err("closed");
        assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
        let entered = h
            .run(s.templates().enter(
                t.id,
                Some(date(2026, 1, 31)),
                &PostingOptions {
                    closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
                },
                IdempotencyKey::new(),
            ))
            .expect("entered");
        assert_eq!(entered.occurrence, Some(date(2026, 1, 31)));
        let again = h
            .run(s.templates().enter(
                t.id,
                Some(date(2026, 1, 31)),
                &PostingOptions {
                    closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
                },
                IdempotencyKey::new(),
            ))
            .expect_err("taken");
        assert!(cl_api::is_occurrence_taken(&again), "{again:?}");
        h.run(s.templates().skip(t.id, date(2026, 2, 28)))
            .expect("dismiss");
        h.run(s.set_closing_password(Some(&pw), None, IdempotencyKey::new()))
            .expect("clear password");
    }
    lock(serde_json::Value::Null);
}

pub fn inactive_account_skipped_no_partial(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Auto,
        date(2025, 9, 1),
        ScheduleEnd::Never,
    );
    let before = (balance(h, s, c.card), balance(h, s, c.software));
    h.run(s.raw_mapped(
        Method::Put,
        &format!("/api/v1/accounts/{}", c.software),
        Some(&json!({ "isActive": false })),
    ))
    .expect("deactivate");
    let r = run(h, s, date(2025, 9, 1));
    assert!(mine(&r, &t).is_empty(), "skipped, not posted");
    assert!(
        r.skipped
            .iter()
            .any(|k| k.template == t.id && k.reason == SkipReason::InvalidAccount)
    );
    assert_eq!(
        (balance(h, s, c.card), balance(h, s, c.software)),
        before,
        "nothing posted, exactly"
    );
    assert!(card_txns(h, s, c.card).is_empty(), "no transaction");
}

pub fn remind_mode_lists_not_posts(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Remind,
        date(2025, 10, 1),
        ScheduleEnd::Never,
    );
    for _ in 0..2 {
        let r = run(h, s, date(2025, 11, 1));
        assert!(mine(&r, &t).is_empty(), "a reminder never posts");
        let due = r
            .reminders
            .iter()
            .find(|d| d.template == t.id)
            .expect("listed");
        assert_eq!(due.dates, vec![date(2025, 10, 1), date(2025, 11, 1)]);
    }
    let att = h.run(s.recurring().attention()).expect("attention");
    assert!(att.reminders.iter().any(|d| d.template == t.id));
    assert!(card_txns(h, s, c.card).is_empty(), "no transaction");
}

pub fn ends_after_count(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let t = auto(
        h,
        s,
        &c,
        ScheduleMode::Auto,
        date(2026, 9, 15),
        ScheduleEnd::After(3),
    );
    let r = run(h, s, date(2027, 3, 1));
    assert_eq!(
        mine(&r, &t),
        vec![date(2026, 9, 15), date(2026, 10, 15), date(2026, 11, 15)],
        "three times, then it stops"
    );
    assert!(mine(&run(h, s, date(2027, 6, 1)), &t).is_empty(), "stopped");
}
