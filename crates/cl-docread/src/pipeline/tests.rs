use super::testdoc::{Doc, invoice};
use super::*;
use crate::amount;
use crate::model::{Bbox, Confidence, Reading, Word};
use cl_core::{Money, Quantity};
use proptest::prelude::*;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).unwrap()
}

fn input<'a>(pages: &'a [PageWords], templates: &'a [Template]) -> ReadInput<'a> {
    ReadInput {
        pages,
        page_count: u16::try_from(pages.len()).unwrap(),
        unread_pages: &[],
        templates,
        hint: JobHint::default(),
        today: today(),
    }
}

fn cents(c: i64) -> String {
    let sign = if c < 0 { "-" } else { "" };
    format!("{sign}{}.{:02}", c.abs() / 100, c.abs() % 100)
}

/// Every amount, quantity and unit price in the reading equals the value
/// of one page word (AC3, T10, T11).
pub(crate) fn every_amount_is_printed(r: &Reading) -> Result<(), String> {
    let words: Vec<&str> = r
        .words
        .iter()
        .flat_map(|p| p.words.iter().map(|w| w.text.as_str()))
        .collect();
    let printed_money = |m: Money| words.iter().any(|w| amount::parse(w) == Some(m));
    let printed_qty = |q: Quantity| words.iter().any(|w| amount::parse_quantity(w) == Some(q));
    let h = &r.header;
    let header = [
        &h.subtotal,
        &h.tax,
        &h.total,
        &h.amount_paid,
        &h.balance_due,
    ]
    .into_iter()
    .flatten()
    .chain(h.fees.iter());
    for f in header {
        if !printed_money(f.value) {
            return Err(format!(
                "header amount {} not on the page",
                f.value.to_wire()
            ));
        }
    }
    for (i, l) in r.lines.iter().enumerate() {
        if !printed_money(l.amount.value) {
            return Err(format!("line {i} amount not on the page"));
        }
        if let Some(p) = &l.unit_price
            && !printed_money(p.value)
        {
            return Err(format!("line {i} price not on the page"));
        }
        if let Some(q) = &l.quantity
            && !printed_qty(q.value)
        {
            return Err(format!("line {i} qty not on the page"));
        }
    }
    Ok(())
}

#[test]
fn words_and_unread_pages_are_carried_through() {
    let pages = [PageWords {
        page: 0,
        aspect: 12_941,
        source: WordSource::Ocr,
        words: vec![Word {
            text: "x".into(),
            bbox: Bbox::PAGE,
            confidence: Confidence::MEDIUM,
        }],
    }];
    let r = read(&ReadInput {
        pages: &pages,
        page_count: 12,
        unread_pages: &[1],
        templates: &[],
        hint: JobHint::default(),
        today: today(),
    });
    assert_eq!(r.reader, Reader::Ocr);
    assert_eq!(r.pages.len(), 2);
    assert_eq!(r.words, pages);
    let codes: Vec<_> = r.checks.iter().map(|c| c.code).collect();
    assert_eq!(
        codes,
        [
            CheckCode::PageNotRead,
            CheckCode::PagesSkipped,
            CheckCode::LinesNotRead
        ]
    );
    r.validate(0).unwrap();
}

#[test]
fn reads_an_invoice_end_to_end() {
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
    let pages = d.pages();
    let r = read(&input(&pages, &[]));
    assert_eq!(r.kind, DocKind::Bill);
    assert_eq!(r.lines.len(), 2);
    assert!(r.checks.is_empty(), "{:?}", r.checks);
    assert!(r.fingerprint.is_some());
    assert_eq!(r.template, None);
    every_amount_is_printed(&r).unwrap();
    r.validate(0).unwrap();
}

#[test]
fn a_saved_template_is_applied_and_named() {
    let first = invoice(
        "RB-1042",
        "07/14/2026",
        &[("Tarp", "2", "10.00", "20.00")],
        "20.00",
        "1.65",
        "21.65",
    );
    let p1 = first.pages();
    let t = templates::learn(&read(&input(&p1, &[]))).unwrap();
    let second = invoice(
        "RB-1077",
        "08/22/2026",
        &[("Rope", "1", "5.00", "5.00")],
        "5.00",
        "0.41",
        "5.41",
    );
    let p2 = second.pages();
    let other = Template {
        samples: 9,
        ..t.clone()
    };
    let mut unrelated = other.clone();
    unrelated.fingerprint.tokens = vec!["zzz".into()];
    let ts = [unrelated, t];
    let r = read(&input(&p2, &ts));
    assert_eq!(r.reader, Reader::Template);
    assert_eq!(r.template, Some(1));
    assert_eq!(
        r.header.total.as_ref().unwrap().value,
        Money::parse_wire("5.41").unwrap()
    );
    assert!(r.checks.is_empty(), "{:?}", r.checks);
    r.validate(ts.len()).unwrap();
}

#[test]
fn checks_are_capped() {
    // Four pages of 60 lines whose qty × rate never matches: 240 line
    // checks, capped at MAX_CHECKS.
    let mut d = Doc::new();
    for p in 0..4 {
        if p > 0 {
            d.page();
        }
        d.text(500, 100, "Description");
        d.text(5500, 100, "Qty");
        d.text(7000, 100, "Rate");
        d.text(8900, 100, "Amount");
        for i in 0..60_u16 {
            let y = 250 + i * 150;
            d.text(500, y, &format!("Part{i}"));
            d.amount(5800, y, "2");
            d.amount(7600, y, "1.00");
            d.amount(9500, y, &format!("{}.00", i + 7));
        }
    }
    let pages = d.pages();
    let r = read(&input(&pages, &[]));
    assert_eq!(r.lines.len(), 240);
    assert_eq!(r.checks.len(), crate::model::MAX_CHECKS);
    r.validate(0).unwrap();
}
#[test]
fn hint_and_kind_unclear() {
    let mut d = Doc::new();
    d.text(500, 300, "Corner Cafe");
    d.text(4000, 1000, "PAID");
    d.text(500, 3000, "Total");
    d.amount(9500, 3000, "12.40");
    let pages = d.pages();
    let r = read(&input(&pages, &[]));
    assert_eq!(r.kind, DocKind::Unknown);
    assert!(r.checks.iter().any(|c| c.code == CheckCode::KindUnclear));
}

/// Build an invoice with `lines` (qty, rate cents), printed subtotal off by
/// `sub_delta` cents and total off by `delta` cents.
fn inconsistent(
    lines: &[(i64, i64)],
    tax: i64,
    sub_delta: i64,
    delta: i64,
    reference: &str,
) -> (Doc, String) {
    let rows: Vec<(String, String, String, String)> = lines
        .iter()
        .enumerate()
        .map(|(i, (q, r))| (format!("Item{i}"), q.to_string(), cents(*r), cents(q * r)))
        .collect();
    let sum: i64 = lines.iter().map(|(q, r)| q * r).sum();
    let printed_sub = sum + sub_delta;
    let total = cents(printed_sub + tax + delta);
    let refs: Vec<(&str, &str, &str, &str)> = rows
        .iter()
        .map(|(a, b, c, d)| (a.as_str(), b.as_str(), c.as_str(), d.as_str()))
        .collect();
    (
        invoice(
            reference,
            "07/14/2026",
            &refs,
            &cents(printed_sub),
            &cents(tax),
            &total,
        ),
        total,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// T10 (AC3, M7): injected inconsistent totals, with and without a
    /// template, on bills and card receipts. Every output amount is a
    /// printed one, the printed total is kept, and a check names each
    /// inconsistency.
    #[test]
    fn t10_inconsistent_totals_are_flagged_never_adjusted(
        lines in prop::collection::vec((1_i64..20, 1_i64..100_000), 1..5),
        tax in 0_i64..50_000,
        sub_delta in prop_oneof![Just(0_i64), -300_i64..300],
        delta in prop_oneof![-5_000_i64..-1, 1_i64..5_000],
        with_template in any::<bool>(),
        receipt in any::<bool>(),
    ) {
        let card = |mut d: Doc| {
            if receipt {
                d.text(500, 9000, "VISA ************4821");
            }
            d
        };
        let templates: Vec<Template> = if with_template {
            let (good, _) = inconsistent(&[(1, 500)], 41, 0, 0, "RB-1001");
            let pages = card(good).pages();
            let r = read(&input(&pages, &[]));
            vec![templates::learn(&r).unwrap()]
        } else {
            Vec::new()
        };
        let (doc, total) = inconsistent(&lines, tax, sub_delta, delta, "RB-2002");
        let pages = card(doc).pages();
        let r = read(&input(&pages, &templates));
        prop_assert_eq!(r.reader == Reader::Template, with_template);
        prop_assert_eq!(r.kind == DocKind::CardCharge, receipt);
        prop_assert!(every_amount_is_printed(&r).is_ok(), "{:?}", every_amount_is_printed(&r));
        prop_assert_eq!(r.header.total.as_ref().map(|t| t.value), Money::parse_wire(&total).ok());
        prop_assert!(r.checks.iter().any(|c| c.code == CheckCode::SubtotalTaxVsTotal));
        prop_assert_eq!(
            r.checks.iter().any(|c| c.code == CheckCode::LinesVsSubtotal),
            sub_delta != 0
        );
        prop_assert!(r.validate(templates.len()).is_ok());

        // The same totals left empty by the rules and filled by an AI
        // reader from the page: reconciled again, flagged the same way
        // (review B2).
        let h = &r.header;
        let ai = crate::ai_gate::AiReading {
            subtotal: h.subtotal.as_ref().map(|f| f.value),
            tax: h.tax.as_ref().map(|f| f.value),
            total: h.total.as_ref().map(|f| f.value),
            confidence: Confidence::HIGH,
            ..crate::ai_gate::AiReading::default()
        };
        let mut gap = r.clone();
        gap.header.subtotal = None;
        gap.header.tax = None;
        gap.header.total = None;
        gap.checks.retain(|c| !matches!(
            c.code,
            CheckCode::SubtotalTaxVsTotal | CheckCode::LinesVsSubtotal
        ));
        let out = crate::ai_gate::merge(gap, &ai, crate::ai_gate::AiSource::Ollama);
        prop_assert!(every_amount_is_printed(&out).is_ok(), "{:?}", every_amount_is_printed(&out));
        prop_assert_eq!(out.header.total.as_ref().map(|t| t.value), Money::parse_wire(&total).ok());
        prop_assert!(out.checks.iter().any(|c| c.code == CheckCode::SubtotalTaxVsTotal));
        prop_assert_eq!(
            out.checks.iter().any(|c| c.code == CheckCode::LinesVsSubtotal),
            sub_delta != 0
        );
    }
}

#[test]
fn a_region_reading_holds_only_the_boxs_words() {
    let mut d = Doc::new();
    d.text(1000, 1000, "INVOICE");
    d.amount(9000, 8000, "544.82");
    d.text(1000, 8000, "Total");
    let pages = d.pages();
    let region = Spot {
        page: 0,
        bbox: Bbox::new(7000, 7500, 9500, 8600).unwrap(),
    };
    let r = region_reading(&pages[0], 2, region.bbox);
    let texts: Vec<&str> = r.words[0].words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(texts, ["544.82"]);
    assert_eq!(r.page_count, 2);
    assert_eq!(r.pages[0].words, 1);
    r.validate(0).unwrap();
    assert!(is_region_reading(&r, region));
    // Anything more than words is not a region answer.
    let mut more = r.clone();
    more.header.total = read(&input(&pages, &[])).header.total;
    assert!(!is_region_reading(&more, region));
    let mut other_page = r.clone();
    other_page.words[0].page = 1;
    assert!(!is_region_reading(&other_page, region));
    let mut outside = r.clone();
    outside.words[0].words = pages[0].words.clone();
    assert!(!is_region_reading(&outside, region));
    let mut fp = r.clone();
    fp.fingerprint = templates::fingerprint(&pages);
    assert!(!is_region_reading(&fp, region));
    assert!(is_region_reading(&Reading::empty(Reader::Ocr, 1), region));
}