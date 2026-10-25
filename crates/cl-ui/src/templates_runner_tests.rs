//! The catch-up runner on the demo engine (P15 C3): open → run → summary;
//! a re-open posts nothing and opens nothing; a new date, the hour and
//! "Check now" run again.

use super::*;
use crate::company::demo::DemoState;

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>, TenantId) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let tenant = link
        .demo_state()
        .and_then(|s| s.companies.first().map(|c| c.tenant_id))
        .expect("tenant");
    (dir, link, tenant)
}

fn posted_count(link: &CompanyLink) -> usize {
    link.demo_state()
        .map(|s| {
            s.occurrences
                .values()
                .filter(|o| o.status == demo::OccStatus::Posted)
                .count()
        })
        .unwrap_or_default()
}

#[test]
fn opened_runs_once_and_a_reopen_opens_nothing() {
    let (_dir, link, tenant) = demo();
    let today = DemoState::today();
    let mut r = TemplatesRunner::default();
    assert_eq!(r.tick(&link, today, 0.0), None, "nothing before an open");
    r.opened(&link, tenant);
    let first = r.tick(&link, today, 0.0).expect("ran");
    assert_eq!(
        first,
        Finished {
            posted: true,
            open_window: true
        }
    );
    assert_eq!(
        posted_count(&link),
        1,
        "10/15 (09/15 is in the closed period)"
    );
    {
        let board = link.due();
        let run = board.run().expect("answer");
        assert_eq!(run.posted.len(), 1);
        assert_eq!(run.skipped.len(), 1);
        assert!(run.skipped[0].new);
        assert_eq!(board.needs_you(), 1, "the chip");
    }
    // Same frame clock, same day: nothing more.
    assert_eq!(r.tick(&link, today, 1.0), None);
    // File > Open Company, the same company: a run, nothing new.
    r.opened(&link, tenant);
    let again = r.tick(&link, today, 2.0).expect("ran again");
    assert_eq!(
        again,
        Finished {
            posted: false,
            open_window: false
        }
    );
    assert_eq!(posted_count(&link), 1, "exactly once across the re-open");
    assert_eq!(link.due().needs_you(), 1, "the old skip keeps the chip");
}

#[test]
fn midnight_the_hour_and_check_now_run_again() {
    let (_dir, link, tenant) = demo();
    let mut r = TemplatesRunner::default();
    r.opened(&link, tenant);
    let today = DemoState::today();
    assert!(r.tick(&link, today, 0.0).is_some());
    assert_eq!(r.runs, 1);
    // The local date changes.
    assert!(r.tick(&link, d(2026, 11, 5), 10.0).is_some());
    assert_eq!(r.runs, 2);
    // An hour later.
    assert!(r.tick(&link, d(2026, 11, 5), 11.0).is_none());
    assert!(r.tick(&link, d(2026, 11, 5), 10.0 + HOURLY_SECS).is_some());
    assert_eq!(r.runs, 3);
    // "Check now".
    link.due().request_check();
    assert!(
        r.tick(&link, d(2026, 11, 5), 10.0 + HOURLY_SECS + 1.0)
            .is_some()
    );
    assert_eq!(r.runs, 4);
    // 11/15 is due by then (the third time ends the schedule).
    assert!(
        r.tick(&link, d(2026, 11, 15), HOURLY_SECS * 3.0)
            .is_some_and(|f| f.posted)
    );
    assert_eq!(posted_count(&link), 2);
    r.closed(&link);
    assert!(link.due().run().is_none());
    assert!(!r.armed());
}

#[test]
fn reminders_open_the_window_once_a_session() {
    let (_dir, link, tenant) = demo();
    let mut r = TemplatesRunner::default();
    r.opened(&link, tenant);
    // Shop rent is due 12/01.
    let first = r.tick(&link, d(2026, 12, 1), 0.0).expect("ran");
    assert!(first.open_window);
    link.due().request_check();
    let second = r.tick(&link, d(2026, 12, 1), 1.0).expect("ran");
    assert!(!second.open_window, "the same reminder: no popup again");
    let reminder = link.due().run().expect("run").reminders[0].clone();
    assert_eq!(reminder.dates, vec![d(2026, 12, 1)]);
    link.due().handled(reminder.template, d(2026, 12, 1));
    assert!(
        link.due().run().expect("run").reminders.is_empty(),
        "handled: off the board"
    );
}

#[test]
fn the_board_folds_rounds_and_keeps_new_skips() {
    let run_id = cl_api::new_run_id();
    let t = TemplateId::parse("d0000000-0000-4000-8000-e00000000001").expect("id");
    let skip = |new| cl_api::SkippedOccurrence {
        template: t,
        name: String::new(),
        date: d(2026, 9, 15),
        reason: cl_api::SkipReason::ClosedPeriod,
        new,
    };
    let round = |new| RunDue {
        as_of: d(2026, 11, 4),
        run_id,
        more: false,
        posted: Vec::new(),
        reminders: Vec::new(),
        skipped: vec![skip(new)],
        failed: Vec::new(),
    };
    let mut seen = BTreeSet::new();
    let a = fold(None, round(true), &mut seen);
    let b = fold(Some(a), round(false), &mut seen);
    assert!(b.skipped[0].new, "skipped by an earlier round of this run");
    assert!(transient(&ApiError::Timeout));
    assert!(!transient(&ApiError::Internal));
}

fn answer(more: bool, capped: bool, posted: usize) -> RunDue {
    let t = TemplateId::parse("d0000000-0000-4000-8000-e00000000003").expect("id");
    RunDue {
        as_of: d(2026, 11, 4),
        run_id: cl_api::new_run_id(),
        more,
        posted: (0..posted)
            .map(|i| cl_api::PostedOccurrence {
                template: t,
                name: String::new(),
                kind: cl_api::TemplateKind::Transfer,
                client_form: None,
                date: d(2026, 1, 1 + u32::try_from(i % 28).expect("small")),
                transaction: cl_core::ids::TransactionId::parse(
                    "e0000000-0000-4000-8000-e00000000001",
                )
                .expect("id"),
                total: None,
            })
            .collect(),
        reminders: Vec::new(),
        skipped: if capped {
            vec![cl_api::SkippedOccurrence {
                template: t,
                name: String::new(),
                date: d(2026, 2, 1),
                reason: cl_api::SkipReason::CatchUpCapped,
                new: true,
            }]
        } else {
            Vec::new()
        },
        failed: Vec::new(),
    }
}

/// Runs [`gather`] over scripted answers (then "still capped" forever);
/// how many calls it made.
fn scripted(mut answers: Vec<Result<RunDue, ApiError>>) -> (Gathered, usize) {
    answers.reverse();
    let calls = std::cell::Cell::new(0);
    let script = std::cell::RefCell::new(answers);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let g = rt.block_on(gather(|| {
        calls.set(calls.get() + 1);
        let next = script
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| Ok(answer(false, true, 0)));
        async move { next }
    }));
    (g, calls.get())
}

#[test]
fn capped_schedules_are_asked_again_a_bounded_number_of_times() {
    // Capped, then clear: two calls.
    let (g, calls) = scripted(vec![
        Ok(answer(false, true, 31)),
        Ok(answer(false, false, 40)),
    ]);
    assert_eq!(calls, 2);
    assert!(g.error.is_none());
    assert_eq!(g.run.expect("run").posted.len(), 40, "the run's posts");
    // Capped every time: 1 + MAX_CAPPED_ROUNDS calls, then left for the
    // next run (the capped row stays on the board, with no Dismiss).
    let (g, calls) = scripted(Vec::new());
    assert_eq!(calls, 1 + MAX_CAPPED_ROUNDS);
    assert!(g.run.expect("run").skipped[0].reason.is_capped());
    // `more` keeps asking without using the capped budget.
    let (_, calls) = scripted(vec![
        Ok(answer(true, false, 1)),
        Ok(answer(true, false, 2)),
        Ok(answer(false, false, 3)),
    ]);
    assert_eq!(calls, 3);
}

#[test]
fn a_failed_later_round_keeps_what_was_gathered_and_the_run_id() {
    let (g, calls) = scripted(vec![Ok(answer(true, false, 2)), Err(ApiError::Internal)]);
    assert_eq!(calls, 2);
    assert!(matches!(g.error, Some(ApiError::Internal)));
    assert_eq!(g.run.as_ref().map(|r| r.posted.len()), Some(2));
    // A lost reply is asked once more under the same id.
    let (lost, calls) = scripted(vec![Err(ApiError::Timeout), Ok(answer(false, false, 1))]);
    assert_eq!(calls, 2);
    assert!(lost.error.is_none());
    // The runner shows the posts, and the next run that day reuses the id.
    let (_dir, link, tenant) = demo();
    let mut r = TemplatesRunner::default();
    r.opened(&link, tenant);
    let today = d(2026, 11, 4);
    let id = cl_api::new_run_id();
    r.current = Some((today, id));
    let f = r.finish(&link, Ok(g)).expect("shown");
    assert!(f.posted && f.open_window, "published and summarised");
    assert_eq!(link.due().run().map(|x| x.posted.len()), Some(2));
    assert_eq!(r.retry, Some((today, id)));
    assert_eq!(r.run_id_for(today), id, "the retry asks under the same id");
    // Another day starts fresh; a clean run forgets the id.
    r.retry = Some((today, id));
    assert_ne!(r.run_id_for(d(2026, 11, 5)), id);
    r.current = Some((today, id));
    let _ = r.finish(
        &link,
        Ok(Gathered {
            run: Some(answer(false, false, 0)),
            error: None,
            questions: Vec::new(),
        }),
    );
    assert_eq!(r.retry, None);
    // No permission: silent, nothing kept.
    r.current = Some((today, id));
    assert!(
        r.finish(&link, Err(ApiError::Forbidden { code: None }))
            .is_none()
    );
    assert_eq!(r.retry, None);
    // A capped row is no chip and opens nothing.
    link.due().set(answer(false, true, 0));
    assert_eq!(link.due().needs_you(), 0);
}

/// An automatic daily transfer from `next` (owner answer 2026-10-24).
fn add_auto(link: &CompanyLink, name: &str, next: CalendarDate) -> TemplateId {
    let mut s = link.demo_state().expect("demo");
    let draft = cl_api::TemplateDraft::Transfer(cl_api::TransferDraft {
        from: crate::company::demo::account_id(1010),
        to: crate::company::demo::account_id(1020),
        date: next,
        amount: cl_core::Money::parse_wire("25.00").expect("m"),
        memo: None,
    });
    crate::company::demo::template_store::create(
        &mut s,
        &cl_api::NewTemplate {
            name: name.into(),
            group: None,
            client_form: None,
            draft,
            schedule: Some(cl_api::TemplateSchedule {
                mode: cl_api::ScheduleMode::Auto,
                frequency: cl_api::Frequency::Daily,
                interval: 1,
                next_date: next,
                end: cl_api::ScheduleEnd::Never,
            }),
        },
    )
    .expect("template")
    .id
}

fn posted_for(link: &CompanyLink, template: TemplateId) -> usize {
    link.demo_state()
        .map(|s| {
            s.occurrences
                .iter()
                .filter(|((t, _), o)| *t == template && o.status == demo::OccStatus::Posted)
                .count()
        })
        .unwrap_or_default()
}

#[test]
fn four_missed_dates_ask_first_and_post_once_after_post_all() {
    let (_dir, link, tenant) = demo();
    let today = DemoState::today();
    let id = add_auto(&link, "Daily sweep", today.add_days(-3).expect("d"));
    let mut r = TemplatesRunner::default();
    r.opened(&link, tenant);
    let f = r.tick(&link, today, 0.0).expect("asked");
    assert_eq!(
        f,
        Finished {
            posted: false,
            open_window: true
        },
        "Templates Due opens with the question"
    );
    assert_eq!(posted_count(&link), 0, "nothing posted before the answer");
    {
        let board = link.due();
        let q = board.questions();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].template, id);
        assert_eq!(q[0].dates.len(), 4);
    }
    // Asking again (the hour, Check now) still posts nothing.
    link.due().request_check();
    let _ = r.tick(&link, today, 1.0);
    assert_eq!(posted_count(&link), 0);
    // Post all: the next run posts the four dates (and the others).
    link.due().confirm(id);
    assert!(link.due().questions().is_empty());
    let f = r.tick(&link, today, 2.0).expect("ran");
    assert!(f.posted);
    assert_eq!(posted_for(&link, id), 4);
    // Nothing posts twice.
    link.due().request_check();
    let _ = r.tick(&link, today, 3.0);
    assert_eq!(posted_for(&link, id), 4);
    assert!(link.due().questions().is_empty());
}

#[test]
fn three_missed_dates_post_automatically_as_before() {
    let (_dir, link, tenant) = demo();
    let today = DemoState::today();
    let id = add_auto(&link, "Daily sweep", today.add_days(-2).expect("d"));
    let mut r = TemplatesRunner::default();
    r.opened(&link, tenant);
    let f = r.tick(&link, today, 0.0).expect("ran");
    assert!(f.posted);
    assert!(link.due().questions().is_empty(), "no question");
    assert_eq!(posted_for(&link, id), 3);
    assert_eq!(
        link.due().run().map(|x| x.posted.len()),
        Some(4),
        "the summary: the three and the sample's one"
    );
}
