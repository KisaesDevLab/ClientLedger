use std::sync::Arc;

use cl_core::{CalendarDate, Money, Secret};
use cl_model::forms::SaveIntent;

use super::*;

fn money(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn account(n: u32) -> AccountId {
    crate::company::demo::account_id(n)
}

fn screen() -> (TransferScreen, Arc<CompanyLink>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let link = Arc::new(CompanyLink::demo(dir.path()));
    let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
    let s = TransferScreen::new(Arc::clone(&link), None, today);
    (s, link, dir)
}

fn fill(s: &mut TransferScreen, from: u32, to: u32, amount: &str) {
    s.set_account(true, account(from));
    s.set_account(false, account(to));
    s.amount_text = amount.into();
    assert!(s.amount_left());
}

#[test]
fn a_transfer_saves_exactly_and_both_balances_hear_it() {
    let (mut s, link, _dir) = screen();
    assert!(s.loaded);
    let accounts: Vec<_> = s.transfer_accounts().iter().map(|a| a.id).collect();
    assert!(accounts.contains(&account(1010)) && accounts.contains(&account(2050)));
    assert!(!accounts.contains(&account(10150)), "no system accounts");
    assert!(!accounts.contains(&account(1200)), "no receivables");
    fill(&mut s, 1010, 2050, "300.00");
    let before = s.balances;
    let mut other = link.bus().subscribe();
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.error);
    assert_eq!(s.saved_count, 1);
    let heard = link.bus().take(&mut other);
    assert!(heard.ledger_touches(account(1010)) && heard.ledger_touches(account(2050)));
    // The engine's balances move by exactly the amount.
    let after = s.balances;
    assert_eq!(
        before[0].and_then(|b| b.checked_sub(money("300.00")).ok()),
        after[0]
    );
    assert_eq!(
        before[1].and_then(|b| b.checked_add(money("300.00")).ok()),
        after[1],
        "the card owes 300.00 less"
    );
    let v = s.state_json();
    assert_eq!(v["amount"], "300.00");
    assert!(v["txn_id"].is_string());
    // Void it (scenario cleanup).
    s.command(CommandId::new("edit.void_transaction"));
    s.send_void("cleanup", None);
    assert_eq!(s.last_action, Some("void"));
}

#[test]
fn an_edit_tells_the_account_it_moved_off_and_a_stale_edit_is_refused() {
    let (mut s, link, _dir) = screen();
    fill(&mut s, 1010, 2050, "40.00");
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.error);
    let id = s.last_saved.expect("saved");
    // Read it again (as the register's Edit would) and move To elsewhere.
    s.clear();
    assert!(s.open_record(id));
    assert_eq!(s.state_json()["mode"], "edit");
    let other = s
        .transfer_accounts()
        .iter()
        .map(|a| a.id)
        .find(|a| *a != account(1010) && *a != account(2050))
        .expect("a third account");
    s.set_account(false, other);
    let mut bus = link.bus().subscribe();
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.error);
    let heard = link.bus().take(&mut bus);
    assert!(heard.ledger_touches(account(2050)), "the old To hears it");
    assert!(heard.ledger_touches(other) && heard.ledger_touches(account(1010)));
    // Someone else changes it meanwhile: this window's version is stale.
    let mut elsewhere = TransferScreen::new(Arc::clone(&link), None, s.today);
    assert!(elsewhere.open_record(id));
    elsewhere.amount_text = "41.00".into();
    assert!(elsewhere.amount_left());
    assert!(elsewhere.start_save(SaveIntent::Save, None));
    s.amount_text = "42.00".into();
    assert!(s.amount_left());
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["stale"], true);
}

#[test]
fn a_retry_reuses_the_key_only_for_the_same_save() {
    let (mut s, _link, _dir) = screen();
    let posting = BankPosting::transfer(TransferDraft {
        from: account(1010),
        to: account(2050),
        date: s.today,
        amount: money("5.00"),
        memo: None,
    })
    .expect("posting");
    let id = |edit, password| SaveIdentity {
        posting: posting.clone(),
        edit,
        password,
    };
    let k = s.key_for(&id(None, false));
    s.save_failed(ApiError::Timeout);
    assert!(
        s.key_for(&id(None, false)) == k,
        "a lost reply resends the key"
    );
    assert!(
        s.key_for(&id(None, true)) != k,
        "with the password: a new key"
    );
    let txn = TransactionId::parse("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id");
    let e = s.key_for(&id(Some((txn, RecordVersion::Fingerprint([0; 32]))), false));
    let newer = Some((txn, RecordVersion::Fingerprint([1; 32])));
    assert!(
        s.key_for(&id(newer, false)) != e,
        "another version: a new key"
    );
}

#[test]
fn prev_on_unsaved_work_asks_and_esc_reverts_the_focused_field() {
    use crate::windows::PromptAnswer;
    let (mut s, _link, _dir) = screen();
    fill(&mut s, 1010, 2050, "12.00");
    assert!(s.start_save(SaveIntent::Save, None), "{:?}", s.error);
    s.amount_text = "13.00".into();
    assert!(s.amount_left());
    assert!(s.is_dirty());
    // Prev: the standard question, not a notice.
    s.navigate(true);
    assert_eq!(s.state_json()["nav_prompt"], true);
    s.answer_nav(PromptAnswer::Keep);
    assert!(s.is_dirty() && s.nav_prompt.is_none());
    // Esc on the amount: back to the saved 12.00, then nothing to revert.
    s.focused = Some(field::AMOUNT);
    assert!(s.command(CommandId::new("form.revert")));
    assert_eq!(s.state_json()["amount"], "12.00");
    assert!(!s.is_dirty());
    assert!(
        !s.command(CommandId::new("form.revert")),
        "clean: the shell closes"
    );
    // Discard moves on without saving.
    s.amount_text = "14.00".into();
    assert!(s.amount_left());
    s.navigate(true);
    s.answer_nav(PromptAnswer::Discard);
    assert!(!s.is_dirty());
    assert_eq!(s.saved_count, 1);
    // Save saves first, then moves.
    s.amount_text = "15.00".into();
    assert!(s.amount_left());
    s.navigate(true);
    s.answer_nav(PromptAnswer::Save);
    assert_eq!(s.saved_count, 2);
    assert!(!s.is_dirty());
    assert!(
        s.last_action == Some("load") || s.notice == Some("transfer.nav.oldest"),
        "it moved on after the save"
    );
}

#[test]
fn a_new_account_from_another_window_appears_once_the_form_is_clean() {
    let (mut s, link, _dir) = screen();
    let before = s.accounts.len();
    {
        let mut demo = link.demo_state().expect("demo");
        let chart = demo.chart.get_or_insert_with(crate::company::demo::chart);
        let mut extra = chart
            .iter()
            .find(|a| a.id == account(1010))
            .cloned()
            .expect("checking");
        extra.id = account(1099);
        extra.fields.account_number = Some("1099".into());
        chart.push(extra);
    }
    fill(&mut s, 1010, 2050, "1.00");
    link.bus().publish(crate::data_bus::Change::Accounts);
    s.hear_changes();
    assert_eq!(s.accounts.len(), before, "dirty: the list waits");
    s.clear();
    s.hear_changes();
    assert_eq!(s.accounts.len(), before + 1);
    assert!(s.transfer_accounts().iter().any(|a| a.id == account(1099)));
}

#[test]
fn the_same_account_is_refused_and_nothing_is_sent() {
    let (mut s, _link, _dir) = screen();
    fill(&mut s, 1010, 1010, "5.00");
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state_json()["save"]["refused"], "same_account");
    assert_eq!(s.saved_count, 0);
}

#[test]
fn a_closed_period_transfer_asks_for_the_password() {
    let (mut s, _link, _dir) = screen();
    fill(&mut s, 1010, 2050, "10.00");
    s.closing = CalendarDate::from_ymd(2026, 10, 31).ok();
    s.form
        .edit_step(|f| f.date = CalendarDate::from_ymd(2026, 10, 15).ok());
    assert!(!s.start_save(SaveIntent::Save, None));
    assert!(s.closing_prompt.dialog_open());
    assert_eq!(s.saved_count, 0);
    assert!(matches!(
        s.closing_prompt.pending(),
        Some(Pending::Save(SaveIntent::Save))
    ));
    s.closing_prompt.cancel();
    // The password goes with the save; the demo engine takes the right one.
    assert!(
        s.start_save(
            SaveIntent::Save,
            Some(Secret::new(
                crate::company::demo::register::DEMO_CLOSING_PASSWORD
            ))
        ),
        "{:?}",
        s.error
    );
    assert_eq!(s.saved_count, 1);
}
