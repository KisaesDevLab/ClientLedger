//! The demo "engine" for P22's finance charges (D2): the settings (rate
//! blank = off, Q-P22a), the preview (simple daily interest / 365 from the
//! due date or the charged-through date, per-invoice round2 half up, the
//! minimum on the client total, grace from the due date, the open-credit
//! hint, Q-P22g/Q-P22s), assess (all or nothing, once per client and month,
//! keyed) and void (an assessment whose invoice is void frees its period
//! and days).
//!
//! `--demo` only; arithmetic in `rust_decimal`, held to
//! `fixtures/receivables/golden.json` case 6. No floats. Synthetic data.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cl_api::{
    AlreadyAssessed, AnnualRate, ApiError, AssessRequest, AssessResult, AssessedInvoice,
    Assessment, FinanceChargeAssessmentId, FinanceChargePreview, FinanceChargeProblem,
    FinanceChargeRow, FinanceChargeRunId, FinanceChargeSettings, FinanceChargeSettingsUpdate,
    FinanceChargeSkip, FinanceChargeSource, FinanceChargeTotals,
};
use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money, Secret};
use rust_decimal::{Decimal, RoundingStrategy};

use super::DemoState;
use super::invoices::receivable;
use super::receivables::{closing, conflict, dec, invalid, money, new_id, refused};
use super::register::check_closing;
use super::statements::{ItemKind, ar_open_items};

/// Finance-charge settings and assessments of the demo engine.
#[derive(Clone, Default)]
pub struct DemoFinance {
    /// Annual percent (`None`: off; the demo company starts blank).
    pub rate: Option<AnnualRate>,
    /// The smallest charge per client.
    pub minimum: Option<Money>,
    /// Days after the due date before charging.
    pub grace_days: u16,
    /// Charge on earlier finance charges.
    pub compounding: bool,
    /// The income account.
    pub income_account: Option<AccountId>,
    /// Finance-charge invoices wait to be printed (default on).
    pub print_later: Option<bool>,
    assessments: Vec<Assessed>,
    charges: BTreeSet<TransactionId>,
    keys: BTreeMap<String, AssessResult>,
    next: u64,
}

impl fmt::Debug for DemoFinance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoFinance")
            .field("rate", &self.rate)
            .field("assessments", &self.assessments.len())
            .finish_non_exhaustive()
    }
}

impl DemoFinance {
    /// True for a finance-charge invoice.
    #[must_use]
    pub fn is_charge(&self, id: TransactionId) -> bool {
        self.charges.contains(&id)
    }
}

#[derive(Clone)]
struct Assessed {
    id: FinanceChargeAssessmentId,
    run: FinanceChargeRunId,
    client: ContactId,
    period: String,
    date: CalendarDate,
    invoice: TransactionId,
    number: String,
    total: Decimal,
    /// (source invoice, through date).
    sources: Vec<(TransactionId, CalendarDate)>,
}

fn period_of(d: CalendarDate) -> String {
    format!("{:04}-{:02}", d.year(), d.month())
}

/// An assessment is live while its invoice is not void.
fn live(state: &DemoState, a: &Assessed) -> bool {
    receivable(state, a.invoice).is_some()
}

/// `PUT /company/settings` (the finance-charge fields).
pub fn set_settings(state: &mut DemoState, u: &FinanceChargeSettingsUpdate) {
    let f = &mut state.finance;
    if let Some(r) = u.annual_rate {
        f.rate = r;
    }
    if let Some(m) = u.minimum {
        f.minimum = Some(m);
    }
    if let Some(g) = u.grace_days {
        f.grace_days = g;
    }
    if let Some(b) = u.on_finance_charges {
        f.compounding = b;
    }
    if let Some(a) = u.income_account_id {
        f.income_account = a;
    }
    if let Some(b) = u.print_later {
        f.print_later = Some(b);
    }
}

/// `GET /company/settings`' finance-charge fields as the demo engine holds
/// them (Preferences reads them).
pub fn fill_settings(f: &DemoFinance, s: &mut cl_api::CompanySettings) {
    s.finance_charge_annual_rate = f.rate;
    s.finance_charge_minimum = Some(f.minimum.unwrap_or(Money::ZERO));
    s.finance_charge_grace_days = Some(f.grace_days);
    s.finance_charge_on_finance_charges = Some(f.compounding);
    s.finance_charge_income_account_id = f.income_account;
    s.finance_charge_print_later = Some(f.print_later.unwrap_or(true));
}

fn settings_echo(state: &DemoState) -> FinanceChargeSettings {
    let f = &state.finance;
    FinanceChargeSettings {
        annual_rate: f.rate,
        minimum: f.minimum,
        grace_days: Some(f.grace_days),
        on_finance_charges: f.compounding,
        income_account_id: f.income_account,
    }
}

fn round_half_up(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

struct ClientCharge {
    row: FinanceChargeRow,
    /// (invoice, through date) per charged source.
    sources: Vec<(TransactionId, CalendarDate)>,
}

/// Everything one client would be charged on `d`.
fn charge_for(
    state: &mut DemoState,
    client: ContactId,
    name: String,
    active: bool,
    d: CalendarDate,
) -> Result<Option<ClientCharge>, ApiError> {
    let rate = state
        .finance
        .rate
        .map_or(Decimal::ZERO, AnnualRate::as_decimal);
    let grace = i64::from(state.finance.grace_days);
    let items = ar_open_items(state, d, Some(client));
    let overdue: Vec<_> = items
        .iter()
        .filter(|i| {
            i.open > Decimal::ZERO
                && match i.kind {
                    ItemKind::Invoice => true,
                    ItemKind::Charge => state.finance.compounding,
                    ItemKind::Credit => false,
                }
                && i.due.unwrap_or(i.date) < d
        })
        .cloned()
        .collect();
    let period = period_of(d);
    let already = state
        .finance
        .assessments
        .iter()
        .find(|a| a.client == client && a.period == period && live(state, a))
        .map(|a| AlreadyAssessed {
            invoice_id: a.invoice,
            number: Some(a.number.clone()),
        });
    if overdue.is_empty() && already.is_none() {
        return Ok(None);
    }
    let total_ar: Decimal = items.iter().map(|i| i.open).sum();
    let skipped = if !active {
        Some(FinanceChargeSkip::Inactive)
    } else if total_ar <= Decimal::ZERO {
        Some(FinanceChargeSkip::InCredit)
    } else {
        None
    };
    let credits: Decimal = -items
        .iter()
        .filter(|i| i.kind == ItemKind::Credit)
        .map(|i| i.open)
        .sum::<Decimal>();
    let mut sources = Vec::new();
    let mut through = Vec::new();
    let mut sum = Decimal::ZERO;
    if skipped.is_none() && already.is_none() {
        for i in &overdue {
            let due = i.due.unwrap_or(i.date);
            let t = state
                .finance
                .assessments
                .iter()
                .filter(|a| live(state, a))
                .flat_map(|a| a.sources.iter())
                .filter(|(inv, _)| *inv == i.txn)
                .map(|(_, through)| *through)
                .max();
            let from = match t {
                Some(t) if t > due => t,
                Some(_) => due,
                None => {
                    if due.days_until(d) <= grace {
                        continue;
                    }
                    due
                }
            };
            let days = from.days_until(d);
            if days <= 0 {
                continue;
            }
            let amount = round_half_up(
                i.open * rate / Decimal::ONE_HUNDRED * Decimal::from(days) / Decimal::from(365),
            );
            sum += amount;
            through.push((i.txn, d));
            sources.push(FinanceChargeSource {
                invoice_id: i.txn,
                number: Some(i.number.clone()),
                due_date: due,
                open: money(i.open)?,
                from_date: from,
                days: u32::try_from(days).map_err(|_| invalid())?,
                amount: money(amount)?,
            });
        }
    }
    let minimum = state.finance.minimum.map_or(Decimal::ZERO, dec);
    let adjustment = if sum > Decimal::ZERO && sum < minimum {
        minimum - sum
    } else {
        Decimal::ZERO
    };
    let overdue_balance: Decimal = overdue.iter().map(|i| i.open).sum();
    Ok(Some(ClientCharge {
        row: FinanceChargeRow {
            contact_id: client,
            name,
            overdue_balance: money(overdue_balance)?,
            charge: money(sum + adjustment)?,
            sources,
            already_assessed: already,
            skipped_reason: skipped,
            has_open_credits: (credits > Decimal::ZERO)
                .then(|| money(credits))
                .transpose()?,
            ticked: None,
        },
        sources: through,
    }))
}

fn clients(state: &mut DemoState, only: Option<&[ContactId]>) -> Vec<(ContactId, String, bool)> {
    state
        .contacts_mut()
        .iter()
        .filter(|c| c.fields.contact_type == cl_api::ContactType::Customer)
        .filter(|c| only.is_none_or(|o| o.contains(&c.id)))
        .map(|c| (c.id, c.fields.display_name.clone(), c.is_active))
        .collect()
}

/// 400 `FINANCE_CHARGE_DATE_IN_FUTURE` when `d` falls in a month after the
/// current one (the engine's rule, preview and assess alike): `current` is
/// the month of the story date + 1 day, so a month-end assessment prepared
/// a few days early still runs (golden 6's 11/30).
fn not_future(d: CalendarDate) -> Result<(), ApiError> {
    let current = DemoState::today().add_days(1).unwrap_or(DemoState::today());
    if (d.year(), d.month()) > (current.year(), current.month()) {
        let latest = format!("{:04}-{:02}", current.year(), current.month());
        return Err(cl_api::engine_refusal(
            400,
            &serde_json::json!({"error": {
                "message": "The assessment date is in a later month.",
                "code": "FINANCE_CHARGE_DATE_IN_FUTURE",
                "details": {"field": "assessmentDate", "latestPeriod": latest},
            }}),
        ));
    }
    Ok(())
}

/// `POST /finance-charges/preview`.
///
/// # Errors
/// 400 `FINANCE_CHARGE_DATE_IN_FUTURE` (a later month); `Internal` for an
/// amount out of range.
pub fn preview(
    state: &mut DemoState,
    d: CalendarDate,
    only: Option<&[ContactId]>,
) -> Result<FinanceChargePreview, ApiError> {
    super::credit_memos::seed(state);
    not_future(d)?;
    let mut problems = Vec::new();
    if state.finance.rate.is_none() {
        problems.push(FinanceChargeProblem {
            code: "RATE_NOT_SET".into(),
        });
    }
    if state.finance.income_account.is_none() {
        problems.push(FinanceChargeProblem {
            code: "INCOME_ACCOUNT_NOT_SET".into(),
        });
    }
    let mut rows = Vec::new();
    if problems.is_empty() {
        for (c, name, active) in clients(state, only) {
            if let Some(mut x) = charge_for(state, c, name, active, d)? {
                x.row.ticked = Some(x.row.chargeable() && x.row.has_open_credits.is_none());
                rows.push(x.row);
            }
        }
    }
    // The engine totals the rows it ticks (chargeable, no open credits).
    let charged: Vec<&FinanceChargeRow> = rows.iter().filter(|r| r.ticked == Some(true)).collect();
    let total: Decimal = charged.iter().map(|r| dec(r.charge)).sum();
    Ok(FinanceChargePreview {
        period: period_of(d),
        settings: Some(settings_echo(state)),
        totals: FinanceChargeTotals {
            count: u32::try_from(charged.len()).map_err(|_| invalid())?,
            charge: money(total)?,
        },
        rows,
        problems,
    })
}

/// `POST /finance-charges/assess` (`key`: the Idempotency-Key; a replay
/// answers the same invoices).
///
/// # Errors
/// 400 `FINANCE_CHARGE_NOT_SET_UP`; 409 `FINANCE_CHARGE_ASSESSED`,
/// `FINANCE_CHARGE_CHANGED`; closing on the date.
pub fn assess(
    state: &mut DemoState,
    r: &AssessRequest,
    key: &str,
    password: Option<&Secret>,
) -> Result<AssessResult, ApiError> {
    super::credit_memos::seed(state);
    if let Some(done) = state.finance.keys.get(key) {
        return Ok(done.clone());
    }
    if state.finance.rate.is_none() || state.finance.income_account.is_none() {
        return Err(refused("FINANCE_CHARGE_NOT_SET_UP"));
    }
    if r.contacts.is_empty() || r.contacts.len() > cl_api::MAX_ASSESS_CONTACTS {
        return Err(invalid());
    }
    not_future(r.assessment_date)?;
    check_closing(closing(state), &[r.assessment_date], password)?;
    let d = r.assessment_date;
    let ids: Vec<ContactId> = r.contacts.iter().map(|c| c.contact_id).collect();
    let all = clients(state, Some(&ids));
    let mut plan = Vec::new();
    for want in &r.contacts {
        let (c, name, active) = all
            .iter()
            .find(|x| x.0 == want.contact_id)
            .cloned()
            .ok_or(ApiError::NotFound { code: None })?;
        let x = charge_for(state, c, name, active, d)?;
        match x {
            Some(x) if x.row.already_assessed.is_some() => {
                return Err(conflict("FINANCE_CHARGE_ASSESSED"));
            }
            Some(x) if x.row.charge == want.expected_charge && x.row.charge.is_positive() => {
                plan.push(x);
            }
            other => {
                let charge = other.map_or_else(|| "0.00".to_string(), |x| x.row.charge.to_wire());
                return Err(super::receivables::conflict_with(
                    "FINANCE_CHARGE_CHANGED",
                    &serde_json::json!({"contactId": c.to_string(), "charge": charge}),
                ));
            }
        }
    }
    state.finance.next += 1;
    let run = FinanceChargeRunId::parse(&format!(
        "d7100000-0000-4000-8000-{:012}",
        state.finance.next
    ))
    .map_err(|_| invalid())?;
    let mut invoices = Vec::new();
    for x in plan {
        let id = new_id(state)?;
        let number = super::invoices::take_number(state);
        super::invoices::seed_receivable(state, id, x.row.contact_id, &number, d, d, x.row.charge)?;
        let print_later = state.finance.print_later.unwrap_or(true);
        super::invoices::mark_finance_charge(state, id, print_later);
        state.finance.charges.insert(id);
        state.finance.next += 1;
        let aid = FinanceChargeAssessmentId::parse(&format!(
            "d7000000-0000-4000-8000-{:012}",
            state.finance.next
        ))
        .map_err(|_| invalid())?;
        state.finance.assessments.push(Assessed {
            id: aid,
            run,
            client: x.row.contact_id,
            period: period_of(d),
            date: d,
            invoice: id,
            number: number.clone(),
            total: dec(x.row.charge),
            sources: x.sources,
        });
        invoices.push(AssessedInvoice {
            contact_id: x.row.contact_id,
            invoice_id: id,
            number: Some(number),
            total: x.row.charge,
        });
    }
    let result = AssessResult {
        run_id: run,
        period: period_of(d),
        invoices,
    };
    state.finance.keys.insert(key.to_string(), result.clone());
    Ok(result)
}

/// `GET /finance-charges?period=`.
#[must_use]
pub fn list(state: &DemoState, period: &str) -> Vec<Assessment> {
    state
        .finance
        .assessments
        .iter()
        .filter(|a| a.period == period)
        .filter_map(|a| {
            Some(Assessment {
                assessment_id: a.id,
                run_id: a.run,
                contact_id: a.client,
                period: a.period.clone(),
                date: a.date,
                invoice_id: a.invoice,
                number: Some(a.number.clone()),
                total: money(a.total).ok()?,
                voided: !live(state, a),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "finance_charges_tests.rs"]
mod tests;
