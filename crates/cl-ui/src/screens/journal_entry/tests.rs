use std::sync::Arc;

use cl_api::ContactType;
use cl_core::ids::ContactId;
use cl_core::{Money, Secret};
use cl_model::forms::journal::{credit_of, debit_of, set_credit, set_debit};
use cl_model::forms::lines::FormLine;

use super::*;
use crate::company::demo::account_id;
use crate::company::demo::register::DEMO_CLOSING_PASSWORD;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).expect("date")
}

fn screen() -> (JournalScreen, Arc<CompanyLink>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let s = JournalScreen::new(Arc::clone(&link), None, day(11, 4));
    (s, link, dir)
}

/// Puts `(account, debit, credit, name)` lines in the grid (plus the blank
/// entry line), as the owner would type them.
fn set_lines(s: &mut JournalScreen, lines: &[(u32, &str, &str, Option<ContactId>)]) {
    let keys = &mut s.lines.form_mut().adapter_mut().keys;
    let seeds = lines
        .iter()
        .map(|(n, d, c, who)| {
            let mut l = FormLine::blank(keys.next_key());
            l.account = Some(account_id(*n));
            if !d.is_empty() {
                set_debit(&mut l, Some(m(d)));
            }
            if !c.is_empty() {
                set_credit(&mut l, Some(m(c)));
            }
            l.customer = *who;
            l
        })
        .collect();
    let v = FormLines::from_lines(seeds, keys);
    s.lines.form_mut().edit_step(|x| *x = v);
    s.seen_lines = s.lines.value().clone();
}

fn journal_count(link: &CompanyLink) -> usize {
    let state = link.demo_state().expect("demo");
    crate::company::demo::journal::list(&state).len()
}

fn contact(link: &CompanyLink, kind: ContactType) -> ContactId {
    let mut state = link.demo_state().expect("demo");
    state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.contact_type == kind)
        .map(|c| c.id)
        .expect("a demo contact")
}

#[test]
fn an_unbalanced_entry_cannot_save_and_nothing_is_sent() {
    let (mut s, link, _dir) = screen();
    assert!(s.books.loaded);
    let before = journal_count(&link);
    set_lines(
        &mut s,
        &[(6100, "1200.00", "", None), (1010, "", "1100.00", None)],
    );
    let v = s.state_json();
    assert_eq!(v["difference"], "100.00");
    assert_eq!(v["can_save"], false);
    assert_eq!(v["blocked_by"], "unbalanced");
    for intent in [
        SaveIntent::SaveAndClose,
        SaveIntent::SaveAndNew,
        SaveIntent::Save,
    ] {
        assert!(!s.start_save(intent, None));
    }
    assert_eq!(
        s.problems,
        vec![JournalProblem::Unbalanced {
            difference: m("100.00")
        }]
    );
    assert_eq!(s.state_json()["save"]["refused"], "unbalanced");
    assert_eq!(s.saved_count, 0);
    assert_eq!(journal_count(&link), before, "the demo engine got nothing");
    // One line alone is refused too ("Add at least two lines").
    s.clear();
    set_lines(&mut s, &[(6100, "5", "", None)]);
    assert_eq!(s.state_json()["blocked_by"], "too_few_lines");
    assert!(!s.start_save(SaveIntent::Save, None));
}

#[test]
fn a_balanced_entry_saves_numbers_and_publishes_every_account() {
    let (mut s, link, _dir) = screen();
    let first = s.header.current().number.clone();
    assert!(!first.is_empty(), "the next number is suggested");
    assert!(!s.is_dirty(), "the suggestion is part of the blank form");
    set_lines(
        &mut s,
        &[(6100, "1200.00", "", None), (1010, "", "1200.00", None)],
    );
    let mut bus = link.bus().subscribe();
    assert!(
        s.start_save(SaveIntent::SaveAndNew, None),
        "{:?}",
        s.problems
    );
    assert_eq!(s.saved_count, 1);
    let heard = link.bus().take(&mut bus);
    assert!(heard.ledger_touches(account_id(6100)) && heard.ledger_touches(account_id(1010)));
    // Save & New: a blank entry with the next number.
    assert_eq!(s.state_json()["mode"], "new");
    assert_eq!(s.lines.value().used().count(), 0);
    let n: u64 = first.parse().expect("digits");
    assert_eq!(s.header.current().number, (n + 1).to_string());
    // The saved entry reads back exactly.
    let id = s.last_saved.expect("saved");
    assert!(s.open_record(id));
    assert_eq!(debit_of(&s.lines.value().lines[0]), Some(m("1200.00")));
    assert_eq!(credit_of(&s.lines.value().lines[1]), Some(m("1200.00")));
    assert_eq!(s.header.current().number, first);
    assert_eq!(s.state_json()["mode"], "edit");
}

#[test]
fn the_next_line_takes_the_balancing_amount() {
    let (mut s, _link, _dir) = screen();
    set_lines(
        &mut s,
        &[(6100, "1200.00", "", None), (1010, "", "1100.00", None)],
    );
    // The owner picks an account on the blank line: it takes 100.00 credit.
    let key = s.lines.value().lines[2].key;
    let lists = Arc::clone(&s.books.lists);
    let cell = cl_grid::CellPos {
        row: key,
        col: crate::forms::lines_grid::L_ACCOUNT,
    };
    s.lines.form_mut().focus(cell, &*lists);
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[2].account = Some(account_id(1020)));
    s.prefill_balance();
    assert_eq!(credit_of(&s.lines.value().lines[2]), Some(m("100.00")));
    assert_eq!(s.state_json()["difference"], "0.00");
    assert_eq!(s.lines.value().lines.len(), 4, "a blank entry line again");
    // A line that already has an amount keeps it.
    s.lines
        .form_mut()
        .edit_step(|v| v.lines[0].amount = Some(m("1300.00")));
    s.prefill_balance();
    assert_eq!(credit_of(&s.lines.value().lines[2]), Some(m("100.00")));
    // "Balance on this line" re-balances the cursor's line.
    s.balance_here();
    assert_eq!(credit_of(&s.lines.value().lines[2]), Some(m("200.00")));
    s.balance_here();
    assert_eq!(
        credit_of(&s.lines.value().lines[2]),
        Some(m("200.00")),
        "steady"
    );
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.problems);
    // With the other lines balanced on their own there is nothing to put.
    set_lines(
        &mut s,
        &[
            (6100, "5", "", None),
            (1010, "", "5", None),
            (6200, "1", "", None),
        ],
    );
    let third = s.lines.value().lines[2].key;
    s.lines.form_mut().focus(
        cl_grid::CellPos {
            row: third,
            col: crate::forms::lines_grid::L_ACCOUNT,
        },
        &*lists,
    );
    s.balance_here();
    assert_eq!(s.notice, Some("journal.balance_here.none"));
}

#[test]
fn ar_and_ap_lines_need_the_right_name_q206() {
    let (mut s, link, _dir) = screen();
    let customer = contact(&link, ContactType::Customer);
    let vendor = contact(&link, ContactType::Vendor);
    set_lines(
        &mut s,
        &[(1200, "50.00", "", None), (4000, "", "50.00", None)],
    );
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "ar_ap_name_missing");
    set_lines(
        &mut s,
        &[(1200, "50.00", "", Some(vendor)), (4000, "", "50.00", None)],
    );
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "ar_needs_customer");
    set_lines(
        &mut s,
        &[
            (1200, "50.00", "", Some(customer)),
            (2000, "", "50.00", Some(vendor)),
        ],
    );
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "ar_ap_two_lines");
    assert_eq!(s.saved_count, 0);
    set_lines(
        &mut s,
        &[
            (1200, "50.00", "", Some(customer)),
            (4000, "", "50.00", None),
        ],
    );
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.problems);
    let v = s.state_json();
    assert_eq!(v["saved_count"], 1);
    assert_eq!(v["mode"], "edit");
    assert_eq!(s.lines.value().lines[0].customer, Some(customer));
}

#[test]
fn reverse_on_and_adjusting_round_trip_with_the_link_both_ways() {
    let (mut s, _link, _dir) = screen();
    s.header.edit_step(|h| {
        h.adjusting = true;
        h.reverse_on = Some(day(11, 4));
    });
    s.sync_inputs();
    set_lines(
        &mut s,
        &[(6100, "75.00", "", None), (1010, "", "75.00", None)],
    );
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "reverse_not_after");
    s.header.edit_step(|h| h.reverse_on = Some(day(12, 1)));
    s.sync_inputs();
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.problems);
    let v = s.state_json();
    assert_eq!(v["adjusting"], true);
    assert_eq!(v["reverse_on"], "2026-12-01");
    let reversal = s
        .editing
        .as_ref()
        .and_then(|e| e.reversal)
        .map(|(r, _)| r)
        .expect("the reversal");
    let original = s.last_saved.expect("saved");
    s.open_linked(reversal);
    assert_eq!(s.state_json()["reverses"], true);
    assert_eq!(s.header.current().date, Some(day(12, 1)));
    assert_eq!(credit_of(&s.lines.value().lines[0]), Some(m("75.00")));
    assert_eq!(debit_of(&s.lines.value().lines[1]), Some(m("75.00")));
    let back = s.editing.as_ref().and_then(|e| e.reverses).expect("linked");
    assert_eq!(back, original);
}

#[test]
fn closed_period_save_void_and_delete_ask_for_the_password() {
    let (mut s, link, _dir) = screen();
    assert_eq!(s.books.closing, Some(day(9, 30)));
    s.header.edit_step(|h| h.date = Some(day(9, 15)));
    s.sync_inputs();
    set_lines(
        &mut s,
        &[(6100, "10.00", "", None), (1010, "", "10.00", None)],
    );
    assert!(!s.start_save(SaveIntent::Save, None));
    assert!(s.closing_prompt.dialog_open());
    assert!(matches!(
        s.closing_prompt.pending(),
        Some(Pending::Save(SaveIntent::Save))
    ));
    s.closing_prompt.cancel();
    assert_eq!(s.saved_count, 0);
    use crate::forms::closing_prompt::Verb;
    let pw = || Some(Secret::new(DEMO_CLOSING_PASSWORD));
    assert!(s.start_save(SaveIntent::Save, pw()), "{:?}", s.error);
    assert_eq!(s.saved_count, 1);
    // Void: the prompt, then with the password.
    s.command(CommandId::new("edit.void_transaction"));
    assert!(s.void_open);
    s.void_reason = "cleanup".into();
    s.send_void(None);
    assert!(matches!(s.closing_prompt.pending(), Some(Pending::Void(_))));
    assert_eq!(s.closing_prompt.verb(), Verb::Void);
    s.closing_prompt.cancel();
    s.send_void(pw());
    assert_eq!(s.last_action, Some("void"));
    // Delete another closed entry.
    let before = journal_count(&link);
    set_lines(
        &mut s,
        &[(6100, "3.00", "", None), (1010, "", "3.00", None)],
    );
    s.header.edit_step(|h| h.date = Some(day(9, 16)));
    s.sync_inputs();
    assert!(s.start_save(SaveIntent::Save, pw()));
    assert_eq!(journal_count(&link), before + 1);
    s.command(CommandId::new("form.delete_item"));
    assert!(s.delete_open);
    s.send_delete(None);
    assert!(matches!(s.closing_prompt.pending(), Some(Pending::Delete)));
    // The prompt's button says Delete (one flow with the confirmation).
    assert_eq!(s.closing_prompt.verb(), Verb::Delete);
    s.closing_prompt.cancel();
    s.send_delete(pw());
    assert_eq!(s.last_action, Some("delete"));
    assert_eq!(journal_count(&link), before);
}

#[test]
fn prev_next_and_a_stale_edit() {
    let (mut s, link, _dir) = screen();
    set_lines(
        &mut s,
        &[(6100, "1.00", "", None), (1010, "", "1.00", None)],
    );
    assert!(s.start_save(SaveIntent::SaveAndNew, None));
    let id = s.last_saved.expect("saved");
    s.navigate(true);
    assert_eq!(s.state_json()["mode"], "edit");
    assert_eq!(s.editing.as_ref().map(|e| e.id), Some(id), "newest first");
    s.navigate(false);
    assert_eq!(s.state_json()["mode"], "new");
    // Someone else changes the entry: this window's edit is stale.
    assert!(s.open_record(id));
    let mut other = JournalScreen::new(Arc::clone(&link), None, s.today);
    assert!(other.open_record(id));
    other.header.edit_step(|h| h.memo = "changed".into());
    assert!(
        other.start_save(SaveIntent::Save, None),
        "{:?}",
        other.problems
    );
    s.header.edit_step(|h| h.memo = "mine".into());
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["stale"], true);
    // Unsaved edits ask before moving.
    s.navigate(true);
    assert_eq!(s.state_json()["nav_prompt"], true);
    s.answer_nav(crate::windows::PromptAnswer::Discard);
    assert!(!s.is_dirty());
}

#[test]
fn save_as_template_leaves_out_the_date_number_and_reverse_on() {
    let (mut s, link, _dir) = screen();
    let vendor = contact(&link, ContactType::Vendor);
    s.header.edit_step(|h| {
        h.memo = "Insurance".into();
        h.adjusting = true;
        h.reverse_on = Some(day(12, 1));
    });
    set_lines(
        &mut s,
        &[
            (6100, "412.50", "", None),
            (2000, "", "412.50", Some(vendor)),
        ],
    );
    // Ctrl+M opens the Template dialog every form shares (named after the
    // memo); its Save stores the entry's draft.
    assert!(s.command(CommandId::new("form.save_as_template")));
    assert!(
        s.state_json()["template"]["dialog"].is_object(),
        "{}",
        s.state_json()
    );
    assert!(s.template.dialog_save(&link, None), "the memo names it");
    assert_eq!(s.state_json()["template"]["saved_count"], 1);
    let stored = link
        .demo_state()
        .map(|st| crate::company::demo::template_store::list(&st))
        .expect("demo");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].name, "Insurance");
    assert_eq!(
        stored[0].client_form.as_deref(),
        Some(cl_api::client_form::JOURNAL_ENTRY)
    );
    let Ok(TemplateDraft::Journal(e)) = stored[0].draft_for(s.today) else {
        panic!("a journal draft");
    };
    // Ctrl+M again on the saved template's form: Replace or Save as new.
    assert!(s.command(CommandId::new("form.save_as_template")));
    assert_eq!(s.state_json()["template"]["replace_prompt"], true);
    assert_eq!(e.number(), None);
    assert_eq!(e.reverse_on(), None);
    assert!(e.adjusting());
    assert_eq!(e.memo(), Some("Insurance"));
    assert_eq!(e.lines()[1].contact, Some(vendor));
    // Use: a fresh form filled from it, dated as the form decides.
    let mut fresh = JournalScreen::new(Arc::clone(&link), None, s.today);
    let open = TemplateOpen {
        kind: CommandId::new(KIND),
        template: cl_core::ids::TemplateId::parse("e0000000-0000-4000-8000-000000000001")
            .expect("id"),
        version: 3,
        occurrence: Some(day(11, 15)),
        draft: TemplateDraft::Journal(e.clone()),
        warnings: Vec::new(),
    };
    assert!(fresh.open_template(open));
    assert_eq!(fresh.header.current().date, Some(day(11, 15)));
    assert!(fresh.header.current().adjusting);
    assert_eq!(debit_of(&fresh.lines.value().lines[0]), Some(m("412.50")));
    assert_eq!(credit_of(&fresh.lines.value().lines[1]), Some(m("412.50")));
    assert_eq!(
        fresh.state_json()["template"]["from_template"],
        "e0000000-0000-4000-8000-000000000001"
    );
    assert_eq!(fresh.state_json()["template"]["occurrence"], "2026-11-15");
    // The same check gives the same entry back (dated by the form).
    let Ok(TemplateDraft::Journal(again)) = fresh.template_draft() else {
        panic!("a journal draft");
    };
    assert_eq!(again.lines(), e.lines());
    assert_eq!(again.memo(), e.memo());
    assert_eq!(again.adjusting(), e.adjusting());
    assert_eq!(again.txn_date(), day(11, 15));
    // An unbalanced form can't be a template either.
    set_lines(
        &mut fresh,
        &[(6100, "1.00", "", None), (1010, "", "2.00", None)],
    );
    assert!(fresh.template_draft().is_err());
}

#[test]
fn a_delete_retried_after_a_lost_reply_takes_not_found_as_done() {
    let (mut s, link, _dir) = screen();
    set_lines(
        &mut s,
        &[(6100, "75.00", "", None), (1010, "", "75.00", None)],
    );
    assert!(
        s.start_save(SaveIntent::SaveAndNew, None),
        "{:?}",
        s.problems
    );
    let id = s.last_saved.expect("saved");
    assert!(s.open_record(id));
    // The delete's reply was lost: the outcome is unknown.
    s.action_failed(&ApiError::Timeout, Pending::Delete);
    assert_eq!(s.delete_unknown, Some(id));
    // It had taken: the entry is gone at the engine.
    let gone = link
        .demo_state()
        .map(|mut st| crate::company::demo::journal::delete(&mut st, id, None));
    assert!(matches!(gone, Some(Ok(()))));
    // The retry hears "not found": done, the form moves on.
    s.send_delete(None);
    assert_eq!(s.last_action, Some("delete"), "{:?}", s.error);
    assert_eq!(s.delete_unknown, None);
    // Without a lost reply "not found" stays an error.
    let nf = || Err(ApiError::NotFound { code: None });
    assert!(save::delete_answer(nf(), false).is_err());
    assert!(save::delete_answer(nf(), true).is_ok());
    // A definite refusal forgets the unknown delete.
    s.action_failed(&ApiError::NotFound { code: None }, Pending::Delete);
    assert_eq!(s.delete_unknown, None);
}

#[test]
fn a_dropped_template_account_fills_in_empty() {
    let (mut s, _link, _dir) = screen();
    let e = cl_api::NewJournalEntry::new(
        s.today,
        Some("Insurance".into()),
        cl_api::Basis::Both,
        vec![
            cl_api::JeLine {
                account: cl_api::MISSING_ACCOUNT,
                side: cl_api::Side::Debit(m("412.50")),
                description: None,
                tag: None,
                contact: None,
            },
            cl_api::JeLine {
                account: account_id(1010),
                side: cl_api::Side::Credit(m("412.50")),
                description: None,
                tag: None,
                contact: None,
            },
        ],
    )
    .expect("entry");
    s.fill_from_template(TemplateDraft::Journal(e), &[]);
    let lines = &s.lines.value().lines;
    assert_eq!(lines[0].account, None, "the placeholder shows empty");
    assert_eq!(debit_of(&lines[0]), Some(m("412.50")), "the amount stays");
    assert_eq!(lines[1].account, Some(account_id(1010)));
}

#[test]
fn a_template_keeps_its_report_basis() {
    let (mut s, _link, _dir) = screen();
    let line = |n, side| cl_api::JeLine {
        account: account_id(n),
        side,
        description: None,
        tag: None,
        contact: None,
    };
    let e = cl_api::NewJournalEntry::new(
        s.today,
        Some("Accrual".into()),
        cl_api::Basis::Accrual,
        vec![
            line(6100, cl_api::Side::Debit(m("10.00"))),
            line(1010, cl_api::Side::Credit(m("10.00"))),
        ],
    )
    .expect("entry");
    s.fill_from_template(TemplateDraft::Journal(e), &[]);
    let Ok(TemplateDraft::Journal(again)) = s.template_draft() else {
        panic!("a journal draft");
    };
    assert_eq!(again.basis(), cl_api::Basis::Accrual, "Save as Template");
    // Save builds the entry the same way (no edit: the template's basis).
    assert_eq!(s.new_basis, cl_api::Basis::Accrual);
    // A blank form is Both again.
    s.new_form(false);
    assert_eq!(s.new_basis, cl_api::Basis::Both);
}

#[test]
fn esc_reverts_the_focused_field_then_lets_the_window_close() {
    let (mut s, _link, _dir) = screen();
    s.header.edit(field::MEMO, |h| h.memo = "typed".into());
    s.focused = Some(field::MEMO);
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.header.current().memo, "");
    assert!(!s.command(CommandId::new("form.revert")));
}
