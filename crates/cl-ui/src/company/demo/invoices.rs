//! The demo "engine" for Create Invoice (P20 WP-A step 5): the sales money
//! rule of the engine (U-122, `sales-cents-v1`) in `rust_decimal`, held to
//! the engine's golden file (`fixtures/invoices/golden.json`, byte-identical
//! to the engine's `packages/shared/fixtures/invoice-golden.json`), the
//! preview (U-129), and a small invoice store with send history (U-126),
//! void and billable-cost claims (U-128).
//!
//! `--demo` only. Every figure the screen shows comes from here, never from
//! the screen:
//!
//! - line amount = round2(quantity × rate), half away from zero;
//! - tax = round2(taxable base × rate) once per document and rate, the base
//!   the signed sum of the taxable lines (a taxable discount reduces it,
//!   Q-P20t); allocated to the taxable lines by largest remainder (shares
//!   floored toward minus infinity, leftover cents to the largest
//!   remainders, the earlier line first);
//! - total = subtotal + tax.
//!
//! No floats anywhere. All data is synthetic.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use cl_api::{
    ApiError, ConflictReason, EditableFields, InvoicePreview, InvoiceRecord, LocalValidation,
    NewInvoice, RecordVersion, SendMethod, SendRecord, UserText, Versioned,
};
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Value, json};

use super::DemoState;
use super::register::check_closing;

/// The demo company's sales tax rate (the mockup's 8.1 %).
pub const DEMO_TAX_RATE: &str = "0.081";
/// The demo company's terms when the invoice and client name none.
pub const DEMO_TERMS: &str = "net_30";

const AR: &str = "d2000000-0000-4000-8000-000000001200";
const SALES_TAX: &str = "d2000000-0000-4000-8000-000000002200";

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn conflict(code: &str) -> ApiError {
    ApiError::Conflict {
        reason: ConflictReason::Unspecified,
        current_version: None,
        code: cl_api::ErrorCode::parse(code),
        message: UserText::default(),
    }
}

/// Half away from zero to `dp` places.
#[must_use]
pub fn round(d: Decimal, dp: u32) -> Decimal {
    d.round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)
}

/// A decimal string as the engine takes it: an optional `-`, at most
/// `int_digits` integer digits and 4 decimals.
fn parse_decimal(s: &str, int_digits: usize, signed: bool) -> Result<Decimal, ApiError> {
    let t = s.trim();
    let body = if signed {
        t.strip_prefix('-').unwrap_or(t)
    } else {
        t
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if int.is_empty()
        || int.len() > int_digits
        || !digits(int)
        || !digits(frac)
        || frac.len() > 4
        || (body.contains('.') && frac.is_empty())
    {
        return Err(invalid());
    }
    Decimal::from_str(t).map_err(|_| invalid())
}

/// A quantity (8 integer digits, 4 places, may be negative).
///
/// # Errors
/// `Local(InvalidValue)`, as the engine's 400 `VALIDATION_ERROR`.
pub fn parse_quantity(s: &str) -> Result<Decimal, ApiError> {
    parse_decimal(s, 8, true)
}

/// A unit price (15 integer digits, 4 places, may be negative).
///
/// # Errors
/// As [`parse_quantity`].
pub fn parse_price(s: &str) -> Result<Decimal, ApiError> {
    parse_decimal(s, 15, true)
}

/// A tax rate as a fraction, rounded to the 4 places the ledger stores;
/// 0 up to (not including) 1.
///
/// # Errors
/// As [`parse_quantity`].
pub fn parse_rate(s: &str) -> Result<Decimal, ApiError> {
    let t = s.trim();
    let (int, frac) = t.split_once('.').unwrap_or((t, ""));
    if int.is_empty()
        || int.len() > 4
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
        || (t.contains('.') && frac.is_empty())
    {
        return Err(invalid());
    }
    let r = round(Decimal::from_str(t).map_err(|_| invalid())?, 4);
    if r >= Decimal::ONE {
        return Err(invalid());
    }
    Ok(r)
}

/// One line as the rule takes it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SalesLineIn {
    /// Quantity.
    pub quantity: Decimal,
    /// Unit price.
    pub unit_price: Decimal,
    /// Taxable.
    pub taxable: bool,
    /// The line's own rate (fraction, 4 places); `None` takes the default.
    pub tax_rate: Option<Decimal>,
}

impl fmt::Debug for SalesLineIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SalesLineIn")
            .field("taxable", &self.taxable)
            .finish_non_exhaustive()
    }
}

/// One line's figures (2 places; the rate 4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SalesLineOut {
    /// round2(quantity × rate).
    pub amount: Decimal,
    /// The rate applied (0 when not taxable).
    pub tax_rate: Decimal,
    /// The line's share of the document tax.
    pub tax_amount: Decimal,
}

/// A document's figures.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SalesDocOut {
    /// Per line, in order.
    pub lines: Vec<SalesLineOut>,
    /// Sum of the line amounts.
    pub subtotal: Decimal,
    /// Signed sum of the taxable line amounts.
    pub taxable_base: Decimal,
    /// The document tax.
    pub tax_total: Decimal,
    /// Subtotal plus tax.
    pub total: Decimal,
}

/// Largest-remainder allocation of `total` over `shares` (any signs): each
/// share floored to the cent, the leftover cents one each to the largest
/// remainders, the earlier first on a tie.
fn allocate(shares: &[Decimal], total: Decimal) -> Result<Vec<Decimal>, ApiError> {
    let hundred = Decimal::ONE_HUNDRED;
    let floors: Vec<Decimal> = shares.iter().map(|s| (s * hundred).floor()).collect();
    let rems: Vec<Decimal> = shares
        .iter()
        .zip(&floors)
        .map(|(s, f)| s * hundred - f)
        .collect();
    let left = total * hundred - floors.iter().copied().sum::<Decimal>();
    let n = Decimal::from(shares.len());
    if !left.fract().is_zero() || left.is_sign_negative() && !left.is_zero() || left > n {
        return Err(ApiError::Internal);
    }
    let mut order: Vec<usize> = (0..shares.len()).collect();
    order.sort_by(|a, b| rems[*b].cmp(&rems[*a]).then(a.cmp(b)));
    let mut cents = floors;
    let mut k = Decimal::ZERO;
    for i in order {
        if k >= left {
            break;
        }
        cents[i] += Decimal::ONE;
        k += Decimal::ONE;
    }
    Ok(cents.into_iter().map(|c| c / hundred).collect())
}

/// Every figure of an invoice or sales receipt (the engine's
/// `computeSalesDocument`).
///
/// # Errors
/// `Internal` only if the allocation leaves its range (never for valid
/// input).
pub fn compute(lines: &[SalesLineIn], default_rate: Decimal) -> Result<SalesDocOut, ApiError> {
    let amounts: Vec<Decimal> = lines
        .iter()
        .map(|l| round(l.quantity * l.unit_price, 2))
        .collect();
    let rates: Vec<Decimal> = lines
        .iter()
        .map(|l| {
            if l.taxable {
                l.tax_rate.unwrap_or(default_rate)
            } else {
                Decimal::ZERO
            }
        })
        .collect();
    // Taxable lines with a rate above zero, grouped by rate, first seen first.
    let mut groups: Vec<(Decimal, Vec<usize>)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !l.taxable || rates[i].is_zero() {
            continue;
        }
        match groups.iter_mut().find(|(r, _)| *r == rates[i]) {
            Some((_, idx)) => idx.push(i),
            None => groups.push((rates[i], vec![i])),
        }
    }
    let mut taxes = vec![Decimal::ZERO; lines.len()];
    let mut tax_total = Decimal::ZERO;
    for (rate, idx) in &groups {
        let base: Decimal = idx.iter().map(|i| amounts[*i]).sum();
        let tax = round(base * rate, 2);
        let shares: Vec<Decimal> = idx.iter().map(|i| amounts[*i] * rate).collect();
        for (i, t) in idx.iter().zip(allocate(&shares, tax)?) {
            taxes[*i] = t;
        }
        tax_total += tax;
    }
    let subtotal: Decimal = amounts.iter().copied().sum();
    let taxable_base: Decimal = lines
        .iter()
        .zip(&amounts)
        .filter(|(l, _)| l.taxable)
        .map(|(_, a)| *a)
        .sum();
    Ok(SalesDocOut {
        lines: (0..lines.len())
            .map(|i| SalesLineOut {
                amount: amounts[i],
                tax_rate: rates[i],
                tax_amount: taxes[i],
            })
            .collect(),
        subtotal,
        taxable_base,
        tax_total,
        total: subtotal + tax_total,
    })
}

/// The demo's default rate.
fn default_rate() -> Decimal {
    Decimal::from_str(DEMO_TAX_RATE).unwrap_or(Decimal::ZERO)
}

fn money(d: Decimal) -> Result<Money, ApiError> {
    Money::try_from(d).map_err(|_| invalid())
}

/// One stored demo invoice.
#[derive(Clone)]
struct Stored {
    record: Value,
    claims: Vec<JournalLineId>,
}

/// Invoices the demo engine keeps.
#[derive(Clone, Default)]
pub struct DemoInvoices {
    stored: BTreeMap<TransactionId, Stored>,
    claimed: BTreeSet<JournalLineId>,
    next: u64,
    /// Write requests that reached the demo engine (tests).
    pub writes: u32,
    /// The Outbox (Q-P20q), oldest first (WP-B).
    outbox: Vec<Value>,
    /// Each client's default markup percent (U-128, WP-B).
    pub markups: BTreeMap<cl_core::ids::ContactId, cl_api::SignedPercent>,
    /// The demo company has no email server (WP-B): sending then fails
    /// with `SMTP_NOT_CONFIGURED`, as an engine without one.
    pub no_smtp: bool,
}

impl fmt::Debug for DemoInvoices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoInvoices")
            .field("invoices", &self.stored.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

/// A line as priced: the engine's request line with its rate resolved.
struct Priced {
    input: SalesLineIn,
    price: Decimal,
    source: &'static str,
    account: Option<AccountId>,
    /// A subtotal line shows the sum of its group and posts nothing.
    display: Option<Decimal>,
}

/// round2(quantity × rate) of the priced lines of `group` that post.
fn group_sum(group: &[Priced]) -> Decimal {
    group
        .iter()
        .filter(|p| p.display.is_none())
        .map(|p| round(p.input.quantity * p.input.unit_price, 2))
        .sum()
}

fn price_lines(state: &mut DemoState, new: &NewInvoice) -> Result<Vec<Priced>, ApiError> {
    if new.lines.is_empty() || new.lines.len() > cl_api::INVOICE_MAX_LINES {
        return Err(invalid());
    }
    let items = state.items.get_or_insert_with(super::lists::items).clone();
    let mut out: Vec<Priced> = Vec::with_capacity(new.lines.len());
    // U-39a: a discount covers the lines since the last subtotal; right
    // under a subtotal it covers that subtotal's lines.
    let mut group_start = 0;
    let mut prev_group: Option<usize> = None;
    for l in &new.lines {
        let item = l
            .item_id
            .map(|id| items.iter().find(|i| i.id == id).ok_or_else(invalid))
            .transpose()?;
        let account = l.account_id.or(item.map(|i| i.fields.income_account_id));
        if account.is_none() {
            return Err(invalid());
        }
        let quantity = parse_quantity(&l.quantity.to_wire())?;
        let taxable = l
            .is_taxable
            .unwrap_or(item.is_some_and(|i| i.fields.is_taxable));
        if let Some(src) = l.billable_source_line_id {
            // Q-P20w: one line whose amount includes the markup; the rate
            // shown is amount ÷ quantity, the amount itself is exact.
            let cost = super::billable::cost(state, src).ok_or_else(invalid)?;
            let markup = l.markup_percent.map(cl_api::SignedPercent::as_decimal);
            let fig = super::billable::figures(cost.amount.as_decimal(), quantity, markup);
            out.push(Priced {
                input: SalesLineIn {
                    quantity: Decimal::ONE,
                    unit_price: fig.amount,
                    taxable,
                    tax_rate: None,
                },
                price: fig.rate,
                source: "billable",
                account,
                display: None,
            });
            continue;
        }
        let kind = item.map_or(cl_api::ItemType::Service, |i| i.fields.item_type);
        if kind == cl_api::ItemType::Subtotal {
            let shown = group_sum(&out[group_start..]);
            out.push(Priced {
                input: SalesLineIn {
                    quantity: Decimal::ZERO,
                    unit_price: Decimal::ZERO,
                    taxable: false,
                    tax_rate: None,
                },
                price: shown,
                source: "computed",
                account,
                display: Some(shown),
            });
            prev_group = Some(group_start);
            group_start = out.len();
            continue;
        }
        if kind == cl_api::ItemType::Discount {
            let from = if out.last().is_some_and(|p| p.display.is_some()) {
                prev_group.unwrap_or(0)
            } else {
                group_start
            };
            let base = group_sum(&out[from..]);
            let amount = match item.and_then(|i| i.fields.discount_percent) {
                Some(p) => -round(base * p.as_decimal() / Decimal::ONE_HUNDRED, 2),
                None => -round(
                    item.and_then(|i| i.fields.unit_price)
                        .map_or(Decimal::ZERO, |m| m.as_decimal().abs()),
                    2,
                ),
            };
            out.push(Priced {
                input: SalesLineIn {
                    quantity: Decimal::ONE,
                    unit_price: amount,
                    taxable,
                    tax_rate: None,
                },
                price: amount,
                source: "computed",
                account,
                display: None,
            });
            continue;
        }
        let (price, source) = if let Some(p) = l.unit_price {
            (parse_price(&p.to_wire())?, "input")
        } else {
            let base = item
                .and_then(|i| i.fields.unit_price)
                .ok_or_else(invalid)?
                .as_decimal();
            super::price_levels::price_for(state, new.contact_id, item.map(|i| i.id), base)
        };
        out.push(Priced {
            input: SalesLineIn {
                quantity,
                unit_price: price,
                taxable,
                tax_rate: None,
            },
            price,
            source,
            account,
            display: None,
        });
    }
    Ok(out)
}

/// The due date of terms `net_N` / `due_on_receipt` from `date`.
fn due_of(code: &str, days: Option<u32>, date: CalendarDate) -> Option<CalendarDate> {
    let n: i64 = match code {
        "due_on_receipt" => 0,
        "custom" => i64::from(days?),
        c => c.strip_prefix("net_")?.parse().ok()?,
    };
    date.add_days(n).ok()
}

/// `POST /invoices/preview` as the demo engine answers it.
///
/// # Errors
/// `Local(InvalidValue)` for what the engine refuses (no lines, unknown
/// item, a line with neither account nor item, bad decimals).
pub fn preview(state: &mut DemoState, new: &NewInvoice) -> Result<InvoicePreview, ApiError> {
    let priced = price_lines(state, new)?;
    let ins: Vec<SalesLineIn> = priced.iter().map(|p| p.input).collect();
    let doc = compute(&ins, default_rate())?;
    let terms = new.payment_terms.as_deref().unwrap_or(DEMO_TERMS);
    let due = new
        .due_date
        .or_else(|| due_of(terms, new.terms_days, new.txn_date));
    let lines: Vec<Value> = priced
        .iter()
        .zip(&doc.lines)
        .map(|(p, o)| {
            json!({
                "unitPrice": p.price.to_string(),
                "unitPriceSource": p.source,
                "amount": p.display.unwrap_or(o.amount).to_string(),
                "taxAmount": o.tax_amount.to_string(),
            })
        })
        .collect();
    let v = json!({
        "lines": lines,
        "subtotal": doc.subtotal.to_string(),
        "taxableBase": doc.taxable_base.to_string(),
        "taxRate": default_rate().to_string(),
        "taxTotal": doc.tax_total.to_string(),
        "total": doc.total.to_string(),
        "dueDate": due,
    });
    serde_json::from_value(v).map_err(|_| ApiError::Internal)
}

fn versioned(record: &Value) -> Result<Versioned<InvoiceRecord>, ApiError> {
    let r: InvoiceRecord =
        serde_json::from_value(record.clone()).map_err(|_| ApiError::Internal)?;
    let version = RecordVersion::Fingerprint(r.fingerprint());
    Ok(Versioned { record: r, version })
}

fn closing(state: &DemoState) -> Option<CalendarDate> {
    let open = state.open?;
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == open)
        .and_then(|c| c.closed_through)
}

fn record_json(
    state: &mut DemoState,
    id: TransactionId,
    number: &str,
    new: &NewInvoice,
) -> Result<(Value, Vec<JournalLineId>), ApiError> {
    let priced = price_lines(state, new)?;
    let ins: Vec<SalesLineIn> = priced.iter().map(|p| p.input).collect();
    let doc = compute(&ins, default_rate())?;
    let m4 = |d: Decimal| money(d).map(Money::to_wire);
    let side = |d: Decimal| -> Result<(String, String), ApiError> {
        if d.is_sign_negative() && !d.is_zero() {
            Ok((m4(-d)?, m4(Decimal::ZERO)?))
        } else {
            Ok((m4(Decimal::ZERO)?, m4(d)?))
        }
    };
    let line_id = |n: usize| format!("d3{:06}-0000-4000-8000-{:012}", id_seq(id), n);
    let mut lines = vec![json!({
        "id": line_id(0), "accountId": AR, "debit": m4(doc.total)?, "credit": m4(Decimal::ZERO)?,
        "lineOrder": 0, "quantity": null,
    })];
    let mut claims = Vec::new();
    for (i, ((p, o), l)) in priced.iter().zip(&doc.lines).zip(&new.lines).enumerate() {
        let (debit, credit) = side(o.amount)?;
        if let Some(src) = l.billable_source_line_id {
            claims.push(src);
        }
        lines.push(json!({
            "id": line_id(i + 1), "accountId": p.account, "debit": debit, "credit": credit,
            "description": l.description, "lineOrder": i + 1, "tagId": l.tag_id, "itemId": l.item_id,
            "quantity": l.quantity.to_wire(), "unitPrice": money(p.price)?.to_wire(),
            "isTaxable": p.input.taxable, "taxRate": round(o.tax_rate, 4).to_string(),
            "taxAmount": m4(o.tax_amount)?, "serviceDate": l.service_date,
            "billableSourceLineId": l.billable_source_line_id, "markupPercent": l.markup_percent,
        }));
    }
    if !doc.tax_total.is_zero() {
        let (debit, credit) = side(doc.tax_total)?;
        lines.push(json!({
            "id": line_id(new.lines.len() + 1), "accountId": SALES_TAX, "debit": debit,
            "credit": credit, "description": "Sales Tax", "lineOrder": new.lines.len() + 1,
            "quantity": null,
        }));
    }
    let terms = new
        .payment_terms
        .clone()
        .unwrap_or_else(|| DEMO_TERMS.into());
    let due = new
        .due_date
        .or_else(|| due_of(&terms, new.terms_days, new.txn_date));
    let v = json!({
        "id": id, "txnType": "invoice", "txnNumber": number, "txnDate": new.txn_date,
        "status": "posted", "contactId": new.contact_id, "memo": new.memo,
        "total": m4(doc.total)?, "subtotal": m4(doc.subtotal)?, "taxAmount": m4(doc.tax_total)?,
        "balanceDue": m4(doc.total)?, "amountPaid": m4(Decimal::ZERO)?, "invoiceStatus": "draft",
        "dueDate": due, "dueDateOverridden": new.due_date.is_some(), "paymentTerms": terms,
        "termsDays": new.terms_days, "poNumber": new.po_number, "customerMessage": new.customer_message,
        "hideZeroLines": new.hide_zero_lines.unwrap_or(false), "tagIds": new.tag_ids.clone().unwrap_or_default(),
        "sentAt": null, "sendHistory": [], "printLater": new.print_later.unwrap_or(false),
        "updatedAt": DemoState::now(), "lines": lines,
    });
    Ok((v, claims))
}

fn id_seq(id: TransactionId) -> u64 {
    let s = id.to_string();
    s.rsplit('-')
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
        % 1_000_000
}

/// Creates an invoice (`POST /invoices`): priced and taxed by the rule
/// above; a closed-period date needs the demo closing password; a billable
/// cost already invoiced is 409 `BILLABLE_ALREADY_BILLED`.
///
/// # Errors
/// `ClosedPeriod`, `ClosingPasswordRejected`, `Conflict`, `Local(InvalidValue)`.
pub fn create(
    state: &mut DemoState,
    new: &NewInvoice,
    password: Option<&Secret>,
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    cl_api::check_invoice(new, &all_caps()).map_err(|_| invalid())?;
    check_closing(closing(state), &[new.txn_date], password)?;
    if new
        .lines
        .iter()
        .filter_map(|l| l.billable_source_line_id)
        .any(|s| state.invoices.claimed.contains(&s))
    {
        return Err(conflict(cl_api::BILLABLE_ALREADY_BILLED));
    }
    state.invoices.next += 1;
    let n = state.invoices.next;
    let id =
        TransactionId::parse(&format!("d1000000-0000-4000-8000-{n:012}")).map_err(|_| invalid())?;
    let number = new
        .txn_number
        .clone()
        .unwrap_or_else(|| (1041 + n).to_string());
    let (record, claims) = record_json(state, id, &number, new)?;
    let saved = versioned(&record)?;
    state.invoices.writes += 1;
    state.invoices.claimed.extend(claims.iter().copied());
    state.invoices.stored.insert(id, Stored { record, claims });
    Ok(saved)
}

/// The capabilities the demo engine plays (E1 to E4).
#[must_use]
pub fn all_caps() -> cl_api::Capabilities {
    [
        cl_api::Capabilities::SALES_CENTS,
        cl_api::Capabilities::INVOICE_EDIT,
        cl_api::Capabilities::INVOICE_TERMS,
        cl_api::Capabilities::INVOICE_FIELDS,
        cl_api::Capabilities::INVOICE_SENT_HISTORY,
        cl_api::Capabilities::CONTACT_OPEN_BALANCE,
        cl_api::Capabilities::INVOICE_PRICING,
        cl_api::Capabilities::BILLABLE_COSTS,
        cl_api::Capabilities::PRICE_LEVELS,
        cl_api::Capabilities::INVOICE_EMAIL,
    ]
    .into_iter()
    .collect()
}

/// `GET /invoices/{id}`.
#[must_use]
pub fn get(state: &DemoState, id: TransactionId) -> Option<Versioned<InvoiceRecord>> {
    state
        .invoices
        .stored
        .get(&id)
        .and_then(|s| versioned(&s.record).ok())
}

/// `POST /invoices/{id}/sent`: a draft becomes sent, the event is kept.
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for a void invoice (`INVOICE_VOID`).
pub fn record_sent(
    state: &mut DemoState,
    id: TransactionId,
    sent: &SendRecord,
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    let s = state
        .invoices
        .stored
        .get_mut(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    if s.record["status"] == "void" {
        return Err(invalid());
    }
    let now = DemoState::now();
    let method = match sent.method {
        SendMethod::Printed => "printed",
        SendMethod::Email => "email",
        SendMethod::Pdf | SendMethod::Other => "pdf",
    };
    if let Some(h) = s.record["sendHistory"].as_array_mut() {
        h.push(json!({"method": method, "recipients": sent.recipients, "sentAt": now}));
    }
    if s.record["sentAt"].is_null() {
        s.record["sentAt"] = json!(now);
    }
    if s.record["invoiceStatus"] == "draft" {
        s.record["invoiceStatus"] = json!("sent");
    }
    state.invoices.writes += 1;
    versioned(&s.record)
}

/// `POST /invoices/{id}/void`: billable costs on it are unbilled again.
///
/// # Errors
/// `NotFound`, `ClosedPeriod`, `ClosingPasswordRejected`.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    let close = closing(state);
    let s = state
        .invoices
        .stored
        .get_mut(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    let date = s.record["txnDate"]
        .as_str()
        .and_then(|d| CalendarDate::parse_wire(d).ok())
        .ok_or(ApiError::Internal)?;
    check_closing(close, &[date], password)?;
    s.record["status"] = json!("void");
    s.record["invoiceStatus"] = json!("void");
    s.record["balanceDue"] = json!(Money::ZERO.to_wire());
    for c in std::mem::take(&mut s.claims) {
        state.invoices.claimed.remove(&c);
    }
    state.invoices.writes += 1;
    versioned(&s.record)
}

/// Invoices never sent (the Outbox's Not sent yet tab, Q-P20s), newest
/// first.
#[must_use]
pub fn unsent(state: &DemoState) -> Vec<TransactionId> {
    state
        .invoices
        .stored
        .iter()
        .rev()
        .filter(|(_, s)| {
            s.record["status"] != "void"
                && s.record["sendHistory"].as_array().is_none_or(Vec::is_empty)
        })
        .map(|(id, _)| *id)
        .collect()
}

/// True when the demo engine has invoiced the cost.
#[must_use]
pub fn is_billed(state: &DemoState, source: JournalLineId) -> bool {
    state.invoices.claimed.contains(&source)
}

// ---- WP-B: edits, the list, email and the Outbox ------------------------------

/// A fresh `updatedAt` per write (the demo clock stands still).
fn stamp(state: &DemoState) -> String {
    cl_core::Timestamp::from_unix_millis(1_793_000_000_000 + i64::from(state.invoices.writes))
        .map_or_else(|_| DemoState::now().to_wire(), cl_core::Timestamp::to_wire)
}

/// `PUT /invoices/{id}` (`invoice-edit-v1`): replaced if still as
/// `expected` was read (else 409 `STALE`); both dates need the closing
/// password when closed; the send history, status and number stay;
/// billable costs taken off are unbilled.
///
/// # Errors
/// `NotFound`, `Conflict` (`STALE`, `BILLABLE_ALREADY_BILLED`),
/// `ClosedPeriod`, `ClosingPasswordRejected`, `Local(InvalidValue)` (void).
pub fn update(
    state: &mut DemoState,
    id: TransactionId,
    expected: Option<cl_core::Timestamp>,
    new: &NewInvoice,
    password: Option<&Secret>,
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    cl_api::check_invoice(new, &all_caps()).map_err(|_| invalid())?;
    let close = closing(state);
    let old = state
        .invoices
        .stored
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if old.record["status"] == "void" {
        return Err(invalid());
    }
    let old_date = old.record["txnDate"]
        .as_str()
        .and_then(|d| CalendarDate::parse_wire(d).ok())
        .ok_or(ApiError::Internal)?;
    if let Some(e) = expected
        && old.record["updatedAt"].as_str() != Some(e.to_wire().as_str())
    {
        return Err(ApiError::Conflict {
            reason: ConflictReason::Stale,
            current_version: None,
            code: cl_api::ErrorCode::parse("STALE"),
            message: UserText::default(),
        });
    }
    check_closing(close, &[old_date, new.txn_date], password)?;
    let taken: BTreeSet<JournalLineId> = state
        .invoices
        .claimed
        .difference(&old.claims.iter().copied().collect())
        .copied()
        .collect();
    if new
        .lines
        .iter()
        .filter_map(|l| l.billable_source_line_id)
        .any(|s| taken.contains(&s))
    {
        return Err(conflict(cl_api::BILLABLE_ALREADY_BILLED));
    }
    let number = new.txn_number.clone().unwrap_or_else(|| {
        old.record["txnNumber"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    let (mut record, claims) = record_json(state, id, &number, new)?;
    for k in ["sendHistory", "sentAt", "invoiceStatus"] {
        record[k] = old.record[k].clone();
    }
    state.invoices.writes += 1;
    record["updatedAt"] = json!(stamp(state));
    let saved = versioned(&record)?;
    for c in &old.claims {
        state.invoices.claimed.remove(c);
    }
    state.invoices.claimed.extend(claims.iter().copied());
    state.invoices.stored.insert(id, Stored { record, claims });
    Ok(saved)
}

/// Every live invoice, newest first (Prev/Next).
#[must_use]
pub fn list(state: &DemoState) -> Vec<TransactionId> {
    state.invoices.stored.keys().rev().copied().collect()
}

/// `POST /invoices/{id}/email`: sends (the demo has no network: it records
/// the send) unless the demo has no email server.
///
/// # Errors
/// `SMTP_NOT_CONFIGURED` as the engine's 409; `NotFound`.
pub fn email(
    state: &mut DemoState,
    id: TransactionId,
    to: &[String],
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    if state.invoices.no_smtp {
        return Err(conflict(cl_api::SMTP_NOT_CONFIGURED));
    }
    record_sent(
        state,
        id,
        &SendRecord {
            method: SendMethod::Email,
            recipients: to.to_vec(),
            sent_at: None,
        },
    )
}

/// `POST /invoices/{id}/email-later`: the message waits in the Outbox.
///
/// # Errors
/// `NotFound`; `Internal` when the item can't be built.
pub fn email_later(
    state: &mut DemoState,
    id: TransactionId,
    message: &cl_api::EmailMessage,
) -> Result<cl_api::OutboxItem, ApiError> {
    let number = state
        .invoices
        .stored
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?
        .record["txnNumber"]
        .clone();
    state.invoices.writes += 1;
    let n = state.invoices.writes;
    let v = json!({
        "id": format!("d5000000-0000-4000-8000-{n:012}"),
        "transactionId": id, "txnNumber": number,
        "to": message.to, "cc": message.cc, "bcc": message.bcc,
        "subject": message.subject, "body": message.body,
        "createdAt": DemoState::now(),
    });
    let item = serde_json::from_value(v.clone()).map_err(|_| ApiError::Internal)?;
    state.invoices.outbox.push(v);
    Ok(item)
}

/// `GET /forms/email-queue`.
#[must_use]
pub fn outbox(state: &DemoState) -> Vec<cl_api::OutboxItem> {
    state
        .invoices
        .outbox
        .iter()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}

/// `DELETE /forms/email-queue/{id}`.
pub fn remove_from_outbox(state: &mut DemoState, id: cl_api::OutboxId) {
    let id = id.to_string();
    state
        .invoices
        .outbox
        .retain(|v| v["id"].as_str() != Some(id.as_str()));
}

/// The client's sales figures (U-38 open balance from the demo invoices,
/// U-40 level, U-128 markup).
#[must_use]
pub fn client_sales(state: &DemoState, client: cl_core::ids::ContactId) -> cl_api::ClientSales {
    let mut open = Decimal::ZERO;
    let mut oldest: Option<CalendarDate> = None;
    for s in state.invoices.stored.values() {
        if s.record["contactId"].as_str() != Some(client.to_string().as_str())
            || s.record["status"] == "void"
        {
            continue;
        }
        let due = s.record["balanceDue"]
            .as_str()
            .and_then(|b| Decimal::from_str(b).ok())
            .unwrap_or_default();
        if due.is_zero() {
            continue;
        }
        open += due;
        let d = s.record["txnDate"]
            .as_str()
            .and_then(|d| CalendarDate::parse_wire(d).ok());
        oldest = match (oldest, d) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    cl_api::ClientSales {
        open_balance: money(open).ok(),
        oldest_open_date: oldest,
        price_level_id: state.price_levels.clients.get(&client).copied(),
        default_markup_percent: state.invoices.markups.get(&client).copied(),
    }
}

#[cfg(test)]
#[path = "invoices_tests.rs"]
mod tests;
