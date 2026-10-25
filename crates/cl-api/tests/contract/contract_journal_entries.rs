//! Make Journal Entry against the real engine (P15 lane J, plan §6
//! Acceptance 1 and the phase tests). The P15 part A capabilities
//! (`je-names-v1`, `je-number-v1`, `je-adjusting-v1`, `je-reversing-v1`,
//! U-58..U-61, `balance-exact-v1`) are PENDING on an engine without them
//! (pinned since 886c6b5); only the unbalanced
//! refusal runs on every engine.
//!
//! - `engine_refuses_unbalanced_raw`: a raw body that bypasses the builder
//!   is refused with 400, posts nothing, and both balances stay exactly.
//! - `exact_balance_no_tolerance` (`balance-exact-v1`): off by 0.0001 is
//!   refused too; balanced exactly it posts.
//! - `ar_ap_names`: an A/R line without a name → 400
//!   `JE_AR_AP_NAME_REQUIRED`; two named A/R lines post since engine
//!   7b267e48, raw and through the typed builder (Q-206, any number);
//!   with a customer it posts and reads back the name.
//! - `numbering`: the suggested next number, then one more after a post.
//! - `adjusting_round_trip`: `isAdjusting` reads back and `adjusting=only`
//!   lists it.
//! - `reversing_creates_linked_entry_on_date`: the reversal's date is
//!   `reverseOn`, lines swapped exactly, linked both ways.
//! - `editing_without_reverse_on_removes_the_reversal`: an edit with the
//!   same Reverse on keeps the reversal; without one (`null`) removes it.

use cl_api::{
    AccountFilter, AccountRoles, Basis, Capabilities, ContactFields, ContactType,
    JE_AR_AP_NAME_REQUIRED, JeLine, JeOptions, Method, NewContact, NewJournalEntry, PostingOptions,
    Session, Side, TxnStatus, has_code,
};
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId};
use serde_json::{Value, json};

use crate::common::{date, key, line, m, pair, post, wire_lines};
use crate::support::{Harness, unique};

fn balance(h: &Harness, s: &Session, id: AccountId) -> Option<String> {
    h.run(s.accounts().get(id))
        .expect("account")
        .record
        .balance
        .map(Money::to_wire)
}

fn raw_je(h: &Harness, s: &Session, body: &Value) -> cl_api::testing::RawReply {
    h.run(s.raw(Method::Post, "/api/v1/transactions", &[], Some(body)))
        .expect("raw post")
}

fn je_line(account: AccountId, debit: &str, credit: &str, contact: Option<ContactId>) -> Value {
    json!({ "accountId": account, "debit": debit, "credit": credit, "contactId": contact })
}

fn customer(h: &Harness, s: &Session) -> ContactId {
    h.run(s.contacts().create(
        &NewContact {
            fields: ContactFields::named(ContactType::Customer, unique("Synthetic Customer")),
            opening_balance: Money::ZERO,
            opening_balance_date: None,
        },
        key(),
    ))
    .unwrap_or_else(|e| panic!("create customer: {e}"))
    .record
    .id
}

/// The company's A/R and A/P accounts, by system tag.
fn roles(h: &Harness, s: &Session) -> AccountRoles {
    let all = h
        .run(s.accounts().list_all(&AccountFilter::default()))
        .expect("accounts");
    let mut r = AccountRoles::default();
    for a in all {
        match a.system_tag.as_deref() {
            Some("accounts_receivable") => {
                r.receivable.insert(a.id);
            }
            Some("accounts_payable") => {
                r.payable.insert(a.id);
            }
            _ => {}
        }
    }
    r
}

pub fn engine_refuses_unbalanced_raw(h: &Harness) {
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let before = (balance(h, s, cash.id), balance(h, s, supplies.id));
    let listed_before = h.run(s.raw(
        Method::Get,
        "/api/v1/transactions",
        &[("accountId", &cash.id.to_string())],
        None,
    ));
    let reply = raw_je(
        h,
        s,
        &json!({
            "txnType": "journal_entry",
            "txnDate": "2026-07-31",
            "basis": "both",
            "lines": [
                je_line(supplies.id, "1200.0000", "0", None),
                je_line(cash.id, "0", "1100.0000", None),
            ],
        }),
    );
    assert_eq!(reply.status, 400, "{}", reply.body);
    assert_eq!(
        (balance(h, s, cash.id), balance(h, s, supplies.id)),
        before,
        "balances unchanged exactly"
    );
    let listed_after = h.run(s.raw(
        Method::Get,
        "/api/v1/transactions",
        &[("accountId", &cash.id.to_string())],
        None,
    ));
    assert_eq!(
        listed_before.expect("list").body["total"],
        listed_after.expect("list").body["total"],
        "no transaction was made"
    );
    // The builder refuses the same entry before sending.
    let local = NewJournalEntry::new(
        date(2026, 7, 31),
        None,
        Basis::Both,
        vec![
            line(supplies.id, Side::Debit(m("1200.00"))),
            line(cash.id, Side::Credit(m("1100.00"))),
        ],
    );
    assert_eq!(local, Err(cl_api::LocalValidation::Unbalanced));
}

/// `balance-exact-v1`: no 0.0001 tolerance. An entry off by one
/// ten-thousandth is refused ("does not balance"), posts nothing, and both
/// balances stay exactly; the same entry balanced exactly posts.
pub fn exact_balance_no_tolerance(h: &Harness) {
    if !h.require_cap(Capabilities::BALANCE_EXACT, "U-65 balance-exact-v1") {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let before = (balance(h, s, cash.id), balance(h, s, supplies.id));
    let body = |debit: &str| {
        json!({
            "txnType": "journal_entry",
            "txnDate": "2026-07-31",
            "basis": "both",
            "lines": [
                je_line(supplies.id, debit, "0", None),
                je_line(cash.id, "0", "10.0000", None),
            ],
        })
    };
    let off = raw_je(h, s, &body("10.0001"));
    assert_eq!(off.status, 400, "{}", off.body);
    assert!(
        off.body["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("does not balance")),
        "{}",
        off.body
    );
    assert_eq!(
        (balance(h, s, cash.id), balance(h, s, supplies.id)),
        before,
        "balances unchanged exactly"
    );
    let ok = raw_je(h, s, &body("10.0000"));
    assert_eq!(ok.status, 201, "{}", ok.body);
    assert_eq!(
        balance(h, s, supplies.id).as_deref(),
        Some("10.0000"),
        "posted exactly"
    );
}

pub fn ar_ap_names(h: &Harness) {
    if !h.require_cap(Capabilities::JE_NAMES, "U-58 je-names-v1") {
        return;
    }
    let s = &h.s1;
    let roles = roles(h, s);
    let Some(&ar) = roles.receivable.iter().next() else {
        panic!("the company has no Accounts Receivable account");
    };
    let (_, income) = pair(h, s);
    let who = customer(h, s);
    let unnamed = raw_je(
        h,
        s,
        &json!({
            "txnType": "journal_entry", "txnDate": "2026-07-31", "basis": "both",
            "lines": [je_line(ar, "50.0000", "0", None), je_line(income.id, "0", "50.0000", None)],
        }),
    );
    assert_eq!(unnamed.status, 400, "{}", unnamed.body);
    assert_eq!(unnamed.body["error"]["code"], JE_AR_AP_NAME_REQUIRED);
    let two = raw_je(
        h,
        s,
        &json!({
            "txnType": "journal_entry", "txnDate": "2026-07-31", "basis": "both",
            "lines": [
                je_line(ar, "50.0000", "0", Some(who)),
                je_line(ar, "0", "50.0000", Some(who)),
            ],
        }),
    );
    // Engine main 7b267e48 (PR #169, commit 33a973fa) dropped
    // `JE_AR_AP_ONE_PER_ENTRY`: every A/R or A/P line needs a name, any
    // number of them may post. Q-206 follows (owner 2026-10-25): the typed
    // builder takes them too.
    assert_eq!(two.status, 201, "{}", two.body);
    let two_typed = NewJournalEntry::new_with(
        date(2026, 7, 31),
        Some(unique("JE two A/R")),
        Basis::Both,
        vec![
            JeLine {
                contact: Some(who),
                ..line(ar, Side::Debit(m("30")))
            },
            JeLine {
                contact: Some(who),
                ..line(ar, Side::Debit(m("20")))
            },
            line(income.id, Side::Credit(m("50"))),
        ],
        JeOptions::default(),
        &roles,
    )
    .expect("two named A/R lines build (Q-206)");
    let made_two = post(h, s, &two_typed);
    let got_two = h
        .run(s.transactions().get(made_two.record.id))
        .expect("get");
    let named_ar = got_two
        .record
        .lines
        .iter()
        .filter(|l| l.account_id == ar && l.contact_id == Some(who))
        .count();
    assert_eq!(named_ar, 2, "both A/R lines read back with the name");
    // Typed path: refused locally, then the engine's code when bypassed.
    let lines = |contact| {
        vec![
            JeLine {
                contact,
                ..line(ar, Side::Debit(m("50")))
            },
            line(income.id, Side::Credit(m("50"))),
        ]
    };
    let mk = |contact, roles: &AccountRoles| {
        NewJournalEntry::new_with(
            date(2026, 7, 31),
            Some(unique("JE name")),
            Basis::Both,
            lines(contact),
            JeOptions::default(),
            roles,
        )
    };
    assert_eq!(
        mk(None, &roles),
        Err(cl_api::LocalValidation::ArApNameMissing)
    );
    let unchecked = mk(None, &AccountRoles::default()).expect("no roles known");
    let e = h
        .run(
            s.transactions()
                .create_journal_entry(&unchecked, &PostingOptions::default(), key()),
        )
        .expect_err("engine refuses");
    assert!(has_code(&e, JE_AR_AP_NAME_REQUIRED), "{e:?}");
    let named = mk(Some(who), &roles).expect("named");
    let made = post(h, s, &named);
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(got.record.lines[0].contact_id, Some(who));
    assert!(got.record.lines[0].contact_name.is_some());
    assert_eq!(got.version, made.version);
}

pub fn numbering(h: &Harness) {
    if !h.require_cap(Capabilities::JE_NUMBER, "U-59 je-number-v1") {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let next = h.run(s.journal().next_number()).expect("next");
    let n: u64 = next.parse().expect("all digits");
    let entry = NewJournalEntry::new_with(
        date(2026, 7, 31),
        Some(unique("JE number")),
        Basis::Both,
        vec![
            line(supplies.id, Side::Debit(m("12.34"))),
            line(cash.id, Side::Credit(m("12.34"))),
        ],
        JeOptions {
            number: Some(next.clone()),
            ..JeOptions::default()
        },
        &AccountRoles::default(),
    )
    .expect("entry");
    let made = post(h, s, &entry);
    assert_eq!(made.record.txn_number.as_deref(), Some(next.as_str()));
    let after = h.run(s.journal().next_number()).expect("next");
    assert_eq!(after, (n + 1).to_string());
    // Duplicates are allowed (shapes-a): the same number posts again.
    let again = post(h, s, &entry);
    assert_eq!(again.record.txn_number.as_deref(), Some(next.as_str()));
    let too_long = raw_je(
        h,
        s,
        &json!({
            "txnType": "journal_entry", "txnDate": "2026-07-31", "basis": "both",
            "txnNumber": "1".repeat(21),
            "lines": [je_line(supplies.id, "1.0000", "0", None), je_line(cash.id, "0", "1.0000", None)],
        }),
    );
    assert_eq!(too_long.status, 400, "{}", too_long.body);
}

pub fn adjusting_round_trip(h: &Harness) {
    if !h.require_cap(Capabilities::JE_ADJUSTING, "U-60 je-adjusting-v1") {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let entry = NewJournalEntry::new_with(
        date(2026, 7, 31),
        Some(unique("AJE")),
        Basis::Both,
        vec![
            line(supplies.id, Side::Debit(m("7.00"))),
            line(cash.id, Side::Credit(m("7.00"))),
        ],
        JeOptions {
            adjusting: true,
            ..JeOptions::default()
        },
        &AccountRoles::default(),
    )
    .expect("entry");
    let made = post(h, s, &entry);
    assert!(made.record.is_adjusting);
    let only = h
        .run(s.raw(
            Method::Get,
            "/api/v1/transactions",
            &[("adjusting", "only"), ("accountId", &cash.id.to_string())],
            None,
        ))
        .expect("list");
    assert_eq!(only.status, 200, "{}", only.body);
    let ids: Vec<&str> = only.body["data"]
        .as_array()
        .map(|a| a.iter().filter_map(|r| r["id"].as_str()).collect())
        .unwrap_or_default();
    assert!(ids.contains(&made.record.id.to_string().as_str()));
    let excluded = h
        .run(s.raw(
            Method::Get,
            "/api/v1/transactions",
            &[
                ("adjusting", "exclude"),
                ("accountId", &cash.id.to_string()),
            ],
            None,
        ))
        .expect("list");
    assert!(
        !excluded
            .body
            .to_string()
            .contains(&made.record.id.to_string()),
        "excluded"
    );
}

pub fn reversing_creates_linked_entry_on_date(h: &Harness) {
    if !h.require_cap(Capabilities::JE_REVERSING, "U-61 je-reversing-v1") {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let on = date(2026, 7, 31);
    let back = date(2026, 8, 1);
    let mk = |reverse_on| {
        NewJournalEntry::new_with(
            on,
            Some(unique("Accrual")),
            Basis::Both,
            vec![
                line(supplies.id, Side::Debit(m("1234.5678"))),
                line(cash.id, Side::Credit(m("1234.5678"))),
            ],
            JeOptions {
                reverse_on,
                ..JeOptions::default()
            },
            &AccountRoles::default(),
        )
    };
    assert_eq!(mk(Some(on)), Err(cl_api::LocalValidation::ReverseNotAfter));
    let made = post(h, s, &mk(Some(back)).expect("entry"));
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(got.record.reverse_on, Some(back));
    let rev_id = got.record.reversal_transaction_id.expect("linked reversal");
    let rev = h.run(s.transactions().get(rev_id)).expect("reversal");
    assert_eq!(rev.record.txn_date, back);
    assert_eq!(rev.record.reverses_transaction_id, Some(made.record.id));
    let swapped: Vec<(String, String)> = wire_lines(&got.record)
        .into_iter()
        .map(|(d, c)| (c, d))
        .collect();
    assert_eq!(wire_lines(&rev.record), swapped, "lines swapped exactly");
    let raw = raw_je(
        h,
        s,
        &json!({
            "txnType": "journal_entry", "txnDate": "2026-07-31", "basis": "both",
            "reverseOn": "2026-07-31",
            "lines": [je_line(supplies.id, "1.0000", "0", None), je_line(cash.id, "0", "1.0000", None)],
        }),
    );
    assert_eq!(raw.status, 400, "{}", raw.body);
}

/// An edit always names Reverse on: the same date keeps the reversal, no
/// date (`null` on the wire) removes it (shapes-a "Reversal", three states).
pub fn editing_without_reverse_on_removes_the_reversal(h: &Harness) {
    if !h.require_cap(Capabilities::JE_REVERSING, "U-61 je-reversing-v1") {
        return;
    }
    let s = &h.s1;
    let (cash, supplies) = pair(h, s);
    let on = date(2026, 7, 31);
    let back = date(2026, 8, 1);
    let memo = unique("Accrual edit");
    let mk = |reverse_on| {
        NewJournalEntry::new_with(
            on,
            Some(memo.clone()),
            Basis::Both,
            vec![
                line(supplies.id, Side::Debit(m("55.00"))),
                line(cash.id, Side::Credit(m("55.00"))),
            ],
            JeOptions {
                reverse_on,
                ..JeOptions::default()
            },
            &AccountRoles::default(),
        )
        .expect("entry")
    };
    let made = post(h, s, &mk(Some(back)));
    let got = h.run(s.transactions().get(made.record.id)).expect("get");
    let rev_id = got.record.reversal_transaction_id.expect("linked reversal");
    // Same date sent explicitly: the reversal stays.
    h.run(s.transactions().update_journal_entry(
        made.record.id,
        &got.version,
        &mk(Some(back)),
        &PostingOptions::default(),
        key(),
    ))
    .expect("edit keeps");
    let reread = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(reread.record.reverse_on, Some(back));
    assert!(reread.record.reversal_transaction_id.is_some());
    // No Reverse on: `reverseOn: null` removes it.
    h.run(s.transactions().update_journal_entry(
        made.record.id,
        &reread.version,
        &mk(None),
        &PostingOptions::default(),
        key(),
    ))
    .expect("edit removes");
    let after = h.run(s.transactions().get(made.record.id)).expect("get");
    assert_eq!(after.record.reverse_on, None);
    assert_eq!(after.record.reversal_transaction_id, None);
    let gone = h.run(s.transactions().get(rev_id));
    assert!(
        gone.is_err() || gone.is_ok_and(|v| v.record.status != TxnStatus::Posted),
        "the reversal no longer posts"
    );
}
