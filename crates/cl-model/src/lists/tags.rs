//! Tag rules (P11 step 5): the order and filter of the Tags window, the tag
//! and group forms' checks, and the application rules the transaction
//! forms use (Q-178 to Q-181):
//!
//! - Q-178: tags sit on the transaction (header) and on lines; the header's
//!   tags are the default for every line ([`effective_line_tags`]).
//! - Q-179: at most one tag per one-per-group group on a line; ungrouped
//!   tags are unlimited ([`check_line_tags`]).
//! - Q-180: a group can be required for chosen account types; a line on
//!   such an account needs a tag of that group. The engine has no such
//!   rule yet (U-42): the forms enforce it from the company's local
//!   preferences.
//! - Q-181: rules and memorized transactions can set tags; their tags
//!   replace the line's tags of the same one-per-group group and add to the
//!   rest ([`apply_rule_tags`]).
//!
//! The engine keeps one tag per journal line today (`journal_lines.tag_id`,
//! U-45): [`engine_line_tag`] refuses a line with more than one tag until
//! the capability arrives (Q-P10t).

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use cl_core::ids::{TagGroupId, TagId};

use crate::lists::accounts::AccountKind;
use crate::lists::engine_len;
use crate::text::TextKey;

/// Longest tag or group name the engine takes.
pub const NAME_MAX: usize = 100;
/// Longest description kept.
pub const DESCRIPTION_MAX: usize = 2000;

/// One tag group as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    /// Id.
    pub id: TagGroupId,
    /// Name.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// One tag of this group per line (Q-179).
    pub single: bool,
    /// Account types whose lines need a tag of this group (Q-180).
    pub required_for: BTreeSet<AccountKind>,
    /// The engine's position.
    pub order: i64,
}

/// One tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    /// Id.
    pub id: TagId,
    /// Name.
    pub name: String,
    /// Group.
    pub group: Option<TagGroupId>,
    /// `#rrggbb`.
    pub color: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Active.
    pub active: bool,
    /// The engine's usage count.
    pub usage: Option<i64>,
}

/// What the Tags window shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagFilter {
    /// Also list inactive tags.
    pub show_inactive: bool,
    /// Tag or group name contains (case-insensitive).
    pub search: String,
}

/// The groups and tags, arranged: groups in the engine's order (then by
/// name), each group's tags by name, ungrouped tags last.
#[derive(Debug, Clone, Default)]
pub struct TagList {
    groups: Vec<GroupEntry>,
    tags: Vec<TagEntry>,
}

fn by_name(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

impl TagList {
    /// The list of `groups` and `tags`. A tag whose group is gone counts as
    /// ungrouped.
    #[must_use]
    pub fn new(mut groups: Vec<GroupEntry>, mut tags: Vec<TagEntry>) -> TagList {
        groups.sort_by(|a, b| {
            a.order
                .cmp(&b.order)
                .then_with(|| by_name(&a.name, &b.name))
        });
        let pos: BTreeMap<TagGroupId, usize> =
            groups.iter().enumerate().map(|(i, g)| (g.id, i)).collect();
        for t in &mut tags {
            if t.group.is_some_and(|g| !pos.contains_key(&g)) {
                t.group = None;
            }
        }
        let rank = |t: &TagEntry| {
            t.group
                .and_then(|g| pos.get(&g).copied())
                .unwrap_or(usize::MAX)
        };
        tags.sort_by(|a, b| {
            rank(a)
                .cmp(&rank(b))
                .then_with(|| by_name(&a.name, &b.name))
        });
        TagList { groups, tags }
    }

    /// Groups, in order.
    #[must_use]
    pub fn groups(&self) -> &[GroupEntry] {
        &self.groups
    }

    /// Tags, in order.
    #[must_use]
    pub fn tags(&self) -> &[TagEntry] {
        &self.tags
    }

    /// Number of tags.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tags.len()
    }

    /// No tags.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
    }

    /// The tag `id`.
    #[must_use]
    pub fn tag(&self, id: TagId) -> Option<&TagEntry> {
        self.tags.iter().find(|t| t.id == id)
    }

    /// The group `id`.
    #[must_use]
    pub fn group(&self, id: TagGroupId) -> Option<&GroupEntry> {
        self.groups.iter().find(|g| g.id == id)
    }

    /// The group of tag `id`.
    #[must_use]
    pub fn group_of(&self, id: TagId) -> Option<&GroupEntry> {
        self.tag(id)
            .and_then(|t| t.group)
            .and_then(|g| self.group(g))
    }

    /// Indexes of the tags shown under `filter` (a search matches the tag's
    /// name or its group's name).
    #[must_use]
    pub fn visible(&self, filter: &TagFilter) -> Vec<usize> {
        let q = filter.search.trim().to_lowercase();
        self.tags
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                (t.active || filter.show_inactive)
                    && (q.is_empty()
                        || t.name.to_lowercase().contains(&q)
                        || t.group
                            .and_then(|g| self.group(g))
                            .is_some_and(|g| g.name.to_lowercase().contains(&q)))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Another tag already called `name` (any case; inactive tags keep
    /// their names, as in the engine).
    #[must_use]
    pub fn tag_name_owner(&self, name: &str, except: Option<TagId>) -> Option<TagId> {
        let want = name.trim().to_lowercase();
        (!want.is_empty())
            .then(|| {
                self.tags
                    .iter()
                    .find(|t| Some(t.id) != except && t.name.trim().to_lowercase() == want)
                    .map(|t| t.id)
            })
            .flatten()
    }

    /// Another group already called `name` (any case).
    #[must_use]
    pub fn group_name_owner(&self, name: &str, except: Option<TagGroupId>) -> Option<TagGroupId> {
        let want = name.trim().to_lowercase();
        (!want.is_empty())
            .then(|| {
                self.groups
                    .iter()
                    .find(|g| Some(g.id) != except && g.name.trim().to_lowercase() == want)
                    .map(|g| g.id)
            })
            .flatten()
    }

    /// The tags `source` may be merged into: active, not `source`, and in
    /// `source`'s own group, ungrouped, or in a group that takes any number
    /// of tags per line. A tag of another one-per-line group is left out,
    /// since the merge could give a line two tags of that group (Q-179).
    /// A tag whose group is gone counts as ungrouped.
    #[must_use]
    pub fn merge_targets(&self, source: TagId) -> Vec<&TagEntry> {
        let own = self.group_of(source).map(|g| g.id);
        self.tags
            .iter()
            .filter(|t| t.id != source && t.active)
            .filter(|t| match t.group.and_then(|g| self.group(g)) {
                None => true,
                Some(g) => Some(g.id) == own || !g.single,
            })
            .collect()
    }
}

/// True for `#rrggbb`.
#[must_use]
pub fn is_color(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 7 && b[0] == b'#' && b[1..].iter().all(u8::is_ascii_hexdigit)
}

/// The tag form's values as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagDraft {
    /// Name.
    pub name: String,
    /// Group.
    pub group: Option<TagGroupId>,
    /// `#rrggbb`, or blank.
    pub color: String,
    /// Description.
    pub description: String,
}

impl TagDraft {
    /// The form over a loaded tag.
    #[must_use]
    pub fn from_entry(t: &TagEntry) -> TagDraft {
        TagDraft {
            name: t.name.clone(),
            group: t.group,
            color: t.color.clone().unwrap_or_default(),
            description: t.description.clone().unwrap_or_default(),
        }
    }
}

/// A tag draft that passed [`check_tag`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedTag {
    /// Name, trimmed.
    pub name: String,
    /// Group.
    pub group: Option<TagGroupId>,
    /// Color.
    pub color: Option<String>,
    /// Description.
    pub description: Option<String>,
}

/// The group form's values as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupDraft {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// One tag per line.
    pub single: bool,
    /// Account types that need a tag of this group.
    pub required_for: BTreeSet<AccountKind>,
}

impl GroupDraft {
    /// The form over a loaded group.
    #[must_use]
    pub fn from_entry(g: &GroupEntry) -> GroupDraft {
        GroupDraft {
            name: g.name.clone(),
            description: g.description.clone().unwrap_or_default(),
            single: g.single,
            required_for: g.required_for.clone(),
        }
    }
}

/// A group draft that passed [`check_group`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedGroup {
    /// Name, trimmed.
    pub name: String,
    /// Description.
    pub description: Option<String>,
    /// One tag per line.
    pub single: bool,
    /// Account types that need a tag of this group.
    pub required_for: BTreeSet<AccountKind>,
}

/// The form field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagField {
    /// Name (tag or group).
    Name,
    /// The tag's group.
    Group,
    /// Color.
    Color,
    /// Description.
    Description,
}

/// Why a tag or group draft cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagProblem {
    /// No name.
    NameMissing,
    /// Longer than the engine takes.
    NameTooLong,
    /// Another tag has this name.
    NameTaken,
    /// Another group has this name.
    GroupNameTaken,
    /// The chosen group is gone.
    GroupGone,
    /// Not `#rrggbb`.
    ColorInvalid,
    /// Longer than kept.
    DescriptionTooLong,
}

impl TagProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            TagProblem::NameMissing => "tags.problem.name_missing",
            TagProblem::NameTooLong => "tags.problem.name_too_long",
            TagProblem::NameTaken => "tags.problem.name_taken",
            TagProblem::GroupNameTaken => "tags.problem.group_name_taken",
            TagProblem::GroupGone => "tags.problem.group_gone",
            TagProblem::ColorInvalid => "tags.problem.color_invalid",
            TagProblem::DescriptionTooLong => "tags.problem.description_too_long",
        })
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> TagField {
        match self {
            TagProblem::NameMissing
            | TagProblem::NameTooLong
            | TagProblem::NameTaken
            | TagProblem::GroupNameTaken => TagField::Name,
            TagProblem::GroupGone => TagField::Group,
            TagProblem::ColorInvalid => TagField::Color,
            TagProblem::DescriptionTooLong => TagField::Description,
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn name_problem(name: &str) -> Option<TagProblem> {
    if name.is_empty() {
        Some(TagProblem::NameMissing)
    } else if engine_len(name) > NAME_MAX {
        Some(TagProblem::NameTooLong)
    } else {
        None
    }
}

/// Checks a tag draft.
///
/// # Errors
/// Every problem found, in field order.
pub fn check_tag(
    draft: &TagDraft,
    list: &TagList,
    editing: Option<TagId>,
) -> Result<CheckedTag, Vec<TagProblem>> {
    let mut problems = Vec::new();
    let name = draft.name.trim();
    if let Some(p) = name_problem(name) {
        problems.push(p);
    } else if list.tag_name_owner(name, editing).is_some() {
        problems.push(TagProblem::NameTaken);
    }
    if draft.group.is_some_and(|g| list.group(g).is_none()) {
        problems.push(TagProblem::GroupGone);
    }
    let color = non_empty(&draft.color).map(|c| c.to_ascii_lowercase());
    if color.as_deref().is_some_and(|c| !is_color(c)) {
        problems.push(TagProblem::ColorInvalid);
    }
    if engine_len(draft.description.trim()) > DESCRIPTION_MAX {
        problems.push(TagProblem::DescriptionTooLong);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(CheckedTag {
        name: name.to_string(),
        group: draft.group,
        color,
        description: non_empty(&draft.description),
    })
}

/// Checks a group draft.
///
/// # Errors
/// Every problem found, in field order.
pub fn check_group(
    draft: &GroupDraft,
    list: &TagList,
    editing: Option<TagGroupId>,
) -> Result<CheckedGroup, Vec<TagProblem>> {
    let mut problems = Vec::new();
    let name = draft.name.trim();
    if let Some(p) = name_problem(name) {
        problems.push(p);
    } else if list.group_name_owner(name, editing).is_some() {
        problems.push(TagProblem::GroupNameTaken);
    }
    if engine_len(draft.description.trim()) > DESCRIPTION_MAX {
        problems.push(TagProblem::DescriptionTooLong);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(CheckedGroup {
        name: name.to_string(),
        description: non_empty(&draft.description),
        single: draft.single,
        required_for: draft.required_for.clone(),
    })
}

/// Why a line's tags break a rule (Q-179, Q-180, U-45).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LineTagProblem {
    /// Two or more tags of one one-per-group group.
    OnePerGroup(TagGroupId),
    /// The line's account type needs a tag of this group.
    RequiredMissing(TagGroupId),
    /// The tag is inactive or gone.
    TagUnavailable(TagId),
    /// More tags than the engine keeps on one line (one until U-45).
    TooManyForEngine,
}

impl LineTagProblem {
    /// Plain-language message (the group name fills `{group}`).
    #[must_use]
    pub const fn key(&self) -> TextKey {
        TextKey(match self {
            LineTagProblem::OnePerGroup(_) => "tags.line.one_per_group",
            LineTagProblem::RequiredMissing(_) => "tags.line.required_missing",
            LineTagProblem::TagUnavailable(_) => "tags.line.tag_unavailable",
            LineTagProblem::TooManyForEngine => "tags.line.too_many_for_engine",
        })
    }
}

/// Checks the tags of one line on an account of `kind` (Q-179, Q-180),
/// and, unless the engine keeps several tags per line (`several`, U-45),
/// that there is at most one.
#[must_use]
pub fn check_line_tags(
    list: &TagList,
    tags: &[TagId],
    kind: Option<AccountKind>,
    several: bool,
) -> Vec<LineTagProblem> {
    let mut problems = Vec::new();
    let mut seen: BTreeMap<TagGroupId, usize> = BTreeMap::new();
    for id in tags {
        match list.tag(*id) {
            Some(t) if t.active => {
                if let Some(g) = t.group {
                    *seen.entry(g).or_default() += 1;
                }
            }
            _ => problems.push(LineTagProblem::TagUnavailable(*id)),
        }
    }
    for g in list.groups() {
        if g.single && seen.get(&g.id).is_some_and(|n| *n > 1) {
            problems.push(LineTagProblem::OnePerGroup(g.id));
        }
        if kind.is_some_and(|k| g.required_for.contains(&k)) && !seen.contains_key(&g.id) {
            problems.push(LineTagProblem::RequiredMissing(g.id));
        }
    }
    if !several && tags.len() > 1 {
        problems.push(LineTagProblem::TooManyForEngine);
    }
    problems
}

/// A line's tags with the header's as defaults (Q-178): the line keeps its
/// own tags; a header tag is added unless the line already has a tag of
/// that header tag's one-per-group group. Order: the line's, then the
/// header's; no repeats.
#[must_use]
pub fn effective_line_tags(list: &TagList, header: &[TagId], line: &[TagId]) -> Vec<TagId> {
    let mut out: Vec<TagId> = Vec::new();
    for id in line {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    for id in header {
        if out.contains(id) {
            continue;
        }
        let taken = list.group_of(*id).is_some_and(|g| {
            g.single
                && out
                    .iter()
                    .any(|o| list.tag(*o).and_then(|t| t.group) == Some(g.id))
        });
        if !taken {
            out.push(*id);
        }
    }
    out
}

/// A rule's or memorized transaction's tags applied to a line (Q-181): each
/// rule tag replaces the line's tags of the same one-per-group group and is
/// added otherwise; the line's other tags stay.
#[must_use]
pub fn apply_rule_tags(list: &TagList, line: &[TagId], rule: &[TagId]) -> Vec<TagId> {
    let replaced: BTreeSet<TagGroupId> = rule
        .iter()
        .filter_map(|id| list.group_of(*id))
        .filter(|g| g.single)
        .map(|g| g.id)
        .collect();
    let mut out: Vec<TagId> = line
        .iter()
        .copied()
        .filter(|id| {
            list.tag(*id)
                .and_then(|t| t.group)
                .is_none_or(|g| !replaced.contains(&g))
        })
        .collect();
    for id in rule {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    out
}

/// The one tag the engine keeps on a line today (U-45): none, or the only
/// one.
///
/// # Errors
/// [`LineTagProblem::TooManyForEngine`] for two or more tags.
pub fn engine_line_tag(tags: &[TagId]) -> Result<Option<TagId>, LineTagProblem> {
    match tags {
        [] => Ok(None),
        [one] => Ok(Some(*one)),
        _ => Err(LineTagProblem::TooManyForEngine),
    }
}

#[cfg(test)]
mod tests;
