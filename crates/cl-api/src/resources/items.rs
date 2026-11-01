//! Products and services (`/api/v1/items`, P05 plan §2.8). DELETE
//! deactivates and answers only a message.

use cl_core::ids::{AccountId, ItemId, TagId};
use cl_core::{Money, Timestamp};
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
}

impl ItemType {
    /// Every kind, in menu order.
    pub const ALL: [ItemType; 5] = [
        ItemType::Service,
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
        }
    }
}

/// Checks an item's kind against the engine before anything is sent.
fn check_kind(s: &Session, f: &ItemFields) -> Result<(), ApiError> {
    let kinds = s
        .client()
        .capabilities()
        .has(crate::Capabilities::ITEM_KINDS_SALES);
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
        check_kind(self.0, new)?;
        common::create_named(
            self.0,
            &SPEC,
            "name",
            new.name.as_str(),
            &editable_with_active(new, true, Item::EDITABLE),
            json_body(new)?,
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
        check_kind(self.0, &update.fields)?;
        common::update(
            self.0,
            &SPEC,
            &id.to_string(),
            expected,
            json_body(update)?,
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
    use crate::resources::testkit::{company_session, json, query, requests};
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
