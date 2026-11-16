//! P29 review B1 (Q-276, Q-180, Q-P29r): the tag gate on every Bank Feed
//! path that posts, checked with the tag each line **actually posts with**
//! on the engine (`engine/packages/api/src/services/bank-feed.service.ts`):
//!
//! - The owner's values (`PUT …/categorize`, accept `Values`): a split
//!   line's own tag, else the choice's tag, else the row's suggested tag
//!   (the desktop leaves `tagId` out when it has none, and the engine then
//!   falls back to `suggestedTagId`). The staged (assigned) tag is never
//!   used here.
//! - Approve (`Categorize(id, None)`) and "as proposed" on an assigned row:
//!   the staged tag only (no suggested fallback).
//! - "As proposed" on a row that isn't assigned: the suggested tag.
//! - A deposit's fee lines: no tag at all (the route takes none).
//! - Matches, transfers, duplicates, Review again and Undo post no new
//!   tagged line (transfers are outside the rule, Q-P29r).
//!
//! A rule's staged split isn't on the list row, so its lines are checked as
//! one line with the row's tag (the engine gives an untagged split line the
//! row's tag).

use std::collections::BTreeMap;

use cl_api::{AcceptItem, FeeLine, FeedChoice, FeedItemId, FeedRow, FeedStatus, RuleAccept};
use cl_core::ids::{AccountId, TagId};
use cl_model::forms::tag_rule::{Place, TagLine, check_form_tags, rules_on};
use cl_model::lists::accounts::AccountKind;
use cl_model::lists::tags::TagList;

use super::BankFeedScreen;
use super::send::Send;

fn line(place: Place, account: Option<AccountId>, tag: Option<TagId>) -> TagLine {
    TagLine {
        place,
        account,
        tags: tag.into_iter().collect(),
    }
}

fn suggested_tag(row: Option<&FeedRow>) -> Option<TagId> {
    row.and_then(|r| r.suggested_tag.as_ref().map(|t| t.id))
}

/// The owner's values: each line with the tag it posts with.
#[must_use]
pub(crate) fn values_lines(row: Option<&FeedRow>, c: &FeedChoice) -> Vec<TagLine> {
    let fallback = c.tag.or_else(|| suggested_tag(row));
    if c.splits.is_empty() {
        vec![line(Place::Entry, c.account, fallback)]
    } else {
        c.splits
            .iter()
            .enumerate()
            .map(|(i, l)| line(Place::Line(i), Some(l.account), l.tag.or(fallback)))
            .collect()
    }
}

/// Approve: what is staged on the row, with the staged tag only.
#[must_use]
pub(crate) fn staged_lines(row: Option<&FeedRow>) -> Vec<TagLine> {
    vec![line(
        Place::Entry,
        row.and_then(|r| r.assigned_account.as_ref().map(|a| a.id)),
        row.and_then(|r| r.assigned_tag.as_ref().map(|t| t.id)),
    )]
}

/// As proposed: an assigned row posts as staged; any other row posts its
/// suggestion with the suggested tag. A row not on the page counts as
/// untagged (nothing is known about it).
#[must_use]
pub(crate) fn proposed_lines(row: Option<&FeedRow>) -> Vec<TagLine> {
    match row {
        Some(r) if r.status == FeedStatus::Assigned && r.assigned_account.is_some() => {
            staged_lines(row)
        }
        Some(r) => vec![line(
            Place::Entry,
            r.suggested_account.as_ref().map(|a| a.id),
            suggested_tag(row),
        )],
        None => vec![line(Place::Entry, None, None)],
    }
}

/// One accept item's lines (a receipt's: what it posts; a match: none).
#[must_use]
pub(crate) fn item_lines(row: Option<&FeedRow>, item: &AcceptItem) -> Vec<TagLine> {
    match item.inner() {
        AcceptItem::AsProposed(_) => proposed_lines(row),
        AcceptItem::Values(_, c) => values_lines(row, c),
        AcceptItem::Match { .. } | AcceptItem::Receipt { .. } => Vec::new(),
    }
}

/// A deposit's fee lines: the route takes no tag for them.
#[must_use]
pub(crate) fn fee_lines(fees: &[FeeLine]) -> Vec<TagLine> {
    fees.iter()
        .enumerate()
        .map(|(i, f)| line(Place::Line(i), Some(f.account), None))
        .collect()
}

/// The company's rules as they are now and the chart's account types.
pub(crate) struct Gate<'a> {
    list: TagList,
    kinds: &'a BTreeMap<AccountId, AccountKind>,
}

impl Gate<'_> {
    /// True when `lines` would break a rule.
    #[must_use]
    pub(crate) fn refuses(&self, lines: &[TagLine]) -> bool {
        check_form_tags(&self.list, lines, |a| self.kinds.get(&a).copied()).is_err()
    }
}

/// The gate, or `None` when the company's rules can't refuse anything.
#[must_use]
pub(crate) fn gate(s: &BankFeedScreen) -> Option<Gate<'_>> {
    let list = crate::forms::tag_gate::form_list(&s.tags, &s.link);
    rules_on(&list).then_some(Gate {
        list,
        kinds: &s.kinds,
    })
}

/// Which i18n notice refuses `send`, if any (nothing is sent then).
#[must_use]
pub(crate) fn refusal(s: &BankFeedScreen, send: &Send) -> Option<&'static str> {
    let g = gate(s)?;
    let refused = match send {
        Send::Categorize(id, Some(c)) => g.refuses(&values_lines(s.row(*id), c)),
        Send::Categorize(id, None) => g.refuses(&staged_lines(s.row(*id))),
        Send::Deposit(_, _, fees) if !fees.is_empty() => {
            return g
                .refuses(&fee_lines(fees))
                .then_some("bank_feed.notice.fee_tag");
        }
        Send::Accept(items) => items
            .iter()
            .any(|i| g.refuses(&item_lines(s.row(i.id()), i))),
        Send::Deposit(..)
        | Send::Match(..)
        | Send::Transfer(..)
        | Send::Duplicate(..)
        | Send::ReviewAgain(_)
        | Send::Undo(..)
        | Send::Rule(_) => false,
    };
    refused.then_some("bank_feed.notice.tag_needed")
}

/// "Accept all from this rule" (the engine picks the rows, some of them
/// maybe not on this page): refused unless the rule itself sets a tag the
/// rules accept (the engine then takes only rows staged with it) and every
/// listed row of the rule posts with one. `Some` holds the listed rows that
/// don't (empty when only the rule is at fault).
#[must_use]
pub(crate) fn rule_refused(s: &BankFeedScreen, r: &RuleAccept) -> Option<Vec<FeedItemId>> {
    let g = gate(s)?;
    let rows: Vec<FeedItemId> = s
        .rows
        .iter()
        .filter(|x| {
            x.status.is_waiting() && x.suggested_rule.as_ref().is_some_and(|y| y.id == r.rule_id)
        })
        .map(|x| x.id)
        .collect();
    let left: Vec<FeedItemId> = rows
        .into_iter()
        .filter(|id| g.refuses(&proposed_lines(s.row(*id))))
        .collect();
    let rule_ok = s
        .rules
        .rules
        .iter()
        .find(|x| x.id.as_deref() == Some(r.rule_id.as_str()))
        .is_some_and(|x| !g.refuses(&[line(Place::Entry, x.account, x.tag)]));
    (!rule_ok || !left.is_empty()).then_some(left)
}

/// Splits rows accepted as proposed into those that pass and those the
/// rules refuse (in order).
#[must_use]
pub(crate) fn split_proposed(
    s: &BankFeedScreen,
    ids: &[FeedItemId],
) -> (Vec<FeedItemId>, Vec<FeedItemId>) {
    let Some(g) = gate(s) else {
        return (ids.to_vec(), Vec::new());
    };
    ids.iter()
        .partition(|id| !g.refuses(&proposed_lines(s.row(**id))))
}

/// "Date · description" for each row the rules left out (the screen names
/// them under its notice).
#[must_use]
pub(crate) fn names(s: &BankFeedScreen, ids: &[FeedItemId]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| s.row(*id))
        .map(|r| format!("{} \u{b7} {}", crate::shell::us_date(r.date), r.description))
        .collect()
}

#[cfg(test)]
#[path = "tag_check_tests.rs"]
mod tests;
