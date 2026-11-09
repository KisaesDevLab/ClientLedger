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
    // Totals are in the books sign: payments are negative (U-188).
    assert_eq!(r.cleared_payments.total, m("-0.01"));
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
    // Unique texts: the shared helper reads one page of every waiting
    // row, which earlier tests may have filled; find ours by search.
    let tag = crate::support::unique("RECON");
    let texts = [
        format!("{tag} DEPOSIT ONE"),
        format!("{tag} FEE TWO"),
        format!("{tag} CHECK 101"),
    ];
    let rows = import(
        h,
        s,
        b.bank,
        &[
            (date(2026, 7, 3), "-500.00", texts[0].as_str()),
            (date(2026, 7, 9), "25.00", texts[1].as_str()),
            (date(2026, 8, 2), "75.00", texts[2].as_str()),
        ],
    );
    let rows = if rows.len() == 3 {
        rows
    } else {
        let mut found = h
            .run(s.feed().list(&cl_api::FeedQuery {
                waiting_only: true,
                search: Some(tag.clone()),
                limit: 500,
                ..cl_api::FeedQuery::default()
            }))
            .expect("list")
            .rows;
        found.sort_by_key(|r| r.date);
        found
    };
    assert_eq!(rows.len(), 3, "the three bank lines wait");
    let mut rows = rows;
    rows.sort_by_key(|r| r.date);
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
    assert!(r.lines.is_none(), "start answers the summary view");
    let lines = get(h, s, r.id).lines.expect("lines");
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
    let sheet = get(h, s, r3.id);
    assert_eq!(sheet.lines.iter().flatten().count(), 3);
    assert!(sheet.lines.iter().flatten().all(|l| !l.cleared));
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
    if !gated(h) || !h.require_cap(Capabilities::LINE_IDENTITY, "U-192") {
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

/// U-193 (review E3): a statement dated on or before the latest finished
/// one is 409 `RECONCILE_STATEMENT_DATE_ORDER`, on Begin and on PATCH.
pub fn statement_date_must_follow_the_last_finished(h: &Harness) {
    if !gated(h) || !h.require_cap(Capabilities::RECONCILE_HARDENING, "U-193") {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let dep = entry(h, s, &b, date(2026, 5, 4), "20.00");
    let first = start(h, s, b.bank, date(2026, 5, 31), "20.00").expect("start");
    tick(h, s, first.id, &[line_of(&get(h, s, first.id), dep)]);
    h.run(s.reconciliations().complete(first.id, None))
        .expect("finish");
    for on in [date(2026, 5, 31), date(2026, 5, 1)] {
        let e = start(h, s, b.bank, on, "20.00").expect_err("not after 05/31");
        assert_eq!(
            refusal(&e),
            Some(ReconcileRefusal::StatementDateOrder),
            "{e:?}"
        );
    }
    let next = start(h, s, b.bank, date(2026, 6, 30), "20.00").expect("after 05/31");
    let e = h
        .run(s.reconciliations().patch(
            next.id,
            &ReconPatch {
                statement_date: Some(date(2026, 5, 31)),
                ..ReconPatch::default()
            },
        ))
        .expect_err("PATCH to 05/31");
    assert_eq!(
        refusal(&e),
        Some(ReconcileRefusal::StatementDateOrder),
        "{e:?}"
    );
}

/// P27 QA-1, QA-2, QA-5: a `checking` account (as the practice company's
/// 1010) reconciles; its journal-entry rows carry the entry memo; once
/// finished, moving the entry's date and voiding it are refused as the
/// reconciled refusal (409 `RECONCILED_TRANSACTION` with `reconcile-qa-v1`,\n/// the code-less 400s before, U-195), and the memo
/// still changes.
pub fn checking_account_rows_and_reconciled_refusals(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let mut b = books(h, s);
    b.bank = fresh(h, s, AccountType::Asset, "checking", "Recon Checking");
    let words = "check 4417 \u{b7} Elm Street Accountants";
    let e = je2(date(2026, 2, 3), words, b.expense, b.bank, "25.00");
    let made = post(h, s, &e);
    let r = start(h, s, b.bank, date(2026, 2, 28), "-25.00").expect("start");
    let sheet = get(h, s, r.id);
    let line = sheet
        .lines
        .iter()
        .flatten()
        .find(|l| l.transaction == made.record.id)
        .expect("on the sheet")
        .clone();
    assert_eq!(line.payee, None, "{line:?}");
    assert_eq!(line.memo.as_deref(), Some(words), "{line:?}");
    tick(h, s, r.id, &[line.journal_line]);
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    // `reconcile-qa-v1` (U-195): the engine's own 409 code; older engines'
    // code-less 400 is read by its text.
    let qa = h.caps.has(Capabilities::RECONCILE_QA);
    let reconciled = |e: &ApiError| match e {
        ApiError::Conflict {
            reason: cl_api::ConflictReason::Reconciled,
            code,
            ..
        } => {
            !qa || code
                .as_ref()
                .is_some_and(|c| c.as_str() == "RECONCILED_TRANSACTION")
        }
        _ => false,
    };
    if qa {
        // U-194: a checking register takes checks; U-196: the summary names
        // this (the latest) reconciliation.
        let page = h
            .run(s.register().page(
                b.bank,
                &cl_api::RegisterQuery::new(date(2026, 1, 1), date(2026, 12, 31)),
                1,
                50,
            ))
            .expect("register");
        assert!(
            page.allowed.contains(&cl_api::EntryType::Expense),
            "{:?}",
            page.allowed
        );
        let sum = h.run(s.register().summary(b.bank, None)).expect("summary");
        assert_eq!(sum.last_reconciled, Some(date(2026, 2, 28)));
    }
    let moved = je2(date(2026, 2, 4), words, b.expense, b.bank, "25.00");
    let err = h
        .run(s.transactions().update_journal_entry(
            made.record.id,
            &made.version,
            &moved,
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("the date is reconciled");
    assert!(reconciled(&err), "{err:?}");
    let err = h
        .run(
            s.transactions()
                .void(made.record.id, "qa", &PostingOptions::default(), key()),
        )
        .expect_err("void is reconciled");
    assert!(reconciled(&err), "{err:?}");
    let memo = je2(date(2026, 2, 3), "memo only", b.expense, b.bank, "25.00");
    h.run(s.transactions().update_journal_entry(
        made.record.id,
        &made.version,
        &memo,
        &PostingOptions::default(),
        key(),
    ))
    .expect("a memo change is fine");
}

/// P27 QA-13: changing only the category of a reconciled entry, as the
/// desktop forms send it (the full entry again, bank line unchanged).
/// The practice company's 10/01 deposit is `Dr 1010 / Cr 10200 A/R`; moving
/// its A/R line to an income account, and a reconciled check's owner's-draw
/// line to an expense, changed no reconciled line. Before
/// `reconciled-category-edit-v1` (U-198) the engine kept every balance-sheet
/// line of a reconciled entry, so both were refused 409
/// `RECONCILED_TRANSACTION` (root cause); with it they save and the
/// reconciled bank line keeps its tick. An expense-to-expense change of a
/// reconciled check saves on every engine.
pub fn category_only_edit_of_a_reconciled_entry(h: &Harness) {
    if !gated(h) {
        return;
    }
    let s = &h.s1;
    let b = books(h, s);
    let fixed = h.caps.has(Capabilities::RECONCILED_CATEGORY_EDIT);
    let receivable = h
        .run(s.accounts().list_all(&cl_api::AccountFilter::default()))
        .expect("accounts")
        .into_iter()
        .find(|a| a.system_tag.as_deref() == Some("accounts_receivable"))
        .expect("the A/R account")
        .id;
    let draw = crate::common::account(h, s, AccountType::Equity, "Recon Draw").id;
    let other = fresh(h, s, AccountType::Expense, "office_expenses", "Recon Other");
    let on = date(2026, 3, 2);
    // The deposit (journal entry) and two checks, all cleared and finished.
    // The A/R line names the customer, as the practice company's does.
    let who = crate::contract_journal_entries::customer(h, s);
    let deposit_to = |other: AccountId| {
        let mut l = crate::common::line(other, cl_api::Side::Credit(m("4688.74")));
        l.contact = Some(who);
        cl_api::NewJournalEntry::new(
            on,
            Some("Payment received".into()),
            cl_api::Basis::Both,
            vec![
                crate::common::line(b.bank, cl_api::Side::Debit(m("4688.74"))),
                l,
            ],
        )
        .expect("balanced")
    };
    let dep = post(h, s, &deposit_to(receivable));
    let check_to = |account: AccountId, amount: &str| {
        cl_api::BankPosting::check(cl_api::CheckDraft {
            bank: b.bank,
            date: on,
            payee: None,
            payee_name: "Recon Payee".into(),
            check_number: None,
            print_later: false,
            payee_address: None,
            memo: Some("check".into()),
            printed_memo: None,
            tag_ids: Vec::new(),
            draft_attachment: None,
            amount: m(amount),
            lines: vec![crate::contract_bank_postings::split(account, amount, None)],
        })
        .expect("check")
    };
    let mk = |p: &cl_api::BankPosting| {
        h.run(
            s.transactions()
                .create_bank_posting(p, &PostingOptions::default(), key()),
        )
        .expect("check")
    };
    let draw_check = mk(&check_to(draw, "40.00"));
    let fee_check = mk(&check_to(b.expense, "40.00"));
    let r = start(h, s, b.bank, date(2026, 3, 31), "4608.74").expect("start");
    let sheet = get(h, s, r.id);
    let ticks: Vec<_> = [dep.record.id, draw_check.record.id, fee_check.record.id]
        .iter()
        .map(|t| line_of(&sheet, *t))
        .collect();
    tick(h, s, r.id, &ticks);
    h.run(s.reconciliations().complete(r.id, None))
        .expect("finish");
    let reconciled = |e: &ApiError| {
        matches!(
            e,
            ApiError::Conflict {
                reason: cl_api::ConflictReason::Reconciled,
                ..
            }
        )
    };
    let still_ticked = |txn: TransactionId| {
        get(h, s, r.id)
            .lines
            .iter()
            .flatten()
            .any(|l| l.transaction == txn && l.cleared)
    };

    // The deposit: A/R line -> income, bank line and date untouched.
    let recat = deposit_to(b.income);
    let res = h.run(s.transactions().update_journal_entry(
        dep.record.id,
        &dep.version,
        &recat,
        &PostingOptions::default(),
        key(),
    ));
    if fixed {
        res.expect("an A/R-to-income change of a reconciled deposit saves");
        assert!(still_ticked(dep.record.id), "the bank line keeps its tick");
    } else {
        let e = res.expect_err("U-198 not in this engine: every balance-sheet line is kept");
        assert!(reconciled(&e), "{e:?}");
    }

    // A check to owner's draw -> an expense.
    let res = h.run(s.transactions().update_bank_posting(
        draw_check.record.id,
        &draw_check.version,
        &check_to(other, "40.00"),
        &PostingOptions::default(),
        key(),
    ));
    if fixed {
        res.expect("an equity-to-expense change of a reconciled check saves");
        assert!(still_ticked(draw_check.record.id));
    } else {
        let e = res.expect_err("U-198 not in this engine");
        assert!(reconciled(&e), "{e:?}");
    }

    // A check expense -> another expense: saves on every engine.
    h.run(s.transactions().update_bank_posting(
        fee_check.record.id,
        &fee_check.version,
        &check_to(other, "40.00"),
        &PostingOptions::default(),
        key(),
    ))
    .expect("an expense-to-expense change of a reconciled check saves");
    assert!(still_ticked(fee_check.record.id));

    // Changing the bank amount is still refused.
    let fee_now = h
        .run(s.transactions().get(fee_check.record.id))
        .expect("get");
    let more = check_to(other, "41.00");
    let e = h
        .run(s.transactions().update_bank_posting(
            fee_check.record.id,
            &fee_now.version,
            &more,
            &PostingOptions::default(),
            key(),
        ))
        .expect_err("the bank amount is reconciled");
    assert!(reconciled(&e), "{e:?}");
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
