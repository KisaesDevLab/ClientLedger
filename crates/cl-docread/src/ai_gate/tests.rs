use super::*;
use crate::model::{Reader, WordSource};
use crate::pipeline::testdoc::{Doc, invoice};
use crate::pipeline::tests::every_amount_is_printed;
use crate::pipeline::{ReadInput, read};
use crate::protocol::JobHint;
use proptest::prelude::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn read_doc(d: &Doc) -> Reading {
    let pages = d.pages();
    read(&ReadInput {
        pages: &pages,
        page_count: u16::try_from(pages.len()).unwrap(),
        unread_pages: &[],
        templates: &[],
        hint: JobHint::default(),
        today: CalendarDate::from_ymd(2026, 10, 26).unwrap(),
    })
}

/// A receipt the rules can't read much of: unlabelled numbers, no table.
fn sparse_receipt() -> Doc {
    let mut d = Doc::new();
    d.text(3000, 300, "Corner Hardware");
    d.text(500, 1000, "07/14/2026");
    d.text(500, 1200, "R-5521");
    d.text(500, 3000, "Hammer");
    d.amount(9500, 3000, "24.99");
    d.text(500, 3200, "Nails");
    d.amount(9500, 3200, "5.01");
    d.amount(9500, 3600, "30.00");
    d.amount(9500, 3800, "2.48");
    d.amount(9500, 4000, "32.48");
    d
}

fn codes(r: &Reading) -> Vec<CheckCode> {
    r.checks.iter().map(|c| c.code).collect()
}

#[test]
fn ai_amounts_fill_only_from_page_words() {
    let r = read_doc(&sparse_receipt());
    assert!(
        r.header.total.is_none(),
        "several unlabelled numbers, no subtotal"
    );
    let ai = AiReading {
        vendor: Some("Corner Hardware".into()),
        reference: Some("r5521".into()),
        date: Some(CalendarDate::from_ymd(2026, 7, 14).unwrap()),
        subtotal: Some(m("30.00")),
        tax: Some(m("2.48")),
        total: Some(m("32.48")),
        amount_paid: Some(m("40.00")),
        confidence: Confidence::HIGH,
        ..AiReading::default()
    };
    let out = merge(r, &ai, AiSource::Ollama);
    let h = &out.header;
    let total = h.total.as_ref().unwrap();
    assert_eq!(total.value, m("32.48"));
    assert_eq!(total.rule.as_str(), "ai.ollama");
    assert!(total.confidence.bp() <= AI_FILL);
    assert_eq!(total.spot.bbox.top(), 4000, "the word's spot");
    assert_eq!(h.subtotal.as_ref().unwrap().value, m("30"));
    assert!(h.amount_paid.is_none(), "40.00 is not on the page");
    let paid_check = out
        .checks
        .iter()
        .find(|c| c.code == CheckCode::AiDisagrees && c.field == Some(FieldName::AmountPaid))
        .unwrap();
    assert!(matches!(paid_check.expected, Some(CheckValue::Money(v)) if v == m("40")));
    assert_eq!(h.reference.as_ref().unwrap().value, "R-5521");
    assert_eq!(h.date.as_ref().unwrap().spot.bbox.top(), 1000);
    every_amount_is_printed(&out).unwrap();
    out.validate(0).unwrap();
}

#[test]
fn agreement_raises_and_disagreement_flags_rules_values() {
    let d = invoice(
        "RB-1042",
        "07/14/2026",
        &[("Tarp", "2", "10.00", "20.00")],
        "20.00",
        "1.65",
        "21.65",
    );
    let r = read_doc(&d);
    let before = r.header.total.as_ref().unwrap().confidence;
    let ai = AiReading {
        total: Some(m("21.65")),
        subtotal: Some(m("20.65")),
        reference: Some("RB-1042".into()),
        lines: vec![
            AiLine {
                amount: Some(m("20.00")),
                ..AiLine::default()
            },
            AiLine {
                amount: Some(m("99.00")),
                ..AiLine::default()
            },
        ],
        confidence: Confidence::HIGH,
        ..AiReading::default()
    };
    let out = merge(r, &ai, AiSource::Cloud);
    assert!(out.header.total.as_ref().unwrap().confidence >= before);
    assert_eq!(
        out.header.subtotal.as_ref().unwrap().value,
        m("20"),
        "rules value kept"
    );
    let c = out
        .checks
        .iter()
        .find(|c| c.field == Some(FieldName::Subtotal))
        .unwrap();
    assert_eq!(c.code, CheckCode::AiDisagrees);
    assert_eq!(c.difference, Some(m("-0.65")));
    assert_eq!(out.lines.len(), 1, "rules lines kept");
    assert!(
        out.checks
            .iter()
            .any(|c| c.code == CheckCode::AiDisagrees && c.line == Some(1))
    );
}

#[test]
fn ai_lines_need_page_words_and_pass_the_line_rule() {
    let r = read_doc(&sparse_receipt());
    assert!(codes(&r).contains(&CheckCode::LinesNotRead));
    let ai = AiReading {
        lines: vec![
            AiLine {
                description: Some("Hammer".into()),
                quantity: Some(Quantity::parse_wire("1").unwrap()),
                unit_price: Some(m("24.99")),
                amount: Some(m("24.99")),
            },
            AiLine {
                description: Some("Nails".into()),
                quantity: Some(Quantity::parse_wire("3").unwrap()),
                unit_price: Some(m("1.67")),
                amount: Some(m("5.01")),
            },
            AiLine {
                amount: Some(m("77.00")),
                ..AiLine::default()
            },
        ],
        confidence: Confidence::HIGH,
        ..AiReading::default()
    };
    let out = merge(r, &ai, AiSource::Ollama);
    assert_eq!(out.lines.len(), 2);
    assert_eq!(out.lines[0].description.as_ref().unwrap().value, "Hammer");
    // "1" is not a word on the page, so the price is dropped with it
    // (no price without a quantity).
    assert!(out.lines[0].quantity.is_none() && out.lines[0].unit_price.is_none());
    assert!(
        out.lines[1].quantity.is_none(),
        "3 and 1.67 are not on the page"
    );
    assert!(!codes(&out).contains(&CheckCode::LinesNotRead));
    assert!(
        out.checks
            .iter()
            .any(|c| c.code == CheckCode::AiDisagrees && c.line == Some(2))
    );
    every_amount_is_printed(&out).unwrap();
}

#[test]
fn two_words_never_make_one_amount() {
    let mut d = Doc::new();
    d.text(500, 3000, "Qty 1 234.56");
    let r = read_doc(&d);
    let ai = AiReading {
        total: Some(m("1234.56")),
        confidence: Confidence::HIGH,
        ..AiReading::default()
    };
    let out = merge(r, &ai, AiSource::Cloud);
    assert!(out.header.total.is_none());
    assert!(codes(&out).contains(&CheckCode::AiDisagrees));
}

#[test]
fn debug_hides_ai_values() {
    let ai = AiReading {
        vendor: Some("Riverbend".into()),
        total: Some(m("544.82")),
        lines: vec![AiLine {
            description: Some("Tarp".into()),
            ..AiLine::default()
        }],
        ..AiReading::default()
    };
    let text = format!("{ai:?} {:?}", ai.lines[0]);
    for secret in ["Riverbend", "544", "Tarp"] {
        assert!(!text.contains(secret), "{text}");
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// T11 (AC3, Q-A03j): after the gate, no amount, quantity or price
    /// lacks an equal page word, whatever the AI says; and an AI amount
    /// that is not on the page always produces a check.
    #[test]
    fn t11_no_ai_amount_without_a_page_word(
        page_cents in prop::collection::vec(0_i64..1_000_000, 0..8),
        ai_cents in prop::collection::vec(prop::option::of(0_i64..1_000_000), 4),
        line_cents in prop::collection::vec((0_i64..1_000_000, prop::option::of(1_i64..50), prop::option::of(0_i64..100_000)), 0..5),
        pick_from_page in prop::collection::vec(any::<prop::sample::Index>(), 4),
    ) {
        let mut d = Doc::new();
        d.source = WordSource::Ocr;
        let mut y = 500;
        for c in &page_cents {
            let text = Money::from_cents(*c).unwrap().format(cl_core::MoneyStyle::default());
            d.amount(9500, y, &text);
            y += 250;
        }
        let r = read_doc(&d);
        prop_assert_eq!(r.reader, Reader::Ocr);
        // Some AI values copy page amounts; others are invented.
        let value = |i: usize| -> Option<Money> {
            let c = ai_cents[i]?;
            let c = if page_cents.is_empty() || c % 2 == 0 { c } else { page_cents[pick_from_page[i].index(page_cents.len())] };
            Some(Money::from_cents(c).unwrap())
        };
        let ai = AiReading {
            subtotal: value(0),
            tax: value(1),
            total: value(2),
            amount_paid: value(3),
            lines: line_cents
                .iter()
                .map(|(a, q, p)| AiLine {
                    description: Some("x".into()),
                    quantity: q.map(|q| Quantity::from_ten_thousandths(i128::from(q) * 10_000).unwrap()),
                    unit_price: p.map(|p| Money::from_cents(p).unwrap()),
                    amount: Some(Money::from_cents(*a).unwrap()),
                })
                .collect(),
            confidence: Confidence::HIGH,
            ..AiReading::default()
        };
        let before = r.clone();
        let out = merge(r, &ai, AiSource::Ollama);
        prop_assert!(every_amount_is_printed(&out).is_ok(), "{:?}", every_amount_is_printed(&out));
        prop_assert_eq!(&out.words, &before.words);
        let on_page = |v: Money| page_cents.iter().any(|c| Money::from_cents(*c).unwrap() == v);
        for (i, field) in [FieldName::Subtotal, FieldName::Tax, FieldName::Total, FieldName::AmountPaid].into_iter().enumerate() {
            if let Some(v) = value(i)
                && !on_page(v)
            {
                let flagged = out.checks.iter().any(|c| c.code == CheckCode::AiDisagrees && c.field == Some(field))
                    || out.checks.len() == crate::model::MAX_CHECKS;
                prop_assert!(flagged, "{field:?}");
            }
        }
        prop_assert!(out.validate(0).is_ok());
    }
}
