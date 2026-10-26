//! A backup made on one engine pin restores on the next (engine pin bumps:
//! the archive carries the old schema version and the new engine's
//! migrations must take it). Not part of CI, which has only one engine:
//! a pin bump runs it twice by hand, with the same file.
//!
//! 1. Old engine (e.g. from `target/engine/.cache/<old commit>-…/`):
//!    `CL_ENGINE_EXE=<old exe> CL_ARCHIVE_OUT=<absolute path>.clbackup
//!    cargo test -p cl-api --test archive_across_pins`
//! 2. Pinned engine: `CL_ARCHIVE_IN=<same path> cargo test -p cl-api
//!    --test archive_across_pins`
//!
//! Step 1 writes a company with a bill, its file and a credit card account
//! into the backup; step 2 restores it, checks the bill, file and account,
//! uses the restored company with the pin's newer features where the engine
//! has them (card digits, document drafts), and backs the restored company
//! up and restores it again on the new engine. Without either variable the
//! binary runs nothing. Synthetic data only.

#![allow(
    clippy::too_many_lines,
    reason = "contract scenarios read top to bottom like the other contract binaries"
)]

#[path = "support/mod.rs"]
mod support;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cl_api::{
    AccountFields, AccountFilter, AccountType, ArchiveRequest, AttachTo, BillDraft, BillLineDraft,
    BillListQuery, Capabilities, CardLast4, ContactFields, ContactType, DraftKey, DraftKind,
    DraftListQuery, IdempotencyKey, ImportRequest, JobState, NewContact, PageRequest,
    PostingOptions, Session,
};
use cl_core::ids::{AccountId, TenantId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use support::{EngineMode, Harness, TestFn, run_suite};

const REFERENCE: &str = "PINS-0001";
const CARD: &str = "Pins company card";
const EXPENSE: &str = "Pins office supplies";
const VENDOR: &str = "Pins Synthetic Supply";
const PDF: &[u8] = b"%PDF-1.4\n% synthetic archive-across-pins bill\n%%EOF\n";

fn passphrase() -> Secret {
    Secret::new(format!("{}-{}", "pins", "synthetic-archive"))
}

fn key() -> IdempotencyKey {
    IdempotencyKey::new()
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or_else(|e| panic!("money {s}: {e:?}"))
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn account(h: &Harness, s: &Session, name: &str, t: AccountType, detail: Option<&str>) -> AccountId {
    h.run(s.accounts().create(
        &AccountFields {
            name: name.to_string(),
            account_number: None,
            account_type: t,
            detail_type: detail.map(str::to_string),
            description: None,
            parent_id: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("account: {e}"))
    .record
    .id
}

fn finish_job(h: &Harness, s: &Session, job: &cl_api::JobId) -> cl_api::ArchiveJobStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let st = h.run(s.tenants().archive_job(job)).expect("job");
        match st.status {
            JobState::Succeeded | JobState::Failed => return st,
            JobState::Queued | JobState::Running => {
                assert!(Instant::now() < deadline, "job did not finish");
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
}

/// Backs `co` up (from the workspace) and returns the file's path.
fn export(h: &Harness, s: &Session, co: TenantId) -> PathBuf {
    let job = h
        .run(s.tenants().export_archive(
            co,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: false,
                host: json!({"app": "ClientLedger", "appVersion": "test", "kind": "backup"}),
                passphrase: passphrase(),
            },
        ))
        .expect("export job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    st.export_result()
        .expect("export result")
        .path
        .clone()
        .expect("path")
}

/// Restores the backup at `path` as a new company (from the workspace).
fn import(h: &Harness, s: &Session, path: PathBuf) -> TenantId {
    let job = h
        .run(s.tenants().import_archive(&ImportRequest {
            path,
            passphrase: passphrase(),
            scratch: false,
            reset_document_drafts: false,
        }))
        .expect("import job");
    let st = finish_job(h, s, &job.job_id);
    assert_eq!(st.status, JobState::Succeeded, "{:?}", st.error);
    st.import_result().expect("import result").tenant_id
}

/// Step 1, on the old engine: a company with a bill, its file and a card.
fn write_backup(h: &Harness) {
    let out = env_path("CL_ARCHIVE_OUT").expect("CL_ARCHIVE_OUT");
    let s = h.s3();
    let co = h.tenant_b();
    let expense = account(h, &s, EXPENSE, AccountType::Expense, None);
    account(h, &s, CARD, AccountType::Liability, Some("credit_card"));
    let vendor = h
        .run(s.contacts().create(
            &NewContact {
                fields: ContactFields::named(ContactType::Vendor, VENDOR.to_string()),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .expect("vendor")
        .record
        .id;
    let draft = DraftKey::new();
    h.run(
        s.attachments()
            .upload_draft(draft, "bill.pdf", PDF, key()),
    )
    .expect("upload");
    let bill = h
        .run(s.bills().create(
            &BillDraft {
                vendor,
                date: CalendarDate::from_ymd(2026, 10, 1).expect("date"),
                vendor_invoice_number: Some(REFERENCE.to_string()),
                terms: None,
                due_date: None,
                memo: None,
                amount_due: m("125.00"),
                lines: vec![BillLineDraft::expense(expense, m("125.00"))],
                received: true,
                draft_attachment: None,
            },
            &PostingOptions::default(),
            key(),
        ))
        .expect("bill")
        .record
        .txn
        .id;
    h.run(s.bills().attach_draft(bill, draft, key()))
        .expect("attach");
    h.run(s.close_company()).expect("workspace");
    let path = export(h, &s, co);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("output folder");
    }
    std::fs::copy(&path, &out).expect("copy the backup out");
    eprintln!("archive_across_pins: backup written");
}

/// Checks the restored company `co` and uses it with the pin's features:
/// `true` when the engine has document drafts (a draft was added).
fn check_restored(h: &Harness, s: &Session, co: TenantId) -> bool {
    h.run(s.open_company(co)).expect("open restored");
    let rows = h
        .run(s.bills().list_capped(
            &BillListQuery {
                search: Some(REFERENCE.to_string()),
                ..BillListQuery::default()
            },
            50,
            1,
        ))
        .expect("bills")
        .rows;
    assert_eq!(rows.len(), 1, "the bill is restored");
    let bill = h.run(s.bills().get(rows[0].id)).expect("bill").record;
    assert_eq!(bill.txn.total, Some(m("125.00")));
    assert_eq!(bill.lines.len(), 1);
    let files = h
        .run(s.attachments().list(
            AttachTo::Bill(rows[0].id),
            PageRequest::new(50, 0).expect("page"),
        ))
        .expect("files");
    assert_eq!(files.total, 1, "the bill's file is restored");
    let card = h
        .run(s.accounts().list_all(&AccountFilter {
            search: Some(CARD.to_string()),
            ..AccountFilter::default()
        }))
        .expect("accounts")
        .into_iter()
        .find(|a| a.fields.name == CARD)
        .expect("the card account is restored");
    assert_eq!(card.card_last4, None, "an older backup has no card digits");
    if !h.require_cap(Capabilities::DOCUMENT_DRAFTS, "U-92") {
        return false;
    }
    let v = h.run(s.accounts().get(card.id)).expect("card");
    let set = h
        .run(
            s.accounts()
                .set_card_last4(&v, CardLast4::parse("4821"), key()),
        )
        .expect("card digits on a restored account");
    assert_eq!(set.record.card_last4, CardLast4::parse("4821"));
    let none = h
        .run(s.document_drafts().list(&DraftListQuery {
            limit: 200,
            summary: true,
            ..DraftListQuery::default()
        }))
        .expect("drafts");
    assert!(none.drafts.is_empty(), "an older backup has no drafts");
    let file = h
        .run(s.attachments().upload_draft(
            DraftKey::new(),
            "document.pdf",
            b"%PDF-1.4\n% synthetic archive-across-pins draft\n%%EOF\n",
            key(),
        ))
        .expect("upload")
        .id;
    h.run(s.document_drafts().create(file, DraftKind::Unknown, key()))
        .expect("a draft in a restored company");
    true
}

/// Step 2, on the pinned engine: restore the old backup, use it, and back
/// the restored company up and restore it again.
fn restore_backup(h: &Harness) {
    let input = env_path("CL_ARCHIVE_IN").expect("CL_ARCHIVE_IN");
    let s = h.s3();
    h.run(s.close_company()).expect("workspace");
    let restored = import(h, &s, input);
    let drafts_live = check_restored(h, &s, restored);
    h.run(s.close_company()).expect("workspace");
    let again = export(h, &s, restored);
    let twice = import(h, &s, again);
    h.run(s.open_company(twice)).expect("open restored twice");
    if !drafts_live {
        h.run(s.close_company()).expect("workspace");
        return;
    }
    let drafts = h
        .run(s.document_drafts().list(&DraftListQuery {
            limit: 200,
            summary: true,
            ..DraftListQuery::default()
        }))
        .expect("drafts");
    assert_eq!(drafts.drafts.len(), 1, "the new draft travels in a backup");
    h.run(s.close_company()).expect("workspace");
}

fn main() -> ExitCode {
    let mut tests: Vec<(&str, TestFn)> = Vec::new();
    if env_path("CL_ARCHIVE_OUT").is_some() {
        tests.push(("pins::write_backup", write_backup));
    }
    if env_path("CL_ARCHIVE_IN").is_some() {
        tests.push(("pins::restore_backup", restore_backup));
    }
    if tests.is_empty() {
        eprintln!("SKIPPED: set CL_ARCHIVE_OUT (old engine) or CL_ARCHIVE_IN (pinned engine)");
        println!("running 0 tests\n\ntest result: ok. 0 passed; 0 failed");
        return ExitCode::SUCCESS;
    }
    run_suite(EngineMode::Normal, &tests)
}
