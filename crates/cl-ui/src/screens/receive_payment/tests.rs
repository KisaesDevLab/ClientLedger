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
