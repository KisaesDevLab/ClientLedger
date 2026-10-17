//! Two clients editing the same record get `Conflict` (AC4). Until U-17 the
//! check is the editable-field fingerprint read before the save; the live
//! server-409 path is PENDING(U-17) and switches on by itself when the engine
//! advertises `record-version-v1`.

use cl_api::{
    AccountType, AccountUpdate, ApiError, Capabilities, ConflictReason, ContactFields, ContactType,
    ContactUpdate, Method, NewContact, PostingOptions, RecordVersion,
};
use cl_core::Money;

use crate::common::{account, date, je2, key, pair, post};
use crate::support::{Harness, unique};

fn assert_stale(e: &ApiError, server: bool) {
    match e {
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version,
            ..
        } => match (server, current_version) {
            (false, Some(RecordVersion::Fingerprint(_)))
            | (true, Some(RecordVersion::Server(_))) => {}
            other => panic!("unexpected version {other:?}"),
        },
        other => panic!("expected Conflict(Stale), got {other:?}"),
    }
}

pub fn two_clients_contact_and_entry(h: &Harness) {
    let s1 = &h.s1;
    let s2 = h.s2();
    let server = h.caps.has(Capabilities::RECORD_VERSION);

    let new = NewContact {
        fields: ContactFields::named(ContactType::Customer, unique("Shared")),
        opening_balance: Money::ZERO,
        opening_balance_date: None,
    };
    let c = h.run(s1.contacts().create(&new, key())).expect("contact");
    let mine = h.run(s1.contacts().get(c.record.id)).expect("get");
    let theirs = h.run(s2.contacts().get(c.record.id)).expect("get");
    let mut f2 = theirs.record.fields.clone();
    f2.phone = Some("555-0199".into());
    h.run(s2.contacts().update(
        c.record.id,
        &theirs.version,
        &ContactUpdate {
            fields: f2,
            is_active: true,
        },
        key(),
    ))
    .expect("other window saves first");
    let puts = h.observer.sent(Method::Put, "/api/v1/contacts/{id}");
    let mut f1 = mine.record.fields.clone();
    f1.notes = Some("mine".into());
    let e = h
        .run(s1.contacts().update(
            c.record.id,
            &mine.version,
            &ContactUpdate {
                fields: f1,
                is_active: true,
            },
            key(),
        ))
        .expect_err("stale");
    assert_stale(&e, server);
    if !server {
        assert_eq!(
            h.observer.sent(Method::Put, "/api/v1/contacts/{id}"),
            puts,
            "the stale PUT must not be sent"
        );
    }

    let (cash, supplies) = pair(h, s1);
    let memo = unique("shared entry");
    let on = date(2026, 8, 1);
    let made = post(h, s1, &je2(on, &memo, supplies.id, cash.id, "10.0000"));
    let mine = h.run(s1.transactions().get(made.record.id)).expect("get");
    let theirs = h.run(s2.transactions().get(made.record.id)).expect("get");
    h.run(s2.transactions().update_journal_entry(
        made.record.id,
        &theirs.version,
        &je2(on, &memo, supplies.id, cash.id, "11.0000"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("other window saves first");
    let e = h
        .run(s1.transactions().update_journal_entry(
            made.record.id,
            &mine.version,
            &je2(on, &memo, supplies.id, cash.id, "12.0000"),
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("stale");
    assert_stale(&e, server);
}

/// Posting to an account changes its balance, not its editable fields, so a
/// rename with the version read before the posting still saves (S7).
pub fn posting_does_not_conflict_an_account_rename(h: &Harness) {
    let s = &h.s1;
    let cash = account(h, s, AccountType::Asset, "Rename me");
    let before = h.run(s.accounts().get(cash.id)).expect("get");
    let exp = account(h, s, AccountType::Expense, "Other side");
    post(
        h,
        s,
        &je2(date(2026, 8, 2), &unique("p"), exp.id, cash.id, "1.0000"),
    );
    let after = h.run(s.accounts().get(cash.id)).expect("get");
    assert_eq!(
        after.version, before.version,
        "balance changed the fingerprint"
    );
    let mut fields = before.record.fields.clone();
    fields.name = format!("{} (renamed)", fields.name);
    h.run(s.accounts().update(
        cash.id,
        &before.version,
        &AccountUpdate {
            fields,
            is_active: true,
        },
        key(),
    ))
    .expect("rename after posting");
}

/// The live server-side 409 STALE (U-17). The client side is covered by the
/// stub contract in `tests/stub`.
pub fn server_version_conflict_u17(h: &Harness) {
    if !h.require_cap(Capabilities::RECORD_VERSION, "U-17") {
        return;
    }
    let s = &h.s1;
    let cash = account(h, s, AccountType::Asset, "Versioned");
    let got = h.run(s.accounts().get(cash.id)).expect("get");
    assert!(matches!(got.version, RecordVersion::Server(_)));
    let s2 = h.s2();
    let theirs = h.run(s2.accounts().get(cash.id)).expect("get");
    let mut f = theirs.record.fields.clone();
    f.description = Some("theirs".into());
    h.run(s2.accounts().update(
        cash.id,
        &theirs.version,
        &AccountUpdate {
            fields: f.clone(),
            is_active: true,
        },
        key(),
    ))
    .expect("first save");
    f.description = Some("mine".into());
    let e = h
        .run(s.accounts().update(
            cash.id,
            &got.version,
            &AccountUpdate {
                fields: f,
                is_active: true,
            },
            key(),
        ))
        .expect_err("409 STALE");
    assert_stale(&e, true);
}
