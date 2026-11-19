//! Demo import flows (P31 D5a): the demo answers as the engine does.

// Short names (`d`, `a`, `b`, `e`, `t`) keep the flows readable.
#![allow(clippy::many_single_char_names)]

use super::*;
use cl_api::imports::{BatchKind, BatchOptions, ContactKind, ImportRefusal, import_refusal};

fn upload(
    d: &mut DemoImports,
    name: &str,
    kind: ImportKind,
    source: SourceSystem,
    role: FileRole,
) -> Result<UploadOutcome, ApiError> {
    d.upload(&UploadRequest {
        kind,
        source,
        role,
        bytes: fixture_bytes(name).expect("fixture").to_vec(),
        file_name: "x.csv".into(),
        contact_kind: (kind == ImportKind::Contacts).then_some(ContactKind::Customer),
        tb_column: None,
        tb_report_date: None,
        update_existing_coa: false,
    })
}

const QBD: SourceSystem = SourceSystem::QuickBooksDesktop;

/// The Shoal Creek files a conversion posts, with the altered file
/// replacing its base file when given.
fn qbd_files(d: &mut DemoImports, journal: &str, invoices: &str) -> Vec<ImportSessionId> {
    let base = "qbd/shoal-creek/";
    let mut ids = Vec::new();
    for (name, kind, role) in [
        ("account-list.csv", ImportKind::Coa, FileRole::Post),
        (
            "tb-opening-2023-12-31.csv",
            ImportKind::TrialBalance,
            FileRole::Post,
        ),
        (
            "tb-2026-10-27.csv",
            ImportKind::TrialBalance,
            FileRole::Reference,
        ),
        (
            "journal-2024.csv",
            ImportKind::GlTransactions,
            FileRole::Post,
        ),
    ] {
        ids.push(
            upload(d, &format!("{base}{name}"), kind, QBD, role)
                .expect("upload")
                .session
                .id,
        );
    }
    ids.push(
        upload(d, journal, ImportKind::GlTransactions, QBD, FileRole::Post)
            .expect("journal")
            .session
            .id,
    );
    ids.push(
        upload(d, invoices, ImportKind::OpenInvoices, QBD, FileRole::Post)
            .expect("invoices")
            .session
            .id,
    );
    ids
}

fn base_files(d: &mut DemoImports) -> Vec<ImportSessionId> {
    qbd_files(
        d,
        "qbd/shoal-creek/journal-2025-2026.csv",
        "qbd/shoal-creek/open-invoices.csv",
    )
}

fn batch_of(d: &mut DemoImports, sessions: Vec<ImportSessionId>) -> Result<ImportBatch, ApiError> {
    d.create_batch(&NewBatch {
        source: QBD,
        kind: BatchKind::Conversion,
        sessions,
        options: BatchOptions {
            require_empty_company: true,
            ..BatchOptions::default()
        },
    })
}

fn commit_req() -> CommitRequest {
    CommitRequest {
        backup_ref: "before-import.clbackup".into(),
        ..CommitRequest::default()
    }
}

fn commit(d: &mut DemoImports, id: ImportBatchId) -> Result<JobAccepted, ApiError> {
    d.commit(id, &commit_req(), &IdempotencyKey::new(), None, None)
}

/// Polls until the job ends; returns the polls it took.
fn finish(d: &mut DemoImports, id: ImportBatchId) -> u8 {
    for polls in 1..=10 {
        let s = d.batch(id).expect("batch");
        if !s.batch.status.is_running() {
            return polls;
        }
        assert!(s.progress.is_some(), "a running job reports progress");
    }
    panic!("the job never ended");
}

fn refusal(e: &ApiError) -> ImportRefusal {
    import_refusal(e).unwrap_or_else(|| panic!("not an import refusal: {e:?}"))
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

#[test]
fn upload_reports_preview_and_row_problems() {
    let mut d = DemoImports::default();
    let tb = upload(
        &mut d,
        "qbd/shoal-creek/tb-2026-10-27.csv",
        ImportKind::TrialBalance,
        QBD,
        FileRole::Reference,
    )
    .expect("upload");
    assert!(tb.is_clean());
    assert_eq!(tb.preview.report_date, day("2026-10-27"));
    assert_eq!(tb.preview.total_debit, Some(money("53785.46")));
    assert_eq!(tb.preview.total_credit, Some(money("53785.46")));
    assert_eq!(tb.session.row_count, 15);
    let opening = upload(
        &mut d,
        "qbd/shoal-creek/tb-opening-2023-12-31.csv",
        ImportKind::TrialBalance,
        QBD,
        FileRole::Post,
    )
    .expect("upload");
    assert_eq!(opening.preview.report_date, day("2023-12-31"));
    assert_eq!(opening.preview.total_debit, Some(money("73500.00")));

    let bad = upload(
        &mut d,
        "imports/transactions/bad-rows.csv",
        ImportKind::Transactions,
        SourceSystem::Generic,
        FileRole::Post,
    )
    .expect("upload");
    assert!(!bad.is_clean());
    let codes: Vec<&str> = bad.problems.iter().map(|p| p.code.as_str()).collect();
    assert_eq!(
        codes,
        [
            "IMPORT_UNBALANCED",
            "IMPORT_UNKNOWN_REF",
            "IMPORT_MIXED_GROUP",
            "IMPORT_AMOUNT_INVALID"
        ]
    );
    assert_eq!(bad.session.error_count, 4);
    let good = upload(
        &mut d,
        "imports/transactions/good.csv",
        ImportKind::Transactions,
        SourceSystem::Generic,
        FileRole::Post,
    )
    .expect("upload");
    assert!(good.is_clean());
}

#[test]
fn an_unknown_file_is_unreadable() {
    let mut d = DemoImports::default();
    let e = d
        .upload(&UploadRequest {
            kind: ImportKind::Coa,
            source: QBD,
            role: FileRole::Post,
            bytes: b"not,a,fixture\n1,2,3\n".to_vec(),
            file_name: "x.csv".into(),
            contact_kind: None,
            tb_column: None,
            tb_report_date: None,
            update_existing_coa: false,
        })
        .expect_err("unreadable");
    assert!(matches!(e, ApiError::Validation(_)));
}

#[test]
fn happy_path_commits_over_polls_then_ties_out_to_zero() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    assert_eq!(b.status, BatchStatus::Draft);
    assert_eq!(b.conversion_date, day("2026-10-27"));
    let key = IdempotencyKey::new();
    let ok = d
        .commit(b.id, &commit_req(), &key, None, None)
        .expect("accepted");
    assert!(!ok.replayed);
    // The same key replays the 202 and starts nothing new.
    assert!(
        d.commit(b.id, &commit_req(), &key, None, None)
            .expect("replay")
            .replayed
    );
    let s = d.batch(b.id).expect("poll");
    assert_eq!(s.batch.status, BatchStatus::Committing);
    assert_eq!(s.progress.as_ref().map(|p| p.done), Some(1));
    assert_eq!(finish(&mut d, b.id), POLLS_TO_FINISH - 1);
    let done = d.batch(b.id).expect("batch").batch;
    assert_eq!(done.status, BatchStatus::Committed);
    assert!(done.committed_at.is_some());
    assert_eq!(done.backup_ref.as_deref(), Some("before-import.clbackup"));

    let t = d.tieout(b.id).expect("tieout");
    assert!(t.all_zero);
    assert_eq!(t.as_of, day("2026-10-27").expect("date"));
    assert_eq!(t.accounts.len(), 15);
    assert!(t.accounts.iter().all(|a| a.diff.is_zero()));
    assert!(t.balance_rows.iter().all(|r| r.diff.is_zero()));
    assert_eq!(t.balance_rows.len(), 4);
    let cash = t
        .accounts
        .iter()
        .find(|a| a.label() == "1000")
        .expect("1000");
    assert_eq!(cash.books, money("26684.35"));

    let v = d.verify(b.id).expect("verify");
    assert!(v.batch.is_verified());
    assert!(v.tieout.verified_at.is_some());
    // Idempotent: again answers with what it kept.
    let again = d.verify(b.id).expect("verify again");
    assert_eq!(again.batch.verified_at, v.batch.verified_at);
    assert_eq!(
        t.totals.ar.map(|x| x.source),
        Some(money("4311.67")),
        "the A/R aging total from EXPECTED.toml"
    );
    let txns = d.transactions(b.id);
    assert!(!txns.is_empty() && txns.iter().any(|x| x.txn_type == "invoice"));
    let items = vec![SpotCheckItem {
        key: "bank:1000".into(),
        label: "Checking".into(),
        done: true,
        note: String::new(),
    }];
    d.save_spot_check(b.id, &items).expect("spot");
    assert_eq!(d.batches().batches[0].spot_check, items);
}

#[test]
fn the_altered_set_differs_on_the_right_accounts_and_cannot_verify() {
    let mut d = DemoImports::default();
    let files = qbd_files(
        &mut d,
        "qbd/shoal-creek-altered/repointed/journal-2025-2026.csv",
        "qbd/shoal-creek/open-invoices.csv",
    );
    let b = batch_of(&mut d, files).expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);
    let t = d.tieout(b.id).expect("tieout");
    assert!(!t.all_zero);
    let diff = |n: &str| t.accounts.iter().find(|a| a.label() == n).expect(n).diff;
    assert_eq!(diff("6100"), money("-389.44"));
    assert_eq!(diff("6200"), money("389.44"));
    let off: Vec<String> = t
        .accounts
        .iter()
        .filter(|a| !a.diff.is_zero())
        .map(cl_api::imports::TieOutAccount::label)
        .collect();
    assert_eq!(off, ["6100", "6200"]);
    assert_eq!(t.difference_count, 2);
    assert_eq!(t.totals.ar.map(|x| x.diff), Some(money("0.00")));
    let e = d.verify(b.id).expect_err("refused");
    let ImportRefusal::TieOutNotZero {
        difference_count,
        first,
    } = refusal(&e)
    else {
        panic!("not zero")
    };
    assert_eq!(difference_count, 2);
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|x| x.section == Some(cl_api::imports::DifferenceSection::Accounts)),
        "ids and rows only"
    );
}

#[test]
fn the_open_item_set_leaves_receivables_before_conversion_off() {
    let mut d = DemoImports::default();
    let files = qbd_files(
        &mut d,
        "qbd/shoal-creek/journal-2025-2026.csv",
        "qbd/shoal-creek-altered/open-item/open-invoices.csv",
    );
    let b = batch_of(&mut d, files).expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);
    let t = d.tieout(b.id).expect("tieout");
    let ar = t
        .balance_rows
        .iter()
        .find(|r| r.key == BalanceKey::HistoryAr)
        .expect("row");
    assert_eq!(ar.diff, money("-10.00"));
    assert_eq!(
        t.accounts
            .iter()
            .find(|a| a.label() == "1200")
            .expect("1200")
            .diff,
        money("10.00")
    );
}

#[test]
fn a_reimport_is_refused_until_the_batch_is_undone() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);

    // The same file again: refused at upload.
    let e = upload(
        &mut d,
        "qbd/shoal-creek/journal-2024.csv",
        ImportKind::GlTransactions,
        QBD,
        FileRole::Post,
    )
    .expect_err("again");
    match refusal(&e) {
        ImportRefusal::AlreadyCommitted {
            batch,
            committed_at,
            ..
        } => {
            assert_eq!(batch, Some(b.id));
            assert!(committed_at.is_some());
        }
        other => panic!("{other:?}"),
    }
    // A committed batch cannot be committed again.
    let e = commit(&mut d, b.id).expect_err("committed");
    assert!(matches!(
        refusal(&e),
        ImportRefusal::AlreadyCommitted { .. }
    ));
    // Days that overlap the committed journals are refused, however the
    // file is made.
    let overlap = upload(
        &mut d,
        "qbd/shoal-creek-altered/journal-overlap.csv",
        ImportKind::GlTransactions,
        QBD,
        FileRole::Post,
    )
    .expect("upload");
    let e = batch_of(&mut d, vec![overlap.session.id]).expect_err("overlap");
    match refusal(&e) {
        ImportRefusal::PeriodOverlap { batch, from, to } => {
            assert_eq!(batch, Some(b.id));
            assert_eq!(from, day("2025-12-01"));
            assert_eq!(to, day("2026-01-31"));
        }
        other => panic!("{other:?}"),
    }

    // Undone: the files can come in again.
    d.undo(b.id, "b.clbackup", &IdempotencyKey::new(), None, None)
        .expect("undo");
    finish(&mut d, b.id);
    assert!(
        upload(
            &mut d,
            "qbd/shoal-creek/journal-2024.csv",
            ImportKind::GlTransactions,
            QBD,
            FileRole::Post,
        )
        .is_ok()
    );
    assert!(batch_of(&mut d, vec![overlap.session.id]).is_ok());
}

#[test]
fn two_files_of_one_batch_may_not_overlap() {
    let mut d = DemoImports::default();
    let a = upload(
        &mut d,
        "qbd/shoal-creek/journal-2025-2026.csv",
        ImportKind::GlTransactions,
        QBD,
        FileRole::Post,
    )
    .expect("a");
    let b = upload(
        &mut d,
        "qbd/shoal-creek-altered/journal-overlap.csv",
        ImportKind::GlTransactions,
        QBD,
        FileRole::Post,
    )
    .expect("b");
    let e = batch_of(&mut d, vec![a.session.id, b.session.id]).expect_err("overlap");
    assert!(matches!(
        refusal(&e),
        ImportRefusal::PeriodOverlap { batch: None, .. }
    ));
}

#[test]
fn undo_runs_over_polls_and_leaves_nothing_behind() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files.clone()).expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);
    assert!(!d.company_empty);
    let key = IdempotencyKey::new();
    d.undo(b.id, "b.clbackup", &key, None, None).expect("undo");
    assert!(
        d.undo(b.id, "b.clbackup", &key, None, None)
            .expect("replay")
            .replayed
    );
    let s = d.batch(b.id).expect("poll");
    assert_eq!(s.batch.status, BatchStatus::Undoing);
    assert!(s.progress.is_some());
    finish(&mut d, b.id);
    let s = d.batch(b.id).expect("batch").batch;
    assert_eq!(s.status, BatchStatus::Undone);
    assert!(
        d.company_empty,
        "a conversion undone leaves the company empty"
    );
    // The tie-out, verify and spot-check need a committed batch: an undone
    // one says so with its own code (E3).
    for e in [
        d.tieout(b.id).expect_err("undone"),
        d.verify(b.id).expect_err("undone"),
        d.save_spot_check(b.id, &[]).expect_err("undone"),
    ] {
        assert_eq!(refusal(&e), ImportRefusal::BatchUndone);
    }
    assert_eq!(d.transactions(b.id), Vec::new());
    // The files are free again: a new batch takes them and commits.
    let again = batch_of(&mut d, files).expect("again");
    assert_eq!(
        refusal(&d.save_spot_check(again.id, &[]).expect_err("draft")),
        ImportRefusal::BatchNotCommitted
    );
    commit(&mut d, again.id).expect("commit again");
    finish(&mut d, again.id);
    assert_eq!(d.batches().batches.len(), 2);
    assert_eq!(d.batches().batches[0].id, again.id, "newest first");
}

#[test]
fn undo_blockers_are_listed_with_reasons() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);
    d.add_blocker(
        b.id,
        UndoBlocker {
            kind: "payment".into(),
            id: Some("pay-1".into()),
            reason: Some("payment_applied".into()),
            target: None,
        },
    )
    .expect("blocker");
    let e = d
        .undo(b.id, "b.clbackup", &IdempotencyKey::new(), None, None)
        .expect_err("blocked");
    match refusal(&e) {
        ImportRefusal::UndoBlocked { blockers } => {
            assert_eq!(blockers.len(), 1);
            assert_eq!(blockers[0].kind, "payment");
            assert_eq!(blockers[0].reason.as_deref(), Some("payment_applied"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        d.batch(b.id).expect("batch").batch.status,
        BatchStatus::Committed
    );
    d.clear_blockers(b.id).expect("clear");
    assert!(
        d.undo(b.id, "b.clbackup", &IdempotencyKey::new(), None, None)
            .is_ok()
    );
}

#[test]
fn only_one_job_runs_and_a_file_is_in_one_batch() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files.clone()).expect("batch");
    // A second batch of a file already in a batch is refused.
    let e = batch_of(&mut d, vec![files[0]]).expect_err("in batch");
    assert_eq!(refusal(&e), ImportRefusal::SessionInBatch);
    assert!(d.discard(files[0]).is_err());

    let other = upload(
        &mut d,
        "imports/generic/coa.csv",
        ImportKind::Coa,
        SourceSystem::Generic,
        FileRole::Post,
    )
    .expect("coa");
    let second = d
        .create_batch(&NewBatch {
            source: SourceSystem::Generic,
            kind: BatchKind::Single,
            sessions: vec![other.session.id],
            options: BatchOptions::default(),
        })
        .expect("second");
    commit(&mut d, b.id).expect("commit");
    let e = commit(&mut d, second.id).expect_err("busy");
    assert_eq!(refusal(&e), ImportRefusal::Busy);
    finish(&mut d, b.id);
    commit(&mut d, second.id).expect("now free");
    // A non-empty company refuses a conversion and a fiscal-year change.
    let e = d
        .create_batch(&NewBatch {
            source: SourceSystem::Generic,
            kind: BatchKind::Single,
            sessions: vec![other.session.id],
            options: BatchOptions {
                fiscal_year_start_month: Some(7),
                ..BatchOptions::default()
            },
        })
        .expect_err("locked");
    assert_eq!(refusal(&e), ImportRefusal::FiscalYearLocked);
}

#[test]
fn a_conversion_needs_an_empty_company() {
    let mut d = DemoImports {
        company_empty: false,
        ..DemoImports::default()
    };
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    let e = commit(&mut d, b.id).expect_err("not empty");
    assert_eq!(refusal(&e), ImportRefusal::CompanyNotEmpty);
}

#[test]
fn a_backup_reference_is_required() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    let e = d
        .commit(
            b.id,
            &CommitRequest::default(),
            &IdempotencyKey::new(),
            None,
            None,
        )
        .expect_err("backup");
    assert!(matches!(
        e,
        ApiError::Local(cl_api::LocalValidation::BackupRequired)
    ));
}

#[test]
fn a_closed_period_needs_the_closing_password() {
    let mut d = DemoImports::default();
    let files = base_files(&mut d);
    let b = batch_of(&mut d, files).expect("batch");
    let closing = day("2025-12-31");
    let key = IdempotencyKey::new();
    let e = d
        .commit(b.id, &commit_req(), &key, None, closing)
        .expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let wrong = Secret::new("nope");
    let e = d
        .commit(b.id, &commit_req(), &key, Some(&wrong), closing)
        .expect_err("wrong");
    assert!(matches!(e, ApiError::ClosingPasswordRejected));
    // Nothing started: the batch is still a draft.
    assert_eq!(
        d.batch(b.id).expect("batch").batch.status,
        BatchStatus::Draft
    );
    let ok = Secret::new(PRACTICE_PASSWORD);
    d.commit(b.id, &commit_req(), &key, Some(&ok), closing)
        .expect("accepted");
    finish(&mut d, b.id);
    assert_eq!(
        d.batch(b.id).expect("batch").batch.status,
        BatchStatus::Committed
    );
    // Undo needs it too.
    let e = d
        .undo(b.id, "b.clbackup", &IdempotencyKey::new(), None, closing)
        .expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)));
    d.undo(
        b.id,
        "b.clbackup",
        &IdempotencyKey::new(),
        Some(&Secret::new(DEMO_CLOSING_PASSWORD)),
        closing,
    )
    .expect("undo");
}

#[test]
fn history_cutoff_moves_the_earliest_posting_day_past_the_closing_date() {
    let closing = day("2025-12-31");
    for (history_from, posts) in [(day("2026-01-01"), true), (None, false)] {
        let mut d = DemoImports::default();
        let j = upload(
            &mut d,
            "qbd/shoal-creek/journal-2025-2026.csv",
            ImportKind::GlTransactions,
            QBD,
            FileRole::Post,
        )
        .expect("journal");
        let b = d
            .create_batch(&NewBatch {
                source: QBD,
                kind: BatchKind::Single,
                sessions: vec![j.session.id],
                options: BatchOptions {
                    history_from,
                    ..BatchOptions::default()
                },
            })
            .expect("batch");
        let r = d.commit(b.id, &commit_req(), &IdempotencyKey::new(), None, closing);
        assert_eq!(r.is_ok(), posts, "{r:?}");
    }
}

#[test]
fn other_systems_tie_out_from_their_own_sets() {
    let mut d = DemoImports::default();
    let mut ids = Vec::new();
    for (name, kind, role) in [
        (
            "imports/generic/tb-opening.csv",
            ImportKind::TrialBalance,
            FileRole::Post,
        ),
        (
            "imports/generic/tb-reference.csv",
            ImportKind::TrialBalance,
            FileRole::Reference,
        ),
        (
            "imports/generic-altered/gl.csv",
            ImportKind::GlTransactions,
            FileRole::Post,
        ),
    ] {
        ids.push(
            upload(&mut d, name, kind, SourceSystem::Generic, role)
                .expect("upload")
                .session
                .id,
        );
    }
    let b = d
        .create_batch(&NewBatch {
            source: SourceSystem::Generic,
            kind: BatchKind::Conversion,
            sessions: ids,
            options: BatchOptions::default(),
        })
        .expect("batch");
    commit(&mut d, b.id).expect("commit");
    finish(&mut d, b.id);
    let t = d.tieout(b.id).expect("tieout");
    assert!(t.balance_rows.is_empty(), "no QBD history rows");
    let diff = |n: &str| t.accounts.iter().find(|a| a.label() == n).expect(n).diff;
    assert_eq!(diff("1010"), money("9.00"));
    assert_eq!(diff("6010"), money("-9.00"));
    assert!(!t.all_zero);
}
