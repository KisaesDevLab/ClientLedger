//! The demo engine's deposits (P14 WP-D.3): payments waiting in Payments
//! Clearing, grouped deposits (409 for a payment already deposited), fees,
//! cash back and the bank-file groupings. Synthetic.
//!
//! It plays the engine's part for `--demo` only (P14-engine-shapes.md):
//! - **Pending** (`GET /payments/pending-deposits?exact=1`): one row per
//!   live transaction whose net Payments Clearing debit is above zero and
//!   that no live deposit took in, by date then id.
//! - **Create** (`POST /transactions` `deposit`): Dr bank the total; each
//!   source and other line Cr its amount; each deduction (fee) Dr; cash
//!   back Dr its account. A source deposited elsewhere is a 409
//!   `ALREADY_DEPOSITED`; a source amount other than exactly what it holds
//!   is refused. Engine 400s are `ApiError::Local` here (the demo cannot
//!   build an engine validation body).
//! - **Void** releases the sources (they are pending again).
//! - **Groupings** (`GET /banking/feed/{id}/deposit-groupings`): exact
//!   subsets of the pending items within ten days of a bank-file deposit.
//!
//! Every figure is exact `Money`; the closing password is "demo-close".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cl_api::{
    Account, ApiError, ConflictReason, DepositDraft, DepositSource, LocalValidation, TxnType,
};
use cl_company::sample::{FeedItem, SampleTransaction};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};

use super::DemoState;
use super::register::{DemoJournal, DemoLedger, DemoLine, check_closing, conflict_code, txn_type};

/// The most parts (sources, lines, deductions) one deposit takes.
pub const MAX_PARTS: usize = 500;
/// How far (days) a pending item may be from a bank-file deposit.
pub const GROUPING_WINDOW_DAYS: i64 = 10;
/// The most pending items a grouping search considers.
pub const GROUPING_POOL: usize = 40;
/// The search budget (nodes) of one grouping search.
pub const GROUPING_NODES: u32 = 250_000;

/// The demo's own deposit facts (the ledger keeps the posting and the
/// links).
#[derive(Clone, Default)]
pub struct DemoDeposits {
    cash_back: BTreeMap<TransactionId, (AccountId, Money)>,
    /// Deposit writes that reached the demo engine (creates, voids); tests
    /// use it to prove a refused save sent nothing.
    pub writes: u32,
}

impl fmt::Debug for DemoDeposits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDeposits")
            .field("with_cash_back", &self.cash_back.len())
            .field("writes", &self.writes)
            .finish()
    }
}

/// One payment (or daily-sales day) waiting in Payments Clearing.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoPending {
    /// The transaction.
    pub source: TransactionId,
    /// `customer_payment`, `daily_sales`, ...
    pub txn_type: TxnType,
    /// Date.
    pub date: CalendarDate,
    /// Received from (the transaction's contact).
    pub received_from: Option<String>,
    /// That contact's id.
    pub contact: Option<ContactId>,
    /// Reference (check) number.
    pub ref_no: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// Payment method as the engine names it (a numbered customer payment
    /// is a check in the demo; otherwise not known).
    pub method: Option<String>,
    /// Exactly what it holds in Payments Clearing.
    pub amount: Money,
}

/// Ids, the type and the date only: names, memos and amounts are owner
/// data.
impl fmt::Debug for DemoPending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoPending")
            .field("source", &self.source)
            .field("txn_type", &self.txn_type)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// A deposit as the demo engine reads it back.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoDepositRecord {
    /// The deposit.
    pub id: TransactionId,
    /// Date.
    pub date: CalendarDate,
    /// The bank account.
    pub account: AccountId,
    /// What it took in: (source, amount), in the order sent.
    pub sources: Vec<(TransactionId, Money)>,
    /// Cash kept back: (account, amount).
    pub cash_back: Option<(AccountId, Money)>,
    /// The bank amount (header `total`).
    pub total: Money,
    /// Voided.
    pub void: bool,
}

/// Ids, the date and counts only.
impl fmt::Debug for DemoDepositRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDepositRecord")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("account", &self.account)
            .field("sources", &self.sources.len())
            .field("void", &self.void)
            .finish_non_exhaustive()
    }
}

fn chart(state: &mut DemoState) -> Vec<Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

fn tagged<'a>(chart: &'a [Account], tag: &str) -> Option<&'a Account> {
    chart.iter().find(|a| a.system_tag.as_deref() == Some(tag))
}

/// Payments Clearing (`system_tag payments_clearing`).
#[must_use]
pub fn clearing_account(state: &mut DemoState) -> Option<AccountId> {
    tagged(&chart(state), "payments_clearing").map(|a| a.id)
}

/// The net Payments Clearing debit of a transaction.
fn held(t: &SampleTransaction, clearing: &str) -> Option<Money> {
    let lines = t.lines.iter().filter(|l| l.account == clearing);
    let mut net = Money::ZERO;
    for l in lines {
        net = net.checked_add(l.debit).ok()?.checked_sub(l.credit).ok()?;
    }
    Some(net)
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

/// Every item waiting in Payments Clearing (the engine's pending-deposits
/// rows), by date then id.
#[must_use]
pub fn pending(state: &mut DemoState) -> Vec<DemoPending> {
    let chart = chart(state);
    let Some(clearing) =
        tagged(&chart, "payments_clearing").and_then(|a| a.fields.account_number.clone())
    else {
        return Vec::new();
    };
    let contacts = state.contacts_mut().clone();
    let ledger = &state.ledger;
    let mut out: Vec<(u32, DemoPending)> = ledger
        .transactions()
        .iter()
        .filter_map(|t| {
            let id = DemoLedger::id_of(t);
            if ledger.is_void(id) || ledger.deposited_in(id).is_some() {
                return None;
            }
            let amount = held(t, &clearing)?;
            if !amount.is_positive() {
                return None;
            }
            let contact = t.contact.as_deref().and_then(|n| {
                contacts
                    .iter()
                    .find(|c| c.fields.display_name == n)
                    .map(|c| c.id)
            });
            Some((
                t.id,
                DemoPending {
                    source: id,
                    txn_type: txn_type(&t.kind),
                    date: t.date,
                    received_from: t.contact.clone(),
                    contact,
                    ref_no: (!t.number.is_empty()).then(|| t.number.clone()),
                    memo: (!t.memo.is_empty()).then(|| t.memo.clone()),
                    method: (t.kind == "payment" && !t.number.is_empty())
                        .then(|| "check".to_string()),
                    amount,
                },
            ))
        })
        .collect();
    out.sort_by(|(a, x), (b, y)| x.date.cmp(&y.date).then(a.cmp(b)));
    out.into_iter().map(|(_, p)| p).collect()
}

/// The Payments Clearing balance as the engine stores it (debits minus
/// credits of every live transaction; never Σpending).
#[must_use]
pub fn clearing_balance(state: &mut DemoState) -> Money {
    let chart = chart(state);
    let Some(clearing) =
        tagged(&chart, "payments_clearing").and_then(|a| a.fields.account_number.clone())
    else {
        return Money::ZERO;
    };
    let ledger = &state.ledger;
    let held_all = ledger
        .transactions()
        .iter()
        .filter(|t| !ledger.is_void(DemoLedger::id_of(t)))
        .filter_map(|t| held(t, &clearing))
        .collect::<Vec<_>>();
    Money::sum(held_all).unwrap_or(Money::ZERO)
}

/// `POST /transactions` with `txnType: "deposit"` (`strictAmounts`).
///
/// # Errors
/// - `Local(NonPositiveAmount)`: a bank amount not above zero
///   (`DEPOSIT_TOTAL_NOT_POSITIVE`), or a part not above zero.
/// - `Local(InvalidValue)`: a duplicate source, a source that holds
///   nothing, a source amount that is not exactly what it holds, a source
///   line off Payments Clearing (or, for a vendor credit, off Accounts
///   Payable or naming another vendor), cash back to the deposit account
///   or to Payments Clearing, A/R, A/P or Sales Tax Payable, an account
///   outside the chart.
/// - `Local(TooManyLines)`: more than [`MAX_PARTS`] parts.
/// - `Conflict` `ALREADY_DEPOSITED` (a source in another live deposit) or
///   `VENDOR_CREDIT_EXCEEDED`; nothing posts.
/// - `ClosedPeriod` / `ClosingPasswordRejected` (closing date).
#[allow(clippy::too_many_lines, reason = "the engine's checks in order")]
pub fn create(
    state: &mut DemoState,
    d: &DepositDraft,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    state.deposits.writes += 1;
    let chart = chart(state);
    let in_chart = |id: AccountId| chart.iter().any(|a| a.id == id);
    if !in_chart(d.account) {
        return Err(invalid());
    }
    if d.payments.len() + d.lines.len() + d.deductions.len() > MAX_PARTS {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    let parts_positive = d.payments.iter().all(|p| p.amount.is_positive())
        && d.lines.iter().all(|l| l.amount.is_positive())
        && d.deductions.iter().all(|l| l.amount.is_positive());
    if !parts_positive {
        return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
    }
    let total = d.total().ok_or(invalid())?;
    if !total.is_positive() {
        return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
    }
    let tag_of = |id: AccountId| {
        chart
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.system_tag.as_deref())
    };
    if let Some(cb) = &d.cash_back {
        let forbidden = matches!(
            tag_of(cb.account),
            Some(
                "payments_clearing"
                    | "accounts_receivable"
                    | "accounts_payable"
                    | "sales_tax_payable"
            )
        );
        if !cb.amount.is_positive() {
            return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
        }
        if cb.account == d.account {
            return Err(ApiError::Local(LocalValidation::SameAccount));
        }
        if forbidden || !in_chart(cb.account) {
            return Err(invalid());
        }
    }
    let mut seen = BTreeSet::new();
    if !d.payments.iter().all(|p| seen.insert(p.source)) {
        // DEPOSIT_SOURCE_DUPLICATE
        return Err(invalid());
    }
    let clearing_number = tagged(&chart, "payments_clearing")
        .and_then(|a| a.fields.account_number.clone())
        .unwrap_or_default();
    let all = state.ledger.transactions();
    for p in &d.payments {
        let t = all
            .iter()
            .find(|t| DemoLedger::id_of(t) == p.source)
            .filter(|_| !state.ledger.is_void(p.source))
            .ok_or_else(invalid)?;
        match p.kind {
            DepositSource::Payment => {
                if tag_of(p.account) != Some("payments_clearing") || t.kind == "vendor_credit" {
                    return Err(invalid());
                }
                if state.ledger.deposited_in(p.source).is_some() {
                    return Err(conflict_code(
                        ConflictReason::AlreadyExists,
                        "ALREADY_DEPOSITED",
                    ));
                }
                let holds = held(t, &clearing_number).ok_or_else(invalid)?;
                if !holds.is_positive() || holds != p.amount {
                    return Err(invalid());
                }
            }
            DepositSource::VendorCredit => {
                if tag_of(p.account) != Some("accounts_payable") {
                    return Err(invalid());
                }
                let (vendor, open) =
                    super::money::vendor_credit_open(state, p.source).ok_or_else(invalid)?;
                if let Some(c) = p.contact {
                    let named = state
                        .contacts_mut()
                        .iter()
                        .find(|x| x.id == c)
                        .map(|x| x.fields.display_name.clone());
                    if named.as_deref() != Some(vendor.as_str()) {
                        return Err(invalid());
                    }
                }
                if p.amount > open {
                    return Err(super::money::credit_exceeded());
                }
            }
        }
    }
    if !d
        .lines
        .iter()
        .chain(&d.deductions)
        .all(|l| in_chart(l.account))
    {
        return Err(invalid());
    }
    check_closing(closing(state), &[d.date], password)?;

    let credit = |account: AccountId, amount: Money, memo: Option<String>| DemoLine {
        account,
        debit: Money::ZERO,
        credit: amount,
        memo,
    };
    let debit = |account: AccountId, amount: Money, memo: Option<String>| DemoLine {
        account,
        debit: amount,
        credit: Money::ZERO,
        memo,
    };
    let mut lines = vec![debit(d.account, total, None)];
    lines.extend(
        d.payments
            .iter()
            .map(|p| credit(p.account, p.amount, p.memo.clone())),
    );
    lines.extend(
        d.lines
            .iter()
            .map(|l| credit(l.account, l.amount, l.memo.clone())),
    );
    lines.extend(
        d.deductions
            .iter()
            .map(|l| debit(l.account, l.amount, l.memo.clone())),
    );
    if let Some(cb) = &d.cash_back {
        lines.push(debit(cb.account, cb.amount, cb.memo.clone()));
    }
    let journal = DemoJournal {
        kind: "deposit",
        date: d.date,
        number: String::new(),
        contact: None,
        memo: d.memo.clone().unwrap_or_default(),
        lines,
    };
    let id = state
        .ledger
        .save_lines(&chart, &journal, None)
        .ok_or_else(invalid)?;
    let sources: Vec<(TransactionId, Money)> =
        d.payments.iter().map(|p| (p.source, p.amount)).collect();
    state.ledger.link_deposit(id, &sources);
    if let Some(cb) = &d.cash_back {
        state.deposits.cash_back.insert(id, (cb.account, cb.amount));
    }
    Ok(id)
}

/// Voids a deposit: its sources are pending again (vendor credits get
/// their balance back).
///
/// # Errors
/// `NotFound`; the closing-date rule; the ledger's refusal of a second
/// void.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    state.deposits.writes += 1;
    let date = state
        .ledger
        .date_of(id)
        .ok_or(ApiError::NotFound { code: None })?;
    check_closing(closing(state), &[date], password)?;
    state.ledger.void(id)
}

/// A deposit as `GET /transactions/{id}` reads it back (its sources and
/// cash back); `None` for anything else.
#[must_use]
pub fn record(state: &mut DemoState, id: TransactionId) -> Option<DemoDepositRecord> {
    let chart = chart(state);
    let t = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == id && t.kind == "deposit")?;
    let bank = t.lines.first()?;
    let account = chart
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(bank.account.as_str()))?
        .id;
    Some(DemoDepositRecord {
        id,
        date: t.date,
        account,
        sources: state.ledger.deposit_links(id),
        cash_back: state.deposits.cash_back.get(&id).copied(),
        total: bank.debit,
        void: state.ledger.is_void(id),
    })
}

/// Pending items that add up exactly to a bank-file deposit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoGroupings {
    /// The bank amount.
    pub target: Money,
    /// Each set, smallest first, then nearest in date; items by date.
    pub candidates: Vec<Vec<DemoPending>>,
    /// A bound cut the search or the pool.
    pub truncated: bool,
}

/// The bank-file deposits (money in) of the sample's feed.
#[must_use]
pub fn feed_deposits() -> Vec<FeedItem> {
    cl_company::Sample::load()
        .feed
        .into_iter()
        .filter(|f| f.amount.is_positive())
        .collect()
}

struct Search<'a> {
    units: &'a [i128],
    suffix: Vec<i128>,
    nodes: u32,
    truncated: bool,
    found: Vec<Vec<usize>>,
}

impl Search<'_> {
    fn run(&mut self, i: usize, left: i128, picked: &mut Vec<usize>) {
        if left == 0 {
            self.found.push(picked.clone());
            return;
        }
        if i >= self.units.len() || self.suffix[i] < left {
            return;
        }
        if self.nodes >= GROUPING_NODES || self.found.len() >= 1000 {
            self.truncated = true;
            return;
        }
        self.nodes += 1;
        if self.units[i] <= left {
            picked.push(i);
            self.run(i + 1, left - self.units[i], picked);
            picked.pop();
        }
        self.run(i + 1, left, picked);
    }
}

/// The engine's deposit groupings for a bank-file deposit: exact subsets
/// (in ten-thousandths) of the pending items dated within
/// [`GROUPING_WINDOW_DAYS`] of it, each at most the bank amount; at most
/// [`GROUPING_POOL`] items (nearest dates) and [`GROUPING_NODES`] nodes.
/// Sets differing only by same-amount items are reported once. At most
/// `limit` sets (1 to 20).
#[must_use]
pub fn deposit_groupings(state: &mut DemoState, feed: &FeedItem, limit: usize) -> DemoGroupings {
    let target = feed.amount.abs();
    let window = |p: &DemoPending| feed.date.days_until(p.date).abs();
    let mut pool: Vec<DemoPending> = pending(state)
        .into_iter()
        .filter(|p| window(p) <= GROUPING_WINDOW_DAYS && p.amount <= target)
        .collect();
    let mut truncated = false;
    pool.sort_by(|a, b| window(a).cmp(&window(b)).then(a.date.cmp(&b.date)));
    if pool.len() > GROUPING_POOL {
        pool.truncate(GROUPING_POOL);
        truncated = true;
    }
    let units: Vec<i128> = pool.iter().map(|p| p.amount.to_ten_thousandths()).collect();
    let mut suffix = vec![0i128; units.len() + 1];
    for i in (0..units.len()).rev() {
        suffix[i] = suffix[i + 1] + units[i];
    }
    let mut search = Search {
        units: &units,
        suffix,
        nodes: 0,
        truncated: false,
        found: Vec::new(),
    };
    if target.is_positive() {
        search.run(0, target.to_ten_thousandths(), &mut Vec::new());
    }
    truncated |= search.truncated;
    // Once per multiset of amounts (the nearest items win: the pool is
    // nearest first and the search takes earlier items first).
    let mut keys = BTreeSet::new();
    let mut sets: Vec<(i64, Vec<DemoPending>)> = Vec::new();
    for set in search.found {
        let mut key: Vec<i128> = set.iter().map(|&i| units[i]).collect();
        key.sort_unstable();
        if !keys.insert(key) {
            continue;
        }
        let distance: i64 = set.iter().map(|&i| window(&pool[i])).sum();
        let mut items: Vec<DemoPending> = set.iter().map(|&i| pool[i].clone()).collect();
        items.sort_by_key(|p| p.date);
        sets.push((distance, items));
    }
    // Fewest items, then the smallest mean distance (compared exactly).
    sets.sort_by(|(da, a), (db, b)| {
        a.len().cmp(&b.len()).then_with(|| {
            let (la, lb) = (
                i64::try_from(a.len()).unwrap_or(i64::MAX),
                i64::try_from(b.len()).unwrap_or(i64::MAX),
            );
            (da.saturating_mul(lb)).cmp(&db.saturating_mul(la))
        })
    });
    DemoGroupings {
        target,
        candidates: sets
            .into_iter()
            .take(limit.clamp(1, 20))
            .map(|(_, s)| s)
            .collect(),
        truncated,
    }
}

/// The "Bank deposits waiting" card: each bank-file deposit with at least
/// one exact grouping (best first).
#[must_use]
pub fn bank_deposits_waiting(state: &mut DemoState) -> Vec<(FeedItem, DemoGroupings)> {
    feed_deposits()
        .into_iter()
        .filter_map(|f| {
            let g = deposit_groupings(state, &f, 5);
            (!g.candidates.is_empty()).then_some((f, g))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use cl_api::{CashBack, GroupedPayment, SplitLine};

    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("m")
    }

    fn story() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("d")
    }

    fn grouped(state: &mut DemoState, items: &[DemoPending]) -> DepositDraft {
        let clearing = clearing_account(state).expect("10150");
        let mut d = DepositDraft::plain(super::super::account_id(1010), story(), None, Vec::new());
        d.payments = items
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
        d
    }

    fn plain_line(n: u32, amount: &str) -> SplitLine {
        SplitLine {
            account: super::super::account_id(n),
            amount: m(amount),
            memo: None,
            contact: None,
            billable: false,
            tag: None,
        }
    }

    /// Mirrors `contract_deposits` (AC1): the two waiting payments group
    /// into one 2,092.50 deposit that empties Payments Clearing; grouping
    /// again is a 409 with nothing posted; a void returns them (E2).
    #[test]
    fn grouped_deposit_clears_payments_clearing() {
        let mut state = DemoState::new();
        let waiting = pending(&mut state);
        let amounts: Vec<Money> = waiting.iter().map(|p| p.amount).collect();
        assert_eq!(amounts, vec![m("1250.00"), m("842.50")]);
        assert_eq!(clearing_balance(&mut state), m("2092.50"));
        assert_eq!(waiting[0].txn_type, TxnType::CustomerPayment);
        assert_eq!(waiting[0].ref_no.as_deref(), Some("5521"));

        let d = grouped(&mut state, &waiting);
        let id = create(&mut state, &d, None).expect("deposit");
        let rec = record(&mut state, id).expect("record");
        assert_eq!(rec.total, m("2092.50"));
        assert_eq!(rec.sources.len(), 2);
        assert_eq!(pending(&mut state), Vec::new());
        assert_eq!(clearing_balance(&mut state), Money::ZERO);

        // Again: 409 ALREADY_DEPOSITED, nothing posts.
        let count = state.ledger.transactions().len();
        let err = create(&mut state, &d, None).expect_err("409");
        assert!(matches!(
            err,
            ApiError::Conflict { code: Some(ref c), .. } if c.as_str() == "ALREADY_DEPOSITED"
        ));
        assert_eq!(state.ledger.transactions().len(), count);
        // A deposited payment can't be voided.
        assert!(state.ledger.void(waiting[0].source).is_err());

        void(&mut state, id, None).expect("void");
        assert_eq!(pending(&mut state).len(), 2);
        assert_eq!(clearing_balance(&mut state), m("2092.50"));
    }

    /// Mirrors the refusals: an amount a cent off, a duplicate source, a
    /// total not above zero, cash back to the bank, a closed period.
    #[test]
    fn refusals_post_nothing() {
        let mut state = DemoState::new();
        let waiting = pending(&mut state);
        let count = state.ledger.transactions().len();
        let mut d = grouped(&mut state, &waiting[..1]);
        d.payments[0].amount = m("1249.99");
        assert_eq!(create(&mut state, &d, None), Err(invalid()));
        let mut d = grouped(&mut state, &waiting[..1]);
        d.payments.push(d.payments[0].clone());
        assert_eq!(create(&mut state, &d, None), Err(invalid()));
        let mut d = grouped(&mut state, &waiting[..1]);
        d.cash_back = Some(CashBack {
            account: super::super::account_id(30170),
            amount: m("1250.00"),
            memo: None,
        });
        assert_eq!(
            create(&mut state, &d, None),
            Err(ApiError::Local(LocalValidation::NonPositiveAmount))
        );
        d.cash_back = Some(CashBack {
            account: super::super::account_id(1010),
            amount: m("50.00"),
            memo: None,
        });
        assert_eq!(
            create(&mut state, &d, None),
            Err(ApiError::Local(LocalValidation::SameAccount))
        );
        let mut d = grouped(&mut state, &waiting[..1]);
        d.date = CalendarDate::from_ymd(2026, 9, 30).expect("d");
        assert!(matches!(
            create(&mut state, &d, None),
            Err(ApiError::ClosedPeriod(_))
        ));
        assert_eq!(state.ledger.transactions().len(), count);
        let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD.to_string());
        create(&mut state, &d, Some(&pw)).expect("with the password");
    }

    /// Mirrors the cash-back contract: other 300.00, a 12.50 fee and 50.00
    /// cash back post Dr bank 237.50, Dr fee, Dr cash back, Cr other.
    #[test]
    fn deductions_and_cash_back_post_exactly() {
        let mut state = DemoState::new();
        let mut d = DepositDraft::plain(
            super::super::account_id(1010),
            story(),
            None,
            vec![plain_line(4000, "300.00")],
        );
        d.deductions = vec![plain_line(6250, "12.50")];
        d.cash_back = Some(CashBack {
            account: super::super::account_id(30170),
            amount: m("50.00"),
            memo: None,
        });
        let id = create(&mut state, &d, None).expect("deposit");
        let rec = record(&mut state, id).expect("record");
        assert_eq!(rec.total, m("237.50"));
        assert_eq!(
            rec.cash_back,
            Some((super::super::account_id(30170), m("50.00")))
        );
        let t = state
            .ledger
            .transactions()
            .into_iter()
            .find(|t| DemoLedger::id_of(t) == id)
            .expect("stored");
        let dr: Vec<(&str, Money)> = t
            .lines
            .iter()
            .filter(|l| l.debit.is_positive())
            .map(|l| (l.account.as_str(), l.debit))
            .collect();
        assert_eq!(
            dr,
            vec![
                ("1010", m("237.50")),
                ("6250", m("12.50")),
                ("30170", m("50.00"))
            ]
        );
    }

    /// Mirrors the feed-groupings contract: the 11/04 bank deposit of
    /// 2,092.50 groups exactly the two waiting payments, and nothing once
    /// they are deposited.
    #[test]
    fn feed_groupings_are_exact_subsets() {
        let mut state = DemoState::new();
        let waiting = bank_deposits_waiting(&mut state);
        assert_eq!(waiting.len(), 1);
        let (feed, g) = &waiting[0];
        assert_eq!(feed.date, story());
        assert_eq!(g.target, m("2092.50"));
        assert_eq!(g.candidates.len(), 1);
        let sum = Money::sum(g.candidates[0].iter().map(|p| p.amount)).expect("sum");
        assert_eq!(sum, g.target);
        assert!(!g.truncated);

        // Not every subset: a 1,250.00 deposit has one single-item set.
        let one = FeedItem {
            date: story(),
            description: "DEPOSIT".into(),
            amount: m("1250.00"),
            status: "ready".into(),
        };
        let g1 = deposit_groupings(&mut state, &one, 5);
        assert_eq!(g1.candidates.len(), 1);
        assert_eq!(g1.candidates[0].len(), 1);

        let items = g.candidates[0].clone();
        let d = grouped(&mut state, &items);
        create(&mut state, &d, None).expect("deposit");
        assert!(bank_deposits_waiting(&mut state).is_empty());
    }
}
