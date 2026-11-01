//! Statements form: defaults, the start date rule and runs of at most 200
//! with stable chunk attempts (P22 plan §6.3).

use super::*;

fn d(m: u32, day: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, m, day).expect("date")
}

fn client(n: usize) -> ContactId {
    ContactId::parse(&format!("3a0c4e8e-6f6e-4c1d-8a37-{n:012}")).expect("id")
}

#[test]
fn defaults_are_open_item_and_only_clients_with_a_balance() {
    let f = StatementsForm::new(Some(d(11, 30)));
    assert_eq!(f.format, StatementFormatChoice::OpenItem);
    assert!(f.open_only && !f.shows_start());
    assert_eq!(f.effective_start(), None);
    assert_eq!(f.run_plan(), Err(vec![StatementsProblem::NoClients]));
}

#[test]
fn balance_forward_starts_on_the_first_of_the_month_unless_typed() {
    let mut f = StatementsForm::new(Some(d(11, 30)));
    f.format = StatementFormatChoice::BalanceForward;
    assert_eq!(f.effective_start(), Some(d(11, 1)));
    f.start = Some(d(12, 1));
    f.toggle(client(1));
    assert_eq!(f.run_plan(), Err(vec![StatementsProblem::StartAfterAsOf]));
    f.start = Some(d(10, 1));
    assert_eq!(f.run_plan().expect("plan").start, Some(d(10, 1)));
    assert_eq!(StatementFormatChoice::Transaction.wire(), "transaction");
}

#[test]
fn runs_hold_at_most_200_clients_and_keep_their_attempt() {
    let mut f = StatementsForm::new(Some(d(11, 30)));
    let all: Vec<ContactId> = (0..450).map(client).collect();
    f.select_only(&all);
    let plan = f.run_plan().expect("plan");
    let sizes: Vec<usize> = plan.chunks.iter().map(|c| c.clients.len()).collect();
    assert_eq!(sizes, vec![200, 200, 50]);
    assert_eq!(plan.chunks[2].index, 2);
    // A retry of the same plan keeps every chunk's attempt (its key).
    assert_eq!(f.run_plan().expect("plan"), plan);
    f.toggle(client(0));
    assert_ne!(
        f.run_plan().expect("plan").chunks[0].attempt,
        plan.chunks[0].attempt
    );
    assert!(!f.is_selected(client(0)));
    assert_eq!(f.selected_count(), 449);
}
