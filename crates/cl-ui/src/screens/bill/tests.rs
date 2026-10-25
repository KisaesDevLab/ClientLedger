//! Enter Bill screen tests (no engine): the demo stand-in feeds the screen,
//! and its write counter proves when nothing was sent.

use std::sync::Arc;

use cl_api::{AttachTo, BillDraft};
use cl_core::Secret;
use cl_model::Scope;
use cl_model::forms::SaveIntent;
use cl_model::forms::lines::FormLine;
use eframe::egui;

use super::*;
use crate::company::demo::bills as demo_bills;
use crate::windows::OpenPayload;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn screen(link: &Arc<CompanyLink>) -> EnterBillScreen {
    let mut s = EnterBillScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn writes(link: &Arc<CompanyLink>) -> u32 {
    link.demo_state().expect("demo").bills.writes
}

fn account(s: &EnterBillScreen, n: &str) -> AccountId {
    s.accounts
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(n))
        .map_or_else(|| panic!("{n}"), |a| a.id)
}

fn contact(s: &EnterBillScreen, name: &str) -> ContactId {
    s.contacts
        .iter()
        .find(|c| c.fields.display_name == name)
        .map_or_else(|| panic!("{name}"), |c| c.id)
}

/// `vendor`, the amount due, and one expense line per `(account, amount)`.
fn fill(s: &mut EnterBillScreen, vendor: &str, amount: &str, lines: &[(&str, &str)]) {
    let v = contact(s, vendor);
    s.pick_vendor(v);
    let amount = money(amount);
    s.header.edit_step(|h| h.set_amount_due(Some(amount)));
    let seeds: Vec<FormLine> = lines
        .iter()
        .map(|(a, m)| {
            let account = account(s, a);
            let key = s.expenses.form_mut().adapter_mut().keys.next_key();
            FormLine {
                account: Some(account),
                amount: Some(money(m)),
                ..FormLine::blank(key)
            }
        })
        .collect();
    let entry = FormLine::blank(s.expenses.form_mut().adapter_mut().keys.next_key());
    s.expenses.form_mut().edit_step(|v| {
        v.lines = seeds;
        v.lines.push(entry);
    });
    s.sync_inputs();
}

/// The open balance the demo engine reports for `vendor` (the vendor
/// panel's figure; never summed by the screen).
fn owed(link: &Arc<CompanyLink>, vendor: ContactId) -> Money {
    let mut demo = link.demo_state().expect("demo");
    demo_bills::vendor_summary(&mut demo, vendor).total_owed
}

fn frames(s: &mut EnterBillScreen, width: f32, n: usize) {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    for _ in 0..n {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
    }
}

#[test]
fn ctrl_b_opens_a_blank_bill_with_the_engine_capabilities() {
    let (_d, link) = link();
    let cmd = cl_model::commands::find(KIND).expect("vendors.enter_bill");
    assert!(cmd.opens && cmd.arrives.is_none());
    let s = make(cmd.id, &link, None, today()).expect("the screen");
    assert_eq!(s.title(), "Enter Bill");
    assert_eq!(s.scope(), Scope::Form);
    let st = s.state();
    assert_eq!(st["screen"], "enter_bill");
    assert_eq!(st["kind"], "bill");
    assert_eq!(st["mode"], "new");
    assert_eq!(st["date"], "2026-11-04");
    assert_eq!(st["dirty"], false);
    assert_eq!(st["bill_received"], true);
    // The demo plays an engine with the P17 work.
    assert_eq!(st["items_tab"], true);
    assert_eq!(st["caps"]["closing"], true);
    let prefill = BillPrefill {
        kind: BillKind::Credit,
        vendor: ContactId::parse("00000000-0000-4000-8000-000000000001").expect("id"),
        date: today(),
        ref_no: "INV-55871".into(),
        amount: Money::ZERO,
    };
    assert!(!format!("{prefill:?}").contains("INV"));
}

#[test]
fn the_mockup_bill_posts_to_payables_and_the_vendor_balance_moves() {
    let (_d, link) = link();
    let mut s = EnterBillScreen::demo_draft(Arc::clone(&link), None, today());
    let vendor = contact(&s, demo_bills::STORY_VENDOR);
    assert_eq!(owed(&link, vendor), Money::ZERO, "a new vendor");
    assert!(s.is_dirty());
    assert_eq!(s.title(), "Enter Bill \u{b7} INV-55871");
    let f = s.compose();
    assert_eq!(f.amount_due, Some(money("1480.00")));
    assert_eq!(f.expenses.used().count(), 2);
    assert_eq!(s.state()["totals"], "balanced");
    // 11/03 + Net 15 (the local preview).
    assert_eq!(s.state()["due_date"], "2026-11-18");
    assert_eq!(s.state()["attachments"]["draft_files"], 1);
    assert!(s.vendor_address().contains("88 Industrial Pkwy"));
    let before = writes(&link);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before + 1, "sent once");
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(s.state()["last_saved"]["kind"], "bill");
    // A blank bill on the same date.
    assert!(!s.is_dirty());
    assert_eq!(s.state()["has_vendor"], false);
    assert_eq!(s.state()["date"], "2026-11-03");
    assert_eq!(owed(&link, vendor), money("1480.00"));
    // In the books: the lines as typed, A/P credited with the total, the
    // file moved onto the bill.
    let id = s.save.last.as_ref().expect("last").id;
    let mut demo = link.demo_state().expect("demo");
    let v = demo_bills::get(&mut demo, id).expect("bill");
    let amounts: Vec<Money> = v.record.lines.iter().map(|l| l.amount).collect();
    assert_eq!(amounts, [money("1330.00"), money("150.00")]);
    assert_eq!(
        v.record.lines[0].customer,
        Some(contact(&s, demo_bills::STORY_CUSTOMER))
    );
    assert!(v.record.lines[0].billable);
    assert_eq!(demo_bills::attachments(&demo, AttachTo::Bill(id)).len(), 1);
}

#[test]
fn lines_that_disagree_by_a_cent_send_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "1480.00",
        &[("6210", "1330.00"), ("6210", "149.99")],
    );
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before, "nothing reached the engine");
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    assert_eq!(s.state()["totals"], "remaining");
    assert!(s.is_dirty());
    // Fixed: the problem goes away, and the save goes through.
    s.expenses
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("150.00")));
    s.recheck_problems();
    assert_eq!(s.state()["save"]["problems"], 0);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before + 1);
}

#[test]
fn a_closed_period_bill_is_refused_without_the_engine_update_and_asks_with_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    let sept = CalendarDate::from_ymd(2026, 9, 15).expect("d");
    s.header.edit_step(|h| h.date = Some(sept));
    // The pinned engine (no ap-closing-v1): refused in plain words.
    link.demo_state().expect("demo").bills.closing_off = true;
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["save"]["refused"], "closed_needs_update");
    assert_eq!(writes(&link), before);
    assert_eq!(
        crate::i18n::t("bill.problem.closed_needs_update"),
        "This date is in a closed period. Saving a bill in a closed period needs a books update."
    );
    // With ap-closing-v1: the closing-date password first.
    link.demo_state().expect("demo").bills.closing_off = false;
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["closing"], "asking");
    assert_eq!(writes(&link), before);
    s.closing_prompt.password = crate::screens::company::form::SecretText::from("wrong");
    s.submit_closing();
    assert_eq!(s.state()["closing"], "rejected");
    s.closing_prompt.password = crate::screens::company::form::SecretText::from(
        crate::company::demo::register::DEMO_CLOSING_PASSWORD,
    );
    s.submit_closing();
    assert_eq!(s.state()["closing"], "done");
    assert_eq!(s.state()["saved_count"], 1);
}

#[test]
fn a_vendor_credit_posts_the_other_way_and_is_read_only_without_ap_edit() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.set_kind(BillKind::Credit);
    assert_eq!(s.title(), "Vendor Credit");
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "150.00",
        &[("6210", "150.00")],
    );
    assert!(s.start_save(SaveIntent::Save, None));
    let st = s.state();
    assert_eq!(st["last_saved"]["kind"], "credit");
    assert_eq!(st["mode"], "edit", "Save stays on the saved credit");
    let id = s.save.last.as_ref().expect("last").id;
    {
        let mut demo = link.demo_state().expect("demo");
        let v = demo_bills::get(&mut demo, id).expect("credit");
        assert_eq!(v.record.kind(), cl_api::ApKind::VendorCredit);
        let vendor = v.record.txn.contact_id.expect("vendor");
        assert_eq!(
            demo_bills::vendor_summary(&mut demo, vendor).available_credits_count,
            1
        );
    }
    // Opened from elsewhere (statement match, the register): its own form.
    let mut opened = screen(&link);
    assert!(opened.open_record(id));
    assert_eq!(opened.state()["kind"], "credit");
    assert_eq!(opened.state()["mode"], "edit");
    // Prev over credits finds it; without ap-edit-v1 it opens read-only.
    s.new_bill(true);
    link.demo_state().expect("demo").bills.edit_off = true;
    s.navigate(true);
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["kind"], "credit");
    assert_eq!(s.state()["read_only"], true);
    assert_eq!(s.state()["read_only_note"], "bill.read_only.credit");
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(writes(&link), before);
    // Void still works.
    assert!(s.start_void(false));
    // A read-only credit isn't voided from the void prompt? It is: void is
    // its only way out.
    s.send_void("Entered twice", false, None);
    assert_eq!(s.state()["last_action"], "void");
}

#[test]
fn prefill_copies_the_last_bill_without_amounts_and_a_new_vendor_gets_its_default() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let v = contact(&s, demo_bills::STORY_VENDOR);
    s.pick_vendor(v);
    assert_eq!(s.state()["prefill"]["source"], "default");
    assert_eq!(s.state()["terms"], "net_15");
    let first = s.expenses.value().lines[0].clone();
    assert_eq!(first.account, Some(account(&s, "6210")));
    assert_eq!(first.amount, None);
    // Saved, the next bill copies it, never an amount.
    s.clear();
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "1480.00",
        &[("6210", "1330.00"), ("6210", "150.00")],
    );
    s.header.edit_step(|h| h.memo = "Rental".into());
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.pick_vendor(v);
    assert_eq!(s.state()["prefill"]["source"], "bill");
    let f = s.compose();
    assert_eq!(f.expenses.used().count(), 2);
    assert!(f.expenses.used().all(|l| l.amount.is_none()));
    assert_eq!(f.amount_due, None);
    assert_eq!(f.memo, "Rental");
}

#[test]
fn an_unknown_outcome_blocks_the_save_in_every_window_until_answered() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    link.demo_state().expect("demo").bills.lose_next_reply = true;
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert!(s.save.lost.pending());
    assert!(s.save_unconfirmed());
    assert!(
        link.lost_saves()
            .is_open(crate::forms::lost_save::LostScope::Form(LOST_SCOPE))
    );
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before, "never sent again blindly");
    assert_eq!(s.state()["notice"], "bill.lost.blocked");
    // A second Enter Bill window is blocked too.
    let mut other = screen(&link);
    frames(&mut other, 1440.0, 1);
    assert!(other.save.lost.pending());
    // The owner confirms: saving works again.
    let ev = s
        .save
        .lost
        .owner_confirmed(&mut s.header, LOST_SCOPE)
        .expect("confirmed");
    s.lost_event(ev);
    assert!(!s.save.lost.pending());
}

/// A file attached by path is read on a background thread (up to 10 MB),
/// never on the egui thread; the frames' polling uploads it.
#[test]
fn a_file_by_path_is_read_off_the_egui_thread() {
    let (dir, link) = link();
    let mut s = screen(&link);
    let pdf = dir.path().join("invoice.pdf");
    std::fs::write(&pdf, demo_bills::STORY_PDF).expect("write");
    s.attach_path(&pdf);
    assert!(s.attach.reading.is_some(), "reading in the background");
    assert!(s.attach.upload.is_none() && s.attach.shown.is_none());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while s.attach.reading.is_some() {
        assert!(std::time::Instant::now() < until, "the read never ended");
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_attach();
    }
    assert_eq!(s.attach.problem, None);
    assert!(s.attach.shown.is_some(), "uploaded and shown");
    // A missing file is a plain message, also from the background.
    s.attach_path(&dir.path().join("missing.pdf"));
    while s.attach.reading.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        s.poll_attach();
    }
    assert_eq!(s.attach.problem, Some("check.attach.not_found"));
}

/// Before `bill-lines-v1` the draft's files move in a second call after the
/// create. A lost reply cuts that off; when Check again finds the bill, the
/// move runs then (the review's should-fix 4).
#[test]
fn a_lost_reply_found_saved_still_moves_the_draft_files() {
    let (_d, link) = link();
    link.demo_state().expect("demo").bills.lines_off = true;
    let mut s = screen(&link);
    s.upload_bytes(demo_bills::STORY_PDF_NAME, demo_bills::STORY_PDF.to_vec());
    let draft = s.attach.draft.expect("a draft key");
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    let before: Vec<_> = demo_bills::list(&mut link.demo_state().expect("demo"), false);
    link.demo_state().expect("demo").bills.lose_next_reply = true;
    assert!(!s.start_save(SaveIntent::Save, None));
    assert!(s.save.lost.pending());
    let id = demo_bills::list(&mut link.demo_state().expect("demo"), false)
        .into_iter()
        .map(|(id, _)| id)
        .find(|id| !before.iter().any(|(b, _)| b == id))
        .expect("the bill was written");
    {
        let demo = link.demo_state().expect("demo");
        assert_eq!(demo_bills::attachments(&demo, AttachTo::Bill(id)).len(), 0);
        assert_eq!(
            demo_bills::attachments(&demo, AttachTo::Draft(draft)).len(),
            1
        );
    }
    s.save.lost.recheck_answering(
        &mut s.header,
        cl_api::Recheck::Committed {
            transaction: Some(id),
        },
    );
    s.poll_save();
    assert!(!s.save.lost.pending());
    let demo = link.demo_state().expect("demo");
    assert_eq!(
        demo_bills::attachments(&demo, AttachTo::Bill(id)).len(),
        1,
        "the file is on the bill"
    );
    assert_eq!(
        demo_bills::attachments(&demo, AttachTo::Draft(draft)).len(),
        0
    );
    drop(demo);
    assert!(s.save.attach_failed.is_none(), "the retry went through");
}

#[test]
fn a_paid_bill_keeps_vendor_date_and_amount_and_is_not_voided() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    assert!(s.start_save(SaveIntent::Save, None));
    let id = s.save.last.as_ref().expect("last").id;
    assert!(demo_bills::mark_paid(
        &mut link.demo_state().expect("demo"),
        id,
        money("10.00")
    ));
    s.read_bill(id, Some(BillKind::Bill));
    assert_eq!(s.state()["has_payments"], true);
    assert_eq!(s.state()["read_only"], false, "memo and lines can change");
    assert!(s.start_void(false));
    assert_eq!(s.state()["save"]["refusal"], "bill.refused.has_payments");
    assert_eq!(s.state()["void_open"], false);
}

#[test]
fn without_bill_lines_the_items_tab_and_line_customers_wait() {
    let (_d, link) = link();
    link.demo_state().expect("demo").bills.lines_off = true;
    let mut s = screen(&link);
    assert_eq!(s.state()["items_tab"], false);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    let customer = contact(&s, demo_bills::STORY_CUSTOMER);
    s.expenses
        .form_mut()
        .edit_step(|v| v.lines[0].customer = Some(customer));
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["refused"], "line_detail_needs_update");
    assert_eq!(writes(&link), before);
}

#[test]
fn a_statement_line_opens_a_prefilled_unsaved_bill() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let vendor = contact(&s, demo_bills::STORY_VENDOR);
    let before = writes(&link);
    assert!(s.open_with(OpenPayload::BillPrefill(BillPrefill {
        kind: BillKind::Credit,
        vendor,
        date: CalendarDate::from_ymd(2026, 10, 20).expect("d"),
        ref_no: "CM-7".into(),
        amount: money("42.00"),
    })));
    let f = s.compose();
    assert_eq!(f.kind, BillKind::Credit);
    assert_eq!(f.vendor, Some(vendor));
    assert_eq!(f.reference, "CM-7");
    assert_eq!(f.amount_due, Some(money("42.00")));
    assert!(s.is_dirty());
    assert_eq!(writes(&link), before, "nothing saved");
}

#[test]
fn an_inbox_file_rides_on_the_save_and_is_filed_onto_the_bill() {
    let (_d, link) = link();
    let draft = cl_api::DraftKey::new();
    let a = demo_bills::upload(
        &mut link.demo_state().expect("demo"),
        AttachTo::Draft(draft),
        demo_bills::STORY_PDF_NAME,
        demo_bills::STORY_PDF,
    )
    .expect("uploaded");
    let mut s = screen(&link);
    assert!(s.open_with(OpenPayload::InboxDraft(
        crate::screens::receipt_inbox::InboxDraft {
            draft,
            attachment: a.id,
            file: crate::widgets::pdf_viewer::DocSource {
                name: demo_bills::STORY_PDF_NAME.into(),
                bytes: Arc::from(demo_bills::STORY_PDF.to_vec()),
            },
        }
    )));
    assert_eq!(s.state()["attachments"]["from_inbox"], true);
    assert_eq!(s.state()["attachments"]["draft_files"], 1);
    frames(&mut s, 1440.0, 1);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let id = s.save.last.as_ref().expect("last").id;
    let demo = link.demo_state().expect("demo");
    let filed = crate::company::demo::checks::find_attachment(&demo, a.id).expect("file");
    assert_eq!(filed.attachable_type, "bill");
    assert_eq!(filed.attachable_id, id.to_string());
}

#[test]
fn the_draft_is_mapped_field_by_field_with_item_lines() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "1480.00",
        &[("6210", "150.00")],
    );
    let rental = account(&s, "6210");
    let mut item = cl_model::forms::item_lines::ItemLine::blank(s.items.keys().next_key());
    item.account = Some(rental);
    item.description = "Mini excavator".into();
    item.set_qty(Some(cl_core::Quantity::parse_wire("2").expect("q")));
    item.set_rate(Some(money("665.00")));
    let entry = cl_model::forms::item_lines::ItemLine::blank(s.items.keys().next_key());
    s.items.form_mut().edit_step(|v| {
        v.lines = vec![item, entry];
    });
    let checked = s.context_check(&s.compose()).expect("checked");
    let save::Draft::Bill(d) = save::to_draft(&checked, None, true) else {
        panic!("a bill");
    };
    assert_eq!(d.amount_due, money("1480.00"));
    assert_eq!(d.lines.len(), 2);
    assert_eq!(d.lines[1].amount, money("1330.00"));
    assert_eq!(d.lines[1].unit_price, Some(money("665.00")));
    assert_eq!(d.terms.as_ref().map(|t| t.code.as_str()), Some("net_15"));
    assert_eq!(d.due_date, None, "the engine works it out");
    let _: &BillDraft = &d;
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let id = s.save.last.as_ref().expect("last").id;
    let mut demo = link.demo_state().expect("demo");
    let v = demo_bills::get(&mut demo, id).expect("bill");
    assert_eq!(v.record.lines[1].quantity, d.lines[1].quantity);
}

#[test]
fn a_bill_changed_elsewhere_is_stale_and_reloads() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(
        &mut s,
        demo_bills::STORY_VENDOR,
        "18.25",
        &[("6210", "18.25")],
    );
    assert!(s.start_save(SaveIntent::Save, None));
    let id = s.save.last.as_ref().expect("last").id;
    // Another window changes it.
    {
        let mut demo = link.demo_state().expect("demo");
        let v = demo_bills::get(&mut demo, id).expect("bill");
        let r = &v.record;
        let d = BillDraft {
            vendor: r.txn.contact_id.expect("vendor"),
            date: r.txn.txn_date,
            vendor_invoice_number: None,
            terms: None,
            due_date: None,
            memo: Some("changed elsewhere".into()),
            amount_due: money("18.25"),
            lines: vec![cl_api::BillLineDraft::expense(
                r.lines[0].account,
                money("18.25"),
            )],
            received: true,
            draft_attachment: None,
        };
        demo_bills::update_bill(&mut demo, id, &v.version, &d, None).expect("other edit");
    }
    s.header.edit_step(|h| h.memo = "mine".into());
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["stale"], true);
    s.reload();
    assert_eq!(s.compose().memo, "changed elsewhere");
}

#[test]
fn the_demo_ap_register_shows_the_sample_bills() {
    let (_d, link) = link();
    let s = screen(&link);
    let ap = account(&s, "2000");
    let mut demo = link.demo_state().expect("demo");
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let summary =
        crate::company::demo::register::summary(&demo.ledger, &chart, ap, None).expect("summary");
    assert!(
        summary
            .current_balance
            .exact()
            .is_some_and(|m| !m.is_zero())
    );
    let chart_balance = chart.iter().find(|a| a.id == ap).and_then(|a| a.balance);
    assert_eq!(chart_balance, summary.current_balance.exact());
}

#[test]
fn the_form_draws_wide_and_narrow() {
    let (_d, link) = link();
    let mut s = EnterBillScreen::demo_draft(Arc::clone(&link), None, today());
    frames(&mut s, 1440.0, 3);
    frames(&mut s, 960.0, 3);
    s.tab = Tab::Items;
    frames(&mut s, 1440.0, 2);
    assert!(s.is_dirty());
    let _ = Secret::new("unused");
}
