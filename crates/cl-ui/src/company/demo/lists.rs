//! The demo "engine" lists beyond the chart and the contacts (P11 steps
//! 4-5): Ozark Ridge's service items and its tags. Synthetic.

use cl_api::{Item, ItemFields, Tag, TagFields, TagGroup, TagGroupFields, TagUsage};
use cl_core::Money;
use cl_core::ids::{ItemId, TagGroupId, TagId};

use super::{DemoState, account_id};

/// A demo item id (stable across runs).
#[must_use]
pub fn item_id(n: u32) -> ItemId {
    ItemId::parse(&format!("d0000000-0000-4000-8000-b{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

fn price(s: &str) -> Option<Money> {
    Money::parse_wire(s).ok()
}

/// The demo items: services Ozark Ridge sells, all posting to Service
/// Income (4000); one is inactive so the filter has something to show.
#[must_use]
pub fn items() -> Vec<Item> {
    // Name, description, price, taxable, active.
    #[allow(clippy::type_complexity)]
    let rows: [(&str, Option<&str>, Option<&str>, bool, bool); 7] = [
        (
            "Weekly Mowing",
            Some("Mow, trim and blow, per visit"),
            Some("45.0000"),
            true,
            true,
        ),
        (
            "Spring Cleanup",
            Some("Beds, leaves and first cut"),
            Some("285.0000"),
            true,
            true,
        ),
        (
            "Mulch Installation",
            Some("Per cubic yard, installed"),
            Some("68.5000"),
            true,
            true,
        ),
        (
            "Hedge Trimming",
            Some("Per hour"),
            Some("55.0000"),
            true,
            true,
        ),
        ("Irrigation Check", None, Some("95.0000"), false, true),
        (
            "Design Consultation",
            Some("Hourly, billed in quarter hours"),
            Some("0.0450"),
            false,
            true,
        ),
        (
            "Snow Plowing",
            Some("Per push, driveways"),
            Some("75.0000"),
            true,
            false,
        ),
    ];
    rows.iter()
        .enumerate()
        .map(|(i, (name, description, p, taxable, active))| Item {
            id: item_id(u32::try_from(i + 1).unwrap_or(u32::MAX)),
            fields: ItemFields {
                name: (*name).to_string(),
                description: description.map(str::to_string),
                unit_price: p.and_then(price),
                income_account_id: account_id(4000),
                is_taxable: *taxable,
            },
            is_active: *active,
            default_tag_id: None,
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
        })
        .collect()
}

/// A demo tag id (stable across runs).
#[must_use]
pub fn tag_id(n: u32) -> TagId {
    TagId::parse(&format!("d0000000-0000-4000-8000-c{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// A demo tag group id (stable across runs).
#[must_use]
pub fn group_id(n: u32) -> TagGroupId {
    TagGroupId::parse(&format!("d0000000-0000-4000-8000-e{n:011}"))
        .unwrap_or_else(|_| unreachable!("a well-formed demo id"))
}

/// The demo tag groups: Route (one per line) and Crew (any number).
#[must_use]
pub fn tag_groups() -> Vec<TagGroup> {
    [
        (1, "Route", "Where the work is", true),
        (2, "Crew", "Who did the work", false),
    ]
    .into_iter()
    .map(|(n, name, description, single)| TagGroup {
        id: group_id(n),
        fields: TagGroupFields {
            name: name.to_string(),
            description: Some(description.to_string()),
            is_single_select: single,
        },
        sort_order: Some(i64::from(n)),
        tags: Vec::new(),
    })
    .collect()
}

/// The demo tags: three routes, two crews, two ungrouped tags and an
/// inactive promotion, then the sample company's own tags (the Write
/// Check story's Install and Crew A).
#[must_use]
pub fn tags() -> Vec<Tag> {
    let mut rows: Vec<(u32, String, Option<TagGroupId>, bool)> = [
        (1, "North Route", Some(1), true),
        (2, "South Route", Some(1), true),
        (3, "Lake Area", Some(1), true),
        (4, "Crew 1", Some(2), true),
        (5, "Crew 2", Some(2), true),
        (6, "Warranty Work", None, true),
        (7, "Storm Cleanup", None, true),
        (8, "Spring Promo 2025", None, false),
    ]
    .into_iter()
    .map(|(n, name, group, active)| (n, name.to_string(), group.map(group_id), active))
    .collect();
    let groups = tag_groups();
    for (n, t) in (9..).zip(cl_company::Sample::load().company.tags) {
        let group = t
            .group
            .as_deref()
            .and_then(|g| groups.iter().find(|x| x.fields.name == g))
            .map(|x| x.id);
        rows.push((n, t.name, group, true));
    }
    rows.into_iter()
        .map(|(n, name, group, active)| Tag {
            id: tag_id(n),
            fields: TagFields {
                name,
                color: None,
                group_id: group,
                description: None,
                tag_number: None,
            },
            is_active: active,
            usage_count: Some(0),
            sort_order: Some(0),
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
        })
        .collect()
}

/// Where a demo tag is used: Lake Area on transaction lines (so it can't be
/// merged yet, U-41), Storm Cleanup on three transactions (header tags
/// only), Crew 1 as an item's default; the rest unused.
#[must_use]
pub fn tag_usage(id: TagId) -> TagUsage {
    if id == tag_id(3) {
        TagUsage {
            transaction_lines: 12,
            transactions: 4,
            total: 16,
            ..TagUsage::default()
        }
    } else if id == tag_id(7) {
        TagUsage {
            transactions: 3,
            total: 3,
            ..TagUsage::default()
        }
    } else if id == tag_id(4) {
        TagUsage {
            items: 1,
            total: 1,
            ..TagUsage::default()
        }
    } else {
        TagUsage::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_tags_have_groups_and_usage() {
        let groups = tag_groups();
        let tags = tags();
        assert!(groups[0].fields.is_single_select && !groups[1].fields.is_single_select);
        assert_eq!(tags.iter().filter(|t| !t.is_active).count(), 1);
        assert!(!tag_usage(tag_id(3)).merge_safe());
        assert!(tag_usage(tag_id(7)).merge_safe());
        assert_eq!(tag_usage(tag_id(1)).total, 0);
    }

    #[test]
    fn demo_items_are_services_on_service_income() {
        let items = items();
        assert_eq!(items.len(), 7);
        assert!(
            items
                .iter()
                .all(|i| i.fields.income_account_id == account_id(4000))
        );
        assert_eq!(items.iter().filter(|i| !i.is_active).count(), 1);
        let ids: std::collections::BTreeSet<_> = items.iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), items.len());
    }
}
