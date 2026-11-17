use std::collections::{BTreeMap, BTreeSet};

use cl_core::Money;
use cl_core::ids::{AccountId, ItemId, TagGroupId};
use proptest::prelude::*;

use super::*;
use crate::forms::item_lines::ItemLine;
use crate::forms::lines::FormLine;
use crate::lists::tags::{TagEntry, TagRules};
use crate::rowkey::RowKeyGen;

fn tid(n: u32) -> TagId {
    TagId::parse(&format!("70000000-0000-4000-8000-{n:012}")).expect("id")
}

fn gid(n: u32) -> TagGroupId {
    TagGroupId::parse(&format!("60000000-0000-4000-8000-{n:012}")).expect("id")
}

fn aid(n: u32) -> AccountId {
    AccountId::parse(&format!("50000000-0000-4000-8000-{n:012}")).expect("id")
}

fn tag(n: u32, group: Option<u32>, active: bool) -> TagEntry {
    TagEntry {
        id: tid(n),
        number: None,
        name: format!("Tag {n}"),
        group: group.map(gid),
        color: None,
        description: None,
        active,
        usage: None,
    }
}

/// Tag 1 in group 1, tag 2 ungrouped, tag 3 inactive.
fn list(every_line: bool, default_tag: Option<TagId>, required: bool) -> TagList {
    let mut req = BTreeMap::new();
    if required {
        req.insert(gid(1), BTreeSet::from([AccountKind::Expense]));
    }
    TagList::for_forms(
        vec![
            tag(1, Some(1), true),
            tag(2, None, true),
            tag(3, None, false),
        ],
        TagRules {
            every_line,
            default_tag,
            required: req,
        },
    )
}

#[allow(clippy::unnecessary_wraps)] // the gate's kind_of shape
fn expense(_: AccountId) -> Option<AccountKind> {
    Some(AccountKind::Expense)
}

fn line(keys: &mut RowKeyGen, tag: Option<TagId>) -> FormLine {
    FormLine {
        account: Some(aid(1)),
        amount: Some(Money::from_cents(100).expect("money")),
        tag,
        ..FormLine::blank(keys.next_key())
    }
}

/// Two used Expenses lines (the second without a tag), a blank entry line,
/// and one item line without a tag.
fn check_lines(keys: &mut RowKeyGen) -> (FormLines, ItemLines) {
    let lines = FormLines {
        lines: vec![
            line(keys, Some(tid(2))),
            line(keys, None),
            FormLine::blank(keys.next_key()),
        ],
    };
    let mut item = ItemLine::blank(keys.next_key());
    item.item = ItemId::parse("40000000-0000-4000-8000-000000000001").ok();
    item.account = Some(aid(2));
    let items = ItemLines { lines: vec![item] };
    (lines, items)
}

#[test]
fn every_line_names_the_untagged_lines_by_tab() {
    let mut keys = RowKeyGen::new();
    let (lines, items) = check_lines(&mut keys);
    let on = list(true, None, false);
    let r = check_form_tags(&on, &tabbed_lines(&[], &lines, &items, &on), expense)
        .expect_err("refused");
    assert_eq!(
        r.lines,
        vec![
            (Place::Expenses(1), LineTagProblem::Missing),
            (Place::Items(0), LineTagProblem::Missing),
        ],
        "Expenses tab, line 2 and Items tab, line 1"
    );
    // Off: it saves.
    let off = list(false, None, false);
    assert_eq!(
        check_form_tags(&off, &tabbed_lines(&[], &lines, &items, &off), expense),
        Ok(())
    );
    // A header tag fills every line (Q-178).
    assert_eq!(
        check_form_tags(&on, &tabbed_lines(&[tid(2)], &lines, &items, &on), expense),
        Ok(())
    );
    // Without tabs the lines are plain lines.
    let plain = expense_lines(&[], &lines, &on, false);
    let r = check_form_tags(&on, &plain, expense).expect_err("refused");
    assert_eq!(r.lines, vec![(Place::Line(1), LineTagProblem::Missing)]);
}

#[test]
fn required_groups_are_enforced_now_and_only_on_their_account_types() {
    let mut keys = RowKeyGen::new();
    let (lines, _) = check_lines(&mut keys);
    let req = list(false, None, true);
    let r = check_form_tags(&req, &expense_lines(&[], &lines, &req, true), expense)
        .expect_err("required");
    assert_eq!(
        r.lines,
        vec![
            (Place::Expenses(0), LineTagProblem::RequiredMissing(gid(1))),
            (Place::Expenses(1), LineTagProblem::RequiredMissing(gid(1))),
        ]
    );
    // An income account isn't covered.
    let income = |_: AccountId| Some(AccountKind::Income);
    assert_eq!(
        check_form_tags(&req, &expense_lines(&[], &lines, &req, true), income),
        Ok(())
    );
    // The group's tag in the header satisfies both lines... except the
    // first, whose own tag is what it is saved with (one tag per line).
    let r = check_form_tags(&req, &expense_lines(&[tid(1)], &lines, &req, true), expense)
        .expect_err("line 1 keeps its own tag");
    assert_eq!(
        r.lines,
        vec![(Place::Expenses(0), LineTagProblem::RequiredMissing(gid(1)))]
    );
}

#[test]
fn an_inactive_tag_already_on_a_line_does_not_block_and_counts_as_a_tag() {
    let on = list(true, None, false);
    let l = [TagLine {
        place: Place::Line(0),
        account: None,
        tags: vec![tid(3)],
    }];
    assert_eq!(check_form_tags(&on, &l, expense), Ok(()));
}

#[test]
fn the_default_tag_fills_new_forms_only_when_active() {
    assert_eq!(
        new_form_header_tags(&list(false, Some(tid(2)), false)),
        vec![tid(2)]
    );
    assert_eq!(
        new_form_header_tags(&list(false, Some(tid(3)), false)),
        Vec::<TagId>::new()
    );
    assert_eq!(
        new_form_header_tags(&list(false, Some(tid(9)), false)),
        Vec::<TagId>::new()
    );
    assert_eq!(
        new_form_header_tags(&list(false, None, false)),
        Vec::<TagId>::new()
    );
    assert!(!rules_on(&list(false, Some(tid(2)), false)));
    assert!(rules_on(&list(true, None, false)));
    assert!(rules_on(&list(false, None, true)));
}

#[test]
fn every_posting_form_says_whether_it_is_checked() {
    use PostingForm as P;
    let checked: Vec<P> = P::ALL.into_iter().filter(|f| f.checks_tags()).collect();
    assert_eq!(
        checked,
        [
            P::Check,
            P::Bill,
            P::CardCharge,
            P::PersonalFunds,
            P::JournalEntry,
            P::Deposit,
            P::Invoice,
            P::DailySales,
            P::LoanPayment,
            P::RegisterEntry,
            P::BulkEntry,
            P::BankFeedAccept,
        ]
    );
    // No form gets a default tag without being checked.
    assert!(
        P::ALL
            .into_iter()
            .all(|f| !f.takes_default_tag() || f.checks_tags())
    );
    let unique: BTreeSet<String> = P::ALL.iter().map(|f| format!("{f:?}")).collect();
    assert_eq!(unique.len(), P::ALL.len());
}

#[test]
fn rules_from_prefs_drop_unknown_account_types() {
    let mut wire = BTreeMap::new();
    wire.insert(
        gid(1),
        BTreeSet::from(["expense".to_string(), "nope".to_string()]),
    );
    wire.insert(gid(2), BTreeSet::from(["nope".to_string()]));
    let r = TagRules::from_prefs(true, Some(tid(1)), &wire);
    assert!(r.every_line);
    assert_eq!(r.required.len(), 1);
    assert!(r.required[&gid(1)].contains(&AccountKind::Expense));
    // Forms keep the group of a required group's tag.
    let l = TagList::for_forms(vec![tag(1, Some(1), true)], r);
    assert_eq!(l.tag(tid(1)).and_then(|t| t.group), Some(gid(1)));
}

proptest! {
    /// With "every line" on and no required groups, the gate passes exactly
    /// when every saved line has a tag.
    #[test]
    fn passes_iff_every_effective_line_has_a_tag(
        header in prop::option::of(1u32..=2),
        own in prop::collection::vec(prop::option::of(1u32..=3), 0..8),
    ) {
        let on = list(true, None, false);
        let mut keys = RowKeyGen::new();
        let lines = FormLines {
            lines: own.iter().map(|t| line(&mut keys, t.map(tid))).collect(),
        };
        let header: Vec<TagId> = header.map(tid).into_iter().collect();
        let tagged = expense_lines(&header, &lines, &on, true);
        let all_tagged = tagged.iter().all(|l| !l.tags.is_empty());
        prop_assert_eq!(check_form_tags(&on, &tagged, expense).is_ok(), all_tagged);
        // And "every effective line has a tag" is "the line or the header
        // has one".
        prop_assert_eq!(all_tagged, header.len() == 1 || own.iter().all(Option::is_some));
    }
}
