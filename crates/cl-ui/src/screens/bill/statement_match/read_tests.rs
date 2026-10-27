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
    s.accept_read_lines();
    s.run_match();
    s.poll();
    assert!(s.outcome.is_some());
    assert_eq!(s.state()["statement"]["kept"], false, "not filed by itself");
    // Re-ticking the box doesn't file it on the other vendor either.
    s.doc.keep = false;
    s.doc.keep = true;
    s.keep_after_match();
    s.poll();
    assert_eq!(s.state()["statement"]["kept"], false);
    // Picking the statement's vendor clears the warning; its Match keeps.
    let named = s.vendors.first().map(|v| v.0);
    s.vendor = named;
    assert_eq!(s.state()["statement"]["vendor_differs"], false);
    s.run_match();
    s.poll();
    assert_eq!(
        s.state()["statement"]["kept"],
        true,
        "kept on the named vendor"
    );
    assert_eq!(files_on(&link, named.expect("vendor")), 1);
    assert_eq!(files_on(&link, other), 0);
}

fn files_on(link: &Arc<CompanyLink>, vendor: cl_core::ids::ContactId) -> usize {
    crate::company::demo::checks::attachments(
        &link.demo_state().expect("demo"),
        cl_api::AttachTo::Contact(vendor),
    )
    .len()
}

/// The reader picks the vendor; the owner then picks another and matches:
/// nothing is filed, the warning shows, and no layout is learned (B2, S3).
#[test]
fn a_vendor_changed_after_the_read_is_never_filed_on() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    let named = s.vendor.expect("picked by the reader");
    let other = s.vendors.get(1).map(|v| v.0).expect("two demo vendors");
    s.vendor = Some(other);
    assert_eq!(s.state()["statement"]["vendor_differs"], true);
    s.accept_read_lines();
    assert_eq!(
        s.state()["statement"]["learned"],
        json!(null),
        "not learned"
    );
    s.run_match();
    s.poll();
    assert!(s.outcome.is_some());
    assert_eq!(s.state()["statement"]["kept"], false);
    assert_eq!(files_on(&link, other), 0);
    assert_eq!(files_on(&link, named), 0);
}

/// A Match for one statement never files the next one: loading a new file
/// drops the Match, and a books change afterwards reads nothing again (B2).
#[test]
fn a_new_file_after_a_match_is_not_kept_by_a_later_reread() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    read(&mut s, JobOutcome::Read(Box::new(october(&name))));
    let vendor = s.vendor.expect("vendor");
    s.accept_read_lines();
    s.run_match();
    s.poll();
    assert_eq!(files_on(&link, vendor), 1, "the first statement is kept");
    read(&mut s, JobOutcome::Failed(JobFailure::Unreadable));
    assert!(s.outcome.is_none(), "the old Match is gone");
    let _ = link.drain();
    link.bus().publish(crate::data_bus::Change::Bills(vendor));
    s.poll();
    s.poll();
    assert!(s.reading.is_none(), "nothing read again");
    assert_eq!(s.state()["statement"]["kept"], false);
    assert_eq!(files_on(&link, vendor), 1, "the second file is not filed");
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
    // Demo parity (review S6): without the capability the demo engine
    // refuses statement claims and statement readings as the engine would.
    {
        use crate::company::demo::{checks, drafts};
        let tenant =
            cl_core::ids::TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("t");
        let mut demo = link.demo_state().expect("demo");
        let file = checks::upload(
            &mut demo,
            cl_api::AttachTo::Draft(cl_api::DraftKey::new()),
            "x.pdf",
            PDF,
        )
        .expect("upload")
        .id;
        let claim = |demo: &mut crate::company::demo::DemoState, kind| {
            drafts::create(demo, tenant, file, kind, cl_api::IdempotencyKey::new())
        };
        assert_eq!(
            claim(&mut demo, cl_api::DraftKind::Statement).map(|_| ()),
            Err(cl_api::ApiError::Local(
                cl_api::LocalValidation::Unsupported
            ))
        );
        let d = claim(&mut demo, cl_api::DraftKind::Unknown).expect("claim");
        let mut local = crate::reading::LocalReading {
            reading: october("Nobody"),
            vendor: None,
            card: None,
        };
        local.reading.kind = DocKind::Statement;
        let r = crate::reading::service::draft_reading(&local).expect("reading");
        assert_eq!(r.kind, cl_api::DraftKind::Statement);
        assert!(drafts::store_reading(&mut demo, tenant, d.id, &d.updated_at, &r).is_err());
        let unknown = cl_api::DraftReading {
            kind: cl_api::DraftKind::Unknown,
            ..r
        };
        drafts::store_reading(&mut demo, tenant, d.id, &d.updated_at, &unknown)
            .expect("unknown is stored");
    }
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

/// Keep from a statement draft (S1): the file goes on the vendor first,
/// then the draft is discarded and its own copy deleted. A failed discard
/// leaves the draft in Review Documents and the file on the vendor; Try
/// again finishes without a second copy.
#[test]
fn keeping_a_statement_draft_never_strands_its_file() {
    use crate::company::demo::{checks, drafts};
    let (_d, link) = link();
    let mut s = screen(&link);
    let name = vendor_name(&s);
    let tenant =
        cl_core::ids::TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("tenant");
    s.doc.tenant = Some(tenant);
    let (file, id) = {
        let mut demo = link.demo_state().expect("demo");
        let file = checks::upload(
            &mut demo,
            cl_api::AttachTo::Draft(cl_api::DraftKey::new()),
            "intake.pdf",
            PDF,
        )
        .expect("upload")
        .id;
        let d = drafts::create(
            &mut demo,
            tenant,
            file,
            cl_api::DraftKind::Statement,
            cl_api::IdempotencyKey::new(),
        )
        .expect("claim");
        (file, d.id)
    };
    s.doc.service = Some(service(JobOutcome::Read(Box::new(october(&name)))));
    s.open_statement_draft(id);
    let start = Instant::now();
    while s.doc.draft_file.is_some() || s.doc.job.is_some() || s.doc.read.is_none() {
        assert!(start.elapsed() < Duration::from_secs(20), "the draft reads");
        std::thread::sleep(Duration::from_millis(5));
        s.poll();
    }
    assert_eq!(s.state()["statement"]["draft"], true);
    let vendor = s.vendor.expect("vendor");
    s.accept_read_lines();
    // The discard fails: the file is already on the vendor, the draft stays.
    link.demo_state().expect("demo").drafts.enabled = false;
    s.run_match();
    s.poll();
    assert_eq!(s.state()["statement"]["keep_failed"], true);
    assert_eq!(s.state()["statement"]["kept"], false);
    assert_eq!(files_on(&link, vendor), 1, "the file is on the vendor");
    link.demo_state().expect("demo").drafts.enabled = true;
    {
        let demo = link.demo_state().expect("demo");
        let d = drafts::get(&demo, tenant, id).expect("draft");
        assert_eq!(
            d.status,
            cl_api::DraftStatus::New,
            "still in Review Documents"
        );
        assert!(checks::find_attachment(&demo, file).is_some());
    }
    // Try again: no second copy, the draft goes, its own copy goes.
    s.keep_after_match();
    s.poll();
    assert_eq!(s.state()["statement"]["kept"], true);
    assert_eq!(s.state()["statement"]["draft"], false);
    assert_eq!(files_on(&link, vendor), 1, "one copy on the vendor");
    let demo = link.demo_state().expect("demo");
    let names: Vec<String> = checks::attachments(&demo, cl_api::AttachTo::Contact(vendor))
        .into_iter()
        .map(|a| a.file_name)
        .collect();
    assert_eq!(
        names,
        ["Statement 2026-10-01 to 2026-10-31.pdf"],
        "named by its period"
    );
    let d = drafts::get(&demo, tenant, id).expect("draft");
    assert_eq!(d.status, cl_api::DraftStatus::Discarded);
    assert!(
        checks::find_attachment(&demo, file).is_none(),
        "its copy is deleted"
    );
}

/// A statement with no closing balance and an unknown vendor; its page
/// prints `amount` and, when `story_vendor`, the demo story vendor's name.
fn sparse_statement(story_vendor: bool, amount: &str) -> Reading {
    let word = |x: u16, y: u16, text: &str| {
        let w = u16::try_from(text.chars().count()).expect("len") * 110;
        cl_docread::model::Word {
            text: text.into(),
            bbox: Bbox::new(x, y, x + w, y + 140).expect("box"),
            confidence: Bp::CERTAIN,
        }
    };
    let mut r = october("Nobody Known Supply");
    if let Some(st) = r.statement.as_mut() {
        st.closing = None;
    }
    let mut words = vec![word(500, 4000, "Total"), word(8500, 4000, amount)];
    if story_vendor {
        words.push(word(500, 300, "Midwest"));
        words.push(word(1400, 300, "Equipment"));
        words.push(word(2600, 300, "Rental"));
    }
    r.words = vec![cl_docread::model::PageWords {
        page: 0,
        aspect: 12_941,
        source: cl_docread::model::WordSource::TextLayer,
        words,
    }];
    r
}

fn assist_on(link: &Arc<CompanyLink>) {
    let mut demo = link.demo_state().expect("demo");
    demo.ai.enabled = true;
    demo.ai.reading = true;
    demo.ai.assist = true;
}

/// Ask AI to help (S4): the demo's answer names a vendor and a total of
/// 1480.00. Printed on the page, the total waits as a suggestion; nothing
/// reaches the picker, the balance line or the keep until the owner uses
/// it, and then it is labelled as the AI's.
#[test]
fn ai_help_waits_for_the_owner_to_use_it() {
    let (_d, link) = link();
    assist_on(&link);
    let mut s = screen(&link);
    read(
        &mut s,
        JobOutcome::Read(Box::new(sparse_statement(true, "1480.00"))),
    );
    assert_eq!(s.vendor, None);
    s.poll();
    assert_eq!(s.state()["statement"]["assist"]["offer"], "local");
    s.start_statement_assist();
    s.poll();
    assert_eq!(s.doc.assist_sent, 1);
    assert_eq!(s.doc.closing, None, "not used yet");
    assert_eq!(s.vendor, None, "the picker is untouched");
    assert_eq!(s.state()["statement"]["closing_suggested"], true);
    let story = s
        .vendors
        .iter()
        .find(|(_, n)| n == crate::company::demo::bills::STORY_VENDOR)
        .map(|v| v.0);
    assert!(story.is_some(), "a demo vendor");
    assert_eq!(s.doc.suggested_vendor, story);
    s.use_suggested_closing();
    assert_eq!(s.doc.closing, Some(money("1480.00")));
    assert!(s.doc.closing_by_ai, "the AI's, and labelled so");
    s.use_suggested_vendor();
    assert_eq!(s.vendor, story, "picked by the owner's Use");
    assert!(!s.vendor_differs());
}

/// The AI's amount gate (Q-A03j): a total the page doesn't print is
/// dropped, never suggested and never used.
#[test]
fn an_ai_amount_not_on_the_page_is_dropped() {
    let (_d, link) = link();
    assist_on(&link);
    let mut s = screen(&link);
    read(
        &mut s,
        JobOutcome::Read(Box::new(sparse_statement(false, "999.99"))),
    );
    s.poll();
    s.start_statement_assist();
    s.poll();
    assert_eq!(s.doc.assist_sent, 1);
    assert_eq!(
        s.doc.suggested_closing, None,
        "invented: dropped by the gate"
    );
    assert_eq!(s.doc.closing, None);
    assert!(!s.doc.closing_by_ai);
}

/// An online provider without the company's consent: Ask AI to help isn't
/// offered and nothing is sent.
#[test]
fn no_consent_means_no_cloud_call() {
    let (_d, link) = link();
    assist_on(&link);
    let mut s = screen(&link);
    read(
        &mut s,
        JobOutcome::Read(Box::new(sparse_statement(false, "1480.00"))),
    );
    s.doc.switches = Some(crate::screens::bill::doc_read::ReadSwitches {
        ai: true,
        reading: true,
        provider: Some(cl_settings::AiProvider::Anthropic),
        assist: true,
        ..crate::screens::bill::doc_read::ReadSwitches::default()
    });
    assert_eq!(
        s.doc.offer(),
        crate::screens::bill::doc_read_local::Offer::NeedsConsent
    );
    s.start_statement_assist();
    assert_eq!(s.doc.assist_sent, 0);
    assert!(s.doc.assist.is_none(), "nothing sent");
}
