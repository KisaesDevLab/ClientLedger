//! The demo's unbilled costs (P20, U-128 played for `--demo`): a stump
//! grinder rental billed to the story client, and the markup rule.
//!
//! - markup = round2(cost × percent ÷ 100), half away from zero; a
//!   negative cost (a vendor refund) gets no markup (Q-P20z);
//! - the invoice line is one line whose amount is cost + markup; its rate is
//!   that amount ÷ quantity to 4 places (Q-P20w), shown only.
//!
//! Synthetic data.

use cl_api::{BillableCost, PageRequest};
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
    let markup = match markup_percent {
        Some(p) if !amount.is_sign_negative() => round(amount * p / Decimal::ONE_HUNDRED, 2),
        _ => Decimal::ZERO,
    };
    let total = amount + markup;
    let rate = if quantity.is_zero() {
        total
    } else {
        round(total / quantity, 4)
    };
    BillableFigures {
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
        "lineId": STORY_COST_LINE,
        "transactionId": "d4000000-0000-4000-8000-000000000101",
        "txnType": "bill",
        "txnDate": "2026-10-20",
        "txnNumber": "INV-55871",
        "vendorName": super::bills::STORY_VENDOR,
        "description": "Stump grinder rental",
        "quantity": "1",
        "unitPrice": STORY_COST,
        "amount": STORY_COST,
    });
    serde_json::from_value(v).map_or_else(|_| Vec::new(), |c| vec![(client, c)])
}

/// One cost by its source line (billed or not).
#[must_use]
pub fn cost(state: &mut DemoState, line: JournalLineId) -> Option<BillableCost> {
    all(state)
        .into_iter()
        .map(|(_, c)| c)
        .find(|c| c.line_id == line)
}

/// `GET /billable-costs?contactId=`: the client's costs not invoiced yet.
#[must_use]
pub fn costs(
    state: &mut DemoState,
    client: ContactId,
    page: PageRequest,
) -> cl_api::Page<BillableCost> {
    let rows: Vec<BillableCost> = all(state)
        .into_iter()
        .filter(|(c, cost)| *c == client && !super::invoices::is_billed(state, cost.line_id))
        .map(|(_, c)| c)
        .collect();
    let total = u64::try_from(rows.len()).unwrap_or(u64::MAX);
    let skip = usize::try_from(page.offset()).unwrap_or(usize::MAX);
    cl_api::Page {
        data: rows
            .into_iter()
            .skip(skip)
            .take(usize::from(page.limit()))
            .collect(),
        total,
    }
}
