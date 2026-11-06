//! The demo engine's inventory routes (P24 WP-2): documents feed the
//! costing ledger of [`super::inventory`], and the adjustment, count,
//! report, reorder and switch routes answer as the engine does
//! (plan §2.4).
//!
//! - Only inventory-part lines move stock; several lines of one part on a
//!   document are one movement (S4).
//! - Closed periods (Q-P24b): a document with parts (or one that had them)
//!   dated on or before the closing date is refused with 409
//!   `INVENTORY_PERIOD_CLOSED` **before** the closing-date password is
//!   looked at, so the password never lets it through.
//! - Every figure (values, averages, on hand, the GL check) comes from the
//!   replay; nothing here is rounded except by the costing rules.
//! - The demo company starts with inventory off (Q-P24j). The first
//!   switch-on adds the sample parts and their opening stock (demo only)
//!   so the inventory screens have something to show.

use std::collections::BTreeMap;

use cl_api::{
    AdjustmentRecord, AdjustmentRecordLine, ApiError, CountResult, CountSheet, CountSheetRow, Item,
    ItemType, LocalValidation, NegativeStock, NewAdjustment, NewCount, ReorderAlert,
    SavedAdjustment, StockStatusRow, TxnStatus, TxnType, ValuationAccount, ValuationDetailRow,
    ValuationRow, ValuationSummary,
};
use cl_core::ids::{AccountId, ItemId, TransactionId, UnitId};
use cl_core::{CalendarDate, Money, Quantity, UnitCost};
use serde_json::json;

use super::DemoState;
use super::inventory::{DemoInventoryError, DocLine, MoveKind};

/// What a document line does to stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Bill, check or card charge: in at the line amount.
    Receipt,
    /// Vendor credit: out at the line amount (N1).
    VendorReturn,
    /// Invoice or sales receipt: out at cost.
    Sale,
    /// Credit memo: back in at the current average (no link, Q-P24e).
    CustomerReturn,
}

/// The company's closing date.
#[must_use]
pub fn closing(state: &DemoState) -> Option<CalendarDate> {
    super::home::closing_date(state)
}

fn part(state: &mut DemoState, id: ItemId) -> Option<Item> {
    state
        .items_mut()
        .iter()
        .find(|i| i.id == id && i.fields.item_type == ItemType::InventoryPart)
        .cloned()
}

fn cost_of(i: &Item) -> Option<UnitCost> {
    i.fields
        .purchase
        .purchase_cost
        .and_then(|m| UnitCost::parse_wire(&m.to_wire()).ok())
}

/// The stock movements of a document's lines `(item, quantity, amount)`:
/// inventory parts only, one per part (quantities and amounts summed; a
/// line without a quantity is one unit).
pub fn doc_lines(
    state: &mut DemoState,
    side: Side,
    lines: impl IntoIterator<Item = (Option<ItemId>, Option<Quantity>, Money)>,
) -> Vec<DocLine> {
    // No line names a unit: nothing can be refused.
    linked_lines(
        state,
        side,
        lines.into_iter().map(|(i, q, a)| (i, q, a, None, None)),
    )
    .unwrap_or_default()
}

/// [`doc_lines`] with each line's unit (U-161): quantities become base
/// units (`round4(quantity × factor)`).
///
/// # Errors
/// 400 `UNIT_NOT_IN_SET`, 400 `UNIT_QUANTITY_TOO_SMALL`.
pub fn unit_lines(
    state: &mut DemoState,
    side: Side,
    lines: impl IntoIterator<Item = (Option<ItemId>, Option<Quantity>, Money, Option<UnitId>)>,
) -> Result<Vec<DocLine>, ApiError> {
    linked_lines(
        state,
        side,
        lines.into_iter().map(|(i, q, a, u)| (i, q, a, None, u)),
    )
}

/// [`doc_lines`] with each line's linked sale (a credit memo's Return of,
/// Q-P24e/u): one movement per (part, linked sale).
///
/// # Errors
/// 400 `UNIT_NOT_IN_SET`, 400 `UNIT_QUANTITY_TOO_SMALL` (a line's unit).
pub fn linked_lines(
    state: &mut DemoState,
    side: Side,
    lines: impl IntoIterator<
        Item = (
            Option<ItemId>,
            Option<Quantity>,
            Money,
            Option<TransactionId>,
            Option<UnitId>,
        ),
    >,
) -> Result<Vec<DocLine>, ApiError> {
    type Sum = (ItemId, Quantity, Money, Option<UnitCost>, Option<String>);
    let mut sums: BTreeMap<(String, Option<String>), Sum> = BTreeMap::new();
    let one = Quantity::parse_wire("1").unwrap_or(Quantity::ZERO);
    for (item, qty, amount, sale, unit) in lines {
        let Some(id) = item else { continue };
        let Some(p) = part(state, id) else { continue };
        // Stock moves in base units (U-161); the unit is locked once used.
        let q = super::units::line_base(state, id, unit, qty.unwrap_or(one).abs())?;
        if let Some(u) = unit {
            state.units.mark_used(u);
        }
        let sale = sale
            .filter(|_| side == Side::CustomerReturn)
            .map(|t| t.to_string());
        let e = sums.entry((id.to_string(), sale.clone())).or_insert((
            id,
            Quantity::ZERO,
            Money::ZERO,
            cost_of(&p),
            sale,
        ));
        e.1 = e.1.checked_add(q).unwrap_or(e.1);
        e.2 = e.2.checked_add(amount.abs()).unwrap_or(e.2);
    }
    Ok(sums
        .into_values()
        .filter(|(_, q, _, _, _)| !q.is_zero())
        .map(
            |(item, quantity, amount, fallback_cost, return_of)| DocLine {
                item,
                kind: match side {
                    Side::Receipt => MoveKind::Receipt { quantity, amount },
                    Side::VendorReturn => MoveKind::VendorReturn { quantity, amount },
                    Side::Sale => MoveKind::Sale { quantity },
                    Side::CustomerReturn => MoveKind::CustomerReturn { quantity },
                },
                fallback_cost,
                return_of,
            },
        )
        .collect())
}

/// [`doc_lines`] over a sales document's lines (invoice, sales receipt,
/// credit memo): the amount does not matter for a sale or a return.
///
/// # Errors
/// 400 `UNIT_NOT_IN_SET`, 400 `UNIT_QUANTITY_TOO_SMALL`.
pub fn sales_lines(
    state: &mut DemoState,
    side: Side,
    lines: &[cl_api::InvoiceLine],
) -> Result<Vec<DocLine>, ApiError> {
    linked_lines(
        state,
        side,
        lines.iter().map(|l| {
            (
                l.item_id,
                l.quantity,
                Money::ZERO,
                l.return_of.flatten(),
                l.unit_id,
            )
        }),
    )
}

/// The engine's answer for a demo refusal.
#[must_use]
pub fn refusal(e: &DemoInventoryError, lock: Option<CalendarDate>) -> ApiError {
    match e {
        DemoInventoryError::PeriodClosed => cl_api::engine_refusal(
            409,
            &json!({"error": {
                "message": "Inventory in a closed period can't change.",
                "code": "INVENTORY_PERIOD_CLOSED",
                "details": {"lockDate": lock.map(CalendarDate::to_wire)},
            }}),
        ),
        DemoInventoryError::NoStock => cl_api::engine_refusal(
            400,
            &json!({"error": {"message": "Nothing on hand.", "code": "INVENTORY_NO_STOCK"}}),
        ),
        DemoInventoryError::Stale(items) => cl_api::engine_refusal(
            409,
            &json!({"error": {
                "message": "On hand changed.",
                "code": "STALE",
                "details": {"items": items.iter().map(ToString::to_string).collect::<Vec<_>>()},
            }}),
        ),
        DemoInventoryError::NotFound => ApiError::NotFound { code: None },
        DemoInventoryError::ReturnBeforeSale | DemoInventoryError::ReturnExceedsSale => {
            cl_api::engine_refusal(
                400,
                &json!({"error": {"message": "That return doesn't match the sale.", "code": e.code()}}),
            )
        }
        DemoInventoryError::SaleHasReturns => cl_api::engine_refusal(
            409,
            &json!({"error": {"message": "This sale has returns.", "code": e.code()}}),
        ),
        DemoInventoryError::Cost(_) => ApiError::Local(LocalValidation::InvalidValue),
    }
}

/// Inventory turned off (400 `INVENTORY_OFF`).
fn off() -> ApiError {
    cl_api::engine_refusal(
        400,
        &json!({"error": {"message": "Inventory is off.", "code": "INVENTORY_OFF"}}),
    )
}

/// The closed-period gate (Q-P24b), checked before the closing-date
/// password: a document with parts now, or with movements before, dated
/// (or moved from a date) on or before the closing date.
///
/// # Errors
/// 409 `INVENTORY_PERIOD_CLOSED`.
pub fn gate(
    state: &DemoState,
    doc: Option<TransactionId>,
    date: CalendarDate,
    lines: &[DocLine],
) -> Result<(), ApiError> {
    let old = doc.and_then(|d| state.inventory.date_of(&d.to_string()));
    if lines.is_empty() && old.is_none() {
        return Ok(());
    }
    let lock = closing(state);
    let closed = |d: CalendarDate| lock.is_some_and(|c| d <= c);
    if closed(date) || old.is_some_and(closed) {
        return Err(refusal(&DemoInventoryError::PeriodClosed, lock));
    }
    // Linked returns (U-158), before anything is saved.
    let key = doc.map_or_else(|| "new-document".to_string(), |d| d.to_string());
    state
        .inventory
        .check_write(&key, date, lines)
        .map_err(|e| refusal(&e, lock))
}

/// The gate for a void or delete of `doc`.
///
/// # Errors
/// 409 `INVENTORY_PERIOD_CLOSED`; 409 `SALE_HAS_RETURNS`.
pub fn gate_remove(state: &DemoState, doc: TransactionId) -> Result<(), ApiError> {
    // Q-P24x: a sale with linked returns can't be voided or deleted.
    let key = doc.to_string();
    if state
        .inventory
        .moves()
        .iter()
        .any(|m| m.return_of.as_deref() == Some(key.as_str()))
    {
        return Err(refusal(&DemoInventoryError::SaleHasReturns, closing(state)));
    }
    match state.inventory.date_of(&doc.to_string()) {
        Some(d) => gate(state, Some(doc), d, &[]),
        None => Ok(()),
    }
}

/// Posts (or re-posts) `doc`'s movements after the document itself was
/// saved; answers the `NEGATIVE_STOCK` warnings (kept for the screen too).
///
/// # Errors
/// As the engine: `INVENTORY_PERIOD_CLOSED`, `INVENTORY_NO_STOCK`.
pub fn post(
    state: &mut DemoState,
    doc: TransactionId,
    kind: TxnType,
    date: CalendarDate,
    lines: &[DocLine],
) -> Result<Vec<NegativeStock>, ApiError> {
    let lock = closing(state);
    state.inventory.closing_date = lock;
    let key = doc.to_string();
    let had = state.inventory.has(&key);
    let r = match (had, lines.is_empty()) {
        (false, true) => Ok(Vec::new()),
        (true, true) => state.inventory.remove(&key),
        (true, false) => state.inventory.edit(&key, date, lines),
        (false, false) => state.inventory.post(&key, date, lines),
    };
    let warnings = r.map_err(|e| refusal(&e, lock))?;
    if !lines.is_empty() {
        state.inventory.doc_types.insert(key, kind);
    }
    state.inventory.last_warnings.clone_from(&warnings);
    refresh(state);
    Ok(warnings)
}

/// Takes `doc`'s movements out (a void or delete); answers the warnings.
///
/// # Errors
/// `INVENTORY_PERIOD_CLOSED`.
pub fn remove(state: &mut DemoState, doc: TransactionId) -> Result<Vec<NegativeStock>, ApiError> {
    let key = doc.to_string();
    if !state.inventory.has(&key) {
        state.inventory.last_warnings.clear();
        return Ok(Vec::new());
    }
    let lock = closing(state);
    state.inventory.closing_date = lock;
    let warnings = state
        .inventory
        .remove(&key)
        .map_err(|e| refusal(&e, lock))?;
    state.inventory.last_warnings.clone_from(&warnings);
    refresh(state);
    Ok(warnings)
}

/// The warnings of the last write, once.
pub fn take_warnings(state: &mut DemoState) -> Vec<NegativeStock> {
    std::mem::take(&mut state.inventory.last_warnings)
}

/// Every part's on hand, average and value as the items list reads them.
pub fn refresh(state: &mut DemoState) {
    let ids: Vec<ItemId> = state
        .items_mut()
        .iter()
        .filter(|i| i.fields.item_type == ItemType::InventoryPart)
        .map(|i| i.id)
        .collect();
    let mut stocks = Vec::new();
    for id in ids {
        if let Ok(s) = state.inventory.stock(id) {
            stocks.push((id, s));
        }
    }
    for (id, s) in stocks {
        if let Some(i) = state.items_mut().iter_mut().find(|i| i.id == id) {
            i.stock = s;
        }
    }
    refresh_unit_prices(state);
}

/// Every item's `unitPrices[]` from its unit set (U-161, engine-computed
/// `round4(price × factor)`).
pub fn refresh_unit_prices(state: &mut DemoState) {
    let units = state.units.clone();
    for i in state.items_mut().iter_mut() {
        i.stock.unit_prices = i
            .fields
            .purchase
            .unit_set_id
            .map(|set| units.unit_prices(set, i.fields.unit_price))
            .unwrap_or_default();
    }
}

// ---- The switch ---------------------------------------------------------------

fn find_account(state: &mut DemoState, tag: &str) -> Option<AccountId> {
    state
        .chart
        .get_or_insert_with(super::chart)
        .iter()
        .find(|a| a.system_tag.as_deref() == Some(tag) && a.is_active)
        .map(|a| a.id)
}

fn add_account(
    state: &mut DemoState,
    number: u32,
    name: &str,
    kind: cl_api::AccountType,
    detail: &str,
    tag: &str,
) -> AccountId {
    if let Some(id) = find_account(state, tag) {
        return id;
    }
    let id = super::account_id(number);
    state
        .chart
        .get_or_insert_with(super::chart)
        .push(cl_api::Account {
            id,
            fields: cl_api::AccountFields {
                name: name.to_string(),
                account_number: Some(number.to_string()),
                account_type: kind,
                detail_type: Some(detail.to_string()),
                description: None,
                parent_id: None,
            },
            is_active: true,
            is_system: true,
            system_tag: Some(tag.to_string()),
            balance: Some(Money::ZERO),
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
            card_last4: None,
        });
    id
}

/// The inventory asset account (U-156: adopted or created on switch-on).
pub fn asset_account(state: &mut DemoState) -> AccountId {
    add_account(
        state,
        1300,
        "Inventory Asset",
        cl_api::AccountType::Asset,
        "other_current_asset",
        "inventory_asset",
    )
}

/// The default cost of goods sold account.
pub fn cogs_account(state: &mut DemoState) -> AccountId {
    add_account(
        state,
        5050,
        "Cost of Goods Sold",
        cl_api::AccountType::Cogs,
        "cost_of_goods_sold",
        "cost_of_goods_sold",
    )
}

/// The opening balance equity account (Q-P24h: created if missing).
pub fn opening_account(state: &mut DemoState) -> AccountId {
    add_account(
        state,
        30000,
        "Opening Balance Equity",
        cl_api::AccountType::Equity,
        "opening_balance_equity",
        "opening_balance",
    )
}

fn on_hand_total(state: &DemoState) -> bool {
    state.inventory.rows().map_or(true, |rows| {
        let mut last: BTreeMap<String, (Quantity, Money)> = BTreeMap::new();
        for r in rows {
            last.insert(r.item.to_string(), (r.on_hand, r.asset_value));
        }
        last.values().any(|(q, v)| !q.is_zero() || !v.is_zero())
    })
}

/// `PUT /company/settings {inventoryEnabled}`: on adopts the accounts (and,
/// the first time, adds the sample parts); off is refused while stock or
/// value remains (409 `INVENTORY_IN_USE`, Q-P24j).
///
/// # Errors
/// 409 `INVENTORY_IN_USE`.
pub fn set_enabled(state: &mut DemoState, on: bool) -> Result<(), ApiError> {
    if on == state.inventory.enabled {
        return Ok(());
    }
    if !on {
        if on_hand_total(state) {
            return Err(cl_api::engine_refusal(
                409,
                &json!({"error": {"message": "Stock remains.", "code": "INVENTORY_IN_USE"}}),
            ));
        }
        state.inventory.enabled = false;
        return Ok(());
    }
    state.inventory.enabled = true;
    asset_account(state);
    cogs_account(state);
    if !state.inventory.seeded {
        state.inventory.seeded = true;
        seed(state);
    }
    Ok(())
}

/// The sample parts (synthetic): name, description, sales price, cost,
/// reorder point, reorder quantity, unit, opening quantity, opening value,
/// preferred vendor.
type SamplePart = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const SAMPLE_PARTS: [SamplePart; 3] = [
    (
        "Hardwood Mulch Bag",
        "2 cu ft bag, shredded hardwood",
        "6.5000",
        "3.2500",
        "40",
        "200",
        "bag",
        "120",
        "390.00",
        "Ozark Mulch Supply",
    ),
    (
        "Landscape Fabric Roll",
        "3 ft x 50 ft, woven",
        "34.0000",
        "18.4000",
        "10",
        "24",
        "roll",
        "6",
        "110.40",
        "Ozark Mulch Supply",
    ),
    (
        "Sprinkler Head",
        "4 in pop-up spray head",
        "12.0000",
        "4.7500",
        "25",
        "100",
        "each",
        "60",
        "285.00",
        "Midwest Irrigation Parts",
    ),
];

/// The first part's demo id (the parts follow it).
pub const FIRST_PART: u32 = 800;

fn seed(state: &mut DemoState) {
    let asset = asset_account(state);
    let cogs = cogs_account(state);
    let opening = opening_account(state);
    let as_of = CalendarDate::from_ymd(2026, 10, 1).unwrap_or_else(|_| DemoState::today());
    for (n, p) in SAMPLE_PARTS.iter().enumerate() {
        let (name, desc, price, cost, point, reorder, unit, qty, value, vendor) = *p;
        let vendor = state
            .contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == vendor)
            .map(|c| c.id);
        let id = super::lists::item_id(FIRST_PART + u32::try_from(n).unwrap_or(0));
        let q = |s: &str| Quantity::parse_wire(s).ok();
        let m = |s: &str| Money::parse_wire(s).ok();
        let item = Item {
            id,
            fields: cl_api::ItemFields {
                name: name.into(),
                description: Some(desc.into()),
                unit_price: m(price),
                income_account_id: super::account_id(4000),
                is_taxable: true,
                item_type: ItemType::InventoryPart,
                discount_percent: None,
                tax_code_id: None,
                purchase: cl_api::ItemPurchase {
                    purchase_description: Some(desc.into()),
                    purchase_cost: m(cost),
                    expense_account_id: None,
                    asset_account_id: Some(asset),
                    cogs_account_id: Some(cogs),
                    preferred_vendor_id: vendor,
                    reorder_point: q(point),
                    reorder_quantity: q(reorder),
                    unit_label: Some(unit.into()),
                    unit_set_id: None,
                    default_purchase_unit_id: None,
                    default_sale_unit_id: None,
                },
            },
            is_active: true,
            default_tag_id: None,
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
            stock: cl_api::ItemStock::default(),
        };
        state.items_mut().push(item);
        if let (Some(quantity), Some(value)) = (q(qty), m(value)) {
            let _ = opening_stock(state, id, quantity, value, as_of, opening);
        }
    }
    refresh(state);
}

/// A new part's opening stock (Q-P24h): an adjustment of both quantity
/// and value against opening balance equity.
///
/// # Errors
/// `INVENTORY_PERIOD_CLOSED`, `INVENTORY_OFF`.
pub fn opening(
    state: &mut DemoState,
    item: ItemId,
    o: &cl_api::OpeningStock,
) -> Result<(), ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    let account = opening_account(state);
    opening_stock(state, item, o.quantity, o.value, o.as_of, account).map(|_| ())
}

fn opening_stock(
    state: &mut DemoState,
    item: ItemId,
    quantity: Quantity,
    value: Money,
    as_of: CalendarDate,
    account: AccountId,
) -> Result<SavedAdjustment, ApiError> {
    let a = NewAdjustment {
        date: as_of,
        adjustment_account_id: account,
        memo: None,
        lines: vec![cl_api::AdjustmentLine {
            item_id: item,
            quantity_change: Some(quantity),
            value_change: Some(value),
            unit_id: None,
        }],
    };
    create_adjustment(state, &a)
}

/// The developer story `dev.demo_inventory_sale`: a sale of two of the
/// first sample part on the story date, as if entered in another window.
///
/// # Errors
/// As [`post`].
pub fn story_sale(state: &mut DemoState) -> Result<Vec<NegativeStock>, ApiError> {
    let n = state.inventory.doc_types.len() + 1;
    let id = TransactionId::parse(&format!("d8100000-0000-4000-8000-{n:012}"))
        .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    let item = super::lists::item_id(FIRST_PART);
    let lines = doc_lines(
        state,
        Side::Sale,
        [(Some(item), Quantity::parse_wire("2").ok(), Money::ZERO)],
    );
    post(state, id, TxnType::Invoice, DemoState::today(), &lines)
}

// ---- Adjustments and counts ----------------------------------------------------

fn next_adjustment_id(state: &DemoState) -> Result<TransactionId, ApiError> {
    let n = state.inventory.adjustments.len() + 1;
    TransactionId::parse(&format!("d8000000-0000-4000-8000-{n:012}"))
        .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))
}

/// A change entered in a unit as base units: `round4(|q| × factor)` with
/// its sign (U-161, Q-P24v).
fn base_change(
    state: &mut DemoState,
    item: ItemId,
    unit: Option<UnitId>,
    q: Quantity,
) -> Result<Quantity, ApiError> {
    if unit.is_none() {
        return Ok(q);
    }
    let b = super::units::line_base(state, item, unit, q.abs())?;
    if let Some(u) = unit {
        state.units.mark_used(u);
    }
    if q.is_negative() {
        b.checked_neg()
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))
    } else {
        Ok(b)
    }
}

fn adjust_lines(state: &mut DemoState, a: &NewAdjustment) -> Result<Vec<DocLine>, ApiError> {
    let mut out = Vec::with_capacity(a.lines.len());
    for l in &a.lines {
        let change = match l.quantity_change {
            Some(q) => Some(base_change(state, l.item_id, l.unit_id, q)?),
            None => None,
        };
        out.push((l, change));
    }
    Ok(out
        .into_iter()
        .map(|(l, change)| DocLine {
            item: l.item_id,
            kind: match (change, l.value_change) {
                (Some(change), Some(value)) => MoveKind::AdjustBoth { change, value },
                (Some(change), None) => MoveKind::AdjustQuantity { change },
                (None, Some(change)) => MoveKind::AdjustValue { change },
                (None, None) => MoveKind::AdjustValue {
                    change: Money::ZERO,
                },
            },
            fallback_cost: None,
            return_of: None,
        })
        .collect())
}

/// The record of adjustment `id` with the values the replay posted.
fn record(
    state: &DemoState,
    id: TransactionId,
    a: &NewAdjustment,
    status: TxnStatus,
) -> AdjustmentRecord {
    let rows = state.inventory.rows().unwrap_or_default();
    let key = id.to_string();
    AdjustmentRecord {
        id,
        txn_type: TxnType::InventoryAdjustment,
        date: a.date,
        adjustment_account_id: Some(a.adjustment_account_id),
        memo: a.memo.clone(),
        lines: a
            .lines
            .iter()
            .map(|l| AdjustmentRecordLine {
                item_id: l.item_id,
                quantity_change: l.quantity_change,
                value_change: l.value_change,
                value: rows
                    .iter()
                    .find(|r| r.doc == key && r.item == l.item_id)
                    .map(|r| r.value),
            })
            .collect(),
        inventory_lines: Vec::new(),
        status,
        updated_at: Some(DemoState::now()),
    }
}

/// `POST /inventory/adjustments`.
///
/// # Errors
/// `INVENTORY_OFF`, `INVENTORY_PERIOD_CLOSED`, `INVENTORY_NO_STOCK`,
/// `Local(…)` from the request's own check.
pub fn create_adjustment(
    state: &mut DemoState,
    a: &NewAdjustment,
) -> Result<SavedAdjustment, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    a.check()?;
    let lines = adjust_lines(state, a)?;
    gate(state, None, a.date, &lines)?;
    let id = next_adjustment_id(state)?;
    let warnings = post(state, id, TxnType::InventoryAdjustment, a.date, &lines)?;
    let rec = record(state, id, a, TxnStatus::Posted);
    state.inventory.adjustments.push(rec.clone());
    Ok(SavedAdjustment {
        adjustment: rec,
        warnings,
    })
}

/// `GET /inventory/adjustments/{id}`.
///
/// # Errors
/// `NotFound`.
pub fn adjustment(state: &DemoState, id: TransactionId) -> Result<AdjustmentRecord, ApiError> {
    state
        .inventory
        .adjustments
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })
}

/// `POST /inventory/adjustments/{id}/void`.
///
/// # Errors
/// `NotFound`, `INVENTORY_PERIOD_CLOSED`.
pub fn void_adjustment(
    state: &mut DemoState,
    id: TransactionId,
) -> Result<Vec<NegativeStock>, ApiError> {
    adjustment(state, id)?;
    gate_remove(state, id)?;
    let w = remove(state, id)?;
    if let Some(a) = state.inventory.adjustments.iter_mut().find(|a| a.id == id) {
        a.status = TxnStatus::Void;
    }
    Ok(w)
}

fn on_hand_as_of(state: &DemoState, item: ItemId, as_of: CalendarDate) -> cl_api::ItemStock {
    state
        .inventory
        .stock_as_of(item, Some(as_of))
        .unwrap_or_default()
}

fn parts(state: &mut DemoState, active_only: bool) -> Vec<Item> {
    let mut v: Vec<Item> = state
        .items_mut()
        .iter()
        .filter(|i| i.fields.item_type == ItemType::InventoryPart && (i.is_active || !active_only))
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        a.fields
            .name
            .to_lowercase()
            .cmp(&b.fields.name.to_lowercase())
    });
    v
}

/// `GET /inventory/count-sheet?asOf=`.
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn count_sheet(state: &mut DemoState, as_of: CalendarDate) -> Result<CountSheet, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    let rows = parts(state, true)
        .into_iter()
        .map(|i| {
            let s = on_hand_as_of(state, i.id, as_of);
            CountSheetRow {
                item_id: i.id,
                name: Some(i.fields.name.clone()),
                description: i.fields.description.clone(),
                preferred_vendor_id: i.fields.purchase.preferred_vendor_id,
                // U-161: a part with a set counts in its base unit.
                unit_label: i
                    .fields
                    .purchase
                    .unit_set_id
                    .and_then(|set| state.units.get(set))
                    .map(|set| set.base_unit_name)
                    .or_else(|| i.fields.purchase.unit_label.clone()),
                on_hand: s.quantity_on_hand.unwrap_or(Quantity::ZERO),
                average_cost: s.average_cost,
                value: s.asset_value,
            }
        })
        .collect();
    Ok(CountSheet { as_of, rows })
}

/// `POST /inventory/counts` (Q-P24g).
///
/// # Errors
/// `INVENTORY_OFF`, `STALE` (listing the items), `INVENTORY_PERIOD_CLOSED`.
pub fn post_count(state: &mut DemoState, c: &NewCount) -> Result<CountResult, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    c.check()?;
    let lock = closing(state);
    state.inventory.closing_date = lock;
    let id = next_adjustment_id(state)?;
    // Counted in a unit: compared with on hand in base units (U-161).
    let mut base_lines = Vec::with_capacity(c.lines.len());
    for l in &c.lines {
        base_lines.push(cl_api::CountLine {
            counted_quantity: base_change(state, l.item_id, l.unit_id, l.counted_quantity)?,
            unit_id: None,
            ..l.clone()
        });
    }
    let (lines, warnings) = state
        .inventory
        .post_count(&id.to_string(), c.as_of, &base_lines)
        .map_err(|e| refusal(&e, lock))?;
    if lines.is_empty() {
        return Ok(CountResult {
            adjustment: None,
            warnings,
        });
    }
    state
        .inventory
        .doc_types
        .insert(id.to_string(), TxnType::InventoryAdjustment);
    let a = NewAdjustment {
        date: c.as_of,
        adjustment_account_id: c.adjustment_account_id,
        memo: c.memo.clone(),
        lines: lines
            .iter()
            .filter_map(|l| match l.kind {
                MoveKind::AdjustQuantity { change } => Some(cl_api::AdjustmentLine {
                    item_id: l.item,
                    quantity_change: Some(change),
                    value_change: None,
                    unit_id: None,
                }),
                _ => None,
            })
            .collect(),
    };
    let rec = record(state, id, &a, TxnStatus::Posted);
    state.inventory.adjustments.push(rec.clone());
    state.inventory.last_warnings.clone_from(&warnings);
    refresh(state);
    Ok(CountResult {
        adjustment: Some(rec),
        warnings,
    })
}

// ---- Reports -------------------------------------------------------------------

/// `GET /reports/inventory-valuation?asOf=` (U-160).
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn valuation(state: &mut DemoState, as_of: CalendarDate) -> Result<ValuationSummary, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    let mut rows = Vec::new();
    let mut by_account: BTreeMap<String, (AccountId, Money)> = BTreeMap::new();
    let mut total = Money::ZERO;
    for i in parts(state, false) {
        let s = on_hand_as_of(state, i.id, as_of);
        let on_hand = s.quantity_on_hand.unwrap_or(Quantity::ZERO);
        let value = s.asset_value.unwrap_or(Money::ZERO);
        if on_hand.is_zero() && value.is_zero() && !i.is_active {
            continue;
        }
        total = total.checked_add(value).unwrap_or(total);
        if let Some(a) = i.fields.purchase.asset_account_id {
            let e = by_account.entry(a.to_string()).or_insert((a, Money::ZERO));
            e.1 = e.1.checked_add(value).unwrap_or(e.1);
        }
        rows.push(ValuationRow {
            item_id: i.id,
            name: Some(i.fields.name.clone()),
            asset_account_id: i.fields.purchase.asset_account_id,
            on_hand,
            average_cost: s.average_cost,
            value,
            percent_of_total: None,
        });
    }
    // The demo's ledger holds exactly the costing's values on the asset
    // account (no journal entry posts to it in the demo).
    let accounts = by_account
        .into_values()
        .map(|(account_id, valuation)| ValuationAccount {
            account_id,
            valuation,
            gl_balance: valuation,
            difference: Money::ZERO,
        })
        .collect();
    // The engine's percentOfTotal: 2 places, half up, none when the total is 0.
    if !total.is_zero() {
        for r in &mut rows {
            let mut pct = (r.value.as_decimal() / total.as_decimal()
                * rust_decimal::Decimal::ONE_HUNDRED)
                .round_dp_with_strategy(2, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
            pct.rescale(2);
            r.percent_of_total = Some(pct.to_string());
        }
    }
    Ok(ValuationSummary {
        as_of,
        rows,
        accounts,
        total_value: Some(total),
        basis_note: None,
    })
}

/// `GET /reports/inventory-valuation-detail?from&to&itemId` (U-160).
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn valuation_detail(
    state: &mut DemoState,
    from: CalendarDate,
    to: CalendarDate,
    item: Option<ItemId>,
) -> Result<Vec<ValuationDetailRow>, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    let rows = state
        .inventory
        .rows()
        .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    let mut out = Vec::new();
    let mut prev: BTreeMap<String, Quantity> = BTreeMap::new();
    for r in rows {
        let before = prev
            .insert(r.item.to_string(), r.on_hand)
            .unwrap_or(Quantity::ZERO);
        if r.date < from || r.date > to || item.is_some_and(|i| i != r.item) {
            continue;
        }
        let Ok(txn) = TransactionId::parse(&r.doc) else {
            continue;
        };
        let quantity = r.on_hand.checked_sub(before).unwrap_or(Quantity::ZERO);
        let value = r
            .value
            .checked_sub(r.correction)
            .and_then(|v| v.checked_sub(r.residual))
            .unwrap_or(r.value);
        out.push(ValuationDetailRow {
            item_id: r.item,
            date: r.date,
            transaction_id: txn,
            txn_type: state.inventory.doc_types.get(&r.doc).copied(),
            kind: None,
            quantity,
            value,
            running_quantity: r.on_hand,
            running_value: r.asset_value,
            average_cost: r.last_cost,
        });
    }
    Ok(out)
}

/// `GET /reports/inventory-stock-status?asOf=` (U-160).
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn stock_status(
    state: &mut DemoState,
    as_of: CalendarDate,
) -> Result<Vec<StockStatusRow>, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    Ok(parts(state, true)
        .into_iter()
        .map(|i| {
            let on_hand = on_hand_as_of(state, i.id, as_of)
                .quantity_on_hand
                .unwrap_or(Quantity::ZERO);
            let p = &i.fields.purchase;
            StockStatusRow {
                item_id: i.id,
                name: Some(i.fields.name.clone()),
                description: i.fields.description.clone(),
                on_hand,
                reorder: Some(p.reorder_point.is_some_and(|r| on_hand <= r)),
                reorder_point: p.reorder_point,
                reorder_quantity: p.reorder_quantity,
                preferred_vendor_id: p.preferred_vendor_id,
                unit_label: p.unit_label.clone(),
                base_unit: p
                    .unit_set_id
                    .and_then(|s| state.units.get(s))
                    .map(|s| s.base_unit_name),
                purchase_unit: None,
                reorder_quantity_in_purchase_unit: None,
            }
        })
        .collect())
}

/// `GET /items/reorder-alerts` (U-78): active parts at or below their
/// reorder point, on hand after every posting.
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn reorder_alerts(state: &mut DemoState) -> Result<Vec<ReorderAlert>, ApiError> {
    if !state.inventory.enabled {
        return Err(off());
    }
    let far = CalendarDate::from_ymd(9999, 12, 31).unwrap_or_else(|_| DemoState::today());
    Ok(stock_status(state, far)?
        .into_iter()
        .filter(|r| r.reorder == Some(true))
        .collect())
}

/// `GET /reports/reorder` (U-78, U-161): the alerts with each part's
/// default purchase unit and the reorder quantity in it,
/// `round4(reorderQuantity / factor)` half away from zero.
///
/// # Errors
/// `INVENTORY_OFF`.
pub fn reorder_report(state: &mut DemoState) -> Result<Vec<ReorderAlert>, ApiError> {
    let mut rows = reorder_alerts(state)?;
    for r in &mut rows {
        let Some(item) = state
            .items_mut()
            .iter()
            .find(|i| i.id == r.item_id)
            .cloned()
        else {
            continue;
        };
        let p = &item.fields.purchase;
        let (Some(set), Some(unit)) = (p.unit_set_id, p.default_purchase_unit_id) else {
            continue;
        };
        let Some(u) = state
            .units
            .get(set)
            .and_then(|s| s.units.into_iter().find(|u| u.id == Some(unit)))
        else {
            continue;
        };
        r.reorder_quantity_in_purchase_unit = r.reorder_quantity.and_then(|q| {
            let d = q
                .as_decimal()
                .checked_div(u.factor.as_decimal())?
                .round_dp_with_strategy(4, rust_decimal::RoundingStrategy::MidpointAwayFromZero);
            Quantity::parse_wire(&d.to_string()).ok()
        });
        r.purchase_unit = Some(cl_api::PurchaseUnit {
            id: unit,
            name: u.name,
            abbreviation: u.abbreviation,
            factor: u.factor,
        });
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "stock_tests.rs"]
mod tests;
