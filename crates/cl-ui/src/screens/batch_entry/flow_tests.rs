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
fn a_journal_entry_typed_by_name_tells_the_matched_accounts_registers() {
    let (mut s, link, _dir) = screen();
    s.add_section(SectionKind::JournalEntries);
    // Typed, not picked: the engine matches " 6100" by account number.
    fill(
        &mut s,
        1,
        &[
            (day(11, 2), " 6100", "25.00"),
            (day(11, 2), "1010", "-25.00"),
        ],
    );
    assert!(s.batch().sections[1].rows[0].account.is_none());
    let mut bus = link.bus().subscribe();
    assert!(s.save_all(SaveIntent::Save), "{:?}", s.state_json());
    let heard = link.bus().take(&mut bus);
    assert!(
        heard.ledger_touches(account_id(6100)),
        "the matched account"
    );
    assert!(heard.ledger_touches(account_id(1010)));
    assert!(
        !heard.ledger_touches(account_id(5000)),
        "only the accounts the batch used"
    );
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
fn a_refusal_goes_away_once_its_problem_is_fixed() {
    let (mut s, _link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[(day(11, 2), "6100", "10.00"), (day(11, 2), "9999", "20.00")],
    );
    assert!(!s.save_all(SaveIntent::Save));
    assert_eq!(s.state_json()["refused"], "engine_problems");
    // Nothing changed: the refusal stays.
    s.after_grid();
    assert_eq!(s.state_json()["refused"], "engine_problems");
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
    s.refresh_looks();
    s.after_grid();
    assert_eq!(
        s.state_json()["refused"],
        serde_json::Value::Null,
        "the fixed problem's refusal is gone at once"
    );
    // Checked and passing: Save all is on, nothing stale under the totals.
    s.start_check(validate::CheckScope::Everything);
    s.after_grid();
    let v = s.state_json();
    assert_eq!(v["can_save"], true, "{v:?}");
    assert_eq!(v["refused"], serde_json::Value::Null);
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
    assert_eq!(
        s.closing_prompt.ask_text("09/30/2026"),
        crate::i18n::t_count("batch_entry.closing.ask", 1, &[("date", "09/30/2026")]),
        "the dialog names the batch and its locked rows"
    );
    assert_eq!(saves(&link), (0, 0));
    assert!(!s.send_save(SaveIntent::Save, Some(Secret::new("wrong"))));
    assert_eq!(saves(&link).1, 0, "a wrong password posts nothing");
    assert!(s.closing_prompt.dialog_open(), "the dialog stays open");
    assert!(
        s.closing_prompt
            .ask_text("x")
            .starts_with("1 row in this batch"),
        "a wrong password keeps the batch's words"
    );
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
    /// 403 with `groupIndex`: the user may not create that group's kind.
    Forbidden(u32),
}

fn forbidden(group: u32) -> cl_api::ApiError {
    cl_api::ApiError::BatchGroup {
        group,
        problem: cl_api::BatchGroupProblem::NotAllowed,
    }
}

/// The demo engine with a script for saves; every key a save used is kept.
#[derive(Default)]
struct Scripted {
    steps: std::collections::VecDeque<SaveStep>,
    keys: Arc<std::sync::Mutex<Vec<String>>>,
    /// Rows sent to validate, over all calls.
    checked_rows: Arc<std::sync::atomic::AtomicUsize>,
    /// Validate refuses this group (403 with `groupIndex`).
    validate_forbids: Option<u32>,
}

impl BatchEngine for Scripted {
    fn validate(
        &mut self,
        link: &CompanyLink,
        groups: &[cl_api::BatchGroup],
    ) -> Result<cl_api::BatchValidation, cl_api::ApiError> {
        let n: usize = groups.iter().map(|g| g.rows.len()).sum();
        self.checked_rows
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        if let Some(g) = self.validate_forbids {
            return Err(forbidden(g));
        }
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
            SaveStep::Forbidden(g) => Err(forbidden(g)),
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
        ..Scripted::default()
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
fn a_section_the_user_may_not_enter_is_named_and_shown() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let engine = Scripted {
        steps: [SaveStep::Forbidden(1)].into(),
        ..Scripted::default()
    };
    let mut s =
        BulkEntryScreen::with_local(Arc::clone(&link), None, day(11, 4), Some(Box::new(engine)));
    fill(&mut s, 0, &[(day(11, 2), "6100", "10.00")]);
    s.add_section(SectionKind::Deposits);
    fill(&mut s, 1, &[(day(11, 3), "4000", "20.00")]);
    s.select_section(0);
    assert!(!s.save_all(SaveIntent::Save));
    assert_eq!(saves(&link).1, 0);
    let v = s.state_json();
    assert_eq!(v["section_refused"]["section"], 1);
    assert_eq!(v["section_refused"]["why"], "not_allowed");
    assert_eq!(v["section"], 1, "the refused section is shown");
    let text = s.section_refused_text().expect("text");
    assert!(
        text.contains("Deposits") && text.contains("section 2"),
        "{text}"
    );
    assert!(s.key.is_none(), "a refusal gets a fresh key next time");
    // It stays through a frame; an edit to the batch clears it.
    let ctx = eframe::egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |ui| {
        crate::windows::Screen::ui(&mut s, ui);
    });
    out.textures_delta.clear();
    assert_eq!(s.state_json()["section_refused"]["section"], 1);
    s.grid
        .edit_step(|b| b.sections[1].rows[0].memo = "Edited".into());
    s.after_grid();
    assert_eq!(s.state_json()["section_refused"], serde_json::Value::Null);
}

#[test]
fn a_section_refused_by_validate_is_named_too() {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let engine = Scripted {
        validate_forbids: Some(0),
        ..Scripted::default()
    };
    let mut s =
        BulkEntryScreen::with_local(Arc::clone(&link), None, day(11, 4), Some(Box::new(engine)));
    fill(&mut s, 0, &[(day(11, 2), "6100", "10.00")]);
    assert!(!s.save_all(SaveIntent::Save));
    let v = s.state_json();
    assert_eq!(v["section_refused"]["section"], 0);
    assert_eq!(v["blocker"], "unchecked");
    assert_eq!(saves(&link).0, 0, "no save was sent");
}

fn counted() -> (
    BulkEntryScreen,
    Arc<CompanyLink>,
    Arc<std::sync::atomic::AtomicUsize>,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = Scripted {
        checked_rows: Arc::clone(&counter),
        ..Scripted::default()
    };
    let s =
        BulkEntryScreen::with_local(Arc::clone(&link), None, day(11, 4), Some(Box::new(engine)));
    (s, link, counter, dir)
}

fn sent(c: &std::sync::atomic::AtomicUsize) -> usize {
    c.load(std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn check_all_rows_sends_every_row_again() {
    use super::validate::CheckScope;
    let (mut s, _link, counter, _dir) = counted();
    fill(
        &mut s,
        0,
        &[(day(11, 2), "6100", "10.00"), (day(11, 3), "6100", "5.00")],
    );
    assert!(s.start_check(CheckScope::Unchecked));
    assert_eq!(sent(&counter), 2);
    assert_eq!(s.state_json()["can_save"], true);
    assert!(
        !s.start_check(CheckScope::Unchecked),
        "Save all sends only rows without an answer"
    );
    assert!(s.start_check(CheckScope::Everything));
    assert_eq!(sent(&counter), 4, "Check all rows sends every row again");
    assert_eq!(s.state_json()["can_save"], true);
}

#[test]
fn new_lists_or_a_new_closing_date_forget_the_engine_answers() {
    use super::validate::CheckScope;
    let (mut s, link, _counter, _dir) = counted();
    fill(&mut s, 0, &[(day(11, 2), "6100", "10.00")]);
    assert!(s.start_check(CheckScope::Unchecked));
    assert_eq!(s.state_json()["can_save"], true);

    // The lists are read again (a save in another window).
    s.books.start(&link, None);
    s.poll();
    assert_eq!(s.state_json()["blocker"], "unchecked");
    assert!(s.changed_at.is_some(), "the rows are checked again soon");
    assert!(s.start_check(CheckScope::Unchecked));
    assert_eq!(s.state_json()["can_save"], true);

    // Only the closing date moved.
    let before = s.books.closing;
    s.books.closing = Some(day(11, 2));
    s.books_arrived(false, before);
    assert_eq!(s.state_json()["blocker"], "unchecked");

    // The same lists and date: the answers stand.
    assert!(s.start_check(CheckScope::Unchecked));
    let now = s.books.closing;
    s.books_arrived(false, now);
    assert_eq!(s.state_json()["blocker"], serde_json::Value::Null);
}

#[test]
fn an_answer_from_before_the_lists_changed_is_dropped() {
    let (mut s, link, _counter, _dir) = counted();
    fill(&mut s, 0, &[(day(11, 2), "6100", "10.00")]);
    let out = s.batch().outgoing(None, None);
    let groups = wire::groups(&out, None).expect("groups");
    let answer = {
        let d = link.demo_state().expect("demo");
        crate::company::demo::batch::validate(&d, &groups).expect("validate")
    };
    assert!(answer.rows[0].ok);
    s.checks.begin(&out);
    s.checking = Some(Checking {
        out,
        task: Task::ready(answer),
        epoch: s.check_epoch,
    });
    s.forget_engine_results();
    s.poll_checking();
    assert!(s.checking.is_none());
    assert_eq!(
        s.state_json()["blocker"],
        "unchecked",
        "the old answer did not land"
    );
}

/// Opens the mapping panel on a paste and reads it as the engine would,
/// returning the rows as a pending import (the live engine's case).
fn import_in_flight(s: &mut BulkEntryScreen) {
    s.open_mapping(
        "Date\tAccount\tAmount\n11/02/2026\t6100\t12.00\n",
        paste::Origin::Paste {
            at: None,
            column: 0,
        },
    );
    let panel = s.mapping.as_ref().expect("panel");
    let rows = crate::company::demo::batch::parse_csv(
        wire::batch_kind(panel.kind),
        &panel.bytes(),
        &wire::column_mapping(&panel.mapping),
    )
    .expect("parsed");
    s.parse_task = Some(Task::ready(rows));
}

#[test]
fn the_type_is_locked_while_an_import_is_read() {
    let (mut s, _link, _dir) = screen();
    import_in_flight(&mut s);
    s.set_kind(0, SectionKind::Deposits);
    assert_eq!(s.batch().sections[0].kind, SectionKind::Checks);
    s.poll_parse();
    assert_eq!(s.batch().filled_count(), 1, "the rows came in");
    assert_eq!(s.state_json()["last_action"], "pasted");
}

#[test]
fn rows_read_for_another_type_are_dropped_with_a_notice() {
    let (mut s, _link, _dir) = screen();
    import_in_flight(&mut s);
    // The type changes underneath (an undo, say).
    s.grid
        .edit_step(|b| b.sections[0] = Section::new(SectionKind::JournalEntries, None));
    s.poll_parse();
    assert_eq!(s.batch().filled_count(), 0, "nothing landed");
    let v = s.state_json();
    assert_eq!(v["notice"], "batch_entry.import.kind_changed");
    assert_eq!(v["mapping"], serde_json::Value::Null);
}

#[test]
fn a_pasted_calculation_is_not_worked_out() {
    let (mut s, _link, _dir) = screen();
    s.open_mapping(
        "Date\tAccount\tAmount\n11/02/2026\t6100\t100/4\n",
        paste::Origin::Paste {
            at: None,
            column: 0,
        },
    );
    s.apply_mapping();
    let rows: Vec<&RowDraft> = s.batch().sections[0].filled().collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].amount, None, "100/4 is not 25.00");
    s.cursor_row = None;
    s.refresh_looks();
    let v = s.state_json();
    assert_eq!(v["problems"], 1, "the row needs attention");
    assert_eq!(v["blocker"], "local_problems");
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
    // The row's only problem is the account the suggestion would fill: it
    // still counts, but it looks suggested and says "Press Tab to accept".
    s.cursor_row = None;
    s.touched.insert(key);
    s.refresh_looks();
    let look = s
        .grid
        .adapter()
        .looks
        .get(&key)
        .cloned()
        .unwrap_or_default();
    assert!(look.error && look.fillable, "{look:?}");
    match &look.note {
        Some(grid::Note::Suggestion(t)) => assert!(t.contains("Press Tab to accept"), "{t}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.problem_rows().len(), 1);
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
    s.refresh_looks();
    assert!(
        s.grid.adapter().looks.get(&key).is_none_or(|l| !l.fillable),
        "accepted: nothing left to fill"
    );
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

#[test]
fn every_row_not_in_the_lists_gets_its_caption() {
    let (mut s, _link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[
            (day(11, 2), "6100", "10.00"),
            (day(11, 2), "9999", "20.00"),
            (day(11, 3), "9998", "30.00"),
        ],
    );
    s.cursor_row = None;
    s.refresh_looks();
    let before = s.grid.adapter().looks_gen;
    s.start_check(validate::CheckScope::Everything);
    s.refresh_looks();
    // The engine's answer changed no row, yet the grid re-measures the
    // rows that now have a caption (row 3's was missing, P15A verify).
    assert_ne!(s.grid.adapter().looks_gen, before);
    let gen_now = s.grid.adapter().looks_gen;
    s.refresh_looks();
    assert_eq!(
        s.grid.adapter().looks_gen,
        gen_now,
        "same looks, same revision"
    );
    let rows: Vec<_> = s.batch().sections[0].rows.iter().map(|r| r.key).collect();
    for k in &rows[1..3] {
        let look = s.grid.adapter().looks.get(k).cloned().unwrap_or_default();
        assert!(look.error, "{look:?}");
        assert!(
            matches!(look.note, Some(grid::Note::Problem(t)) if t.0 == "batch_entry.problem.not_in_lists"),
            "{look:?}"
        );
    }
}

#[test]
fn a_row_left_after_the_timer_ran_is_checked_without_another_edit() {
    let (mut s, _link, _dir) = screen();
    fill(
        &mut s,
        0,
        &[(day(11, 2), "6100", "10.00"), (day(11, 2), "9999", "20.00")],
    );
    s.after_grid();
    let k = s.batch().sections[0].rows[1].key;
    // The timer ran while the cursor was still on the row: nothing was
    // sent for it, and no edit follows.
    s.touched.clear();
    s.changed_at = None;
    s.cursor_row = Some(k);
    s.mark_cursor_row_left();
    assert!(s.changed_at.is_some(), "leaving the row starts the timer");
    s.changed_at = Some(Instant::now() - CHECK_AFTER);
    s.after_grid();
    s.refresh_looks();
    let look = s.grid.adapter().looks.get(&k).cloned().unwrap_or_default();
    assert!(
        matches!(look.note, Some(grid::Note::Problem(t)) if t.0 == "batch_entry.problem.not_in_lists"),
        "{look:?}"
    );
    // Leaving a row that has its answer starts nothing.
    s.changed_at = None;
    s.cursor_row = Some(k);
    s.touched.remove(&k);
    s.mark_cursor_row_left();
    assert!(s.changed_at.is_none());
}
