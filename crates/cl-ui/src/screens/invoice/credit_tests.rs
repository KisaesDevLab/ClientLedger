//! Credit Memo and Sales Receipt (P22 D4) against the `--demo` engine:
//! totals from the kind's preview, the apply-or-keep question on save
//! (Q-P22b), the saved memo's history, Refund opening Refund a Credit, a
//! sales receipt's Deposit to and a negative receipt needing a bank.

use std::sync::Arc;
use std::time::Duration;

use cl_core::ids::ContactId;
use cl_core::{CalendarDate, Money, Quantity};
use cl_model::forms::SaveIntent;
use cl_model::forms::invoice::{InvoiceLine, SalesDocKind, TotalsView};

use eframe::egui;

use super::*;
use crate::company::{CompanyEvent, CompanyLink};

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn screen(kind: SalesDocKind) -> (tempfile::TempDir, InvoiceScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = InvoiceScreen::new_kind(link, None, today(), kind);
    (dir, s)
}

fn bluestem(s: &InvoiceScreen) -> ContactId {
    s.books
        .contact(crate::company::demo::credit_memos::BLUESTEM)
        .expect("Bluestem")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn settle(s: &mut InvoiceScreen) {
    s.poll_preview();
    s.clock = s
        .clock
        .checked_sub(Duration::from_millis(400))
        .unwrap_or(s.clock);
    s.poll_preview();
}

/// Golden case 1's lines: 2 x 150.00 taxable, 1 x 75.00 not (the demo
/// company's own 8.1 % rate: the form never sends a rate, Q-P20a).
fn golden_lines(s: &mut InvoiceScreen, client: ContactId) {
    s.pick_client(client);
    let income = crate::company::demo::account_id(4000);
    let mut lines = Vec::new();
    for (qty, rate, tax) in [("2", "150.00", true), ("1", "75.00", false)] {
        let key = s.grid.keys().next_key();
        lines.push(InvoiceLine {
            account: Some(income),
            description: "Boarding refund".into(),
            quantity: Some(Quantity::parse_wire(qty).expect("q")),
            rate: Some(m(rate)),
            taxable: Some(tax),
            ..InvoiceLine::blank(key)
        });
    }
    s.grid.edit(|v, keys| {
        v.lines = lines;
        v.ensure_entry_line(keys);
    });
    settle(s);
}

fn client_balance(s: &InvoiceScreen, c: ContactId) -> String {
    let d = s.link.demo_state().expect("demo");
    crate::company::demo::receivables::contact_balance(&d, c)
        .round_dp(2)
        .to_string()
}

#[test]
fn credit_memo_totals_come_from_the_credit_memo_preview() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    let TotalsView::Ready(t) = s.tracker.totals() else {
        panic!("answered: {:?}", s.tracker.totals());
    };
    assert_eq!(t.subtotal, m("375.00"));
    assert_eq!(t.tax_total, m("24.30"));
    assert_eq!(t.total, m("399.30"));
    let v = s.state_json();
    assert_eq!(v["screen"], "credit_memo");
    assert_eq!(v["credit"]["open_invoices"], 2);
    assert_eq!(
        v["credit"]["suggested"],
        serde_json::json!(["250.00", "149.30"])
    );
    assert_eq!(s.title(), t_title(CREDIT_MEMO_KIND));
}

fn t_title(kind: &str) -> String {
    crate::i18n::t(&format!("cmd.{kind}"))
}

#[test]
fn saving_a_memo_with_open_invoices_asks_apply_or_keep_first() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    let writes = s.link.demo_state().expect("demo").credits.writes;
    s.start_save(SaveIntent::Save, None);
    assert!(s.apply_open.is_some(), "the panel opened");
    assert!(s.editing.is_none(), "nothing saved yet");
    assert_eq!(s.link.demo_state().expect("demo").credits.writes, writes);
    // Apply (the engine's oldest-first suggestion) saves in the same call.
    s.submit_apply_panel(false);
    let memo = s
        .editing
        .as_ref()
        .and_then(Editing::memo)
        .cloned()
        .expect("saved memo");
    assert_eq!(memo.total, m("399.30"));
    assert_eq!(memo.remaining, m("0.00"));
    let applied: Vec<Money> = memo.live_applications().map(|a| a.amount).collect();
    assert_eq!(applied, vec![m("250.00"), m("149.30")]);
    assert_eq!(client_balance(&s, c), "350.70");
    assert!(s.apply_open.is_none());
    assert_eq!(s.state_json()["credit"]["saved_remaining"], "0.00");
}

#[test]
fn keep_as_credit_saves_the_whole_memo_as_credit() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    s.start_save(SaveIntent::Save, None);
    s.submit_apply_panel(true);
    let memo = s
        .editing
        .as_ref()
        .and_then(Editing::memo)
        .cloned()
        .expect("saved");
    assert_eq!(memo.remaining, m("399.30"));
    assert_eq!(memo.live_applications().count(), 0);
    assert_eq!(client_balance(&s, c), "350.70");
    // Refund opens Refund a Credit with this memo picked.
    let _ = s.link.drain();
    s.form_command(CommandId::new("credit_memo.refund"));
    let events = s.link.drain();
    let opened = events.iter().any(|e| {
        matches!(e, CompanyEvent::OpenWith(k, p)
            if k.as_str() == crate::screens::refund::KIND
                && matches!(**p, crate::windows::OpenPayload::RefundFrom(cl, src)
                    if cl == c && src.id == memo.id && src.remaining == m("399.30")))
    });
    assert!(opened, "{events:?}");
}

#[test]
fn back_asks_again_and_a_typed_amount_is_the_owners() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    s.start_save(SaveIntent::Save, None);
    s.close_apply_panel();
    assert!(s.apply_open.is_none());
    s.start_save(SaveIntent::Save, None);
    assert!(s.apply_open.is_some(), "asked again");
    // Only 100.00 on invoice A; B unticked.
    let a = crate::company::demo::credit_memos::seed_invoice(1201).expect("A");
    let b = crate::company::demo::credit_memos::seed_invoice(1202).expect("B");
    s.toggle_apply_row(b, false);
    s.apply_cells.insert(a, "100.00".into());
    s.submit_apply_panel(false);
    let memo = s
        .editing
        .as_ref()
        .and_then(Editing::memo)
        .cloned()
        .expect("saved");
    assert_eq!(memo.remaining, m("299.30"));
    assert_eq!(
        memo.live_applications()
            .map(|x| x.amount)
            .collect::<Vec<_>>(),
        vec![m("100.00")]
    );
}

#[test]
fn a_saved_memo_reads_back_navigates_and_voids() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    s.start_save(SaveIntent::Save, None);
    s.submit_apply_panel(true);
    let id = s.editing.as_ref().map(|e| e.id).expect("saved");
    s.new_form(false);
    s.navigate(true);
    assert_eq!(s.editing.as_ref().map(|e| e.id), Some(id), "newest first");
    assert!(!s.is_dirty());
    s.void_reason = "Entered twice".into();
    s.send_void(None);
    assert!(s.editing.as_ref().is_some_and(Editing::void));
    assert_eq!(client_balance(&s, c), "750.00");
}

/// Review D-B1: a saved memo opened again (Prev/Next, Find, a list) and
/// edited saves without the apply panel and sends no applications (the
/// demo refuses them on an edit, as the engine does); Apply credit is
/// pointed to.
#[test]
fn editing_a_saved_memo_never_asks_or_sends_applications() {
    let (_d, mut s) = screen(SalesDocKind::CreditMemo);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    s.start_save(SaveIntent::Save, None);
    s.submit_apply_panel(true);
    let id = s.editing.as_ref().map(|e| e.id).expect("saved");
    s.new_form(false);
    // Opened as from Find in a fresh window: no client figures yet.
    s.sales = None;
    s.navigate(true);
    assert_eq!(s.editing.as_ref().map(|e| e.id), Some(id));
    settle(&mut s);
    // Change a line: 1 x 75.00 becomes 1 x 80.00.
    s.grid.edit(|v, _| {
        if let Some(l) = v.lines.iter_mut().find(|l| l.rate == Some(m("75.00"))) {
            l.rate = Some(m("80.00"));
        }
    });
    settle(&mut s);
    assert!(s.is_dirty());
    s.start_save(SaveIntent::Save, None);
    assert!(s.apply_open.is_none(), "no apply panel on an edit");
    assert!(s.error.is_none(), "{:?}", s.error);
    let memo = s
        .editing
        .as_ref()
        .and_then(Editing::memo)
        .cloned()
        .expect("saved");
    assert_eq!(memo.id, id);
    assert_eq!(memo.live_applications().count(), 0, "nothing applied");
    assert!(memo.remaining.is_positive());
    assert_eq!(s.notice, Some("credit_memo.saved.apply_hint"));
    assert!(!s.is_dirty());
}

#[test]
fn a_sales_receipt_saves_to_payments_clearing_by_default() {
    let (_d, mut s) = screen(SalesDocKind::SalesReceipt);
    let c = bluestem(&s);
    golden_lines(&mut s, c);
    assert_eq!(s.state_json()["screen"], "sales_receipt");
    assert_eq!(s.state_json()["method"], "check");
    s.start_save(SaveIntent::Save, None);
    let r = s
        .editing
        .as_ref()
        .and_then(Editing::receipt)
        .cloned()
        .expect("saved");
    assert_eq!(r.total, m("399.30"));
    let clearing = {
        let mut d = s.link.demo_state().expect("demo");
        crate::company::demo::receivables::clearing(&mut d)
    };
    assert_eq!(r.deposit_to_account_id, clearing);
    // No apply question for a receipt; the client's balance is untouched.
    assert!(s.apply_open.is_none());
    assert_eq!(client_balance(&s, c), "750.00");
}

#[test]
fn a_negative_receipt_to_payments_clearing_needs_a_bank() {
    let (_d, mut s) = screen(SalesDocKind::SalesReceipt);
    let c = bluestem(&s);
    s.pick_client(c);
    let income = crate::company::demo::account_id(4000);
    let key = s.grid.keys().next_key();
    let line = InvoiceLine {
        account: Some(income),
        description: "Return".into(),
        quantity: Some(Quantity::parse_wire("1").expect("q")),
        rate: Some(m("-20.00")),
        taxable: Some(false),
        ..InvoiceLine::blank(key)
    };
    s.grid.edit(|v, keys| {
        v.lines = vec![line];
        v.ensure_entry_line(keys);
    });
    settle(&mut s);
    s.start_save(SaveIntent::Save, None);
    assert!(s.editing.is_none(), "refused before sending");
    assert!(
        s.problems
            .contains(&cl_model::forms::invoice::InvoiceProblem::NegativeReceiptNeedsBank)
    );
}

#[test]
fn the_three_windows_close_their_own_kind() {
    for (kind, cmd) in [
        (SalesDocKind::Invoice, KIND),
        (SalesDocKind::CreditMemo, CREDIT_MEMO_KIND),
        (SalesDocKind::SalesReceipt, SALES_RECEIPT_KIND),
    ] {
        let (_d, mut s) = screen(kind);
        let _ = s.link.drain();
        s.follow_up(SaveIntent::SaveAndClose);
        let events = s.link.drain();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, CompanyEvent::CloseWindow(k) if k.as_str() == cmd)),
            "{cmd}: {events:?}"
        );
        assert!(
            s.command(CommandId::new(cmd)),
            "{cmd} takes its own command"
        );
    }
}

#[test]
fn the_new_kinds_name_every_widget() {
    for kind in [SalesDocKind::CreditMemo, SalesDocKind::SalesReceipt] {
        let (_d, mut s) = screen(kind);
        let c = bluestem(&s);
        golden_lines(&mut s, c);
        if kind == SalesDocKind::CreditMemo {
            s.start_save(SaveIntent::Save, None);
        }
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
        assert!(unnamed.is_empty(), "{kind:?}: {unnamed:?}");
    }
}

/// Q-P20h: a new sales receipt starts on the last Deposit to picked in
/// this run, without making the blank form dirty.
#[test]
fn a_new_receipt_remembers_the_last_deposit_to() {
    let (_d, mut s) = screen(SalesDocKind::SalesReceipt);
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    let frame = |s: &mut InvoiceScreen| {
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
    };
    frame(&mut s);
    let bank = s
        .books
        .accounts
        .iter()
        .find(|a| a.is_active && a.fields.detail_type.as_deref() == Some("bank"))
        .map(|a| a.id)
        .expect("a bank");
    ctx.data_mut(|d| d.insert_temp(super::credit::last_deposit_id(), Some(bank)));
    frame(&mut s);
    assert_eq!(s.header.current().deposit_to, Some(bank));
    assert!(!s.is_dirty(), "a remembered default is not an edit");
}
