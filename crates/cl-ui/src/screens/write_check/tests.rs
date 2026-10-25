//! Write Check screen tests (no engine): the demo stand-in feeds the
//! screen, and its write counter proves when nothing was sent.

use super::*;
use cl_api::SplitLine;
use cl_core::ids::ContactId;
use cl_model::forms::SaveIntent;
use cl_model::forms::check::{CheckedCheck, check};
use cl_model::forms::lines::{FormLine, PostingLine};

use crate::company::Task;
use crate::screens::company::form::SecretText;
use cl_grid::RowKey;

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

fn screen(link: &Arc<CompanyLink>) -> WriteCheckScreen {
    let mut s = WriteCheckScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn writes(link: &Arc<CompanyLink>) -> u32 {
    link.demo_state().expect("demo").checks.writes
}

fn account(s: &WriteCheckScreen, n: &str) -> AccountId {
    s.accounts
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(n))
        .map_or_else(|| panic!("{n}"), |a| a.id)
}

fn contact(s: &WriteCheckScreen, name: &str) -> ContactId {
    s.contacts
        .iter()
        .find(|c| c.fields.display_name == name)
        .map_or_else(|| panic!("{name}"), |c| c.id)
}

/// Ozark Feed & Seed, `amount`, and one line per `(account, amount)`.
fn fill(s: &mut WriteCheckScreen, amount: &str, lines: &[(&str, &str)]) {
    let payee = contact(s, "Ozark Feed & Seed");
    s.pick_payee(payee);
    let amount = money(amount);
    s.header.edit_step(|h| h.amount = Some(amount));
    let seeds: Vec<FormLine> = lines
        .iter()
        .map(|(a, m)| {
            let account = account(s, a);
            let key = s.lines.form_mut().adapter_mut().keys.next_key();
            FormLine {
                account: Some(account),
                amount: Some(money(m)),
                ..FormLine::blank(key)
            }
        })
        .collect();
    let entry = FormLine::blank(s.lines.form_mut().adapter_mut().keys.next_key());
    s.lines.form_mut().edit_step(|v| {
        v.lines = seeds;
        v.lines.push(entry);
    });
    s.sync_inputs();
}

#[test]
fn opens_on_the_bank_account_with_the_next_number_and_the_engine_balance() {
    let (_d, link) = link();
    let s = screen(&link);
    assert_eq!(s.bank(), Some(account(&s, "1010")));
    assert_eq!(
        s.header.current().number,
        NumberField::Proposed(4418),
        "the highest check in 1010 is 4417"
    );
    assert!(!s.is_dirty(), "the proposal is part of the blank form");
    assert_eq!(s.title(), "Write Check \u{b7} #4418");
    // The demo engine's account balance, as sent (the mockup figure).
    assert_eq!(s.balance, Some(money("84210.55")));
    let st = s.state();
    assert_eq!(st["screen"], "write_check");
    assert_eq!(st["number"], 4418);
    assert_eq!(st["number_proposed"], true);
    assert_eq!(st["ending_balance_shown"], true);
    assert_eq!(st["line_billable"], true);
    assert_eq!(s.company.name, "Ozark Ridge Landscaping LLC");
}

#[test]
fn the_demo_tab_is_the_mockup_draft_unsaved() {
    let (_d, link) = link();
    let s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today());
    assert!(s.is_dirty());
    assert_eq!(s.title(), "Write Check \u{b7} #4418");
    let f = s.compose();
    assert!(!f.print_later);
    assert_eq!(f.amount, Some(money("1286.40")));
    assert_eq!(f.lines.used().count(), 2);
    assert_eq!(
        f.totals_status(),
        Some(cl_model::forms::check::TotalsStatus::Balanced)
    );
    assert_eq!(s.state()["words"], true);
}

#[test]
fn lines_that_disagree_by_a_cent_save_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.24")]);
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before, "nothing reached the engine");
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    assert_eq!(s.state()["saved_count"], 0);
    assert!(s.is_dirty(), "the owner's entries stay");
    assert_eq!(s.state()["totals"], "remaining");
    // Over by a cent is refused too.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("444.26")));
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.state()["totals"], "over");
}

#[test]
fn a_fixed_problem_goes_away_before_the_next_save() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.24")]);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    // Still off by a different amount: the problem stays.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("444.20")));
    s.recheck_problems();
    assert_eq!(s.state()["save"]["refused"], "lines_disagree");
    assert!(matches!(
        s.save.problems.first(),
        Some(CheckProblem::LinesDisagree { difference }) if *difference == money("0.05")
    ));
    // The lines now equal the check amount: the stale message clears.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[1].amount = Some(money("444.25")));
    s.recheck_problems();
    assert_eq!(s.state()["save"]["refused"], Value::Null);
    assert_eq!(s.state()["save"]["problems"], 0);
    assert_eq!(s.state()["totals"], "balanced");
}

#[test]
fn save_and_new_posts_the_check_and_proposes_the_next_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let date = today();
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    assert_eq!(s.state()["totals"], "balanced");
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(s.state()["last_saved"]["number"], 4418);
    assert_eq!(s.state()["last_saved"]["print_status"], "hand_written");
    // The engine's balance after the save (84,210.55 − 1,286.40).
    assert_eq!(s.balance, Some(money("82924.15")));
    // A blank check on the same account and date with the next number.
    assert!(!s.is_dirty());
    assert_eq!(s.header.current().number, NumberField::Proposed(4419));
    assert_eq!(s.header.current().date, Some(date));
    assert_eq!(s.bank(), Some(account(&s, "1010")));
    assert!(s.compose().lines.used().next().is_none());
    // The GL effect in the demo ledger: the check's lines as typed.
    let mut demo = link.demo_state().expect("demo");
    let bank = s.bank().expect("bank");
    let saved = crate::company::demo::checks::list(&mut demo, bank)
        .into_iter()
        .find(|c| c.number == Some(4418))
        .expect("4418");
    let record = crate::company::demo::checks::get(&mut demo, saved.id)
        .expect("get")
        .record;
    let debits: Vec<Money> = record
        .txn
        .lines
        .iter()
        .filter(|l| l.debit.is_positive())
        .map(|l| l.debit)
        .collect();
    assert_eq!(debits, [money("842.15"), money("444.25")]);
    let credit: Vec<Money> = record
        .txn
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| l.credit)
        .collect();
    assert_eq!(credit, [money("1286.40")]);
}

#[test]
fn a_used_number_offers_the_next_one_and_never_changes_it_silently() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    fill(&mut s, "20.00", &[("5110", "20.00")]);
    s.header
        .edit_step(|h| h.number = NumberField::Typed("4418".into()));
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.save.duplicate.map(|(n, _)| n), Some(4418));
    assert_eq!(s.state()["save"]["duplicate"], 4419);
    assert_eq!(
        s.header.current().number,
        NumberField::Typed("4418".into()),
        "the typed number stays until the owner takes the offer"
    );
    s.use_offered_number();
    assert_eq!(s.header.current().number, NumberField::Typed("4419".into()));
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["last_saved"]["number"], 4419);
}

#[test]
fn a_closed_period_check_asks_for_the_password() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "18.25", &[("5110", "18.25")]);
    let locked = CalendarDate::from_ymd(2026, 9, 15).expect("d");
    s.header.edit_step(|h| h.date = Some(locked));
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["closing"], "asking");
    assert_eq!(writes(&link), before, "asked before sending");
    // Cancel saves nothing.
    s.closing_prompt.cancel();
    assert_eq!(s.state()["saved_count"], 0);
    // A wrong password: the dialog stays, nothing saved.
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    s.closing_prompt.password = SecretText::from("wrong");
    s.submit_closing();
    assert_eq!(s.state()["closing"], "rejected");
    assert_eq!(s.state()["saved_count"], 0);
    s.closing_prompt.password =
        SecretText::from(crate::company::demo::register::DEMO_CLOSING_PASSWORD);
    s.submit_closing();
    assert_eq!(s.state()["closing"], "done");
    assert_eq!(s.state()["saved_count"], 1);
}

#[test]
fn print_later_shows_to_print_and_saves_unnumbered() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "100.00", &[("5110", "100.00")]);
    s.set_print_later(true);
    assert_eq!(s.state()["number_text"], "To print");
    assert_eq!(s.title(), "Write Check \u{b7} To print");
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    assert_eq!(s.state()["last_saved"]["print_status"], "queue");
    assert_eq!(s.state()["last_saved"]["number"], Value::Null);
    assert_eq!(
        s.header.current().number,
        NumberField::Proposed(4418),
        "a queued check takes no number"
    );
    // Unticking brings the number back.
    s.set_print_later(true);
    s.set_print_later(false);
    assert_eq!(s.header.current().number, NumberField::Proposed(4418));
}

#[test]
fn the_check_amount_follows_a_single_line_and_takes_the_lines_when_blank() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "50.00", &[("5110", "50.00")]);
    s.amount_typed(Some(money("75.00")));
    assert_eq!(s.lines.value().lines[0].amount, Some(money("75.00")));
    // A blank check amount takes the lines total when the owner leaves
    // the grid (Q-P13r b).
    s.amount_typed(None);
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(money("12.34")));
    s.left_lines();
    assert_eq!(s.header.current().amount, Some(money("12.34")));
    assert!(s.header.current().amount_from_lines);
}

#[test]
fn prev_opens_the_newest_check_and_void_frees_its_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["number"], 4418);
    assert!(!s.is_dirty());
    assert_eq!(s.compose().amount, Some(money("10.00")));
    // Edit and save it again.
    s.header.edit_step(|h| h.memo = "changed".into());
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["mode"], "edit");
    // Void (reason required), then the number is free again.
    assert!(s.start_void());
    assert!(s.void.open);
    s.send_void("  ", None);
    assert_eq!(s.save.notice, Some("check.void.reason_needed"));
    s.send_void("Wrong vendor", None);
    s.poll();
    assert_eq!(s.state()["last_action"], "void");
    assert_eq!(s.state()["mode"], "new");
    assert_eq!(s.header.current().number, NumberField::Proposed(4418));
    // Next from a new check stays; Prev again shows the void check read-only.
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["read_only"], true);
    assert!(!s.start_save(SaveIntent::Save, None));
    s.toggle_history();
    assert!(s.history.rows.len() >= 2);
}

#[test]
fn prev_on_unsaved_work_asks_save_discard_or_keep_editing() {
    use crate::windows::PromptAnswer;
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    // Keep editing: nothing moves, nothing is lost.
    s.navigate(true);
    assert_eq!(s.state()["nav_prompt"], true);
    assert_eq!(s.state()["mode"], "new");
    s.answer_nav(PromptAnswer::Keep);
    assert_eq!(s.state()["nav_prompt"], false);
    assert!(s.is_dirty());
    // Esc is Keep editing too.
    s.navigate(true);
    assert!(s.form_command(CommandId::new("form.revert")));
    assert!(s.nav_prompt.is_none() && s.is_dirty());
    // Save: the check is saved, then Prev opens the newest check (it).
    let before = writes(&link);
    s.navigate(true);
    s.answer_nav(PromptAnswer::Save);
    s.poll();
    assert_eq!(writes(&link), before + 1);
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["number"], 4418);
    // Discard: the change is thrown away and Prev moves on.
    s.header.edit_step(|h| h.memo = "changed".into());
    s.navigate(true);
    let before = writes(&link);
    s.answer_nav(PromptAnswer::Discard);
    s.poll();
    assert_eq!(writes(&link), before, "nothing saved");
    assert!(!s.is_dirty());
    assert_eq!(s.state()["mode"], "edit");
    assert_ne!(s.state()["number"], 4418, "moved to the next older check");
    s.clear();
    assert!(!s.is_dirty());
    assert_eq!(s.state()["last_action"], "clear");
}

#[test]
fn picking_the_payee_copies_the_last_check_but_never_an_amount() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.amount_typed(Some(money("1286.40")));
    let payee = contact(&s, "Ozark Feed & Seed");
    s.pick_payee(payee);
    s.poll();
    let st = s.state();
    assert_eq!(st["prefill"]["applied"], true);
    assert_eq!(st["prefill"]["source"], "last");
    let f = s.compose();
    let used: Vec<_> = f.lines.used().collect();
    assert_eq!(
        used.iter().map(|l| l.account).collect::<Vec<_>>(),
        [Some(account(&s, "5100")), Some(account(&s, "5110"))]
    );
    assert!(
        used.iter().all(|l| l.amount.is_none()),
        "amounts stay blank"
    );
    assert_eq!(
        f.amount,
        Some(money("1286.40")),
        "the check amount is not touched"
    );
    assert!(used[0].customer.is_some(), "the job's customer is copied");
    assert!(used.iter().all(|l| l.tag.is_some()), "line tags are copied");
    assert!(f.address.contains("1200 Commerce Dr"), "{:?}", f.address);
    assert_eq!(s.focus, Some(Field::Amount));
    assert!(!f.memo.is_empty(), "the memo is copied");
}

#[test]
fn prefill_waits_for_an_untouched_check_and_a_new_one() {
    let (_d, link) = link();
    let mut s = screen(&link);
    // A typed line: the payee pick copies nothing.
    let key = s.lines.form_mut().adapter_mut().keys.next_key();
    s.lines.form_mut().edit_step(|v| {
        v.lines = vec![FormLine {
            memo: "mine".into(),
            ..FormLine::blank(key)
        }];
    });
    s.pick_payee(contact(&s, "Ozark Feed & Seed"));
    s.poll();
    assert_eq!(s.state()["prefill"]["applied"], false);
    assert_eq!(s.compose().lines.used().count(), 1);
    // A saved check never prefills.
    let (_d2, link2) = self::link();
    let mut s = screen(&link2);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    s.pick_payee(contact(&s, "Galena Gravel"));
    s.poll();
    assert_eq!(s.state()["prefill"]["applied"], false);
}

#[test]
fn without_history_the_vendor_default_account_fills_the_first_line() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let galena = contact(&s, "Galena Gravel");
    s.header.edit_step(|h| h.payee = Some(galena));
    s.prefill.task = Some((
        galena,
        Task::ready(prefill::PrefillData {
            last: None,
            suggest: None,
        }),
    ));
    s.poll();
    assert_eq!(s.state()["prefill"]["source"], "default");
    let f = s.compose();
    let first = f.lines.used().next().expect("a line");
    assert_eq!(first.account, Some(account(&s, "5200")));
    assert_eq!(first.amount, None);
}

#[test]
fn the_panel_shows_the_open_bill_and_recent_checks_and_never_blocks_the_save() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    s.poll();
    let st = s.state();
    assert_eq!(st["panel"]["open_bills"], 1);
    assert_eq!(st["panel"]["bill_warning"], true);
    assert_eq!(st["panel"]["recent_checks"], json!([4401, 4392, 4380]));
    let summary = s.panel.summary.as_ref().and_then(panel::Card::ready);
    assert_eq!(summary.map(|v| v.total_owed), Some(money("1286.40")));
    let note = panel::bill_note(
        summary,
        s.panel
            .bills
            .as_ref()
            .and_then(panel::Card::ready)
            .map(Vec::as_slice),
        s.money_style,
    );
    assert_eq!(
        note.as_deref(),
        Some(
            "You have an open bill from this vendor for 1,286.40, due 11/05. Pay it in Pay Bills so the bill closes."
        )
    );
    // Q-248: the note never stops the save.
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["saved_count"], 1);
}

#[test]
fn several_open_bills_give_the_count_and_the_engines_total() {
    let bill = |id: &str, owed: &str| -> cl_api::OpenBill {
        serde_json::from_value(json!({
            "id": id, "txnDate": "2026-10-01", "dueDate": "2026-11-05", "balanceDue": owed,
        }))
        .expect("bill")
    };
    let bills = [
        bill("7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a18", "100.00"),
        bill("7c0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a19", "50.10"),
    ];
    let summary: cl_api::VendorSummary = serde_json::from_value(json!({
        "unpaidCount": 2, "totalOwed": "150.10", "overdueCount": 0, "totalOverdue": "0",
    }))
    .expect("summary");
    assert_eq!(
        panel::bill_note(Some(&summary), Some(&bills), cl_core::MoneyStyle::default()).as_deref(),
        Some(
            "You have 2 open bills from this vendor for 150.10. Pay them in Pay Bills so the bills close."
        )
    );
    assert_eq!(
        panel::bill_note(Some(&summary), Some(&[]), cl_core::MoneyStyle::default()).map(|_| ()),
        Some(()),
        "the summary alone still warns"
    );
    let none: cl_api::VendorSummary = serde_json::from_value(json!({
        "unpaidCount": 0, "totalOwed": "0", "overdueCount": 0, "totalOverdue": "0",
    }))
    .expect("summary");
    assert_eq!(
        panel::bill_note(Some(&none), Some(&[]), cl_core::MoneyStyle::default()),
        None
    );
}

/// Ozark Feed & Seed and one line with `memo`, `amount` and no account.
fn unassigned_line(s: &mut WriteCheckScreen, memo: &str, amount: &str) -> RowKey {
    s.pick_payee(contact(s, "Ozark Feed & Seed"));
    s.poll();
    s.amount_typed(Some(money(amount)));
    let key = s.lines.form_mut().adapter_mut().keys.next_key();
    let entry = s.lines.form_mut().adapter_mut().keys.next_key();
    let line = FormLine {
        memo: memo.into(),
        amount: Some(money(amount)),
        ..FormLine::blank(key)
    };
    s.lines
        .form_mut()
        .edit_step(|v| v.lines = vec![line, FormLine::blank(entry)]);
    key
}

#[test]
fn an_ai_suggestion_is_a_ghost_until_the_owner_accepts_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let key = unassigned_line(&mut s, "Hardwood mulch", "100.00");
    s.maybe_suggest();
    s.poll();
    assert_eq!(s.state()["ai"]["chip"], true);
    assert_eq!(s.lines.open_suggestions(), [(key, account(&s, "5110"))]);
    // The ghost is not the line's account: the save is refused, nothing sent.
    assert_eq!(s.lines.value().lines[0].account, None);
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["save"]["refused"], "line_account_missing");
    assert_eq!(writes(&link), before);
    // The amount was never touched.
    assert_eq!(s.lines.value().lines[0].amount, Some(money("100.00")));
    // Accept sets it.
    s.accept_suggestion(key);
    assert_eq!(s.lines.value().lines[0].account, Some(account(&s, "5110")));
    assert_eq!(s.state()["ai"]["accepted"], 1);
    assert_eq!(s.state()["ai"]["chip"], false);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
}

#[test]
fn with_ai_off_nothing_is_asked_and_no_chip_shows() {
    let (_d, link) = link();
    let mut s = screen(&link);
    link.demo_state().expect("demo").ai.categorization = false;
    unassigned_line(&mut s, "Hardwood mulch", "100.00");
    s.maybe_suggest();
    s.poll();
    assert_eq!(s.state()["ai"]["chip"], false);
    assert_eq!(link.demo_state().expect("demo").checks.ai_requests, 0);
    // A rule still answers offline, before any AI.
    link.demo_state().expect("demo").ai.categorization = true;
    let plants = account(&s, "5100");
    link.demo_state()
        .expect("demo")
        .checks
        .rules
        .push(crate::company::demo::checks::DemoRule {
            name: "Mulch to plants".into(),
            contains: "mulch".into(),
            account: plants,
        });
    let key = unassigned_line(&mut s, "More mulch", "20.00");
    s.maybe_suggest();
    s.poll();
    assert_eq!(s.state()["ai"]["rule"], true);
    assert_eq!(s.state()["ai"]["chip"], false);
    assert_eq!(s.lines.open_suggestions(), [(key, plants)]);
    assert_eq!(link.demo_state().expect("demo").checks.ai_requests, 0);
}

#[test]
fn accepts_are_counted_locally_per_payee_and_account() {
    let (dir, link) = link();
    let tenant = TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id");
    let mut s = WriteCheckScreen::new(Arc::clone(&link), Some((tenant, "Ozark".into())), today());
    s.poll();
    let key = unassigned_line(&mut s, "Hardwood mulch", "100.00");
    s.maybe_suggest();
    s.poll();
    s.accept_suggestion(key);
    let settings = dir.path().join("settings");
    let book = crate::prefs::SuggestionAccepts::load(&settings);
    let payee = contact(&s, "Ozark Feed & Seed");
    assert_eq!(
        book.count(&crate::prefs::SuggestionAccepts::key(
            &tenant.to_string(),
            &payee.to_string(),
            &account(&s, "5110").to_string(),
            "ai",
        )),
        1
    );
}

#[test]
fn a_receipt_rides_on_the_first_save_and_bad_files_are_never_sent() {
    let (dir, link) = link();
    let mut s = screen(&link);
    // Not a PDF or image: refused here.
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, b"hello").expect("write");
    s.attach_path(&text);
    assert_eq!(s.attach.problem, Some("check.attach.wrong_type"));
    assert!(s.header.current().draft_attachment.is_none());
    let missing = dir.path().join("missing.pdf");
    s.attach_path(&missing);
    assert_eq!(s.attach.problem, Some("check.attach.not_found"));
    // A PDF: uploaded as a draft, listed, carried on the create.
    let pdf = dir.path().join("receipt.pdf");
    std::fs::write(&pdf, b"%PDF-1.4\n%synthetic\n").expect("write");
    s.attach_path(&pdf);
    assert_eq!(s.attach.problem, None);
    assert!(s.header.current().draft_attachment.is_some());
    assert_eq!(s.state()["attachments"]["count"], 1);
    // One file before the first save.
    s.attach_path(&pdf);
    assert_eq!(s.attach.problem, Some("check.attach.one_before_save"));
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(s.state()["attachments"]["count"], 0, "a new check");
    // The saved check has it; more files attach directly.
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["attachments"]["count"], 1);
    s.attach_path(&pdf);
    assert_eq!(s.state()["attachments"]["count"], 2);
}

#[test]
fn a_printed_check_edit_asks_reprint_or_keep_the_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.navigate(true);
    s.poll();
    let id = s.editing.as_ref().map(|e| e.id).expect("editing");
    assert!(crate::company::demo::checks::mark_printed(
        &mut link.demo_state().expect("demo"),
        id
    ));
    // Change the printed amount (and its line).
    s.amount_typed(Some(money("12.00")));
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["printed_prompt"], true);
    assert_eq!(s.printed.and_then(|p| p.number), Some(4418));
    assert!(s.is_dirty(), "nothing changed; the owner's edit stays");
    // Cancel: nothing saved.
    s.answer_printed(None);
    assert_eq!(s.state()["printed_prompt"], false);
    assert_eq!(writes(&link), before + 1, "only the refused request");
    // Keep number: saved as a handwritten check with its number.
    assert!(!s.start_save(SaveIntent::Save, None));
    s.answer_printed(Some(cl_api::PrintedCheckChange::KeepNumber));
    assert_eq!(s.state()["last_saved"]["print_status"], "hand_written");
    assert_eq!(s.state()["last_saved"]["number"], 4418);
    // Reprint: back to the print queue without a number.
    assert!(crate::company::demo::checks::mark_printed(
        &mut link.demo_state().expect("demo"),
        id
    ));
    s.amount_typed(Some(money("13.00")));
    assert!(!s.start_save(SaveIntent::Save, None));
    s.answer_printed(Some(cl_api::PrintedCheckChange::Reprint));
    assert_eq!(s.state()["last_saved"]["print_status"], "queue");
}

/// The demo-only `dev.check_mark_printed` (the scenario's way to the
/// printed-check question before P19): only a saved check, then the edit
/// asks.
#[test]
fn the_dev_command_marks_the_demo_check_printed() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let mark = cl_model::CommandId::new("dev.check_mark_printed");
    assert!(!s.command(mark), "a new check is not saved yet");
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.navigate(true);
    s.poll();
    assert!(s.command(mark));
    assert_eq!(s.state()["edit_print_status"], "printed");
    s.amount_typed(Some(money("11.00")));
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["printed_prompt"], true);
}

#[test]
fn the_draft_is_the_checked_check_field_by_field() {
    let account = AccountId::parse("00000000-0000-0000-0000-000000005100").expect("id");
    let bank = AccountId::parse("00000000-0000-0000-0000-000000001010").expect("id");
    let payee = ContactId::parse("00000000-0000-0000-0000-000000000007").expect("id");
    let job = ContactId::parse("00000000-0000-0000-0000-000000000009").expect("id");
    let tag = TagId::parse("00000000-0000-0000-0000-000000000003").expect("id");
    let c = CheckedCheck {
        bank,
        number: Some(4418),
        print_later: false,
        date: today(),
        payee,
        payee_name: String::new(),
        address: Some("1200 Commerce Dr".into()),
        amount: money("1286.40"),
        memo: Some("Acct 7712".into()),
        header_tags: vec![tag],
        lines: vec![PostingLine {
            account,
            amount: money("1286.40"),
            memo: Some("Perennials".into()),
            contact: Some(job),
            billable: true,
            tag: Some(tag),
        }],
        draft_attachment: None,
    };
    let d = to_draft(&c, "Ozark Feed & Seed").expect("draft");
    assert_eq!(d.bank, bank);
    assert_eq!(d.date, today());
    assert_eq!(d.payee, Some(payee));
    assert_eq!(
        d.payee_name, "Ozark Feed & Seed",
        "a blank name takes the display name"
    );
    assert_eq!(d.check_number, Some(4418));
    assert!(!d.print_later);
    assert_eq!(d.payee_address.as_deref(), Some("1200 Commerce Dr"));
    assert_eq!(d.memo.as_deref(), Some("Acct 7712"));
    assert_eq!(d.printed_memo, d.memo, "Q-P13n");
    assert_eq!(d.tag_ids, [tag]);
    assert_eq!(d.amount, money("1286.40"));
    assert_eq!(
        d.lines,
        [SplitLine {
            account,
            amount: money("1286.40"),
            memo: Some("Perennials".into()),
            contact: Some(job),
            billable: true,
            tag: Some(tag),
        }]
    );
    let named = CheckedCheck {
        payee_name: "Ozark Feed and Seed Co.".into(),
        draft_attachment: Some(cl_model::forms::check::AttachmentRef {
            id: "not-a-uuid".into(),
            name: "r.pdf".into(),
        }),
        ..c.clone()
    };
    assert!(
        to_draft(&named, "x").is_err(),
        "an unknown draft id is refused"
    );
    let named = CheckedCheck {
        draft_attachment: None,
        ..named
    };
    assert_eq!(
        to_draft(&named, "x").expect("draft").payee_name,
        "Ozark Feed and Seed Co."
    );
    // The posting builder takes it, and refuses billable lines without
    // the capability (nothing is sent).
    assert!(BankPosting::check_with(d.clone(), true).is_ok());
    assert_eq!(
        BankPosting::check_with(d, false),
        Err(cl_api::LocalValidation::LineBillableUnavailable)
    );
}

#[test]
fn every_problem_has_a_name_and_a_message() {
    let (_d, link) = link();
    let s = screen(&link);
    let ctx = cl_model::forms::check::CheckContext {
        tags: &s.tag_list,
        editing: false,
    };
    let Err(problems) = check(&s.compose(), &ctx) else {
        panic!("a blank check is refused");
    };
    assert!(problems.contains(&CheckProblem::PayeeMissing));
    for p in problems {
        assert_ne!(problem_name(p), "");
        assert!(crate::i18n::has(p.key().0), "{}", p.key().0);
    }
}

use cl_api::BankPosting;

/// Draws the screen a few frames and returns every accessible (role,
/// name, disabled), a text input's name being the label it is labelled by.
fn tree(s: &mut WriteCheckScreen, width: f32) -> Vec<(egui::accesskit::Role, String, bool)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut all = std::collections::HashMap::new();
    for _ in 0..3 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 940.0))),
            ..RawInput::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
        });
        out.textures_delta.clear();
        if let Some(t) = out.platform_output.accesskit_update {
            all.extend(t.nodes);
        }
    }
    let nodes: Vec<_> = all.into_iter().collect();
    let labels: std::collections::HashMap<_, _> = nodes
        .iter()
        .filter_map(|(id, n)| n.label().or(n.value()).map(|l| (*id, l.to_string())))
        .collect();
    nodes
        .iter()
        .map(|(_, n)| {
            let own = n.label().or(if n.role() == egui::accesskit::Role::Label {
                n.value()
            } else {
                None
            });
            let name = own.map_or_else(
                || {
                    n.labelled_by()
                        .iter()
                        .filter_map(|id| labels.get(id).cloned())
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                str::to_string,
            );
            (n.role(), name, n.is_disabled())
        })
        .collect()
}

#[test]
fn every_input_is_named_by_its_label_and_the_toolbar_matches_the_plan() {
    use egui::accesskit::Role;
    let (_d, link) = link();
    for width in [1440.0, 960.0] {
        let mut s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today());
        let nodes = tree(&mut s, width);
        let has = |role: Role, name: &str| nodes.iter().any(|(r, n, _)| *r == role && n == name);
        for name in [
            "No.",
            "Date",
            "Pay to the order of",
            "Amount ($)",
            "Address",
            "Memo",
        ] {
            assert!(
                has(Role::TextInput, name) || has(Role::MultilineTextInput, name),
                "{name}: {nodes:#?}"
            );
        }
        for name in [
            "Clear",
            "Save & New",
            "Save & Close",
            "Find",
            "History",
            "Prev",
            "Next",
        ] {
            assert!(has(Role::Button, name), "{name}");
        }
        assert!(has(Role::CheckBox, "Print later"));
        let print = nodes
            .iter()
            .find(|(r, n, _)| *r == Role::Button && n == "Print")
            .expect("Print");
        assert!(print.2, "Print is off until P19");
        // P15 adds Save as Template (Q-256); the rest stay out.
        assert!(has(Role::Button, "Save as Template"));
        for name in ["Memorize", "Pay online", "Items"] {
            assert!(!nodes.iter().any(|(_, n, _)| n.starts_with(name)), "{name}");
        }
        let named = |want: &str| nodes.iter().any(|(_, n, _)| n.starts_with(want));
        assert!(named("One thousand two hundred eighty-six and 40/100"));
        assert!(named("Lines total 1,286.40 = check amount"));
        assert!(named("84,210.55"), "the ending balance");
        assert!(named("Expenses 1,286.40"));
    }
}

// ------------------------------------------------------------------------
// Review fixes (P13): the expense edit path, header commit, line
// customers without the capability, the bank line, 409 codes, files and
// the AI cap.

/// The saved check numbered `n` on 1010, read as the demo engine has it.
fn saved_check(link: &Arc<CompanyLink>, s: &WriteCheckScreen, n: u32) -> cl_api::CheckRecord {
    let mut demo = link.demo_state().expect("demo");
    let bank = account(s, "1010");
    let id = crate::company::demo::checks::list(&mut demo, bank)
        .into_iter()
        .find(|c| c.number == Some(n))
        .map_or_else(|| panic!("check {n}"), |c| c.id);
    crate::company::demo::checks::get(&mut demo, id)
        .expect("get")
        .record
}

fn bank_credit(r: &cl_api::CheckRecord) -> Vec<Money> {
    r.txn
        .lines
        .iter()
        .filter(|l| l.credit.is_positive())
        .map(|l| l.credit)
        .collect()
}

#[test]
fn without_check_edit_an_edit_goes_through_the_ledger_record_and_saves_again() {
    let (_d, link) = link();
    link.demo_state().expect("demo").checks.check_edit_off = true;
    let mut s = screen(&link);
    assert_eq!(s.state()["check_edit"], false);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    // The first edit: guarded by the plain transaction's version, never
    // the check's (which would always look changed elsewhere).
    s.header.edit_step(|h| h.memo = "changed".into());
    let before = writes(&link);
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["stale"], false);
    assert_eq!(writes(&link), before + 1);
    assert_eq!(s.state()["saved_count"], 2);
    assert_eq!(
        saved_check(&link, &s, 4418).txn.memo.as_deref(),
        Some("changed")
    );
    // A second edit of the same check, with a new date: guarded by the
    // version the first save left (not stale), and the old date follows.
    let moved = CalendarDate::from_ymd(2026, 11, 5).expect("date");
    s.header.edit_step(|h| {
        h.memo = "again".into();
        h.date = Some(moved);
    });
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["stale"], false);
    assert_eq!(s.state()["saved_count"], 3);
    assert_eq!(s.editing.as_ref().map(|e| e.old_date), Some(moved));
    let r = saved_check(&link, &s, 4418);
    assert_eq!(r.txn.memo.as_deref(), Some("again"));
    assert_eq!(r.check_number, Some(4418), "the number stays");
    // Changed somewhere else: refused as stale, nothing sent.
    let id = r.txn.id;
    {
        let mut demo = link.demo_state().expect("demo");
        let other = crate::company::demo::checks::get(&mut demo, id).expect("get");
        let version = other.transaction_version();
        let draft = cl_api::CheckDraft {
            bank: account(&s, "1010"),
            date: moved,
            payee: other.record.txn.contact_id,
            payee_name: other.record.payee_name.clone(),
            check_number: Some(4418),
            print_later: false,
            payee_address: None,
            memo: Some("elsewhere".into()),
            printed_memo: None,
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: money("10.00"),
            lines: vec![SplitLine {
                account: account(&s, "5110"),
                amount: money("10.00"),
                memo: None,
                contact: None,
                billable: false,
                tag: None,
            }],
        };
        crate::company::demo::checks::update_expense(&mut demo, id, &version, &draft, None)
            .expect("changed elsewhere");
    }
    s.header.edit_step(|h| h.memo = "mine".into());
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["stale"], true);
    assert_eq!(writes(&link), before, "nothing sent");
}

#[test]
fn alt_s_saves_the_amount_and_payee_still_being_typed() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "50.00", &[("5110", "50.00")]);
    // Typed, the field not left yet: the one line follows the new amount.
    s.inputs.amount = "75.00".into();
    assert!(s.form_command(CommandId::new("form.save_close")));
    assert_eq!(s.state()["saved_count"], 1);
    assert_eq!(bank_credit(&saved_check(&link, &s, 4418)), [money("75.00")]);
    // On a saved check too.
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    s.sync_inputs();
    s.inputs.amount = "80.00".into();
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(bank_credit(&saved_check(&link, &s, 4418)), [money("80.00")]);
    // A typed payee name is resolved before the save.
    s.sync_inputs();
    s.inputs.payee = "Galena".into();
    assert!(s.start_save(SaveIntent::Save, None));
    assert_eq!(
        saved_check(&link, &s, 4418).txn.contact_id,
        Some(contact(&s, "Galena Gravel"))
    );
}

#[test]
fn a_typed_amount_or_payee_that_is_not_accepted_saves_nothing() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "50.00", &[("5110", "50.00")]);
    let before = writes(&link);
    s.inputs.amount = "12..5".into();
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.focus, Some(Field::Amount));
    assert_eq!(s.header.current().amount, Some(money("50.00")));
    s.sync_inputs();
    s.inputs.payee = "Nobody By This Name".into();
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.focus, Some(Field::Payee));
    assert!(s.inputs.payee_problem);
}

#[test]
fn without_line_billable_no_line_customer_is_copied_or_sent() {
    let (_d, link) = link();
    link.demo_state().expect("demo").checks.line_billable_off = true;
    let mut s = screen(&link);
    assert_eq!(s.state()["line_billable"], false);
    s.pick_payee(contact(&s, "Ozark Feed & Seed"));
    s.poll();
    assert_eq!(s.state()["prefill"]["applied"], true);
    let f = s.compose();
    assert!(f.lines.used().count() >= 1);
    assert!(
        f.lines.used().all(|l| l.customer.is_none() && !l.billable),
        "the last check's line customer is not copied"
    );
    // A line customer that got onto the form anyway: refused here with a
    // plain message, nothing sent.
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    let job = contact(&s, "Hollister Family Trust");
    s.lines.form_mut().edit_step(|v| {
        v.lines[0].customer = Some(job);
        v.lines[0].billable = true;
    });
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::SaveAndNew, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.state()["notice"], "check.line_billable_unavailable");
    assert!(crate::i18n::has("check.line_billable_unavailable"));
}

#[test]
fn a_check_with_a_line_customer_is_read_only_without_line_billable() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    let job = contact(&s, "Hollister Family Trust");
    s.lines.form_mut().edit_step(|v| {
        v.lines[0].customer = Some(job);
        v.lines[0].billable = true;
    });
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    // The engine without `line-billable-v1` (Q-P13x).
    link.demo_state().expect("demo").checks.line_billable_off = true;
    let mut s = screen(&link);
    s.navigate(true);
    s.poll();
    assert_eq!(s.state()["mode"], "edit");
    assert_eq!(s.state()["read_only"], true);
    assert_eq!(
        s.state()["read_only_note"],
        "check.read_only.line_customers"
    );
    assert!(crate::i18n::has("check.read_only.line_customers"));
    s.header.edit_step(|h| h.memo = "changed".into());
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(writes(&link), before);
    assert_eq!(s.state()["notice"], "check.read_only.line_customers");
}

#[test]
fn the_bank_line_is_the_credit_on_the_checks_bank_account() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let r = saved_check(&link, &s, 4418);
    let bank = account(&s, "1010");
    let keys = &mut cl_model::rowkey::RowKeyGen::new();
    let (f, lines, exact) = save::form_of(&r, bank, keys);
    assert!(exact);
    assert_eq!(f.bank, Some(bank));
    assert_eq!(f.amount, Some(money("1286.40")));
    assert_eq!(lines.used().count(), 2);
    // Read against another account: not this form's check.
    let (_, _, exact) = save::form_of(&r, account(&s, "5100"), keys);
    assert!(!exact);
    // A second credit line (before the bank line): shown read-only, never
    // taken for the bank line.
    let mut odd = r.clone();
    let mut refund = odd.txn.lines[0].clone();
    refund.debit = Money::ZERO;
    refund.credit = money("5.00");
    odd.txn.lines.insert(0, refund);
    let (_, _, exact) = save::form_of(&odd, bank, keys);
    assert!(!exact);
    // Loaded into the screen: read-only with its note.
    s.load_bank = Some(bank);
    s.show_record(&cl_api::Versioned {
        version: RecordVersion::Fingerprint(cl_api::EditableFields::fingerprint(&odd)),
        record: odd,
    });
    assert_eq!(s.state()["read_only"], true);
    assert_eq!(s.state()["read_only_note"], "check.read_only.shape");
    assert!(crate::i18n::has("check.read_only.shape"));
}

fn conflict(code: Option<&str>) -> ApiError {
    ApiError::Conflict {
        reason: cl_api::ConflictReason::Unspecified,
        current_version: None,
        code: code.and_then(cl_api::ErrorCode::parse),
        message: cl_api::UserText::default(),
    }
}

#[test]
fn only_a_code_less_409_offers_another_number() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    s.save_failed(conflict(Some(cl_api::LINE_BILLED)));
    assert_eq!(s.save.duplicate, None);
    assert_eq!(s.state()["notice"], "check.line_billed");
    assert!(crate::i18n::has("check.line_billed"));
    s.save.notice = None;
    s.save_failed(conflict(Some("SOMETHING_NEW")));
    assert_eq!(s.save.duplicate, None);
    assert!(s.save.error.is_some(), "a plain error, not a used number");
    s.save.error = None;
    s.save_failed(conflict(None));
    assert_eq!(s.save.duplicate, Some((4418, 4419)));
}

#[test]
fn a_saved_check_needs_a_number_unless_printed_later() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "10.00", &[("5110", "10.00")]);
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.navigate(true);
    s.poll();
    s.header
        .edit_step(|h| h.number = NumberField::Typed(String::new()));
    let before = writes(&link);
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["refused"], "number_invalid");
    assert_eq!(writes(&link), before);
}

#[test]
fn the_proposal_counts_checks_that_pay_bills() {
    let (_d, link) = link();
    let s = screen(&link);
    let bank = account(&s, "1010");
    let mut demo = link.demo_state().expect("demo");
    let chart = demo
        .chart
        .get_or_insert_with(crate::company::demo::chart)
        .clone();
    let posting = crate::company::demo::register::DemoPosting {
        kind: "bill_payment",
        date: today(),
        number: "4500".into(),
        contact: None,
        memo: String::new(),
        own: bank,
        own_debit: false,
        lines: vec![(account(&s, "5110"), money("1.00"))],
    };
    demo.ledger
        .save(&chart, &posting, None)
        .expect("bill payment");
    assert_eq!(
        crate::company::demo::checks::numbers(&mut demo, bank),
        (None, Some(4500))
    );
}

#[test]
fn a_saved_copy_never_overwrites_and_is_named_by_its_bytes() {
    let dir = tempfile::tempdir().expect("tmp");
    let pdf = b"%PDF-1.4\n%synthetic\n";
    let first = attach::save_copy(dir.path(), "receipt.pdf", pdf).expect("first");
    std::fs::write(&first, b"owner edits").expect("edit");
    let second = attach::save_copy(dir.path(), "receipt.pdf", pdf).expect("second");
    assert_ne!(first, second);
    assert_eq!(std::fs::read(&first).expect("read"), b"owner edits");
    assert_eq!(std::fs::read(&second).expect("read"), pdf);
    assert!(second.ends_with("receipt (2).pdf"), "{second:?}");
    // The extension is the detected type's; the name loses any path.
    let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0];
    let p = attach::save_copy(dir.path(), "..\\..\\evil.exe", &png).expect("png");
    assert_eq!(p.parent(), Some(dir.path()));
    assert!(p.ends_with("evil.png"), "{p:?}");
    // Not a PDF or image: nothing written.
    assert_eq!(
        attach::save_copy(dir.path(), "notes.txt", b"hello"),
        Err("check.attach.download_wrong_type")
    );
    assert!(crate::i18n::has("check.attach.download_wrong_type"));
    for (name, want) in [
        ("CON", "file-CON"),
        ("nul.pdf", "file-nul"),
        ("Com1.txt.pdf", "file-Com1.txt"),
        ("lpt9", "file-lpt9"),
        ("report. . .pdf", "report"),
        ("../etc/passwd", "passwd"),
        ("a/b\\c:d.pdf", "cd"),
        ("...", "attachment"),
        ("COM10", "COM10"),
    ] {
        assert_eq!(attach::safe_stem(name), want, "{name}");
    }
}

#[test]
fn the_ai_cap_checks_every_provider_and_an_unreadable_store_blocks() {
    let consent: cl_api::CompanyConsent = serde_json::from_value(json!({
        "companyId": "d0000000-0000-4000-8000-000000000001",
        "aiEnabled": true,
        "acceptedAt": "2026-10-01T00:00:00.000Z",
        "acceptedVersion": 1,
        "systemVersion": 1,
        "isStale": false,
        "enabledTasks": { "categorization": true },
    }))
    .expect("consent");
    let config = |chain: &[&str], task: Option<&str>| cl_api::AiConfig {
        is_enabled: true,
        fallback_chain: chain.iter().map(|p| (*p).to_string()).collect(),
        categorization_provider: task.map(str::to_string),
        ..cl_api::AiConfig::default()
    };
    let reachable = |c: &cl_api::AiConfig, under_cap: bool| {
        suggest::gate_of(Some(c), Some(&consent), under_cap).provider_reachable
    };
    assert!(reachable(&config(&["ollama"], None), false));
    assert!(
        !reachable(&config(&["ollama", "anthropic"], None), false),
        "a cloud fallback needs the cap"
    );
    assert!(reachable(&config(&["ollama", "anthropic"], None), true));
    assert!(!reachable(&config(&["ollama"], Some("openai")), false));
    assert!(!reachable(&config(&["ollama", "something"], None), true));
    assert!(!reachable(&config(&[], None), true));
    assert!(suggest::gate_of(Some(&config(&["ollama"], None)), Some(&consent), false).is_open());
    // The local store can't be read: the cap is unknown, so no cloud call.
    let (dir, link) = link();
    let tenant = TenantId::parse("d0000000-0000-4000-8000-000000000001").expect("id");
    let s = WriteCheckScreen::new(Arc::clone(&link), Some((tenant, "Ozark".into())), today());
    assert!(s.under_cap(), "nothing stored yet: no cap");
    std::fs::create_dir_all(dir.path().join("settings").join("companies.json")).expect("dir");
    assert!(!s.under_cap());
}

/// One frame of the screen with `events`; the AccessKit nodes it gave.
fn frame_with(
    s: &mut WriteCheckScreen,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> Vec<(egui::accesskit::NodeId, egui::accesskit::Node)> {
    use eframe::egui::{Pos2, RawInput, Rect, vec2};
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1440.0, 940.0))),
        events,
        ..RawInput::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| s.ui(ui));
    });
    out.textures_delta.clear();
    out.platform_output
        .accesskit_update
        .map(|u| u.nodes)
        .unwrap_or_default()
}

/// Q-250: a click on "Pay to the order of" opens the names list at once;
/// Tab into the field does not; Esc closes it and keeps the payee.
#[test]
fn a_click_on_the_payee_opens_the_names_list_and_tab_does_not() {
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2};
    let (_d, link) = link();
    let mut s = WriteCheckScreen::demo_draft(Arc::clone(&link), None, today());
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
    crate::fonts::install(&ctx);
    ctx.enable_accesskit();
    let mut nodes = Vec::new();
    for _ in 0..3 {
        nodes.extend(frame_with(&mut s, &ctx, vec![]));
    }
    let label = nodes
        .iter()
        .find(|(_, n)| {
            n.role() == egui::accesskit::Role::Label && n.value() == Some("Pay to the order of")
        })
        .map(|(id, _)| *id)
        .expect("payee label");
    let field = nodes
        .iter()
        .rev()
        .find(|(_, n)| n.labelled_by().contains(&label))
        .and_then(|(_, n)| n.bounds())
        .expect("payee field");
    // AccessKit bounds are f64 copies of egui's f32 points.
    #[allow(clippy::cast_possible_truncation)]
    let at = Pos2::new(
        field.x0 as f32 + 24.0,
        f64::midpoint(field.y0, field.y1) as f32,
    );
    let payee = s.header.current().payee;
    assert!(payee.is_some(), "the draft has a payee");
    assert_eq!(s.state_json()["payee_list"], false);

    // A click opens the list.
    let button = |pressed| Event::PointerButton {
        pos: at,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    frame_with(&mut s, &ctx, vec![Event::PointerMoved(at), button(true)]);
    frame_with(&mut s, &ctx, vec![button(false)]);
    frame_with(&mut s, &ctx, vec![]);
    assert_eq!(s.state_json()["payee_list"], true, "a click opens it");

    // Esc closes it; the payee stays.
    assert!(s.command(CommandId::new("form.revert")));
    frame_with(&mut s, &ctx, vec![]);
    assert_eq!(s.state_json()["payee_list"], false);
    assert_eq!(s.header.current().payee, payee, "Esc changed nothing");

    // Shift+Tab out and Tab back in: focused, no list.
    let tab = |m: Modifiers| {
        [true, false]
            .into_iter()
            .map(|pressed| Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed,
                repeat: false,
                modifiers: m,
            })
            .collect::<Vec<_>>()
    };
    frame_with(&mut s, &ctx, tab(Modifiers::SHIFT));
    frame_with(&mut s, &ctx, vec![]);
    assert_ne!(s.focused, Some(Field::Payee), "Shift+Tab left the payee");
    frame_with(&mut s, &ctx, tab(Modifiers::NONE));
    frame_with(&mut s, &ctx, vec![]);
    assert_eq!(s.focused, Some(Field::Payee), "Tab came back");
    assert_eq!(s.state_json()["payee_list"], false, "Tab never opens it");
    assert_eq!(s.header.current().payee, payee);
}

/// Q-264: Enter in a header field is Save & New by default and moves to
/// the next field (Shift+Enter back) when "Enter moves between fields" is on.
#[test]
fn header_enter_saves_or_moves_by_the_preference() {
    use crate::forms::enter::testing::{context, enter, frame, settle};
    use cl_model::forms::EnterMode;
    for mode in [EnterMode::SaveAndNew, EnterMode::MoveBetweenFields] {
        let (_d, link) = link();
        let mut s = screen(&link);
        fill(&mut s, "10.00", &[("5110", "10.00")]);
        let ctx = context();
        s.focus = Some(Field::Date);
        settle(&ctx, mode, &mut |ui| s.ui(ui));
        assert_eq!(s.focused, Some(Field::Date), "{mode:?}");
        frame(&ctx, mode, enter(false), &mut |ui| s.ui(ui));
        settle(&ctx, mode, &mut |ui| s.ui(ui));
        match mode {
            EnterMode::SaveAndNew => {
                assert_eq!(s.state()["saved_count"], 1, "Enter = Save & New (Q-190)");
            }
            EnterMode::MoveBetweenFields => {
                assert_eq!(s.state()["saved_count"], 0, "Enter never saves here");
                assert_eq!(s.focused, Some(Field::Payee), "the next field");
                frame(&ctx, mode, enter(true), &mut |ui| s.ui(ui));
                settle(&ctx, mode, &mut |ui| s.ui(ui));
                assert_eq!(s.focused, Some(Field::Date), "Shift+Enter goes back");
                assert_eq!(s.state()["saved_count"], 0);
            }
        }
    }
}

fn unknown() -> ApiError {
    ApiError::OutcomeUnknown {
        request_id: cl_core::ids::RequestId::parse("00000000-0000-4000-8000-000000000003")
            .expect("id"),
        idempotency_key: Some(IdempotencyKey::new()),
    }
}

fn checks_numbered(link: &Arc<CompanyLink>, bank: AccountId, n: u32) -> Vec<TransactionId> {
    let mut demo = link.demo_state().expect("demo");
    crate::company::demo::checks::list(&mut demo, bank)
        .into_iter()
        .filter(|c| c.number == Some(n))
        .map(|c| c.id)
        .collect()
}

#[test]
fn b2_after_an_unknown_outcome_save_sends_nothing_and_a_used_number_opens_the_first_check() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    let bank = s.bank().expect("bank");
    let header = s.header.current().clone();
    let lines = s.lines.value().clone();
    // The first try reached the engine, but its reply was lost.
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    s.poll();
    let first = checks_numbered(&link, bank, 4418);
    assert_eq!(first.len(), 1);
    s.header.edit_step(|h| *h = header.clone());
    let lists = Arc::clone(&s.lists);
    s.lines.form_mut().edit_step(|v| *v = lines.clone());
    let _ = s.lines.form_mut().begin_save(SaveIntent::Save, &*lists);
    let _ = s.header.begin_save(SaveIntent::Save);
    s.save_failed(unknown());
    assert!(s.save.lost.pending());
    let before = writes(&link);
    for intent in [
        SaveIntent::Save,
        SaveIntent::SaveAndNew,
        SaveIntent::SaveAndClose,
    ] {
        assert!(!s.start_save(intent, None));
    }
    assert_eq!(writes(&link), before, "a second Save sends nothing");
    // The recheck can't tell (demo); the owner looks and confirms.
    s.save.lost.recheck(&mut s.header, &link, None);
    s.poll();
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(writes(&link), before);
    s.save.lost.ask_owner();
    s.save.lost.owner_confirmed(&mut s.header, "write_check");
    s.start_save(SaveIntent::Save, None);
    s.poll();
    // The number is used: the first check opens; no 4419 is offered.
    assert_eq!(s.save.duplicate, None, "never n+1 after an unknown outcome");
    assert_eq!(s.editing.as_ref().map(|e| e.id), Some(first[0]));
    assert_eq!(checks_numbered(&link, bank, 4418).len(), 1);
    assert!(
        checks_numbered(&link, bank, 4419).is_empty(),
        "no second check"
    );
}

#[test]
fn b2_a_recheck_that_finds_the_check_shows_it() {
    let (_d, link) = link();
    let mut s = screen(&link);
    fill(&mut s, "1286.40", &[("5100", "842.15"), ("5110", "444.25")]);
    let bank = s.bank().expect("bank");
    let lists = Arc::clone(&s.lists);
    let _ = s.lines.form_mut().begin_save(SaveIntent::Save, &*lists);
    let _ = s.header.begin_save(SaveIntent::Save);
    s.save_failed(unknown());
    // Say the engine had it after all (its id from another check here).
    let other = checks_numbered(&link, bank, 4417);
    s.save.lost.recheck_answering(
        &mut s.header,
        cl_api::Recheck::Committed {
            transaction: other.first().copied(),
        },
    );
    s.poll();
    assert!(!s.header.is_unconfirmed());
    assert_eq!(s.editing.as_ref().map(|e| e.id), other.first().copied());
    assert!(!s.is_dirty());
}
