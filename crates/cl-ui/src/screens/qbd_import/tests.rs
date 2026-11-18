//! The QuickBooks Desktop import against the demo engine (P31 D3): the
//! walk to Import, the backup before every commit (a failed backup sends
//! nothing), one commit with progress, a lost answer (Check status, Try
//! again with the same key, Cancel/Back off, nothing discarded), the
//! closing-date password, refusals in owner words, undo with blockers and
//! the restore, and the close prompt only while a job runs.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 18).expect("date")
}

struct Fixture {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    {
        // The demo story's books are closed through 09/30/2026: open them.
        let mut d = link.demo_state().expect("demo");
        let open = d.open.expect("open");
        if let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open) {
            c.closed_through = None;
        }
    }
    Fixture { _dir: dir, link }
}

fn screen(f: &Fixture) -> ImportScreen {
    let mut s = ImportScreen::new(Arc::clone(&f.link), today(), Source::Qbd);
    s.poll();
    s
}

fn cmd(s: &mut ImportScreen, id: &str) {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    assert!(s.command(CommandId::new(id)), "{id}");
    s.poll();
}

fn jobs(f: &Fixture) -> u32 {
    f.link.demo_state().expect("demo").imports.jobs_received
}

fn backups(f: &Fixture) -> u32 {
    f.link.demo_state().expect("demo").imports.backups_taken
}

fn lost() -> ApiError {
    ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000003")
            .expect("id"),
        idempotency_key: Some(IdempotencyKey::new()),
        keyed: true,
    }
}

/// Export, files (the whole Shoal Creek set), matches, dates: on Import.
fn to_import(s: &mut ImportScreen) {
    assert_eq!(s.wizard.step, Step::Source);
    s.next();
    assert_eq!(s.wizard.step, Step::Files);
    cmd(s, "dev.imports_demo_file_all");
    for _ in 0..3 {
        s.poll();
    }
    assert!(s.can_next(), "{}", s.state());
    s.next();
    assert_eq!(s.wizard.step, Step::Mapping);
    assert!(s.can_next(), "{}", s.state());
    s.next();
    assert_eq!(s.wizard.step, Step::Dates);
    assert_eq!(
        s.date_text, "10/27/2026",
        "from the reference trial balance"
    );
    assert_eq!(
        s.wizard.depth,
        cl_model::imports::wizard::HistoryDepth::TwoYears,
        "Q-142"
    );
    assert!(s.can_next(), "{}", s.state());
    s.next();
    assert_eq!(s.wizard.step, Step::Import);
}

fn finish(s: &mut ImportScreen) {
    for _ in 0..6 {
        if s.wizard.imported {
            break;
        }
        s.poll();
    }
    s.poll();
}

#[test]
fn a_failed_backup_sends_nothing_and_try_again_backs_up_first() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    cmd(&mut s, "dev.imports_demo_backup_fail");
    s.next();
    s.poll();
    assert_eq!(s.trouble, Some(Trouble::BackupFailed));
    assert_eq!(jobs(&f), 0, "no commit without a backup");
    assert_eq!(s.jobs_sent, 0);
    assert!(!s.wizard.imported);
    // Try again: a backup, then the commit.
    s.start_import();
    s.poll();
    assert_eq!(backups(&f), 1);
    assert_eq!(jobs(&f), 1);
    finish(&mut s);
    assert!(s.wizard.imported);
}

#[test]
fn the_commit_is_sent_once_and_its_progress_shown_until_the_tie_out() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    s.next();
    s.poll();
    assert_eq!(jobs(&f), 1);
    assert_eq!(backups(&f), 1, "the backup came first");
    assert_eq!(s.running, Some(Running::Commit));
    let st = s.state();
    assert_eq!(st["running"], "commit");
    assert!(st["progress"]["total"].as_u64().is_some(), "{st}");
    assert!(!s.can_back() && s.locked(), "Back is off while it runs");
    // Pressing Import again does nothing.
    s.next();
    s.start_import();
    finish(&mut s);
    assert!(s.wizard.imported);
    assert_eq!(jobs(&f), 1, "sent once");
    assert_eq!(s.jobs_sent, 1);
    assert_eq!(s.wizard.step, Step::TieOut);
    s.poll();
    assert_eq!(s.tieout.as_ref().map(|t| t.all_zero), Some(true));
    assert!(!s.can_back(), "never back past the import");
    s.next();
    s.poll();
    assert!(s.wizard.verified);
    assert_eq!(s.wizard.step, Step::SpotCheck);
    let st = s.state().to_string();
    assert!(!st.contains(".csv") && !st.contains("clbackup"), "{st}");
}

/// Held back, then the answer lost after the engine took it.
fn lose_the_answer(f: &Fixture, s: &mut ImportScreen) {
    to_import(s);
    cmd(s, "dev.imports_demo_backup_hold");
    s.next();
    s.poll();
    assert!(s.in_flight(), "the backup runs");
    assert!(!s.can_back(), "Back is off while the backup runs");
    s.after_backup(Ok("held.clbackup".into()));
    // The engine got it; its answer was lost.
    s.job_out = Some(Job::Done(Some(Err(lost()))));
    s.poll();
    assert_eq!(jobs(f), 1);
}

#[test]
fn a_lost_answer_offers_check_status_and_locks_cancel_and_back() {
    let f = fixture();
    let mut s = screen(&f);
    lose_the_answer(&f, &mut s);
    assert!(s.unsettled);
    assert_eq!(s.trouble, Some(Trouble::Unknown));
    assert!(s.locked() && !s.can_back());
    assert!(s.dirty() && s.save_unconfirmed());
    // Nothing discards the files (B1).
    s.start_over();
    assert!(s.batch.is_some() && !s.uploads.is_empty());
    assert!(!s.can_next(), "Import is off: Check status first");
    assert_eq!(jobs(&f), 1, "never sent again on its own");
    s.check_status();
    s.poll();
    assert!(!s.unsettled, "the batch runs: it was taken");
    finish(&mut s);
    assert!(s.wizard.imported);
    assert_eq!(jobs(&f), 1);
}

#[test]
fn try_again_after_a_lost_answer_sends_the_same_key() {
    let f = fixture();
    let mut s = screen(&f);
    lose_the_answer(&f, &mut s);
    let key = s.held.as_ref().map(|h| h.key.clone()).expect("held");
    s.try_again();
    assert_eq!(s.held.as_ref().map(|h| h.key.clone()), Some(key));
    s.poll();
    assert_eq!(jobs(&f), 2, "sent again on the click");
    assert_eq!(backups(&f), 0, "the same body: no new backup");
    finish(&mut s);
    assert!(s.wizard.imported, "the demo replayed the first commit");
}

#[test]
fn a_closed_period_asks_for_the_password_and_retries_with_it() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    cmd(&mut s, "dev.imports_demo_close_books");
    s.next();
    s.poll();
    assert!(s.closing.dialog_open(), "{}", s.state());
    assert!(!s.wizard.imported && s.running.is_none());
    // A wrong one keeps the dialog.
    s.closing.password = crate::screens::company::form::SecretText::from("nope");
    s.submit_password();
    s.poll();
    assert!(s.closing.dialog_open());
    assert!(s.running.is_none());
    s.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::imports::PRACTICE_PASSWORD,
    );
    s.submit_password();
    s.poll();
    assert!(!s.closing.dialog_open(), "{}", s.state());
    assert_eq!(s.running, Some(Running::Commit));
    assert_eq!(backups(&f), 3, "a backup before every send");
    finish(&mut s);
    assert!(s.wizard.imported);
}

#[test]
fn a_reimport_is_refused_in_plain_words() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    s.next();
    finish(&mut s);
    assert!(s.wizard.imported);
    drop(s);
    let mut again = screen(&f);
    again.next();
    cmd(&mut again, "dev.imports_demo_file_all");
    again.poll();
    let refused: Vec<&SlotFile> = again
        .uploads
        .values()
        .flatten()
        .filter(|x| x.refusal.is_some())
        .collect();
    assert!(!refused.is_empty(), "{}", again.state());
    let text = draw::refusal_text(refused[0].refusal.as_ref().expect("refusal"));
    assert!(text.contains("imported before"), "{text}");
    assert!(!again.can_next());
}

#[test]
fn a_company_with_transactions_is_refused_for_qbd() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    cmd(&mut s, "dev.imports_demo_not_empty");
    s.next();
    s.poll();
    assert_eq!(
        s.trouble,
        Some(Trouble::Refused(ImportRefusal::CompanyNotEmpty))
    );
    assert!(!s.wizard.imported && s.running.is_none());
    assert!(draw::trouble_text(s.trouble.as_ref().expect("t")).contains("Nothing was posted"));
}

#[test]
fn undo_with_blockers_lists_them_and_offers_the_restore() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    s.next();
    finish(&mut s);
    let id = s.batch.expect("batch");
    cmd(&mut s, "dev.imports_demo_block_undo");
    s.ask_undo(id);
    assert!(s.confirm_undo.is_some(), "asks first");
    s.undo();
    s.poll();
    assert!(
        matches!(&s.trouble, Some(Trouble::Refused(ImportRefusal::UndoBlocked { blockers })) if blockers.len() == 1),
        "{:?}",
        s.trouble
    );
    assert_eq!(s.state()["blocked_by"], 1);
    assert!(s.blocked_backup().is_some(), "the restore is offered");
    assert!(draw::blocker_kind_text("payment").contains("payment"));
    assert!(!s.undone);
    f.link
        .demo_state()
        .expect("demo")
        .imports
        .clear_blockers(id)
        .expect("clear");
    s.ask_undo(id);
    s.undo();
    for _ in 0..6 {
        s.poll();
    }
    assert!(s.undone, "{}", s.state());
}

#[test]
fn the_close_prompt_asks_only_while_the_job_still_runs() {
    let f = fixture();
    let mut s = screen(&f);
    assert!(!s.dirty(), "nothing added");
    to_import(&mut s);
    assert!(s.dirty(), "files added, nothing imported");
    s.next();
    s.poll();
    // One read short of the end: still running.
    assert_eq!(s.running, Some(Running::Commit));
    assert!(s.dirty());
    let w = s.close_words().expect("words");
    assert_eq!(w.message, t("imports.close.running"));
    // The next reads find it done: no question.
    finish(&mut s);
    assert!(s.wizard.imported);
    assert!(!s.dirty(), "{}", s.state());
}

#[test]
fn an_unmatched_account_blocks_review_matches_until_picked() {
    let f = fixture();
    let mut s = screen(&f);
    cmd(&mut s, "dev.imports_demo_unmatched");
    s.next();
    cmd(&mut s, "dev.imports_demo_file_all");
    s.poll();
    s.next();
    assert_eq!(s.wizard.step, Step::Mapping);
    assert_eq!(s.unmapped(), 1);
    assert!(!s.can_next());
    let any = s.chart.first().map(|a| a.id).expect("chart");
    s.account_map.insert("Old suspense".into(), any);
    s.sync_mapping();
    assert!(s.can_next());
}

#[test]
fn only_the_admin_may_import() {
    let f = fixture();
    f.link
        .set_rights(cl_model::permissions::Rights::all().without_admin());
    let s = screen(&f);
    assert!(!s.allowed());
    assert_eq!(s.trouble, Some(Trouble::Restricted));
    assert!(!s.can_next() || !s.allowed());
}

/// Every text key the model names resolves (sources, slots, steps,
/// blockers, dates, depth, history statuses).
#[test]
fn every_model_key_is_in_the_catalog() {
    use cl_model::imports::wizard::{DateProblem, HistoryDepth};
    let mut missing = Vec::new();
    let mut check = |k: String| {
        if !crate::i18n::has(&k) {
            missing.push(k);
        }
    };
    for s in Source::ALL {
        check(format!("imports.source.{}", s.key()));
        for slot in s.slots() {
            check(format!("imports.slot.{}.name", slot.key));
            check(format!("imports.slot.{}.how", slot.key));
        }
        for st in s.steps() {
            if *st != Step::Source {
                check(format!("imports.step.{}", st.key()));
            }
        }
    }
    for p in [
        DateProblem::BadFiscalMonth,
        DateProblem::ConversionInFuture,
        DateProblem::OutOfRange,
        DateProblem::OpeningNotBefore,
    ] {
        check(format!("imports.dates.problem.{}", p.key()));
    }
    for d in [HistoryDepth::TwoYears, HistoryDepth::All] {
        check(format!("imports.depth.{}", d.key()));
    }
    for b in cl_api::imports::BatchStatus::ALL {
        check(format!("imports.status.{}", b.as_wire()));
    }
    for k in [
        "no_conversion_date",
        "missing_file",
        "not_uploaded",
        "row_problems",
        "wrong_date",
        "one_file_only",
        "mapping_incomplete",
        "not_imported",
        "not_verified",
        "company_not_empty",
        "no_opening_date",
    ] {
        check(format!("imports.blocker.{k}"));
    }
    for tr in [
        Trouble::Unknown,
        Trouble::PasswordWait,
        Trouble::Restricted,
        Trouble::Unsupported,
        Trouble::EngineDown,
        Trouble::BackupFailed,
        Trouble::Failed,
        Trouble::Gone,
        Trouble::Other,
    ] {
        check(format!("imports.trouble.{}", tr.code()));
    }
    assert!(missing.is_empty(), "{missing:#?}");
}
