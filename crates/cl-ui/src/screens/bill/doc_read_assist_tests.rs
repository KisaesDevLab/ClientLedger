//! Tests for Ask AI to help on Enter Bill (A03 step 21, plan D5, D7,
//! Q-A03j, Q-A03k): a stand-in reader answers the local reading, the demo
//! engine answers the assist (no AI is called).

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::{CalendarDate, Money};
use cl_docread::model::{
    Bbox, CheckCode, Confidence as Bp, Found, PageWords, Reader as Source, Reading, RuleId, Spot,
    Word, WordSource,
};
use cl_host::{DocJob, JobOutcome};
use cl_settings::AiProvider;

use super::Offer;
use crate::company::CompanyLink;
use crate::company::demo::bills::{STORY_PDF, STORY_PDF_NAME, STORY_VENDOR};
use crate::reading::assist::Route;
use crate::reading::{Reader, ReadingService};
use crate::screens::bill::EnterBillScreen;
use crate::screens::bill::doc_read::ReadSwitches;
use crate::windows::Screen as _;

fn link(assist: bool) -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    link.demo_state().expect("demo").ai.assist = assist;
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn word(x: u16, y: u16, text: &str) -> Word {
    let w = u16::try_from(text.chars().count()).expect("len") * 110;
    Word {
        text: text.into(),
        bbox: Bbox::new(x, y, x + w, y + 140).expect("box"),
        confidence: Bp::CERTAIN,
    }
}

/// The story invoice's words; `with_delivery` false leaves the 150.00 line
/// off the page.
fn words(with_delivery: bool) -> Vec<PageWords> {
    let mut w = vec![
        word(500, 300, "Midwest"),
        word(1400, 300, "Equipment"),
        word(2600, 300, "Rental"),
        word(500, 1000, "Invoice"),
        word(1500, 1000, "INV-55871"),
        word(500, 1200, "Date"),
        word(1200, 1200, "11/03/2026"),
        word(500, 3000, "Mini"),
        word(1200, 3000, "excavator"),
        word(8500, 3000, "1330.00"),
        word(500, 4000, "Total"),
        word(8500, 4000, "1480.00"),
    ];
    if with_delivery {
        w.push(word(500, 3200, "Delivery"));
        w.push(word(8500, 3200, "150.00"));
    }
    vec![PageWords {
        page: 0,
        aspect: 12_941,
        source: WordSource::TextLayer,
        words: w,
    }]
}

fn found<T>(value: T, bp: u16) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(500, 300, 3300, 440).expect("box"),
        },
        rule: RuleId::new("test"),
        confidence: Bp::new(bp).expect("bp"),
    }
}

/// The rules read only the vendor and the date: no ref. no., no total, no
/// lines.
fn sparse(with_delivery: bool) -> Reading {
    let mut r = Reading::empty(Source::TextPdf, 1);
    r.words = words(with_delivery);
    r.header.vendor = Some(found(STORY_VENDOR.to_string(), 10_000));
    r.header.date = Some(found(
        CalendarDate::from_ymd(2026, 11, 3).expect("d"),
        9_000,
    ));
    r.checks.push(cl_docread::model::Check {
        code: CheckCode::LinesNotRead,
        rule: RuleId::new("test"),
        field: None,
        line: None,
        expected: None,
        found: None,
        difference: None,
        spot: None,
    });
    r
}

struct Canned(JobOutcome);

impl Reader for Canned {
    fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
        jobs.iter().map(|_| self.0.clone()).collect()
    }
}

fn screen(link: &Arc<CompanyLink>, reading: Reading) -> EnterBillScreen {
    let mut s = EnterBillScreen::new(Arc::clone(link), None, today());
    s.poll();
    s.upload_bytes(STORY_PDF_NAME, STORY_PDF.to_vec());
    let outcome = JobOutcome::Read(Box::new(reading));
    s.read.local.service = Some(Arc::new(ReadingService::start(Box::new(move || {
        Some(Box::new(Canned(outcome)) as Box<dyn Reader>)
    }))));
    s.poll_read();
    s
}

fn settle(s: &mut EnterBillScreen) {
    let start = Instant::now();
    while s.read.local.busy() {
        assert!(start.elapsed() < Duration::from_secs(10), "an answer");
        std::thread::sleep(Duration::from_millis(5));
        s.poll_read();
    }
    s.poll_read();
    s.track_read();
}

fn read_local(s: &mut EnterBillScreen, command: &'static str) {
    assert!(s.form_command(cl_model::CommandId::new(command)));
    settle(s);
}

#[test]
fn with_the_assist_there_is_one_read_document_entry() {
    let (_d, link) = link(true);
    let mut s = screen(&link, sparse(true));
    s.poll_read();
    // P17's Read with AI is not offered; its command reads here instead.
    assert_eq!(s.state()["read"]["available"], false);
    read_local(&mut s, "bill.read_document");
    let st = s.state();
    assert_eq!(st["read"]["local"]["requests"], 1);
    assert_eq!(st["read"]["requests"], 0, "nothing sent to P17's extract");
    assert_eq!(st["read"]["assist"]["offer"], "local");
    assert_eq!(st["read"]["assist"]["needs_help"], true);
}

#[test]
fn without_the_assist_p17_stays_and_nothing_is_offered() {
    let (_d, link) = link(false);
    let mut s = screen(&link, sparse(true));
    s.poll_read();
    read_local(&mut s, "bill.read_local");
    let st = s.state();
    assert_eq!(st["read"]["available"], true, "Read with AI stays");
    assert_eq!(st["read"]["assist"]["offer"], "none");
    s.start_assist();
    assert_eq!(s.read.local.assist_sent, 0);
}

#[test]
fn ask_ai_fills_only_printed_values_and_each_needs_accept() {
    let (_d, link) = link(true);
    let mut s = screen(&link, sparse(true));
    read_local(&mut s, "bill.read_local");
    let before = s.read.unaccepted;
    s.start_assist();
    settle(&mut s);
    let st = s.state();
    assert_eq!(st["read"]["assist"]["requests"], 1);
    assert_eq!(st["read"]["assist"]["outcome"], "helped");
    assert_eq!(st["read"]["assist"]["helped"], true);
    assert_eq!(st["read"]["assist"]["offer"], "none", "one ask per reading");
    assert_eq!(st["read"]["filled_locally"], true);
    let f = s.header.current();
    assert_eq!(f.reference, "INV-55871");
    assert_eq!(f.amount_due, Some(money("1480.00")));
    let amounts: Vec<_> = s
        .expenses
        .value()
        .lines
        .iter()
        .filter_map(|l| l.amount)
        .collect();
    assert_eq!(amounts, vec![money("1330.00"), money("150.00")]);
    // Every value the AI helped with waits for Accept.
    assert!(
        s.read.unaccepted > before,
        "{} > {before}",
        s.read.unaccepted
    );
    let mut form = s.compose();
    s.read.accept(&mut form);
    s.take_form(form);
    s.track_read();
    assert_eq!(s.read.unaccepted, 0);
}

#[test]
fn an_ai_amount_not_printed_is_never_filled() {
    let (_d, link) = link(true);
    // The 150.00 line isn't on this page.
    let mut s = screen(&link, sparse(false));
    read_local(&mut s, "bill.read_local");
    s.start_assist();
    settle(&mut s);
    let amounts: Vec<_> = s
        .expenses
        .value()
        .lines
        .iter()
        .filter_map(|l| l.amount)
        .collect();
    assert_eq!(amounts, vec![money("1330.00")]);
    let checks = s.state()["read"]["checks"].clone();
    assert!(
        checks
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c == "aiDisagrees")),
        "{checks}"
    );
}

#[test]
fn nothing_is_sent_with_ai_off_or_reading_off() {
    for (enabled, reading) in [(false, true), (true, false)] {
        let (_d, link) = link(true);
        {
            let mut demo = link.demo_state().expect("demo");
            demo.ai.enabled = enabled;
            demo.ai.reading = reading;
        }
        let mut s = screen(&link, sparse(true));
        read_local(&mut s, "bill.read_local");
        assert_eq!(s.state()["read"]["assist"]["offer"], "none");
        s.start_assist();
        settle(&mut s);
        assert_eq!(s.read.local.assist_sent, 0);
    }
}

#[test]
fn an_online_provider_needs_the_companys_consent() {
    let (_d, link) = link(true);
    let mut s = screen(&link, sparse(true));
    read_local(&mut s, "bill.read_local");
    let cloud = ReadSwitches {
        ai: true,
        reading: true,
        provider: Some(AiProvider::Anthropic),
        assist: true,
        ..ReadSwitches::default()
    };
    assert_eq!(s.read.local.offer(Some(cloud)), Offer::NeedsConsent);
    assert_eq!(
        s.read.local.offer(Some(ReadSwitches {
            text_cloud: true,
            ..cloud
        })),
        Offer::Ask(Route::Cloud { images: false })
    );
    // `receipt_ocr` alone (reading on) never stands in for the consent;
    // without the engine's assist nothing is offered at all.
    assert_eq!(
        s.read.local.offer(Some(ReadSwitches {
            assist: false,
            text_cloud: true,
            ..cloud
        })),
        Offer::None
    );
}

#[test]
fn a_full_reading_offers_no_help() {
    let (_d, link) = link(true);
    let mut r = sparse(true);
    r.checks.clear();
    r.header.reference = Some(found("INV-55871".to_string(), 10_000));
    r.header.total = Some(found(money("1480.00"), 10_000));
    r.lines = vec![cl_docread::model::Line {
        description: None,
        quantity: None,
        unit_price: None,
        amount: found(money("1480.00"), 10_000),
    }];
    let mut s = screen(&link, r);
    read_local(&mut s, "bill.read_local");
    assert_eq!(s.state()["read"]["assist"]["needs_help"], false);
    assert_eq!(s.state()["read"]["assist"]["offer"], "none");
}
