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
    let at_finish = (v["left"]["checked"].clone(), v["right"]["checked"].clone());
    assert_eq!(at_finish, (serde_json::json!(5), serde_json::json!(8)));
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
    // Review m5: Continue shows the checks exactly as at Finish.
    s.continue_open();
    s.poll();
    let v = state(&s);
    assert_eq!(v["step"], "window");
    assert_eq!(
        (v["left"]["checked"].clone(), v["right"]["checked"].clone()),
        at_finish
    );
    s.back_to_begin();
    s.poll();
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
    // Review D1: a wrong password asks again, in the dialog, cleared.
    s.password = "not it".into();
    s.finish();
    s.poll();
    let v = state(&s);
    assert_eq!(v["dialog"], "closingpassword", "{v}");
    assert_eq!(v["password_rejected"], true);
    assert!(s.password.is_empty(), "the wrong password is gone");
    assert_eq!(v["step"], "window");
    s.password = crate::company::demo::register::DEMO_CLOSING_PASSWORD.into();
    s.finish();
    s.poll();
    assert_eq!(state(&s)["last"], "finished");
    assert_eq!(state(&s)["password_rejected"], false);
}

#[test]
fn statement_date_on_or_before_the_last_one_is_refused() {
    // Review E3: Begin's own check, then the engine's refusal (a history
    // not read yet) shows the same words.
    let (_d, _link, mut s) = screen();
    s.poll();
    s.pick_account(crate::company::demo::account_id(1010));
    s.poll();
    s.poll();
    assert!(s.last_finished().is_some(), "reconciled through 09/30");
    s.date_in
        .set(CalendarDate::from_ymd(2026, 9, 30).expect("date"));
    s.form.ending = "71436.20".into();
    s.begin();
    assert_eq!(
        state(&s)["problems"],
        serde_json::json!(["reconcile.problem.date_not_after_last"])
    );
    assert_eq!(
        begin::problem_text(&s, BeginProblem::DateNotAfterLast),
        "This account is reconciled through 09/30/2026. Use a later statement date."
    );
    s.history.clear();
    s.begin();
    s.poll();
    assert_eq!(state(&s)["step"], "begin");
    assert_eq!(
        state(&s)["problems"],
        serde_json::json!(["reconcile.problem.date_not_after_last"])
    );
    assert!(s.error.is_none(), "a Begin problem, not an error");
}

fn card_begin(s: &mut ReconcileScreen) {
    s.poll();
    s.pick_account(crate::company::demo::account_id(2050));
    s.poll();
    s.date_in
        .set(CalendarDate::from_ymd(2026, 10, 31).expect("date"));
    s.form.ending = "1,038.82".into();
    s.form.service_charge.amount = "18.25".into();
    s.begin();
    s.poll();
}

#[test]
fn card_pay_step_after_continue_uses_the_engine_balance() {
    // Review D2: Continue leaves Begin's ending text empty; the pay step
    // still offers the statement balance the engine held.
    let (_d, link, mut first) = screen();
    card_begin(&mut first);
    let mut s = ReconcileScreen::new(Arc::clone(&link), None, today());
    s.poll();
    s.pick_account(crate::company::demo::account_id(2050));
    s.poll();
    s.poll();
    s.continue_open();
    s.poll();
    assert_eq!(state(&s)["step"], "window");
    assert_eq!(s.form.ending, "", "Continue fills no ending text");
    s.mark_all(true);
    s.poll();
    s.finish();
    s.poll();
    let v = state(&s);
    assert_eq!(v["dialog"], "done");
    assert_eq!(v["card_pay"], "1,038.82");
    let _ = link.drain();
    s.dialog = Some(Dialog::CardPay);
    finish::pay(&mut s, "vendors.write_check");
    let events = link.drain();
    let Some(CompanyEvent::OpenWith(_, p)) = events.first() else {
        panic!("{events:?}");
    };
    let OpenPayload::CardPayment(pay) = &**p else {
        panic!("card payment");
    };
    assert_eq!(
        pay.amount,
        cl_core::Money::parse_wire("1038.82").expect("m")
    );
    assert_eq!(pay.date, CalendarDate::from_ymd(2026, 10, 31).expect("d"));
}

#[test]
fn no_pay_step_when_nothing_is_owed() {
    // Review D2: a zero or credit balance skips the pay step.
    let (_d, _link, mut s) = screen();
    card_begin(&mut s);
    assert!(s.card_pay_due().is_some());
    for books in ["0", "25.00"] {
        if let Some(r) = s.rec.as_mut() {
            r.statement_ending_balance = cl_core::Money::parse_wire(books).expect("m");
        }
        assert!(s.card_pay_due().is_none(), "{books}");
    }
    // A bank account never has one.
    let (_d2, _l2, mut b) = screen();
    begin_october(&mut b);
    assert!(b.card_pay_due().is_none());
}

#[test]
fn answers_about_another_worksheet_are_ignored() {
    // Review D4.
    let (_d, _link, mut s) = screen();
    begin_october(&mut s);
    let shown = s.rec.clone().expect("rec");
    let mut other = shown.clone();
    other.id = s.last_finished().expect("09/30").id;
    other.statement_ending_balance = cl_core::Money::parse_wire("1").expect("m");
    s.took(other);
    let now = s.rec.as_ref().expect("rec");
    assert_eq!(now.id, shown.id);
    assert_eq!(now.statement_ending_balance, shown.statement_ending_balance);
}

#[test]
fn a_read_from_before_finish_does_not_bring_the_worksheet_back() {
    // Review D4: a refresh landing after Finish is dropped.
    let (_d, _link, mut s) = screen();
    begin_october(&mut s);
    let id = s.rec.as_ref().map(|r| r.id).expect("id");
    s.refresh_after_error(id);
    s.back_to_begin();
    s.poll();
    assert_eq!(state(&s)["step"], "begin");
    assert!(s.rec.is_none());
}

#[test]
fn a_read_asked_before_a_tick_batch_is_read_again() {
    // Review D4: the epoch moved (a batch sent): the answer may predate
    // it, so it's read again rather than replacing the ticks.
    let (_d, _link, mut s) = screen();
    begin_october(&mut s);
    let id = s.rec.as_ref().map(|r| r.id).expect("id");
    s.refresh_after_error(id);
    s.rec_epoch = s.rec_epoch.wrapping_add(1);
    s.poll();
    assert!(s.rec_job.is_some(), "asked again");
    s.poll();
    assert!(s.rec_job.is_none());
    assert_eq!(state(&s)["step"], "window");
}

#[test]
fn finish_waits_for_a_worksheet_read() {
    // Review D4.
    let (_d, _link, mut s) = screen();
    begin_october(&mut s);
    let check = s
        .lines
        .iter()
        .find(|l| l.number.as_deref() == Some("4409"))
        .map(|l| l.journal_line)
        .expect("4409");
    s.toggle(check);
    s.poll();
    let id = s.rec.as_ref().map(|r| r.id).expect("id");
    s.refresh_after_error(id);
    s.finish();
    assert!(s.act_job.is_none(), "a read is on its way");
    s.poll();
    s.finish();
    s.poll();
    assert_eq!(state(&s)["last"], "finished");
}

#[test]
fn leave_for_now_waits_for_the_ticks() {
    // Review D5: ticks not yet at the engine keep the window open (and
    // dirty) until they arrive.
    let (_d, link, mut s) = screen();
    begin_october(&mut s);
    let _ = link.drain();
    let first = s.lines.first().map(|l| l.journal_line).expect("line");
    s.toggle(first);
    assert!(s.dirty());
    s.leave();
    let closes = |l: &Arc<CompanyLink>| {
        l.drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::CloseWindow(k) if k.as_str() == KIND))
    };
    assert!(!closes(&link), "not before the ticks are in");
    assert_eq!(state(&s)["leaving"], true);
    s.poll();
    assert!(closes(&link));
    assert!(!s.dirty());
    // The close prompt's Save waits the same way.
    s.toggle(first);
    assert!(!s.save());
    s.poll();
    assert!(closes(&link));
    assert!(s.save(), "nothing pending");
}

#[test]
fn discrepancy_report_shows_a_card_as_printed_with_the_drift() {
    // Review m4: the effect column and the drift in the owner's sign.
    let m = |s: &str| cl_core::Money::parse_wire(s).expect("m");
    let row = cl_api::DiscrepancyRow {
        kind: "changed".into(),
        statement_date: None,
        txn_date: None,
        txn_type: None,
        number: None,
        payee: None,
        reconciled_amount: Some(m("-40")),
        effect: Some(m("-12.5")),
        changed_at: None,
        changed_by: None,
        fields: vec!["amount".into()],
        transaction: None,
    };
    let d = cl_api::DiscrepancyReport {
        rows: vec![row.clone()],
        drift: Some(cl_api::BeginningDrift {
            expected: m("-100"),
            actual: m("-112.5"),
            difference: m("-12.5"),
        }),
    };
    assert_eq!(reports::effect_text(&row, ReconKind::Card), "12.50");
    assert_eq!(reports::effect_text(&row, ReconKind::Bank), "-12.50");
    let doc = reports::discrepancy_doc(&d, "2050 Card", ReconKind::Card);
    assert_eq!(doc.subtitle.len(), 2, "{:?}", doc.subtitle);
    assert!(doc.subtitle[1].contains("12.50") && !doc.subtitle[1].contains("-12.50"));
    let cl_print::reports::Section::Table { rows, .. } = &doc.sections[0] else {
        panic!("a table");
    };
    assert_eq!(rows[0].last().map(String::as_str), Some("12.50"));
    cl_print::reports::render_report(&doc).expect("pdf");
    let none = cl_api::DiscrepancyReport {
        rows: Vec::new(),
        drift: None,
    };
    assert_eq!(
        reports::discrepancy_doc(&none, "1010", ReconKind::Bank)
            .subtitle
            .len(),
        1
    );
}
