//! Tests of the draft session against the demo engine (A04 D2, D3, D5;
//! T1, T2, T3, T11, T19).

use cl_api::{AttachTo, DraftKey};
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

    /// The card form's checked values for `amount` on `date`.
    fn checked(&self, amount: &str, date: CalendarDate) -> cl_model::forms::card::CheckedCard {
        cl_model::forms::card::CheckedCard {
            card: self.account("2050"),
            mode: cl_model::forms::card::CardMode::Charge,
            payee: None,
            date,
            ref_no: None,
            amount: money(amount),
            memo: None,
            header_tags: Vec::new(),
            lines: vec![cl_model::forms::lines::PostingLine {
                account: self.account("6210"),
                amount: money(amount),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
            item_lines: Vec::new(),
        }
    }

    /// What the card form sends at Accept: the form-side digest of its
    /// checked values and the body its own builder makes from them.
    fn accept(&self, amount: &str, date: CalendarDate, password: Option<&str>) -> Accept {
        let c = self.checked(amount, date);
        Self::accept_with(
            &c,
            crate::screens::card_charge::CardChargeScreen::charge_draft(&c),
            password,
        )
    }

    fn accept_with(
        c: &cl_model::forms::card::CheckedCard,
        body: CardChargeDraft,
        password: Option<&str>,
    ) -> Accept {
        let (amount, date) = (c.amount, c.date);
        Accept {
            form_digest: Canon::of_card(DraftKind::CardCharge, c).digest(),
            body: PostBody::CardCharge(body),
            billable: true,
            intent: SaveIntent::SaveAndClose,
            password: password.map(Secret::new),
            probe: MatchProbe {
                amount,
                date,
                payee: None,
                contact: None,
                account: None,
                account_name: None,
                reference: None,
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
    // The control channel's proof (S3): no new record, the file is on it.
    let proof = digests(&w.ctx).attached.expect("attach proof");
    assert_eq!(proof.records_before, proof.records_after);
    assert_eq!(proof.files_on_record, 1);
    assert!(proof.books_unchanged && proof.file_filed);
    assert_eq!(digests(&w.ctx).to_json()["match"], false, "no convert ran");
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

/// AC1, review S2: the posted digest reads the request body, not the
/// form's values: a body that differs from what the form reviewed shows
/// (here by a cent on the line; the form's own gate never lets this
/// happen, the digest is the independent proof).
#[test]
fn a_body_other_than_the_reviewed_one_shows_in_the_digests() {
    let w = World::new();
    let id = w.draft("dg", "12.50", day(14));
    let mut session = w.session(id);
    let checked = w.checked("12.50", day(14));
    let mut body = crate::screens::card_charge::CardChargeScreen::charge_draft(&checked);
    body.lines[0].amount = money("12.51");
    let mut accept = World::accept_with(&checked, body, None);
    accept.probe.amount = money("12.51");
    assert!(session.accept(accept));
    let _ = tick(&w, &mut session);
    let got = digests(&w.ctx);
    assert!(got.form_digest.is_some() && got.posted_digest.is_some());
    assert_ne!(got.form_digest, got.posted_digest);
    assert_eq!(got.to_json()["match"], false);
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

/// UI check item 4: a failed Read document on a draft not read yet shows
/// "Couldn't read this document"; Read document stays for another try.
#[test]
fn a_failed_read_shows_couldnt_read_and_can_be_tried_again() {
    let w = World::new();
    let id = {
        let mut d = w.demo();
        let a = crate::company::demo::checks::upload(
            &mut d,
            AttachTo::Draft(DraftKey::new()),
            "receipt.pdf",
            b"%PDF-1.4\n% unread\n%%EOF\n",
        )
        .expect("upload")
        .id;
        demo_drafts::create(
            &mut d,
            w.tenant,
            a,
            DraftKind::CardCharge,
            IdempotencyKey::new(),
        )
        .expect("claim")
        .id
    };
    let mut s = w.session(id);
    assert_eq!(s.state_json()["stored"], "unread");
    s.read_failed();
    assert_eq!(s.state_json()["stored"], "failed");
    // Only a draft not read yet changes.
    s.read_failed();
    assert_eq!(s.state_json()["stored"], "failed");
}

/// A waiting money-out line of the demo's Bank Feed: its date and amount.
fn waiting_line(w: &World) -> (CalendarDate, Money) {
    let mut d = w.demo();
    let q = cl_api::FeedQuery {
        waiting_only: true,
        limit: 500,
        ..cl_api::FeedQuery::default()
    };
    let page = crate::company::demo::feed_review::list(&mut d, &q).expect("feed");
    page.rows
        .iter()
        .find(|r| {
            r.is_money_out() && !r.possible_duplicate && !r.entry_voided && r.status.is_waiting()
        })
        .map(|r| (r.date, r.amount.abs()))
        .expect("a waiting line")
}

/// A05 QA-4 (Q-A05h): a document matching a bank line waiting in the Bank
/// Feed asks first. Go to Bank Feed enters nothing and opens the feed;
/// Save anyway posts once and isn't asked again.
#[test]
fn a_receipt_waiting_in_the_bank_feed_asks_before_it_is_entered() {
    let w = World::new();
    let (date, amount) = waiting_line(&w);
    let amount = amount.to_wire();
    let id = w.draft("feed", &amount, date);
    let mut s = w.session(id);
    let before = w.transactions();
    let _ = w.link.drain();
    assert!(s.accept(w.accept(&amount, date, None)));
    let _ = tick(&w, &mut s);
    assert!(
        matches!(s.dialog, Some(Dialog::FeedLine { .. })),
        "{:?}",
        s.dialog
    );
    assert_eq!(s.state_json()["dialog"], "feed_line");
    // Go to Bank Feed: nothing entered, the feed opens.
    s.answer_dialog(true, None);
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Cancelled)));
    assert_eq!(w.transactions(), before);
    assert_eq!(w.status(id), DraftStatus::Ready);
    assert!(w.link.drain().iter().any(|e| matches!(
        e,
        CompanyEvent::OpenWindow(c) if c.as_str() == "banking.feed"
    )));
    // Saved again: asked again; Save anyway posts once.
    assert!(s.accept(w.accept(&amount, date, None)));
    let _ = tick(&w, &mut s);
    assert!(matches!(s.dialog, Some(Dialog::FeedLine { .. })));
    s.answer_dialog(false, None);
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
    assert_eq!(w.transactions(), before + 1);
}

/// A document a cent off, or dated more than 7 days from every waiting
/// line, is entered without the question.
#[test]
fn a_receipt_unlike_any_waiting_line_is_entered_without_asking() {
    let w = World::new();
    let (date, amount) = waiting_line(&w);
    let off = amount.checked_add(money("0.01")).expect("sum").to_wire();
    let id = w.draft("feed-off", &off, date);
    let mut s = w.session(id);
    assert!(s.accept(w.accept(&off, date, None)));
    let ev = tick(&w, &mut s);
    assert!(s.dialog.is_none());
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
        "{ev:?}"
    );
}

/// PR-A1 S1 / T4 (Q-PRA1a): a card receipt of an unpaid bill offers
/// "Pay this bill" (picked, the main action); it opens Pay Bills on the
/// bill, paid from the receipt's card on its date; paying posts one bill
/// payment, the bill closes, the receipt is filed on the payment, and A/P
/// drops by the exact amount. Nothing is entered as a new expense.
#[test]
fn a_receipt_of_an_unpaid_bill_pays_it_in_pay_bills() {
    use crate::company::demo::bills as demo_bills;
    use crate::windows::{OpenPayload, Screen};

    let w = World::new();
    let vendor = w
        .demo()
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == demo_bills::STORY_VENDOR)
        .map(|c| c.id)
        .expect("vendor");
    let bill = {
        let mut d = w.demo();
        let draft = cl_api::BillDraft {
            vendor,
            date: day(10),
            vendor_invoice_number: Some("SYN-4598943".into()),
            terms: None,
            due_date: None,
            memo: None,
            amount_due: money("2197.97"),
            lines: vec![cl_api::BillLineDraft::expense(
                crate::company::demo::account_id(6210),
                money("2197.97"),
            )],
            received: true,
            draft_attachment: None,
        };
        demo_bills::create_bill(&mut d, &draft, None).expect("bill")
    };
    let owed = |w: &World| {
        demo_bills::get(&mut w.demo(), bill)
            .and_then(|v| v.record.balance_due)
            .expect("balance")
    };
    assert_eq!(owed(&w), money("2197.97"));
    let id = w.draft("pay", "2197.97", day(14));
    let mut s = w.session(id);
    let card = w.account("2050");
    let mut form = w.checked("2197.97", day(14));
    form.payee = Some(vendor);
    let mut accept = World::accept_with(
        &form,
        crate::screens::card_charge::CardChargeScreen::charge_draft(&form),
        None,
    );
    accept.probe.contact = Some(vendor);
    accept.probe.account = Some(card);
    let before = w.transactions();
    assert!(s.accept(accept));
    let _ = tick(&w, &mut s);
    let d = s.attach.as_ref().expect("the dialog");
    assert_eq!(d.picked_bill(), Some(bill), "the bill is picked");
    assert_eq!(d.state_json()["payable"], 1);
    s.dialog_answer(DialogAnswer::PayBill(bill));
    let ev = tick(&w, &mut s);
    assert!(ev.iter().any(|e| matches!(e, DraftEvent::Close)), "{ev:?}");
    assert_eq!(w.transactions(), before, "nothing entered as new");
    let payload = w
        .link
        .drain()
        .into_iter()
        .find_map(|e| match e {
            CompanyEvent::OpenWith(kind, p) if kind.as_str() == crate::screens::pay_bills::KIND => {
                Some(*p)
            }
            _ => None,
        })
        .expect("Pay Bills opens");
    let OpenPayload::PayBill(p) = &payload else {
        panic!("a bill to pay: {payload:?}");
    };
    assert_eq!((p.bill, p.from, p.date), (bill, Some(card), day(14)));

    // Pay Bills: ticked, by card from the receipt's card, then paid.
    let mut pay =
        crate::screens::pay_bills::PayBillsScreen::new(Arc::clone(&w.link), None, day(20));
    assert!(pay.open_with(payload));
    pay.poll();
    let st = pay.state();
    assert_eq!(st["selected"], 1, "{st}");
    assert_eq!(st["method"], "card");
    assert_eq!(st["pay_from"], card.to_string());
    assert_eq!(st["date"], day(14).to_wire());
    let row = pay
        .form
        .all_rows()
        .iter()
        .find(|r| r.bill == bill)
        .map(|r| r.balance_due);
    assert_eq!(row, Some(money("2197.97")), "owed in full");
    assert!(pay.save(), "paid");
    pay.poll();
    let st = pay.state();
    assert_eq!(st["receipt"]["filed"], true, "{st}");
    assert_eq!(w.transactions(), before + 1, "one bill payment");
    // A/P drops by exactly the bill: it leaves the payable list (the
    // demo's aging report reads only the sample).
    pay.poll();
    assert!(
        !pay.form.all_rows().iter().any(|r| r.bill == bill),
        "the bill is paid"
    );
    assert_eq!(st["last_bills"], 1, "one bill paid");
    let d = demo_drafts::get(&w.demo(), w.tenant, id).expect("draft");
    assert_eq!(d.status, DraftStatus::Converted);
    assert_eq!(d.converted_type.as_deref(), Some("bill_payment"));
}

/// Q-PRA1a: a bill already paid when "Pay this bill" is picked opens
/// nothing and says so; nothing posts.
#[test]
fn pay_this_bill_on_a_paid_bill_says_so() {
    let w = World::new();
    let id = w.draft("paid", "25.00", day(14));
    let mut s = w.session(id);
    let target = w.post_expense("25.00", day(13));
    // A bill id the demo doesn't know reads as nothing owed.
    s.work = Work::Choosing {
        accept: Box::new(w.accept("25.00", day(14), None)),
    };
    s.dialog_answer(DialogAnswer::PayBill(target));
    let ev = tick(&w, &mut s);
    assert!(
        ev.iter().any(|e| matches!(e, DraftEvent::Cancelled)),
        "{ev:?}"
    );
    assert_eq!(s.note, Some("doc_form.pay_bill.paid"));
    assert!(
        w.link
            .drain()
            .iter()
            .all(|e| !matches!(e, CompanyEvent::OpenWith(..)))
    );
}

/// PR-A1 T13: the app is killed after a convert was sent and before its
/// answer (the engine committed it). On restart the keys are gone: the
/// draft opens as entered, the entry exists once, and accepting again
/// posts nothing. (The original's move is the inbox follow, tested there.)
#[test]
fn a_convert_cut_off_by_a_crash_is_entered_once_after_restart() {
    let w = World::new();
    let id = w.draft("crash", "40.00", day(14));
    let before = w.transactions();
    {
        let mut s = w.session(id);
        w.demo().drafts.lose_next_reply = true;
        assert!(s.accept(w.accept("40.00", day(14), None)));
        assert_eq!(s.work.name(), "resend", "the answer was lost");
        // Killed here: the session (and its key) is gone.
    }
    assert_eq!(w.transactions(), before + 1, "the engine committed it");
    assert_eq!(w.status(id), DraftStatus::Converted);
    // Restart: a fresh session on the same draft.
    let mut s = DraftSession::new(
        Arc::clone(&w.link),
        Some(w.tenant),
        DraftOpen {
            id,
            form: DraftForm::CardCharge,
        },
    );
    let _ = tick(&w, &mut s);
    let _ = s.accept(w.accept("40.00", day(14), None));
    for _ in 0..3 {
        let ev = tick(&w, &mut s);
        assert!(
            !ev.iter().any(|e| matches!(e, DraftEvent::Posted { .. })),
            "{ev:?}"
        );
    }
    assert_eq!(w.transactions(), before + 1, "entered once");
}

/// PR-A1 QA-3: a card charge with no vendor on the form, dated weeks after
/// an open bill with the same Ref no. and total, still asks "Is this
/// already in your books?" with Pay this bill before anything posts.
#[test]
fn a_receipt_with_an_open_bills_reference_asks_before_posting_without_a_vendor() {
    use crate::company::demo::bills as demo_bills;
    let w = World::new();
    let vendor = w
        .demo()
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == demo_bills::STORY_VENDOR)
        .map(|c| c.id)
        .expect("vendor");
    let bill = {
        let mut d = w.demo();
        let draft = cl_api::BillDraft {
            vendor,
            date: day(1),
            vendor_invoice_number: Some("SYN-33994105".into()),
            terms: None,
            due_date: None,
            memo: None,
            amount_due: money("685.55"),
            lines: vec![cl_api::BillLineDraft::expense(
                crate::company::demo::account_id(6210),
                money("685.55"),
            )],
            received: true,
            draft_attachment: None,
        };
        demo_bills::create_bill(&mut d, &draft, None).expect("bill")
    };
    let id = w.draft("ozark", "685.55", day(28));
    let mut s = w.session(id);
    let mut accept = w.accept("685.55", day(28), None);
    accept.probe.reference = Some("SYN-33994105".into());
    let before = w.transactions();
    assert!(s.accept(accept));
    let _ = tick(&w, &mut s);
    let d = s
        .attach
        .as_ref()
        .expect("the dialog, before anything posts");
    assert_eq!(d.picked_bill(), Some(bill), "Pay this bill is offered");
    assert_eq!(w.transactions(), before, "nothing posted yet");

    // Without the reference the dates are too far apart for the amount
    // search, and the draft has no vendor: it would have posted (QA-3).
    let w2 = World::new();
    let id2 = w2.draft("ozark", "685.55", day(28));
    let mut s2 = w2.session(id2);
    assert!(s2.accept(w2.accept("685.55", day(28), None)));
    let _ = tick(&w2, &mut s2);
    assert!(s2.attach.is_none());
}
