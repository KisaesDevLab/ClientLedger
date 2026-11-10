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
    .filter(|r| r.suggested_account.is_some() && r.match_type.as_deref() != Some("transfer"))
    .collect();
    // Close the books through 11/01: rows of 11/01 are locked.
    s.companies[0].closed_through = CalendarDate::from_ymd(2026, 11, 1).ok();
    let locked: Vec<FeedItemId> = ready
        .iter()
        .filter(|r| r.date <= CalendarDate::from_ymd(2026, 11, 1).expect("d"))
        .map(|r| r.id)
        .collect();
    assert_ne!(locked.len(), 0);
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

#[test]
fn rules_edit_and_the_rows_follow() {
    let mut s = state();
    let list = rules(&mut s);
    let fuel = list
        .iter()
        .find(|r| r.name == "Riverbend Fuel")
        .cloned()
        .expect("fuel rule");
    assert!(fuel.auto_accept && fuel.simple);
    let off = cl_api::FeedRule {
        auto_accept: false,
        ..fuel.clone()
    };
    save_rule(&mut s, &off).expect("saved");
    let row = find(&mut s, "RIVERBEND FUEL");
    let staged = row.suggested_rule.expect("still staged");
    assert!(!staged.auto_accept);
    assert_ne!(staged.updated_at, fuel.updated_at, "a new change token");
    // The old token is refused (409 RULE_CHANGED), as on the engine.
    let e = accept(
        &mut s,
        &AcceptRequest::Rule(cl_api::RuleAccept {
            rule_id: fuel.id.clone().expect("id"),
            kind: "conditional".into(),
            updated_at: fuel.updated_at.clone().expect("t"),
            connection: None,
            after: None,
        }),
        None,
    )
    .expect_err("not auto-accept now");
    assert!(cl_api::feed_refusal(&e).is_some());
    let new = cl_api::FeedRule {
        id: None,
        name: "Tolls".into(),
        description: Some("TOLL".into()),
        ..off
    };
    save_rule(&mut s, &new).expect("created");
    assert_eq!(rules(&mut s).len(), list.len() + 1);
    delete_rule(&mut s, fuel.id.as_deref().expect("id")).expect("deleted");
    assert!(find(&mut s, "RIVERBEND FUEL").suggested_rule.is_none());
}

#[test]
fn five_same_choices_suggest_a_rule_and_undo_counts_against() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    categorize(&mut s, fuel.id, None, None).expect("posted");
    let key = s.feed_review.taught.get(&fuel.id).cloned().expect("taught");
    s.feed_review.learned.insert(key.clone(), (5, 0));
    // The fuel rule already covers it: no suggestion.
    assert_eq!(rule_suggestions(&mut s).len(), 0);
    let _ = rules(&mut s);
    delete_rule(&mut s, fuel.suggested_rule.expect("rule").id.as_str()).expect("deleted");
    let offered = rule_suggestions(&mut s);
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].times_confirmed, 5);
    // One Undo is an override: 1 in 6 is above 10%.
    undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect("undo");
    assert_eq!(rule_suggestions(&mut s).len(), 0);
    s.feed_review.learned.insert(key, (12, 1));
    let offered = rule_suggestions(&mut s);
    dismiss_suggestion(&mut s, &offered[0]);
    assert_eq!(rule_suggestions(&mut s).len(), 0);
}

#[test]
fn a_transfer_posts_one_entry_and_undo_deletes_it() {
    let mut s = state();
    let before = entries(&s);
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    assert_eq!(card.match_type.as_deref(), Some("transfer"));
    let to = card.suggested_account.as_ref().expect("card account").id;
    let p = transfer(&mut s, card.id, TransferTarget::Account(to), None).expect("transfer");
    assert_eq!(entries(&s), before + 1);
    let e = undo(
        &mut s,
        &UndoTarget::Group(p.undo_group.expect("g")),
        false,
        None,
    )
    .expect("undo");
    assert!(e.undone[0].deleted);
    assert_eq!(entries(&s), before);
    let own = crate::company::demo::chart()
        .into_iter()
        .find(|a| a.fields.account_number.as_deref() == Some("1010"))
        .expect("1010")
        .id;
    let same = transfer(&mut s, card.id, TransferTarget::Account(own), None);
    assert!(matches!(
        same.as_ref().map_err(cl_api::feed_refusal),
        Err(Some(cl_api::FeedRefusal::TransferMismatch { .. }))
    ));
}

/// P26 review M2: Undo of one side of a two-row transfer deletes the entry
/// and takes both rows back (the engine unlinks every row on the entry);
/// the other side's Undo is then a stale one, not "changed since".
#[test]
fn undo_of_one_side_of_a_transfer_restores_both_rows() {
    let mut s = state();
    let before = entries(&s);
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    let to = card.suggested_account.as_ref().expect("card account").id;
    // The card's own bank line: the other side, money in on the card.
    let mut other = s
        .feed_review
        .rows
        .iter()
        .find(|r| r.id == card.id)
        .cloned()
        .expect("row");
    other.id = super::super::id("d26f9000", 1);
    other.account = to;
    other.amount = card.amount.checked_neg().expect("neg");
    other.transfer = false;
    other.rule = None;
    s.feed_review.rows.push(other.clone());
    let p = transfer(&mut s, card.id, TransferTarget::OtherItem(other.id), None).expect("transfer");
    assert_eq!(p.items.len(), 2);
    assert_eq!(entries(&s), before + 1);
    let u = undo(&mut s, &UndoTarget::Items(vec![card.id]), false, None).expect("undo");
    let mut ids: Vec<FeedItemId> = u.undone.iter().map(|x| x.id).collect();
    ids.sort();
    let mut want = vec![card.id, other.id];
    want.sort();
    assert_eq!(ids, want, "both sides are reported");
    assert!(u.undone.iter().all(|x| x.deleted));
    assert_eq!(entries(&s), before);
    for id in [card.id, other.id] {
        let r = s.feed_review.rows.iter().find(|r| r.id == id).expect("row");
        assert!(r.waiting() && r.matched.is_none() && r.group.is_none());
    }
    let again = undo(&mut s, &UndoTarget::Items(vec![other.id]), false, None)
        .expect("a stale undo, not FEED_UNDO_CHANGED");
    assert_eq!(again.undone.len(), 0);
    assert_eq!(again.failures[0].code, AcceptFailure::NOT_ACCEPTED);
    // Undo of the group after a fresh transfer reports each row once.
    let p = transfer(&mut s, card.id, TransferTarget::OtherItem(other.id), None)
        .expect("transfer again");
    let u = undo(
        &mut s,
        &UndoTarget::Group(p.undo_group.expect("g")),
        false,
        None,
    )
    .expect("undo group");
    assert_eq!(u.undone.len(), 2);
    assert_eq!(entries(&s), before);
}

/// P26 review m7: Undo puts the suggestion back as it was (the engine's
/// prior state), not only the status.
#[test]
fn undo_restores_the_suggestion() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    categorize(&mut s, fuel.id, None, None).expect("posted");
    if let Ok(r) = s.feed_review.row_mut(fuel.id) {
        r.suggested = None;
        r.rule = None;
    }
    undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect("undo");
    let back = find(&mut s, "RIVERBEND FUEL");
    assert_eq!(back.suggested_account, fuel.suggested_account);
    assert_eq!(
        back.suggested_rule.map(|r| r.id),
        fuel.suggested_rule.map(|r| r.id)
    );
}

/// P26 review m7: the owner's payee and memo are on the posted entry.
#[test]
fn categorize_keeps_the_owners_payee_and_memo() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let contact = s.contacts_mut()[0].clone();
    let choice = FeedChoice {
        account: fuel.suggested_account.as_ref().map(|a| a.id),
        contact: Some(contact.id),
        memo: Some("Truck fuel".into()),
        ..FeedChoice::default()
    };
    let t = categorize(&mut s, fuel.id, Some(&choice), None)
        .expect("posted")
        .expect("entry");
    let tx = s
        .ledger
        .transactions()
        .into_iter()
        .find(|x| DemoLedger::id_of(x) == t)
        .expect("entry");
    assert_eq!(
        tx.contact.as_deref(),
        Some(contact.fields.display_name.as_str())
    );
    assert_eq!(tx.memo, "Truck fuel");
    let unknown = FeedChoice {
        contact: Some("d26c0000-0000-4000-8000-000000000999".parse().expect("id")),
        ..choice
    };
    let other = find(&mut s, "MONETT TIRE");
    assert!(matches!(
        categorize(&mut s, other.id, Some(&unknown), None),
        Err(ApiError::NotFound { .. })
    ));
}

/// P26 review m7: rule mode skips a row whose staging no longer matches the
/// rule (`staging_differs`), as the engine.
#[test]
fn rule_mode_skips_rows_whose_staging_differs() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let staged = fuel.suggested_rule.clone().expect("rule");
    let other_account = crate::company::demo::account_id(6050);
    assert_ne!(
        fuel.suggested_account.as_ref().map(|a| a.id),
        Some(other_account)
    );
    let mut rule = rules(&mut s)
        .into_iter()
        .find(|r| r.id.as_deref() == Some(staged.id.as_str()))
        .expect("rule");
    rule.account = Some(other_account);
    save_rule(&mut s, &rule).expect("saved");
    let token = find(&mut s, "RIVERBEND FUEL")
        .suggested_rule
        .and_then(|r| r.updated_at)
        .expect("token");
    let o = accept(
        &mut s,
        &AcceptRequest::Rule(RuleAccept {
            rule_id: staged.id.clone(),
            kind: staged.kind.clone(),
            updated_at: token,
            connection: None,
            after: None,
        }),
        None,
    )
    .expect("accept");
    assert_eq!(o.posted, 0);
    assert!(o.skipped.contains(&(fuel.id, AcceptSkip::StagingDiffers)));
}

/// Engine #238: an Undo that keeps the entry (a later match alone) takes
/// back only the rows asked for; one with the row that posted the entry
/// deletes it and takes back every row on it.
#[test]
fn undo_of_a_later_match_alone_keeps_the_entry_and_its_other_rows() {
    let mut s = state();
    let before = entries(&s);
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    let to = card.suggested_account.as_ref().expect("card account").id;
    let posted = transfer(&mut s, card.id, TransferTarget::Account(to), None).expect("transfer");
    let t = posted.transaction.expect("entry");
    // A later bank line matched to the transfer's other line.
    let link = |s: &mut DemoState, id: FeedItemId| {
        let r = s.feed_review.row_mut(id).expect("row");
        r.status = FeedStatus::Matched;
        r.matched = Some(t);
        r.posted = false;
        r.how = "match";
        r.group = Some(UndoGroupId::new_random());
    };
    let later = find(&mut s, "MONETT TIRE").id;
    link(&mut s, later);
    let u = undo(&mut s, &UndoTarget::Items(vec![later]), false, None).expect("undo");
    assert_eq!(u.undone.len(), 1);
    assert_eq!(u.undone[0].id, later);
    assert!(!u.undone[0].deleted, "a match alone keeps the entry");
    assert_eq!(entries(&s), before + 1);
    let own = s
        .feed_review
        .rows
        .iter()
        .find(|r| r.id == card.id)
        .expect("row");
    assert_eq!(own.matched, Some(t), "the transfer's own row stays");
    link(&mut s, later);
    let u = undo(
        &mut s,
        &UndoTarget::Items(vec![card.id, later]),
        false,
        None,
    )
    .expect("undo");
    assert_eq!(u.undone.len(), 2);
    assert!(u.undone.iter().all(|x| x.deleted));
    assert_eq!(entries(&s), before);
}

/// Engine #238: transfer and deposit refuse a side flagged as a possible
/// duplicate (409 `FEED_ITEM_POSSIBLE_DUPLICATE`).
#[test]
fn transfer_and_deposit_refuse_a_possible_duplicate() {
    let mut s = state();
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    let zelle = find(&mut s, "ZELLE FROM T ROCK");
    for id in [card.id, zelle.id] {
        s.feed_review.row_mut(id).expect("row").possible_duplicate = true;
    }
    let to = card.suggested_account.as_ref().expect("card account").id;
    let e = transfer(&mut s, card.id, TransferTarget::Account(to), None).expect_err("dup");
    assert!(matches!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::PossibleDuplicate { id: Some(i), .. }) if i == card.id
    ));
    let pay = deposit_payments(&mut s, zelle.id)[0].source;
    let e = deposit(&mut s, zelle.id, &[pay], &[], None).expect_err("dup");
    assert!(matches!(
        cl_api::feed_refusal(&e),
        Some(cl_api::FeedRefusal::PossibleDuplicate { .. })
    ));
}

/// U-177 parity: the other side of a transfer 5 days away is a transfer
/// counterpart and in Matches only with a window of 5 or more; the counts
/// follow the same window.
#[test]
fn the_transfer_window_moves_a_pair_into_matches() {
    let mut s = state();
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    let to = card.suggested_account.as_ref().expect("card account").id;
    let mut other = s
        .feed_review
        .rows
        .iter()
        .find(|r| r.id == card.id)
        .cloned()
        .expect("row");
    other.id = super::super::id("d26f9000", 7);
    other.account = to;
    other.amount = card.amount.checked_neg().expect("neg");
    other.date = card.date.add_days(5).expect("date");
    other.transfer = false;
    other.rule = None;
    s.feed_review.rows.push(other.clone());
    let read = |s: &mut DemoState, days: Option<u8>| {
        let p = list(
            s,
            &FeedQuery {
                waiting_only: true,
                transfer_window_days: days,
                limit: 500,
                ..FeedQuery::default()
            },
        )
        .expect("list");
        let r = p.rows.into_iter().find(|r| r.id == card.id).expect("row");
        let c = crate::company::demo::feed_review::counts_in(s, None, days);
        (r, c)
    };
    let (r3, c3) = read(&mut s, None);
    assert!(r3.transfer_candidate.is_none());
    assert_ne!(r3.bucket, Some(FeedBucket::Matches));
    let (r5, c5) = read(&mut s, Some(5));
    assert_eq!(
        r5.transfer_candidate.as_ref().map(|t| t.feed_item),
        Some(other.id)
    );
    assert_eq!(r5.bucket, Some(FeedBucket::Matches));
    assert_eq!(c5.matches, c3.matches + 2, "both sides");
    assert_eq!(c5.waiting, c3.waiting);
    let matches = list(
        &mut s,
        &FeedQuery {
            bucket: Some(FeedBucket::Matches),
            transfer_window_days: Some(5),
            limit: 500,
            ..FeedQuery::default()
        },
    )
    .expect("matches");
    assert_eq!(matches.total, c5.matches);
}

/// U-178 parity: accepted rows say what the accept was; Undo clears it.
#[test]
fn accepted_rows_carry_their_accept_kind() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let card = find(&mut s, "BUSINESS CREDIT CARD");
    let to = card.suggested_account.as_ref().expect("card account").id;
    assert_eq!(fuel.accept_kind, None);
    categorize(&mut s, fuel.id, None, None).expect("posted");
    transfer(&mut s, card.id, TransferTarget::Account(to), None).expect("transfer");
    let accepted = list(
        &mut s,
        &FeedQuery {
            accepted: true,
            limit: 500,
            ..FeedQuery::default()
        },
    )
    .expect("accepted")
    .rows;
    let kind = |id| {
        accepted
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| r.accept_kind)
    };
    assert_eq!(kind(fuel.id), Some(cl_api::AcceptKind::Post));
    assert_eq!(kind(card.id), Some(cl_api::AcceptKind::Transfer));
    undo(&mut s, &UndoTarget::Items(vec![fuel.id]), false, None).expect("undo");
    assert_eq!(find(&mut s, "RIVERBEND FUEL").accept_kind, None);
}

/// Q-PR5a (PR-5 B1, U-199 parity): an amount edit of an entry accepted from
/// a bank line flags the line "Entry changed" (kept accepted, counted);
/// Review again puts it back to waiting.
#[test]
fn an_edited_entry_flags_its_bank_line_until_review_again() {
    let mut s = state();
    let fuel = find(&mut s, "RIVERBEND FUEL");
    let t = categorize(&mut s, fuel.id, None, None)
        .expect("posted")
        .expect("entry");
    assert_eq!(counts(&mut s, None).entry_changed, 0, "agrees as posted");
    let posted = s
        .ledger
        .transactions()
        .into_iter()
        .find(|x| DemoLedger::id_of(x) == t)
        .expect("in the ledger");
    let other: u32 = posted
        .lines
        .iter()
        .find(|l| l.account != "1010")
        .and_then(|l| l.account.parse().ok())
        .expect("the expense line");
    let edited = DemoPosting {
        kind: "check",
        date: posted.date,
        number: String::new(),
        contact: None,
        memo: String::new(),
        own: account(1010),
        own_debit: false,
        lines: vec![(
            account(other),
            fuel.amount.abs().checked_add(m("9.00")).expect("sum"),
        )],
    };
    let chart = crate::company::demo::chart();
    assert_eq!(s.ledger.save(&chart, &edited, Some(t)), Some(t));
    let c = counts(&mut s, None);
    assert_eq!((c.accepted, c.entry_changed, c.entry_voided), (1, 1, 0));
    let changed = list(
        &mut s,
        &FeedQuery {
            entry_changed: Some(true),
            ..FeedQuery::default()
        },
    )
    .expect("list");
    assert_eq!(changed.rows.len(), 1);
    assert!(changed.rows[0].entry_changed && changed.rows[0].status.is_accepted());
    review_again(&mut s, fuel.id).expect("review again");
    let c = counts(&mut s, None);
    assert_eq!((c.entry_changed, c.waiting), (0, 37));
}
