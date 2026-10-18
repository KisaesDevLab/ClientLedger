//! The demo "engine" lists beyond the chart and the contacts (P11 steps
//! 4-5): Ozark Ridge's service items and its tags. Synthetic.

use cl_api::{Item, ItemFields};
use cl_core::Money;
use cl_core::ids::ItemId;

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

#[cfg(test)]
mod tests {
    use super::*;

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
