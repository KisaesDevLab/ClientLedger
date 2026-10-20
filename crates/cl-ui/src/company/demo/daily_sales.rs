//! The demo engine's daily sales (P14 WP-D.3): layouts, days, the preview
//! with its over/short, post and void. Synthetic.
//!
//! It plays the engine's `/daily-sales` part for `--demo` only:
//! - **Layouts** (templates): the sample's "Plant sale counter" on first
//!   use; create (blank or from the retail/restaurant preset), rename,
//!   replace lines (lines left out are deactivated) and delete.
//! - **Days** (entries): drafts, the preview, post and void, with
//!   `daily-sales-lock-v1` (the closing password is "demo-close").
//! - [`compute`] is an exact `Money` twin of the engine's `computeEntry`:
//!   inactive lines, missing and zero values are skipped; a line without
//!   an account is "unmapped"; `over_short = debits − credits`. Posting
//!   adds the over/short line on Cash Over/Short (60900): a negative
//!   figure (short) is a debit, a positive one (over) a credit. The sales
//!   tax line is typed by the owner and posts to Sales Tax Payable (20900).
//!
//! Engine 400s are `ApiError::Local` here (the demo cannot build an engine
//! validation body).

use std::fmt;

use cl_api::{Account, AccountType, ApiError, LocalValidation};
use cl_core::ids::{AccountId, TransactionId};
use cl_core::{CalendarDate, Money, MoneyError, Secret};

use super::DemoState;
use super::register::{DemoJournal, DemoLine, check_closing};

/// The most lines a layout has, and the most values a day has.
pub const MAX_LINES: usize = 200;

/// A layout line's section (the engine's `section`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// Sales (credit).
    Sales,
    /// Sales tax collected (credit).
    Tax,
    /// Tips (credit).
    Tips,
    /// Discounts and returns (debit).
    Discount,
    /// Tenders: cash, cards (debit).
    Payment,
    /// Paid out of the drawer (debit).
    Payout,
    /// Anything else (credit).
    Other,
}

impl Section {
    /// The engine's name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Section::Sales => "sales",
            Section::Tax => "tax",
            Section::Tips => "tips",
            Section::Discount => "discount",
            Section::Payment => "payment",
            Section::Payout => "payout",
            Section::Other => "other",
        }
    }

    /// The section of an engine name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Section> {
        [
            Section::Sales,
            Section::Tax,
            Section::Tips,
            Section::Discount,
            Section::Payment,
            Section::Payout,
            Section::Other,
        ]
        .into_iter()
        .find(|x| x.as_str() == s)
    }

    /// The fixed side (Q-P14s): discount, payment and payout lines are
    /// debits; sales, tax, tips and other lines credits.
    #[must_use]
    pub const fn is_debit(self) -> bool {
        matches!(self, Section::Discount | Section::Payment | Section::Payout)
    }
}

/// One line of a layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoLayoutLine {
    /// Line id (0 on a new line sent to [`replace_lines`]).
    pub id: u32,
    /// Section.
    pub section: Section,
    /// The label the owner sees.
    pub label: String,
    /// The account it posts to (`None`: unmapped).
    pub account: Option<AccountId>,
    /// Normal side is debit (the engine's `normalSide`).
    pub debit: bool,
    /// Active (inactive lines are skipped).
    pub active: bool,
}

/// A layout (the engine's daily-sales template).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DemoLayout {
    /// Id.
    pub id: u32,
    /// Name.
    pub name: String,
    /// `retail`, `restaurant` or `custom`.
    pub preset: String,
    /// Active (a deleted layout is not listed).
    pub active: bool,
    /// Lines in order.
    pub lines: Vec<DemoLayoutLine>,
}

/// A day's status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DayStatus {
    /// Saved, not posted.
    Draft,
    /// Posted.
    Posted,
    /// Voided.
    Void,
}

/// One day (the engine's daily-sales entry).
#[derive(Clone, PartialEq, Eq)]
pub struct DemoDay {
    /// Id.
    pub id: u32,
    /// Its layout.
    pub layout: u32,
    /// Business date.
    pub date: CalendarDate,
    /// Notes.
    pub notes: Option<String>,
    /// (line id, amount).
    pub values: Vec<(u32, Money)>,
    /// Status.
    pub status: DayStatus,
    /// The posted transaction.
    pub transaction: Option<TransactionId>,
    /// The over/short posted (debits − credits), once posted.
    pub over_short: Option<Money>,
}

/// Ids, the date and the status only.
impl fmt::Debug for DemoDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDay")
            .field("id", &self.id)
            .field("layout", &self.layout)
            .field("date", &self.date)
            .field("status", &self.status)
            .field("values", &self.values.len())
            .finish_non_exhaustive()
    }
}

/// What [`compute`] makes of a day.
#[derive(Clone, PartialEq, Eq)]
pub struct DemoComputed {
    /// The journal lines (without the over/short line).
    pub lines: Vec<DemoLine>,
    /// Σ debit-side amounts.
    pub debits: Money,
    /// Σ credit-side amounts.
    pub credits: Money,
    /// `debits − credits`: negative is short, positive over.
    pub over_short: Money,
    /// Σ sales lines.
    pub sales: Money,
    /// Σ tax lines.
    pub tax: Money,
    /// Σ tender lines.
    pub payments: Money,
    /// Labels of lines with an amount and no account.
    pub unmapped: Vec<String>,
}

/// Counts only: amounts are owner data.
impl fmt::Debug for DemoComputed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoComputed")
            .field("lines", &self.lines.len())
            .field("unmapped", &self.unmapped.len())
            .finish_non_exhaustive()
    }
}

/// The preview (`POST /daily-sales/entries/preview`).
#[derive(Clone, PartialEq, Eq)]
pub struct DemoPreview {
    /// Σ debits.
    pub debits: Money,
    /// Σ credits.
    pub credits: Money,
    /// `debits − credits`.
    pub over_short: Money,
    /// Within the engine's tolerance (0.0001), i.e. zero.
    pub balanced: bool,
    /// Σ sales.
    pub sales: Money,
    /// Σ tax.
    pub tax: Money,
    /// Σ tenders.
    pub payments: Money,
    /// Unmapped labels.
    pub unmapped: Vec<String>,
}

/// The flag and counts only.
impl fmt::Debug for DemoPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoPreview")
            .field("balanced", &self.balanced)
            .field("unmapped", &self.unmapped.len())
            .finish_non_exhaustive()
    }
}

/// The demo's layouts and days.
#[derive(Clone, Default)]
pub struct DemoDailySales {
    layouts: Option<Vec<DemoLayout>>,
    days: Vec<DemoDay>,
    next_id: u32,
    /// Writes that reached the demo engine (tests prove a refused post
    /// sent nothing).
    pub writes: u32,
}

impl fmt::Debug for DemoDailySales {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDailySales")
            .field("layouts", &self.layouts.as_ref().map(Vec::len))
            .field("days", &self.days.len())
            .field("writes", &self.writes)
            .finish_non_exhaustive()
    }
}

impl DemoDailySales {
    fn fresh(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
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

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn not_found() -> ApiError {
    ApiError::NotFound { code: None }
}

/// The exact twin of the engine's `computeEntry`.
///
/// # Errors
/// A sum outside the money range.
pub fn compute(layout: &DemoLayout, values: &[(u32, Money)]) -> Result<DemoComputed, MoneyError> {
    let mut c = DemoComputed {
        lines: Vec::new(),
        debits: Money::ZERO,
        credits: Money::ZERO,
        over_short: Money::ZERO,
        sales: Money::ZERO,
        tax: Money::ZERO,
        payments: Money::ZERO,
        unmapped: Vec::new(),
    };
    for line in layout.lines.iter().filter(|l| l.active) {
        let Some(&(_, amount)) = values.iter().find(|(id, _)| *id == line.id) else {
            continue;
        };
        if amount.is_zero() {
            continue;
        }
        let Some(account) = line.account else {
            c.unmapped.push(line.label.clone());
            continue;
        };
        if line.debit {
            c.debits = c.debits.checked_add(amount)?;
        } else {
            c.credits = c.credits.checked_add(amount)?;
        }
        match line.section {
            Section::Sales => c.sales = c.sales.checked_add(amount)?,
            Section::Tax => c.tax = c.tax.checked_add(amount)?,
            Section::Payment => c.payments = c.payments.checked_add(amount)?,
            _ => {}
        }
        c.lines.push(DemoLine {
            account,
            debit: if line.debit { amount } else { Money::ZERO },
            credit: if line.debit { Money::ZERO } else { amount },
            memo: Some(line.label.clone()),
        });
    }
    c.over_short = c.debits.checked_sub(c.credits)?;
    Ok(c)
}

fn by_number(chart: &[Account], n: &str) -> Option<AccountId> {
    chart
        .iter()
        .find(|a| a.fields.account_number.as_deref() == Some(n))
        .map(|a| a.id)
}

fn by_tag(chart: &[Account], tag: &str) -> Option<AccountId> {
    chart
        .iter()
        .find(|a| a.system_tag.as_deref() == Some(tag))
        .map(|a| a.id)
}

fn layouts_mut(state: &mut DemoState) -> &mut Vec<DemoLayout> {
    if state.daily_sales.layouts.is_none() {
        let chart = chart(state);
        let sample = cl_company::Sample::load();
        let mut out = Vec::new();
        for l in &sample.company.daily_sales_layouts {
            let accounts = l.resolved_accounts(|n| by_number(&chart, n).is_some());
            let id = state.daily_sales.fresh();
            let mut lines = Vec::new();
            for (sl, account) in l.lines.iter().zip(accounts) {
                let section = Section::parse(&sl.section).unwrap_or(Section::Other);
                lines.push(DemoLayoutLine {
                    id: state.daily_sales.fresh(),
                    section,
                    label: sl.label.clone(),
                    account: account.and_then(|n| by_number(&chart, &n)),
                    debit: sl.side == cl_company::sample::LayoutSide::Debit,
                    active: true,
                });
            }
            out.push(DemoLayout {
                id,
                name: l.name.clone(),
                preset: l.preset.clone(),
                active: true,
                lines,
            });
        }
        state.daily_sales.layouts = Some(out);
    }
    state.daily_sales.layouts.get_or_insert_with(Vec::new)
}

/// The active layouts, by name.
#[must_use]
pub fn layouts(state: &mut DemoState) -> Vec<DemoLayout> {
    let mut v: Vec<DemoLayout> = layouts_mut(state)
        .iter()
        .filter(|l| l.active)
        .cloned()
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// One layout (a deleted one too, as the engine's GET answers it).
#[must_use]
pub fn layout(state: &mut DemoState, id: u32) -> Option<DemoLayout> {
    layouts_mut(state).iter().find(|l| l.id == id).cloned()
}

/// The engine preset's lines (`presetType`), mapped as the engine maps
/// them: a system-tag line to that system account, any other to the first
/// active account of its suggested type (else unmapped).
fn preset_lines(
    chart: &[Account],
    preset: &str,
) -> Vec<(Section, &'static str, Option<AccountId>)> {
    let first = |t: AccountType| {
        chart
            .iter()
            .find(|a| a.is_active && a.fields.account_type == t)
            .map(|a| a.id)
    };
    let revenue = first(AccountType::Revenue);
    let expense = first(AccountType::Expense);
    let tax = by_tag(chart, "sales_tax_payable");
    let clearing = by_tag(chart, "payments_clearing");
    let gift = by_tag(chart, "gift_card_liability");
    let tips = by_tag(chart, "tips_payable");
    match preset {
        "retail" => vec![
            (Section::Sales, "Department 1 Sales", revenue),
            (Section::Sales, "Department 2 Sales", revenue),
            (Section::Discount, "Returns / Discounts", revenue),
            (Section::Tax, "Sales Tax Collected", tax),
            (Section::Payment, "Cash", clearing),
            (Section::Payment, "Credit / Debit Cards", clearing),
            (Section::Payment, "Other Tender", clearing),
            (Section::Payment, "Gift Card Redeemed", clearing),
            (Section::Other, "Gift Cards Sold", gift),
            (Section::Payout, "Petty-Cash Payouts", expense),
        ],
        "restaurant" => vec![
            (Section::Sales, "Food Sales", revenue),
            (Section::Sales, "Beverage / Liquor Sales", revenue),
            (Section::Discount, "Comps / Discounts", revenue),
            (Section::Tax, "Sales Tax Collected", tax),
            (Section::Tips, "Tips / Gratuity", tips),
            (Section::Payment, "Cash", clearing),
            (Section::Payment, "Visa / Mastercard", clearing),
            (Section::Payment, "American Express", clearing),
            (Section::Payment, "Gift Card Redeemed", clearing),
            (Section::Other, "Gift Cards Sold", gift),
            (Section::Payout, "Paid-Outs (from drawer)", expense),
        ],
        _ => Vec::new(),
    }
}

/// Creates a layout, blank or from a preset (`retail`, `restaurant`).
///
/// # Errors
/// `Local(InvalidValue)` for an empty name or an unknown preset.
pub fn create_layout(
    state: &mut DemoState,
    name: &str,
    preset: Option<&str>,
) -> Result<u32, ApiError> {
    state.daily_sales.writes += 1;
    let preset = preset.unwrap_or("custom");
    if name.trim().is_empty() || !matches!(preset, "custom" | "retail" | "restaurant") {
        return Err(invalid());
    }
    let chart = chart(state);
    layouts_mut(state);
    let id = state.daily_sales.fresh();
    let mut lines = Vec::new();
    for (section, label, account) in preset_lines(&chart, preset) {
        lines.push(DemoLayoutLine {
            id: state.daily_sales.fresh(),
            section,
            label: label.to_string(),
            account,
            debit: section.is_debit(),
            active: true,
        });
    }
    layouts_mut(state).push(DemoLayout {
        id,
        name: name.to_string(),
        preset: preset.to_string(),
        active: true,
        lines,
    });
    Ok(id)
}

/// Renames a layout.
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for an empty name.
pub fn rename_layout(state: &mut DemoState, id: u32, name: &str) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    if name.trim().is_empty() {
        return Err(invalid());
    }
    let l = layouts_mut(state)
        .iter_mut()
        .find(|l| l.id == id && l.active)
        .ok_or_else(not_found)?;
    l.name = name.to_string();
    Ok(())
}

/// Deletes a layout (it is no longer listed; posted days keep it).
///
/// # Errors
/// `NotFound`.
pub fn delete_layout(state: &mut DemoState, id: u32) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    let l = layouts_mut(state)
        .iter_mut()
        .find(|l| l.id == id && l.active)
        .ok_or_else(not_found)?;
    l.active = false;
    Ok(())
}

/// Replaces a layout's lines (`PUT /templates/{id}/lines`): a line with a
/// known id is updated, id 0 is added, and a line left out is deactivated.
///
/// # Errors
/// `NotFound`; `Local(TooManyLines)` above [`MAX_LINES`];
/// `Local(InvalidValue)` for an empty label, an unknown line id or an
/// account outside the chart.
pub fn replace_lines(
    state: &mut DemoState,
    id: u32,
    lines: &[DemoLayoutLine],
) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    if lines.len() > MAX_LINES {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    let chart = chart(state);
    let current = layout(state, id)
        .filter(|l| l.active)
        .ok_or_else(not_found)?;
    let ok = lines.iter().all(|l| {
        !l.label.trim().is_empty()
            && (l.id == 0 || current.lines.iter().any(|c| c.id == l.id))
            && l.account.is_none_or(|a| chart.iter().any(|x| x.id == a))
    });
    if !ok {
        return Err(invalid());
    }
    let mut next = Vec::new();
    for l in lines {
        let mut l = l.clone();
        if l.id == 0 {
            l.id = state.daily_sales.fresh();
        }
        next.push(l);
    }
    for old in &current.lines {
        if !next.iter().any(|l| l.id == old.id) {
            let mut gone = old.clone();
            gone.active = false;
            next.push(gone);
        }
    }
    if let Some(l) = layouts_mut(state).iter_mut().find(|l| l.id == id) {
        l.lines = next;
    }
    Ok(())
}

fn check_values(layout: &DemoLayout, values: &[(u32, Money)]) -> Result<(), ApiError> {
    if values.len() > MAX_LINES {
        return Err(ApiError::Local(LocalValidation::TooManyLines));
    }
    if values.iter().any(|(_, m)| m.is_negative()) {
        return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
    }
    if !values
        .iter()
        .all(|(id, _)| layout.lines.iter().any(|l| l.id == *id))
    {
        return Err(invalid());
    }
    Ok(())
}

/// Saves a day as a draft.
///
/// # Errors
/// `NotFound` (layout); `Local(NonPositiveAmount)` for a negative amount;
/// `Local(TooManyLines)`; `Local(InvalidValue)` for a line of another
/// layout.
pub fn create_day(
    state: &mut DemoState,
    layout_id: u32,
    date: CalendarDate,
    notes: Option<String>,
    values: &[(u32, Money)],
) -> Result<u32, ApiError> {
    state.daily_sales.writes += 1;
    let l = layout(state, layout_id)
        .filter(|l| l.active)
        .ok_or_else(not_found)?;
    check_values(&l, values)?;
    let id = state.daily_sales.fresh();
    state.daily_sales.days.push(DemoDay {
        id,
        layout: layout_id,
        date,
        notes,
        values: values.to_vec(),
        status: DayStatus::Draft,
        transaction: None,
        over_short: None,
    });
    Ok(id)
}

/// Changes a draft day.
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for a posted or void day; as
/// [`create_day`].
pub fn update_day(
    state: &mut DemoState,
    id: u32,
    date: CalendarDate,
    notes: Option<String>,
    values: &[(u32, Money)],
) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    let d = day(state, id).ok_or_else(not_found)?;
    if d.status != DayStatus::Draft {
        return Err(invalid());
    }
    let l = layout(state, d.layout).ok_or_else(not_found)?;
    check_values(&l, values)?;
    if let Some(d) = state.daily_sales.days.iter_mut().find(|d| d.id == id) {
        d.date = date;
        d.notes = notes;
        d.values = values.to_vec();
    }
    Ok(())
}

/// Deletes a draft day.
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for a posted or void day.
pub fn delete_day(state: &mut DemoState, id: u32) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    let d = day(state, id).ok_or_else(not_found)?;
    if d.status != DayStatus::Draft {
        return Err(invalid());
    }
    state.daily_sales.days.retain(|d| d.id != id);
    Ok(())
}

/// One day.
#[must_use]
pub fn day(state: &DemoState, id: u32) -> Option<DemoDay> {
    state.daily_sales.days.iter().find(|d| d.id == id).cloned()
}

/// The days with `status` and on `date` (each when given), newest first
/// (the outcome probe lists drafts of a date).
#[must_use]
pub fn days(
    state: &DemoState,
    status: Option<DayStatus>,
    date: Option<CalendarDate>,
) -> Vec<DemoDay> {
    let mut v: Vec<DemoDay> = state
        .daily_sales
        .days
        .iter()
        .filter(|d| status.is_none_or(|s| d.status == s) && date.is_none_or(|x| d.date == x))
        .cloned()
        .collect();
    v.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    v
}

/// The preview of unsaved values (nothing is stored).
///
/// # Errors
/// `NotFound` (layout); a sum outside the money range.
pub fn preview(
    state: &mut DemoState,
    layout_id: u32,
    values: &[(u32, Money)],
) -> Result<DemoPreview, ApiError> {
    let l = layout(state, layout_id).ok_or_else(not_found)?;
    let c = compute(&l, values).map_err(|_| invalid())?;
    Ok(DemoPreview {
        debits: c.debits,
        credits: c.credits,
        over_short: c.over_short,
        balanced: c.over_short.is_zero(),
        sales: c.sales,
        tax: c.tax,
        payments: c.payments,
        unmapped: c.unmapped,
    })
}

/// Posts a draft day: its computed lines plus the over/short line on Cash
/// Over/Short. The tenders wait in Payments Clearing for a deposit.
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for a day that is not a draft, an
/// unmapped line with an amount or no amounts at all;
/// `ClosedPeriod` / `ClosingPasswordRejected` (`daily-sales-lock-v1`).
pub fn post(
    state: &mut DemoState,
    id: u32,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    state.daily_sales.writes += 1;
    let d = day(state, id).ok_or_else(not_found)?;
    if d.status != DayStatus::Draft {
        return Err(invalid());
    }
    let l = layout(state, d.layout).ok_or_else(not_found)?;
    let c = compute(&l, &d.values).map_err(|_| invalid())?;
    if !c.unmapped.is_empty() || c.lines.is_empty() {
        return Err(invalid());
    }
    check_closing(closing(state), &[d.date], password)?;
    let chart = chart(state);
    let mut lines = c.lines;
    if !c.over_short.is_zero() {
        let account = by_tag(&chart, "cash_over_short").ok_or_else(invalid)?;
        let short = c.over_short.is_negative();
        lines.push(DemoLine {
            account,
            debit: if short {
                c.over_short.abs()
            } else {
                Money::ZERO
            },
            credit: if short { Money::ZERO } else { c.over_short },
            memo: Some("Cash Over/Short".into()),
        });
    }
    let journal = DemoJournal {
        kind: "daily_sales",
        date: d.date,
        number: String::new(),
        contact: None,
        memo: format!(
            "Daily sales \u{2014} {} \u{2014} {}",
            l.name,
            d.date.to_wire()
        ),
        lines,
    };
    let txn = state
        .ledger
        .save_lines(&chart, &journal, None)
        .ok_or_else(invalid)?;
    if let Some(x) = state.daily_sales.days.iter_mut().find(|x| x.id == id) {
        x.status = DayStatus::Posted;
        x.transaction = Some(txn);
        x.over_short = Some(c.over_short);
    }
    Ok(txn)
}

/// Voids a posted day (its posting is voided).
///
/// # Errors
/// `NotFound`; `Local(InvalidValue)` for a day that is not posted; the
/// closing-date rule; `Conflict` `TXN_DEPOSITED` while a deposit holds its
/// tenders.
pub fn void_day(state: &mut DemoState, id: u32, password: Option<&Secret>) -> Result<(), ApiError> {
    state.daily_sales.writes += 1;
    let d = day(state, id).ok_or_else(not_found)?;
    let (DayStatus::Posted, Some(txn)) = (d.status, d.transaction) else {
        return Err(invalid());
    };
    check_closing(closing(state), &[d.date], password)?;
    state.ledger.void(txn)?;
    if let Some(x) = state.daily_sales.days.iter_mut().find(|x| x.id == id) {
        x.status = DayStatus::Void;
    }
    Ok(())
}

// ------------------------------------------------- the screen's DailyBooks

/// Ids of the demo's layouts, lines and days as engine UUIDs (the low 12
/// hex digits carry the demo's number).
const LAYOUT_ID: &str = "da170000-0000-4000-8000-";
const LINE_ID: &str = "da171000-0000-4000-8000-";
const DAY_ID: &str = "da172000-0000-4000-8000-";

fn to_uuid<T: std::str::FromStr>(prefix: &str, n: u32) -> Result<T, ApiError> {
    format!("{prefix}{n:012x}")
        .parse()
        .map_err(|_| ApiError::Internal)
}

fn from_uuid(prefix: &str, id: &impl fmt::Display) -> Result<u32, ApiError> {
    let s = id.to_string();
    s.strip_prefix(prefix)
        .and_then(|n| u32::from_str_radix(n, 16).ok())
        .ok_or_else(not_found)
}

fn layout_id(id: cl_core::ids::DailyTemplateId) -> Result<u32, ApiError> {
    from_uuid(LAYOUT_ID, &id)
}

fn day_id(id: cl_core::ids::DailyEntryId) -> Result<u32, ApiError> {
    from_uuid(DAY_ID, &id)
}

fn template_of(l: &DemoLayout) -> Result<cl_api::DailyTemplate, ApiError> {
    let mut lines = Vec::with_capacity(l.lines.len());
    for (i, x) in l.lines.iter().enumerate() {
        lines.push(cl_api::DailyLine {
            id: to_uuid(LINE_ID, x.id)?,
            section: cl_api::Section::from_wire(x.section.as_str()),
            label: x.label.clone(),
            account: x.account,
            tag: None,
            side: if x.debit {
                cl_api::NormalSide::Debit
            } else {
                cl_api::NormalSide::Credit
            },
            required: false,
            allow_tag: false,
            active: x.active,
            sort: u32::try_from(i).map_err(|_| invalid())?,
        });
    }
    Ok(cl_api::DailyTemplate {
        id: to_uuid(LAYOUT_ID, l.id)?,
        name: l.name.clone(),
        preset: match l.preset.as_str() {
            "retail" => cl_api::Preset::Retail,
            "restaurant" => cl_api::Preset::Restaurant,
            _ => cl_api::Preset::Custom,
        },
        is_active: l.active,
        default_tag: None,
        lines,
    })
}

fn status_of(s: DayStatus) -> cl_api::EntryStatus {
    match s {
        DayStatus::Draft => cl_api::EntryStatus::Draft,
        DayStatus::Posted => cl_api::EntryStatus::Posted,
        DayStatus::Void => cl_api::EntryStatus::Void,
    }
}

fn values_in(values: &[cl_api::DailyValue]) -> Result<Vec<(u32, Money)>, ApiError> {
    values
        .iter()
        .map(|v| from_uuid(LINE_ID, &v.line).map(|id| (id, v.amount)))
        .collect()
}

/// The demo's daily sales as the screen's [`DailyBooks`]: every call locks
/// the demo state of `link` and answers at once.
///
/// [`DailyBooks`]: crate::screens::daily_sales::DailyBooks
pub struct DemoDailyBooks {
    link: std::sync::Arc<crate::company::CompanyLink>,
}

impl DemoDailyBooks {
    /// The books of `link`'s demo state.
    #[must_use]
    pub fn new(link: std::sync::Arc<crate::company::CompanyLink>) -> DemoDailyBooks {
        DemoDailyBooks { link }
    }

    fn with<T>(
        &self,
        f: impl FnOnce(&mut DemoState) -> Result<T, ApiError>,
    ) -> Result<T, ApiError> {
        let mut state = self.link.demo_state().ok_or(ApiError::Internal)?;
        f(&mut state)
    }
}

impl fmt::Debug for DemoDailyBooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDailyBooks").finish_non_exhaustive()
    }
}

fn entry_of(state: &mut DemoState, id: u32) -> Result<cl_api::DailyEntry, ApiError> {
    let d = day(state, id).ok_or_else(not_found)?;
    let l = layout(state, d.layout).ok_or_else(not_found)?;
    let c = compute(&l, &d.values).map_err(|_| invalid())?;
    let mut values = Vec::with_capacity(d.values.len());
    for (line, amount) in &d.values {
        values.push(cl_api::DailyValue {
            line: to_uuid(LINE_ID, *line)?,
            amount: *amount,
            tag: None,
        });
    }
    Ok(cl_api::DailyEntry {
        id: to_uuid(DAY_ID, d.id)?,
        template: to_uuid(LAYOUT_ID, d.layout)?,
        date: d.date,
        status: status_of(d.status),
        transaction: d.transaction,
        tag: None,
        notes: d.notes,
        values,
        over_short: d.over_short.unwrap_or(Money::ZERO),
        totals: cl_api::DailyTotals {
            sales: c.sales,
            tax: c.tax,
            payments: c.payments,
        },
        layout: Some(template_of(&l)?),
    })
}

impl crate::screens::daily_sales::DailyBooks for DemoDailyBooks {
    fn templates(&mut self) -> Result<Vec<cl_api::DailyTemplate>, ApiError> {
        self.with(|s| {
            layouts(s)
                .iter()
                .map(|l| {
                    template_of(l).map(|t| cl_api::DailyTemplate {
                        lines: Vec::new(),
                        ..t
                    })
                })
                .collect()
        })
    }

    fn template(
        &mut self,
        id: cl_core::ids::DailyTemplateId,
    ) -> Result<cl_api::DailyTemplate, ApiError> {
        let id = layout_id(id)?;
        self.with(|s| template_of(&layout(s, id).ok_or_else(not_found)?))
    }

    fn create_template(
        &mut self,
        name: &str,
        preset: Option<cl_api::Preset>,
    ) -> Result<cl_api::DailyTemplate, ApiError> {
        self.with(|s| {
            let id = create_layout(s, name, preset.map(cl_api::Preset::as_wire))?;
            template_of(&layout(s, id).ok_or_else(not_found)?)
        })
    }

    fn rename(
        &mut self,
        id: cl_core::ids::DailyTemplateId,
        name: &str,
    ) -> Result<cl_api::DailyTemplate, ApiError> {
        let id = layout_id(id)?;
        self.with(|s| {
            rename_layout(s, id, name)?;
            template_of(&layout(s, id).ok_or_else(not_found)?)
        })
    }

    fn replace_lines(
        &mut self,
        id: cl_core::ids::DailyTemplateId,
        lines: &[cl_api::DailyLineInput],
    ) -> Result<cl_api::DailyTemplate, ApiError> {
        let id = layout_id(id)?;
        let mut demo = Vec::with_capacity(lines.len());
        for l in lines {
            let section = l
                .section
                .as_wire()
                .and_then(Section::parse)
                .ok_or_else(invalid)?;
            demo.push(DemoLayoutLine {
                id: match l.id {
                    Some(x) => from_uuid(LINE_ID, &x).map_err(|_| invalid())?,
                    None => 0,
                },
                section,
                label: l.label.trim().to_string(),
                account: l.account,
                debit: l.side == cl_api::NormalSide::Debit,
                active: l.active,
            });
        }
        self.with(|s| {
            replace_lines(s, id, &demo)?;
            template_of(&layout(s, id).ok_or_else(not_found)?)
        })
    }

    fn delete_template(&mut self, id: cl_core::ids::DailyTemplateId) -> Result<(), ApiError> {
        let id = layout_id(id)?;
        self.with(|s| delete_layout(s, id))
    }

    fn list(
        &mut self,
        q: &cl_api::DailyEntryQuery,
    ) -> Result<Vec<cl_api::DailyEntrySummary>, ApiError> {
        let status = match &q.status {
            None => None,
            Some(cl_api::EntryStatus::Draft) => Some(DayStatus::Draft),
            Some(cl_api::EntryStatus::Posted) => Some(DayStatus::Posted),
            Some(cl_api::EntryStatus::Void) => Some(DayStatus::Void),
            Some(cl_api::EntryStatus::Unknown(_)) => return Ok(Vec::new()),
        };
        let template = q.template.map(layout_id).transpose()?;
        let limit = usize::from(q.limit.unwrap_or(500));
        self.with(|s| {
            let mut out = Vec::new();
            for d in days(s, status, q.date)
                .into_iter()
                .filter(|d| template.is_none_or(|t| t == d.layout))
                .take(limit)
            {
                let l = layout(s, d.layout).ok_or_else(not_found)?;
                let sales = compute(&l, &d.values).map_err(|_| invalid())?.sales;
                out.push(cl_api::DailyEntrySummary {
                    id: to_uuid(DAY_ID, d.id)?,
                    template: to_uuid(LAYOUT_ID, d.layout)?,
                    template_name: Some(l.name.clone()),
                    date: d.date,
                    status: status_of(d.status),
                    transaction: d.transaction,
                    over_short: d.over_short.unwrap_or(Money::ZERO),
                    sales,
                });
            }
            Ok(out)
        })
    }

    fn entry(&mut self, id: cl_core::ids::DailyEntryId) -> Result<cl_api::DailyEntry, ApiError> {
        let id = day_id(id)?;
        self.with(|s| entry_of(s, id))
    }

    fn create_draft(
        &mut self,
        d: &cl_api::DailyEntryDraft,
    ) -> Result<cl_api::DailyEntry, ApiError> {
        let layout = layout_id(d.template)?;
        let values = values_in(&d.values).map_err(|_| invalid())?;
        self.with(|s| {
            let id = create_day(s, layout, d.date, d.notes.clone(), &values)?;
            entry_of(s, id)
        })
    }

    fn update_draft(
        &mut self,
        id: cl_core::ids::DailyEntryId,
        d: &cl_api::DailyEntryDraft,
    ) -> Result<cl_api::DailyEntry, ApiError> {
        let id = day_id(id)?;
        let values = values_in(&d.values).map_err(|_| invalid())?;
        self.with(|s| {
            update_day(s, id, d.date, d.notes.clone(), &values)?;
            entry_of(s, id)
        })
    }

    fn preview(
        &mut self,
        template: cl_core::ids::DailyTemplateId,
        values: &[cl_api::DailyValue],
    ) -> Result<cl_api::DailyPreview, ApiError> {
        let layout = layout_id(template)?;
        let values = values_in(values).map_err(|_| invalid())?;
        self.with(|s| {
            let p = preview(s, layout, &values)?;
            Ok(cl_api::DailyPreview {
                debits: p.debits,
                credits: p.credits,
                over_short: p.over_short,
                balanced: p.balanced,
                sales: p.sales,
                tax: p.tax,
                payments: p.payments,
                unmapped: p.unmapped,
            })
        })
    }

    fn post(
        &mut self,
        id: cl_core::ids::DailyEntryId,
        password: Option<&Secret>,
    ) -> Result<cl_api::DailyEntry, ApiError> {
        let id = day_id(id)?;
        self.with(|s| {
            post(s, id, password)?;
            entry_of(s, id)
        })
    }

    fn void(
        &mut self,
        id: cl_core::ids::DailyEntryId,
        password: Option<&Secret>,
    ) -> Result<cl_api::DailyEntry, ApiError> {
        let id = day_id(id)?;
        self.with(|s| {
            void_day(s, id, password)?;
            entry_of(s, id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(s: &str) -> Money {
        Money::parse_wire(s).expect("m")
    }

    fn story() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("d")
    }

    fn counter(state: &mut DemoState) -> DemoLayout {
        layouts(state)
            .into_iter()
            .find(|l| l.name == "Plant sale counter")
            .expect("layout")
    }

    fn values(l: &DemoLayout, amounts: &[&str]) -> Vec<(u32, Money)> {
        l.lines
            .iter()
            .zip(amounts)
            .map(|(x, a)| (x.id, m(a)))
            .collect()
    }

    /// Mirrors `contract_daily_sales`: 1,000.00 sales, 80.00 tax, cash
    /// 500.00 and cards 577.65 preview 2.35 short and post Cr sales 1,000,
    /// Cr 20900 80.00, Dr 10150 1,077.65 and Dr 60900 2.35; the tenders
    /// wait for a deposit; void reverses it.
    #[test]
    fn short_day_posts_over_short_and_tax() {
        let mut state = DemoState::new();
        let l = counter(&mut state);
        let v = values(&l, &["1000.00", "80.00", "500.00", "577.65"]);
        let p = preview(&mut state, l.id, &v).expect("preview");
        assert_eq!(p.over_short, m("-2.35"));
        assert!(!p.balanced);
        assert_eq!(
            (p.sales, p.tax, p.payments),
            (m("1000.00"), m("80.00"), m("1077.65"))
        );
        assert_eq!(p.unmapped, Vec::<String>::new());

        let id = create_day(&mut state, l.id, story(), None, &v).expect("draft");
        assert_eq!(days(&state, Some(DayStatus::Draft), Some(story())).len(), 1);
        let txn = post(&mut state, id, None).expect("post");
        let t = state
            .ledger
            .transactions()
            .into_iter()
            .find(|t| super::super::register::DemoLedger::id_of(t) == txn)
            .expect("posted");
        let side = |n: &str| {
            t.lines
                .iter()
                .filter(|x| x.account == n)
                .map(|x| (x.debit, x.credit))
                .collect::<Vec<_>>()
        };
        assert_eq!(side("60900"), vec![(m("2.35"), Money::ZERO)]);
        assert_eq!(side("20900"), vec![(Money::ZERO, m("80.00"))]);
        assert_eq!(side("4000"), vec![(Money::ZERO, m("1000.00"))]);
        assert_eq!(side("10150").len(), 2);
        assert_eq!(day(&state, id).and_then(|d| d.over_short), Some(m("-2.35")));
        // The tenders wait in Payments Clearing (one row per transaction).
        let waiting = super::super::deposits::pending(&mut state);
        assert!(
            waiting
                .iter()
                .any(|p| p.source == txn && p.amount == m("1077.65"))
        );
        // Posting twice is refused.
        assert_eq!(post(&mut state, id, None), Err(invalid()));

        void_day(&mut state, id, None).expect("void");
        assert_eq!(day(&state, id).map(|d| d.status), Some(DayStatus::Void));
        assert!(
            !super::super::deposits::pending(&mut state)
                .iter()
                .any(|p| p.source == txn)
        );
    }

    /// The over case: cash 503.00 → Cr Cash Over/Short 0.65. An exact day
    /// posts no over/short line.
    #[test]
    fn over_and_even_days() {
        let mut state = DemoState::new();
        let l = counter(&mut state);
        let v = values(&l, &["1000.00", "80.00", "503.00", "577.65"]);
        let id = create_day(&mut state, l.id, story(), None, &v).expect("draft");
        let txn = post(&mut state, id, None).expect("post");
        let t = state
            .ledger
            .transactions()
            .into_iter()
            .find(|t| super::super::register::DemoLedger::id_of(t) == txn)
            .expect("posted");
        let os: Vec<_> = t.lines.iter().filter(|x| x.account == "60900").collect();
        assert_eq!(os.len(), 1);
        assert_eq!((os[0].debit, os[0].credit), (Money::ZERO, m("0.65")));

        let v = values(&l, &["1000.00", "80.00", "502.35", "577.65"]);
        let c = compute(&l, &v).expect("c");
        assert!(c.over_short.is_zero());
        let id = create_day(&mut state, l.id, story(), None, &v).expect("draft");
        let txn = post(&mut state, id, None).expect("post");
        let t = state
            .ledger
            .transactions()
            .into_iter()
            .find(|t| super::super::register::DemoLedger::id_of(t) == txn)
            .expect("posted");
        assert!(!t.lines.iter().any(|x| x.account == "60900"));
    }

    /// The twin skips inactive and zero lines and reports unmapped ones;
    /// refusals: negative amounts, unmapped lines, closed periods.
    #[test]
    fn twin_rules_and_refusals() {
        let mut state = DemoState::new();
        let id = create_layout(&mut state, "Counter 2", Some("retail")).expect("layout");
        let mut l = layout(&mut state, id).expect("l");
        assert_eq!(l.lines.len(), 10);
        // Gift Cards Sold has no account in the demo chart.
        let gift = l
            .lines
            .iter()
            .find(|x| x.label == "Gift Cards Sold")
            .expect("g")
            .id;
        let v = vec![
            (l.lines[0].id, m("10.00")),
            (gift, m("5.00")),
            (l.lines[1].id, Money::ZERO),
        ];
        let c = compute(&l, &v).expect("c");
        assert_eq!(c.unmapped, vec!["Gift Cards Sold".to_string()]);
        assert_eq!(c.lines.len(), 1);
        assert_eq!(c.over_short, m("-10.00"));
        let draft = create_day(&mut state, id, story(), None, &v).expect("draft");
        assert_eq!(post(&mut state, draft, None), Err(invalid()));

        // Deactivated lines are skipped.
        l.lines[0].active = false;
        assert_eq!(compute(&l, &v).expect("c").lines, Vec::new());

        // Replace lines: a new line is added, a missing one deactivated.
        let mut lines = l.lines.clone();
        lines.truncate(9);
        lines.push(DemoLayoutLine {
            id: 0,
            section: Section::Payment,
            label: "Gift cards".into(),
            account: super::super::deposits::clearing_account(&mut state),
            debit: true,
            active: true,
        });
        replace_lines(&mut state, id, &lines).expect("replace");
        let after = layout(&mut state, id).expect("l");
        assert_eq!(after.lines.iter().filter(|x| x.active).count(), 9);
        assert_eq!(after.lines.len(), 11);

        assert_eq!(
            create_day(
                &mut state,
                id,
                story(),
                None,
                &[(after.lines[1].id, m("-1.00"))]
            ),
            Err(ApiError::Local(LocalValidation::NonPositiveAmount))
        );
        delete_layout(&mut state, id).expect("delete");
        assert!(layouts(&mut state).iter().all(|x| x.name != "Counter 2"));

        // A closed period needs the password.
        let l = counter(&mut state);
        let v = values(&l, &["100.00", "8.00", "108.00"]);
        let sept = CalendarDate::from_ymd(2026, 9, 30).expect("d");
        let d = create_day(&mut state, l.id, sept, None, &v).expect("draft");
        assert!(matches!(
            post(&mut state, d, None),
            Err(ApiError::ClosedPeriod(_))
        ));
        let pw = Secret::new(super::super::register::DEMO_CLOSING_PASSWORD);
        post(&mut state, d, Some(&pw)).expect("with the password");
        assert!(matches!(
            void_day(&mut state, d, None),
            Err(ApiError::ClosedPeriod(_))
        ));
        void_day(&mut state, d, Some(&pw)).expect("void with the password");
    }
}
