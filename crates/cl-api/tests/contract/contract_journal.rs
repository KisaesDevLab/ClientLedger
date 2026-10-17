//! Journal entries: create, read, update, void, list by type and date, the
//! ledger-version stamp, and outcome reconciliation after a lost reply
//! (AC1, AC5; PR-P05 B1 missing test 2).

use cl_api::{
    Basis, EditableFields, Method, NewJournalEntry, PageRequest, PostingOptions, Side,
    TransactionFilter, TxnStatus, TxnType,
};

use crate::common::{date, je2, key, line, m, pair, post, wire_lines};
use crate::support::{Harness, unique};

pub fn round_trip_create_read_update_void_list(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("JE round trip");
    let on = date(2026, 3, 14);
    let stamp0 = h.run(s.ledger_version()).expect("stamp");
    let entry = NewJournalEntry::new(
        on,
        Some(memo.clone()),
        Basis::Both,
        vec![
            line(supplies.id, Side::Debit(m("1234.5678"))),
            line(supplies.id, Side::Debit(m("0.0001"))),
            line(cash.id, Side::Credit(m("1234.5679"))),
        ],
    )
    .expect("entry");
    let made = post(h, s, &entry);
    assert_eq!(made.record.txn_type, TxnType::JournalEntry);
    assert_eq!(made.record.status, TxnStatus::Posted);
    let stamp1 = h.run(s.ledger_version()).expect("stamp");
    assert_ne!(stamp1, stamp0, "posting must move the ledger stamp");

    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(got.record.txn_date, on);
    assert_eq!(got.record.memo.as_deref(), Some(memo.as_str()));
    assert_eq!(
        wire_lines(&got.record),
        vec![
            ("1234.5678".to_string(), "0.0000".to_string()),
            ("0.0001".to_string(), "0.0000".to_string()),
            ("0.0000".to_string(), "1234.5679".to_string()),
        ]
    );
    assert_eq!(got.record.fingerprint(), entry.fingerprint());

    // Identical lines: the engine recreates the lines (new ids), but the
    // editable fingerprint is unchanged (S7).
    let same = h
        .run(s.transactions().update_journal_entry(
            made.record.id,
            &got.version,
            &entry,
            &PostingOptions::default(),
            key(),
        ))
        .expect("identical update");
    let old_ids: Vec<_> = got.record.lines.iter().map(|l| l.id).collect();
    let reread = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(reread.version, got.version, "fingerprint moved");
    assert_eq!(same.version, got.version);
    assert!(
        reread.record.lines.iter().all(|l| !old_ids.contains(&l.id)),
        "the engine is expected to recreate lines on update"
    );

    let changed = je2(on, &memo, supplies.id, cash.id, "77.7700");
    let upd = h
        .run(s.transactions().update_journal_entry(
            made.record.id,
            &reread.version,
            &changed,
            &PostingOptions::default(),
            key(),
        ))
        .expect("update");
    let after = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(after.version, upd.version);
    assert_eq!(
        wire_lines(&after.record),
        vec![
            ("77.7700".to_string(), "0.0000".to_string()),
            ("0.0000".to_string(), "77.7700".to_string()),
        ]
    );

    let filter = TransactionFilter {
        txn_type: Some(TxnType::JournalEntry),
        start_date: Some(on),
        end_date: Some(on),
        search: Some(memo.clone()),
        ..TransactionFilter::default()
    };
    let page = h
        .run(s.transactions().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1);
    assert_eq!(page.data[0].id, made.record.id);

    let voided = h
        .run(s.transactions().void(
            made.record.id,
            "entered twice",
            &PostingOptions::default(),
            key(),
        ))
        .expect("void");
    assert_eq!(voided.record.status, TxnStatus::Void);
    let reread = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(reread.record.status, TxnStatus::Void);
    assert_ne!(h.run(s.ledger_version()).expect("stamp"), stamp1);
}

/// The engine commits a JE but its reply is lost after it answered (the
/// observer drops it). cl-api reconciles through the ledger stamp and the
/// editable fingerprint, reports success, and there is exactly one posting.
/// The same for a void.
pub fn lost_reply_is_reconciled_to_exactly_one_posting(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let memo = unique("lost reply");
    let on = date(2026, 4, 2);
    let entry = je2(on, &memo, supplies.id, cash.id, "42.4242");
    let posts_before = h.observer.sent(Method::Post, "/api/v1/transactions");
    h.observer
        .drop_next_reply(Method::Post, "/api/v1/transactions");
    let made = h
        .run(
            s.transactions()
                .create_journal_entry(&entry, &PostingOptions::default(), key()),
        )
        .expect("reconciled as committed");
    assert_eq!(h.observer.dropped(), 1, "the reply was not dropped");
    assert_eq!(
        h.observer.sent(Method::Post, "/api/v1/transactions") - posts_before,
        1,
        "the create must not be re-sent"
    );
    assert_eq!(made.record.fingerprint(), entry.fingerprint());
    let filter = TransactionFilter {
        txn_type: Some(TxnType::JournalEntry),
        start_date: Some(on),
        end_date: Some(on),
        search: Some(memo),
        ..TransactionFilter::default()
    };
    let page = h
        .run(s.transactions().list(&filter, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1, "exactly one posting");

    h.observer
        .drop_next_reply(Method::Post, "/api/v1/transactions/{id}/void");
    let voided = h
        .run(
            s.transactions()
                .void(made.record.id, "lost", &PostingOptions::default(), key()),
        )
        .expect("void reconciled");
    assert_eq!(h.observer.dropped(), 2);
    assert_eq!(voided.record.status, TxnStatus::Void);
}
