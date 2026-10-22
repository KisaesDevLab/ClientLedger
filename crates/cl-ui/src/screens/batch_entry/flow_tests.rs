//! Bulk Entry against the demo engine: Save all, refusals, the closed
//! period, paste and suggestions (plan §6).

use std::sync::Arc;

use cl_core::{Money, Secret};
use cl_model::forms::SaveIntent;
use cl_model::forms::batch::{RowDraft, SectionKind};

use super::*;
use crate::company::demo::account_id;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn screen() -> (BulkEntryScreen, Arc<CompanyLink>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = BulkEntryScreen::new(Arc::clone(&link), None, day(11, 4));
    (s, link, dir)
}

/// (date, account number or typed text, amount; `-x` is a credit on a
/// journal entry row).
type Row<'a> = (CalendarDate, &'a str, &'a str);

/// Fills section `i` with rows as the owner would type them.
fn fill(s: &mut BulkEntryScreen, i: usize, rows: &[Row<'_>]) {
    let kind = s.batch().sections[i].kind;
    let today = s.today;
    let highest = s.grid.adapter().highest.clone();
    let mut keys = std::mem::take(&mut s.grid.adapter_mut().keys);
    s.grid.edit_step(|b| {
        let sec = &mut b.sections[i];
        sec.rows.retain(|r| !r.is_blank());
        for (date, account, amount) in rows {
            let mut r = RowDraft::new(keys.next_key());
            r.date = Some(*date);
            match account.parse::<u32>() {
                Ok(n) if (1000..9000).contains(&n) => r.account = Some(account_id(n)),
                _ => r.account_text = (*account).to_string(),
            }
            if kind.is_journal() {
                if let Some(c) = amount.strip_prefix('-') {
                    r.credit = Some(m(c));
                } else {
                    r.debit = Some(m(amount));
                }
                r.number = "J1".into();
            } else {
                r.amount = Some(m(amount));
            }
            r.memo = "Scenario".into();
            sec.rows.push(r);
        }
        grid::ensure_entry(b, i, &mut keys, today, &highest);
    });
    s.grid.adapter_mut().keys = keys;
}

fn saves(link: &CompanyLink) -> (u32, usize) {
    let d = link.demo_state().expect("demo");
    (d.batch.saves, d.batch.posted.len())
}

#[test]
fn mixed_checks_and_deposits_save_in_one_call_and_tell_the_registers() {
    let (mut s, link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[
            (day(11, 2), "6100", "12.35"),
            (day(11, 3), "6100", "0.05"),
            (day(11, 3), "5000", "100.00"),
        ],
    );
    s.add_section(SectionKind::Deposits);
    assert_eq!(s.selected(), 1);
    assert_eq!(s.batch().sections[1].context, Some(account_id(1010)));
    fill(
        &mut s,
        1,
        &[(day(11, 3), "4000", "250.00"), (day(11, 4), "4000", "0.10")],
    );
    assert_eq!(s.state_json()["blocker"], "unchecked");
    let mut bus = link.bus().subscribe();
    assert!(s.save_all(SaveIntent::Save), "{:?}", s.state_json());
    assert_eq!(saves(&link), (1, 1), "one call, one batch");
    assert_eq!(s.saved_count, 5);
    let v = s.state_json();
    assert_eq!(v["summary"]["rows"], 5);
    assert_eq!(v["summary"]["totals"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["rows"], 0, "a fresh batch");
    assert!(!s.dirty());
    assert!(link.bus().take(&mut bus).ledger_touches(account_id(1010)));
    let sum = s.summary.as_ref().expect("summary");
    assert!(
        sum.totals
            .contains(&(account_id(1010), SectionKind::Checks, m("112.40")))
    );
    assert!(
        sum.totals
            .contains(&(account_id(1010), SectionKind::Deposits, m("250.10")))
    );
    // The new batch's entry row is numbered after the saved checks.
    let next = &s.batch().sections[0].rows[0].number;
    assert_ne!(next, "");
}

#[test]
fn a_row_not_in_the_lists_blocks_save_all_and_nothing_is_sent() {
    let (mut s, link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[(day(11, 2), "6100", "10.00"), (day(11, 2), "9999", "20.00")],
    );
    assert!(!s.save_all(SaveIntent::Save));
    let v = s.state_json();
    assert_eq!(v["can_save"], false);
    assert_eq!(v["blocker"], "engine_problems");
    assert_eq!(v["refused"], "engine_problems");
    assert_eq!(v["problems"], 1);
    assert_eq!(v["saved_count"], 0);
    assert_eq!(saves(&link), (0, 0), "nothing reached the save");
    assert_eq!(v["grid"]["row"], 1, "the cursor is on the bad row");
    assert_eq!(v["grid"]["column"], "account");
    let key = s.batch().sections[0].rows[1].key;
    s.grid.edit_step(|b| {
        let r = b.sections[0]
            .rows
            .iter_mut()
            .find(|r| r.key == key)
            .expect("row");
        r.account_text.clear();
        r.account = Some(account_id(5000));
    });
    assert!(s.save_all(SaveIntent::SaveAndNew));
    assert_eq!(saves(&link), (1, 1));
    assert!(!s.summary_open, "Save all & New goes on with a fresh batch");
}

#[test]
fn local_problems_block_and_f8_walks_them_across_sections() {
    let (mut s, _link, _dir) = screen();
    fill(&mut s, 0, &[(day(11, 2), "6100", "10.00")]);
    s.grid
        .edit_step(|b| b.sections[0].rows[0].amount = Some(m("-1")));
    s.add_section(SectionKind::JournalEntries);
    fill(
        &mut s,
        1,
        &[
            (day(11, 2), "6100", "100.00"),
            (day(11, 2), "1010", "-90.00"),
        ],
    );
    s.cursor_row = None;
    s.refresh_looks();
    let v = s.state_json();
    assert_eq!(v["blocker"], "local_problems");
    assert_eq!(v["unbalanced"], serde_json::json!(["10.00"]));
    assert_eq!(v["problems"], 3);
    s.go_to_problem(true);
    let first = s.selected();
    let mut moved = false;
    for _ in 0..3 {
        s.go_to_problem(true);
        moved |= s.selected() != first;
    }
    assert!(moved, "F8 crossed into the other section");
    s.remove_section(1, false);
    assert_eq!(s.state_json()["confirm_remove"], true, "it has rows: ask");
    s.remove_section(1, true);
    assert_eq!(s.state_json()["sections"], 1);
}

#[test]
fn a_closed_period_row_asks_once_for_the_password() {
    let (mut s, link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[(day(9, 15), "6100", "10.00"), (day(11, 2), "6100", "5.00")],
    );
    assert_eq!(s.state_json()["locked_rows"], 1);
    assert!(!s.save_all(SaveIntent::Save));
    assert_eq!(s.state_json()["closing"], "asking");
    assert_eq!(saves(&link), (0, 0));
    assert!(!s.send_save(SaveIntent::Save, Some(Secret::new("wrong"))));
    assert_eq!(saves(&link).1, 0, "a wrong password posts nothing");
    assert!(s.closing_prompt.dialog_open(), "the dialog stays open");
    assert!(s.send_save(SaveIntent::Save, Some(Secret::new(DEMO_CLOSING_PASSWORD))));
    assert_eq!(saves(&link).1, 1);
    assert_eq!(s.saved_count, 2);
}

#[test]
fn the_same_content_keeps_its_key_and_a_change_gets_a_new_one() {
    let (mut s, _link, _dir) = screen();
    let a = s.key_for(7);
    assert_eq!(s.key_for(7), a);
    assert_ne!(s.key_for(8), a);
}

/// What the scripted engine does with the next save.
#[derive(Clone, Copy, Debug)]
enum SaveStep {
    /// The save posts, but its reply is lost (an unknown outcome).
    LoseReply,
    /// 409 `BATCH_BUSY`: the first save still holds the company's lock.
    Busy,
    /// The demo engine answers (a replay for a key it has seen).
    Answer,
}

/// The demo engine with a script for saves; every key a save used is kept.
struct Scripted {
    steps: std::collections::VecDeque<SaveStep>,
    keys: Arc<std::sync::Mutex<Vec<String>>>,
}

impl BatchEngine for Scripted {
    fn validate(
        &mut self,
        link: &CompanyLink,
        groups: &[cl_api::BatchGroup],
    ) -> Result<cl_api::BatchValidation, cl_api::ApiError> {
        DemoBatchEngine.validate(link, groups)
    }

    fn save(
        &mut self,
        link: &CompanyLink,
        groups: &[cl_api::BatchGroup],
        key: &cl_api::IdempotencyKey,
        add_new_names: bool,
    ) -> Result<cl_api::BatchSaved, cl_api::ApiError> {
        self.keys.lock().expect("keys").push(key.as_header());
        match self.steps.pop_front().unwrap_or(SaveStep::Answer) {
            SaveStep::LoseReply => {
                DemoBatchEngine.save(link, groups, key, add_new_names)?;
                Err(cl_api::ApiError::OutcomeUnknown {
                    request_id: cl_core::ids::RequestId::parse(
                        "00000000-0000-4000-8000-000000000001",
                    )
                    .expect("id"),
                    idempotency_key: Some(key.clone()),
                })
            }
            SaveStep::Busy => Err(cl_api::ApiError::Conflict {
                reason: cl_api::ConflictReason::Busy,
                current_version: None,
                code: cl_api::ErrorCode::parse("BATCH_BUSY"),
                message: cl_api::UserText::default(),
            }),
            SaveStep::Answer => DemoBatchEngine.save(link, groups, key, add_new_names),
        }
    }

    fn suggest_account(
        &mut self,
        link: &CompanyLink,
        contact: cl_core::ids::ContactId,
    ) -> Result<Option<cl_api::SuggestedAccount>, cl_api::ApiError> {
        DemoBatchEngine.suggest_account(link, contact)
    }

    fn parse_csv(
        &mut self,
        link: &CompanyLink,
        kind: cl_api::BatchKind,
        bytes: &[u8],
        mapping: &cl_api::ColumnMapping,
    ) -> Result<cl_api::ParsedRows, cl_api::ApiError> {
        DemoBatchEngine.parse_csv(link, kind, bytes, mapping)
    }
}

#[test]
fn a_lost_reply_then_busy_retries_with_the_same_key_and_posts_once() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let keys = Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = Scripted {
        steps: [SaveStep::LoseReply, SaveStep::Busy, SaveStep::Answer].into(),
        keys: Arc::clone(&keys),
    };
    let mut s =
        BulkEntryScreen::with_local(Arc::clone(&link), None, day(11, 4), Some(Box::new(engine)));
    fill(&mut s, 0, &[(day(11, 2), "6100", "12.35")]);

    // 1. The reply is lost: the books may have the batch.
    assert!(!s.save_all(SaveIntent::Save));
    assert_eq!(s.state_json()["notice"], "batch_entry.save.unconfirmed");
    assert!(s.key.is_some(), "an unknown outcome keeps the key");
    assert_eq!(saves(&link).1, 1, "it did post");

    // 2. The retry meets BATCH_BUSY: "try again soon", same key kept.
    assert!(!s.save_all(SaveIntent::Save));
    let v = s.state_json();
    assert_eq!(v["notice"], "batch_entry.save.busy");
    assert_eq!(v["error"], true);
    assert!(s.key.is_some(), "busy keeps the key");
    let msg = cl_core::OwnerFacing::owner_message(s.error.as_ref().expect("error"));
    assert_eq!(msg.suggestion_key, Some("fix.api.try_again_soon"));

    // 3. The next retry replays the first save: nothing posts twice.
    assert!(s.save_all(SaveIntent::Save), "{:?}", s.state_json());
    assert_eq!(saves(&link).1, 1, "one batch in the books");
    assert_eq!(s.saved_count, 1);
    let keys = keys.lock().expect("keys");
    assert_eq!(keys.len(), 3);
    assert!(keys.iter().all(|k| *k == keys[0]), "every try used one key");
}

#[test]
fn a_pasted_block_maps_and_rounds_three_decimal_amounts() {
    let (mut s, _link, _dir) = screen();
    s.open_mapping(
        "Date\tPayee\tAccount\tAmount\tMemo\n11/02/2026\tHollister Fuel Stop\t6100\t12.345\tFuel\n11/03/2026\tNew Vendor\t6100\t7\tx\n",
        paste::Origin::Paste {
            at: None,
            column: 0,
        },
    );
    let v = s.state_json();
    assert_eq!(v["mapping"]["first_row_headers"], true);
    assert_eq!(v["mapping"]["preview_rows"], 2);
    s.mapping.as_mut().expect("panel").remember = true;
    s.apply_mapping();
    assert_eq!(s.state_json()["mapping"], serde_json::Value::Null);
    let rows: Vec<&RowDraft> = s.batch().sections[0].filled().collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].amount, Some(m("12.35")), "Q-232");
    assert!(rows[0].contact.is_some(), "a name in the lists is picked");
    assert_eq!(rows[1].contact_text, "New Vendor");
    assert_eq!(s.pasted_rows, 2);
    assert_eq!(s.memory().entries.len(), 1, "remembered");
    // Undo takes the whole paste back.
    s.form_command(CommandId::new("edit.undo"));
    assert_eq!(s.batch().filled_count(), 0);
}

#[test]
fn a_name_suggestion_shows_as_a_ghost_until_accepted() {
    let (mut s, link, _dir) = screen();
    let fuel = {
        let mut d = link.demo_state().expect("demo");
        d.contacts_mut()
            .iter()
            .find(|c| c.fields.display_name == "Hollister Fuel Stop")
            .map(|c| c.id)
            .expect("vendor")
    };
    fill(&mut s, 0, &[(day(11, 2), "x", "10.00")]);
    let key = s.batch().sections[0].rows[0].key;
    s.grid.edit_step(|b| {
        let r = &mut b.sections[0].rows[0];
        r.account_text.clear();
        r.contact = Some(fuel);
    });
    s.poll();
    assert_eq!(s.open_suggestions(), vec![(key, account_id(6100))]);
    assert!(s.suggestion_caption(key).is_some());
    assert_eq!(
        s.batch().sections[0].rows[0].account,
        None,
        "never set unasked"
    );
    assert_eq!(s.accept_suggestions(None), 1);
    assert_eq!(
        s.batch().sections[0].rows[0].account,
        Some(account_id(6100))
    );
    assert_eq!(s.state_json()["suggest"]["accepted"], 1);
}

#[test]
fn the_window_draws_in_a_frame() {
    let (mut s, _link, _dir) = screen();
    fill(&mut s, 0, &[(day(11, 2), "9999", "10.00")]);
    let ctx = eframe::egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Dark);
    crate::fonts::install(&ctx);
    for _ in 0..2 {
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
            crate::windows::Screen::ui(&mut s, ui);
        });
        out.textures_delta.clear();
    }
    assert_eq!(s.state_json()["screen"], "batch_entry");
}
