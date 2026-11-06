//! Products and services (`/api/v1/items`, P05 plan §2.8). DELETE
//! deactivates and answers only a message.

use cl_core::ids::{AccountId, ContactId, ItemId, TagId};
use cl_core::{CalendarDate, Money, Quantity, Timestamp, UnitCost};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::{ApiError, ConflictReason};
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{self, Query, SortDir, Spec, editable_with_active, null_true, yes};
use crate::resources::invoices::SignedPercent;
use crate::resources::operations::{ITEM_DELETE, ITEM_GET, ITEM_PUT, ITEMS_CREATE, ITEMS_LIST};
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned};
use crate::session::{Session, json_body};
use crate::wire::{Page, PageRequest};

pub(crate) const SPEC: Spec = Spec {
    list: &ITEMS_LIST,
    create: &ITEMS_CREATE,
    get: &ITEM_GET,
    put: &ITEM_PUT,
    delete: &ITEM_DELETE,
    base: "/api/v1/items",
    item_key: "item",
    list_key: "data",
};

/// The fields the item form edits (also the create body).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemFields {
    /// Name (unique, 1-255 characters).
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// Sales price (exact; a decimal string on the wire).
    #[serde(default)]
    pub unit_price: Option<Money>,
    /// Income account the sale posts to.
    pub income_account_id: AccountId,
    /// Subject to sales tax.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_taxable: bool,
    /// Kind (U-39a, `item-kinds-sales-v1`); a service is not sent, so an
    /// older engine takes it as before.
    #[serde(default, skip_serializing_if = "ItemType::is_service")]
    pub item_type: ItemType,
    /// A discount item's percent off (0 < p <= 100); None with a
    /// `unit_price` takes that amount off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discount_percent: Option<SignedPercent>,
    /// The item's tax code (`taxCodeId`, `sales-tax-setup-v1`, U-25);
    /// sent only when set (P23 WP-1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tax_code_id: Option<crate::resources::sales_tax::TaxCodeId>,
    /// The purchase side, the inventory accounts and the reorder fields
    /// (U-39b widened, `item-types-v1`); every field is sent only when set.
    #[serde(flatten)]
    pub purchase: ItemPurchase,
}

/// The purchase side of an item, its inventory accounts and its reorder
/// fields (U-39b widened, `item-types-v1`, P24 plan §2.1). Every field is
/// optional and sent only when set, so an older engine and every existing
/// fingerprint see nothing new.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPurchase {
    /// What purchase documents print (owner data).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_description: Option<String>,
    /// The usual cost per unit (`decimal(19,4)`); a part's fallback cost
    /// while nothing is on hand (Q-P24d).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_cost: Option<Money>,
    /// Where purchases of a non-stock item post.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expense_account_id: Option<AccountId>,
    /// A part's inventory asset account (required on parts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_account_id: Option<AccountId>,
    /// A part's cost of goods sold account (required on parts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cogs_account_id: Option<AccountId>,
    /// The vendor it is usually bought from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_vendor_id: Option<ContactId>,
    /// Alert when on hand is at or below this (U-78).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reorder_point: Option<Quantity>,
    /// How many to order when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reorder_quantity: Option<Quantity>,
    /// Display label of the unit of an item without a unit set (`each`,
    /// `bag`; at most 10 characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_label: Option<String>,
    /// The item's unit set (`units-of-measure-v1`, U-161); stock is kept in
    /// its base unit. Changes only while no line names the item (409
    /// `UNIT_IN_USE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_set_id: Option<cl_core::ids::UnitSetId>,
    /// The unit purchase lines start in (`None`: the base unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_purchase_unit_id: Option<cl_core::ids::UnitId>,
    /// The unit sales lines start in (`None`: the base unit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_sale_unit_id: Option<cl_core::ids::UnitId>,
}

impl ItemPurchase {
    /// True when nothing is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == ItemPurchase::default()
    }
}

/// The wire keys of [`ItemPurchase`].
const PURCHASE_KEYS: &[&str] = &[
    "purchaseDescription",
    "purchaseCost",
    "expenseAccountId",
    "assetAccountId",
    "cogsAccountId",
    "preferredVendorId",
    "reorderPoint",
    "reorderQuantity",
    "unitLabel",
    "unitSetId",
    "defaultPurchaseUnitId",
    "defaultSaleUnitId",
];

/// The most characters of a unit label (Q-P24i).
pub const UNIT_LABEL_MAX_CHARS: usize = 10;

/// A new part's opening quantity and value (`opening` on item create,
/// `inventory-adjust-v1`, Q-P24h): an adjustment against opening balance
/// equity dated `as_of`.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpeningStock {
    /// Units on hand (above zero).
    pub quantity: Quantity,
    /// Their total value (zero or more, to the cent).
    pub value: Money,
    /// The date they were on hand.
    pub as_of: CalendarDate,
}

impl std::fmt::Debug for OpeningStock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpeningStock")
            .field("as_of", &self.as_of)
            .finish_non_exhaustive()
    }
}

/// Sales item kinds (U-39a, `item-kinds-sales-v1`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemType {
    /// A service (the default).
    #[default]
    Service,
    /// A product not tracked in inventory.
    NonInventory,
    /// Another charge (delivery, fees).
    OtherCharge,
    /// A discount off the lines since the last subtotal (a percent, or an
    /// amount in `unit_price`).
    Discount,
    /// A subtotal of the lines above it (posts nothing).
    Subtotal,
    /// A part tracked in inventory at average cost (`item-types-v1`, P24).
    InventoryPart,
}

impl ItemType {
    /// Every kind, in menu order.
    pub const ALL: [ItemType; 6] = [
        ItemType::Service,
        ItemType::InventoryPart,
        ItemType::NonInventory,
        ItemType::OtherCharge,
        ItemType::Discount,
        ItemType::Subtotal,
    ];

    /// A service.
    #[must_use]
    #[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if
    pub fn is_service(&self) -> bool {
        *self == ItemType::Service
    }

    /// The wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ItemType::Service => "service",
            ItemType::NonInventory => "non_inventory",
            ItemType::OtherCharge => "other_charge",
            ItemType::Discount => "discount",
            ItemType::Subtotal => "subtotal",
            ItemType::InventoryPart => "inventory_part",
        }
    }
}

/// Checks an item's kind against the engine before anything is sent.
fn check_kind(s: &Session, f: &ItemFields) -> Result<(), ApiError> {
    let caps = s.client().capabilities();
    // `item-types-v1` (U-39b) carries the sales kinds too.
    let kinds = caps.has(crate::Capabilities::ITEM_KINDS_SALES)
        || caps.has(crate::Capabilities::ITEM_TYPES);
    if (!f.item_type.is_service() || f.discount_percent.is_some()) && !kinds {
        return Err(ApiError::Local(crate::LocalValidation::Unsupported));
    }
    let pct_ok = f.discount_percent.is_none_or(|p| {
        let d = p.as_decimal();
        d > rust_decimal::Decimal::ZERO && d <= rust_decimal::Decimal::ONE_HUNDRED
    });
    let shape_ok = match f.item_type {
        ItemType::Discount => !(f.discount_percent.is_some() && f.unit_price.is_some()),
        ItemType::Subtotal => f.discount_percent.is_none() && f.unit_price.is_none(),
        _ => f.discount_percent.is_none(),
    };
    if !pct_ok || !shape_ok {
        return Err(ApiError::Local(crate::LocalValidation::InvalidValue));
    }
    check_purchase(
        s.client()
            .capabilities()
            .has(crate::Capabilities::ITEM_TYPES),
        f,
    )
}

/// The P24 fields (U-39b widened): refused without `item-types-v1` (an
/// older engine would drop them and answer success), and the plan §2.1
/// shapes: a part names its asset and COGS accounts; costs and reorder
/// figures are not negative, a unit label has at most 10 characters.
fn check_purchase(item_types: bool, f: &ItemFields) -> Result<(), ApiError> {
    let p = &f.purchase;
    let part = f.item_type == ItemType::InventoryPart;
    if (part || !p.is_empty()) && !item_types {
        return Err(ApiError::Local(crate::LocalValidation::Unsupported));
    }
    let accounts_ok = !part || (p.asset_account_id.is_some() && p.cogs_account_id.is_some());
    let not_negative = p.purchase_cost.is_none_or(|c| !c.is_negative())
        && p.reorder_point.is_none_or(|q| !q.is_negative())
        && p.reorder_quantity.is_none_or(|q| !q.is_negative());
    let label_ok = p
        .unit_label
        .as_deref()
        .is_none_or(|l| l.chars().count() <= UNIT_LABEL_MAX_CHARS);
    if !accounts_ok || !not_negative || !label_ok {
        return Err(ApiError::Local(crate::LocalValidation::InvalidValue));
    }
    Ok(())
}

/// The opening stock of a new part (Q-P24h): only on a part, with
/// `inventory-adjust-v1`; a quantity above zero, a value of zero or more in
/// whole cents.
fn check_opening(s: &Session, f: &ItemFields, o: &OpeningStock) -> Result<(), ApiError> {
    if !s
        .client()
        .capabilities()
        .has(crate::Capabilities::INVENTORY_ADJUST)
    {
        return Err(ApiError::Local(crate::LocalValidation::Unsupported));
    }
    let cents = o.value.to_ten_thousandths() % 100 == 0;
    if f.item_type != ItemType::InventoryPart
        || !o.quantity.is_positive()
        || o.value.is_negative()
        || !cents
    {
        return Err(ApiError::Local(crate::LocalValidation::InvalidValue));
    }
    Ok(())
}

/// Body of an item update: every editable field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemUpdate {
    /// Form fields.
    #[serde(flatten)]
    pub fields: ItemFields,
    /// Active.
    pub is_active: bool,
}

/// An item as the engine returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// Id.
    pub id: ItemId,
    /// Form fields.
    #[serde(flatten)]
    pub fields: ItemFields,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// Default tag of new lines with this item.
    #[serde(default)]
    pub default_tag_id: Option<TagId>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// A part's stock as the engine reports it (read-only, never sent).
    #[serde(flatten)]
    pub stock: ItemStock,
}

/// A part's stock on `GET /items` (`item-types-v1`, P24 plan §2.1): from
/// its latest movement, every posting included (Q-P24l). Read-only; the
/// desktop never adds these up or derives them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemStock {
    /// Units on hand (below zero after overselling, Q-148).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_on_hand: Option<Quantity>,
    /// The average cost (six places, `round6(T / Q)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub average_cost: Option<UnitCost>,
    /// The value on hand (what the inventory asset account holds for it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_value: Option<Money>,
    /// The sale price in each unit of the item's set, the base unit first
    /// (`round4(price × factor)`, engine-computed; U-161). Empty without a
    /// set.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::resources::customer_payments::null_vec"
    )]
    pub unit_prices: Vec<UnitPrice>,
}

/// One unit's sale price on `GET /items` (`unitPrices[]`, U-161).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnitPrice {
    /// The unit (`None`: the base unit).
    #[serde(default)]
    pub unit_id: Option<cl_core::ids::UnitId>,
    /// Its name.
    pub name: String,
    /// Its abbreviation.
    #[serde(default)]
    pub abbreviation: Option<String>,
    /// Base units per unit.
    pub factor: cl_core::UnitFactor,
    /// The price in this unit (`None` without a base price).
    #[serde(default)]
    pub unit_price: Option<Money>,
}

impl EditableFields for Item {
    const EDITABLE: &'static [&'static str] = &[
        "name",
        "description",
        "unitPrice",
        "incomeAccountId",
        "isTaxable",
        "itemType",
        "discountPercent",
        "isActive",
        // P24 (`item-types-v1`): null on every older item.
        "purchaseDescription",
        "purchaseCost",
        "expenseAccountId",
        "assetAccountId",
        "cogsAccountId",
        "preferredVendorId",
        "reorderPoint",
        "reorderQuantity",
        "unitLabel",
        "unitSetId",
        "defaultPurchaseUnitId",
        "defaultSaleUnitId",
    ];

    fn editable(&self) -> Value {
        editable_with_active(&self.fields, self.is_active, Self::EDITABLE)
    }
}

/// Sort keys of the item list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemSort {
    /// Name.
    Name,
    /// Price.
    Price,
    /// Taxable.
    Taxable,
    /// Active/inactive.
    Status,
}

impl ItemSort {
    const fn as_str(self) -> &'static str {
        match self {
            ItemSort::Name => "name",
            ItemSort::Price => "price",
            ItemSort::Taxable => "taxable",
            ItemSort::Status => "status",
        }
    }
}

/// Filters of the item list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemFilter {
    /// Active or inactive only.
    pub is_active: Option<bool>,
    /// Taxable or not only.
    pub is_taxable: Option<bool>,
    /// Name contains.
    pub search: Option<String>,
    /// Sort order.
    pub sort: Option<(ItemSort, SortDir)>,
}

impl ItemFilter {
    /// Query pairs (camelCase).
    #[must_use]
    pub fn to_query(&self) -> Vec<(String, String)> {
        self.query().0
    }

    pub(crate) fn query(&self) -> Query {
        let mut q = Query::default();
        q.opt_bool("isActive", self.is_active);
        q.opt_bool("isTaxable", self.is_taxable);
        q.opt_str("search", self.search.as_deref());
        q.sort(self.sort.map(|(s, d)| (s.as_str(), d)));
        q
    }
}

/// Item operations of a session (`session.items()`).
#[derive(Clone, Copy, Debug)]
pub struct Items<'a>(pub(crate) &'a Session);

impl Session {
    /// Products and services of the open company.
    #[must_use]
    pub fn items(&self) -> Items<'_> {
        Items(self)
    }
}

impl Items<'_> {
    /// One page of items.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(
        &self,
        filter: &ItemFilter,
        page: PageRequest,
    ) -> Result<Page<Item>, ApiError> {
        common::list_page(self.0, &SPEC, filter.query(), page).await
    }

    /// Every matching item (pages of 500, at most 100,000 rows).
    ///
    /// # Errors
    /// `Decode` beyond the cap; any [`ApiError`].
    pub async fn list_all(&self, filter: &ItemFilter) -> Result<Vec<Item>, ApiError> {
        common::list_all(self.0, &SPEC, &filter.query()).await
    }

    /// One item with its version.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: ItemId) -> Result<Versioned<Item>, ApiError> {
        common::get(self.0, &SPEC, &id.to_string()).await
    }

    /// Creates an item.
    ///
    /// # Errors
    /// `Conflict { AlreadyExists }` for a used name; `Validation`; any
    /// [`ApiError`].
    pub async fn create(
        &self,
        new: &ItemFields,
        key: IdempotencyKey,
    ) -> Result<Versioned<Item>, ApiError> {
        self.create_with_opening(new, None, key).await
    }

    /// Creates an item; a new part may bring its opening quantity and value
    /// (`opening`, Q-P24h: an adjustment against opening balance equity).
    ///
    /// # Errors
    /// `Local(Unsupported)` for an opening without `inventory-adjust-v1`,
    /// `Local(InvalidValue)` for an opening on another kind, a quantity not
    /// above zero or a value below zero or in part cents (nothing sent);
    /// otherwise as [`Items::create`].
    pub async fn create_with_opening(
        &self,
        new: &ItemFields,
        opening: Option<&OpeningStock>,
        key: IdempotencyKey,
    ) -> Result<Versioned<Item>, ApiError> {
        check_kind(self.0, new)?;
        let mut body = serde_json::to_value(new).map_err(|_| ApiError::Decode { what: "item" })?;
        if let Some(o) = opening {
            check_opening(self.0, new, o)?;
            if let Value::Object(m) = &mut body {
                m.insert(
                    "opening".into(),
                    serde_json::json!({
                        "quantity": o.quantity.to_wire(),
                        "value": crate::resources::customer_payments::cents_wire(o.value)?,
                        "asOf": o.as_of.to_wire(),
                    }),
                );
            }
        }
        common::create_named(
            self.0,
            &SPEC,
            "name",
            new.name.as_str(),
            &editable_with_active(new, true, Item::EDITABLE),
            json_body(&body)?,
            key,
        )
        .await
        .map_err(name_taken)
    }

    /// Saves an item if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent); any [`ApiError`].
    pub async fn update(
        &self,
        id: ItemId,
        expected: &RecordVersion,
        update: &ItemUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Item>, ApiError> {
        self.update_expecting(id, expected, None, update, key).await
    }

    /// [`Items::update`] that also sends the item's `updatedAt` as read
    /// (`expectedUpdatedAt`, `item-types-v1`: the engine locks the row and
    /// answers 409 `STALE` when it changed). With `item-types-v1` every
    /// purchase field left blank is sent as `null`, so clearing one clears
    /// it on the engine.
    ///
    /// # Errors
    /// As [`Items::update`].
    pub async fn update_expecting(
        &self,
        id: ItemId,
        expected: &RecordVersion,
        updated_at: Option<Timestamp>,
        update: &ItemUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Item>, ApiError> {
        check_kind(self.0, &update.fields)?;
        let item_types = self
            .0
            .client()
            .capabilities()
            .has(crate::Capabilities::ITEM_TYPES);
        let mut body =
            serde_json::to_value(update).map_err(|_| ApiError::Decode { what: "item" })?;
        if item_types && let Value::Object(m) = &mut body {
            for k in PURCHASE_KEYS {
                m.entry(*k).or_insert(Value::Null);
            }
            if let Some(t) = updated_at {
                m.insert("expectedUpdatedAt".into(), Value::String(t.to_wire()));
            }
        }
        common::update(
            self.0,
            &SPEC,
            &id.to_string(),
            expected,
            json_body(&body)?,
            key,
        )
        .await
    }

    /// Deactivates an item.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn deactivate(&self, id: ItemId, key: IdempotencyKey) -> Result<(), ApiError> {
        common::deactivate(self.0, &SPEC, &id.to_string(), key)
            .await
            .map(drop)
    }
}

/// The engine's item create answers a used name with a 409 that carries no
/// code (its only codeless 409 there); say so, so callers can tell it from
/// coded conflicts such as `IDEMPOTENCY_IN_PROGRESS`.
fn name_taken(e: ApiError) -> ApiError {
    match e {
        ApiError::Conflict {
            reason: ConflictReason::Unspecified,
            current_version,
            code: None,
            message,
            ..
        } => ApiError::Conflict {
            reason: ConflictReason::AlreadyExists,
            current_version,
            code: None,
            message,
            details: crate::ConflictDetails::default(),
        },
        other => other,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn sales_kinds_round_trip_and_a_service_sends_no_kind() {
        let v = serde_json::json!({
            "id": "d0000000-0000-4000-8000-b00000000009",
            "name": "Loyalty discount",
            "incomeAccountId": "d0000000-0000-4000-8000-a00000004000",
            "isTaxable": true,
            "itemType": "discount",
            "discountPercent": "10.0000",
            "unitPrice": null,
        });
        let item: Item = serde_json::from_value(v).expect("an E2 item");
        assert_eq!(item.fields.item_type, ItemType::Discount);
        assert!(item.fields.discount_percent.is_some());
        let body = serde_json::to_value(&item.fields).expect("json");
        assert_eq!(body["itemType"], "discount");
        let service = ItemFields {
            item_type: ItemType::Service,
            discount_percent: None,
            ..item.fields
        };
        let body = serde_json::to_value(&service).expect("json");
        assert!(
            body.get("itemType").is_none(),
            "older engines see no new key"
        );
        let old: Item = serde_json::from_value(serde_json::json!({
            "id": "d0000000-0000-4000-8000-b00000000001",
            "name": "Mowing",
            "incomeAccountId": "d0000000-0000-4000-8000-a00000004000",
        }))
        .expect("an older item");
        assert_eq!(old.fields.item_type, ItemType::Service);
        for k in ItemType::ALL {
            assert_eq!(serde_json::to_value(k).expect("json"), k.as_wire());
        }
    }

    /// P24 (U-39b widened): a part reads its accounts, reorder fields and
    /// stock as exact strings; a service sends none of the new keys.
    #[test]
    fn an_inventory_part_reads_its_purchase_side_and_stock() {
        let v = serde_json::json!({
            "id": "d0000000-0000-4000-8000-b00000000010",
            "name": "Widget", "incomeAccountId": "d0000000-0000-4000-8000-a00000004000",
            "itemType": "inventory_part", "unitPrice": "9.0000",
            "purchaseDescription": "Widget, boxed", "purchaseCost": "3.3333",
            "assetAccountId": "d0000000-0000-4000-8000-a00000001300",
            "cogsAccountId": "d0000000-0000-4000-8000-a00000005000",
            "preferredVendorId": "d0000000-0000-4000-8000-d00000000001",
            "reorderPoint": "5.0000", "reorderQuantity": "20", "unitLabel": "each",
            "quantityOnHand": "-3.0000", "averageCost": "3.589091", "assetValue": "-10.77",
        });
        let item: Item = serde_json::from_value(v).expect("a part");
        assert_eq!(item.fields.item_type, ItemType::InventoryPart);
        let p = &item.fields.purchase;
        assert_eq!(
            p.purchase_cost.map(Money::to_wire).as_deref(),
            Some("3.3333")
        );
        assert_eq!(
            p.reorder_quantity.map(Quantity::to_wire).as_deref(),
            Some("20.0000")
        );
        assert_eq!(p.unit_label.as_deref(), Some("each"));
        assert_eq!(
            item.stock
                .quantity_on_hand
                .map(Quantity::to_wire)
                .as_deref(),
            Some("-3.0000")
        );
        assert_eq!(
            item.stock.average_cost.map(UnitCost::to_wire).as_deref(),
            Some("3.589091")
        );
        assert_eq!(
            item.stock.asset_value.map(Money::to_wire).as_deref(),
            Some("-10.7700")
        );
        let body = serde_json::to_value(&item.fields).expect("json");
        assert_eq!(body["itemType"], "inventory_part");
        assert_eq!(
            body["cogsAccountId"],
            "d0000000-0000-4000-8000-a00000005000"
        );
        assert!(body.get("quantityOnHand").is_none(), "stock is never sent");
        // A float on the wire is refused (money is exact).
        let float = serde_json::json!({
            "id": "d0000000-0000-4000-8000-b00000000010", "name": "W",
            "incomeAccountId": "d0000000-0000-4000-8000-a00000004000", "averageCost": 3.5,
        });
        assert!(serde_json::from_value::<Item>(float).is_err());
        // An older item: every new key absent, nothing new sent.
        let old: Item = serde_json::from_str(ITEM).expect("old");
        assert!(old.fields.purchase.is_empty());
        assert_eq!(old.stock, ItemStock::default());
        let body = serde_json::to_value(&old.fields).expect("json");
        for k in PURCHASE_KEYS {
            assert!(body.get(*k).is_none(), "{k}");
        }
    }

    #[test]
    fn purchase_fields_need_item_types_and_part_shapes() {
        let aid =
            |n: u32| AccountId::parse(&format!("d0000000-0000-4000-8000-a{n:011}")).expect("id");
        let mut f: ItemFields = serde_json::from_str::<Item>(ITEM).expect("item").fields;
        assert!(check_purchase(false, &f).is_ok(), "a plain service");
        f.purchase.purchase_cost = Money::parse_wire("2").ok();
        assert!(matches!(
            check_purchase(false, &f),
            Err(ApiError::Local(crate::LocalValidation::Unsupported))
        ));
        assert!(check_purchase(true, &f).is_ok(), "two-sided service");
        f.item_type = ItemType::InventoryPart;
        let invalid = |f: &ItemFields| {
            matches!(
                check_purchase(true, f),
                Err(ApiError::Local(crate::LocalValidation::InvalidValue))
            )
        };
        assert!(invalid(&f), "a part names its accounts");
        f.purchase.asset_account_id = Some(aid(1300));
        assert!(invalid(&f));
        f.purchase.cogs_account_id = Some(aid(5000));
        assert!(check_purchase(true, &f).is_ok());
        f.purchase.unit_label = Some("x".repeat(11));
        assert!(invalid(&f), "10 characters at most");
        f.purchase.unit_label = Some("é".repeat(10));
        assert!(check_purchase(true, &f).is_ok(), "characters, not bytes");
        f.purchase.reorder_point = Quantity::parse_wire("-1").ok();
        assert!(invalid(&f));
        f.purchase.reorder_point = None;
        f.purchase.purchase_cost = Money::parse_wire("-0.01").ok();
        assert!(invalid(&f));
        f.purchase = ItemPurchase::default();
        assert!(matches!(
            check_purchase(false, &f),
            Err(ApiError::Local(crate::LocalValidation::Unsupported))
        ));
    }

    #[test]
    fn a_price_level_reads_e2_unit_prices() {
        let v = serde_json::json!({
            "id": "d0000000-0000-4000-8000-e00000000001",
            "name": "Contractor", "percent": "-7.5000", "isActive": true,
            "itemPrices": [{"itemId": "d0000000-0000-4000-8000-b00000000002", "unitPrice": "30.0000"}],
            "itemPriceCount": 1,
        });
        let l: crate::PriceLevel = serde_json::from_value(v).expect("an E2 level");
        assert_eq!(l.item_prices[0].price, Money::parse_wire("30").expect("m"));
        let back = serde_json::to_value(&l.item_prices[0]).expect("json");
        assert!(back.get("unitPrice").is_some());
    }
    use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const ITEM: &str = r#"{"id":"4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "name":"Canary Widget","description":null,"unitPrice":"12.3400",
        "incomeAccountId":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","isTaxable":true,
        "isActive":true,"defaultTagId":null,"createdAt":"2026-10-01T10:00:00.000Z"}"#;

    #[test]
    fn filter_query() {
        let f = ItemFilter {
            is_active: None,
            is_taxable: Some(false),
            search: Some("wid".into()),
            sort: Some((ItemSort::Price, SortDir::Desc)),
        };
        let pairs: Vec<String> = f
            .to_query()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            pairs,
            [
                "isTaxable=false",
                "search=wid",
                "sortBy=price",
                "sortDir=desc"
            ]
        );
    }

    #[tokio::test]
    async fn unit_price_round_trips_as_a_string_and_validation_maps() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/items"))
            .respond_with(json(201, &format!(r#"{{"item":{ITEM}}}"#)))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(
                400,
                r#"{"error":{"message":"Validation failed","code":"VALIDATION_ERROR",
                    "details":[{"path":["incomeAccountId"],"message":"Required"}]}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, &format!(r#"{{"data":[{ITEM}],"total":1}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(json(200, r#"{"message":"Deactivated"}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let it: Item = serde_json::from_str(ITEM).expect("item");
        let created = s
            .items()
            .create(&it.fields, IdempotencyKey::new())
            .await
            .expect("create");
        assert_eq!(
            created
                .record
                .fields
                .unit_price
                .map(Money::to_wire)
                .as_deref(),
            Some("12.3400")
        );
        let post: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(post["unitPrice"], "12.3400");
        let e = s
            .items()
            .create(&it.fields, IdempotencyKey::new())
            .await
            .expect_err("invalid");
        assert!(matches!(e, ApiError::Validation(_)));

        s.items()
            .list(
                &ItemFilter {
                    is_active: Some(true),
                    ..ItemFilter::default()
                },
                PageRequest::first_max(),
            )
            .await
            .expect("list");
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            "isActive=true&limit=500&offset=0"
        );
        s.items()
            .deactivate(it.id, IdempotencyKey::new())
            .await
            .expect("off");
    }

    /// A new part's opening stock rides on the create (Q-P24h); an edit
    /// with `item-types-v1` clears blank purchase fields and sends the
    /// `updatedAt` it read.
    #[tokio::test]
    async fn opening_stock_and_guarded_part_edits() {
        let server = MockServer::start().await;
        let part = r#"{"id":"4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Widget",
            "incomeAccountId":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","itemType":"inventory_part",
            "assetAccountId":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02",
            "cogsAccountId":"2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a03",
            "isActive":true,"updatedAt":"2026-10-01T10:00:00.000Z"}"#;
        Mock::given(method("POST"))
            .respond_with(json(201, &format!(r#"{{"item":{part}}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/items"))
            .respond_with(json(200, r#"{"data":[],"total":0}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/items/4a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"))
            .respond_with(json(200, &format!(r#"{{"item":{part}}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(200, &format!(r#"{{"item":{part}}}"#)))
            .mount(&server)
            .await;
        let s = session_with_caps(
            &server,
            &[
                crate::Capabilities::ITEM_TYPES,
                crate::Capabilities::INVENTORY_ADJUST,
            ],
        )
        .await;
        let it: Item = serde_json::from_str(part).expect("part");
        let opening = OpeningStock {
            quantity: Quantity::parse_wire("10").expect("q"),
            value: Money::parse_wire("33.33").expect("m"),
            as_of: CalendarDate::from_ymd(2026, 1, 1).expect("d"),
        };
        let made = s
            .items()
            .create_with_opening(&it.fields, Some(&opening), IdempotencyKey::new())
            .await
            .expect("create");
        let post: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(
            post["opening"],
            serde_json::json!({"quantity": "10.0000", "value": "33.33", "asOf": "2026-01-01"})
        );
        assert!(
            !format!("{opening:?}").contains("33"),
            "no amounts in Debug"
        );
        // A fraction of a cent, a zero quantity or a service: refused, nothing sent.
        for bad in [
            OpeningStock {
                value: Money::parse_wire("1.001").expect("m"),
                ..opening.clone()
            },
            OpeningStock {
                quantity: Quantity::ZERO,
                ..opening.clone()
            },
        ] {
            let e = s
                .items()
                .create_with_opening(&it.fields, Some(&bad), IdempotencyKey::new())
                .await
                .expect_err("refused");
            assert!(matches!(e, ApiError::Local(_)), "{e:?}");
        }
        assert_eq!(requests(&server, "POST").await.len(), 1);

        let update = ItemUpdate {
            fields: it.fields.clone(),
            is_active: true,
        };
        s.items()
            .update_expecting(
                it.id,
                &made.version,
                it.updated_at,
                &update,
                IdempotencyKey::new(),
            )
            .await
            .expect("update");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["expectedUpdatedAt"], "2026-10-01T10:00:00.000Z");
        assert_eq!(put["purchaseCost"], Value::Null, "a blank field clears");
        assert_eq!(
            put["assetAccountId"],
            "2a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02"
        );
    }

    /// A codeless 409 on create is the used-name answer; coded ones stay.
    #[tokio::test]
    async fn used_name_is_already_exists_coded_conflicts_stay() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"An item with this name already exists"}}"#,
            ))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"busy","code":"TENANT_BUSY"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(json(200, r#"{"data":[],"total":0}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let it: Item = serde_json::from_str(ITEM).expect("item");
        let e = s
            .items()
            .create(&it.fields, IdempotencyKey::new())
            .await
            .expect_err("taken");
        assert!(
            matches!(
                e,
                ApiError::Conflict {
                    reason: ConflictReason::AlreadyExists,
                    code: None,
                    ..
                }
            ),
            "{e:?}"
        );
        let e = s
            .items()
            .create(&it.fields, IdempotencyKey::new())
            .await
            .expect_err("busy");
        assert!(
            matches!(
                e,
                ApiError::Conflict {
                    reason: ConflictReason::Busy,
                    ..
                }
            ),
            "{e:?}"
        );
    }
}
