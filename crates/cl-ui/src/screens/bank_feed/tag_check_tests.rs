//! P29 review B1: every Bank Feed posting path checks the tag each line
//! posts with (Q-276 every line, Q-180 required groups).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cl_api::{AcceptItem, FeeLine, FeedChoice, FeedSplitLine, FeedStatus, Named};
use cl_core::Money;
use cl_core::ids::TagId;
use cl_model::CommandId;
use cl_model::bank_feed::Segment;

use super::super::BankFeedScreen;
use super::super::send::{self, Send};
use super::*;
use crate::company::CompanyLink;
use crate::windows::Screen;

fn screen() -> (tempfile::TempDir, BankFeedScreen) {
    let dir = tempfile::tempdir().expect("dir");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = BankFeedScreen::new(link, None);
    (dir, s)
}

/// Turns "tag on every line" on and returns an active tag.
fn every_line(s: &BankFeedScreen) -> TagId {
    let mut d = s.link.demo_state().expect("demo");
    d.require_tag_every_line = true;
    d.tags_mut()
        .iter()
        .find(|t| t.is_active)
        .expect("an active tag")
        .id
}

fn named(id: TagId) -> Named<TagId> {
    Named {
        id,
        name: "Tag".into(),
    }
}

fn ready(s: &mut BankFeedScreen) {
    s.segment = Segment::Ready;
    s.reload();
    assert!(s.rows.len() > 2);
}

fn cmd(s: &mut BankFeedScreen, id: &'static str) {
    assert!(s.command(CommandId::new(id)), "{id}");
    s.act_on_keys();
    s.poll();
}

#[test]
fn off_nothing_is_refused() {
    let (_d, mut s) = screen();
    ready(&mut s);
    let id = s.rows[0].id;
    assert!(gate(&s).is_none());
    assert_eq!(
        refusal(&s, &Send::Accept(vec![AcceptItem::AsProposed(id)])),
        None
    );
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, 1, "off: an untagged row records");
}

/// A (as proposed through the one accept route): an untagged row is
/// refused and stays waiting; with its suggested tag it records.
#[test]
fn a_as_proposed_needs_the_suggested_tag() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, 0, "nothing recorded");
    assert_eq!(s.notice.map(|n| n.0), Some("bank_feed.notice.tag_needed"));
    s.rows[0].suggested_tag = Some(named(tag));
    cmd(&mut s, "feed.accept");
    assert_eq!(s.counts.accepted, 1, "the suggested tag posts");
}

/// The owner's values: the choice's tag or the suggested one counts; the
/// staged (assigned) tag doesn't (the engine never posts it here).
#[test]
fn owner_values_use_the_choice_or_the_suggested_tag() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    let r = s.rows[0].clone();
    let account = r.suggested_account.as_ref().map(|a| a.id);
    let plain = FeedChoice {
        account,
        ..FeedChoice::default()
    };
    s.rows[0].assigned_tag = Some(named(tag));
    let send = Send::Categorize(r.id, Some(plain.clone()));
    assert_eq!(refusal(&s, &send), Some("bank_feed.notice.tag_needed"));
    send::start(&mut s, send, None);
    assert_eq!(s.counts.accepted, 0);
    let tagged = FeedChoice {
        tag: Some(tag),
        ..plain.clone()
    };
    assert_eq!(refusal(&s, &Send::Categorize(r.id, Some(tagged))), None);
    s.rows[0].suggested_tag = Some(named(tag));
    assert_eq!(
        refusal(&s, &Send::Categorize(r.id, Some(plain))),
        None,
        "the engine falls back to the suggested tag"
    );
}

/// A split: a line without its own tag takes the choice's tag, else the
/// suggested one; otherwise it is refused.
#[test]
fn split_lines_fall_back_to_the_choice_tag() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    let r = s.rows[0].clone();
    let account = r.suggested_account.as_ref().map(|a| a.id).expect("account");
    let split = |t: Option<TagId>| FeedSplitLine {
        account,
        amount: Money::parse_wire("1.00").expect("m"),
        tag: t,
        memo: None,
    };
    let mut c = FeedChoice {
        account: Some(account),
        splits: vec![split(Some(tag)), split(None)],
        ..FeedChoice::default()
    };
    let lines = values_lines(s.row(r.id), &c);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].tags, Vec::<TagId>::new());
    assert!(refusal(&s, &Send::Categorize(r.id, Some(c.clone()))).is_some());
    c.tag = Some(tag);
    assert!(refusal(&s, &Send::Categorize(r.id, Some(c))).is_none());
}

/// Approve and "as proposed" on an assigned row post the staged tag only.
#[test]
fn approve_posts_the_staged_tag_only() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    let id = s.rows[0].id;
    s.rows[0].status = FeedStatus::Assigned;
    s.rows[0].assigned_account = s.rows[0].suggested_account.clone();
    s.rows[0].suggested_tag = Some(named(tag));
    let approve = Send::Categorize(id, None);
    let proposed = Send::Accept(vec![AcceptItem::AsProposed(id)]);
    assert!(refusal(&s, &approve).is_some(), "no suggested fallback");
    assert!(refusal(&s, &proposed).is_some());
    s.rows[0].assigned_tag = Some(named(tag));
    assert!(refusal(&s, &approve).is_none());
    assert!(refusal(&s, &proposed).is_none());
}

/// A receipt's accept is checked on what it posts; a match posts nothing.
#[test]
fn receipts_are_checked_on_what_they_post() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    let id = s.rows[0].id;
    let account = s.rows[0].suggested_account.as_ref().map(|a| a.id);
    let draft = cl_api::DraftLink {
        id: "d0000000-0000-4000-8000-dd0000000001".parse().expect("d"),
        expected_updated_at: "t".into(),
        acknowledged_duplicates: Vec::new(),
    };
    let with = |item: AcceptItem| {
        Send::Accept(vec![AcceptItem::Receipt {
            item: Box::new(item),
            draft: draft.clone(),
        }])
    };
    let values = |t: Option<TagId>| {
        AcceptItem::Values(
            id,
            FeedChoice {
                account,
                tag: t,
                ..FeedChoice::default()
            },
        )
    };
    assert!(refusal(&s, &with(values(None))).is_some());
    assert!(refusal(&s, &with(values(Some(tag)))).is_none());
    assert!(refusal(&s, &with(AcceptItem::AsProposed(id))).is_some());
    let m = AcceptItem::Match {
        id,
        transaction: "33333333-3333-4333-8333-333333333333".parse().expect("t"),
        line: None,
    };
    assert!(refusal(&s, &with(m)).is_none(), "a match posts no line");
}

/// A deposit with fee lines can't carry a tag: refused with its own words;
/// one without fees isn't checked.
#[test]
fn deposits_with_fees_are_refused_while_tags_are_required() {
    let (_d, mut s) = screen();
    every_line(&s);
    ready(&mut s);
    let id = s.rows[0].id;
    let account = s.rows[0]
        .suggested_account
        .as_ref()
        .map(|a| a.id)
        .expect("a");
    let fee = FeeLine {
        account,
        amount: Money::parse_wire("2.50").expect("m"),
        memo: None,
    };
    assert_eq!(
        refusal(&s, &Send::Deposit(id, Vec::new(), vec![fee])),
        Some("bank_feed.notice.fee_tag")
    );
    assert_eq!(
        refusal(&s, &Send::Deposit(id, Vec::new(), Vec::new())),
        None
    );
    assert!(crate::i18n::has("bank_feed.notice.fee_tag"));
}

/// Accept selected: untagged rows stay waiting and are named; the tagged
/// ones record.
#[test]
fn accept_selected_skips_and_names_untagged_rows() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    ready(&mut s);
    s.rows[0].suggested_tag = Some(named(tag));
    cmd(&mut s, "feed.select");
    cmd(&mut s, "feed.next");
    cmd(&mut s, "feed.select");
    cmd(&mut s, "feed.accept_selected");
    assert_eq!(s.counts.accepted, 1, "only the tagged row");
    assert_eq!(s.tag_skipped.len(), 1);
    assert_eq!(s.state()["tag_skipped"], 1);
    assert!(
        s.tag_skipped[0].contains(" \u{b7} "),
        "{}",
        s.tag_skipped[0]
    );
}

/// Accept all from a rule goes whole or not at all (the engine picks the
/// rows): refused while a listed row of the rule has no tag or the rule
/// sets none; with both, it goes as before.
#[test]
fn rule_accepts_need_a_tagging_rule_and_tagged_rows() {
    let (_d, mut s) = screen();
    let tag = every_line(&s);
    s.reload();
    let rule = s
        .rows
        .iter()
        .find_map(|r| r.suggested_rule.clone())
        .expect("a rule row");
    let of_rule: Vec<_> = s
        .rows
        .iter()
        .filter(|r| {
            r.status.is_waiting() && r.suggested_rule.as_ref().is_some_and(|y| y.id == rule.id)
        })
        .map(|r| r.id)
        .collect();
    let accept = cl_api::RuleAccept {
        rule_id: rule.id.clone(),
        kind: rule.kind.clone(),
        updated_at: rule.updated_at.clone().unwrap_or_default(),
        connection: None,
        after: None,
    };
    send::start(&mut s, Send::Rule(accept.clone()), None);
    assert_eq!(s.counts.accepted, 0, "nothing recorded");
    assert_eq!(s.notice.map(|n| n.0), Some("bank_feed.notice.tag_rule"));
    assert_eq!(s.tag_skipped.len(), of_rule.len(), "every listed row named");
    for r in s.rows.iter_mut().filter(|r| of_rule.contains(&r.id)) {
        r.suggested_tag = Some(named(tag));
    }
    // Every listed row tagged, but the rule sets no tag: rows not on this
    // page could post untagged.
    s.rules
        .rules
        .retain(|x| x.id.as_deref() != Some(rule.id.as_str()));
    s.rules.rules.push(cl_api::FeedRule {
        id: Some(rule.id.clone()),
        ..cl_api::FeedRule::default()
    });
    assert_eq!(rule_refused(&s, &accept), Some(Vec::new()));
    if let Some(x) = s.rules.rules.last_mut() {
        x.tag = Some(tag);
    }
    assert_eq!(rule_refused(&s, &accept), None);
}
/// Required tag groups (Q-180) follow the same gate with every line off: an
/// expense line needs a tag of the group; another group's tag doesn't do.
#[test]
fn required_groups_follow_the_same_gate() {
    let (_dir, mut s) = screen();
    ready(&mut s);
    let (in_group, other) = {
        let mut demo = s.link.demo_state().expect("demo");
        let tags = demo.tags_mut().clone();
        let first = tags
            .iter()
            .find(|t| t.is_active && t.fields.group_id.is_some())
            .expect("a grouped tag");
        let g = first.fields.group_id.expect("group");
        let second = tags
            .iter()
            .find(|t| t.is_active && t.fields.group_id != Some(g))
            .expect("another tag");
        let kinds: BTreeSet<String> = ["expense", "cost_of_goods_sold", "other_expense"]
            .into_iter()
            .map(str::to_string)
            .collect();
        demo.required_tag_groups = BTreeMap::from([(g, kinds)]);
        (first.id, second.id)
    };
    let r = s
        .rows
        .iter()
        .find(|r| {
            r.suggested_account.as_ref().is_some_and(|a| {
                matches!(
                    s.kinds.get(&a.id),
                    Some(cl_model::lists::accounts::AccountKind::Expense)
                )
            })
        })
        .cloned()
        .expect("an expense row");
    let choice = |t: TagId| {
        Send::Categorize(
            r.id,
            Some(FeedChoice {
                account: r.suggested_account.as_ref().map(|a| a.id),
                tag: Some(t),
                ..FeedChoice::default()
            }),
        )
    };
    assert!(gate(&s).is_some());
    assert!(refusal(&s, &choice(other)).is_some(), "another group's tag");
    assert!(refusal(&s, &choice(in_group)).is_none());
}
