//! The bank lifecycle guards of PR-5 against the live engine (U-199
//! `feed-entry-changed-v1`, U-200 `feed-guards-v1`). PENDING on engines
//! before the PR-5 pin.
//!
//! - `an_amount_edit_flags_the_matched_line` (T2): a bank line matched to
//!   an entry, then the entry's amount edited: the line says "Entry
//!   changed", is counted, the edited entry is no candidate for another
//!   bank line, and Review again puts it back to waiting.
//! - `a_transfer_of_a_line_in_the_books_asks` (S3): a transfer of a bank
//!   line that matches an entry is 409 `FEED_ITEM_HAS_MATCH`; `force`
//!   under a new key records it.
//! - `a_replace_of_a_staged_item_is_refused_and_said` (S1): the preview
//!   says Replace won't be done (`existingReplaceable: false`) and the
//!   commit says why (`replaceRefused: staged`).
//! - `accept_takes_the_owners_transfer_window` (N1): `transferWindowDays`
//!   on `POST /feed/accept`.
//!
//! Every test uses its own fresh accounts, dated in July 2027, clear of
//! the other feed tests' dates.

use cl_api::{
    AcceptItem, AcceptRequest, AccountType, BankFileFormat, BankImportRequest, BankImportRow,
    Capabilities, ChangedAction, FeedQuery, FeedRefusal, FeedStatus, PostingOptions,
    TransferTarget, feed_refusal,
};
use rust_decimal::Decimal;

use crate::common::{date, je2, key, post};
use crate::contract_feed_accept::{fresh, import, status_of, values};
use crate::support::{Harness, unique};

fn gated(h: &Harness) -> bool {
    h.require_cap(Capabilities::FEED_UNDO, "U-169")
        && h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
        && h.require_cap(Capabilities::FEED_MATCH_V2, "U-166")
}

/// T2 (Q-PR5a, U-199).
pub fn an_amount_edit_flags_the_matched_line(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::FEED_ENTRY_CHANGED, "U-199") {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let made = post(h, s, &je2(date(2027, 7, 5), "Changed", exp, bank, "45.00"));
    let t = made.record.id;
    let rows = import(h, s, bank, &[(date(2027, 7, 6), "45.00", "OFFICE SUPPLY")]);
    let id = rows[0].id;
    let c = h
        .run(s.feed().match_candidates(id))
        .expect("candidates")
        .into_iter()
        .find(|c| c.transaction == t)
        .expect("the entry is a candidate");
    h.run(s.feed().match_line(id, t, Some(c.line), key()))
        .unwrap_or_else(|e| panic!("match: {e}"));
    let before = h.run(s.feed().counts(None)).expect("counts").entry_changed;
    // The owner adds the forgotten cable: 54.00.
    h.run(s.transactions().update_journal_entry(
        t,
        &made.version,
        &je2(date(2027, 7, 5), "Changed", exp, bank, "54.00"),
        &PostingOptions::default(),
        key(),
    ))
    .unwrap_or_else(|e| panic!("edit: {e}"));
    let changed = h
        .run(s.feed().list(&FeedQuery {
            entry_changed: Some(true),
            limit: 500,
            ..FeedQuery::default()
        }))
        .expect("list");
    let row = changed
        .rows
        .iter()
        .find(|r| r.id == id)
        .expect("the line is flagged");
    assert!(row.entry_changed && row.entry_changed_at.is_some());
    assert!(row.status.is_accepted(), "kept accepted (Q-PR5a)");
    assert_eq!(
        h.run(s.feed().counts(None)).expect("counts").entry_changed,
        before + 1
    );
    // A second 54.00 bank line can't take the edited entry.
    let second = import(
        h,
        s,
        bank,
        &[(date(2027, 7, 9), "54.00", "OFFICE SUPPLY 2")],
    );
    let offered = h
        .run(s.feed().match_candidates(second[0].id))
        .expect("candidates");
    assert!(
        offered.iter().all(|c| c.transaction != t),
        "the edited entry's line stays held by the first bank line"
    );
    // Review again: back to waiting, the flag gone.
    h.run(s.feed().review_again(id, key()))
        .unwrap_or_else(|e| panic!("review again: {e}"));
    assert!(status_of(h, s, id).is_some_and(FeedStatus::is_waiting));
}

/// S3 (U-200): `FEED_ITEM_HAS_MATCH`, then `force`.
pub fn a_transfer_of_a_line_in_the_books_asks(h: &Harness) {
    if !gated(h)
        || !h.require_cap(Capabilities::FEED_MATCH_POST, "U-171")
        || !h.require_cap(Capabilities::FEED_GUARDS, "U-200")
    {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let savings = fresh(h, s, AccountType::Asset, Some("bank"));
    post(
        h,
        s,
        &je2(
            date(2027, 7, 12),
            "Transfer in books",
            savings,
            bank,
            "500.00",
        ),
    );
    let rows = import(
        h,
        s,
        bank,
        &[(date(2027, 7, 12), "500.00", "TRANSFER TO SAVINGS")],
    );
    let id = rows[0].id;
    let e = h
        .run(
            s.feed()
                .transfer(id, TransferTarget::Account(savings), None, key()),
        )
        .expect_err("a line already in the books");
    match feed_refusal(&e) {
        Some(FeedRefusal::HasMatch { candidates }) => assert!(candidates >= 1),
        other => panic!("refusal {other:?} ({e})"),
    }
    assert!(status_of(h, s, id).is_some_and(FeedStatus::is_waiting));
    h.run(
        s.feed()
            .transfer_with(id, TransferTarget::Account(savings), true, None, key()),
    )
    .unwrap_or_else(|e| panic!("force: {e}"));
    assert_eq!(status_of(h, s, id), Some(FeedStatus::Matched));
}

fn request(account: cl_core::ids::AccountId, fitid: &str, amount: &str) -> BankImportRequest {
    let on = date(2027, 7, 20);
    BankImportRequest {
        account,
        format: BankFileFormat::Qfx,
        file_name: "replace.qfx".into(),
        source_file_sha256: "cd".repeat(32),
        section_index: 0,
        section_start: Some(on),
        section_end: Some(on),
        amounts_flipped: false,
        csv_mapping: None,
        relink_identity: false,
        bank_identity: None,
        file_ending_balance: None,
        file_ending_date: None,
        rows: vec![BankImportRow {
            line: 1,
            date: on,
            amount: amount.parse::<Decimal>().expect("decimal"),
            description: "HARDWARE STORE".into(),
            memo: None,
            check_number: None,
            fitid: Some(fitid.to_string()),
            include: None,
            changed_action: None,
        }],
    }
}

/// S1 (U-200): `existingReplaceable` and `replaceRefused`.
pub fn a_replace_of_a_staged_item_is_refused_and_said(h: &Harness) {
    if !gated(h)
        || !h.require_cap(Capabilities::FEED_EXACT, "U-164")
        || !h.require_cap(Capabilities::FEED_GUARDS, "U-200")
    {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let fitid = unique("R");
    let first = h
        .run(
            s.bank_import()
                .commit(&request(bank, &fitid, "80.00"), key()),
        )
        .expect("import");
    let id = first.inserted_items[0];
    h.run(s.feed().assign(id, &values(exp)))
        .unwrap_or_else(|e| panic!("assign: {e}"));
    let mut again = request(bank, &fitid, "85.00");
    again.rows[0].changed_action = Some(ChangedAction::Replace);
    let p = h.run(s.bank_import().preview(&again)).expect("preview");
    let row = &p.rows[0];
    assert_eq!(row.status, cl_api::ImportRowStatus::Changed, "{row:?}");
    assert_eq!(row.existing_replaceable, Some(false), "staged: no Replace");
    let done = h
        .run(s.bank_import().commit(&again, key()))
        .expect("commit");
    let row = &done.rows[0];
    assert!(!row.replaced);
    assert_eq!(row.replace_refused.as_deref(), Some("staged"));
}

/// N1 (U-200): the owner's transfer window goes with an accept.
pub fn accept_takes_the_owners_transfer_window(h: &Harness) {
    if !gated(h)
        || !h.require_cap(Capabilities::FEED_ACCEPT, "U-170")
        || !h.require_cap(Capabilities::FEED_GUARDS, "U-200")
    {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let rows = import(h, s, bank, &[(date(2027, 7, 25), "12.34", "WINDOW")]);
    let o = h
        .run(s.feed().accept_in(
            &AcceptRequest::Items(vec![AcceptItem::Values(rows[0].id, values(exp))]),
            Some(7),
            None,
            key(),
        ))
        .unwrap_or_else(|e| panic!("accept: {e}"));
    assert_eq!(o.posted, 1);
}
