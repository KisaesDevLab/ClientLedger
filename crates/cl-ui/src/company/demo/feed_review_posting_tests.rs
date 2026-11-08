use cl_api::{FeedBucket, FeedQuery, FeedSplitLine, RuleAccept};

use super::*;
use crate::company::demo::feed_review::{counts, list};
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("m")
}

fn state() -> DemoState {
    let mut s = DemoState::new();
    s.open = Some(s.companies[0].tenant_id);
    s
}

fn waiting(s: &mut DemoState) -> Vec<cl_api::FeedRow> {
    list(
        s,
        &FeedQuery {
            waiting_only: true,
            limit: 500,
            ..FeedQuery::default()
        },
    )
    .expect("list")
    .rows
}

fn find(s: &mut DemoState, text: &str) -> cl_api::FeedRow {
    waiting(s)
        .into_iter()
        .find(|r| r.description.contains(text))
        .unwrap_or_else(|| panic!("{text}"))
}

fn entries(s: &DemoState) -> usize {
    s.ledger.transactions().len()
}

fn account(n: u32) -> AccountId {
    crate::company::demo::account_id(n)
}

#[test]
fn categorize_posts_a_real_entry_and_undo_removes_it() {
    let mut s = state();
    let before = entries(&s);
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let t = categorize(&mut s, fuel.id, None, None)
        .expect("posted")
        .expect("entry");
    assert_eq!(entries(&s), before + 1, "one demo entry");
    let posted = s
        .ledger
        .transactions()
        .into_iter()
        .find(|x| DemoLedger::id_of(x) == t)
        .expect("in the ledger");
    // Money out of 1010 for exactly the bank amount.
    assert!(
        posted
            .lines
            .iter()
            .any(|l| l.account == "1010" && l.credit == m("463.83"))
    );
    let u = undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert!(u.undone[0].deleted);
    assert_eq!(entries(&s), before, "the entry is gone");
    assert_eq!(counts(&mut s, None).waiting, 37);
    // A stale Undo does nothing.
    let again = undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect("stale");
    assert_eq!(again.undone.len(), 0);
    assert_eq!(again.failures[0].code, AcceptFailure::NOT_ACCEPTED);
}

#[test]
fn a_split_must_total_exactly() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let line = |n: u32, v: &str| FeedSplitLine {
        account: account(n),
        amount: m(v),
        tag: None,
        memo: None,
    };
    let bad = FeedChoice {
        splits: vec![line(6100, "400.00"), line(6350, "63.82")],
        ..FeedChoice::default()
    };
    let e = categorize(&mut s, fuel.id, Some(&bad), None).expect_err("short");
    let Some(cl_api::FeedRefusal::SplitsDontBalance { difference }) = cl_api::feed_refusal(&e)
    else {
        panic!("{e:?}");
    };
    assert_eq!(difference, Some(m("0.01")));
    let good = FeedChoice {
        splits: vec![line(6100, "400.00"), line(6350, "63.83")],
        ..FeedChoice::default()
    };
    categorize(&mut s, fuel.id, Some(&good), None).expect("posted");
}

#[test]
fn two_waiting_payments_become_one_deposit_and_undo_returns_them() {
    let mut s = state();
    assert_eq!(add_next_file(&mut s), 1);
    let dep = waiting(&mut s)
        .into_iter()
        .find(|r| r.description == "DEPOSIT")
        .expect("the 11/04 deposit");
    assert!(
        dep.match_candidate_count > 0,
        "a deposit grouping is a match signal"
    );
    let groups = deposit_groupings(&mut s, dep.id);
    let ids: Vec<TransactionId> = groups[0].iter().map(|p| p.source).collect();
    assert_eq!(ids.len(), 2);
    let posted = deposit(&mut s, dep.id, &ids, &[], None).expect("deposit");
    assert_eq!(
        Money::sum(posted.payments.iter().map(|(_, a)| *a)).expect("sum"),
        m("2092.50")
    );
    assert_eq!(deposits::pending(&mut s).len(), 0);
    let u = undo(
        &mut s,
        &UndoTarget::Group(posted.undo_group.expect("group")),
        false,
        None,
    )
    .expect("undo");
    assert!(u.undone[0].deleted);
    assert_eq!(
        deposits::pending(&mut s).len(),
        2,
        "back in Payments Clearing"
    );
}

#[test]
fn deposit_with_a_fee_line_balances_exactly() {
    let mut s = state();
    let zelle = find(&mut s, "ZELLE FROM T ROCK");
    let pays = deposit_payments(&mut s, zelle.id);
    let big = pays
        .iter()
        .find(|p| p.amount == m("1250.00"))
        .expect("1,250.00 waiting")
        .source;
    let fee = |v: &str, n: u32| FeeLine {
        account: account(n),
        amount: m(v),
        memo: None,
    };
    let e = deposit(&mut s, zelle.id, &[big], &[fee("324.99", 6050)], None).expect_err("off");
    let Some(cl_api::FeedRefusal::ItemMismatch { difference, .. }) = cl_api::feed_refusal(&e)
    else {
        panic!("{e:?}");
    };
    assert_eq!(difference, Some(m("-0.01")));
    let e = deposit(&mut s, zelle.id, &[big], &[fee("325.00", 1010)], None).expect_err("bank");
    assert!(matches!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::FeeAccountInvalid { .. })
    ));
    let posted = deposit(&mut s, zelle.id, &[big], &[fee("325.00", 6050)], None).expect("ok");
    let t = posted.transaction.expect("deposit");
    let entry = s
        .ledger
        .transactions()
        .into_iter()
        .find(|x| DemoLedger::id_of(x) == t)
        .expect("entry");
    assert!(
        entry
            .lines
            .iter()
            .any(|l| l.account == "1010" && l.debit == m("925.00"))
    );
    assert!(
        entry
            .lines
            .iter()
            .any(|l| l.account == "6050" && l.debit == m("325.00"))
    );
    undo(&mut s, &UndoTarget::Items(vec![zelle.id]), false, None).expect("undo");
    let back = deposits::pending(&mut s);
    assert!(
        back.iter()
            .any(|p| p.source == big && p.amount == m("1250.00"))
    );
}

#[test]
fn a_match_links_and_undo_only_unlinks() {
    let mut s = state();
    let before = entries(&s);
    let fence = find(&mut s, "SYCAMORE FENCE");
    let c = match_candidates(&s, fence.id);
    assert_eq!(c.len(), 1, "the bill payment of 10/23");
    let g = match_line(&mut s, fence.id, c[0].transaction).expect("match");
    assert!(g.is_some());
    assert_eq!(entries(&s), before, "nothing posted");
    let u = undo(&mut s, &UndoTarget::Items(vec![fence.id]), false, None).expect("undo");
    assert!(!u.undone[0].deleted, "the entry is kept");
    assert_eq!(entries(&s), before);
}

#[test]
fn accept_selection_skips_and_locks_then_posts_with_the_password() {
    let mut s = state();
    let ready: Vec<cl_api::FeedRow> = list(
        &mut s,
        &FeedQuery {
            bucket: Some(FeedBucket::Ready),
            limit: 500,
            ..FeedQuery::default()
        },
    )
    .expect("ready")
    .rows
    .into_iter()
    .filter(|r| r.suggested_account.is_some())
    .collect();
    // Close the books through 11/01: rows of 11/01 are locked.
    s.companies[0].closed_through = CalendarDate::from_ymd(2026, 11, 1).ok();
    let locked: Vec<FeedItemId> = ready
        .iter()
        .filter(|r| r.date <= CalendarDate::from_ymd(2026, 11, 1).expect("d"))
        .map(|r| r.id)
        .collect();
    assert!(!locked.is_empty());
    let fence = find(&mut s, "SYCAMORE FENCE");
    let mut items: Vec<AcceptItem> = ready.iter().map(|r| AcceptItem::AsProposed(r.id)).collect();
    items.push(AcceptItem::AsProposed(fence.id));
    let o = accept(&mut s, &AcceptRequest::Items(items), None).expect("accept");
    assert_eq!(o.skipped, vec![(fence.id, AcceptSkip::HasMatch)]);
    let mut got = o.locked();
    got.sort();
    let mut want = locked.clone();
    want.sort();
    assert_eq!(got, want);
    assert_eq!(o.posted as usize, ready.len() - locked.len());
    let pw = Secret::new(DEMO_CLOSING_PASSWORD);
    let retry = AcceptRequest::Items(
        locked
            .iter()
            .map(|id| AcceptItem::AsProposed(*id))
            .collect(),
    );
    let o2 = accept(&mut s, &retry, Some(&pw)).expect("retry");
    assert_eq!(o2.posted as usize, locked.len());
    // Undo of the locked ones needs the password too.
    let u = undo(&mut s, &UndoTarget::Group(o2.undo_group), false, None).expect("undo");
    assert_eq!(u.locked().len(), locked.len());
    let u = undo(&mut s, &UndoTarget::Group(o2.undo_group), false, Some(&pw)).expect("undo");
    assert_eq!(u.undone.len(), locked.len());
}

#[test]
fn an_edited_entry_asks_before_undo() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let t = categorize(&mut s, fuel.id, None, None)
        .expect("posted")
        .expect("entry");
    let chart = crate::company::demo::chart();
    let edited = DemoPosting {
        kind: "expense",
        date: fuel.date,
        number: String::new(),
        contact: None,
        memo: "edited in the register".into(),
        own: account(1010),
        own_debit: false,
        lines: vec![(account(6100), m("463.83"))],
    };
    s.ledger.save(&chart, &edited, Some(t)).expect("edited");
    let e = undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect_err("changed");
    let Some(cl_api::FeedRefusal::UndoChanged { ids }) = cl_api::feed_refusal(&e) else {
        panic!("{e:?}");
    };
    assert_eq!(ids, vec![fuel.id]);
    let u = undo(&mut s, &UndoTarget::Items(vec![fuel.id]), true, None).expect("forced");
    assert!(u.undone[0].deleted);
}

#[test]
fn a_voided_entry_stays_accepted_until_review_again() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    categorize(&mut s, fuel.id, None, None).expect("posted");
    assert!(void_first_accepted(&mut s));
    let c = counts(&mut s, None);
    assert_eq!((c.accepted, c.entry_voided, c.waiting), (1, 1, 36));
    // Accept never touches it.
    let o = accept(
        &mut s,
        &AcceptRequest::Items(vec![AcceptItem::AsProposed(fuel.id)]),
        None,
    )
    .expect("accept");
    assert_eq!(o.skipped, vec![(fuel.id, AcceptSkip::NotPending)]);
    review_again(&mut s, fuel.id).expect("review again");
    assert_eq!(counts(&mut s, None).waiting, 37);
    let e = review_again(&mut s, fuel.id).expect_err("not voided");
    assert_eq!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::EntryNotVoided)
    );
}

#[test]
fn accept_all_from_an_auto_accept_rule() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let rule = fuel.suggested_rule.clone().expect("a rule staged it");
    assert!(rule.auto_accept);
    let req = |updated_at: &str| {
        AcceptRequest::Rule(RuleAccept {
            rule_id: rule.id.clone(),
            kind: rule.kind.clone(),
            updated_at: updated_at.into(),
            connection: None,
            after: None,
        })
    };
    let e = accept(&mut s, &req("2020-01-01T00:00:00.000Z"), None).expect_err("changed");
    assert!(matches!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::RuleChanged { .. })
    ));
    let o = accept(
        &mut s,
        &req(rule.updated_at.as_deref().expect("token")),
        None,
    )
    .expect("all");
    assert!(o.posted >= 1);
    assert_eq!((o.remaining, o.next_after), (0, None));
    let shepherd = find(&mut s, "MONETT TIRE");
    if let Some(r) = shepherd.suggested_rule.filter(|r| !r.auto_accept) {
        let e = accept(
            &mut s,
            &AcceptRequest::Rule(RuleAccept {
                rule_id: r.id,
                kind: r.kind,
                updated_at: r.updated_at.unwrap_or_default(),
                connection: None,
                after: None,
            }),
            None,
        )
        .expect_err("not auto-accept");
        assert_eq!(
            cl_api::feed_refusal(&e),
            Some(cl_api::FeedRefusal::RuleNotAutoAccept)
        );
    }
}

#[test]
fn duplicates_keep_both_or_remove_this_one() {
    let mut s = state();
    assert!(mark_duplicate(&mut s));
    let dup = waiting(&mut s)
        .into_iter()
        .find(|r| r.possible_duplicate)
        .expect("flagged");
    assert!(
        dup.duplicate_of.is_some(),
        "shown beside the row it resembles"
    );
    let o = accept(
        &mut s,
        &AcceptRequest::Items(vec![AcceptItem::AsProposed(dup.id)]),
        None,
    )
    .expect("accept");
    assert_eq!(o.skipped, vec![(dup.id, AcceptSkip::PossibleDuplicate)]);
    decide_duplicate(&mut s, dup.id, DuplicateDecision::Remove).expect("remove");
    assert_eq!(counts(&mut s, None).excluded, 1);
    let e = decide_duplicate(&mut s, dup.id, DuplicateDecision::Keep).expect_err("decided");
    assert_eq!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::NotAPossibleDuplicate)
    );
}
