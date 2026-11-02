//! The demo "engine" for what sales tax is owed and paid (P23 WP-3, U-149,
//! U-150, U-151, U-152, U-153, U-154): the liability report on both bases,
//! what is owed, Pay Sales Tax with adjustments, linked outside payments,
//! voids, exemption certificates and bad-debt recoveries. Held to the
//! non-engineOnly parts of goldens 7-10 and 12-14 in
//! `fixtures/sales_tax/golden.json`.
//!
//! Rules (plan §1, §2.5-2.8):
//! - **Accrual:** sale rows by document date; with Streamlined Sales Tax
//!   on, each settlement of an invoice takes its running-total share over
//!   cash + discount + write-off, split by largest remainder; the discount
//!   and write-off parts are discount and bad-debt rows. Recoveries add
//!   back `round2(w × X_cum / W)` less what earlier recoveries took.
//! - **Cash (Q-SPF1, Q-SPF3, Q-P23e, Q-P23y):** receipts and daily sales on
//!   their date; an invoice's tax and base as each settlement's
//!   running-total share over cash + discount (never write-offs), on the
//!   date the money was applied (a credit applied later counts at its apply
//!   date, a deposit at its draw date, a bounce as a negative mirror).
//!   Credit memos settle invoices, never cash. Bad debts and recoveries
//!   never show.
//! - **Owed** = cumulative columns + adjustments − paid; Pay's owed counts
//!   rows dated through the date and every live payment line whatever its
//!   date (S1).
//!
//! The demo derives the SSUTA shares when the report is read (the engine
//! stores them at posting): the switch therefore applies to every posting,
//! not only those made while it is on (Q-P23w; a listed demo limit). No
//! floats.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use cl_api::{
    ApiError, ExpiringExemption, LinkPaymentRequest, NewRecovery, PayPreview, PaySalesTaxRequest,
    PaySalesTaxResult, RecoveryResult, SalesTaxBasis, SalesTaxLiability, SalesTaxOwed,
    SalesTaxPayMethod, SalesTaxPaymentRecord, TaxExemption, TaxExemptionDraft,
};
use cl_core::ids::{
    AccountId, ContactId, SalesTaxAgencyId, SalesTaxItemId, TaxCodeId, TaxExemptionId,
    TransactionId,
};
use cl_core::{CalendarDate, Money, Secret};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::DemoState;
use super::invoices::{allocate, round};
use super::register::{DemoJournal, DemoLine, check_closing};
use super::sales_tax::{DocKind, TaxRow};

/// One payment the demo keeps (a Pay Sales Tax check or a linked outside
/// payment).
#[derive(Clone)]
struct TaxPayment {
    agency: SalesTaxAgencyId,
    date: CalendarDate,
    through: Option<CalendarDate>,
    method: SalesTaxPayMethod,
    bank: Option<AccountId>,
    total: Decimal,
    check_number: Option<String>,
    queued: bool,
    linked: bool,
    voided: bool,
    /// (item, kind `paid`/`adjustment`, amount, account, reason).
    lines: Vec<PaymentLine>,
    memo: Option<String>,
}

/// A stored payment line: (item, kind `paid`/`adjustment`, amount,
/// account, reason).
type PaymentLine = (
    SalesTaxItemId,
    &'static str,
    Decimal,
    Option<AccountId>,
    Option<String>,
);

/// A preview's line to post: (item, paid before adjustments, adjustment,
/// owed).
type FigureLine = (SalesTaxItemId, Decimal, Decimal, Decimal);

/// One recovery of a written-off invoice.
#[derive(Clone)]
struct Recovery {
    invoice: TransactionId,
    date: CalendarDate,
    amount: Decimal,
    /// Recovered tax per item (with SSUTA on).
    items: Vec<(SalesTaxItemId, Option<SalesTaxAgencyId>, Decimal)>,
}

/// The demo's payments, recoveries and certificates.
#[derive(Clone, Default)]
pub struct DemoTaxBook {
    payments: BTreeMap<TransactionId, TaxPayment>,
    keys: BTreeMap<String, Value>,
    recoveries: BTreeMap<TransactionId, Recovery>,
    exemptions: Vec<TaxExemption>,
    next: u32,
    /// Writes that reached the demo engine (tests).
    pub writes: u32,
}

impl fmt::Debug for DemoTaxBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoTaxBook")
            .field("payments", &self.payments.len())
            .field("recoveries", &self.recoveries.len())
            .field("exemptions", &self.exemptions.len())
            .finish_non_exhaustive()
    }
}

fn s2(d: Decimal) -> String {
    format!("{:.2}", round(d, 2))
}

fn refused(status: u16, code: &str, details: &Value) -> ApiError {
    cl_api::engine_refusal(
        status,
        &json!({"error": {"message": "refused", "code": code, "details": details}}),
    )
}

fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, ApiError> {
    serde_json::from_value(v).map_err(|_| ApiError::Internal)
}

// ---- Settlements ---------------------------------------------------------------------

/// One settlement of an invoice (a mirror of a bounce is negative).
#[derive(Clone, Copy)]
struct Settlement {
    date: CalendarDate,
    cash: Decimal,
    discount: Decimal,
    write_off: Decimal,
}

/// Each invoice's settlements in order (effective date, then the order
/// they were made): payments applied at save on the payment date, credits
/// applied later on their apply date, deposit draws on the draw date, and
/// a bounce as a negative mirror on its date.
fn settlements(state: &DemoState) -> BTreeMap<TransactionId, Vec<Settlement>> {
    let mut out: BTreeMap<TransactionId, Vec<(CalendarDate, usize, Settlement)>> = BTreeMap::new();
    let mut seq = 0usize;
    for p in super::receivables::pay_views(state) {
        if p.void {
            continue;
        }
        for a in p.apps.iter().filter(|a| !a.voided) {
            let s = Settlement {
                date: a.applied,
                cash: a.settled - a.adjustments,
                discount: a.discount,
                write_off: a.write_off,
            };
            seq += 1;
            out.entry(a.invoice).or_default().push((s.date, seq, s));
            if a.reversed
                && let Some(d) = p.bounce
            {
                seq += 1;
                out.entry(a.invoice).or_default().push((
                    d,
                    seq,
                    Settlement {
                        date: d,
                        cash: -s.cash,
                        discount: -s.discount,
                        write_off: -s.write_off,
                    },
                ));
            }
        }
    }
    out.into_iter()
        .map(|(k, mut v)| {
            v.sort_by_key(|(d, n, _)| (*d, *n));
            (k, v.into_iter().map(|(_, _, s)| s).collect())
        })
        .collect()
}

fn invoice_total(state: &DemoState, id: TransactionId) -> Option<Decimal> {
    let s = state.invoices.stored.get(&id)?;
    if s.record["status"] == "void" {
        return None;
    }
    s.record["total"]
        .as_str()
        .and_then(|t| Decimal::from_str(t).ok())
        .filter(|t| !t.is_zero())
}

/// `round2(x × p / total) − round2(x × q / total)`.
fn share(x: Decimal, p: Decimal, q: Decimal, total: Decimal) -> Decimal {
    round(x * p / total, 2) - round(x * q / total, 2)
}

/// The (discount, write-off) parts of a settlement's accrual share of
/// `tax` (SSUTA, plan §1): the running total over cash + discount +
/// write-off, split by largest remainder.
fn ssuta_split(tax: Decimal, s: Settlement, before: Decimal, total: Decimal) -> (Decimal, Decimal) {
    let all = s.cash + s.discount + s.write_off;
    if all.is_zero() || tax.is_zero() {
        return (Decimal::ZERO, Decimal::ZERO);
    }
    let k = share(tax, before + all, before, total);
    let parts: Vec<Decimal> = [s.cash, s.discount, s.write_off]
        .iter()
        .map(|x| k * x / all)
        .collect();
    if let Ok(v) = allocate(&parts, k) {
        (v[1], v[2])
    } else {
        // A negative mirror: the same split, negated.
        let neg: Vec<Decimal> = parts.iter().map(|x| -x).collect();
        allocate(&neg, -k).map_or((Decimal::ZERO, Decimal::ZERO), |v| (-v[1], -v[2]))
    }
}

// ---- Events --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Col {
    Collected,
    Discounts,
    BadDebts,
    Recoveries,
    Adjustments,
    Paid,
}

/// One figure on one item on one date.
struct Event {
    date: CalendarDate,
    item: Option<SalesTaxItemId>,
    agency: Option<SalesTaxAgencyId>,
    col: Col,
    tax: Decimal,
    /// The base (taxable when `code` is `None`); collected only.
    base: Decimal,
    code: Option<TaxCodeId>,
    /// The unrounded cash share (cash collected only).
    exact: Option<Decimal>,
}

fn row_event(r: &TaxRow, date: CalendarDate, tax: Decimal, base: Decimal) -> Event {
    Event {
        date,
        item: r.tax_item,
        agency: r.agency,
        col: Col::Collected,
        tax,
        base,
        code: r.code,
        exact: None,
    }
}

/// Every figure on `basis` (payments included).
fn events(state: &DemoState, basis: SalesTaxBasis) -> Vec<Event> {
    let st = &state.sales_tax;
    let ssuta = st.ssuta;
    let settled = settlements(state);
    let mut out = Vec::new();
    for (txn, rows) in &st.rows {
        let invoice = rows.first().is_some_and(|r| r.doc == DocKind::Invoice);
        let total = if invoice {
            invoice_total(state, *txn)
        } else {
            None
        };
        let empty = Vec::new();
        let sets = settled.get(txn).unwrap_or(&empty);
        for r in rows {
            let cash = basis == SalesTaxBasis::Cash;
            if !cash || matches!(r.doc, DocKind::Receipt | DocKind::Daily) {
                out.push(row_event(r, r.event_date, r.tax, r.base));
            } else if r.doc == DocKind::Invoice
                && let Some(total) = total
            {
                // Cash: the running-total share over cash + discount.
                let mut p = Decimal::ZERO;
                for s in sets {
                    let add = s.cash + s.discount;
                    if add.is_zero() {
                        continue;
                    }
                    let mut e = row_event(
                        r,
                        s.date,
                        share(r.tax, p + add, p, total),
                        share(r.base, p + add, p, total),
                    );
                    e.exact = Some(r.tax * add / total);
                    out.push(e);
                    p += add;
                }
            }
            // SSUTA discount (both bases) and bad-debt (accrual) parts.
            if ssuta
                && r.doc == DocKind::Invoice
                && r.code.is_none()
                && let Some(total) = total
            {
                let mut p = Decimal::ZERO;
                for s in sets {
                    let (d, w) = ssuta_split(r.tax, *s, p, total);
                    p += s.cash + s.discount + s.write_off;
                    for (col, x) in [(Col::Discounts, d), (Col::BadDebts, w)] {
                        if x.is_zero() || col == Col::BadDebts && cash {
                            continue;
                        }
                        out.push(Event {
                            date: s.date,
                            item: r.tax_item,
                            agency: r.agency,
                            col,
                            tax: -x,
                            base: Decimal::ZERO,
                            code: None,
                            exact: None,
                        });
                    }
                }
            }
        }
    }
    if basis == SalesTaxBasis::Accrual {
        for rec in st.book.recoveries.values() {
            for (item, agency, x) in &rec.items {
                out.push(Event {
                    date: rec.date,
                    item: Some(*item),
                    agency: *agency,
                    col: Col::Recoveries,
                    tax: *x,
                    base: Decimal::ZERO,
                    code: None,
                    exact: None,
                });
            }
        }
    }
    for p in st.book.payments.values().filter(|p| !p.voided) {
        for (item, kind, amount, _, _) in &p.lines {
            out.push(Event {
                date: p.date,
                item: Some(*item),
                agency: Some(p.agency),
                col: if *kind == "paid" {
                    Col::Paid
                } else {
                    Col::Adjustments
                },
                tax: *amount,
                base: Decimal::ZERO,
                code: None,
                exact: None,
            });
        }
    }
    out
}

/// One item's columns.
#[derive(Default, Clone)]
struct Cols {
    agency: Option<SalesTaxAgencyId>,
    taxable: Decimal,
    non: BTreeMap<TaxCodeId, Decimal>,
    collected: Decimal,
    discounts: Decimal,
    bad_debts: Decimal,
    recoveries: Decimal,
    adjustments: Decimal,
    paid: Decimal,
    owed: Decimal,
}

impl Cols {
    fn add(&mut self, e: &Event, in_range: bool) {
        let signed = if e.col == Col::Paid { -e.tax } else { e.tax };
        self.owed += signed;
        if !in_range {
            return;
        }
        match e.col {
            Col::Collected => {
                self.collected += e.tax;
                match e.code {
                    Some(c) => *self.non.entry(c).or_default() += e.base,
                    None => self.taxable += e.base,
                }
            }
            Col::Discounts => self.discounts += e.tax,
            Col::BadDebts => self.bad_debts += e.tax,
            Col::Recoveries => self.recoveries += e.tax,
            Col::Adjustments => self.adjustments += e.tax,
            Col::Paid => self.paid += e.tax,
        }
    }

    fn json(&self) -> Value {
        json!({
            "collected": s2(self.collected),
            "discounts": s2(self.discounts),
            "badDebts": s2(self.bad_debts),
            "recoveries": s2(self.recoveries),
            "adjustments": s2(self.adjustments),
            "paid": s2(self.paid),
            "owedAtEnd": s2(self.owed),
        })
    }

    fn plus(&mut self, o: &Cols) {
        self.collected += o.collected;
        self.discounts += o.discounts;
        self.bad_debts += o.bad_debts;
        self.recoveries += o.recoveries;
        self.adjustments += o.adjustments;
        self.paid += o.paid;
        self.owed += o.owed;
    }
}

fn basis_or(state: &DemoState, basis: Option<SalesTaxBasis>) -> SalesTaxBasis {
    match basis {
        Some(b @ (SalesTaxBasis::Accrual | SalesTaxBasis::Cash)) => b,
        _ => state.sales_tax.basis,
    }
}

/// Per item through `end` (columns over `start..=end`), plus the
/// unassigned tax through `end` and the cash rounding left in range.
/// `all_payments`: every live payment line counts whatever its date (what
/// is owed for Pay and the owed figure, S1).
fn columns(
    state: &DemoState,
    basis: SalesTaxBasis,
    start: CalendarDate,
    end: CalendarDate,
    all_payments: bool,
) -> (BTreeMap<SalesTaxItemId, Cols>, Decimal, Decimal) {
    let mut by: BTreeMap<SalesTaxItemId, Cols> = BTreeMap::new();
    let mut unassigned = Decimal::ZERO;
    let mut rounding = Decimal::ZERO;
    for e in events(state, basis) {
        let payment = matches!(e.col, Col::Paid | Col::Adjustments);
        if e.date > end && !(all_payments && payment) {
            continue;
        }
        let in_range = e.date >= start;
        let Some(item) = e.item else {
            unassigned += e.tax;
            continue;
        };
        if in_range && let Some(x) = e.exact {
            rounding += x - e.tax;
        }
        let c = by.entry(item).or_default();
        if c.agency.is_none() {
            c.agency = e.agency;
        }
        c.add(&e, in_range);
    }
    (by, unassigned, round(rounding, 6))
}

/// `GET /reports/sales-tax-liability` (v2) for `start..=end` on `basis`
/// (`None`: the company's).
///
/// # Errors
/// `Local(BadRange)` for an end before the start; `Internal` if the demo's
/// own answer does not decode.
pub fn liability(
    state: &mut DemoState,
    start: CalendarDate,
    end: CalendarDate,
    basis: Option<SalesTaxBasis>,
) -> Result<SalesTaxLiability, ApiError> {
    if end < start {
        return Err(ApiError::Local(cl_api::LocalValidation::BadRange));
    }
    let _ = super::sales_tax::agencies(state, true);
    let basis = basis_or(state, basis);
    let (by, unassigned, rounding) = columns(state, basis, start, end, false);
    let contacts = state.contacts_mut().clone();
    let st = &state.sales_tax;
    let name_of_agency = |a: Option<SalesTaxAgencyId>| {
        a.and_then(|a| st.agency_vendor(a))
            .and_then(|v| contacts.iter().find(|c| c.id == v))
            .map(|c| c.fields.display_name.clone())
            .unwrap_or_default()
    };
    // Agencies in setup order, then items without one.
    let mut order: Vec<Option<SalesTaxAgencyId>> = st.agency_ids().into_iter().map(Some).collect();
    order.push(None);
    let mut agencies = Vec::new();
    let mut total = Cols::default();
    for a in order {
        let mut items: Vec<(&SalesTaxItemId, &Cols)> =
            by.iter().filter(|(_, c)| c.agency == a).collect();
        // Every item of the agency shows, even with nothing in it.
        let mut zero = Vec::new();
        if let Some(a) = a {
            for i in st.agency_items(a) {
                if !by.contains_key(&i) {
                    zero.push((i, Cols::default()));
                }
            }
        }
        items.extend(zero.iter().map(|(i, c)| (i, c)));
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|(i, _)| st.item_name(**i).unwrap_or_default().to_string());
        let mut totals = Cols::default();
        let rows: Vec<Value> =
            items
                .iter()
                .map(|(i, c)| {
                    totals.plus(c);
                    let mut v = c.json();
                    v["taxItemId"] = json!(i);
                    v["name"] = json!(st.item_name(**i).unwrap_or_default());
                    v["rate"] = json!(st.item_rate(**i).map(|r| round(r, 6).to_string()));
                    v["taxableSales"] = json!(s2(c.taxable));
                    v["nonTaxable"] =
                        json!(c.non.iter().map(|(code, amount)| json!({
                    "codeId": code,
                    "code": st.code(*code).map(|x| x.code.clone()).unwrap_or_default(),
                    "amount": s2(*amount),
                })).collect::<Vec<_>>());
                    v
                })
                .collect();
        total.plus(&totals);
        agencies.push(json!({
            "agencyId": a,
            "name": name_of_agency(a),
            "items": rows,
            "totals": totals.json(),
        }));
    }
    let movement =
        total.collected + total.discounts + total.bad_debts + total.recoveries + total.adjustments
            - total.paid;
    let cash = basis == SalesTaxBasis::Cash;
    let v = json!({
        "basis": basis.as_wire(),
        "startDate": start,
        "endDate": end,
        "agencies": agencies,
        "unassigned": s2(unassigned),
        "openingPaid": "0.00",
        "otherEntries": "0.00",
        "glMovement": s2(movement),
        "roundingDifference": cash.then(|| format!("{rounding:.6}")),
        "balanceCheck": {
            "stpBalance": s2(total.owed + unassigned),
            "owedAtEnd": s2(total.owed),
            "otherEntriesCumulative": "0.00",
            "openingPaid": "0.00",
        },
    });
    decode(v)
}

/// `GET /sales-tax/owed?asOf&basis&agencyId`: by agency and item.
///
/// # Errors
/// `Internal` if the demo's own answer does not decode.
pub fn owed(
    state: &mut DemoState,
    as_of: CalendarDate,
    basis: Option<SalesTaxBasis>,
    agency: Option<SalesTaxAgencyId>,
) -> Result<SalesTaxOwed, ApiError> {
    decode(owed_json(state, as_of, basis, agency))
}

/// [`owed`] as the engine's JSON.
pub fn owed_json(
    state: &mut DemoState,
    as_of: CalendarDate,
    basis: Option<SalesTaxBasis>,
    agency: Option<SalesTaxAgencyId>,
) -> Value {
    let _ = super::sales_tax::agencies(state, true);
    let basis = basis_or(state, basis);
    let (by, unassigned, _) = columns(state, basis, CalendarDate::min(), as_of, true);
    let mut agencies: Vec<(Option<SalesTaxAgencyId>, Vec<Value>, Decimal)> = Vec::new();
    let mut total = if agency.is_none() {
        unassigned
    } else {
        Decimal::ZERO
    };
    for (item, c) in &by {
        if agency.is_some() && c.agency != agency {
            continue;
        }
        total += c.owed;
        let v = json!({"taxItemId": item, "owed": s2(c.owed)});
        match agencies.iter_mut().find(|a| a.0 == c.agency) {
            Some(a) => {
                a.1.push(v);
                a.2 += c.owed;
            }
            None => agencies.push((c.agency, vec![v], c.owed)),
        }
    }
    json!({
        "agencies": agencies.into_iter().map(|(a, items, t)| json!({
            "agencyId": a, "items": items, "total": s2(t),
        })).collect::<Vec<_>>(),
        "unassigned": s2(if agency.is_none() { unassigned } else { Decimal::ZERO }),
        "total": s2(total),
    })
}

// ---- Pay Sales Tax -------------------------------------------------------------------

/// What each of the agency's items owes for Pay (S1): rows on the company
/// basis dated through `through` + every live adjustment − every live
/// paid line, whatever their dates.
fn owed_for_pay(
    state: &DemoState,
    agency: SalesTaxAgencyId,
    through: CalendarDate,
) -> BTreeMap<SalesTaxItemId, Decimal> {
    let st = &state.sales_tax;
    let mut out: BTreeMap<SalesTaxItemId, Decimal> = st
        .agency_items(agency)
        .into_iter()
        .map(|i| (i, Decimal::ZERO))
        .collect();
    for e in events(state, st.basis) {
        let Some(item) = e.item else { continue };
        if e.agency != Some(agency) {
            continue;
        }
        let x = match e.col {
            Col::Paid => -e.tax,
            Col::Adjustments => e.tax,
            _ if e.date <= through => e.tax,
            _ => continue,
        };
        *out.entry(item).or_default() += x;
    }
    out
}

fn money_of(m: Money) -> Decimal {
    m.as_decimal()
}

/// The preview of a payment (no writes) and the lines it would post:
/// (item, paid, adjustment, owed).
fn figure(
    state: &mut DemoState,
    r: &PaySalesTaxRequest,
) -> Result<(Value, Vec<FigureLine>), ApiError> {
    r.check()?;
    let _ = super::sales_tax::agencies(state, true);
    if state.sales_tax.agency_vendor(r.agency_id).is_none() {
        return Err(ApiError::NotFound { code: None });
    }
    let owed = owed_for_pay(state, r.agency_id, r.through_date);
    let mut problems = Vec::new();
    for a in &r.adjustments {
        if !owed.contains_key(&a.tax_item_id) {
            return Err(refused(400, "TAX_ITEM_NOT_FOR_AGENCY", &json!({})));
        }
    }
    let mut lines = Vec::new();
    let mut out = Vec::new();
    let mut total = Decimal::ZERO;
    for (item, o) in &owed {
        let typed = r
            .lines
            .as_ref()
            .and_then(|ls| ls.iter().find(|l| l.tax_item_id == *item))
            .map(|l| money_of(l.amount));
        if let Some(ls) = &r.lines
            && ls.iter().any(|l| !owed.contains_key(&l.tax_item_id))
        {
            return Err(refused(400, "TAX_ITEM_NOT_FOR_AGENCY", &json!({})));
        }
        let adj: Decimal = r
            .adjustments
            .iter()
            .filter(|a| a.tax_item_id == *item)
            .map(|a| money_of(a.amount))
            .sum();
        // Without typed lines everything owed is paid.
        let pay = match (&r.lines, typed) {
            (_, Some(t)) => t,
            (None, None) => *o,
            (Some(_), None) => Decimal::ZERO,
        };
        if o.is_zero() && pay.is_zero() && adj.is_zero() {
            continue;
        }
        if pay > *o && !pay.is_zero() {
            problems.push(json!({"code": "AMOUNT_MORE_THAN_OWED", "taxItemId": item}));
        }
        let amount = pay + adj;
        total += amount;
        out.push(json!({
            "taxItemId": item,
            "name": state.sales_tax.item_name(*item).unwrap_or_default(),
            "owed": s2(*o),
            "adjustment": s2(adj),
            "amount": s2(amount),
            "expectedOwed": s2(*o),
        }));
        lines.push((*item, pay, adj, *o));
    }
    if r.method == SalesTaxPayMethod::Check {
        let vendor = state.sales_tax.agency_vendor(r.agency_id);
        let has_address = state
            .contacts_mut()
            .iter()
            .find(|c| Some(c.id) == vendor)
            .is_some_and(|c| c.fields.billing_line1.is_some());
        if !has_address {
            problems.push(json!({"code": "NO_VENDOR_ADDRESS"}));
        }
    }
    let (_, unassigned, _) = columns(
        state,
        state.sales_tax.basis,
        CalendarDate::min(),
        r.through_date,
        true,
    );
    if !unassigned.is_zero() {
        problems.push(json!({"code": "LEGACY_UNASSIGNED"}));
    }
    Ok((
        json!({"lines": out, "total": s2(total), "problems": problems}),
        lines,
    ))
}

/// `POST /sales-tax/payments/preview`.
///
/// # Errors
/// `Local(…)` from the request check; 404 for an unknown agency; 400
/// `TAX_ITEM_NOT_FOR_AGENCY`.
pub fn preview(state: &mut DemoState, r: &PaySalesTaxRequest) -> Result<PayPreview, ApiError> {
    figure(state, r).and_then(|(v, _)| decode(v))
}

fn stp(state: &mut DemoState) -> Option<AccountId> {
    let chart = super::receivables::chart(state);
    super::receivables::tagged(&chart, "sales_tax_payable")
}

/// `POST /sales-tax/payments` (keyed; closing on the date).
///
/// # Errors
/// As [`preview`]; 409 `SALES_TAX_OWED_CHANGED` when an `expectedOwed`
/// moved; 400 `PAYMENT_TOTAL_NOT_POSITIVE`; closing.
pub fn create(
    state: &mut DemoState,
    r: &PaySalesTaxRequest,
    password: Option<&Secret>,
    key: Option<&str>,
) -> Result<PaySalesTaxResult, ApiError> {
    if let Some(done) = key.and_then(|k| state.sales_tax.book.keys.get(k)) {
        return decode(done.clone());
    }
    let (_, lines) = figure(state, r)?;
    for l in r.lines.as_deref().unwrap_or_default() {
        let now = lines
            .iter()
            .find(|x| x.0 == l.tax_item_id)
            .map_or(Decimal::ZERO, |x| x.3);
        if l.expected_owed.map(money_of) != Some(now) {
            return Err(refused(409, "SALES_TAX_OWED_CHANGED", &json!({})));
        }
    }
    let total: Decimal = lines.iter().map(|l| l.1 + l.2).sum();
    if total <= Decimal::ZERO {
        return Err(refused(400, "PAYMENT_TOTAL_NOT_POSITIVE", &json!({})));
    }
    check_closing(super::receivables::closing(state), &[r.date], password)?;
    let stp = stp(state).ok_or(ApiError::Internal)?;
    let money = super::receivables::money;
    let mut jl = Vec::new();
    let mut stored = Vec::new();
    for (item, pay, adj, _) in &lines {
        let paid = *pay + *adj;
        if !pay.is_zero() {
            // Sales Tax Payable is relieved by what was owed and paid.
            jl.push(DemoLine {
                account: stp,
                debit: money(*pay)?,
                credit: Money::ZERO,
                memo: None,
            });
        }
        if !paid.is_zero() {
            stored.push((*item, "paid", paid, None, None));
        }
    }
    for a in &r.adjustments {
        let x = money_of(a.amount);
        // A lowering adjustment (negative) is income; a raise is expense.
        jl.push(DemoLine {
            account: a.account_id,
            debit: money(x.max(Decimal::ZERO))?,
            credit: money((-x).max(Decimal::ZERO))?,
            memo: None,
        });
        // Adjustments lower what is owed and are paid with it: owed moves
        // by the adjustment and the paid line holds the cash.
        stored.push((
            a.tax_item_id,
            "adjustment",
            x,
            Some(a.account_id),
            Some(a.reason.clone()),
        ));
    }
    jl.push(DemoLine {
        account: r.bank_account_id,
        debit: Money::ZERO,
        credit: money(total)?,
        memo: None,
    });
    let vendor = state.sales_tax.agency_vendor(r.agency_id);
    let payee = state
        .contacts_mut()
        .iter()
        .find(|c| Some(c.id) == vendor)
        .map(|c| c.fields.display_name.clone());
    let chart = super::receivables::chart(state);
    let id = state
        .ledger
        .save_lines(
            &chart,
            &DemoJournal {
                kind: "check",
                date: r.date,
                number: r.check_number.clone().unwrap_or_default(),
                contact: payee,
                memo: r.memo.clone().unwrap_or_default(),
                lines: jl,
            },
            None,
        )
        .ok_or(ApiError::Internal)?;
    let queued = r.method == SalesTaxPayMethod::Check && r.print_later;
    let book = &mut state.sales_tax.book;
    book.next += 1;
    book.writes += 1;
    let number = format!("STP-{}", book.next);
    book.payments.insert(
        id,
        TaxPayment {
            agency: r.agency_id,
            date: r.date,
            through: Some(r.through_date),
            method: r.method,
            bank: Some(r.bank_account_id),
            total,
            check_number: r.check_number.clone(),
            queued,
            linked: false,
            voided: false,
            lines: stored,
            memo: r.memo.clone(),
        },
    );
    let v = json!({
        "id": id,
        "number": number,
        "checkNumber": r.check_number,
        "printStatus": queued.then_some("queue"),
    });
    if let Some(k) = key {
        book.keys.insert(k.to_string(), v.clone());
    }
    decode(v)
}

/// `POST /sales-tax/payments/link`: an outside check or entry that debited
/// Sales Tax Payable becomes this agency's payment.
///
/// # Errors
/// 404; 400 `NOT_STP_PAYMENT`, `LINK_TOTAL_MISMATCH`,
/// `TAX_ITEM_NOT_FOR_AGENCY`; 409 `ALREADY_LINKED`; closing.
pub fn link(
    state: &mut DemoState,
    r: &LinkPaymentRequest,
    password: Option<&Secret>,
    key: Option<&str>,
) -> Result<(), ApiError> {
    if key.is_some_and(|k| state.sales_tax.book.keys.contains_key(k)) {
        return Ok(());
    }
    r.check()?;
    let _ = super::sales_tax::agencies(state, true);
    if state
        .sales_tax
        .book
        .payments
        .contains_key(&r.transaction_id)
    {
        return Err(refused(409, "ALREADY_LINKED", &json!({})));
    }
    let stp = stp(state).ok_or(ApiError::Internal)?;
    let txn = super::checks::transaction(state, r.transaction_id)
        .ok_or(ApiError::NotFound { code: None })?;
    let debit: Decimal = txn
        .lines
        .iter()
        .filter(|l| l.account_id == stp)
        .map(|l| l.debit.as_decimal() - l.credit.as_decimal())
        .sum();
    if debit <= Decimal::ZERO {
        return Err(refused(400, "NOT_STP_PAYMENT", &json!({})));
    }
    let items = state.sales_tax.agency_items(r.agency_id);
    if r.lines.iter().any(|l| !items.contains(&l.tax_item_id)) {
        return Err(refused(400, "TAX_ITEM_NOT_FOR_AGENCY", &json!({})));
    }
    let sum: Decimal = r.lines.iter().map(|l| money_of(l.amount)).sum();
    if sum != debit {
        return Err(refused(400, "LINK_TOTAL_MISMATCH", &json!({})));
    }
    check_closing(
        super::receivables::closing(state),
        &[txn.txn_date],
        password,
    )?;
    let book = &mut state.sales_tax.book;
    book.writes += 1;
    book.payments.insert(
        r.transaction_id,
        TaxPayment {
            agency: r.agency_id,
            date: txn.txn_date,
            through: None,
            method: SalesTaxPayMethod::Check,
            bank: None,
            total: sum,
            check_number: None,
            queued: false,
            linked: true,
            voided: false,
            lines: r
                .lines
                .iter()
                .map(|l| (l.tax_item_id, "paid", money_of(l.amount), None, None))
                .collect(),
            memo: None,
        },
    );
    if let Some(k) = key {
        book.keys.insert(k.to_string(), Value::Null);
    }
    Ok(())
}

fn record_json(id: TransactionId, p: &TaxPayment) -> Value {
    json!({
        "id": id,
        "agencyId": p.agency,
        "date": p.date,
        "throughDate": p.through,
        "method": p.method,
        "bankAccountId": p.bank,
        "total": s2(p.total),
        "checkNumber": p.check_number,
        "printStatus": p.queued.then_some("queue"),
        "linked": p.linked,
        "voided": p.voided,
        "lines": p.lines.iter().map(|(i, k, a, acct, reason)| json!({
            "taxItemId": i, "kind": k, "amount": s2(*a),
            "accountId": acct, "reason": reason,
        })).collect::<Vec<_>>(),
        "memo": p.memo,
    })
}

/// `GET /sales-tax/payments/:id`.
///
/// # Errors
/// 404.
pub fn get(state: &DemoState, id: TransactionId) -> Result<SalesTaxPaymentRecord, ApiError> {
    let p = state
        .sales_tax
        .book
        .payments
        .get(&id)
        .ok_or(ApiError::NotFound { code: None })?;
    decode(record_json(id, p))
}

/// `GET /sales-tax/payments` (newest first).
#[must_use]
pub fn list(state: &DemoState, agency: Option<SalesTaxAgencyId>) -> Vec<SalesTaxPaymentRecord> {
    let mut out: Vec<SalesTaxPaymentRecord> = state
        .sales_tax
        .book
        .payments
        .iter()
        .filter(|(_, p)| agency.is_none_or(|a| p.agency == a))
        .filter_map(|(id, p)| decode(record_json(*id, p)).ok())
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse((r.date, r.id)));
    out
}

/// Voids a payment (`/transactions/:id/void`): what it paid is owed again
/// and a queued check leaves the print queue. A linked payment is only
/// unlinked (the outside check stays).
///
/// # Errors
/// 404; closing on its date.
pub fn void(
    state: &mut DemoState,
    id: TransactionId,
    password: Option<&Secret>,
) -> Result<(), ApiError> {
    let p = state
        .sales_tax
        .book
        .payments
        .get(&id)
        .cloned()
        .ok_or(ApiError::NotFound { code: None })?;
    if p.voided {
        return Err(refused(400, "VALIDATION_ERROR", &json!({})));
    }
    check_closing(super::receivables::closing(state), &[p.date], password)?;
    if p.linked {
        state.sales_tax.book.payments.remove(&id);
    } else {
        state.ledger.void(id)?;
        if let Some(x) = state.sales_tax.book.payments.get_mut(&id) {
            x.voided = true;
            x.queued = false;
        }
    }
    state.sales_tax.book.writes += 1;
    Ok(())
}

/// The sales tax checks waiting to print on `bank`.
pub fn print_queue(
    state: &mut DemoState,
    bank: AccountId,
    printed: &dyn Fn(TransactionId) -> bool,
) -> Vec<cl_api::QueuedCheck> {
    let contacts = state.contacts_mut().clone();
    let st = &state.sales_tax;
    st.book
        .payments
        .iter()
        .filter(|(id, p)| p.queued && !p.voided && p.bank == Some(bank) && !printed(**id))
        .filter_map(|(id, p)| {
            let vendor = st
                .agency_vendor(p.agency)
                .and_then(|v| contacts.iter().find(|c| c.id == v));
            Some(cl_api::QueuedCheck {
                id: *id,
                date: p.date,
                amount: super::receivables::money(p.total).ok()?,
                payee: vendor
                    .map(|c| c.fields.display_name.clone())
                    .unwrap_or_default(),
                memo: p.memo.clone().unwrap_or_default(),
                kind: cl_api::QueuedCheckKind::SalesTaxPayment,
                bank_account: Some(bank),
                address: vendor
                    .map(|c| {
                        let f = &c.fields;
                        let city = [
                            f.billing_city.clone().unwrap_or_default(),
                            f.billing_state.clone().unwrap_or_default(),
                            f.billing_zip.clone().unwrap_or_default(),
                        ]
                        .join(" ");
                        [f.billing_line1.clone().unwrap_or_default(), city]
                            .into_iter()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// The voucher stub of a queued sales tax check.
#[must_use]
pub fn stub(state: &mut DemoState, id: TransactionId) -> Option<cl_print::StubData> {
    let r = get(state, id).ok()?;
    let items = super::sales_tax::items(state, true);
    Some(crate::screens::print_forms::job::stub_of_sales_tax_payment(
        &r, &items,
    ))
}

// ---- Recoveries ----------------------------------------------------------------------

/// What was written off an invoice (live write-offs, net of bounces) and
/// each item's bad-debt share of it (SSUTA on).
fn written_off(
    state: &DemoState,
    invoice: TransactionId,
) -> (
    Decimal,
    Vec<(SalesTaxItemId, Option<SalesTaxAgencyId>, Decimal)>,
) {
    let sets = settlements(state).remove(&invoice).unwrap_or_default();
    let w: Decimal = sets.iter().map(|s| s.write_off).sum();
    let mut items = Vec::new();
    if let (true, Some(total), Some(rows)) = (
        state.sales_tax.ssuta,
        invoice_total(state, invoice),
        state.sales_tax.rows.get(&invoice),
    ) {
        for r in rows.iter().filter(|r| r.code.is_none()) {
            let Some(item) = r.tax_item else { continue };
            let mut p = Decimal::ZERO;
            let mut wt = Decimal::ZERO;
            for s in &sets {
                wt += ssuta_split(r.tax, *s, p, total).1;
                p += s.cash + s.discount + s.write_off;
            }
            items.push((item, r.agency, wt));
        }
    }
    (w, items)
}

/// The invoices of `client` with a write-off left to recover: (invoice,
/// number, written off, left).
#[must_use]
pub fn recoverable(
    state: &DemoState,
    client: Option<ContactId>,
) -> Vec<(TransactionId, String, Money, Money)> {
    let mut out = Vec::new();
    for (id, s) in &state.invoices.stored {
        if client.is_some_and(|c| s.record["contactId"].as_str() != Some(c.to_string().as_str())) {
            continue;
        }
        let (w, _) = written_off(state, *id);
        if w <= Decimal::ZERO {
            continue;
        }
        let taken: Decimal = state
            .sales_tax
            .book
            .recoveries
            .values()
            .filter(|r| r.invoice == *id)
            .map(|r| r.amount)
            .sum();
        let left = w - taken;
        if left <= Decimal::ZERO {
            continue;
        }
        let number = s.record["txnNumber"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if let (Ok(w), Ok(left)) = (
            super::receivables::money(w),
            super::receivables::money(left),
        ) {
            out.push((*id, number, w, left));
        }
    }
    out
}

/// True when part of `invoice`'s write-off was recovered (a bounce of the
/// payment that wrote it off is refused, 409 `WRITE_OFF_RECOVERED`).
#[must_use]
pub fn recovered_on(state: &DemoState, invoice: TransactionId) -> bool {
    state
        .sales_tax
        .book
        .recoveries
        .values()
        .any(|r| r.invoice == invoice)
}

/// `POST /payments/write-off-recoveries` (keyed; closing on the date): the
/// invoice reopens by the amount; with SSUTA on the recovered tax per item
/// is `round2(w × X_cum / W)` less earlier recoveries.
///
/// # Errors
/// 404; 409 `RECOVERY_EXCEEDS_WRITE_OFF {available}`; closing.
pub fn recover(
    state: &mut DemoState,
    r: &NewRecovery,
    password: Option<&Secret>,
    key: Option<&str>,
) -> Result<RecoveryResult, ApiError> {
    if let Some(done) = key.and_then(|k| state.sales_tax.book.keys.get(k)) {
        return decode(done.clone());
    }
    r.check()?;
    if invoice_total(state, r.invoice_id).is_none() {
        return Err(ApiError::NotFound { code: None });
    }
    let (w, shares) = written_off(state, r.invoice_id);
    let earlier: Vec<&Recovery> = state
        .sales_tax
        .book
        .recoveries
        .values()
        .filter(|x| x.invoice == r.invoice_id)
        .collect();
    let before: Decimal = earlier.iter().map(|x| x.amount).sum();
    let x = money_of(r.amount);
    if before + x > w {
        let available = super::receivables::money((w - before).max(Decimal::ZERO))?;
        return Err(refused(
            409,
            "RECOVERY_EXCEEDS_WRITE_OFF",
            &json!({"available": available.to_wire()}),
        ));
    }
    check_closing(super::receivables::closing(state), &[r.date], password)?;
    let items: Vec<(SalesTaxItemId, Option<SalesTaxAgencyId>, Decimal)> = shares
        .iter()
        .map(|(item, agency, wt)| {
            let taken: Decimal = earlier
                .iter()
                .flat_map(|e| e.items.iter())
                .filter(|(i, _, _)| i == item)
                .map(|(_, _, t)| *t)
                .sum();
            (*item, *agency, round(*wt * (before + x) / w, 2) - taken)
        })
        .collect();
    let tax: Decimal = items.iter().map(|i| i.2).sum();
    super::invoices::settle(state, r.invoice_id, -x)?;
    let balance = state
        .invoices
        .stored
        .get(&r.invoice_id)
        .and_then(|s| s.record["balanceDue"].as_str())
        .and_then(|b| Decimal::from_str(b).ok())
        .unwrap_or_default();
    let book = &mut state.sales_tax.book;
    book.next += 1;
    book.writes += 1;
    let id = TransactionId::parse(&format!("d7300000-0000-4000-8000-{:012}", book.next))
        .map_err(|_| ApiError::Internal)?;
    book.recoveries.insert(
        id,
        Recovery {
            invoice: r.invoice_id,
            date: r.date,
            amount: x,
            items,
        },
    );
    let v = json!({
        "id": id,
        "invoiceBalanceDue": s2(balance),
        "recoveredTax": s2(tax),
    });
    if let Some(k) = key {
        book.keys.insert(k.to_string(), v.clone());
    }
    decode(v)
}

// ---- Exemption certificates ------------------------------------------------------------

/// `GET /sales-tax/exemptions?contactId=`.
#[must_use]
pub fn exemptions(
    state: &DemoState,
    client: Option<ContactId>,
    include_inactive: bool,
) -> Vec<TaxExemption> {
    state
        .sales_tax
        .book
        .exemptions
        .iter()
        .filter(|e| client.is_none_or(|c| e.contact_id == c))
        .filter(|e| include_inactive || e.is_active)
        .cloned()
        .collect()
}

/// `POST /sales-tax/exemptions` (`id: None`) or `PUT
/// /sales-tax/exemptions/:id`.
///
/// # Errors
/// `Local(InvalidValue)` from the draft; 404; 409 `STALE`.
pub fn save_exemption(
    state: &mut DemoState,
    id: Option<TaxExemptionId>,
    d: &TaxExemptionDraft,
    version: Option<cl_core::Timestamp>,
) -> Result<TaxExemption, ApiError> {
    let body = d.body()?;
    let book = &mut state.sales_tax.book;
    book.next += 1;
    book.writes += 1;
    let stamp = cl_core::Timestamp::parse_wire(&format!(
        "2026-11-04T09:{:02}:{:02}.000Z",
        book.next / 60 % 60,
        book.next % 60
    ))
    .unwrap_or_else(|_| DemoState::now());
    let id = match id {
        Some(id) => {
            let e = book
                .exemptions
                .iter()
                .find(|e| e.id == id)
                .ok_or(ApiError::NotFound { code: None })?;
            if version.is_some() && version != e.updated_at {
                return Err(refused(409, "STALE", &json!({})));
            }
            id
        }
        None => TaxExemptionId::parse(&format!("d7400000-0000-4000-8000-{:012}", book.next))
            .map_err(|_| ApiError::Internal)?,
    };
    let mut v = body;
    v["id"] = json!(id);
    v["isActive"] = json!(true);
    v["updatedAt"] = json!(stamp);
    let e: TaxExemption = decode(v)?;
    match book.exemptions.iter_mut().find(|x| x.id == id) {
        Some(x) => *x = e.clone(),
        None => book.exemptions.push(e.clone()),
    }
    Ok(e)
}

/// Deactivates a certificate (kept for history).
///
/// # Errors
/// 404.
pub fn deactivate_exemption(
    state: &mut DemoState,
    id: TaxExemptionId,
) -> Result<TaxExemption, ApiError> {
    let book = &mut state.sales_tax.book;
    let e = book
        .exemptions
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or(ApiError::NotFound { code: None })?;
    e.is_active = false;
    book.writes += 1;
    Ok(e.clone())
}

/// `GET /reports/exemptions-expiring?asOf&within`: active certificates
/// expired or expiring within `within` days, soonest first.
#[must_use]
pub fn expiring(state: &mut DemoState, as_of: CalendarDate, within: u32) -> Vec<ExpiringExemption> {
    let limit = as_of.add_days(i64::from(within)).unwrap_or(as_of);
    let contacts = state.contacts_mut().clone();
    let mut out: Vec<ExpiringExemption> = state
        .sales_tax
        .book
        .exemptions
        .iter()
        .filter(|e| e.is_active)
        .filter_map(|e| {
            let on = e.expires_on?;
            if on > limit {
                return None;
            }
            decode(json!({
                "id": e.id,
                "contactId": e.contact_id,
                "contactName": contacts.iter().find(|c| c.id == e.contact_id)
                    .map(|c| c.fields.display_name.clone()).unwrap_or_default(),
                "number": e.number,
                "state": e.state,
                "expiresOn": on,
                "status": if on < as_of { "expired" } else { "expiring" },
            }))
            .ok()
        })
        .collect();
    out.sort_by_key(|e| (e.expires_on, e.contact_name.clone()));
    out
}

/// The certificate warning of a document for `client` on `date` (Q-P23n):
/// only for a client whose tax code is not taxable; never blocks a save.
#[must_use]
pub fn exemption_problem(
    state: &mut DemoState,
    client: ContactId,
    date: CalendarDate,
) -> Option<&'static str> {
    let code = state
        .contacts_mut()
        .iter()
        .find(|c| c.id == client)
        .and_then(|c| c.fields.tax_code_id)?;
    if state.sales_tax.code(code).is_none_or(|c| c.taxable) {
        return None;
    }
    let best = state
        .sales_tax
        .book
        .exemptions
        .iter()
        .filter(|e| e.is_active && e.contact_id == client)
        .map(|e| e.expires_on)
        .max_by_key(|d| d.map_or((1, CalendarDate::min()), |d| (0, d)));
    // A certificate without an expiry never expires.
    match best {
        None => Some("EXEMPTION_MISSING"),
        Some(Some(on)) if on < date => Some("EXEMPTION_EXPIRED"),
        Some(Some(on)) if date.add_days(30).is_ok_and(|soon| on <= soon) => {
            Some("EXEMPTION_EXPIRING")
        }
        Some(_) => None,
    }
}

#[cfg(test)]
#[path = "sales_tax_owed_tests.rs"]
mod tests;
