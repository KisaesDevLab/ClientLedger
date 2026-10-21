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
