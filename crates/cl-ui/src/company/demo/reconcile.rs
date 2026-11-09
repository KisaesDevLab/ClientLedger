//! The demo engine's reconciliations (P27): the whole documented contract
//! in memory (U-188 to U-191), on synthetic statements that reproduce the
//! `Reconcile` mockup exactly.
//!
//! - **1010 Operating Checking** was last reconciled on 09/30/2026 at
//!   71,436.20 (the sample's cleared balance then). Its October statement
//!   holds the mockup's lines: five deposits and seven payments from bank
//!   files (pre-checked), check 4409 to Monett Tire & Lube (412.18, typed
//!   by hand, unchecked) and two lines after the statement date. With the
//!   statement's 48.00 service charge and 12.84 interest and an ending
//!   balance of 86,902.13 the worksheet opens at 6 of 6 · 40,107.84,
//!   8 of 9 · 24,229.73, cleared 87,314.31 and difference −412.18.
//! - **2050 Business Credit Card** has never been reconciled: three
//!   charges and a payment (the card scenario).
//!
//! The worksheet lines are the statement's own, not the sample register's:
//! the sample has many more October lines than the mockup shows. The
//! demo plays the engine, so it does the engine's arithmetic here (exact
//! `Money`); the screen never does.

use std::collections::{BTreeMap, BTreeSet};

use cl_api::{
    ApiError, BeginningDrift, ClearedSide, DiscrepancyReport, DiscrepancyRow, HeldCharge,
    ReconHistoryRow, ReconLine, ReconPatch, ReconStart, ReconStatus, Reconciliation,
    ReconciliationId, StatementCharge, UndoMode,
};
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money, Secret, Timestamp};
use serde_json::json;

use super::DemoState;
use super::register::{DemoJournal, DemoLine, check_closing};

/// 1010 Operating Checking.
pub const OPERATING: u32 = 1010;
/// 2050 Business Credit Card.
pub const CARD: u32 = 2050;
/// The service-charge account of the demo (6050 Bank and Merchant Fees).
pub const BANK_FEES: &str = "6050";
/// The interest account (Interest Income).
pub const INTEREST_INCOME: &str = "4900";

/// One statement line of the demo bank.
#[derive(Clone, Debug)]
struct PoolLine {
    n: u32,
    account: u32,
    date: CalendarDate,
    kind: &'static str,
    number: Option<&'static str>,
    payee: &'static str,
    amount: Money,
    from_feed: bool,
}

/// A reconciliation as the demo engine stores it.
#[derive(Clone, Debug)]
struct Rec {
    id: ReconciliationId,
    account: AccountId,
    status: ReconStatus,
    statement_date: CalendarDate,
    beginning: Money,
    ending: Money,
    service_charge: Option<HeldCharge>,
    interest: Option<HeldCharge>,
    /// Worksheet: line → ticked.
    lines: BTreeMap<JournalLineId, bool>,
    started: Timestamp,
    completed: Option<Timestamp>,
}

/// The demo's reconciliations.
#[derive(Clone, Debug, Default)]
pub struct DemoRecon {
    recs: Vec<Rec>,
    /// Lines cleared by a finished reconciliation.
    reconciled: BTreeSet<JournalLineId>,
    /// Discrepancy rows recorded (an undo, a demo edit).
    discrepancies: Vec<(AccountId, DiscrepancyRow)>,
    next: u32,
    seeded: bool,
}

fn m(s: &str) -> Money {
    Money::parse_wire(s).unwrap_or_else(|_| unreachable!("a demo amount"))
}

fn day(mo: u32, d: u32) -> CalendarDate {
    CalendarDate::from_ymd(2026, mo, d).unwrap_or_else(|_| unreachable!("a demo date"))
}

fn line_id(n: u32) -> JournalLineId {
    JournalLineId::parse(&format!("d0000000-0000-4000-8000-c{n:011}"))
        .unwrap_or_else(|_| unreachable!("a demo id"))
}

fn txn_id(n: u32) -> TransactionId {
    TransactionId::parse(&format!("d0000000-0000-4000-8000-e{n:011}"))
        .unwrap_or_else(|_| unreachable!("a demo id"))
}

fn rec_id(n: u32) -> ReconciliationId {
    ReconciliationId::parse(&format!("d0000000-0000-4000-8000-f{n:011}"))
        .unwrap_or_else(|_| unreachable!("a demo id"))
}

#[allow(clippy::too_many_arguments)]
fn pl(
    n: u32,
    account: u32,
    date: CalendarDate,
    kind: &'static str,
    number: Option<&'static str>,
    payee: &'static str,
    amount: &str,
    from_feed: bool,
) -> PoolLine {
    PoolLine {
        n,
        account,
        date,
        kind,
        number,
        payee,
        amount: m(amount),
        from_feed,
    }
}

/// The statements' lines (synthetic, the mockup's figures).
fn pool() -> Vec<PoolLine> {
    let o = OPERATING;
    let c = CARD;
    vec![
        pl(
            1,
            o,
            day(10, 2),
            "check",
            Some("4401"),
            "Ozark Feed & Seed",
            "-2318.60",
            true,
        ),
        pl(
            2,
            o,
            day(10, 3),
            "deposit",
            None,
            "Branson Hills HOA",
            "8450.00",
            true,
        ),
        pl(
            3,
            o,
            day(10, 5),
            "journal_entry",
            Some("ACH"),
            "Payroll \u{b7} 10/05",
            "-9120.00",
            true,
        ),
        pl(
            4,
            o,
            day(10, 8),
            "deposit",
            None,
            "Hollister Family",
            "12600.00",
            true,
        ),
        pl(
            5,
            o,
            day(10, 9),
            "check",
            Some("4404"),
            "Midwest Equipment Rental",
            "-1850.00",
            true,
        ),
        pl(
            6,
            o,
            day(10, 14),
            "deposit",
            None,
            "Table Rock Marina",
            "6210.00",
            true,
        ),
        pl(
            7,
            o,
            day(10, 15),
            "expense",
            Some("ACH"),
            "Lakeside Power Co-op",
            "-286.14",
            true,
        ),
        pl(
            8,
            o,
            day(10, 19),
            "journal_entry",
            Some("ACH"),
            "Payroll \u{b7} 10/19",
            "-9120.00",
            true,
        ),
        pl(
            9,
            o,
            day(10, 21),
            "deposit",
            None,
            "Neosho Dental Group",
            "4385.00",
            true,
        ),
        pl(
            10,
            o,
            day(10, 22),
            "check",
            Some("4406"),
            "Ozark Office Supply",
            "-274.51",
            true,
        ),
        pl(
            11,
            o,
            day(10, 24),
            "check",
            Some("4407"),
            "Riverbend Fuel",
            "-1212.48",
            true,
        ),
        pl(
            12,
            o,
            day(10, 27),
            "check",
            Some("4409"),
            "Monett Tire & Lube",
            "-412.18",
            false,
        ),
        pl(
            13,
            o,
            day(10, 28),
            "deposit",
            None,
            "Branson Hills HOA",
            "8450.00",
            true,
        ),
        pl(
            14,
            o,
            day(11, 2),
            "deposit",
            None,
            "Kimberling Marina",
            "2744.67",
            true,
        ),
        pl(
            15,
            o,
            day(11, 4),
            "check",
            Some("4417"),
            "Elm Street Accountants",
            "-493.15",
            false,
        ),
        pl(
            21,
            c,
            day(10, 5),
            "card_charge",
            None,
            "Riverbend Fuel",
            "-184.17",
            true,
        ),
        pl(
            22,
            c,
            day(10, 12),
            "card_charge",
            None,
            "Ozark Office Supply",
            "-96.40",
            true,
        ),
        pl(
            23,
            c,
            day(10, 20),
            "transfer",
            None,
            "Payment \u{b7} thank you",
            "500.00",
            true,
        ),
        pl(
            24,
            c,
            day(10, 27),
            "card_charge",
            None,
            "Midwest Equipment Rental",
            "-1240.00",
            false,
        ),
    ]
}

fn pool_of(account: AccountId) -> Vec<PoolLine> {
    pool()
        .into_iter()
        .filter(|p| super::account_id(p.account) == account)
        .collect()
}

fn line_of(n: u32) -> Option<PoolLine> {
    pool().into_iter().find(|p| p.n == n)
}

fn n_of(id: JournalLineId) -> Option<u32> {
    pool().into_iter().find(|p| line_id(p.n) == id).map(|p| p.n)
}

fn is_card(account: AccountId) -> bool {
    account == super::account_id(CARD)
}

fn coded(status: u16, code: &str, details: serde_json::Value) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code, "details": details}}),
    )
}

fn not_in_progress() -> ApiError {
    coded(
        409,
        "RECONCILIATION_NOT_IN_PROGRESS",
        json!({"status": "complete"}),
    )
}

fn closed_through(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

impl DemoRecon {
    /// Seeds the finished 09/30 reconciliation of 1010 (once).
    fn seed(&mut self) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        self.next = 100;
        self.recs.push(Rec {
            id: rec_id(1),
            account: super::account_id(OPERATING),
            status: ReconStatus::Complete,
            statement_date: day(9, 30),
            beginning: m("63210.55"),
            ending: m("71436.20"),
            service_charge: None,
            interest: None,
            lines: BTreeMap::new(),
            started: DemoState::now(),
            completed: Some(DemoState::now()),
        });
    }

    fn get(&self, id: ReconciliationId) -> Result<&Rec, ApiError> {
        self.recs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| coded(404, "NOT_FOUND", json!({})))
    }

    fn get_mut(&mut self, id: ReconciliationId) -> Result<&mut Rec, ApiError> {
        self.recs
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or_else(|| coded(404, "NOT_FOUND", json!({})))
    }

    fn latest_complete(&self, account: AccountId) -> Option<&Rec> {
        self.recs
            .iter()
            .filter(|r| r.account == account && r.status == ReconStatus::Complete)
            .max_by_key(|r| r.statement_date)
    }

    fn in_progress(&self, account: AccountId) -> Option<&Rec> {
        self.recs
            .iter()
            .find(|r| r.account == account && r.status == ReconStatus::InProgress)
    }

    /// The engine's answer for `r` (with or without its lines).
    fn view(&self, r: &Rec, with_lines: bool, locked: Option<CalendarDate>) -> Reconciliation {
        let mut deposits = (0u32, Money::ZERO);
        let mut payments = (0u32, Money::ZERO);
        let mut cleared = r.beginning;
        let mut lines = Vec::new();
        for (id, on) in &r.lines {
            let Some(p) = n_of(*id).and_then(line_of) else {
                continue;
            };
            if *on {
                cleared = cleared.checked_add(p.amount).unwrap_or(cleared);
                let side = if p.amount.is_positive() {
                    &mut deposits
                } else {
                    &mut payments
                };
                side.0 += 1;
                side.1 = side.1.checked_add(p.amount.abs()).unwrap_or(side.1);
            }
            lines.push(ReconLine {
                journal_line: *id,
                transaction: txn_id(p.n),
                cleared: *on,
                date: p.date,
                txn_type: p.kind.to_string(),
                number: p.number.map(str::to_string),
                payee: Some(p.payee.to_string()),
                amount: p.amount,
                after_statement_date: p.date > r.statement_date,
                from_bank_feed: p.from_feed,
                likely_duplicate: false,
            });
        }
        lines.sort_by_key(|l| (l.date, l.journal_line));
        // Pending charges count at once (U-189): a charge lowers the
        // balance, interest raises it, for every account type. Once posted
        // their lines are cleared rows of this reconciliation, so they
        // still count (after a reopen too).
        for (c, sign_in) in [(&r.interest, true), (&r.service_charge, false)] {
            if let Some(h) = c {
                let a = h.charge.amount;
                if sign_in {
                    cleared = cleared.checked_add(a).unwrap_or(cleared);
                    deposits.0 += 1;
                    deposits.1 = deposits.1.checked_add(a).unwrap_or(deposits.1);
                } else {
                    cleared = cleared.checked_sub(a).unwrap_or(cleared);
                    payments.0 += 1;
                    payments.1 = payments.1.checked_add(a).unwrap_or(payments.1);
                }
            }
        }
        let difference = r.ending.checked_sub(cleared).unwrap_or(Money::ZERO);
        let pending_dates = [&r.service_charge, &r.interest]
            .into_iter()
            .flatten()
            .filter(|h| h.posted.is_none())
            .map(|h| h.charge.date);
        let posting_locked = locked.is_some_and(|c| pending_dates.into_iter().any(|d| d <= c));
        Reconciliation {
            id: r.id,
            account: r.account,
            status: r.status,
            statement_date: r.statement_date,
            beginning_balance: r.beginning,
            statement_ending_balance: r.ending,
            cleared_balance: cleared,
            difference,
            cleared_deposits: ClearedSide {
                count: deposits.0,
                total: deposits.1,
            },
            cleared_payments: ClearedSide {
                count: payments.0,
                total: payments.1,
            },
            service_charge: r.service_charge,
            interest: r.interest,
            include_later_lines: true,
            drift: None,
            continuity: None,
            posting_locked,
            started: Some(r.started),
            lines: with_lines.then_some(lines),
        }
    }
}

fn books_ending(account: AccountId, printed: Money) -> Money {
    if is_card(account) {
        printed.checked_neg().unwrap_or(printed)
    } else {
        printed
    }
}

fn held(c: Option<StatementCharge>) -> Option<HeldCharge> {
    c.map(|charge| HeldCharge {
        charge,
        posted: None,
    })
}

fn check_charge(account: AccountId, c: Option<&StatementCharge>) -> Result<(), ApiError> {
    if let Some(c) = c
        && (c.account == account || !c.amount.is_positive())
    {
        return Err(coded(400, "VALIDATION_ERROR", json!({})));
    }
    Ok(())
}

/// An account's reconciliations, newest statement first.
///
/// # Errors
/// None in practice.
pub fn history(
    state: &mut DemoState,
    account: AccountId,
) -> Result<Vec<ReconHistoryRow>, ApiError> {
    let r = &mut state.reconcile;
    r.seed();
    let mut rows: Vec<ReconHistoryRow> = r
        .recs
        .iter()
        .filter(|x| x.account == account)
        .map(|x| ReconHistoryRow {
            id: x.id,
            status: x.status,
            statement_date: x.statement_date,
            statement_ending_balance: Some(x.ending),
            completed: x.completed,
            started: Some(x.started),
        })
        .collect();
    rows.sort_by(|a, b| b.statement_date.cmp(&a.statement_date));
    Ok(rows)
}

/// Begin (U-188/U-189): one in progress per account; bank-file lines on or
/// before the statement date pre-checked.
///
/// # Errors
/// 409 `RECONCILIATION_IN_PROGRESS`; a charge on the account itself.
pub fn start(state: &mut DemoState, s: &ReconStart) -> Result<Reconciliation, ApiError> {
    let locked = closed_through(state);
    let r = &mut state.reconcile;
    r.seed();
    if let Some(open) = r.in_progress(s.account) {
        return Err(coded(
            409,
            "RECONCILIATION_IN_PROGRESS",
            json!({"reconciliationId": open.id.to_string()}),
        ));
    }
    check_charge(s.account, s.service_charge.as_ref())?;
    check_charge(s.account, s.interest.as_ref())?;
    let beginning = r
        .latest_complete(s.account)
        .map_or(Money::ZERO, |x| x.ending);
    let lines = pool_of(s.account)
        .into_iter()
        .filter(|p| !r.reconciled.contains(&line_id(p.n)))
        .map(|p| (line_id(p.n), p.from_feed && p.date <= s.statement_date))
        .collect();
    r.next += 1;
    let rec = Rec {
        id: rec_id(r.next),
        account: s.account,
        status: ReconStatus::InProgress,
        statement_date: s.statement_date,
        beginning,
        ending: books_ending(s.account, s.statement_ending_balance),
        service_charge: held(s.service_charge),
        interest: held(s.interest),
        lines,
        started: DemoState::now(),
        completed: None,
    };
    let out = r.view(&rec, true, locked);
    r.recs.push(rec);
    Ok(out)
}

/// The worksheet with its lines.
///
/// # Errors
/// 404 for an unknown id.
pub fn get(state: &mut DemoState, id: ReconciliationId) -> Result<Reconciliation, ApiError> {
    let locked = closed_through(state);
    let r = &mut state.reconcile;
    r.seed();
    let rec = r.get(id)?;
    Ok(r.view(rec, true, locked))
}

/// Ticks in one batch; the summary view.
///
/// # Errors
/// 409 `RECONCILIATION_NOT_IN_PROGRESS`.
pub fn tick(
    state: &mut DemoState,
    id: ReconciliationId,
    ticks: &[(JournalLineId, bool)],
) -> Result<Reconciliation, ApiError> {
    let locked = closed_through(state);
    let r = &mut state.reconcile;
    let rec = r.get_mut(id)?;
    if rec.status != ReconStatus::InProgress {
        return Err(not_in_progress());
    }
    for (l, on) in ticks {
        if let Some(v) = rec.lines.get_mut(l) {
            *v = *on;
        }
    }
    let rec = r.get(id)?;
    Ok(r.view(rec, false, locked))
}

/// Change statement info.
///
/// # Errors
/// 409 `RECONCILE_CHARGE_POSTED` or `RECONCILIATION_NOT_IN_PROGRESS`.
pub fn patch(
    state: &mut DemoState,
    id: ReconciliationId,
    p: &ReconPatch,
) -> Result<Reconciliation, ApiError> {
    let locked = closed_through(state);
    let r = &mut state.reconcile;
    let rec = r.get_mut(id)?;
    if rec.status != ReconStatus::InProgress {
        return Err(not_in_progress());
    }
    for (kind, change, slot) in [
        ("service_charge", &p.service_charge, &rec.service_charge),
        ("interest", &p.interest, &rec.interest),
    ] {
        if change.is_some() && slot.is_some_and(|h| h.posted.is_some()) {
            return Err(coded(409, "RECONCILE_CHARGE_POSTED", json!({"kind": kind})));
        }
    }
    if let Some(d) = p.statement_date {
        rec.statement_date = d;
    }
    if let Some(e) = p.statement_ending_balance {
        rec.ending = books_ending(rec.account, e);
    }
    if let Some(c) = &p.service_charge {
        rec.service_charge = held(*c);
    }
    if let Some(c) = &p.interest {
        rec.interest = held(*c);
    }
    let rec = r.get(id)?;
    Ok(r.view(rec, true, locked))
}

fn by_number(state: &mut DemoState, number: &str) -> Option<AccountId> {
    super::home::accounts(state)
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(number))
        .map(|a| a.id)
}

/// The demo's default charge accounts (Bank Fees, Interest Income).
#[must_use]
pub fn charge_accounts(state: &mut DemoState) -> (Option<AccountId>, Option<AccountId>) {
    (
        by_number(state, BANK_FEES),
        by_number(state, INTEREST_INCOME),
    )
}

fn post_charge(
    state: &mut DemoState,
    account: AccountId,
    h: &HeldCharge,
    interest: bool,
) -> Option<TransactionId> {
    let chart = super::home::accounts(state);
    let (debit, credit, kind, memo) = if interest {
        (
            account,
            h.charge.account,
            "deposit",
            "Interest (from statement info)",
        )
    } else {
        (
            h.charge.account,
            account,
            "expense",
            "Service charge (from statement info)",
        )
    };
    let j = DemoJournal {
        kind,
        date: h.charge.date,
        number: String::new(),
        contact: None,
        memo: memo.into(),
        lines: vec![
            DemoLine {
                account: debit,
                debit: h.charge.amount,
                credit: Money::ZERO,
                memo: None,
            },
            DemoLine {
                account: credit,
                debit: Money::ZERO,
                credit: h.charge.amount,
                memo: None,
            },
        ],
    };
    state.ledger.save_lines(&chart, &j, None)
}

/// Finish: strict zero, the closing date for pending charges, then the
/// charges post once and the ticked lines are reconciled.
///
/// # Errors
/// 400 `RECONCILE_DIFFERENCE_NOT_ZERO`, 409 not in progress, the
/// closed-period errors.
pub fn complete(
    state: &mut DemoState,
    id: ReconciliationId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let locked = closed_through(state);
    let view = {
        let r = &state.reconcile;
        let rec = r.get(id)?;
        if rec.status != ReconStatus::InProgress {
            return Err(not_in_progress());
        }
        r.view(rec, false, locked)
    };
    if !view.difference.is_zero() {
        return Err(coded(
            400,
            "RECONCILE_DIFFERENCE_NOT_ZERO",
            json!({"difference": view.difference.to_wire()}),
        ));
    }
    if view.posting_locked {
        let dates: Vec<CalendarDate> = [view.service_charge, view.interest]
            .into_iter()
            .flatten()
            .filter(|h| h.posted.is_none())
            .map(|h| h.charge.date)
            .collect();
        check_closing(locked, &dates, password)?;
    }
    let account = view.account;
    let svc = view.service_charge.filter(|h| h.posted.is_none());
    let int = view.interest.filter(|h| h.posted.is_none());
    let svc_txn = svc.and_then(|h| post_charge(state, account, &h, false));
    let int_txn = int.and_then(|h| post_charge(state, account, &h, true));
    let r = &mut state.reconcile;
    let rec = r.get_mut(id)?;
    if let (Some(h), Some(t)) = (rec.service_charge.as_mut(), svc_txn) {
        h.posted = Some(t);
    }
    if let (Some(h), Some(t)) = (rec.interest.as_mut(), int_txn) {
        h.posted = Some(t);
    }
    rec.status = ReconStatus::Complete;
    rec.completed = Some(DemoState::now());
    let cleared: Vec<JournalLineId> = rec
        .lines
        .iter()
        .filter(|(_, on)| **on)
        .map(|(id, _)| *id)
        .collect();
    r.reconciled.extend(cleared);
    Ok(())
}

/// Undo the latest finished reconciliation (U-190).
///
/// # Errors
/// 409 `RECONCILIATION_NOT_LATEST` / `RECONCILIATION_IN_PROGRESS`; the
/// closed-period errors.
pub fn undo(
    state: &mut DemoState,
    id: ReconciliationId,
    mode: UndoMode,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let locked = closed_through(state);
    let r = &mut state.reconcile;
    r.seed();
    let rec = r.get(id)?.clone();
    if rec.status != ReconStatus::Complete
        || r.latest_complete(rec.account).map(|x| x.id) != Some(id)
    {
        return Err(coded(409, "RECONCILIATION_NOT_LATEST", json!({})));
    }
    if let Some(open) = r.in_progress(rec.account) {
        return Err(coded(
            409,
            "RECONCILIATION_IN_PROGRESS",
            json!({"reconciliationId": open.id.to_string()}),
        ));
    }
    check_closing(locked, &[rec.statement_date], password)?;
    for (l, on) in &rec.lines {
        if *on {
            r.reconciled.remove(l);
        }
    }
    r.discrepancies.push((
        rec.account,
        DiscrepancyRow {
            kind: "reconciliation_undone".into(),
            statement_date: Some(rec.statement_date),
            txn_date: None,
            txn_type: None,
            number: None,
            payee: None,
            reconciled_amount: None,
            effect: None,
            changed_at: Some(DemoState::now()),
            changed_by: Some("Kurt".into()),
            fields: Vec::new(),
            transaction: None,
        },
    ));
    match mode {
        UndoMode::KeepChecks => {
            let rec = r.get_mut(id)?;
            rec.status = ReconStatus::InProgress;
            rec.completed = None;
        }
        UndoMode::Discard => r.recs.retain(|x| x.id != id),
    }
    Ok(())
}

/// Discard one in progress.
///
/// # Errors
/// 409 not in progress.
pub fn cancel(state: &mut DemoState, id: ReconciliationId) -> Result<(), ApiError> {
    let r = &mut state.reconcile;
    if r.get(id)?.status != ReconStatus::InProgress {
        return Err(not_in_progress());
    }
    r.recs.retain(|x| x.id != id);
    Ok(())
}

/// The discrepancy report of an account.
///
/// # Errors
/// None in practice.
pub fn discrepancy(
    state: &mut DemoState,
    account: AccountId,
) -> Result<DiscrepancyReport, ApiError> {
    let r = &mut state.reconcile;
    r.seed();
    let rows = r
        .discrepancies
        .iter()
        .filter(|(a, _)| *a == account)
        .map(|(_, d)| d.clone())
        .collect();
    Ok(DiscrepancyReport {
        rows,
        drift: None::<BeginningDrift>,
    })
}

/// Demo edit (developer command): Branson Hills HOA's 10/03 deposit,
/// once reconciled, is moved to another income account: a `changed` row
/// with the field name and no effect on the reconciled amount.
pub fn recategorize_reconciled(state: &mut DemoState) -> bool {
    let r = &mut state.reconcile;
    let id = line_id(2);
    if !r.reconciled.contains(&id) {
        return false;
    }
    let Some(p) = line_of(2) else {
        return false;
    };
    r.discrepancies.push((
        super::account_id(OPERATING),
        DiscrepancyRow {
            kind: "changed".into(),
            statement_date: Some(day(10, 31)),
            txn_date: Some(p.date),
            txn_type: Some(p.kind.into()),
            number: None,
            payee: Some(p.payee.into()),
            reconciled_amount: Some(p.amount),
            effect: Some(Money::ZERO),
            changed_at: Some(DemoState::now()),
            changed_by: Some("Kurt".into()),
            fields: vec!["accountId".into()],
            transaction: Some(txn_id(p.n)),
        },
    ));
    true
}

/// The finished reconciliation with the latest statement date of
/// `account`, if any (Home's "last reconciled", Undo).
#[must_use]
pub fn last_completed(
    state: &mut DemoState,
    account: AccountId,
) -> Option<(ReconciliationId, CalendarDate)> {
    let r = &mut state.reconcile;
    r.seed();
    r.latest_complete(account).map(|x| (x.id, x.statement_date))
}

/// The detail report of a reconciliation, as the engine's JSON reads
/// (amounts as numbers there, display text here).
///
/// # Errors
/// 404 for an unknown id.
pub fn report(
    state: &mut DemoState,
    id: ReconciliationId,
) -> Result<cl_api::ReconReport, ApiError> {
    let locked = closed_through(state);
    let chart = super::home::accounts(state);
    let r = &mut state.reconcile;
    r.seed();
    let rec = r.get(id)?;
    let view = r.view(rec, true, locked);
    let account = chart.iter().find(|a| a.id == rec.account);
    let line = |l: &ReconLine| {
        let out = !l.amount.is_positive();
        json!({
            "txnDate": l.date.to_string(),
            "txnType": l.txn_type,
            "txnNumber": l.number,
            "description": l.payee,
            "payment": if out { Some(l.amount.abs().to_wire()) } else { None },
            "deposit": if out { None } else { Some(l.amount.to_wire()) },
        })
    };
    let lines = view.lines.clone().unwrap_or_default();
    let cleared: Vec<_> = lines.iter().filter(|l| l.cleared).map(line).collect();
    let uncleared: Vec<_> = lines
        .iter()
        .filter(|l| !l.cleared && !l.after_statement_date)
        .map(line)
        .collect();
    let body = json!({
        "title": "Reconciliation Detail",
        "reconciliation": {
            "id": rec.id.to_string(),
            "accountName": account.map(|a| a.fields.name.clone()).unwrap_or_default(),
            "accountNumber": account.and_then(|a| a.fields.account_number.clone()),
            "statementDate": rec.statement_date.to_string(),
            "beginningBalance": view.beginning_balance.to_wire(),
            "statementEndingBalance": view.statement_ending_balance.to_wire(),
            "clearedBalance": view.cleared_balance.to_wire(),
            "difference": view.difference.to_wire(),
            "completedBy": "Kurt",
        },
        "cleared": cleared,
        "uncleared": uncleared,
        "totals": {
            "clearedPayments": view.cleared_payments.total.to_wire(),
            "clearedDeposits": view.cleared_deposits.total.to_wire(),
        },
    });
    let bytes = serde_json::to_vec(&body).map_err(|_| ApiError::Internal)?;
    cl_api::reconciliation_report_from(&bytes)
}

/// Starts over (scenarios run light, then dark).
pub fn reset(state: &mut DemoState) {
    state.reconcile = DemoRecon::default();
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
