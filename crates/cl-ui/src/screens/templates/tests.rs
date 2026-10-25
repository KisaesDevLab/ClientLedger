//! The Templates list on the demo engine: grouping, Use, Enter now (with
//! the closing-date password), Edit, Delete and the empty state.

use cl_api::{
    Frequency, NewTemplate, ScheduleEnd, ScheduleMode, TemplateDraft, TemplateSchedule,
    TransferDraft,
};
use cl_core::Money;

use super::*;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;

fn d(y: i32, m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(y, m, day).expect("date")
}

fn today() -> CalendarDate {
    d(2026, 11, 4)
}

fn demo() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    if let Some(mut s) = link.demo_state() {
        s.open = s.companies.first().map(|c| c.tenant_id);
    }
    (dir, link)
}

fn add(
    link: &CompanyLink,
    name: &str,
    next: Option<CalendarDate>,
    group: Option<&str>,
) -> Template {
    let mut s = link.demo_state().expect("demo");
    let group = group.map(|g| {
        template_store::create_group(&mut s, g)
            .or_else(|_| {
                template_store::groups(&s)
                    .into_iter()
                    .find(|x| x.name == g)
                    .ok_or(ApiError::Internal)
            })
            .expect("group")
            .id
    });
    template_store::create(
        &mut s,
        &NewTemplate {
            name: name.into(),
            group,
            client_form: None,
            draft: TemplateDraft::Transfer(TransferDraft {
                from: crate::company::demo::account_id(1010),
                to: crate::company::demo::account_id(1020),
                date: today(),
                amount: Money::parse_wire("100.00").expect("m"),
                memo: None,
            }),
            schedule: next.map(|n| TemplateSchedule {
                mode: ScheduleMode::Remind,
                frequency: Frequency::Monthly,
                interval: 1,
                next_date: n,
                end: ScheduleEnd::Never,
            }),
        },
    )
    .expect("template")
}

fn screen(link: &Arc<CompanyLink>) -> TemplatesScreen {
    TemplatesScreen::new(Arc::clone(link), None, today())
}

#[test]
fn empty_state_then_grouped_rows() {
    let (_dir, link) = demo();
    let s = screen(&link);
    assert_eq!(s.state_json()["empty"], true);
    add(
        &link,
        "Savings",
        Some(d(2026, 12, 1)),
        Some("Monthly bills"),
    );
    add(&link, "Loose", None, None);
    let s = screen(&link);
    let v = s.state_json();
    assert_eq!(
        (v["count"].as_u64(), v["groups"].as_u64()),
        (Some(2), Some(1))
    );
    // The group first, then no group.
    assert!(!v["rows"][0]["group"].is_null(), "grouped first");
    assert_eq!(v["rows"][0]["mode"], "remind");
    assert_eq!(v["rows"][1]["mode"], "never");
    assert_eq!(v["rows"][0]["next_date"], "2026-12-01");
    assert!(
        !v.to_string().contains("Savings"),
        "no names on the channel"
    );
}

#[test]
fn use_opens_the_form_filled_in() {
    let (_dir, link) = demo();
    let made = add(&link, "Savings", None, None);
    let mut s = screen(&link);
    s.selected = Some(made.id);
    s.use_selected();
    let events = link.drain();
    let Some(CompanyEvent::OpenTemplate(open)) = events.last() else {
        panic!("no OpenTemplate: {events:?}");
    };
    assert_eq!(open.kind.as_str(), crate::screens::transfer::KIND);
    assert_eq!(open.template, made.id);
    let TemplateDraft::Transfer(t) = &open.draft else {
        panic!("a transfer");
    };
    assert_eq!(t.date, today(), "dated today");
    assert_eq!(s.last_action, Some("use"));
}

#[test]
fn enter_now_asks_for_the_closing_password_in_a_closed_period() {
    let (_dir, link) = demo();
    // The demo company is closed through 09/30/2026.
    let made = add(&link, "Old", Some(d(2026, 9, 15)), None);
    let mut s = screen(&link);
    let mut watcher = link.bus().subscribe();
    s.selected = Some(made.id);
    s.enter_selected();
    assert!(s.closing_prompt.dialog_open(), "{:?}", s.error);
    assert!(link.bus().take(&mut watcher).is_empty(), "nothing posted");
    // The password retries the same template and date, under the same key
    // (a refusal is definite: a fresh one).
    assert_eq!(
        s.closing_prompt.pending(),
        Some(&EnterNow(made.id, d(2026, 9, 15)))
    );
    s.enter_at(
        made.id,
        d(2026, 9, 15),
        Some(Secret::new(DEMO_CLOSING_PASSWORD.to_string())),
    );
    assert_eq!(s.last_action, Some("enter"));
    assert!(s.entered.is_some());
    assert!(
        link.bus().take(&mut watcher).ledger_any(),
        "registers hear it"
    );
    // The schedule moved on.
    s.poll();
    assert_eq!(s.state_json()["rows"][0]["next_date"], "2026-10-15");
}

#[test]
fn enter_now_without_a_schedule_asks_for_the_date() {
    let (_dir, link) = demo();
    let made = add(&link, "Loose", None, None);
    let mut s = screen(&link);
    s.selected = Some(made.id);
    s.enter_selected();
    assert_eq!(s.state_json()["enter_date"], "2026-11-04", "today to start");
    assert!(s.enter_job.is_none(), "nothing sent before the date");
    // A date that doesn't parse is refused here.
    if let Some((_, input)) = s.enter_date.as_mut() {
        input.text = "13/45".into();
        input.reparse(today());
    }
    s.enter_date_confirmed();
    assert_eq!(s.notice, Some("templates.enter.date_missing"));
    if let Some((_, input)) = s.enter_date.as_mut() {
        input.set(d(2026, 11, 2));
    }
    s.enter_date_confirmed();
    assert!(s.enter_date.is_none());
    assert_eq!(s.last_action, Some("enter"));
    let txn = s.entered.expect("entered").0;
    let mut st = link.demo_state().expect("demo");
    let v = crate::company::demo::journal::get(&mut st, txn).expect("posted");
    drop(st);
    assert_eq!(v.record.txn_date, d(2026, 11, 2), "dated as chosen");
    // Esc closes the date card.
    s.enter_selected();
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.enter_date.is_none());
}

#[test]
fn enter_now_keeps_one_key_per_date_until_a_definite_answer() {
    let (_dir, link) = demo();
    let made = add(&link, "Savings", Some(d(2026, 12, 1)), None);
    let other = add(&link, "Other", Some(d(2026, 12, 1)), None);
    let mut s = screen(&link);
    let occ = (made.id, d(2026, 12, 1));
    let k1 = s.enter_key_for(occ);
    // A lost reply: the outcome is unknown, the key stays for the retry.
    s.enter_for = Some(occ);
    s.enter_done(Err(ApiError::Timeout));
    assert_eq!(s.enter_key_for(occ), k1, "the retry is the same post");
    // Another date or template is another post.
    assert_ne!(s.enter_key_for((made.id, d(2026, 12, 2))), k1);
    let k2 = s.enter_key_for(occ);
    assert_ne!(k2, k1, "the key follows the occurrence");
    assert_ne!(s.enter_key_for((other.id, d(2026, 12, 1))), k2);
    // "Already entered" after the lost reply counts as done and ends it.
    let k3 = s.enter_key_for(occ);
    s.enter_for = Some(occ);
    s.enter_done(Err(ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: cl_api::ErrorCode::parse(cl_api::OCCURRENCE_TAKEN),
        message: cl_api::UserText::default(),
    }));
    assert_eq!(s.notice, Some("templates.entered_already"));
    assert_ne!(s.enter_key_for(occ), k3, "a definite answer: a fresh key");
    // A definite refusal also ends it.
    let k4 = s.enter_key_for(occ);
    s.enter_for = Some(occ);
    s.enter_done(Err(ApiError::NotFound { code: None }));
    assert_ne!(s.enter_key_for(occ), k4);
}

#[test]
fn edit_changes_the_schedule_and_delete_confirms() {
    let (_dir, link) = demo();
    let made = add(&link, "Savings", Some(d(2026, 12, 1)), None);
    let mut s = screen(&link);
    s.selected = Some(made.id);
    s.edit_selected();
    let (id, version, mut dlg) = s.dialog.take().expect("dialog");
    assert!(dlg.set("next_date", "01/15/2027"));
    let c = dlg.save().expect("ok");
    s.save_edit(id, version, c);
    assert_eq!(s.last_action, Some("edit"));
    s.poll();
    assert_eq!(s.state_json()["rows"][0]["next_date"], "2027-01-15");
    // A stale version is refused and the list reloads.
    s.save_edit(id, version, dlg.save().expect("ok"));
    assert!(s.stale);
    s.confirm_delete = Some(made.id);
    s.delete_confirmed();
    s.poll();
    assert_eq!(s.state_json()["empty"], true);
}

#[test]
fn an_automatic_start_too_far_back_says_the_earliest_date() {
    let (_dir, link) = demo();
    let made = add(&link, "Savings", Some(d(2026, 12, 1)), None);
    let mut s = screen(&link);
    s.selected = Some(made.id);
    s.edit_selected();
    let (id, version, mut dlg) = s.dialog.take().expect("dialog");
    assert!(dlg.set("mode", "auto"));
    assert!(dlg.set("next_date", "01/01/2025"));
    let c = dlg.save().expect("ok");
    s.dialog = Some((id, version, dlg));
    s.save_edit(id, version, c);
    assert_eq!(s.state_json()["auto_start_too_early"], true);
    assert!(s.dialog.is_some(), "the dialog stays open to fix the date");
    let e = s.error.as_ref().expect("refused");
    let text = auto_start_text(e, today()).expect("plain words");
    assert!(text.contains("11/03/2025"), "{text}");
    // Any other error keeps its own message.
    assert!(auto_start_text(&ApiError::Internal, today()).is_none());
    // The same through Save as Template on a form.
    let mut f = SaveFlow::default();
    f.error = Some(e.clone());
    assert_eq!(f.state_json()["auto_start_too_early"], true);
}

#[test]
fn new_group_and_keys() {
    let (_dir, link) = demo();
    add(&link, "A", None, None);
    add(&link, "B", None, None);
    let mut s = screen(&link);
    s.new_group = Some("Monthly bills".into());
    s.create_group();
    s.poll();
    assert_eq!(s.groups.len(), 1);
    let first = s.selected;
    s.step(true);
    assert_ne!(s.selected, first);
    s.step(false);
    assert_eq!(s.selected, first);
    assert!(s.command(CommandId::new(KIND)));
    s.new_group = Some(String::new());
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.new_group.is_none());
    // Delete (the key) asks first, and the last "entered" note goes.
    s.notice = Some("templates.entered");
    assert!(s.command(CommandId::new("list.delete")));
    assert_eq!(s.confirm_delete, s.selected);
    assert!(s.notice.is_none());
    assert!(s.command(CommandId::new("form.revert")));
    assert!(s.confirm_delete.is_none(), "Esc cancels the question");
    // Ctrl+E edits the selected template.
    assert!(s.command(CommandId::new("edit.edit_transaction")));
    assert!(s.dialog.is_some());
}

#[test]
fn draws_light_and_dark() {
    let (_dir, link) = demo();
    add(
        &link,
        "Savings",
        Some(d(2026, 12, 1)),
        Some("Monthly bills"),
    );
    let mut s = screen(&link);
    let ctx = egui::Context::default();
    for dark in [false, true] {
        ctx.set_visuals(if dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui));
        out.textures_delta.clear();
    }
    s.edit_selected();
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui));
    out.textures_delta.clear();
    assert!(s.state_json()["dialog"].is_object());
    assert_eq!(
        form_kind(Some(client_form::CARD_CREDIT), &TemplateKind::Deposit),
        Some(crate::screens::card_charge::KIND)
    );
    assert_eq!(form_kind(None, &TemplateKind::Legacy), None);
}

/// Review fix-PR-1 blocking 2: closing the window does not lift the block
/// of a lost Enter now; the next window shows it again, Use waits too, and
/// its answer clears it for every window.
#[test]
fn a_lost_enter_now_outlives_its_window() {
    use crate::forms::lost_save::LostScope;
    let (_dir, link) = demo();
    let made = add(&link, "Loose", None, None);
    let mut s = screen(&link);
    s.enter_for = Some((made.id, d(2026, 11, 2)));
    s.enter_done(Err(ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000017")
            .expect("id"),
        idempotency_key: Some(cl_api::IdempotencyKey::new()),
    }));
    assert!(s.lost.pending());
    drop(s);
    assert!(link.lost_saves().is_open(LostScope::Template(made.id)));
    // A new Templates window: the same banner and block.
    let mut again = screen(&link);
    assert!(again.lost.pending(), "the block outlives the window");
    let mut watcher = link.bus().subscribe();
    again.enter_at(made.id, d(2026, 11, 2), None);
    assert!(again.entered.is_none());
    again.selected = Some(made.id);
    again.use_selected();
    assert!(again.use_job.is_none(), "Use could enter it twice: it waits");
    assert!(link.bus().take(&mut watcher).is_empty(), "nothing posted");
    // Check again here: found; every window hears it.
    let txn = TransactionId::parse("d0000000-0000-4000-8000-b00000000019").expect("t");
    again.lost.recheck_answering(
        &mut again.lost_gate,
        cl_api::Recheck::Committed {
            transaction: Some(txn),
        },
    );
    again.poll();
    assert_eq!(again.entered.map(|(t, _)| t), Some(txn));
    assert!(!link.lost_saves().is_open(LostScope::Template(made.id)));
    assert!(!screen(&link).lost.pending(), "a third window starts clean");
}

#[test]
fn b2_a_lost_enter_now_without_a_schedule_sends_nothing_until_the_recheck_answers() {
    let (_dir, link) = demo();
    let made = add(&link, "Loose", None, None);
    let mut s = screen(&link);
    let occ = (made.id, d(2026, 11, 2));
    s.enter_for = Some(occ);
    s.enter_done(Err(ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000007")
            .expect("id"),
        idempotency_key: Some(cl_api::IdempotencyKey::new()),
    }));
    assert!(s.error.is_none(), "the banner speaks");
    assert!(s.lost.pending());
    // Enter now again on the same template: nothing is sent.
    s.enter_at(made.id, d(2026, 11, 2), None);
    assert!(s.enter_job.is_none() && s.entered.is_none());
    s.lost
        .recheck_answering(&mut s.lost_gate, cl_api::Recheck::Unknown);
    s.poll();
    s.enter_at(made.id, d(2026, 11, 2), None);
    assert!(s.entered.is_none(), "still waiting");
    // Found by its provenance: shown as entered, never posted again.
    let txn = TransactionId::parse("d0000000-0000-4000-8000-b00000000009").expect("t");
    s.lost.recheck_answering(
        &mut s.lost_gate,
        cl_api::Recheck::Committed {
            transaction: Some(txn),
        },
    );
    s.poll();
    assert_eq!(s.entered.map(|(t, _)| t), Some(txn));
    assert!(!s.lost.pending());
}
