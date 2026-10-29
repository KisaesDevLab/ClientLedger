use proptest::prelude::*;

use super::*;
use crate::forms::FormState;
use crate::forms::doc_read::{Confidence, ReadTarget};

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn qty(s: &str) -> Quantity {
    Quantity::parse_wire(s).expect("qty")
}

fn uuid(n: u64) -> String {
    format!("00000000-0000-0000-0000-{n:012}")
}

fn account(n: u64) -> AccountId {
    AccountId::parse(&uuid(n)).expect("id")
}

fn contact(n: u64) -> ContactId {
    ContactId::parse(&uuid(n)).expect("id")
}

fn txn(n: u64) -> TransactionId {
    TransactionId::parse(&uuid(n)).expect("id")
}

fn tag(n: u64) -> TagId {
    TagId::parse(&uuid(n)).expect("id")
}

fn ymd(y: i32, m: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, d).expect("date")
}

fn no_tags() -> TagList {
    TagList::new(Vec::new(), Vec::new())
}

const ALL: BillCaps = BillCaps {
    closing: true,
    lines: true,
    terms: true,
    received: true,
};

fn ctx<'a>(tags: &'a TagList, terms: &'a [PaymentTerm], caps: BillCaps) -> BillContext<'a> {
    BillContext {
        tags,
        terms,
        closing_date: None,
        original_date: None,
        caps,
    }
}

/// The mockup bill: Midwest Equipment Rental, 11/03/2026, INV-55871,
/// 1,330.00 expense + 150.00 item = 1,480.00.
fn mockup(keys: &mut RowKeyGen) -> BillForm {
    let mut f = BillForm::new(Some(ymd(2026, 11, 3)), keys);
    f.vendor = Some(contact(1));
    f.reference = "INV-55871".into();
    f.terms = Some("net_15".into());
    f.amount_due = Some(money("1480"));
    f.expenses.lines[0].account = Some(account(6210));
    f.expenses.lines[0].amount = Some(money("1330"));
    f.expenses.ensure_entry(keys);
    let k = f.items.lines[0].key;
    let l = f.items.get_mut(k).expect("line");
    l.description = "Delivery".into();
    l.account = Some(account(6210));
    l.set_qty(Some(qty("2")));
    l.set_rate(Some(money("75")));
    f.items.ensure_entry(keys);
    f
}

fn problems(form: &BillForm, c: &BillContext<'_>) -> Vec<BillProblem> {
    check(form, c).err().unwrap_or_default()
}

#[test]
fn a_new_form_is_a_bill() {
    let f = BillForm::default();
    assert_eq!(f.kind, BillKind::Bill);
}

#[test]
fn the_mockup_bill_checks_exactly() {
    let mut keys = RowKeyGen::new();
    let f = mockup(&mut keys);
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    assert_eq!(f.lines_total(), Some(money("1480")));
    assert_eq!(f.totals_status(), Some(TotalsStatus::Balanced));
    let c = check(&f, &ctx(&tags, &terms, ALL)).expect("valid");
    assert_eq!(c.amount, money("1480"));
    assert_eq!(c.reference.as_deref(), Some("INV-55871"));
    assert_eq!(c.expense_lines.len(), 1);
    assert_eq!(c.item_lines.len(), 1);
    assert_eq!(c.item_lines[0].amount, money("150"));
    assert_eq!(c.item_lines[0].qty, Some(qty("2")));
    assert_eq!(c.item_lines[0].rate, Some(money("75")));
    assert_eq!(c.item_lines[0].description.as_deref(), Some("Delivery"));
    assert_eq!(
        c.terms,
        Some(CheckedTerms {
            code: "net_15".into(),
            days: None,
            discount_percent: None,
            discount_days: None
        })
    );
    assert_eq!(c.due_date, None, "a terms date is not sent");
    assert_eq!(c.bill_received, Some(true));
    assert!(!c.needs_closing_password);
    let s = format!("{c:?}");
    assert!(!s.contains("1480") && !s.contains("INV"), "{s}");
    let s = format!("{f:?}");
    assert!(!s.contains("1480") && !s.contains("INV"), "{s}");
}

#[test]
fn one_cent_off_is_refused_with_the_exact_difference() {
    let mut keys = RowKeyGen::new();
    let mut f = mockup(&mut keys);
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    f.amount_due = Some(money("1480.01"));
    assert_eq!(
        f.totals_status(),
        Some(TotalsStatus::Remaining(money("0.01")))
    );
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::LinesDisagree {
            difference: money("0.01")
        }]
    );
    f.amount_due = Some(money("1479.99"));
    assert_eq!(f.totals_status(), Some(TotalsStatus::Over(money("0.01"))));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::LinesDisagree {
            difference: money("-0.01")
        }]
    );
}

#[test]
fn a_blank_amount_due_takes_the_lines_total() {
    let mut keys = RowKeyGen::new();
    let mut f = mockup(&mut keys);
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    f.amount_due = None;
    let c = check(&f, &ctx(&tags, &terms, ALL)).expect("valid");
    assert_eq!(c.amount, money("1480"));
    f.leave_lines();
    assert_eq!(f.amount_due, Some(money("1480")));
    assert!(f.amount_from_lines);
    // An amount from the lines follows them.
    f.expenses.lines[0].amount = Some(money("1000"));
    f.leave_lines();
    assert_eq!(f.amount_due, Some(money("1150")));
    // A typed amount never moves on its own.
    f.set_amount_due(Some(money("9")));
    f.leave_lines();
    assert_eq!(f.amount_due, Some(money("9")));
    // No amounts left: an amount from the lines clears.
    let mut g = BillForm::new(None, &mut keys);
    g.expenses.lines[0].amount = Some(money("5"));
    g.leave_lines();
    assert!(g.amount_from_lines);
    g.expenses.lines[0].amount = None;
    g.leave_lines();
    assert_eq!(g.amount_due, None);
    assert!(!g.amount_from_lines);
}

#[test]
fn header_problems() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let f = BillForm::new(None, &mut keys);
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![
            BillProblem::VendorMissing,
            BillProblem::DateMissing,
            BillProblem::AmountMissing,
            BillProblem::NoLines,
        ]
    );
    let mut f = mockup(&mut keys);
    f.reference = format!(" {} ", "R".repeat(REF_MAX));
    assert!(
        check(&f, &ctx(&tags, &terms, ALL)).is_ok(),
        "100 after trimming"
    );
    // Counted in UTF-16 units like cl-api: 50 emoji are 100 units (fits),
    // 51 are 102 (too long) though only 51 characters.
    f.reference = "\u{1F69C}".repeat(REF_MAX / 2);
    assert!(check(&f, &ctx(&tags, &terms, ALL)).is_ok(), "100 units");
    f.reference = "\u{1F69C}".repeat(REF_MAX / 2 + 1);
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::RefTooLong]
    );
    f.reference = "é".repeat(REF_MAX + 1);
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::RefTooLong]
    );
    let mut f = mockup(&mut keys);
    f.amount_due = Some(Money::ZERO);
    let p = problems(&f, &ctx(&tags, &terms, ALL));
    assert_eq!(p[0], BillProblem::AmountNotPositive);
    assert_eq!(BillProblem::AmountNotPositive.field(), BillField::AmountDue);
}

#[test]
fn line_problems() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let mut f = mockup(&mut keys);
    let e = f.expenses.lines[0].key;
    let i = f.items.lines[0].key;
    f.expenses.lines[0].account = None;
    f.items.lines[0].account = None;
    let found = problems(&f, &ctx(&tags, &terms, ALL));
    assert_eq!(
        found,
        vec![
            BillProblem::LineAccountMissing(e),
            BillProblem::LineAccountMissing(i)
        ]
    );
    assert_eq!(found[1].field(), BillField::LineAccount(i));

    let mut f = mockup(&mut keys);
    let e = f.expenses.lines[0].key;
    f.expenses.lines[0].amount = Some(money("-1"));
    f.amount_due = Some(money("149"));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::LineAmountNotPositive(e)]
    );

    let mut f = mockup(&mut keys);
    let e = f.expenses.lines[0].key;
    f.expenses.lines[0].billable = true;
    let found = problems(&f, &ctx(&tags, &terms, ALL));
    assert_eq!(found, vec![BillProblem::BillableNeedsCustomer(e)]);
    assert_eq!(found[0].field(), BillField::LineCustomer(e));
    f.expenses.lines[0].customer = Some(contact(9));
    let checked = check(&f, &ctx(&tags, &terms, ALL)).expect("ok");
    assert!(checked.expense_lines[0].billable);
    assert_eq!(checked.expense_lines[0].contact, Some(contact(9)));
}

#[test]
fn item_line_problems() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let c = ctx(&tags, &terms, ALL);
    let base = mockup(&mut keys);
    let i = base.items.lines[0].key;

    let mut f = base.clone();
    f.items.lines[0].qty = None;
    f.items.lines[0].amount = Some(money("150"));
    assert_eq!(problems(&f, &c), vec![BillProblem::ItemRateNeedsQty(i)]);
    assert_eq!(
        BillProblem::ItemRateNeedsQty(i).field(),
        BillField::LineQty(i)
    );

    let mut f = base.clone();
    f.items.lines[0].set_qty(Some(Quantity::ZERO));
    let p = problems(&f, &c);
    assert_eq!(p[0], BillProblem::ItemQtyNotPositive(i));

    let mut f = base.clone();
    f.items.lines[0].set_rate(Some(money("-75")));
    let p = problems(&f, &c);
    assert_eq!(p[0], BillProblem::ItemRateNegative(i));
    assert_eq!(p[0].field(), BillField::LineRate(i));

    let mut f = base.clone();
    f.items.lines[0].set_qty(Some(Quantity::MAX));
    f.items.lines[0].set_rate(Some(Money::MAX));
    assert_eq!(problems(&f, &c), vec![BillProblem::ItemAmountTooLarge(i)]);

    let mut f = base.clone();
    f.items.lines[0].set_rate(Some(Money::ZERO));
    f.amount_due = Some(money("1330"));
    assert_eq!(
        problems(&f, &c),
        vec![BillProblem::LineAmountNotPositive(i)]
    );

    // Qty + amount without a rate (S2).
    let mut f = base.clone();
    f.items.lines[0].set_rate(None);
    f.items.lines[0].set_amount(Some(money("150")));
    let ok = check(&f, &c).expect("valid");
    assert_eq!(ok.item_lines[0].qty, Some(qty("2")));
    assert_eq!(ok.item_lines[0].rate, None);
    assert_eq!(ok.item_lines[0].amount, money("150"));

    // Items need bill-lines-v1.
    let caps = BillCaps {
        lines: false,
        ..ALL
    };
    assert_eq!(
        problems(&base, &ctx(&tags, &terms, caps)),
        vec![BillProblem::ItemsNeedUpdate]
    );
    assert_eq!(BillProblem::ItemsNeedUpdate.field(), BillField::Items);
}

#[test]
fn too_many_lines_and_a_total_out_of_range() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let mut f = BillForm::new(Some(ymd(2026, 1, 1)), &mut keys);
    f.vendor = Some(contact(1));
    f.expenses.lines = (0..=MAX_LINES)
        .map(|_| FormLine {
            account: Some(account(1)),
            amount: Some(money("1")),
            ..FormLine::blank(keys.next_key())
        })
        .collect();
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::TooManyLines]
    );
    let mut g = BillForm::new(Some(ymd(2026, 1, 1)), &mut keys);
    g.vendor = Some(contact(1));
    g.expenses.lines = (0..2)
        .map(|_| FormLine {
            account: Some(account(1)),
            amount: Some(Money::MAX),
            ..FormLine::blank(keys.next_key())
        })
        .collect();
    g.amount_due = Some(Money::MAX);
    assert_eq!(g.lines_total(), None);
    assert_eq!(g.totals_status(), None);
    assert_eq!(
        problems(&g, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::LinesTotalTooLarge]
    );
}

#[test]
fn terms_rules() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let fixed = PaymentTerm::fixed();
    let mut f = mockup(&mut keys);
    f.terms = Some("net_45".into());
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, ALL)),
        vec![BillProblem::TermsUnknown]
    );
    f.terms = Some(CUSTOM_TERMS.into());
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, ALL)),
        vec![BillProblem::TermsDaysInvalid]
    );
    f.terms_days = Some(TERMS_DAYS_MAX + 1);
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, ALL)),
        vec![BillProblem::TermsDaysInvalid]
    );
    f.terms_days = Some(45);
    let c = check(&f, &ctx(&tags, &fixed, ALL)).expect("ok");
    assert_eq!(c.terms.as_ref().and_then(|t| t.days), Some(45));

    // Discounts: only with bill-terms-v1, percent and days together, days
    // fewer than the net days.
    f.discount_percent = Percent::parse_user("2%");
    f.discount_days = Some(10);
    let no_terms = BillCaps {
        terms: false,
        ..ALL
    };
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, no_terms)),
        vec![BillProblem::DiscountNeedsUpdate]
    );
    let c = check(&f, &ctx(&tags, &fixed, ALL)).expect("ok");
    let t = c.terms.expect("terms");
    assert_eq!(
        t.discount_percent.map(Percent::to_wire).as_deref(),
        Some("2.0000")
    );
    assert_eq!(t.discount_days, Some(10));
    f.discount_days = Some(45);
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, ALL)),
        vec![BillProblem::DiscountInvalid]
    );
    f.discount_days = None;
    assert_eq!(
        problems(&f, &ctx(&tags, &fixed, ALL)),
        vec![BillProblem::DiscountInvalid]
    );
    assert_eq!(BillProblem::DiscountInvalid.field(), BillField::Discount);

    // A fixed discount term from the engine list needs bill-terms-v1.
    let mut listed = fixed.clone();
    listed.push(PaymentTerm {
        code: "2_10_net_30".into(),
        net_days: Some(30),
        discount_percent: Percent::parse_user("2"),
        discount_days: Some(10),
    });
    let mut g = mockup(&mut keys);
    g.terms = Some("2_10_net_30".into());
    assert!(check(&g, &ctx(&tags, &listed, ALL)).is_ok());
    assert_eq!(
        problems(&g, &ctx(&tags, &listed, no_terms)),
        vec![BillProblem::DiscountNeedsUpdate]
    );
}

#[test]
fn percent_bounds() {
    assert_eq!(Percent::parse_user("0"), None);
    assert_eq!(Percent::parse_user("100"), None);
    assert_eq!(Percent::parse_user("-1"), None);
    assert_eq!(Percent::parse_user("1.00001"), None, "at most 4 decimals");
    assert_eq!(Percent::parse_user("abc"), None);
    let p = Percent::parse_user(" 99.9999 ").expect("ok");
    assert_eq!(p.value(), qty("99.9999"));
    assert_eq!(
        Percent::parse_user("0.0001")
            .map(Percent::to_wire)
            .as_deref(),
        Some("0.0001")
    );
}

#[test]
fn due_and_discount_overrides_s3() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let mut f = mockup(&mut keys);
    let net15 = terms.iter().find(|t| t.code == "net_15").expect("net_15");
    let d = preview_dates(ymd(2026, 11, 3), net15, None, None);
    assert_eq!(d.due, Some(ymd(2026, 11, 18)));
    assert_eq!(d.discount, None);
    f.apply_terms_dates(d);
    assert_eq!(f.due_date, Some(ymd(2026, 11, 18)));
    assert!(!f.due_overridden);
    assert_eq!(
        check(&f, &ctx(&tags, &terms, ALL)).expect("ok").due_date,
        None
    );

    // The owner's date wins and is sent; the terms no longer move it.
    f.set_due_by_owner(Some(ymd(2026, 11, 20)));
    f.apply_terms_dates(TermsDates {
        due: Some(ymd(2026, 12, 3)),
        discount: Some(ymd(2026, 11, 13)),
    });
    assert_eq!(f.due_date, Some(ymd(2026, 11, 20)));
    assert_eq!(f.discount_date, Some(ymd(2026, 11, 13)));
    assert_eq!(
        check(&f, &ctx(&tags, &terms, ALL)).expect("ok").due_date,
        Some(ymd(2026, 11, 20))
    );
    // Cleared: back to the terms.
    f.set_due_by_owner(None);
    assert!(!f.due_overridden);
    f.apply_terms_dates(d);
    assert_eq!(f.due_date, Some(ymd(2026, 11, 18)));

    // A due date before the bill date.
    f.set_due_by_owner(Some(ymd(2026, 11, 2)));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::DueBeforeDate]
    );
    assert_eq!(BillProblem::DueBeforeDate.field(), BillField::DueDate);
    f.set_due_by_owner(Some(ymd(2026, 11, 18)));

    // The discount date: needs bill-terms-v1, within date..=due.
    f.set_discount_by_owner(Some(ymd(2026, 11, 13)));
    assert!(f.discount_overridden);
    let c = check(&f, &ctx(&tags, &terms, ALL)).expect("ok");
    assert_eq!(c.discount_date, Some(ymd(2026, 11, 13)));
    let no_terms = BillCaps {
        terms: false,
        ..ALL
    };
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, no_terms)),
        vec![BillProblem::DiscountNeedsUpdate]
    );
    f.set_discount_by_owner(Some(ymd(2026, 11, 19)));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::DiscountDateInvalid]
    );
    f.set_discount_by_owner(Some(ymd(2026, 11, 1)));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::DiscountDateInvalid]
    );
    f.set_discount_by_owner(None);
    assert!(!f.discount_overridden);
}

#[test]
fn preview_dates_for_custom_terms_are_calendar_days() {
    let custom = PaymentTerm {
        code: CUSTOM_TERMS.into(),
        net_days: None,
        discount_percent: None,
        discount_days: None,
    };
    // Mar 1 + 15 crosses the US DST change: still Mar 16 (U-86c).
    let d = preview_dates(ymd(2026, 3, 1), &custom, Some(15), Some(10));
    assert_eq!(d.due, Some(ymd(2026, 3, 16)));
    assert_eq!(d.discount, Some(ymd(2026, 3, 11)));
    assert_eq!(
        preview_dates(ymd(2026, 3, 1), &custom, None, None),
        TermsDates::default()
    );
    assert_eq!(
        preview_dates(CalendarDate::max(), &custom, Some(1), None).due,
        None,
        "past the supported dates"
    );
}

#[test]
fn every_bill_is_sent_received() {
    // Q-P18h: "Not received yet" was removed; a bill goes as received
    // whenever the engine has the flag.
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let f = mockup(&mut keys);
    let no_recv = BillCaps {
        received: false,
        ..ALL
    };
    let c = check(&f, &ctx(&tags, &terms, ALL)).expect("ok");
    assert_eq!(c.bill_received, Some(true));
    let c = check(&f, &ctx(&tags, &terms, no_recv)).expect("ok");
    assert_eq!(c.bill_received, None, "not sent without the capability");
}
#[test]
fn a_credit_carries_no_terms_due_date_or_received_flag() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let mut f = mockup(&mut keys);
    f.kind = BillKind::Credit;
    f.terms = Some("net_45".into());
    f.set_due_by_owner(Some(ymd(2020, 1, 1)));
    f.set_discount_by_owner(Some(ymd(2020, 1, 1)));
    f.items.lines[0].billable = true;
    let no_caps = BillCaps {
        closing: true,
        lines: true,
        ..BillCaps::default()
    };
    let c = check(&f, &ctx(&tags, &terms, no_caps)).expect("a credit ignores them");
    assert_eq!(c.kind, BillKind::Credit);
    assert_eq!(c.terms, None);
    assert_eq!(c.due_date, None);
    assert_eq!(c.discount_date, None);
    assert_eq!(c.bill_received, None);
    assert!(
        !c.item_lines[0].billable,
        "credit lines are not billable (S2)"
    );
    // Switching back keeps what was typed.
    f.kind = BillKind::Bill;
    assert!(f.due_overridden);
    assert!(check(&f, &ctx(&tags, &terms, ALL)).is_err());
}

#[test]
fn line_detail_needs_bill_lines_v1() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let no_lines = BillCaps {
        lines: false,
        ..ALL
    };
    let mut f = mockup(&mut keys);
    f.items = ItemLines::from_lines(Vec::new(), &mut keys);
    f.amount_due = Some(money("1330"));
    let e = f.expenses.lines[0].key;
    assert!(check(&f, &ctx(&tags, &terms, no_lines)).is_ok());
    f.expenses.lines[0].customer = Some(contact(9));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, no_lines)),
        vec![BillProblem::LineDetailNeedsUpdate(e)]
    );
    f.expenses.lines[0].customer = None;
    f.expenses.lines[0].billable = true;
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, no_lines)),
        vec![BillProblem::LineDetailNeedsUpdate(e)]
    );
    f.expenses.lines[0].billable = false;
    // Credits drop line tags before bill-lines-v1; an explicit one is
    // refused, a header default is left off.
    f.kind = BillKind::Credit;
    f.header_tags = vec![tag(5)];
    let c = check(&f, &ctx(&tags, &terms, no_lines)).expect("ok");
    assert_eq!(c.expense_lines[0].tag, None);
    f.expenses.lines[0].tag = Some(tag(5));
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, no_lines)),
        vec![BillProblem::LineDetailNeedsUpdate(e)]
    );
    let c = check(&f, &ctx(&tags, &terms, ALL)).expect("ok");
    assert_eq!(c.expense_lines[0].tag, Some(tag(5)));
}

#[test]
fn closed_period_rules() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let f = mockup(&mut keys);
    let mut c = ctx(&tags, &terms, ALL);
    c.closing_date = Some(ymd(2026, 11, 3));
    assert!(
        check(&f, &c).expect("ok").needs_closing_password,
        "on the closing date"
    );
    c.closing_date = Some(ymd(2026, 11, 2));
    assert!(!check(&f, &c).expect("ok").needs_closing_password);
    c.original_date = Some(ymd(2026, 10, 1));
    assert!(
        check(&f, &c).expect("ok").needs_closing_password,
        "moving a bill out of a closed period"
    );
    c.caps.closing = false;
    assert_eq!(problems(&f, &c), vec![BillProblem::ClosedPeriodNeedsUpdate]);
    assert_eq!(
        BillProblem::ClosedPeriodNeedsUpdate.field(),
        BillField::Date
    );
}

#[test]
fn unaccepted_read_fields_block_save() {
    let mut keys = RowKeyGen::new();
    let tags = no_tags();
    let terms = PaymentTerm::fixed();
    let mut f = mockup(&mut keys);
    f.read.mark_read(ReadTarget::AmountDue, Confidence::High);
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::ReadNotAccepted]
    );
    assert_eq!(BillProblem::ReadNotAccepted.field(), BillField::ReadBanner);
    f.read.edited(ReadTarget::AmountDue);
    assert!(
        check(&f, &ctx(&tags, &terms, ALL)).is_ok(),
        "editing accepts"
    );
    // A read line that is gone no longer blocks.
    f.read
        .mark_read(ReadTarget::Line(RowKey(9999)), Confidence::Low);
    assert!(check(&f, &ctx(&tags, &terms, ALL)).is_ok());
    let k = f.expenses.lines[0].key;
    f.read.mark_read(ReadTarget::Line(k), Confidence::Low);
    assert_eq!(
        problems(&f, &ctx(&tags, &terms, ALL)),
        vec![BillProblem::ReadNotAccepted]
    );
    f.read.accept_all();
    assert!(check(&f, &ctx(&tags, &terms, ALL)).is_ok());
}

#[test]
fn every_problem_has_a_bill_key() {
    let k = RowKey(1);
    let all = [
        BillProblem::VendorMissing,
        BillProblem::DateMissing,
        BillProblem::ClosedPeriodNeedsUpdate,
        BillProblem::RefTooLong,
        BillProblem::TermsUnknown,
        BillProblem::TermsDaysInvalid,
        BillProblem::DiscountInvalid,
        BillProblem::DiscountNeedsUpdate,
        BillProblem::DueBeforeDate,
        BillProblem::DiscountDateInvalid,
        BillProblem::AmountMissing,
        BillProblem::AmountNotPositive,
        BillProblem::NoLines,
        BillProblem::TooManyLines,
        BillProblem::LinesTotalTooLarge,
        BillProblem::LinesDisagree {
            difference: Money::ZERO,
        },
        BillProblem::LineAccountMissing(k),
        BillProblem::LineAmountNotPositive(k),
        BillProblem::BillableNeedsCustomer(k),
        BillProblem::LineDetailNeedsUpdate(k),
        BillProblem::ItemsNeedUpdate,
        BillProblem::ItemRateNeedsQty(k),
        BillProblem::ItemQtyNotPositive(k),
        BillProblem::ItemRateNegative(k),
        BillProblem::ItemAmountTooLarge(k),
        BillProblem::ReadNotAccepted,
    ];
    let mut keys: Vec<&str> = all.iter().map(|p| p.key().as_str()).collect();
    assert!(keys.iter().all(|k| k.starts_with("bill.problem.")));
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), all.len(), "one key per problem");
    assert_eq!(BillProblem::NoLines.field(), BillField::Lines);
    assert_eq!(BillProblem::VendorMissing.field(), BillField::Vendor);
    assert_eq!(BillProblem::RefTooLong.field(), BillField::Reference);
    assert_eq!(BillProblem::TermsUnknown.field(), BillField::Terms);
    assert_eq!(
        BillProblem::DiscountDateInvalid.field(),
        BillField::DiscountDate
    );
    assert_eq!(
        BillProblem::ItemAmountTooLarge(k).field(),
        BillField::LineAmount(k)
    );
}

#[test]
fn prefill_copies_lines_without_amounts_as_one_step() {
    let mut keys = RowKeyGen::new();
    let mut last = mockup(&mut keys);
    last.memo = "Rental".into();
    last.terms_days = Some(3);
    last.header_tags = vec![tag(4)];
    let mut fresh = BillForm::new(Some(ymd(2026, 12, 1)), &mut keys);
    fresh.vendor = Some(contact(1));
    fresh.amount_due = Some(money("77"));
    assert!(fresh.can_prefill());
    let mut state = FormState::new(fresh);
    assert!(state.apply_prefill(1, &last));
    let f = state.current();
    assert_eq!(f.amount_due, Some(money("77")), "amount due untouched");
    assert_eq!(f.expenses.used().count(), 1);
    assert!(f.expenses.lines.iter().all(|l| l.amount.is_none()));
    assert_eq!(f.expenses.lines[0].account, Some(account(6210)));
    assert!(f.expenses.lines.last().is_some_and(FormLine::is_blank));
    assert_eq!(f.items.used().count(), 1);
    assert!(
        f.items
            .lines
            .iter()
            .all(|l| l.qty.is_none() && l.rate.is_none() && l.amount.is_none())
    );
    assert_eq!(f.items.lines[0].description, "Delivery");
    assert_eq!(f.memo, "Rental");
    assert_eq!(f.terms.as_deref(), Some("net_15"));
    assert_eq!(f.terms_days, Some(3));
    assert_eq!(f.header_tags, vec![tag(4)]);
    assert!(!f.can_prefill());
    assert!(state.undo());
    assert_eq!(state.current().expenses.used().count(), 0);
}

#[test]
fn vendor_default_fills_the_first_untouched_line() {
    let mut keys = RowKeyGen::new();
    let mut f = BillForm::new(None, &mut keys);
    f.amount_due = Some(money("12.5"));
    assert!(f.apply_vendor_default(account(6210), Some(tag(2)), &mut keys));
    assert_eq!(f.expenses.lines[0].account, Some(account(6210)));
    assert_eq!(f.expenses.lines[0].amount, Some(money("12.5")));
    assert_eq!(f.expenses.lines[0].tag, Some(tag(2)));
    assert_eq!(f.expenses.lines.len(), 2);
    assert!(
        !f.apply_vendor_default(account(1), None, &mut keys),
        "used now"
    );
    let mut g = BillForm {
        amount_due: Some(money("3")),
        amount_from_lines: true,
        ..BillForm::default()
    };
    assert!(g.apply_vendor_default(account(1), None, &mut keys));
    assert_eq!(
        g.expenses.lines[0].amount, None,
        "a lines total is not copied"
    );
}

#[test]
fn ref_matches_trims_and_case_folds_nothing_else() {
    assert!(ref_matches("INV-55871", " inv-55871\t"));
    assert!(ref_matches("Straße-1", "STRASSE-1"));
    assert!(ref_matches("ΟΔΟΣ", "οδος"), "final sigma folds");
    assert!(ref_matches("οδοσ", "οδος"));
    assert!(ref_matches("", "  "));
    assert!(!ref_matches("INV-55871", "INV 55871"));
    assert!(!ref_matches("INV-55871", "INV-558710"));
    assert!(!ref_matches("INV–1", "INV-1"), "no dash folding");
    // NFC vs NFD are different text: no normalisation.
    assert!(!ref_matches("caf\u{e9}", "cafe\u{301}"));
}

#[test]
fn duplicate_window_examples() {
    assert_eq!(
        duplicate_window(ymd(2026, 11, 3)),
        (ymd(2025, 5, 3), ymd(2028, 5, 3))
    );
    // Aug 31 − 18 months = Feb 28 (or 29 in a leap year), clamped.
    assert_eq!(duplicate_window(ymd(2026, 8, 31)).0, ymd(2025, 2, 28));
    assert_eq!(duplicate_window(ymd(2025, 8, 31)).0, ymd(2024, 2, 29));
    assert_eq!(duplicate_window(ymd(2026, 8, 31)).1, ymd(2028, 2, 29));
    // At the edges of the supported dates.
    assert_eq!(duplicate_window(CalendarDate::min()).0, CalendarDate::min());
    assert_eq!(duplicate_window(CalendarDate::max()).1, CalendarDate::max());
}

#[test]
fn find_duplicates_filters_exactly() {
    let date = ymd(2026, 11, 3);
    let row = |id: u64, vendor: u64, d: CalendarDate, r: &str, posted: bool| DuplicateCandidate {
        id: txn(id),
        vendor: contact(vendor),
        date: d,
        reference: r.into(),
        posted,
    };
    let rows = vec![
        row(1, 1, ymd(2025, 5, 3), "inv-55871", true), // exactly 18 months back
        row(2, 1, ymd(2025, 5, 2), "INV-55871", true), // one day further: out
        row(3, 1, ymd(2026, 1, 9), "INV-55871", false), // void
        row(4, 2, ymd(2026, 1, 9), "INV-55871", true), // other vendor
        row(5, 1, ymd(2026, 11, 3), "INV-55871", true), // this bill
        row(6, 1, ymd(2028, 5, 3), " INV-55871 ", true), // exactly 18 months on
        row(7, 1, ymd(2028, 5, 4), "INV-55871", true), // one day past
        row(8, 1, ymd(2026, 2, 1), "INV-5587", true),  // a different ref
    ];
    let got: Vec<TransactionId> =
        find_duplicates(contact(1), "INV-55871", date, Some(txn(5)), &rows)
            .into_iter()
            .map(|r| r.id)
            .collect();
    assert_eq!(got, vec![txn(1), txn(6)]);
    let got = find_duplicates(contact(1), "INV-55871", date, None, &rows);
    assert_eq!(got.len(), 3, "a new bill also sees row 5");
    assert_eq!(
        find_duplicates(contact(1), "  ", date, None, &rows).len(),
        0
    );
    assert!(!format!("{:?}", rows[0]).contains("inv"));
}

proptest! {
    /// Proptest (3): the window always contains the date and spans exactly
    /// 18 calendar months on each side, the day clamped to the month end.
    #[test]
    fn window_contains_the_date_and_spans_18_months(
        y in 1902_i32..=2197, m in 1_u32..=12, d in 1_u32..=31
    ) {
        let first = ymd(y, m, 1);
        let day = d.min(first.end_of_month().day());
        let date = ymd(y, m, day);
        let (from, to) = duplicate_window(date);
        prop_assert!(from <= date && date <= to);
        for (edge, sign) in [(from, -1_i32), (to, 1_i32)] {
            let months = (edge.year() - date.year()) * 12
                + i32::try_from(edge.month()).expect("m")
                - i32::try_from(date.month()).expect("m");
            prop_assert_eq!(months, sign * DUPLICATE_WINDOW_MONTHS);
            prop_assert_eq!(edge.day(), date.day().min(edge.end_of_month().day()));
        }
    }

    /// `ref_matches` ignores surrounding space and ASCII case, and never
    /// matches a different ASCII ref.
    #[test]
    fn ref_matches_is_trim_and_case_insensitive(
        a in "[A-Za-z0-9-]{1,12}", b in "[A-Za-z0-9-]{1,12}", pad in " {0,3}"
    ) {
        let padded = format!("{pad}{}{pad}", a.to_uppercase());
        prop_assert!(ref_matches(&a, &padded));
        prop_assert_eq!(ref_matches(&a, &b), a.eq_ignore_ascii_case(&b));
    }
}
