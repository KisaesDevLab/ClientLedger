//! Bank file imports against the live engine (P25 plan §6, U-06
//! `bank-import-batch-v1`): the preview changes nothing and takes no key,
//! a commit needs one and replays it, re-importing adds nothing (by bank id
//! and, without one, by row), undo brings rows back on the next import,
//! amounts stay exact, and a non-bank account is refused.
//!
//! Every test makes its own bank (or card) account in Contract Co, so the
//! feeds never mix. Rows are built inline (synthetic; the parsers' fixtures
//! are cl-import's own tests).

use cl_api::{
    AccountType, BankFileFormat, BankImportRefusal, BankImportRequest, BankImportRow, Capabilities,
    ImportRowReason, ImportRowStatus, Method, Session, bank_import_refusal,
};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use rust_decimal::Decimal;

use crate::common::{account_fields, date, key, m};
use crate::support::{Harness, unique};

fn caps(h: &Harness) -> bool {
    h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
}

fn dec(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

/// A fresh account of `t` with `detail` (`bank`, `credit_card`).
fn fresh_account(h: &Harness, s: &Session, t: AccountType, detail: &str) -> AccountId {
    let mut f = account_fields(t, "Import");
    f.detail_type = Some(detail.into());
    h.run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
        .id
}

fn row(
    line: u32,
    day: CalendarDate,
    amount: &str,
    text: &str,
    fitid: Option<&str>,
) -> BankImportRow {
    BankImportRow {
        line,
        date: day,
        amount: dec(amount),
        description: text.into(),
        memo: None,
        check_number: None,
        fitid: fitid.map(str::to_string),
        include: None,
        changed_action: None,
    }
}

fn request(
    account: AccountId,
    format: BankFileFormat,
    rows: Vec<BankImportRow>,
) -> BankImportRequest {
    BankImportRequest {
        account,
        format,
        file_name: "contract.qfx".into(),
        source_file_sha256: "ab".repeat(32),
        section_index: 0,
        section_start: rows.iter().map(|r| r.date).min(),
        section_end: rows.iter().map(|r| r.date).max(),
        amounts_flipped: false,
        csv_mapping: None,
        relink_identity: false,
        bank_identity: None,
        file_ending_balance: None,
        file_ending_date: None,
        rows,
    }
}

/// Three rows with bank ids unique to this run.
fn ofx_rows() -> Vec<BankImportRow> {
    let tag = unique("F");
    vec![
        row(
            10,
            date(2027, 3, 2),
            "186.40",
            "MONETT TIRE AND LUBE",
            Some(&format!("{tag}-1")),
        ),
        row(
            14,
            date(2027, 3, 3),
            "-8450.00",
            "ACH CREDIT HOA",
            Some(&format!("{tag}-2")),
        ),
        row(
            18,
            date(2027, 3, 4),
            "45.00",
            "SQ FARMERS MKT",
            Some(&format!("{tag}-3")),
        ),
    ]
}

/// The engine advertises both P25 capabilities.
pub fn capabilities(h: &Harness) {
    if !caps(h) {
        return;
    }
    assert!(
        h.s1.client()
            .capabilities()
            .has(Capabilities::FEED_CLOSING_DATE)
    );
}

/// A preview changes no batch; a commit without a key is refused (400
/// `IDEMPOTENCY_KEY_REQUIRED`).
pub fn preview_changes_nothing_and_commit_needs_a_key(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let before = h
        .run(s.bank_import().batches(Some(acct), None))
        .expect("batches");
    let p = h.run(s.bank_import().preview(&req)).expect("preview");
    assert_eq!(p.batch, None);
    assert_eq!(p.counts.in_file, 3);
    assert_eq!(p.counts.new, 3);
    assert_eq!(p.counts.inserted, 0);
    assert!(
        p.rows
            .iter()
            .all(|r| r.status == ImportRowStatus::New && !r.inserted)
    );
    h.run(s.bank_import().preview(&req)).expect("preview again");
    let after = h
        .run(s.bank_import().batches(Some(acct), None))
        .expect("batches");
    assert_eq!(before.len(), after.len(), "a preview records no batch");
    let body = req.body(false).expect("body");
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/banking/feed/import-batch",
            &[],
            Some(&body),
        ))
        .expect("raw");
    assert_eq!(raw.status, 400, "{}", raw.body);
    assert_eq!(raw.body["error"]["code"], "IDEMPOTENCY_KEY_REQUIRED");
}

/// Plan §6 "Re-import adds 0": the same rows twice add nothing the second
/// time, by bank id; rows without one match by row.
pub fn reimport_adds_nothing(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let first = h.run(s.bank_import().commit(&req, key())).expect("first");
    assert_eq!(first.counts.inserted, 3);
    assert!(first.batch.is_some());
    assert_eq!(first.inserted_items.len(), 3);
    assert_eq!(first.inserted_total, m("-8218.60"));
    let again = h.run(s.bank_import().commit(&req, key())).expect("again");
    assert_eq!(again.counts.inserted, 0, "re-import adds nothing");
    assert_eq!(again.counts.duplicate, 3);
    assert_eq!(again.counts.changed, 0);
    assert!(
        again
            .rows
            .iter()
            .all(|r| r.reason == Some(ImportRowReason::SameBankId))
    );
    assert!(again.same_file_imported_at.is_some());
    assert!(
        again.batch.is_some(),
        "a commit that adds nothing still records a batch"
    );

    // CSV rows (no bank id): matched by date, amount and description.
    let tag = unique("CSV ROW");
    let csv = request(
        acct,
        BankFileFormat::Csv,
        vec![
            row(2, date(2027, 4, 1), "12.34", &tag, None),
            // Identical rows on one day are both new (ordinals 1 and 2).
            row(3, date(2027, 4, 1), "12.34", &tag, None),
        ],
    );
    let first = h.run(s.bank_import().commit(&csv, key())).expect("csv");
    assert_eq!(first.counts.inserted, 2);
    let again = h
        .run(s.bank_import().commit(&csv, key()))
        .expect("csv again");
    assert_eq!(again.counts.inserted, 0);
    assert!(
        again
            .rows
            .iter()
            .all(|r| r.status == ImportRowStatus::Duplicate
                && r.reason == Some(ImportRowReason::SameRow))
    );
}

/// A lost answer: the same key replays the first commit, marked replayed,
/// and adds nothing more.
pub fn commit_replays_its_key(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let k = key();
    let first = h
        .run(s.bank_import().commit(&req, k.clone()))
        .expect("first");
    let replay = h.run(s.bank_import().commit(&req, k)).expect("replay");
    assert!(replay.replayed, "Idempotent-Replayed: true");
    assert!(!first.replayed);
    assert_eq!(replay.batch, first.batch);
    assert_eq!(replay.counts, first.counts);
    assert_eq!(replay.inserted_total, first.inserted_total);
}

/// Undo takes the untouched rows out; importing again brings them back.
pub fn undo_then_reimport_brings_rows_back(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let first = h.run(s.bank_import().commit(&req, key())).expect("first");
    let batch = first.batch.expect("batch");
    // Post-processing runs detached; undo waits for it.
    let mut undone = None;
    for _ in 0..40 {
        match h.run(s.bank_import().undo(batch, false, key())) {
            Ok(u) => {
                undone = Some(u);
                break;
            }
            Err(e) if bank_import_refusal(&e) == Some(BankImportRefusal::StillPreparing) => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(e) => panic!("undo: {e}"),
        }
    }
    let undone = undone.expect("undone once preparation finished");
    assert_eq!((undone.removed, undone.kept), (3, 0));
    let e = h
        .run(s.bank_import().undo(batch, false, key()))
        .expect_err("twice");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::AlreadyUndone)
    );
    let listed = h
        .run(
            s.bank_import()
                .batches(Some(acct), Some(BankFileFormat::Qfx)),
        )
        .expect("batches");
    assert!(
        listed
            .iter()
            .any(|b| b.id == batch && b.undone_at.is_some())
    );
    let back = h.run(s.bank_import().commit(&req, key())).expect("again");
    assert_eq!(back.counts.inserted, 3, "the rows come back");
}

/// Amounts stay exact: the smallest and the largest amount are accepted
/// and add up exactly; a third decimal never leaves the desktop.
pub fn amounts_are_exact(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let tag = unique("EXACT");
    let req = request(
        acct,
        BankFileFormat::Csv,
        vec![
            row(2, date(2027, 5, 1), "0.01", &tag, None),
            row(3, date(2027, 5, 2), "-999999999999.99", &tag, None),
        ],
    );
    let done = h.run(s.bank_import().commit(&req, key())).expect("commit");
    assert_eq!(done.counts.inserted, 2);
    assert_eq!(done.inserted_total, m("-999999999999.98"));
    let mut bad = req.clone();
    bad.rows[0].amount = dec("12.345");
    assert!(h.run(s.bank_import().preview(&bad)).is_err());
}

/// An income account is not a bank or card account (422).
pub fn non_bank_account_is_refused(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Revenue, "service");
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let e = h.run(s.bank_import().preview(&req)).expect_err("422");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::NotBankOrCard)
    );
}

/// A card file with a balance gets the engine's balance check (amounts
/// owed); the desktop only displays it.
pub fn card_balance_check(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Liability, "credit_card");
    let tag = unique("F");
    let mut req = request(
        acct,
        BankFileFormat::Qfx,
        vec![
            row(
                5,
                date(2027, 6, 1),
                "64.38",
                "OFFICE SUPPLY",
                Some(&format!("{tag}-1")),
            ),
            row(
                9,
                date(2027, 6, 2),
                "35.62",
                "FUEL",
                Some(&format!("{tag}-2")),
            ),
        ],
    );
    // The card statement says -100.00 (owed, holder's sign).
    req.file_ending_balance = Some(dec("-100.00"));
    req.file_ending_date = Some(date(2027, 6, 2));
    let p = h.run(s.bank_import().preview(&req)).expect("preview");
    let b = p.balance_check.expect("balance check");
    assert_eq!(b.file_balance, m("-100.00"));
    assert_eq!(b.new_total, m("100.00"));
    assert_eq!(
        b.difference,
        m("0.00"),
        "owed 100.00 once both are accepted"
    );
}
