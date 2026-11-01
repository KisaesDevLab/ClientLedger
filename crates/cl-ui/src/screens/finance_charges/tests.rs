//! Assess Finance Charges against the demo engine: the setup first with a
//! blank rate (Q-P22a), golden case 6's rows (Q-P22g; open credits
//! unticked, Q-P22s), assess once per month (Q-P22p), a charge that moved
//! since the preview (409 CHANGED), the closing-date prompt, Client
//! Center's single client and AccessKit names.

use std::sync::Arc;

use eframe::egui;

use super::*;
use crate::company::CompanyLink;
use crate::company::demo::credit_memos::{CEDAR_GAP, LOW_WATER, PRAIRIE_FORK, client_named};

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn oct31() -> CalendarDate {
    CalendarDate::from_ymd(2026, 10, 31).expect("date")
}

fn settle(s: &mut FinanceChargesScreen) {
    for _ in 0..50 {
        s.poll();
        if !s.busy() && s.preview_job.is_none() && s.settings_job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn client(link: &Arc<CompanyLink>, name: &str) -> ContactId {
    let mut d = link.demo_state().expect("demo");
    crate::company::demo::credit_memos::seed(&mut d);
    client_named(&mut d, name).expect("client")
}

fn screen(link: &Arc<CompanyLink>) -> FinanceChargesScreen {
    let mut s = FinanceChargesScreen::new(Arc::clone(link), oct31());
    settle(&mut s);
    s
}

/// Golden 6's settings through the setup fields.
fn set_up(s: &mut FinanceChargesScreen) {
    let income = s.incomes().first().map(|(id, _)| *id).expect("an income");
    let e = s.setup.as_mut().expect("setup shown");
    e.rate = "18".into();
    e.minimum = "2.00".into();
    e.grace_days = "10".into();
    e.income_account = Some(income);
    assert!(s.save_setup());
    settle(s);
}

fn row(s: &FinanceChargesScreen, c: ContactId) -> &cl_api::FinanceChargeRow {
    s.current()
        .expect("preview")
        .rows
        .iter()
        .find(|r| r.contact_id == c)
        .expect("row")
}

#[test]
fn a_blank_rate_asks_for_the_setup_first() {
    let (_d, link) = link();
    let mut s = screen(&link);
    assert!(s.setup.is_some(), "Q-P22a: the setup first");
    assert_eq!(s.state()["not_set_up"], true);
    assert!(!s.start_assess(None));
    assert_eq!(s.problems, vec![FinanceChargeProblem::NotSetUp]);
    // The setup asks for the rate, then the account.
    assert!(!s.save_setup());
    assert_eq!(s.setup_problem, Some("finance_charges.setup.needs_rate"));
    s.setup.as_mut().expect("setup").rate = "18".into();
    assert!(!s.save_setup());
    assert_eq!(s.setup_problem, Some("finance_charges.setup.needs_account"));
    s.setup.as_mut().expect("setup").rate = "abc".into();
    assert!(!s.save_setup());
    assert_eq!(s.setup_problem, Some("finance_charges.setup.bad_rate"));
    // Cancel can't leave the setup while the rate is blank.
    s.cancel_setup();
    assert!(s.setup.is_some());
}

#[test]
fn golden_6_rows_assess_once_per_month() {
    let (_d, link) = link();
    let pf = client(&link, PRAIRIE_FORK);
    let lw = client(&link, LOW_WATER);
    let cg = client(&link, CEDAR_GAP);
    let mut s = screen(&link);
    set_up(&mut s);
    assert!(s.setup.is_none());
    assert_eq!(s.state()["period"], "2026-10");
    // The settings went to the demo engine.
    assert!(link.demo_state().expect("demo").finance.rate.is_some());
    assert_eq!(row(&s, pf).charge.format(MoneyStyle::default()), "15.68");
    assert_eq!(row(&s, lw).charge.format(MoneyStyle::default()), "2.00");
    assert_eq!(row(&s, cg).charge.format(MoneyStyle::default()), "9.17");
    assert!(s.form.is_ticked(pf) && s.form.is_ticked(lw));
    assert!(!s.form.is_ticked(cg), "Q-P22s: open credits start unticked");
    assert_eq!(
        row(&s, cg)
            .has_open_credits
            .map(|m| m.format(MoneyStyle::default())),
        Some("400.00".into())
    );
    // Only Prairie Fork and Low Water; Cedar Gap stays unticked.
    let others: Vec<ContactId> = s
        .current()
        .expect("preview")
        .rows
        .iter()
        .map(|r| r.contact_id)
        .filter(|c| ![pf, lw].contains(c) && s.form.is_ticked(*c))
        .collect();
    for c in others {
        s.toggle(c);
    }
    assert_eq!(s.ticked_count(), 2);
    assert!(s.start_assess(None));
    settle(&mut s);
    assert_eq!(s.notice, Some("finance_charges.done"));
    let totals: Vec<String> = s
        .results
        .iter()
        .map(|r| r.total.format(MoneyStyle::default()))
        .collect();
    assert_eq!(totals, vec!["15.68".to_string(), "2.00".to_string()]);
    assert_eq!(s.state()["print_later"], true);
    // The month is charged: the rows say so and can't be ticked.
    assert!(row(&s, pf).already_assessed.is_some());
    assert!(!s.form.is_ticked(pf));
    s.toggle(pf);
    assert!(!s.form.is_ticked(pf));
    // Open goes to the invoice form.
    let first = s.results[0].clone();
    s.open_invoice(&first);
    let events = link.drain();
    assert!(events.iter().any(|e| matches!(
        e,
        CompanyEvent::OpenRecord(k, id)
            if k.as_str() == crate::screens::invoice::KIND && *id == first.invoice_id
    )));
}

/// A finance-charge invoice can be edited like any other (P22 plan §2.7:
/// the engine never refuses it); the month stays assessed, so assessing
/// it again is still 409 ASSESSED.
#[test]
fn a_finance_charge_invoice_can_be_edited_and_the_month_stays_assessed() {
    let (_d, link) = link();
    let pf = client(&link, PRAIRIE_FORK);
    let mut s = screen(&link);
    set_up(&mut s);
    let others: Vec<ContactId> = s
        .current()
        .expect("preview")
        .rows
        .iter()
        .map(|r| r.contact_id)
        .filter(|c| *c != pf && s.form.is_ticked(*c))
        .collect();
    for c in others {
        s.toggle(c);
    }
    assert!(s.start_assess(None));
    settle(&mut s);
    let id = s.results[0].invoice_id;
    let mut inv = crate::screens::invoice::InvoiceScreen::new_kind(
        Arc::clone(&link),
        None,
        oct31(),
        cl_model::forms::invoice::SalesDocKind::Invoice,
    );
    inv.load(id);
    inv.poll();
    assert_eq!(inv.editing.as_ref().map(|e| e.id), Some(id));
    inv.grid.edit(|v, _| {
        if let Some(l) = v.lines.iter_mut().find(|l| l.rate.is_some()) {
            l.rate = Some(Money::parse_wire("99.00").expect("money"));
        }
    });
    inv.poll_preview();
    inv.clock = inv
        .clock
        .checked_sub(std::time::Duration::from_millis(400))
        .unwrap_or(inv.clock);
    inv.poll_preview();
    let before = crate::company::demo::invoices::get(&link.demo_state().expect("demo"), id)
        .expect("stored")
        .record
        .balance_due;
    inv.start_save(cl_model::forms::SaveIntent::Save, None);
    for _ in 0..50 {
        inv.poll();
        if inv.save_job.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_ne!(inv.notice, Some("invoice.finance_charge_invoice"));
    assert!(inv.error.is_none(), "{:?}", inv.error);
    let after = crate::company::demo::invoices::get(&link.demo_state().expect("demo"), id)
        .expect("stored")
        .record
        .balance_due;
    assert_ne!(after, before, "the edit saved");
    // The month stays assessed: a second assessment is refused.
    let again = screen(&link);
    assert!(again.current().is_some_and(|p| {
        p.rows
            .iter()
            .any(|r| r.contact_id == pf && r.already_assessed.is_some())
    }));
    assert!(!again.form.is_ticked(pf));
}

#[test]
fn a_charge_that_moved_previews_again_and_names_the_client() {
    let (_d, link) = link();
    let pf = client(&link, PRAIRIE_FORK);
    let mut s = screen(&link);
    set_up(&mut s);
    // Only Prairie Fork, ticked.
    let ids: Vec<ContactId> = s
        .current()
        .expect("preview")
        .rows
        .iter()
        .map(|r| r.contact_id)
        .collect();
    for c in ids {
        if (c == pf) != s.form.is_ticked(c) {
            s.toggle(c);
        }
    }
    // The rate changes behind the preview's back.
    crate::company::demo::finance_charges::set_settings(
        &mut link.demo_state().expect("demo"),
        &cl_api::FinanceChargeSettingsUpdate {
            annual_rate: Some(Some(cl_api::AnnualRate::parse("24").expect("rate"))),
            ..cl_api::FinanceChargeSettingsUpdate::default()
        },
    );
    assert!(s.start_assess(None));
    // The engine names the client (`details.contactId`).
    assert_eq!(s.changed_named, Some(pf));
    settle(&mut s);
    assert_eq!(s.notice, Some("finance_charges.refusal.changed"));
    assert_eq!(s.results.len(), 0);
    assert_eq!(s.changed_names, vec![PRAIRIE_FORK.to_string()]);
    assert_ne!(
        row(&s, pf).charge.format(MoneyStyle::default()),
        "15.68",
        "the new figure"
    );
    // Assess again with the new figures: one invoice.
    let keep: Vec<ContactId> = s
        .current()
        .expect("preview")
        .rows
        .iter()
        .map(|r| r.contact_id)
        .filter(|c| *c != pf && s.form.is_ticked(*c))
        .collect();
    for c in keep {
        s.toggle(c);
    }
    let charge = row(&s, pf).charge;
    assert!(s.start_assess(None));
    settle(&mut s);
    assert_eq!(s.results.len(), 1);
    assert_eq!(s.results[0].total, charge);
}

#[test]
fn a_closed_date_asks_for_the_closing_password() {
    let (_d, link) = link();
    {
        // Books closed through the assessment date.
        let mut d = link.demo_state().expect("demo");
        let open = d.open.expect("a demo company");
        for c in d.companies.iter_mut().filter(|c| c.tenant_id == open) {
            c.closed_through = Some(oct31());
        }
    }
    let mut s = screen(&link);
    set_up(&mut s);
    assert!(s.ticked_count() > 0);
    assert!(!s.start_assess(None));
    assert!(s.closing_prompt.dialog_open());
    assert!(s.assess_job.is_none(), "nothing sent before the password");
}

/// A date in a later month is the engine's 400 on the preview (P22 fix
/// pass), shown in plain words; nothing can be assessed. Back in this
/// month, the preview returns and the notice goes.
#[test]
fn a_future_assessment_date_is_refused_in_plain_words() {
    let (_d, link) = link();
    let mut s = screen(&link);
    set_up(&mut s);
    assert!(s.ticked_count() > 0);
    s.set_date(Some(CalendarDate::from_ymd(2026, 12, 1).expect("date")));
    settle(&mut s);
    assert_eq!(s.notice, Some("finance_charges.refusal.future_date"));
    assert!(s.error.is_none(), "a plain notice, not an error");
    assert_eq!(s.ticked_count(), 0);
    assert!(!s.start_assess(None), "nothing to assess");
    assert_eq!(s.results.len(), 0);
    s.set_date(Some(CalendarDate::from_ymd(2026, 11, 30).expect("date")));
    settle(&mut s);
    assert_eq!(s.notice, None);
    assert!(s.ticked_count() > 0);
}

/// DESIGN.md form keys: Alt+N assesses and stays; Alt+S assesses and
/// closes once posted.
#[test]
fn alt_n_assesses_and_stays_alt_s_assesses_and_closes() {
    let closes = |link: &Arc<CompanyLink>| {
        link.drain()
            .iter()
            .any(|e| matches!(e, CompanyEvent::CloseWindow(k) if k.as_str() == KIND))
    };
    let (_d, link) = link();
    let pf = client(&link, PRAIRIE_FORK);
    let lw = client(&link, LOW_WATER);
    let mut s = screen(&link);
    set_up(&mut s);
    let only = |s: &mut FinanceChargesScreen, keep: ContactId| {
        let ids: Vec<ContactId> = s
            .current()
            .expect("preview")
            .rows
            .iter()
            .map(|r| r.contact_id)
            .collect();
        for c in ids {
            if (c == keep) != s.form.is_ticked(c) {
                s.toggle(c);
            }
        }
    };
    only(&mut s, pf);
    let _ = link.drain();
    assert!(s.command(CommandId::new("form.save_new")));
    settle(&mut s);
    assert_eq!(s.results.len(), 1);
    assert!(!closes(&link), "Alt+N stays");
    only(&mut s, lw);
    assert!(s.command(CommandId::new("form.save_close")));
    settle(&mut s);
    assert_eq!(s.results.len(), 2);
    assert!(closes(&link), "Alt+S closes once posted");
}

#[test]
fn client_center_previews_one_client() {
    let (_d, link) = link();
    let pf = client(&link, PRAIRIE_FORK);
    let mut s = screen(&link);
    set_up(&mut s);
    assert!(s.open_with(OpenPayload::ForClient(pf)));
    settle(&mut s);
    let p = s.current().expect("preview");
    assert_eq!(p.rows.len(), 1);
    assert_eq!(p.rows[0].contact_id, pf);
    assert_eq!(s.state()["only"], true);
    // Statements' note opens it on its date.
    let nov30 = CalendarDate::from_ymd(2026, 11, 30).expect("date");
    assert!(s.open_with(OpenPayload::OnDate(nov30)));
    settle(&mut s);
    assert_eq!(s.state()["date"], "2026-11-30");
    assert_eq!(s.state()["period"], "2026-11");
}

#[test]
fn month_labels_read_as_words() {
    assert_eq!(month_label("2026-10"), "October 2026");
    assert_eq!(month_label("2026-1x"), "2026-1x");
}

#[test]
fn every_widget_is_named_in_both_themes() {
    let (_d, link) = link();
    for theme in [
        crate::theme::ThemeChoice::Light,
        crate::theme::ThemeChoice::Dark,
    ] {
        for setup in [true, false] {
            let mut s = screen(&link);
            if !setup && s.setup.is_some() {
                set_up(&mut s);
                let first = s
                    .current()
                    .and_then(|p| p.rows.iter().find(|r| !r.sources.is_empty()))
                    .map(|r| r.contact_id);
                if let Some(c) = first {
                    s.expand(c);
                }
            }
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
}
