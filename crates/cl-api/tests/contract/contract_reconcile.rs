//! Reconcile against the live engine (P27 plan §7; engine U-188 to U-192).
//! Gated on `reconcile-exact-v1`, `reconcile-begin-v1` and
//! `reconcile-undo-v2` (the discrepancy tests also on
//! `reconcile-discrepancy-v1`): PENDING on engines before the P27 pin bump.
//!
//! Every test reconciles its own fresh bank (or card) account, so the
//! "one in progress per account" rule and the history never cross tests.
//! Closed-period tests run in tenant B (`h.s3()`) and clear the date again.

use std::time::Instant;

use cl_api::{
    AccountType, ApiError, Capabilities, ClosedPeriodAuthorization, FeedChoice, Method,
    PostingOptions, ReconPatch, ReconStart, ReconStatus, ReconcileRefusal, Reconciliation,
    ReconciliationId, Session, StatementCharge, UndoMode, reconcile_refusal,
};
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use crate::common::{account_fields, date, je2, key, m, post};
use crate::contract_feed_accept::import;
use crate::support::Harness;

fn gated(h: &Harness) -> bool {
    h.require_cap(Capabilities::RECONCILE_EXACT, "U-188")
        && h.require_cap(Capabilities::RECONCILE_BEGIN, "U-189")
        && h.require_cap(Capabilities::RECONCILE_UNDO, "U-190")
}

fn fresh(h: &Harness, s: &Session, t: AccountType, detail: &str, prefix: &str) -> AccountId {
    let mut f = account_fields(t, prefix);
    f.detail_type = Some(detail.to_string());
    h.run(s.accounts().create(&f, key()))
        .unwrap_or_else(|e| panic!("create account: {e}"))
        .record
        .id
}

struct Books {
    bank: AccountId,
    income: AccountId,
    expense: AccountId,
}

fn books(h: &Harness, s: &Session) -> Books {
    Books {
        bank: fresh(h, s, AccountType::Asset, "bank", "Recon Bank"),
        income: fresh(
            h,
            s,
            AccountType::Revenue,
            "service_fee_income",
            "Recon Income",
        ),
        expense: fresh(h, s, AccountType::Expense, "bank_charges", "Recon Fees"),
    }
}

/// Money in (`dr` the bank) or out, on `on`; returns the entry.
fn entry(h: &Harness, s: &Session, b: &Books, on: CalendarDate, amount: &str) -> TransactionId {
    let e = if let Some(out) = amount.strip_prefix('-') {
        je2(on, "recon", b.expense, b.bank, out)
    } else {
        je2(on, "recon", b.bank, b.income, amount)
    };
    post(h, s, &e).record.id
}

fn start(
    h: &Harness,
    s: &Session,
    account: AccountId,
    on: CalendarDate,
    ending: &str,
) -> Result<Reconciliation, ApiError> {
    h.run(s.reconciliations().start(&ReconStart {
        account,
        statement_date: on,
        statement_ending_balance: m(ending),
        service_charge: None,
        interest: None,
    }))
}

fn get(h: &Harness, s: &Session, id: ReconciliationId) -> Reconciliation {
    h.run(s.reconciliations().get(id)).expect("get")
}

fn line_of(r: &Reconciliation, txn: TransactionId) -> JournalLineId {
    r.lines
        .as_ref()
        .and_then(|ls| ls.iter().find(|l| l.transaction == txn))
        .map(|l| l.journal_line)
        .expect("the entry's line is on the worksheet")
}

fn tick(h: &Harness, s: &Session, id: ReconciliationId, lines: &[JournalLineId]) -> Reconciliation {
    let ticks: Vec<_> = lines.iter().map(|l| (*l, true)).collect();
    h.run(s.reconciliations().tick(id, &ticks)).expect("tick")
}

fn refusal(e: &ApiError) -> Option<ReconcileRefusal> {
    reconcile_refusal(e)
}

/// AC1: Finish needs exactly zero. 0.01 off is refused with the engine's
/// difference; ticking the last line ties out and Finish works.
pub fn finish_needs_exact_zero(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let dep = entry(h, s, &b, date(2026, 3, 3), "100.00");
    let fee = entry(h, s, &b, date(2026, 3, 9), "-0.01");
    let r = start(h, s, b.bank, date(2026, 3, 31), "99.99").expect("start");
    assert_eq!(r.beginning_balance, Money::ZERO, "first reconciliation");
    let r = tick(h, s, r.id, &[line_of(&get(h, s, r.id), dep)]);
    assert_eq!(r.difference, m("-0.01"));
    assert!(r.lines.is_none(), "ticks answer the summary view");
    let e = h
        .run(s.reconciliations().complete(r.id, None))
        .expect_err("0.01 off");
    assert_eq!(
        refusal(&e),
        Some(ReconcileRefusal::DifferenceNotZero {
            difference: Some(m("-0.01"))
        }),
        "{e:?}"
    );
    let r = tick(h, s, r.id, &[line_of(&get(h, s, r.id), fee)]);
    assert!(r.ties_out());
    assert_eq!(r.cleared_deposits.count, 1);
    assert_eq!(r.cleared_payments.total, m("0.01"));
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    assert_eq!(get(h, s, r.id).status, ReconStatus::Complete);
}

/// Concurrency: a second Begin on the account is 409 naming the first;
/// ticks after Finish are 409 `RECONCILIATION_NOT_IN_PROGRESS`.
pub fn second_start_and_ticks_after_finish_are_409(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let dep = entry(h, s, &b, date(2026, 4, 2), "50.00");
    let first = start(h, s, b.bank, date(2026, 4, 30), "50.00").expect("start");
    let e = start(h, s, b.bank, date(2026, 4, 30), "50.00").expect_err("second");
    assert_eq!(
        refusal(&e),
        Some(ReconcileRefusal::InProgress {
            reconciliation: Some(first.id)
        }),
        "{e:?}"
    );
    let line = line_of(&get(h, s, first.id), dep);
    tick(h, s, first.id, &[line]);
    h.run(s.reconciliations().complete(first.id, None))
        .expect("finish");
    let e = h
        .run(s.reconciliations().tick(first.id, &[(line, false)]))
        .expect_err("finished");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::NotInProgress), "{e:?}");
    // A retried finish gets a 409 too (the screen then reads the status).
    let e = h
        .run(s.reconciliations().complete(first.id, None))
        .expect_err("again");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::NotInProgress), "{e:?}");
}

/// Two Begins at once on the same account: one wins, one gets 409.
pub fn parallel_starts_one_wins(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = h.s1.clone();
    let s2 = h.s2();
    let b = books(h, &s);
    let req = ReconStart {
        account: b.bank,
        statement_date: date(2026, 5, 31),
        statement_ending_balance: m("0"),
        service_charge: None,
        interest: None,
    };
    let (one, two) = (s.reconciliations(), s2.reconciliations());
    let (a, c) = h.run(async { tokio::join!(one.start(&req), two.start(&req)) });
    let wins = usize::from(a.is_ok()) + usize::from(c.is_ok());
    assert_eq!(wins, 1, "{a:?} / {c:?}");
    let loser = a.err().or(c.err()).expect("one refused");
    assert!(matches!(
        refusal(&loser),
        Some(ReconcileRefusal::InProgress { .. })
    ));
}

/// Service charge and interest entered at Begin count at once, post once
/// at Finish (cleared, reconciled) and are never posted again.
pub fn charges_count_before_and_post_once_on_finish(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let dep = entry(h, s, &b, date(2026, 6, 4), "1000.00");
    let on = date(2026, 6, 30);
    let r = h
        .run(s.reconciliations().start(&ReconStart {
            account: b.bank,
            statement_date: on,
            statement_ending_balance: m("964.84"),
            service_charge: Some(StatementCharge {
                amount: m("48.00"),
                date: on,
                account: b.expense,
            }),
            interest: Some(StatementCharge {
                amount: m("12.84"),
                date: on,
                account: b.income,
            }),
        }))
        .expect("start");
    // 0 − 48.00 + 12.84 counted before any tick.
    assert_eq!(r.cleared_balance, m("-35.16"));
    assert!(r.service_charge.is_some_and(|c| c.posted.is_none()));
    let r = tick(h, s, r.id, &[line_of(&get(h, s, r.id), dep)]);
    assert!(r.ties_out(), "{:?}", r.difference);
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    let done = get(h, s, r.id);
    let svc = done
        .service_charge
        .and_then(|c| c.posted)
        .expect("service charge posted");
    let int = done
        .interest
        .and_then(|c| c.posted)
        .expect("interest posted");
    for t in [svc, int] {
        h.run(s.transactions().get(t)).expect("posted entry exists");
    }
    // A posted charge can't be changed through the reconciliation after a
    // reopen, and isn't posted again by the next finish.
    h.run(s.reconciliations().undo(r.id, UndoMode::KeepChecks, None))
        .expect("undo keep");
    let e = h
        .run(s.reconciliations().patch(
            r.id,
            &ReconPatch {
                service_charge: Some(None),
                ..ReconPatch::default()
            },
        ))
        .expect_err("posted");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::ChargePosted), "{e:?}");
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish again");
    let again = get(h, s, r.id);
    assert_eq!(again.service_charge.and_then(|c| c.posted), Some(svc));
    assert_eq!(again.interest.and_then(|c| c.posted), Some(int));
}

/// A charge dated on or before the closing date: `postingLocked`, Finish
/// is refused without the password and works with it.
pub fn closed_period_charge_needs_password(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::CLOSING_PASSWORD, "U-15") {
        return;
    }
    let s = h.s3();
    let b = books(h, &s);
    let on = date(2025, 12, 31);
    let r = h
        .run(s.reconciliations().start(&ReconStart {
            account: b.bank,
            statement_date: on,
            statement_ending_balance: m("-5.00"),
            service_charge: Some(StatementCharge {
                amount: m("5.00"),
                date: on,
                account: b.expense,
            }),
            interest: None,
        }))
        .expect("start");
    // The closing date first, then the password (the contract_batch
    // precedent): once a password is set, changing the date needs it too.
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": "2025-12-31"})),
    ))
    .expect("set the closing date");
    let pw = Secret::new(crate::support::unique("recon-close"));
    h.run(s.set_closing_password(None, Some(&pw), key()))
        .expect("password");
    assert!(get(h, &s, r.id).posting_locked);
    let e = h
        .run(s.reconciliations().complete(r.id, None))
        .expect_err("locked");
    assert!(
        matches!(
            e,
            ApiError::ClosedPeriod(_) | ApiError::ClosingPasswordRequired
        ),
        "{e:?}"
    );
    let auth = ClosedPeriodAuthorization::with_password(pw.clone());
    let done = h.run(s.reconciliations().complete(r.id, Some(&auth)));
    h.run(s.set_closing_password(Some(&pw), None, key()))
        .expect("clear password");
    h.run(s.raw_mapped(
        Method::Put,
        "/api/v1/company/settings",
        Some(&json!({"lockDate": null})),
    ))
    .expect("clear the closing date");
    done.expect("finish with the password");
}

/// Q-100: accepted bank-file lines dated on or before the statement date
/// are pre-checked; a manual entry and a check whose bank line came after
/// the statement date are not.
pub fn bank_feed_lines_are_prechecked(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::FEED_EXACT, "U-164") {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let rows = import(
        h,
        s,
        b.bank,
        &[
            (date(2026, 7, 3), "-500.00", "DEPOSIT ONE"),
            (date(2026, 7, 9), "25.00", "FEE TWO"),
            (date(2026, 8, 2), "75.00", "CHECK 101"),
        ],
    );
    for r in &rows[..2] {
        let choice = FeedChoice {
            account: Some(if r.amount.is_negative() {
                b.income
            } else {
                b.expense
            }),
            ..FeedChoice::default()
        };
        h.run(s.feed().categorize(r.id, &choice, None, key()))
            .expect("accept");
    }
    let manual = entry(h, s, &b, date(2026, 7, 15), "10.00");
    let check = entry(h, s, &b, date(2026, 7, 28), "-75.00");
    if h.require_cap(Capabilities::FEED_MATCH_V2, "U-166") {
        h.run(s.feed().match_line(rows[2].id, check, None, key()))
            .expect("match");
    }
    let r = start(h, s, b.bank, date(2026, 7, 31), "475.00").expect("start");
    assert!(r.ties_out());
    let lines = r.lines.expect("lines");
    let ticked: Vec<_> = lines.iter().filter(|l| l.cleared).collect();
    assert_eq!(ticked.len(), 2, "only the two accepted lines");
    assert!(ticked.iter().all(|l| l.from_bank_feed));
    for t in [manual, check] {
        let l = lines.iter().find(|l| l.transaction == t).expect("on sheet");
        assert!(!l.cleared);
    }
}

/// Q-60: a card's printed balance (positive owed) ties out on Start and
/// after a PATCH of the ending balance.
pub fn card_printed_balance_ties_out_on_start_and_patch(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let card = fresh(h, s, AccountType::Liability, "credit_card", "Recon Card");
    let fuel = fresh(h, s, AccountType::Expense, "auto", "Recon Fuel");
    let charge = post(h, s, &je2(date(2026, 8, 5), "fuel", fuel, card, "84.17"))
        .record
        .id;
    let r = start(h, s, card, date(2026, 8, 31), "80.00").expect("start");
    assert_eq!(r.statement_ending_balance, m("-80.00"), "flipped");
    let r = tick(h, s, r.id, &[line_of(&get(h, s, r.id), charge)]);
    assert_eq!(r.difference, m("4.17"));
    let r = h
        .run(s.reconciliations().patch(
            r.id,
            &ReconPatch {
                statement_ending_balance: Some(m("84.17")),
                ..ReconPatch::default()
            },
        ))
        .expect("patch");
    assert!(r.ties_out(), "{:?}", r.difference);
}

/// Undo restores exactly: keep checks reopens with the ticks at Finish;
/// discard gives back the state before Begin; earlier reconciliations are
/// untouched.
pub fn undo_keep_and_discard_restore_exact_states(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let a = entry(h, s, &b, date(2026, 9, 2), "40.00");
    let r1 = start(h, s, b.bank, date(2026, 9, 30), "40.00").expect("start 1");
    tick(h, s, r1.id, &[line_of(&get(h, s, r1.id), a)]);
    h.run(s.reconciliations().complete(r1.id, None))
        .expect("finish 1");
    let x = entry(h, s, &b, date(2026, 10, 3), "10.00");
    let y = entry(h, s, &b, date(2026, 10, 5), "-3.00");
    let _z = entry(h, s, &b, date(2026, 10, 7), "-2.00");
    let r2 = start(h, s, b.bank, date(2026, 10, 31), "47.00").expect("start 2");
    assert_eq!(r2.beginning_balance, m("40.00"));
    let sheet = get(h, s, r2.id);
    tick(h, s, r2.id, &[line_of(&sheet, x), line_of(&sheet, y)]);
    h.run(s.reconciliations().complete(r2.id, None))
        .expect("finish 2");
    // The older one can't be undone first.
    let e = h
        .run(s.reconciliations().undo(r1.id, UndoMode::KeepChecks, None))
        .expect_err("not latest");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::NotLatest), "{e:?}");
    let ticks = |r: &Reconciliation| {
        let mut v: Vec<_> = r
            .lines
            .iter()
            .flatten()
            .filter(|l| l.cleared)
            .map(|l| l.transaction)
            .collect();
        v.sort();
        v
    };
    let at_finish = ticks(&get(h, s, r2.id));
    h.run(s.reconciliations().undo(r2.id, UndoMode::KeepChecks, None))
        .expect("keep");
    let reopened = get(h, s, r2.id);
    assert_eq!(reopened.status, ReconStatus::InProgress);
    assert_eq!(ticks(&reopened), at_finish);
    h.run(s.reconciliations().complete(r2.id, None))
        .expect("finish again");
    h.run(s.reconciliations().undo(r2.id, UndoMode::Discard, None))
        .expect("discard");
    let history = h.run(s.reconciliations().history(b.bank)).expect("history");
    assert_eq!(history.len(), 1, "discarded");
    assert_eq!(history[0].id, r1.id);
    assert_eq!(history[0].status, ReconStatus::Complete);
    // Begin again: the October lines are all uncleared, as before Begin.
    let r3 = start(h, s, b.bank, date(2026, 10, 31), "47.00").expect("start 3");
    assert_eq!(r3.beginning_balance, m("40.00"));
    assert!(r3.lines.iter().flatten().all(|l| !l.cleared));
}

/// Undo is Admin only: a bookkeeper with banking write gets 403
/// `RECONCILE_ADMIN_REQUIRED`.
pub fn undo_admin_only(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let a = entry(h, s, &b, date(2026, 2, 2), "9.00");
    let r = start(h, s, b.bank, date(2026, 2, 28), "9.00").expect("start");
    tick(h, s, r.id, &[line_of(&get(h, s, r.id), a)]);
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    // A bookkeeper with full banking rights is still not an Admin.
    let (user, bk) = h.bookkeeper();
    let r2 = h
        .run(s.raw(
            Method::Put,
            &format!("/api/v1/company/users/{user}/permissions"),
            &[],
            Some(&json!({ "overrides": { "banking": "full" } })),
        ))
        .expect("permissions");
    assert_eq!(r2.status, 200, "{}", r2.body);
    let e = h
        .run(bk.reconciliations().undo(r.id, UndoMode::KeepChecks, None))
        .expect_err("not an admin");
    assert_eq!(refusal(&e), Some(ReconcileRefusal::AdminRequired), "{e:?}");
}

/// AC2: recategorizing a reconciled expense and a backdated entry appear
/// in the discrepancy report (field names only, the effect exact).
pub fn discrepancy_lists_changed_and_backdated_entries(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::RECONCILE_DISCREPANCY, "U-191") {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let other = fresh(h, s, AccountType::Expense, "office_expenses", "Recon Other");
    let fee = je2(date(2026, 1, 9), "recon", b.expense, b.bank, "12.00");
    let made = post(h, s, &fee);
    let r = start(h, s, b.bank, date(2026, 1, 31), "-12.00").expect("start");
    tick(h, s, r.id, &[line_of(&get(h, s, r.id), made.record.id)]);
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    let moved = je2(date(2026, 1, 9), "recon", other, b.bank, "12.00");
    h.run(s.transactions().update_journal_entry(
        made.record.id,
        &made.version,
        &moved,
        &PostingOptions::default(),
        key(),
    ))
    .expect("recategorize");
    entry(h, s, &b, date(2026, 1, 20), "-1.00");
    let rep = h
        .run(s.reconciliations().discrepancy(b.bank))
        .expect("report");
    let changed = rep
        .rows
        .iter()
        .find(|r| r.kind == "changed")
        .expect("a changed row");
    assert_eq!(changed.effect, Some(Money::ZERO));
    assert_ne!(changed.fields, Vec::<String>::new());
    assert!(rep.rows.iter().any(|r| r.kind == "added_after"));
}

/// U-192: editing a ticked entry's memo keeps its tick.
pub fn editing_a_ticked_entry_keeps_its_tick(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let e = je2(date(2026, 11, 2), "before", b.bank, b.income, "30.00");
    let made = post(h, s, &e);
    let r = start(h, s, b.bank, date(2026, 11, 30), "30.00").expect("start");
    tick(h, s, r.id, &[line_of(&get(h, s, r.id), made.record.id)]);
    let edited = je2(date(2026, 11, 2), "after", b.bank, b.income, "30.00");
    h.run(s.transactions().update_journal_entry(
        made.record.id,
        &made.version,
        &edited,
        &PostingOptions::default(),
        key(),
    ))
    .expect("edit memo");
    let after = get(h, s, r.id);
    let line = after
        .lines
        .iter()
        .flatten()
        .find(|l| l.transaction == made.record.id)
        .expect("still on the sheet");
    assert!(line.cleared, "the tick survived the edit");
    assert!(after.ties_out());
}

/// Large data: a worksheet of many lines reads within the budget (logged).
pub fn large_worksheet_reads_quickly(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    for i in 0..200u32 {
        entry(h, s, &b, date(2026, 12, 1 + (i % 28)), "1.00");
    }
    let r = start(h, s, b.bank, date(2026, 12, 31), "200.00").expect("start");
    let t = Instant::now();
    let full = get(h, s, r.id);
    eprintln!("    reconcile GET of 200 lines: {:?}", t.elapsed());
    let all: Vec<_> = full
        .lines
        .iter()
        .flatten()
        .map(|l| l.journal_line)
        .collect();
    assert_eq!(all.len(), 200);
    let r = tick(h, s, r.id, &all);
    assert!(r.ties_out());
    h.run(s.reconciliations().cancel(r.id)).expect("cancel");
}
