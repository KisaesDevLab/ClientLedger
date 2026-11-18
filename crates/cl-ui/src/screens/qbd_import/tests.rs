//! The QuickBooks Desktop import against the demo engine (P31 D3): the
//! walk to Import, the backup before every commit (a failed backup sends
//! nothing), one commit with progress, a lost answer (Check status, Try
//! again with the same key, Cancel/Back off, nothing discarded), the
//! closing-date password, refusals in owner words, undo with blockers and
//! the restore, and the close prompt only while a job runs.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;
use serde_json::json;

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

/// E1 as built: a blocker found after the undo's 202 lands as the batch's
/// error; it is shown as a blocked undo with the restore, not "stopped".
#[test]
fn an_undo_blocked_after_it_started_offers_the_restore() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    s.next();
    finish(&mut s);
    let id = s.batch.expect("batch");
    let mut st = s.state.clone().expect("state");
    assert_eq!(st.batch.status, cl_api::imports::BatchStatus::Committed);
    st.error = Some(cl_api::imports::BatchFailure {
        code: "IMPORT_UNDO_BLOCKED".into(),
        row_number: None,
        session: None,
    });
    s.undo_pending = Some(id);
    s.held = Some(Held {
        what: Running::Undo,
        batch: id,
        backup_ref: "b.clbackup".into(),
        password: None,
        confirm_closed: false,
        key: IdempotencyKey::new(),
    });
    s.running = Some(Running::Undo);
    s.after_batch_read(Ok(st));
    assert!(
        matches!(
            s.trouble,
            Some(Trouble::Refused(ImportRefusal::UndoBlocked { .. }))
        ),
        "{:?}",
        s.trouble
    );
    assert!(s.running.is_none() && !s.undone);
    assert!(s.blocked_backup().is_some());
}

// ---- D4: tie-out, spot-check, Retry, the closed period without password ----

/// Imports the files on the cards (Import and its polls) to the tie-out.
fn import_now(s: &mut ImportScreen) {
    s.next();
    finish(s);
    assert!(s.wizard.imported, "{}", s.state());
    assert_eq!(s.wizard.step, Step::TieOut);
    s.poll();
    assert!(s.tieout.is_some() && s.tie.is_some(), "{}", s.state());
}

fn demo_batch(f: &Fixture, s: &ImportScreen) -> cl_api::imports::ImportBatch {
    let id = s.batch.expect("batch");
    f.link
        .demo_state()
        .expect("demo")
        .imports
        .batches()
        .batches
        .into_iter()
        .find(|b| b.id == id)
        .expect("batch in the demo")
}

fn diff_of(s: &ImportScreen, label: &str) -> String {
    let st = s.tie.as_ref().expect("tie");
    st.view
        .rows
        .iter()
        .find(|r| r.input.label == label)
        .map(|r| r.diff_text.clone())
        .unwrap_or_else(|| panic!("no row {label}"))
}

#[test]
fn the_tie_out_is_verified_only_when_every_difference_is_zero() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    import_now(&mut s);
    let st = s.state();
    assert_eq!(st["tieout_all_zero"], true);
    assert_eq!(
        st["tieout_tabs"],
        json!(["accounts", "receivables", "payables", "inventory"])
    );
    let tie = s.tie.as_ref().expect("tie");
    let tile = tie.tile(tieout::Tab::TrialBalance, s.tieout.as_ref().expect("t"));
    assert_eq!(tile.figure, "0.00");
    assert!(tile.tick && tile.matches);
    assert_eq!(
        tie.rows(tieout::Tab::Receivables).len(),
        4,
        "the open invoices"
    );
    assert_eq!(tie.checks(tieout::Tab::Inventory).len(), 2);
    assert!(s.can_next(), "Mark verified is on at all-zero");
    s.next();
    s.poll();
    assert!(s.wizard.verified);
    assert!(demo_batch(&f, &s).is_verified());
    assert_eq!(s.wizard.step, Step::SpotCheck);
    // The spot-check list is made from this tie-out and saved at once.
    assert!(!s.spot.items.is_empty());
    assert!(s.spot.items.iter().any(|i| i.key.starts_with("ar:")));
    assert!(s.spot.items.iter().any(|i| i.key == "inventory"));
    assert_eq!(s.spot.saves, 1);
    assert_eq!(demo_batch(&f, &s).spot_check.len(), s.spot.items.len());
}

#[test]
fn an_altered_open_invoice_shows_on_receivables_and_its_history_account() {
    let f = fixture();
    let mut s = screen(&f);
    s.next();
    cmd(&mut s, "dev.imports_demo_file_all");
    cmd(
        &mut s,
        "dev.imports_demo_file_qbd_open_invoices__qbd__shoal-creek-altered__open-item__open-invoices.csv",
    );
    s.poll();
    s.next();
    s.next();
    s.next();
    assert_eq!(s.wizard.step, Step::Import, "{}", s.state());
    import_now(&mut s);
    assert_eq!(s.state()["tieout_all_zero"], false);
    // Acceptance 2: the difference is on the right account, to the cent.
    assert_eq!(diff_of(&s, "1200"), "10.00");
    assert_eq!(diff_of(&s, "history_ar"), "-10.00");
    assert_eq!(diff_of(&s, "1000"), "0.00");
    let tie = s.tie.as_ref().expect("tie");
    let t = s.tieout.as_ref().expect("t");
    let ar = tie.tile(tieout::Tab::Receivables, t);
    assert_eq!(ar.figure, "-10.00");
    assert!(!ar.matches && !ar.tick);
    assert!(
        tie.rows(tieout::Tab::Receivables).iter().all(|r| r.zero),
        "the item rows tie: the file is on both sides"
    );
    // Show only differences: the two rows.
    cmd(&mut s, "dev.imports_demo_differences_only");
    assert_eq!(s.state()["tieout_rows_shown"], 2, "1200 and history_ar");
    cmd(&mut s, "dev.imports_demo_tab_receivables");
    assert_eq!(s.state()["tieout_rows_shown"], 1, "history_ar only");
    // Mark verified is off; the engine refuses it too, and the rows reload.
    assert!(!s.can_next());
    s.verify();
    s.poll();
    assert!(!s.wizard.verified);
    assert!(
        matches!(
            s.trouble,
            Some(Trouble::Refused(ImportRefusal::TieOutNotZero { .. }))
        ),
        "{:?}",
        s.trouble
    );
    s.poll();
    assert_eq!(s.state()["tieout_tab"], "receivables", "the tab is kept");
    assert!(!demo_batch(&f, &s).is_verified());
}

#[test]
fn a_repointed_line_differs_on_its_two_accounts_only() {
    let f = fixture();
    let mut s = screen(&f);
    s.next();
    cmd(&mut s, "dev.imports_demo_file_all");
    // The journal card takes several files: swap the 2025-2026 one.
    let i = s.uploads["qbd_journal"]
        .iter()
        .position(|x| x.name == "journal-2025-2026.csv")
        .expect("journal");
    s.remove_file("qbd_journal", i);
    cmd(
        &mut s,
        "dev.imports_demo_file_qbd_journal__qbd__shoal-creek-altered__repointed__journal-2025-2026.csv",
    );
    s.poll();
    s.next();
    s.next();
    s.next();
    import_now(&mut s);
    assert_eq!(diff_of(&s, "6100"), "-389.44");
    assert_eq!(diff_of(&s, "6200"), "389.44");
    let tie = s.tie.as_ref().expect("tie");
    assert_eq!(tie.view.differences().len(), 2, "those two only");
    assert!(!s.can_next());
}

#[test]
fn the_spot_check_is_saved_after_a_pause_and_on_close() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    import_now(&mut s);
    s.next();
    s.poll();
    assert_eq!(s.spot.saves, 1);
    // A change waits for the pause, then saves.
    s.spot.items[0].done = true;
    s.spot.changed_at = Some(std::time::Instant::now());
    s.poll();
    assert_eq!(s.spot.saves, 1, "not yet: debounced");
    assert!(s.state()["spot_unsaved"].as_bool().expect("flag"));
    s.spot.changed_at = std::time::Instant::now().checked_sub(spot_check::DEBOUNCE);
    s.poll();
    s.poll();
    assert_eq!(s.spot.saves, 2);
    assert!(demo_batch(&f, &s).spot_check[0].done);
    // A note typed just before closing is saved on close.
    s.spot.items[1].note = "Matches the bank statement".into();
    s.spot.changed_at = Some(std::time::Instant::now());
    let b = s.batch.expect("batch");
    drop(s);
    let saved = f
        .link
        .demo_state()
        .expect("demo")
        .imports
        .batches()
        .batches
        .into_iter()
        .find(|x| x.id == b)
        .expect("batch")
        .spot_check;
    assert_eq!(saved[1].note, "Matches the bank statement");
    // Reopened from History: the saved list, on the spot-check step.
    let mut again = screen(&f);
    again.show_history();
    again.poll();
    let batch = again
        .history
        .as_ref()
        .and_then(|h| h.iter().find(|x| x.id == b).cloned())
        .expect("listed");
    again.open_batch(&batch);
    again.poll();
    assert_eq!(again.wizard.step, Step::SpotCheck);
    assert!(again.wizard.verified && again.wizard.imported);
    assert_eq!(again.spot.items.len(), saved.len());
    assert!(again.spot.items[0].done);
    assert!(
        !again.spot.unsaved(),
        "nothing to save: it is the saved list"
    );
}

#[test]
fn retry_from_history_backs_up_and_uses_a_new_key() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    cmd(&mut s, "dev.imports_demo_fail_commit");
    s.next();
    for _ in 0..6 {
        s.poll();
    }
    assert_eq!(s.trouble, Some(Trouble::Failed), "{}", s.state());
    assert_eq!(s.failure, Some(ImportRefusal::Interrupted));
    let text = draw::refusal_text(s.failure.as_ref().expect("failure"));
    assert!(text.contains("nothing was posted"), "{text}");
    assert!(!s.wizard.imported);
    let first = demo_key(&f, &s);
    assert_eq!((jobs(&f), backups(&f)), (1, 1));
    s.show_history();
    s.poll();
    let b = s
        .history
        .as_ref()
        .and_then(|h| h.first().cloned())
        .expect("the failed batch");
    assert_eq!(b.status, BatchStatus::Failed);
    s.retry_batch(&b);
    s.poll();
    assert_eq!(s.view, View::Import);
    assert_eq!((jobs(&f), backups(&f)), (2, 2), "a backup, then the commit");
    let second = demo_key(&f, &s);
    assert_ne!(first, second, "a new attempt, a new key");
    finish(&mut s);
    assert!(s.wizard.imported, "{}", s.state());
    assert_eq!(s.wizard.step, Step::TieOut);
}

/// The key of the batch's last commit (the demo keeps it).
fn demo_key(_f: &Fixture, s: &ImportScreen) -> Option<String> {
    s.last_key.as_ref().map(IdempotencyKey::as_header)
}

#[test]
fn a_closed_period_without_a_password_asks_and_sends_the_confirmation() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    cmd(&mut s, "dev.imports_demo_close_books");
    cmd(&mut s, "dev.imports_demo_no_closing_password");
    s.next();
    s.poll();
    assert!(s.ask_closed.is_some(), "{}", s.state());
    assert!(!s.closing.dialog_open(), "no password to ask for");
    assert!(!s.can_next() && s.running.is_none());
    assert_eq!(
        f.link.demo_state().expect("demo").imports.confirmed_closed,
        0
    );
    // Esc cancels: nothing is sent.
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.ask_closed.is_none());
    s.next();
    s.poll();
    assert!(s.ask_closed.is_some());
    cmd(&mut s, "dev.imports_demo_confirm_closed");
    assert_eq!(backups(&f), 3, "a backup before every send");
    assert_eq!(
        f.link.demo_state().expect("demo").imports.confirmed_closed,
        1
    );
    finish(&mut s);
    assert!(s.wizard.imported);
}

#[test]
fn every_new_message_resolves_with_a_fix() {
    for r in [
        ImportRefusal::Interrupted,
        ImportRefusal::BatchUndone,
        ImportRefusal::BatchNotCommitted,
        ImportRefusal::BatchEmpty,
        ImportRefusal::SourceMismatch,
        ImportRefusal::Busy,
    ] {
        let key = format!("imports.refusal.{}", refusal_code(&r));
        assert!(crate::i18n::has(&key), "{key}");
        let text = draw::refusal_text(&r);
        // Q-119: what happened, then what to do (two sentences at least).
        assert!(text.matches(". ").count() >= 1, "{key}: {text}");
    }
    // CLOSING_PASSWORD_RATE_LIMIT (429) is the password wait.
    let e = ApiError::ClosingPasswordRateLimited { retry_after: None };
    assert_eq!(Trouble::of(&e), Trouble::PasswordWait);
    let text = draw::trouble_text(&Trouble::PasswordWait);
    assert!(crate::i18n::has("imports.trouble.password_wait"));
    assert!(text.contains("try again"), "{text}");
    for k in [
        "imports.closed.title",
        "imports.closed.body",
        "imports.closed.confirm",
        "imports.history.open",
        "orphan.import_spot_check",
    ] {
        assert!(crate::i18n::has(k), "{k}");
    }
}

#[test]
fn the_tie_out_pdf_has_every_comparison() {
    let f = fixture();
    let mut s = screen(&f);
    to_import(&mut s);
    import_now(&mut s);
    let bytes = s.tieout_pdf().expect("pdf");
    assert!(bytes.starts_with(b"%PDF"));
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("check.pdf");
    s.write_pdf(&path, &bytes);
    assert_eq!(s.pdfs_saved, 1);
    assert!(std::fs::read(&path).expect("written").starts_with(b"%PDF"));
    // The report, as text, against the golden file (plan §5).
    let st = s.tie.as_ref().expect("tie");
    let doc = tieout::report(
        st,
        s.tieout.as_ref().expect("t"),
        "QuickBooks Desktop",
        false,
    );
    let text = report_text(&doc);
    let golden = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/qbd/shoal-creek/tieout.golden.txt"
    );
    if std::env::var_os("CL_UPDATE_GOLDEN").is_some() {
        std::fs::write(golden, &text).expect("golden");
    }
    let want = std::fs::read_to_string(golden)
        .expect("golden file")
        .replace("\r\n", "\n");
    assert_eq!(text, want);
}

/// A report as plain text: title, subtitle, then each section's rows.
fn report_text(r: &cl_print::reports::Report) -> String {
    use cl_print::reports::Section as S;
    let mut out = format!("{}\n", r.title);
    for l in &r.subtitle {
        out.push_str(&format!("{l}\n"));
    }
    for sec in &r.sections {
        match sec {
            S::Table {
                heading,
                columns,
                rows,
                ..
            } => {
                out.push_str(&format!("\n## {heading}\n"));
                let head: Vec<&str> = columns.iter().map(|c| c.heading.as_str()).collect();
                out.push_str(&format!("{}\n", head.join(" | ")));
                for row in rows {
                    out.push_str(&format!("{}\n", row.join(" | ")));
                }
            }
            S::Summary { heading, rows } => {
                out.push_str(&format!("\n## {heading}\n"));
                for (label, amount, _) in rows {
                    out.push_str(&format!("{label} | {amount}\n"));
                }
            }
        }
    }
    out
}
