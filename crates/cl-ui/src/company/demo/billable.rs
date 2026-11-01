//! The demo's unbilled costs (P20, U-128 played for `--demo`): a stump
//! grinder rental billed to the story client, and the markup rule, in the
//! engine's shapes (`GET /billable-costs` `items`, `billable-costs.service`).
//!
//! - cost = round2(source amount), half away from zero;
//! - markup = round2(cost × percent ÷ 100), half away from zero; a
//!   negative cost (a vendor refund) gets no markup (Q-P20z);
//! - the invoice shows one line whose amount is cost + markup; its rate is
//!   that amount ÷ quantity to 4 places (Q-P20w), shown only. The engine
//!   posts the cost and the markup as two journal lines.
//!
//! Synthetic data.

use cl_api::{BillableCost, BillableCosts};
use cl_core::ids::{ContactId, JournalLineId};
use rust_decimal::Decimal;
use serde_json::json;

use super::DemoState;
use super::invoices::round;

/// The story cost's source line.
pub const STORY_COST_LINE: &str = "d4000000-0000-4000-8000-000000000001";
/// The story cost (the plan's AC2: 842.15, 10 % markup → 84.22).
pub const STORY_COST: &str = "842.15";

/// A cost's invoice figures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BillableFigures {
    /// The cost billed (2 places).
    pub cost: Decimal,
    /// The markup (zero for none or a negative cost).
    pub markup: Decimal,
    /// The line amount, cost plus markup.
    pub amount: Decimal,
    /// amount ÷ quantity, 4 places (the amount when the quantity is zero).
    pub rate: Decimal,
}

/// The figures of a cost of `amount` over `quantity` with `markup_percent`.
#[must_use]
pub fn figures(
    amount: Decimal,
    quantity: Decimal,
    markup_percent: Option<Decimal>,
) -> BillableFigures {
    let cost = round(amount, 2);
    let markup = match markup_percent {
        Some(p) if !cost.is_sign_negative() => round(cost * p / Decimal::ONE_HUNDRED, 2),
        _ => Decimal::ZERO,
    };
    let total = cost + markup;
    let rate = if quantity.is_zero() {
        total
    } else {
        round(total / quantity, 4)
    };
    BillableFigures {
        cost,
        markup,
        amount: total,
        rate,
    }
}

fn story_client(state: &mut DemoState) -> Option<ContactId> {
    let contacts = state.contacts.get_or_insert_with(super::contacts::contacts);
    contacts
        .iter()
        .find(|c| c.fields.display_name == super::bills::STORY_CUSTOMER)
        .map(|c| c.id)
}

fn all(state: &mut DemoState) -> Vec<(ContactId, BillableCost)> {
    let Some(client) = story_client(state) else {
        return Vec::new();
    };
    let v = json!({
        "sourceLineId": STORY_COST_LINE,
        "sourceTransactionId": "d4000000-0000-4000-8000-000000000101",
        "sourceTxnType": "bill",
        "sourceNumber": "INV-55871",
        "sourceDate": "2026-10-20",
        "contactId": client,
        "vendorName": super::bills::STORY_VENDOR,
        "description": "Stump grinder rental",
        "expenseAccountId": super::account_id(6100),
        "itemId": null,
        "quantity": null,
        "unitCost": null,
        "sourceAmount": "842.1500",
        "costAmount": STORY_COST,
        // No item: the engine suggests nothing; the owner picks.
        "suggestedIncomeAccountId": null,
        "suggestedIncomeAccountSource": null,
    });
    serde_json::from_value(v).map_or_else(|_| Vec::new(), |c| vec![(client, c)])
}

/// One cost by its source line (billed or not) with its client.
#[must_use]
pub fn cost(state: &mut DemoState, line: JournalLineId) -> Option<(ContactId, BillableCost)> {
    all(state)
        .into_iter()
        .find(|(_, c)| c.source_line_id == line)
}

/// `GET /billable-costs?contactId=`: the client's costs not invoiced yet,
/// with the count and sum over every page and the client's markup.
#[must_use]
pub fn costs(state: &mut DemoState, client: ContactId) -> BillableCosts {
    let rows: Vec<BillableCost> = all(state)
        .into_iter()
        .filter(|(c, cost)| *c == client && !super::invoices::is_billed(state, cost.source_line_id))
        .map(|(_, c)| c)
        .collect();
    let sum: Decimal = rows.iter().map(|c| c.cost_amount.as_decimal()).sum();
    let v = json!({
        "items": [],
        "totalCount": rows.len(),
        "totalAmount": round(sum, 2).to_string(),
        "defaultMarkupPercent": state.invoices.markups.get(&client),
        "limit": 500,
        "offset": 0,
    });
    let mut out: BillableCosts = serde_json::from_value(v).unwrap_or_else(|_| BillableCosts {
        items: Vec::new(),
        total_count: 0,
        total_amount: cl_core::Money::ZERO,
        default_markup_percent: None,
    });
    out.items = rows;
    out
}
