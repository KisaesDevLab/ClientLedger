use super::*;
use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money};
use cl_docread::model::{Bbox, CheckValue, PageWords, Reader, RuleId, Spot, Word, WordSource};
use cl_docread::vendor_match::VendorMatch;

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap()
}

fn word(x: u16, y: u16, text: &str) -> Word {
    let w = u16::try_from(text.chars().count()).unwrap() * 120;
    Word {
        text: text.into(),
        bbox: Bbox::new(x, y, x + w, y + 150).unwrap(),
        confidence: Confidence::CERTAIN,
    }
}

/// A sparse receipt: the rules read the vendor's name only.
fn page() -> Vec<PageWords> {
    vec![PageWords {
        page: 0,
        aspect: 12_941,
        source: WordSource::TextLayer,
        words: vec![
            word(3000, 300, "Corner"),
            word(3800, 300, "Hardware"),
            word(500, 1000, "R-5521"),
            word(500, 3000, "Hammer"),
            word(9000, 3000, "24.99"),
            word(9000, 4000, "32.48"),
        ],
    }]
}

fn found<T>(value: T, bp: u16) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(3000, 300, 4800, 450).unwrap(),
        },
        rule: RuleId::new("test"),
        confidence: Confidence::new(bp).unwrap(),
    }
}

fn contact() -> ContactId {
    ContactId::parse("00000000-0000-0000-0000-000000000007").unwrap()
}

fn local() -> LocalReading {
    let mut r = Reading::empty(Reader::TextPdf, 1);
    r.words = page();
    r.header.vendor = Some(found("Corner Hardware".to_string(), 10_000));
    LocalReading {
        reading: r,
        vendor: None,
        card: None,
    }
}

fn candidates() -> Vec<VendorCandidate> {
    vec![VendorCandidate {
        contact: contact(),
        display_name: "Corner Hardware".into(),
        company_name: None,
        account_number: None,
        phone: None,
        address: None,
        layouts: Vec::new(),
    }]
}

fn answer(reader: AssistReader) -> AssistSuggestion {
    cl_api::suggestion_from_json(
        serde_json::json!({"suggestion": {
            "reader": match reader { AssistReader::Cloud => "cloud", AssistReader::SelfHosted => "self_hosted" },
            "vendor": "Corner Hardware", "vendorInvoiceNumber": "R-5521",
            "billDate": null, "total": "32.48", "tax": "2.48",
            "lineItems": [{"description": "Hammer", "amount": "24.99", "quantity": null,
                "unitPrice": null, "confidence": "high"},
                {"description": "Ghost", "amount": "999.00", "confidence": "high"}],
            "confidence": 0.9,
        }})
        .to_string()
        .as_bytes(),
    )
    .unwrap()
}

fn switches() -> AssistSwitches {
    AssistSwitches {
        capability: true,
        ai: true,
        reading: true,
        provider: Some(AiProvider::Ollama),
        text_cloud: false,
        images_cloud: false,
        cloud_vision: false,
    }
}

#[test]
fn ollama_needs_no_cloud_switch_and_cloud_needs_its_own() {
    assert_eq!(route(switches()), Ok(Route::Local));
    let cloud = AssistSwitches {
        provider: Some(AiProvider::Anthropic),
        ..switches()
    };
    assert_eq!(route(cloud), Err(NoRoute::NeedsConsent));
    let text = AssistSwitches {
        text_cloud: true,
        ..cloud
    };
    assert_eq!(route(text), Ok(Route::Cloud { images: false }));
    // Pictures need their own switch and the engine's cloud vision.
    let pics = AssistSwitches {
        images_cloud: true,
        ..text
    };
    assert_eq!(route(pics), Ok(Route::Cloud { images: false }));
    assert_eq!(
        route(AssistSwitches {
            cloud_vision: true,
            ..pics
        }),
        Ok(Route::Cloud { images: true })
    );
    // Without the engine's assist, AI off, reading off or no provider:
    // nothing.
    assert_eq!(
        route(AssistSwitches {
            capability: false,
            ..switches()
        }),
        Err(NoRoute::Unsupported)
    );
    for off in [
        AssistSwitches {
            ai: false,
            ..switches()
        },
        AssistSwitches {
            reading: false,
            ..text
        },
        AssistSwitches {
            provider: None,
            ..switches()
        },
    ] {
        assert_eq!(route(off), Err(NoRoute::Off));
    }
}

#[test]
fn help_is_offered_for_empty_or_unsure_fields_only() {
    assert!(needs_help(&local()), "no vendor matched, no total");
    let mut full = local();
    let h = &mut full.reading.header;
    h.date = Some(found(CalendarDate::from_ymd(2026, 7, 14).unwrap(), 9_000));
    h.reference = Some(found("R-5521".to_string(), 9_000));
    h.total = Some(found(m("32.48"), 9_000));
    full.vendor = Some(VendorMatch {
        contact: contact(),
        tier: cl_docread::vendor_match::VendorTier::Name(1),
        confidence: Confidence::HIGH,
        layout_of: None,
    });
    assert!(!needs_help(&full));
    full.reading.header.total = Some(found(m("32.48"), 3_000));
    assert!(needs_help(&full), "an unsure total");
}

#[test]
fn the_text_is_the_pages_words_line_by_line() {
    let text = page_text(&local().reading);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "Corner Hardware");
    assert!(lines.contains(&"R-5521"));
    assert!(text.contains("32.48"));
}

#[test]
fn a_picture_goes_only_online_with_consent_and_only_a_small_png() {
    let png = {
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend_from_slice(b"page");
        p
    };
    let l = local();
    let count = |route, doc: &[u8]| request(&l, route, doc).page_images.len();
    assert_eq!(count(Route::Cloud { images: true }, &png), 1);
    assert_eq!(count(Route::Cloud { images: false }, &png), 0);
    assert_eq!(count(Route::Local, &png), 0);
    assert_eq!(
        count(Route::Cloud { images: true }, b"%PDF-1.7"),
        0,
        "a PDF is never rendered here"
    );
    let r = request(&l, Route::Local, b"%PDF-1.7");
    assert_eq!(r.kind_hint, Some(DraftKind::Bill));
    assert!(r.check().is_ok());
    assert!(!format!("{r:?}").contains("Corner"));
}

#[test]
fn ai_amounts_are_kept_only_when_printed_and_the_vendor_is_matched() {
    let out = merge(&local(), &answer(AssistReader::SelfHosted), &candidates());
    let h = &out.reading.header;
    // On the page: kept, at the word's spot.
    assert_eq!(h.total.as_ref().map(|f| f.value), Some(m("32.48")));
    assert_eq!(h.total.as_ref().unwrap().rule.as_str(), "ai.ollama");
    assert_eq!(
        h.reference.as_ref().map(|f| f.value.as_str()),
        Some("R-5521")
    );
    // Not on the page: a check naming the AI's value, nothing filled.
    assert!(h.tax.is_none());
    let tax = out
        .reading
        .checks
        .iter()
        .find(|c| c.code == CheckCode::AiDisagrees && c.found.is_none())
        .expect("a check");
    assert!(matches!(tax.expected, Some(CheckValue::Money(_))));
    // The lines: only the printed amount becomes a line.
    let amounts: Vec<Money> = out.reading.lines.iter().map(|l| l.amount.value).collect();
    assert_eq!(amounts, vec![m("24.99")]);
    assert_eq!(out.vendor.map(|v| v.contact), Some(contact()));
    // A cloud answer is marked as such.
    let cloud = merge(&local(), &answer(AssistReader::Cloud), &candidates());
    assert_eq!(
        cloud.reading.header.total.as_ref().unwrap().rule.as_str(),
        "ai.cloud"
    );
}

#[test]
fn the_rules_values_win() {
    let mut l = local();
    l.reading.header.total = Some(found(m("24.99"), 9_000));
    let out = merge(&l, &answer(AssistReader::SelfHosted), &candidates());
    assert_eq!(
        out.reading.header.total.as_ref().map(|f| f.value),
        Some(m("24.99"))
    );
    assert!(
        out.reading
            .checks
            .iter()
            .any(|c| c.code == CheckCode::AiDisagrees
                && c.field == Some(cl_docread::model::FieldName::Total))
    );
}
