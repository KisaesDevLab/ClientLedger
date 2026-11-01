//! The demo "engine" for P22's credit memos, sales receipts and refunds
//! (D2): preview (P20's demo invoice compute, so preview = post), create
//! with apply at save (Q-P22b), apply later, unapply, edit, void, open
//! credits, refunds of payment credits, credit memos and held deposits
//! (checks queued to print by default, Q-P22d) and their voids.
//!
//! `--demo` only. The arithmetic is the engine's rule in `rust_decimal`,
//! held to `fixtures/receivables/golden.json` (`credit_memos_tests.rs`).
//! Whole cents. No floats. All data is synthetic.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    ApiError, AppliedAmount, ApplyRequest, ApplyResult, CreditKind, CreditMemoAppliedRow,
    CreditMemoPreview, CreditMemoRecord, CreditMemoRefundRow, CreditOpenInvoice, InvoicePreview,
    NewCreditMemo, NewCustomerRefund, NewInvoice, NewRefund, NewSalesReceipt, OpenCredit,
    PreviewProblem, RefundCheck, RefundMethod, RefundPrintStatus, RefundRecord, RefundSource,
    RefundSourceRef, SalesDocLine, SalesDocStatus, SalesReceiptPreview, SalesReceiptRecord,
    SuggestedApplication,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret, Timestamp};
use rust_decimal::Decimal;

use super::DemoState;
use super::invoices::{receivable, receivables_of, settle};
use super::receivables::{
    chart, closing, conflict, dec, invalid, money, new_id, pay_views, post, refused, stale, tagged,
    token, version_of,
};
use super::register::check_closing;

/// The P22 demo clients: (name, street, city, email). Bluestem holds
/// golden cases 1-5; the other three golden case 6.
pub const P22_CLIENTS: [(&str, &str, &str, Option<&str>); 4] = [
    (
        BLUESTEM,
        "1200 Bluestem Ln",
        "Neosho",
        Some("office@bluestem-vet.example"),
    ),
    (PRAIRIE_FORK, "77 Kennel Rd", "Granby", None),
    (
        LOW_WATER,
        "5 Riverside Dr",
        "Seneca",
        Some("bait@lowwater.example"),
    ),
    (CEDAR_GAP, "300 Ridge Rd", "Diamond", None),
];

/// The credit memo, refund and statement story client.
pub const BLUESTEM: &str = "Bluestem Veterinary Clinic";
/// Finance-charge clients (golden case 6).
pub const PRAIRIE_FORK: &str = "Prairie Fork Kennels";
/// Finance-charge client below the minimum.
pub const LOW_WATER: &str = "Low Water Bait & Tackle";
/// Finance-charge client with an open credit memo.
pub const CEDAR_GAP: &str = "Cedar Gap Outfitters";

const AR: u32 = 1200;
const INCOME: u32 = 4000;

/// The seeded invoices: (name, client, number, date, due, total).
/// (name, client, number, (month, day) date, (month, day) due, total).
type SeedInvoice = (
    &'static str,
    &'static str,
    u32,
    (u32, u32),
    (u32, u32),
    &'static str,
);

const SEED_INVOICES: [SeedInvoice; 6] = [
    ("A", BLUESTEM, 1201, (9, 15), (10, 15), "250.00"),
    ("B", BLUESTEM, 1202, (10, 20), (11, 19), "500.00"),
    ("PF1", PRAIRIE_FORK, 1301, (8, 31), (9, 30), "1000.00"),
    ("PF2", PRAIRIE_FORK, 1302, (9, 15), (10, 15), "50.00"),
    ("LW1", LOW_WATER, 1311, (9, 15), (10, 15), "30.00"),
    ("CG1", CEDAR_GAP, 1321, (8, 31), (9, 30), "600.00"),
];

/// The id of seeded invoice `number` (outside the Create Invoice numbering).
#[must_use]
pub fn seed_invoice(number: u32) -> Option<TransactionId> {
    TransactionId::parse(&format!("d1000000-0000-4000-8000-0000000{number:05}")).ok()
}

#[derive(Clone)]
struct MemoApp {
    invoice: TransactionId,
    amount: Decimal,
    /// `None`: with the memo, at save.
    applied: Option<CalendarDate>,
    voided: bool,
}

#[derive(Clone)]
#[allow(clippy::struct_field_names)] // the memo text is "memo" on the wire
struct Memo {
    customer: ContactId,
    date: CalendarDate,
    number: String,
    lines: Vec<SalesDocLine>,
    subtotal: Decimal,
    tax: Decimal,
    total: Decimal,
    memo: Option<String>,
    message: Option<String>,
    version: i64,
    void: bool,
    apps: Vec<MemoApp>,
    ledger: Option<TransactionId>,
}

#[derive(Clone)]
struct Refund {
    customer: ContactId,
    date: CalendarDate,
    amount: Decimal,
    source: TransactionId,
    kind: CreditKind,
    method: RefundMethod,
    check_number: Option<String>,
    queued: bool,
    ref_no: Option<String>,
    void: bool,
    ledger: TransactionId,
    version: i64,
    /// The bank it is paid from (the check print queue's account).
    bank: AccountId,
}

#[derive(Clone)]
struct Receipt {
    record: SalesReceiptRecord,
    request: NewSalesReceipt,
    version: i64,
    ledger: Option<TransactionId>,
}

/// Credit memos, sales receipts and refunds of the demo engine.
#[derive(Clone, Default)]
pub struct DemoCredits {
    seeded: bool,
    memos: BTreeMap<TransactionId, Memo>,
    refunds: BTreeMap<TransactionId, Refund>,
    receipts: BTreeMap<TransactionId, Receipt>,
    next_memo: u32,
    next_receipt: u32,
    /// Write requests that reached the demo engine (tests).
    pub writes: u32,
}

impl fmt::Debug for DemoCredits {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoCredits")
            .field("memos", &self.memos.len())
            .field("refunds", &self.refunds.len())
            .field("receipts", &self.receipts.len())
            .finish_non_exhaustive()
    }
}

// ---- Seed --------------------------------------------------------------------------

/// A demo client's id by name.
pub fn client_named(state: &mut DemoState, name: &str) -> Option<ContactId> {
    state
        .contacts_mut()
        .iter()
        .find(|c| c.fields.display_name == name)
        .map(|c| c.id)
}

/// Seeds P22's story once: Bluestem's invoices A (1201) and B (1202), the
/// finance-charge clients' overdue invoices and Cedar Gap's open 400.00
/// credit memo. Neosho's 1091/1095 are untouched.
pub fn seed(state: &mut DemoState) {
    if state.credits.seeded {
        return;
    }
    state.credits.seeded = true;
    let day = |(m, d): (u32, u32)| CalendarDate::from_ymd(2026, m, d).ok();
    for (_, client, n, date, due, total) in SEED_INVOICES {
        let (Some(c), Some(id), Some(date), Some(due), Ok(total)) = (
            client_named(state, client),
            seed_invoice(n),
            day(date),
            day(due),
            Money::parse_wire(total),
        ) else {
            continue;
        };
        let _ = super::invoices::seed_receivable(state, id, c, &n.to_string(), date, due, total);
    }
    if let (Some(c), Some(date), Ok(q), Ok(p)) = (
        client_named(state, CEDAR_GAP),
        day((10, 1)),
        cl_core::Quantity::parse_wire("1"),
        Money::parse_wire("400.00"),
    ) {
        let mut l = cl_api::InvoiceLine::account(super::account_id(INCOME), q, p);
        l.is_taxable = Some(false);
        let _ = create_memo(state, &NewCreditMemo::new(c, date, vec![l]), None);
    }
}

// ---- Shared figures ----------------------------------------------------------------

fn as_invoice(contact: ContactId, date: CalendarDate, lines: &[cl_api::InvoiceLine]) -> NewInvoice {
    NewInvoice::new(contact, date, lines.to_vec())
}

fn doc_lines(
    state: &mut DemoState,
    lines: &[cl_api::InvoiceLine],
    p: &InvoicePreview,
) -> Vec<SalesDocLine> {
    let items = state.items_mut().clone();
    lines
        .iter()
        .zip(&p.lines)
        .map(|(l, f)| SalesDocLine {
            account_id: l.account_id.or_else(|| {
                l.item_id
                    .and_then(|i| items.iter().find(|x| x.id == i))
                    .map(|i| i.fields.income_account_id)
            }),
            item_id: l.item_id,
            description: l.description.clone(),
            service_date: l.service_date,
            quantity: f.quantity.or(l.quantity),
            unit_price: f.unit_price.or(l.unit_price),
            discount_percent: f.discount_percent,
            is_taxable: l.is_taxable.unwrap_or(true),
            amount: f.amount,
            tax_amount: f.tax_amount,
            tag_id: l.tag_id,
        })
        .collect()
}

/// Every refund of a source (live and not).
fn refunds_of(state: &DemoState, source: TransactionId) -> impl Iterator<Item = &Refund> {
    state
        .credits
        .refunds
        .values()
        .filter(move |r| r.source == source)
}

fn memo_remaining(state: &DemoState, id: TransactionId, m: &Memo) -> Decimal {
    if m.void {
        return Decimal::ZERO;
    }
    let applied: Decimal = m.apps.iter().filter(|a| !a.voided).map(|a| a.amount).sum();
    let refunded: Decimal = refunds_of(state, id)
        .filter(|r| !r.void)
        .map(|r| r.amount)
        .sum();
    m.total - applied - refunded
}

/// Credit memo credit a client has left (the client balance subtracts it).
#[must_use]
pub fn memo_credit(state: &DemoState, client: ContactId) -> Decimal {
    state
        .credits
        .memos
        .iter()
        .filter(|(_, m)| m.customer == client)
        .map(|(id, m)| memo_remaining(state, *id, m))
        .sum()
}

/// One credit memo as the P22 open items read it.
#[derive(Clone, Debug)]
pub(super) struct MemoView {
    pub id: TransactionId,
    pub customer: ContactId,
    pub date: CalendarDate,
    pub number: String,
    pub total: Decimal,
    /// (invoice, amount, applied date or `None` = with the memo).
    pub apps: Vec<(TransactionId, Decimal, Option<CalendarDate>)>,
    /// Live refunds: (date, amount).
    pub refunds: Vec<(CalendarDate, Decimal)>,
}

/// Every live credit memo.
pub(super) fn memo_views(state: &DemoState) -> Vec<MemoView> {
    state
        .credits
        .memos
        .iter()
        .filter(|(_, m)| !m.void)
        .map(|(id, m)| MemoView {
            id: *id,
            customer: m.customer,
            date: m.date,
            number: m.number.clone(),
            total: m.total,
            apps: m
                .apps
                .iter()
                .filter(|a| !a.voided)
                .map(|a| (a.invoice, a.amount, a.applied))
                .collect(),
            refunds: refunds_of(state, *id)
                .filter(|r| !r.void)
                .map(|r| (r.date, r.amount))
                .collect(),
        })
        .collect()
}

/// Live refunds of payments and held deposits: (source, date, amount),
/// dated (P21's inline refunds are not here; they count at the payment
/// date).
pub(super) fn payment_refunds(state: &DemoState) -> Vec<(TransactionId, CalendarDate, Decimal)> {
    state
        .credits
        .refunds
        .values()
        .filter(|r| !r.void && r.kind != CreditKind::CreditMemo)
        .map(|r| (r.source, r.date, r.amount))
        .collect()
}

/// Refund checks on ank waiting in the check print queue (P22 D5, the
/// engine's listChecks with customer_refund): payee the client, the
/// client's billing address.
#[must_use]
pub fn refund_checks(
    state: &mut DemoState,
    bank: AccountId,
    is_printed: &dyn Fn(TransactionId) -> bool,
) -> Vec<cl_api::QueuedCheck> {
    let rows: Vec<(TransactionId, Refund)> = state
        .credits
        .refunds
        .iter()
        .filter(|(id, r)| r.queued && !r.void && r.bank == bank && !is_printed(**id))
        .map(|(id, r)| (*id, r.clone()))
        .collect();
    let contacts = state.contacts_mut().clone();
    rows.into_iter()
        .filter_map(|(id, r)| {
            let c = contacts.iter().find(|c| c.id == r.customer)?;
            let (address, _) = crate::screens::invoice::draw::addresses(&c.fields);
            Some(cl_api::QueuedCheck {
                id,
                date: r.date,
                amount: money(r.amount).ok()?,
                payee: c.fields.display_name.clone(),
                memo: String::new(),
                kind: cl_api::QueuedCheckKind::Check,
                bank_account: Some(bank),
                address,
            })
        })
        .collect()
}

/// Refund checks waiting in the print queue: (refund, client, amount).
#[must_use]
pub fn refund_print_queue(state: &DemoState) -> Vec<(TransactionId, ContactId, Money)> {
    state
        .credits
        .refunds
        .iter()
        .filter(|(_, r)| r.queued && !r.void)
        .filter_map(|(id, r)| Some((*id, r.customer, money(r.amount).ok()?)))
        .collect()
}

fn problem(code: &str) -> PreviewProblem {
    PreviewProblem {
        code: code.to_string(),
        invoice_id: None,
    }
}

fn check_lines(lines: &[cl_api::InvoiceLine]) -> Result<(), ApiError> {
    if lines.is_empty()
        || lines
            .iter()
            .any(|l| l.billable_source_line_id.is_some() || l.markup_percent.is_some())
    {
        return Err(invalid());
    }
    Ok(())
}

// ---- Credit memos ----------------------------------------------------------------

/// `POST /credit-memos/preview`.
///
/// # Errors
/// `Local(InvalidValue)` for what the engine refuses (no lines, billable
/// lines, bad decimals).
pub fn preview_memo(
    state: &mut DemoState,
    m: &NewCreditMemo,
    editing: Option<TransactionId>,
) -> Result<CreditMemoPreview, ApiError> {
    seed(state);
    check_lines(&m.lines)?;
    let mut figures =
        super::invoices::preview(state, &as_invoice(m.contact_id, m.txn_date, &m.lines), None)?;
    let total = dec(figures.total);
    let own = editing
        .and_then(|id| state.credits.memos.get(&id).map(|x| (id, x.clone())))
        .filter(|(_, x)| x.customer == m.contact_id)
        .map_or(Decimal::ZERO, |(id, x)| memo_remaining(state, id, &x));
    let before = super::receivables::contact_balance(state, m.contact_id) + own;
    figures.contact_balance_after = Some(money(before - total)?);
    figures.due_date = None;
    let mut open = Vec::new();
    let mut suggested = Vec::new();
    let mut left = total.max(Decimal::ZERO);
    for r in receivables_of(state, m.contact_id) {
        if r.balance <= Decimal::ZERO {
            continue;
        }
        open.push(CreditOpenInvoice {
            invoice_id: r.id,
            number: Some(r.number.clone()),
            date: r.date,
            due_date: r.due,
            balance_due: money(r.balance)?,
            balance_due_exact: token(r.balance)?,
        });
        let x = left.min(r.balance);
        if x > Decimal::ZERO {
            suggested.push(SuggestedApplication {
                invoice_id: r.id,
                amount: money(x)?,
            });
            left -= x;
        }
    }
    let mut problems = Vec::new();
    if total <= Decimal::ZERO {
        problems.push(problem("CREDIT_MEMO_TOTAL_NOT_POSITIVE"));
    }
    Ok(CreditMemoPreview {
        figures,
        open_invoices: open,
        suggested_applications: suggested,
        remaining_credit: money(left)?,
        problems,
    })
}

fn memo_lines_ledger(
    state: &mut DemoState,
    lines: &[SalesDocLine],
    tax: Decimal,
    total: Decimal,
) -> Vec<(AccountId, Decimal, Decimal)> {
    let tax_acct = tagged(&chart(state), "sales_tax_payable");
    let mut out: Vec<(AccountId, Decimal, Decimal)> = lines
        .iter()
        .map(|l| {
            let a = l.account_id.unwrap_or_else(|| super::account_id(INCOME));
            let x = dec(l.amount);
            if x.is_sign_negative() {
                (a, Decimal::ZERO, -x)
            } else {
                (a, x, Decimal::ZERO)
            }
        })
        .collect();
    if let Some(t) = tax_acct {
        out.push((t, tax, Decimal::ZERO));
    }
    out.push((super::account_id(AR), Decimal::ZERO, total));
    out
}

/// `POST /credit-memos`: posts the memo and applies `applications` in the
/// same step.
///
/// # Errors
/// 400 `CREDIT_MEMO_TOTAL_NOT_POSITIVE`, `OVERAPPLIED`,
/// `INVOICE_NOT_FOR_CUSTOMER`; 409 `INVOICE_BALANCE_CHANGED`,
/// `CREDIT_NOT_AVAILABLE`; closing.
pub fn create_memo(
    state: &mut DemoState,
    m: &NewCreditMemo,
    password: Option<&Secret>,
) -> Result<CreditMemoRecord, ApiError> {
    seed(state);
    check_lines(&m.lines)?;
    check_closing(closing(state), &[m.txn_date], password)?;
    let p = super::invoices::preview(state, &as_invoice(m.contact_id, m.txn_date, &m.lines), None)?;
    let total = dec(p.total);
    if total <= Decimal::ZERO {
        return Err(refused("CREDIT_MEMO_TOTAL_NOT_POSITIVE"));
    }
    let mut left = total;
    let mut plan = Vec::new();
    for a in &m.applications {
        let inv =
            receivable(state, a.invoice_id).ok_or_else(|| refused("INVOICE_NOT_FOR_CUSTOMER"))?;
        if inv.client != m.contact_id {
            return Err(refused("INVOICE_NOT_FOR_CUSTOMER"));
        }
        if token(inv.balance)? != a.expected_balance_due {
            return Err(conflict("INVOICE_BALANCE_CHANGED"));
        }
        let x = dec(a.amount);
        if x > inv.balance {
            return Err(refused("OVERAPPLIED"));
        }
        if x > left {
            return Err(conflict("CREDIT_NOT_AVAILABLE"));
        }
        left -= x;
        plan.push((a.invoice_id, x));
    }
    let lines = doc_lines(state, &m.lines, &p);
    let tax = dec(p.tax_total);
    state.credits.next_memo += 1;
    let number = m
        .txn_number
        .clone()
        .unwrap_or_else(|| format!("CM-{}", 1000 + state.credits.next_memo));
    let posting = memo_lines_ledger(state, &lines, tax, total);
    let ledger = post(
        state,
        "credit_memo",
        m.txn_date,
        m.contact_id,
        &number,
        posting,
    )?;
    let id = new_id(state)?;
    let mut apps = Vec::new();
    for (inv, x) in plan {
        settle(state, inv, x)?;
        apps.push(MemoApp {
            invoice: inv,
            amount: x,
            applied: None,
            voided: false,
        });
    }
    state.credits.memos.insert(
        id,
        Memo {
            customer: m.contact_id,
            date: m.txn_date,
            number,
            lines,
            subtotal: dec(p.subtotal),
            tax,
            total,
            memo: m.memo.clone(),
            message: m.customer_message.clone(),
            version: 1,
            void: false,
            apps,
            ledger: Some(ledger),
        },
    );
    state.credits.writes += 1;
    get_memo(state, id)
}

/// `GET /credit-memos/{id}`.
///
/// # Errors
/// `NotFound`.
pub fn get_memo(state: &DemoState, id: TransactionId) -> Result<CreditMemoRecord, ApiError> {
    let m = state
        .credits
        .memos
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    let number_of = |inv| receivable(state, inv).map(|r| r.number);
    Ok(CreditMemoRecord {
        id,
        number: Some(m.number.clone()),
        date: m.date,
        customer_id: m.customer,
        lines: m.lines.clone(),
        subtotal: money(m.subtotal)?,
        tax_amount: money(m.tax)?,
        total: money(m.total)?,
        memo: m.memo.clone(),
        customer_message: m.message.clone(),
        updated_at: version_of(m.version),
        status: if m.void {
            SalesDocStatus::Void
        } else {
            SalesDocStatus::Posted
        },
        applications: m
            .apps
            .iter()
            .map(|a| {
                Ok(CreditMemoAppliedRow {
                    invoice_id: a.invoice,
                    invoice_number: number_of(a.invoice),
                    amount: money(a.amount)?,
                    applied_date: a.applied,
                    voided: a.voided,
                })
            })
            .collect::<Result<_, ApiError>>()?,
        refunds: refunds_of(state, id)
            .map(|r| {
                Ok(CreditMemoRefundRow {
                    refund_id: r.ledger,
                    date: r.date,
                    amount: money(r.amount)?,
                    method: serde_json::to_value(r.method)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_string)),
                    voided: r.void,
                })
            })
            .collect::<Result<_, ApiError>>()?,
        remaining: money(memo_remaining(state, id, m))?,
    })
}

fn memo_of(state: &DemoState, id: TransactionId) -> Result<Memo, ApiError> {
    state
        .credits
        .memos
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })
}

fn check_version(version: i64, expected: Option<Timestamp>) -> Result<(), ApiError> {
    match expected {
        Some(t) if t != version_of(version) => Err(stale()),
        _ => Ok(()),
    }
}

/// `POST /credit-memos/{id}/apply`.
///
/// # Errors
/// 400 `APPLY_DATE_BEFORE_SOURCE`, `OVERAPPLIED`; 409
/// `CREDIT_NOT_AVAILABLE`, `INVOICE_BALANCE_CHANGED`, `STALE`; closing.
pub fn apply_memo(
    state: &mut DemoState,
    id: TransactionId,
    r: &ApplyRequest,
    password: Option<&Secret>,
) -> Result<ApplyResult, ApiError> {
    seed(state);
    let m = memo_of(state, id)?;
    check_version(m.version, r.expected_updated_at)?;
    if m.void {
        return Err(conflict("CREDIT_NOT_AVAILABLE"));
    }
    if r.date < m.date {
        return Err(refused("APPLY_DATE_BEFORE_SOURCE"));
    }
    check_closing(closing(state), &[r.date], password)?;
    let mut left = memo_remaining(state, id, &m);
    let mut plan = Vec::new();
    for a in &r.applications {
        let inv =
            receivable(state, a.invoice_id).ok_or_else(|| refused("INVOICE_NOT_FOR_CUSTOMER"))?;
        if inv.client != m.customer {
            return Err(refused("INVOICE_NOT_FOR_CUSTOMER"));
        }
        if let Some(t) = &a.expected_balance_due
            && token(inv.balance)? != *t
        {
            return Err(conflict("INVOICE_BALANCE_CHANGED"));
        }
        let x = a.amount.map_or_else(|| left.min(inv.balance), dec);
        if x > left || x <= Decimal::ZERO {
            return Err(conflict("CREDIT_NOT_AVAILABLE"));
        }
        if x > inv.balance {
            return Err(refused("OVERAPPLIED"));
        }
        left -= x;
        plan.push((a.invoice_id, x));
    }
    let mut applied = Vec::new();
    for (inv, x) in plan {
        settle(state, inv, x)?;
        if let Some(mm) = state.credits.memos.get_mut(&id) {
            mm.apps.push(MemoApp {
                invoice: inv,
                amount: x,
                applied: Some(r.date),
                voided: false,
            });
        }
        applied.push(AppliedAmount {
            invoice_id: inv,
            amount: money(x)?,
        });
    }
    bump(state, id);
    Ok(ApplyResult {
        applied,
        remaining: money(left)?,
    })
}

fn bump(state: &mut DemoState, id: TransactionId) {
    if let Some(m) = state.credits.memos.get_mut(&id) {
        m.version += 1;
    }
    state.credits.writes += 1;
}

/// `POST /credit-memos/{id}/unapply`: answers what is left.
///
/// # Errors
/// `NotFound`; 409 `STALE`; closing on the row's date.
pub fn unapply_memo(
    state: &mut DemoState,
    id: TransactionId,
    invoice: TransactionId,
    expected: Option<Timestamp>,
    password: Option<&Secret>,
) -> Result<Money, ApiError> {
    let m = memo_of(state, id)?;
    check_version(m.version, expected)?;
    let (i, a) = m
        .apps
        .iter()
        .enumerate()
        .find(|(_, a)| a.invoice == invoice && !a.voided)
        .ok_or(ApiError::NotFound { code: None })?;
    check_closing(closing(state), &[a.applied.unwrap_or(m.date)], password)?;
    settle(state, invoice, -a.amount)?;
    if let Some(mm) = state.credits.memos.get_mut(&id)
        && let Some(row) = mm.apps.get_mut(i)
    {
        row.voided = true;
    }
    bump(state, id);
    let m = memo_of(state, id)?;
    money(memo_remaining(state, id, &m))
}

/// Voids a credit memo (`POST /transactions/{id}/void`): its applications
/// are taken off and the invoices reopen (Q-P22j).
///
/// # Errors
/// 409 `CREDIT_REFUNDED` with a live refund; closing on the memo date and
/// every live application's date.
pub fn void_memo(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let m = memo_of(state, id)?;
    if m.void {
        return Ok(());
    }
    if refunds_of(state, id).any(|r| !r.void) {
        return Err(conflict("CREDIT_REFUNDED"));
    }
    let mut dates = vec![m.date];
    dates.extend(
        m.apps
            .iter()
            .filter(|a| !a.voided)
            .filter_map(|a| a.applied),
    );
    check_closing(closing(state), &dates, password)?;
    for a in m.apps.iter().filter(|a| !a.voided) {
        settle(state, a.invoice, -a.amount)?;
    }
    if let Some(l) = m.ledger {
        let _ = state.ledger.void(l);
    }
    if let Some(mm) = state.credits.memos.get_mut(&id) {
        mm.void = true;
        for a in &mut mm.apps {
            a.voided = true;
        }
    }
    bump(state, id);
    Ok(())
}

/// `PUT /credit-memos/{id}`.
///
/// # Errors
/// 409 `STALE`, `CREDIT_IN_USE`; 400 `SOURCE_DATE_AFTER_USE`,
/// `CREDIT_MEMO_TOTAL_NOT_POSITIVE`; closing on the old and new dates and
/// every live refund's date.
pub fn update_memo(
    state: &mut DemoState,
    id: TransactionId,
    new: &NewCreditMemo,
    expected: Option<Timestamp>,
    password: Option<&Secret>,
) -> Result<CreditMemoRecord, ApiError> {
    let m = memo_of(state, id)?;
    check_version(m.version, expected)?;
    check_lines(&new.lines)?;
    let refunds: Vec<(CalendarDate, Decimal)> = refunds_of(state, id)
        .filter(|r| !r.void)
        .map(|r| (r.date, r.amount))
        .collect();
    let mut dates = vec![m.date, new.txn_date];
    dates.extend(refunds.iter().map(|r| r.0));
    check_closing(closing(state), &dates, password)?;
    let p = super::invoices::preview(
        state,
        &as_invoice(new.contact_id, new.txn_date, &new.lines),
        None,
    )?;
    let total = dec(p.total);
    if total <= Decimal::ZERO {
        return Err(refused("CREDIT_MEMO_TOTAL_NOT_POSITIVE"));
    }
    let live: Vec<&MemoApp> = m.apps.iter().filter(|a| !a.voided).collect();
    let used: Decimal = live.iter().map(|a| a.amount).sum::<Decimal>()
        + refunds.iter().map(|r| r.1).sum::<Decimal>();
    if total < used || (new.contact_id != m.customer && used > Decimal::ZERO) {
        return Err(conflict("CREDIT_IN_USE"));
    }
    let first_use = live
        .iter()
        .filter_map(|a| a.applied)
        .chain(refunds.iter().map(|r| r.0))
        .min();
    if first_use.is_some_and(|d| new.txn_date > d) {
        return Err(refused("SOURCE_DATE_AFTER_USE"));
    }
    let lines = doc_lines(state, &new.lines, &p);
    let tax = dec(p.tax_total);
    if let Some(l) = m.ledger {
        let _ = state.ledger.void(l);
    }
    let number = new.txn_number.clone().unwrap_or_else(|| m.number.clone());
    let posting = memo_lines_ledger(state, &lines, tax, total);
    let ledger = post(
        state,
        "credit_memo",
        new.txn_date,
        new.contact_id,
        &number,
        posting,
    )?;
    if let Some(mm) = state.credits.memos.get_mut(&id) {
        mm.customer = new.contact_id;
        mm.date = new.txn_date;
        mm.number = number;
        mm.lines = lines;
        mm.subtotal = dec(p.subtotal);
        mm.tax = tax;
        mm.total = total;
        mm.memo.clone_from(&new.memo);
        mm.message.clone_from(&new.customer_message);
        mm.ledger = Some(ledger);
    }
    bump(state, id);
    get_memo(state, id)
}

/// `GET /payments/credits?customerId=`: payment credits and credit memos
/// with something left, oldest first (held deposits stay on
/// `deposits-on-hand`).
pub fn credits(state: &mut DemoState, client: ContactId) -> Vec<OpenCredit> {
    seed(state);
    let mut out: Vec<OpenCredit> = pay_views(state)
        .into_iter()
        .filter(|p| p.customer == client && p.credit_remaining > Decimal::ZERO)
        .filter_map(|p| {
            Some(OpenCredit {
                source_id: p.id,
                kind: CreditKind::Payment,
                date: p.date,
                number: p.ref_no.clone(),
                total: money(p.amount).ok()?,
                remaining: money(p.credit_remaining).ok()?,
                intended_invoice_id: None,
            })
        })
        .collect();
    for (id, m) in &state.credits.memos {
        let left = memo_remaining(state, *id, m);
        if m.customer != client || left <= Decimal::ZERO {
            continue;
        }
        if let (Ok(total), Ok(remaining)) = (money(m.total), money(left)) {
            out.push(OpenCredit {
                source_id: *id,
                kind: CreditKind::CreditMemo,
                date: m.date,
                number: Some(m.number.clone()),
                total,
                remaining,
                intended_invoice_id: None,
            });
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then(a.source_id.cmp(&b.source_id)));
    out
}

// ---- Refunds ---------------------------------------------------------------------

fn bank_type(state: &mut DemoState, a: AccountId) -> bool {
    let chart = chart(state);
    chart.iter().find(|x| x.id == a).is_some_and(|x| {
        x.fields.account_type == cl_api::AccountType::Asset
            && !matches!(
                x.system_tag.as_deref(),
                Some("accounts_receivable" | "customer_deposits" | "payments_clearing")
            )
            && matches!(
                x.fields.detail_type.as_deref(),
                Some("checking" | "savings" | "cash_on_hand" | "cash" | "money_market" | "bank")
            )
    })
}

/// `POST /customer-refunds` with a source (P22): answers the refund id.
///
/// # Errors
/// 400 `REFUND_ACCOUNT_INVALID`, `REFUND_DATE_BEFORE_SOURCE`,
/// `CREDIT_NOT_FOR_CUSTOMER`; 409 `CREDIT_NOT_AVAILABLE`,
/// `DEPOSIT_NOT_AVAILABLE`, `CHECK_NUMBER_IN_USE`; closing.
pub fn create_refund(
    state: &mut DemoState,
    r: &NewRefund,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    seed(state);
    if !bank_type(state, r.from_account) {
        return Err(refused("REFUND_ACCOUNT_INVALID"));
    }
    let x = dec(r.amount);
    if x <= Decimal::ZERO {
        return Err(invalid());
    }
    let (check_number, queued) = match &r.check {
        RefundCheck::NotACheck => (None, false),
        RefundCheck::PrintLater => (None, true),
        RefundCheck::Number(n) => (Some(n.trim().to_string()), false),
    };
    if let Some(n) = &check_number
        && state
            .credits
            .refunds
            .values()
            .any(|o| !o.void && o.check_number.as_deref() == Some(n.as_str()))
    {
        return Err(conflict("CHECK_NUMBER_IN_USE"));
    }
    let source = r.source.id();
    let is_memo = state.credits.memos.contains_key(&source);
    if is_memo != matches!(r.source, RefundSource::CreditMemo(_)) {
        return Err(ApiError::NotFound { code: None });
    }
    let (kind, ledger) = if let Some(m) = state.credits.memos.get(&source).cloned() {
        if m.customer != r.customer_id {
            return Err(refused("CREDIT_NOT_FOR_CUSTOMER"));
        }
        if r.date < m.date {
            return Err(refused("REFUND_DATE_BEFORE_SOURCE"));
        }
        if x > memo_remaining(state, source, &m) || m.void {
            return Err(conflict("CREDIT_NOT_AVAILABLE"));
        }
        check_closing(closing(state), &[r.date], password)?;
        let number = check_number.clone().unwrap_or_default();
        let ledger = post(
            state,
            "customer_refund",
            r.date,
            r.customer_id,
            &number,
            vec![
                (super::account_id(AR), x, Decimal::ZERO),
                (r.from_account, Decimal::ZERO, x),
            ],
        )?;
        (CreditKind::CreditMemo, ledger)
    } else {
        let pay = pay_views(state)
            .into_iter()
            .find(|p| p.id == source)
            .ok_or(ApiError::NotFound { code: None })?;
        if r.date < pay.date {
            return Err(refused("REFUND_DATE_BEFORE_SOURCE"));
        }
        let kind = if pay.hold {
            CreditKind::Deposit
        } else {
            CreditKind::Payment
        };
        let left = if pay.hold {
            super::receivables::held_remaining(state, source).unwrap_or_default()
        } else {
            pay.credit_remaining
        };
        if x > left {
            return Err(conflict(if pay.hold {
                "DEPOSIT_NOT_AVAILABLE"
            } else {
                "CREDIT_NOT_AVAILABLE"
            }));
        }
        let legacy = NewCustomerRefund {
            customer_id: r.customer_id,
            date: r.date,
            from_account: r.from_account,
            amount: r.amount,
            source_payment: source,
            method: None,
            ref_no: check_number.clone(),
            memo: r.memo.clone(),
        };
        (kind, super::receivables::refund(state, &legacy, password)?)
    };
    state.credits.refunds.insert(
        ledger,
        Refund {
            customer: r.customer_id,
            date: r.date,
            amount: x,
            source,
            kind,
            method: r.method,
            check_number,
            queued,
            ref_no: r.ref_no.clone(),
            void: false,
            ledger,
            version: 1,
            bank: r.from_account,
        },
    );
    if let Some(m) = state.credits.memos.get_mut(&source) {
        m.version += 1;
    }
    state.credits.writes += 1;
    Ok(ledger)
}

/// `GET /customer-refunds/{id}`.
///
/// # Errors
/// `NotFound`.
pub fn get_refund(state: &DemoState, id: TransactionId) -> Result<RefundRecord, ApiError> {
    let r = state
        .credits
        .refunds
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    let number = state
        .credits
        .memos
        .get(&r.source)
        .map(|m| m.number.clone())
        .or_else(|| {
            pay_views(state)
                .into_iter()
                .find(|p| p.id == r.source)
                .and_then(|p| p.ref_no)
        });
    Ok(RefundRecord {
        id,
        date: r.date,
        customer_id: r.customer,
        amount: money(r.amount)?,
        method: Some(r.method),
        ref_no: r.ref_no.clone(),
        check_number: r.check_number.clone(),
        from_account: Some(r.bank),
        print_status: Some(if r.queued && !r.void {
            RefundPrintStatus::Queue
        } else {
            RefundPrintStatus::None
        }),
        source: Some(RefundSourceRef {
            id: r.source,
            kind: r.kind,
            number,
        }),
        updated_at: version_of(r.version),
        status: if r.void {
            SalesDocStatus::Void
        } else {
            SalesDocStatus::Posted
        },
    })
}

/// Voids a refund: its credit or deposit comes back; a queued check leaves
/// the print queue (U-115).
///
/// # Errors
/// `NotFound`; closing on its date.
pub fn void_refund(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let r = state
        .credits
        .refunds
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if r.void {
        return Ok(());
    }
    check_closing(closing(state), &[r.date], password)?;
    let _ = state.ledger.void(r.ledger);
    if r.kind != CreditKind::CreditMemo {
        super::receivables::unrefund(state, r.source, r.amount);
    }
    if let Some(x) = state.credits.refunds.get_mut(&id) {
        x.void = true;
        x.queued = false;
        x.version += 1;
    }
    if let Some(m) = state.credits.memos.get_mut(&r.source) {
        m.version += 1;
    }
    state.credits.writes += 1;
    Ok(())
}

// ---- Sales receipts ----------------------------------------------------------------

/// The contact a sale without a client is priced for (no price level, no
/// balance).
fn walk_in() -> ContactId {
    ContactId::parse("d0000000-0000-4000-8000-000000000000")
        .unwrap_or_else(|_| unreachable!("a valid uuid literal"))
}

fn receipt_target(state: &mut DemoState, r: &NewSalesReceipt) -> Option<AccountId> {
    r.deposit_to_account_id
        .or_else(|| super::receivables::clearing(state))
}

fn negative_to_clearing(state: &mut DemoState, r: &NewSalesReceipt, total: Decimal) -> bool {
    let clearing = super::receivables::clearing(state);
    total < Decimal::ZERO && receipt_target(state, r) == clearing
}

/// `POST /sales-receipts/preview`.
///
/// # Errors
/// `Local(InvalidValue)` for what the engine refuses.
pub fn preview_receipt(
    state: &mut DemoState,
    r: &NewSalesReceipt,
) -> Result<SalesReceiptPreview, ApiError> {
    check_lines(&r.lines)?;
    let contact = r.contact_id.unwrap_or_else(walk_in);
    let mut figures =
        super::invoices::preview(state, &as_invoice(contact, r.txn_date, &r.lines), None)?;
    figures.due_date = None;
    figures.contact_balance_after = None;
    let mut problems = Vec::new();
    if negative_to_clearing(state, r, dec(figures.total)) {
        problems.push(problem("NEGATIVE_RECEIPT_NEEDS_BANK"));
    }
    Ok(SalesReceiptPreview { figures, problems })
}

fn receipt_ledger(
    state: &mut DemoState,
    lines: &[SalesDocLine],
    tax: Decimal,
    total: Decimal,
    target: AccountId,
) -> Vec<(AccountId, Decimal, Decimal)> {
    let tax_acct = tagged(&chart(state), "sales_tax_payable");
    let side = |a: AccountId, x: Decimal, debit: bool| {
        if (x.is_sign_negative()) ^ debit {
            (a, x.abs(), Decimal::ZERO)
        } else {
            (a, Decimal::ZERO, x.abs())
        }
    };
    let mut out = vec![side(target, total, true)];
    for l in lines {
        let a = l.account_id.unwrap_or_else(|| super::account_id(INCOME));
        out.push(side(a, dec(l.amount), false));
    }
    if let Some(t) = tax_acct {
        out.push(side(t, tax, false));
    }
    out
}

/// `POST /sales-receipts` (`existing`: the PUT of that receipt).
///
/// # Errors
/// 400 `NEGATIVE_RECEIPT_NEEDS_BANK`; 409 `STALE`, `TXN_DEPOSITED`;
/// closing.
pub fn save_receipt(
    state: &mut DemoState,
    r: &NewSalesReceipt,
    existing: Option<(TransactionId, Option<Timestamp>)>,
    password: Option<&Secret>,
) -> Result<SalesReceiptRecord, ApiError> {
    check_lines(&r.lines)?;
    let old = match existing {
        Some((id, expected)) => {
            let o = state
                .credits
                .receipts
                .get(&id)
                .cloned()
                .ok_or(ApiError::NotFound { code: None })?;
            check_version(o.version, expected)?;
            Some((id, o))
        }
        None => None,
    };
    let mut dates = vec![r.txn_date];
    dates.extend(old.as_ref().map(|(_, o)| o.record.date));
    check_closing(closing(state), &dates, password)?;
    let preview = preview_receipt(state, r)?;
    let total = dec(preview.figures.total);
    if !preview.problems.is_empty() {
        return Err(refused("NEGATIVE_RECEIPT_NEEDS_BANK"));
    }
    let target = receipt_target(state, r).ok_or_else(invalid)?;
    if let Some((_, o)) = &old
        && let Some(l) = o.ledger
        && state.ledger.deposited_in(l).is_some()
        && (o.request.deposit_to_account_id != r.deposit_to_account_id
            || dec(o.record.total) != total)
    {
        return Err(conflict("TXN_DEPOSITED"));
    }
    let lines = doc_lines(state, &r.lines, &preview.figures);
    let tax = dec(preview.figures.tax_total);
    if let Some((_, o)) = &old
        && let Some(l) = o.ledger
    {
        let _ = state.ledger.void(l);
    }
    let (id, version, number) = if let Some((id, o)) = &old {
        (*id, o.version + 1, o.record.number.clone())
    } else {
        {
            state.credits.next_receipt += 1;
            (
                new_id(state)?,
                1,
                Some(format!("SR-{}", 1000 + state.credits.next_receipt)),
            )
        }
    };
    let number = r.txn_number.clone().or(number);
    let client = r.contact_id.unwrap_or_else(walk_in);
    let posting = receipt_ledger(state, &lines, tax, total, target);
    let ledger = post(
        state,
        "cash_sale",
        r.txn_date,
        client,
        number.as_deref().unwrap_or_default(),
        posting,
    )?;
    let record = SalesReceiptRecord {
        id,
        number,
        date: r.txn_date,
        customer_id: r.contact_id,
        deposit_to_account_id: Some(target),
        payment_method: r.payment_method,
        ref_no: r.ref_no.clone(),
        lines,
        subtotal: preview.figures.subtotal,
        tax_amount: preview.figures.tax_total,
        total: preview.figures.total,
        memo: r.memo.clone(),
        customer_message: r.customer_message.clone(),
        updated_at: version_of(version),
        status: SalesDocStatus::Posted,
        deposit_id: None,
        deposited: false,
    };
    state.credits.receipts.insert(
        id,
        Receipt {
            record: record.clone(),
            request: r.clone(),
            version,
            ledger: Some(ledger),
        },
    );
    state.credits.writes += 1;
    Ok(record)
}

/// `GET /sales-receipts/{id}`.
///
/// # Errors
/// `NotFound`.
pub fn get_receipt(state: &DemoState, id: TransactionId) -> Result<SalesReceiptRecord, ApiError> {
    let r = state
        .credits
        .receipts
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    let mut rec = r.record.clone();
    rec.deposit_id = r.ledger.and_then(|l| state.ledger.deposited_in(l));
    rec.deposited = rec.deposit_id.is_some();
    Ok(rec)
}

/// The ledger entry of a sales receipt (Make Deposit's source; tests).
#[must_use]
pub fn receipt_ledger_id(state: &DemoState, id: TransactionId) -> Option<TransactionId> {
    state.credits.receipts.get(&id).and_then(|r| r.ledger)
}

/// Every credit memo, newest first (`GET /transactions?txnType=credit_memo`;
/// Prev/Next on the Credit Memo form).
#[must_use]
pub fn memo_ids(state: &DemoState) -> Vec<TransactionId> {
    let mut v: Vec<(CalendarDate, String, TransactionId)> = state
        .credits
        .memos
        .iter()
        .map(|(id, m)| (m.date, m.number.clone(), *id))
        .collect();
    v.sort();
    v.into_iter().rev().map(|(_, _, id)| id).collect()
}

/// Every sales receipt, newest first (Prev/Next on the Sales Receipt form).
#[must_use]
pub fn receipt_ids(state: &DemoState) -> Vec<TransactionId> {
    let mut v: Vec<(CalendarDate, String, TransactionId)> = state
        .credits
        .receipts
        .iter()
        .map(|(id, r)| {
            (
                r.record.date,
                r.record.number.clone().unwrap_or_default(),
                *id,
            )
        })
        .collect();
    v.sort();
    v.into_iter().rev().map(|(_, _, id)| id).collect()
}

/// Voids a sales receipt (`POST /transactions/{id}/void`): its ledger entry
/// is reversed. A deposited receipt is refused (409 `TXN_DEPOSITED`).
///
/// # Errors
/// `NotFound`; 409 `TXN_DEPOSITED`; closing on its date.
pub fn void_receipt(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let r = state
        .credits
        .receipts
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if r.record.status == SalesDocStatus::Void {
        return Ok(());
    }
    check_closing(closing(state), &[r.record.date], password)?;
    if let Some(l) = r.ledger {
        if state.ledger.deposited_in(l).is_some() {
            return Err(conflict("TXN_DEPOSITED"));
        }
        let _ = state.ledger.void(l);
    }
    if let Some(x) = state.credits.receipts.get_mut(&id) {
        x.record.status = SalesDocStatus::Void;
        x.version += 1;
        x.record.updated_at = version_of(x.version);
    }
    state.credits.writes += 1;
    Ok(())
}

#[cfg(test)]
#[path = "credit_memos_tests.rs"]
pub(super) mod tests;
