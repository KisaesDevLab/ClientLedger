//! Inventory (P24, engine U-159, U-160, U-78; contract per the P24 plan
//! §2.4, client-declared until the WP-2 pin bump).
//!
//! - Adjustments move quantity, value or both for one or more parts against
//!   an adjustment account (`inventory-adjust-v1`). Keyed under
//!   `inventory-adjust-v1`. Anything dated on or before the closing
//!   date is refused with 409 `INVENTORY_PERIOD_CLOSED`, even with the
//!   closing-date password (owner 2026-11-06; nothing is restated).
//! - The count sheet lists on hand as of a date; posting a count sends the
//!   sheet's figure with each counted quantity, and the engine refuses with
//!   409 `STALE` (listing the items) when on hand moved since (Q-P24g).
//! - Reports (`inventory-reports-v1`) and reorder alerts
//!   (`reorder-alerts-v1`) are the engine's strings; the desktop never adds
//!   them up, subtracts them or derives a balance (plan §4).
//! - `NEGATIVE_STOCK` is a warning, never a refusal (Q-148): it comes in a
//!   preview's `problems[]` and in the `warnings[]` of a create, edit, void
//!   or delete reply ([`Problem`]).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, ItemId, TransactionId};
use cl_core::{CalendarDate, Money, Quantity, Timestamp, UnitCost};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, null_vec, require, too_long};
use crate::resources::operations::{
    INVENTORY_ADJUSTMENT_CREATE, INVENTORY_ADJUSTMENT_DELETE, INVENTORY_ADJUSTMENT_GET,
    INVENTORY_ADJUSTMENT_PUT, INVENTORY_ADJUSTMENT_VOID, INVENTORY_COUNT_POST,
    INVENTORY_COUNT_SHEET, ITEMS_REORDER_ALERTS, REPORT_INVENTORY_STOCK_STATUS,
    REPORT_INVENTORY_VALUATION, REPORT_INVENTORY_VALUATION_DETAIL, REPORT_REORDER,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::sales_tax::{decode_list, decode_one};
use crate::resources::transactions::{InventoryLine, TxnStatus, TxnType};
use crate::session::Session;
use crate::wire::Capabilities;

/// The most lines of one adjustment (one per part).
pub const ADJUSTMENT_MAX_LINES: usize = 500;

/// The longest adjustment memo kept.
pub const ADJUSTMENT_MEMO_MAX: usize = 4000;

// ---- Refusals ---------------------------------------------------------------

/// The engine's inventory refusals (plan §2.3, §2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InventoryRefusal {
    /// Inventory is off for the company (400 `INVENTORY_OFF`).
    Off,
    /// Turning inventory off while stock or value remains (409
    /// `INVENTORY_IN_USE`).
    InUse,
    /// A value-only adjustment with nothing on hand at its date (400
    /// `INVENTORY_NO_STOCK`, Q-P24s).
    NoStock,
    /// Converting an item to or from a part with posted lines (409
    /// `ITEM_IN_USE`, Q-P24r): make a new part instead.
    ItemInUse,
    /// Changing a part's asset account once it has movements (409
    /// `ITEM_HAS_INVENTORY`, Q-P24k).
    ItemHasInventory,
    /// A bulk account move or a memorized template of a document with parts
    /// (409 `INVENTORY_DOCUMENT`, Q-P24q).
    Document,
    /// A plain line on an inventory asset account outside a journal entry
    /// (400 `INVENTORY_ACCOUNT_DIRECT`): use an item line.
    AccountDirect,
    /// Adding, editing, voiding or deleting a document with inventory (or
    /// an adjustment) dated on or before the closing date (409
    /// `INVENTORY_PERIOD_CLOSED`; owner 2026-11-06: refused even with the
    /// closing-date password, never restated).
    PeriodClosed,
    /// Deactivating, deleting or merging an account parts use (409
    /// `ACCOUNT_IN_USE`).
    AccountInUse,
    /// A credit memo line linked to a sale dated after it (400
    /// `RETURN_BEFORE_SALE`, U-158).
    ReturnBeforeSale,
    /// The returns linked to a sale would be more than it sold (400
    /// `RETURN_EXCEEDS_SALE`, U-158).
    ReturnExceedsSale,
    /// Voiding, deleting or changing a sale that has linked returns (409
    /// `SALE_HAS_RETURNS`, Q-P24x): void or unlink the credit memo first.
    SaleHasReturns,
}

const REFUSALS: &[(&str, InventoryRefusal)] = &[
    ("INVENTORY_OFF", InventoryRefusal::Off),
    ("INVENTORY_IN_USE", InventoryRefusal::InUse),
    ("INVENTORY_NO_STOCK", InventoryRefusal::NoStock),
    ("ITEM_IN_USE", InventoryRefusal::ItemInUse),
    ("ITEM_HAS_INVENTORY", InventoryRefusal::ItemHasInventory),
    ("INVENTORY_DOCUMENT", InventoryRefusal::Document),
    ("INVENTORY_ACCOUNT_DIRECT", InventoryRefusal::AccountDirect),
    ("INVENTORY_PERIOD_CLOSED", InventoryRefusal::PeriodClosed),
    ("ACCOUNT_IN_USE", InventoryRefusal::AccountInUse),
    ("RETURN_BEFORE_SALE", InventoryRefusal::ReturnBeforeSale),
    ("RETURN_EXCEEDS_SALE", InventoryRefusal::ReturnExceedsSale),
    ("SALE_HAS_RETURNS", InventoryRefusal::SaleHasReturns),
];

/// Every engine code [`inventory_refusal`] knows.
#[must_use]
pub fn inventory_refusal_codes() -> Vec<&'static str> {
    REFUSALS.iter().map(|(c, _)| *c).collect()
}

/// Classifies `e` (`None` for anything else).
#[must_use]
pub fn inventory_refusal(e: &ApiError) -> Option<InventoryRefusal> {
    let code = error_code(e)?;
    REFUSALS.iter().find(|(c, _)| *c == code).map(|(_, r)| *r)
}

/// The items whose on hand moved since the count sheet was read, from a 409
/// `STALE` on posting a count (Q-P24g: reload and highlight them). `None`
/// for any other error.
#[must_use]
pub fn count_stale_items(e: &ApiError) -> Option<Vec<ItemId>> {
    match e {
        ApiError::Conflict { details, .. } if error_code(e) == Some("STALE") => {
            Some(details.ids("items"))
        }
        _ => None,
    }
}

// ---- Problems and warnings -----------------------------------------------------

/// `NEGATIVE_STOCK` (plan §2.2): the first row per item whose running
/// on hand newly goes below zero. Posting is allowed (Q-148); the cost is
/// corrected when more is received.
#[derive(Clone, PartialEq, Eq)]
pub struct NegativeStock {
    /// The part.
    pub item_id: ItemId,
    /// The date on hand goes below zero.
    pub date: CalendarDate,
    /// On hand after that row (below zero).
    pub on_hand: Quantity,
}

impl fmt::Debug for NegativeStock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NegativeStock")
            .field("item_id", &self.item_id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One entry of a preview's `problems[]` or a reply's `warnings[]` as the
/// inventory screens read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// `NEGATIVE_STOCK {itemId, date, onHand}` (flat or under `details`).
    NegativeStock(NegativeStock),
    /// Any other code (kept for the caller; a malformed `NEGATIVE_STOCK`
    /// reads here too, never guessed).
    Other(String),
}

impl Problem {
    /// The engine code.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Problem::NegativeStock(_) => "NEGATIVE_STOCK",
            Problem::Other(c) => c,
        }
    }

    /// Reads one entry (`None` without a `code`).
    #[must_use]
    pub fn from_value(v: &Value) -> Option<Problem> {
        let code = v.get("code")?.as_str()?;
        if code != "NEGATIVE_STOCK" {
            return Some(Problem::Other(code.to_string()));
        }
        Some(
            negative_stock(v)
                .map_or_else(|| Problem::Other(code.to_string()), Problem::NegativeStock),
        )
    }
}

/// The fields of a `NEGATIVE_STOCK` entry, flat or under `details`.
fn negative_stock(v: &Value) -> Option<NegativeStock> {
    let field = |k: &str| {
        v.get(k)
            .or_else(|| v.get("details").and_then(|d| d.get(k)))
            .and_then(Value::as_str)
    };
    Some(NegativeStock {
        item_id: field("itemId")?.parse().ok()?,
        date: CalendarDate::parse_wire(field("date")?).ok()?,
        on_hand: Quantity::parse_wire(field("onHand")?).ok()?,
    })
}

/// The problems under `key` (`warnings` or `problems`) of a reply body;
/// none when absent or `null`.
#[must_use]
pub fn problems_in(body: &Value, key: &str) -> Vec<Problem> {
    body.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Problem::from_value).collect())
        .unwrap_or_default()
}

/// The `NEGATIVE_STOCK` warnings of a document reply (create, edit, void
/// or delete; `warnings[]` and, on a preview, `problems[]`), each item
/// once (its earliest date).
#[must_use]
pub fn stock_warnings(body: &Value) -> Vec<NegativeStock> {
    let mut out: Vec<NegativeStock> = Vec::new();
    for p in problems_in(body, "warnings")
        .into_iter()
        .chain(problems_in(body, "problems"))
    {
        if let Problem::NegativeStock(n) = p {
            match out.iter_mut().find(|o| o.item_id == n.item_id) {
                Some(o) if n.date < o.date => *o = n,
                Some(_) => {}
                None => out.push(n),
            }
        }
    }
    out
}

/// The same, from raw reply bytes (none when unreadable).
#[must_use]
pub fn stock_warnings_from(body: &[u8]) -> Vec<NegativeStock> {
    serde_json::from_slice::<Value>(body)
        .map(|v| stock_warnings(&v))
        .unwrap_or_default()
}

// ---- Adjustments ---------------------------------------------------------------

/// One part's change: a quantity (value at average cost, engine-derived), a
/// value (whole cents, user-entered), or both (a quantity at a given value).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustmentLine {
    /// The part.
    pub item_id: ItemId,
    /// Signed units (`parseQuantityChange`: non-zero, `|q| < 1e8`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity_change: Option<Quantity>,
    /// Signed value in whole cents.
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "crate::resources::customer_payments::ser_cents_opt"
    )]
    pub value_change: Option<Money>,
}

impl fmt::Debug for AdjustmentLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdjustmentLine")
            .field("item_id", &self.item_id)
            .field("quantity", &self.quantity_change.is_some())
            .field("value", &self.value_change.is_some())
            .finish()
    }
}

/// `POST /inventory/adjustments` (and the body of an edit).
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAdjustment {
    /// The adjustment date.
    pub date: CalendarDate,
    /// Where the other side posts (shrinkage, a write-down account).
    pub adjustment_account_id: AccountId,
    /// Memo (owner data).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// One line per part.
    pub lines: Vec<AdjustmentLine>,
}

impl fmt::Debug for NewAdjustment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewAdjustment")
            .field("date", &self.date)
            .field("lines", &self.lines)
            .finish_non_exhaustive()
    }
}

impl NewAdjustment {
    /// Checks the request before anything is sent (the engine stays
    /// authoritative).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no lines or more than
    /// [`ADJUSTMENT_MAX_LINES`], a part twice, a line with neither change, a
    /// zero quantity, a zero value or a fraction of a cent, a long memo.
    pub fn check(&self) -> Result<(), ApiError> {
        let bad = || Err(ApiError::Local(LocalValidation::InvalidValue));
        if self.lines.is_empty() || self.lines.len() > ADJUSTMENT_MAX_LINES {
            return bad();
        }
        if too_long(self.memo.as_deref(), ADJUSTMENT_MEMO_MAX) {
            return bad();
        }
        let mut seen = std::collections::BTreeSet::new();
        for l in &self.lines {
            if !seen.insert(l.item_id.to_string()) {
                return bad();
            }
            if l.quantity_change.is_none() && l.value_change.is_none() {
                return bad();
            }
            // `Quantity` holds |q| < 1e8 by construction (plan §2.0).
            if l.quantity_change.is_some_and(Quantity::is_zero) {
                return bad();
            }
            if let Some(v) = l.value_change
                && (v.is_zero() || v.to_ten_thousandths() % 100 != 0)
            {
                return bad();
            }
        }
        Ok(())
    }
}

/// One line of a saved adjustment, with the engine's figures.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustmentRecordLine {
    /// The part.
    pub item_id: ItemId,
    /// Units changed.
    #[serde(default)]
    pub quantity_change: Option<Quantity>,
    /// The value the owner gave.
    #[serde(default)]
    pub value_change: Option<Money>,
    /// The value posted (the engine's: at average cost for a quantity-only
    /// line).
    #[serde(default)]
    pub value: Option<Money>,
}

impl fmt::Debug for AdjustmentRecordLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdjustmentRecordLine")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// A saved adjustment (`{adjustment: …}`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdjustmentRecord {
    /// The `inventory_adjustment` transaction.
    pub id: TransactionId,
    /// Kind (always `inventory_adjustment`).
    #[serde(default = "adjustment_type")]
    pub txn_type: TxnType,
    /// Date.
    #[serde(alias = "txnDate")]
    pub date: CalendarDate,
    /// The other side's account.
    #[serde(default)]
    pub adjustment_account_id: Option<AccountId>,
    /// Memo (owner data).
    #[serde(default)]
    pub memo: Option<String>,
    /// Lines.
    #[serde(default, deserialize_with = "null_vec")]
    pub lines: Vec<AdjustmentRecordLine>,
    /// The engine-made lines (corrections, residuals).
    #[serde(default, deserialize_with = "null_vec")]
    pub inventory_lines: Vec<InventoryLine>,
    /// Posted, void or deleted.
    pub status: TxnStatus,
    /// Sent back as `expectedUpdatedAt`.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

fn adjustment_type() -> TxnType {
    TxnType::InventoryAdjustment
}

impl fmt::Debug for AdjustmentRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AdjustmentRecord")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("status", &self.status)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// What a create or edit answered: the adjustment and its warnings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedAdjustment {
    /// The adjustment as committed.
    pub adjustment: AdjustmentRecord,
    /// `NEGATIVE_STOCK` warnings (none on an idempotent replay).
    pub warnings: Vec<NegativeStock>,
}

fn saved_adjustment(body: &[u8]) -> Result<SavedAdjustment, ApiError> {
    let adjustment: AdjustmentRecord = decode_one(body, "adjustment", "inventory adjustment")
        .map_err(crate::resources::versioning::saved_unreadable)?;
    Ok(SavedAdjustment {
        adjustment,
        warnings: stock_warnings_from(body),
    })
}

// ---- Counts --------------------------------------------------------------------

/// One row of the count sheet: on hand as of the sheet's date.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountSheetRow {
    /// The part.
    pub item_id: ItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: Option<String>,
    /// Its description (owner data; plan §2.4).
    #[serde(default)]
    pub description: Option<String>,
    /// Usually bought from (plan §2.4).
    #[serde(default)]
    pub preferred_vendor_id: Option<ContactId>,
    /// Its unit label.
    #[serde(default)]
    pub unit_label: Option<String>,
    /// On hand as of the date (send it back as `sheetOnHand`).
    pub on_hand: Quantity,
    /// Average cost as of the date.
    #[serde(default)]
    pub average_cost: Option<UnitCost>,
    /// Value as of the date.
    #[serde(default)]
    pub value: Option<Money>,
}

impl fmt::Debug for CountSheetRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CountSheetRow")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// `GET /inventory/count-sheet?asOf=`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountSheet {
    /// The date.
    pub as_of: CalendarDate,
    /// Every active part.
    #[serde(default, deserialize_with = "null_vec", alias = "items")]
    pub rows: Vec<CountSheetRow>,
}

/// One counted part.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CountLine {
    /// The part.
    pub item_id: ItemId,
    /// On hand as the sheet showed it.
    pub sheet_on_hand: Quantity,
    /// What was counted (zero or more).
    pub counted_quantity: Quantity,
}

impl fmt::Debug for CountLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CountLine")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// `POST /inventory/counts`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCount {
    /// The count date (the adjustment's date).
    pub as_of: CalendarDate,
    /// Where the differences post.
    pub adjustment_account_id: AccountId,
    /// Memo (owner data).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memo: Option<String>,
    /// The counted parts.
    pub lines: Vec<CountLine>,
}

impl fmt::Debug for NewCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewCount")
            .field("as_of", &self.as_of)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

impl NewCount {
    /// Checks the request before anything is sent.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for no lines or too many, a part twice, a
    /// counted quantity below zero, a long memo.
    pub fn check(&self) -> Result<(), ApiError> {
        let bad = || Err(ApiError::Local(LocalValidation::InvalidValue));
        if self.lines.is_empty() || self.lines.len() > ADJUSTMENT_MAX_LINES {
            return bad();
        }
        if too_long(self.memo.as_deref(), ADJUSTMENT_MEMO_MAX) {
            return bad();
        }
        let mut seen = std::collections::BTreeSet::new();
        for l in &self.lines {
            if !seen.insert(l.item_id.to_string()) || l.counted_quantity.is_negative() {
                return bad();
            }
        }
        Ok(())
    }
}

/// What posting a count answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountResult {
    /// The one adjustment it posted (`None` when every count matched).
    pub adjustment: Option<AdjustmentRecord>,
    /// `NEGATIVE_STOCK` warnings.
    pub warnings: Vec<NegativeStock>,
}

// ---- Reports -------------------------------------------------------------------

/// One part of the valuation summary.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuationRow {
    /// The part.
    pub item_id: ItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: Option<String>,
    /// Its asset account.
    #[serde(default)]
    pub asset_account_id: Option<AccountId>,
    /// On hand as of the date.
    pub on_hand: Quantity,
    /// Average cost.
    #[serde(default)]
    pub average_cost: Option<UnitCost>,
    /// Value (what the GL holds for it).
    pub value: Money,
    /// Share of the total value, as the engine reports it.
    #[serde(default)]
    pub percent_of_total: Option<String>,
}

impl fmt::Debug for ValuationRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValuationRow")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// The GL check of one inventory asset account (accrual, Q-P24m).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuationAccount {
    /// The asset account.
    pub account_id: AccountId,
    /// Σ of its parts' values.
    pub valuation: Money,
    /// Its accrual GL balance as of the date.
    pub gl_balance: Money,
    /// `glBalance − valuation` as the engine computed it ("0.00" unless a
    /// journal entry posted to the account directly).
    pub difference: Money,
}

impl fmt::Debug for ValuationAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValuationAccount")
            .field("account_id", &self.account_id)
            .field("balanced", &self.difference.is_zero())
            .finish_non_exhaustive()
    }
}

/// `GET /reports/inventory-valuation?asOf=` (U-160).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuationSummary {
    /// The date.
    pub as_of: CalendarDate,
    /// One row per part.
    #[serde(default, deserialize_with = "null_vec", alias = "items")]
    pub rows: Vec<ValuationRow>,
    /// The GL check per asset account.
    #[serde(default, deserialize_with = "null_vec")]
    pub accounts: Vec<ValuationAccount>,
    /// Σ values (the engine's).
    #[serde(default)]
    pub total_value: Option<Money>,
    /// The cash-basis note (present on cash-basis companies, Q-P24m).
    #[serde(default)]
    pub basis_note: Option<String>,
}

/// One movement of the valuation detail.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuationDetailRow {
    /// The part.
    pub item_id: ItemId,
    /// The movement's date.
    pub date: CalendarDate,
    /// The document.
    pub transaction_id: TransactionId,
    /// Its kind.
    #[serde(default)]
    pub txn_type: Option<TxnType>,
    /// The movement kind (`receipt`, `sale`, `adjust_qty`, …; as sent).
    #[serde(default)]
    pub kind: Option<String>,
    /// Signed units.
    pub quantity: Quantity,
    /// Signed value posted.
    pub value: Money,
    /// On hand after the row.
    pub running_quantity: Quantity,
    /// Value after the row.
    pub running_value: Money,
    /// Average cost after the row.
    #[serde(default)]
    pub average_cost: Option<UnitCost>,
}

impl fmt::Debug for ValuationDetailRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValuationDetailRow")
            .field("item_id", &self.item_id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One part of the stock status report.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockStatusRow {
    /// The part.
    pub item_id: ItemId,
    /// Its name (owner data).
    #[serde(default)]
    pub name: Option<String>,
    /// Its description (owner data; plan §2.4).
    #[serde(default)]
    pub description: Option<String>,
    /// On hand as of the date.
    pub on_hand: Quantity,
    /// At or below its reorder point (the engine's flag, plan §2.4).
    #[serde(default)]
    pub reorder: Option<bool>,
    /// Its reorder point.
    #[serde(default)]
    pub reorder_point: Option<Quantity>,
    /// How many to order.
    #[serde(default)]
    pub reorder_quantity: Option<Quantity>,
    /// Usually bought from.
    #[serde(default)]
    pub preferred_vendor_id: Option<ContactId>,
    /// Its unit label.
    #[serde(default)]
    pub unit_label: Option<String>,
}

impl fmt::Debug for StockStatusRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StockStatusRow")
            .field("item_id", &self.item_id)
            .finish_non_exhaustive()
    }
}

/// A part at or below its reorder point (`GET /items/reorder-alerts`,
/// U-78). The same shape as a stock status row.
pub type ReorderAlert = StockStatusRow;

// ---- Operations ----------------------------------------------------------------

/// Inventory operations of a session (`session.inventory()`).
#[derive(Clone, Copy, Debug)]
pub struct Inventory<'a>(pub(crate) &'a Session);

impl Session {
    /// Inventory of the open company (P24).
    #[must_use]
    pub fn inventory(&self) -> Inventory<'_> {
        Inventory(self)
    }
}

fn adjustment_body(
    s: &Session,
    a: &NewAdjustment,
    opts: &PostingOptions,
    expected_updated_at: Option<Timestamp>,
) -> Result<Vec<u8>, ApiError> {
    a.check()?;
    let mut body =
        serde_json::to_value(a).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    if let (Some(t), Value::Object(m)) = (expected_updated_at, &mut body) {
        m.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
    }
    posting_body(s, body, opts)
}

fn id_path(route: &str, id: TransactionId) -> String {
    route.replace("{id}", &id.to_string())
}

impl Inventory<'_> {
    /// Records an adjustment.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `inventory-adjust-v1`; `Local(…)` from
    /// [`NewAdjustment::check`]; [`InventoryRefusal::Off`] or
    /// [`InventoryRefusal::NoStock`]; closing on the date; any [`ApiError`].
    pub async fn create_adjustment(
        &self,
        a: &NewAdjustment,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<SavedAdjustment, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVENTORY_ADJUST)?;
        let body = adjustment_body(s, a, opts, None)?;
        let mut call = Call::at(Method::Post, INVENTORY_ADJUSTMENT_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVENTORY_ADJUSTMENT_CREATE, call, None).await?;
        saved_adjustment(&reply.body)
    }

    /// One adjustment.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; `Decode`; any [`ApiError`].
    pub async fn adjustment(&self, id: TransactionId) -> Result<AdjustmentRecord, ApiError> {
        require(self.0, Capabilities::INVENTORY_ADJUST)?;
        let call = Call::new(
            Method::Get,
            INVENTORY_ADJUSTMENT_GET.route(),
            id_path(INVENTORY_ADJUSTMENT_GET.route(), id),
        );
        let reply = self.0.send(&INVENTORY_ADJUSTMENT_GET, call, None).await?;
        decode_one(&reply.body, "adjustment", "inventory adjustment")
    }

    /// Saves an edit if the adjustment is still at `updated_at` (409
    /// `STALE` otherwise).
    ///
    /// # Errors
    /// As [`Inventory::create_adjustment`]; `Conflict` (`STALE`).
    pub async fn update_adjustment(
        &self,
        id: TransactionId,
        updated_at: Timestamp,
        a: &NewAdjustment,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<SavedAdjustment, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVENTORY_ADJUST)?;
        let body = adjustment_body(s, a, opts, Some(updated_at))?;
        let mut call = Call::new(
            Method::Put,
            INVENTORY_ADJUSTMENT_PUT.route(),
            id_path(INVENTORY_ADJUSTMENT_PUT.route(), id),
        )
        .json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVENTORY_ADJUSTMENT_PUT, call, None).await?;
        saved_adjustment(&reply.body)
    }

    /// Voids an adjustment; answers the `NEGATIVE_STOCK` warnings the walk
    /// found (S6).
    ///
    /// # Errors
    /// `Local(Unsupported)`; [`InventoryRefusal::PeriodClosed`]; any
    /// [`ApiError`].
    pub async fn void_adjustment(
        &self,
        id: TransactionId,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Vec<NegativeStock>, ApiError> {
        self.remove(&INVENTORY_ADJUSTMENT_VOID, Method::Post, id, opts, key)
            .await
    }

    /// Deletes an adjustment; answers the `NEGATIVE_STOCK` warnings.
    ///
    /// # Errors
    /// As [`Inventory::void_adjustment`].
    pub async fn delete_adjustment(
        &self,
        id: TransactionId,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Vec<NegativeStock>, ApiError> {
        self.remove(&INVENTORY_ADJUSTMENT_DELETE, Method::Delete, id, opts, key)
            .await
    }

    async fn remove(
        &self,
        op: &'static crate::resources::operations::Operation,
        method: Method,
        id: TransactionId,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<Vec<NegativeStock>, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVENTORY_ADJUST)?;
        let body = posting_body(s, Value::Object(serde_json::Map::new()), opts)?;
        let mut call = Call::new(method, op.route(), id_path(op.route(), id)).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(op, call, None).await?;
        Ok(stock_warnings_from(&reply.body))
    }

    /// The count sheet as of `as_of`.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Decode`; any [`ApiError`].
    pub async fn count_sheet(&self, as_of: CalendarDate) -> Result<CountSheet, ApiError> {
        require(self.0, Capabilities::INVENTORY_ADJUST)?;
        let mut call = Call::at(Method::Get, INVENTORY_COUNT_SHEET.route());
        call.query.push(("asOf".into(), as_of.to_wire()));
        let reply = self.0.send(&INVENTORY_COUNT_SHEET, call, None).await?;
        decode_one(&reply.body, "countSheet", "count sheet")
    }

    /// Posts a count as one adjustment dated `as_of` (non-zero differences
    /// only). On 409 `STALE` read the moved items with
    /// [`count_stale_items`] and reload the sheet.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(…)` from [`NewCount::check`];
    /// `Conflict` (`STALE`); closing on the date; any [`ApiError`].
    pub async fn post_count(
        &self,
        c: &NewCount,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<CountResult, ApiError> {
        let s = &self.0.pinned();
        require(s, Capabilities::INVENTORY_ADJUST)?;
        c.check()?;
        let body =
            serde_json::to_value(c).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let body = posting_body(s, body, opts)?;
        let mut call = Call::at(Method::Post, INVENTORY_COUNT_POST.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = s.send(&INVENTORY_COUNT_POST, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| {
            crate::resources::versioning::saved_unreadable(ApiError::Decode { what: "count" })
        })?;
        let adjustment = match v.get("adjustment") {
            Some(a) if a.is_object() => Some(
                serde_json::from_value(a.clone())
                    .map_err(|_| ApiError::Decode { what: "count" })
                    .map_err(crate::resources::versioning::saved_unreadable)?,
            ),
            _ => None,
        };
        Ok(CountResult {
            adjustment,
            warnings: stock_warnings(&v),
        })
    }

    /// The valuation summary as of `as_of` with the GL check.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `inventory-reports-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn valuation(&self, as_of: CalendarDate) -> Result<ValuationSummary, ApiError> {
        require(self.0, Capabilities::INVENTORY_REPORTS)?;
        let mut call = Call::at(Method::Get, REPORT_INVENTORY_VALUATION.route());
        call.query.push(("asOf".into(), as_of.to_wire()));
        let reply = self.0.send(&REPORT_INVENTORY_VALUATION, call, None).await?;
        decode_one(&reply.body, "report", "inventory valuation")
    }

    /// The valuation detail from `from` to `to`, optionally for one part.
    ///
    /// # Errors
    /// As [`Inventory::valuation`].
    pub async fn valuation_detail(
        &self,
        from: CalendarDate,
        to: CalendarDate,
        item: Option<ItemId>,
    ) -> Result<Vec<ValuationDetailRow>, ApiError> {
        require(self.0, Capabilities::INVENTORY_REPORTS)?;
        if to < from {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::at(Method::Get, REPORT_INVENTORY_VALUATION_DETAIL.route());
        call.query.push(("from".into(), from.to_wire()));
        call.query.push(("to".into(), to.to_wire()));
        if let Some(i) = item {
            call.query.push(("itemId".into(), i.to_string()));
        }
        let reply = self
            .0
            .send(&REPORT_INVENTORY_VALUATION_DETAIL, call, None)
            .await?;
        decode_list(&reply.body, "rows", "inventory valuation detail")
    }

    /// Stock status as of `as_of`.
    ///
    /// # Errors
    /// As [`Inventory::valuation`].
    pub async fn stock_status(&self, as_of: CalendarDate) -> Result<Vec<StockStatusRow>, ApiError> {
        require(self.0, Capabilities::INVENTORY_REPORTS)?;
        let mut call = Call::at(Method::Get, REPORT_INVENTORY_STOCK_STATUS.route());
        call.query.push(("asOf".into(), as_of.to_wire()));
        let reply = self
            .0
            .send(&REPORT_INVENTORY_STOCK_STATUS, call, None)
            .await?;
        decode_list(&reply.body, "rows", "stock status")
    }

    /// Parts at or below their reorder point (Home's Needs attention line
    /// and Items to Reorder).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `reorder-alerts-v1`; `Decode`; any
    /// [`ApiError`].
    pub async fn reorder_alerts(&self) -> Result<Vec<ReorderAlert>, ApiError> {
        require(self.0, Capabilities::REORDER_ALERTS)?;
        let call = Call::at(Method::Get, ITEMS_REORDER_ALERTS.route());
        let reply = self.0.send(&ITEMS_REORDER_ALERTS, call, None).await?;
        decode_list(&reply.body, "items", "reorder alerts")
    }

    /// The reorder report.
    ///
    /// # Errors
    /// As [`Inventory::reorder_alerts`].
    pub async fn reorder_report(&self) -> Result<Vec<ReorderAlert>, ApiError> {
        require(self.0, Capabilities::REORDER_ALERTS)?;
        let call = Call::at(Method::Get, REPORT_REORDER.route());
        let reply = self.0.send(&REPORT_REORDER, call, None).await?;
        decode_list(&reply.body, "rows", "reorder report")
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
