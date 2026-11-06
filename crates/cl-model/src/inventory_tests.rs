//! Tests of the inventory forms (P24).

use super::*;

fn aid(n: u32) -> AccountId {
    AccountId::parse(&format!("d0000000-0000-4000-8000-a{n:011}")).expect("id")
}
fn iid(n: u32) -> ItemId {
    ItemId::parse(&format!("d0000000-0000-4000-8000-e{n:011}")).expect("id")
}
fn vid(n: u32) -> ContactId {
    ContactId::parse(&format!("d0000000-0000-4000-8000-c{n:011}")).expect("id")
}
fn q(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("q")
}
fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("m")
}
fn d(s: &str) -> CalendarDate {
    CalendarDate::parse_wire(s).expect("d")
}
fn ok(_: AccountId) -> bool {
    true
}

fn part() -> PartDraft {
    PartDraft {
        cost: "3.3333".into(),
        asset_account: Some(aid(1300)),
        cogs_account: Some(aid(5000)),
        reorder_point: "5".into(),
        reorder_quantity: "20".into(),
        unit_label: " each ".into(),
        ..PartDraft::default()
    }
}

#[test]
fn a_part_needs_its_accounts_and_keeps_exact_figures() {
    let c = check_part(&part(), true, false, None, ok).expect("part");
    assert_eq!(c.cost, Some(m("3.3333")));
    assert_eq!(c.reorder_point, Some(q("5")));
    assert_eq!(c.unit_label.as_deref(), Some("each"));
    assert_eq!(c.opening, None);
    let mut d0 = part();
    d0.asset_account = None;
    d0.cogs_account = Some(aid(9));
    let e = check_part(&d0, true, false, None, |a| a != aid(9)).expect_err("accounts");
    assert_eq!(
        e,
        vec![
            PartProblem::AssetAccountMissing,
            PartProblem::CogsAccountMissing
        ]
    );
    assert_eq!(e[0].field(), PartField::AssetAccount);
    // A service keeps its purchase side but no inventory fields.
    let svc = check_part(&d0, false, false, None, ok).expect("service");
    assert_eq!(svc.cost, Some(m("3.3333")));
    assert_eq!((svc.asset_account, svc.reorder_point), (None, None));
    let mut bad = part();
    bad.cost = "-1".into();
    bad.reorder_point = "x".into();
    bad.reorder_quantity = "-2".into();
    bad.unit_label = "abcdefghijk".into();
    bad.purchase_description = "x".repeat(PURCHASE_DESCRIPTION_MAX + 1);
    let e = check_part(&bad, true, false, None, ok).expect_err("bad");
    assert_eq!(
        e,
        vec![
            PartProblem::PurchaseDescriptionTooLong,
            PartProblem::CostInvalid,
            PartProblem::ReorderPointInvalid,
            PartProblem::ReorderQuantityInvalid,
            PartProblem::UnitLabelTooLong
        ]
    );
    for p in e {
        assert!(p.key().as_str().starts_with("inventory.problem."));
    }
}

#[test]
fn a_unit_set_names_the_default_units_and_drops_the_label() {
    let set = UnitSetId::parse("d0000000-0000-4000-8000-f00000000001").expect("set");
    let case = UnitId::parse("d0000000-0000-4000-8000-f00000000002").expect("unit");
    let mut p = part();
    p.purchase_unit = Some(case);
    let c = check_part(&p, true, false, None, ok).expect("no set");
    assert_eq!(
        (c.unit_set, c.purchase_unit, c.unit_label.as_deref()),
        (None, None, Some("each")),
        "default units need a set"
    );
    p.unit_set = Some(set);
    p.sale_unit = Some(case);
    let c = check_part(&p, true, false, None, ok).expect("set");
    assert_eq!(c.unit_set, Some(set));
    assert_eq!((c.purchase_unit, c.sale_unit), (Some(case), Some(case)));
    assert_eq!(c.unit_label, None, "the set names the units");
}

#[test]
fn opening_stock_on_a_new_part_only() {
    let mut p = part();
    p.opening_quantity = "10".into();
    p.opening_value = "33.33".into();
    p.opening_date = Some(d("2026-01-01"));
    let c = check_part(&p, true, true, None, ok).expect("new part");
    assert_eq!(
        c.opening,
        Some(CheckedOpening {
            quantity: q("10"),
            value: m("33.33"),
            as_of: d("2026-01-01")
        })
    );
    assert_eq!(
        check_part(&p, true, false, None, ok).expect("edit").opening,
        None
    );
    // Closed periods refuse inventory (owner 2026-11-06).
    let e = check_part(&p, true, true, Some(d("2026-01-01")), ok).expect_err("closed");
    assert_eq!(e, vec![PartProblem::OpeningInClosedPeriod]);
    let mut z = p.clone();
    z.opening_quantity = "0".into();
    assert_eq!(
        check_part(&z, true, true, None, ok),
        Err(vec![PartProblem::OpeningQuantityInvalid])
    );
    let mut no_date = p.clone();
    no_date.opening_date = None;
    assert_eq!(
        check_part(&no_date, true, true, None, ok),
        Err(vec![PartProblem::OpeningIncomplete])
    );
    let mut value_only = p;
    value_only.opening_quantity = String::new();
    assert_eq!(
        check_part(&value_only, true, true, None, ok),
        Err(vec![PartProblem::OpeningIncomplete])
    );
}

fn adj(mode: AdjustMode, rows: Vec<AdjustRow>) -> AdjustmentDraft {
    AdjustmentDraft {
        date: Some(d("2026-01-20")),
        account: Some(aid(6100)),
        memo: " broken ".into(),
        mode,
        rows,
    }
}

fn row(item: u32, quantity: &str, value: &str) -> AdjustRow {
    AdjustRow {
        item: Some(iid(item)),
        quantity: quantity.into(),
        value: value.into(),
        on_hand: Some(q("7")),
        unit: None,
    }
}

#[test]
fn adjustments_by_mode() {
    let c = check_adjustment(
        &adj(
            AdjustMode::Quantity,
            vec![row(1, "-1", "99"), AdjustRow::default()],
        ),
        None,
    )
    .expect("qty");
    assert_eq!(c.memo.as_deref(), Some("broken"));
    assert_eq!(
        c.lines,
        vec![CheckedAdjustLine {
            item: iid(1),
            quantity_change: Some(q("-1")),
            value_change: None,
            unit: None,
        }],
        "the value column is ignored in quantity mode"
    );
    assert_eq!(new_quantity(&row(1, "-1", "")), Some(q("6")));
    let v =
        check_adjustment(&adj(AdjustMode::Value, vec![row(1, "", "1.50")]), None).expect("value");
    assert_eq!(v.lines[0].value_change, Some(m("1.50")));
    assert_eq!(v.lines[0].quantity_change, None);
    let b =
        check_adjustment(&adj(AdjustMode::Both, vec![row(1, "2", "-2.50")]), None).expect("both");
    assert_eq!(b.lines[0].quantity_change, Some(q("2")));
    assert_eq!(b.lines[0].value_change, Some(m("-2.50")));

    let e = check_adjustment(
        &adj(
            AdjustMode::Quantity,
            vec![
                row(1, "0", ""),
                row(1, "2", ""),
                AdjustRow {
                    item: None,
                    quantity: "1".into(),
                    ..AdjustRow::default()
                },
            ],
        ),
        None,
    )
    .expect_err("rows");
    assert_eq!(
        e,
        vec![
            AdjustProblem::QuantityInvalid(0),
            AdjustProblem::ItemMissing(2)
        ]
    );
    let e = check_adjustment(
        &adj(AdjustMode::Quantity, vec![row(1, "1", ""), row(1, "2", "")]),
        None,
    )
    .expect_err("twice");
    assert_eq!(e, vec![AdjustProblem::ItemTwice(1)]);
    let e = check_adjustment(&adj(AdjustMode::Quantity, vec![]), Some(d("2026-01-31")))
        .expect_err("closed");
    assert_eq!(e, vec![AdjustProblem::ClosedPeriod]);
    let mut none = adj(AdjustMode::Quantity, vec![]);
    none.date = None;
    none.account = None;
    let e = check_adjustment(&none, None).expect_err("empty");
    assert_eq!(
        e,
        vec![AdjustProblem::DateMissing, AdjustProblem::AccountMissing]
    );
    assert_eq!(
        check_adjustment(&adj(AdjustMode::Quantity, vec![]), None),
        Err(vec![AdjustProblem::NothingToAdjust])
    );
    assert!(
        check_adjustment(&adj(AdjustMode::Value, vec![row(1, "", "1.005")]), None).is_ok(),
        "typed money rounds to the cent (Q-232)"
    );
}

#[test]
fn the_count_worksheet_posts_counted_rows_and_marks_stale_ones() {
    let mut w = CountWorksheet::new(
        d("2026-01-31"),
        [
            (iid(2), "Widget X".into(), None, q("4")),
            (iid(1), "widget W".into(), Some("each".into()), q("4")),
            (iid(3), "Widget Y".into(), None, q("4")),
        ],
    );
    assert_eq!(w.rows[0].item, iid(1), "name order, any case");
    assert!(!w.can_post(), "Post is disabled on an empty count");
    w.account = Some(aid(6100));
    assert_eq!(w.check(None), Err(vec![CountProblem::NothingCounted]));
    w.rows[0].counted = "3.5".into();
    w.rows[2].counted = "6".into();
    assert!(w.can_post());
    assert_eq!(w.difference(0), Some(q("-0.5")));
    assert_eq!(w.difference(1), None, "not counted");
    let lines = w.check(None).expect("lines");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].sheet_on_hand, q("4"));
    assert_eq!(lines[1].counted, q("6"));
    assert_eq!(
        w.check(Some(d("2026-01-31"))),
        Err(vec![CountProblem::ClosedPeriod])
    );
    w.rows[1].counted = "-1".into();
    assert_eq!(w.check(None), Err(vec![CountProblem::CountInvalid(1)]));
    w.rows[1].counted = String::new();
    // A sale entered meanwhile: the engine says STALE; the reload keeps the
    // typed counts and marks the moved row.
    w.reload(
        [(iid(1), q("3")), (iid(2), q("4")), (iid(3), q("4"))],
        &[iid(1)],
    );
    assert!(w.rows[0].changed && !w.rows[1].changed && !w.rows[2].changed);
    assert_eq!(w.rows[0].counted, "3.5");
    assert_eq!(w.difference(0), Some(q("0.5")));
    for p in [
        CountProblem::DateMissing,
        CountProblem::AccountMissing,
        CountProblem::NothingCounted,
        CountProblem::CountInvalid(0),
    ] {
        assert!(p.key().as_str().starts_with("inventory.count.problem."));
    }
}

#[test]
fn the_reorder_list_groups_by_vendor() {
    let r = |item: u32, name: &str, vendor: Option<u32>| ReorderRow {
        item: iid(item),
        name: name.into(),
        on_hand: q("2"),
        reorder_point: q("5"),
        reorder_quantity: Some(q("20")),
        in_purchase_unit: None,
        preferred_vendor: vendor.map(vid),
        average_cost: None,
    };
    let list = ReorderList::new(vec![
        r(1, "Zinc bolt", None),
        r(2, "Anchor", Some(1)),
        r(3, "bracket", Some(1)),
    ]);
    assert_eq!(list.count(), 3);
    assert_eq!(list.rows()[0].name, "Anchor");
    assert_eq!(list.rows()[1].name, "bracket");
    assert_eq!(list.rows()[2].preferred_vendor, None, "no vendor last");
    assert_eq!(list.for_vendor(Some(vid(1))).count(), 2);
    assert_eq!(list.for_vendor(None).count(), 1);
    assert_eq!(ReorderList::default().count(), 0);
    assert_eq!(quantity_text(q("3.5")), "3.5");
    assert_eq!(quantity_text(q("10")), "10");
}

/// U-161: a change or a count in another unit goes to the engine with its
/// unit; the desktop shows no base-unit figure it would have to convert.
#[test]
fn rows_in_a_unit_carry_it_and_derive_nothing() {
    let case = UnitId::parse("d0000000-0000-4000-8000-f00000000002").expect("unit");
    let mut r = row(1, "-1", "");
    r.on_hand = Some(q("24"));
    assert_eq!(new_quantity(&r), Some(q("23")));
    r.unit = Some(case);
    assert_eq!(new_quantity(&r), None);
    let draft = adj(AdjustMode::Quantity, vec![r]);
    let c = check_adjustment(&draft, None).expect("adjust");
    assert_eq!(c.lines[0].unit, Some(case));
    let mut w = CountWorksheet::new(d("2026-11-02"), [(iid(1), "Wine".into(), None, q("24"))]);
    w.account = Some(aid(1));
    w.rows[0].counted = "2".into();
    assert_eq!(w.difference(0), Some(q("-22")));
    w.rows[0].unit = Some(case);
    assert_eq!(w.difference(0), None);
    assert_eq!(w.check(None).expect("count")[0].unit, Some(case));
}
