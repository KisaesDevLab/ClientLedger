use super::*;
use crate::layout::analyze;
use crate::pipeline::testdoc::{Doc, invoice};

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).unwrap()
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn run(d: &Doc) -> Extracted {
    let pages = d.pages();
    extract(&analyze(&pages), &pages, None, today())
}

#[test]
fn reads_a_plain_invoice() {
    let d = invoice(
        "RB-1042",
        "07/14/2026",
        &[
            ("Canvas tarp", "12", "24.50", "294.00"),
            ("Rope", "2", "5.00", "10.00"),
        ],
        "304.00",
        "25.08",
        "329.08",
    );
    let e = run(&d);
    let h = &e.header;
    assert_eq!(h.vendor.as_ref().unwrap().value, "Riverbend Supply Co.");
    assert_eq!(
        h.vendor_address.as_ref().unwrap().value,
        "1200 Mill Road\nSpringfield, MO 65801"
    );
    assert_eq!(h.vendor_phone.as_ref().unwrap().value, "(417) 555-0142");
    assert_eq!(h.reference.as_ref().unwrap().value, "RB-1042");
    assert_eq!(
        h.date.as_ref().unwrap().value,
        CalendarDate::from_ymd(2026, 7, 14).unwrap()
    );
    let terms = &h.terms.as_ref().unwrap().value;
    assert_eq!((terms.text.as_str(), terms.days), ("Net 30", Some(30)));
    assert_eq!(h.subtotal.as_ref().unwrap().value, m("304"));
    assert_eq!(h.tax.as_ref().unwrap().value, m("25.08"));
    assert_eq!(h.total.as_ref().unwrap().value, m("329.08"));
    assert_eq!(
        h.total.as_ref().unwrap().rule.as_str(),
        "fields.total.label-right"
    );
    assert!(h.total.as_ref().unwrap().confidence >= Confidence::HIGH);
    assert!(e.checks.is_empty(), "{:?}", e.checks);
    assert!(e.signals.looks_like_bill);
    assert!(h.payment.is_none());
}

#[test]
fn values_below_their_labels() {
    let mut d = Doc::new();
    d.text(500, 400, "Hilltop Feed");
    d.text(5000, 1000, "Invoice No.");
    d.text(7000, 1000, "Invoice Date");
    d.text(5000, 1150, "004521");
    d.text(7000, 1150, "July 3rd, 2026");
    d.text(6000, 6000, "Amount Due");
    d.amount(9500, 6000, "$1,250.00");
    let h = run(&d).header;
    assert_eq!(h.reference.as_ref().unwrap().value, "004521");
    assert_eq!(
        h.reference.as_ref().unwrap().rule.as_str(),
        "fields.reference.label-below"
    );
    assert_eq!(
        h.date.unwrap().value,
        CalendarDate::from_ymd(2026, 7, 3).unwrap()
    );
    assert_eq!(
        h.total.unwrap().value,
        m("1250"),
        "amount due is the total when no total"
    );
}

#[test]
fn amount_due_is_the_balance_when_a_total_is_printed() {
    let mut d = Doc::new();
    d.text(500, 400, "Hilltop Feed");
    d.text(6000, 6000, "Total");
    d.amount(9500, 6000, "500.00");
    d.text(6000, 6200, "Payments");
    d.amount(9500, 6200, "(200.00)");
    d.text(6000, 6400, "Amount Due");
    d.amount(9500, 6400, "300.00");
    let h = run(&d).header;
    assert_eq!(h.total.unwrap().value, m("500"));
    assert_eq!(h.amount_paid.unwrap().value, m("-200"));
    assert_eq!(h.balance_due.unwrap().value, m("300"));
}

#[test]
fn bill_to_block_is_never_the_vendor() {
    let mut d = Doc::new();
    d.text(500, 300, "Bill To:");
    d.text(500, 450, "Ozark Ridge Farm");
    d.text(6000, 300, "Northside Electric LLC");
    let h = run(&d).header;
    assert_eq!(h.vendor.unwrap().value, "Northside Electric LLC");
}

#[test]
fn ambiguous_date_gets_a_day_first_check() {
    let mut d = Doc::new();
    d.text(6000, 1000, "Date: 07/08/2026");
    let e = run(&d);
    assert_eq!(
        e.header.date.unwrap().confidence.bucket(),
        crate::model::ConfidenceBucket::Medium
    );
    assert_eq!(e.checks.len(), 1);
    assert_eq!(e.checks[0].code, CheckCode::DateCouldBeDayFirst);
    assert_eq!(e.checks[0].field, Some(FieldName::Date));
}

#[test]
fn receipt_signs() {
    let mut d = Doc::new();
    d.text(3000, 300, "Corner Hardware");
    d.text(500, 3000, "TOTAL");
    d.amount(9500, 3000, "54.82");
    d.text(500, 3200, "VISA ************4821");
    d.amount(9500, 3200, "54.82");
    let e = run(&d);
    assert_eq!(e.signals.card.as_ref().unwrap().value.as_str(), "4821");
    assert!(matches!(
        e.header.payment.as_ref().unwrap().value,
        Payment::Card { .. }
    ));
    assert_eq!(e.header.amount_paid.unwrap().value, m("54.82"));
    assert_eq!(e.header.total.unwrap().value, m("54.82"));

    let mut d = Doc::new();
    d.text(500, 3000, "Total 12.40");
    d.text(500, 3200, "Cash 20.00");
    d.text(500, 3400, "Change 7.60");
    let e = run(&d);
    assert!(e.signals.cash.is_some() && e.signals.change);
    assert!(matches!(e.header.payment.unwrap().value, Payment::Cash));

    let mut d = Doc::new();
    d.text(500, 3000, "Paid by Check # 1187");
    let e = run(&d);
    assert_eq!(e.signals.check.unwrap().value, "1187");

    let mut d = Doc::new();
    d.text(4000, 2000, "PAID");
    d.text(500, 3000, "Visa ending in 4821");
    let e = run(&d);
    assert!(e.signals.paid_stamp.is_some());
    assert_eq!(e.signals.card.unwrap().value.as_str(), "4821");
    assert!(
        e.header.amount_paid.is_none(),
        "card digits are not an amount"
    );
}

/// M7: no labelled total and several lone numbers; only the one equal to
/// subtotal + tax is picked, with a check.
#[test]
fn unlabelled_total_picked_by_arithmetic_is_flagged() {
    let mut d = Doc::new();
    d.text(6000, 6000, "Subtotal");
    d.amount(9500, 6000, "100.00");
    d.text(6000, 6200, "Tax");
    d.amount(9500, 6200, "8.25");
    d.amount(9500, 6600, "108.25");
    d.amount(9500, 7000, "250.00");
    let e = run(&d);
    assert_eq!(e.header.total.as_ref().unwrap().value, m("108.25"));
    assert!(e.header.total.as_ref().unwrap().confidence.bp() <= PICKED);
    assert_eq!(e.checks.len(), 1);
    assert_eq!(e.checks[0].code, CheckCode::PickedUsingTotals);

    // Nothing matches: no total rather than a guess.
    let mut d = Doc::new();
    d.text(6000, 6000, "Subtotal");
    d.amount(9500, 6000, "100.00");
    d.amount(9500, 6600, "99.00");
    d.amount(9500, 7000, "250.00");
    assert!(run(&d).header.total.is_none());
}

/// M7: a labelled total that does not reconcile is kept, never swapped
/// for a number that balances.
#[test]
fn labelled_total_is_never_replaced() {
    let d = invoice(
        "A-1",
        "07/14/2026",
        &[("Tarp", "1", "100.00", "100.00")],
        "100.00",
        "8.25",
        "109.25",
    );
    let pages = d.pages();
    let e = extract(&analyze(&pages), &pages, None, today());
    assert_eq!(e.header.total.unwrap().value, m("109.25"));
}

#[test]
fn template_values_win_with_a_disagreement_check() {
    let doc = invoice(
        "RB-1042",
        "07/14/2026",
        &[("Tarp", "1", "5.00", "5.00")],
        "5.00",
        "0.41",
        "5.41",
    );
    let pages = doc.pages();
    let layout = analyze(&pages);
    // A template that points the total at the subtotal's spot.
    let sub_spot = extract(&layout, &pages, None, today())
        .header
        .subtotal
        .unwrap()
        .spot;
    let template = Template {
        fingerprint: templates::fingerprint(&pages).unwrap(),
        fields: vec![crate::model::TemplateField {
            field: FieldName::Total,
            page: 0,
            region: sub_spot.bbox,
            label: None,
        }],
        line_table: None,
        samples: 1,
    };
    let got = extract(&layout, &pages, Some(&template), today());
    assert!(got.template_used);
    assert_eq!(got.header.total.as_ref().unwrap().value, m("5.00"));
    assert_eq!(got.header.total.as_ref().unwrap().rule.as_str(), "template");
    let c = got
        .checks
        .iter()
        .find(|c| c.code == CheckCode::ReadersDisagree)
        .unwrap();
    assert_eq!(c.field, Some(FieldName::Total));
    assert!(matches!(c.expected, Some(CheckValue::Money(v)) if v == m("5.41")));
}

#[test]
fn terms_days_table() {
    for (text, want) in [
        ("Net 30", Some(30)),
        ("NET30", Some(30)),
        ("n/45", Some(45)),
        ("2/10 Net 30", Some(30)),
        ("2% 10, Net 60", Some(60)),
        ("Due on receipt", Some(0)),
        ("Due in 15 days", Some(15)),
        ("COD", Some(0)),
        ("See contract", None),
    ] {
        assert_eq!(terms_days(text), want, "{text}");
    }
}

#[test]
fn phone_table() {
    assert_eq!(
        phone_in("Tel (417) 555-0142 ext 3").as_deref(),
        Some("(417) 555-0142")
    );
    assert_eq!(phone_in("417.555.0142").as_deref(), Some("417.555.0142"));
    assert_eq!(
        phone_in("+1 417-555-0142").as_deref(),
        Some("+1 417-555-0142")
    );
    assert_eq!(
        phone_in("4175550142"),
        None,
        "no separator: could be anything"
    );
    assert_eq!(phone_in("Springfield, MO 65801"), None);
}

#[test]
fn ocr_word_confidence_caps_the_field() {
    let mut low = Doc::new();
    low.confidence = Confidence::saturating(5_000);
    low.text(6000, 6000, "Total");
    low.amount(9500, 6000, "12.00");
    let h = run(&low).header;
    assert_eq!(h.total.unwrap().confidence.bp(), 5_000);
}

fn ymd(y: i32, mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, mo, d).unwrap()
}

/// A register receipt prints its date bare; "MAR 04 2026" style too.
#[test]
fn unlabelled_receipt_dates() {
    let mut d = Doc::new();
    d.text(3000, 300, "FreshWay Market");
    d.text(500, 1800, "09/26/2025 12:52 AM");
    d.text(500, 2100, "TRANS # 9007");
    d.text(1800, 6500, "TOTAL");
    d.amount(9500, 6500, "104.23");
    let h = run(&d).header;
    let date = h.date.unwrap();
    assert_eq!(date.value, ymd(2025, 9, 26));
    assert_eq!(date.rule.as_str(), "fields.date.unlabelled");
    assert!(date.confidence < Confidence::HIGH);
    assert_eq!(h.reference.unwrap().value, "9007");

    let mut d = Doc::new();
    d.text(3000, 300, "BLUEBIRD CAFE");
    d.text(500, 2200, "MAR 04 2026 05:43 PM");
    d.text(500, 2600, "INVOICE 755249");
    let h = run(&d).header;
    assert_eq!(h.date.unwrap().value, ymd(2026, 3, 4));
    assert_eq!(h.reference.unwrap().value, "755249");

    // A due date alone is not the document's date.
    let mut d = Doc::new();
    d.text(500, 300, "Acme Supply");
    d.text(500, 1200, "Due Date 11/30/2026");
    assert!(run(&d).header.date.is_none());
}

#[test]
fn reference_labels() {
    for (label, value) in [
        ("Invoice ID", "CN-2026-15498"),
        ("No.:", "578"),
        ("Check #", "1629"),
    ] {
        let mut d = Doc::new();
        d.text(500, 300, "Some Vendor LLC");
        d.text(500, 1600, &format!("{label} {value}"));
        let h = run(&d).header;
        assert_eq!(h.reference.unwrap().value, value, "{label}");
    }
    // "Invoice" with its value across the line, and in a grid below.
    let mut d = Doc::new();
    d.text(500, 300, "Tri-County Auto Parts");
    d.text(5400, 1600, "Invoice");
    d.text(8400, 1600, "L3-99240");
    assert_eq!(run(&d).header.reference.unwrap().value, "L3-99240");
    let mut d = Doc::new();
    d.text(500, 300, "Paperwise Office Supply");
    d.text(1300, 1600, "Invoice");
    d.text(8000, 1600, "Order No.");
    d.text(1300, 1800, "6697176");
    d.text(8000, 1800, "1024418");
    assert_eq!(run(&d).header.reference.unwrap().value, "6697176");
    // A title "PARTS INVOICE" does not take an address beside it.
    let mut d = Doc::new();
    d.text(500, 600, "PARTS INVOICE");
    d.text(7000, 600, "915 W Walnut St");
    assert!(run(&d).header.reference.is_none());
}

#[test]
fn statements_have_a_balance_and_no_reference() {
    let mut d = Doc::new();
    d.text(500, 400, "Prairie Lumber & Hardware");
    d.text(7000, 400, "STATEMENT");
    d.text(5400, 1600, "Statement Date");
    d.text(8300, 1600, "09/16/2025");
    d.text(2200, 2400, "Reference");
    d.text(2200, 2600, "INV A95423");
    d.text(5400, 3500, "Balance Due");
    d.amount(9500, 3500, "$617.90");
    let e = run(&d);
    assert!(e.signals.statement);
    let h = e.header;
    assert!(h.reference.is_none());
    assert_eq!(h.date.unwrap().value, ymd(2025, 9, 16));
    let total = h.total.unwrap();
    assert_eq!(total.value, m("617.90"));
    assert_eq!(total.rule.as_str(), "fields.total.balance");
}

#[test]
fn amount_paid_is_the_total_when_none_is_printed() {
    let mut d = Doc::new();
    d.text(500, 650, "Receipt");
    d.text(6500, 650, "Cloudnest Software, Inc.");
    d.text(5400, 1600, "Invoice ID");
    d.text(8000, 1600, "CN-2026-15498");
    d.text(5400, 1750, "Date paid");
    d.text(7700, 1750, "September 27, 2025");
    d.text(5400, 3100, "Subtotal");
    d.amount(9500, 3100, "$43.02");
    d.text(5400, 3350, "Amount paid");
    d.amount(9500, 3350, "$43.02");
    d.text(5400, 3650, "Paid with Discover ending in 1172");
    let e = run(&d);
    let h = &e.header;
    assert_eq!(h.date.as_ref().unwrap().value, ymd(2025, 9, 27));
    assert_eq!(h.reference.as_ref().unwrap().value, "CN-2026-15498");
    let total = h.total.as_ref().unwrap();
    assert_eq!(total.value, m("43.02"));
    assert_eq!(total.rule.as_str(), "fields.total.amount-paid");
    // The card's last four digits are never an amount.
    assert_eq!(h.amount_paid.as_ref().unwrap().value, m("43.02"));
}

#[test]
fn an_amount_after_payment_due_is_the_amount_due() {
    let mut d = Doc::new();
    d.text(500, 400, "Ozark Mutual");
    d.text(5400, 3000, "Payment Due");
    d.amount(9500, 3000, "$84.10");
    assert_eq!(run(&d).header.total.unwrap().value, m("84.10"));
}
