//! Inventory forms and lists (P24 plan §3): the inventory section of the
//! item form, the adjustment form, the physical count worksheet and the
//! reorder list. No egui; the screens (WP-2) draw these.
//!
//! - Every figure the engine computes (value at average cost, on hand,
//!   averages, valuation) is shown from the engine; the desktop parses
//!   what the owner types with `rust_decimal`-backed types and never
//!   derives a balance (plan §4).
//! - A count's difference (counted − on hand as the sheet showed it) is the
//!   one subtraction here: a quantity the owner reviews before posting;
//!   the engine recomputes on hand and refuses a stale sheet (Q-P24g).
//! - Closed periods (owner 2026-11-06): anything dated on or before the
//!   closing date is refused for inventory, even with the password; the
//!   forms say so before sending.

use cl_core::ids::{AccountId, ContactId, ItemId, UnitId, UnitSetId};
use cl_core::{CalendarDate, Money, Quantity, UnitCost, UserInput};

use crate::amount_entry::{self, Amount, AmountKind, AmountRules, Sign};
use crate::lists::engine_len;
use crate::text::TextKey;

/// The most characters of a unit label (Q-P24i).
pub const UNIT_LABEL_MAX: usize = 10;
/// The longest purchase description kept.
pub const PURCHASE_DESCRIPTION_MAX: usize = 2000;
/// The longest adjustment memo kept.
pub const MEMO_MAX: usize = 4000;

const COST_RULES: AmountRules = AmountRules {
    kind: AmountKind::Money,
    input: UserInput::FULL,
    sign: Sign::NonNegative,
};
const QTY_RULES: AmountRules = AmountRules {
    kind: AmountKind::Quantity,
    input: UserInput::FULL,
    sign: Sign::NonNegative,
};
const QTY_CHANGE_RULES: AmountRules = AmountRules {
    kind: AmountKind::Quantity,
    input: UserInput::FULL,
    sign: Sign::Any,
};
const VALUE_RULES: AmountRules = AmountRules::MONEY;

fn parse_qty(text: &str, rules: AmountRules) -> Result<Option<Quantity>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    match amount_entry::parse(text, rules) {
        Ok(p) => match p.value {
            Amount::Quantity(q) => Ok(Some(q)),
            _ => Err(()),
        },
        Err(_) => Err(()),
    }
}

fn parse_money(text: &str, rules: AmountRules) -> Result<Option<Money>, ()> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    match amount_entry::parse(text, rules) {
        Ok(p) => p.value.as_money().map(Some).ok_or(()),
        Err(_) => Err(()),
    }
}

/// Text of a quantity for a field: no trailing zeros (`3.5`, `10`).
#[must_use]
pub fn quantity_text(q: Quantity) -> String {
    let w = q.to_wire();
    w.trim_end_matches('0').trim_end_matches('.').to_string()
}

// ---- The item form's inventory section ------------------------------------------

/// What the item form's purchase and inventory fields hold as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartDraft {
    /// Purchase description.
    pub purchase_description: String,
    /// Cost per unit (up to four decimals).
    pub cost: String,
    /// Expense account (non-stock purchases).
    pub expense_account: Option<AccountId>,
    /// Inventory asset account (parts; Q-P24k: fixed once it has stock).
    pub asset_account: Option<AccountId>,
    /// Cost of goods sold account (parts).
    pub cogs_account: Option<AccountId>,
    /// Preferred vendor.
    pub preferred_vendor: Option<ContactId>,
    /// Reorder point.
    pub reorder_point: String,
    /// Quantity to reorder.
    pub reorder_quantity: String,
    /// Unit label (display only, for an item without a unit set).
    pub unit_label: String,
    /// The item's unit set (U-161; stock is kept in its base unit).
    pub unit_set: Option<UnitSetId>,
    /// The unit purchase lines start in (`None`: the base unit).
    pub purchase_unit: Option<UnitId>,
    /// The unit sales lines start in (`None`: the base unit).
    pub sale_unit: Option<UnitId>,
    /// Opening quantity on hand (a new part only, Q-P24h).
    pub opening_quantity: String,
    /// Opening total value.
    pub opening_value: String,
    /// The date that quantity was on hand.
    pub opening_date: Option<CalendarDate>,
}

/// A part's opening stock, checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedOpening {
    /// Units (above zero).
    pub quantity: Quantity,
    /// Total value (zero or more, whole cents).
    pub value: Money,
    /// The date.
    pub as_of: CalendarDate,
}

/// The inventory section after [`check_part`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CheckedPart {
    /// Purchase description (`None` when blank).
    pub purchase_description: Option<String>,
    /// Cost per unit.
    pub cost: Option<Money>,
    /// Expense account.
    pub expense_account: Option<AccountId>,
    /// Asset account (always set on a part).
    pub asset_account: Option<AccountId>,
    /// COGS account (always set on a part).
    pub cogs_account: Option<AccountId>,
    /// Preferred vendor.
    pub preferred_vendor: Option<ContactId>,
    /// Reorder point.
    pub reorder_point: Option<Quantity>,
    /// Reorder quantity.
    pub reorder_quantity: Option<Quantity>,
    /// Unit label (`None` when blank or when a unit set is chosen).
    pub unit_label: Option<String>,
    /// Unit set.
    pub unit_set: Option<UnitSetId>,
    /// Default purchase unit (only with a unit set).
    pub purchase_unit: Option<UnitId>,
    /// Default sale unit (only with a unit set).
    pub sale_unit: Option<UnitId>,
    /// Opening stock (a new part only).
    pub opening: Option<CheckedOpening>,
}

/// A field of the inventory section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartField {
    /// Purchase description.
    PurchaseDescription,
    /// Cost.
    Cost,
    /// Asset account.
    AssetAccount,
    /// COGS account.
    CogsAccount,
    /// Reorder point.
    ReorderPoint,
    /// Reorder quantity.
    ReorderQuantity,
    /// Unit label.
    UnitLabel,
    /// Opening quantity.
    OpeningQuantity,
    /// Opening value.
    OpeningValue,
    /// Opening date.
    OpeningDate,
}

/// Why the inventory section cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PartProblem {
    /// Longer than kept.
    PurchaseDescriptionTooLong,
    /// The cost is not an amount of zero or more.
    CostInvalid,
    /// A part needs an inventory asset account.
    AssetAccountMissing,
    /// A part needs a cost of goods sold account.
    CogsAccountMissing,
    /// Not a quantity of zero or more.
    ReorderPointInvalid,
    /// Not a quantity of zero or more.
    ReorderQuantityInvalid,
    /// More than ten characters.
    UnitLabelTooLong,
    /// Not a quantity above zero.
    OpeningQuantityInvalid,
    /// Not an amount of zero or more.
    OpeningValueInvalid,
    /// A value or date without a quantity, or a quantity without a date.
    OpeningIncomplete,
    /// Dated on or before the closing date (owner 2026-11-06).
    OpeningInClosedPeriod,
}

impl PartProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            PartProblem::PurchaseDescriptionTooLong => {
                "inventory.problem.purchase_description_long"
            }
            PartProblem::CostInvalid => "inventory.problem.cost_invalid",
            PartProblem::AssetAccountMissing => "inventory.problem.asset_account_missing",
            PartProblem::CogsAccountMissing => "inventory.problem.cogs_account_missing",
            PartProblem::ReorderPointInvalid => "inventory.problem.reorder_point_invalid",
            PartProblem::ReorderQuantityInvalid => "inventory.problem.reorder_quantity_invalid",
            PartProblem::UnitLabelTooLong => "inventory.problem.unit_label_long",
            PartProblem::OpeningQuantityInvalid => "inventory.problem.opening_quantity_invalid",
            PartProblem::OpeningValueInvalid => "inventory.problem.opening_value_invalid",
            PartProblem::OpeningIncomplete => "inventory.problem.opening_incomplete",
            PartProblem::OpeningInClosedPeriod => "inventory.problem.closed_period",
        })
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> PartField {
        match self {
            PartProblem::PurchaseDescriptionTooLong => PartField::PurchaseDescription,
            PartProblem::CostInvalid => PartField::Cost,
            PartProblem::AssetAccountMissing => PartField::AssetAccount,
            PartProblem::CogsAccountMissing => PartField::CogsAccount,
            PartProblem::ReorderPointInvalid => PartField::ReorderPoint,
            PartProblem::ReorderQuantityInvalid => PartField::ReorderQuantity,
            PartProblem::UnitLabelTooLong => PartField::UnitLabel,
            PartProblem::OpeningQuantityInvalid | PartProblem::OpeningIncomplete => {
                PartField::OpeningQuantity
            }
            PartProblem::OpeningValueInvalid => PartField::OpeningValue,
            PartProblem::OpeningInClosedPeriod => PartField::OpeningDate,
        }
    }
}

/// Checks the inventory section. `part` is the inventory-part kind (asset
/// and COGS accounts required, opening stock allowed when `new_item`);
/// `account_ok` says whether an account still exists and is active.
///
/// # Errors
/// Every problem found, in field order.
pub fn check_part(
    d: &PartDraft,
    part: bool,
    new_item: bool,
    closing_date: Option<CalendarDate>,
    account_ok: impl Fn(AccountId) -> bool,
) -> Result<CheckedPart, Vec<PartProblem>> {
    let mut problems = Vec::new();
    let description = d.purchase_description.trim();
    if engine_len(description) > PURCHASE_DESCRIPTION_MAX {
        problems.push(PartProblem::PurchaseDescriptionTooLong);
    }
    let cost = parse_money(&d.cost, COST_RULES).unwrap_or_else(|()| {
        problems.push(PartProblem::CostInvalid);
        None
    });
    let asset = d.asset_account.filter(|a| account_ok(*a));
    let cogs = d.cogs_account.filter(|a| account_ok(*a));
    if part && asset.is_none() {
        problems.push(PartProblem::AssetAccountMissing);
    }
    if part && cogs.is_none() {
        problems.push(PartProblem::CogsAccountMissing);
    }
    let reorder_point = parse_qty(&d.reorder_point, QTY_RULES).unwrap_or_else(|()| {
        problems.push(PartProblem::ReorderPointInvalid);
        None
    });
    let reorder_quantity = parse_qty(&d.reorder_quantity, QTY_RULES).unwrap_or_else(|()| {
        problems.push(PartProblem::ReorderQuantityInvalid);
        None
    });
    let label = d.unit_label.trim();
    if label.chars().count() > UNIT_LABEL_MAX {
        problems.push(PartProblem::UnitLabelTooLong);
    }
    let opening = if part && new_item {
        check_opening(d, closing_date, &mut problems)
    } else {
        None
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(CheckedPart {
        purchase_description: (!description.is_empty()).then(|| description.to_string()),
        cost,
        expense_account: d.expense_account.filter(|a| account_ok(*a)),
        asset_account: if part { asset } else { None },
        cogs_account: if part { cogs } else { None },
        preferred_vendor: d.preferred_vendor,
        reorder_point: if part { reorder_point } else { None },
        reorder_quantity: if part { reorder_quantity } else { None },
        // A unit set names the units; the label is for items without one.
        unit_label: (!label.is_empty() && d.unit_set.is_none()).then(|| label.to_string()),
        unit_set: d.unit_set,
        purchase_unit: d.purchase_unit.filter(|_| d.unit_set.is_some()),
        sale_unit: d.sale_unit.filter(|_| d.unit_set.is_some()),
        opening,
    })
}

fn check_opening(
    d: &PartDraft,
    closing_date: Option<CalendarDate>,
    problems: &mut Vec<PartProblem>,
) -> Option<CheckedOpening> {
    let quantity = match parse_qty(&d.opening_quantity, QTY_RULES) {
        Ok(Some(q)) if q.is_positive() => Some(q),
        Ok(None) => None,
        _ => {
            problems.push(PartProblem::OpeningQuantityInvalid);
            return None;
        }
    };
    let value = match parse_money(&d.opening_value, VALUE_RULES) {
        Ok(v) if v.is_none_or(|v| !v.is_negative()) => v,
        _ => {
            problems.push(PartProblem::OpeningValueInvalid);
            return None;
        }
    };
    match (quantity, d.opening_date) {
        (None, _) if value.is_none() => None,
        (Some(quantity), Some(as_of)) => {
            if closing_date.is_some_and(|c| as_of <= c) {
                problems.push(PartProblem::OpeningInClosedPeriod);
                return None;
            }
            Some(CheckedOpening {
                quantity,
                value: value.unwrap_or(Money::ZERO),
                as_of,
            })
        }
        _ => {
            problems.push(PartProblem::OpeningIncomplete);
            None
        }
    }
}

// ---- Adjust Quantity/Value on Hand -----------------------------------------------

/// What the adjustment changes (QuickBooks-style mode switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AdjustMode {
    /// Quantity only: the engine values it at average cost.
    #[default]
    Quantity,
    /// Value only (refused with nothing on hand, Q-P24s).
    Value,
    /// Quantity and a given total value.
    Both,
}

/// One row of the adjustment form as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdjustRow {
    /// The part.
    pub item: Option<ItemId>,
    /// Quantity change (signed; or a new quantity, see `new_quantity`).
    pub quantity: String,
    /// Value change (signed, whole cents).
    pub value: String,
    /// On hand at the date as the engine reported it (shown; used for the
    /// "New quantity" column).
    pub on_hand: Option<Quantity>,
    /// The unit the quantity change is in (U-161; `None`: the base unit).
    pub unit: Option<UnitId>,
}

/// The adjustment form.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdjustmentDraft {
    /// Date.
    pub date: Option<CalendarDate>,
    /// Adjustment account.
    pub account: Option<AccountId>,
    /// Memo.
    pub memo: String,
    /// Mode.
    pub mode: AdjustMode,
    /// Rows (blank rows are ignored).
    pub rows: Vec<AdjustRow>,
}

/// One checked line, ready for `cl_api::AdjustmentLine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedAdjustLine {
    /// The part.
    pub item: ItemId,
    /// Signed quantity change.
    pub quantity_change: Option<Quantity>,
    /// Signed value change.
    pub value_change: Option<Money>,
    /// The unit of `quantity_change` (`None`: the base unit).
    pub unit: Option<UnitId>,
}

/// The adjustment, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedAdjustment {
    /// Date.
    pub date: CalendarDate,
    /// Adjustment account.
    pub account: AccountId,
    /// Memo (`None` when blank).
    pub memo: Option<String>,
    /// Lines.
    pub lines: Vec<CheckedAdjustLine>,
}

/// Why an adjustment cannot be saved (`row` is the form row, if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdjustProblem {
    /// No date.
    DateMissing,
    /// Dated on or before the closing date (owner 2026-11-06).
    ClosedPeriod,
    /// No adjustment account.
    AccountMissing,
    /// Memo longer than kept.
    MemoTooLong,
    /// No row with a part and a change.
    NothingToAdjust,
    /// A row's part is already on another row.
    ItemTwice(usize),
    /// A row's quantity is not a non-zero quantity.
    QuantityInvalid(usize),
    /// A row's value is not a non-zero amount in cents.
    ValueInvalid(usize),
    /// A row with a change but no part.
    ItemMissing(usize),
}

impl AdjustProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            AdjustProblem::DateMissing => "inventory.adjust.problem.date_missing",
            AdjustProblem::ClosedPeriod => "inventory.problem.closed_period",
            AdjustProblem::AccountMissing => "inventory.adjust.problem.account_missing",
            AdjustProblem::MemoTooLong => "inventory.adjust.problem.memo_long",
            AdjustProblem::NothingToAdjust => "inventory.adjust.problem.nothing",
            AdjustProblem::ItemTwice(_) => "inventory.adjust.problem.item_twice",
            AdjustProblem::QuantityInvalid(_) => "inventory.adjust.problem.quantity_invalid",
            AdjustProblem::ValueInvalid(_) => "inventory.adjust.problem.value_invalid",
            AdjustProblem::ItemMissing(_) => "inventory.adjust.problem.item_missing",
        })
    }
}

/// The new quantity a row shows (on hand + change), when both are known.
#[must_use]
pub fn new_quantity(row: &AdjustRow) -> Option<Quantity> {
    // In another unit the engine converts (round4(q × factor)); the
    // preview of on hand in base units is not the desktop's to derive.
    if row.unit.is_some() {
        return None;
    }
    let change = parse_qty(&row.quantity, QTY_CHANGE_RULES).ok().flatten()?;
    row.on_hand?.checked_add(change).ok()
}

/// Checks the form.
///
/// # Errors
/// Every problem found.
pub fn check_adjustment(
    d: &AdjustmentDraft,
    closing_date: Option<CalendarDate>,
) -> Result<CheckedAdjustment, Vec<AdjustProblem>> {
    let mut problems = Vec::new();
    match d.date {
        None => problems.push(AdjustProblem::DateMissing),
        Some(date) if closing_date.is_some_and(|c| date <= c) => {
            problems.push(AdjustProblem::ClosedPeriod);
        }
        Some(_) => {}
    }
    if d.account.is_none() {
        problems.push(AdjustProblem::AccountMissing);
    }
    if d.memo.trim().chars().count() > MEMO_MAX {
        problems.push(AdjustProblem::MemoTooLong);
    }
    let (want_qty, want_value) = match d.mode {
        AdjustMode::Quantity => (true, false),
        AdjustMode::Value => (false, true),
        AdjustMode::Both => (true, true),
    };
    let mut lines: Vec<CheckedAdjustLine> = Vec::new();
    for (i, r) in d.rows.iter().enumerate() {
        let blank = r.quantity.trim().is_empty() && r.value.trim().is_empty();
        if blank {
            continue;
        }
        let Some(item) = r.item else {
            problems.push(AdjustProblem::ItemMissing(i));
            continue;
        };
        if lines.iter().any(|l| l.item == item) {
            problems.push(AdjustProblem::ItemTwice(i));
            continue;
        }
        let quantity_change = if want_qty {
            match parse_qty(&r.quantity, QTY_CHANGE_RULES) {
                Ok(Some(q)) if !q.is_zero() => Some(q),
                _ => {
                    problems.push(AdjustProblem::QuantityInvalid(i));
                    None
                }
            }
        } else {
            None
        };
        let value_change = if want_value {
            match parse_money(&r.value, VALUE_RULES) {
                Ok(Some(v)) if !v.is_zero() => Some(v),
                _ => {
                    problems.push(AdjustProblem::ValueInvalid(i));
                    None
                }
            }
        } else {
            None
        };
        if quantity_change.is_some() || value_change.is_some() {
            lines.push(CheckedAdjustLine {
                item,
                unit: r.unit.filter(|_| quantity_change.is_some()),
                quantity_change,
                value_change,
            });
        }
    }
    if lines.is_empty() && problems.is_empty() {
        problems.push(AdjustProblem::NothingToAdjust);
    }
    match (d.date, d.account, problems.is_empty()) {
        (Some(date), Some(account), true) => Ok(CheckedAdjustment {
            date,
            account,
            memo: (!d.memo.trim().is_empty()).then(|| d.memo.trim().to_string()),
            lines,
        }),
        _ => Err(problems),
    }
}

// ---- Physical count worksheet -----------------------------------------------------

/// One row of the worksheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountRow {
    /// The part.
    pub item: ItemId,
    /// Its name.
    pub name: String,
    /// Its unit label.
    pub unit_label: Option<String>,
    /// On hand as of the sheet's date (the engine's).
    pub on_hand: Quantity,
    /// What the owner counted, as typed (blank: not counted).
    pub counted: String,
    /// The unit counted in (U-161; `None`: the base unit, as on hand is).
    pub unit: Option<UnitId>,
    /// On hand changed since the owner started (after a 409 `STALE`
    /// reload): highlighted until the owner looks again.
    pub changed: bool,
}

/// The physical count worksheet (Q-P24g; N9 help text on the screen).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CountWorksheet {
    /// The count date.
    pub as_of: Option<CalendarDate>,
    /// Rows, in name order.
    pub rows: Vec<CountRow>,
    /// Where differences post.
    pub account: Option<AccountId>,
    /// Memo.
    pub memo: String,
}

/// Why a count cannot be posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CountProblem {
    /// No date.
    DateMissing,
    /// Dated on or before the closing date (owner 2026-11-06).
    ClosedPeriod,
    /// No adjustment account.
    AccountMissing,
    /// Nothing counted (Post is disabled).
    NothingCounted,
    /// A row's count is not a quantity of zero or more.
    CountInvalid(usize),
}

impl CountProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            CountProblem::DateMissing => "inventory.count.problem.date_missing",
            CountProblem::ClosedPeriod => "inventory.problem.closed_period",
            CountProblem::AccountMissing => "inventory.count.problem.account_missing",
            CountProblem::NothingCounted => "inventory.count.problem.nothing",
            CountProblem::CountInvalid(_) => "inventory.count.problem.count_invalid",
        })
    }
}

/// One counted line, ready for `cl_api::CountLine`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckedCountLine {
    /// The part.
    pub item: ItemId,
    /// On hand as the sheet showed it.
    pub sheet_on_hand: Quantity,
    /// Counted (in `unit`).
    pub counted: Quantity,
    /// The unit counted in (`None`: the base unit).
    pub unit: Option<UnitId>,
}

impl CountWorksheet {
    /// A worksheet over the engine's sheet rows `(item, name, unit label,
    /// on hand)`, sorted by name.
    #[must_use]
    pub fn new(
        as_of: CalendarDate,
        rows: impl IntoIterator<Item = (ItemId, String, Option<String>, Quantity)>,
    ) -> CountWorksheet {
        let mut rows: Vec<CountRow> = rows
            .into_iter()
            .map(|(item, name, unit_label, on_hand)| CountRow {
                item,
                name,
                unit_label,
                on_hand,
                counted: String::new(),
                unit: None,
                changed: false,
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        CountWorksheet {
            as_of: Some(as_of),
            rows,
            account: None,
            memo: String::new(),
        }
    }

    /// The difference a row would post (counted − on hand), when counted
    /// and valid; zero rows post nothing.
    #[must_use]
    pub fn difference(&self, i: usize) -> Option<Quantity> {
        let r = self.rows.get(i)?;
        // Counted in another unit: the engine converts it (Q-P24v).
        if r.unit.is_some() {
            return None;
        }
        let counted = parse_qty(&r.counted, QTY_RULES).ok().flatten()?;
        counted.checked_sub(r.on_hand).ok()
    }

    /// Whether Post is enabled: at least one row counted.
    #[must_use]
    pub fn can_post(&self) -> bool {
        self.rows.iter().any(|r| !r.counted.trim().is_empty())
    }

    /// After a 409 `STALE`: the reloaded sheet's on hand replaces the old,
    /// typed counts stay, and the rows whose on hand changed (or that the
    /// engine named) are marked.
    pub fn reload(
        &mut self,
        fresh: impl IntoIterator<Item = (ItemId, Quantity)>,
        stale: &[ItemId],
    ) {
        for (item, on_hand) in fresh {
            if let Some(r) = self.rows.iter_mut().find(|r| r.item == item) {
                r.changed = r.on_hand != on_hand || stale.contains(&item);
                r.on_hand = on_hand;
            }
        }
    }

    /// The counted lines.
    ///
    /// # Errors
    /// Every problem found.
    pub fn check(
        &self,
        closing_date: Option<CalendarDate>,
    ) -> Result<Vec<CheckedCountLine>, Vec<CountProblem>> {
        let mut problems = Vec::new();
        match self.as_of {
            None => problems.push(CountProblem::DateMissing),
            Some(d) if closing_date.is_some_and(|c| d <= c) => {
                problems.push(CountProblem::ClosedPeriod);
            }
            Some(_) => {}
        }
        if self.account.is_none() {
            problems.push(CountProblem::AccountMissing);
        }
        let mut lines = Vec::new();
        for (i, r) in self.rows.iter().enumerate() {
            match parse_qty(&r.counted, QTY_RULES) {
                Ok(Some(counted)) => lines.push(CheckedCountLine {
                    item: r.item,
                    sheet_on_hand: r.on_hand,
                    counted,
                    unit: r.unit,
                }),
                Ok(None) => {}
                Err(()) => problems.push(CountProblem::CountInvalid(i)),
            }
        }
        if lines.is_empty()
            && !problems
                .iter()
                .any(|p| matches!(p, CountProblem::CountInvalid(_)))
        {
            problems.push(CountProblem::NothingCounted);
        }
        if problems.is_empty() {
            Ok(lines)
        } else {
            Err(problems)
        }
    }
}

// ---- Items to Reorder ---------------------------------------------------------------

/// One part to reorder, as the engine reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReorderRow {
    /// The part.
    pub item: ItemId,
    /// Its name.
    pub name: String,
    /// On hand.
    pub on_hand: Quantity,
    /// Its reorder point.
    pub reorder_point: Quantity,
    /// How many to order (`None`: not set).
    pub reorder_quantity: Option<Quantity>,
    /// Usually bought from.
    pub preferred_vendor: Option<ContactId>,
    /// Average cost (shown).
    pub average_cost: Option<UnitCost>,
}

/// Items to Reorder: the engine's alerts grouped for ordering. Hidden on
/// Home when no part has a reorder point (plan §5 empty states).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReorderList {
    rows: Vec<ReorderRow>,
}

impl ReorderList {
    /// The list, by preferred vendor (parts without one last), then name.
    #[must_use]
    pub fn new(mut rows: Vec<ReorderRow>) -> ReorderList {
        rows.sort_by(|a, b| {
            (
                a.preferred_vendor.is_none(),
                a.preferred_vendor.map(|v| v.to_string()),
            )
                .cmp(&(
                    b.preferred_vendor.is_none(),
                    b.preferred_vendor.map(|v| v.to_string()),
                ))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        ReorderList { rows }
    }

    /// Rows in display order.
    #[must_use]
    pub fn rows(&self) -> &[ReorderRow] {
        &self.rows
    }

    /// How many parts to reorder (Home's count).
    #[must_use]
    pub fn count(&self) -> usize {
        self.rows.len()
    }

    /// The parts of one vendor (`None`: no preferred vendor), for one
    /// purchase list per vendor.
    pub fn for_vendor(&self, vendor: Option<ContactId>) -> impl Iterator<Item = &ReorderRow> {
        self.rows
            .iter()
            .filter(move |r| r.preferred_vendor == vendor)
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
