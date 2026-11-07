//! Bank file imports against the live engine (P25 plan §6, U-06
//! `bank-import-batch-v1`): the preview changes nothing and takes no key,
//! a commit needs one and replays it, re-importing adds nothing (by bank id
//! and, without one, by row), undo brings rows back on the next import,
//! amounts stay exact, and a non-bank account is refused. The review
//! fix pass adds the bank id rules (reused, per-download, a moved date),
//! changed rows, the engine-held account link (Q-P25u), the row and total
//! limits, a view-only role (Q-P25ab) and Undo's used lines.
//!
//! Every test makes its own bank (or card) account in Contract Co, so the
//! feeds never mix. Rows are built inline (synthetic; the parsers' fixtures
//! are cl-import's own tests).

use cl_api::{
    AccountType, ApiError, BankFileFormat, BankIdentity, BankImportRefusal, BankImportRequest,
    BankImportRow, BankLookupBy, BankLookupVia, Capabilities, ChangedAction, ImportRowReason,
    ImportRowStatus, LocalValidation, Method, Session, bank_import_refusal,
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
    // Refused here, never sent (review DS6: not any error, this one).
    let e = h
        .run(s.bank_import().preview(&bad))
        .expect_err("3 decimals");
    assert!(
        matches!(e, ApiError::Local(LocalValidation::NotWholeCents)),
        "{e:?}"
    );
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

/// QA-3: `fixtures/bank/card_positive_issuer.qfx` (issuer's signs, the
/// wizard flips them) and `card_ccstmt.qfx` (holder's signs, as is) send
/// the same feed-sign rows and the same holder-signed balance, -429.12.
/// The engine answers in amounts owed and echoes the file's balance as
/// sent: on a fresh card account the rows net -70.78 owed (charges
/// 454.22, credits 525.00), so projected is -70.78 and the difference is
/// -(-429.12) - (-70.78) = 499.90. The desktop shows "Bank says you owe"
/// as -file_balance so the three figures read in one sign.
pub fn card_fixture_balance_in_either_convention(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let tag = unique("CC");
    let day = |d: u32| date(2026, 10, d);
    // Feed sign: a charge (money out) is positive.
    let rows = || {
        [
            (5, "45.67", "OFFICE DEPOT #221"),
            (7, "12.00", "PARKING GARAGE"),
            (10, "230.40", "AIRLINE TICKET"),
            (12, "-500.00", "PAYMENT THANK YOU"),
            (15, "89.10", "HOTEL STAY"),
            (18, "15.25", "LUNCH MEETING"),
            (22, "-25.00", "RETURN OFFICE DEPOT"),
            (25, "61.80", "FUEL"),
        ]
        .iter()
        .enumerate()
        .map(|(i, (d, amount, text))| {
            let line = u32::try_from(i).expect("line") * 6 + 40;
            row(line, day(*d), amount, text, Some(&format!("{tag}-{i}")))
        })
        .collect::<Vec<_>>()
    };
    for flipped in [true, false] {
        let acct = fresh_account(h, s, AccountType::Liability, "credit_card");
        let mut req = request(acct, BankFileFormat::Qfx, rows());
        req.amounts_flipped = flipped;
        req.file_ending_balance = Some(dec("-429.12"));
        req.file_ending_date = Some(day(31));
        let p = h.run(s.bank_import().preview(&req)).expect("preview");
        let b = p.balance_check.expect("balance check");
        assert_eq!(b.file_balance, m("-429.12"), "echoed in the holder's sign");
        assert_eq!(b.books_at_date, m("0.00"));
        assert_eq!(b.new_total, m("-70.78"), "feed sign, all 8 rows");
        assert_eq!(b.projected, m("-70.78"), "owed once accepted");
        assert_eq!(b.difference, m("499.90"), "flipped={flipped}");
        // The desktop's reading: owed 429.12 - owed -70.78.
        let owed = m("0.00").checked_sub(b.file_balance).expect("neg");
        assert_eq!(owed.checked_sub(b.projected).expect("sub"), b.difference);
    }
}

fn fitid_row(line: u32, day: CalendarDate, amount: &str, text: &str, fitid: &str) -> BankImportRow {
    row(line, day, amount, text, Some(fitid))
}

/// Waits out the detached preparation, then undoes `batch`.
fn undo_when_prepared(
    h: &Harness,
    s: &Session,
    batch: cl_api::ImportBatchId,
    pending_only: bool,
) -> Result<cl_api::ImportUndoOutcome, ApiError> {
    for _ in 0..40 {
        match h.run(s.bank_import().undo(batch, pending_only, key())) {
            Err(e) if bank_import_refusal(&e) == Some(BankImportRefusal::StillPreparing) => {
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            other => return other,
        }
    }
    panic!("the import was never prepared");
}

/// Engine fixtures 22, 22r, 22b: a bank id repeated in one file is
/// ignored on every occurrence, in any order; nothing becomes "changed".
pub fn reused_fitids_never_change(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let id = unique("DUP");
    let f22 = vec![
        fitid_row(1, date(2027, 7, 10), "15.00", "LUNCH SPOT", &id),
        fitid_row(2, date(2027, 7, 11), "30.00", "BOOK STORE", &id),
    ];
    let first = h
        .run(
            s.bank_import()
                .commit(&request(acct, BankFileFormat::Qfx, f22.clone()), key()),
        )
        .expect("first");
    assert_eq!((first.counts.new, first.counts.inserted), (2, 2));
    assert!(
        first
            .rows
            .iter()
            .all(|r| r.reason == Some(ImportRowReason::RepeatedBankId))
    );
    let mut reversed = f22.clone();
    reversed.reverse();
    for rows in [f22.clone(), reversed, vec![f22[1].clone()]] {
        let r = h
            .run(
                s.bank_import()
                    .commit(&request(acct, BankFileFormat::Qfx, rows), key()),
            )
            .expect("again");
        assert_eq!((r.counts.inserted, r.counts.changed), (0, 0));
    }
}

/// Engine fixture 23: a bank that numbers each download from 1 reuses
/// ids for other transactions; the next month's rows are new
/// (`bank_id_reused`), and both files again add nothing.
pub fn per_download_fitids_are_new(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let tag = unique("PD");
    let month = |m: u32, cents: &str| -> Vec<BankImportRow> {
        (1..=3)
            .map(|i| {
                fitid_row(
                    i,
                    date(2027, m, 10 + i),
                    &format!("{i}{cents}"),
                    &format!("{tag} {m} {i}"),
                    &format!("{tag}-{i}"),
                )
            })
            .collect()
    };
    let mut nov = request(acct, BankFileFormat::Qfx, month(8, "0.00"));
    nov.source_file_sha256 = "e".repeat(64);
    let mut dec = request(acct, BankFileFormat::Qfx, month(9, "1.00"));
    dec.source_file_sha256 = "f".repeat(64);
    h.run(s.bank_import().commit(&nov, key())).expect("nov");
    let d = h.run(s.bank_import().commit(&dec, key())).expect("dec");
    assert_eq!(
        (d.counts.new, d.counts.changed, d.counts.inserted),
        (3, 0, 3)
    );
    assert!(
        d.rows
            .iter()
            .all(|r| r.reason == Some(ImportRowReason::BankIdReused))
    );
    for again in [&nov, &dec] {
        let r = h.run(s.bank_import().commit(again, key())).expect("again");
        assert_eq!(r.counts.inserted, 0);
    }
}

/// Engine fixture 24: the same bank id, amount and a date moved 2 days is
/// the same transaction.
pub fn date_shift_is_a_duplicate(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let rows = ofx_rows();
    h.run(
        s.bank_import()
            .commit(&request(acct, BankFileFormat::Qfx, rows.clone()), key()),
    )
    .expect("first");
    let mut shifted = rows[0].clone();
    shifted.date = date(2027, 3, 4);
    let p = h
        .run(
            s.bank_import()
                .preview(&request(acct, BankFileFormat::Qfx, vec![shifted])),
        )
        .expect("preview");
    assert_eq!(p.rows[0].status, ImportRowStatus::Duplicate);
    assert_eq!(p.rows[0].reason, Some(ImportRowReason::SameBankId));
}

/// A row the bank changed (same id, another amount): "replace" updates the
/// waiting item; once the item is used, replace is refused for the row
/// and "add" inserts it as new.
pub fn changed_rows_replace_and_add(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let id = unique("CH");
    let first = fitid_row(1, date(2027, 8, 20), "80.00", "HARDWARE STORE 12", &id);
    let c = h
        .run(
            s.bank_import()
                .commit(&request(acct, BankFileFormat::Qfx, vec![first]), key()),
        )
        .expect("first");
    let item = c.inserted_items[0];
    let changed = fitid_row(1, date(2027, 8, 21), "85.00", "HARDWARE STORE 12", &id);
    let p = h
        .run(
            s.bank_import()
                .preview(&request(acct, BankFileFormat::Qfx, vec![changed.clone()])),
        )
        .expect("preview");
    assert_eq!(p.rows[0].status, ImportRowStatus::Changed);
    assert_eq!(p.rows[0].existing_item, Some(item));
    assert_eq!(p.rows[0].existing_pending, Some(true));
    let mut replace = changed.clone();
    replace.changed_action = Some(ChangedAction::Replace);
    let r = h
        .run(
            s.bank_import()
                .commit(&request(acct, BankFileFormat::Qfx, vec![replace]), key()),
        )
        .expect("replace");
    assert!(r.rows[0].replaced && !r.rows[0].inserted);
    // The owner uses the item in the Bank Feed (excluded).
    let used = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/banking/feed/{item}/exclude"),
            &[],
            None,
        ))
        .expect("exclude");
    assert_eq!(used.status, 200, "{}", used.body);
    let mut again = changed.clone();
    again.amount = dec("90.00");
    again.changed_action = Some(ChangedAction::Replace);
    let refused = h
        .run(s.bank_import().commit(
            &request(acct, BankFileFormat::Qfx, vec![again.clone()]),
            key(),
        ))
        .expect("replace refused for the row");
    assert!(!refused.rows[0].replaced && !refused.rows[0].inserted);
    again.changed_action = Some(ChangedAction::Add);
    let added = h
        .run(
            s.bank_import()
                .commit(&request(acct, BankFileFormat::Qfx, vec![again]), key()),
        )
        .expect("add");
    assert!(added.rows[0].inserted);
    assert_eq!(added.counts.inserted, 1);
}

/// Q-P25u: the engine links a file's account number (raw in the body,
/// hashed under the company's key) on commit; lookup by number finds it
/// by hash, by mask (last 4) otherwise; another account's preview says so,
/// its commit is refused (409) until relinked.
pub fn identity_links_and_relinks(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh_account(h, s, AccountType::Asset, "bank");
    let card = fresh_account(h, s, AccountType::Liability, "credit_card");
    let digits: String = unique("")
        .chars()
        .filter(char::is_ascii_digit)
        .chain("0000000000".chars())
        .take(10)
        .collect();
    let ident = BankIdentity {
        bank_id: Some("021000021".into()),
        acct_id: format!("88{digits}"),
    };
    let mut req = request(bank, BankFileFormat::Qfx, ofx_rows());
    req.bank_identity = Some(ident.clone());
    h.run(s.bank_import().commit(&req, key())).expect("linked");
    let hit = h
        .run(
            s.bank_import()
                .lookup(&BankLookupBy::Account(ident.clone())),
        )
        .expect("lookup");
    assert_eq!(hit[0].account, bank);
    assert_eq!(hit[0].via, BankLookupVia::Hash);
    let mask = hit[0].mask.clone().expect("mask");
    assert!(ident.acct_id.ends_with(&mask), "the last 4 only");
    let other_bank = BankIdentity {
        bank_id: Some("111000025".into()),
        acct_id: ident.acct_id.clone(),
    };
    let by_mask = h
        .run(s.bank_import().lookup(&BankLookupBy::Account(other_bank)))
        .expect("by mask");
    assert!(
        by_mask
            .iter()
            .any(|m| m.account == bank && m.via == BankLookupVia::Mask)
    );
    let mut to_card = request(card, BankFileFormat::Qfx, ofx_rows());
    to_card.bank_identity = Some(ident);
    let p = h.run(s.bank_import().preview(&to_card)).expect("preview");
    assert!(p.identity_linked_elsewhere.is_some());
    let e = h
        .run(s.bank_import().commit(&to_card, key()))
        .expect_err("409");
    assert!(matches!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::LinkedElsewhere(_))
    ));
    to_card.relink_identity = true;
    h.run(s.bank_import().commit(&to_card, key()))
        .expect("relinked");
}

/// Over 20,000 rows: refused by the desktop before sending, and by the
/// engine (422 `too_many_rows`) when sent anyway.
pub fn too_many_rows(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let tag = unique("MANY");
    let rows: Vec<BankImportRow> = (1..=20_001)
        .map(|i| row(i, date(2027, 1, 5), "1.00", &tag, None))
        .collect();
    let req = request(acct, BankFileFormat::Csv, rows);
    let e = h
        .run(s.bank_import().preview(&req))
        .expect_err("refused here");
    assert!(
        matches!(e, ApiError::Local(LocalValidation::TooManyLines)),
        "{e:?}"
    );
    let mut small = req.clone();
    small.rows.truncate(1);
    let mut raw = small.body(true).expect("body");
    raw["rows"] = serde_json::Value::Array(
        (1..=20_001)
            .map(|i| serde_json::json!({"line": i, "date": "2027-01-05", "amount": "1.00", "description": tag}))
            .collect(),
    );
    let r = h
        .run(s.raw(
            Method::Post,
            "/api/v1/banking/feed/import-batch",
            &[],
            Some(&raw),
        ))
        .expect("raw");
    assert_eq!(r.status, 422, "{}", r.body);
    assert_eq!(
        r.body["error"]["code"]
            .as_str()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("too_many_rows")
    );
}

/// The rows of one batch may not add up to 10^15 or more (422
/// `batch_total_too_large`).
pub fn batch_total_too_large(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let tag = unique("HUGE");
    let rows: Vec<BankImportRow> = (1..=1001)
        .map(|i| {
            row(
                i,
                date(2027, 1, 6),
                "999999999999.99",
                &format!("{tag} {i}"),
                None,
            )
        })
        .collect();
    let e = h
        .run(
            s.bank_import()
                .preview(&request(acct, BankFileFormat::Csv, rows)),
        )
        .expect_err("422");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::TotalTooLarge)
    );
}

/// Q-P25ab: a role that may only view banking can preview a file but not
/// import it (403).
pub fn view_only_previews_but_cannot_import(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let (user, bk) = h.bookkeeper();
    let r = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&serde_json::json!({ "overrides": { "banking": "view" } })),
        ))
        .expect("permissions");
    assert_eq!(r.status, 200, "{}", r.body);
    let req = request(acct, BankFileFormat::Qfx, ofx_rows());
    let p = h
        .run(bk.bank_import().preview(&req))
        .expect("a view may preview");
    assert_eq!(p.counts.new, 3);
    let e = h
        .run(bk.bank_import().commit(&req, key()))
        .expect_err("403");
    assert!(matches!(e, ApiError::Forbidden { .. }), "{e:?}");
    // Back to the bookkeeper's defaults for the tests after this one.
    let _ = h.run(s.raw(
        Method::Put,
        &format!("/api/v1/company/users/{user}/permissions"),
        &[],
        Some(&serde_json::json!({ "overrides": { "banking": "full" } })),
    ));
}

/// Undo refuses a batch whose rows were used, naming their lines; with
/// pending-only the rest go.
pub fn undo_names_the_used_lines(h: &Harness) {
    if !caps(h) {
        return;
    }
    let s = &h.s1;
    let acct = fresh_account(h, s, AccountType::Asset, "bank");
    let c = h
        .run(
            s.bank_import()
                .commit(&request(acct, BankFileFormat::Qfx, ofx_rows()), key()),
        )
        .expect("commit");
    let batch = c.batch.expect("batch");
    let first = c.inserted_items[0];
    let used = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/banking/feed/{first}/exclude"),
            &[],
            None,
        ))
        .expect("exclude");
    assert_eq!(used.status, 200, "{}", used.body);
    let e = undo_when_prepared(h, s, batch, false).expect_err("409");
    assert_eq!(
        bank_import_refusal(&e),
        Some(BankImportRefusal::RowsAlreadyUsed(vec![10]))
    );
    let u = undo_when_prepared(h, s, batch, true).expect("pending only");
    assert_eq!((u.removed, u.kept), (2, 1));
}
