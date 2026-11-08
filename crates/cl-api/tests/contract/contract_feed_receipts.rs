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
//! - A match carrying a receipt files it on the entry in the books; Undo
//!   only unlinks and the receipt stays on it (Q-A05g).
//! - The receipt's lines post as two splits at their exact amounts.
//! - A late receipt: A04's exact-amount search finds the feed's entry.
//!
//! Every test imports its own rows into its own fresh bank account and
//! claims its own synthetic receipt.

use cl_api::{
    AcceptFailure, AcceptItem, AcceptRequest, AccountType, AttachTo, Capabilities, DocumentDraft,
    DraftKind, DraftLink, DraftReader, DraftReading, DraftStatus, FeedChoice, FeedQuery,
    FeedSplitLine, FeedStatus, PageRequest, Session, TransactionFilter, TxnStatus, TxnType,
    UndoTarget,
};
use cl_core::ids::TransactionId;
use cl_core::{CalendarDate, Money};
use serde_json::Map;

use crate::common::{date, je2, key, post};
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
        acknowledged_duplicates: Vec::new(),
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
            acknowledged_duplicates: Vec::new(),
        },
    }]);
    let o = h.run(s.feed().accept(&stale, None, key())).expect("accept");
    assert_eq!((o.posted, o.failures.len()), (0, 1), "{o:?}");
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

/// The receipt's file is filed on `t`.
fn filed_on(h: &Harness, s: &Session, d: &DocumentDraft, t: TransactionId) -> bool {
    let file = d.attachment_id.expect("the receipt has a file");
    h.run(s.attachments().get(file))
        .is_ok_and(|a| a.attachable_type != "draft" && a.attachable_id == t.to_string())
}

/// A match carrying a receipt files it on the entry already in the books
/// (nothing posts); Undo only unlinks: the entry and its receipt stay
/// (Q-A05g), nothing is reopened.
pub fn a_match_files_the_receipt_on_the_entry_in_the_books(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::FEED_MATCH_V2, "U-166") {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let t = post(
        h,
        s,
        &je2(date(2027, 7, 2), "Receipt match", supplies, bank, "63.40"),
    )
    .record
    .id;
    let d = receipt(h, s, "63.4000", date(2027, 7, 1));
    let rows = import(h, s, bank, &[(date(2027, 7, 3), "63.40", "A MATCH 1042")]);
    let req = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Match {
            id: rows[0].id,
            transaction: t,
            line: None,
        }),
        draft: link(&d),
    }]);
    let o = h
        .run(s.feed().accept(&req, None, key()))
        .unwrap_or_else(|e| panic!("accept: {e:?}"));
    assert_eq!((o.matched, o.posted), (1, 0), "{:?}", o.failures);
    assert_eq!(entry_of(h, s, rows[0].id), Some(t), "matched, not posted");
    let closed = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(closed.status, DraftStatus::Converted);
    assert_eq!(closed.converted_transaction_id, Some(t));
    assert!(filed_on(h, s, &d, t), "the receipt is on the entry");

    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Group(o.undo_group), false, None, key()),
        )
        .expect("undo");
    assert!(u.undone.iter().all(|x| !x.deleted), "a match only unlinks");
    assert_eq!(u.reopened_drafts, Vec::new(), "Q-A05g: nothing reopened");
    let kept = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(kept.status, DraftStatus::Converted);
    assert!(filed_on(h, s, &d, t), "the receipt stays on the kept entry");
    assert!(status_of(h, s, rows[0].id).is_some_and(FeedStatus::is_waiting));
}

/// The receipt's lines as the panel sends them (Q-A05b: two whole-cent
/// lines adding up to the bank amount exactly) post as two splits on the
/// panel's account, at their exact amounts, with the receipt filed.
pub fn receipt_lines_post_as_two_splits(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let d = receipt(h, s, "84.1700", date(2027, 7, 8));
    let rows = import(h, s, bank, &[(date(2027, 7, 9), "84.17", "A HARDWARE TWO")]);
    let choice = FeedChoice {
        account: Some(supplies),
        memo: Some("Synthetic Hardware Co. R-1042".into()),
        splits: vec![
            FeedSplitLine {
                account: supplies,
                amount: m("60.00"),
                tag: None,
                memo: Some("Shop supplies".into()),
            },
            FeedSplitLine {
                account: supplies,
                amount: m("24.17"),
                tag: None,
                memo: Some("Hardware".into()),
            },
        ],
        ..FeedChoice::default()
    };
    let req = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[0].id, choice)),
        draft: link(&d),
    }]);
    let o = h
        .run(s.feed().accept(&req, None, key()))
        .unwrap_or_else(|e| panic!("accept: {e:?}"));
    assert_eq!((o.posted, o.failures.len()), (1, 0), "{:?}", o.failures);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let tx = h.run(s.transactions().get(t)).expect("entry").record;
    let split: Vec<Money> = tx
        .lines
        .iter()
        .filter(|l| l.account_id == supplies)
        .map(|l| l.debit)
        .collect();
    assert_eq!(split.len(), 2, "two splits");
    assert!(split.contains(&m("60.00")) && split.contains(&m("24.17")));
    assert_eq!(Money::sum(split).expect("sum"), m("84.17"));
    let on_bank: Vec<Money> = tx
        .lines
        .iter()
        .filter(|l| l.account_id == bank)
        .map(|l| l.credit)
        .collect();
    assert_eq!(on_bank, vec![m("84.17")], "the bank line");
    assert!(filed_on(h, s, &d, t));
}

/// The late receipt (A05 step 2): A04's register search (the exact
/// `amount` filter, posted, a window around the receipt's date) finds the
/// entry the Bank Feed posted, and attaching files the receipt on it.
pub fn a_late_receipts_amount_search_finds_the_feed_entry(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::DOCUMENT_DRAFTS_ATTACH, "U-95") {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let rows = import(h, s, bank, &[(date(2027, 7, 15), "37.21", "A LATE TWO")]);
    let posted = h
        .run(s.feed().accept(
            &AcceptRequest::Items(vec![AcceptItem::Values(rows[0].id, values(supplies))]),
            None,
            key(),
        ))
        .expect("post");
    assert_eq!(posted.posted, 1);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let d = receipt(h, s, "37.2100", date(2027, 7, 14));
    let filter = TransactionFilter {
        status: Some(TxnStatus::Posted),
        start_date: Some(date(2027, 7, 7)),
        end_date: Some(date(2027, 7, 21)),
        amount: d.doc_total,
        ..TransactionFilter::default()
    };
    let page = PageRequest::new(200, 0).expect("page");
    let found = h
        .run(s.transactions().list(&filter, page))
        .unwrap_or_else(|e| panic!("search: {e:?}"));
    assert!(
        found.data.iter().any(|r| r.id == t),
        "the feed's entry is found by its exact amount"
    );
    let attached = h
        .run(s.document_drafts().attach(d.id, t, &d.updated_at, key()))
        .unwrap_or_else(|e| panic!("attach: {e:?}"));
    assert_eq!(attached.transaction, t);
    assert!(filed_on(h, s, &d, t));
}

/// A05 QA-7: a credit memo read below zero. The engine keeps `docTotal`
/// at zero or more, so the client stores none and the summary lists none;
/// the full draft keeps the signed total in its reading. Filed on a
/// money-in line, the deposit posts with the credit on it and the draft
/// closes into it.
pub fn a_credit_keeps_its_sign_in_the_reading_and_files_on_money_in(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let supplies = fresh(h, s, AccountType::Expense, None);
    let bytes = format!(
        "%PDF-1.4\n% synthetic credit memo {}\n%%EOF\n",
        key().as_header()
    );
    let file = h
        .run(s.attachments().upload_draft(
            cl_api::DraftKey::new(),
            "credit.pdf",
            bytes.as_bytes(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("upload: {e}"))
        .id;
    let d = h
        .run(
            s.document_drafts()
                .create(file, DraftKind::CardCharge, key()),
        )
        .unwrap_or_else(|e| panic!("claim: {e:?}"));
    let mut fields = Map::new();
    fields.insert("total".into(), serde_json::json!({ "value": "-45.10" }));
    let reading = DraftReading {
        kind: DraftKind::CardCharge,
        reader: DraftReader::TextPdf,
        fields,
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: None,
        doc_reference: Some("CM-7".into()),
        doc_total: Some(m("-45.10")),
        doc_date: Some(date(2027, 7, 14)),
    };
    let d = h
        .run(
            s.document_drafts()
                .store_reading(d.id, &d.updated_at, &reading),
        )
        .unwrap_or_else(|e| panic!("store reading: {e:?}"));
    assert_eq!(d.doc_total, None, "no total below zero is stored");
    let listed = h
        .run(s.document_drafts().list(&cl_api::DraftListQuery {
            statuses: vec![DraftStatus::Ready],
            summary: true,
            limit: cl_api::draft_limits::MAX_PAGE,
            ..cl_api::DraftListQuery::default()
        }))
        .expect("list");
    let summary = listed.drafts.iter().find(|x| x.id == d.id).expect("listed");
    assert_eq!(summary.doc_total, None);
    assert!(summary.fields.is_none(), "a summary has no reading");
    assert!(summary.doc_date.is_some() && summary.reader.is_some());
    let full = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(
        full.fields
            .as_ref()
            .and_then(|f| f["total"]["value"].as_str()),
        Some("-45.10"),
        "the reading keeps the sign"
    );
    // Money in (the feed's sign: below zero).
    let rows = import(
        h,
        s,
        bank,
        &[(date(2027, 7, 15), "-45.10", "A TOOL SUPPLY REFUND")],
    );
    let req = AcceptRequest::Items(vec![AcceptItem::Receipt {
        item: Box::new(AcceptItem::Values(rows[0].id, values(supplies))),
        draft: link(&full),
    }]);
    let o = h
        .run(s.feed().accept(&req, None, key()))
        .unwrap_or_else(|e| panic!("accept: {e:?}"));
    assert_eq!((o.posted, o.failures.len()), (1, 0), "{:?}", o.failures);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    let closed = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(closed.status, DraftStatus::Converted);
    assert_eq!(closed.converted_transaction_id, Some(t));
    assert!(
        filed_on(h, s, &full, t),
        "the credit is filed on the deposit"
    );
}

/// PR-A1 B1 / T1 (`feed-accept-draft-duplicates-v1`, Q-PRA1b): the owner
/// entered the bill by hand; the vendor's PDF of it is a live receipt; the
/// autopay line comes in. An accept carrying the receipt fails its row
/// `DOCUMENT_POSSIBLE_DUPLICATE` naming the bill (not the bank line's own
/// duplicate), posts nothing and leaves the receipt waiting; sent again
/// acknowledging the bill, it posts once and files the receipt.
pub fn a_receipt_of_a_bill_in_the_books_needs_a_confirmation(h: &Harness) {
    if !gated(h)
        || !h.require_cap(
            Capabilities::FEED_ACCEPT_DRAFT_DUPLICATES,
            "PR-A1 B1 feed accept duplicates",
        )
    {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let utilities = fresh(h, s, AccountType::Expense, None);
    let vendor = h
        .run(s.contacts().create(
            &cl_api::NewContact {
                fields: cl_api::ContactFields::named(
                    cl_api::ContactType::Vendor,
                    format!("Synthetic Power {}", key().as_header()),
                ),
                opening_balance: Money::ZERO,
                opening_balance_date: None,
            },
            key(),
        ))
        .unwrap_or_else(|e| panic!("vendor: {e}"))
        .record
        .id;
    let bill = h
        .run(s.bills().create(
            &cl_api::BillDraft {
                vendor,
                date: date(2027, 7, 2),
                vendor_invoice_number: Some("SYN-33994105".into()),
                terms: None,
                due_date: None,
                memo: None,
                amount_due: m("685.55"),
                lines: vec![cl_api::BillLineDraft::expense(utilities, m("685.55"))],
                received: true,
                draft_attachment: None,
            },
            &cl_api::PostingOptions::default(),
            key(),
        ))
        .unwrap_or_else(|e| panic!("bill: {e:?}"))
        .record
        .txn
        .id;
    // The vendor's PDF of the same invoice, read with the vendor.
    let plain = receipt(h, s, "685.55", date(2027, 7, 2));
    let reading = DraftReading {
        kind: DraftKind::CashExpense,
        reader: DraftReader::TextPdf,
        fields: Map::new(),
        lines: Vec::new(),
        checks: Vec::new(),
        suggested_contact: Some(vendor),
        doc_reference: Some("SYN-33994105".into()),
        doc_total: Some(m("685.55")),
        doc_date: Some(date(2027, 7, 2)),
    };
    let d = h
        .run(
            s.document_drafts()
                .store_reading(plain.id, &plain.updated_at, &reading),
        )
        .unwrap_or_else(|e| panic!("store reading: {e:?}"));
    let rows = import(
        h,
        s,
        bank,
        &[(date(2027, 7, 5), "685.55", "SYN POWER AUTOPAY")],
    );
    let item = |draft: DraftLink| {
        AcceptRequest::Items(vec![AcceptItem::Receipt {
            item: Box::new(AcceptItem::Values(rows[0].id, values(utilities))),
            draft,
        }])
    };
    let o = h
        .run(s.feed().accept(&item(link(&d)), None, key()))
        .expect("accept");
    assert_eq!((o.posted, o.failures.len()), (0, 1), "{o:?}");
    let f = &o.failures[0];
    assert_eq!(f.code, AcceptFailure::DRAFT_POSSIBLE_DUPLICATE);
    assert!(f.is_receipt());
    let named: Vec<_> = f.duplicates().into_iter().map(|x| x.source).collect();
    assert!(
        named.contains(&cl_api::MatchSource::Transaction(bill)),
        "names the bill: {named:?}"
    );
    assert!(status_of(h, s, rows[0].id).is_some_and(FeedStatus::is_waiting));
    let still = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert!(still.status.is_live(), "the receipt waits");
    assert_eq!(still.updated_at, d.updated_at, "unchanged");

    // "Use it anyway": acknowledged, it posts once.
    let acked = DraftLink {
        acknowledged_duplicates: f
            .duplicates()
            .into_iter()
            .filter_map(|x| match x.source {
                cl_api::MatchSource::Transaction(t) => Some(t),
                cl_api::MatchSource::Draft(_) => None,
            })
            .collect(),
        ..link(&d)
    };
    let o = h
        .run(s.feed().accept(&item(acked), None, key()))
        .expect("accept");
    assert_eq!((o.posted, o.failures.len()), (1, 0), "{:?}", o.failures);
    let t = entry_of(h, s, rows[0].id).expect("entry");
    assert_ne!(t, bill);
    let closed = h.run(s.document_drafts().get(d.id)).expect("draft");
    assert_eq!(closed.converted_transaction_id, Some(t));
}
