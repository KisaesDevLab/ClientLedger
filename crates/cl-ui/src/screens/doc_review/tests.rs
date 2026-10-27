//! Review Documents on the demo engine (A04 WP-F1, plan §6 AC1): statuses,
//! J/K, Enter's hand-off to the form, the kind chooser, Delete with its
//! confirm, and an engine without document drafts.

use cl_api::{
    AttachTo, DraftKey, DraftKind, DraftReader, DraftReading, DraftStatus, IdempotencyKey,
};
use cl_core::Money;
use serde_json::Map;

use super::*;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn tenant(link: &CompanyLink) -> TenantId {
    link.demo_state().expect("demo").open.expect("open company")
}

/// A claimed draft of `kind` on a fresh synthetic file.
fn claim(link: &CompanyLink, n: u8, kind: DraftKind) -> DocumentDraft {
    let t = tenant(link);
    let mut s = link.demo_state().expect("demo");
    let bytes = format!("%PDF-1.4\n% queue {n}\n%%EOF\n");
    let a = crate::company::demo::checks::upload(
        &mut s,
        AttachTo::Draft(DraftKey::new()),
        "synthetic.pdf",
        bytes.as_bytes(),
    )
    .expect("upload");
    demo_drafts::create(&mut s, t, a.id, kind, IdempotencyKey::new()).expect("claim")
}

fn read(link: &CompanyLink, d: &DocumentDraft, checks: Vec<Value>) -> DocumentDraft {
    let t = tenant(link);
    let mut s = link.demo_state().expect("demo");
    let reading = DraftReading {
        kind: d.kind,
        reader: DraftReader::TextPdf,
        fields: Map::new(),
        lines: Vec::new(),
        checks,
        suggested_contact: None,
        doc_reference: Some("INV-7".into()),
        doc_total: Some(Money::parse_wire("12.50").expect("money")),
        doc_date: Some(today()),
    };
    demo_drafts::store_reading(&mut s, t, d.id, &d.updated_at, &reading).expect("read")
}

fn fail(link: &CompanyLink, d: &DocumentDraft) {
    let t = tenant(link);
    let mut s = link.demo_state().expect("demo");
    demo_drafts::store_failure(&mut s, t, d.id, &d.updated_at, "unreadable").expect("fail");
}

fn screen(link: &Arc<CompanyLink>) -> DocReviewScreen {
    let t = tenant(link);
    DocReviewScreen::new(Arc::clone(link), Some((t, "Demo".into())), today())
}

fn frame(s: &mut DocReviewScreen, ctx: &egui::Context) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui));
    // Nothing paints here; egui insists the texture updates are consumed.
    out.textures_delta.clear();
}

fn statuses(s: &DocReviewScreen) -> Vec<String> {
    s.state()["queue"]["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["status"].as_str().expect("status").to_string())
        .collect()
}

#[test]
fn opens_its_own_kind_only_with_the_list_scope() {
    let (_dir, link) = link();
    let cmd = cl_model::commands::find(KIND).expect("a command");
    assert!(cmd.opens && cmd.arrives.is_none());
    let mut s = make(cmd.id, &link, None, today()).expect("its screen");
    assert_eq!(s.title(), "Review Documents");
    assert_eq!(s.scope(), Scope::List);
    assert!(!s.dirty());
    let other = cl_model::commands::find("vendors.write_check").expect("cmd");
    assert!(make(other.id, &link, None, today()).is_none());
    // A built screen reports its state.
    assert_eq!(s.state()["screen"], "review_documents");
    assert_eq!(s.state()["queue"]["rows"], json!([]));
    assert_eq!(s.state()["queue"]["selected"], Value::Null);
    // J/K on an empty list do nothing and say so.
    assert!(!s.command(CommandId::new("queue.next")));
    assert!(!s.command(CommandId::new("form.save_close")));
}

#[test]
fn the_queue_state_follows_the_window_wherever_it_is() {
    let (_dir, link) = link();
    let mut windows = Windows::default();
    assert_eq!(queue_state(&windows), Value::Null);
    let kind = CommandId::new(KIND);
    let screen = make(kind, &link, None, today()).expect("screen");
    windows.open_new(kind, screen);
    windows.open_new(
        CommandId::new("vendors.enter_bill"),
        Box::new(crate::windows::Placeholder::for_command(CommandId::new(
            "vendors.enter_bill",
        ))),
    );
    assert_eq!(queue_state(&windows)["total"], 0);
}

#[test]
fn the_empty_queue_draws_its_note() {
    let (_dir, link) = link();
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    assert_eq!(s.queue.rows().len(), 0);
    assert_eq!(s.state()["queue"]["supported"], true);
    assert!(format!("{s:?}").contains("DocReviewScreen"));
}

/// AC1: the four statuses of the plan's queue scenario, J/K, a refresh
/// that keeps the selection, and no owner data in the state.
#[test]
fn statuses_keys_and_refresh_on_the_demo() {
    let (_dir, link) = link();
    let ready = claim(&link, 1, DraftKind::Bill);
    read(&link, &ready, Vec::new());
    let check = claim(&link, 2, DraftKind::CardCharge);
    read(&link, &check, vec![json!({"kind": "totals_mismatch"})]);
    let failed = claim(&link, 3, DraftKind::Check);
    fail(&link, &failed);
    let waiting = claim(&link, 4, DraftKind::Unknown);
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    frame(&mut s, &ctx);
    // Newest first.
    assert_eq!(
        statuses(&s),
        ["waiting", "couldnt_read", "check_fields", "ready"]
    );
    let st = s.state()["queue"].clone();
    assert_eq!(st["total"], 4);
    assert_eq!(st["selected"], 0);
    assert_eq!(st["rows"][3]["duplicate"], "clear", "read rows are checked");
    assert_eq!(st["rows"][0]["duplicate"], "not_checked");
    let text = st.to_string();
    for owner in ["INV-7", "12.5", "synthetic", &ready.id.to_string()] {
        assert!(!text.contains(owner), "{owner} in state.queue");
    }
    assert!(s.command(CommandId::new("queue.next")));
    assert!(s.command(CommandId::new("queue.next")));
    assert_eq!(s.queue.selected(), Some(check.id));
    // Another draft arrives; the selection stays on the same document.
    claim(&link, 5, DraftKind::Bill);
    s.reload();
    assert_eq!(s.queue.selected(), Some(check.id));
    assert_eq!(s.state()["queue"]["selected"], 3);
    assert_eq!(s.state()["queue"]["total"], 5);
    let _ = waiting;
}

/// Enter publishes the hand-off and opens the form's window; a draft of
/// unknown kind asks first, with no default (Q-A04n).
#[test]
fn enter_hands_the_draft_to_its_form() {
    let (_dir, link) = link();
    let bill = claim(&link, 1, DraftKind::Bill);
    let bill = read(&link, &bill, Vec::new());
    let unknown = claim(&link, 2, DraftKind::Unknown);
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    let _ = link.drain();
    // The unknown draft is newest: Enter shows the chooser, opens nothing.
    assert_eq!(s.queue.selected(), Some(unknown.id));
    assert!(s.command(CommandId::new("queue.open")));
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["choose_kind"], true);
    assert_eq!(link.drain().len(), 0);
    assert_eq!(take_open(&ctx, DraftForm::EnterBill), None);
    s.open(&ctx, unknown.id, Some(DraftForm::CashExpense));
    assert_eq!(s.state()["queue"]["choose_kind"], false);
    // Cash expenses use the card charge window (its "Expense paid from").
    assert_eq!(take_open(&ctx, DraftForm::EnterBill), None);
    let o = take_open(&ctx, DraftForm::CashExpense).expect("hand-off");
    assert_eq!((o.id, o.form), (unknown.id, DraftForm::CashExpense));
    let _ = link.drain();
    // J, Enter: the bill opens in Enter Bill.
    assert!(s.command(CommandId::new("queue.next")));
    assert!(s.command(CommandId::new("queue.open")));
    frame(&mut s, &ctx);
    let events = link.drain();
    assert!(
        events.iter().any(|e| matches!(
            e,
            CompanyEvent::OpenWindow(c) if c.as_str() == "vendors.enter_bill"
        )),
        "{} events",
        events.len()
    );
    let o = take_open(&ctx, DraftForm::EnterBill).expect("hand-off");
    assert_eq!(o.id, bill.id);
    assert_eq!(o.updated_at, bill.updated_at);
    assert_eq!(take_open(&ctx, DraftForm::EnterBill), None, "taken once");
    assert_eq!(form_key(DraftForm::WriteCheck), "doc_review.choose.check");
    for f in DraftForm::ALL {
        assert!(crate::i18n::has(form_key(f)));
    }
}

/// Delete asks; "Keep it" keeps; Discard discards in the engine and the
/// next document is selected. Nothing posts.
#[test]
fn delete_asks_then_discards_and_selects_the_next() {
    let (_dir, link) = link();
    let older = claim(&link, 1, DraftKind::Bill);
    let newer = claim(&link, 2, DraftKind::Bill);
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    assert_eq!(s.queue.selected(), Some(newer.id));
    assert!(s.command(CommandId::new("queue.discard")));
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["confirm_discard"], true);
    // "Keep it".
    s.confirm = None;
    assert_eq!(s.state()["queue"]["total"], 2);
    assert!(s.command(CommandId::new("queue.discard")));
    frame(&mut s, &ctx);
    s.discard_now(newer.id);
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["confirm_discard"], false);
    assert_eq!(s.state()["queue"]["total"], 1);
    assert_eq!(s.queue.selected(), Some(older.id), "the next one");
    let t = tenant(&link);
    let st = link.demo_state().expect("demo");
    let d = demo_drafts::get(&st, t, newer.id).expect("draft");
    assert_eq!(d.status, DraftStatus::Discarded);
    drop(st);
    // A draft already entered elsewhere can't be discarded: said plainly.
    s.discard_now(newer.id);
    assert_eq!(
        s.state()["queue"]["problem"],
        Value::Null,
        "already gone is fine"
    );
    // Read all without an inbox here says there's nothing to read.
    assert!(s.command(CommandId::new("queue.read_all")));
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["problem"], "doc_review.read_all.none");
}

#[test]
fn an_engine_without_drafts_has_no_queue() {
    let (_dir, link) = link();
    link.demo_state().expect("demo").drafts.enabled = false;
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["supported"], false);
    assert_eq!(s.queue.rows().len(), 0);
}

#[test]
fn pages_of_fifty() {
    let (_dir, link) = link();
    for n in 0..52 {
        claim(&link, n, DraftKind::Bill);
    }
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    assert_eq!(s.queue.rows().len(), 50);
    assert_eq!(s.state()["queue"]["total"], 52);
    s.offset = QUEUE_PAGE;
    s.reload();
    assert_eq!(s.queue.rows().len(), 2);
    assert_eq!(s.state()["queue"]["offset"], 50);
}

/// Home › Needs attention: the engine's count of live drafts, a count and
/// never an amount; none on an engine without drafts.
#[test]
fn home_counts_documents_to_review() {
    use crate::screens::home::{HomeData, Slot, attention};
    use cl_model::home::AttentionKind;
    let (_dir, link) = link();
    let t = tenant(&link);
    let first = claim(&link, 1, DraftKind::Bill);
    let second = claim(&link, 2, DraftKind::Bill);
    claim(&link, 3, DraftKind::Bill);
    fail(&link, &first);
    {
        let mut s = link.demo_state().expect("demo");
        demo_drafts::discard(&mut s, t, second.id).expect("discard");
    }
    let n = {
        let s = link.demo_state().expect("demo");
        assert_eq!(attention::documents_demo(&s, None).ok(), Some(0));
        attention::documents_demo(&s, Some(t)).expect("count")
    };
    assert_eq!(n, 2, "new and failed count; discarded doesn't");
    link.demo_state().expect("demo").drafts.enabled = false;
    let s = link.demo_state().expect("demo");
    assert_eq!(attention::documents_demo(&s, Some(t)).ok(), Some(0));
    drop(s);
    let find = |data: &HomeData| {
        attention::items(data, 7, true, false)
            .into_iter()
            .find(|i| i.kind == AttentionKind::DocumentsToReview)
            .expect("item")
    };
    let data = HomeData {
        documents: Slot::ready(3),
        ..HomeData::default()
    };
    let item = find(&data);
    assert_eq!(item.count, Some(3));
    assert!(item.needs_attention());
    assert_eq!(find(&HomeData::default()).count, None, "still loading");
    assert_eq!(
        crate::i18n::t_count("home.attention.documents", 3, &[]),
        "3 documents to review"
    );
}

/// Enter or Delete pressed on the confirm card's or the chooser's buttons
/// open and discard nothing from the list behind them.
#[test]
fn keys_wait_while_a_card_asks() {
    let (_dir, link) = link();
    let bill = claim(&link, 1, DraftKind::Bill);
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    frame(&mut s, &ctx);
    let _ = link.drain();
    s.confirm = Some(bill.id);
    assert!(s.command(CommandId::new("queue.open")));
    frame(&mut s, &ctx);
    assert_eq!(link.drain().len(), 0, "nothing opened");
    assert_eq!(take_open(&ctx, DraftForm::EnterBill), None);
    assert_eq!(s.confirm, Some(bill.id), "the card still asks");
    s.confirm = None;
    s.choose = Some(bill.id);
    assert!(s.command(CommandId::new("queue.discard")));
    frame(&mut s, &ctx);
    assert_eq!(s.state()["queue"]["confirm_discard"], false);
    assert_eq!(s.state()["queue"]["choose_kind"], true);
}
