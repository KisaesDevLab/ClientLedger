//! Tag chips on a transaction or line: at most one tag per group (Q-179,
//! P10 plan §2.7).
//!
//! Tags are opaque here: an id, the label the screen shows, and the group the
//! tag belongs to (`Crew` for `Crew A` and `Crew B`). Ungrouped tags are
//! unlimited. Required groups and per-account rules are list rules (P11) and
//! are checked by the screen, not here.

use crate::text::TextKey;

/// One tag on a cell.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TagChip {
    /// The tag's id (opaque).
    pub id: u64,
    /// The tag's name as shown on the chip.
    pub label: String,
    /// The tag group's id; `None` for an ungrouped tag.
    pub group: Option<u64>,
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
