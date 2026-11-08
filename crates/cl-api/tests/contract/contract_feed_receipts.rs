//! Receipts on Bank Feed accepts against the live engine (A05; engine
//! U-181 `feed-accept-draft-v1`, U-180 `document-drafts-reopen-v1`).
//! PENDING on engines without them.
//!
//! - An accept item with `documentDraft` posts the line, files the
//!   receipt on the entry and closes the draft in one transaction.
//! - A stale or already-used receipt fails its row (`STALE`,
//!   `DOCUMENT_DRAFT_CLOSED {transactionId}`): nothing posts.
//! - Undo deletes the entry and lists the draft in `reopenedDrafts`; the
//!   draft is ready again with its file.
//!
//! Every test imports its own rows into its own fresh bank account and
//! claims its own synthetic receipt.

use cl_api::{
    AcceptFailure, AcceptItem, AcceptRequest, AccountType, AttachTo, Capabilities, DocumentDraft,
    DraftKind, DraftLink, DraftReader, DraftReading, DraftStatus, FeedQuery, FeedStatus,
    PageRequest, Session, TxnType, UndoTarget,
};
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Money};
use serde_json::Map;

use crate::common::{date, key};
use crate::contract_feed_accept::{fresh, import, status_of, values};
use crate::support::Harness;

fn gated(h: &Harness) -> bool {
    h.require_cap(Capabilities::FEED_ACCEPT, "U-170")
        && h.require_cap(Capabilities::FEED_UNDO, "U-169")
        && h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
        && h.require_cap(Capabilities::DOCUMENT_DRAFTS, "U-92")
        && h.require_cap(Capabilities::FEED_ACCEPT_DRAFT, "U-181")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or_else(|e| panic!("money {s}: {e:?}"))
}

/// A ready receipt (cash expense) of `total` dated `on`, its own file.
fn receipt(h: &Harness, s: &Session, total: &str, on: CalendarDate) -> DocumentDraft {
    let bytes = format!(
        "%PDF-1.4\n% synthetic feed receipt {}\n%%EOF\n",
        key().as_header()
    );
    let file = h
        .run(s.attachments().upload_draft(
            cl_api::DraftKey::new(),
            "receipt.pdf",
            bytes.as_bytes(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("upload: {e}"))
        .id;
    let d = h
        .run(
            s.document_drafts()
                .create(file, DraftKind::CashExpense, key()),
        )
        .unwrap_or_else(|e| panic!("claim: {e:?}"));
    let reading = DraftReading {
        kind: DraftKind::CashExpense,
        reader: DraftReader::TextPdf,
        fields: Map::new(),
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: None,
        doc_reference: Some("R-1042".into()),
        doc_total: Some(m(total)),
        doc_date: Some(on),
    };
    h.run(
        s.document_drafts()
            .store_reading(d.id, &d.updated_at, &reading),
    )
    .unwrap_or_else(|e| panic!("store reading: {e:?}"))
}

fn link(d: &DocumentDraft) -> DraftLink {
    DraftLink {
        id: d.id,
        expected_updated_at: d.updated_at.clone(),
    }
}

fn entry_of(h: &Harness, s: &Session, id: cl_api::FeedItemId) -> Option<TransactionId> {
    h.run(s.feed().list(&FeedQuery {
        limit: 500,
        ..FeedQuery::default()
    }))
    .expect("list")
    .rows
    .into_iter()
    .find(|r| r.id == id)
    .and_then(|r| r.matched_transaction)
}

fn files_on(h: &Harness, s: &Session, t: TransactionId) -> usize {
    let page = PageRequest::new(50, 0).expect("page");
    h.run(
        s.attachments()
            .list(AttachTo::Txn(TxnType::Expense, t), page),
    )
    .map_or(0, |p| p.data.len())
}

/// Receipt first, then the bank line: one entry with the receipt on it and
/// the draft closed into it (the 4-decimal total matches the 2-decimal
/// bank amount); Undo deletes the entry and the receipt is ready again.
pub fn accept_files_the_receipt_and_undo_reopens_it(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let d = receipt(h, s, "84.1700", date(2027, 6, 1));
    let rows = import(
        h,
        s,
        bank,
        &[(date(2027, 6, 3), "84.17", "A HARDWARE 1042")],
    );
    let req = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[0].id, values(supplies))),
        draft: link(&d),
    }]);
    let o = h
        .run(s.feed().accept(&req, None, key()))
        .unwrap_or_else(|e| panic!("accept: {e:?}"));
    assert_eq!((o.posted, o.failures.len()), (1, 0), "{:?}", o.failures);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let closed = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(closed.status, DraftStatus::Converted);
    assert_eq!(closed.converted_transaction_id, Some(t));
    assert_eq!(files_on(h, s, t), 1, "the receipt is filed on the entry");

    if !h.require_cap(Capabilities::DOCUMENT_DRAFTS_REOPEN, "U-180") {
        return;
    }
    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Group(o.undo_group), false, None, key()),
        )
        .expect("undo");
    assert!(u.undone.iter().all(|x| x.deleted));
    assert_eq!(u.reopened_drafts, vec![d.id]);
    let reopened = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(reopened.status, DraftStatus::Ready);
    assert_eq!(reopened.converted_transaction_id, None);
    assert_eq!(reopened.attachment_id, d.attachment_id, "its file is back");
    assert!(status_of(h, s, rows[0].id).is_some_and(FeedStatus::is_waiting));
}

/// A receipt changed since it was read fails its row; a receipt already
/// used fails with the entry it went to. Nothing posts for those rows.
pub fn stale_or_used_receipts_post_nothing(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let d = receipt(h, s, "19.99", date(2027, 6, 10));
    let rows = import(
        h,
        s,
        bank,
        &[
            (date(2027, 6, 11), "19.99", "A SHOP ONE"),
            (date(2027, 6, 12), "19.99", "B SHOP TWO"),
        ],
    );
    let stale = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[0].id, values(supplies))),
        draft: DraftLink {
            id: d.id,
            expected_updated_at: "2020-01-01T00:00:00.000Z".into(),
        },
    }]);
    let o = h.run(s.feed().accept(&stale, None, key())).expect("accept");
    assert_eq!(o.posted, 0);
    assert_eq!(o.failures[0].code, AcceptFailure::DRAFT_STALE);
    assert!(o.failures[0].is_receipt());
    assert!(status_of(h, s, rows[0].id).is_some_and(FeedStatus::is_waiting));

    let ok = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[0].id, values(supplies))),
        draft: link(&d),
    }]);
    let o = h.run(s.feed().accept(&ok, None, key())).expect("accept");
    assert_eq!(o.posted, 1);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let now = h.run(s.document_drafts().get(d.id)).expect("draft");
    let used = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[1].id, values(supplies))),
        draft: link(&now),
    }]);
    let o = h.run(s.feed().accept(&used, None, key())).expect("accept");
    assert_eq!(o.posted, 0);
    assert_eq!(o.failures[0].code, AcceptFailure::DRAFT_CLOSED);
    assert_eq!(
        o.failures[0].details.id::<TransactionId>("transactionId"),
        Some(t)
    );
    assert!(status_of(h, s, rows[1].id).is_some_and(FeedStatus::is_waiting));
}

/// The bank line first, then the receipt (the late receipt, A05 step 2):
/// the entry the feed posted takes the receipt through A04's attach; no
/// second entry.
pub fn a_late_receipt_attaches_to_the_feed_entry(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let rows = import(h, s, bank, &[(date(2027, 6, 20), "42.00", "A LATE ONE")]);
    let posted = h
        .run(s.feed().accept(
            &AcceptRequest::Items(vec![AcceptItem::Values(rows[0].id, values(supplies))]),
            None,
            key(),
        ))
        .expect("post");
    assert_eq!(posted.posted, 1);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let d = receipt(h, s, "42.0000", date(2027, 6, 19));
    let attached = h
        .run(s.document_drafts().attach(d.id, t, &d.updated_at, key()))
        .unwrap_or_else(|e| panic!("attach: {e:?}"));
    assert_eq!(attached.transaction, t);
    assert_eq!(files_on(h, s, t), 1);
}
