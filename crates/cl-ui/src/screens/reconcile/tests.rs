//! The Reconcile screen on the demo engine: Begin to the mockup's figures,
//! a tick to 0.00, Reconcile now, the reports, Undo, the card pay step.

use std::sync::Arc;

use cl_api::UndoMode;
use cl_core::CalendarDate;
use cl_model::CommandId;

use super::*;
use crate::company::{CompanyEvent, CompanyLink};
use crate::windows::OpenPayload;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn screen() -> (tempfile::TempDir, Arc<CompanyLink>, ReconcileScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    if let Some(mut s) = link.demo_state() {
        s.open = s.companies.first().map(|c| c.tenant_id);
    }
    let s = ReconcileScreen::new(Arc::clone(&link), None, today());
    (dir, link, s)
}

fn begin_october(s: &mut ReconcileScreen) {
    s.poll();
    let op = crate::company::demo::account_id(1010);
    s.pick_account(op);
    s.poll();
    s.date_in
        .set(CalendarDate::from_ymd(2026, 10, 31).expect("date"));
    s.form.ending = "86,902.13".into();
    s.form.service_charge.amount = "48.00".into();
    s.form.interest.amount = "12.84".into();
    s.begin();
    s.poll();
}

fn state(s: &ReconcileScreen) -> serde_json::Value {
    s.state_json()
}

#[test]
fn make_opens_only_its_kind() {
    let (_d, link, _) = screen();
    let cmd = cl_model::commands::find(KIND).expect("command");
    assert!(cmd.opens && cmd.arrives.is_none(), "built");
    let s = make(cmd.id, &link, None, today()).expect("screen");
    assert_eq!(s.title(), t("cmd.banking.reconcile"));
    assert!(!s.state().is_null());
    let other = CommandId::new("vendors.write_check");
    assert!(make(other, &link, None, today()).is_none());
}

#[test]
fn begin_shows_the_mockup_and_finishes_at_zero() {
    let (_d, link, mut s) = screen();
    begin_october(&mut s);
    let v = state(&s);
    assert_eq!(v["step"], "window", "{v}");
    assert_eq!(v["beginning"], "71,436.20");
    assert_eq!(v["ending"], "86,902.13");
    assert_eq!(v["cleared"], "87,314.31");
    assert_eq!(v["difference"], "-412.18");
    assert_eq!(v["deposits"]["count"], 6);
    assert_eq!(v["deposits"]["total"], "40,107.84");
    assert_eq!(v["payments"]["total"], "24,229.73");
    assert_eq!(v["left"]["rows"], 5);
    assert_eq!(v["right"]["rows"], 8);
    assert_eq!(v["right"]["checked"], 7);
    assert_eq!(v["can_finish"], false);
    assert_eq!(
        v["hint"],
        "One unchecked item equals the difference: check 4409 to Monett Tire & Lube."
    );
    // Later lines show with the box off; Mark all leaves hidden ones alone.
    s.hide_later = false;
    assert_eq!(state(&s)["left"]["rows"], 6);
    s.hide_later = true;
    // Tick 4409: the engine answers 0.00.
    let check = s
        .lines
        .iter()
        .find(|l| l.number.as_deref() == Some("4409"))
        .map(|l| l.journal_line)
        .expect("4409");
    s.toggle(check);
    assert_eq!(state(&s)["pending"], true);
    assert_eq!(state(&s)["can_finish"], false, "a tick waits");
    s.poll();
    let v = state(&s);
    assert_eq!(v["difference"], "0.00");
    assert_eq!(v["can_finish"], true);
    assert!(s.command(CommandId::new("reconcile.finish")));
    s.poll();
    let v = state(&s);
    assert_eq!(v["last"], "finished");
    assert_eq!(v["dialog"], "done");
    assert_eq!(v["step"], "begin");
    // The reports open as PDFs in the document tab.
    let target = s.report_target();
    assert!(target.is_some());
    let rep = link
        .demo_state()
        .map(|mut d| crate::company::demo::reconcile::report(&mut d, target.expect("id")))
        .expect("demo");
    let rep = rep.expect("report");
    cl_print::reports::render_report(&reports::report(&rep, reports::Kind::Detail)).expect("pdf");
    s.reports
        .open(&s.link, s.group.as_ref(), target, reports::Kind::Detail);
    s.poll();
    assert_eq!(
        s.reports.state()["opened"],
        "detail",
        "{}",
        s.reports.state()
    );
    assert!(link.drain().iter().any(|e| matches!(
        e,
        CompanyEvent::OpenWith(k, p)
            if k.as_str() == crate::screens::document_view::KIND
                && matches!(**p, OpenPayload::Document(_))
    )));
    // Undo, keeping the checks: back in progress.
    s.dialog = None;
    s.poll();
    assert!(s.last_finished().is_some());
    s.undo_mode = UndoMode::KeepChecks;
    s.undo_last();
    s.poll();
    assert_eq!(state(&s)["last"], "undone");
    // The history is read again (one frame to ask, one to take).
    s.poll();
    s.poll();
    assert!(s.unfinished().is_some());
    // The discrepancy report lists the undo.
    s.open_discrepancy();
    s.poll();
    assert_eq!(state(&s)["discrepancy_rows"], 1);
}

#[test]
fn mark_all_and_unmark_all_act_on_shown_lines() {
    let (_d, _link, mut s) = screen();
    begin_october(&mut s);
    s.mark_all(false);
    s.poll();
    let v = state(&s);
    assert_eq!(v["left"]["checked"], 0);
    assert_eq!(v["right"]["checked"], 0);
    s.mark_all(true);
    s.poll();
    let v = state(&s);
    assert_eq!(v["right"]["checked"], 8);
    // Later lines stay unchecked (hidden).
    assert!(
        s.lines
            .iter()
            .filter(|l| l.after_statement_date)
            .all(|l| !s.queue.ticked(l.journal_line))
    );
}

#[test]
fn begin_problems_and_unfinished_continue() {
    let (_d, _link, mut s) = screen();
    s.poll();
    s.pick_account(crate::company::demo::account_id(1010));
    s.poll();
    s.begin();
    assert_eq!(state(&s)["problems"][0], "reconcile.problem.no_ending");
    begin_october(&mut s);
    assert_eq!(s.step, Step::Window);
    // A new window: the unfinished one is offered.
    let mut again = ReconcileScreen::new(Arc::clone(&s.link), None, today());
    again.poll();
    again.poll();
    assert_eq!(state(&again)["unfinished"], true);
    again.continue_open();
    again.poll();
    assert_eq!(state(&again)["step"], "window");
    assert_eq!(state(&again)["difference"], "-412.18");
}

#[test]
fn card_pay_step_hands_off_three_ways() {
    let (_d, link, mut s) = screen();
    s.poll();
    s.pick_account(crate::company::demo::account_id(2050));
    s.poll();
    assert_eq!(state(&s)["kind"], "card");
    s.date_in
        .set(CalendarDate::from_ymd(2026, 10, 31).expect("date"));
    s.form.ending = "1,038.82".into();
    s.form.service_charge.amount = "18.25".into();
    s.begin();
    s.poll();
    assert_eq!(state(&s)["ending"], "1,038.82", "shown as printed");
    s.mark_all(true);
    s.poll();
    assert_eq!(state(&s)["difference"], "0.00");
    s.finish();
    s.poll();
    assert_eq!(state(&s)["dialog"], "done");
    for (kind, last) in [
        ("vendors.write_check", "pay_check"),
        ("vendors.enter_bill", "pay_bill"),
        ("banking.transfer", "pay_transfer"),
    ] {
        let _ = link.drain();
        s.dialog = Some(Dialog::CardPay);
        finish::pay(&mut s, kind);
        assert_eq!(s.last, Some(last));
        let events = link.drain();
        let Some(CompanyEvent::OpenWith(k, p)) = events.first() else {
            panic!("{events:?}");
        };
        assert_eq!(k.as_str(), kind);
        let OpenPayload::CardPayment(pay) = &**p else {
            panic!("card payment");
        };
        assert_eq!(pay.card, crate::company::demo::account_id(2050));
        assert_eq!(
            pay.amount,
            cl_core::Money::parse_wire("1038.82").expect("m")
        );
    }
}

#[test]
fn closed_period_asks_for_the_password() {
    let (_d, _link, mut s) = screen();
    assert!(s.command(CommandId::new("dev.demo_reconcile_close_books")));
    begin_october(&mut s);
    assert_eq!(state(&s)["posting_locked"], true);
    let check = s
        .lines
        .iter()
        .find(|l| l.number.as_deref() == Some("4409"))
        .map(|l| l.journal_line)
        .expect("4409");
    s.toggle(check);
    s.poll();
    s.finish();
    assert_eq!(state(&s)["dialog"], "closingpassword");
    s.password = crate::company::demo::register::DEMO_CLOSING_PASSWORD.into();
    s.finish();
    s.poll();
    assert_eq!(state(&s)["last"], "finished");
}
