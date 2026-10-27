//! Record Card Charge in draft mode: "Enter this as" a cash expense in the
//! same window, and Ask AI to help refilling the form (demo engine).

use cl_api::{AttachTo, DocumentDraftId, DraftKey, DraftKind, IdempotencyKey};
use cl_docread::model::{
    Bbox, Confidence as Bp, Found, PageWords, Reader, Reading, RuleId, Spot, Word, WordSource,
};

use super::*;
use crate::reading::LocalReading;
use crate::screens::card_charge::tests::{link, money, screen};

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

/// A claimed card-charge draft read with a date and, when given, a total.
fn claim(link: &Arc<crate::company::CompanyLink>, total: Option<&str>) -> DocumentDraftId {
    let mut d = link.demo_state().expect("demo");
    let tenant = d.open.expect("company");
    let a = crate::company::demo::checks::upload(
        &mut d,
        AttachTo::Draft(DraftKey::new()),
        "receipt.pdf",
        b"%PDF-1.4\n% card kind-ai\n%%EOF\n",
    )
    .expect("upload")
    .id;
    let draft = crate::company::demo::drafts::create(
        &mut d,
        tenant,
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

fn opened(
    link: &Arc<crate::company::CompanyLink>,
    id: DocumentDraftId,
) -> (CardChargeScreen, egui::Context) {
    let ctx = egui::Context::default();
    let mut s = screen(link);
    s.open_draft(DraftOpen {
        id,
        form: DraftForm::CardCharge,
    });
    s.poll_draft(&ctx);
    (s, ctx)
}

#[test]
fn a_card_charge_becomes_a_cash_expense_in_this_window_keeping_its_marks() {
    let (_d, link) = link();
    let id = claim(&link, Some("25.10"));
    let (mut s, ctx) = opened(&link, id);
    let read = s.header.current().read.clone();
    assert!(matches!(
        read.state(ReadTarget::Total),
        Some(ReadField::Read { .. })
    ));
    s.accept_read();
    s.enter_as(&ctx, DraftForm::CashExpense);
    assert!(s.draft.expense, "the same window, now Expense paid from");
    s.poll_draft(&ctx);
    let f = s.header.current();
    assert_eq!(f.amount, Some(money("25.10")));
    assert_eq!(f.date, Some(day(14)));
    assert_eq!(f.read.state(ReadTarget::Total), Some(ReadField::Accepted));
    assert_eq!(f.read.state(ReadTarget::Date), Some(ReadField::Accepted));
    assert_eq!(f.read.unaccepted(), Vec::new(), "nothing to accept again");
    let st = s.draft_state();
    assert_eq!(st["form"], "cash_expense", "{st}");
    assert_eq!(st["carried"], 2);
    assert_eq!(st["expense_mode"], true);
}

#[test]
fn ask_ai_refills_what_was_not_accepted_and_keeps_what_was() {
    let (_d, link) = link();
    {
        let mut d = link.demo_state().expect("demo");
        d.ai.enabled = true;
        d.ai.reading = true;
        d.ai.assist = true;
    }
    let id = claim(&link, None);
    let (mut s, ctx) = opened(&link, id);
    // The owner changed the date the reading found.
    s.header.edit_step(|h| h.date = Some(day(15)));
    s.track_draft_read();
    assert_eq!(
        s.header.current().read.state(ReadTarget::Date),
        Some(ReadField::Edited)
    );
    assert_eq!(s.draft_state()["assist"]["ready"], false);
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
    let session = s.draft.session.as_mut().expect("draft mode");
    session.give_words(Arc::new(LocalReading {
        reading: page,
        vendor: None,
        card: None,
    }));
    assert!(session.ask_ai(Vec::new()));
    s.poll_draft(&ctx);
    let f = s.header.current();
    assert_eq!(f.amount, Some(money("1480")), "printed on the page");
    assert!(matches!(
        f.read.state(ReadTarget::Total),
        Some(ReadField::Read { .. })
    ));
    assert_eq!(f.date, Some(day(15)), "the owner's date stays");
    assert_eq!(f.read.state(ReadTarget::Date), Some(ReadField::Edited));
    assert_eq!(s.draft_state()["assist"]["helped"], true);
}

/// Review L3: "Remember card ending" is offered only for a credit card
/// account without digits, and never when another account already has the
/// printed digits.
#[test]
fn remember_card_is_not_offered_when_another_account_has_the_digits() {
    let (_d, link) = link();
    let id = claim(&link, Some("25.10"));
    {
        // The document printed the card's last four.
        let mut d = link.demo_state().expect("demo");
        let tenant = d.open.expect("company");
        let draft = crate::company::demo::drafts::get(&d, tenant, id).expect("draft");
        let mut r = Reading::empty(Reader::TextPdf, 1);
        r.kind = cl_docread::model::DocKind::CardCharge;
        r.header.date = Some(found(day(14)));
        r.header.payment = Some(found(cl_docread::model::Payment::Card {
            last4: cl_docread::model::Last4::parse("4417").expect("digits"),
        }));
        let local = LocalReading {
            reading: r,
            vendor: None,
            card: None,
        };
        let reading = crate::reading::service::draft_reading(&local).expect("reading");
        crate::company::demo::drafts::store_reading(
            &mut d,
            tenant,
            id,
            &draft.updated_at,
            &reading,
        )
        .expect("stored");
    }
    let (mut s, _ctx) = opened(&link, id);
    let card = s.books.by_number("2050").expect("card account");
    s.header.edit_step(|h| h.card = Some(card));
    for a in &mut s.books.accounts {
        if a.id == card {
            a.card_last4 = None;
        }
    }
    assert!(s.card_offer(), "a credit card without digits");
    // Another account already has these digits.
    let other = s.books.by_number("6210").expect("another account");
    for a in &mut s.books.accounts {
        if a.id == other {
            a.card_last4 = cl_api::CardLast4::parse("4417");
        }
    }
    assert!(!s.card_offer(), "two accounts with one ending");
    for a in &mut s.books.accounts {
        if a.id == other {
            a.card_last4 = None;
        }
        if a.id == card {
            a.fields.detail_type = Some("checking".into());
        }
    }
    assert!(!s.card_offer(), "only a credit card account");
}

/// Review L4: Save in credit mode in draft mode says why; nothing is sent.
#[test]
fn credit_mode_in_draft_mode_says_why_and_sends_nothing() {
    let (_d, link) = link();
    let id = claim(&link, Some("25.10"));
    let (mut s, _ctx) = opened(&link, id);
    let card = s.books.by_number("2050").expect("card account");
    let c = cl_model::forms::card::CheckedCard {
        card,
        mode: cl_model::forms::card::CardMode::Credit,
        payee: None,
        date: day(14),
        ref_no: None,
        amount: money("25.10"),
        memo: None,
        header_tags: Vec::new(),
        lines: Vec::new(),
    };
    let before = link.demo_state().expect("demo").drafts.requests;
    assert!(!s.accept_draft(&c, cl_model::forms::SaveIntent::SaveAndClose, None));
    assert_eq!(s.flow.notice, Some("doc_form.credit_unavailable"));
    assert_eq!(link.demo_state().expect("demo").drafts.requests, before);
}
