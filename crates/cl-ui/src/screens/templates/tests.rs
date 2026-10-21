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
    s.enter_selected(None);
    assert!(s.closing_prompt.dialog_open(), "{:?}", s.error);
    assert!(link.bus().take(&mut watcher).is_empty(), "nothing posted");
    s.enter_selected(Some(Secret::new(DEMO_CLOSING_PASSWORD.to_string())));
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
