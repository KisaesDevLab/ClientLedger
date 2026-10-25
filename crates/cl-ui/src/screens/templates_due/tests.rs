//! Templates Due on the demo engine: the first open's catch-up, Enter now
//! with the closing password, Skip, Open, and "Enter it now" through the
//! form (the form's own password prompt, then the date is marked entered).

use super::*;
use crate::company::demo::DemoState;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;
use crate::screens::company::form::SecretText;
use crate::templates_runner::TemplatesRunner;
use cl_api::{Frequency, NewTemplate, ScheduleEnd, ScheduleMode, TemplateSchedule, TransferDraft};

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

/// Opens the demo's first company and runs the catch-up once.
fn caught_up(link: &Arc<CompanyLink>, today: CalendarDate) -> TemplatesRunner {
    let tenant = link
        .demo_state()
        .and_then(|s| s.companies.first().map(|c| c.tenant_id))
        .expect("tenant");
    let mut r = TemplatesRunner::default();
    r.opened(link, tenant);
    assert!(r.tick(link, today, 0.0).is_some());
    r
}

fn screen(link: &Arc<CompanyLink>, today: CalendarDate) -> TemplatesDueScreen {
    let mut s = TemplatesDueScreen::new(Arc::clone(link), None, today);
    s.poll();
    s
}

fn select(s: &mut TemplatesDueScreen, section: &str) {
    s.selected = s.rows.iter().position(|r| r.section.word() == section);
    assert!(s.selected.is_some(), "a {section} row");
}

fn remind(link: &Arc<CompanyLink>, name: &str, next: CalendarDate) -> TemplateId {
    let mut st = link.demo_state().expect("demo");
    template_store::create(
        &mut st,
        &NewTemplate {
            name: name.into(),
            group: None,
            client_form: None,
            draft: cl_api::TemplateDraft::Transfer(TransferDraft {
                from: crate::company::demo::account_id(1010),
                to: crate::company::demo::account_id(1020),
                date: next,
                amount: Money::parse_wire("25.00").expect("m"),
                memo: None,
            }),
            schedule: Some(TemplateSchedule {
                mode: ScheduleMode::Remind,
                frequency: Frequency::Monthly,
                interval: 1,
                next_date: next,
                end: ScheduleEnd::Never,
            }),
        },
    )
    .expect("template")
    .id
}

#[test]
fn the_first_open_shows_what_was_entered_and_what_waits() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let _r = caught_up(&link, today);
    let mut s = screen(&link, today);
    let st = s.state();
    assert_eq!(st["as_of"], "2026-11-04");
    assert_eq!(st["entered"], 1);
    assert_eq!(st["skipped"], 1);
    assert_eq!(st["waiting"], 0);
    assert_eq!(st["rows"][1]["reason"], "CLOSED_PERIOD");
    let posted = s.rows[0].clone();
    assert_eq!(posted.name, "Accounting software");
    assert_eq!(posted.amount, Money::parse_wire("49.00").ok());
    assert_eq!(
        s.account_text(posted.template),
        "2050 \u{b7} Business Credit Card"
    );
    // Open: the posted charge in its form.
    select(&mut s, "entered");
    s.default_action(None);
    let events = link.drain();
    assert!(
        events.iter().any(|e| matches!(
            e,
            CompanyEvent::OpenRecord(k, _) if k.as_str() == crate::screens::card_charge::KIND
        )),
        "{events:?}"
    );
    // Check now: the runner runs again at its next frame.
    s.check_now();
    assert_eq!(s.state()["checking"], true);
}

#[test]
fn enter_it_now_goes_through_the_form_and_its_password() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let _r = caught_up(&link, today);
    let mut s = screen(&link, today);
    select(&mut s, "skipped");
    s.default_action(None);
    assert_eq!(s.last_action, Some("open_in_form"));
    let open = link
        .drain()
        .into_iter()
        .find_map(|e| match e {
            CompanyEvent::OpenTemplate(o) => Some(*o),
            _ => None,
        })
        .expect("OpenTemplate");
    assert_eq!(open.occurrence, Some(d(2026, 9, 15)));
    assert_eq!(open.kind.as_str(), crate::screens::card_charge::KIND);
    // The card form, filled for 09/15, saves with the closing password.
    let mut form =
        crate::screens::card_charge::CardChargeScreen::new(Arc::clone(&link), None, today);
    form.poll();
    assert!(form.open_template(open));
    assert!(!form.save(), "the closed period asks first");
    assert!(form.flow.closing_prompt.dialog_open());
    assert_eq!(link.due().needs_you(), 1);
    let pw = Secret::new(DEMO_CLOSING_PASSWORD.to_string());
    assert!(
        form.start_save(cl_model::forms::SaveIntent::Save, Some(pw)),
        "{}",
        form.state()
    );
    assert_eq!(link.due().needs_you(), 0, "marked entered, off the list");
    s.poll();
    assert_eq!(s.state()["skipped"], 0);
    let occ = link
        .demo_state()
        .and_then(|st| {
            st.occurrences
                .iter()
                .find(|((_, d0), _)| *d0 == d(2026, 9, 15))
                .map(|(_, o)| o.status)
        })
        .expect("occurrence");
    assert_eq!(occ, demo::OccStatus::Entered);
}

#[test]
fn enter_now_asks_for_the_closing_password_and_skip_moves_on() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    // Closed through 09/30: a reminder from 09/30 waits for the password.
    let id = remind(&link, "Savings", d(2026, 9, 30));
    let _r = caught_up(&link, today);
    let mut s = screen(&link, today);
    assert_eq!(s.state()["waiting"], 2, "09/30 and 10/30");
    let mut watcher = link.bus().subscribe();
    s.selected = s
        .rows
        .iter()
        .position(|r| r.template == id && r.date == Some(d(2026, 9, 30)));
    s.default_action(None);
    assert!(s.closing_prompt.dialog_open(), "{:?}", s.error);
    assert!(link.bus().take(&mut watcher).is_empty(), "nothing posted");
    s.closing_prompt.password = SecretText::from(DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    assert_eq!(s.last_action, Some("enter"));
    assert!(s.entered.is_some());
    assert!(
        link.bus().take(&mut watcher).ledger_any(),
        "registers hear it"
    );
    assert_eq!(s.state()["waiting"], 1);
    // Skip this one: 10/30 goes, the schedule moves to 11/30.
    s.selected = s.rows.iter().position(|r| r.template == id);
    s.skip_selected();
    assert_eq!(s.last_action, Some("skip"));
    assert_eq!(s.state()["waiting"], 0);
    let next = link
        .demo_state()
        .and_then(|st| template_store::get(&st, id).ok())
        .and_then(|t| t.schedule)
        .map(|sc| sc.next_date);
    assert_eq!(next, Some(d(2026, 11, 30)));
}

/// Review fix-PR-1 blocking 2: an Enter now lost in Templates (or in a
/// window closed since) blocks Enter now and Open in form of that template
/// here too, with the banner.
#[test]
fn a_lost_enter_now_from_another_window_blocks_this_template_here() {
    use crate::forms::lost_save::LostScope;
    let (_dir, link) = demo();
    let today = DemoState::today();
    let id = remind(&link, "Savings", d(2026, 10, 30));
    let _r = caught_up(&link, today);
    link.lost_saves()
        .remember(LostScope::Template(id), Some(cl_api::IdempotencyKey::new()));
    let mut s = screen(&link, today);
    assert!(s.lost.pending(), "the banner shows on open");
    let mut watcher = link.bus().subscribe();
    s.selected = s.rows.iter().position(|r| r.template == id);
    assert!(s.selected.is_some());
    s.enter_now(None);
    assert!(
        s.entered.is_none() && s.last_action.is_none(),
        "nothing sent"
    );
    s.open_in_form();
    assert!(
        !link
            .drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::OpenTemplate(_))),
        "no form to enter it a second time"
    );
    assert!(link.bus().take(&mut watcher).is_empty());
    assert_eq!(s.state()["waiting"], 1);
    // The owner looked and confirms: Enter now works again.
    s.lost.ask_owner();
    s.lost
        .owner_confirmed(&mut s.lost_gate, "templates_due_enter");
    assert!(!link.lost_saves().is_open(LostScope::Template(id)));
    s.enter_now(None);
    assert_eq!(s.last_action, Some("enter"));
}

#[test]
fn keys_and_the_state_name_no_owner_text() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let _r = caught_up(&link, today);
    let mut s = screen(&link, today);
    s.selected = Some(0);
    s.step(true);
    assert_eq!(s.selected, Some(1));
    s.step(true);
    assert_eq!(s.selected, Some(1), "stays on the last row");
    s.step(false);
    assert_eq!(s.selected, Some(0));
    let text = s.state().to_string();
    assert!(!text.contains("Accounting software"), "no names");
    assert!(!text.contains("49.00"), "no amounts");
    assert!(s.command(CommandId::new("edit.edit_transaction")));
    assert_eq!(s.title(), t("cmd.lists.templates_due"));
}

#[test]
fn nothing_due_and_failed_rows() {
    let (_dir, link) = demo();
    let s = screen(&link, DemoState::today());
    assert_eq!(s.state()["entered"], 0);
    assert!(s.rows.is_empty(), "no run yet");
    let t = TemplateId::parse("d0000000-0000-4000-8000-e00000000001").expect("id");
    let run = RunDue {
        as_of: d(2026, 11, 4),
        run_id: cl_api::new_run_id(),
        more: false,
        posted: Vec::new(),
        reminders: Vec::new(),
        skipped: Vec::new(),
        failed: vec![cl_api::FailedTemplate {
            template: t,
            reason: "INTERNAL".into(),
        }],
    };
    let rows = rows_of(&run, &BTreeMap::new());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].section, Section::Failed("INTERNAL".into()));
}

#[test]
fn nothing_starts_while_a_call_is_on_its_way() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    remind(&link, "Savings", d(2026, 10, 30));
    let _r = caught_up(&link, today);
    let mut s = screen(&link, today);
    select(&mut s, "waiting");
    // A call still running (it never answers here).
    s.open_job = Some(crate::screens::templates::engine::Job::Done(None));
    s.enter_now(None);
    assert!(s.enter_job.is_none(), "Enter waits for the running call");
    s.skip_selected();
    assert!(s.skip_job.is_none(), "Skip waits too");
    assert_eq!(s.state()["waiting"], 1, "nothing changed");
}

#[test]
fn a_capped_catch_up_has_no_skip_or_dismiss() {
    let (_dir, link) = demo();
    let mut s = screen(&link, DemoState::today());
    let tid = TemplateId::parse("d0000000-0000-4000-8000-e00000000002").expect("id");
    let skipped = |reason| cl_api::SkippedOccurrence {
        template: tid,
        name: String::new(),
        date: d(2026, 1, 31),
        reason,
        new: true,
    };
    let run = RunDue {
        as_of: d(2026, 11, 4),
        run_id: cl_api::new_run_id(),
        more: false,
        posted: Vec::new(),
        reminders: Vec::new(),
        skipped: vec![
            skipped(SkipReason::CatchUpCapped),
            skipped(SkipReason::Permission),
        ],
        failed: Vec::new(),
    };
    s.rows = rows_of(&run, &BTreeMap::new());
    assert!(s.rows[0].capped() && !s.rows[1].capped());
    let st = s.state();
    assert_eq!(st["rows"][0]["reason"], "CATCH_UP_CAPPED");
    assert_eq!(st["rows"][0]["can_dismiss"], false);
    assert_eq!(st["rows"][1]["can_dismiss"], true);
    // Dismiss (keyboard or channel) does nothing on a capped row.
    s.selected = Some(0);
    s.skip_selected();
    assert!(s.skip_job.is_none(), "nothing sent");
    // Plain words for both new reasons.
    for code in ["CATCH_UP_CAPPED", "PERMISSION"] {
        let key = cl_model::templates::reason_key(code);
        assert_ne!(key.as_str(), "templates_due.reason.other");
        assert_ne!(t(key.as_str()), key.as_str(), "has English text");
    }
}

#[test]
fn s2_a_failed_link_shows_already_entered_and_is_tried_again_with_the_saved_txn() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let tid = remind(&link, "Savings", today);
    let mut r = caught_up(&link, today);
    let txn = {
        let mut st = link.demo_state().expect("demo");
        let chart = st
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let p = crate::company::demo::register::DemoPosting {
            kind: "transfer",
            date: today,
            number: String::new(),
            contact: None,
            memo: String::new(),
            own: crate::company::demo::account_id(1010),
            own_debit: false,
            lines: vec![(
                crate::company::demo::account_id(1020),
                Money::parse_wire("25.00").expect("m"),
            )],
        };
        st.ledger.save(&chart, &p, None).expect("saved")
    };
    // The form saved; its link to the date failed (a lost reply, say).
    link.due().linking(tid, today, txn);
    link.due().link_failed(tid, today);
    let mut s = screen(&link, today);
    let row = s
        .rows
        .iter()
        .position(|r| r.template == tid)
        .expect("the row");
    assert_eq!(s.rows[row].section, Section::Linking, "already entered");
    s.selected = Some(row);
    s.default_action(None);
    s.enter_now(None);
    assert!(
        s.enter_job.is_none() && s.open_job.is_none(),
        "never Enter it now"
    );
    // The runner tries the link again with the saved transaction.
    let _ = r.tick(&link, today, 100.0);
    assert!(!link.due().is_linking(tid, today));
    let st = link.demo_state().expect("demo");
    let occ = st.occurrences.get(&(tid, today)).expect("linked");
    assert_eq!(occ.transaction, Some(txn));
    drop(st);
    s.poll();
    assert!(s.rows.iter().all(|r| r.template != tid), "handled");
}

/// Review fix-PR-1 should-fix 5: a link the engine refused for good never
/// brings "Enter it now" back: the date shows "Saved separately" with
/// Dismiss, which goes through the skip path.
#[test]
fn a_refused_link_shows_saved_separately_and_dismiss_settles_it() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let tid = remind(&link, "Savings", today);
    let _r = caught_up(&link, today);
    let txn = TransactionId::parse("d0000000-0000-4000-8000-b0000000aa02").expect("t");
    link.due().linking(tid, today, txn);
    link.due().link_refused(tid, today);
    let mut s = screen(&link, today);
    let row = s
        .rows
        .iter()
        .position(|r| r.template == tid)
        .expect("the row");
    assert_eq!(s.rows[row].section, Section::SavedSeparately);
    assert_eq!(s.state()["rows"][row]["saved_separately"], true);
    assert_eq!(s.state()["rows"][row]["can_dismiss"], true);
    s.selected = Some(row);
    s.enter_now(None);
    s.open_in_form();
    assert!(
        s.enter_job.is_none() && s.open_job.is_none(),
        "never entered again"
    );
    // Dismiss (Enter on the row): skipped in the books, off the board.
    s.default_action(None);
    assert_eq!(s.last_action, Some("skip"));
    assert!(!link.due().is_saved_separately(tid, today));
    s.poll();
    assert!(s.rows.iter().all(|r| r.template != tid), "handled");
}

#[test]
fn catch_up_skip_these_sends_each_date_through_the_skip_path() {
    let (_dir, link) = demo();
    let today = DemoState::today();
    let tid = {
        let mut st = link.demo_state().expect("demo");
        template_store::create(
            &mut st,
            &NewTemplate {
                name: "Daily sweep".into(),
                group: None,
                client_form: None,
                draft: cl_api::TemplateDraft::Transfer(TransferDraft {
                    from: crate::company::demo::account_id(1010),
                    to: crate::company::demo::account_id(1020),
                    date: today,
                    amount: Money::parse_wire("25.00").expect("m"),
                    memo: None,
                }),
                schedule: Some(TemplateSchedule {
                    mode: ScheduleMode::Auto,
                    frequency: Frequency::Daily,
                    interval: 1,
                    next_date: today.add_days(-3).expect("d"),
                    end: ScheduleEnd::Never,
                }),
            },
        )
        .expect("template")
        .id
    };
    let mut r = caught_up(&link, today);
    let mut s = screen(&link, today);
    assert_eq!(s.questions.len(), 1);
    assert_eq!(s.questions[0].dates.len(), 4);
    s.catch_up_skip(tid);
    s.poll();
    assert_eq!(s.last_action, Some("catch_up_skip"));
    {
        let st = link.demo_state().expect("demo");
        let skipped = st
            .occurrences
            .iter()
            .filter(|((t, _), o)| {
                *t == tid && o.status != crate::company::demo::templates::OccStatus::Posted
            })
            .count();
        assert_eq!(skipped, 4, "each date kept as skipped, none lost");
    }
    // The run goes on and posts nothing for it.
    let _ = r.tick(&link, today, 1.0);
    let st = link.demo_state().expect("demo");
    assert!(
        st.occurrences.iter().all(|((t, _), o)| *t != tid
            || o.status != crate::company::demo::templates::OccStatus::Posted)
    );
    drop(st);
    assert_eq!(link.due().questions().len(), 0);
}
