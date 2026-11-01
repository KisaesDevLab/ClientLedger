//! Refund form: the amount defaults to the source's remaining string, a
//! check queues to print by default (Q-P22d), and the problems (P22 plan
//! §6.2, §6.3).

use super::*;

fn m(s: &str) -> Money {
    Money::parse_wire(s).expect("money")
}

fn d(day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, 11, day).expect("date")
}

fn source(remaining: &str) -> RefundSourceChoice {
    RefundSourceChoice {
        id: TransactionId::parse("4b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
        kind: RefundSourceKind::CreditMemo,
        remaining: m(remaining),
    }
}

fn client() -> ContactId {
    ContactId::parse("3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
}

fn bank() -> AccountId {
    AccountId::parse("2b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id")
}

#[test]
fn the_amount_defaults_to_the_remaining_string_and_check_prints_later() {
    let f = RefundForm::for_source(client(), source("399.75"), Some(d(5)), Some(bank()));
    assert_eq!(f.amount(), Some(m("399.75")));
    assert!(f.print_later());
    let plan = f.save_plan(&RefundContext::default()).expect("plan");
    assert_eq!(plan.amount.to_wire(), m("399.75").to_wire());
    assert_eq!(plan.check, CheckPlan::PrintLater);
    assert_eq!(plan.method.wire(), "check");
}

#[test]
fn a_typed_amount_wins_and_a_new_source_resets_it() {
    let mut f = RefundForm::for_source(client(), source("399.75"), Some(d(5)), Some(bank()));
    f.set_amount(Some(m("100.00")));
    assert_eq!(f.amount(), Some(m("100.00")));
    f.set_source(Some(source("50.00")));
    assert_eq!(f.amount(), Some(m("50.00")));
    f.set_client(None);
    assert_eq!(f.source(), None);
}

#[test]
fn problems_in_form_order() {
    let f = RefundForm::new(None, None);
    assert_eq!(
        f.save_plan(&RefundContext::default()),
        Err(vec![
            RefundProblem::NoClient,
            RefundProblem::NoSource,
            RefundProblem::NoAmount,
            RefundProblem::NoAccount,
            RefundProblem::NoDate,
        ])
    );
    let mut f = RefundForm::for_source(client(), source("10.00"), Some(d(5)), Some(bank()));
    f.set_print_later(false);
    assert_eq!(
        f.save_plan(&RefundContext::default()),
        Err(vec![RefundProblem::CheckNoOrPrintLater])
    );
    f.set_check_no("1042");
    assert_eq!(
        f.save_plan(&RefundContext::default()).expect("plan").check,
        CheckPlan::Number("1042".into())
    );
    f.set_amount(Some(m("0.00")));
    assert_eq!(
        f.save_plan(&RefundContext::default()),
        Err(vec![RefundProblem::NoAmount])
    );
}

#[test]
fn other_methods_send_no_check_and_closing_needs_the_password() {
    let mut f = RefundForm::for_source(client(), source("10.00"), Some(d(5)), Some(bank()));
    f.set_method(RefundMethodChoice::Card);
    assert!(!f.print_later());
    let closed = RefundContext {
        closing_date: Some(d(30)),
        password_given: false,
    };
    assert_eq!(f.save_plan(&closed), Err(vec![RefundProblem::Closing]));
    let ok = RefundContext {
        password_given: true,
        ..closed
    };
    let plan = f.save_plan(&ok).expect("plan");
    assert_eq!(plan.check, CheckPlan::NotACheck);
    assert_eq!(plan.method.wire(), "credit_card");
    // Back to check: queued again.
    f.set_method(RefundMethodChoice::Check);
    assert!(f.print_later());
}

#[test]
fn the_attempt_is_stable_until_an_edit() {
    let mut f = RefundForm::for_source(client(), source("10.00"), Some(d(5)), Some(bank()));
    let a = f
        .save_plan(&RefundContext::default())
        .expect("plan")
        .attempt;
    assert_eq!(
        f.save_plan(&RefundContext::default())
            .expect("plan")
            .attempt,
        a
    );
    f.set_memo("Returned leash");
    assert_ne!(f.attempt(), a);
    assert!(!format!("{f:?}").contains("leash"));
}
