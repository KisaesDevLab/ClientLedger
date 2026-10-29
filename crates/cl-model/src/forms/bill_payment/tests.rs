use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn tid(n: u8) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
}

fn vid(n: u8) -> ContactId {
    ContactId::parse(&format!("c0000000-0000-4000-8000-0000000000{n:02x}")).expect("id")
}

const FEED: u8 = 1;
const FUEL: u8 = 2;
const TIRE: u8 = 3;
const OFFICE: u8 = 4;
const NURSERY: u8 = 5;
const CM114: u8 = 9;

fn row(n: u8, vendor: &str, reference: &str, due: CalendarDate, amount: &str) -> PayableRow {
    PayableRow {
        bill: tid(n),
        vendor: Some(vid(n)),
        vendor_name: vendor.into(),
        note: None,
        reference: Some(reference.into()),
        due: Some(due),
        discount_date: None,
        terms: None,
        balance_due: m(amount),
        discount_available: Money::ZERO,
    }
}

/// The mockup's five bills (listed through 11/11) and Table Rock's credit.
fn mockup() -> PayBillsForm {
    let mut office = row(OFFICE, "Ozark Office Supply", "7781", day(11, 10), "218.44");
    office.discount_date = Some(day(11, 6));
    office.discount_available = m("4.37");
    let rows = vec![
        row(FEED, "Ozark Feed & Seed", "20931", day(11, 5), "1286.40"),
        row(FUEL, "Riverbend Fuel", "STMT-1031", day(11, 6), "1412.06"),
        row(TIRE, "Monett Tire & Lube", "2214", day(11, 8), "640.00"),
        office,
        row(
            NURSERY,
            "Table Rock Nursery",
            "8813",
            day(11, 11),
            "3915.00",
        ),
    ];
    let credits = vec![OpenCredit {
        credit: tid(CM114),
        vendor: Some(vid(NURSERY)),
        date: day(10, 20),
        label: "CM-114".into(),
        open: m("250.00"),
    }];
    let mut f = PayBillsForm::new(day(11, 11));
    f.load(rows, credits);
    f
}

fn select_mockup(f: &mut PayBillsForm) {
    for n in [FEED, FUEL, OFFICE, NURSERY] {
        f.toggle(tid(n));
    }
}

/// P18 test: the mockup's totals to the penny (4 of 5 selected).
#[test]
fn the_mockup_totals_to_the_penny() {
    let mut f = mockup();
    select_mockup(&mut f);
    let t = f.totals().expect("totals");
    assert_eq!((t.listed, t.selected), (5, 4));
    // Q-P18b (owner 2026-10-28): every column totals the selected bills;
    // the five listed bills owe 7,471.90, the four selected 6,831.90.
    assert_eq!(t.amount_due, m("6831.90"));
    assert_eq!(t.discount, m("4.37"));
    assert_eq!(t.credits, m("250.00"));
    assert_eq!(t.cash, m("6577.53"));
    assert_eq!(f.ending_after(m("84210.55")), Some(m("77633.02")));
    // Per row, as the grid shows them.
    assert_eq!(f.cash(tid(OFFICE)), m("214.07"));
    assert_eq!(f.discount(tid(OFFICE)), m("4.37"));
    assert_eq!(f.cash(tid(NURSERY)), m("3665.00"));
    assert_eq!(f.credit_total(tid(NURSERY)), m("250.00"));
    assert_eq!(f.cash(tid(TIRE)), Money::ZERO, "not selected");
    // The plan the engine gets: cash, discount and the credit apart.
    let plan = f.plan(true, true).expect("plan");
    let bills: Vec<(TransactionId, Money, Money)> = plan
        .bills
        .iter()
        .map(|b| (b.bill, b.cash, b.discount))
        .collect();
    assert_eq!(
        bills,
        vec![
            (tid(FEED), m("1286.40"), Money::ZERO),
            (tid(FUEL), m("1412.06"), Money::ZERO),
            (tid(OFFICE), m("214.07"), m("4.37")),
            (tid(NURSERY), m("3665.00"), Money::ZERO),
        ]
    );
    assert_eq!(plan.credits.len(), 1);
    assert_eq!(plan.credits[0].credit, tid(CM114));
    assert_eq!(plan.credits[0].bill, tid(NURSERY));
    assert_eq!(plan.credits[0].amount, m("250.00"));
    let cash = Money::sum(plan.bills.iter().map(|b| b.cash)).expect("sum");
    assert_eq!(cash, m("6577.53"));
}

/// P18 test: a partial payment leaves the right balance.
#[test]
fn a_partial_payment_leaves_the_rest_owed() {
    let mut f = mockup();
    f.set_cash(tid(NURSERY), m("1000.00"));
    assert!(f.is_selected(tid(NURSERY)), "typing an amount selects");
    assert_eq!(f.credit_total(tid(NURSERY)), m("250.00"));
    let plan = f.plan(true, true).expect("plan");
    assert_eq!(plan.bills.len(), 1);
    let b = plan.bills[0];
    let applied = b
        .cash
        .checked_add(plan.credits[0].amount)
        .and_then(|x| x.checked_add(b.discount))
        .expect("sum");
    assert_eq!(applied, m("1250.00"));
    assert_eq!(
        m("3915.00").checked_sub(applied).expect("left"),
        m("2665.00"),
        "what the bill still owes"
    );
    assert_eq!(f.ending_after(m("84210.55")), Some(m("83210.55")));
}

/// Owner answer 2026-10-28: paid by credit card, the card's amount owed
/// goes up by the cash (the engine stores a card's balance as a credit).
#[test]
fn a_card_preview_shows_the_amount_owed_rising() {
    let mut f = mockup();
    select_mockup(&mut f);
    assert_eq!(
        f.preview(m("-1250.00"), true),
        Some((m("1250.00"), m("7827.53")))
    );
    assert_eq!(
        f.preview(m("84210.55"), false),
        Some((m("84210.55"), m("77633.02")))
    );
}

#[test]
fn discounts_follow_the_switch_and_set_discount() {
    let mut f = mockup();
    f.toggle(tid(OFFICE));
    assert_eq!(f.cash(tid(OFFICE)), m("214.07"));
    f.set_take_discounts(false);
    assert_eq!(f.discount(tid(OFFICE)), Money::ZERO);
    assert_eq!(
        f.cash(tid(OFFICE)),
        m("218.44"),
        "the cash follows the bill"
    );
    // Set Discount wins over the switch; None goes back to the engine's.
    f.set_discount(tid(OFFICE), Some(m("5.00")));
    assert_eq!(f.discount(tid(OFFICE)), m("5.00"));
    assert_eq!(f.cash(tid(OFFICE)), m("213.44"));
    f.set_take_discounts(true);
    f.set_discount(tid(OFFICE), None);
    assert_eq!(f.discount(tid(OFFICE)), m("4.37"));
    // An unselected bill takes no discount.
    f.toggle(tid(OFFICE));
    assert_eq!(f.discount(tid(OFFICE)), Money::ZERO);
    // Without bill-discounts-v1 a discount can't be sent.
    f.toggle(tid(OFFICE));
    assert_eq!(
        f.plan(true, false),
        Err(vec![PayProblem::DiscountNeedsUpdate(tid(OFFICE))])
    );
}

#[test]
fn the_rules_refuse_what_the_engine_would() {
    let mut f = mockup();
    assert_eq!(
        f.plan(false, true),
        Err(vec![
            PayProblem::AccountMissing,
            PayProblem::NothingSelected
        ])
    );
    // More than the bill owes (cash + credit).
    f.set_cash(tid(NURSERY), m("3665.01"));
    assert_eq!(
        f.plan(true, true),
        Err(vec![PayProblem::MoreThanDue(tid(NURSERY))])
    );
    // Nothing applied: no cash (typed while the credit was on) and then no
    // credit.
    f.set_cash(tid(NURSERY), Money::ZERO);
    f.set_credits(tid(NURSERY), Vec::new());
    assert_eq!(
        f.plan(true, true),
        Err(vec![PayProblem::NothingApplied(tid(NURSERY))])
    );
    // Negative cash.
    f.set_cash(tid(NURSERY), m("-1.00"));
    assert_eq!(
        f.plan(true, true),
        Err(vec![PayProblem::Negative(tid(NURSERY))])
    );
    // N4: fractions of a cent name the bill.
    f.set_cash(tid(NURSERY), m("100.005"));
    let e = f.plan(true, true).expect_err("sub-cent");
    assert_eq!(e, vec![PayProblem::SubCent(tid(NURSERY))]);
    assert_eq!(e[0].bill(), Some(tid(NURSERY)));
    // A credit used beyond what it has.
    f.set_cash(tid(NURSERY), m("100.00"));
    f.set_credits(tid(NURSERY), vec![(tid(CM114), m("250.01"))]);
    assert_eq!(
        f.plan(true, true),
        Err(vec![PayProblem::CreditOverused(tid(CM114))])
    );
    assert_eq!(PayProblem::MoreThanDue(tid(1)).bill(), Some(tid(1)));
    assert_eq!(
        PayProblem::NothingSelected.key().0,
        "paybills.problem.nothing_selected"
    );
}

#[test]
fn credits_are_shared_between_a_vendors_bills() {
    let mut f = mockup();
    // A second Table Rock bill of 100.00.
    let mut rows = f.all_rows().to_vec();
    let mut extra = row(10, "Table Rock Nursery", "8814", day(11, 9), "100.00");
    extra.vendor = Some(vid(NURSERY));
    rows.push(extra);
    let credits = f.open_credits().to_vec();
    f.load(rows, credits);
    f.toggle(tid(10));
    assert_eq!(f.credit_total(tid(10)), m("100.00"), "covers the bill");
    assert_eq!(f.cash(tid(10)), Money::ZERO);
    f.toggle(tid(NURSERY));
    assert_eq!(f.credit_total(tid(NURSERY)), m("150.00"), "what is left");
    assert_eq!(f.credit_left(tid(CM114), None), Money::ZERO);
    let plan = f.plan(true, true).expect("plan");
    assert_eq!(plan.credits.len(), 2);
    // Clearing a bill frees its credit.
    f.toggle(tid(10));
    assert_eq!(f.credit_left(tid(CM114), Some(tid(NURSERY))), m("250.00"));
}

#[test]
fn filters_sort_and_reload() {
    let mut f = mockup();
    f.show = ShowBills::DueOnOrBefore(day(11, 8));
    assert_eq!(f.visible().len(), 3);
    f.show = ShowBills::All;
    f.sort = SortBy::AmountDue;
    assert_eq!(f.visible()[0].bill, tid(NURSERY));
    f.sort = SortBy::Vendor;
    assert_eq!(f.visible()[0].vendor_name, "Monett Tire & Lube");
    f.vendor = Some(vid(FUEL));
    assert_eq!(f.visible().len(), 1);
    assert_eq!(f.vendors().len(), 5);
    // A filtered-out selection is kept, counted apart and not paid.
    f.vendor = None;
    f.toggle(tid(FEED));
    f.vendor = Some(vid(FUEL));
    let t = f.totals().expect("t");
    assert_eq!((t.selected, t.hidden), (0, 1));
    assert!(f.is_selected(tid(FEED)));
    assert_eq!(f.plan(true, true), Err(vec![PayProblem::NothingSelected]));
    f.vendor = None;
    assert_eq!(f.totals().expect("t").hidden, 0);
    // A reload keeps what is still listed.
    let rows: Vec<PayableRow> = f
        .all_rows()
        .iter()
        .filter(|r| r.bill != tid(FUEL))
        .cloned()
        .collect();
    f.toggle(tid(FUEL));
    f.load(rows, Vec::new());
    assert!(f.is_selected(tid(FEED)));
    assert!(!f.is_selected(tid(FUEL)));
    f.select_all();
    assert_eq!(f.totals().expect("t").selected, 4);
    f.clear();
    assert_eq!(f.totals().expect("t").selected, 0);
}

#[test]
fn debug_shows_no_amounts_or_names() {
    let mut f = mockup();
    select_mockup(&mut f);
    let shown = format!(
        "{f:?} {:?} {:?} {:?}",
        f.totals(),
        f.plan(true, true),
        f.all_rows()
    );
    for owner in ["7471", "6577", "218", "Ozark", "7781", "CM-114", "250"] {
        assert!(!shown.contains(owner), "{owner}: {shown}");
    }
}

/// Review (Low): credits fitted when the bill was ticked are fitted again
/// when the discount changes, so credits + discount never pass the bill.
#[test]
fn credits_are_fitted_again_when_the_discount_changes() {
    let mut f = mockup();
    // Table Rock's 250.00 credit against a 218.44 bill with a 4.37 discount
    // available on another date.
    let mut rows = f.all_rows().to_vec();
    let mut small = row(11, "Table Rock Nursery", "8815", day(11, 9), "218.44");
    small.vendor = Some(vid(NURSERY));
    rows.push(small.clone());
    let credits = f.open_credits().to_vec();
    f.load(rows.clone(), credits.clone());
    f.set_take_discounts(false);
    f.toggle(tid(11));
    assert_eq!(f.credit_total(tid(11)), m("218.44"), "the credit covers it");
    // The payment date moves into the discount window: the list is read
    // again with the engine's 4.37.
    small.discount_available = m("4.37");
    let rows: Vec<PayableRow> = rows
        .into_iter()
        .map(|r| if r.bill == tid(11) { small.clone() } else { r })
        .collect();
    f.load(rows, credits);
    f.set_take_discounts(true);
    assert_eq!(f.discount(tid(11)), m("4.37"));
    assert_eq!(f.credit_total(tid(11)), m("214.07"), "fitted again");
    assert!(f.plan(true, true).is_ok());
    // Set Credits' choice is kept.
    f.set_credits(tid(11), vec![(tid(CM114), m("100.00"))]);
    f.set_take_discounts(false);
    assert_eq!(f.credit_total(tid(11)), m("100.00"));
}

/// Review (Low): typing 0 on a bill with nothing else on it clears it.
#[test]
fn typing_zero_clears_a_plain_bill() {
    let mut f = mockup();
    f.toggle(tid(FEED));
    f.set_cash(tid(FEED), Money::ZERO);
    assert!(!f.is_selected(tid(FEED)));
    // A bill with a credit keeps it (the credit pays).
    f.toggle(tid(NURSERY));
    f.set_cash(tid(NURSERY), Money::ZERO);
    assert!(f.is_selected(tid(NURSERY)));
    // Paid bills come off the selection; the others stay.
    f.toggle(tid(FUEL));
    f.clear_bills(&[tid(NURSERY)]);
    assert!(!f.is_selected(tid(NURSERY)));
    assert!(f.is_selected(tid(FUEL)));
}
