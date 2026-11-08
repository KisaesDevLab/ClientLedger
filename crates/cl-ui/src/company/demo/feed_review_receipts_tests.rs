//! A05 demo parity: receipts in the Bank Feed, both acceptance orders,
//! Undo putting the receipt back, and refused receipts posting nothing.

#![allow(
    clippy::many_single_char_names,
    reason = "s, t, d, o, u as in the other demo tests"
)]

use cl_api::{
    AcceptItem, AcceptRequest, DocumentDraftId, DraftLink, DraftStatus, FeedChoice, FeedItemId,
    FeedQuery, FeedRow, UndoTarget,
};
use cl_core::ids::TenantId;
use cl_docread::register_match::{self, DocSide, Probe};

use super::super::{accept, undo};
use super::*;
use crate::company::demo::{checks, drafts};

fn state() -> (DemoState, TenantId) {
    let mut s = DemoState::new();
    let t = s.companies[0].tenant_id;
    s.open = Some(t);
    (s, t)
}

fn rows(s: &mut DemoState, accepted: bool) -> Vec<FeedRow> {
    super::super::list(
        s,
        &FeedQuery {
            waiting_only: !accepted,
            accepted,
            limit: 500,
            ..FeedQuery::default()
        },
    )
    .expect("list")
    .rows
}

fn feed_row(s: &mut DemoState, id: FeedItemId) -> FeedRow {
    rows(s, false)
        .into_iter()
        .chain(rows(s, true))
        .find(|r| r.id == id)
        .expect("row")
}

fn live_drafts(s: &DemoState, t: TenantId) -> Vec<cl_api::DocumentDraft> {
    crate::screens::bank_feed::receipts::load_demo(s, Some(t)).expect("drafts on")
}

fn draft(s: &DemoState, t: TenantId, id: DocumentDraftId) -> cl_api::DocumentDraft {
    drafts::get(s, t, id).expect("draft")
}

fn link(s: &DemoState, t: TenantId, id: DocumentDraftId) -> DraftLink {
    DraftLink {
        id,
        expected_updated_at: draft(s, t, id).updated_at,
    }
}

fn entries(s: &DemoState) -> usize {
    s.ledger.transactions().len()
}

/// The receipt's own coding, as the panel sends it.
fn with_receipt(
    s: &mut DemoState,
    t: TenantId,
    line: FeedItemId,
    d: DocumentDraftId,
) -> AcceptItem {
    let r = feed_row(s, line);
    let full = draft(s, t, d);
    let account = r.suggested_account.as_ref().map(|a| a.id).expect("account");
    let coded = crate::screens::bank_feed::receipts::coding(&full, r.amount);
    AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(
            line,
            crate::screens::bank_feed::receipts::choice_of(&coded, account, None),
        )),
        draft: link(s, t, d),
    }
}

/// Receipt first, then the bank line: the line is offered the receipt and
/// accepting it posts one entry with the receipt filed on it.
#[test]
fn receipt_first_then_bank_line_ends_with_one_entry_and_the_receipt_attached() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, true).expect("seeded");
    // 4 decimals stored, the bank's 2 compared exactly.
    let total = draft(&s, t, d).doc_total.expect("total");
    assert_eq!(total.to_wire().split('.').nth(1).map(str::len), Some(4));
    let waiting = rows(&mut s, false);
    let offers = crate::screens::bank_feed::receipts::offers_for(&waiting, &live_drafts(&s, t));
    assert_eq!(offers.get(&line).map(Vec::as_slice), Some(&[d][..]));
    assert_eq!(offers.len(), 1, "only the line it matches");

    let before = entries(&s);
    let item = with_receipt(&mut s, t, line, d);
    let o = accept(&mut s, &AcceptRequest::Items(vec![item]), None).expect("accept");
    assert_eq!((o.posted, o.failures.len()), (1, 0));
    assert_eq!(entries(&s), before + 1, "one entry");
    let posted = feed_row(&mut s, line).matched_transaction.expect("entry");
    let closed = draft(&s, t, d);
    assert_eq!(closed.status, DraftStatus::Converted);
    assert_eq!(closed.converted_transaction_id, Some(posted));
    assert_eq!(checks::files_on(&s, posted), 1, "the receipt is on it");
    // Its lines added up exactly: two splits with the receipt's memos.
    let tx = s
        .ledger
        .transactions()
        .into_iter()
        .find(|x| crate::company::demo::register::DemoLedger::id_of(x) == posted)
        .expect("entry");
    assert_eq!(
        tx.lines.len(),
        3,
        "the bank line and the receipt's two lines"
    );
    assert!(tx.memo.contains("R-1042"));
    // Nothing is offered any more.
    let left =
        crate::screens::bank_feed::receipts::offers_for(&rows(&mut s, false), &live_drafts(&s, t));
    assert!(left.is_empty());
}

/// The bank line first, then the receipt: A04's register search finds the
/// entry the Bank Feed posted, first, and attaching files the receipt on
/// it without a second entry.
#[test]
fn bank_line_first_then_receipt_ends_with_one_entry_and_the_receipt_attached() {
    let (mut s, t) = state();
    let (line, _) = {
        // Pick the line the seed would, without a receipt yet.
        let (line, d) = seed_receipt(&mut s, false).expect("seeded");
        drafts::discard(&mut s, t, d).expect("discard");
        (line, d)
    };
    let o = accept(
        &mut s,
        &AcceptRequest::Items(vec![AcceptItem::AsProposed(line)]),
        None,
    )
    .expect("accept");
    assert_eq!(o.posted, 1);
    let posted = feed_row(&mut s, line).matched_transaction.expect("entry");
    let before = entries(&s);

    let d = seed_receipt_for(&mut s, line, false).expect("late receipt");
    let rd = draft(&s, t, d);
    let probe = Probe {
        amount: rd.doc_total.expect("total"),
        date: rd.doc_date.expect("date"),
        payee: None,
        contact: rd.suggested_contact_id,
        side: DocSide::Purchase,
    };
    let (start, end) = register_match::search_window(probe.date);
    let found = drafts::search_amount(
        &mut s,
        &drafts::AmountSearch {
            amount: probe.amount,
            start,
            end,
            account: None,
        },
    )
    .expect("search");
    let rows: Vec<register_match::RegisterRow> = found
        .iter()
        .filter_map(|r| crate::screens::doc_review::attach::row_of(r, false))
        .collect();
    let c = register_match::candidates(&probe, &rows);
    assert_eq!(
        c.first().map(|c| c.id),
        Some(posted),
        "the bank entry first"
    );

    drafts::attach(
        &mut s,
        t,
        d,
        posted,
        &rd.updated_at,
        cl_api::IdempotencyKey::new(),
    )
    .expect("attach");
    assert_eq!(entries(&s), before, "no second entry");
    assert_eq!(checks::files_on(&s, posted), 1);
    assert_eq!(draft(&s, t, d).converted_transaction_id, Some(posted));
}

/// Undo deletes the entry and the receipt goes back to the review queue
/// with its file (U-180).
#[test]
fn undo_puts_the_receipt_back_in_the_queue_with_its_file() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, false).expect("seeded");
    let item = with_receipt(&mut s, t, line, d);
    let o = accept(&mut s, &AcceptRequest::Items(vec![item]), None).expect("accept");
    let posted = feed_row(&mut s, line).matched_transaction.expect("entry");
    let u = undo(&mut s, &UndoTarget::Group(o.undo_group), false, None).expect("undo");
    assert_eq!(u.reopened_drafts, vec![d]);
    let back = draft(&s, t, d);
    assert_eq!(back.status, DraftStatus::Ready);
    assert_eq!(back.converted_transaction_id, None);
    assert_eq!(checks::files_on(&s, posted), 0);
    let file = back.attachment_id.expect("file");
    let a = checks::find_attachment(&s, file).expect("kept");
    assert_eq!(a.attachable_type, "draft");
    // Offered again, and accepted again with a fresh token.
    let again =
        crate::screens::bank_feed::receipts::offers_for(&rows(&mut s, false), &live_drafts(&s, t));
    assert_eq!(again.get(&line).map(Vec::len), Some(1));
    let item = with_receipt(&mut s, t, line, d);
    let o = accept(&mut s, &AcceptRequest::Items(vec![item]), None).expect("again");
    assert_eq!(o.posted, 1);
}

/// A receipt changed or used since it was read fails its row: nothing
/// posts, the row stays waiting, the receipt is untouched.
#[test]
fn a_stale_or_used_receipt_posts_nothing() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, false).expect("seeded");
    let before = entries(&s);
    let stale = AcceptItem::Receipt {
        item: Box::new(AcceptItem::AsProposed(line)),
        draft: DraftLink {
            id: d,
            expected_updated_at: "2020-01-01T00:00:00.000Z".into(),
        },
    };
    let o = accept(&mut s, &AcceptRequest::Items(vec![stale]), None).expect("accept");
    assert_eq!(o.posted, 0);
    assert_eq!(o.failures[0].code, cl_api::AcceptFailure::DRAFT_STALE);
    assert_eq!(entries(&s), before);
    assert!(feed_row(&mut s, line).status.is_waiting());
    assert_eq!(draft(&s, t, d).status, DraftStatus::Ready);

    // Used elsewhere (attached by hand to another entry the feed posted):
    // closed.
    let other = rows(&mut s, false)
        .into_iter()
        .find(|r| r.id != line && r.suggested_account.is_some() && r.is_money_out())
        .expect("another line");
    let o = accept(
        &mut s,
        &AcceptRequest::Items(vec![AcceptItem::AsProposed(other.id)]),
        None,
    )
    .expect("post the other");
    assert_eq!(o.posted, 1);
    let elsewhere = feed_row(&mut s, other.id)
        .matched_transaction
        .expect("entry");
    let token = draft(&s, t, d).updated_at;
    drafts::attach(
        &mut s,
        t,
        d,
        elsewhere,
        &token,
        cl_api::IdempotencyKey::new(),
    )
    .expect("attach");
    assert_eq!(draft(&s, t, d).status, DraftStatus::Converted);
    let used = AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(
            line,
            FeedChoice {
                account: feed_row(&mut s, line).suggested_account.map(|a| a.id),
                ..FeedChoice::default()
            },
        )),
        draft: DraftLink {
            id: d,
            expected_updated_at: draft(&s, t, d).updated_at,
        },
    };
    let o = accept(&mut s, &AcceptRequest::Items(vec![used]), None).expect("accept");
    assert_eq!(o.posted, 0);
    assert_eq!(o.failures[0].code, cl_api::AcceptFailure::DRAFT_CLOSED);
    assert!(feed_row(&mut s, line).status.is_waiting());
}

/// The closing date is checked before the receipt, as on the engine: a
/// locked row asks for the password even with a stale receipt.
#[test]
fn the_closing_date_comes_before_the_receipt() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, false).expect("seeded");
    let on = feed_row(&mut s, line).date;
    if let Some(c) = s.companies.iter_mut().find(|c| c.tenant_id == t) {
        c.closed_through = Some(on);
    }
    let stale = AcceptItem::Receipt {
        item: Box::new(AcceptItem::AsProposed(line)),
        draft: DraftLink {
            id: d,
            expected_updated_at: "2020-01-01T00:00:00.000Z".into(),
        },
    };
    let o = accept(&mut s, &AcceptRequest::Items(vec![stale]), None).expect("accept");
    assert_eq!(o.posted, 0);
    assert_eq!(o.failures[0].code, cl_api::AcceptFailure::PERIOD_LOCKED);
    assert_eq!(draft(&s, t, d).status, DraftStatus::Ready);
}

/// U-180 parity: deleting the feed's entry from the register puts its
/// receipt back in the queue with its file, as Undo does.
#[test]
fn a_register_delete_puts_the_receipt_back() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, false).expect("seeded");
    let item = with_receipt(&mut s, t, line, d);
    accept(&mut s, &AcceptRequest::Items(vec![item]), None).expect("accept");
    let posted = feed_row(&mut s, line).matched_transaction.expect("entry");
    assert_eq!(checks::files_on(&s, posted), 1);
    let back = drafts::delete_entry(&mut s, posted).expect("delete");
    assert_eq!(back, vec![d]);
    let ready = draft(&s, t, d);
    assert_eq!(ready.status, DraftStatus::Ready);
    let file = ready.attachment_id.expect("file");
    assert_eq!(
        checks::find_attachment(&s, file)
            .expect("kept")
            .attachable_type,
        "draft"
    );
    assert_eq!(checks::files_on(&s, posted), 0);
}

/// U-180 parity: a draft converted in the review queue (no feed key on
/// record) goes back on its own id when its entry is deleted; a draft no
/// longer converted is left alone.
#[test]
fn reopen_uses_the_drafts_id_and_only_converted_drafts() {
    let (mut s, t) = state();
    let (line, d) = seed_receipt(&mut s, false).expect("seeded");
    let o = accept(
        &mut s,
        &AcceptRequest::Items(vec![AcceptItem::AsProposed(line)]),
        None,
    )
    .expect("post");
    assert_eq!(o.posted, 1);
    let posted = feed_row(&mut s, line).matched_transaction.expect("entry");
    let token = draft(&s, t, d).updated_at;
    // A04's attach (not the feed): no key recorded.
    drafts::attach(&mut s, t, d, posted, &token, cl_api::IdempotencyKey::new()).expect("attach");
    let back = drafts::reopen_for(&mut s, posted);
    assert_eq!(back, vec![d]);
    let file = draft(&s, t, d).attachment_id.expect("file");
    let a = checks::find_attachment(&s, file).expect("kept");
    assert_eq!(
        (a.attachable_type.as_str(), a.attachable_id),
        ("draft", d.to_string())
    );
    // Ready now: a second delete reopens nothing.
    assert_eq!(
        drafts::reopen_for(&mut s, posted),
        Vec::<DocumentDraftId>::new()
    );
}
