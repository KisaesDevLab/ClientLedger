//! Import Transactions from a Spreadsheet against the demo engine (P31 D5):
//! the good file imports once after a backup, a failed backup sends
//! nothing, a file with problems sends nothing, a lost answer waits for
//! Check status and is never resent on its own, Undo, re-import and the
//! closing-date password.

use std::sync::Arc;

use super::*;
use crate::company::CompanyLink;

struct Fixture {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    {
        let mut d = link.demo_state().expect("demo");
        let open = d.open.expect("open");
        if let Some(c) = d.companies.iter_mut().find(|c| c.tenant_id == open) {
            c.closed_through = None;
        }
    }
    Fixture { _dir: dir, link }
}

fn screen(f: &Fixture) -> CsvTransactionsScreen {
    let open = f
        .link
        .demo_state()
        .and_then(|d| d.open)
        .map(|t| (t, "Ozark Ridge".to_string()));
    CsvTransactionsScreen::new(Arc::clone(&f.link), open)
}

fn cmd(s: &mut CsvTransactionsScreen, id: &str) {
    let id: &'static str = Box::leak(id.to_string().into_boxed_str());
    assert!(s.command(CommandId::new(id)), "{id}");
    s.poll();
}

/// Polls until the run settles (each demo poll moves one step).
fn settle(s: &mut CsvTransactionsScreen) {
    for _ in 0..20 {
        s.next_poll = s.next_poll.map(|_| Instant::now());
        s.poll();
    }
}

fn demo<R>(f: &Fixture, read: impl FnOnce(&crate::company::demo::DemoState) -> R) -> R {
    read(&f.link.demo_state().expect("demo"))
}

fn to_preview(s: &mut CsvTransactionsScreen, stem: &str) {
    cmd(s, &format!("dev.txn_import_demo_file_{stem}"));
    assert_eq!(s.step, Step::Map, "{}", s.state());
    s.next();
    assert_eq!(s.step, Step::Preview);
}

#[test]
fn csv_transactions_good_file_imports_once_after_a_backup() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    assert!(s.plan.as_ref().is_some_and(TxnPlan::can_import));
    s.next();
    s.poll();
    assert_eq!(s.phase, Phase::Running(Action::Commit), "{}", s.state());
    assert!(Screen::dirty(&s), "the close prompt shows while running");
    settle(&mut s);
    assert_eq!(s.step, Step::Done, "{}", s.state());
    assert!(!Screen::dirty(&s));
    let c = s.counts();
    assert_eq!((c.invoices, c.bills, c.journal_entries), (2, 2, 1));
    assert!(s.backup_ref.is_some());
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 1);
        assert_eq!(d.txn_import.commits_received, 1, "sent once");
        assert_eq!(d.txn_import.posted_documents, 5);
    });
}

#[test]
fn csv_transactions_backup_failure_sends_nothing() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    cmd(&mut s, "dev.txn_import_demo_backup_fails");
    s.next();
    s.poll();
    assert_eq!(s.trouble, Some(Trouble::BackupFailed(Action::Commit)));
    assert_eq!(s.phase, Phase::Idle);
    demo(&f, |d| {
        assert_eq!(d.txn_import.commits_received, 0, "nothing sent");
    });
    s.try_again();
    s.poll();
    settle(&mut s);
    assert_eq!(s.step, Step::Done);
    demo(&f, |d| {
        assert_eq!(d.txn_import.commits_received, 1);
    });
}

/// QA-20: the backup finishes while the close prompt is up. Close anyway
/// drops the window and nothing is sent; nothing posts.
#[test]
fn csv_transactions_close_during_the_backup_sends_nothing() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    s.next();
    // The prompt goes up while the backup is being made.
    Screen::close_asked(&mut s, true);
    settle(&mut s);
    assert!(s.parked, "{}", s.state());
    assert!(matches!(s.phase, Phase::BackingUp(Action::Commit)));
    assert!(Screen::dirty(&s), "still asks: the import is held");
    let words = Screen::close_words(&s, false).expect("words");
    assert!(
        words.message.contains("cancels the import"),
        "{}",
        words.message
    );
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 1);
        assert_eq!(d.txn_import.commits_received, 0, "held, not sent");
    });
    // Close anyway: the window goes.
    drop(s);
    demo(&f, |d| {
        assert_eq!(d.txn_import.commits_received, 0, "nothing sent after close");
        assert_eq!(d.txn_import.posted_documents, 0, "nothing posted");
    });
}

/// QA-20: Keep the window open after the backup finished under the prompt:
/// the import is sent then, once.
#[test]
fn csv_transactions_keep_after_the_backup_sends_once() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    s.next();
    Screen::close_asked(&mut s, true);
    settle(&mut s);
    assert!(s.parked);
    Screen::close_asked(&mut s, false);
    settle(&mut s);
    assert_eq!(s.step, Step::Done, "{}", s.state());
    demo(&f, |d| assert_eq!(d.txn_import.commits_received, 1));
}

/// QA-2/QA-18: with no backup passphrase, Import asks for one in the
/// window; a short one is refused with nothing sent.
#[test]
fn csv_transactions_ask_for_the_passphrase_before_anything_goes_up() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    s.need_passphrase = true;
    s.passphrase.fill("short", "short");
    s.start_import();
    s.poll();
    assert_eq!(s.phase, Phase::Idle);
    assert!(matches!(
        s.passphrase.error,
        Some(cl_company::CompanyError::PassphraseTooShort)
    ));
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 0, "nothing backed up");
        assert_eq!(d.txn_import.commits_received, 0);
    });
    assert!(s.need_passphrase);
    let st = s.state().to_string();
    assert!(!st.contains("short"), "never in state: {st}");
}

#[test]
fn csv_transactions_problem_rows_block_the_import() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "bad_rows");
    let plan = s.plan.clone().expect("plan");
    assert!(!plan.can_import());
    assert!(
        plan.problems.iter().any(|p| matches!(
            p.kind,
            cl_model::imports::csv_txn::ProblemKind::Unbalanced(_)
        )),
        "the unbalanced entry is named"
    );
    let words: Vec<String> = plan.problems.iter().map(draw::problem_text).collect();
    assert!(
        words.iter().all(|w| !w.starts_with("txn_import.")),
        "{words:?}"
    );
    s.start_import();
    s.poll();
    assert_eq!(s.phase, Phase::Idle);
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 0);
        assert_eq!(d.txn_import.commits_received, 0);
    });
}

#[test]
fn csv_transactions_lost_answer_waits_for_check_status() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    cmd(&mut s, "dev.txn_import_demo_lose_answer");
    s.next();
    s.poll();
    assert_eq!(s.phase, Phase::Unknown(Action::Commit));
    assert!(Screen::dirty(&s) && Screen::save_unconfirmed(&s));
    // Nothing happens on its own, and Back and Cancel are off.
    settle(&mut s);
    s.back();
    s.cancel();
    assert_eq!(s.phase, Phase::Unknown(Action::Commit));
    s.check_status();
    settle(&mut s);
    assert_eq!(s.step, Step::Done, "{}", s.state());
    demo(&f, |d| {
        assert_eq!(d.txn_import.commits_received, 1, "never resent");
    });
}

#[test]
fn csv_transactions_undo_then_reimport_and_refusal_before() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    s.next();
    settle(&mut s);
    assert_eq!(s.step, Step::Done);
    // The same file in a new window is refused while imported.
    let mut again = screen(&f);
    to_preview(&mut again, "good");
    again.next();
    again.poll();
    assert!(
        matches!(again.trouble, Some(Trouble::Already { .. })),
        "{}",
        again.state()
    );
    drop(again);
    s.ask_undo();
    assert!(s.confirm_undo);
    s.undo();
    s.poll();
    assert_eq!(s.phase, Phase::Running(Action::Undo));
    settle(&mut s);
    assert!(s.undone, "{}", s.state());
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 2, "undo backs up first");
        assert_eq!(d.txn_import.undos_received, 1);
        assert_eq!(d.txn_import.posted_documents, 0);
    });
    let mut third = screen(&f);
    to_preview(&mut third, "good");
    third.next();
    settle(&mut third);
    assert_eq!(
        third.step,
        Step::Done,
        "re-import after undo: {}",
        third.state()
    );
}

#[test]
fn csv_transactions_closed_books_ask_for_the_password() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    cmd(&mut s, "dev.txn_import_demo_close_books");
    s.next();
    s.poll();
    assert!(s.closing.dialog_open(), "{}", s.state());
    assert_eq!(s.phase, Phase::Idle);
    s.closing.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_password();
    s.poll();
    settle(&mut s);
    assert_eq!(s.step, Step::Done, "{}", s.state());
    demo(&f, |d| {
        assert_eq!(d.txn_import.backups_made, 2, "a backup before each send");
    });
}

#[test]
fn csv_transactions_state_has_no_names_or_amounts() {
    let f = fixture();
    let mut s = screen(&f);
    to_preview(&mut s, "good");
    let st = s.state().to_string();
    assert!(!st.contains("Riverbend") && !st.contains("1250"), "{st}");
}
