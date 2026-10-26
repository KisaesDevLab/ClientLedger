//! The compare tools' controller (A04 D4; AC2, T10). No floats here.

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use cl_core::Money;
use cl_docread::model::{Bbox, Confidence};
use cl_host::{DocJob, JobOutcome};

use super::*;
use crate::reading::service::Reader;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 26).unwrap()
}

fn word(text: &str, l: u16, t: u16, r: u16, b: u16) -> Word {
    Word {
        text: text.into(),
        bbox: Bbox::new(l, t, r, b).unwrap(),
        confidence: Confidence::CERTAIN,
    }
}

const TOTAL: TargetKey = TargetKey::field("amount_due");
const DATE: TargetKey = TargetKey::field("bill_date");
const VENDOR: TargetKey = TargetKey::field("vendor");

fn targets() -> Vec<CompareTarget> {
    vec![
        CompareTarget {
            key: VENDOR,
            label: "Vendor".into(),
            spot: PageBox::new(0, 500, 500, 4000, 900),
            level: TargetLevel::Sure,
            fill: None,
        },
        CompareTarget {
            key: DATE,
            label: "Date".into(),
            spot: PageBox::new(0, 6000, 500, 8000, 900),
            level: TargetLevel::Sure,
            fill: Some(FillKind::Date),
        },
        CompareTarget {
            key: TOTAL,
            label: "Amount due".into(),
            spot: None,
            level: TargetLevel::NotFound,
            fill: Some(FillKind::Money),
        },
    ]
}

fn words(source: WordSource) -> Arc<DocWords> {
    Arc::new(DocWords {
        pages: vec![PageWords {
            page: 0,
            aspect: 12_941,
            source,
            words: vec![
                word("Total", 5000, 8000, 6000, 8400),
                word("2", 6500, 8000, 6700, 8400),
                word("$", 7000, 8000, 7200, 8400),
                word("12.50", 7300, 8000, 8000, 8400),
                word("Mar", 6000, 500, 6500, 900),
                word("14,", 6600, 500, 7000, 900),
                word("2026", 7100, 500, 8000, 900),
            ],
        }],
    })
}

fn tools(source: WordSource) -> CompareTools {
    let mut c = CompareTools::new();
    c.set_targets(
        targets(),
        vec!["Ozark".into(), "3/14/2026".into(), String::new()],
    );
    c.set_words(Some(words(source)));
    c
}

fn env() -> CompareEnv<'static> {
    CompareEnv {
        today: today(),
        service: None,
        bytes: None,
    }
}

fn click(page: u16, first: u32, last: u32) -> CompareEvent {
    CompareEvent::PageClick { page, first, last }
}

#[test]
fn focus_arms_fillable_fields_and_the_arm_survives_the_viewer() {
    let mut c = tools(WordSource::TextLayer);
    let g = c.feed().generation;
    assert!(c.feed().highlights, "on by default");
    c.focus(Some(VENDOR));
    assert_eq!(c.feed().focused, Some(VENDOR));
    assert_eq!(c.feed().armed, None, "a list field never arms");
    c.focus(Some(TOTAL));
    assert_eq!(c.feed().armed, Some(TOTAL));
    // Focus moves to the viewer: both stay.
    c.focus(None);
    assert_eq!(c.feed().armed, Some(TOTAL));
    assert_eq!(c.feed().focused, Some(TOTAL));
    // Focus on the vendor outlines it and keeps the arm (FillArm).
    c.focus(Some(VENDOR));
    assert_eq!(c.feed().armed, Some(TOTAL));
    assert!(c.feed().generation > g);
    // The same targets again change nothing.
    let g = c.feed().generation;
    c.set_targets(
        targets(),
        vec!["Ozark".into(), "3/14/2026".into(), String::new()],
    );
    assert_eq!(c.feed().generation, g);
    // The armed field leaves: the arm goes.
    c.set_targets(targets().into_iter().take(2).collect(), Vec::new());
    assert_eq!(c.feed().armed, None);
    c.toggle_highlights();
    assert!(!c.view().feed.highlights);
}

/// AC2 / T10: a money word fills exactly what it says; two numbers never.
#[test]
fn click_to_fill_exact_cents_and_refuse_two_numbers() {
    let mut c = tools(WordSource::TextLayer);
    assert_eq!(c.on_event(click(0, 3, 3), &env()), None);
    assert_eq!(c.note(), Some(CompareNote::NoField), "click a field first");
    c.focus(Some(TOTAL));
    assert_eq!(c.note(), None);
    let filled = c.on_event(click(0, 3, 3), &env());
    assert_eq!(
        filled,
        Some(CompareAction::Fill {
            key: TOTAL,
            value: FillValue::Money(Money::parse_wire("12.50").unwrap()),
            spot: PageBox::new(0, 7300, 8000, 8000, 8400).unwrap(),
        })
    );
    // "$" + "12.50" (Shift+click from "$").
    assert!(matches!(
        c.on_event(click(0, 2, 3), &env()),
        Some(CompareAction::Fill { value: FillValue::Money(m), .. }) if m == Money::parse_wire("12.50").unwrap()
    ));
    // "2" "$" "12.50": two numbers, nothing changes.
    assert_eq!(c.on_event(click(0, 1, 3), &env()), None);
    assert_eq!(
        c.note(),
        Some(CompareNote::Refused(FillRefusal::NotOneAmount))
    );
    assert_eq!(c.view().note.map(CompareNote::name), Some("not_one_amount"));
    // A date joins words.
    c.focus(Some(DATE));
    assert!(matches!(
        c.on_event(click(0, 4, 6), &env()),
        Some(CompareAction::Fill { key: DATE, value: FillValue::Date(d), .. }) if d == CalendarDate::from_ymd(2026, 3, 14).unwrap()
    ));
    assert_eq!(c.note(), None, "a fill clears the note");
}

#[test]
fn without_words_a_click_waits_for_the_page() {
    let mut c = CompareTools::new();
    c.set_targets(targets(), Vec::new());
    c.focus(Some(TOTAL));
    assert_eq!(c.on_event(click(0, 0, 0), &env()), None);
    assert_eq!(c.note(), Some(CompareNote::WordsLoading));
    c.set_words(Some(words(WordSource::TextLayer)));
    assert_eq!(c.note(), None);
}

#[test]
fn a_box_on_a_text_page_uses_its_words() {
    let mut c = tools(WordSource::TextLayer);
    c.focus(Some(TOTAL));
    let area = PageBox::new(0, 6900, 7900, 8100, 8500).unwrap();
    assert!(matches!(
        c.on_event(CompareEvent::PageBox(area), &env()),
        Some(CompareAction::Fill { value: FillValue::Money(m), .. }) if m == Money::parse_wire("12.50").unwrap()
    ));
    // Over "2", "$" and "12.50": refused.
    let area = PageBox::new(0, 6400, 7900, 8100, 8500).unwrap();
    assert_eq!(c.on_event(CompareEvent::PageBox(area), &env()), None);
    assert_eq!(
        c.note(),
        Some(CompareNote::Refused(FillRefusal::NotOneAmount))
    );
    // Too small.
    let tiny = PageBox::new(0, 100, 100, 150, 400).unwrap();
    assert_eq!(c.on_event(CompareEvent::PageBox(tiny), &env()), None);
    assert_eq!(c.note(), Some(CompareNote::BoxTooSmall));
    // No words inside and no reading service: not available.
    let empty = PageBox::new(0, 100, 2000, 3000, 3000).unwrap();
    assert_eq!(c.on_event(CompareEvent::PageBox(empty), &env()), None);
    assert_eq!(c.note(), Some(CompareNote::BoxUnavailable));
}

/// Reads boxes: "2" and "12.50" in a box at the top, "12.50" alone below.
struct BoxReader;

impl Reader for BoxReader {
    fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
        jobs.iter()
            .map(|_| JobOutcome::CouldNotRead(cl_host::CouldNotRead::Unavailable))
            .collect()
    }

    fn read_region(&mut self, request: &RegionRequest, _c: &Arc<AtomicBool>) -> RegionOutcome {
        let a = request.area;
        if a.t < 5000 {
            RegionOutcome::Words(vec![
                word("2", a.l, a.t, a.l + 50, a.t + 50),
                word("12.50", a.l + 60, a.t, a.l + 99, a.t + 50),
            ])
        } else {
            RegionOutcome::Words(vec![
                word("12.50", a.l, a.t, a.l + 99, a.t + 50),
                // Outside the box: ignored.
                word("99.00", 0, 0, 10, 10),
            ])
        }
    }
}

fn wait(c: &mut CompareTools) -> Option<CompareAction> {
    for _ in 0..1000 {
        if let Some(a) = c.poll(today()) {
            return Some(a);
        }
        if !c.reading_box() {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

/// A box on a scan is read in the worker; the M4 rule applies to its words.
#[test]
fn a_box_on_a_scan_is_read_again_and_filled_by_the_same_rule() {
    let service = ReadingService::start(Box::new(|| Some(Box::new(BoxReader) as Box<dyn Reader>)));
    let mut c = tools(WordSource::Ocr);
    c.focus(Some(TOTAL));
    let env = CompareEnv {
        today: today(),
        service: Some(&service),
        bytes: Some(Arc::from(b"scan".to_vec())),
    };
    let low = PageBox::new(0, 6000, 7000, 9000, 9000).unwrap();
    assert_eq!(c.on_event(CompareEvent::PageBox(low), &env), None);
    assert!(c.reading_box());
    assert_eq!(c.note(), Some(CompareNote::ReadingBox));
    let filled = wait(&mut c);
    assert_eq!(
        filled,
        Some(CompareAction::Fill {
            key: TOTAL,
            value: FillValue::Money(Money::parse_wire("12.50").unwrap()),
            spot: PageBox::new(0, 6000, 7000, 6099, 7050).unwrap(),
        })
    );
    let high = PageBox::new(0, 1000, 1000, 4000, 3000).unwrap();
    assert_eq!(c.on_event(CompareEvent::PageBox(high), &env), None);
    assert_eq!(wait(&mut c), None);
    assert_eq!(
        c.note(),
        Some(CompareNote::Refused(FillRefusal::NotOneAmount))
    );
    // A reader without box reading says so.
    let plain = ReadingService::start(Box::new(|| None));
    let env = CompareEnv {
        service: Some(&plain),
        ..env
    };
    assert_eq!(c.on_event(CompareEvent::PageBox(low), &env), None);
    assert_eq!(wait(&mut c), None);
    assert_eq!(c.note(), Some(CompareNote::BoxUnavailable));
}

#[test]
fn target_clicks_focus_and_looks_right_passes_through() {
    let mut c = tools(WordSource::TextLayer);
    assert_eq!(
        c.on_event(CompareEvent::TargetClicked(DATE), &env()),
        Some(CompareAction::Focus(DATE))
    );
    assert_eq!(c.feed().focused, Some(DATE));
    assert_eq!(c.feed().armed, Some(DATE));
    assert_eq!(
        c.on_event(CompareEvent::LooksRight(TOTAL), &env()),
        Some(CompareAction::LooksRight(TOTAL))
    );
    assert_eq!(c.on_event(CompareEvent::ToggleHighlights, &env()), None);
    assert!(!c.feed().highlights);
    c.clear();
    assert_eq!(c.feed().armed, None);
}

#[test]
fn nothing_owner_shaped_reaches_debug_or_the_control_channel() {
    let mut c = tools(WordSource::TextLayer);
    c.focus(Some(TOTAL));
    let _ = c.on_event(click(0, 3, 3), &env());
    let debug = format!("{c:?} {:?}", c.view());
    for secret in ["Ozark", "12.50", "3/14", "Mar"] {
        assert!(!debug.contains(secret), "{debug}");
    }
    let action = c.on_event(click(0, 3, 3), &env());
    assert!(!format!("{action:?}").contains("12.50"));
    assert_eq!(
        key_name(TargetKey::line("line_amount", cl_model::rowkey::RowKey(3))),
        "line_amount:3"
    );
}

#[test]
fn the_last_published_highlight_is_reported() {
    let ctx = egui::Context::default();
    assert_eq!(highlight(&ctx), HighlightState::default());
    assert_eq!(highlight(&ctx).to_json()["on"], false);
    let s = HighlightState {
        on: true,
        drawn: 4,
        focused: Some("amount_due".into()),
        page: Some(0),
        level: Some(TargetLevel::CheckThis.name()),
        armed: Some("amount_due".into()),
        words: 12,
        cursor: Some(3),
        picked: 1,
        note: Some("not_one_amount"),
        checklist: false,
    };
    publish_highlight(&ctx, s.clone());
    assert_eq!(highlight(&ctx), s);
    let v = highlight(&ctx).to_json();
    assert_eq!(v["level"], "check_this");
    assert_eq!(v["drawn"], 4);
    assert_eq!(v["note"], "not_one_amount");
}
