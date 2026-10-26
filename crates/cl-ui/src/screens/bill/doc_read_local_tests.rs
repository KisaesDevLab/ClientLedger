//! Tests for Read document on Enter Bill (A03 step 18, plan §6 AC3, S11):
//! a stand-in reader answers on the reading service's thread, so these run
//! without the worker; the scenario `bill.yaml` drives the real one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::{CalendarDate, Money};
use cl_docread::model::{
    Bbox, Check, CheckCode, CheckValue, Confidence as Bp, Found, Line, Reader as Source, Reading,
    RuleId, Spot,
};
use cl_docread::protocol::JobFailure;
use cl_host::{DocJob, JobOutcome};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::BillProblem;
use eframe::egui;

use crate::company::CompanyLink;
use crate::company::demo::bills::{STORY_PDF, STORY_PDF_NAME, STORY_VENDOR};
use crate::reading::{Reader, ReadingService};
use crate::screens::bill::EnterBillScreen;
use crate::windows::Screen as _;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn found<T>(value: T, bp: u16) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(100, 100, 200, 200).expect("box"),
        },
        rule: RuleId::new("test"),
        confidence: Bp::new(bp).expect("bp"),
    }
}

fn line(desc: &str, amount: &str, bp: u16) -> Line {
    Line {
        description: Some(found(desc.to_string(), 10_000)),
        quantity: None,
        unit_price: None,
        amount: found(money(amount), bp),
    }
}

/// The story invoice as the worker reads it (`midwest_inv_55871.pdf`).
fn story(vendor: &str) -> Reading {
    let mut r = Reading::empty(Source::TextPdf, 1);
    r.header.vendor = Some(found(vendor.to_string(), 10_000));
    r.header.date = Some(found(
        CalendarDate::from_ymd(2026, 11, 3).expect("d"),
        9_000,
    ));
    r.header.reference = Some(found("INV-55871".to_string(), 10_000));
    r.header.total = Some(found(money("1480.00"), 10_000));
    r.lines = vec![
        line("Mini excavator, 2 days", "1330.00", 10_000),
        line("Delivery and pickup", "150.00", 4_000),
    ];
    r
}

/// Answers every job with `outcome`.
struct Canned(JobOutcome);

impl Reader for Canned {
    fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
        jobs.iter().map(|_| self.0.clone()).collect()
    }
}

fn service(outcome: JobOutcome) -> Arc<ReadingService> {
    Arc::new(ReadingService::start(Box::new(move || {
        Some(Box::new(Canned(outcome)) as Box<dyn Reader>)
    })))
}

/// A new bill with the story invoice attached and shown, reading through
/// `service`.
fn with_document(link: &Arc<CompanyLink>, service: Option<Arc<ReadingService>>) -> EnterBillScreen {
    let mut s = EnterBillScreen::new(Arc::clone(link), None, today());
    s.poll();
    s.upload_bytes(STORY_PDF_NAME, STORY_PDF.to_vec());
    assert!(s.attach.viewer.source().is_some(), "the document is shown");
    s.read.local.service = service;
    s
}

/// Reads until cancelled; says when it stopped.
struct UntilCancelled(std::sync::mpsc::Sender<()>);

impl Reader for UntilCancelled {
    fn read(&mut self, _jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
        unreachable!("read through read_cancellable")
    }

    fn read_cancellable(
        &mut self,
        jobs: &[DocJob],
        _today: CalendarDate,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
    ) -> Vec<JobOutcome> {
        let _ = self.0.send(());
        while !cancel.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = self.0.send(());
        jobs.iter()
            .map(|_| JobOutcome::CouldNotRead(cl_host::CouldNotRead::Cancelled))
            .collect()
    }
}

/// Review S2: Cancel stops the reading with no message and the form keeps
/// working; a new bill stops one still running.
#[test]
fn cancel_stops_reading_quietly() {
    let (_d, link) = link();
    let (tx, stopped) = std::sync::mpsc::channel();
    let svc = Arc::new(ReadingService::start(Box::new(move || {
        Some(Box::new(UntilCancelled(tx)) as Box<dyn Reader>)
    })));
    let mut s = with_document(&link, Some(Arc::clone(&svc)));
    assert!(s.form_command(cl_model::CommandId::new("bill.read_local")));
    assert!(s.read.local.busy());
    stopped
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader started");
    s.read.local.cancel_job();
    stopped
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader stopped");
    s.poll_read();
    assert!(!s.read.local.busy());
    assert_eq!(s.read.problem, None, "a cancel shows no message");
    assert_eq!(s.state()["read"]["local"]["outcome"], "cancelled");
    assert!(s.read.filled.is_none());
    // The form still works, and a new reading can start.
    assert!(s.form_command(cl_model::CommandId::new("bill.read_local")));
    assert!(s.read.local.busy());
    stopped
        .recv_timeout(Duration::from_secs(10))
        .expect("the reader started again");
    // A new bill stops it too.
    s.new_bill(false);
    stopped
        .recv_timeout(Duration::from_secs(10))
        .expect("stopped by the new bill");
}

fn writes(link: &Arc<CompanyLink>) -> u32 {
    link.demo_state().expect("demo").bills.writes
}

/// Read document (the keymap command), then poll until the answer is in.
fn read_local(s: &mut EnterBillScreen) {
    assert!(s.form_command(cl_model::CommandId::new("bill.read_local")));
    let start = Instant::now();
    while s.read.local.busy() {
        assert!(start.elapsed() < Duration::from_secs(10), "an answer");
        std::thread::sleep(Duration::from_millis(5));
        s.poll_read();
    }
    s.poll_read();
    s.track_read();
}

fn ai_off(link: &Arc<CompanyLink>) {
    let mut demo = link.demo_state().expect("demo");
    demo.ai.enabled = false;
    demo.ai.reading = false;
}

#[test]
fn read_document_fills_as_read_with_ai_off_and_creates_no_draft() {
    let (_d, link) = link();
    ai_off(&link);
    let mut s = with_document(
        &link,
        Some(service(JobOutcome::Read(Box::new(story(STORY_VENDOR))))),
    );
    let files = s.state()["attachments"]["draft_files"].clone();
    let before = writes(&link);
    read_local(&mut s);
    let st = s.state();
    assert_eq!(st["read"]["local"]["requests"], 1);
    assert_eq!(st["read"]["local"]["outcome"], "read");
    assert_eq!(
        st["read"]["requests"], 0,
        "nothing went to the engine or AI"
    );
    assert_eq!(writes(&link), before, "nothing was written");
    assert_eq!(
        st["attachments"]["draft_files"], files,
        "no draft, no new file"
    );
    assert_eq!(st["read"]["filled"], true);
    assert_eq!(st["read"]["filled_locally"], true);
    // Vendor, date, ref. no., amount due and two lines: all read, none
    // accepted; line 2 was printed unclearly.
    assert_eq!(st["read"]["unaccepted"], 6);
    assert_eq!(st["read"]["low"], 1);
    assert_eq!(st["read"]["reconciled"], true);
    let f = s.header.current().clone();
    assert_eq!(f.reference, "INV-55871");
    assert_eq!(f.amount_due, Some(money("1480.00")));
    assert!(f.vendor.is_some(), "the vendor matched on the desktop");
    let amounts: Vec<_> = s.expenses.value().used().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![Some(money("1330.00")), Some(money("150.00"))]);
    // Refused until accepted, as P17's.
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert!(s.save.problems.contains(&BillProblem::ReadNotAccepted));
    assert_eq!(writes(&link), before);
    assert!(s.form_command(cl_model::CommandId::new("bill.accept_read")));
    s.track_read();
    assert_eq!(s.state()["read"]["unaccepted"], 0);
    let keys: Vec<_> = s.expenses.value().used().map(|l| l.key).collect();
    for k in keys {
        assert!(
            s.expenses.accept_suggestion(k),
            "the vendor's default as a ghost"
        );
    }
    s.poll_read();
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(
        writes(&link),
        before + 1,
        "the owner's save is the only write"
    );
}

#[test]
fn checks_are_listed_and_nothing_is_fixed() {
    let (_d, link) = link();
    let mut r = story(STORY_VENDOR);
    r.header.subtotal = Some(found(money("1490.00"), 10_000));
    r.checks.push(Check {
        code: CheckCode::LinesVsSubtotal,
        rule: RuleId::new("validate.lines"),
        field: None,
        line: None,
        expected: Some(CheckValue::Money(money("1480.00"))),
        found: Some(CheckValue::Money(money("1490.00"))),
        difference: Some(money("10.00")),
        spot: None,
    });
    let mut s = with_document(&link, Some(service(JobOutcome::Read(Box::new(r)))));
    read_local(&mut s);
    let st = s.state();
    assert_eq!(st["read"]["reconciled"], false);
    assert_eq!(st["read"]["checks"][0], "linesVsSubtotal");
    // The printed values stand: the total and the lines as printed.
    assert_eq!(s.header.current().amount_due, Some(money("1480.00")));
    let amounts: Vec<_> = s.expenses.value().used().map(|l| l.amount).collect();
    assert_eq!(amounts, vec![Some(money("1330.00")), Some(money("150.00"))]);
    let text = super::check_text(
        &s.read.filled.as_ref().expect("filled").checks[0],
        cl_core::MoneyStyle::default(),
    );
    assert!(
        text.contains("1,480.00") && text.contains("1,490.00"),
        "{text}"
    );
}

/// Review S6: lines + tax checked against the total says "lines", not
/// "subtotal".
#[test]
fn lines_and_tax_against_total_names_the_lines() {
    let c = Check {
        code: CheckCode::SubtotalTaxVsTotal,
        rule: RuleId::new("validate.lines-total"),
        field: None,
        line: None,
        expected: Some(CheckValue::Money(money("12.00"))),
        found: Some(CheckValue::Money(money("13.00"))),
        difference: Some(money("1.00")),
        spot: None,
    };
    let text = super::check_text(&c, cl_core::MoneyStyle::default());
    assert!(text.starts_with("The lines, tax and fees"), "{text}");
    assert!(text.contains("12.00") && text.contains("13.00"), "{text}");
    // Review S1: a second OCR read's other amount is named.
    let c = Check {
        code: CheckCode::ReadersDisagree,
        rule: RuleId::new("ocr.second-read"),
        ..c
    };
    let text = super::check_text(&c, cl_core::MoneyStyle::default());
    assert!(text.contains("read as 13.00 could also be 12.00"), "{text}");
    // Review S4: a printed name over another vendor's layout names it.
    let c = Check {
        rule: RuleId::new("vendor.layout"),
        expected: Some(CheckValue::Text("Hilltop Feed".into())),
        found: Some(CheckValue::Text("Midwest Equipment Rental".into())),
        difference: None,
        ..c
    };
    let text = super::check_text(&c, cl_core::MoneyStyle::default());
    assert!(
        text.contains("laid out like Hilltop Feed's bills"),
        "{text}"
    );
}

#[test]
fn a_failure_says_enter_by_hand_and_the_form_still_works() {
    for (outcome, key) in [
        (
            JobOutcome::Failed(JobFailure::Protected),
            "bill.read.local.failed.protected",
        ),
        (
            JobOutcome::Failed(JobFailure::Unreadable),
            "bill.read.local.failed",
        ),
        (
            JobOutcome::CouldNotRead(cl_host::CouldNotRead::Crashed),
            "bill.read.local.failed",
        ),
    ] {
        let (_d, link) = link();
        let mut s = with_document(&link, Some(service(outcome)));
        read_local(&mut s);
        assert_eq!(s.read.problem, Some(key));
        assert_eq!(s.state()["read"]["filled"], false);
        assert!(s.header.current().read.is_empty());
        assert!(
            crate::i18n::t(key)
                .to_lowercase()
                .contains("enter the bill by hand"),
            "{key}"
        );
        // Typing by hand still works.
        s.header.edit_step(|h| h.reference = "TYPED".into());
        assert_eq!(s.header.current().reference, "TYPED");
    }
}

#[test]
fn no_reader_on_this_computer_is_a_plain_failure() {
    let (_d, link) = link();
    let none = Arc::new(ReadingService::start(Box::new(|| None)));
    let mut s = with_document(&link, Some(none));
    read_local(&mut s);
    assert_eq!(s.read.problem, Some("bill.read.local.failed.unavailable"));
    assert_eq!(s.state()["read"]["local"]["outcome"], "unavailable");
}

#[test]
fn an_unknown_vendor_is_offered_never_added() {
    let (_d, link) = link();
    let mut s = with_document(
        &link,
        Some(service(JobOutcome::Read(Box::new(story(
            "Nowhere Supply Co.",
        ))))),
    );
    read_local(&mut s);
    assert_eq!(s.state()["read"]["unknown_vendor"], true);
    assert_eq!(s.header.current().vendor, None);
}

#[test]
fn the_button_is_there_with_ai_off_and_named() {
    use crate::theme::ThemeChoice;
    let (_d, link) = link();
    ai_off(&link);
    let mut s = with_document(&link, None);
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut labels = Vec::new();
    for _ in 0..3 {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1440.0, 940.0),
            )),
            ..egui::RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(u) = out.platform_output.accesskit_update {
            labels.extend(
                u.nodes
                    .into_iter()
                    .filter_map(|(_, n)| n.label().map(str::to_string)),
            );
        }
    }
    assert!(labels.iter().any(|l| l == "Read document"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "Read with AI"), "AI is off");
}
