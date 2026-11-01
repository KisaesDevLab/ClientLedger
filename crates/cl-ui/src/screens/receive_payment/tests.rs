//! Receive Payment against the demo engine: the mockup's payment, the
//! state keys the scenario reads, save, bounce, and AccessKit names.

use std::sync::Arc;

use eframe::egui;

use super::*;
use crate::company::CompanyLink;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

/// Polls until the preview of the current form is back.
fn settle(s: &mut ReceivePaymentScreen) {
    for _ in 0..60 {
        s.poll();
        if !matches!(s.form.figures(), FiguresView::Updating) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// The mockup: Neosho Dental Group pays 4,000.00.
fn mockup(link: &Arc<CompanyLink>) -> ReceivePaymentScreen {
    let mut s = ReceivePaymentScreen::new(Arc::clone(link), today());
    s.poll();
    let client = s
        .books
        .contact(crate::company::demo::receivables::STORY_CLIENT)
        .expect("Neosho");
    s.for_client(client);
    s.amount_text = "4000".into();
    s.amount_typed();
    settle(&mut s);
    s
}

#[test]
fn the_mockup_balances_exactly() {
    let (_d, link) = link();
    let s = mockup(&link);
    let v = s.state();
    assert_eq!(v["figures"], "ready");
    assert_eq!(v["rows"][0]["number"], "1091");
    assert_eq!(v["rows"][0]["payment"], "4,000.00");
    assert_eq!(v["rows"][1]["payment"], "0.00");
    assert_eq!(v["amount_due"], "5,645.00");
    assert_eq!(v["applied"], "4,000.00");
    assert_eq!(v["balance_before"], "5,645.00");
    assert_eq!(v["balance_after"], "1,645.00");
    assert_eq!(v["shortfalls"], 1);
    assert_eq!(v["average_days"], 24);
    // The deposit account defaults to Undeposited Funds (Q-202).
    assert_eq!(s.form.deposit_to(), s.clearing());
}

#[test]
fn save_new_posts_and_leaves_385_open_then_bounce_restores() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    assert!(s.start_save(After::New, None));
    s.poll();
    assert_eq!(s.saved_count, 1);
    assert_eq!(s.state()["mode"], "new");
    // The next step counts the sample's two waiting payments and this one.
    assert_eq!(s.state()["next_step"]["count"], 3);
    // Open it again (Prev) and bounce it.
    s.nav(-1);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    s.open_bounce();
    assert_eq!(s.state()["dialog"], "bounce");
    if let Some(f) = s.bounce.as_mut() {
        f.next_step().expect("date");
        f.bank_fee = "35".into();
        f.bank_fee_account = s
            .books
            .accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("6050"))
            .map(|a| a.id);
        // Never deposited: the fee comes out of Operating Checking.
        f.fee_bank_account = s
            .books
            .accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id);
    }
    assert!(s.send_bounce(None), "{:?}", s.notice);
    s.poll();
    let v = s.state();
    assert_eq!(v["bounce"]["done"], true);
    assert_eq!(v["bounce"]["reopened"][0], "4,385.00");
    s.poll();
    assert_eq!(s.state()["bounced"], true);
    assert_eq!(s.state()["read_only"], true);
}

#[test]
fn every_widget_is_named_in_both_themes() {
    let (_d, link) = link();
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        let mut s = mockup(&link);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, theme);
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
        for want in ["Apply, invoice 1091", "Payment, invoice 1095"] {
            assert!(nodes.iter().any(|(_, n)| n.label() == Some(want)), "{want}");
        }
        let unnamed = crate::screens::unnamed_widgets(&nodes);
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }
}

#[test]
fn nothing_applied_asks_and_holds_a_customer_deposit() {
    let (_d, link) = link();
    let mut s = ReceivePaymentScreen::new(Arc::clone(&link), today());
    s.poll();
    // A client with no open invoices in the demo.
    let client = s
        .books
        .contacts
        .iter()
        .find(|c| {
            c.fields.contact_type == cl_api::ContactType::Customer
                && c.fields.display_name != crate::company::demo::receivables::STORY_CLIENT
        })
        .map(|c| c.id)
        .expect("a client");
    s.for_client(client);
    s.amount_text = "500".into();
    s.amount_typed();
    settle(&mut s);
    assert_eq!(s.state()["ask_deposit"], true);
    assert!(!s.start_save(After::New, None));
    assert!(s.problems.contains(&PaymentProblem::AskDeposit));
    s.form.answer_deposit(NothingApplied::Hold);
    settle(&mut s);
    assert!(s.start_save(After::New, None));
    s.poll();
    assert_eq!(s.saved_count, 1);
    let mut demo = link.demo_state().expect("demo");
    let held = crate::company::demo::receivables::deposits_on_hand(&mut demo, client);
    assert_eq!(held.len(), 1);
    assert_eq!(shown(held[0].remaining), "500.00");
}

/// The open invoices' ids by number, from the ready preview.
fn invoice_ids(s: &ReceivePaymentScreen) -> BTreeMap<String, TransactionId> {
    match s.form.figures() {
        FiguresView::Ready(f) => f
            .rows
            .iter()
            .map(|r| (r.number.clone(), r.invoice))
            .collect(),
        _ => BTreeMap::new(),
    }
}

/// D-B1: a payment that used a credit opens with it, and a memo edit sends
/// it back unchanged (the demo keeps the posting: a memo-only edit).
#[test]
fn a_saved_payment_keeps_its_credit_on_a_memo_edit() {
    let (_d, link) = link();
    // 4,400.00 on 1091 alone: 15.00 stays as a credit.
    let mut s = mockup(&link);
    let ids = invoice_ids(&s);
    let client = s.form.client;
    s.form.unapply_all();
    s.toggle(ids["1091"]);
    s.amount_text = "4400".into();
    s.amount_typed();
    settle(&mut s);
    assert_eq!(s.state()["overpayment"], "15.00");
    assert!(s.start_save(After::New, None), "{:?}", s.problems);
    s.poll();
    let credit = s.last_saved.as_ref().map(|r| r.id).expect("credit");
    // 100.00 on 1095 plus the 15.00 credit.
    s.clear();
    s.form.set_client(client);
    s.amount_text = "100".into();
    s.amount_typed();
    settle(&mut s);
    s.form.unapply_all();
    s.toggle(ids["1095"]);
    s.form.use_credit(credit, ids["1095"], None);
    settle(&mut s);
    assert_eq!(s.state()["discounts_and_credits"], "15.00", "{}", s.state());
    assert!(s.start_save(After::Stay, None), "{:?}", s.problems);
    s.poll();
    let rec = s.editing.clone().expect("opened");
    assert_eq!(rec.credit_applications.len(), 1);
    assert_eq!(s.form.credits().len(), 1);
    let ledger = {
        let demo = link.demo_state().expect("demo");
        crate::company::demo::receivables::ledger_of(&demo, rec.id)
    };
    // A memo edit: the same credit goes back and nothing reposts.
    settle(&mut s);
    s.form.memo = "thanks".into();
    assert!(s.start_save(After::Stay, None), "{:?}", s.problems);
    s.poll();
    let after = s.editing.clone().expect("opened");
    assert_eq!(after.memo.as_deref(), Some("thanks"));
    assert_eq!(after.credit_applications.len(), 1);
    assert_eq!(shown(after.credit_applications[0].amount), "15.00");
    let demo = link.demo_state().expect("demo");
    assert_eq!(
        crate::company::demo::receivables::ledger_of(&demo, rec.id),
        ledger
    );
    let open = crate::company::demo::invoices::receivables_of(&demo, rec.customer_id);
    let n1095 = open.iter().find(|i| i.number == "1095").expect("1095");
    assert_eq!(
        shown(Money::try_from(n1095.balance).expect("m")),
        "1,145.00"
    );
}

/// The credit row another receipt used later the same day is not loaded as
/// the payment's own cash.
#[test]
fn a_credit_used_later_the_same_day_is_not_own_cash() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    let ids = invoice_ids(&s);
    let client = s.form.client;
    s.form.unapply_all();
    s.toggle(ids["1091"]);
    s.amount_text = "4400".into();
    s.amount_typed();
    settle(&mut s);
    assert!(s.start_save(After::New, None));
    s.poll();
    let credit = s.last_saved.as_ref().map(|r| r.id).expect("credit");
    s.clear();
    s.form.set_client(client);
    s.amount_text = "100".into();
    s.amount_typed();
    settle(&mut s);
    s.form.unapply_all();
    s.toggle(ids["1095"]);
    s.form.use_credit(credit, ids["1095"], None);
    settle(&mut s);
    assert!(s.start_save(After::New, None));
    s.poll();
    // Open the credit's payment: only 1091 is its own row.
    s.open_id(credit);
    s.poll();
    let rec = s.editing.clone().expect("opened");
    assert_eq!(rec.live_applications().count(), 2);
    let req = s.form.to_preview_request().expect("request");
    let rows = req.rows.expect("explicit");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].invoice, ids["1091"]);
}

/// Runs one frame of the screen with `events`.
fn frame(ctx: &egui::Context, s: &mut ReceivePaymentScreen, events: Vec<egui::Event>) {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440.0, 940.0),
        )),
        events,
        ..egui::RawInput::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
    });
    out.textures_delta.clear();
}

fn key(k: egui::Key) -> Vec<egui::Event> {
    vec![
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

/// UI standard: the arrows move between invoice rows in the same column;
/// Space on a focused Apply box ticks it.
#[test]
fn arrows_move_between_rows_and_space_ticks() {
    use draw::{GridCol, arrow_target, cell_id};
    let (_d, link) = link();
    let mut s = mockup(&link);
    let ids = invoice_ids(&s);
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    for _ in 0..2 {
        frame(&ctx, &mut s, Vec::new());
    }
    // The Payment column.
    ctx.memory_mut(|m| m.request_focus(cell_id(ids["1091"], GridCol::Payment)));
    frame(&ctx, &mut s, Vec::new());
    frame(&ctx, &mut s, key(egui::Key::ArrowDown));
    frame(&ctx, &mut s, Vec::new());
    assert_eq!(
        ctx.memory(egui::Memory::focused),
        Some(cell_id(ids["1095"], GridCol::Payment))
    );
    frame(&ctx, &mut s, key(egui::Key::ArrowUp));
    frame(&ctx, &mut s, Vec::new());
    assert_eq!(
        ctx.memory(egui::Memory::focused),
        Some(cell_id(ids["1091"], GridCol::Payment))
    );
    // The Apply column.
    let boxes = s.grid_boxes.clone();
    assert_eq!(boxes.len(), 2);
    ctx.memory_mut(|m| m.request_focus(boxes[0].1));
    frame(&ctx, &mut s, Vec::new());
    frame(&ctx, &mut s, key(egui::Key::ArrowDown));
    frame(&ctx, &mut s, Vec::new());
    assert_eq!(ctx.memory(egui::Memory::focused), Some(boxes[1].1));
    // The last row stays put on Down.
    frame(&ctx, &mut s, key(egui::Key::ArrowDown));
    frame(&ctx, &mut s, Vec::new());
    assert_eq!(ctx.memory(egui::Memory::focused), Some(boxes[1].1));
    // Space ticks 1095 (Explicit mode, a fill).
    frame(&ctx, &mut s, key(egui::Key::Space));
    assert_eq!(
        s.form.row_choice(ids["1095"]),
        cl_model::forms::payment::RowChoice::Fill
    );
    // The pure step.
    let order = [ids["1091"], ids["1095"]];
    assert_eq!(
        arrow_target(&order, (0, GridCol::Apply), true),
        Some((ids["1095"], GridCol::Apply))
    );
    assert_eq!(arrow_target(&order, (0, GridCol::Payment), false), None);
}

/// Review should-fix 5: any saved payment opens from the register or the
/// Client Center's history (by its transaction id), and Prev walks them
/// all.
#[test]
fn a_payment_opens_from_its_transaction_and_history() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    assert!(s.start_save(After::New, None));
    s.poll();
    let pay = s.last_saved.as_ref().map(|r| r.id).expect("saved");
    let ledger = {
        let demo = link.demo_state().expect("demo");
        crate::company::demo::receivables::ledger_of(&demo, pay).expect("ledger")
    };
    // The register's row (the ledger transaction) opens the payment.
    let mut other = ReceivePaymentScreen::new(Arc::clone(&link), today());
    assert!(crate::windows::Screen::open_record(&mut other, ledger));
    other.poll();
    assert_eq!(other.editing.as_ref().map(|e| e.id), Some(pay));
    assert_eq!(other.state()["mode"], "edit");
    // The bounce is offered on it.
    other.open_bounce();
    assert_eq!(other.state()["dialog"], "bounce");
    // The Client Center's history row of a payment opens Receive Payment.
    assert_eq!(
        crate::screens::lists::center::model::history_form(cl_api::TxnType::CustomerPayment),
        Some(KIND)
    );
    assert_eq!(
        crate::screens::lists::center::model::history_form(cl_api::TxnType::Bill),
        None
    );
}

/// Closes the demo books through `d` (the engine's date and the screen's).
fn close_books(link: &Arc<CompanyLink>, s: &mut ReceivePaymentScreen, d: CalendarDate) {
    let mut demo = link.demo_state().expect("demo");
    let open = demo.open.expect("open");
    if let Some(c) = demo.companies.iter_mut().find(|c| c.tenant_id == open) {
        c.closed_through = Some(d);
    }
    s.books.closing = Some(d);
}

fn type_closing_password(s: &mut ReceivePaymentScreen) {
    s.closing_prompt.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_closing();
    s.poll();
}

/// Review should-fix 7: a bounce in a closed period asks for the password
/// once; the fee invoice, dated the same day, goes with it instead of
/// failing.
#[test]
fn a_closed_period_bounce_sends_its_fee_invoice_with_the_same_password() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    assert!(s.start_save(After::Stay, None));
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    close_books(&link, &mut s, today());
    s.open_bounce();
    let income = s
        .books
        .accounts
        .iter()
        .find(|a| a.is_active && a.fields.account_type == cl_api::AccountType::Revenue)
        .map(|a| a.id);
    let fee_bank = s
        .books
        .accounts
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some("1010"))
        .map(|a| a.id);
    if let Some(f) = s.bounce.as_mut() {
        f.bounce_date = Some(today());
        f.next_step().expect("date");
        f.customer_fee = "25".into();
        f.fee_income_account = income;
        f.fee_bank_account = fee_bank;
    }
    assert!(!s.send_bounce(None));
    assert!(s.closing_prompt.dialog_open(), "the bounce asks first");
    type_closing_password(&mut s);
    s.poll();
    let v = s.state();
    assert_eq!(v["bounce"]["done"], true, "{v}");
    assert_eq!(v["bounce"]["fee_invoice"], true, "{v}");
    assert_eq!(v["bounce"]["fee_failed"], false);
    assert!(!s.closing_prompt.dialog_open());
    assert!(s.bounce_password.is_none(), "dropped once the fee is in");
}

/// Q-P22m: "Refund it" saves the payment and opens Refund a Credit with
/// its credit picked and the preview's overpayment as the amount.
#[test]
fn refund_it_opens_refund_a_credit_prefilled() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    let ids = invoice_ids(&s);
    s.form.unapply_all();
    s.toggle(ids["1091"]);
    s.amount_text = "4400".into();
    s.amount_typed();
    settle(&mut s);
    s.form.set_over(Over::Refund);
    let _ = link.drain();
    assert!(s.start_save(After::Stay, None));
    s.poll();
    let rec = s.last_saved.clone().expect("saved");
    let events = link.drain();
    let opened = events.iter().any(|e| {
        matches!(e, CompanyEvent::OpenWith(k, p)
            if k.as_str() == crate::screens::refund::KIND
                && matches!(**p, crate::windows::OpenPayload::RefundFrom(c, src)
                    if c == rec.customer_id
                        && src.id == rec.id
                        && src.remaining == Money::parse_wire("15.00").expect("m")))
    });
    assert!(opened, "{events:?}");
    assert_eq!(
        s.state()["dialog"],
        serde_json::Value::Null,
        "no inline dialog"
    );
}
/// Void of a payment dated in a closed period asks for the password, then
/// voids (non-negotiable 8).
#[test]
fn a_closed_period_void_asks_for_the_password() {
    let (_d, link) = link();
    let mut s = mockup(&link);
    assert!(s.start_save(After::Stay, None));
    s.poll();
    close_books(&link, &mut s, today());
    assert!(!s.start_void(None));
    assert!(s.closing_prompt.dialog_open());
    assert!(s.misc.is_none(), "nothing sent");
    type_closing_password(&mut s);
    assert_eq!(s.notice, Some("payment.voided"));
    s.poll();
    assert_eq!(s.state()["read_only"], true);
}

/// Saves 4,400.00 on invoice 1091 (4,385.00): a 15.00 credit is left on
/// it. Returns the screen (cleared for the next payment), the credit and
/// the open invoices' ids.
fn with_a_credit(
    link: &Arc<CompanyLink>,
) -> (
    ReceivePaymentScreen,
    TransactionId,
    BTreeMap<String, TransactionId>,
) {
    let mut s = mockup(link);
    let ids = invoice_ids(&s);
    s.form.unapply_all();
    s.toggle(ids["1091"]);
    s.amount_text = "4400".into();
    s.amount_typed();
    settle(&mut s);
    assert!(s.start_save(After::New, None));
    s.poll();
    let credit = s.last_saved.as_ref().map(|r| r.id).expect("credit");
    (s, credit, ids)
}

/// Uses `credit` on invoice 1095 with nothing received.
fn use_credit_alone(s: &mut ReceivePaymentScreen, credit: TransactionId, invoice: TransactionId) {
    let client = s
        .books
        .contact(crate::company::demo::receivables::STORY_CLIENT)
        .expect("Neosho");
    s.for_client(client);
    s.amount_text = "0".into();
    s.amount_typed();
    settle(s);
    s.form.unapply_all();
    s.form.use_credit(credit, invoice, None);
    settle(s);
}

/// Q-P21b: credits alone are not a receipt (the preview reports
/// AMOUNT_REQUIRED, as the engine does), so Save applies the credit to the
/// invoice through `/payments/:id/apply` with the preview's amount.
#[test]
fn credits_alone_apply_to_the_invoice() {
    let (_d, link) = link();
    let (mut s, credit, ids) = with_a_credit(&link);
    use_credit_alone(&mut s, credit, ids["1095"]);
    let plan = s.form.credit_only_plan().expect("credits alone");
    assert_eq!(plan.sources.len(), 1);
    assert_eq!(plan.sources[0].1[0].amount.to_wire(), "15.0000");
    assert!(s.start_save(After::New, None), "{:?}", s.problems);
    s.poll();
    let v = s.state();
    assert_eq!(v["credits_applied"], 1, "{v}");
    assert_eq!(v["saved_count"], 1, "no new payment");
    assert_eq!(v["notice"], serde_json::Value::Null);
    assert!(s.credit_keys.is_none());
    {
        let mut demo = link.demo_state().expect("demo");
        let after = crate::company::demo::receivables::get(&mut demo, credit).expect("rec");
        assert_eq!(after.unapplied_remaining, Money::ZERO);
    }
    // The form is cleared; the invoice now owes 1,245.00.
    let client = s
        .books
        .contact(crate::company::demo::receivables::STORY_CLIENT)
        .expect("Neosho");
    s.for_client(client);
    settle(&mut s);
    let v = s.state();
    assert_eq!(v["rows"][0]["number"], "1095");
    assert_eq!(v["rows"][0]["due"], "1,245.00");
    // Nothing used and nothing received: still "enter the amount".
    s.amount_text = "0".into();
    s.amount_typed();
    settle(&mut s);
    assert!(!s.start_save(After::New, None));
    assert!(
        s.problems.contains(&PaymentProblem::NoAmount),
        "{:?}",
        s.problems
    );
}

/// A credits-only apply dated in a closed period asks for the closing-date
/// password, keeps its key, and applies once the password is given.
#[test]
fn a_closed_period_credit_apply_asks_for_the_password() {
    let (_d, link) = link();
    let (mut s, credit, ids) = with_a_credit(&link);
    use_credit_alone(&mut s, credit, ids["1095"]);
    close_books(&link, &mut s, today());
    assert!(!s.start_save(After::New, None));
    assert!(s.closing_prompt.dialog_open());
    assert!(s.credit_apply.is_none(), "nothing sent");
    type_closing_password(&mut s);
    assert_eq!(s.state()["credits_applied"], 1);
    let mut demo = link.demo_state().expect("demo");
    let after = crate::company::demo::receivables::get(&mut demo, credit).expect("rec");
    assert_eq!(after.unapplied_remaining, Money::ZERO);
}

/// A refused apply keeps the form and says why; the credit is untouched.
#[test]
fn a_refused_credit_apply_shows_the_reason() {
    let (_d, link) = link();
    let (mut s, credit, ids) = with_a_credit(&link);
    use_credit_alone(&mut s, credit, ids["1095"]);
    // Another window takes the credit first.
    {
        let mut demo = link.demo_state().expect("demo");
        crate::company::demo::receivables::apply(
            &mut demo,
            credit,
            &cl_api::ApplyRequest {
                date: today(),
                applications: vec![cl_api::ApplyInvoice {
                    invoice_id: ids["1095"],
                    amount: None,
                    expected_balance_due: None,
                }],
                expected_updated_at: None,
            },
            None,
        )
        .expect("applied elsewhere");
    }
    assert!(s.start_save(After::New, None));
    s.poll();
    let v = s.state();
    assert_eq!(v["credits_applied"], 0);
    assert!(v["notice"].is_string(), "{v}");
    assert!(s.form.client.is_some(), "the form stays");
}

/// The grouped-deposit path through the screen: a check deposited with
/// another payment bounces against the deposit's bank; the other payment
/// stays deposited and the deposit can't be voided while the bounce stands.
#[test]
fn a_check_in_a_grouped_deposit_bounces_against_the_bank() {
    use crate::company::demo::{deposits, receivables};
    let (_d, link) = link();
    let mut s = mockup(&link);
    assert!(s.start_save(After::Stay, None));
    s.poll();
    let pay = s.editing.as_ref().map(|r| r.id).expect("saved");
    let (deposit, other, bank) = {
        let mut demo = link.demo_state().expect("demo");
        let uf = receivables::clearing(&mut demo).expect("uf");
        let bank = s
            .books
            .accounts
            .iter()
            .find(|a| a.fields.account_number.as_deref() == Some("1010"))
            .map(|a| a.id)
            .expect("1010");
        let ledger = receivables::ledger_of(&demo, pay).expect("ledger");
        let waiting = deposits::pending(&mut demo);
        let other = waiting
            .iter()
            .find(|p| p.source != ledger)
            .expect("another payment waiting");
        let group = |source, amount, contact| cl_api::GroupedPayment {
            source,
            kind: cl_api::DepositSource::Payment,
            account: uf,
            amount,
            contact,
            memo: None,
        };
        let draft = cl_api::DepositDraft {
            account: bank,
            date: today(),
            memo: None,
            payments: vec![
                group(
                    ledger,
                    Money::parse_wire("4000.00").expect("m"),
                    s.form.client,
                ),
                group(other.source, other.amount, other.contact),
            ],
            lines: Vec::new(),
            deductions: Vec::new(),
            cash_back: None,
        };
        let dep = deposits::create(&mut demo, &draft, None).expect("deposit");
        (dep, other.source, bank)
    };
    // Open it again: deposited, and the bounce explains the bank.
    s.open_id(pay);
    s.poll();
    assert_eq!(s.state()["deposited"], true);
    s.open_bounce();
    assert!(s.bounce.as_ref().is_some_and(|f| !f.never_deposited));
    if let Some(f) = s.bounce.as_mut() {
        f.bounce_date = Some(today());
        f.next_step().expect("date");
    }
    assert!(s.send_bounce(None), "{:?}", s.notice);
    s.poll();
    let v = s.state();
    assert_eq!(v["bounce"]["done"], true, "{v}");
    assert_eq!(v["bounce"]["reopened"][0], "4,385.00");
    assert_eq!(
        s.bounce
            .as_ref()
            .and_then(|f| f.bounce.as_ref())
            .map(|o| o.held_in),
        Some(bank)
    );
    let mut demo = link.demo_state().expect("demo");
    assert_eq!(
        demo.ledger.deposited_in(other),
        Some(deposit),
        "the other check stays"
    );
    let err = deposits::void(&mut demo, deposit, None).expect_err("refused");
    assert_eq!(
        cl_api::payment_refusal(&err),
        Some(cl_api::PaymentRefusal::Bounced)
    );
}
