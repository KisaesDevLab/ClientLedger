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

/// PR-2 review #8: Reprint on a check the engine skips as void: it is not
/// looked for in the list (it isn't there) and the skipped notice stays.
#[test]
fn reprinting_a_void_check_keeps_the_skipped_notice() {
    let (_d, mut s) = screen(false);
    let id = s.checks[0].id;
    let date = s.checks[0].date;
    let bank = s.bank;
    s.picked = [id].into_iter().collect();
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    s.answer(true);
    s.poll();
    if let Some(mut d) = s.link.demo_state() {
        d.printing.voided.insert(id);
    }
    assert!(s.open_with(OpenPayload::ReprintCheck(id, date)));
    for _ in 0..4 {
        s.poll();
    }
    assert_eq!(s.notice, Some("print.confirm.skipped"));
    assert!(s.preselect.is_empty());
    assert!(s.preview.is_none());
    assert_eq!(s.bank, bank, "no hunt through the other accounts");
}

fn lose_next(s: &PrintFormsScreen, mode: crate::company::demo::printing::LoseReply) {
    if let Some(mut d) = s.link.demo_state() {
        d.printing.lose_next = Some(mode);
    }
}

fn print_calls(s: &PrintFormsScreen) -> u32 {
    s.link.demo_state().map_or(0, |d| d.printing.print_calls)
}

fn saved(s: &PrintFormsScreen) -> cl_settings::UnconfirmedChecks {
    s.unconfirmed.as_ref().expect("store").load()
}

/// B2: a lost reply on an engine that replays the key: the same request
/// goes again under the same key, the first batch comes back (numbered
/// once), the pages go out and the question shows.
#[test]
fn a_lost_print_reply_on_a_keyed_engine_resends_under_the_key() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: true });
    s.print_clicked();
    s.poll();
    assert_eq!(print_calls(&s), 1, "numbered once; the resend was replayed");
    assert!(s.asking);
    assert_eq!(s.sent, Some(Sent::Printed));
    let all = saved(&s);
    let c = s.company_key().expect("company");
    assert_eq!(all.printed_for(&c).count(), 1);
    assert_eq!(all.intents_for(&c).count(), 0);
}

/// B2: a lost reply on an unkeyed engine after the numbers were recorded:
/// read back, found recorded: the batch is saved as printed, the pages
/// go out and "Did all checks print correctly?" shows.
#[test]
fn a_lost_print_reply_reads_back_and_continues_as_recorded() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    let c = s.company_key().expect("company");
    assert_eq!(
        saved(&s).intents_for(&c).count(),
        1,
        "saved before the answer"
    );
    assert!(s.recording());
    s.poll();
    assert!(!s.recording());
    assert!(s.asking, "the question shows");
    assert_eq!(s.sent, Some(Sent::Printed));
    assert!(s.has_unanswered());
    let all = saved(&s);
    assert_eq!(all.intents_for(&c).count(), 0);
    assert_eq!(all.printed_for(&c).next().map(|b| b.checks.len()), Some(2));
    s.answer(true);
    assert!(!s.has_unanswered());
}

/// S-a: the reply was lost before anything was recorded. In this session
/// the first send may still record, so the saved run stays undecided:
/// nothing is printed or asked and Print can't run it again; the next open
/// reads it back, finds nothing and drops it.
#[test]
fn a_lost_print_reply_not_recorded_stays_undecided_until_the_next_open() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::BeforeRecording);
    s.print_clicked();
    s.poll();
    assert!(!s.asking);
    assert_eq!(s.sent, None, "nothing went to the printer");
    assert_eq!(s.notice, Some("print.recover.not_yet"));
    let c = s.company_key().expect("company");
    assert_eq!(saved(&s).intents_for(&c).count(), 1, "kept, undecided");
    assert!(s.stuck_intent().is_some(), "Forget this print is offered");
    assert!(s.run.is_none(), "not printed again from this run");
    let link = Arc::clone(&s.link);
    let tenant = s.tenant.clone();
    drop(s);
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    let mut s = PrintFormsScreen::new(link, tenant, today, false);
    s.poll();
    s.poll();
    assert!(!s.asking);
    assert_eq!(saved(&s).batches.len(), 0, "read back on the next open");
    assert_eq!(s.unresolved.len(), 0);
    s.preview_checks();
    s.poll();
    s.print_clicked();
    s.poll();
    assert!(s.asking, "printed on the next try");
}

/// S-a: a keyed resend that gives up while the first send is still in
/// progress, and the read-back finds nothing yet: kept as undecided.
#[test]
fn a_resend_that_gave_up_in_progress_keeps_the_run_undecided() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::KeyedStillRunning);
    s.print_clicked();
    s.poll();
    s.poll();
    assert!(!s.asking);
    assert_eq!(s.sent, None);
    assert_eq!(s.notice, Some("print.recover.not_yet"));
    let c = s.company_key().expect("company");
    assert_eq!(saved(&s).intents_for(&c).count(), 1, "kept, undecided");
    assert_eq!(s.unresolved.len(), 1);
}
/// B2: only some checks carry the planned numbers: those are treated as
/// printed (nothing is sent: the preview has pages for the others) and the
/// question asks about them; the others stay in the list.
#[test]
fn a_partial_read_back_asks_about_the_recorded_checks_only() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let first: u64 = s.first_number.parse().expect("n");
    lose_next(&s, LoseReply::Partial);
    s.print_clicked();
    s.poll();
    assert!(s.asking);
    assert_eq!(s.sent, None);
    assert_eq!(s.notice, Some("print.recover.partial"));
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items().len(), 1);
    assert_eq!(run.items()[0].number, Some(first));
    let c = s.company_key().expect("company");
    assert_eq!(
        saved(&s).printed_for(&c).next().map(|b| b.checks.len()),
        Some(1)
    );
    s.first_bad = first.to_string();
    s.answer(false);
    s.poll();
    assert!(!s.has_unanswered());
    assert_eq!(s.checks.len(), 2, "both wait to print again");
}

/// B2: a crash after the engine recorded the numbers but before the answer
/// was handled: on the next open the saved intent is read back, the batch
/// is saved as printed and the question shows.
#[test]
fn a_saved_intent_is_recovered_on_the_next_open() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    // The window goes before the read-back is handled.
    let link = Arc::clone(&s.link);
    let tenant = s.tenant.clone();
    let tid = tenant.as_ref().map(|(t, _)| *t).expect("tenant");
    drop(s);
    assert!(waiting_answer(&link, tid), "Home reminds meanwhile");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    let mut s = PrintFormsScreen::new(link, tenant, today, false);
    s.poll();
    s.poll();
    assert!(s.asking, "the question for the recovered batch");
    let c = s.company_key().expect("company");
    let all = saved(&s);
    assert_eq!(all.intents_for(&c).count(), 0);
    assert_eq!(all.printed_for(&c).count(), 1);
    assert_eq!(s.run.as_ref().map(|r| r.items().len()), Some(2));
    s.answer(true);
    assert!(!s.has_unanswered());
}

/// B2: while the numbers are out, Print from a form and a new preview
/// don't replace the run; the reply is handled for the run that was sent.
#[test]
fn open_with_during_the_record_keeps_the_run() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    let ids: Vec<TransactionId> = s.checks.iter().map(|c| c.id).collect();
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    assert!(s.recording());
    assert!(s.open_with(OpenPayload::PrintChecks(vec![ids[1]])));
    assert_eq!(s.notice, Some("print.busy_recording"));
    s.preview_checks();
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items().len(), 2, "the run sent stays");
    assert!(s.open_with(OpenPayload::ReprintCheck(ids[0], s.checks[0].date)));
    s.poll();
    assert!(s.asking, "the reply was handled for the run that was sent");
    assert_eq!(s.run.as_ref().map(|r| r.items().len()), Some(2));
    assert!(s.has_unanswered());
}

/// PR-2 review #2: Print from a form while a run is recording: once the
/// run settles with no question (here it stays undecided), the checks are
/// previewed and the undecided run is still said.
#[test]
fn print_from_a_form_during_the_record_previews_once_settled() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    let ids: Vec<TransactionId> = s.checks.iter().map(|c| c.id).collect();
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::BeforeRecording);
    s.print_clicked();
    assert!(s.recording());
    assert!(s.open_with(OpenPayload::PrintChecks(vec![ids[1]])));
    s.poll();
    s.poll();
    assert!(!s.asking);
    assert_eq!(s.picked, [ids[1]].into_iter().collect());
    let p = s.preview.as_ref().expect("previewed once settled");
    assert_eq!(p.kind, PreviewKind::Checks);
    assert_eq!(s.notice, Some("print.recover.not_yet"));
}

/// PR-2 B2: plan [a, b]; the reply is lost and b is voided before the
/// read-back: a is recorded (the question asks about it), b is not, and
/// the intent is resolved. A void check never fails the whole read-back.
#[test]
fn a_void_check_in_a_lost_run_is_not_recorded_and_the_rest_is() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let first: u64 = s.first_number.parse().expect("n");
    lose_next(&s, LoseReply::VoidLast);
    s.print_clicked();
    s.poll();
    assert!(s.asking, "asked about the recorded check");
    assert_eq!(s.notice, Some("print.recover.partial"));
    let run = s.run.as_ref().expect("run");
    assert_eq!(run.items().len(), 1);
    assert_eq!(run.items()[0].number, Some(first));
    let c = s.company_key().expect("company");
    let all = saved(&s);
    assert_eq!(all.intents_for(&c).count(), 0, "intent resolved");
    assert_eq!(all.printed_for(&c).next().map(|b| b.checks.len()), Some(1));
    assert_eq!(s.unresolved.len(), 0);
}

/// The read-back decides each check on its own: gone, void, not a check or
/// numbered by another run is not recorded.
#[test]
fn decide_readback_is_per_check() {
    use cl_api::{CheckPrintState, PrintStatus};
    let ids: Vec<TransactionId> = (1..=5u128)
        .map(|n| TransactionId::from_uuid(uuid::Uuid::from_u128(n)))
        .collect();
    let planned: Vec<(TransactionId, u64)> = ids.iter().copied().zip(1001..).collect();
    let st = |n: Option<u32>, status: Option<PrintStatus>, void: bool, batch: Option<&str>| {
        Ok(Some(CheckPrintState {
            check_number: n,
            print_status: status,
            void,
            print_batch_id: batch.map(str::to_string),
        }))
    };
    let states = vec![
        st(Some(1001), Some(PrintStatus::Printed), false, Some("b1")),
        Ok(None),
        st(Some(1003), Some(PrintStatus::Printed), true, Some("b1")),
        st(None, None, false, None),
        st(Some(1099), Some(PrintStatus::Printed), false, Some("b1")),
    ];
    let found = decide_readback(&planned, &states).expect("one batch");
    let recorded: Vec<bool> = found.iter().map(|(_, r)| *r == Found::Recorded).collect();
    assert_eq!(recorded, vec![true, false, false, false, false]);
    // No batch on the record: not numbered by a print run.
    let nobatch = vec![st(Some(1001), Some(PrintStatus::Printed), false, None)];
    assert_eq!(
        decide_readback(&planned[..1], &nobatch).expect("decided")[0].1,
        Found::NotRecorded
    );
}

/// S-b: 1001 was later reprinted alone under its number in batch B; 1002
/// and 1003 still name batch A, the majority: those stay recorded. Two
/// batches with no majority leave the run undecided.
#[test]
fn decide_readback_picks_the_majority_batch() {
    use cl_api::{CheckPrintState, PrintStatus};
    let ids: Vec<TransactionId> = (1..=4u128)
        .map(|n| TransactionId::from_uuid(uuid::Uuid::from_u128(n)))
        .collect();
    let planned: Vec<(TransactionId, u64)> = ids.iter().copied().zip(1001..).collect();
    let st = |n: u32, batch: &str| -> ReadState {
        Ok(Some(CheckPrintState {
            check_number: Some(n),
            print_status: Some(PrintStatus::Printed),
            void: false,
            print_batch_id: Some(batch.to_string()),
        }))
    };
    let states = vec![st(1001, "B"), st(1002, "A"), st(1003, "A")];
    let found = decide_readback(&planned[..3], &states).expect("A is the majority");
    let recorded: Vec<bool> = found.iter().map(|(_, r)| *r == Found::Recorded).collect();
    assert_eq!(recorded, vec![false, true, true]);
    // 2 of 4 is not more than half.
    let states = vec![st(1001, "B"), st(1002, "A"), st(1003, "A"), Ok(None)];
    assert!(decide_readback(&planned, &states).is_none(), "undecided");
    let states = vec![st(1001, "B"), st(1002, "A")];
    assert!(
        decide_readback(&planned[..2], &states).is_none(),
        "undecided"
    );
}

/// PR-2 review #6: a check whose read fails is undecided on its own; the
/// others are still decided.
#[test]
fn decide_readback_leaves_a_failed_read_undecided() {
    use cl_api::{CheckPrintState, PrintStatus};
    let ids: Vec<TransactionId> = (1..=3u128)
        .map(|n| TransactionId::from_uuid(uuid::Uuid::from_u128(n)))
        .collect();
    let planned: Vec<(TransactionId, u64)> = ids.iter().copied().zip(1001..).collect();
    let st = |n: u32| -> ReadState {
        Ok(Some(CheckPrintState {
            check_number: Some(n),
            print_status: Some(PrintStatus::Printed),
            void: false,
            print_batch_id: Some("A".into()),
        }))
    };
    let states = vec![st(1001), Err(ApiError::Timeout), st(1003)];
    let found = decide_readback(&planned, &states).expect("decided");
    let f: Vec<Found> = found.iter().map(|(_, f)| *f).collect();
    assert_eq!(f, vec![Found::Recorded, Found::Unknown, Found::Recorded]);
    let states = vec![Err(ApiError::Timeout), Ok(None), Err(ApiError::Timeout)];
    let f: Vec<Found> = decide_readback(&planned, &states)
        .expect("decided")
        .iter()
        .map(|(_, f)| *f)
        .collect();
    assert_eq!(f, vec![Found::Unknown, Found::NotRecorded, Found::Unknown]);
}

/// PR-2 review #6: the read-back of one check fails: the recorded check is
/// asked about, the other stays saved under the run's key (undecided, may
/// be forgotten), and Home keeps reminding.
#[test]
fn a_failed_read_keeps_only_that_check_saved() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let first: u64 = s.first_number.parse().expect("n");
    let ids: Vec<TransactionId> = s.picked_checks().iter().map(|c| c.id).collect();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    // The demo's answer is dropped; the engine's per-check answer instead.
    s.readback_task = None;
    s.read_back(&[(ids[0], Found::Recorded), (ids[1], Found::Unknown)]);
    assert!(s.asking, "asked about the recorded check");
    assert_eq!(s.run.as_ref().map(|r| r.items().len()), Some(1));
    let c = s.company_key().expect("company");
    let all = saved(&s);
    let left = all
        .intents_for(&c)
        .next()
        .expect("the undecided check waits");
    assert_eq!(left.checks.len(), 1);
    assert_eq!(left.checks[0].number, first + 1);
    assert_eq!(all.printed_for(&c).next().map(|b| b.checks.len()), Some(1));
    assert_eq!(s.stuck_intent().map(|(_, n)| n), Some(vec![first + 1]));
    // Nothing recorded and one read failed: the whole run waits.
    let (_d2, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let ids: Vec<TransactionId> = s.picked_checks().iter().map(|c| c.id).collect();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    s.readback_task = None;
    s.read_back(&[(ids[0], Found::NotRecorded), (ids[1], Found::Unknown)]);
    assert!(!s.asking);
    assert_eq!(s.notice, Some("print.recover.unknown"));
    let c = s.company_key().expect("company");
    assert_eq!(
        saved(&s).intents_for(&c).next().map(|b| b.checks.len()),
        Some(2)
    );
}

/// Nits: Preview while "Did all checks print correctly?" waits says so;
/// "Forget this print" is refused while the question is open.
#[test]
fn the_open_question_blocks_preview_and_forget() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    s.readback_task = None;
    s.readback_failed();
    let (key, _) = s.stuck_intent().expect("stuck");
    s.asking = true;
    s.preview_checks();
    assert_eq!(s.notice, Some("print.answer_first"));
    s.forget_intent(&key);
    assert_ne!(s.notice, Some("print.recover.forgotten"));
    assert!(s.stuck_intent().is_some(), "not forgotten while asking");
    s.asking = false;
    s.forget_intent(&key);
    assert_eq!(s.notice, Some("print.recover.forgotten"));
}

/// Every saved intent is read back, one after the other: the second after
/// the first one's question is answered.
#[test]
fn two_saved_intents_are_both_recovered() {
    let (_d, s) = screen(false);
    let ids: Vec<TransactionId> = s.checks.iter().map(|c| c.id).collect();
    let bank = s.bank.expect("bank");
    let company = s.company_key().expect("company");
    let store = s.unconfirmed.clone().expect("store");
    let link = Arc::clone(&s.link);
    let tenant = s.tenant.clone();
    drop(s);
    let mut all = store.load();
    for (i, id) in ids.iter().enumerate() {
        let number = 3001 + i as u64;
        if let Some(mut d) = link.demo_state() {
            let req = PrintChecks {
                bank_account: bank,
                checks: vec![*id],
                starting_number: number,
                format: CheckFormat::Voucher,
            };
            crate::company::demo::printing::print(&mut d, &req, None).expect("recorded");
        }
        all.add(UnconfirmedBatch {
            company_id: company.clone(),
            bank_account_id: bank.to_string(),
            printed_at: "2026-11-04T10:00:00Z".into(),
            checks: vec![UnconfirmedCheck {
                check_id: id.to_string(),
                number,
            }],
            state: BatchState::Intent,
            client_key: Some(format!("key-{i}")),
            format: Some("voucher".into()),
        })
        .expect("room");
    }
    store.save(&all).expect("save");
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    let mut s = PrintFormsScreen::new(link, tenant, today, false);
    s.poll();
    s.poll();
    assert!(s.asking, "the first intent's question");
    assert_eq!(
        saved(&s).intents_for(&company).count(),
        1,
        "one still to read back"
    );
    s.answer(true);
    s.poll();
    assert!(s.asking, "the second intent's question");
    assert_eq!(saved(&s).intents_for(&company).count(), 0);
    s.answer(true);
    s.poll();
    assert!(!s.asking);
    assert!(!s.has_unanswered());
}

/// PR-2 #3: after the keyed resend any error reads back (here the engine
/// had recorded the run: the pages go out and the question shows).
#[test]
fn an_error_after_the_keyed_resend_reads_back() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::KeyedThenRefused);
    s.print_clicked();
    s.poll();
    assert!(s.asking, "read back and found recorded");
    assert_eq!(s.sent, Some(Sent::Printed));
    let c = s.company_key().expect("company");
    assert_eq!(saved(&s).intents_for(&c).count(), 0);
    assert_eq!(saved(&s).printed_for(&c).count(), 1);
}

/// PR-2 #3: an answer that says the run was saved but can't be read reads
/// back; a plain refusal drops the intent.
#[test]
fn saved_unreadable_reads_back_and_a_refusal_drops_the_intent() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::Unreadable);
    s.print_clicked();
    s.poll();
    assert!(s.asking);
    assert_eq!(s.sent, Some(Sent::Printed));
    assert!(keeps_key(&ApiError::SavedUnreadable { what: "x" }));
    assert!(keeps_key(&ApiError::Timeout));
    assert!(keeps_key(&ApiError::IdempotencyKeyReused { entity: None }));
    assert!(!keeps_key(&ApiError::ClosingPasswordRequired));
    assert!(!keeps_key(&ApiError::NotFound { code: None }));
}

/// PR-2 review #5: print keeps the key exactly as the other keyed saves: a
/// first send answered "still in progress" is read back, never dropped as
/// a refusal; refusals that record nothing drop it.
#[test]
fn a_busy_first_send_reads_back_like_every_keyed_save() {
    let busy = ApiError::Conflict {
        reason: cl_api::ConflictReason::Busy,
        current_version: None,
        code: cl_api::ErrorCode::parse("IDEMPOTENCY_IN_PROGRESS"),
        message: cl_api::UserText::default(),
    };
    assert!(keeps_key(&busy));
    assert!(!keeps_key(&ApiError::AccountPasswordRejected));
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    // The first send is answered 409 IDEMPOTENCY_IN_PROGRESS.
    if let Some(mut d) = s.link.demo_state() {
        d.printing.still_running = true;
    }
    s.print_clicked();
    s.poll();
    let c = s.company_key().expect("company");
    assert_eq!(saved(&s).intents_for(&c).count(), 1, "kept, read back");
    assert_eq!(s.notice, Some("print.recover.not_yet"));
}

/// PR-2 #4: the intent can't be saved: nothing is sent and the owner is
/// told.
#[test]
fn an_unsaved_intent_sends_nothing() {
    let (d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    // A file where the store's folder should be: every save fails.
    let blocker = d.path().join("not-a-folder");
    std::fs::write(&blocker, b"x").expect("file");
    s.unconfirmed = Some(UnconfirmedChecksStore::in_dir(&blocker));
    s.print_clicked();
    s.poll();
    assert_eq!(print_calls(&s), 0, "nothing sent");
    assert_eq!(s.notice, Some("print.intent_unsaved"));
    assert!(!s.asking);
    assert!(!s.recording());
    assert_eq!(
        s.run.as_ref().map(cl_print::PrintRun::state),
        Some(cl_print::RunState::Ready)
    );
}

/// PR-2 review #3: "Forget this print" is drawn each frame from a value
/// read when the run became undecided, never from the file each frame.
#[test]
fn the_stuck_intent_is_read_once_not_each_frame() {
    use crate::company::demo::printing::LoseReply;
    let (d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    s.readback_task = None;
    s.readback_failed();
    let stuck = s.stuck_intent().expect("stuck");
    // The file can't be read any more: the drawn value stays.
    let blocker = d.path().join("not-a-folder");
    std::fs::write(&blocker, b"x").expect("file");
    s.unconfirmed = Some(UnconfirmedChecksStore::in_dir(&blocker));
    assert_eq!(s.stuck_intent(), Some(stuck));
}

/// PR-2 review #9: with the file full of other companies' unanswered
/// batches, Print sends nothing and says why; nothing saved is dropped.
#[test]
fn a_full_file_sends_nothing_and_drops_nothing() {
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let store = s.unconfirmed.clone().expect("store");
    let mut all = store.load();
    for i in 0..cl_settings::unconfirmed_checks::MAX_BATCHES {
        all.add(UnconfirmedBatch {
            company_id: "another-company".into(),
            bank_account_id: "b".into(),
            printed_at: "2026-11-04T10:00:00Z".into(),
            checks: vec![UnconfirmedCheck {
                check_id: format!("c{i}"),
                number: 1,
            }],
            state: BatchState::Printed,
            client_key: None,
            format: None,
        })
        .expect("room");
    }
    store.save(&all).expect("save");
    s.print_clicked();
    s.poll();
    assert_eq!(print_calls(&s), 0, "nothing sent");
    assert_eq!(s.notice, Some("print.too_many_waiting"));
    assert_eq!(saved(&s), all, "nothing dropped");
}

/// Q-PR2d: an intent the read-back can't decide is skipped this session
/// and the owner may forget it; Home's reminder clears.
#[test]
fn an_undecided_intent_can_be_forgotten() {
    use crate::company::demo::printing::LoseReply;
    let (_d, mut s) = screen(false);
    s.preview_checks();
    s.poll();
    let first: u64 = s.first_number.parse().expect("n");
    lose_next(&s, LoseReply::AfterRecording { keyed: false });
    s.print_clicked();
    // The engine can't be asked: the read-back fails.
    s.readback_task = None;
    s.readback_failed();
    assert_eq!(s.notice, Some("print.recover.unknown"));
    assert_eq!(s.unresolved.len(), 1);
    let (key, numbers) = s.stuck_intent().expect("stuck");
    assert_eq!(numbers, vec![first, first + 1]);
    s.recover_intents();
    assert!(!s.recording(), "not read back again this session");
    let tenant = s.tenant.as_ref().map(|(t, _)| *t).expect("tenant");
    assert!(waiting_answer(&s.link, tenant));
    s.forget_intent(&key);
    assert_eq!(s.notice, Some("print.recover.forgotten"));
    assert_eq!(s.unresolved.len(), 0);
    assert!(s.stuck_intent().is_none());
    assert!(!waiting_answer(&s.link, tenant));
}
