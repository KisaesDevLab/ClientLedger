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
        details: cl_api::ConflictDetails::default(),
    }
}

/// The engine's 400 with `code` (and `details`), mapped as cl-api maps it.
fn refused(code: &str, details: &Value) -> ApiError {
    cl_api::engine_refusal(
        400,
        &json!({"error": {"message": "refused", "code": code, "details": details}}),
    )
}

/// The billable line rules the engine checks before anything else
/// (`invoice.service.ts` resolveInvoiceLines, U-128): item, quantity, price
/// and discount come from the cost (400 `BILLABLE_FIELDS_READONLY`); the
/// income account is required (400 `BILLABLE_INCOME_ACCOUNT_REQUIRED`); a
/// cost once (400 `BILLABLE_DUPLICATE_LINE`); markup 0 to 1000.
fn billable_rules(new: &NewInvoice) -> Result<(), ApiError> {
    let mut seen = BTreeSet::new();
    for (i, l) in new.lines.iter().enumerate() {
        let Some(src) = l.billable_source_line_id else {
            continue;
        };
        if l.item_id.is_some()
            || l.quantity.is_some()
            || l.unit_price.is_some()
            || l.discount_percent.is_some()
        {
            return Err(refused("BILLABLE_FIELDS_READONLY", &json!({"line": i})));
        }
        if l.account_id.is_none() {
            return Err(refused(
                "BILLABLE_INCOME_ACCOUNT_REQUIRED",
                &json!({"line": i}),
            ));
        }
        if !seen.insert(src) {
            return Err(refused(
                "BILLABLE_DUPLICATE_LINE",
                &json!({"sourceLineId": src}),
            ));
        }
        if l.markup_percent
            .is_some_and(cl_api::SignedPercent::is_negative)
        {
            return Err(refused("VALIDATION_ERROR", &json!({})));
        }
    }
    Ok(())
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
    let positive = amounts.iter().copied().sum::<Decimal>() > Decimal::ZERO;
    // The negative group bases floored to 0.00 (they leave the taxable base).
    let mut floored = Decimal::ZERO;
    for (rate, idx) in &groups {
        let base: Decimal = idx.iter().map(|i| amounts[*i]).sum();
        // Q-P20ag/Q-P20aj: on a positive document the tax base floors at
        // 0.00 (a discount can't make tax negative); a wholly negative
        // document reverses its tax.
        if positive && base.is_sign_negative() && !base.is_zero() {
            floored += base;
            continue;
        }
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
        .sum::<Decimal>()
        - floored;
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
    /// When it was made (the engine's `createdAt` tiebreaker in the list).
    made: u64,
}

/// Invoices the demo engine keeps.
#[derive(Clone, Default)]
pub struct DemoInvoices {
    stored: BTreeMap<TransactionId, Stored>,
    claimed: BTreeSet<JournalLineId>,
    next: u64,
    /// Invoices made so far, seeded or created (each one's `made`).
    made: u64,
    /// Write requests that reached the demo engine (tests).
    pub writes: u32,
    /// The Outbox (Q-P20q), oldest first (WP-B).
    outbox: Vec<Value>,
    /// Each client's default markup percent (U-128, WP-B).
    pub markups: BTreeMap<cl_core::ids::ContactId, cl_api::SignedPercent>,
    /// The demo company has no email server (WP-B): sending then fails
    /// with `SMTP_NOT_CONFIGURED`, as an engine without one.
    pub no_smtp: bool,
    /// The account markups post to (company `billableMarkupAccountId`);
    /// `None`: the demo's income account 4000.
    pub markup_account: Option<AccountId>,
}

impl DemoInvoices {
    /// The next invoice's `made`.
    fn made_one(&mut self) -> u64 {
        self.made += 1;
        self.made
    }

    fn markup_account(&self) -> AccountId {
        self.markup_account
            .unwrap_or_else(|| super::account_id(4000))
    }
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
    /// The quantity stored (`None` on subtotal, discount and billable
    /// lines whose source has none).
    quantity: Option<Decimal>,
    /// The unit price stored (`None` on a subtotal or a percent discount).
    price: Option<Decimal>,
    source: &'static str,
    account: Option<AccountId>,
    /// A subtotal line shows the sum of its group and posts nothing.
    display: Option<Decimal>,
    /// A discount line's percent.
    discount_percent: Option<Decimal>,
    /// A billable line: its source, figures and source quantity.
    billable: Option<(JournalLineId, super::billable::BillableFigures, Decimal)>,
}

impl Priced {
    /// A discount line (no quantity, not a subtotal, not a billable cost).
    fn is_discount(&self) -> bool {
        self.quantity.is_none() && self.display.is_none() && self.billable.is_none()
    }
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
        let quantity = match l.quantity {
            Some(q) => parse_quantity(&q.to_wire())?,
            None => Decimal::ONE,
        };
        if let Some(src) = l.billable_source_line_id {
            // Q-P20w: one line whose amount includes the markup; the rate
            // shown is amount ÷ quantity, the amount itself is exact. The
            // quantity is the source's (U-128).
            let (owner, cost) = super::billable::cost(state, src).ok_or_else(|| {
                cl_api::engine_refusal(
                    409,
                    &json!({"error": {"message": "x", "code": "BILLABLE_COST_UNAVAILABLE"}}),
                )
            })?;
            if owner != new.contact_id {
                return Err(refused(
                    "BILLABLE_CONTACT_MISMATCH",
                    &json!({"sourceLineId": src}),
                ));
            }
            let q = cost
                .quantity
                .map_or(Decimal::ONE, cl_core::Quantity::as_decimal);
            let markup = l.markup_percent.map(cl_api::SignedPercent::as_decimal);
            let fig = super::billable::figures(cost.cost_amount.as_decimal(), q, markup);
            out.push(Priced {
                input: SalesLineIn {
                    quantity: Decimal::ONE,
                    unit_price: fig.amount,
                    taxable: l.is_taxable.unwrap_or(true),
                    tax_rate: None,
                },
                // The source's quantity, 1 when it had none (as the
                // saved invoice's `billableCosts[]`).
                quantity: Some(q),
                price: None,
                source: "billable_cost",
                account,
                display: None,
                discount_percent: None,
                billable: Some((src, fig, q)),
            });
            continue;
        }
        let taxable = l
            .is_taxable
            .unwrap_or(item.is_some_and(|i| i.fields.is_taxable));
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
                quantity: None,
                price: None,
                source: "none",
                account,
                display: Some(shown),
                discount_percent: None,
                billable: None,
            });
            prev_group = Some(group_start);
            group_start = out.len();
            continue;
        }
        if kind == cl_api::ItemType::Discount {
            // Under a subtotal a discount covers that subtotal's group;
            // discounts stacked there compound (100 → −10.00 → −4.50).
            let since = &out[group_start..];
            let stacked = !since.is_empty() && since.iter().all(Priced::is_discount);
            let from = if out.last().is_some_and(|p| p.display.is_some()) || stacked {
                prev_group.unwrap_or(0)
            } else {
                group_start
            };
            let base = group_sum(&out[from..]);
            // The line's own percent (as stored), else its amount off, else
            // the item's (U-129).
            let percent = l
                .discount_percent
                .map(cl_api::SignedPercent::as_decimal)
                .or_else(|| {
                    l.unit_price
                        .is_none()
                        .then(|| item.and_then(|i| i.fields.discount_percent))
                        .flatten()
                        .map(cl_api::SignedPercent::as_decimal)
                });
            let typed = l.discount_percent.is_some() || l.unit_price.is_some();
            let amount = match (percent, l.unit_price) {
                // A % of a group at or below 0.00 is 0.00.
                (Some(_), _) if base <= Decimal::ZERO => Decimal::ZERO,
                (Some(p), _) => -round(base * p / Decimal::ONE_HUNDRED, 2),
                (None, Some(off)) => -round(off.as_decimal().abs(), 2),
                (None, None) => -round(
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
                quantity: None,
                price: percent.is_none().then_some(amount),
                source: if typed { "input" } else { "item" },
                account,
                display: None,
                discount_percent: percent,
                billable: None,
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
                // A line's own rate (the P22 golden cases' 8.25 %; the
                // desktop never sends one, so the demo's default).
                tax_rate: l.tax_rate.map(cl_core::Rate::as_decimal),
            },
            quantity: Some(quantity),
            price: Some(price),
            source,
            account,
            display: None,
            discount_percent: None,
            billable: None,
        });
    }
    Ok(out)
}

/// The invoice that already bills `src` (not `editing`), with its number.
fn held_by(
    state: &DemoState,
    src: JournalLineId,
    editing: Option<TransactionId>,
) -> Option<(TransactionId, String)> {
    state
        .invoices
        .stored
        .iter()
        .find(|(id, s)| Some(**id) != editing && s.claims.contains(&src))
        .map(|(id, s)| {
            (
                *id,
                s.record["txnNumber"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
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

/// `POST /invoices/preview` as the demo engine answers it (`editing`: the
/// invoice an edit previews, whose own total leaves the client's balance
/// first and whose costs are its own).
///
/// # Errors
/// The billable line refusals; `Local(InvalidValue)` for what the engine
/// refuses (no lines, unknown item, a line with neither account nor item,
/// bad decimals).
pub fn preview(
    state: &mut DemoState,
    new: &NewInvoice,
    editing: Option<TransactionId>,
) -> Result<InvoicePreview, ApiError> {
    billable_rules(new)?;
    let priced = price_lines(state, new)?;
    let ins: Vec<SalesLineIn> = priced.iter().map(|p| p.input).collect();
    let doc = compute(&ins, default_rate())?;
    let terms = new.payment_terms.as_deref().unwrap_or(DEMO_TERMS);
    let due = new
        .due_date
        .or_else(|| due_of(terms, new.terms_days, new.txn_date));
    let s = |d: Option<Decimal>| d.map(|d| d.to_string());
    let lines: Vec<Value> = priced
        .iter()
        .zip(&doc.lines)
        .map(|(p, o)| {
            let mut v = json!({
                "quantity": s(p.quantity),
                "unitPrice": s(p.billable.map(|(_, f, _)| f.rate).or(p.price)),
                "unitPriceSource": p.source,
                "discountPercent": s(p.discount_percent),
                "amount": p.display.unwrap_or(o.amount).to_string(),
                "taxAmount": o.tax_amount.to_string(),
            });
            if let Some((src, f, _)) = p.billable {
                let error = held_by(state, src, editing).map(|(id, n)| {
                    json!({"code": cl_api::BILLABLE_ALREADY_BILLED, "invoiceId": id, "invoiceNumber": n})
                });
                v["billableSourceLineId"] = json!(src);
                v["costAmount"] = json!(f.cost.to_string());
                v["markupAmount"] = json!(f.markup.to_string());
                v["displayRate"] = json!(f.rate.to_string());
                v["error"] = error.unwrap_or(Value::Null);
            }
            v
        })
        .collect();
    // The client's balance (U-38) with this invoice's total; an edit's own
    // posted total leaves it first.
    let before = client_sales(state, new.contact_id)
        .open_balance
        .map_or(Decimal::ZERO, Money::as_decimal);
    let own = editing
        .and_then(|id| state.invoices.stored.get(&id))
        .filter(|s| {
            s.record["status"] != "void"
                && s.record["contactId"].as_str() == Some(new.contact_id.to_string().as_str())
        })
        .and_then(|s| s.record["balanceDue"].as_str())
        .and_then(|b| Decimal::from_str(b).ok())
        .unwrap_or_default();
    let v = json!({
        "lines": lines,
        "subtotal": doc.subtotal.to_string(),
        "taxableBase": doc.taxable_base.to_string(),
        "taxRate": default_rate().to_string(),
        "taxTotal": doc.tax_total.to_string(),
        "total": doc.total.to_string(),
        "dueDate": due,
        "contactBalanceBefore": round(before - own, 2).to_string(),
        "contactBalanceAfter": round(before - own + doc.total, 2).to_string(),
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
    // The engine's journal (`buildInvoiceJournal`): A/R, then each line
    // (a subtotal posts $0 with no quantity or price; a discount keeps its
    // percent; a billable cost posts its cost line and, when not zero, a
    // markup line to the markup account), then sales tax.
    let mut lines = vec![json!({
        "id": line_id(0), "accountId": AR, "debit": m4(doc.total)?, "credit": m4(Decimal::ZERO)?,
        "lineOrder": 0, "quantity": null, "unitPriceSource": null, "lineRole": null,
    })];
    let mut claims = Vec::new();
    let mut costs = Vec::new();
    let markup_account = state.invoices.markup_account();
    let opt = |d: Option<Decimal>| -> Result<Option<String>, ApiError> {
        d.map(|d| money(d).map(Money::to_wire)).transpose()
    };
    for ((p, o), l) in priced.iter().zip(&doc.lines).zip(&new.lines) {
        let n = lines.len();
        let posted = if p.display.is_some() {
            Decimal::ZERO
        } else {
            p.billable.map_or(o.amount, |(_, f, _)| f.cost)
        };
        let (debit, credit) = side(posted)?;
        let mut line = json!({
            "id": line_id(n), "accountId": p.account, "debit": debit, "credit": credit,
            "description": l.description, "lineOrder": n, "tagId": l.tag_id,
            "itemId": if p.billable.is_some() { None } else { l.item_id },
            "quantity": opt(p.quantity)?, "unitPrice": opt(p.price)?,
            "discountPercent": p.discount_percent.map(|d| round(d, 4).to_string()),
            "unitPriceSource": p.source,
            "isTaxable": p.input.taxable, "taxRate": round(o.tax_rate, 4).to_string(),
            "taxAmount": m4(o.tax_amount)?, "serviceDate": l.service_date,
        });
        if let Some((src, f, q)) = p.billable {
            claims.push(src);
            line["lineRole"] = json!("billable_cost");
            line["billableSourceLineId"] = json!(src);
            lines.push(line);
            if !f.markup.is_zero() {
                let (debit, credit) = side(f.markup)?;
                let n = lines.len();
                lines.push(json!({
                    "id": line_id(n), "accountId": markup_account, "debit": debit, "credit": credit,
                    "description": l.description, "lineOrder": n, "unitPriceSource": "billable_cost",
                    "lineRole": "billable_markup", "billableSourceLineId": src,
                    "isTaxable": p.input.taxable, "taxAmount": m4(Decimal::ZERO)?,
                }));
            }
            let source = super::billable::cost(state, src).map(|(_, c)| c.source_transaction_id);
            costs.push(json!({
                "sourceLineId": src, "sourceTransactionId": source,
                "costAmount": round(f.cost, 2).to_string(),
                "markupPercent": l.markup_percent.map_or_else(|| "0.0000".to_string(), |m| round(m.as_decimal(), 4).to_string()),
                "markupAmount": round(f.markup, 2).to_string(), "amount": round(f.amount, 2).to_string(),
                "quantity": q.to_string(), "displayRate": f.rate.to_string(),
                "incomeAccountId": p.account,
                "markupAccountId": (!f.markup.is_zero()).then_some(markup_account),
                "billed": true,
            }));
            continue;
        }
        lines.push(line);
    }
    if !doc.tax_total.is_zero() {
        let (debit, credit) = side(doc.tax_total)?;
        let n = lines.len();
        lines.push(json!({
            "id": line_id(n), "accountId": SALES_TAX, "debit": debit,
            "credit": credit, "description": "Sales Tax", "lineOrder": n,
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
        "updatedAt": DemoState::now(), "lines": lines, "billableCosts": costs,
        "layoutId": new.layout_id.id(),
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
    billable_rules(new)?;
    if !new.release_billable_source_line_ids.is_empty() {
        return Err(refused("VALIDATION_ERROR", &json!({})));
    }
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
    let (mut record, claims) = record_json(state, id, &number, new)?;
    // U-130: no layout sent starts on the client's default layout.
    if new.layout_id.is_omitted() {
        record["layoutId"] = json!(state.layouts.clients.get(&new.contact_id));
    }
    let saved = versioned(&record)?;
    state.invoices.writes += 1;
    state.invoices.claimed.extend(claims.iter().copied());
    let made = state.invoices.made_one();
    state.invoices.stored.insert(
        id,
        Stored {
            record,
            claims,
            made,
        },
    );
    Ok(saved)
}

/// `GET /document-numbers/next?type=invoice`: what the next invoice gets.
#[must_use]
pub fn next_number(state: &DemoState) -> String {
    (1041 + state.invoices.next + 1).to_string()
}

/// Takes the next invoice number (P22 finance-charge invoices).
pub fn take_number(state: &mut DemoState) -> String {
    let n = next_number(state);
    state.invoices.next += 1;
    n
}

/// Puts a message in the Outbox (P22 statements; `item` is the engine's
/// row JSON).
pub fn push_outbox(state: &mut DemoState, item: Value) {
    state.invoices.outbox.push(item);
}

/// Marks an invoice as a finance-charge invoice: due on receipt and
/// waiting to be printed when `print_later` (P22, U-141).
pub fn mark_finance_charge(state: &mut DemoState, id: TransactionId, print_later: bool) {
    if let Some(s) = state.invoices.stored.get_mut(&id) {
        s.record["isFinanceCharge"] = json!(true);
        s.record["paymentTerms"] = json!("due_on_receipt");
        s.record["printLater"] = json!(print_later);
    }
}

/// A typed number another invoice already has (Q-94 warning).
#[must_use]
pub fn number_in_use(
    state: &DemoState,
    number: Option<&str>,
    except: Option<TransactionId>,
) -> bool {
    let Some(n) = number.map(str::trim).filter(|n| !n.is_empty()) else {
        return false;
    };
    state
        .invoices
        .stored
        .iter()
        .any(|(id, s)| Some(*id) != except && s.record["txnNumber"].as_str() == Some(n))
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
        cl_api::Capabilities::DOCUMENT_NUMBERS,
        cl_api::Capabilities::INVOICE_LAYOUTS,
    ]
    .into_iter()
    .collect()
}

/// [`all_caps`] and the Receive Payment capabilities the demo engine plays
/// (P21, U-132 to U-136).
#[must_use]
pub fn all_caps_with_payments() -> cl_api::Capabilities {
    let mut v: Vec<&str> = vec![
        cl_api::Capabilities::RECEIVE_PAYMENT,
        cl_api::Capabilities::IDEMPOTENCY_RECEIVE_PAYMENT,
        cl_api::Capabilities::PAYMENT_BOUNCE,
        cl_api::Capabilities::PAYMENT_SETTLE,
        cl_api::Capabilities::PAYMENT_DISCOUNTS,
        cl_api::Capabilities::CUSTOMER_DEPOSITS,
        cl_api::Capabilities::FEED_CUSTOMER_PAYMENTS,
    ];
    let base = all_caps();
    v.extend(
        [
            cl_api::Capabilities::SALES_CENTS,
            cl_api::Capabilities::INVOICE_EDIT,
        ]
        .into_iter()
        .filter(|c| base.has(c)),
    );
    v.into_iter().collect()
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
    let date = state
        .invoices
        .stored
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?
        .record["txnDate"]
        .as_str()
        .and_then(|d| CalendarDate::parse_wire(d).ok())
        .ok_or(ApiError::Internal)?;
    check_closing(close, &[date], password)?;
    // The engine refuses a void while payments or credits are applied
    // (PR-3 S2): unapply or void those first.
    let (payments, credits) = live_on_invoice(state, id);
    if payments + credits > 0 {
        return Err(super::receivables::conflict_with(
            "INVOICE_HAS_PAYMENTS",
            &json!({"payments": payments, "credits": credits}),
        ));
    }
    let s = state
        .invoices
        .stored
        .get_mut(&id)
        .ok_or(ApiError::NotFound { code: None })?;
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
    billable_rules(new)?;
    cl_api::check_invoice(new, &all_caps()).map_err(|_| invalid())?;
    let close = closing(state);
    let old = state
        .invoices
        .stored
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if old.record["status"] == "void" || old.record["invoiceStatus"] == "paid" {
        return Err(refused("VALIDATION_ERROR", &json!({})));
    }
    // U-145, U-146: the invoice keeps its client while anything points at
    // it (a finance-charge invoice always).
    if old.record["contactId"].as_str() != Some(new.contact_id.to_string().as_str()) {
        client_change_allowed(state, id, &old.record)?;
    }
    // U-128: every cost the invoice bills is resent or released (409
    // BILLABLE_LINES_MISSING otherwise); not both.
    let sent: BTreeSet<JournalLineId> = new
        .lines
        .iter()
        .filter_map(|l| l.billable_source_line_id)
        .collect();
    let release: BTreeSet<JournalLineId> = new
        .release_billable_source_line_ids
        .iter()
        .copied()
        .collect();
    if sent.intersection(&release).next().is_some() {
        return Err(refused("VALIDATION_ERROR", &json!({})));
    }
    if old
        .claims
        .iter()
        .any(|c| !sent.contains(c) && !release.contains(c))
    {
        return Err(conflict("BILLABLE_LINES_MISSING"));
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
            details: cl_api::ConflictDetails::default(),
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
    // A finance-charge invoice can be edited like any other (P22 plan
    // §2.7: the engine allows it and its assessment's sources stay
    // charged); only a void frees the period.
    for k in ["sendHistory", "sentAt", "invoiceStatus", "isFinanceCharge"] {
        if !old.record[k].is_null() {
            record[k] = old.record[k].clone();
        }
    }
    // What was paid stays paid: the balance is the new total less it (409
    // INVOICE_TOTAL_BELOW_PAID below it); an edit down to exactly the
    // amount paid makes the invoice paid (U-146 N2).
    let paid = old.record["amountPaid"]
        .as_str()
        .and_then(|v| Decimal::from_str(v).ok())
        .unwrap_or_default();
    if !paid.is_zero() {
        let total = record["total"]
            .as_str()
            .and_then(|v| Decimal::from_str(v).ok())
            .ok_or(ApiError::Internal)?;
        if total < paid {
            return Err(conflict("INVOICE_TOTAL_BELOW_PAID"));
        }
        let balance = total - paid;
        record["amountPaid"] = json!(money(paid)?.to_wire());
        record["balanceDue"] = json!(money(balance)?.to_wire());
        record["invoiceStatus"] = json!(if balance.is_zero() { "paid" } else { "partial" });
    }
    // Omitted fields keep their values on an edit (layout, print later).
    if new.layout_id.is_omitted() {
        record["layoutId"] = old.record["layoutId"].clone();
    }
    if new.print_later.is_none() {
        record["printLater"] = old.record["printLater"].clone();
    }
    state.invoices.writes += 1;
    record["updatedAt"] = json!(stamp(state));
    let saved = versioned(&record)?;
    for c in &old.claims {
        state.invoices.claimed.remove(c);
    }
    state.invoices.claimed.extend(claims.iter().copied());
    state.invoices.stored.insert(
        id,
        Stored {
            record,
            claims,
            made: old.made,
        },
    );
    Ok(saved)
}

/// Every live invoice, newest first (Prev/Next): as the engine's list,
/// by date and then by when it was made, both descending. Not by id: the
/// seeded invoices' ids carry their numbers (1091, 1201, ...) and would
/// come before every invoice the owner creates.
#[must_use]
pub fn list(state: &DemoState) -> Vec<TransactionId> {
    let mut v: Vec<(String, u64, TransactionId)> = state
        .invoices
        .stored
        .iter()
        .map(|(id, s)| {
            let date = s.record["txnDate"].as_str().unwrap_or_default().to_string();
            (date, s.made, *id)
        })
        .collect();
    v.sort();
    v.into_iter().rev().map(|(_, _, id)| id).collect()
}

/// `POST /invoices/{id}/email`: sends (the demo has no network: it records
/// the send, to + cc + bcc, as the engine does) unless the demo has no
/// email server. With `outbox_id` the Outbox message goes in the same
/// commit (404 `OUTBOX_ITEM_NOT_FOUND` when it is already gone).
///
/// # Errors
/// `SMTP_NOT_CONFIGURED` as the engine's 400; `NotFound`.
pub fn email(
    state: &mut DemoState,
    id: TransactionId,
    message: &cl_api::EmailMessage,
    outbox_id: Option<cl_api::OutboxId>,
) -> Result<Versioned<InvoiceRecord>, ApiError> {
    if let Some(o) = outbox_id
        && !state
            .invoices
            .outbox
            .iter()
            .any(|v| v["id"].as_str() == Some(o.to_string().as_str()))
    {
        return Err(ApiError::NotFound {
            code: cl_api::ErrorCode::parse("OUTBOX_ITEM_NOT_FOUND"),
        });
    }
    if state.invoices.no_smtp {
        return Err(refused(cl_api::SMTP_NOT_CONFIGURED, &json!({})));
    }
    let recipients: Vec<String> = message
        .to
        .iter()
        .chain(&message.cc)
        .chain(&message.bcc)
        .cloned()
        .collect();
    let saved = record_sent(
        state,
        id,
        &SendRecord {
            method: SendMethod::Email,
            recipients,
            sent_at: None,
        },
    )?;
    if let Some(o) = outbox_id {
        let _ = remove_from_outbox(state, o);
    }
    Ok(saved)
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

/// `DELETE /forms/email-queue/{id}`: 204, or 404 `OUTBOX_ITEM_NOT_FOUND`
/// when it is already gone.
///
/// # Errors
/// `NotFound`.
pub fn remove_from_outbox(state: &mut DemoState, id: cl_api::OutboxId) -> Result<(), ApiError> {
    let id = id.to_string();
    let before = state.invoices.outbox.len();
    state
        .invoices
        .outbox
        .retain(|v| v["id"].as_str() != Some(id.as_str()));
    if state.invoices.outbox.len() == before {
        return Err(ApiError::NotFound {
            code: cl_api::ErrorCode::parse("OUTBOX_ITEM_NOT_FOUND"),
        });
    }
    Ok(())
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
        default_invoice_layout_id: state.layouts.clients.get(&client).copied(),
    }
}

// ---- P21: the receivables side (company/demo/receivables.rs) ----------------

/// One invoice as Receive Payment sees it (P21).
#[derive(Clone, PartialEq, Eq)]
pub struct Receivable {
    /// The invoice.
    pub id: TransactionId,
    /// Its number.
    pub number: String,
    /// Its date.
    pub date: CalendarDate,
    /// When it is due.
    pub due: Option<CalendarDate>,
    /// Its total.
    pub total: Decimal,
    /// What is still owed (stored).
    pub balance: Decimal,
    /// Its client.
    pub client: cl_core::ids::ContactId,
    /// It was sent (a restore reopens it as sent, not draft).
    pub sent: bool,
}

impl fmt::Debug for Receivable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Receivable")
            .field("id", &self.id)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

fn receivable_of(id: TransactionId, s: &Stored) -> Option<Receivable> {
    let r = &s.record;
    if r["status"] == "void" {
        return None;
    }
    let dec = |k: &str| r[k].as_str().and_then(|v| Decimal::from_str(v).ok());
    let date = |k: &str| r[k].as_str().and_then(|v| CalendarDate::parse_wire(v).ok());
    Some(Receivable {
        id,
        number: r["txnNumber"].as_str().unwrap_or_default().to_string(),
        date: date("txnDate")?,
        due: date("dueDate"),
        total: dec("total")?,
        balance: dec("balanceDue")?,
        client: r["contactId"]
            .as_str()
            .and_then(|c| cl_core::ids::ContactId::parse(c).ok())?,
        sent: r["sentAt"].is_string() || r["sendHistory"].as_array().is_some_and(|h| !h.is_empty()),
    })
}

/// A client's live invoices (any balance), by date, number, id.
#[must_use]
pub fn receivables_of(state: &DemoState, client: cl_core::ids::ContactId) -> Vec<Receivable> {
    let mut v: Vec<Receivable> = state
        .invoices
        .stored
        .iter()
        .filter_map(|(id, s)| receivable_of(*id, s))
        .filter(|r| r.client == client)
        .collect();
    v.sort_by(|a, b| {
        a.date
            .cmp(&b.date)
            .then_with(|| a.number.cmp(&b.number))
            .then_with(|| a.id.cmp(&b.id))
    });
    v
}

/// One live invoice.
#[must_use]
pub fn receivable(state: &DemoState, id: TransactionId) -> Option<Receivable> {
    state
        .invoices
        .stored
        .get(&id)
        .and_then(|s| receivable_of(id, s))
}

/// Settles `paid` (cash + discount + write-off; negative restores) on an
/// invoice: `balanceDue` and `amountPaid` move by it and the status is
/// recomputed (P21 plan §2.1: paid at 0.00, partial while anything is
/// applied, else sent or draft).
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` when it would go below 0.00 or above
/// the total.
pub fn settle(state: &mut DemoState, id: TransactionId, paid: Decimal) -> Result<(), ApiError> {
    let s = state
        .invoices
        .stored
        .get_mut(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    let now = receivable_of(id, s).ok_or(ApiError::NotFound { code: None })?;
    let balance = now.balance - paid;
    let amount_paid = now.total - balance;
    if balance.is_sign_negative() && !balance.is_zero() || balance > now.total {
        return Err(invalid());
    }
    let status = if balance.is_zero() {
        "paid"
    } else if !amount_paid.is_zero() {
        "partial"
    } else if now.sent {
        "sent"
    } else {
        "draft"
    };
    s.record["balanceDue"] = json!(money(balance)?.to_wire());
    s.record["amountPaid"] = json!(money(amount_paid)?.to_wire());
    s.record["invoiceStatus"] = json!(status);
    s.record["updatedAt"] = json!(DemoState::now());
    Ok(())
}

/// Puts a sent, single-line invoice in the store under `id` without moving
/// the Create Invoice numbering (the Receive Payment story's invoices and
/// the golden cases).
///
/// # Errors
/// `Local(InvalidValue)` for a total the demo can't price.
pub fn seed_receivable(
    state: &mut DemoState,
    id: TransactionId,
    client: cl_core::ids::ContactId,
    number: &str,
    date: CalendarDate,
    due: CalendarDate,
    total: Money,
) -> Result<(), ApiError> {
    let one = cl_core::Quantity::parse_wire("1").map_err(|_| invalid())?;
    let mut line = cl_api::InvoiceLine::account(super::account_id(4000), one, total);
    line.is_taxable = Some(false);
    let mut new = NewInvoice::new(client, date, vec![line]);
    new.due_date = Some(due);
    new.txn_number = Some(number.to_string());
    let (mut record, claims) = record_json(state, id, number, &new)?;
    record["invoiceStatus"] = json!("sent");
    record["sentAt"] = json!(DemoState::now());
    let made = state.invoices.made_one();
    state.invoices.stored.insert(
        id,
        Stored {
            record,
            claims,
            made,
        },
    );
    Ok(())
}

#[cfg(test)]
#[path = "invoices_tests.rs"]
mod tests;

/// The live payment and credit applications on an invoice, as the
/// engine's `assertInvoiceUnapplied` counts them (`{payments, credits}`):
/// payment rows and credit memo rows count as payments; the demo has no
/// legacy refund pointers, so credits stay 0.
#[must_use]
pub fn live_on_invoice(state: &DemoState, id: TransactionId) -> (usize, usize) {
    let payments = super::receivables::live_rows_on(state, id)
        + super::credit_memos::live_memo_rows_on(state, id);
    (payments, 0)
}

/// A saved invoice's client may change (U-145, U-146): a finance-charge
/// invoice never (409 FINANCE_CHARGE_CLIENT_CHANGE); any other not while
/// a payment or credit is applied (409 INVOICE_HAS_PAYMENTS).
fn client_change_allowed(
    state: &DemoState,
    id: TransactionId,
    old: &Value,
) -> Result<(), ApiError> {
    if old["isFinanceCharge"] == true {
        return Err(super::receivables::conflict_with(
            "FINANCE_CHARGE_CLIENT_CHANGE",
            &json!({}),
        ));
    }
    let (payments, credits) = live_on_invoice(state, id);
    if payments + credits > 0 {
        return Err(super::receivables::conflict_with(
            "INVOICE_HAS_PAYMENTS",
            &json!({"payments": payments, "credits": credits}),
        ));
    }
    Ok(())
}
