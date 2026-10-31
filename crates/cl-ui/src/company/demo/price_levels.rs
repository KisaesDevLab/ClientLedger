//! The demo's price levels (P20, U-40 played for `--demo`, Q-P20l): a name
//! and a percent up or down, with per-item prices that win. A price is
//! round2(item price × (1 + percent ÷ 100)), half away from zero. The
//! discount helper is the U-39a rule: percent of the lines it covers,
//! negative. Synthetic data.

use std::collections::BTreeMap;

use cl_api::{PriceLevel, SignedPercent};
use cl_core::ids::{ContactId, ItemId};
use rust_decimal::Decimal;

use super::DemoState;
use super::invoices::round;

/// Price levels and which client is on which.
#[derive(Clone, Debug, Default)]
pub struct DemoPriceLevels {
    /// The levels.
    pub levels: Vec<PriceLevel>,
    /// One level per client.
    pub clients: BTreeMap<ContactId, cl_api::PriceLevelId>,
}

impl DemoPriceLevels {
    /// The demo's one level (P20 WP-B): Commercial, 7.5 % down, on the
    /// story client, so an unpriced item line shows a level price.
    #[must_use]
    pub fn seeded() -> DemoPriceLevels {
        let mut p = DemoPriceLevels::default();
        let client = super::contacts::contacts()
            .into_iter()
            .find(|c| c.fields.display_name == super::bills::STORY_CUSTOMER)
            .map(|c| c.id);
        let level: Option<PriceLevel> = serde_json::from_value(serde_json::json!({
            "id": "d6000000-0000-4000-8000-000000000101",
            "name": "Commercial", "percent": "-7.5", "isActive": true,
        }))
        .ok();
        if let (Some(c), Some(l)) = (client, level) {
            p.clients.insert(c, l.id);
            p.levels.push(l);
        }
        p
    }
}

/// The price of an item with `base` price for `client` and where it came
/// from (`item`, `price_level`, `price_level_percent`).
#[must_use]
pub fn price_for(
    state: &DemoState,
    client: ContactId,
    item: Option<ItemId>,
    base: Decimal,
) -> (Decimal, &'static str) {
    let p = &state.price_levels;
    let Some(level) = p
        .clients
        .get(&client)
        .and_then(|id| p.levels.iter().find(|l| l.id == *id && l.is_active))
    else {
        return (base, "item");
    };
    if let Some(ip) = item.and_then(|i| level.item_prices.iter().find(|ip| ip.item_id == i)) {
        return (ip.price.as_decimal(), "price_level");
    }
    (level_price(base, level.percent), "price_level_percent")
}

/// round2(base × (1 + percent ÷ 100)).
#[must_use]
pub fn level_price(base: Decimal, percent: SignedPercent) -> Decimal {
    round(
        base * (Decimal::ONE_HUNDRED + percent.as_decimal()) / Decimal::ONE_HUNDRED,
        2,
    )
}

/// A percent discount of the lines it covers: −round2(base × percent ÷ 100).
#[must_use]
pub fn discount_of(base: Decimal, percent: Decimal) -> Decimal {
    -round(base * percent / Decimal::ONE_HUNDRED, 2)
}
