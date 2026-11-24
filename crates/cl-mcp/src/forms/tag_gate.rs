//! The tag gate over the engine's lists (P29 WP7, Q-276, Q-180): the forms'
//! tag list built in one place with the company's rules, and the gate run
//! against the chart the form read. The rule itself is
//! `cl_model::forms::tag_rule`; the refusal's words are the desktop's.

use cl_api::{Account, Tag};
use cl_core::ids::AccountId;
use cl_model::forms::tag_rule::{TagLine, TagRefusal, check_form_tags};
use cl_model::lists::accounts::AccountKind;
use cl_model::lists::tags::{TagEntry, TagList, TagRules};

/// The tag as `cl_model`'s tag rules see it.
#[must_use]
pub fn entry(tag: &Tag) -> TagEntry {
    TagEntry {
        id: tag.id,
        number: tag
            .fields
            .tag_number
            .clone()
            .filter(|n| !n.trim().is_empty()),
        name: tag.fields.name.clone(),
        group: tag.fields.group_id,
        color: tag.fields.color.clone(),
        description: tag.fields.description.clone(),
        active: tag.is_active,
        usage: tag.usage_count,
    }
}

/// The tag list every form uses: the tags as read, with the company's tag
/// rules.
#[must_use]
pub fn form_list(tags: &[Tag], rules: TagRules) -> TagList {
    TagList::for_forms(tags.iter().map(entry).collect(), rules)
}

/// An account's type from the chart the form read.
#[must_use]
pub fn kind_in(accounts: &[Account], id: AccountId) -> Option<AccountKind> {
    accounts
        .iter()
        .find(|a| a.id == id)
        .map(|a| super::accounts::kind_of(a.fields.account_type))
}

/// Runs the gate on a form's tag lines. Nothing is sent on `Err`.
///
/// # Errors
/// The refused lines, in form order.
pub fn check(list: &TagList, lines: &[TagLine], accounts: &[Account]) -> Result<(), TagRefusal> {
    check_form_tags(list, lines, |a| kind_in(accounts, a))
}
