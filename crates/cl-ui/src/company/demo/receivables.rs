//! The demo "engine" for Receive Payment (P21 D2): payment preview
//! (allocation oldest first, explicit fills, discounts, write-offs,
//! credits, deposits), receive, edit, void, unapply, apply, bounce (always
//! one dated "Bounced payment" entry, Q-P21i/u), refunds and the
//! receivables settings, on the shared [`DemoState`]. Invoices move through
//! `demo/invoices.rs`; postings go to the demo ledger, so Make Deposit and
//! the registers see them.
//!
//! `--demo` only. The arithmetic here is the engine's rule in
//! `rust_decimal`, held to `fixtures/payments/golden.json` (every case runs
//! through it in `receivables_tests.rs`). Whole cents only (Q-P21t). No
//! floats. All data is synthetic.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    Account, AmountSource, ApiError, AppliedAmount, ApplyRequest, ApplyResult, BalanceToken,
    BounceRef, BounceRequest, BounceResult, ConflictReason, CreditSource, DepositOnHand,
    DepositedIn, HeldIn, HeldKind, LocalValidation, NewCustomerPayment, NewCustomerRefund,
    PaymentApplication, PaymentPreview, PaymentPreviewRequest, PaymentRecord, PaymentStatus,
    PaymentSummary, PaymentTotals, PendingAfter, PreviewCreditUse, PreviewInvoice, PreviewProblem,
    RecordCreditUse, ReopenedInvoice, Shortfall, UserText,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret, Timestamp};
use rust_decimal::Decimal;
use serde_json::json;

use super::DemoState;
use super::invoices::{Receivable, receivable, receivables_of, round, settle};
use super::register::{DemoJournal, DemoLedger, DemoLine, check_closing};

/// The mockup's client.
pub const STORY_CLIENT: &str = "Neosho Dental Group";

/// Customer Deposits (liability; created on first use, as the engine's
/// lazy system account).
pub const CUSTOMER_DEPOSITS: u32 = 2400;
/// Bad debts (the demo's write-off account choice; not a default).
pub const BAD_DEBTS: u32 = 6960;
/// Sales discounts (the demo's discount account choice; not a default).
pub const SALES_DISCOUNTS: u32 = 4950;
/// Accounts Receivable.
const AR: u32 = 1200;
/// Operating Checking.
pub const OPERATING: u32 = 1010;

/// Invoice ids of the story (outside the Create Invoice numbering).
fn story_invoice(n: u32) -> Option<TransactionId> {
    TransactionId::parse(&format!("d1000000-0000-4000-8000-0000000{n:05}")).ok()
}

/// The receivables settings (engine company settings, Q-P21q).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoReceivablesSettings {
    /// Sales discounts post here.
    pub discount_account: Option<AccountId>,
    /// Write-offs post here.
    pub write_off_account: Option<AccountId>,
    /// The write-off limit (1.00 by default, Q-P21h).
    pub write_off_threshold: Option<Money>,
    /// The bank fee expense account.
    pub nsf_bank_fee_account: Option<AccountId>,
    /// The fee invoice income account.
    pub nsf_fee_income_account: Option<AccountId>,
    /// The client's bounced-check fee (blank until set, Q-P21d).
    pub nsf_customer_fee: Option<Money>,
}

impl Default for DemoReceivablesSettings {
    fn default() -> Self {
        DemoReceivablesSettings {
            discount_account: None,
            write_off_account: None,
            write_off_threshold: Money::parse_wire("1.00").ok(),
            nsf_bank_fee_account: None,
            nsf_fee_income_account: None,
            nsf_customer_fee: None,
        }
    }
}

/// One application row.
#[derive(Clone, PartialEq, Eq)]
struct App {
    invoice: TransactionId,
    cash: Decimal,
    discount: Decimal,
    write_off: Decimal,
    applied: CalendarDate,
    /// A credit used inside another receipt: that receipt.
    via: Option<TransactionId>,
    /// A credit of this payment applied after it was received (inside
    /// another receipt or by Apply), whatever its date.
    later: bool,
    voided: bool,
    reversed: bool,
}

impl App {
    fn live(&self) -> bool {
        !self.voided && !self.reversed
    }
    fn settled(&self) -> Decimal {
        self.cash + self.discount + self.write_off
    }
}

/// One customer payment the demo engine keeps.
#[derive(Clone)]
struct Pay {
    customer: ContactId,
    date: CalendarDate,
    amount: Decimal,
    deposit_to: AccountId,
    method: Option<String>,
    ref_no: Option<String>,
    memo: Option<String>,
    version: i64,
    void: bool,
    hold: bool,
    /// A draw on this held deposit.
    from_deposit: Option<TransactionId>,
    /// A draw made inside this receipt.
    inside: Option<TransactionId>,
    apps: Vec<App>,
    ledger: Option<TransactionId>,
    discount_account: Option<AccountId>,
    write_off_account: Option<AccountId>,
    bounce: Option<(TransactionId, CalendarDate)>,
    refunded: Decimal,
}

/// Customer payments, settings and history of the demo engine.
#[derive(Clone, Default)]
pub struct DemoReceivables {
    seeded: bool,
    pays: BTreeMap<TransactionId, Pay>,
    next: u64,
    keys: BTreeMap<String, TransactionId>,
    bounce_keys: BTreeMap<String, BounceResult>,
    /// Early-payment terms of seeded invoices: (percent, days).
    terms: BTreeMap<TransactionId, (Decimal, i64)>,
    /// Invoices a discount was taken on.
    discounted: BTreeMap<TransactionId, bool>,
    /// Days each client took to pay invoices paid before the demo
    /// (`averageDaysToPay`, Q-P21r).
    history: BTreeMap<ContactId, Vec<i64>>,
    /// The settings.
    pub settings: DemoReceivablesSettings,
    /// Write requests that reached the demo engine (tests).
    pub writes: u32,
}

impl fmt::Debug for DemoReceivables {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoReceivables")
            .field("payments", &self.pays.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

// ---- Errors ------------------------------------------------------------------------

pub(super) fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

pub(super) fn conflict(code: &str) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: cl_api::ErrorCode::parse(code),
        message: UserText::default(),
        details: cl_api::ConflictDetails::default(),
    }
}

/// The engine's 409 with `code` and its `details` (`{sourceId,
/// remaining}`, `{contactId, charge}`), mapped as cl-api maps it.
pub(super) fn conflict_with(code: &str, details: &serde_json::Value) -> ApiError {
    cl_api::engine_refusal(
        409,
        &json!({"error": {"message": "conflict", "code": code, "details": details}}),
    )
}

/// The 409 `CREDIT_NOT_AVAILABLE` of `source` with what is left of it.
pub(super) fn credit_not_available(source: TransactionId, left: Decimal) -> ApiError {
    let remaining =
        money(left.max(Decimal::ZERO)).map_or_else(|_| "0.00".to_string(), Money::to_wire);
    conflict_with(
        "CREDIT_NOT_AVAILABLE",
        &json!({"sourceId": source.to_string(), "remaining": remaining}),
    )
}

pub(super) fn refused(code: &str) -> ApiError {
    cl_api::engine_refusal(
        400,
        &json!({"error": {"message": "refused", "code": code, "details": {}}}),
    )
}

pub(super) fn stale() -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Stale,
        current_version: None,
        code: cl_api::ErrorCode::parse("STALE"),
        message: UserText::default(),
        details: cl_api::ConflictDetails::default(),
    }
}

pub(super) fn money(d: Decimal) -> Result<Money, ApiError> {
    Money::try_from(round(d, 2)).map_err(|_| invalid())
}

pub(super) fn dec(m: Money) -> Decimal {
    m.as_decimal()
}

/// The stored balance as the engine echoes it (`balanceDueExact`).
pub(super) fn token(d: Decimal) -> Result<BalanceToken, ApiError> {
    Ok(BalanceToken::from_wire(&money(d)?.to_wire()))
}

pub(super) fn version_of(v: i64) -> Timestamp {
    let base = DemoState::now().unix_millis();
    Timestamp::from_unix_millis(base + v).unwrap_or_else(|_| DemoState::now())
}

pub(super) fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

// ---- Chart and seed ----------------------------------------------------------------

pub(super) fn chart(state: &mut DemoState) -> Vec<Account> {
    state.chart.get_or_insert_with(super::chart).clone()
}

pub(super) fn tagged(chart: &[Account], tag: &str) -> Option<AccountId> {
    chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .map(|a| a.id)
}

pub(super) fn ensure_account(
    state: &mut DemoState,
    number: u32,
    name: &str,
    kind: cl_api::AccountType,
    detail: &str,
    tag: Option<&str>,
) -> AccountId {
    let id = super::account_id(number);
    let chart = state.chart.get_or_insert_with(super::chart);
    if !chart.iter().any(|a| a.id == id) {
        chart.push(Account {
            id,
            fields: cl_api::AccountFields {
                name: name.into(),
                account_number: Some(number.to_string()),
                account_type: kind,
                detail_type: Some(detail.into()),
                description: None,
                parent_id: None,
            },
            is_active: true,
            is_system: tag.is_some(),
            system_tag: tag.map(str::to_string),
            balance: Some(Money::ZERO),
            created_at: Some(DemoState::now()),
            updated_at: Some(DemoState::now()),
            card_last4: None,
        });
    }
    id
}

/// Customer Deposits, created on first use.
pub fn customer_deposits(state: &mut DemoState) -> AccountId {
    ensure_account(
        state,
        CUSTOMER_DEPOSITS,
        "Customer Deposits",
        cl_api::AccountType::Liability,
        "other_current_liability",
        Some("customer_deposits"),
    )
}

/// Payments Clearing (Undeposited Funds).
pub fn clearing(state: &mut DemoState) -> Option<AccountId> {
    tagged(&chart(state), "payments_clearing")
}

/// The story client's id.
pub fn story_client(state: &mut DemoState) -> Option<ContactId> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == STORY_CLIENT)
        .map(|c| c.id)
}

/// Seeds the story once: Neosho Dental Group's invoices 1091 (10/05/2026,
/// due 11/04, 4,385.00) and 1095 (10/28, due 11/27, 1,260.00), its paid
/// history (usually 24 days) and the discount and bad-debt accounts the
/// owner may pick.
pub fn seed(state: &mut DemoState) {
    if state.receivables.seeded {
        return;
    }
    state.receivables.seeded = true;
    ensure_account(
        state,
        BAD_DEBTS,
        "Bad Debts",
        cl_api::AccountType::Expense,
        "other_expense",
        None,
    );
    ensure_account(
        state,
        SALES_DISCOUNTS,
        "Sales Discounts",
        cl_api::AccountType::Revenue,
        "discounts_refunds_given",
        None,
    );
    let Some(client) = story_client(state) else {
        return;
    };
    let day = |m, d| CalendarDate::from_ymd(2026, m, d).ok();
    let rows = [
        (1091, day(10, 5), day(11, 4), "4385.00"),
        (1095, day(10, 28), day(11, 27), "1260.00"),
    ];
    for (n, date, due, total) in rows {
        if let (Some(id), Some(date), Some(due), Ok(total)) =
            (story_invoice(n), date, due, Money::parse_wire(total))
        {
            let _ = super::invoices::seed_receivable(
                state,
                id,
                client,
                &n.to_string(),
                date,
                due,
                total,
            );
        }
    }
    state.receivables.history.insert(client, vec![22, 24, 26]);
}

/// Seeds a golden or test invoice with early-payment terms.
///
/// # Errors
/// As [`super::invoices::seed_receivable`].
pub fn seed_invoice_with_terms(
    state: &mut DemoState,
    id: TransactionId,
    client: ContactId,
    number: &str,
    dates: (CalendarDate, CalendarDate),
    total: Money,
    terms: Option<(Decimal, i64)>,
) -> Result<(), ApiError> {
    super::invoices::seed_receivable(state, id, client, number, dates.0, dates.1, total)?;
    if let Some(t) = terms {
        state.receivables.terms.insert(id, t);
    }
    Ok(())
}

// ---- Reads -------------------------------------------------------------------------

pub(super) fn name_of(state: &mut DemoState, client: ContactId) -> Option<String> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.id == client)
        .map(|c| c.fields.display_name.clone())
}

fn bounced(p: &Pay) -> bool {
    p.bounce.is_some()
}

/// Credit left on a payment (0 with a live bounce, a void, or a draw).
fn credit_remaining(p: &Pay) -> Decimal {
    if p.void || bounced(p) || p.hold || p.from_deposit.is_some() {
        return Decimal::ZERO;
    }
    let used: Decimal = p.apps.iter().filter(|a| a.live()).map(|a| a.cash).sum();
    p.amount - used - p.refunded
}

/// What is left of a held deposit.
fn deposit_remaining(r: &DemoReceivables, id: TransactionId) -> Decimal {
    let Some(p) = r.pays.get(&id) else {
        return Decimal::ZERO;
    };
    if p.void || bounced(p) || !p.hold {
        return Decimal::ZERO;
    }
    let drawn: Decimal = r
        .pays
        .values()
        .filter(|d| d.from_deposit == Some(id) && !d.void)
        .flat_map(|d| d.apps.iter().filter(|a| a.live()))
        .map(|a| a.cash)
        .sum();
    p.amount - drawn - p.refunded
}

/// The client's balance: open invoice balances less unapplied credits
/// and credit memo credit (P22; held deposits are a liability, not A/R).
#[must_use]
pub fn contact_balance(state: &DemoState, client: ContactId) -> Decimal {
    payments_balance(state, client) - super::credit_memos::memo_credit(state, client)
}

fn payments_balance(state: &DemoState, client: ContactId) -> Decimal {
    let open: Decimal = receivables_of(state, client)
        .iter()
        .map(|r| r.balance)
        .sum();
    let credits: Decimal = state
        .receivables
        .pays
        .values()
        .filter(|p| p.customer == client)
        .map(credit_remaining)
        .sum();
    open - credits
}

/// The Customer Deposits balance (what is held, not yet applied).
#[must_use]
pub fn deposits_held(state: &DemoState) -> Decimal {
    let r = &state.receivables;
    r.pays
        .iter()
        .filter(|(_, p)| p.hold)
        .map(|(id, _)| deposit_remaining(r, *id))
        .sum()
}

/// True when the ledger transaction is a payment with a live bounce (Make
/// Deposit leaves it out; a deposit holding it can't be voided).
#[must_use]
pub fn is_bounced_source(state: &DemoState, ledger: TransactionId) -> bool {
    state
        .receivables
        .pays
        .values()
        .any(|p| p.ledger == Some(ledger) && bounced(p))
}

fn deposit_of(state: &DemoState, p: &Pay) -> Option<(TransactionId, CalendarDate)> {
    let l = p.ledger?;
    let dep = state.ledger.deposited_in(l)?;
    Some((dep, state.ledger.date_of(dep)?))
}

fn record_of(state: &DemoState, id: TransactionId) -> Result<PaymentRecord, ApiError> {
    let r = &state.receivables;
    let p = r.pays.get(&id).ok_or(ApiError::NotFound { code: None })?;
    let mut apps = Vec::new();
    for a in &p.apps {
        apps.push(PaymentApplication {
            invoice_id: a.invoice,
            invoice_number: receivable(state, a.invoice)
                .map(|i| i.number)
                .unwrap_or_default(),
            amount: money(a.cash)?,
            discount: money(a.discount)?,
            write_off: money(a.write_off)?,
            applied_date: Some(a.applied),
            voided: a.voided,
            reversed: a.reversed,
            applied_in_txn_id: a.via,
            applied_later: a.later,
        });
    }
    let mut credit_applications = Vec::new();
    for (source, x, inv) in used_inside(state, id) {
        credit_applications.push(RecordCreditUse {
            source_payment_id: source,
            invoice_id: inv,
            invoice_number: receivable(state, inv).map(|i| i.number).unwrap_or_default(),
            amount: money(x)?,
            from_deposit: r.pays.get(&source).is_some_and(|s| s.hold),
        });
    }
    Ok(PaymentRecord {
        id,
        customer_id: p.customer,
        date: p.date,
        amount: money(p.amount)?,
        // A payment of 0 that only wrote off has no deposit line.
        deposit_to: (!p.amount.is_zero()).then_some(p.deposit_to),
        payment_method: p.method.clone(),
        ref_no: p.ref_no.clone(),
        memo: p.memo.clone(),
        updated_at: version_of(p.version),
        status: if p.void {
            PaymentStatus::Void
        } else {
            PaymentStatus::Posted
        },
        deposited_in: deposit_of(state, p)
            .map(|(deposit_id, date)| DepositedIn { deposit_id, date }),
        bounce: p
            .bounce
            .map(|(return_id, date)| BounceRef { return_id, date }),
        applications: apps,
        unapplied_remaining: money(credit_remaining(p))?,
        holds_deposit: p.hold,
        deposit_remaining: money(deposit_remaining(r, id))?,
        credit_applications,
        write_off_account_id: p.write_off_account,
        discount_account_id: p.discount_account,
    })
}

/// The live credits and deposit draws used inside receipt `id`: source,
/// amount, invoice (the record's `creditApplications`).
fn used_inside(
    state: &DemoState,
    id: TransactionId,
) -> Vec<(TransactionId, Decimal, TransactionId)> {
    let mut v = Vec::new();
    for (pid, p) in &state.receivables.pays {
        for a in p.apps.iter().filter(|a| a.live() && a.via == Some(id)) {
            v.push((*pid, a.cash, a.invoice));
        }
        if p.inside == Some(id)
            && !p.void
            && let Some(source) = p.from_deposit
        {
            for a in p.apps.iter().filter(|a| a.live()) {
                v.push((source, a.cash, a.invoice));
            }
        }
    }
    v
}

/// What of credit or deposit `source` the receipt being edited uses (an
/// edit sees it as available again, as the engine's preview does).
fn used_by_edit(
    state: &DemoState,
    source: TransactionId,
    editing: Option<TransactionId>,
) -> Decimal {
    let Some(e) = editing else {
        return Decimal::ZERO;
    };
    used_inside(state, e)
        .into_iter()
        .filter(|(s, _, _)| *s == source)
        .map(|(_, x, _)| x)
        .sum()
}

/// `GET /payments/:id`.
///
/// # Errors
/// `NotFound`.
pub fn get(state: &mut DemoState, id: TransactionId) -> Result<PaymentRecord, ApiError> {
    seed(state);
    // The register and Client Center hand over the ledger row's id (the
    // engine's payment and its transaction share one id).
    let id = if state.receivables.pays.contains_key(&id) {
        id
    } else {
        payment_of_ledger(state, id).unwrap_or(id)
    };
    record_of(state, id)
}

/// `GET /payments`: receipts (not draws), oldest first, optionally one
/// client's.
/// The client's customer payments as Client Center history rows (newest
/// first), as the engine's contact transactions list them.
#[must_use]
pub fn history_rows(state: &DemoState, client: ContactId) -> Vec<cl_api::TransactionSummary> {
    let mut v: Vec<cl_api::TransactionSummary> = state
        .receivables
        .pays
        .iter()
        .filter(|(_, p)| p.from_deposit.is_none() && p.customer == client)
        .filter_map(|(id, p)| {
            let amount = money(p.amount).ok()?;
            Some(cl_api::TransactionSummary {
                id: *id,
                txn_type: cl_api::TxnType::CustomerPayment,
                txn_number: p.ref_no.clone(),
                txn_date: p.date,
                status: if p.void {
                    cl_api::TxnStatus::Void
                } else {
                    cl_api::TxnStatus::Posted
                },
                contact_id: Some(client),
                contact_name: None,
                memo: None,
                total: Some(amount),
                display_total: Some(amount),
                created_at: None,
                check_number: None,
            })
        })
        .collect();
    v.sort_by(|a, b| b.txn_date.cmp(&a.txn_date).then(b.id.cmp(&a.id)));
    v
}

pub fn list(state: &mut DemoState, client: Option<ContactId>) -> Vec<PaymentSummary> {
    seed(state);
    let mut v: Vec<(CalendarDate, TransactionId, PaymentSummary)> = state
        .receivables
        .pays
        .iter()
        .filter(|(_, p)| p.from_deposit.is_none() && client.is_none_or(|c| c == p.customer))
        .filter_map(|(id, p)| {
            Some((
                p.date,
                *id,
                PaymentSummary {
                    id: *id,
                    customer_id: p.customer,
                    date: p.date,
                    amount: money(p.amount).ok()?,
                    ref_no: p.ref_no.clone(),
                    status: if p.void {
                        PaymentStatus::Void
                    } else {
                        PaymentStatus::Posted
                    },
                    holds_deposit: p.hold,
                    deposit_remaining: money(deposit_remaining(&state.receivables, *id)).ok()?,
                    unapplied_remaining: money(credit_remaining(p)).ok()?,
                },
            ))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    v.into_iter().map(|(_, _, s)| s).collect()
}

/// `GET /payments/deposits-on-hand?customerId=`.
pub fn deposits_on_hand(state: &mut DemoState, client: ContactId) -> Vec<DepositOnHand> {
    seed(state);
    let r = &state.receivables;
    r.pays
        .iter()
        .filter(|(_, p)| p.customer == client && p.hold)
        .filter_map(|(id, p)| {
            let left = deposit_remaining(r, *id);
            (left > Decimal::ZERO).then(|| {
                Some(DepositOnHand {
                    payment_id: *id,
                    date: p.date,
                    ref_no: p.ref_no.clone(),
                    amount: money(p.amount).ok()?,
                    remaining: money(left).ok()?,
                })
            })?
        })
        .collect()
}

fn credits_of(
    state: &DemoState,
    client: ContactId,
    except: Option<TransactionId>,
) -> Vec<CreditSource> {
    state
        .receivables
        .pays
        .iter()
        .filter(|(id, p)| p.customer == client && Some(**id) != except)
        .filter_map(|(id, p)| {
            let left = credit_remaining(p) + used_by_edit(state, *id, except);
            (left > Decimal::ZERO).then(|| {
                Some(CreditSource {
                    source_payment_id: *id,
                    date: p.date,
                    ref_no: p.ref_no.clone(),
                    remaining: money(left).ok()?,
                })
            })?
        })
        .collect()
}

/// Days the client usually takes to pay (Q-P21r): the mean over invoices
/// fully paid, from the invoice date to the payment that cleared it; none
/// below three.
fn average_days(state: &DemoState, client: ContactId) -> Option<u32> {
    let mut days: Vec<i64> = state
        .receivables
        .history
        .get(&client)
        .cloned()
        .unwrap_or_default();
    for p in state
        .receivables
        .pays
        .values()
        .filter(|p| p.customer == client && !p.void)
    {
        for a in p.apps.iter().filter(|a| a.live()) {
            if let Some(inv) = receivable(state, a.invoice)
                && inv.balance.is_zero()
            {
                days.push(inv.date.days_until(a.applied));
            }
        }
    }
    if days.len() < 3 {
        return None;
    }
    let n = i64::try_from(days.len()).ok()?;
    let sum: i64 = days.iter().sum();
    u32::try_from((sum + n / 2) / n).ok()
}

// ---- The compute (preview = post) ------------------------------------------------

/// One row as computed.
#[derive(Clone)]
struct Row {
    inv: Receivable,
    /// The cent balance this payment sees (an edit adds its own share back).
    balance: Decimal,
    cash: Decimal,
    discount: Decimal,
    write_off: Decimal,
    credit: Decimal,
    available: Decimal,
    suggest: bool,
    pct: Option<(Decimal, CalendarDate)>,
}

impl Row {
    fn settled(&self) -> Decimal {
        self.cash + self.discount + self.write_off + self.credit
    }
}

/// The result of one compute.
struct Computed {
    amount: Decimal,
    source: AmountSource,
    rows: Vec<Row>,
    credit_uses: Vec<(TransactionId, TransactionId, Decimal)>,
    problems: Vec<PreviewProblem>,
}

/// An application as the compute takes it (preview or save).
struct AppIn {
    invoice: TransactionId,
    cash: Option<Decimal>,
    discount: Decimal,
    write_off: Decimal,
}

/// A credit or deposit use as the compute takes it.
struct CreditIn {
    source: TransactionId,
    invoice: TransactionId,
    amount: Option<Decimal>,
}

fn problem(code: &str, invoice: Option<TransactionId>) -> PreviewProblem {
    PreviewProblem {
        code: code.into(),
        invoice_id: invoice,
    }
}

/// The edited payment's own live share of each invoice (an edit sees the
/// balances as if it were not there).
fn own_share(
    state: &DemoState,
    editing: Option<TransactionId>,
) -> BTreeMap<TransactionId, Decimal> {
    let mut m = BTreeMap::new();
    let Some(e) = editing else {
        return m;
    };
    for p in state
        .receivables
        .pays
        .iter()
        .filter_map(|(id, p)| (*id == e || p.inside == Some(e)).then_some(p))
    {
        for a in p.apps.iter().filter(|a| a.live() && a.via.is_none()) {
            *m.entry(a.invoice).or_insert(Decimal::ZERO) += a.settled();
        }
    }
    for p in state.receivables.pays.values() {
        for a in p.apps.iter().filter(|a| a.live() && a.via == Some(e)) {
            *m.entry(a.invoice).or_insert(Decimal::ZERO) += a.settled();
        }
    }
    m
}

#[allow(clippy::too_many_lines)]
fn compute(
    state: &DemoState,
    client: ContactId,
    date: CalendarDate,
    amount: Option<Decimal>,
    apps: Option<&[AppIn]>,
    credits: &[CreditIn],
    editing: Option<TransactionId>,
) -> Computed {
    let share = own_share(state, editing);
    let mut rows: Vec<Row> = receivables_of(state, client)
        .into_iter()
        .filter_map(|inv| {
            let balance = inv.balance + share.get(&inv.id).copied().unwrap_or_default();
            (balance > Decimal::ZERO).then(|| {
                let pct = state
                    .receivables
                    .terms
                    .get(&inv.id)
                    .and_then(|(p, d)| Some((*p, inv.date.add_days(*d).ok()?)));
                Row {
                    inv,
                    balance,
                    cash: Decimal::ZERO,
                    discount: Decimal::ZERO,
                    write_off: Decimal::ZERO,
                    credit: Decimal::ZERO,
                    available: Decimal::ZERO,
                    suggest: false,
                    pct,
                }
            })
        })
        .collect();
    let mut problems = Vec::new();
    let (amount, source) = match apps {
        None => {
            let a = amount.unwrap_or_default();
            let mut left = a;
            for r in &mut rows {
                let c = left.min(r.balance).max(Decimal::ZERO);
                r.cash = c;
                left -= c;
            }
            (a, AmountSource::Typed)
        }
        Some(apps) => {
            for a in apps {
                match rows.iter_mut().find(|r| r.inv.id == a.invoice) {
                    Some(r) => {
                        r.discount = a.discount;
                        r.write_off = a.write_off;
                        if let Some(c) = a.cash {
                            r.cash = c;
                        }
                    }
                    None => problems.push(problem("INVOICE_NOT_FOR_CUSTOMER", Some(a.invoice))),
                }
            }
            let typed: Decimal = apps.iter().filter_map(|a| a.cash).sum();
            let fills: Vec<TransactionId> = rows
                .iter()
                .filter(|r| {
                    apps.iter()
                        .any(|a| a.invoice == r.inv.id && a.cash.is_none())
                })
                .map(|r| r.inv.id)
                .collect();
            if let Some(a) = amount {
                {
                    let mut left = a - typed;
                    for id in fills {
                        if let Some(r) = rows.iter_mut().find(|r| r.inv.id == id) {
                            let room = (r.balance - r.discount - r.write_off).max(Decimal::ZERO);
                            let c = left.min(room).max(Decimal::ZERO);
                            r.cash = c;
                            left -= c;
                        }
                    }
                    (a, AmountSource::Typed)
                }
            } else {
                {
                    let mut sum = typed;
                    for id in fills {
                        if let Some(r) = rows.iter_mut().find(|r| r.inv.id == id) {
                            let c = (r.balance - r.discount - r.write_off).max(Decimal::ZERO);
                            r.cash = c;
                            sum += c;
                        }
                    }
                    (sum, AmountSource::FromApplications)
                }
            }
        }
    };
    // Credits and deposits.
    let mut used: BTreeMap<TransactionId, Decimal> = BTreeMap::new();
    let mut credit_uses = Vec::new();
    for c in credits {
        let Some(p) = state.receivables.pays.get(&c.source) else {
            problems.push(problem("CREDIT_NOT_AVAILABLE", Some(c.invoice)));
            continue;
        };
        if p.customer != client || bounced(p) {
            problems.push(problem(
                if bounced(p) {
                    "PAYMENT_BOUNCED"
                } else {
                    "CREDIT_NOT_AVAILABLE"
                },
                Some(c.invoice),
            ));
            continue;
        }
        let left = if p.hold {
            deposit_remaining(&state.receivables, c.source)
        } else {
            credit_remaining(p)
        } + used_by_edit(state, c.source, editing)
            - used.get(&c.source).copied().unwrap_or_default();
        let Some(r) = rows.iter_mut().find(|r| r.inv.id == c.invoice) else {
            problems.push(problem("INVOICE_NOT_FOR_CUSTOMER", Some(c.invoice)));
            continue;
        };
        let room = (r.balance - r.settled()).max(Decimal::ZERO);
        let x = c
            .amount
            .unwrap_or_else(|| left.min(room).max(Decimal::ZERO));
        if x > left {
            problems.push(problem(
                if p.hold {
                    "DEPOSIT_NOT_AVAILABLE"
                } else {
                    "CREDIT_NOT_AVAILABLE"
                },
                Some(c.invoice),
            ));
        }
        r.credit += x;
        *used.entry(c.source).or_insert(Decimal::ZERO) += x;
        credit_uses.push((c.source, c.invoice, x));
    }
    for r in &mut rows {
        if r.settled() > r.balance {
            problems.push(problem("OVERAPPLIED", Some(r.inv.id)));
        }
        if let Some((pct, until)) = r.pct
            && date <= until
            && !state
                .receivables
                .discounted
                .get(&r.inv.id)
                .copied()
                .unwrap_or(false)
        {
            r.available = round(r.inv.total * pct / Decimal::from(100), 2).min(r.balance);
            r.suggest = r.available > Decimal::ZERO
                && r.discount.is_zero()
                && r.cash == r.balance - r.available;
        }
    }
    let cash: Decimal = rows.iter().map(|r| r.cash).sum();
    if cash > amount {
        problems.push(problem("APPLICATIONS_EXCEED_AMOUNT", None));
    }
    Computed {
        amount,
        source,
        rows,
        credit_uses,
        problems,
    }
}

fn cents(m: Option<Money>) -> Option<Decimal> {
    m.map(dec)
}

/// `POST /payments/preview` as the demo engine answers it.
///
/// # Errors
/// `Local(…)` for what the engine's 400 `VALIDATION_ERROR` refuses (a
/// fraction of a cent, a negative amount); `NotFound` for an unknown
/// client.
#[allow(clippy::too_many_lines)]
pub fn preview(
    state: &mut DemoState,
    r: &PaymentPreviewRequest,
) -> Result<PaymentPreview, ApiError> {
    seed(state);
    cl_api::check_preview(r, &super::invoices::all_caps_with_payments())?;
    if name_of(state, r.customer_id).is_none() {
        return Err(ApiError::NotFound { code: None });
    }
    let apps: Option<Vec<AppIn>> = r.applications.as_ref().map(|v| {
        v.iter()
            .map(|a| AppIn {
                invoice: a.invoice_id,
                cash: cents(a.amount),
                discount: cents(a.discount_amount).unwrap_or_default(),
                write_off: cents(a.write_off_amount).unwrap_or_default(),
            })
            .collect()
    });
    let credits: Vec<CreditIn> = r
        .credit_applications
        .iter()
        .map(|c| CreditIn {
            source: c.source_payment_id,
            invoice: c.invoice_id,
            amount: cents(c.amount),
        })
        .collect();
    let c = compute(
        state,
        r.customer_id,
        r.date,
        cents(r.amount),
        apps.as_deref(),
        &credits,
        r.editing_payment_id,
    );
    let mut invoices = Vec::new();
    let mut shortfalls = Vec::new();
    let threshold = state.receivables.settings.write_off_threshold;
    for row in &c.rows {
        let remaining = row.balance - row.settled();
        invoices.push(PreviewInvoice {
            invoice_id: row.inv.id,
            invoice_number: row.inv.number.clone(),
            txn_date: row.inv.date,
            due_date: row.inv.due,
            total: money(row.inv.total)?,
            balance_due: money(row.balance)?,
            balance_due_exact: token(row.balance)?,
            payment: money(row.cash)?,
            remaining: money(remaining.max(Decimal::ZERO))?,
            discount: money(row.discount)?,
            write_off: money(row.write_off)?,
            credit: money(row.credit)?,
            discount_percent: row.pct.map(|(p, _)| p.normalize().to_string()),
            discount_date: row.pct.map(|(_, d)| d),
            discount_available: money(row.available)?,
            suggest_discount: row.suggest,
        });
        if row.settled() > Decimal::ZERO && remaining > Decimal::ZERO {
            let amount = money(remaining)?;
            shortfalls.push(Shortfall {
                invoice_id: row.inv.id,
                amount,
                within_write_off_limit: threshold.is_some_and(|t| amount <= t),
            });
        }
    }
    let applied: Decimal = c.rows.iter().map(|r| r.cash).sum();
    let discounts: Decimal = c.rows.iter().map(|r| r.discount).sum();
    let write_offs: Decimal = c.rows.iter().map(|r| r.write_off).sum();
    let credit_total: Decimal = c.credit_uses.iter().map(|(_, _, x)| *x).sum();
    let draws: Decimal = c
        .credit_uses
        .iter()
        .filter(|(s, _, _)| state.receivables.pays.get(s).is_some_and(|p| p.hold))
        .map(|(_, _, x)| *x)
        .sum();
    let unapplied = c.amount - applied;
    let mut before = contact_balance(state, r.customer_id);
    if let Some(e) = r.editing_payment_id
        && let Some(p) = state.receivables.pays.get(&e)
        && !p.hold
        && !p.void
    {
        let own: Decimal = p
            .apps
            .iter()
            .filter(|a| a.live())
            .map(|a| a.discount + a.write_off)
            .sum();
        before += p.amount + own - credit_remaining(p);
        before -= p.amount
            - p.apps
                .iter()
                .filter(|a| a.live())
                .map(|a| a.cash)
                .sum::<Decimal>();
    }
    let after = if r.hold_as_deposit {
        before
    } else {
        before - c.amount - discounts - write_offs - draws
    };
    let mut problems = c.problems;
    if c.amount.is_zero()
        && discounts.is_zero()
        && write_offs.is_zero()
        && !c.credit_uses.is_empty()
    {
        // As the engine: credits alone are not a receipt (Apply them).
        problems.push(problem("AMOUNT_REQUIRED", None));
    }
    let offered = applied.is_zero()
        && c.credit_uses.is_empty()
        && discounts.is_zero()
        && write_offs.is_zero()
        && c.amount > Decimal::ZERO;
    let pending = super::deposits::pending(state);
    let clearing = clearing(state);
    let adds = r.editing_payment_id.is_none()
        && c.amount > Decimal::ZERO
        && r.deposit_to.is_some()
        && r.deposit_to == clearing;
    let mut total = Money::sum(pending.iter().map(|p| p.amount)).unwrap_or(Money::ZERO);
    if adds {
        total = total.checked_add(money(c.amount)?).map_err(|_| invalid())?;
    }
    let count = u32::try_from(pending.len()).unwrap_or(u32::MAX) + u32::from(adds);
    let mut uses = Vec::new();
    for (s, i, x) in &c.credit_uses {
        uses.push(PreviewCreditUse {
            source_payment_id: *s,
            invoice_id: *i,
            amount: money(*x)?,
        });
    }
    Ok(PaymentPreview {
        contact_balance_before: money(before)?,
        contact_balance_after: money(after)?,
        amount: money(c.amount)?,
        amount_source: c.source,
        invoices,
        totals: PaymentTotals {
            amount_due: money(c.rows.iter().map(|r| r.balance).sum())?,
            applied: money(applied)?,
            discounts_and_credits: money(discounts + credit_total)?,
            unapplied: money(unapplied)?,
            write_offs: money(write_offs)?,
        },
        shortfalls,
        overpayment: (unapplied > Decimal::ZERO && !r.hold_as_deposit)
            .then(|| money(unapplied))
            .transpose()?,
        credits: credits_of(state, r.customer_id, r.editing_payment_id),
        deposits_on_hand: deposits_on_hand(state, r.customer_id),
        credit_applications: uses,
        hold_as_deposit_offered: offered,
        pending_deposits: Some(PendingAfter { count, total }),
        average_days_to_pay: average_days(state, r.customer_id),
        problems,
    })
}

// ---- Writes ------------------------------------------------------------------------

fn line(account: AccountId, debit: Decimal, credit: Decimal) -> Result<DemoLine, ApiError> {
    Ok(DemoLine {
        account,
        debit: money(debit)?,
        credit: money(credit)?,
        memo: None,
    })
}

/// Posts balanced lines (zero lines dropped) to the demo ledger.
pub(super) fn post(
    state: &mut DemoState,
    kind: &'static str,
    date: CalendarDate,
    client: ContactId,
    number: &str,
    lines: Vec<(AccountId, Decimal, Decimal)>,
) -> Result<TransactionId, ApiError> {
    let mut out = Vec::new();
    for (a, d, c) in lines {
        if d.is_zero() && c.is_zero() {
            continue;
        }
        out.push(line(a, d, c)?);
    }
    let contact = name_of(state, client);
    let chart = chart(state);
    state
        .ledger
        .save_lines(
            &chart,
            &DemoJournal {
                kind,
                date,
                number: number.to_string(),
                contact,
                memo: String::new(),
                lines: out,
            },
            None,
        )
        .ok_or_else(invalid)
}

pub(super) fn is_deposit_target(state: &mut DemoState, a: AccountId) -> bool {
    let chart = chart(state);
    let Some(acct) = chart.iter().find(|x| x.id == a) else {
        return false;
    };
    acct.fields.account_type == cl_api::AccountType::Asset
        && !matches!(
            acct.system_tag.as_deref(),
            Some("accounts_receivable" | "customer_deposits")
        )
}

pub(super) fn new_id(state: &mut DemoState) -> Result<TransactionId, ApiError> {
    state.receivables.next += 1;
    TransactionId::parse(&format!(
        "d4000000-0000-4000-8000-{:012}",
        state.receivables.next
    ))
    .map_err(|_| invalid())
}

fn account_or(
    given: Option<AccountId>,
    default: Option<AccountId>,
    code: &str,
) -> Result<AccountId, ApiError> {
    given.or(default).ok_or_else(|| refused(code))
}

/// Applies `rows` (already checked) to the invoices and returns the app
/// rows.
fn apply_rows(
    state: &mut DemoState,
    date: CalendarDate,
    rows: &[(TransactionId, Decimal, Decimal, Decimal)],
) -> Result<Vec<App>, ApiError> {
    let mut out = Vec::new();
    for (inv, cash, discount, write_off) in rows {
        let total = *cash + *discount + *write_off;
        if total.is_zero() {
            continue;
        }
        settle(state, *inv, total)?;
        if !discount.is_zero() {
            state.receivables.discounted.insert(*inv, true);
        }
        out.push(App {
            invoice: *inv,
            cash: *cash,
            discount: *discount,
            write_off: *write_off,
            applied: date,
            via: None,
            later: false,
            voided: false,
            reversed: false,
        });
    }
    Ok(out)
}

/// Restores an application onto its invoice.
fn restore(state: &mut DemoState, a: &App) -> Result<(), ApiError> {
    settle(state, a.invoice, -a.settled())?;
    if !a.discount.is_zero() {
        state.receivables.discounted.remove(&a.invoice);
    }
    Ok(())
}

/// `POST /payments/receive` (and the re-apply half of an edit).
///
/// # Errors
/// The engine's refusals (see [`cl_api::PaymentRefusal`]); `ClosedPeriod`
/// and `ClosingPasswordRejected` for a date on or before the closing date.
#[allow(clippy::too_many_lines)]
pub fn receive(
    state: &mut DemoState,
    p: &NewCustomerPayment,
    password: Option<&Secret>,
    key: Option<&str>,
) -> Result<PaymentRecord, ApiError> {
    seed(state);
    if let Some(id) = key.and_then(|k| state.receivables.keys.get(k)).copied() {
        return record_of(state, id);
    }
    let id = receive_inner(state, p, password, None)?;
    if let Some(k) = key {
        state.receivables.keys.insert(k.to_string(), id);
    }
    record_of(state, id)
}

#[allow(clippy::too_many_lines)]
fn receive_inner(
    state: &mut DemoState,
    p: &NewCustomerPayment,
    password: Option<&Secret>,
    editing: Option<TransactionId>,
) -> Result<TransactionId, ApiError> {
    cl_api::check_payment(p, &super::invoices::all_caps_with_payments())?;
    check_closing(closing(state), &[p.date], password)?;
    if !is_deposit_target(state, p.deposit_to) {
        return Err(refused("DEPOSIT_ACCOUNT_INVALID"));
    }
    if p.hold_as_deposit && (!p.applications.is_empty() || !p.credit_applications.is_empty()) {
        return Err(refused("DEPOSIT_WITH_APPLICATIONS"));
    }
    let apps: Vec<AppIn> = p
        .applications
        .iter()
        .map(|a| AppIn {
            invoice: a.invoice_id,
            cash: Some(dec(a.amount)),
            discount: cents(a.discount_amount).unwrap_or_default(),
            write_off: cents(a.write_off_amount).unwrap_or_default(),
        })
        .collect();
    let credits: Vec<CreditIn> = p
        .credit_applications
        .iter()
        .map(|c| CreditIn {
            source: c.source_payment_id,
            invoice: c.invoice_id,
            amount: Some(dec(c.amount)),
        })
        .collect();
    // Tokens: the stored balance the preview read (an edit adds its own
    // share back, as its preview did).
    let share = own_share(state, editing);
    let expected = p
        .applications
        .iter()
        .map(|a| (a.invoice_id, &a.expected_balance_due))
        .chain(
            p.credit_applications
                .iter()
                .map(|c| (c.invoice_id, &c.expected_balance_due)),
        );
    for (inv, t) in expected {
        let r = receivable(state, inv).ok_or_else(|| refused("INVOICE_NOT_FOR_CUSTOMER"))?;
        if r.client != p.customer_id {
            return Err(refused("INVOICE_NOT_FOR_CUSTOMER"));
        }
        let now = r.balance + share.get(&inv).copied().unwrap_or_default();
        if token(now)? != *t {
            return Err(conflict("INVOICE_BALANCE_CHANGED"));
        }
    }
    let c = compute(
        state,
        p.customer_id,
        p.date,
        Some(dec(p.amount)),
        Some(&apps),
        &credits,
        editing,
    );
    if let Some(pr) = c.problems.first() {
        let code = pr.code.as_str();
        return Err(match code {
            "CREDIT_NOT_AVAILABLE" | "DEPOSIT_NOT_AVAILABLE" | "PAYMENT_BOUNCED" => conflict(code),
            _ => refused(code),
        });
    }
    let discounts: Decimal = c.rows.iter().map(|r| r.discount).sum();
    let write_offs: Decimal = c.rows.iter().map(|r| r.write_off).sum();
    let settings = state.receivables.settings.clone();
    let discount_account = (!discounts.is_zero())
        .then(|| {
            account_or(
                p.discount_account_id,
                settings.discount_account,
                "DISCOUNT_ACCOUNT_REQUIRED",
            )
        })
        .transpose()?;
    let write_off_account = (!write_offs.is_zero())
        .then(|| {
            account_or(
                p.write_off_account_id,
                settings.write_off_account,
                "WRITE_OFF_ACCOUNT_REQUIRED",
            )
        })
        .transpose()?;
    // As the engine: a payment of 0 is allowed only when it writes off or
    // discounts; credits alone go through Apply (400 AMOUNT_REQUIRED).
    if c.amount.is_zero() && discounts.is_zero() && write_offs.is_zero() {
        return Err(refused("AMOUNT_REQUIRED"));
    }
    let ar = super::account_id(AR);
    let amount = c.amount;
    let lines = if p.hold_as_deposit {
        let cd = customer_deposits(state);
        vec![
            (p.deposit_to, amount, Decimal::ZERO),
            (cd, Decimal::ZERO, amount),
        ]
    } else {
        let mut l = vec![(p.deposit_to, amount, Decimal::ZERO)];
        if let Some(a) = discount_account {
            l.push((a, discounts, Decimal::ZERO));
        }
        if let Some(a) = write_off_account {
            l.push((a, write_offs, Decimal::ZERO));
        }
        l.push((ar, Decimal::ZERO, amount + discounts + write_offs));
        l
    };
    let number = p.ref_no.clone().unwrap_or_default();
    let ledger = post(state, "payment", p.date, p.customer_id, &number, lines)?;
    let rows: Vec<(TransactionId, Decimal, Decimal, Decimal)> = c
        .rows
        .iter()
        .map(|r| (r.inv.id, r.cash, r.discount, r.write_off))
        .collect();
    let app_rows = apply_rows(state, p.date, &rows)?;
    let id = match editing {
        Some(e) => e,
        None => new_id(state)?,
    };
    let version = state
        .receivables
        .pays
        .get(&id)
        .map_or(0, |old| old.version + 1);
    state.receivables.pays.insert(
        id,
        Pay {
            customer: p.customer_id,
            date: p.date,
            amount,
            deposit_to: p.deposit_to,
            method: p.payment_method.clone(),
            ref_no: p.ref_no.clone(),
            memo: p.memo.clone(),
            version,
            void: false,
            hold: p.hold_as_deposit,
            from_deposit: None,
            inside: None,
            apps: app_rows,
            ledger: Some(ledger),
            discount_account,
            write_off_account,
            bounce: None,
            refunded: Decimal::ZERO,
        },
    );
    // Credits and deposits used inside this receipt.
    for (source, inv, x) in c.credit_uses {
        use_credit(state, source, inv, x, p.date, Some(id))?;
    }
    state.receivables.writes += 1;
    Ok(id)
}

/// Applies `x` of a credit or held deposit to an invoice: a credit row on
/// the source (no GL), or a draw (Dr Customer Deposits, Cr A/R).
fn use_credit(
    state: &mut DemoState,
    source: TransactionId,
    invoice: TransactionId,
    x: Decimal,
    date: CalendarDate,
    inside: Option<TransactionId>,
) -> Result<(), ApiError> {
    if x.is_zero() {
        return Ok(());
    }
    let hold = state
        .receivables
        .pays
        .get(&source)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    settle(state, invoice, x)?;
    let row = App {
        invoice,
        cash: x,
        discount: Decimal::ZERO,
        write_off: Decimal::ZERO,
        applied: date,
        via: inside,
        later: true,
        voided: false,
        reversed: false,
    };
    if hold.hold {
        let cd = customer_deposits(state);
        let ar = super::account_id(AR);
        let ledger = post(
            state,
            "payment",
            date,
            hold.customer,
            "",
            vec![(cd, x, Decimal::ZERO), (ar, Decimal::ZERO, x)],
        )?;
        let id = new_id(state)?;
        state.receivables.pays.insert(
            id,
            Pay {
                customer: hold.customer,
                date,
                amount: x,
                deposit_to: cd,
                method: None,
                ref_no: hold.ref_no.clone(),
                memo: None,
                version: 0,
                void: false,
                hold: false,
                from_deposit: Some(source),
                inside,
                apps: vec![App {
                    via: None,
                    later: false,
                    ..row
                }],
                ledger: Some(ledger),
                discount_account: None,
                write_off_account: None,
                bounce: None,
                refunded: Decimal::ZERO,
            },
        );
    } else if let Some(p) = state.receivables.pays.get_mut(&source) {
        p.apps.push(row);
        p.version += 1;
    }
    if let Some(p) = state.receivables.pays.get_mut(&source) {
        p.version += 1;
    }
    Ok(())
}

fn check_version(p: &Pay, expected: Option<Timestamp>) -> Result<(), ApiError> {
    match expected {
        Some(t) if t != version_of(p.version) => Err(stale()),
        _ => Ok(()),
    }
}

/// Voids every live row of `id` and the rows and draws made inside it,
/// restoring the invoices.
fn take_back(state: &mut DemoState, id: TransactionId) -> Result<(), ApiError> {
    let mut restores = Vec::new();
    let mut draws = Vec::new();
    for (pid, p) in &mut state.receivables.pays {
        for a in &mut p.apps {
            let mine = (*pid == id && a.via.is_none()) || a.via == Some(id);
            if mine && a.live() {
                a.voided = true;
                restores.push(a.clone());
            }
        }
        if p.inside == Some(id) && !p.void {
            p.void = true;
            draws.push(p.ledger);
        }
    }
    for a in &restores {
        restore(state, a)?;
    }
    for l in draws.into_iter().flatten() {
        state.ledger.void(l)?;
    }
    Ok(())
}

/// `POST /payments/:id/void`: the invoices reopen; refused while deposited,
/// bounced, refunded or (a held deposit) applied.
///
/// # Errors
/// `Conflict` (`STALE`, `TXN_DEPOSITED`, `PAYMENT_BOUNCED`,
/// `PAYMENT_REFUNDED`, `DEPOSIT_APPLIED`); closing on the payment date.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    expected: Option<Timestamp>,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    seed(state);
    let p = state
        .receivables
        .pays
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    check_version(&p, expected)?;
    if p.void {
        return Err(refused("ALREADY_VOID"));
    }
    if bounced(&p)
        || p.from_deposit
            .and_then(|d| state.receivables.pays.get(&d))
            .is_some_and(bounced)
    {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    if deposit_of(state, &p).is_some() {
        return Err(conflict("TXN_DEPOSITED"));
    }
    if !p.refunded.is_zero() {
        return Err(conflict("PAYMENT_REFUNDED"));
    }
    if p.hold && deposit_remaining(&state.receivables, id) != p.amount {
        return Err(conflict("DEPOSIT_APPLIED"));
    }
    check_closing(closing(state), &[p.date], password)?;
    take_back(state, id)?;
    if let Some(l) = p.ledger {
        state.ledger.void(l)?;
    }
    if let Some(p) = state.receivables.pays.get_mut(&id) {
        p.void = true;
        p.version += 1;
    }
    state.receivables.writes += 1;
    Ok(())
}

/// `PUT /payments/:id` (Q-P21o): a deposited payment allows memo and
/// reference only; a bounced one is read-only.
///
/// # Errors
/// As [`receive`] and [`void`].
pub fn update(
    state: &mut DemoState,
    id: TransactionId,
    p: &NewCustomerPayment,
    expected: Option<Timestamp>,
    password: Option<&Secret>,
) -> Result<PaymentRecord, ApiError> {
    seed(state);
    let old = state
        .receivables
        .pays
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    check_version(&old, expected)?;
    if bounced(&old) {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    if old.void {
        return Err(refused("ALREADY_VOID"));
    }
    if old.amount.is_zero() {
        return Err(refused("NO_MONEY_RECEIVED"));
    }
    let same_money = dec(p.amount) == old.amount
        && p.deposit_to == old.deposit_to
        && p.customer_id == old.customer
        && p.date == old.date
        && p.applications.len() == old.apps.iter().filter(|a| a.live()).count()
        && p.applications.iter().all(|n| {
            old.apps.iter().any(|a| {
                a.live()
                    && a.invoice == n.invoice_id
                    && a.cash == dec(n.amount)
                    && a.discount == cents(n.discount_amount).unwrap_or_default()
                    && a.write_off == cents(n.write_off_amount).unwrap_or_default()
            })
        })
        && same_credits(state, id, p)
        && p.hold_as_deposit == old.hold;
    if same_money {
        check_closing(closing(state), &[old.date], password)?;
        if let Some(x) = state.receivables.pays.get_mut(&id) {
            x.memo.clone_from(&p.memo);
            x.ref_no.clone_from(&p.ref_no);
            x.method.clone_from(&p.payment_method);
            x.version += 1;
        }
        return record_of(state, id);
    }
    if deposit_of(state, &old).is_some() {
        return Err(conflict("TXN_DEPOSITED"));
    }
    check_closing(closing(state), &[old.date, p.date], password)?;
    // Validate against the balances as the edit's preview saw them, then
    // take the old rows back and post again under the same id.
    let snapshot = state.clone();
    take_back(state, id)?;
    if let Some(l) = old.ledger {
        state.ledger.void(l)?;
    }
    match receive_inner(state, p, password, None) {
        Ok(new) => {
            if let Some(mut pay) = state.receivables.pays.remove(&new) {
                pay.version = old.version + 1;
                state.receivables.pays.insert(id, pay);
            }
            for x in state.receivables.pays.values_mut() {
                if x.inside == Some(new) {
                    x.inside = Some(id);
                }
                for a in &mut x.apps {
                    if a.via == Some(new) {
                        a.via = Some(id);
                    }
                }
            }
            record_of(state, id)
        }
        Err(e) => {
            *state = snapshot;
            Err(e)
        }
    }
}

/// True when an edit sends back exactly the credits and draws the receipt
/// used (a memo edit of a payment that used credits stays a memo edit).
fn same_credits(state: &DemoState, id: TransactionId, p: &NewCustomerPayment) -> bool {
    let mut old: Vec<(TransactionId, TransactionId, Decimal)> = used_inside(state, id)
        .into_iter()
        .map(|(s, x, i)| (s, i, x))
        .collect();
    let mut new: Vec<(TransactionId, TransactionId, Decimal)> = p
        .credit_applications
        .iter()
        .map(|c| (c.source_payment_id, c.invoice_id, dec(c.amount)))
        .collect();
    old.sort();
    new.sort();
    old == new
}

/// `POST /payments/unapply`: one invoice off a payment.
///
/// # Errors
/// `NotFound`; `Conflict` (`STALE`, `PAYMENT_BOUNCED`); closing.
pub fn unapply(
    state: &mut DemoState,
    payment: TransactionId,
    invoice: TransactionId,
    expected: Option<Timestamp>,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let p = state
        .receivables
        .pays
        .get(&payment)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    check_version(&p, expected)?;
    if bounced(&p) {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    check_closing(closing(state), &[p.date], password)?;
    let mut back = Vec::new();
    if let Some(x) = state.receivables.pays.get_mut(&payment) {
        for a in x
            .apps
            .iter_mut()
            .filter(|a| a.live() && a.invoice == invoice)
        {
            a.voided = true;
            back.push(a.clone());
        }
        x.version += 1;
    }
    if back.is_empty() {
        return Err(ApiError::NotFound { code: None });
    }
    for a in &back {
        restore(state, a)?;
    }
    state.receivables.writes += 1;
    Ok(())
}

/// `POST /payments/:id/apply`: a credit (no GL) or a held deposit (a draw).
///
/// # Errors
/// `Conflict` (`CREDIT_NOT_AVAILABLE`, `DEPOSIT_NOT_AVAILABLE`,
/// `PAYMENT_BOUNCED`, `INVOICE_BALANCE_CHANGED`); `OVERAPPLIED`; closing.
pub fn apply(
    state: &mut DemoState,
    source: TransactionId,
    r: &ApplyRequest,
    password: Option<&Secret>,
) -> Result<ApplyResult, ApiError> {
    seed(state);
    let p = state
        .receivables
        .pays
        .get(&source)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    check_version(&p, r.expected_updated_at)?;
    if bounced(&p) {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    check_closing(closing(state), &[r.date], password)?;
    let mut left = if p.hold {
        deposit_remaining(&state.receivables, source)
    } else {
        credit_remaining(&p)
    };
    let not_available = if p.hold {
        "DEPOSIT_NOT_AVAILABLE"
    } else {
        "CREDIT_NOT_AVAILABLE"
    };
    let mut plan = Vec::new();
    for a in &r.applications {
        let inv =
            receivable(state, a.invoice_id).ok_or_else(|| refused("INVOICE_NOT_FOR_CUSTOMER"))?;
        if inv.client != p.customer {
            return Err(refused("INVOICE_NOT_FOR_CUSTOMER"));
        }
        if let Some(t) = &a.expected_balance_due
            && token(inv.balance)? != *t
        {
            return Err(conflict("INVOICE_BALANCE_CHANGED"));
        }
        let x = a.amount.map_or_else(|| left.min(inv.balance), dec);
        if x > left {
            return Err(conflict(not_available));
        }
        if x > inv.balance {
            return Err(refused("OVERAPPLIED"));
        }
        left -= x;
        plan.push((a.invoice_id, x));
    }
    let mut applied = Vec::new();
    for (inv, x) in plan {
        use_credit(state, source, inv, x, r.date, None)?;
        applied.push(AppliedAmount {
            invoice_id: inv,
            amount: money(x)?,
        });
    }
    state.receivables.writes += 1;
    Ok(ApplyResult {
        applied,
        remaining: money(left)?,
    })
}

fn bank_of_deposit(state: &mut DemoState, deposit: TransactionId) -> Option<AccountId> {
    let chart = chart(state);
    let t = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == deposit)?;
    let bank = t.lines.iter().find(|l| l.debit.is_positive())?;
    chart
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(bank.account.as_str()))
        .map(|a| a.id)
}

/// `POST /payments/:id/bounce`: one dated "Bounced payment" entry against
/// the account holding the money; every live application restored and
/// marked reversed; the bank fee in the same step.
///
/// # Errors
/// `Conflict` (`PAYMENT_BOUNCED`, `PAYMENT_REFUNDED`, `STALE`);
/// `BOUNCE_DATE_INVALID`, `FEE_ACCOUNT_REQUIRED`, `FEE_BANK_REQUIRED`;
/// closing on the bounce date only.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub fn bounce(
    state: &mut DemoState,
    id: TransactionId,
    r: &BounceRequest,
    password: Option<&Secret>,
    key: Option<&str>,
) -> Result<BounceResult, ApiError> {
    seed(state);
    if let Some(done) = key.and_then(|k| state.receivables.bounce_keys.get(k)) {
        return Ok(done.clone());
    }
    let p = state
        .receivables
        .pays
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    if bounced(&p) {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    check_version(&p, Some(r.expected_updated_at))?;
    if p.void || p.from_deposit.is_some() {
        return Err(refused("VALIDATION_ERROR"));
    }
    if p.amount.is_zero() {
        return Err(refused("NO_MONEY_RECEIVED"));
    }
    if !p.refunded.is_zero() {
        return Err(conflict("PAYMENT_REFUNDED"));
    }
    let deposit = deposit_of(state, &p);
    if r.date < p.date || deposit.is_some_and(|(_, d)| r.date < d) {
        return Err(refused("BOUNCE_DATE_INVALID"));
    }
    check_closing(closing(state), &[r.date], password)?;
    let clearing = clearing(state);
    let (held, kind) = match deposit.and_then(|(d, _)| bank_of_deposit(state, d)) {
        Some(bank) => (bank, HeldKind::Bank),
        None if Some(p.deposit_to) != clearing => (p.deposit_to, HeldKind::Bank),
        None => (p.deposit_to, HeldKind::PaymentsClearing),
    };
    let fee = r.bank_fee_amount.map(dec).filter(|f| *f > Decimal::ZERO);
    let fee_account = fee
        .map(|_| {
            account_or(
                r.bank_fee_account_id,
                state.receivables.settings.nsf_bank_fee_account,
                "FEE_ACCOUNT_REQUIRED",
            )
        })
        .transpose()?;
    let fee_bank = match (fee, kind) {
        (Some(_), HeldKind::PaymentsClearing) => Some(
            r.bank_fee_bank_account_id
                .ok_or_else(|| refused("FEE_BANK_REQUIRED"))?,
        ),
        (Some(_), _) => Some(held),
        (None, _) => None,
    };
    // The fee can't go to the bank it comes out of.
    if fee_account.is_some() && fee_account == fee_bank {
        return Err(refused("FEE_ACCOUNT_INVALID"));
    }
    let ar = super::account_id(AR);
    // What it reverses: its own live rows (including credits used later),
    // or, for a held deposit, its draws' rows.
    let mut reversed: Vec<App> = Vec::new();
    let mut lines = vec![(held, Decimal::ZERO, p.amount)];
    if p.hold {
        let mut drawn = Decimal::ZERO;
        for d in state.receivables.pays.values_mut() {
            if d.from_deposit == Some(id) && !d.void {
                for a in d.apps.iter_mut().filter(|a| a.live()) {
                    a.reversed = true;
                    drawn += a.cash;
                    reversed.push(a.clone());
                }
            }
        }
        let cd = customer_deposits(state);
        lines.push((ar, drawn, Decimal::ZERO));
        lines.push((cd, p.amount - drawn, Decimal::ZERO));
    } else {
        let mut discounts = Decimal::ZERO;
        let mut write_offs = Decimal::ZERO;
        if let Some(x) = state.receivables.pays.get_mut(&id) {
            for a in x.apps.iter_mut().filter(|a| a.live()) {
                a.reversed = true;
                discounts += a.discount;
                write_offs += a.write_off;
                reversed.push(a.clone());
            }
        }
        lines.push((ar, p.amount + discounts + write_offs, Decimal::ZERO));
        if let Some(a) = p.discount_account {
            lines.push((a, Decimal::ZERO, discounts));
        }
        if let Some(a) = p.write_off_account {
            lines.push((a, Decimal::ZERO, write_offs));
        }
    }
    let ret = post(state, "returned_payment", r.date, p.customer, "", lines)?;
    let fee_id = match (fee, fee_account, fee_bank) {
        (Some(f), Some(a), Some(b)) => Some(post(
            state,
            "expense",
            r.date,
            p.customer,
            "",
            vec![(a, f, Decimal::ZERO), (b, Decimal::ZERO, f)],
        )?),
        _ => None,
    };
    let mut reopened: Vec<ReopenedInvoice> = Vec::new();
    for a in &reversed {
        restore(state, a)?;
        if !reopened.iter().any(|x| x.invoice_id == a.invoice) {
            reopened.push(ReopenedInvoice {
                invoice_id: a.invoice,
                balance_due: Money::ZERO,
            });
        }
    }
    for x in &mut reopened {
        x.balance_due = receivable(state, x.invoice_id)
            .map(|i| money(i.balance))
            .transpose()?
            .unwrap_or(Money::ZERO);
    }
    if let Some(x) = state.receivables.pays.get_mut(&id) {
        x.bounce = Some((ret, r.date));
        x.version += 1;
    }
    let out = BounceResult {
        return_id: ret,
        fee_id,
        held_in: HeldIn {
            account_id: held,
            kind,
        },
        reopened,
    };
    if let Some(k) = key {
        state
            .receivables
            .bounce_keys
            .insert(k.to_string(), out.clone());
    }
    state.receivables.writes += 1;
    Ok(out)
}

/// A refund of a credit or a held deposit (Q-P21m): Dr A/R (credit) or Dr
/// Customer Deposits (deposit), Cr the bank.
///
/// # Errors
/// `Conflict` (`CREDIT_NOT_AVAILABLE`, `PAYMENT_BOUNCED`); closing.
pub fn refund(
    state: &mut DemoState,
    r: &NewCustomerRefund,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    seed(state);
    let p = state
        .receivables
        .pays
        .get(&r.source_payment)
        .ok_or(ApiError::NotFound { code: None })?
        .clone();
    if p.customer != r.customer_id {
        return Err(refused("CREDIT_NOT_FOR_CUSTOMER"));
    }
    if bounced(&p) {
        return Err(conflict("PAYMENT_BOUNCED"));
    }
    let left = if p.hold {
        deposit_remaining(&state.receivables, r.source_payment)
    } else {
        credit_remaining(&p)
    };
    let x = dec(r.amount);
    if x <= Decimal::ZERO || x > left {
        return Err(conflict("CREDIT_NOT_AVAILABLE"));
    }
    check_closing(closing(state), &[r.date], password)?;
    let debit = if p.hold {
        customer_deposits(state)
    } else {
        super::account_id(AR)
    };
    let number = r.ref_no.clone().unwrap_or_default();
    let id = post(
        state,
        "customer_refund",
        r.date,
        p.customer,
        &number,
        vec![
            (debit, x, Decimal::ZERO),
            (r.from_account, Decimal::ZERO, x),
        ],
    )?;
    if let Some(x0) = state.receivables.pays.get_mut(&r.source_payment) {
        x0.refunded += x;
        x0.version += 1;
    }
    state.receivables.writes += 1;
    Ok(id)
}

// ---- P22 read-only views (company/demo/credit_memos.rs, statements.rs,
// finance_charges.rs) -----------------------------------------------------

/// One application of a payment, as the P22 open items read it.
#[derive(Clone, Copy, Debug)]
pub(super) struct PayAppView {
    pub invoice: TransactionId,
    /// Cash + discount + write-off.
    pub settled: Decimal,
    /// Discount + write-off.
    pub adjustments: Decimal,
    pub applied: CalendarDate,
    pub later: bool,
    pub voided: bool,
    pub reversed: bool,
}

/// One customer payment, as the P22 open items read it.
#[derive(Clone, Debug)]
pub(super) struct PayView {
    pub id: TransactionId,
    pub customer: ContactId,
    pub date: CalendarDate,
    pub amount: Decimal,
    pub hold: bool,
    pub void: bool,
    pub bounce: Option<CalendarDate>,
    pub refunded: Decimal,
    pub credit_remaining: Decimal,
    pub ref_no: Option<String>,
    pub apps: Vec<PayAppView>,
}

/// Every customer payment the demo engine keeps.
pub(super) fn pay_views(state: &DemoState) -> Vec<PayView> {
    state
        .receivables
        .pays
        .iter()
        .map(|(id, p)| PayView {
            id: *id,
            customer: p.customer,
            date: p.date,
            amount: p.amount,
            hold: p.hold,
            void: p.void,
            bounce: p.bounce.map(|(_, d)| d),
            refunded: p.refunded,
            credit_remaining: credit_remaining(p),
            ref_no: p.ref_no.clone(),
            apps: p
                .apps
                .iter()
                .map(|a| PayAppView {
                    invoice: a.invoice,
                    settled: a.settled(),
                    adjustments: a.discount + a.write_off,
                    applied: a.applied,
                    later: a.later,
                    voided: a.voided,
                    reversed: a.reversed,
                })
                .collect(),
        })
        .collect()
}

/// What is left of a held deposit (P22 refunds).
pub(super) fn held_remaining(state: &DemoState, id: TransactionId) -> Option<Decimal> {
    let p = state.receivables.pays.get(&id)?;
    p.hold.then(|| deposit_remaining(&state.receivables, id))
}

/// Gives back `x` of a refunded credit or deposit (a P22 refund void).
pub(super) fn unrefund(state: &mut DemoState, id: TransactionId, x: Decimal) {
    if let Some(p) = state.receivables.pays.get_mut(&id) {
        p.refunded -= x;
        p.version += 1;
    }
}

/// The receivables settings as `CompanySettings` would carry them.
#[must_use]
pub fn settings(state: &DemoState) -> DemoReceivablesSettings {
    state.receivables.settings.clone()
}

/// The payment that holds a ledger transaction (tests and the deposit
/// side).
#[must_use]
pub fn payment_of_ledger(state: &DemoState, ledger: TransactionId) -> Option<TransactionId> {
    state
        .receivables
        .pays
        .iter()
        .find(|(_, p)| p.ledger == Some(ledger))
        .map(|(id, _)| *id)
}

/// The ledger transaction of a payment (Make Deposit's source id).
#[must_use]
pub fn ledger_of(state: &DemoState, payment: TransactionId) -> Option<TransactionId> {
    state.receivables.pays.get(&payment).and_then(|p| p.ledger)
}

#[cfg(test)]
#[path = "receivables_tests.rs"]
mod tests;
