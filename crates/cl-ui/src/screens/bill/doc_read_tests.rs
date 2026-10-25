//! Tests for reading a bill with AI and the ghost line accounts (P17 WP7,
//! plan §6 AC3). The demo engine answers with a canned reading; no AI
//! provider is ever called.

use std::sync::Arc;

use cl_api::{ApiError, AttachmentId, BillReading};
use cl_core::ids::{AccountId, ContactId};
use cl_core::{CalendarDate, Money, MoneyStyle};
use cl_model::forms::SaveIntent;
use cl_model::forms::bill::BillProblem;
use cl_model::forms::doc_read::{Confidence, ReadField, ReadTarget};
use cl_model::forms::lines::FormLine;
use eframe::egui;

use super::*;
use crate::company::CompanyLink;
use crate::company::demo::bills::{STORY_PDF, STORY_PDF_NAME, STORY_VENDOR};
use crate::screens::bill::EnterBillScreen;
use crate::windows::Screen as _;

fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    (dir, link)
}

fn today() -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, 4).expect("date")
}

fn day(d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, d).expect("date")
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

/// A new bill with the story invoice attached (not read).
fn with_document(link: &Arc<CompanyLink>) -> EnterBillScreen {
    let mut s = screen(link);
    s.upload_bytes(STORY_PDF_NAME, STORY_PDF.to_vec());
    assert!(s.attach.shown.is_some(), "the document is shown");
    s
}

fn input(s: &EnterBillScreen) -> ReadInput<'_> {
    ReadInput {
        link: &s.link,
        file_name: Some(STORY_PDF_NAME),
        attachment: s.attach.shown,
        from_inbox: false,
        tenant: None,
        money_style: MoneyStyle::default(),
    }
}

/// Read with AI (the keymap command), then one frame's polling.
fn read(s: &mut EnterBillScreen) {
    assert!(s.form_command(cl_model::CommandId::new("bill.read_document")));
    s.poll_read();
    s.track_read();
}

fn frames(s: &mut EnterBillScreen, n: usize) {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    for _ in 0..n {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
    }
}

/// Three frames at `size`, returning every AccessKit node drawn.
fn nodes(
    s: &mut EnterBillScreen,
    size: (f32, f32),
    theme: crate::theme::ThemeChoice,
) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, theme);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut all = Vec::new();
    for _ in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(size.0, size.1))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        all.extend(
            out.platform_output
                .accesskit_update
                .map(|u| u.nodes)
                .unwrap_or_default(),
        );
    }
    all
}

/// Every interactive widget of Enter Bill has an accessible name: a new
/// bill, a read document (banner, marks, Accept), the Items tab, and a
/// vendor credit, at both scenario sizes in light and dark.
#[test]
fn every_enter_bill_widget_has_an_accessible_name() {
    use crate::theme::ThemeChoice;
    for theme in [ThemeChoice::Light, ThemeChoice::Dark] {
        for size in [(1440.0, 940.0), (960.0, 640.0)] {
            let (_d, link) = link();
            let mut s = screen(&link);
            let blank = nodes(&mut s, size, theme);
            assert!(blank.len() > 20, "drawn");
            let mut s = with_document(&link);
            read(&mut s);
            let read_nodes = nodes(&mut s, size, theme);
            s.tab = crate::screens::bill::Tab::Items;
            let items = nodes(&mut s, size, theme);
            s.header.edit_step(|h| h.kind = cl_model::forms::bill::BillKind::Credit);
            let credit = nodes(&mut s, size, theme);
            for (what, n) in [
                ("new", blank),
                ("read", read_nodes),
                ("items", items),
                ("credit", credit),
            ] {
                let unnamed = crate::screens::unnamed_widgets(&n);
                assert!(
                    unnamed.is_empty(),
                    "{what} {size:?} {theme:?}: unnamed widgets: {unnamed:?}"
                );
            }
        }
    }
}

fn reading(body: &str) -> BillReading {
    cl_api::reading_from_json(body.as_bytes()).expect("reading")
}

#[test]
fn ai_off_or_reading_off_offers_no_button_sends_nothing_and_manual_entry_saves() {
    for (enabled, reading_on) in [(false, true), (true, false), (false, false)] {
        let (_d, link) = link();
        {
            let mut demo = link.demo_state().expect("demo");
            demo.ai.enabled = enabled;
            demo.ai.reading = reading_on;
        }
        let mut s = with_document(&link);
        let i = input(&s);
        let mut st = ReadState::default();
        assert!(
            !st.available(&i),
            "no Read with AI ({enabled}, {reading_on})"
        );
        // The command (or a stale click) sends nothing.
        read(&mut s);
        assert_eq!(s.read.sent, 0);
        assert_eq!(s.state()["read"]["requests"], 0);
        assert_eq!(s.read.problem, Some("bill.read.problem.off"));
        assert!(s.header.current().read.is_empty());
        // Manual entry is untouched: the whole bill saves.
        let v = contact(&s, STORY_VENDOR);
        s.pick_vendor(v);
        let rental = account(&s, "6210");
        s.header
            .edit_step(|h| h.set_amount_due(Some(money("25.00"))));
        let key = s.expenses.form_mut().adapter_mut().keys.next_key();
        let entry = FormLine::blank(s.expenses.form_mut().adapter_mut().keys.next_key());
        s.expenses.form_mut().edit_step(|l| {
            l.lines = vec![
                FormLine {
                    account: Some(rental),
                    amount: Some(money("25.00")),
                    ..FormLine::blank(key)
                },
                entry,
            ];
        });
        let before = writes(&link);
        assert!(s.start_save(SaveIntent::SaveAndNew, None));
        assert_eq!(writes(&link), before + 1, "the manual bill was sent");
        assert_eq!(s.state()["saved_count"], 1);
    }
}

#[test]
fn the_engine_switches_need_ai_consent_reading_and_a_provider() {
    let config: cl_api::AiConfig = serde_json::from_value(serde_json::json!({
        "isEnabled": true, "fallbackChain": ["anthropic"], "ocrProvider": "",
    }))
    .expect("config");
    let consent: cl_api::CompanyConsent = serde_json::from_value(serde_json::json!({
        "companyId": "d0000000-0000-4000-8000-0000000000c1",
        "systemVersion": 2, "acceptedVersion": 2, "acceptedAt": "2026-10-01T08:05:00.000Z",
        "aiEnabled": true, "enabledTasks": {"receipt_ocr": true}, "isStale": false,
    }))
    .expect("consent");
    let s = ReadSwitches::from_engine(Some(&config), Some(&consent));
    assert!(s.open());
    assert_eq!(s.provider, Some(AiProvider::Anthropic));
    assert!(s.provider.is_some_and(AiProvider::is_cloud));
    let mut stale = consent.clone();
    stale.is_stale = true;
    assert!(!ReadSwitches::from_engine(Some(&config), Some(&stale)).open());
    let mut off = consent.clone();
    off.enabled_tasks.receipt_ocr = Some(false);
    assert!(!ReadSwitches::from_engine(Some(&config), Some(&off)).open());
    let mut unknown = config.clone();
    unknown.fallback_chain = vec!["somebody".into()];
    assert!(!ReadSwitches::from_engine(Some(&unknown), Some(&consent)).open());
    assert!(!ReadSwitches::from_engine(None, Some(&consent)).open());
    assert!(!ReadSwitches::from_engine(Some(&config), None).open());
    let gate = ReadSwitches::default().gate();
    assert!(!gate.is_open());
}

#[test]
fn a_reading_fills_empty_fields_as_read_and_save_is_refused_until_accepted() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    let input = input(&s);
    let mut probe = ReadState::default();
    assert!(probe.available(&input), "AI and reading are on in the demo");
    read(&mut s);
    assert_eq!(s.read.sent, 1, "one request, on the owner's click");
    let f = s.header.current().clone();
    assert_eq!(f.vendor, Some(contact(&s, STORY_VENDOR)));
    assert_eq!(
        f.date,
        Some(day(3)),
        "the document's date replaces the new bill's"
    );
    assert_eq!(f.due_date, Some(day(18)));
    assert_eq!(f.reference, "INV-55871");
    assert_eq!(f.amount_due, Some(money("1480.00")));
    assert_eq!(
        f.terms.as_deref(),
        Some("net_15"),
        "the vendor's terms come along"
    );
    let lines: Vec<_> = s.expenses.value().used().cloned().collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].amount, Some(money("1330.00")));
    assert_eq!(lines[1].amount, Some(money("150.00")));
    assert!(
        lines.iter().all(|l| l.account.is_none()),
        "accounts are ghosts only"
    );
    let st = s.state();
    assert_eq!(st["read"]["filled"], true);
    assert_eq!(st["read"]["unaccepted"], 7);
    assert_eq!(st["read"]["low"], 1, "line 2 was read with low confidence");
    assert_eq!(st["read"]["reconciled"], true);
    assert_eq!(st["read"]["subtotal_tax_ok"], true);
    assert_eq!(st["read"]["ghosts"]["open"], 2);
    assert_eq!(st["totals"], "balanced");
    assert_eq!(
        s.read_state_of(ReadTarget::Line(lines[1].key)),
        Some(ReadField::Read {
            level: Confidence::Low
        })
    );
    // Save is refused: nothing reaches the engine, the keyboard goes to
    // Accept.
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before);
    assert!(s.save.problems.contains(&BillProblem::ReadNotAccepted));
    s.poll_read();
    assert!(s.read.focus_accept);
    // Accept (Alt+A), then the ghosts with Accept / Tab.
    assert!(s.form_command(cl_model::CommandId::new("bill.accept_read")));
    s.track_read();
    assert_eq!(s.state()["read"]["unaccepted"], 0);
    assert!(!s.read.focus_accept);
    for l in &lines {
        assert!(s.expenses.accept_suggestion(l.key));
    }
    s.poll_read();
    assert_eq!(s.read.ghosts.accepted, 2);
    let rental = account(&s, "6210");
    assert!(s.expenses.value().used().all(|l| l.account == Some(rental)));
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before + 1);
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(
        s.state()["read"]["filled"],
        false,
        "a new bill starts unread"
    );
}

#[test]
fn editing_a_read_field_accepts_that_field_only() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    read(&mut s);
    s.header.edit(super::field::REFERENCE, |h| {
        h.reference = "INV-55871A".into();
    });
    s.track_read();
    assert_eq!(
        s.read_state_of(ReadTarget::Reference),
        Some(ReadField::Edited)
    );
    assert_eq!(s.read.unaccepted, 6);
    // A read line's description edited: its AI-read amount is NOT accepted
    // (non-negotiable 4); only the amount (or Accept) accepts it.
    let key = s.expenses.value().lines[0].key;
    s.expenses
        .form_mut()
        .edit_step(|v| v.lines[0].memo = "Excavator, two days".into());
    s.track_read();
    assert!(
        matches!(
            s.read_state_of(ReadTarget::Line(key)),
            Some(ReadField::Read { .. })
        ),
        "a description edit leaves the read amount unaccepted"
    );
    assert_eq!(s.read.unaccepted, 6);
    // A read line's amount typed over: that line is accepted.
    s.expenses
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(money("1331.00")));
    s.track_read();
    assert_eq!(
        s.read_state_of(ReadTarget::Line(key)),
        Some(ReadField::Edited)
    );
    assert_eq!(s.read.unaccepted, 5);
    // A deleted read line is forgotten.
    let second = s.expenses.value().lines[1].key;
    s.expenses
        .form_mut()
        .edit_step(|v| v.lines.retain(|l| l.key != second));
    s.track_read();
    assert_eq!(s.read_state_of(ReadTarget::Line(second)), None);
    assert_eq!(s.read.unaccepted, 4);
    assert_eq!(s.read.low, 0);
    // Still refused while the others wait.
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert!(s.save.problems.contains(&BillProblem::ReadNotAccepted));
}

#[test]
fn clear_removes_only_what_was_read_and_not_accepted() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    s.header.edit_step(|h| h.memo = "typed".into());
    read(&mut s);
    s.header
        .edit(super::field::REFERENCE, |h| h.reference = "MINE".into());
    s.track_read();
    s.clear_read();
    let f = s.header.current();
    assert_eq!(f.vendor, None);
    assert_eq!(f.amount_due, None);
    assert_eq!(f.due_date, None);
    assert!(!f.due_overridden);
    assert_eq!(f.date, Some(today()), "back to the bill's own date");
    assert_eq!(f.reference, "MINE", "the owner's edit stays");
    assert_eq!(f.memo, "typed");
    assert!(f.read.is_empty());
    assert_eq!(s.expenses.value().used().count(), 0);
    assert_eq!(s.expenses.value().lines.len(), 1, "the entry line");
    assert_eq!(s.state()["read"]["filled"], false);
    assert_eq!(s.state()["read"]["ghosts"]["open"], 0);
}

#[test]
fn what_the_owner_typed_is_never_replaced() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    s.header.edit_step(|h| {
        h.reference = "OWN-1".into();
        h.date = Some(day(1));
        h.set_amount_due(Some(money("9.99")));
    });
    read(&mut s);
    let f = s.header.current();
    assert_eq!(f.reference, "OWN-1");
    assert_eq!(f.date, Some(day(1)));
    assert_eq!(f.amount_due, Some(money("9.99")));
    assert_eq!(
        s.read_state_of(ReadTarget::Reference),
        None,
        "not marked: not read"
    );
    // The lines still disagree with the owner's amount: the lines-total
    // rule decides Save, as always.
    assert_eq!(s.state()["totals"], "over");
}

#[test]
fn figures_that_disagree_unknown_vendors_and_text_only_readings() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    s.fill_from_reading(&reading(
        r#"{"extraction":{"status":"ok","vendor":"Bluff City Hydraulics","total":"100.00",
        "subtotal":"90.00","tax":"9.00","lineItems":[{"description":"Hose","amount":"90.00"},
        {"description":"Fitting","amount":"one"}]}}"#,
    ));
    s.track_read();
    let st = s.state();
    assert_eq!(
        st["read"]["reconciled"],
        serde_json::Value::Null,
        "a line has no amount"
    );
    assert_eq!(st["read"]["subtotal_tax_ok"], false);
    assert_eq!(st["read"]["unknown_vendor"], true);
    assert_eq!(st["read"]["unreadable"], true);
    assert_eq!(
        s.header.current().vendor,
        None,
        "an unknown vendor is never made up"
    );
    let filled = s.read.filled.clone().expect("filled");
    assert_eq!(
        filled.unknown_vendor.as_deref(),
        Some("Bluff City Hydraulics")
    );
    assert!(!format!("{filled:?}").contains("Bluff"));
    // Line 2's amount was unreadable: blank and marked low.
    let second = s.expenses.value().lines[1].clone();
    assert_eq!(second.amount, None);
    assert_eq!(
        s.read_state_of(ReadTarget::Line(second.key)),
        Some(ReadField::Read {
            level: Confidence::Low
        })
    );

    // The lines read and the total disagree by exactly a cent.
    let (_d2, link2) = self::link();
    let mut s = with_document(&link2);
    s.fill_from_reading(&reading(
        r#"{"extraction":{"status":"ok","total":"100.01","lineItems":[
        {"description":"Hose","amount":"100.00"}]}}"#,
    ));
    let check = s.read.filled.as_ref().and_then(|f| f.check).expect("check");
    assert_eq!(check.total_ok, Some(false));
    assert_eq!(check.difference, Some(money("0.01")));
    assert_eq!(s.state()["read"]["reconciled"], false);

    // Text only: shown read-only, nothing filled.
    let (_d3, link3) = self::link();
    let mut s = with_document(&link3);
    s.read.job = Some(crate::company::Task::ready(Ok(reading(
        r#"{"extraction":{"status":"ocr_only","rawText":"PAID IN FULL"}}"#,
    ))));
    s.poll_read();
    assert_eq!(s.read.found_text.as_deref(), Some("PAID IN FULL"));
    assert_eq!(s.state()["read"]["ocr_only"], true);
    assert_eq!(s.state()["read"]["outcome"], "ocr_only");
    assert!(s.header.current().read.is_empty());
    assert_eq!(s.header.current().vendor, None);
    frames(&mut s, 2);
}

#[test]
fn a_failed_reading_is_a_plain_message_and_manual_entry_goes_on() {
    for (e, key) in [
        (ApiError::AiUnavailable, "bill.read.problem.off"),
        (
            ApiError::OfflineUnavailable {
                feature: Some("ai".into()),
            },
            "bill.read.problem.unreachable",
        ),
        (ApiError::Timeout, "bill.read.problem.unreachable"),
        (
            ApiError::EngineUnavailable { permanent: false },
            "bill.read.problem.unreachable",
        ),
        (ApiError::NotFound { code: None }, "bill.read.problem.gone"),
        (
            ApiError::RateLimited { retry_after: None },
            "bill.read.problem.busy",
        ),
        (ApiError::PayloadTooLarge, "bill.read.problem.failed"),
    ] {
        assert_eq!(problem_of(&e), key, "{e:?}");
        assert!(crate::i18n::has(key), "{key}");
        let (_d, link) = link();
        let mut s = with_document(&link);
        s.read.job = Some(crate::company::Task::ready(Err(e)));
        s.poll_read();
        assert_eq!(s.read.problem, Some(key));
        assert!(!s.read.busy());
        assert!(s.header.current().read.is_empty());
        assert_eq!(s.header.current().vendor, None);
    }
}

/// A scanned PDF (the engine's `PDF_RENDER_FAILED` / `PDF_RENDER_EMPTY`,
/// no GPL rasterizer is shipped) is a plain "not available yet" message,
/// never the engine's code or its generic failure.
#[test]
fn a_scanned_pdf_says_reading_it_is_not_available_yet() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let att = cl_api::AttachmentId::parse("5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
    let open = cl_api::BillReadGate {
        ai_enabled: true,
        bill_reading_on: true,
        provider_set: true,
    };
    for code in ["PDF_RENDER_FAILED", "PDF_RENDER_EMPTY"] {
        let e = rt.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(format!("/api/v1/bills/extract-from-attachment/{att}")))
                .respond_with(ResponseTemplate::new(422).set_body_raw(
                    format!(r#"{{"error":{{"message":"Failed to render PDF: spawn pdftoppm ENOENT","code":"{code}"}}}}"#),
                    "application/json",
                ))
                .mount(&server)
                .await;
            let session = cl_api::testing::stub_session(server.address().port());
            session
                .bill_reading()
                .extract(open, att, cl_api::IdempotencyKey::new())
                .await
                .expect_err("scanned")
        });
        let key = problem_of(&e);
        assert_eq!(key, "bill.read.problem.scanned_pdf", "{e:?}");
        let shown = crate::i18n::t(key);
        assert!(
            shown.contains("scanned PDFs isn't available yet"),
            "{shown}"
        );
        assert!(!shown.contains("PDF_RENDER"), "{shown}");
        let (_d, link) = link();
        let mut s = with_document(&link);
        s.read.job = Some(crate::company::Task::ready(Err(e)));
        s.poll_read();
        assert_eq!(s.read.problem, Some("bill.read.problem.scanned_pdf"));
        assert!(s.header.current().read.is_empty());
    }
}

#[test]
fn a_line_without_an_account_gets_a_ghost_that_only_tab_or_accept_sets() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let v = contact(&s, STORY_VENDOR);
    s.pick_vendor(v);
    let key = s.expenses.form_mut().adapter_mut().keys.next_key();
    let entry = FormLine::blank(s.expenses.form_mut().adapter_mut().keys.next_key());
    s.expenses.form_mut().edit_step(|l| {
        l.lines = vec![
            FormLine {
                memo: "Trencher, half day".into(),
                amount: Some(money("95.00")),
                ..FormLine::blank(key)
            },
            entry,
        ];
    });
    s.poll_read();
    assert_eq!(s.read.ghosts.open, 1, "the vendor's usual account");
    assert_eq!(s.state()["read"]["ghosts"]["ai"], false);
    let first = s.expenses.value().lines[0].clone();
    assert_eq!(first.account, None, "a ghost is never the value");
    // Saving refuses the line without an account.
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    // Asked once per line content.
    s.poll_read();
    assert_eq!(s.read.ghosts.asking.len(), 0);
    assert!(s.expenses.accept_suggestion(key));
    s.poll_read();
    assert_eq!(s.read.ghosts.accepted, 1);
    assert_eq!(
        s.expenses.value().lines[0].account,
        Some(account(&s, "6210"))
    );
    frames(&mut s, 2);
}

#[test]
fn the_demo_reads_only_the_story_document_and_logs_nothing_of_it() {
    let (_d, link) = link();
    let s = with_document(&link);
    let id = s.attach.shown.expect("shown");
    let r = demo_reading(&link, id).expect("reading");
    assert_eq!(r.status, cl_api::ReadStatus::Ok);
    let shown = format!("{r:?}");
    for canary in ["Midwest", "1480", "55871", "excavator"] {
        assert!(!shown.contains(canary), "{shown}");
    }
    let unknown = AttachmentId::parse("5c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
    assert!(demo_reading(&link, unknown).is_err());
    link.demo_state().expect("demo").ai.reading = false;
    assert_eq!(demo_reading(&link, id), Err(ApiError::AiUnavailable));
    let st = format!("{:?}", s.read);
    assert!(!st.contains("midwest"), "{st}");
}

#[test]
fn the_banner_marks_and_captions_draw_in_both_states() {
    let (_d, link) = link();
    let mut s = with_document(&link);
    frames(&mut s, 1);
    assert_eq!(s.state()["read"]["available"], true);
    read(&mut s);
    frames(&mut s, 3);
    assert_eq!(s.state()["read"]["unaccepted"], 7);
    s.accept_read();
    frames(&mut s, 2);
    assert_eq!(s.state()["read"]["unaccepted"], 0);
    // The low-confidence caption and the strings exist.
    for key in [
        "bill.read.mark",
        "bill.read.mark.low",
        "bill.read.banner",
        "bill.read.lines.low",
        "bill.read.accept",
        "bill.read.clear",
    ] {
        assert!(crate::i18n::has(key), "{key}");
    }
}
