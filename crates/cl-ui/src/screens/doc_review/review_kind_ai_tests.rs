//! "Enter this as" and Ask AI to help in draft mode, against the demo
//! engine (its assist answers the story invoice; no AI is called).

use std::sync::Arc;

use cl_api::{AttachTo, DraftKey, IdempotencyKey};
use cl_core::ids::TenantId;
use cl_docread::model::{
    Bbox, CheckCode, Confidence as Bp, Found, PageWords, Reader, Reading, RuleId, Spot, Word,
    WordSource,
};
use cl_model::forms::doc_read::Confidence;
use cl_model::rowkey::RowKeyGen;

use super::super::{DraftOpen, demo_drafts};
use super::*;
use crate::company::CompanyLink;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, d).expect("date")
}

fn found<T>(value: T) -> Found<T> {
    Found {
        value,
        spot: Spot {
            page: 0,
            bbox: Bbox::new(7000, 8000, 8000, 8200).expect("box"),
        },
        rule: RuleId::new("test"),
        confidence: Bp::CERTAIN,
    }
}

fn word(x: u16, y: u16, text: &str) -> Word {
    let w = u16::try_from(text.chars().count()).expect("short") * 120;
    Word {
        text: text.into(),
        bbox: Bbox::new(x, y, x + w, y + 150).expect("box"),
        confidence: Bp::CERTAIN,
    }
}

/// The page words of a re-read: `amount` printed as the total.
fn words(amount: &str) -> Arc<LocalReading> {
    let mut r = Reading::empty(Reader::TextPdf, 1);
    r.words = vec![PageWords {
        page: 0,
        aspect: 12_941,
        source: WordSource::TextLayer,
        words: vec![
            word(500, 300, "Receipt"),
            word(500, 3000, "Total"),
            word(9000, 3000, amount),
        ],
    }];
    Arc::new(LocalReading {
        reading: r,
        vendor: None,
        card: None,
    })
}

struct World {
    _dir: tempfile::TempDir,
    link: Arc<CompanyLink>,
    tenant: TenantId,
    ctx: egui::Context,
}

impl World {
    fn new() -> World {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        let tenant = link.demo_state().and_then(|d| d.open).expect("company");
        World {
            _dir: dir,
            link,
            tenant,
            ctx: egui::Context::default(),
        }
    }

    fn ai(&self, on: bool) {
        let mut d = self.link.demo_state().expect("demo");
        d.ai.enabled = on;
        d.ai.reading = on;
        d.ai.assist = on;
    }

    /// A claimed card-charge draft read with a date and, when given, a
    /// total.
    fn draft(&self, total: Option<&str>) -> DocumentDraftId {
        let mut d = self.link.demo_state().expect("demo");
        let a = crate::company::demo::checks::upload(
            &mut d,
            AttachTo::Draft(DraftKey::new()),
            "receipt.pdf",
            b"%PDF-1.4\n% kind-ai\n%%EOF\n",
        )
        .expect("upload")
        .id;
        let draft = demo_drafts::create(
            &mut d,
            self.tenant,
            a,
            DraftKind::CardCharge,
            IdempotencyKey::new(),
        )
        .expect("claim");
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.kind = cl_docread::model::DocKind::CardCharge;
        r.header.date = Some(found(day(14)));
        r.header.total = total.map(|t| found(money(t)));
        let local = LocalReading {
            reading: r,
            vendor: None,
            card: None,
        };
        let reading = crate::reading::service::draft_reading(&local).expect("reading");
        demo_drafts::store_reading(&mut d, self.tenant, draft.id, &draft.updated_at, &reading)
            .expect("stored");
        draft.id
    }

    fn session(&self, id: DocumentDraftId, form: DraftForm) -> DraftSession {
        let mut s = DraftSession::new(
            Arc::clone(&self.link),
            Some(self.tenant),
            DraftOpen { id, form },
        );
        let ev = s.tick(&self.ctx, &[], day(20));
        assert!(matches!(ev.as_slice(), [DraftEvent::Loaded]), "{ev:?}");
        s
    }
}

// ============================================================== assist

/// Q-A03j in draft mode: offered as A03 offers it, clickable only once the
/// words are ready, and an AI amount is kept only when a page word equals
/// it.
#[test]
fn ask_ai_waits_for_the_words_and_keeps_only_printed_amounts() {
    let w = World::new();
    w.ai(true);
    let id = w.draft(None);
    let mut s = w.session(id, DraftForm::CardCharge);
    let st = s.assist_json();
    assert_eq!(st["offer"], "local", "{st}");
    assert_eq!(st["needs_help"], true, "no total was read");
    assert_eq!(st["ready"], false);
    assert!(!s.ask_ai(Vec::new()), "nothing goes before the words");
    assert_eq!(s.assist.sent, 0);

    // The re-read's words arrive: the demo's answer says 1480.00 and the
    // page prints it.
    s.set_words(words("1480.00"));
    assert_eq!(s.assist_json()["ready"], true);
    assert!(s.ask_ai(Vec::new()));
    let ev = s.tick(&w.ctx, &[], day(20));
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Helped)), "{ev:?}");
    let filled = s.take_fill().expect("the merged values");
    let h = &filled.reading.header;
    assert_eq!(h.total.as_ref().map(|f| f.value), Some(money("1480")));
    // Its lines (1330.00, 150.00) are printed nowhere: not kept.
    assert_eq!(filled.reading.lines.len(), 0);
    assert!(
        filled
            .reading
            .checks
            .iter()
            .any(|c| c.code == CheckCode::AiDisagrees),
        "an AI amount not on the page is a check, never a value"
    );
    // The date the reading found stays the reading's.
    assert_eq!(h.date.as_ref().map(|f| f.value), Some(day(14)));
    let st = s.assist_json();
    assert_eq!(st["helped"], true);
    assert_eq!(st["outcome"], "helped");
    assert_eq!(st["requests"], 1);
    assert_eq!(st["offer"], "none", "one ask per reading");
    assert!(!s.ask_ai(Vec::new()));
    assert_eq!(s.state_json()["assist"]["helped"], true);
}

#[test]
fn an_ai_total_with_no_equal_page_word_is_never_kept() {
    let w = World::new();
    w.ai(true);
    let id = w.draft(None);
    let mut s = w.session(id, DraftForm::WriteCheck);
    s.set_words(words("14.80"));
    assert!(s.ask_ai(Vec::new()));
    let _ = s.tick(&w.ctx, &[], day(20));
    let filled = s.take_fill().expect("values");
    assert_eq!(filled.reading.header.total, None);
    assert!(
        filled
            .reading
            .checks
            .iter()
            .any(|c| c.code == CheckCode::AiDisagrees)
    );
}

#[test]
fn ask_ai_is_not_offered_when_ai_is_off_or_on_a_bill() {
    let w = World::new();
    w.ai(false);
    let id = w.draft(None);
    let mut s = w.session(id, DraftForm::CardCharge);
    s.set_words(words("1480.00"));
    assert_eq!(s.assist_json()["offer"], "none");
    assert!(!s.ask_ai(Vec::new()));

    w.ai(true);

    // Enter Bill offers A03's own button; the session never does.
    let mut s = w.session(id, DraftForm::EnterBill);
    s.set_words(words("1480.00"));
    assert_eq!(s.assist_json()["offer"], "none");
    let base = s.assist_base().expect("values and words");
    assert!(!base.reading.words.is_empty(), "the gate gets the words");
    assert_eq!(
        base.reading.header.date.as_ref().map(|f| f.value),
        Some(day(14))
    );
}

/// An answer that arrives after the draft was entered changes nothing.
#[test]
fn a_late_answer_is_dropped_when_the_draft_is_closed() {
    let w = World::new();
    w.ai(true);
    let id = w.draft(None);
    let mut s = w.session(id, DraftForm::CardCharge);
    s.set_words(words("1480.00"));
    assert!(s.ask_ai(Vec::new()));
    s.closed_elsewhere = true;
    let ev = s.tick(&w.ctx, &[], day(20));
    assert!(!ev.iter().any(|e| matches!(e, DraftEvent::Helped)));
    assert_eq!(s.assist.outcome, Some("dropped"));
}

// ============================================================ enter as

fn values(marks: &[(ReadTarget, ReadField)], lines: Vec<FormLine>) -> FormValues {
    let mut m = ReadMarks::default();
    for (t, s) in marks {
        m.mark_read(*t, Confidence::High);
        m.restore(*t, *s);
    }
    FormValues {
        payee: Some(ContactId::parse("00000000-0000-0000-0000-000000000007").expect("id")),
        date: Some(day(15)),
        total: Some(money("25.10")),
        paid_from: Some(AccountId::parse("00000000-0000-0000-0000-000000000009").expect("id")),
        lines,
        marks: m,
    }
}

/// Q-A04n: accepted and changed values go with their marks, mapped onto
/// the other form's fields; values still read go nowhere (the new form
/// reads them again and they still need Accept); a changed value never
/// becomes an accepted read value.
#[test]
fn enter_as_carries_accepted_and_changed_values_with_their_marks() {
    let w = World::new();
    let id = w.draft(Some("25.10"));
    let s = w.session(id, DraftForm::CardCharge);
    let read = ReadField::Read {
        level: Confidence::High,
    };
    let v = values(
        &[
            (ReadTarget::Total, ReadField::Accepted),
            (ReadTarget::Date, ReadField::Edited),
            (ReadTarget::Payee, read),
        ],
        Vec::new(),
    );
    assert!(s.enter_as(DraftForm::CardCharge, &v).is_none(), "same form");
    let c = s.enter_as(DraftForm::EnterBill, &v).expect("carry");
    assert_eq!(c.kept(), 2);
    assert!(c.payee.is_none(), "a value still read isn't carried");
    assert!(!format!("{c:?}").contains("25"), "Debug shows no values");

    // The bill read the same document: its fields are read, not accepted.
    let mut marks = ReadMarks::default();
    for t in [
        ReadTarget::AmountDue,
        ReadTarget::BillDate,
        ReadTarget::Vendor,
    ] {
        marks.mark_read(t, Confidence::High);
    }
    let mut bill = FormValues {
        payee: None,
        date: Some(day(14)),
        total: Some(money("25.10")),
        paid_from: None,
        lines: Vec::new(),
        marks,
    };
    c.apply_to(DraftForm::EnterBill, &mut bill, |_| None, |_| true);
    assert_eq!(
        bill.marks.state(ReadTarget::AmountDue),
        Some(ReadField::Accepted)
    );
    assert_eq!(
        bill.marks.state(ReadTarget::BillDate),
        Some(ReadField::Edited),
        "changed stays changed"
    );
    assert_eq!(bill.date, Some(day(15)), "the owner's own date");
    assert_eq!(bill.marks.unaccepted(), vec![ReadTarget::Vendor]);
    assert_eq!(bill.paid_from, None, "a bill has no paid-from account");
}

#[test]
fn enter_as_maps_read_lines_by_their_order_and_checks_the_account() {
    let w = World::new();
    let id = w.draft(Some("25.10"));
    let mut s = w.session(id, DraftForm::CardCharge);
    let mut keys = RowKeyGen::default();
    let (k0, k1) = (keys.next_key(), keys.next_key());
    s.filled(&[ReadTarget::Line(k0), ReadTarget::Line(k1)]);
    let line = |key, amount: &str| FormLine {
        amount: Some(money(amount)),
        memo: "Lumber".into(),
        ..FormLine::blank(key)
    };
    let v = values(
        &[
            (ReadTarget::Line(k1), ReadField::Edited),
            (ReadTarget::PaidFrom, ReadField::Accepted),
        ],
        vec![line(k0, "10.00"), line(k1, "15.10")],
    );
    let c = s.enter_as(DraftForm::WriteCheck, &v).expect("carry");
    assert_eq!(c.lines.len(), 2);
    assert!(c.lines[0].is_none() && c.lines[1].is_some());

    // The check filled its own rows from the reading.
    let (n0, n1) = (keys.next_key(), keys.next_key());
    let mut marks = ReadMarks::default();
    marks.mark_read(ReadTarget::Line(n0), Confidence::High);
    marks.mark_read(ReadTarget::Line(n1), Confidence::High);
    let mut check = FormValues {
        lines: vec![line(n0, "10.00"), line(n1, "15.00")],
        marks,
        ..FormValues::default()
    };
    let rows = [n0, n1];
    c.apply_to(
        DraftForm::WriteCheck,
        &mut check,
        |i| rows.get(i).copied(),
        |_| false,
    );
    assert_eq!(check.lines[1].amount, Some(money("15.10")));
    assert_eq!(
        check.marks.state(ReadTarget::Line(n1)),
        Some(ReadField::Edited)
    );
    assert_eq!(check.marks.unaccepted(), vec![ReadTarget::Line(n0)]);
    assert_eq!(check.paid_from, None, "an account this form doesn't list");
    assert_eq!(check.marks.state(ReadTarget::PaidFrom), None);
}

/// The carry reaches the form it was made for, once, on its first frame,
/// and brings the session's values for the same draft version.
#[test]
fn a_carry_reaches_only_its_form_once_with_the_sessions_values() {
    let w = World::new();
    let id = w.draft(Some("25.10"));
    let mut s = w.session(id, DraftForm::CardCharge);
    // AI helped in this form: its values aren't stored on the draft.
    if let Some(Stored::Read(r)) = s.stored.as_mut() {
        r.reading.header.total = Some(found(money("99.00")));
    }
    s.assist.helped = true;
    let v = values(&[(ReadTarget::Total, ReadField::Accepted)], Vec::new());
    let c = s.enter_as(DraftForm::WriteCheck, &v).expect("carry");
    publish_carry(&w.ctx, c);

    // Another form of the same draft doesn't take it.
    let other = w.session(id, DraftForm::EnterBill);
    assert_eq!(other.state_json()["carried"], serde_json::Value::Null);

    let mut check = w.session(id, DraftForm::WriteCheck);
    assert_eq!(check.state_json()["carried"], 1);
    let fill = check.take_fill().expect("values");
    assert_eq!(
        fill.reading.header.total.as_ref().map(|f| f.value),
        Some(money("99")),
        "the session's values, not the stored ones"
    );
    assert_eq!(check.assist_json()["helped"], true, "one ask per reading");
    assert!(check.take_carry().is_some());
    assert!(check.take_carry().is_none(), "once");

    let again = w.session(id, DraftForm::WriteCheck);
    assert_eq!(again.state_json()["carried"], serde_json::Value::Null);
}

#[test]
fn enter_as_is_refused_while_the_draft_is_closed() {
    let w = World::new();
    let id = w.draft(Some("25.10"));
    let mut s = w.session(id, DraftForm::CardCharge);
    s.closed_elsewhere = true;
    let v = values(&[], Vec::new());
    assert!(s.enter_as(DraftForm::EnterBill, &v).is_none());
    assert_eq!(kind_of(DraftForm::CashExpense), DraftKind::CashExpense);
    assert_eq!(kind_of(DraftForm::WriteCheck), DraftKind::Check);
}
