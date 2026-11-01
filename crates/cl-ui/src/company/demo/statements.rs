//! The demo "engine" for P22's A/R open items, A/R aging and customer
//! statements (D2): open items as of a date built the engine's way (each
//! document's own A/R amount, then the settled amount of every application
//! in effect moved from the credit to the invoice, then display netting of
//! refunds), the aging buckets, the three statement formats, numbered
//! snapshot runs, send history and statement email through the demo
//! Outbox.
//!
//! `--demo` only; arithmetic in `rust_decimal`, held to
//! `fixtures/receivables/golden.json` case 5. No floats. Synthetic data.

use std::collections::BTreeMap;
use std::fmt;

use cl_api::{
    AgingBuckets, ApiError, ArAgingCustomer, ArAgingSummary, CustomerStatement,
    CustomerStatementId, EmailMessage, IssuedStatement, IssuedStatementRecord, LastStatement,
    NewStatementRun, OutboxItem, SendEvent, SendMethod, SendRecord, StatementAging,
    StatementContact, StatementFormat, StatementHistoryRow, StatementLine, StatementListQuery,
    StatementListRow, StatementQuery, StatementRun, StatementRunId,
};
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;
use serde_json::json;

use super::DemoState;
use super::invoices::{receivable, round};
use super::receivables::{dec, invalid, money, pay_views, refused};

/// What an open item is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// An invoice.
    Invoice,
    /// A finance-charge invoice.
    Charge,
    /// A payment credit or a credit memo.
    Credit,
}

/// One open A/R item as of a date (the engine's `OpenItem`).
#[derive(Clone, PartialEq, Eq)]
pub struct OpenItem {
    /// The client.
    pub client: ContactId,
    /// The document.
    pub txn: TransactionId,
    /// `invoice`, `customer_payment` or `credit_memo`.
    pub txn_type: &'static str,
    /// Invoice, charge or credit.
    pub kind: ItemKind,
    /// Its number.
    pub number: String,
    /// Its date.
    pub date: CalendarDate,
    /// When it is due (invoices).
    pub due: Option<CalendarDate>,
    /// Its own A/R amount (credits negative).
    pub original: Decimal,
    /// What is open of it as of the date (cents).
    pub open: Decimal,
}

impl fmt::Debug for OpenItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenItem")
            .field("txn", &self.txn)
            .field("kind", &self.kind)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

impl OpenItem {
    /// Days past due as of `as_of` (credits age by their own date).
    #[must_use]
    pub fn days(&self, as_of: CalendarDate) -> i64 {
        let from = match self.kind {
            ItemKind::Credit => self.date,
            _ => self.due.unwrap_or(self.date),
        };
        from.days_until(as_of)
    }
}

fn later(a: CalendarDate, b: CalendarDate) -> CalendarDate {
    if a > b { a } else { b }
}

/// Every client's open A/R items as of `as_of` (`client`: one client).
#[must_use]
pub fn ar_open_items(
    state: &DemoState,
    as_of: CalendarDate,
    client: Option<ContactId>,
) -> Vec<OpenItem> {
    let wanted = |c: ContactId| client.is_none_or(|x| x == c);
    let invoice_date = |id: TransactionId| receivable(state, id).map(|r| r.date);
    let mut moved: BTreeMap<TransactionId, Decimal> = BTreeMap::new();
    let mut items = Vec::new();
    // Payments: own A/R credit = amount + discounts + write-offs taken.
    let dated = super::credit_memos::payment_refunds(state);
    for p in pay_views(state) {
        if p.hold || p.void || p.date > as_of || p.bounce.is_some_and(|b| b <= as_of) {
            continue;
        }
        let own_adj: Decimal = p
            .apps
            .iter()
            .filter(|a| !a.later && !a.voided)
            .map(|a| a.adjustments)
            .sum();
        let mut open = -(p.amount + own_adj);
        for a in &p.apps {
            let Some(inv_date) = invoice_date(a.invoice) else {
                continue;
            };
            let when = later(later(a.applied, p.date), inv_date);
            let reversed_by_then = a.reversed && p.bounce.is_none_or(|b| b <= as_of);
            if a.voided || reversed_by_then || when > as_of {
                continue;
            }
            *moved.entry(a.invoice).or_default() += a.settled;
            open += a.settled;
        }
        let dated_all: Decimal = dated.iter().filter(|r| r.0 == p.id).map(|r| r.2).sum();
        let dated_now: Decimal = dated
            .iter()
            .filter(|r| r.0 == p.id && r.1 <= as_of)
            .map(|r| r.2)
            .sum();
        // P21's inline refunds carry no date: they count at the payment's.
        open += (p.refunded - dated_all) + dated_now;
        if wanted(p.customer) {
            items.push(OpenItem {
                client: p.customer,
                txn: p.id,
                txn_type: "customer_payment",
                kind: ItemKind::Credit,
                number: p.ref_no.clone().unwrap_or_default(),
                date: p.date,
                due: None,
                original: -(p.amount + own_adj),
                open,
            });
        }
    }
    for m in super::credit_memos::memo_views(state) {
        if m.date > as_of {
            continue;
        }
        let mut open = -m.total;
        for (inv, x, applied) in &m.apps {
            let Some(inv_date) = invoice_date(*inv) else {
                continue;
            };
            let when = later(later(applied.unwrap_or(m.date), m.date), inv_date);
            if when > as_of {
                continue;
            }
            *moved.entry(*inv).or_default() += *x;
            open += *x;
        }
        open += m
            .refunds
            .iter()
            .filter(|r| r.0 <= as_of)
            .map(|r| r.1)
            .sum::<Decimal>();
        if wanted(m.customer) {
            items.push(OpenItem {
                client: m.customer,
                txn: m.id,
                txn_type: "credit_memo",
                kind: ItemKind::Credit,
                number: m.number.clone(),
                date: m.date,
                due: None,
                original: -m.total,
                open,
            });
        }
    }
    for id in super::invoices::list(state) {
        let Some(r) = receivable(state, id) else {
            continue;
        };
        if r.date > as_of || !wanted(r.client) {
            continue;
        }
        let open = r.total - moved.get(&id).copied().unwrap_or_default();
        items.push(OpenItem {
            client: r.client,
            txn: id,
            txn_type: "invoice",
            kind: if state.finance.is_charge(id) {
                ItemKind::Charge
            } else {
                ItemKind::Invoice
            },
            number: r.number,
            date: r.date,
            due: r.due,
            original: r.total,
            open,
        });
    }
    for i in &mut items {
        i.open = round(i.open, 2);
    }
    items.retain(|i| !i.open.is_zero());
    items.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.number.cmp(&b.number))
            .then_with(|| a.txn.cmp(&b.txn))
    });
    items
}

/// Aging buckets (exact sums) and their total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Aged {
    /// Not yet due.
    pub current: Decimal,
    /// 1-30 days.
    pub d30: Decimal,
    /// 31-60.
    pub d60: Decimal,
    /// 61-90.
    pub d90: Decimal,
    /// Over 90.
    pub over: Decimal,
}

impl Aged {
    /// Σ buckets.
    #[must_use]
    pub fn total(&self) -> Decimal {
        self.current + self.d30 + self.d60 + self.d90 + self.over
    }

    fn buckets(&self) -> Result<AgingBuckets, ApiError> {
        Ok(AgingBuckets {
            current: money(self.current)?,
            days_1_30: money(self.d30)?,
            days_31_60: money(self.d60)?,
            days_61_90: money(self.d90)?,
            over_90: money(self.over)?,
        })
    }
}

/// The engine's `ageOpenItems`.
#[must_use]
pub fn age(items: &[OpenItem], as_of: CalendarDate) -> Aged {
    let mut a = Aged::default();
    for i in items {
        let slot = match i.days(as_of) {
            d if d <= 0 => &mut a.current,
            1..=30 => &mut a.d30,
            31..=60 => &mut a.d60,
            61..=90 => &mut a.d90,
            _ => &mut a.over,
        };
        *slot += i.open;
    }
    a
}

fn client_name(state: &mut DemoState, c: ContactId) -> Option<String> {
    state
        .contacts_mut()
        .iter()
        .find(|x| x.id == c)
        .map(|x| x.fields.display_name.clone())
}

/// `GET /reports/ar-aging-summary` (exact part): one row per client with
/// anything open (the demo has no A/R lines without a client).
///
/// # Errors
/// `Internal` only for an amount out of range.
pub fn ar_aging(state: &mut DemoState, as_of: CalendarDate) -> Result<ArAgingSummary, ApiError> {
    super::credit_memos::seed(state);
    let items = ar_open_items(state, as_of, None);
    let mut clients: Vec<ContactId> = items.iter().map(|i| i.client).collect();
    clients.sort();
    clients.dedup();
    let mut customers = Vec::new();
    for c in clients {
        let mine: Vec<OpenItem> = items.iter().filter(|i| i.client == c).cloned().collect();
        let a = age(&mine, as_of);
        customers.push(ArAgingCustomer {
            contact_id: Some(c),
            name: client_name(state, c),
            buckets: a.buckets()?,
            total: money(a.total())?,
        });
    }
    let all = age(&items, as_of);
    Ok(ArAgingSummary {
        as_of_date: Some(as_of),
        buckets_exact: all.buckets()?,
        total_exact: money(all.total())?,
        customers,
    })
}

// ---- Statements ------------------------------------------------------------------

fn contact_block(state: &mut DemoState, c: ContactId) -> Result<StatementContact, ApiError> {
    let x = state
        .contacts_mut()
        .iter()
        .find(|x| x.id == c)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    let f = &x.fields;
    let mut address = Vec::new();
    if let Some(l) = f.billing_line1.clone().filter(|l| !l.is_empty()) {
        address.push(l);
    }
    let city = [f.billing_city.clone(), f.billing_state.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
    let city = match &f.billing_zip {
        Some(z) if !city.is_empty() => format!("{city} {z}"),
        _ => city,
    };
    if !city.is_empty() {
        address.push(city);
    }
    Ok(StatementContact {
        id: Some(c),
        name: f.display_name.clone(),
        address,
        email: f.email.clone().filter(|e| !e.trim().is_empty()),
    })
}

/// One posted document's own A/R amount for the balance-forward and
/// transaction formats: (date, txn, type, number, amount).
fn activity(
    state: &DemoState,
    c: ContactId,
    from: CalendarDate,
    to: CalendarDate,
) -> Vec<(CalendarDate, TransactionId, &'static str, String, Decimal)> {
    let mut out = Vec::new();
    let inside = |d: CalendarDate| d >= from && d <= to;
    for id in super::invoices::list(state) {
        if let Some(r) = receivable(state, id)
            && r.client == c
            && inside(r.date)
        {
            out.push((r.date, id, "invoice", r.number, r.total));
        }
    }
    for p in pay_views(state) {
        if p.customer != c || p.hold || p.void {
            continue;
        }
        let own: Decimal = p.amount
            + p.apps
                .iter()
                .filter(|a| !a.later && !a.voided)
                .map(|a| a.adjustments)
                .sum::<Decimal>();
        if inside(p.date) {
            out.push((
                p.date,
                p.id,
                "customer_payment",
                p.ref_no.clone().unwrap_or_default(),
                -own,
            ));
        }
        if let Some(b) = p.bounce.filter(|b| inside(*b)) {
            out.push((b, p.id, "returned_payment", String::new(), own));
        }
    }
    for (src, date, x) in super::credit_memos::payment_refunds(state) {
        if inside(date)
            && pay_views(state)
                .iter()
                .any(|p| p.id == src && p.customer == c && !p.hold)
        {
            out.push((date, src, "customer_refund", String::new(), x));
        }
    }
    for m in super::credit_memos::memo_views(state) {
        if m.customer != c {
            continue;
        }
        if inside(m.date) {
            out.push((m.date, m.id, "credit_memo", m.number.clone(), -m.total));
        }
        for (date, x) in m.refunds.iter().filter(|r| inside(r.0)) {
            out.push((*date, m.id, "customer_refund", String::new(), *x));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.3.cmp(&b.3)));
    out
}

#[allow(clippy::too_many_arguments)] // one statement line's columns
fn line(
    date: CalendarDate,
    txn: TransactionId,
    txn_type: &str,
    number: String,
    amount: Decimal,
    open: Option<Decimal>,
    due: Option<CalendarDate>,
    balance: Decimal,
) -> Result<StatementLine, ApiError> {
    Ok(StatementLine {
        date,
        txn_id: Some(txn),
        txn_type: txn_type.to_string(),
        number: Some(number).filter(|n| !n.is_empty()),
        description: None,
        due_date: due,
        amount: money(amount)?,
        open: open.map(money).transpose()?,
        balance: money(balance)?,
    })
}

/// `GET /reports/customer-statement`.
///
/// # Errors
/// `NotFound` for an unknown client.
pub fn statement(state: &mut DemoState, q: &StatementQuery) -> Result<CustomerStatement, ApiError> {
    super::credit_memos::seed(state);
    let contact = contact_block(state, q.contact)?;
    let items = ar_open_items(state, q.as_of, Some(q.contact));
    let aged = age(&items, q.as_of);
    let start = q.start.unwrap_or_else(|| q.as_of.start_of_month());
    let mut lines = Vec::new();
    let mut opening = None;
    match q.format {
        StatementFormat::OpenItem => {
            let mut bal = Decimal::ZERO;
            for i in &items {
                bal += i.open;
                lines.push(line(
                    i.date,
                    i.txn,
                    i.txn_type,
                    i.number.clone(),
                    i.original,
                    Some(i.open),
                    i.due,
                    bal,
                )?);
            }
        }
        StatementFormat::BalanceForward | StatementFormat::Transaction => {
            let before = start.add_days(-1).map_err(|_| invalid())?;
            let open: Decimal = ar_open_items(state, before, Some(q.contact))
                .iter()
                .map(|i| i.open)
                .sum();
            let mut bal = open;
            opening = Some(money(open)?);
            for (date, txn, kind, number, x) in activity(state, q.contact, start, q.as_of) {
                bal += x;
                lines.push(line(date, txn, kind, number, x, None, None, bal)?);
            }
        }
    }
    let total = aged.total();
    Ok(CustomerStatement {
        contact,
        as_of: q.as_of,
        format: q.format,
        start: (q.format != StatementFormat::OpenItem).then_some(start),
        opening_balance: opening,
        lines,
        aging: StatementAging {
            buckets: aged.buckets()?,
            total: money(total)?,
        },
        amount_due: money(total)?,
        difference: Money::ZERO,
    })
}

/// `GET /reports/customer-statements` (all rows; the screen pages).
pub fn statement_list(state: &mut DemoState, q: &StatementListQuery) -> Vec<StatementListRow> {
    super::credit_memos::seed(state);
    let items = ar_open_items(state, q.as_of, None);
    let customers: Vec<(ContactId, String, bool)> = state
        .contacts_mut()
        .iter()
        .filter(|c| c.fields.contact_type == cl_api::ContactType::Customer && c.is_active)
        .map(|c| {
            (
                c.id,
                c.fields.display_name.clone(),
                c.fields
                    .email
                    .as_deref()
                    .is_some_and(|e| !e.trim().is_empty()),
            )
        })
        .collect();
    let mut out = Vec::new();
    for (id, name, has_email) in customers {
        let mine: Vec<&OpenItem> = items.iter().filter(|i| i.client == id).collect();
        let due: Decimal = mine.iter().map(|i| i.open).sum();
        let oldest = mine
            .iter()
            .filter(|i| i.kind != ItemKind::Credit && i.open > Decimal::ZERO)
            .map(|i| i.days(q.as_of))
            .filter(|d| *d > 0)
            .max()
            .and_then(|d| u32::try_from(d).ok());
        if (q.open_only && due.is_zero()) || oldest.unwrap_or(0) < q.min_days_overdue {
            continue;
        }
        let last = state
            .statements
            .issued
            .values()
            .filter(|s| s.contact_id == id)
            .max_by_key(|s| (s.as_of, s.number.clone()))
            .map(|s| LastStatement {
                date: s.as_of,
                number: Some(s.number.clone()),
            });
        let Ok(amount_due) = money(due) else {
            continue;
        };
        out.push(StatementListRow {
            contact_id: id,
            name,
            amount_due,
            oldest_days_overdue: oldest,
            has_email,
            last_statement: last,
        });
    }
    out
}

/// Statement runs, snapshots and send history of the demo engine.
#[derive(Clone, Default)]
pub struct DemoStatements {
    runs: BTreeMap<String, StatementRun>,
    issued: BTreeMap<CustomerStatementId, IssuedStatementRecord>,
    next_number: u32,
    next_id: u64,
    /// Email keys already sent (a replay sends once).
    sent_keys: BTreeMap<String, CustomerStatementId>,
}

impl fmt::Debug for DemoStatements {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoStatements")
            .field("runs", &self.runs.len())
            .field("issued", &self.issued.len())
            .finish_non_exhaustive()
    }
}

fn snapshot_of(mut s: CustomerStatement) -> CustomerStatement {
    s.contact.id = None;
    for l in &mut s.lines {
        l.txn_id = None;
    }
    s
}

fn next_uuid(state: &mut DemoState, prefix: &str) -> String {
    state.statements.next_id += 1;
    format!("{prefix}-0000-4000-8000-{:012}", state.statements.next_id)
}

/// `POST /customer-statement-runs` (`key`: the Idempotency-Key; a replay
/// answers the same run).
///
/// # Errors
/// 400 `TOO_MANY_CONTACTS`; `NotFound` for an unknown client.
pub fn create_run(
    state: &mut DemoState,
    r: &NewStatementRun,
    key: &str,
) -> Result<StatementRun, ApiError> {
    if let Some(done) = state.statements.runs.get(key) {
        return Ok(done.clone());
    }
    if r.contact_ids.len() > cl_api::MAX_RUN_CONTACTS {
        return Err(refused("TOO_MANY_CONTACTS"));
    }
    if r.contact_ids.is_empty() {
        return Err(invalid());
    }
    let run_id = StatementRunId::parse(&next_uuid(state, "d6100000")).map_err(|_| invalid())?;
    let mut statements = Vec::new();
    for c in &r.contact_ids {
        let s = statement(
            state,
            &StatementQuery {
                contact: *c,
                as_of: r.as_of,
                format: r.format,
                start: r.start,
            },
        )?;
        state.statements.next_number += 1;
        let number = (1000 + state.statements.next_number).to_string();
        let id =
            CustomerStatementId::parse(&next_uuid(state, "d6000000")).map_err(|_| invalid())?;
        let amount_due = s.amount_due;
        state.statements.issued.insert(
            id,
            IssuedStatementRecord {
                id,
                number: number.clone(),
                contact_id: *c,
                as_of: r.as_of,
                amount_due,
                snapshot: snapshot_of(s),
                send_history: Vec::new(),
                created_at: Some(DemoState::now()),
            },
        );
        statements.push(IssuedStatement {
            statement_id: id,
            contact_id: *c,
            number,
            amount_due,
        });
    }
    let run = StatementRun { run_id, statements };
    state.statements.runs.insert(key.to_string(), run.clone());
    Ok(run)
}

/// `GET /customer-statements/{id}`.
///
/// # Errors
/// `NotFound`.
pub fn get(state: &DemoState, id: CustomerStatementId) -> Result<IssuedStatementRecord, ApiError> {
    state
        .statements
        .issued
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })
}

/// `GET /customer-statements?contactId=`, newest first.
#[must_use]
pub fn history(state: &DemoState, contact: ContactId) -> Vec<StatementHistoryRow> {
    let mut v: Vec<StatementHistoryRow> = state
        .statements
        .issued
        .values()
        .filter(|s| s.contact_id == contact)
        .map(|s| StatementHistoryRow {
            id: s.id,
            number: s.number.clone(),
            as_of: s.as_of,
            amount_due: s.amount_due,
            created_at: s.created_at,
            send_history: s.send_history.clone(),
        })
        .collect();
    v.sort_by(|a, b| b.as_of.cmp(&a.as_of).then_with(|| b.number.cmp(&a.number)));
    v
}

/// `POST /customer-statements/{id}/sent`.
///
/// # Errors
/// `NotFound`.
pub fn mark_sent(
    state: &mut DemoState,
    id: CustomerStatementId,
    r: &SendRecord,
) -> Result<(), ApiError> {
    let s = state
        .statements
        .issued
        .get_mut(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    s.send_history.push(SendEvent {
        method: r.method,
        recipients: r.recipients.clone(),
        sent_at: Some(r.sent_at.unwrap_or_else(DemoState::now)),
    });
    Ok(())
}

/// `POST /customer-statements/{id}/email` (the demo has no mail server
/// unless a test turns `no_smtp` off; the send is recorded once per key).
///
/// # Errors
/// 400 `SMTP_NOT_CONFIGURED`; `NotFound`.
pub fn email(
    state: &mut DemoState,
    id: CustomerStatementId,
    message: &EmailMessage,
    outbox: Option<cl_api::OutboxId>,
    key: &str,
) -> Result<(), ApiError> {
    if state.statements.sent_keys.contains_key(key) {
        return Ok(());
    }
    if state.invoices.no_smtp {
        return Err(refused(cl_api::SMTP_NOT_CONFIGURED));
    }
    mark_sent(
        state,
        id,
        &SendRecord {
            method: SendMethod::Email,
            recipients: message.to.clone(),
            sent_at: None,
        },
    )?;
    if let Some(o) = outbox {
        let _ = super::invoices::remove_from_outbox(state, o);
    }
    state.statements.sent_keys.insert(key.to_string(), id);
    Ok(())
}

/// `POST /customer-statements/{id}/email-later`: the message waits in the
/// Outbox with `kind: customer_statement`.
///
/// # Errors
/// `NotFound`; `Internal` when the row can't be built.
pub fn email_later(
    state: &mut DemoState,
    id: CustomerStatementId,
    message: &EmailMessage,
) -> Result<OutboxItem, ApiError> {
    let number = get(state, id)?.number;
    let row = json!({
        "id": next_uuid(state, "d5100000"),
        "kind": "customer_statement",
        "customerStatementId": id,
        "txnNumber": number,
        "to": message.to, "cc": message.cc, "bcc": message.bcc,
        "subject": message.subject, "body": message.body,
        "createdAt": DemoState::now(),
    });
    let item = serde_json::from_value(row.clone()).map_err(|_| ApiError::Internal)?;
    super::invoices::push_outbox(state, row);
    Ok(item)
}

/// The client's balance as the open items total it (tests: = the A/R
/// aging row and the demo contact balance).
#[must_use]
pub fn items_total(state: &DemoState, as_of: CalendarDate, client: ContactId) -> Decimal {
    ar_open_items(state, as_of, Some(client))
        .iter()
        .map(|i| i.open)
        .sum()
}

/// Exact value of a statement amount (tests).
#[must_use]
pub fn amount(m: Money) -> Decimal {
    dec(m)
}

#[cfg(test)]
#[path = "statements_tests.rs"]
mod tests;
