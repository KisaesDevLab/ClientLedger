//! Loan Payment tests (no engine): the demo stand-in feeds the screen.

use super::*;
use crate::screens::card_charge::tests::{lines_of, link, money, posted, today};

fn screen(link: &Arc<CompanyLink>) -> LoanPaymentScreen {
    let mut s = LoanPaymentScreen::new(Arc::clone(link), None, today());
    s.poll();
    s
}

fn lakeview(s: &LoanPaymentScreen) -> ContactId {
    s.books.contact("Lakeview Credit Union").expect("lender")
}

/// The lender statement fixture (synthetic).
fn statement() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../../../fixtures/sample/ozark_ridge/lender_statement.json"
    ))
    .expect("fixture")
}

fn type_amounts(s: &mut LoanPaymentScreen, payment: &str, interest: &str) {
    let (p, i) = (money(payment), money(interest));
    s.header.edit_step(|h| {
        h.payment = Some(p);
        h.interest = Some(i);
    });
    s.sync_inputs();
}

#[test]
fn the_lender_fills_the_accounts_never_the_amounts() {
    let (_d, link) = link();
    let mut s = screen(&link);
    assert_eq!(s.title(), "Loan Payment");
    let lender = lakeview(&s);
    s.pick_lender(NameOutcome::Picked(lender));
    s.poll();
    let f = s.header.current();
    assert_eq!(
        f.loan,
        s.books.by_number("2200"),
        "the loan from the last payment"
    );
    assert!(f.interest_account.is_some(), "the interest account too");
    assert_eq!(f.payment, None, "amounts are never prefilled (Q-191)");
    assert_eq!(f.interest, None);
    assert_eq!(s.state()["prefill"]["applied"], true);
}

#[test]
fn the_prefill_never_replaces_a_pay_from_already_there() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let savings = s.books.by_number("1020").expect("1020");
    let checking = s.books.by_number("1010").expect("1010");
    s.header.edit_step(|h| h.pay_from = Some(savings));
    s.apply_prefill(LoanPrefill {
        loan: s.books.by_number("2200"),
        interest_account: None,
        pay_from: Some(checking),
    });
    let f = s.header.current();
    assert_eq!(f.pay_from, Some(savings), "the owner's Pay From stays");
    assert_eq!(f.loan, s.books.by_number("2200"));
    // A blank Pay From is filled.
    s.header.edit_step(|h| h.pay_from = None);
    s.apply_prefill(LoanPrefill {
        loan: None,
        interest_account: None,
        pay_from: Some(checking),
    });
    assert_eq!(s.header.current().pay_from, Some(checking));
}

#[test]
fn principal_is_exact_and_the_engine_balance_matches_the_lender() {
    let (_d, link) = link();
    let mut s = screen(&link);
    let v = statement();
    let text = |k: &str| v[k].as_str().expect("string").to_string();
    s.pick_lender(NameOutcome::Picked(lakeview(&s)));
    s.poll();
    type_amounts(&mut s, &text("payment"), &text("interest"));
    assert_eq!(s.compose().principal(), Some(money(&text("principal"))));
    assert_eq!(s.compose().principal(), Some(money("684.57")));
    // The engine's balance before the payment is the statement's.
    let loan = s.header.current().loan.expect("loan");
    assert_eq!(
        s.balances.shown(loan, Some(AccountClass::OtherLiability)),
        Some(money(&text("balanceBefore")))
    );
    let after = money(&text("balanceAfter"));
    s.header.edit_step(|h| h.lender_balance = Some(after));
    s.sync_inputs();
    let before = posted(&link);
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    assert_eq!(posted(&link), before + 1);
    s.poll();
    assert_eq!(s.state()["lender_match"], "matches");
    assert_eq!(
        s.balances.shown(loan, Some(AccountClass::OtherLiability)),
        Some(after),
        "the engine's balance, not before − principal"
    );
    let (_, lines) = lines_of(&link, s.flow.last.expect("saved").id);
    assert!(lines.contains(&("2200".into(), money("684.57"), Money::ZERO)));
    assert!(lines.contains(&("1010".into(), Money::ZERO, money("892.00"))));
    // Every window showing the bank, the loan and the interest hears it.
    assert_eq!(s.state()["published_accounts"], 4);
}

#[test]
fn a_negative_principal_is_refused_with_nothing_sent() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.pick_lender(NameOutcome::Picked(lakeview(&s)));
    s.poll();
    type_amounts(&mut s, "892.00", "900.00");
    assert_eq!(s.state()["principal_negative"], true);
    let before = posted(&link);
    assert!(!s.start_save(SaveIntent::SaveAndClose, None));
    assert_eq!(posted(&link), before);
    assert_eq!(s.state()["save"]["refused"], "principal_negative");
}

#[test]
fn a_lender_balance_that_differs_is_reported_and_the_payment_voids() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.pick_lender(NameOutcome::Picked(lakeview(&s)));
    s.poll();
    type_amounts(&mut s, "892.00", "207.43");
    let lender = money("30515.40");
    s.header.edit_step(|h| h.lender_balance = Some(lender));
    s.sync_inputs();
    assert!(s.start_save(SaveIntent::Save, None));
    s.poll();
    assert_eq!(s.state()["lender_match"], "differs");
    assert_eq!(s.lender_result, Some(LenderMatch::Differs(money("0.03"))));
    assert!(s.command(CommandId::new("edit.void_transaction")));
    s.flow.void_reason = "test".into();
    s.void_now(None);
    assert_eq!(s.state()["last_action"], "void");
}

#[test]
fn interest_must_be_typed_even_when_zero() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.pick_lender(NameOutcome::Picked(lakeview(&s)));
    s.poll();
    let p = money("100.00");
    s.header.edit_step(|h| h.payment = Some(p));
    s.sync_inputs();
    assert!(!s.start_save(SaveIntent::Save, None));
    assert_eq!(s.state()["save"]["refused"], "interest_missing");
}

#[test]
fn a_saved_payment_opens_with_prev_exactly_and_changes_in_place() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.pick_lender(NameOutcome::Picked(lakeview(&s)));
    s.poll();
    type_amounts(&mut s, "892.00", "194.23");
    assert!(s.start_save(SaveIntent::SaveAndNew, None), "{}", s.state());
    let id = s.flow.last.expect("saved").id;
    s.command(CommandId::new("form.prev_record"));
    assert_eq!(s.flow.editing.map(|e| e.id), Some(id));
    assert_eq!(s.state()["read_only"], Value::Null, "{}", s.state());
    let f = s.compose();
    assert_eq!(f.payment, Some(money("892.00")));
    assert_eq!(f.interest, Some(money("194.23")));
    assert_eq!(f.principal(), Some(money("697.77")), "payment − interest");
    assert_eq!(f.loan, s.books.by_number("2200"));
    assert!(!s.is_dirty());
    let before = posted(&link);
    type_amounts(&mut s, "892.00", "188.41");
    assert!(s.start_save(SaveIntent::Save, None), "{}", s.state());
    assert_eq!(posted(&link), before, "changed in place");
    let (_, lines) = lines_of(&link, id);
    assert!(lines.contains(&("2200".into(), money("703.59"), Money::ZERO)));
}

#[test]
fn esc_reverts_the_focused_field_first() {
    let (_d, link) = link();
    let mut s = screen(&link);
    s.focused = Some(field::PAYMENT);
    s.inputs.payment.text = "5".into();
    assert!(
        s.command(CommandId::new("form.revert")),
        "typed text dropped"
    );
    assert_eq!(s.inputs.payment.text, "");
    s.focused = Some(field::MEMO);
    s.header.edit(field::MEMO, |h| h.memo = "x".into());
    assert!(s.command(CommandId::new("form.revert")));
    assert!(!s.is_dirty());
    assert!(!s.command(CommandId::new("form.revert")));
}
