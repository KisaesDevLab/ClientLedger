//! Statements against the demo engine: the batch list, the preview's aging
//! row (= the A/R aging row, Q-177), issuing numbered snapshots (Q-P22n)
//! with Print, Mark as sent and the Outbox, and AccessKit names.
#![allow(clippy::many_single_char_names)]

use std::sync::Arc;

use eframe::egui;

use super::*;
use crate::company::CompanyLink;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn nov30() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 30).expect("date")
}

fn settle(s: &mut StatementsScreen) {
    for _ in 0..50 {
        s.poll();
        if !s.busy() && s.list_job.is_none() && s.preview_job.is_none() && s.finance_job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn bluestem(link: &Arc<CompanyLink>) -> ContactId {
    let mut d = link.demo_state().expect("demo");
    crate::company::demo::credit_memos::seed(&mut d);
    d.contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == crate::company::demo::credit_memos::BLUESTEM)
        .map(|c| c.id)
        .expect("Bluestem")
}

fn screen(link: &Arc<CompanyLink>) -> StatementsScreen {
    let mut s = StatementsScreen::new(Arc::clone(link), None, nov30());
    settle(&mut s);
    s
}

#[test]
fn the_list_shows_clients_with_a_balance_and_the_preview_ages_like_ar_aging() {
    let (_d, link) = link();
    let c = bluestem(&link);
    let mut s = screen(&link);
    assert!(s.loaded);
    assert!(s.rows.iter().any(|r| r.contact_id == c));
    assert!(s.rows.iter().all(|r| !r.amount_due.is_zero()));
    s.show_client(c);
    settle(&mut s);
    let v = s.state();
    assert_eq!(v["preview"]["amount_due"], "750.00");
    assert_eq!(v["preview"]["aging"]["days_1_30"], "500.00");
    assert_eq!(v["preview"]["aging"]["days_31_60"], "250.00");
    // Statement aging = the A/R aging row (Q-177).
    let mut d = link.demo_state().expect("demo");
    let aging = crate::company::demo::statements::ar_aging(&mut d, nov30()).expect("aging");
    let row = aging.row(Some(c)).expect("row");
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(row.buckets, p.aging.buckets);
    assert_eq!(row.total, p.aging.total);
}

#[test]
fn formats_change_the_preview_and_show_the_start() {
    let (_d, link) = link();
    let c = bluestem(&link);
    let mut s = screen(&link);
    s.show_client(c);
    settle(&mut s);
    assert_eq!(s.state()["shows_start"], false);
    s.set_format(StatementFormatChoice::BalanceForward);
    settle(&mut s);
    assert_eq!(s.state()["shows_start"], true);
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.format, StatementFormat::BalanceForward);
    assert_eq!(p.amount_due.format(MoneyStyle::default()), "750.00");
}

#[test]
fn print_issues_numbered_snapshots_and_records_printed() {
    let (_d, link) = link();
    let c = bluestem(&link);
    let mut s = screen(&link);
    s.start(Action::Print);
    assert_eq!(s.state()["problems"][0], "NoClients");
    s.for_client(c);
    s.start(Action::Print);
    settle(&mut s);
    assert_eq!(s.issued_count, 1);
    assert_eq!(s.marked_count, 1);
    assert_eq!(s.last, Some("printed"));
    let id = s.issued[0].id;
    let rec = crate::company::demo::statements::get(&link.demo_state().expect("demo"), id)
        .expect("issued");
    assert_eq!(rec.send_history.len(), 1);
    assert_eq!(rec.send_history[0].method, SendMethod::Printed);
    // A second Print issues a new number (each send is a snapshot).
    let first = s.issued[0].number.clone();
    s.start(Action::Print);
    settle(&mut s);
    assert_ne!(s.issued[0].number, first);
}

#[test]
fn pdf_records_sent_only_when_ticked_and_mark_as_sent_records() {
    let (_d, link) = link();
    let c = bluestem(&link);
    let mut s = screen(&link);
    s.for_client(c);
    s.start(Action::Pdf);
    settle(&mut s);
    assert_eq!(s.last, Some("pdf"));
    assert_eq!(s.marked_count, 0);
    s.start(Action::MarkSent);
    settle(&mut s);
    assert_eq!(s.marked_count, 1);
}

#[test]
fn email_puts_statements_with_an_address_in_the_outbox() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let Some(with) = s.rows.iter().find(|r| r.has_email).map(|r| r.contact_id) else {
        // Every client without email: Email says to print instead.
        s.select_all(true);
        s.start(Action::Email);
        assert_eq!(s.notice, Some("statements.email.nobody"));
        return;
    };
    s.form.select_only(&[with]);
    s.start(Action::Email);
    settle(&mut s);
    assert!(s.email.is_some());
    s.send_email(EmailWay::Outbox);
    settle(&mut s);
    assert_eq!(s.email_outcome.map(|o| o.queued), Some(1));
    let items = crate::company::demo::invoices::outbox(&link.demo_state().expect("demo"));
    assert!(
        items
            .iter()
            .any(|i| i.kind == cl_api::OutboxKind::CustomerStatement)
    );
}

#[test]
fn every_widget_is_named_in_both_themes() {
    let (_d, link) = link();
    let c = bluestem(&link);
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        let mut s = screen(&link);
        s.for_client(c);
        settle(&mut s);
        s.open_email();
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
        let unnamed = crate::screens::unnamed_widgets(&nodes);
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }
}

#[test]
fn the_finance_charge_note_shows_while_charges_are_due() {
    let (_d, link) = link();
    // Blank rate (Q-P22a): no note.
    let mut s = screen(&link);
    assert_eq!(s.state()["finance_due"], false);
    // Set up: as of 11/30 the preview charges someone.
    crate::company::demo::finance_charges::set_settings(
        &mut link.demo_state().expect("demo"),
        &cl_api::FinanceChargeSettingsUpdate {
            annual_rate: Some(Some(cl_api::AnnualRate::parse("18").expect("rate"))),
            minimum: Some(cl_core::Money::parse_wire("2.00").expect("money")),
            grace_days: Some(10),
            income_account_id: Some(Some(crate::company::demo::account_id(4000))),
            ..cl_api::FinanceChargeSettingsUpdate::default()
        },
    );
    let s2 = screen(&link);
    assert_eq!(s2.state()["finance_due"], true);
    assert_eq!(s2.finance_due.as_deref(), Some("2026-11"));
    // Its link opens Assess Finance Charges on the statement date.
    let _ = link.drain();
    s2.open_finance_charges();
    let events = link.drain();
    assert!(events.iter().any(|e| matches!(
        e,
        CompanyEvent::OpenWith(k, p)
            if k.as_str() == crate::screens::finance_charges::KIND
                && matches!(**p, crate::windows::OpenPayload::OnDate(d) if d == nov30())
    )));
    // Another date checks again.
    s.set_as_of(CalendarDate::from_ymd(2026, 12, 31).ok());
    settle(&mut s);
    assert_eq!(s.state()["finance_due"], true);
}
