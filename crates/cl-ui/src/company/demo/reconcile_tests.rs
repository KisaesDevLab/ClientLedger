//! The demo engine's reconciliations hold the documented contract and
//! reproduce the mockup's figures.

use super::*;
use cl_api::ReconcileRefusal;

fn opened() -> DemoState {
    let mut s = DemoState::new();
    let first = s.companies.first().map(|c| c.tenant_id);
    s.open = first;
    s
}

fn statement(s: &mut DemoState) -> ReconStart {
    let (fees, interest) = charge_accounts(s);
    ReconStart {
        account: super::super::account_id(OPERATING),
        statement_date: day(10, 31),
        statement_ending_balance: m("86902.13"),
        service_charge: Some(StatementCharge {
            amount: m("48.00"),
            date: day(10, 31),
            account: fees.expect("bank fees"),
        }),
        interest: Some(StatementCharge {
            amount: m("12.84"),
            date: day(10, 31),
            account: interest.expect("interest income"),
        }),
    }
}

fn refusal(e: &ApiError) -> Option<ReconcileRefusal> {
    cl_api::reconcile_refusal(e)
}

#[test]
fn begin_reproduces_the_mockup() {
    let mut s = opened();
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    assert_eq!(r.beginning_balance, m("71436.20"));
    assert_eq!(r.cleared_deposits.count, 6);
    assert_eq!(r.cleared_deposits.total, m("40107.84"));
    assert_eq!(r.cleared_payments.count, 8);
    assert_eq!(r.cleared_payments.total, m("-24229.73"));
    assert_eq!(r.cleared_balance, m("87314.31"));
    assert_eq!(r.difference, m("-412.18"));
    let lines = r.lines.expect("lines");
    let unticked: Vec<_> = lines
        .iter()
        .filter(|l| !l.cleared && !l.after_statement_date)
        .collect();
    assert_eq!(unticked.len(), 1);
    assert_eq!(unticked[0].number.as_deref(), Some("4409"));
    assert!(!unticked[0].from_bank_feed);
    // Later lines are on the sheet, flagged, and not pre-checked.
    assert_eq!(lines.iter().filter(|l| l.after_statement_date).count(), 2);
    assert!(
        lines
            .iter()
            .filter(|l| l.after_statement_date)
            .all(|l| !l.cleared)
    );
}

#[test]
fn strict_zero_then_finish_posts_charges_once() {
    let mut s = opened();
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    let e = complete(&mut s, r.id, None).expect_err("not zero");
    assert_eq!(
        refusal(&e),
        Some(ReconcileRefusal::DifferenceNotZero {
            difference: Some(m("-412.18"))
        })
    );
    let check = line_id(12);
    let t = tick(&mut s, r.id, &[(check, true)]).expect("tick");
    assert!(t.lines.is_none(), "summary view");
    assert!(t.ties_out());
    complete(&mut s, r.id, None).expect("finish");
    let done = get(&mut s, r.id).expect("get");
    assert_eq!(done.status, ReconStatus::Complete);
    assert!(done.service_charge.and_then(|h| h.posted).is_some());
    assert!(done.interest.and_then(|h| h.posted).is_some());
    // Ticks and a second finish are 409.
    let e = tick(&mut s, r.id, &[(check, false)]).expect_err("finished");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::NotInProgress));
    // The next Begin starts at the statement's ending balance, without
    // the reconciled lines.
    let next = start(
        &mut s,
        &ReconStart {
            statement_date: day(11, 30),
            service_charge: None,
            interest: None,
            ..req
        },
    )
    .expect("next");
    assert_eq!(next.beginning_balance, m("86902.13"));
    assert_eq!(next.lines.expect("lines").len(), 2);
}

#[test]
fn one_in_progress_per_account_and_cancel() {
    let mut s = opened();
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    let e = start(&mut s, &req).expect_err("second");
    assert_eq!(
        refusal(&e),
        Some(ReconcileRefusal::InProgress {
            reconciliation: Some(r.id)
        })
    );
    cancel(&mut s, r.id).expect("cancel");
    start(&mut s, &req).expect("again");
}

#[test]
fn undo_keep_and_discard_restore_exact_states() {
    let mut s = opened();
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    tick(&mut s, r.id, &[(line_id(12), true)]).expect("tick");
    complete(&mut s, r.id, None).expect("finish");
    let ticks = |r: &Reconciliation| -> Vec<JournalLineId> {
        r.lines
            .iter()
            .flatten()
            .filter(|l| l.cleared)
            .map(|l| l.journal_line)
            .collect()
    };
    let at_finish = ticks(&get(&mut s, r.id).expect("get"));
    // The 09/30 one isn't the latest.
    let e = undo(&mut s, rec_id(1), UndoMode::KeepChecks, None).expect_err("older");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::NotLatest));
    undo(&mut s, r.id, UndoMode::KeepChecks, None).expect("keep");
    let reopened = get(&mut s, r.id).expect("get");
    assert_eq!(reopened.status, ReconStatus::InProgress);
    assert_eq!(ticks(&reopened), at_finish);
    // Posted charges stay posted (their cleared lines still count).
    assert!(reopened.service_charge.and_then(|h| h.posted).is_some());
    assert!(reopened.ties_out());
    let e = patch(
        &mut s,
        r.id,
        &ReconPatch {
            service_charge: Some(None),
            ..ReconPatch::default()
        },
    )
    .expect_err("posted");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::ChargePosted));
    // An undo while one is in progress: 409.
    let e = undo(&mut s, rec_id(1), UndoMode::Discard, None).expect_err("in progress");
    assert!(matches!(
        refusal(&e),
        Some(ReconcileRefusal::NotLatest | ReconcileRefusal::InProgress { .. })
    ));
    cancel(&mut s, r.id).expect("discard the reopened one");
    // Discard of the 09/30 one (books closed through 09/30: the closing
    // password): the account starts from zero.
    let e = undo(&mut s, rec_id(1), UndoMode::Discard, None).expect_err("closed");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD);
    undo(&mut s, rec_id(1), UndoMode::Discard, Some(&pw)).expect("discard");
    let fresh = start(&mut s, &req).expect("fresh");
    assert_eq!(fresh.beginning_balance, Money::ZERO);
    let rows = discrepancy(&mut s, req.account).expect("report").rows;
    assert_eq!(
        rows.iter()
            .filter(|r| r.kind == "reconciliation_undone")
            .count(),
        2
    );
}

#[test]
fn closed_period_charge_needs_the_password() {
    let mut s = opened();
    if let Some(c) = s.companies.first_mut() {
        c.closed_through = Some(day(11, 1));
    }
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    assert!(r.posting_locked);
    tick(&mut s, r.id, &[(line_id(12), true)]).expect("tick");
    let e = complete(&mut s, r.id, None).expect_err("locked");
    assert!(matches!(e, ApiError::ClosedPeriod(_)), "{e:?}");
    let wrong = Secret::new("nope");
    assert_eq!(
        complete(&mut s, r.id, Some(&wrong)),
        Err(ApiError::ClosingPasswordRejected)
    );
    let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD);
    complete(&mut s, r.id, Some(&pw)).expect("with the password");
}

#[test]
fn card_balance_is_flipped_and_ties_out() {
    let mut s = opened();
    let (fees, _) = charge_accounts(&mut s);
    let card = super::super::account_id(CARD);
    let r = start(
        &mut s,
        &ReconStart {
            account: card,
            statement_date: day(10, 31),
            statement_ending_balance: m("1038.82"),
            service_charge: Some(StatementCharge {
                amount: m("18.25"),
                date: day(10, 31),
                account: fees.expect("fees"),
            }),
            interest: None,
        },
    )
    .expect("start");
    assert_eq!(r.statement_ending_balance, m("-1038.82"));
    let all: Vec<_> = r
        .lines
        .iter()
        .flatten()
        .map(|l| (l.journal_line, true))
        .collect();
    let t = tick(&mut s, r.id, &all).expect("tick");
    assert!(t.ties_out(), "{:?}", t.difference);
}

#[test]
fn recategorized_reconciled_line_is_listed() {
    let mut s = opened();
    assert!(!recategorize_reconciled(&mut s), "nothing reconciled yet");
    let req = statement(&mut s);
    let r = start(&mut s, &req).expect("start");
    tick(&mut s, r.id, &[(line_id(12), true)]).expect("tick");
    complete(&mut s, r.id, None).expect("finish");
    assert!(recategorize_reconciled(&mut s));
    let rows = discrepancy(&mut s, req.account).expect("report").rows;
    let changed = rows.iter().find(|r| r.kind == "changed").expect("row");
    assert_eq!(changed.effect, Some(Money::ZERO));
    assert_eq!(changed.fields, vec!["accountId".to_string()]);
}
