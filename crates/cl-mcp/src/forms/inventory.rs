//! Inventory (P24): a checked quantity/value adjustment or a physical count
//! as the engine's request, field by field (the engine values the stock).

use cl_api::{NewAdjustment, NewCount};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_model::inventory::{CheckedAdjustment, CheckedCountLine};

/// The checked adjustment as the engine's request.
#[must_use]
pub fn adjustment(checked: CheckedAdjustment) -> NewAdjustment {
    NewAdjustment {
        date: checked.date,
        adjustment_account_id: checked.account,
        memo: checked.memo,
        lines: checked
            .lines
            .iter()
            .map(|l| cl_api::AdjustmentLine {
                item_id: l.item,
                quantity_change: l.quantity_change,
                value_change: l.value_change,
                unit_id: l.unit,
            })
            .collect(),
    }
}

/// The checked count sheet as the engine's request. `memo` is the sheet's
/// memo as typed (sent trimmed, or not at all when blank).
#[must_use]
pub fn count(
    as_of: CalendarDate,
    account: AccountId,
    memo: &str,
    lines: &[CheckedCountLine],
) -> NewCount {
    NewCount {
        as_of,
        adjustment_account_id: account,
        memo: (!memo.trim().is_empty()).then(|| memo.trim().to_string()),
        lines: lines
            .iter()
            .map(|l| cl_api::CountLine {
                item_id: l.item,
                sheet_on_hand: l.sheet_on_hand,
                counted_quantity: l.counted,
                unit_id: l.unit,
            })
            .collect(),
    }
}
