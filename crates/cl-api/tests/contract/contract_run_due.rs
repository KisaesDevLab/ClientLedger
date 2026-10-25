//! The catch-up run against the real engine (P15 lane T, U-63
//! `recurring-run-due-v1`; plan §6 acceptance 2). PENDING until the engine
//! pin carries the capability.
//!
//! The run's `asOf` is the simulated clock: past dates built from today, so
//! it is never in the future (`RUN_DUE_FUTURE`) and every automatic start
//! stays within the engine's 366 days back. Every check compares exact
//! engine strings.

use cl_api::{
    AccountRoles, AccountType, ApiError, Basis, Capabilities, CardChargeDraft,
    ClosedPeriodAuthorization, Frequency, IdempotencyKey, JeLine, JeOptions, Method,
    NewJournalEntry, NewTemplate, PageRequest, PostingOptions, RunDue, RunDueFilter, ScheduleEnd,
    ScheduleMode, Session, Side, SkipReason, SplitLine, Template, TemplateDraft, TemplateSchedule,
    TransactionFilter, new_run_id,
};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account, date, m};
use crate::support::{Harness, unique};

/// Today (UTC): the engine's `CURRENT_DATE` is this day or the next, so
/// dates on or before it are never refused as future.
fn today() -> CalendarDate {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    date(1970, 1, 1)
        .add_days(i64::try_from(days).expect("days"))
        .expect("today")
}

/// Day `day` of the month `months_back` months before this one.
fn day_back(months_back: i32, day: u32) -> CalendarDate {
    today()
        .start_of_month()
        .add_months(-months_back)
        .and_then(|m| m.add_days(i64::from(day) - 1))
        .expect("date")
}

/// The end of the month `n` months after `base`'s.
fn month_end_after(base: CalendarDate, n: i32) -> CalendarDate {
    base.start_of_month()
        .add_months(n)
        .expect("month")
        .end_of_month()
}

/// A 31st, 7 or 8 months back: the anchor the clamped month rule counts
/// from (each later date is its month's last day). Two months without a
/// 31st never follow each other, so one step back at most.
fn a_31st_back() -> CalendarDate {
    let mut m = day_back(7, 1);
    while m.end_of_month().day() != 31 {
        m = m.add_months(-1).expect("month");
    }
    m.end_of_month()
}

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
    // A 31st 7–8 months back, then each month's last day (computed from
    // today, so the start stays within 366 days and `asOf` in the past).
    let start = a_31st_back();
    let ends = |from: i32, to: i32| -> Vec<CalendarDate> {
        (from..=to).map(|n| month_end_after(start, n)).collect()
    };
    let t = auto(h, s, &c, ScheduleMode::Auto, start, ScheduleEnd::Never);
    let before = balance(h, s, c.software);
    let first_as_of = month_end_after(start, 3);
    let first = run(h, s, first_as_of);
    assert_eq!(mine(&first, &t), ends(0, 3), "clamped to each month's end");
    assert!(mine(&run(h, s, first_as_of), &t).is_empty(), "again: 0");
    h.kill_engine_and_wait();
    let s = h.login();
    h.run(s.open_company(h.contract_co)).expect("open");
    assert!(
        mine(&run(h, &s, first_as_of), &t).is_empty(),
        "after a restart: 0"
    );
    assert_eq!(mine(&run(h, &s, month_end_after(start, 5)), &t), ends(4, 5));
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
        day_back(4, 15),
        ScheduleEnd::Never,
    );
    let as_of = day_back(1, 15);
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
        day_back(3, 10),
        ScheduleEnd::Never,
    );
    let id = new_run_id();
    let as_of = day_back(2, 10);
    let first = h.run(s.recurring().run_due(as_of, id, 200)).expect("run");
    let again = h
        .run(s.recurring().run_due(as_of, id, 200))
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
    let first = a_31st_back();
    let (second, third) = (month_end_after(first, 1), month_end_after(first, 2));
    let t = auto(h, &s, &c, ScheduleMode::Auto, first, ScheduleEnd::Never);
    let lock = |v: serde_json::Value| {
        h.run(s.raw_mapped(
            Method::Put,
            "/api/v1/company/settings",
            Some(&json!({ "lockDate": v })),
        ))
        .expect("closing date");
    };
    lock(json!(second.to_wire()));
    let r = run(h, &s, third);
    assert_eq!(mine(&r, &t), vec![third]);
    let skipped: Vec<_> = r
        .skipped
        .iter()
        .filter(|k| k.template == t.id)
        .map(|k| (k.date, k.reason.clone()))
        .collect();
    assert_eq!(
        skipped,
        vec![
            (first, SkipReason::ClosedPeriod),
            (second, SkipReason::ClosedPeriod)
        ]
    );
    if h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15 closing-password-v1") {
        let pw = Secret::new(unique("closing-run"));
        h.run(s.set_closing_password(None, Some(&pw), IdempotencyKey::new()))
            .expect("password");
        let e = h
            .run(s.templates().enter(
                t.id,
                Some(first),
                &PostingOptions::default(),
                IdempotencyKey::new(),
            ))
            .expect_err("closed");
        assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
        let entered = h
            .run(s.templates().enter(
                t.id,
                Some(first),
                &PostingOptions {
                    closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
                },
                IdempotencyKey::new(),
            ))
            .expect("entered");
        assert_eq!(entered.occurrence, Some(first));
        let again = h
            .run(s.templates().enter(
                t.id,
                Some(first),
                &PostingOptions {
                    closed_period: Some(ClosedPeriodAuthorization::with_password(pw.clone())),
                },
                IdempotencyKey::new(),
            ))
            .expect_err("taken");
        assert!(cl_api::is_occurrence_taken(&again), "{again:?}");
        h.run(s.templates().skip(t.id, second)).expect("dismiss");
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
        day_back(2, 1),
        ScheduleEnd::Never,
    );
    let before = (balance(h, s, c.card), balance(h, s, c.software));
    h.run(s.raw_mapped(
        Method::Put,
        &format!("/api/v1/accounts/{}", c.software),
        Some(&json!({ "isActive": false })),
    ))
    .expect("deactivate");
    let r = run(h, s, day_back(2, 1));
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
        day_back(3, 1),
        ScheduleEnd::Never,
    );
    for _ in 0..2 {
        let r = run(h, s, day_back(2, 1));
        assert!(mine(&r, &t).is_empty(), "a reminder never posts");
        let due = r
            .reminders
            .iter()
            .find(|d| d.template == t.id)
            .expect("listed");
        assert_eq!(due.dates, vec![day_back(3, 1), day_back(2, 1)]);
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
        day_back(7, 15),
        ScheduleEnd::After(3),
    );
    let r = run(h, s, today());
    assert_eq!(
        mine(&r, &t),
        vec![day_back(7, 15), day_back(6, 15), day_back(5, 15)],
        "three times, then it stops"
    );
    assert!(mine(&run(h, s, today()), &t).is_empty(), "stopped");
}

/// More than 31 due dates of one schedule: one call posts 31 and lists the
/// schedule once as `CATCH_UP_CAPPED` (its next due date, `new`); nothing
/// else is recorded, and the next call posts the rest, each date once.
pub fn catch_up_capped_continues_next_call(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let as_of = today().add_days(-1).expect("yesterday");
    let start = as_of.add_days(-39).expect("start");
    let t = h
        .run(s.templates().create(&NewTemplate {
            name: unique("Daily sweep"),
            group: None,
            client_form: None,
            draft: TemplateDraft::CardCharge(CardChargeDraft {
                pay_from: c.card,
                date: start,
                number: None,
                payee: None,
                memo: None,
                lines: vec![SplitLine {
                    account: c.software,
                    amount: m("1.25"),
                    memo: None,
                    contact: None,
                    billable: false,
                    tag: None,
                }],
            }),
            schedule: Some(TemplateSchedule {
                mode: ScheduleMode::Auto,
                frequency: Frequency::Daily,
                interval: 1,
                next_date: start,
                end: ScheduleEnd::Never,
            }),
        }))
        .unwrap_or_else(|e| panic!("template: {e}"));
    let day = |n: i64| start.add_days(n).expect("day");
    let first = h
        .run(s.recurring().run_due(as_of, new_run_id(), 200))
        .unwrap_or_else(|e| panic!("run-due: {e}"));
    assert_eq!(
        mine(&first, &t),
        (0..31).map(day).collect::<Vec<_>>(),
        "31 dates in one call"
    );
    let capped: Vec<_> = first
        .skipped
        .iter()
        .filter(|k| k.template == t.id)
        .map(|k| (k.date, k.reason.clone(), k.new))
        .collect();
    assert_eq!(
        capped,
        vec![(day(31), SkipReason::CatchUpCapped, true)],
        "listed once, at its next due date"
    );
    assert_eq!(card_txns(h, s, c.card).len(), 31);
    let second = run(h, s, as_of);
    assert_eq!(
        mine(&second, &t),
        (31..40).map(day).collect::<Vec<_>>(),
        "the rest on the next call"
    );
    assert!(
        !second
            .skipped
            .iter()
            .any(|k| k.template == t.id && k.reason == SkipReason::CatchUpCapped),
        "no longer capped"
    );
    let txns = card_txns(h, s, c.card);
    let mut dates: Vec<_> = txns.iter().map(|(_, d)| *d).collect();
    dates.sort();
    dates.dedup();
    assert_eq!((txns.len(), dates.len()), (40, 40), "each date once");
    assert_eq!(
        balance(h, s, c.software).to_wire(),
        "50.0000",
        "exactly 40 × 1.25"
    );
    assert!(mine(&run(h, s, as_of), &t).is_empty(), "nothing left");
}

fn permissions(h: &Harness, s: &Session, user: &str, transactions: &str) {
    let r = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({"overrides": {
                "recurring": "full",
                "transactions": transactions,
                "accounts": "full",
            }})),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
}

/// An automatic template posts only while its owner (the person who last
/// saved it) may still enter it: a bookkeeper's card-charge template is
/// skipped `PERMISSION` (recorded, advanced, nothing posted) once the
/// bookkeeper's transactions permission drops to view, and posts again
/// once it is back. The run itself is the owner's.
pub fn owner_without_permission_skipped(h: &Harness) {
    if !ready(h) {
        return;
    }
    let s = &h.s1;
    let c = card(h, s);
    let (user, bk) = h.bookkeeper();
    permissions(h, s, &user, "full");
    let first = day_back(3, 12);
    let t = auto(h, &bk, &c, ScheduleMode::Auto, first, ScheduleEnd::Never);
    permissions(h, s, &user, "view");
    let before = (balance(h, s, c.card), balance(h, s, c.software));
    let r = run(h, s, first);
    assert!(mine(&r, &t).is_empty(), "not posted");
    let skipped: Vec<_> = r
        .skipped
        .iter()
        .filter(|k| k.template == t.id)
        .map(|k| (k.date, k.reason.clone(), k.new))
        .collect();
    assert_eq!(skipped, vec![(first, SkipReason::Permission, true)]);
    assert_eq!(
        (balance(h, s, c.card), balance(h, s, c.software)),
        before,
        "nothing posted, exactly"
    );
    assert!(card_txns(h, s, c.card).is_empty(), "no transaction");
    // Recorded and advanced: the next run does not post it again.
    permissions(h, s, &user, "full");
    let second = day_back(2, 12);
    let r = run(h, s, second);
    assert_eq!(mine(&r, &t), vec![second], "posts again, next date only");
    assert!(
        r.skipped
            .iter()
            .any(|k| k.template == t.id && k.date == first && !k.new),
        "the skipped date stays listed for the owner"
    );
    assert_eq!(card_txns(h, s, c.card).len(), 1);
    h.run(s.templates().skip(t.id, first)).expect("dismiss");
}

/// U-82 (`run-due-filter-v1`, Q-PR1d): while the owner is asked about one
/// template (more than three missed dates), the run posts the others with
/// `excludeTemplateIds: [it]` and leaves it untouched and still due; once
/// the owner answers, `templateIds: [it]` posts its dates and nothing else,
/// each date once.
pub fn filter_holds_only_the_asked_template_u82(h: &Harness) {
    if !ready(h) || !h.require_cap(Capabilities::RUN_DUE_FILTER, "U-82 run-due-filter-v1") {
        return;
    }
    let s = &h.s1;
    let (ca, cb) = (card(h, s), card(h, s));
    let asked = auto(
        h,
        s,
        &ca,
        ScheduleMode::Auto,
        day_back(6, 5),
        ScheduleEnd::Never,
    );
    let other = auto(
        h,
        s,
        &cb,
        ScheduleMode::Auto,
        day_back(3, 5),
        ScheduleEnd::Never,
    );
    let as_of = day_back(2, 5);
    let id = new_run_id();
    let rest = h
        .run(
            s.recurring()
                .run_due_filtered(as_of, id, 200, &RunDueFilter::Except(vec![asked.id])),
        )
        .unwrap_or_else(|e| panic!("run-due except: {e}"));
    assert!(!rest.more);
    assert_eq!(mine(&rest, &other), vec![day_back(3, 5), day_back(2, 5)]);
    assert!(mine(&rest, &asked).is_empty(), "the asked one waits");
    assert!(card_txns(h, s, ca.card).is_empty(), "nothing posted for it");
    assert!(
        !rest.skipped.iter().any(|k| k.template == asked.id),
        "not touched: no skip recorded"
    );
    let stored = h.run(s.templates().get(asked.id)).expect("template");
    assert_eq!(
        stored.schedule.as_ref().map(|s| s.next_date),
        Some(day_back(6, 5)),
        "still due from its first missed date"
    );
    // The owner said "Post all": that template alone, a fresh run id.
    let one = h
        .run(s.recurring().run_due_filtered(
            as_of,
            new_run_id(),
            200,
            &RunDueFilter::Only(vec![asked.id]),
        ))
        .unwrap_or_else(|e| panic!("run-due only: {e}"));
    assert_eq!(
        mine(&one, &asked),
        (2..=6).rev().map(|n| day_back(n, 5)).collect::<Vec<_>>()
    );
    assert!(mine(&one, &other).is_empty(), "only the asked one");
    assert_eq!(card_txns(h, s, ca.card).len(), 5);
    assert_eq!(card_txns(h, s, cb.card).len(), 2);
    // A plain run now posts nothing more for either; the first run id
    // replayed with "only" posts nothing twice.
    let again = h
        .run(
            s.recurring()
                .run_due_filtered(as_of, id, 200, &RunDueFilter::Only(vec![asked.id])),
        )
        .expect("replay");
    assert!(mine(&again, &asked).is_empty(), "nothing new for it");
    assert_eq!(
        mine(&again, &other),
        vec![day_back(3, 5), day_back(2, 5)],
        "the run id's earlier posts come back"
    );
    assert!(mine(&run(h, s, as_of), &asked).is_empty(), "nothing left");
    assert_eq!(
        (
            card_txns(h, s, ca.card).len(),
            card_txns(h, s, cb.card).len()
        ),
        (5, 2),
        "each date once"
    );
}

/// A journal entry template (adjusting, a name on its A/P line) posted by
/// the catch-up run keeps the name, the adjusting mark and the basis, and
/// takes the company's next entry number (`recurring-guard-v1`, U-64).
pub fn journal_template_keeps_names_and_adjusting(h: &Harness) {
    if !ready(h)
        || !h.require_cap(Capabilities::RECURRING_GUARD, "U-64 recurring-guard-v1")
        || !h.require_cap(Capabilities::JE_NAMES, "U-58 je-names-v1")
        || !h.require_cap(Capabilities::JE_ADJUSTING, "U-60 je-adjusting-v1")
    {
        return;
    }
    let s = &h.s1;
    let accounts = h
        .run(s.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("accounts");
    let ap = accounts
        .iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_payable"))
        .map(|a| a.id)
        .expect("an A/P account");
    let expense = account(h, s, AccountType::Expense, "Accrued rent").id;
    let vendor = h
        .run(s.contacts().create(
            &cl_api::NewContact {
                fields: cl_api::ContactFields::named(
                    cl_api::ContactType::Vendor,
                    unique("Landlord"),
                ),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            IdempotencyKey::new(),
        ))
        .expect("vendor")
        .record
        .id;
    let on = day_back(2, 20);
    let entry = NewJournalEntry::new_with(
        on,
        Some(unique("Accrue rent")),
        Basis::Accrual,
        vec![
            JeLine {
                account: expense,
                side: Side::Debit(m("1200.00")),
                description: Some("Rent".into()),
                tag: None,
                contact: None,
            },
            JeLine {
                account: ap,
                side: Side::Credit(m("1200.00")),
                description: None,
                tag: None,
                contact: Some(vendor),
            },
        ],
        JeOptions {
            adjusting: true,
            ..JeOptions::default()
        },
        &AccountRoles::default(),
    )
    .expect("entry");
    let t = h
        .run(s.templates().create(&NewTemplate {
            name: unique("Accrue rent"),
            group: None,
            client_form: None,
            draft: TemplateDraft::Journal(entry),
            schedule: Some(TemplateSchedule {
                mode: ScheduleMode::Auto,
                frequency: Frequency::Monthly,
                interval: 1,
                next_date: on,
                end: ScheduleEnd::After(1),
            }),
        }))
        .unwrap_or_else(|e| panic!("template: {e}"));
    let r = run(h, s, on);
    let posted: Vec<_> = r.posted.iter().filter(|p| p.template == t.id).collect();
    assert_eq!(posted.len(), 1);
    let got = h
        .run(s.raw_mapped(
            Method::Get,
            &format!("/api/v1/transactions/{}", posted[0].transaction),
            None,
        ))
        .expect("read back");
    let txn = &got["transaction"];
    assert_eq!(txn["isAdjusting"], json!(true), "{txn}");
    assert_eq!(txn["basis"], json!("accrual"), "{txn}");
    assert!(
        txn["txnNumber"].as_str().is_some_and(|n| !n.is_empty()),
        "numbered: {txn}"
    );
    let lines = txn["lines"].as_array().expect("lines");
    let named: Vec<_> = lines
        .iter()
        .filter(|l| l["contactId"] == json!(vendor.to_string()))
        .collect();
    assert_eq!(named.len(), 1, "the A/P line keeps its name");
    assert_eq!(named[0]["accountId"], json!(ap.to_string()));
    assert_eq!(named[0]["credit"], json!("1200.0000"));
}
