//! The demo twin of run-due, skip and resolve: one test per contract case
//! of `crates/cl-api/tests/contract/contract_run_due.rs` (P15 C3).

use super::*;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;
use crate::company::demo::template_store;
use cl_api::{CardChargeDraft, Frequency, NewTemplate, SplitLine, TemplateSchedule, new_run_id};
use cl_core::{Money, Secret};

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

/// The demo with Ozark Ridge open and its books not closed (the contract
/// tests' fresh company).
fn open_books() -> DemoState {
    let mut s = DemoState::new();
    s.open = s.companies.first().map(|c| c.tenant_id);
    s.companies[0].closed_through = None;
    s
}

fn card(amount: &str) -> TemplateDraft {
    TemplateDraft::CardCharge(CardChargeDraft {
        pay_from: super::super::account_id(2050),
        date: d(2026, 1, 1),
        number: None,
        payee: None,
        memo: None,
        lines: vec![SplitLine {
            account: super::super::account_id(6250),
            amount: m(amount),
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        }],
    })
}

fn schedule(mode: ScheduleMode, next: CalendarDate, end: ScheduleEnd) -> TemplateSchedule {
    TemplateSchedule {
        mode,
        frequency: Frequency::Monthly,
        interval: 1,
        next_date: next,
        end,
    }
}

fn make(s: &mut DemoState, name: &str, sched: TemplateSchedule) -> TemplateId {
    template_store::create(
        s,
        &NewTemplate {
            name: name.into(),
            group: None,
            client_form: Some(cl_api::client_form::CARD_CHARGE.into()),
            draft: card("49.00"),
            schedule: Some(sched),
        },
    )
    .expect("template")
    .id
}

/// Credits minus debits on the card account (2050) over the demo ledger.
fn owed_on_card(s: &DemoState) -> Money {
    s.ledger
        .transactions()
        .iter()
        .flat_map(|t| t.lines.iter())
        .filter(|l| l.account == "2050")
        .try_fold(Money::ZERO, |acc, l| {
            acc.checked_add(l.credit)
                .and_then(|x| x.checked_sub(l.debit))
        })
        .expect("sum")
}

fn run(s: &mut DemoState, as_of: CalendarDate) -> RunDue {
    run_due(s, as_of, new_run_id(), 200, as_of).expect("run").0
}

#[test]
fn monthly_catch_up_exactly_once_across_restarts() {
    let mut s = open_books();
    let before = owed_on_card(&s);
    let id = make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 1, 31), ScheduleEnd::Never),
    );
    let first = run(&mut s, d(2026, 4, 30));
    let dates: Vec<CalendarDate> = first.posted.iter().map(|p| p.date).collect();
    assert_eq!(
        dates,
        vec![
            d(2026, 1, 31),
            d(2026, 2, 28),
            d(2026, 3, 31),
            d(2026, 4, 30)
        ],
        "the clamped month rule"
    );
    assert!(first.posted.iter().all(|p| p.total == Some(m("49.00"))));
    assert!(!first.more);
    assert!(run(&mut s, d(2026, 4, 30)).posted.is_empty(), "again: none");
    // A restart: the engine keeps only its tables (the twin's state); the
    // app forgets its run.
    let mut restarted = s.clone();
    assert!(
        run(&mut restarted, d(2026, 4, 30)).posted.is_empty(),
        "after the restart: none"
    );
    let later = run(&mut restarted, d(2026, 6, 30));
    assert_eq!(later.posted.len(), 2);
    let posted: Vec<_> = restarted
        .occurrences
        .iter()
        .filter(|((t, _), o)| *t == id && o.status == OccStatus::Posted)
        .collect();
    assert_eq!(posted.len(), 6);
    let txns: BTreeSet<TransactionId> = posted.iter().filter_map(|(_, o)| o.transaction).collect();
    assert_eq!(txns.len(), 6, "six transactions");
    let change = owed_on_card(&restarted)
        .checked_sub(before)
        .expect("change");
    assert_eq!(change, m("294.00"), "6 × 49.00 exactly");
    assert_eq!(
        template_store::get(&restarted, id)
            .expect("t")
            .schedule
            .expect("s")
            .next_date,
        d(2026, 7, 31)
    );
}

#[test]
fn parallel_runs_post_once() {
    let mut s = open_books();
    make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 1, 31), ScheduleEnd::Never),
    );
    let as_of = d(2026, 4, 30);
    // Four runs with their own ids, one after another (the engine
    // serializes them under its run lock).
    let total: usize = (0..4).map(|_| run(&mut s, as_of).posted.len()).sum();
    assert_eq!(total, 4);
}

#[test]
fn lost_reply_same_run_id() {
    let mut s = open_books();
    make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 2, 15), ScheduleEnd::Never),
    );
    let run_id = new_run_id();
    let as_of = d(2026, 3, 20);
    let (lost, touched) = run_due(&mut s, as_of, run_id, 200, as_of).expect("run");
    assert_eq!(lost.posted.len(), 2);
    assert_eq!(touched.len(), 2, "the card and the expense account");
    let (again, _) = run_due(&mut s, as_of, run_id, 200, as_of).expect("again");
    assert_eq!(again.posted, lost.posted, "the same posts, nothing new");
    assert_eq!(
        s.occurrences
            .values()
            .filter(|o| o.status == OccStatus::Posted)
            .count(),
        2
    );
    assert!(run(&mut s, as_of).posted.is_empty(), "another run id: none");
}

#[test]
fn closed_period_skipped_then_entered_with_password() {
    let mut s = open_books();
    s.companies[0].closed_through = Some(d(2026, 9, 30));
    let id = make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 9, 15), ScheduleEnd::Never),
    );
    let first = run(&mut s, d(2026, 11, 4));
    assert_eq!(
        first.posted.iter().map(|p| p.date).collect::<Vec<_>>(),
        vec![d(2026, 10, 15)]
    );
    assert_eq!(first.skipped.len(), 1);
    assert_eq!(first.skipped[0].date, d(2026, 9, 15));
    assert_eq!(first.skipped[0].reason, SkipReason::ClosedPeriod);
    assert!(first.skipped[0].new);
    // The open skip stays listed, no longer new.
    let second = run(&mut s, d(2026, 11, 4));
    assert_eq!(second.skipped.len(), 1);
    assert!(!second.skipped[0].new);
    // Entered with the password.
    let today = d(2026, 11, 4);
    let e =
        template_store::enter(&mut s, id, Some(d(2026, 9, 15)), today, None).expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let pw = Secret::new(DEMO_CLOSING_PASSWORD.to_string());
    template_store::enter(&mut s, id, Some(d(2026, 9, 15)), today, Some(&pw)).expect("entered");
    assert!(attention(&s, today).skipped.is_empty(), "entered");
    let again = template_store::enter(&mut s, id, Some(d(2026, 9, 15)), today, Some(&pw))
        .expect_err("taken");
    assert!(cl_api::is_occurrence_taken(&again));
}

#[test]
fn inactive_account_skipped_no_partial() {
    let mut s = open_books();
    make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 10, 15), ScheduleEnd::Never),
    );
    let _ = s.chart.get_or_insert_with(super::super::chart);
    if let Some(c) = s.chart.as_mut() {
        for a in c.iter_mut() {
            if a.id == super::super::account_id(6250) {
                a.is_active = false;
            }
        }
    }
    let before = s.ledger.transactions().len();
    let r = run(&mut s, d(2026, 11, 4));
    assert!(r.posted.is_empty(), "skipped, not posted");
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].reason, SkipReason::InvalidAccount);
    assert_eq!(s.ledger.transactions().len(), before, "nothing posted");
}

#[test]
fn remind_mode_lists_not_posts() {
    let mut s = open_books();
    let id = make(
        &mut s,
        "Rent",
        schedule(ScheduleMode::Remind, d(2026, 8, 31), ScheduleEnd::Never),
    );
    let before = s.ledger.transactions().len();
    let r = run(&mut s, d(2026, 11, 4));
    assert!(r.posted.is_empty(), "reminders never post");
    assert_eq!(r.reminders.len(), 1);
    assert_eq!(
        r.reminders[0].dates,
        vec![d(2026, 8, 31), d(2026, 9, 30), d(2026, 10, 31)]
    );
    assert_eq!(r.reminders[0].count, 3);
    assert_eq!(s.ledger.transactions().len(), before);
    assert_eq!(
        template_store::get(&s, id)
            .expect("t")
            .schedule
            .expect("s")
            .next_date,
        d(2026, 8, 31),
        "not advanced"
    );
    // Skip this one: the next date moves on.
    skip(&mut s, id, d(2026, 8, 31)).expect("skip");
    assert_eq!(attention(&s, d(2026, 11, 4)).reminders[0].count, 2);
    assert!(cl_api::is_occurrence_taken(
        &skip(&mut s, id, d(2026, 8, 31)).expect_err("again")
    ));
    assert_eq!(
        skip(&mut s, id, d(2026, 12, 31)).expect_err("not due"),
        ApiError::Local(LocalValidation::InvalidValue)
    );
}

#[test]
fn ends_after_count() {
    let mut s = open_books();
    let id = make(
        &mut s,
        "Software",
        schedule(ScheduleMode::Auto, d(2026, 1, 15), ScheduleEnd::After(3)),
    );
    let r = run(&mut s, d(2026, 11, 4));
    assert_eq!(r.posted.len(), 3);
    assert!(
        template_store::get(&s, id).expect("t").schedule.is_none(),
        "the schedule ended"
    );
    assert!(
        run(&mut s, d(2026, 12, 31)).posted.is_empty(),
        "no more after the count"
    );
}

#[test]
fn limit_and_more() {
    let mut s = open_books();
    make(
        &mut s,
        "Daily",
        TemplateSchedule {
            frequency: Frequency::Daily,
            ..schedule(ScheduleMode::Auto, d(2026, 10, 1), ScheduleEnd::Never)
        },
    );
    let run_id = new_run_id();
    let as_of = d(2026, 10, 31);
    let (a, _) = run_due(&mut s, as_of, run_id, 20, as_of).expect("a");
    assert!(a.more);
    assert_eq!(a.posted.len(), 20);
    let (b, _) = run_due(&mut s, as_of, run_id, 20, as_of).expect("b");
    assert!(!b.more);
    assert_eq!(b.posted.len(), 31, "every post of the run");
    assert_eq!(
        run_due(&mut s, as_of, run_id, 0, as_of).expect_err("limit"),
        ApiError::Local(LocalValidation::PageLimit)
    );
    assert!(
        run_due(&mut s, d(2026, 11, 2), run_id, 20, as_of).is_err(),
        "future"
    );
}

#[test]
fn resolve_links_a_form_save_once() {
    let mut s = open_books();
    let id = make(
        &mut s,
        "Rent",
        schedule(ScheduleMode::Remind, d(2026, 10, 1), ScheduleEnd::Never),
    );
    let txn = TransactionId::parse("d0000000-0000-4000-8000-b00000000777").expect("t");
    resolve(&mut s, id, d(2026, 10, 1), txn).expect("resolved");
    assert_eq!(
        s.occurrences.get(&(id, d(2026, 10, 1))).map(|o| o.status),
        Some(OccStatus::Entered)
    );
    assert_eq!(
        template_store::get(&s, id)
            .expect("t")
            .schedule
            .expect("s")
            .next_date,
        d(2026, 11, 1)
    );
    assert!(
        resolve(&mut s, id, d(2026, 11, 1), txn).is_err(),
        "already linked"
    );
}

#[test]
fn the_sample_templates_catch_up_on_the_first_open() {
    let mut s = DemoState::new();
    seed_sample(&mut s);
    seed_sample(&mut s);
    assert_eq!(s.templates.len(), 3, "added once");
    assert_eq!(s.template_groups.len(), 1);
    let before = owed_on_card(&s);
    let today = DemoState::today();
    let r = run(&mut s, today);
    // The demo's books are closed through 09/30: 09/15 waits for the
    // password, 10/15 posts.
    assert_eq!(
        r.posted.iter().map(|p| p.date).collect::<Vec<_>>(),
        vec![d(2026, 10, 15)]
    );
    assert_eq!(r.posted[0].name, "Accounting software");
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].reason, SkipReason::ClosedPeriod);
    assert!(r.reminders.is_empty(), "Shop rent is due 12/01");
    assert_eq!(owed_on_card(&s).checked_sub(before), Ok(m("49.00")));
    // A re-open: nothing new.
    let again = run(&mut s, today);
    assert!(again.posted.is_empty(), "a re-open posts nothing");
    assert!(again.skipped.iter().all(|k| !k.new));
    // The live sample (no closing date) posts both: +98.00.
    let mut live = open_books();
    seed_sample(&mut live);
    let before = owed_on_card(&live);
    assert_eq!(run(&mut live, today).posted.len(), 2);
    assert_eq!(owed_on_card(&live).checked_sub(before), Ok(m("98.00")));
}
