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
