//! Read a statement on Match Vendor Statement (A06): a stand-in reader
//! answers on the reading service's thread, so these run without the
//! worker; the scenario `statement_match.yaml` drives the real one.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cl_core::{CalendarDate, Money};
use cl_docread::model::{
    Bbox, Check, CheckCode, Confidence as Bp, DocKind, Found, Reader as Source, Reading, RuleId,
    SignSource, Spot, StatementLineRead, StatementRead,
};
use cl_docread::protocol::JobFailure;
use cl_host::{DocJob, JobOutcome};
use serde_json::json;

use super::super::StatementMatchScreen;
use super::COULDNT;
use crate::company::CompanyLink;
use crate::reading::{Reader, ReadingService};
use crate::windows::Screen as _;

const PDF: &[u8] = b"%PDF-1.4\n% synthetic statement\n%%EOF\n";

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn d(m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, m, day).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn found<T>(value: T, bp: u16, row: u16) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(1000, 2000 + row * 300, 9000, 2200 + row * 300).expect("box"),
        },
        rule: RuleId::new("test"),
        confidence: Bp::new(bp).expect("bp"),
    }
}

fn line(
    row: u16,
    date: CalendarDate,
    r: &str,
    amount: &str,
    sign: SignSource,
) -> StatementLineRead {
    StatementLineRead {
        date: found(date, 10_000, row),
        reference: Some(found(r.to_string(), 10_000, row)),
        description: None,
        amount: found(money(amount), if row == 3 { 4_000 } else { 10_000 }, row),
        sign_from: sign,
        running: None,
    }
}

/// The October statement (`vendor_oct.tsv`) as the reader gives it.
fn october(vendor: &str) -> Reading {
    let mut r = Reading::empty(Source::TextPdf, 1);
    r.kind = DocKind::Statement;
    r.statement = Some(StatementRead {
        vendor: Some(found(vendor.to_string(), 10_000, 0)),
        period_from: Some(found(d(10, 1), 10_000, 0)),
        period_to: Some(found(d(10, 31), 10_000, 0)),
        opening: Some(found(money("0"), 10_000, 0)),
        closing: Some(found(money("1032.90"), 10_000, 9)),
        lines: vec![
            line(
                1,
                d(10, 2),
                "INV-55790",
                "1330.00",
                SignSource::ChargesColumn,
            ),
            line(
                2,
                d(10, 9),
                "INV-55811",
                "480.00",
                SignSource::ChargesColumn,
            ),
            line(3, d(10, 12), "CM-2041", "-75.00", SignSource::CreditsColumn),
            line(
                4,
                d(10, 15),
                "PMT 1043",
                "-1810.00",
                SignSource::PaymentsColumn,
            ),
            line(
                5,
                d(10, 20),
                "INV-55840",
                "265.50",
                SignSource::ChargesColumn,
            ),
            line(
                6,
                d(10, 27),
                "inv-55852",
                "912.40",
                SignSource::ChargesColumn,
            ),
        ],
        ..StatementRead::default()
    });
    r
}

struct Canned(JobOutcome);

impl Reader for Canned {
    fn read(&mut self, jobs: &[DocJob], _today: CalendarDate) -> Vec<JobOutcome> {
        assert!(jobs.iter().all(|j| j.hint.statement), "a statement job");
        jobs.iter().map(|_| self.0.clone()).collect()
    }
}

fn service(outcome: JobOutcome) -> Arc<ReadingService> {
    Arc::new(ReadingService::start(Box::new(move || {
        Some(Box::new(Canned(outcome)) as Box<dyn Reader>)
    })))
}

fn screen(link: &Arc<CompanyLink>) -> StatementMatchScreen {
    let mut s = StatementMatchScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn vendor_name(s: &StatementMatchScreen) -> String {
    s.vendors
        .first()
        .map(|v| v.1.clone())
        .expect("a demo vendor")
}

/// Reads `outcome` and waits for the answer.
fn read(s: &mut StatementMatchScreen, outcome: JobOutcome) {
    s.doc.service = Some(service(outcome));
    s.statement_bytes("statement.pdf", PDF.to_vec());
    let start = Instant::now();
    while s.doc.job.is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the reading answers"
        );
        std::thread::sleep(Duration::from_millis(5));
        s.poll();
    }
}

#[test]
fn a_read_statement_fills_the_box_and_waits_for_accept() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    assert_eq!(s.doc.problem, None);
    assert_eq!(
        s.vendor,
        s.vendors.first().map(|v| v.0),
        "the vendor is picked"
    );
    assert_eq!(s.from.value, Some(d(10, 1)));
    assert_eq!(s.to.value, Some(d(10, 31)));
    let st = s.state();
    assert_eq!(st["statement"]["lines"], 6);
    assert_eq!(st["statement"]["pending"], 6);
    assert_eq!(st["statement"]["unsure"], 1);
    assert_eq!(st["statement"]["spots"], 6);
    // Match waits for the owner's accept.
    s.run_match();
    assert_eq!(s.problem, Some("statement.problem.accept_first"));
    assert!(s.outcome.is_none());
    s.accept_read_line(1);
    assert_eq!(s.state()["statement"]["pending"], 5);
    s.accept_read_lines();
    assert!(
        matches!(
            s.state()["statement"]["learned"].as_str(),
            Some("stored" | "nothing")
        ),
        "the statement layout learns on accept"
    );
    s.run_match();
    s.poll();
    assert_eq!(s.problem, None);
    // The read lines go through Match exactly as the typed ones do.
    let r = &s.state()["result"];
    assert_eq!(r["missing_in_books"], json!([5]));
    assert_eq!(r["missing_on_statement"], 1);
    assert_eq!(r["matched"].as_array().map(Vec::len), Some(5));
    // Create bill from the missing line hands over the statement's amount.
    let _ = link.drain();
    let i = s.outcome.as_ref().expect("o").result.missing_in_books[0];
    s.create_from(i);
    assert_eq!(s.state()["last_handoff"], "bill");
}

#[test]
fn picking_a_line_outlines_its_spot_and_a_spot_picks_its_line() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    s.select_read_line(2);
    let key = s.doc.compare.feed().focused.expect("focused");
    assert_eq!(crate::widgets::compare::key_name(key), "statement_line:3");
    s.on_statement_event(crate::widgets::compare::CompareEvent::TargetClicked(
        cl_model::doc_draft::TargetKey::line(super::LINE_TARGET, cl_model::rowkey::RowKey(5)),
    ));
    assert_eq!(s.doc.selected, Some(4));
}

#[test]
fn typing_over_the_read_lines_makes_them_the_owners() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    s.text.push_str("10/30/2026\tINV-9\t5.00\n");
    assert!(!s.doc.awaiting_accept(&s.text));
    assert_eq!(s.state()["statement"]["typed_over"], true);
    s.run_match();
    assert_ne!(s.problem, Some("statement.problem.accept_first"));
}

#[test]
fn unreadable_statements_say_type_or_paste() {
    for outcome in [
        JobOutcome::Read(Box::new(Reading::empty(Source::Ocr, 1))),
        JobOutcome::Failed(JobFailure::Unreadable),
    ] {
        let (_d, link) = link();
        let mut s = screen(&link);
        read(&mut s, outcome);
        assert_eq!(s.doc.problem, Some(COULDNT));
        assert!(s.text.is_empty(), "nothing filled");
        assert_eq!(
            crate::i18n::t(COULDNT),
            "Couldn't read this statement \u{2014} type or paste the lines instead."
        );
    }
}

#[test]
fn another_vendor_on_the_statement_changes_nothing_and_warns() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    let other = s.vendors.get(1).map(|v| v.0).expect("two demo vendors");
    s.vendor = Some(other);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    assert_eq!(s.vendor, Some(other), "unchanged");
    assert_eq!(s.state()["statement"]["vendor_differs"], true);
}

#[test]
fn statement_checks_are_listed_and_change_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    let mut r = october(&name);
    r.checks.push(Check {
        code: CheckCode::StmtSum,
        rule: RuleId::new("stmt.sum"),
        field: None,
        line: None,
        expected: None,
        found: None,
        difference: Some(money("10.00")),
        spot: None,
    });
    read(&mut s, JobOutcome::Read(Box::new(r)));
    assert_eq!(s.state()["statement"]["checks"], json!(["stmtSum"]));
    assert!(
        s.text.contains("1330.00") && s.text.contains("912.40"),
        "as printed"
    );
    assert_eq!(s.doc.closing, Some(money("1032.90")), "never adjusted");
}

/// After Match: the balance line (demo report) and the statement kept with
/// the vendor, named by its period, once.
#[test]
fn match_shows_the_balance_line_and_keeps_the_statement() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    s.accept_read_lines();
    s.run_match();
    s.poll();
    s.poll();
    let (stmt, books, diff) = s.balance_line().expect("balance line");
    assert_eq!(stmt, money("1032.90"));
    // The demo books: 1330 + 480 + 912.40 + 640 − 75 − 1810.
    assert_eq!(books, money("1477.40"));
    assert_eq!(diff, money("-444.50"));
    assert_eq!(s.state()["balance_shown"], true);
    let vendor = s.vendor.expect("vendor");
    let files = crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        cl_api::AttachTo::Contact(vendor),
    );
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_name, "Statement 2026-10-01 to 2026-10-31.pdf");
    assert_eq!(s.state()["statement"]["kept"], true);
    s.run_match();
    s.poll();
    let again = crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        cl_api::AttachTo::Contact(vendor),
    );
    assert_eq!(again.len(), 1, "kept once");
}

#[test]
fn keep_off_keeps_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    s.doc.keep = false;
    s.accept_read_lines();
    s.run_match();
    s.poll();
    let vendor = s.vendor.expect("vendor");
    let files = crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        cl_api::AttachTo::Contact(vendor),
    );
    assert_eq!(files.len(), 0);
    assert_eq!(s.state()["statement"]["kept"], false);
}

#[test]
fn state_carries_no_amounts_names_or_refs() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    s.accept_read_lines();
    s.run_match();
    s.poll();
    let shown = s.state().to_string();
    for secret in [
        "1,330",
        "1330",
        "1032",
        "INV-",
        "CM-",
        "statement.pdf",
        name.as_str(),
    ] {
        assert!(!shown.contains(secret), "{secret} in {shown}");
    }
}

/// The Vendor statements folder kind needs `document-drafts-statement-v1`
/// (the demo has it unless a test turns it off); its files are claimed as
/// statement drafts and read as statements.
#[test]
fn vendor_statement_folders_need_the_capability() {
    use crate::screens::preferences::intake_folders::{kind_key, source};
    let (_d, link) = link();
    assert!(source::statements_supported(&link));
    link.demo_state().expect("demo").intake.no_statements = true;
    assert!(!source::statements_supported(&link));
    assert_eq!(
        crate::i18n::t(kind_key(cl_core::intake::IntakeKind::Statements)),
        "Vendor statements"
    );
    assert_eq!(
        crate::screens::receipt_inbox::intake::drafts::kind_hint(Some(
            cl_core::intake::IntakeKind::Statements
        )),
        cl_api::DraftKind::Statement
    );
}
