//! Print Forms on the demo engine: preview, numbers recorded at Print,
//! "Did all checks print correctly?" (all, first bad number, later), forms,
//! Printer Setup.

use super::*;

fn screen(setup: bool) -> (tempfile::TempDir, PrintFormsScreen) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let tenant = link.demo_state().and_then(|mut d| {
        let c = d.companies.first().cloned()?;
        d.open = Some(c.tenant_id);
        Some((c.tenant_id, c.name))
    });
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    let mut s = PrintFormsScreen::new(link, tenant, today, setup);
    s.poll();
    s.poll();
    (dir, s)
}

#[test]
fn checks_preview_then_print_records_numbers_and_all_printed_clears_them() {
    let (_d, mut s) = screen(false);
    assert!(s.loaded);
    assert_eq!(s.checks.len(), 2);
    assert_eq!(s.picked.len(), 2);
    let first: u64 = s.first_number.parse().expect("the next number");
    s.preview_checks();
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.kind, PreviewKind::Checks);
    assert_eq!(p.doc.pages.len(), 2, "voucher: one check per page");
    let calls = |s: &PrintFormsScreen| s.link.demo_state().map_or(0, |d| d.printing.print_calls);
    assert_eq!(calls(&s), 0, "nothing recorded before Print");
    s.print_clicked();
    s.poll();
    assert_eq!(calls(&s), 1);
    assert!(s.asking);
    assert_eq!(s.sent, Some(Sent::Printed));
    assert!(s.has_unanswered(), "saved until answered");
    s.answer(true);
    assert!(!s.asking);
    assert!(!s.has_unanswered());
    s.poll();
    assert!(s.checks.is_empty(), "printed checks leave the queue");
    assert_eq!(s.first_number, (first + 2).to_string());
}

/// B1 (Q-94): a first number already used in the register is refused in
/// plain words before any page is laid out, and the next free one offered.
#[test]
fn a_used_first_number_warns_and_offers_the_next_free_one() {
    let (_d, mut s) = screen(false);
    let next: u64 = s.first_number.parse().expect("the next number");
    assert_eq!(next, 4418, "the register's next check number");
    s.first_number = "4417".into();
    s.preview_checks();
    s.poll();
    assert!(s.preview.is_none());
    assert_eq!(s.problem, Some(PrintProblem::NumberInUse));
    assert_eq!(s.offer, Some(4418));
    s.take_offer();
    assert_eq!(s.first_number, "4418");
    assert_eq!(s.problem, None);
    s.preview_checks();
    s.poll();
    assert!(s.preview.is_some());
    // Esc goes back to the list.
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.preview.is_none());
}

#[test]
fn a_first_bad_number_sends_it_and_later_checks_back() {
    let (_d, mut s) = screen(false);
    let second = s.first_number.parse::<u64>().expect("next") + 1;
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    s.first_bad = second.to_string();
    s.answer(false);
    s.poll();
    assert!(!s.asking);
    assert_eq!(s.notice, Some("print.confirm.requeued"));
    assert_eq!(s.checks.len(), 1, "1002 waits to print again");
    let spoiled = s
        .link
        .demo_state()
        .map(|d| crate::company::demo::printing::spoiled(&d))
        .unwrap_or_default();
    assert_eq!(spoiled[0].number, second);
    assert!(!s.has_unanswered());
}

#[test]
fn decide_later_keeps_the_reminder_and_it_reopens() {
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    let mut bus = s.link.bus().subscribe();
    s.later();
    assert!(s.link.bus().take(&mut bus).printing, "Home hears of it");
    assert!(s.has_unanswered());
    let tenant = s.tenant.as_ref().map(|(t, _)| *t).expect("tenant");
    assert!(waiting_answer(&s.link, tenant));
    s.reopen_question();
    assert!(s.asking);
    s.answer(true);
    assert!(!s.has_unanswered());
    assert!(s.link.bus().take(&mut bus).printing);
    assert!(!waiting_answer(&s.link, tenant));
}

#[test]
fn a_used_first_number_is_refused_and_the_preview_stays() {
    let (_d, mut s) = screen(false);
    let used = s.first_number.clone();
    let first = s.checks[0].id;
    s.picked = [first].into_iter().collect();
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    s.answer(true);
    s.poll();
    // The remaining check, from a number already used: refused before the
    // preview, the next free number offered.
    s.first_number.clone_from(&used);
    s.preview_checks();
    s.poll();
    assert_eq!(s.problem, Some(PrintProblem::NumberInUse));
    let after: u64 = used.parse::<u64>().expect("n") + 1;
    assert_eq!(s.offer, Some(after));
    assert!(s.preview.is_none());
    // As against the engine (used numbers unknown): the 409 at Print.
    s.used.clear();
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    assert_eq!(s.problem, Some(PrintProblem::NumberInUse));
    assert_eq!(s.offer, Some(after));
    assert!(!s.asking);
    assert!(s.preview.is_some());
    // N3: "Use next N" renumbers the run and the preview, and Print sends N.
    let remaining = s.checks[0].id;
    s.take_offer();
    s.poll();
    assert_eq!(s.first_number, after.to_string());
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items()[0].number, Some(after), "the run follows");
    assert!(s.preview.is_some(), "the preview is laid out again");
    s.print_clicked();
    s.poll();
    assert!(s.asking);
    let printed = s
        .link
        .demo_state()
        .and_then(|d| crate::company::demo::printing::printed_number(&d, remaining));
    assert_eq!(printed, Some(after), "Print sent the offered number");
}

/// N3: the offer after a used number is the engine's next when it is past
/// the typed one, else the next number on.
#[test]
fn the_offer_moves_past_the_used_number() {
    assert_eq!(offer_after(Some(4420), 4418), 4420);
    assert_eq!(offer_after(Some(4418), 4418), 4419);
    assert_eq!(offer_after(Some(4000), 4418), 4419);
    assert_eq!(offer_after(None, 4418), 4419);
}

#[test]
fn forms_print_with_copies_and_leave_the_queue() {
    let (_d, mut s) = screen(false);
    s.tab = Tab::Forms;
    s.load_forms();
    s.poll();
    assert_eq!(s.forms.len(), 3);
    s.preview_forms();
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.kind, PreviewKind::Forms);
    assert_eq!(p.doc.pages.len(), 3);
    s.preview_envelopes();
    assert_eq!(
        s.preview.as_ref().map(|p| p.kind),
        Some(PreviewKind::Envelopes)
    );
    s.preview_forms();
    s.poll();
    s.copies = "2".into();
    s.print_clicked();
    s.poll();
    s.poll();
    assert_eq!(s.forms.len(), 0);
}

#[test]
fn printer_setup_saves_the_offset_per_printer_and_the_style_per_account() {
    let (_d, mut s) = screen(true);
    assert!(s.setup_open);
    assert_eq!(s.printer.as_deref(), Some("Office Laser"));
    s.offset_x = "1.5".into();
    s.offset_y = "-0.5".into();
    s.setup_style = CheckFormat::Standard;
    s.setup_block = true;
    s.save_setup();
    s.poll();
    s.poll();
    assert_eq!(s.notice, Some("print.setup.saved"));
    assert_eq!(s.layout.format, CheckFormat::Standard);
    assert!((s.prefs.offset_for("Office Laser").offset_x_mm - 1.5).abs() < 1e-6);
    s.offset_x = "12".into();
    s.save_setup();
    assert_eq!(s.notice, Some("print.setup.bad_offset"));
    s.preview_alignment();
    assert_eq!(
        s.preview.as_ref().map(|p| p.kind),
        Some(PreviewKind::Alignment)
    );
    // Standard stock: both checks on one page.
    s.preview = None;
    s.preview_checks();
    s.poll();
    assert_eq!(s.preview.as_ref().map(|p| p.doc.pages.len()), Some(1));
}

#[test]
fn opening_with_checks_previews_them() {
    let (_d, mut s) = screen(false);
    let id = s.checks[1].id;
    assert!(s.open_with(OpenPayload::PrintChecks(vec![id])));
    s.poll();
    assert_eq!(s.picked.len(), 1);
    assert!(s.preview.is_some());
    assert!(!s.state().is_null());
}

#[test]
fn reprint_check_requeues_it_and_previews_with_the_next_number() {
    let (_d, mut s) = screen(false);
    let first: u64 = s.first_number.parse().expect("next");
    let id = s.checks[0].id;
    let date = s.checks[0].date;
    s.picked = [id].into_iter().collect();
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    s.answer(true);
    s.poll();
    assert!(s.open_with(OpenPayload::ReprintCheck(id, date)));
    s.poll();
    let spoiled = s
        .link
        .demo_state()
        .map(|d| crate::company::demo::printing::spoiled(&d))
        .unwrap_or_default();
    assert_eq!(spoiled[0].number, first, "the old number is logged");
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items()[0].id, id);
    assert_eq!(run.items()[0].number, Some(first + 1), "the next number");
    assert!(s.preview.is_some());
}

#[test]
fn a_deposit_summary_previews_with_its_rows_and_prints() {
    use cl_model::forms::deposit::{DepositSummary, SummaryRow};
    let (_d, mut s) = screen(false);
    let m = |v: &str| Money::parse_wire(v).expect("money");
    let row = |label: &str, amount: &str| SummaryRow {
        label: label.into(),
        detail: String::new(),
        check_no: if amount.starts_with('-') {
            String::new()
        } else {
            "5521".into()
        },
        method: if amount.starts_with('-') {
            String::new()
        } else {
            "Check".into()
        },
        amount: m(amount),
    };
    let summary = DepositSummary {
        date: CalendarDate::from_ymd(2026, 11, 4).expect("date"),
        account: "Operating Checking".into(),
        memo: String::new(),
        items: vec![row("Pat Doe", "300.00")],
        other: vec![row("Bank fees", "-12.50")],
        cash_back: Some(m("50.00")),
        total: m("237.50"),
    };
    let id = s.checks.first().map(|c| c.id).expect("an id");
    assert!(s.open_with(OpenPayload::PrintDeposit(id, Box::new(summary))));
    s.poll();
    let p = s.preview.as_ref().expect("preview");
    assert_eq!(p.kind, PreviewKind::Deposit);
    let page = &cl_print::resolve(&p.doc).expect("pages")[0];
    let text = |tag: &str| page.runs_of(tag).next().map(|r| r.text.clone());
    assert_eq!(text("title").as_deref(), Some("Deposit Summary"));
    assert_eq!(text("row1.amount").as_deref(), Some("-12.50"));
    assert_eq!(text("row0.check_no").as_deref(), Some("5521"));
    assert_eq!(text("row0.payment_method").as_deref(), Some("Check"));
    assert_eq!(text("total0.value").as_deref(), Some("287.50"), "subtotal");
    assert_eq!(text("total2.value").as_deref(), Some("237.50"));
    assert_eq!(s.state()["preview"]["kind"], "deposit");
    s.print_clicked();
    s.poll();
    assert_eq!(s.sent, Some(Sent::Printed));
}

/// The interactive widgets one screen draws that have no accessible name.
fn unnamed_of(s: &mut dyn Screen) -> Vec<String> {
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        let mut out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1440.0, 940.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| s.ui(ui),
        );
        out.textures_delta.clear();
        nodes.extend(
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default(),
        );
    }
    assert!(
        nodes
            .iter()
            .any(|(_, n)| n.role() == egui::accesskit::Role::Button),
        "drawn"
    );
    crate::screens::unnamed_widgets(&nodes)
}

/// Every widget of the Print Forms queues, the preview, Printer Setup and
/// Company Information has an accessible name (screen readers and the
/// control channel).
#[test]
fn print_and_company_info_widgets_have_accessible_names() {
    let (_d, mut s) = screen(false);
    assert_ne!(s.checks.len(), 0);
    let u = unnamed_of(&mut s);
    assert!(u.is_empty(), "checks queue: {u:?}");
    s.tab = Tab::Forms;
    s.load_forms();
    s.poll();
    assert_ne!(s.forms.len(), 0);
    let u = unnamed_of(&mut s);
    assert!(u.is_empty(), "forms queue: {u:?}");
    let (_d2, mut setup) = screen(true);
    let u = unnamed_of(&mut setup);
    assert!(u.is_empty(), "printer setup: {u:?}");
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let tenant = link
        .demo_state()
        .and_then(|d| d.companies.first().map(|c| (c.tenant_id, c.name.clone())));
    let mut info = crate::screens::company_info::CompanyInfoScreen::new(link, tenant);
    let u = unnamed_of(&mut info);
    assert!(u.is_empty(), "company information: {u:?}");
}

/// PR-2 S1: a $0.00 check is never ticked, and a forced pick never prints.
#[test]
fn zero_checks_stay_unticked_and_never_print() {
    let (_d, mut s) = screen(false);
    let mut zero = s.checks[0].clone();
    zero.id = TransactionId::from_uuid(uuid::Uuid::from_u128(
        0x5019_0000_0000_4000_8000_0000_0000_00ff,
    ));
    zero.amount = Money::ZERO;
    s.checks.push(zero.clone());
    assert!(!printable(&zero));
    let picks = default_picks(&s.checks, &[]);
    assert_eq!(picks.len(), 2);
    assert!(!picks.contains(&zero.id));
    assert!(default_picks(&s.checks, &[zero.id]).is_empty());
    s.picked.insert(zero.id);
    s.preview_checks();
    s.poll();
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items().len(), 2, "the $0.00 check is left out");
    assert!(run.items().iter().all(|i| i.id != zero.id));
}

/// PR-2 S2: a requeue whose answer skips a void check still answers it:
/// the saved batch is resolved and Home's reminder clears.
#[test]
fn skipped_checks_resolve_the_saved_batch() {
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    assert!(s.has_unanswered());
    let ids: Vec<TransactionId> = s
        .run
        .as_ref()
        .expect("run")
        .items()
        .iter()
        .map(|i| i.id)
        .collect();
    s.requeue_ids = vec![ids[0]];
    s.requeued(Ok(Requeued {
        checks: Vec::new(),
        skipped: vec![ids[1]],
    }));
    // The first was asked for, the second skipped by the engine: both
    // are answered.
    let tenant = s.tenant.as_ref().map(|(t, _)| *t).expect("tenant");
    assert!(!waiting_answer(&s.link, tenant));
    assert!(!s.has_unanswered());
    assert_eq!(s.notice, Some("print.confirm.skipped"));
}
