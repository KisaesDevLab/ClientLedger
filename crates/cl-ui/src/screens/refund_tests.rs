//! Refund a Credit (P22 D5) against the `--demo` engine: a kept credit
//! memo refunded by check queues in Print Checks with the client's
//! address (Q-P22d), void brings the credit back, a payment credit
//! prefilled from Receive Payment, the engine's over-refund refusal, the
//! empty state, a typed check number, retry keys and AccessKit names.

use cl_api::NewCreditMemo;
use cl_core::Quantity;
use eframe::egui;

use super::*;

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn screen() -> (tempfile::TempDir, RefundScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = RefundScreen::new(link, today());
    (dir, s)
}

fn bluestem(s: &RefundScreen) -> ContactId {
    s.books
        .contact(crate::company::demo::credit_memos::BLUESTEM)
        .expect("Bluestem")
}

/// A 399.30 memo kept as credit (the demo's 8.1 %: 375.00 + 24.30).
fn kept_memo(s: &RefundScreen, c: ContactId) -> TransactionId {
    let income = crate::company::demo::account_id(4000);
    let line = |q: &str, p: &str, tax: bool| {
        let mut l = cl_api::InvoiceLine::account(income, Quantity::parse_wire(q).expect("q"), m(p));
        l.is_taxable = Some(tax);
        l
    };
    let date = CalendarDate::from_ymd(2026, 11, 2).expect("date");
    let mut d = s.link.demo_state().expect("demo");
    crate::company::demo::credit_memos::create_memo(
        &mut d,
        &NewCreditMemo::new(
            c,
            date,
            vec![line("2", "150.00", true), line("1", "75.00", false)],
        ),
        None,
    )
    .expect("memo")
    .id
}

fn queue(s: &RefundScreen) -> Vec<cl_api::QueuedCheck> {
    let mut d = s.link.demo_state().expect("demo");
    let bank = crate::company::demo::printing::demo_bank(&mut d).expect("bank");
    crate::company::demo::printing::check_queue(&mut d, bank)
}

fn memo_left(s: &RefundScreen, id: TransactionId) -> Money {
    let d = s.link.demo_state().expect("demo");
    crate::company::demo::credit_memos::get_memo(&d, id)
        .expect("memo")
        .remaining
}

#[test]
fn a_memo_refunded_by_check_waits_in_print_checks_and_void_brings_it_back() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    let memo = kept_memo(&s, c);
    let before = queue(&s).len();
    s.set_client(Some(c));
    assert_eq!(s.sources.len(), 1, "the memo's credit");
    let row = s.sources[0].clone();
    assert_eq!(row.kind, RefundSourceKind::CreditMemo);
    s.pick_source(&row);
    let v = s.state_json();
    assert_eq!(v["amount"], "399.30", "the engine's remaining string");
    assert_eq!(v["method"], "check");
    assert_eq!(v["print_later"], true, "Q-P22d");
    assert!(s.start_save(After::Stay, None));
    assert_eq!(s.saved_count, 1, "{:?} {:?}", s.problems, s.notice);
    assert_eq!(s.state_json()["saved"]["queued"], true);
    assert_eq!(memo_left(&s, memo), m("0.00"));
    let q = queue(&s);
    assert_eq!(q.len(), before + 1);
    let check = q.iter().find(|x| x.amount == m("399.30")).expect("queued");
    assert_eq!(check.payee, crate::company::demo::credit_memos::BLUESTEM);
    assert!(!check.address.is_empty(), "the client's address");
    // Void: the credit comes back and the check leaves the queue.
    s.void_reason = "Wrong amount".into();
    assert!(s.start_void(None));
    assert_eq!(s.state_json()["saved"]["void"], true);
    assert_eq!(memo_left(&s, memo), m("399.30"));
    assert_eq!(queue(&s).len(), before);
}

#[test]
fn receive_payments_refund_it_prefills_the_payment_credit() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    // A 50.00 payment on nothing: a payment credit.
    let pay = {
        let mut d = s.link.demo_state().expect("demo");
        let clearing = crate::company::demo::receivables::clearing(&mut d).expect("clearing");
        let p = cl_api::NewCustomerPayment {
            customer_id: c,
            date: today(),
            amount: m("50.00"),
            deposit_to: clearing,
            payment_method: None,
            ref_no: None,
            memo: None,
            applications: Vec::new(),
            credit_applications: Vec::new(),
            write_off_account_id: None,
            discount_account_id: None,
            hold_as_deposit: false,
            bank_feed_item_id: None,
        };
        crate::company::demo::receivables::receive(&mut d, &p, None, None)
            .expect("payment")
            .id
    };
    let source = RefundSourceChoice {
        id: pay,
        kind: RefundSourceKind::PaymentCredit,
        remaining: m("50.00"),
    };
    assert!(s.open_with(OpenPayload::RefundFrom(c, source)));
    let v = s.state_json();
    assert_eq!(v["client"], true);
    assert_eq!(v["source"], "paymentcredit");
    assert_eq!(v["amount"], "50.00");
    // Cash: nothing to print.
    s.form.set_method(RefundMethodChoice::Cash);
    assert!(s.start_save(After::Stay, None));
    assert_eq!(s.saved_count, 1, "{:?} {:?}", s.problems, s.notice);
    assert_eq!(s.state_json()["saved"]["queued"], false);
    assert_eq!(s.state_json()["saved"]["source"], "payment");
}

#[test]
fn more_than_is_left_is_the_engines_refusal() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    let memo = kept_memo(&s, c);
    s.set_client(Some(c));
    let row = s.sources[0].clone();
    s.pick_source(&row);
    s.amount_text = "500.00".into();
    s.amount_typed();
    assert!(s.start_save(After::Stay, None), "the UI compares nothing");
    assert_eq!(s.saved_count, 0);
    // The engine's figure from the 409's details (review: details).
    assert_eq!(s.notice, Some("refund.refusal.only_left"));
    assert_eq!(s.left, Some(memo_left(&s, memo)));
    assert!(s.key.is_none(), "a definite refusal: the next try is new");
}

#[test]
fn no_credits_says_so_and_nothing_picked_refuses() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    s.set_client(Some(c));
    assert_eq!(s.sources.len(), 0);
    assert!(!s.start_save(After::Stay, None));
    assert!(s.problems.contains(&RefundProblem::NoSource));
}

#[test]
fn a_typed_check_number_is_not_queued() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    kept_memo(&s, c);
    s.set_client(Some(c));
    let row = s.sources[0].clone();
    s.pick_source(&row);
    s.form.set_print_later(false);
    assert!(!s.start_save(After::Stay, None));
    assert!(s.problems.contains(&RefundProblem::CheckNoOrPrintLater));
    s.check_no = "5120".into();
    assert!(s.start_save(After::Stay, None));
    assert_eq!(s.state_json()["saved"]["queued"], false);
}

#[test]
fn the_new_refund_names_every_widget() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    kept_memo(&s, c);
    s.set_client(Some(c));
    let row = s.sources[0].clone();
    s.pick_source(&row);
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
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
        nodes.extend(
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default(),
        );
    }
    assert_ne!(nodes.len(), 0);
    let unnamed = crate::screens::unnamed_widgets(&nodes);
    assert!(unnamed.is_empty(), "{unnamed:?}");
}

/// Prev/Next walk the saved refunds newest first, like the other forms;
/// Next past the newest is a new refund.
#[test]
fn prev_and_next_walk_the_saved_refunds() {
    let (_d, mut s) = screen();
    let c = bluestem(&s);
    kept_memo(&s, c);
    let mut ids = Vec::new();
    for amount in ["10.00", "20.00"] {
        s.clear();
        s.set_client(Some(c));
        let row = s.sources[0].clone();
        s.pick_source(&row);
        s.amount_text = amount.into();
        s.amount_typed();
        assert!(s.start_save(After::Stay, None));
        ids.push(s.saved.as_ref().map(|r| r.id).expect("saved"));
    }
    s.clear();
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.saved.as_ref().map(|r| r.id), Some(ids[1]), "newest first");
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.saved.as_ref().map(|r| r.id), Some(ids[0]));
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.notice, Some("refund.nav.oldest"));
    s.command(CommandId::new("form.next_record"));
    assert_eq!(s.saved.as_ref().map(|r| r.id), Some(ids[1]));
    s.command(CommandId::new("form.next_record"));
    assert!(s.saved.is_none(), "a new refund");
}
