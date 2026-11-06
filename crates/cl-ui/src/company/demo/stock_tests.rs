use super::*;

fn day(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

fn on() -> DemoState {
    let mut d = DemoState::new();
    set_enabled(&mut d, true).expect("on");
    d
}

fn part_id(n: u32) -> ItemId {
    super::super::lists::item_id(FIRST_PART + n)
}

#[test]
fn switching_on_adds_the_sample_parts_once_with_their_stock() {
    let mut d = on();
    let parts: Vec<_> = d
        .items_mut()
        .iter()
        .filter(|i| i.fields.item_type == ItemType::InventoryPart)
        .map(|i| (i.fields.name.clone(), i.stock.quantity_on_hand))
        .collect();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].1, Some(q("120")));
    set_enabled(&mut d, true).expect("still on");
    assert!(d.inventory.adjustments.len() == 3, "seeded once");
}

#[test]
fn turning_off_with_stock_is_refused() {
    let mut d = on();
    let e = set_enabled(&mut d, false).expect_err("stock remains");
    assert_eq!(
        cl_api::inventory_refusal(&e),
        Some(cl_api::InventoryRefusal::InUse)
    );
    assert!(d.inventory.enabled);
    let mut fresh = DemoState::new();
    fresh.inventory.enabled = true;
    set_enabled(&mut fresh, false).expect("nothing on hand");
}

#[test]
fn closed_period_is_refused_before_the_password() {
    let mut d = on();
    let lines = doc_lines(
        &mut d,
        Side::Receipt,
        [(
            Some(part_id(0)),
            Some(q("2")),
            Money::parse_wire("6.50").expect("m"),
        )],
    );
    assert_eq!(lines.len(), 1);
    // Ozark Ridge is closed through 2026-09-30.
    let e = gate(&d, None, day(2026, 9, 30), &lines).expect_err("closed");
    assert_eq!(
        cl_api::inventory_refusal(&e),
        Some(cl_api::InventoryRefusal::PeriodClosed)
    );
    gate(&d, None, day(2026, 10, 2), &lines).expect("open");
    // A document without parts is not this gate's business.
    gate(&d, None, day(2026, 9, 1), &[]).expect("no parts");
}

#[test]
fn services_never_move_stock_and_lines_of_one_part_are_one_movement() {
    let mut d = on();
    let service = super::super::lists::item_id(1);
    let m = Money::parse_wire("10.00").expect("m");
    let lines = doc_lines(
        &mut d,
        Side::Sale,
        [
            (Some(service), Some(q("1")), m),
            (Some(part_id(2)), Some(q("2")), m),
            (Some(part_id(2)), Some(q("3")), m),
        ],
    );
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].kind, MoveKind::Sale { quantity: q("5") });
}

#[test]
fn overselling_warns_and_a_void_restores() {
    let mut d = on();
    let id = TransactionId::parse("d1000000-0000-4000-8000-000000000999").expect("id");
    let lines = doc_lines(
        &mut d,
        Side::Sale,
        [(Some(part_id(1)), Some(q("8")), Money::ZERO)],
    );
    let w = post(&mut d, id, TxnType::Invoice, day(2026, 11, 2), &lines).expect("posted");
    assert_eq!(w.len(), 1);
    assert_eq!(w[0].on_hand, q("-2"));
    assert_eq!(take_warnings(&mut d).len(), 1);
    assert!(take_warnings(&mut d).is_empty(), "read once");
    remove(&mut d, id).expect("removed");
    let s = d.inventory.stock(part_id(1)).expect("stock");
    assert_eq!(s.quantity_on_hand, Some(q("6")));
}

#[test]
fn a_count_on_a_stale_sheet_names_the_items() {
    let mut d = on();
    let sheet = count_sheet(&mut d, day(2026, 11, 4)).expect("sheet");
    let row = &sheet.rows[0];
    let c = NewCount {
        as_of: day(2026, 11, 4),
        adjustment_account_id: cogs_account(&mut d),
        memo: None,
        lines: vec![cl_api::CountLine {
            item_id: row.item_id,
            sheet_on_hand: q("1"),
            counted_quantity: q("3"),
            unit_id: None,
        }],
    };
    let e = post_count(&mut d, &c).expect_err("stale");
    assert_eq!(cl_api::count_stale_items(&e), Some(vec![row.item_id]));
    let ok = NewCount {
        lines: vec![cl_api::CountLine {
            item_id: row.item_id,
            sheet_on_hand: row.on_hand,
            counted_quantity: q("100"),
            unit_id: None,
        }],
        ..c
    };
    let r = post_count(&mut d, &ok).expect("posted");
    assert!(r.adjustment.is_some());
}

#[test]
fn reports_follow_the_replay() {
    let mut d = on();
    let v = valuation(&mut d, day(2026, 11, 4)).expect("valuation");
    assert_eq!(v.rows.len(), 3);
    assert_eq!(v.total_value, Money::parse_wire("785.40").ok());
    assert!(v.accounts.iter().all(|a| a.difference.is_zero()));
    let alerts = reorder_alerts(&mut d).expect("alerts");
    assert_eq!(alerts.len(), 1, "the fabric roll is at 6, point 10");
    let detail = valuation_detail(&mut d, day(2026, 1, 1), day(2026, 12, 31), None).expect("rows");
    assert_eq!(detail.len(), 3);
    assert_eq!(detail[0].txn_type, Some(TxnType::InventoryAdjustment));
    let before = valuation(&mut d, day(2026, 9, 30)).expect("before");
    assert_eq!(before.total_value, Some(Money::ZERO));
}

#[test]
fn routes_refuse_while_inventory_is_off() {
    let mut d = DemoState::new();
    let e = count_sheet(&mut d, day(2026, 11, 4)).expect_err("off");
    assert_eq!(
        cl_api::inventory_refusal(&e),
        Some(cl_api::InventoryRefusal::Off)
    );
}

/// U-161: an adjustment and a count entered in a unit move stock in base
/// units, `round4(q × factor)` half away from zero.
#[test]
fn adjustments_and_counts_in_a_unit_move_base_units() {
    let mut d = on();
    let unit = |name: &str, factor: &str| cl_api::Unit {
        id: None,
        name: name.into(),
        abbreviation: None,
        factor: cl_core::UnitFactor::parse_wire(factor).expect("factor"),
    };
    let set = d
        .units
        .create(&cl_api::NewUnitSet {
            name: "Bag".into(),
            base_unit_name: "bag".into(),
            base_unit_abbreviation: None,
            is_active: true,
            units: vec![unit("pallet", "40"), unit("third", "0.333333")],
        })
        .expect("set");
    let pallet = set.units[0].id;
    let third = set.units[1].id;
    let item = part_id(0);
    if let Some(i) = d.items_mut().iter_mut().find(|i| i.id == item) {
        i.fields.purchase.unit_set_id = Some(set.id);
    }
    let before = d.inventory.stock(item).expect("stock").quantity_on_hand;
    assert_eq!(before, Some(q("120")));
    let a = NewAdjustment {
        date: day(2026, 11, 2),
        adjustment_account_id: opening_account(&mut d),
        memo: None,
        lines: vec![cl_api::AdjustmentLine {
            item_id: item,
            quantity_change: Some(q("-1")),
            value_change: None,
            unit_id: pallet,
        }],
    };
    create_adjustment(&mut d, &a).expect("adjust");
    let after = d.inventory.stock(item).expect("stock").quantity_on_hand;
    assert_eq!(after, Some(q("80")), "one pallet is 40 bags");
    // Counted 3 thirds: round4(3 × 0.333333 = 0.999999) = 1.0000 bag.
    let c = NewCount {
        as_of: day(2026, 11, 3),
        adjustment_account_id: opening_account(&mut d),
        memo: None,
        lines: vec![cl_api::CountLine {
            item_id: item,
            sheet_on_hand: q("80"),
            counted_quantity: q("3"),
            unit_id: third,
        }],
    };
    post_count(&mut d, &c).expect("count");
    assert_eq!(
        d.inventory.stock(item).expect("stock").quantity_on_hand,
        Some(q("1"))
    );
}
