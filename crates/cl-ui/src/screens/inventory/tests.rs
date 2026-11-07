//! Shared inventory text and the demo reset.

use super::*;

#[test]
fn the_negative_stock_warning_names_item_date_and_on_hand() {
    let w = NegativeStock {
        item_id: crate::company::demo::lists::item_id(800),
        date: CalendarDate::from_ymd(2026, 11, 2).expect("d"),
        on_hand: Quantity::parse_wire("-2").expect("q"),
    };
    let text = negative_text("Mulch", &w);
    assert!(text.contains("Mulch"), "{text}");
    assert!(text.contains("11/02/2026"), "{text}");
    assert!(text.contains("-2 on hand"), "{text}");
    assert!(text.contains("You can save"), "{text}");
}

#[test]
fn the_closed_period_message_never_offers_the_password_way() {
    let text = closed_text(CalendarDate::from_ymd(2026, 9, 30).ok());
    assert!(text.contains("09/30/2026"), "{text}");
    assert!(text.contains("even with the closing password"), "{text}");
}

#[test]
fn refusals_read_in_plain_words() {
    let e = cl_api::engine_refusal(
        409,
        &serde_json::json!({"error": {"code": "INVENTORY_IN_USE", "message": "x"}}),
    );
    assert_eq!(refusal_text(&e, None), Some(t("inventory.refusal.in_use")));
    assert!(refusal_text(&ApiError::NotFound { code: None }, None).is_none());
}

#[test]
fn the_demo_reset_seeds_the_parts_again() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = CompanyLink::demo(dir.path());
    assert!(demo_reset(&link));
    assert!(demo_reset(&link), "twice is the same seed");
    let mut d = link.demo_state().expect("demo");
    let parts = d
        .items_mut()
        .iter()
        .filter(|i| i.fields.item_type == cl_api::ItemType::InventoryPart)
        .count();
    assert_eq!(parts, 3);
}
