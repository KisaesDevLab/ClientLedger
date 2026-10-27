//! Enter Bill in draft mode: A03's Ask AI to help works on the draft once
//! the page words are ready (demo engine; no AI is called).

use cl_api::{AttachTo, DocumentDraftId, DraftKey, DraftKind, IdempotencyKey};
use cl_core::{CalendarDate, Money};
use cl_docread::model::{
    Bbox, Confidence as Bp, Found, PageWords, Reader, Reading, RuleId, Spot, Word, WordSource,
};
use cl_model::forms::doc_read::ReadField;

use super::*;
use crate::company::CompanyLink;
use crate::reading::LocalReading;

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, d).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
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

/// A claimed bill draft whose reading found the date only.
fn claim(link: &Arc<CompanyLink>) -> DocumentDraftId {
    let mut d = link.demo_state().expect("demo");
    d.ai.enabled = true;
    d.ai.reading = true;
    d.ai.assist = true;
    let tenant = d.open.expect("company");
    let a = crate::company::demo::checks::upload(
        &mut d,
        AttachTo::Draft(DraftKey::new()),
        "invoice.pdf",
        b"%PDF-1.4\n% bill kind-ai\n%%EOF\n",
    )
    .expect("upload")
    .id;
    let draft = crate::company::demo::drafts::create(
        &mut d,
        tenant,
        a,
        DraftKind::Bill,
        IdempotencyKey::new(),
    )
    .expect("claim");
    let mut r = Reading::empty(Reader::TextPdf, 1);
    r.kind = cl_docread::model::DocKind::Bill;
    r.header.date = Some(found(day(14)));
    let local = LocalReading {
        reading: r,
        vendor: None,
        card: None,
    };
    let reading = crate::reading::service::draft_reading(&local).expect("reading");
    crate::company::demo::drafts::store_reading(
        &mut d,
        tenant,
        draft.id,
        &draft.updated_at,
        &reading,
    )
    .expect("stored");
    draft.id
}

#[test]
fn ask_ai_on_a_bill_draft_waits_for_the_words_and_keeps_the_owners_values() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let id = claim(&link);
    let ctx = egui::Context::default();
    let mut s = EnterBillScreen::new(Arc::clone(&link), None, day(4));
    s.poll();
    s.open_draft(DraftOpen {
        id,
        form: DraftForm::EnterBill,
    });
    s.poll_draft(&ctx);
    assert_eq!(s.header.current().date, Some(day(14)));
    // The owner changed the date the reading found.
    s.header.edit_step(|h| {
        h.date = Some(day(15));
        h.read.edited(ReadTarget::BillDate);
    });
    // No words yet: nothing to ask with.
    assert_eq!(s.read.state_json()["assist"]["offer"], "none");
    s.start_assist();
    assert_eq!(s.read.local.assist_sent, 0);

    let mut page = Reading::empty(Reader::TextPdf, 1);
    page.words = vec![PageWords {
        page: 0,
        aspect: 12_941,
        source: WordSource::TextLayer,
        words: vec![Word {
            text: "1480.00".into(),
            bbox: Bbox::new(9000, 3000, 9840, 3150).expect("box"),
            confidence: Bp::CERTAIN,
        }],
    }];
    s.draft
        .as_mut()
        .expect("draft mode")
        .give_words(Arc::new(LocalReading {
            reading: page,
            vendor: None,
            card: None,
        }));
    s.poll_draft(&ctx);
    let st = s.read.state_json();
    assert_eq!(st["assist"]["offer"], "local", "{st}");
    s.start_assist();
    assert_eq!(s.read.local.assist_sent, 1);
    s.poll_assist();
    let f = s.header.current();
    assert_eq!(f.amount_due, Some(money("1480")), "printed on the page");
    assert!(matches!(
        f.read.state(ReadTarget::AmountDue),
        Some(ReadField::Read { .. })
    ));
    assert_eq!(f.date, Some(day(15)), "the owner's date stays");
    assert_eq!(f.read.state(ReadTarget::BillDate), Some(ReadField::Edited));
    // The answer's lines are printed nowhere: none was filled.
    assert!(s.expenses.value().lines.iter().all(|l| l.amount.is_none()));
    assert_eq!(s.draft_state()["assist"]["helped"], true);
    assert_eq!(s.read.state_json()["assist"]["offer"], "none");
}
