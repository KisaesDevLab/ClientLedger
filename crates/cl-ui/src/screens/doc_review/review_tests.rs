//! Tests of the draft session against the demo engine (A04 D2, D3, D5;
//! T1, T2, T3, T11, T19).

use cl_api::{AttachTo, DraftKey, SplitLine};
use cl_core::Money;
use cl_docread::model::{Confidence as Bp2, Found, RuleId};

use super::*;
use crate::company::demo::DemoState;
use crate::company::demo::register::{DEMO_CLOSING_PASSWORD, DemoPosting};

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
        confidence: Bp2::CERTAIN,
    }
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
        let tenant = link
            .demo_state()
            .and_then(|d| d.open)
            .expect("open company");
        World {
            _dir: dir,
            link,
            tenant,
            ctx: egui::Context::default(),
        }
    }

    fn demo(&self) -> std::sync::MutexGuard<'_, DemoState> {
        self.link.demo_state().expect("demo")
    }

    fn account(&self, number: &str) -> AccountId {
        let mut d = self.demo();
        let chart = d.chart.get_or_insert_with(crate::company::demo::chart);
        chart
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some(number))
            .map(|a| a.id)
            .expect("account")
    }

    /// A claimed card-charge draft read as `amount` on `date`.
    fn draft(&self, content: &str, amount: &str, date: CalendarDate) -> DocumentDraftId {
        let mut d = self.demo();
        let bytes = format!("%PDF-1.4\n% {content}\n%%EOF\n");
        let a = crate::company::demo::checks::upload(
            &mut d,
            AttachTo::Draft(DraftKey::new()),
            "receipt.pdf",
            bytes.as_bytes(),
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
        r.header.total = Some(found(money(amount)));
        r.header.date = Some(found(date));
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

    fn session(&self, id: DocumentDraftId) -> DraftSession {
        let mut s = DraftSession::new(
            Arc::clone(&self.link),
            Some(self.tenant),
            DraftOpen {
                id,
                form: DraftForm::CardCharge,
            },
        );
        let ev = s.tick(&self.ctx, &[], day(20));
        assert!(matches!(ev.as_slice(), [DraftEvent::Loaded]), "{ev:?}");
        s
    }

    fn accept(&self, amount: &str, date: CalendarDate, password: Option<&str>) -> Accept {
        let body = CardChargeDraft {
            pay_from: self.account("2050"),
            date,
            number: None,
            payee: None,
            memo: None,
            lines: vec![SplitLine {
                account: self.account("6210"),
                amount: money(amount),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        };
        let body = PostBody::CardCharge(body);
        Accept {
            form_digest: body.canon().digest(),
            body,
            billable: true,
            intent: SaveIntent::SaveAndClose,
            password: password.map(Secret::new),
            probe: MatchProbe {
                amount: money(amount),
                date,
                payee: None,
                contact: None,
                account: None,
                account_name: None,
            },
            learn: None,
        }
    }

    fn transactions(&self) -> usize {
        self.demo().ledger.transactions().len()
    }

    fn status(&self, id: DocumentDraftId) -> DraftStatus {
        demo_drafts::get(&self.demo(), self.tenant, id)
            .expect("draft")
            .status
    }

    fn post_expense(&self, amount: &str, date: CalendarDate) -> TransactionId {
        let (card, expense) = (self.account("2050"), self.account("6210"));
        let mut d = self.demo();
        let chart = d
            .chart
            .get_or_insert_with(crate::company::demo::chart)
            .clone();
        let p = DemoPosting {
            kind: "card_charge",
            date,
            number: String::new(),
            contact: None,
            memo: String::new(),
            own: card,
            own_debit: false,
            lines: vec![(expense, money(amount))],
        };
        d.ledger.save(&chart, &p, None).expect("posted")
    }
}

fn tick(w: &World, s: &mut DraftSession) -> Vec<DraftEvent> {
    s.tick(&w.ctx, &[], day(20))
}

#[test]
fn a_stored_draft_reads_back_with_exact_values() {
    let w = World::new();
    let id = w.draft("a", "25.10", day(14));
    let mut s = w.session(id);
    let local = s.take_fill().expect("values");
    assert_eq!(
        local.reading.header.total.as_ref().map(|f| f.value),
        Some(money("25.1"))
    );
    assert_eq!(s.take_fill(), None, "once per load");
    assert!(s.spot_of(ReadTarget::Total).is_some());
    let st = s.state_json();
    assert_eq!(st["stored"], "read");
    assert!(!st.to_string().contains("25.1"), "no amounts");
}

#[test]
fn a_stored_reading_this_version_cant_read_opens_empty() {
    let mut d = DocumentDraft {
        status: DraftStatus::Ready,
        reader: Some(DraftReader::TextPdf),
        fields: Some(json!({"total": "not a found value"})),
        ..serde_json::from_value(json!({
            "id": "d0000000-0000-4000-8000-dd0000000001", "kind": "bill", "status": "new",
            "contentSha256": "", "updatedAt": "t"
        }))
        .expect("draft")
    };
    assert_eq!(stored_reading(&d, &[]), Stored::Unreadable);
    d.status = DraftStatus::New;
    assert_eq!(stored_reading(&d, &[]), Stored::Unread);
}

/// AC1: accept posts once, the digests agree, the draft is converted.
#[test]
fn accept_posts_exactly_once_and_the_digests_agree() {
    let w = World::new();
    let id = w.draft("b", "25.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    assert!(s.accept(w.accept("25.00", day(14), None)));
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before + 1);
    assert_eq!(w.status(id), DraftStatus::Converted);
    let d = digests(&w.ctx);
    assert!(d.form_digest.is_some());
    assert_eq!(d.form_digest, d.posted_digest);
    assert!(s.closed());
    assert!(!s.accept(w.accept("25.00", day(14), None)), "never twice");
}

/// T1: a lost answer is resent identically under the same key: one post.
#[test]
fn a_lost_answer_is_resent_under_the_same_key_and_posts_once() {
    let w = World::new();
    let id = w.draft("c", "40.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    w.demo().drafts.lose_next_reply = true;
    assert!(s.accept(w.accept("40.00", day(14), None)));
    assert_eq!(s.work.name(), "resend");
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(
        w.transactions(),
        before + 1,
        "the replay posted nothing more"
    );
}

/// T1: a closed period asks for the password; the resend with it posts.
#[test]
fn a_closed_period_needs_the_password_then_posts() {
    let w = World::new();
    {
        let mut d = w.demo();
        let t = w.tenant;
        let c = d
            .companies
            .iter_mut()
            .find(|c| c.tenant_id == t)
            .expect("company");
        c.closed_through = Some(day(30));
    }
    let id = w.draft("d", "12.50", day(14));
    let mut s = w.session(id);
    assert!(s.accept(w.accept("12.50", day(14), None)));
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(
            e,
            DraftEvent::Failed {
                error: ApiError::ClosedPeriod(_),
                ..
            }
        )),
        "{ev:?}"
    );
    assert!(s.accept(w.accept("12.50", day(14), Some("wrong"))));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(
        e,
        DraftEvent::Failed {
            error: ApiError::ClosingPasswordRejected,
            ..
        }
    )));
    assert!(s.accept(w.accept("12.50", day(14), Some(DEMO_CLOSING_PASSWORD))));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })));
}

fn close_books_through(w: &World, through: CalendarDate) {
    let mut d = w.demo();
    let t = w.tenant;
    d.companies
        .iter_mut()
        .find(|c| c.tenant_id == t)
        .expect("company")
        .closed_through = Some(through);
}

/// Review B1 (Q-A04f): a receipt dated in the closed period that is
/// already in the books: the register search comes first and the attach
/// needs no closing-date password (nothing posts).
#[test]
fn an_attach_in_the_closed_period_needs_no_password() {
    let w = World::new();
    let target = w.post_expense("25.00", day(13));
    close_books_through(&w, day(30));
    let id = w.draft("cp", "25.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    assert!(s.accept(w.accept("25.00", day(14), None)));
    let ev = tick(&w, &mut s);
    assert!(
        !ev.iter().any(|e| matches!(e, DraftEvent::Failed { .. })),
        "no closing-date question before the search: {ev:?}"
    );
    let d = s.attach.as_mut().expect("the attach dialog first");
    d.select(0);
    assert!(d.ask_attach());
    let answer = d.confirm().expect("confirmed");
    s.dialog_answer(answer);
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter()
            .any(|e| matches!(e, DraftEvent::Attached { transaction } if *transaction == target)),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|e| matches!(e, DraftEvent::Failed { .. })));
    assert_eq!(w.transactions(), before, "nothing posted");
    assert_eq!(w.status(id), DraftStatus::Converted);
}

/// Review B1: a closed-period document with nothing in the books: the
/// search runs, then the engine's 423 asks for the password; the resend
/// with it does not search again and posts once.
#[test]
fn a_closed_period_convert_searches_first_then_asks() {
    let w = World::new();
    close_books_through(&w, day(30));
    let id = w.draft("cq", "12.75", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    assert!(s.accept(w.accept("12.75", day(14), None)));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(
        e,
        DraftEvent::Failed {
            error: ApiError::ClosedPeriod(_),
            ..
        }
    )));
    assert_eq!(w.transactions(), before);
    // A search that would fail now proves the resend doesn't search.
    w.demo().drafts.fail_next_search = true;
    assert!(s.accept(w.accept("12.75", day(14), Some(DEMO_CLOSING_PASSWORD))));
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before + 1);
}

/// Review B2: a failed register search never posts; Try again searches
/// again (and then posts when nothing matches).
#[test]
fn a_failed_search_never_posts_and_try_again_searches_again() {
    let w = World::new();
    let id = w.draft("fs", "18.20", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    w.demo().drafts.fail_next_search = true;
    assert!(s.accept(w.accept("18.20", day(14), None)));
    let ev = tick(&w, &mut s);
    assert!(
        !ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(s.work.name(), "search_failed");
    assert_eq!(s.note, Some("doc_form.match.failed"));
    assert!(
        s.locked(),
        "the reviewed values can't change under the choice"
    );
    assert!(
        !s.accept(w.accept("18.20", day(14), None)),
        "Save doesn't post"
    );
    assert_eq!(w.transactions(), before);
    assert_eq!(w.status(id), DraftStatus::Ready);
    // Esc: back to the form, nothing sent.
    assert_eq!(s.command(CommandId::new("form.revert")), Some(true));
    assert_eq!(s.work.name(), "idle");
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Cancelled)));
    assert_eq!(w.transactions(), before);
    // Again, failing; Try again searches and posts.
    w.demo().drafts.fail_next_search = true;
    assert!(s.accept(w.accept("18.20", day(14), None)));
    assert_eq!(s.work.name(), "search_failed");
    s.try_again();
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before + 1);
}

/// Review B2: after a failed search, Enter as new (Alt+E) is the owner's
/// explicit choice and posts once.
#[test]
fn enter_as_new_after_a_failed_search_posts_once() {
    let w = World::new();
    let id = w.draft("fn", "44.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    w.demo().drafts.fail_next_search = true;
    assert!(s.accept(w.accept("44.00", day(14), None)));
    assert_eq!(s.work.name(), "search_failed");
    assert!(s.attach.is_none());
    // Save & Close again (Enter on the form) doesn't choose for the owner.
    assert!(!s.accept(w.accept("44.00", day(14), None)));
    assert_eq!(w.transactions(), before);
    assert_eq!(s.command(CommandId::new("form.enter_as_new")), Some(true));
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before + 1);
    assert_eq!(w.status(id), DraftStatus::Converted);
    assert!(!s.accept(w.accept("44.00", day(14), None)), "never twice");
}

/// T3: changed elsewhere: "Keep my values and enter" posts the reviewed
/// body under the fresh token.
#[test]
fn a_stale_draft_keeps_my_values_and_enters() {
    let w = World::new();
    let id = w.draft("e", "9.99", day(14));
    let mut s = w.session(id);
    demo_drafts::touch(&mut w.demo(), w.tenant, id).expect("touched");
    assert!(s.accept(w.accept("9.99", day(14), None)));
    let _ = tick(&w, &mut s);
    assert!(matches!(s.dialog, Some(Dialog::Stale { .. })));
    assert!(s.locked());
    s.answer_dialog(true, None);
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.status(id), DraftStatus::Converted);
}

/// Discarded on another computer: nothing posts, and nothing can.
#[test]
fn a_draft_discarded_elsewhere_posts_nothing() {
    let w = World::new();
    let id = w.draft("f", "5.00", day(14));
    let mut s = w.session(id);
    demo_drafts::discard(&mut w.demo(), w.tenant, id).expect("discarded");
    let before = w.transactions();
    assert!(s.accept(w.accept("5.00", day(14), None)));
    let _ = tick(&w, &mut s);
    assert!(matches!(s.dialog, Some(Dialog::DiscardedElsewhere)));
    assert_eq!(w.transactions(), before);
    s.answer_dialog(true, None);
    assert!(!s.accept(w.accept("5.00", day(14), None)));
    assert_eq!(s.note, Some("doc_form.discarded_elsewhere"));
}

/// AC3, T11: a receipt already in the register: the dialog comes first;
/// Cancel sends nothing; a pick and a confirmation attach it without a
/// second transaction.
#[test]
fn a_receipt_already_in_the_books_is_attached_not_entered_again() {
    let w = World::new();
    let target = w.post_expense("25.00", day(13));
    let id = w.draft("g", "25.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    assert!(s.accept(w.accept("25.00", day(14), None)));
    let d = s.attach.as_ref().expect("the attach dialog");
    assert_eq!(d.ids(), vec![target]);
    assert_eq!(d.selected, None);
    // Esc (Cancel): nothing sent.
    assert_eq!(s.command(CommandId::new("form.revert")), Some(true));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Cancelled)));
    assert_eq!(w.transactions(), before);
    assert_eq!(w.status(id), DraftStatus::Ready);
    // Again: pick, Alt+T, confirm.
    assert!(s.accept(w.accept("25.00", day(14), None)));
    // Alt+T without a pick does nothing.
    let _ = s.command(CommandId::new("form.attach_selected"));
    assert!(!s.attach.as_ref().expect("dialog").confirming);
    s.attach.as_mut().expect("dialog").select(0);
    let _ = s.command(CommandId::new("form.attach_selected"));
    let answer = s
        .attach
        .as_mut()
        .expect("dialog")
        .confirm()
        .expect("confirmed");
    s.dialog_answer(answer);
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter()
            .any(|e| matches!(e, DraftEvent::Attached { transaction } if *transaction == target)),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before, "nothing posted");
    let d = demo_drafts::get(&w.demo(), w.tenant, id).expect("draft");
    assert_eq!(d.converted_transaction_id, Some(target));
    assert_eq!(s.learn.layout_outcome, None, "no learning after an attach");
}

/// Enter as new acknowledges the shown records and posts.
#[test]
fn enter_as_new_posts_acknowledging_the_rows_shown() {
    let w = World::new();
    let _ = w.post_expense("30.00", day(14));
    let id = w.draft("h", "30.00", day(14));
    let mut s = w.session(id);
    let before = w.transactions();
    assert!(s.accept(w.accept("30.00", day(14), None)));
    assert_eq!(s.command(CommandId::new("form.enter_as_new")), Some(true));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })));
    assert_eq!(w.transactions(), before + 1);
}

/// A record a cent away, or 8 days away, is never offered.
#[test]
fn near_misses_are_not_offered() {
    let w = World::new();
    let _ = w.post_expense("30.01", day(14));
    let _ = w.post_expense("30.00", day(22));
    let id = w.draft("i", "30.00", day(14));
    let mut s = w.session(id);
    assert!(s.accept(w.accept("30.00", day(14), None)));
    assert!(s.attach.is_none());
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })));
}

#[test]
fn discard_closes_the_draft_and_the_window() {
    let w = World::new();
    let id = w.draft("j", "1.00", day(14));
    let mut s = w.session(id);
    assert_eq!(
        s.command(CommandId::new("form.discard_document")),
        Some(true)
    );
    assert!(matches!(s.dialog, Some(Dialog::ConfirmDiscard)));
    s.answer_dialog(true, Some(&w.ctx));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Close)));
    assert_eq!(w.status(id), DraftStatus::Discarded);
}

#[test]
fn save_and_new_finds_the_next_draft() {
    let w = World::new();
    let first = w.draft("k", "1.00", day(14));
    let second = w.draft("l", "2.00", day(14));
    let mut s = w.session(second);
    assert!(s.accept(w.accept("2.00", day(14), None)));
    let _ = tick(&w, &mut s);
    s.find_next();
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter()
            .any(|e| matches!(e, DraftEvent::Next(Some(o)) if o.id == first)),
        "{ev:?}"
    );
    // With the queue's order published, Page Down steps through it.
    publish_queue(
        &w.ctx,
        vec![(second, DraftKind::CardCharge), (first, DraftKind::Bill)],
    );
    let mut s2 = w.session(first);
    let _ = tick(&w, &mut s2);
    assert!(s2.step(Step::Prev, true));
    assert!(matches!(s2.dialog, Some(Dialog::Leave(_))), "dirty asks");
}

#[test]
fn the_canonical_text_is_the_same_from_the_form_and_the_body() {
    let w = World::new();
    let c = cl_model::forms::card::CheckedCard {
        card: w.account("2050"),
        mode: cl_model::forms::card::CardMode::Charge,
        payee: None,
        date: day(14),
        ref_no: None,
        amount: money("12.50"),
        memo: None,
        header_tags: Vec::new(),
        lines: vec![cl_model::forms::lines::PostingLine {
            account: w.account("6210"),
            amount: money("12.5"),
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        }],
    };
    let body = PostBody::CardCharge(CardChargeDraft {
        pay_from: c.card,
        date: c.date,
        number: None,
        payee: None,
        memo: None,
        lines: vec![SplitLine {
            account: w.account("6210"),
            amount: money("12.50"),
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        }],
    });
    assert_eq!(
        Canon::of_card(DraftKind::CardCharge, &c).digest(),
        body.canon().digest()
    );
    let other = PostBody::CardCharge(CardChargeDraft {
        pay_from: c.card,
        date: c.date,
        number: None,
        payee: None,
        memo: None,
        lines: vec![SplitLine {
            account: w.account("6210"),
            amount: money("12.51"),
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        }],
    });
    assert_ne!(
        body.canon().digest(),
        other.canon().digest(),
        "a cent shows"
    );
}

/// Security review L6: keyed, stable within the run, not the plain SHA-256.
#[test]
fn digests_are_keyed_hex_and_stable_within_the_run() {
    let d = digest(b"abc");
    assert_eq!(d.len(), 64);
    assert_eq!(d, digest(b"abc"));
    assert_ne!(d, digest(b"abd"));
    assert_ne!(
        d,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_ne!(key(), &[0u8; 32]);
    assert_ne!(fallback_key(), [0u8; 32]);
}
