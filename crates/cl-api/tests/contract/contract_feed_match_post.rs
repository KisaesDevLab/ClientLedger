//! The single-row feed routes that post or link against the live engine
//! (P26 E4 U-171, E2 U-166, E3 U-169; plan §6 "Deposit matching clears
//! both undeposited payments with one deposit", "Undo"). PENDING on
//! engines before the P26 pin bump.
//!
//! - `deposit_of_two_payments_and_a_fee`: two waiting customer payments
//!   less one fee line post one deposit; neither payment waits in Payments
//!   Clearing any more and Payments Clearing drops by both exactly; Undo
//!   deletes the deposit and both payments wait again.
//! - `match_then_undo_keeps_the_entry`: `PUT match` links a bank line to an
//!   entry already in the books; Undo only unlinks.
//! - `assign_approve_then_undo_restores_the_staging`: `PUT assign` stages
//!   an account, `POST approve` posts it; Undo deletes the entry and puts
//!   the row back to assigned (its state before the accept).
//!
//! Every test imports its own rows into its own fresh bank account, dated
//! in June 2027, clear of the other feed tests' dates.

use cl_api::{AccountType, Capabilities, FeeLine, FeedStatus, TxnStatus, UndoTarget};
use cl_core::ids::TransactionId;

use crate::common::{date, je2, key, m, post};
use crate::contract_feed_accept::{fresh, import, status_of, values};
use crate::contract_invoices::customer;
use crate::contract_receive_payment::{balance, clearing, invoice, receive_auto};
use crate::support::Harness;

fn gated(h: &Harness) -> bool {
    h.require_cap(Capabilities::FEED_UNDO, "U-169")
        && h.require_cap(Capabilities::BANK_IMPORT_BATCH, "U-06")
}

fn pending_ids(h: &Harness, s: &cl_api::Session) -> Vec<TransactionId> {
    h.run(s.payments().pending_deposits())
        .expect("pending")
        .items
        .iter()
        .map(|p| p.source)
        .collect()
}

/// Plan §6: two waiting payments (413.27 + 286.19) less a 12.50 fee are
/// one 686.96 bank deposit; Undo returns both payments to waiting.
pub fn deposit_of_two_payments_and_a_fee(h: &Harness) {
    if !gated(h)
        || !h.require_cap(Capabilities::FEED_MATCH_POST, "U-171")
        || !h.require_cap(Capabilities::DEPOSIT_GROUPING, "U-52")
        || !h.require_cap(Capabilities::RECEIVE_PAYMENT, "U-132")
        || !h.require_cap(Capabilities::IDEMPOTENCY_RECEIVE_PAYMENT, "U-132")
    {
        return;
    }
    let s = &h.s1;
    let uf = clearing(h, s);
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let income = fresh(h, s, AccountType::Revenue, None);
    let fees = fresh(h, s, AccountType::Expense, None);
    let held = balance(h, s, uf);
    let on = date(2027, 6, 10);
    let pay = |name: &str, amount: &str| {
        let c = customer(h, s, name);
        invoice(h, s, c, income, date(2027, 6, 1), amount);
        receive_auto(h, s, c, on, amount).id
    };
    let x = pay("Feed deposit A", "413.27");
    let y = pay("Feed deposit B", "286.19");
    let waiting = pending_ids(h, s);
    assert!(waiting.contains(&x) && waiting.contains(&y), "both wait");
    let both = m("699.46");
    assert_eq!(
        balance(h, s, uf),
        held.checked_add(both).expect("add"),
        "both are in Payments Clearing"
    );
    let rows = import(h, s, bank, &[(date(2027, 6, 12), "-686.96", "DEPOSIT")]);
    assert_eq!(rows.len(), 1);
    let id = rows[0].id;
    let fee = [FeeLine {
        account: fees,
        amount: m("12.50"),
        memo: Some("Processor fee".into()),
    }];
    let k = key();
    let posted = h
        .run(s.feed().deposit(id, &[x, y], &fee, None, k.clone()))
        .unwrap_or_else(|e| panic!("deposit: {e}"));
    let deposit = posted.transaction.expect("the deposit");
    let mut paid = posted.payments.clone();
    paid.sort();
    let mut want = vec![(x, m("413.27")), (y, m("286.19"))];
    want.sort();
    assert_eq!(paid, want, "the payments at their exact amounts");
    assert_eq!(posted.items.len(), 1);
    let again = h
        .run(s.feed().deposit(id, &[x, y], &fee, None, k))
        .expect("replay");
    assert_eq!(again.transaction, posted.transaction, "a replay posts once");
    assert_eq!(status_of(h, s, id), Some(FeedStatus::Matched));
    let waiting = pending_ids(h, s);
    assert!(
        !waiting.contains(&x) && !waiting.contains(&y),
        "neither waits any more"
    );
    assert_eq!(
        balance(h, s, uf),
        held,
        "Payments Clearing is clear of both"
    );

    let g = posted.undo_group.expect("an accept group");
    let u = h
        .run(s.feed().undo(&UndoTarget::Group(g), false, None, key()))
        .expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert_eq!(u.undone[0].transaction, deposit);
    assert!(u.undone[0].deleted, "the deposit is deleted");
    assert!(status_of(h, s, id).is_some_and(FeedStatus::is_waiting));
    let waiting = pending_ids(h, s);
    assert!(
        waiting.contains(&x) && waiting.contains(&y),
        "both wait again"
    );
    assert_eq!(balance(h, s, uf), held.checked_add(both).expect("add"));
    // Leave nothing waiting for later tests.
    let rows2 = import(h, s, bank, &[(date(2027, 6, 13), "-699.46", "DEPOSIT 2")]);
    h.run(s.feed().deposit(rows2[0].id, &[x, y], &[], None, key()))
        .unwrap_or_else(|e| panic!("deposit again: {e}"));
}

/// `PUT match` links a bank line to an entry in the books (nothing posts);
/// Undo unlinks it and keeps the entry.
pub fn match_then_undo_keeps_the_entry(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::FEED_MATCH_V2, "U-166") {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let t = post(
        h,
        s,
        &je2(date(2027, 6, 20), "Feed match", exp, bank, "45.00"),
    )
    .record
    .id;
    let rows = import(h, s, bank, &[(date(2027, 6, 21), "45.00", "SQ MATCH")]);
    let id = rows[0].id;
    let c = h
        .run(s.feed().match_candidates(id))
        .expect("candidates")
        .into_iter()
        .find(|c| c.transaction == t)
        .unwrap_or_else(|| panic!("the entry {t} is a candidate"));
    let g = h
        .run(s.feed().match_line(id, t, Some(c.line), key()))
        .unwrap_or_else(|e| panic!("match: {e}"))
        .expect("an accept group");
    assert_eq!(status_of(h, s, id), Some(FeedStatus::Matched));
    let u = h
        .run(s.feed().undo(&UndoTarget::Group(g), false, None, key()))
        .expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert!(!u.undone[0].deleted, "a match only unlinks");
    assert!(status_of(h, s, id).is_some_and(FeedStatus::is_waiting));
    let kept = h.run(s.transactions().get(t)).expect("still there").record;
    assert_eq!(kept.status, TxnStatus::Posted);
}

/// `PUT assign` stages an account; `POST approve` posts it (a replay
/// answers the same entry); Undo deletes it and the row is assigned again.
pub fn assign_approve_then_undo_restores_the_staging(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::FEED_EXACT, "U-164") {
        return;
    }
    let s = &h.s1;
    let bank = fresh(h, s, AccountType::Asset, Some("bank"));
    let exp = fresh(h, s, AccountType::Expense, None);
    let rows = import(h, s, bank, &[(date(2027, 6, 25), "19.99", "ASSIGN ME")]);
    let id = rows[0].id;
    h.run(s.feed().assign(id, &values(exp)))
        .unwrap_or_else(|e| panic!("assign: {e}"));
    assert_eq!(status_of(h, s, id), Some(FeedStatus::Assigned));
    let k = key();
    let t = h
        .run(s.feed().approve(id, None, k.clone()))
        .unwrap_or_else(|e| panic!("approve: {e}"))
        .expect("entry");
    let again = h.run(s.feed().approve(id, None, k)).expect("replay");
    assert_eq!(again, Some(t));
    assert_eq!(status_of(h, s, id), Some(FeedStatus::Categorized));
    let entry = h.run(s.transactions().get(t)).expect("entry").record;
    assert_eq!(entry.status, TxnStatus::Posted);
    let u = h
        .run(
            s.feed()
                .undo(&UndoTarget::Items(vec![id]), false, None, key()),
        )
        .expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert!(u.undone[0].deleted);
    assert_eq!(
        status_of(h, s, id),
        Some(FeedStatus::Assigned),
        "back to its state before the accept"
    );
}
