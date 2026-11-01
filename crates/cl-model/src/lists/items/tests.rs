use super::*;

fn iid(n: u32) -> ItemId {
    ItemId::parse(&format!("10000000-0000-4000-8000-{n:012}")).expect("id")
}

fn aid(n: u32) -> AccountId {
    AccountId::parse(&format!("a0000000-0000-4000-8000-{n:012}")).expect("id")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn entry(n: u32, name: &str, active: bool) -> ItemEntry {
    ItemEntry {
        id: iid(n),
        name: name.to_string(),
        description: None,
        price: None,
        income_account: aid(1),
        taxable: true,
        active,
        kind: ItemKind::Service,
        discount_percent: None,
    }
}

fn sample() -> ItemList {
    let mut mowing = entry(1, "Mowing", true);
    mowing.description = Some("Weekly lawn mowing".into());
    mowing.price = Some(m("45.0000"));
    ItemList::new(vec![
        entry(3, "snow removal", true),
        mowing,
        entry(2, "Aeration", false),
    ])
}

fn income_ok(a: AccountId) -> bool {
    a == aid(1)
}

#[test]
fn sorted_by_name_ignoring_case() {
    let l = sample();
    let names: Vec<&str> = l.entries().iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Aeration", "Mowing", "snow removal"]);
    assert_eq!(l.len(), 3);
    assert!(!l.is_empty());
    assert_eq!(l.entry(iid(1)).map(|e| e.name.as_str()), Some("Mowing"));
}

#[test]
fn filter_hides_inactive_and_searches_description() {
    let l = sample();
    assert_eq!(l.visible(&ItemFilter::default()).len(), 2);
    let all = ItemFilter {
        show_inactive: true,
        ..ItemFilter::default()
    };
    assert_eq!(l.visible(&all).len(), 3);
    let lawn = ItemFilter {
        search: " LAWN ".into(),
        ..ItemFilter::default()
    };
    let rows = l.visible(&lawn);
    assert_eq!(rows.len(), 1);
    assert_eq!(l.entries()[rows[0]].name, "Mowing");
}

#[test]
fn inactive_items_keep_their_names() {
    let l = sample();
    assert_eq!(l.name_owner("aeration", None), Some(iid(2)));
    assert_eq!(l.name_owner("Aeration", Some(iid(2))), None);
    assert_eq!(l.name_owner("  ", None), None);
}

#[test]
fn only_services_until_the_engine_keeps_item_types() {
    assert_eq!(ItemKind::available(false), vec![ItemKind::Service]);
    assert_eq!(ItemKind::available(true).len(), 5);
    let mut d = ItemDraft::new(Some(aid(1)));
    d.name = "Delivery".into();
    d.kind = ItemKind::OtherCharge;
    let e = check(&d, &sample(), None, false, income_ok).expect_err("kind");
    assert_eq!(e, vec![ItemProblem::KindNotAvailable]);
    assert_eq!(e[0].field(), ItemField::Kind);
    assert!(check(&d, &sample(), None, true, income_ok).is_ok());
    for k in ItemKind::ALL {
        assert!(k.label().as_str().starts_with("items.kind."));
    }
}

#[test]
fn prices_are_exact_to_four_decimals() {
    assert_eq!(parse_price(""), Ok(None));
    assert_eq!(parse_price("12.5"), Ok(Some(m("12.5000"))));
    assert_eq!(parse_price("0.045"), Ok(Some(m("0.0450"))));
    assert_eq!(parse_price("$1,200.00"), Ok(Some(m("1200.0000"))));
    assert_eq!(parse_price("40*1.5"), Ok(Some(m("60.0000"))));
    assert_eq!(parse_price("-3"), Err(ItemProblem::PriceNegative));
    assert_eq!(parse_price("abc"), Err(ItemProblem::PriceInvalid));
    assert_eq!(parse_price("1.23456"), Err(ItemProblem::PriceInvalid));
}

/// Q-P10x rounds a calculation once to the field's unit; a price's unit is
/// a ten-thousandth, not a cent, and halves round away from zero.
#[test]
fn calculated_prices_round_to_four_decimals() {
    assert_eq!(parse_price("100/3"), Ok(Some(m("33.3333"))));
    assert_eq!(parse_price("2/3"), Ok(Some(m("0.6667"))));
    assert_eq!(parse_price("0.0001/2"), Ok(Some(m("0.0001"))));
    assert_eq!(parse_price("0.0003/2"), Ok(Some(m("0.0002"))));
    assert_eq!(parse_price("0.0001/3"), Ok(Some(m("0.0000"))));
}

#[test]
fn price_text_trims_past_the_cents() {
    assert_eq!(price_text(m("12.5000")), "12.50");
    assert_eq!(price_text(m("0.0450")), "0.045");
    assert_eq!(price_text(m("7")), "7.00");
    let d = ItemDraft::from_entry(&sample().entries()[1]);
    assert_eq!(d.price, "45.00");
    assert_eq!(d.description, "Weekly lawn mowing");
    assert_eq!(d.income_account, Some(aid(1)));
}

#[test]
fn check_reports_every_problem_in_field_order() {
    let mut d = ItemDraft::new(None);
    d.price = "x".into();
    d.description = "d".repeat(DESCRIPTION_MAX + 1);
    let e = check(&d, &sample(), None, false, income_ok).expect_err("bad");
    assert_eq!(
        e,
        vec![
            ItemProblem::NameMissing,
            ItemProblem::DescriptionTooLong,
            ItemProblem::PriceInvalid,
            ItemProblem::IncomeAccountMissing,
        ]
    );
    for p in &e {
        assert!(p.key().as_str().starts_with("items.problem."));
    }
    d = ItemDraft::new(Some(aid(9)));
    d.name = "n".repeat(NAME_MAX + 1);
    let e = check(&d, &sample(), None, false, income_ok).expect_err("bad");
    assert_eq!(
        e,
        vec![ItemProblem::NameTooLong, ItemProblem::IncomeAccountMissing]
    );
    d = ItemDraft::new(Some(aid(1)));
    d.name = "mowing".into();
    let e = check(&d, &sample(), None, false, income_ok).expect_err("taken");
    assert_eq!(e, vec![ItemProblem::NameTaken]);
    assert!(check(&d, &sample(), Some(iid(1)), false, income_ok).is_ok());
}

#[test]
fn checked_item_is_trimmed() {
    let mut d = ItemDraft::new(Some(aid(1)));
    d.name = "  Edging ".into();
    d.description = "  ".into();
    d.price = " 30 ".into();
    d.taxable = false;
    let c = check(&d, &sample(), None, false, income_ok).expect("ok");
    assert_eq!(
        c,
        CheckedItem {
            name: "Edging".into(),
            description: None,
            price: Some(m("30.0000")),
            income_account: aid(1),
            taxable: false,
            kind: ItemKind::Service,
            discount_percent: None,
        }
    );
}

/// U-39a: discount and subtotal items need an income account; a discount
/// takes a percent or an amount, not both; a subtotal takes no price.
#[test]
fn sales_kinds_are_checked() {
    let mut d = ItemDraft::new(None);
    d.name = "Loyalty".into();
    d.kind = ItemKind::Discount;
    d.discount = "10".into();
    let e = check(&d, &sample(), None, true, income_ok).expect_err("no account");
    assert_eq!(e, vec![ItemProblem::IncomeAccountMissing]);
    d.income_account = Some(aid(1));
    let c = check(&d, &sample(), None, true, income_ok).expect("ok");
    assert_eq!(c.kind, ItemKind::Discount);
    assert_eq!(
        c.discount_percent
            .map(cl_core::Quantity::to_wire)
            .as_deref(),
        Some("10.0000")
    );
    d.price = "5".into();
    let e = check(&d, &sample(), None, true, income_ok).expect_err("both");
    assert_eq!(e, vec![ItemProblem::DiscountBoth]);
    d.price.clear();
    d.discount = "150".into();
    let e = check(&d, &sample(), None, true, income_ok).expect_err("over 100");
    assert_eq!(e, vec![ItemProblem::DiscountInvalid]);
    let mut s = ItemDraft::new(Some(aid(1)));
    s.name = "Subtotal".into();
    s.kind = ItemKind::Subtotal;
    s.price = "1".into();
    let e = check(&s, &sample(), None, true, income_ok).expect_err("priced");
    assert_eq!(e, vec![ItemProblem::SubtotalPriced]);
    s.price.clear();
    assert!(
        check(&s, &sample(), None, false, income_ok).is_err(),
        "needs the engine"
    );
}

/// P11 acceptance: 20k-record lists filter under 100 ms.
#[test]
fn twenty_thousand_items_filter_quickly() {
    let entries: Vec<ItemEntry> = (0..20_000u32)
        .map(|n| {
            let mut e = entry(n, &format!("Item {n:05}"), n % 9 != 0);
            e.description = Some(format!("Service number {n}"));
            e
        })
        .collect();
    let list = ItemList::new(entries);
    let filter = ItemFilter {
        show_inactive: false,
        search: "item 19".into(),
    };
    let ((rows, all), took) = crate::lists::fastest_of(5, || {
        (list.visible(&filter), list.visible(&ItemFilter::default()))
    });
    assert!(!rows.is_empty() && !all.is_empty());
    assert!(took.as_millis() < 100, "filtering took {took:?}");
}
