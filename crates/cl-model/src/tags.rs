//! Tag chips on a transaction or line: at most one tag per group (Q-179,
//! P10 plan §2.7).
//!
//! Tags are opaque here: an id, the label the screen shows, and the group the
//! tag belongs to (`Crew` for `Crew A` and `Crew B`). Ungrouped tags are
//! unlimited. Required groups and per-account rules are list rules (P11) and
//! are checked by the screen, not here.
//!
//! Tag numbers (Q-270): a tag shows as `number · name` ([`label`]) and lists
//! sort by number, then name ([`sort_key`]), the order the chart of accounts
//! uses (`900` before `1000`, unnumbered tags last).

use std::cmp::Ordering;

use crate::lists::accounts::number_cmp;
use crate::text::TextKey;

/// Separator between a tag's number and its name.
pub const NUMBER_SEPARATOR: &str = " · ";

/// The tag as every screen shows it: `10 · Lake Area`, or just the name when
/// it has no number (a blank number counts as none).
#[must_use]
pub fn label(number: Option<&str>, name: &str) -> String {
    match number.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => format!("{n}{NUMBER_SEPARATOR}{name}"),
        None => name.to_string(),
    }
}

/// The order of tags in lists and pickers: see [`sort_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSortKey {
    number: Option<String>,
    name_lc: String,
    name: String,
}

impl Ord for TagSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let by_number = match (&self.number, &other.number) {
            (Some(a), Some(b)) => number_cmp(a, b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        };
        by_number
            .then_with(|| self.name_lc.cmp(&other.name_lc))
            .then_with(|| self.name.cmp(&other.name))
            .then_with(|| self.number.cmp(&other.number))
    }
}

impl PartialOrd for TagSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Sort key of a tag: numbered tags first, by number (natural order: runs of
/// digits as numbers, other text ignoring case, like account numbers), then
/// by name ignoring case;
/// tags without a number (or with a blank one) last, by name.
#[must_use]
pub fn sort_key(number: Option<&str>, name: &str) -> TagSortKey {
    TagSortKey {
        number: number
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string),
        name_lc: name.to_lowercase(),
        name: name.to_string(),
    }
}

/// One tag on a cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagChip {
    /// The tag's id (opaque).
    pub id: u64,
    /// The tag as shown on the chip ([`label`]: `10 · Lake Area`).
    pub label: String,
    /// The tag group's id; `None` for an ungrouped tag.
    pub group: Option<u64>,
}

impl TagChip {
    /// A chip labelled with [`label`].
    #[must_use]
    pub fn new(id: u64, number: Option<&str>, name: &str, group: Option<u64>) -> TagChip {
        TagChip {
            id,
            label: label(number, name),
            group,
        }
    }
}

/// What [`add`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAdd {
    /// The tag was added at the end.
    Added,
    /// Another tag of the same group was replaced, in place.
    Replaced {
        /// The tag that was removed.
        previous: TagChip,
    },
    /// The tag was already there; nothing changed.
    Duplicate,
}

impl TagAdd {
    /// The caption to show after the change, when there is one ("Crew B
    /// replaced Crew A: one tag per group").
    #[must_use]
    pub fn caption_key(&self) -> Option<TextKey> {
        match self {
            TagAdd::Replaced { .. } => Some(TextKey("grid.tags.replaced")),
            TagAdd::Duplicate => Some(TextKey("grid.tags.duplicate")),
            TagAdd::Added => None,
        }
    }
}

/// Add `new` to `chips`, keeping at most one tag per group: a tag of the
/// same group is replaced where it stood.
pub fn add(chips: &mut Vec<TagChip>, new: TagChip) -> TagAdd {
    if chips.iter().any(|c| c.id == new.id) {
        return TagAdd::Duplicate;
    }
    if let Some(group) = new.group
        && let Some(slot) = chips.iter_mut().find(|c| c.group == Some(group))
    {
        let previous = std::mem::replace(slot, new);
        return TagAdd::Replaced { previous };
    }
    chips.push(new);
    TagAdd::Added
}

/// Remove the last chip (Backspace in an empty tag input).
pub fn remove_last(chips: &mut Vec<TagChip>) -> Option<TagChip> {
    chips.pop()
}

/// Remove the chip with this id (the chip's "Remove tag" button).
pub fn remove(chips: &mut Vec<TagChip>, id: u64) -> Option<TagChip> {
    let at = chips.iter().position(|c| c.id == id)?;
    Some(chips.remove(at))
}

/// Do these chips hold at most one tag per group and no tag twice?
#[must_use]
pub fn is_valid(chips: &[TagChip]) -> bool {
    chips.iter().enumerate().all(|(i, c)| {
        chips[..i]
            .iter()
            .all(|p| p.id != c.id && (c.group.is_none() || p.group != c.group))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREW: u64 = 7;

    fn chip(id: u64, label: &str, group: Option<u64>) -> TagChip {
        TagChip {
            id,
            label: label.to_string(),
            group,
        }
    }

    #[test]
    fn same_group_replaces_in_place() {
        let mut chips = vec![chip(1, "Install", None), chip(2, "Crew A", Some(CREW))];
        assert_eq!(add(&mut chips, chip(3, "Spring", None)), TagAdd::Added);
        let r = add(&mut chips, chip(4, "Crew B", Some(CREW)));
        assert_eq!(
            r,
            TagAdd::Replaced {
                previous: chip(2, "Crew A", Some(CREW))
            }
        );
        assert_eq!(r.caption_key(), Some(TextKey("grid.tags.replaced")));
        let labels: Vec<&str> = chips.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Install", "Crew B", "Spring"]);
        assert!(is_valid(&chips));
    }

    #[test]
    fn ungrouped_tags_are_unlimited_and_duplicates_ignored() {
        let mut chips = Vec::new();
        for id in 1..=5 {
            assert_eq!(add(&mut chips, chip(id, "t", None)), TagAdd::Added);
        }
        assert_eq!(chips.len(), 5);
        let d = add(&mut chips, chip(3, "t", None));
        assert_eq!(d, TagAdd::Duplicate);
        assert_eq!(d.caption_key(), Some(TextKey("grid.tags.duplicate")));
        assert_eq!(TagAdd::Added.caption_key(), None);
        assert_eq!(chips.len(), 5);
        // Re-adding the tag already in a group is a duplicate, not a swap.
        let mut g = vec![chip(2, "Crew A", Some(CREW))];
        assert_eq!(
            add(&mut g, chip(2, "Crew A", Some(CREW))),
            TagAdd::Duplicate
        );
    }

    #[test]
    fn removing() {
        let mut chips = vec![
            chip(1, "a", None),
            chip(2, "b", Some(CREW)),
            chip(3, "c", None),
        ];
        assert_eq!(remove(&mut chips, 2).map(|c| c.id), Some(2));
        assert_eq!(remove(&mut chips, 2), None);
        assert_eq!(remove_last(&mut chips).map(|c| c.id), Some(3));
        assert_eq!(remove_last(&mut chips).map(|c| c.id), Some(1));
        assert_eq!(remove_last(&mut chips), None);
    }

    #[test]
    fn labels_put_the_number_first() {
        assert_eq!(label(Some("10"), "Lake Area"), "10 · Lake Area");
        assert_eq!(label(Some(" 10 "), "Lake Area"), "10 · Lake Area");
        assert_eq!(label(None, "Lake Area"), "Lake Area");
        assert_eq!(label(Some(""), "Lake Area"), "Lake Area");
        assert_eq!(label(Some("   "), "Lake Area"), "Lake Area");
        let c = TagChip::new(4, Some("A-2"), "Crew B", Some(CREW));
        assert_eq!(c.label, "A-2 · Crew B");
        assert_eq!(c, chip(4, "A-2 · Crew B", Some(CREW)));
        assert_eq!(TagChip::new(5, None, "Spring", None).label, "Spring");
    }

    #[test]
    fn sort_by_number_then_name_unnumbered_last() {
        let mut tags = vec![
            (None, "beta"),
            (Some("1000"), "Annex"),
            (Some("900"), "Zed"),
            (Some(""), "Alpha"),
            (Some("B-1"), "Crew"),
            (Some("a-1"), "Crew"),
            (None, "Apple"),
            (Some("10"), "Lake Area"),
            (Some("010"), "Lake Area"),
            (Some("10"), "Hill"),
        ];
        tags.sort_by_key(|(n, name)| sort_key(*n, name));
        let got: Vec<String> = tags.iter().map(|(n, name)| label(*n, name)).collect();
        assert_eq!(
            got,
            [
                "10 · Hill",
                "10 · Lake Area",
                "010 · Lake Area",
                "900 · Zed",
                "1000 · Annex",
                "a-1 · Crew",
                "B-1 · Crew",
                "Alpha",
                "Apple",
                "beta",
            ]
        );
        // The same name in different cases still has a fixed order.
        assert!(sort_key(None, "Apple") < sort_key(None, "apple"));
        assert_eq!(
            sort_key(Some("5"), "x").partial_cmp(&sort_key(Some("5"), "x")),
            Some(Ordering::Equal)
        );
    }

    #[test]
    fn validity() {
        assert!(is_valid(&[]));
        assert!(is_valid(&[chip(1, "a", None), chip(2, "b", None)]));
        assert!(!is_valid(&[
            chip(1, "a", Some(CREW)),
            chip(2, "b", Some(CREW))
        ]));
        assert!(!is_valid(&[chip(1, "a", None), chip(1, "a", None)]));
        assert!(is_valid(&[chip(1, "a", Some(1)), chip(2, "b", Some(2))]));
    }
}
