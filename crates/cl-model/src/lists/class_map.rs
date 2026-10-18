//! Classes to tags (P11 step 7, shared with the `QuickBooks` Desktop import in
//! P31; Q-P11z). `ClientLedger` has no classes: a class becomes a tag, and a
//! `Parent:Child` class becomes the tag `Child` in the tag group `Parent`.
//!
//! The rule:
//! - A class name is split on `:`; each part is trimmed and empty parts are
//!   dropped (`Crew: :North` is `Crew:North`).
//! - One part: an ungrouped tag of that name.
//! - Two or more parts: the group is the first part and the tag is the rest
//!   joined with `:` (`Region:North:Lake` is the tag `North:Lake` in
//!   `Region`), so no two classes collapse into one.
//! - A class that only serves as a parent becomes the group alone; when
//!   transactions use it directly (`used_directly`), it also gets an
//!   ungrouped tag of its own name.
//! - Groups made from classes take one tag per line (a class is one per
//!   line, Q-179). Group names match ignoring case; the first spelling wins.
//! - Tag names are unique ignoring case, against the company's existing tags
//!   and each other. A clash gets the group in brackets (`East (Crew)`), then
//!   a number (`East (Crew) 2`).
//! - Names longer than the engine takes (100 for tags and groups) are
//!   reported, not cut.
//! - An inactive class becomes an inactive tag.

use std::collections::{HashMap, HashSet};

use crate::lists::engine_len;
use crate::text::TextKey;

/// Longest tag or group name the engine takes.
pub const NAME_MAX: usize = 100;

/// One class as read from the old file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassInput {
    /// Full name, `Parent:Child` for a subclass.
    pub full_name: String,
    /// Active in the old file.
    pub active: bool,
    /// Transactions use this class itself (matters for parents only).
    pub used_directly: bool,
}

impl ClassInput {
    /// An active class used on transactions.
    #[must_use]
    pub fn new(full_name: &str) -> ClassInput {
        ClassInput {
            full_name: full_name.to_owned(),
            active: true,
            used_directly: true,
        }
    }
}

/// The group a class's tag goes in, and the tag's name, by the naming rule
/// alone (no clash handling). `None` for a blank name.
#[must_use]
pub fn split_class(full_name: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<&str> = full_name
        .split(':')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    match parts.as_slice() {
        [] => None,
        [one] => Some((None, (*one).to_owned())),
        [group, rest @ ..] => Some((Some((*group).to_owned()), rest.join(":"))),
    }
}

/// A tag group the classes need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassGroup {
    /// Name.
    pub name: String,
    /// One tag of the group per line (always true for classes).
    pub one_per_line: bool,
}

/// The tag one class becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassTag {
    /// The class's full name as read (trimmed).
    pub class: String,
    /// Its group, if any.
    pub group: Option<String>,
    /// The tag name (unique ignoring case).
    pub tag: String,
    /// Active.
    pub active: bool,
}

/// Why a class can't become a tag as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassProblem {
    /// The name is blank.
    Blank,
    /// The tag or group name would be longer than the engine takes.
    TooLong,
}

impl ClassProblem {
    /// Owner-facing text.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            ClassProblem::Blank => TextKey("classes.problem.blank"),
            ClassProblem::TooLong => TextKey("classes.problem.too_long"),
        }
    }
}

/// Every group and tag a class list becomes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClassPlan {
    /// Groups to create, in first-seen order.
    pub groups: Vec<ClassGroup>,
    /// One tag per class that needs one, in input order.
    pub tags: Vec<ClassTag>,
    /// Classes left out, with why.
    pub problems: Vec<(String, ClassProblem)>,
}

impl ClassPlan {
    /// The tag a class (full name, any case, spacing around `:` ignored)
    /// becomes; `None` for a parent-only class or one left out.
    #[must_use]
    pub fn tag_for(&self, class: &str) -> Option<&ClassTag> {
        let key = class_key(class);
        self.tags.iter().find(|t| class_key(&t.class) == key)
    }
}

fn class_key(class: &str) -> String {
    class
        .split(':')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(":")
        .to_lowercase()
}

/// Plans the tags and groups for `classes`. `existing_tags` are the
/// company's tag names (any case), which new names must not repeat.
#[must_use]
pub fn plan_classes(classes: &[ClassInput], existing_tags: &[&str]) -> ClassPlan {
    let mut plan = ClassPlan::default();
    let mut taken: HashSet<String> = existing_tags.iter().map(|t| t.to_lowercase()).collect();
    let mut group_names: HashMap<String, String> = HashMap::new();
    let mut seen_classes: HashSet<String> = HashSet::new();
    // Parents: a class whose key starts another class's key plus `:`.
    let keys: Vec<String> = classes.iter().map(|c| class_key(&c.full_name)).collect();
    let parents: HashSet<&str> = keys
        .iter()
        .filter_map(|k| k.split_once(':').map(|(p, _)| p))
        .collect();
    for (c, key) in classes.iter().zip(&keys) {
        let trimmed = c.full_name.trim().to_owned();
        let Some((group, tag)) = split_class(&c.full_name) else {
            plan.problems.push((trimmed, ClassProblem::Blank));
            continue;
        };
        if !seen_classes.insert(key.clone()) {
            continue;
        }
        let is_parent = group.is_none() && parents.contains(key.as_str());
        if is_parent {
            if engine_len(&tag) > NAME_MAX {
                plan.problems.push((trimmed, ClassProblem::TooLong));
                continue;
            }
            group_for(&mut plan, &mut group_names, &tag);
            if !c.used_directly {
                continue;
            }
        }
        let too_long =
            engine_len(&tag) > NAME_MAX || group.as_ref().is_some_and(|g| engine_len(g) > NAME_MAX);
        if too_long {
            plan.problems.push((trimmed, ClassProblem::TooLong));
            continue;
        }
        let group = group.map(|g| group_for(&mut plan, &mut group_names, &g));
        let name = unique_name(&tag, group.as_deref(), &taken);
        if engine_len(&name) > NAME_MAX {
            plan.problems.push((trimmed, ClassProblem::TooLong));
            continue;
        }
        taken.insert(name.to_lowercase());
        plan.tags.push(ClassTag {
            class: trimmed,
            group,
            tag: name,
            active: c.active,
        });
    }
    plan
}

/// The group named `name` (ignoring case), added if new; returns the
/// spelling in use.
fn group_for(plan: &mut ClassPlan, names: &mut HashMap<String, String>, name: &str) -> String {
    names
        .entry(name.to_lowercase())
        .or_insert_with(|| {
            plan.groups.push(ClassGroup {
                name: name.to_owned(),
                one_per_line: true,
            });
            name.to_owned()
        })
        .clone()
}

fn unique_name(tag: &str, group: Option<&str>, taken: &HashSet<String>) -> String {
    if !taken.contains(&tag.to_lowercase()) {
        return tag.to_owned();
    }
    let base = match group {
        Some(g) => format!("{tag} ({g})"),
        None => tag.to_owned(),
    };
    if !taken.contains(&base.to_lowercase()) {
        return base;
    }
    // Bounded: of the `taken.len() + 1` candidates at least one is free.
    (2..=taken.len() + 2)
        .map(|n| format!("{base} {n}"))
        .find(|c| !taken.contains(&c.to_lowercase()))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests;
