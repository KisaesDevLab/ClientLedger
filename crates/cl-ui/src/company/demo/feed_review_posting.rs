//! The demo's Bank Feed posting (Q-P26x: accepting a demo row creates a
//! real demo entry, visible in the register and reports; Undo removes it).
//!
//! - Categorize posts an expense (money out) or a deposit (money in) on
//!   the row's bank account, one line or the owner's split; the split must
//!   total the bank amount exactly (`SPLITS_DONT_BALANCE {difference}`).
//! - Match links a row to a bank line already in the books (nothing posts);
//!   a transfer posts one transfer; a deposit posts one deposit of waiting
//!   payments less fee lines (payments − fees = bank exactly, Q-P26t).
//! - Every accept writes an accept group; Undo deletes what was posted and
//!   unlinks what was matched, restoring the row's review state. An entry
//!   edited since is `FEED_UNDO_CHANGED` until the owner confirms (`force`);
//!   a reconciled one is refused; a dated-closed one needs the password.
//! - Voided entries (Q-P26u) stay accepted, marked; Review again puts the
//!   row back to waiting; Undo on it only unlinks.

use std::collections::BTreeMap;

use cl_api::{
    AcceptFailure, AcceptItem, AcceptOutcome, AcceptRequest, AcceptSkip, ApiError, DepositSource,
    DuplicateDecision, FeeLine, FeedChoice, FeedItemId, FeedPosted, FeedStatus, GroupedPayment,
    MatchCandidate, SplitLine, TransferCandidate, TransferTarget, UndoGroupId, UndoOutcome,
    UndoTarget, Undone, ValidationError,
};
use cl_core::ids::{AccountId, ContactId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use serde_json::json;

use super::{DemoState, Prior, Row, conflict, not_pending, settle};
use crate::company::demo::register::{DemoLedger, DemoPosting, check_closing};
use crate::company::demo::{deposits, journal};

/// How far a demo match candidate may be from the bank date (days).
const MATCH_WINDOW_DAYS: i64 = 15;
/// How far a waiting payment may be from a bank deposit (days).
const DEPOSIT_WINDOW_DAYS: i64 = 10;

fn validation(code: &str, details: serde_json::Value) -> ApiError {
    ApiError::Validation(ValidationError::coded(code, details))
}

fn invalid() -> ApiError {
    ApiError::Local(cl_api::LocalValidation::InvalidValue)
}

fn row(state: &DemoState, id: FeedItemId) -> Result<Row, ApiError> {
    state
        .feed_review
        .rows
        .iter()
        .find(|r| r.id == id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })
}

fn number_of(chart: &[cl_api::Account], id: AccountId) -> Option<String> {
    chart
        .iter()
        .find(|a| a.id == id)
        .and_then(|a| a.fields.account_number.clone())
}

/// A demo journal line's id: the entry's number and the line's place.
fn line_id(txn: u32, index: usize) -> JournalLineId {
    format!("d26e0000-0000-4000-8000-{txn:08}{index:04}")
        .parse()
        .unwrap_or_else(|_| unreachable!("a well-formed demo line id"))
}

/// The posted entry as it stands (Undo compares it with what was posted).
fn fingerprint(state: &DemoState, t: TransactionId) -> Option<String> {
    let tx = state
        .ledger
        .transactions()
        .into_iter()
        .find(|x| DemoLedger::id_of(x) == t)?;
    let mut s = format!(
        "{:?}|{}|{}|{}",
        tx.date,
        tx.kind,
        tx.contact.as_deref().unwrap_or(""),
        tx.memo
    );
    for l in &tx.lines {
        s.push('|');
        s.push_str(&l.account);
        s.push(':');
        s.push_str(&l.debit.to_wire());
        s.push(':');
        s.push_str(&l.credit.to_wire());
    }
    Some(s)
}

/// Marks a row accepted under `group`.
fn accept_row(
    state: &mut DemoState,
    id: FeedItemId,
    status: FeedStatus,
    entry: Option<TransactionId>,
    posted: bool,
    how: &'static str,
    group: UndoGroupId,
) {
    let fp = entry.filter(|_| posted).and_then(|t| fingerprint(state, t));
    if let Some(r) = state.feed_review.rows.iter_mut().find(|r| r.id == id) {
        r.prior = Some(Prior {
            status: r.status,
            assigned: r.assigned.clone(),
            suggested: r.suggested.clone(),
            payee: r.payee.clone(),
            source: r.source,
            confidence: r.confidence,
            rule: r.rule.clone(),
        });
        r.status = status;
        r.matched = entry;
        r.posted = posted;
        r.how = how;
        r.group = Some(group);
        r.fingerprint = fp;
        r.entry_voided = false;
    }
    state.feed_review.groups.entry(group).or_default().push(id);
    settle(state, id);
}

fn waiting_row(state: &mut DemoState, id: FeedItemId) -> Result<Row, ApiError> {
    state.feed_review.write_check(id)?;
    let r = row(state, id)?;
    if !r.waiting() {
        return Err(not_pending(r.status));
    }
    Ok(r)
}

/// Posts `r` with its account (or the owner's split) as a demo entry.
#[allow(clippy::single_match_else, reason = "a split or one line")]
fn post(
    state: &mut DemoState,
    r: &Row,
    choice: Option<&FeedChoice>,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    let chart = super::super::chart();
    let total = r.amount.abs();
    let lines: Vec<(AccountId, Money)> = match choice.filter(|c| !c.splits.is_empty()) {
        Some(c) => {
            if !cl_model::bank_feed::whole_cents(r.amount) {
                return Err(validation("AMOUNT_NOT_CENTS", json!({})));
            }
            let typed = Money::sum(c.splits.iter().map(|l| l.amount)).map_err(|_| invalid())?;
            let diff = total.checked_sub(typed).map_err(|_| invalid())?;
            if !diff.is_zero() {
                return Err(validation(
                    "SPLITS_DONT_BALANCE",
                    json!({ "difference": diff.to_wire() }),
                ));
            }
            c.splits.iter().map(|l| (l.account, l.amount)).collect()
        }
        None => {
            let account = choice
                .and_then(|c| c.account)
                .or_else(|| r.assigned.as_ref().map(|a| a.id))
                .or_else(|| r.suggested.as_ref().map(|a| a.id))
                .ok_or_else(|| validation("INCOMPLETE", json!({})))?;
            vec![(account, total)]
        }
    };
    if !lines.iter().all(|(a, _)| chart.iter().any(|x| x.id == *a)) {
        return Err(ApiError::NotFound { code: None });
    }
    check_closing(journal::closing(state), &[r.date], password)?;
    let money_out = r.amount.is_positive();
    // The owner's payee and memo win over the row's (as the engine).
    let contact = match choice.and_then(|c| c.contact) {
        Some(id) => Some(contact_name(state, id).ok_or(ApiError::NotFound { code: None })?),
        None => r.payee.clone(),
    };
    let memo = choice
        .and_then(|c| c.memo.as_deref())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map_or_else(|| r.description.clone(), str::to_string);
    let p = DemoPosting {
        kind: if money_out { "expense" } else { "deposit" },
        date: r.date,
        number: String::new(),
        contact,
        memo,
        own: r.account,
        own_debit: !money_out,
        lines,
    };
    let t = state.ledger.save(&chart, &p, None).ok_or_else(invalid)?;
    if let [(account, _)] = p.lines.as_slice() {
        let key = (pattern_of(&r.description), *account);
        state.feed_review.learned.entry(key.clone()).or_default().0 += 1;
        state.feed_review.taught.insert(r.id, key);
    }
    Ok(t)
}

/// A demo contact's display name (owner data, never logged).
fn contact_name(state: &mut DemoState, id: ContactId) -> Option<String> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
}

/// The description words before the first one with a digit (the demo's
/// payee pattern; owner data, never logged).
fn pattern_of(description: &str) -> String {
    description
        .split_whitespace()
        .take_while(|w| !w.chars().any(|c| c.is_ascii_digit()))
        .take(3)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `PUT /banking/feed/:id/categorize` (and approve with `None`): posts the
/// owner's choice, or what is staged or suggested, as a demo entry.
///
/// # Errors
/// 403 for a view-only user; 409 `FEED_ITEM_NOT_PENDING`; `NotFound`;
/// `SPLITS_DONT_BALANCE`, `AMOUNT_NOT_CENTS`; the closed period.
pub fn categorize(
    state: &mut DemoState,
    id: FeedItemId,
    choice: Option<&FeedChoice>,
    password: Option<&Secret>,
) -> Result<Option<TransactionId>, ApiError> {
    let r = waiting_row(state, id)?;
    let t = post(state, &r, choice, password)?;
    accept_row(
        state,
        id,
        FeedStatus::Categorized,
        Some(t),
        true,
        "categorize",
        UndoGroupId::new_random(),
    );
    Ok(Some(t))
}

/// The bank lines already in the books a row may be (`match-candidates`):
/// same account, same amount and direction, within 15 days, not matched
/// by another row. Only the sample's match rows and imported rows look.
#[must_use]
pub fn match_candidates(state: &DemoState, id: FeedItemId) -> Vec<MatchCandidate> {
    let Ok(r) = row(state, id) else {
        return Vec::new();
    };
    if !r.waiting() || !(r.kind == "match" || r.import_key.is_some()) {
        return Vec::new();
    }
    let chart = super::super::chart();
    let Some(bank) = number_of(&chart, r.account) else {
        return Vec::new();
    };
    let taken: Vec<TransactionId> = state
        .feed_review
        .rows
        .iter()
        .filter(|x| x.status.is_accepted())
        .filter_map(|x| x.matched)
        .collect();
    let want = r.amount.abs();
    let mut out = Vec::new();
    for t in state.ledger.transactions() {
        let tid = DemoLedger::id_of(&t);
        if state.ledger.is_void(tid) || taken.contains(&tid) {
            continue;
        }
        if r.date.days_until(t.date).abs() > MATCH_WINDOW_DAYS {
            continue;
        }
        for (i, l) in t.lines.iter().enumerate() {
            let side = if r.amount.is_positive() {
                l.credit
            } else {
                l.debit
            };
            if l.account == bank && side == want {
                out.push(MatchCandidate {
                    transaction: tid,
                    line: line_id(t.id, i),
                    txn_type: Some(t.kind.clone()),
                    txn_number: (!t.number.is_empty()).then(|| t.number.clone()),
                    date: Some(t.date),
                    amount: r.amount,
                    memo: (!t.memo.is_empty()).then(|| t.memo.clone()),
                    contact_name: t.contact.clone(),
                    check_number: None,
                });
            }
        }
    }
    out
}

/// Waiting payments a money-in row may deposit (±10 days).
#[must_use]
pub fn deposit_payments(state: &mut DemoState, id: FeedItemId) -> Vec<deposits::DemoPending> {
    let Ok(r) = row(state, id) else {
        return Vec::new();
    };
    if r.amount.is_positive() {
        return Vec::new();
    }
    deposits::pending(state)
        .into_iter()
        .filter(|p| r.date.days_until(p.date).abs() <= DEPOSIT_WINDOW_DAYS)
        .collect()
}

/// The exact groupings of waiting payments for a money-in row.
#[must_use]
pub fn deposit_groupings(state: &mut DemoState, id: FeedItemId) -> Vec<Vec<deposits::DemoPending>> {
    let Ok(r) = row(state, id) else {
        return Vec::new();
    };
    if r.amount.is_positive() || !r.waiting() {
        return Vec::new();
    }
    let item = cl_company::sample::FeedItem {
        date: r.date,
        description: r.description.clone(),
        amount: r.amount.abs(),
        status: String::new(),
    };
    deposits::deposit_groupings(state, &item, 5).candidates
}

/// Match signals per waiting row: line candidates, or exact deposit
/// groupings for money in.
pub(super) fn match_signals(state: &mut DemoState) -> BTreeMap<FeedItemId, u32> {
    let ids: Vec<(FeedItemId, bool)> = state
        .feed_review
        .rows
        .iter()
        .filter(|r| r.waiting())
        .map(|r| (r.id, r.amount.is_positive()))
        .collect();
    let mut out = BTreeMap::new();
    for (id, money_out) in ids {
        let mut n = u32::try_from(match_candidates(state, id).len()).unwrap_or(u32::MAX);
        if n == 0 && !money_out {
            n = u32::try_from(deposit_groupings(state, id).len()).unwrap_or(u32::MAX);
        }
        if n > 0 {
            out.insert(id, n);
        }
    }
    out
}

fn match_with(
    state: &mut DemoState,
    id: FeedItemId,
    transaction: TransactionId,
    group: UndoGroupId,
) -> Result<(), ApiError> {
    waiting_row(state, id)?;
    let fits = match_candidates(state, id)
        .iter()
        .any(|c| c.transaction == transaction);
    if !fits {
        return Err(validation(
            "MATCH_MISMATCH",
            json!({ "field": "transactionId" }),
        ));
    }
    accept_row(
        state,
        id,
        FeedStatus::Matched,
        Some(transaction),
        false,
        "match",
        group,
    );
    Ok(())
}

/// `PUT /banking/feed/:id/match`.
///
/// # Errors
/// As [`categorize`]; 400 `MATCH_MISMATCH` for an entry that doesn't fit.
pub fn match_line(
    state: &mut DemoState,
    id: FeedItemId,
    transaction: TransactionId,
) -> Result<Option<UndoGroupId>, ApiError> {
    let g = UndoGroupId::new_random();
    match_with(state, id, transaction, g)?;
    Ok(Some(g))
}

/// Matches the row to its one candidate.
///
/// # Errors
/// As [`match_line`]; `LINE_REQUIRED`-like refusal with none or several.
pub fn match_one(state: &mut DemoState, id: FeedItemId) -> Result<(), ApiError> {
    match match_candidates(state, id).as_slice() {
        [one] => match_line(state, id, one.transaction).map(|_| ()),
        _ => Err(validation(
            "MATCH_MISMATCH",
            json!({ "field": "transactionId" }),
        )),
    }
}

/// `GET /banking/feed/:id/transfer-candidates`: waiting rows on another
/// account with the opposite amount within `window` days.
#[must_use]
pub fn transfer_candidates(
    state: &DemoState,
    id: FeedItemId,
    window: u8,
) -> Vec<TransferCandidate> {
    let Ok(r) = row(state, id) else {
        return Vec::new();
    };
    let chart = super::super::chart();
    state
        .feed_review
        .rows
        .iter()
        .filter(|o| {
            o.waiting()
                && o.account != r.account
                && o.amount.checked_neg().ok() == Some(r.amount)
                && r.date.days_until(o.date).abs() <= i64::from(window)
        })
        .map(|o| TransferCandidate {
            feed_item: o.id,
            account: o.account,
            account_name: chart
                .iter()
                .find(|a| a.id == o.account)
                .map(|a| a.fields.name.clone())
                .unwrap_or_default(),
            date: o.date,
            amount: o.amount,
        })
        .collect()
}

/// 409 `FEED_ITEM_POSSIBLE_DUPLICATE`: transfer and deposit refuse a side
/// flagged as a possible duplicate until the owner decides (engine #238).
fn no_duplicate(r: &Row) -> Result<(), ApiError> {
    if r.possible_duplicate {
        return Err(conflict(
            "FEED_ITEM_POSSIBLE_DUPLICATE",
            json!({
                "id": r.id.to_string(),
                "possibleDuplicateOf": r.duplicate_of.map(|d| d.to_string()),
            }),
        ));
    }
    Ok(())
}

/// `POST /banking/feed/:id/transfer`: one transfer between the row's bank
/// account and the other account; both rows (when the other side is a
/// bank line) are matched to it. Dated on the money-out side.
///
/// # Errors
/// As [`categorize`]; `TRANSFER_MISMATCH` for the same account or an
/// other side that doesn't fit.
pub fn transfer(
    state: &mut DemoState,
    id: FeedItemId,
    target: TransferTarget,
    password: Option<&Secret>,
) -> Result<FeedPosted, ApiError> {
    let r = waiting_row(state, id)?;
    no_duplicate(&r)?;
    let (other_account, other) = match target {
        TransferTarget::Account(a) => (a, None),
        TransferTarget::OtherItem(o) => {
            let ok = transfer_candidates(state, id, 10)
                .iter()
                .any(|c| c.feed_item == o);
            if !ok {
                return Err(validation(
                    "TRANSFER_MISMATCH",
                    json!({ "field": "otherFeedItemId" }),
                ));
            }
            let o = row(state, o)?;
            no_duplicate(&o)?;
            (o.account, Some(o))
        }
    };
    if other_account == r.account {
        return Err(validation(
            "TRANSFER_MISMATCH",
            json!({ "field": "toAccountId" }),
        ));
    }
    let money_out = r.amount.is_positive();
    let date = match (&other, money_out) {
        (Some(o), false) => o.date,
        _ => r.date,
    };
    check_closing(journal::closing(state), &[date], password)?;
    let chart = super::super::chart();
    let p = DemoPosting {
        kind: "transfer",
        date,
        number: String::new(),
        contact: None,
        memo: r.description.clone(),
        own: r.account,
        own_debit: !money_out,
        lines: vec![(other_account, r.amount.abs())],
    };
    let t = state.ledger.save(&chart, &p, None).ok_or_else(invalid)?;
    let g = UndoGroupId::new_random();
    accept_row(state, id, FeedStatus::Matched, Some(t), true, "transfer", g);
    let mut items = vec![(id, line_id(0, 0))];
    if let Some(o) = other {
        accept_row(
            state,
            o.id,
            FeedStatus::Matched,
            Some(t),
            true,
            "transfer",
            g,
        );
        items.push((o.id, line_id(0, 1)));
    }
    Ok(FeedPosted {
        transaction: Some(t),
        items,
        payments: Vec::new(),
        undo_group: Some(g),
    })
}

/// A fee line may not use a bank or card account, Payments Clearing, A/R
/// or A/P (Q-P26w).
fn fee_account_ok(chart: &[cl_api::Account], account: AccountId) -> bool {
    let Some(a) = chart.iter().find(|a| a.id == account) else {
        return false;
    };
    let (ar, ap) = journal::ar_ap(chart);
    let banky = a
        .fields
        .detail_type
        .as_deref()
        .is_some_and(|d| d == "credit_card" || cl_model::register::is_bank_detail(d));
    let system = matches!(
        a.system_tag.as_deref(),
        Some("payments_clearing" | "accounts_receivable" | "accounts_payable")
    );
    !(banky || system || ar.contains(&account) || ap.contains(&account))
}

/// `POST /banking/feed/:id/deposit`: one deposit of waiting payments less
/// fee lines for a money-in row; payments − fees must equal the bank
/// amount exactly (the payments' live amounts, never typed ones).
///
/// # Errors
/// As [`categorize`]; `FEED_ITEM_MISMATCH {difference | reason}`,
/// `FEE_ACCOUNT_INVALID {accountId}`, `PAYMENT_NOT_PENDING_DEPOSIT`,
/// `PAYMENT_BOUNCED`; more than 10 fee lines.
pub fn deposit(
    state: &mut DemoState,
    id: FeedItemId,
    payments: &[TransactionId],
    fees: &[FeeLine],
    password: Option<&Secret>,
) -> Result<FeedPosted, ApiError> {
    let r = waiting_row(state, id)?;
    no_duplicate(&r)?;
    if r.amount.is_positive() {
        return Err(validation(
            "FEED_ITEM_MISMATCH",
            json!({ "reason": "not_money_in" }),
        ));
    }
    if payments.is_empty() || fees.len() > cl_api::FEE_LINES_MAX {
        return Err(invalid());
    }
    let chart = super::super::chart();
    if let Some(bad) = fees.iter().find(|f| !fee_account_ok(&chart, f.account)) {
        return Err(validation(
            "FEE_ACCOUNT_INVALID",
            json!({ "accountId": bad.account.to_string() }),
        ));
    }
    let waiting = deposits::pending(state);
    let mut picked = Vec::with_capacity(payments.len());
    for p in payments {
        let w = waiting
            .iter()
            .find(|w| w.source == *p)
            .ok_or_else(|| validation("PAYMENT_NOT_PENDING_DEPOSIT", json!({})))?;
        picked.push(w.clone());
    }
    let paid = Money::sum(picked.iter().map(|p| p.amount)).map_err(|_| invalid())?;
    let fee_total = Money::sum(fees.iter().map(|f| f.amount)).map_err(|_| invalid())?;
    let net = paid.checked_sub(fee_total).map_err(|_| invalid())?;
    let diff = r.amount.abs().checked_sub(net).map_err(|_| invalid())?;
    if !diff.is_zero() {
        return Err(validation(
            "FEED_ITEM_MISMATCH",
            json!({ "difference": diff.to_wire() }),
        ));
    }
    let clearing = deposits::clearing_account(state).ok_or_else(invalid)?;
    let mut d =
        cl_api::DepositDraft::plain(r.account, r.date, Some(r.description.clone()), Vec::new());
    d.payments = picked
        .iter()
        .map(|p| GroupedPayment {
            source: p.source,
            kind: DepositSource::Payment,
            account: clearing,
            amount: p.amount,
            contact: p.contact,
            memo: None,
        })
        .collect();
    d.deductions = fees
        .iter()
        .map(|f| SplitLine {
            account: f.account,
            amount: f.amount,
            memo: f.memo.clone(),
            contact: None,
            billable: false,
            tag: None,
            item: None,
        })
        .collect();
    let t = deposits::create(state, &d, password)?;
    let g = UndoGroupId::new_random();
    accept_row(state, id, FeedStatus::Matched, Some(t), true, "deposit", g);
    Ok(FeedPosted {
        transaction: Some(t),
        items: vec![(id, line_id(0, 0))],
        payments: picked.iter().map(|p| (p.source, p.amount)).collect(),
        undo_group: Some(g),
    })
}

fn failure(id: FeedItemId, e: &ApiError) -> AcceptFailure {
    let (code, details) = match e {
        ApiError::ClosedPeriod(_) | ApiError::ClosingPasswordRejected => (
            AcceptFailure::PERIOD_LOCKED.to_string(),
            cl_api::ConflictDetails::default(),
        ),
        ApiError::Validation(v) => (
            v.code()
                .map_or("VALIDATION_ERROR", |c| c.as_str())
                .to_string(),
            v.details().clone(),
        ),
        ApiError::Conflict { code, details, .. } => (
            code.as_ref().map_or("CONFLICT", |c| c.as_str()).to_string(),
            details.clone(),
        ),
        _ => ("FAILED".to_string(), cl_api::ConflictDetails::default()),
    };
    AcceptFailure { id, code, details }
}

/// Has a match signal (a candidate line, a deposit grouping, a transfer).
fn has_match(state: &mut DemoState, r: &Row) -> bool {
    r.transfer
        || !match_candidates(state, r.id).is_empty()
        || (!r.amount.is_positive() && !deposit_groupings(state, r.id).is_empty())
}

/// `POST /banking/feed/accept`: a selection or every waiting row one
/// auto-accept rule staged (500 a call), in one group.
///
/// # Errors
/// 403 for a view-only user; rule mode: 404 `RULE_NOT_FOUND`, 409
/// `RULE_NOT_AUTO_ACCEPT`, 409 `RULE_CHANGED {updatedAt}`.
pub fn accept(
    state: &mut DemoState,
    req: &AcceptRequest,
    password: Option<&Secret>,
) -> Result<AcceptOutcome, ApiError> {
    if state.feed_review.view_only {
        return Err(ApiError::Forbidden { code: None });
    }
    super::refresh(state);
    let group = UndoGroupId::new_random();
    let mut out = AcceptOutcome {
        undo_group: group,
        posted: 0,
        matched: 0,
        skipped: Vec::new(),
        failures: Vec::new(),
        remaining: 0,
        next_after: None,
    };
    // Rule mode: the rule's account as it stands now (a row whose staging
    // no longer matches it is skipped `staging_differs`, as the engine).
    let mut rule_account: Option<Option<AccountId>> = None;
    let items: Vec<AcceptItem> = match req {
        AcceptRequest::Items(items) => {
            let mut v = items.clone();
            v.sort_by_key(AcceptItem::id);
            v
        }
        AcceptRequest::Rule(rule) => {
            rule_account = Some(
                rules(state)
                    .iter()
                    .find(|x| x.id.as_deref() == Some(rule.rule_id.as_str()))
                    .and_then(|x| x.account),
            );
            let staged: Vec<&Row> = state
                .feed_review
                .rows
                .iter()
                .filter(|r| r.rule.as_ref().is_some_and(|x| x.id == rule.rule_id))
                .collect();
            let Some(known) = staged.first().and_then(|r| r.rule.clone()) else {
                return Err(ApiError::NotFound {
                    code: cl_api::ErrorCode::parse("RULE_NOT_FOUND"),
                });
            };
            if !known.auto_accept {
                return Err(conflict("RULE_NOT_AUTO_ACCEPT", json!({})));
            }
            if known.updated_at.as_deref() != Some(rule.updated_at.as_str()) {
                return Err(conflict(
                    "RULE_CHANGED",
                    json!({ "updatedAt": known.updated_at }),
                ));
            }
            let mut ids: Vec<FeedItemId> = staged
                .iter()
                .filter(|r| {
                    r.status == FeedStatus::Pending
                        && rule
                            .connection
                            .is_none_or(|c| c == super::connection_of(r.account))
                        && rule.after.is_none_or(|a| r.id > a)
                })
                .map(|r| r.id)
                .collect();
            ids.sort();
            let page: Vec<FeedItemId> = ids.iter().copied().take(cl_api::ACCEPT_MAX).collect();
            out.remaining = (ids.len() - page.len()) as u64;
            out.next_after = (out.remaining > 0).then(|| page.last().copied()).flatten();
            page.into_iter().map(AcceptItem::AsProposed).collect()
        }
    };
    // `conflict_next`: another computer takes the first row now.
    if std::mem::take(&mut state.feed_review.conflict_next)
        && let Some(first) = items.first()
        && let Ok(r) = state.feed_review.row_mut(first.id())
        && r.waiting()
    {
        r.status = FeedStatus::Categorized;
    }
    for item in items {
        let id = item.id();
        let Ok(r) = row(state, id) else {
            out.skipped.push((id, AcceptSkip::NotFound));
            continue;
        };
        if !r.waiting() {
            out.skipped.push((id, AcceptSkip::NotPending));
            continue;
        }
        if r.possible_duplicate {
            out.skipped.push((id, AcceptSkip::PossibleDuplicate));
            continue;
        }
        // U-181: the receipt is checked before the row posts; a refused
        // receipt fails the row (nothing posted). The closing date comes
        // first, as on the engine (a locked row asks for the password
        // whatever its receipt).
        let draft = item.draft().cloned();
        if draft.is_some()
            && !matches!(item.inner(), AcceptItem::Match { .. })
            && let Err(e) = check_closing(journal::closing(state), &[r.date], password)
        {
            out.failures.push(failure(id, &e));
            continue;
        }
        if let Some(link) = &draft
            && let Err(e) = super::super::drafts::feed_draft_ready(state, state.open, link)
        {
            out.failures.push(AcceptFailure {
                id,
                code: e.code.to_string(),
                details: cl_api::ConflictDetails::new(e.details),
            });
            continue;
        }
        // The row and its receipt are one step: a filing that fails after
        // the posting takes the posting back (the engine's transaction).
        let savepoint = draft.as_ref().map(|_| state.clone());
        let result = match item.inner() {
            AcceptItem::Receipt { .. } => Err(invalid()),
            AcceptItem::Match { transaction, .. } => {
                match_with(state, id, *transaction, group).map(|()| false)
            }
            AcceptItem::AsProposed(_) if has_match(state, &r) => {
                out.skipped.push((id, AcceptSkip::HasMatch));
                continue;
            }
            AcceptItem::AsProposed(_)
                if rule_account.is_some_and(|a| {
                    a.is_some_and(|a| r.suggested.as_ref().map(|s| s.id) != Some(a))
                }) =>
            {
                out.skipped.push((id, AcceptSkip::StagingDiffers));
                continue;
            }
            AcceptItem::AsProposed(_) if r.assigned.is_none() && r.suggested.is_none() => {
                out.skipped.push((id, AcceptSkip::Incomplete));
                continue;
            }
            AcceptItem::AsProposed(_) => post(state, &r, None, password).map(|t| {
                accept_row(
                    state,
                    id,
                    FeedStatus::Categorized,
                    Some(t),
                    true,
                    "categorize",
                    group,
                );
                true
            }),
            AcceptItem::Values(_, c) => post(state, &r, Some(c), password).map(|t| {
                accept_row(
                    state,
                    id,
                    FeedStatus::Categorized,
                    Some(t),
                    true,
                    "categorize",
                    group,
                );
                true
            }),
        };
        if let (Ok(_), Some(link)) = (&result, &draft) {
            let entry = row(state, id).ok().and_then(|r| r.matched);
            // PR-A1 B1: a posted row whose receipt looks like an entry
            // already in the books fails (the posting taken back) unless
            // the owner acknowledged every match. A match files the
            // receipt on the entry it is, so it isn't checked.
            let vendor = match item.inner() {
                AcceptItem::Values(_, c) => c.contact,
                _ => None,
            };
            let dups = match (state.open, item.inner()) {
                // Money in posts no bill or expense: not checked (as the engine).
                (Some(tenant), AcceptItem::AsProposed(_) | AcceptItem::Values(..))
                    if r.amount.is_positive() =>
                {
                    super::super::drafts::feed_duplicates(
                        state,
                        tenant,
                        link,
                        vendor,
                        r.amount.abs(),
                        r.date,
                        entry,
                    )
                }
                _ => Vec::new(),
            };
            if !dups.is_empty() {
                if let Some(before) = savepoint {
                    *state = before;
                }
                out.failures.push(AcceptFailure {
                    id,
                    code: AcceptFailure::DRAFT_POSSIBLE_DUPLICATE.to_string(),
                    details: cl_api::ConflictDetails::new(json!({ "matches": dups })),
                });
                continue;
            }
            let filed = match (state.open, entry) {
                (Some(tenant), Some(t)) => {
                    super::super::drafts::feed_file_on(state, tenant, link.id, t)
                }
                _ => Err(ApiError::NotFound { code: None }),
            };
            if filed.is_err() {
                if let Some(before) = savepoint {
                    *state = before;
                }
                out.failures.push(AcceptFailure {
                    id,
                    code: AcceptFailure::DRAFT_NOT_FOUND.to_string(),
                    details: cl_api::ConflictDetails::default(),
                });
                continue;
            }
        }
        match result {
            Ok(true) => out.posted += 1,
            Ok(false) => out.matched += 1,
            Err(e) => out.failures.push(failure(id, &e)),
        }
    }
    if out.posted + out.matched == 0 {
        state.feed_review.groups.remove(&group);
    }
    Ok(out)
}

/// `POST /banking/feed/undo`: takes rows or a whole group back.
///
/// # Errors
/// 403 for a view-only user; 404 for an unknown group; 409
/// `FEED_UNDO_CHANGED {ids}` when an entry to delete was edited since it
/// was posted and `force` isn't set (nothing undone).
pub fn undo(
    state: &mut DemoState,
    target: &UndoTarget,
    force: bool,
    password: Option<&Secret>,
) -> Result<UndoOutcome, ApiError> {
    if state.feed_review.view_only {
        return Err(ApiError::Forbidden { code: None });
    }
    super::refresh(state);
    let ids: Vec<FeedItemId> = match target {
        UndoTarget::Items(ids) => ids.clone(),
        UndoTarget::Group(g) => state
            .feed_review
            .groups
            .get(g)
            .cloned()
            .ok_or(ApiError::NotFound { code: None })?,
    };
    let mut out = UndoOutcome::default();
    let mut changed = Vec::new();
    let mut todo = Vec::new();
    for id in ids {
        let r = match row(state, id) {
            Ok(r) if r.status.is_accepted() && r.group.is_some() => r,
            _ => {
                out.failures.push(AcceptFailure {
                    id,
                    code: AcceptFailure::NOT_ACCEPTED.into(),
                    details: cl_api::ConflictDetails::default(),
                });
                continue;
            }
        };
        todo.push(r);
    }
    // The entry goes when a row that posted it (categorize, transfer,
    // deposit) is in the request, even with later-matched rows selected
    // too; a match alone only unlinks (engine #238).
    let posting: Vec<TransactionId> = todo
        .iter()
        .filter(|r| r.posted && !r.entry_voided)
        .filter_map(|r| r.matched)
        .collect();
    let todo: Vec<(Row, bool)> = todo
        .into_iter()
        .map(|r| {
            let deletes = !r.entry_voided && r.matched.is_some_and(|t| posting.contains(&t));
            (r, deletes)
        })
        .collect();
    for (r, deletes) in &todo {
        if *deletes
            && r.fingerprint.is_some()
            && r.matched.and_then(|t| fingerprint(state, t)).as_ref() != r.fingerprint.as_ref()
        {
            changed.push(r.id);
        }
    }
    if !changed.is_empty() && !force {
        return Err(conflict(
            "FEED_UNDO_CHANGED",
            json!({ "ids": changed.iter().map(ToString::to_string).collect::<Vec<_>>() }),
        ));
    }
    let closing = journal::closing(state);
    for (r, deletes) in todo {
        let Some(t) = r.matched else { continue };
        // Already taken back with the other side of its entry.
        if out.undone.iter().any(|u| u.id == r.id) {
            continue;
        }
        let entry = state
            .ledger
            .transactions()
            .into_iter()
            .find(|x| DemoLedger::id_of(x) == t);
        let fail = |code: &str| AcceptFailure {
            id: r.id,
            code: code.into(),
            details: cl_api::ConflictDetails::default(),
        };
        if entry.as_ref().is_some_and(|e| {
            e.cleared
                && state
                    .ledger
                    .reconciled_through()
                    .is_some_and(|r| e.date <= r)
        }) {
            out.failures.push(fail(AcceptFailure::UNDO_RECONCILED));
            continue;
        }
        if deletes {
            let date = entry.as_ref().map_or(r.date, |e| e.date);
            if check_closing(closing, &[date], password).is_err() {
                out.failures.push(fail(AcceptFailure::PERIOD_LOCKED));
                continue;
            }
            // A deposit holding a bounced payment stays (the engine's
            // `assertDepositSourcesNotBounced`).
            if entry.as_ref().is_some_and(|e| e.kind == "deposit")
                && state
                    .ledger
                    .deposit_links(t)
                    .iter()
                    .any(|(src, _)| super::super::receivables::is_bounced_source(state, *src))
            {
                out.failures.push(fail(AcceptFailure::PAYMENT_BOUNCED));
                continue;
            }
            // U-180: its receipts go back to the review queue with their
            // files.
            if state.ledger.date_of(t).is_some() {
                match super::super::drafts::delete_entry(state, t) {
                    Ok(back) => out.reopened_drafts.extend(back),
                    Err(e) => {
                        out.failures.push(undo_failure(r.id, &e));
                        continue;
                    }
                }
            } else {
                out.reopened_drafts
                    .extend(super::super::drafts::reopen_for(state, t));
            }
        }
        // A deleted entry takes every row on it back (the other side of a
        // transfer too); a kept one only the rows asked for.
        let on_entry: Vec<Row> = if deletes {
            state
                .feed_review
                .rows
                .iter()
                .filter(|x| x.matched == Some(t) && x.status.is_accepted())
                .cloned()
                .collect()
        } else {
            vec![r.clone()]
        };
        for x in on_entry {
            let group = x.group.or(r.group).unwrap_or_else(UndoGroupId::new_random);
            if let Some(key) = state.feed_review.taught.remove(&x.id) {
                state.feed_review.learned.entry(key).or_default().1 += 1;
            }
            restore(state, x.id);
            out.undone.push(Undone {
                id: x.id,
                kind: x.how.to_string(),
                transaction: t,
                deleted: deletes,
                group,
            });
        }
    }
    Ok(out)
}

/// An Undo delete the demo ledger refused, in the engine's words.
fn undo_failure(id: FeedItemId, e: &ApiError) -> AcceptFailure {
    match e {
        ApiError::Conflict {
            reason: cl_api::ConflictReason::Reconciled,
            ..
        } => AcceptFailure {
            id,
            code: AcceptFailure::UNDO_RECONCILED.into(),
            details: cl_api::ConflictDetails::default(),
        },
        other => failure(id, other),
    }
}

/// Puts a row back to its state before the accept.
fn restore(state: &mut DemoState, id: FeedItemId) {
    let review = &mut state.feed_review;
    let mut group = None;
    if let Some(r) = review.rows.iter_mut().find(|r| r.id == id) {
        if let Some(p) = r.prior.take() {
            r.status = p.status;
            r.assigned = p.assigned;
            r.suggested = p.suggested;
            r.payee = p.payee;
            r.source = p.source;
            r.confidence = p.confidence;
            r.rule = p.rule;
        } else {
            r.status = FeedStatus::Pending;
            r.assigned = None;
        }
        r.matched = None;
        r.posted = false;
        r.how = "";
        r.fingerprint = None;
        r.entry_voided = false;
        r.entry_changed = false;
        group = r.group.take();
    }
    if let Some(g) = group
        && let Some(v) = review.groups.get_mut(&g)
    {
        v.retain(|x| *x != id);
        if v.is_empty() {
            review.groups.remove(&g);
        }
    }
}

/// `POST /banking/feed/:id/duplicate`: Keep both clears the flag; Remove
/// this one excludes it with reason `duplicate` (Q-P26g).
///
/// # Errors
/// 403; 409 `NOT_A_POSSIBLE_DUPLICATE`; `NotFound`.
pub fn decide_duplicate(
    state: &mut DemoState,
    id: FeedItemId,
    decision: DuplicateDecision,
) -> Result<(), ApiError> {
    state.feed_review.write_check(id)?;
    let r = state.feed_review.row_mut(id)?;
    if !r.possible_duplicate || !r.waiting() {
        return Err(conflict("NOT_A_POSSIBLE_DUPLICATE", json!({})));
    }
    r.possible_duplicate = false;
    if decision == DuplicateDecision::Remove {
        r.status = FeedStatus::Excluded;
        r.exclusion_reason = Some("duplicate".into());
    }
    Ok(())
}

/// Q-PR5a (engine U-199): the accepted row's entry still has its bank
/// line: on the row's account, on its side, for its amount. An entry the
/// demo no longer has agrees (a delete takes the row back elsewhere).
pub(super) fn entry_agrees(
    txns: &[cl_company::sample::SampleTransaction],
    chart: &[cl_api::Account],
    account: AccountId,
    amount: Money,
    t: TransactionId,
) -> bool {
    let Some(bank) = number_of(chart, account) else {
        return true;
    };
    let Some(tx) = txns.iter().find(|x| DemoLedger::id_of(x) == t) else {
        return true;
    };
    let want = amount.abs();
    tx.lines.iter().any(|l| {
        let side = if amount.is_positive() {
            l.credit
        } else {
            l.debit
        };
        l.account == bank && side == want
    })
}

/// `POST /banking/feed/:id/review-again`: a row whose entry was voided
/// (Q-P26u) or changed since (Q-PR5a, U-199) goes back to waiting with the
/// state it had before.
///
/// # Errors
/// 403; 409 `ENTRY_NOT_VOIDED`; `NotFound`.
pub fn review_again(state: &mut DemoState, id: FeedItemId) -> Result<(), ApiError> {
    super::refresh(state);
    state.feed_review.write_check(id)?;
    let r = state.feed_review.row_mut(id)?;
    if !r.entry_voided && !r.entry_changed {
        return Err(conflict("ENTRY_NOT_VOIDED", json!({})));
    }
    restore(state, id);
    Ok(())
}

/// Developer: the entry of the first accepted row is voided in the
/// register (Q-P26u). A posted entry is voided in the demo ledger (the row
/// is then flagged on the next read); a row without one is flagged.
pub fn void_first_accepted(state: &mut DemoState) -> bool {
    super::refresh(state);
    let pick = state
        .feed_review
        .rows
        .iter()
        .find(|r| r.status.is_accepted() && !r.entry_voided)
        .map(|r| (r.id, r.matched.filter(|_| r.posted)));
    match pick {
        Some((_, Some(t))) if state.ledger.void(t).is_ok() => {
            super::refresh(state);
            true
        }
        Some((id, _)) => {
            if let Ok(r) = state.feed_review.row_mut(id) {
                r.entry_voided = true;
            }
            true
        }
        None => false,
    }
}

/// Developer: the first review row is flagged a possible duplicate of the
/// first other row with the nearest date (Q-P26b compare).
pub fn mark_duplicate(state: &mut DemoState) -> bool {
    super::refresh(state);
    let rows = &state.feed_review.rows;
    let Some(dup) = rows
        .iter()
        .position(|r| r.waiting() && r.kind == "review" && !r.possible_duplicate)
    else {
        return false;
    };
    let date: CalendarDate = rows[dup].date;
    let other = rows
        .iter()
        .enumerate()
        .filter(|(i, r)| *i != dup && r.account == rows[dup].account)
        .min_by_key(|(_, r)| date.days_until(r.date).abs())
        .map(|(_, r)| r.id);
    let r = &mut state.feed_review.rows[dup];
    r.possible_duplicate = true;
    r.duplicate_of = other;
    true
}

/// The rule list (`GET /practice/conditional-rules`): the sample's payee
/// rules, then the owner's.
#[must_use]
pub fn rules(state: &mut DemoState) -> Vec<cl_api::FeedRule> {
    super::refresh(state);
    let review = &mut state.feed_review;
    if review.rules.is_none() {
        let mut out: Vec<cl_api::FeedRule> = Vec::new();
        for r in &review.rows {
            let (Some(rule), Some(account)) = (&r.rule, &r.suggested) else {
                continue;
            };
            if out
                .iter()
                .any(|x| x.id.as_deref() == Some(rule.id.as_str()))
            {
                continue;
            }
            out.push(cl_api::FeedRule {
                id: Some(rule.id.clone()),
                name: rule.name.clone(),
                active: true,
                auto_accept: rule.auto_accept,
                updated_at: rule.updated_at.clone(),
                description: Some(pattern_of(&r.description)),
                sign: if r.amount.is_positive() {
                    cl_api::RuleSign::MoneyOut
                } else {
                    cl_api::RuleSign::MoneyIn
                },
                account: Some(account.id),
                simple: true,
                ..cl_api::FeedRule::default()
            });
        }
        review.rules = Some(out);
    }
    review.rules.clone().unwrap_or_default()
}

#[allow(clippy::single_match_else, reason = "replace or create")]
/// Creates or replaces a rule; a change makes a new change token, and the
/// rows the rule staged follow (name, auto-accept, token).
///
/// # Errors
/// 403 for a view-only user; the body's checks; `NotFound`.
pub fn save_rule(state: &mut DemoState, rule: &cl_api::FeedRule) -> Result<(), ApiError> {
    if state.feed_review.view_only {
        return Err(ApiError::Forbidden { code: None });
    }
    cl_api::feed_rule_body(rule)?;
    let _ = rules(state);
    let review = &mut state.feed_review;
    review.rule_edits += 1;
    let token = format!("2026-11-04T10:{:02}:00.000Z", review.rule_edits % 60);
    let mut saved = rule.clone();
    saved.updated_at = Some(token);
    saved.simple = true;
    let list = review.rules.get_or_insert_with(Vec::new);
    match &rule.id {
        Some(id) => {
            let at = list
                .iter()
                .position(|x| x.id.as_ref() == Some(id))
                .ok_or(ApiError::NotFound { code: None })?;
            list[at] = saved.clone();
        }
        None => {
            saved.id = Some(format!("d26b0000-0000-4000-8000-{:012}", review.rule_edits));
            list.push(saved.clone());
        }
    }
    for r in &mut review.rows {
        if let Some(x) = r.rule.as_mut().filter(|x| Some(&x.id) == saved.id.as_ref()) {
            x.name.clone_from(&saved.name);
            x.auto_accept = saved.auto_accept;
            x.updated_at.clone_from(&saved.updated_at);
        }
    }
    Ok(())
}

/// Runs the rules over the waiting lines of `connection` (the engine's
/// `bulk-reprocess-rules`, P26 QA-10): a line a rule fits gets the rule's
/// account as its suggestion and the rule's name; nothing posts. Lines the
/// owner already staged, matches and transfers are left alone.
///
/// # Errors
/// 403 for a view-only user.
pub fn reprocess_rules(
    state: &mut DemoState,
    connection: Option<cl_api::BankConnectionId>,
) -> Result<cl_api::RulesRerun, ApiError> {
    if state.feed_review.view_only {
        return Err(ApiError::Forbidden { code: None });
    }
    let chart = super::super::chart();
    let rules: Vec<cl_api::FeedRule> = rules(state).into_iter().filter(|r| r.active).collect();
    let review = &mut state.feed_review;
    let mut out = cl_api::RulesRerun::default();
    for row in &mut review.rows {
        if !row.waiting()
            || connection.is_some_and(|c| super::connection_of(row.account) != c)
            || row.assigned.is_some()
            || row.kind == "match"
            || row.transfer
        {
            continue;
        }
        out.processed += 1;
        let text = row.description.to_lowercase();
        let fits = |r: &&cl_api::FeedRule| {
            let words = r.description.as_deref().map(str::to_lowercase);
            let sign = match r.sign {
                cl_api::RuleSign::Either => true,
                cl_api::RuleSign::MoneyOut => row.amount.is_positive(),
                cl_api::RuleSign::MoneyIn => !row.amount.is_positive(),
            };
            words.is_some_and(|w| text.contains(&w))
                && sign
                && r.bank_account.is_none_or(|b| b == row.account)
                && r.amount.is_none_or(|a| a == row.amount.abs())
        };
        let Some(rule) = rules.iter().find(fits) else {
            continue;
        };
        let Some((rid, account)) = rule.id.clone().zip(rule.account) else {
            continue;
        };
        let name = chart
            .iter()
            .find(|a| a.id == account)
            .map(|a| match a.fields.account_number.as_deref() {
                Some(n) => format!("{n} \u{b7} {}", a.fields.name),
                None => a.fields.name.clone(),
            })
            .unwrap_or_default();
        row.suggested = Some(cl_api::Named { id: account, name });
        row.rule = Some(cl_api::SuggestedRule {
            id: rid,
            kind: "conditional".into(),
            name: rule.name.clone(),
            auto_accept: rule.auto_accept,
            updated_at: rule.updated_at.clone(),
        });
        row.source = Some("rule");
        row.confidence = None;
        row.kind = "ready";
        out.matched += 1;
        out.suggestions_updated += 1;
    }
    Ok(out)
}

/// Deletes a rule (its rows keep their suggestion, without the rule).
///
/// # Errors
/// 403; `NotFound`.
pub fn delete_rule(state: &mut DemoState, id: &str) -> Result<(), ApiError> {
    if state.feed_review.view_only {
        return Err(ApiError::Forbidden { code: None });
    }
    let _ = rules(state);
    let review = &mut state.feed_review;
    let list = review.rules.get_or_insert_with(Vec::new);
    let before = list.len();
    list.retain(|x| x.id.as_deref() != Some(id));
    if list.len() == before {
        return Err(ApiError::NotFound { code: None });
    }
    for r in &mut review.rows {
        if r.rule.as_ref().is_some_and(|x| x.id == id) {
            r.rule = None;
        }
    }
    Ok(())
}

/// Rule suggestions (Q-P26k): a description and account the owner chose
/// at least 5 times, overridden (Undo counts) under 10% of the time, with
/// no rule for it yet and not dismissed.
#[must_use]
pub fn rule_suggestions(state: &mut DemoState) -> Vec<cl_api::RuleSuggestion> {
    let _ = rules(state);
    let chart = super::super::chart();
    let review = &state.feed_review;
    review
        .learned
        .iter()
        .filter(|((p, a), (yes, no))| {
            *yes >= 5
                && u64::from(*no) * 10 < u64::from(*yes + *no)
                && !review.dismissed.contains(&(p.clone(), *a))
                && !review.rules.as_ref().is_some_and(|rs| {
                    rs.iter()
                        .any(|x| x.description.as_deref() == Some(p.as_str()))
                })
        })
        .map(|((p, a), (yes, _))| cl_api::RuleSuggestion {
            payee_pattern: p.clone(),
            account: *a,
            account_name: chart
                .iter()
                .find(|x| x.id == *a)
                .map(|x| x.fields.name.clone())
                .unwrap_or_default(),
            times_confirmed: *yes,
        })
        .collect()
}

/// Never suggests this one again.
pub fn dismiss_suggestion(state: &mut DemoState, s: &cl_api::RuleSuggestion) {
    state
        .feed_review
        .dismissed
        .insert((s.payee_pattern.clone(), s.account));
}
/// Developer: the sample's next bank file (after 11/03) arrives in the
/// review, as an import would bring it (its 11/04 deposit of the two
/// waiting payments). Once.
pub fn add_next_file(state: &mut DemoState) -> usize {
    super::refresh(state);
    let chart = super::super::chart();
    let Some(operating) = chart
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some("1010"))
        .map(|a| a.id)
    else {
        return 0;
    };
    let review = &mut state.feed_review;
    if review.rows.iter().any(|r| r.id == super::id("d26f2000", 1)) {
        return 0;
    }
    let mut n = 0u64;
    for item in cl_company::Sample::load().feed_after_last_file() {
        n += 1;
        review.rows.push(Row {
            id: super::id("d26f2000", n),
            account: operating,
            date: item.date,
            amount: item.amount.checked_neg().unwrap_or(item.amount),
            description: item.description.trim().to_string(),
            status: FeedStatus::Pending,
            kind: "review",
            suggested: None,
            payee: None,
            source: None,
            confidence: None,
            rule: None,
            assigned: None,
            transfer: false,
            entry_voided: false,
            entry_changed: false,
            exclusion_reason: None,
            matched: None,
            import_key: None,
            posted: false,
            how: "",
            prior: None,
            group: None,
            fingerprint: None,
            possible_duplicate: false,
            duplicate_of: None,
        });
    }
    usize::try_from(n).unwrap_or(usize::MAX)
}

#[cfg(test)]
#[path = "feed_review_posting_tests.rs"]
mod tests;
