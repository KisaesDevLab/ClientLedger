//! Items (P11 step 4): a service item made inactive and active again, a
//! used name refused, and the price kept exact. The engine gaps U-39 (item
//! types, purchase side, unit of measure) and U-40 (price levels) are
//! capability probes: each asserts what the engine does today and prints
//! `PENDING(U-xx)` until the capability is advertised.

use cl_api::{
    AccountType, ApiError, Capabilities, ConflictReason, ItemFields, ItemFilter, ItemUpdate,
    Method, PageRequest,
};
use cl_core::Money;
use serde_json::json;

use crate::common::{account, key, m};
use crate::support::{Harness, unique};

fn service(h: &Harness, prefix: &str) -> cl_api::Item {
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Service income");
    h.run(s.items().create(
        &ItemFields {
            name: unique(prefix),
            description: None,
            unit_price: Some(m("45.0450")),
            income_account_id: income.id,
            is_taxable: true,
            item_type: cl_api::ItemType::Service,
            discount_percent: None,
        },
        key(),
    ))
    .expect("create")
    .record
}

/// Keyboard-only create/edit/inactivate rests on these: deactivate, then
/// active again through the update; a used name (even an inactive one) is
/// a conflict; the price comes back exactly as sent.
pub fn reactivate_and_unique_names(h: &Harness) {
    let s = &h.s1;
    let item = service(h, "Mowing");
    assert_eq!(
        item.fields.unit_price.map(Money::to_wire).as_deref(),
        Some("45.0450")
    );
    h.run(s.items().deactivate(item.id, key())).expect("off");
    let off = h.run(s.items().get(item.id)).expect("get");
    assert!(!off.record.is_active);
    let inactive = ItemFilter {
        is_active: Some(false),
        search: Some(item.fields.name.clone()),
        ..ItemFilter::default()
    };
    let page = h
        .run(s.items().list(&inactive, PageRequest::first_max()))
        .expect("list");
    assert_eq!(page.data.len(), 1);

    // The inactive item still holds its name.
    let dup = h
        .run(s.items().create(&item.fields, key()))
        .expect_err("used name");
    assert!(
        matches!(
            dup,
            ApiError::Conflict {
                reason: ConflictReason::AlreadyExists,
                ..
            }
        ),
        "{dup:?}"
    );

    let on = h
        .run(s.items().update(
            item.id,
            &off.version,
            &ItemUpdate {
                fields: off.record.fields.clone(),
                is_active: true,
            },
            key(),
        ))
        .expect("on");
    assert!(on.record.is_active);
    assert_eq!(on.record.fields, item.fields);
    h.run(s.items().deactivate(item.id, key())).expect("off");
}

/// U-39b: the purchase side and the unit label are not kept yet (the sales
/// kinds are U-39a, `sales_kinds_u39a`); PENDING until `item-types-v1`.
pub fn item_types_u39(h: &Harness) {
    let s = &h.s1;
    let income = account(h, s, AccountType::Revenue, "Discounts");
    let raw = h
        .run(s.raw(
            Method::Post,
            "/api/v1/items",
            &[],
            Some(&json!({
                "name": unique("Spring discount"),
                "incomeAccountId": income.id.to_string(),
                "itemType": "discount",
                "discountPercent": "0.1000",
                "unitCost": "12.0000",
                "uomLabel": "hour",
            })),
        ))
        .expect("raw");
    assert_eq!(raw.status, 201, "{}", raw.body);
    let item = &raw.body["item"];
    if h.require_cap(Capabilities::ITEM_TYPES, "U-39") {
        assert_eq!(item["itemType"], "discount", "{item}");
    } else {
        for f in ["unitCost", "uomLabel"] {
            assert!(item[f].is_null(), "{f} is not kept today: {item}");
        }
    }
    let id = item["id"].as_str().expect("id").to_string();
    let off = h
        .run(s.raw(Method::Delete, &format!("/api/v1/items/{id}"), &[], None))
        .expect("raw");
    assert_eq!(off.status, 200, "{}", off.body);
}

/// U-39a (`item-kinds-sales-v1`): a discount item keeps its kind and its
/// percent; a subtotal item takes no price.
pub fn sales_kinds_u39a(h: &Harness) {
    let s = &h.s1;
    if !h.require_cap(Capabilities::ITEM_KINDS_SALES, "U-39a") {
        return;
    }
    let income = account(h, s, AccountType::Revenue, "Discounts given");
    let pct = cl_api::SignedPercent::parse("10").expect("pct");
    let fields = ItemFields {
        name: unique("Loyalty discount"),
        description: None,
        unit_price: None,
        income_account_id: income.id,
        is_taxable: true,
        item_type: cl_api::ItemType::Discount,
        discount_percent: Some(pct),
    };
    let d = h.run(s.items().create(&fields, key())).expect("discount");
    assert_eq!(d.record.fields.item_type, cl_api::ItemType::Discount);
    assert_eq!(d.record.fields.discount_percent, Some(pct));
    let sub = ItemFields {
        name: unique("Subtotal"),
        item_type: cl_api::ItemType::Subtotal,
        discount_percent: None,
        ..fields
    };
    let t = h.run(s.items().create(&sub, key())).expect("subtotal");
    assert_eq!(t.record.fields.item_type, cl_api::ItemType::Subtotal);
    for id in [d.record.id, t.record.id] {
        h.run(s.items().deactivate(id, key())).expect("off");
    }
}

/// U-40 (`price-levels-v1`): a level with a % and an item price round-trips
/// through create, get, update and delete.
pub fn price_levels_u40(h: &Harness) {
    let s = &h.s1;
    let raw = h
        .run(s.raw(Method::Get, "/api/v1/price-levels", &[], None))
        .expect("raw");
    if h.require_cap(Capabilities::PRICE_LEVELS, "U-40") {
        assert_eq!(raw.status, 200, "{}", raw.body);
        let income = account(h, s, AccountType::Revenue, "Level sales");
        let item = h
            .run(s.items().create(
                &ItemFields {
                    name: unique("Shrubs"),
                    description: None,
                    unit_price: Some(cl_core::Money::parse_wire("32.50").expect("m")),
                    income_account_id: income.id,
                    is_taxable: true,
                    item_type: cl_api::ItemType::Service,
                    discount_percent: None,
                },
                key(),
            ))
            .expect("item");
        let levels = s.invoices();
        let levels = levels.price_levels();
        let draft = cl_api::PriceLevelDraft {
            name: unique("Contractor"),
            percent: cl_api::SignedPercent::parse("-7.5").expect("pct"),
            is_active: true,
            item_prices: vec![cl_api::ItemPrice {
                item_id: item.record.id,
                price: cl_core::Money::parse_wire("30").expect("m"),
            }],
        };
        let made = h.run(levels.create(&draft)).expect("create");
        assert_eq!(made.percent.to_wire(), "-7.5");
        let got = h.run(levels.get(made.id)).expect("get");
        assert_eq!(got.item_prices.len(), 1);
        let off = cl_api::PriceLevelDraft {
            is_active: false,
            ..draft
        };
        let upd = h.run(levels.update(made.id, &off)).expect("update");
        assert!(!upd.is_active);
        h.run(levels.delete(made.id)).expect("delete");
        h.run(s.items().deactivate(item.record.id, key()))
            .expect("off");
    } else {
        assert_eq!(raw.status, 404, "no such route today: {}", raw.body);
    }
}
