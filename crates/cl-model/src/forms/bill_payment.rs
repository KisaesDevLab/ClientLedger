//! Pay Bills (P18): which bills are paid, how much of each, the discount
//! and credits on each, and the totals the screen shows. No egui.
//!
//! Each row keeps three separate amounts, and the engine gets them back in
//! its own terms ([`PayBillsForm::plan`]):
//!
//! - **cash**: what the owner pays by check ("Amt. to pay"). Until the
//!   owner types it, it follows the bill: balance due − discount − credits.
//! - **discount**: the engine's `discountAvailable` for the payment date
//!   when "Take discounts I qualify for" is on, or what Set Discount typed.
//!   Nothing here multiplies a percent (the engine computes it, U-101).
//! - **credits**: vendor credits applied to the bill. Selecting a bill
//!   applies the vendor's open credits to it automatically (oldest first,
//!   as far as the bill needs them after its discount; Q-P18i); they are
//!   fitted again when the discount changes (Take discounts, a new payment
//!   date) or the list is read again, until Set Credits sets them.
//!
//! The rules: cash ≥ 0; cash + discount + credits ≤ balance due; something
//! is applied; a credit is never used beyond what it has left. Totals
//! (Q-P18b, owner 2026-10-28): every column adds the selected bills the
//! list shows; selected bills a filter hides are kept, counted apart and
//! not paid. The ending balance after is the engine's account balance
//! minus the selected cash (Q-P18c): a preview, not a balance worked out
//! here.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::Money;
use cl_core::ids::{ContactId, TransactionId};
use cl_core::{CalendarDate, MoneyError};

use crate::text::TextKey;

/// One bill the engine lists as payable.
#[derive(Clone, PartialEq, Eq)]
pub struct PayableRow {
    /// The bill.
    pub bill: TransactionId,
    /// The vendor.
    pub vendor: Option<ContactId>,
    /// The vendor's name (owner data).
    pub vendor_name: String,
    /// A note under the vendor (the bill's memo; owner data).
    pub note: Option<String>,
    /// The vendor's invoice number, else the bill's own number.
    pub reference: Option<String>,
    /// Due date.
    pub due: Option<CalendarDate>,
    /// Last day of the early-payment discount.
    pub discount_date: Option<CalendarDate>,
    /// Terms for the selected-bill panel (`Net 30`, `2% 5, Net 15`).
    pub terms: Option<String>,
    /// What is still owed.
    pub balance_due: Money,
    /// The engine's discount for the payment date (zero for none).
    pub discount_available: Money,
}

impl fmt::Debug for PayableRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayableRow")
            .field("bill", &self.bill)
            .field("due", &self.due)
            .finish_non_exhaustive()
    }
}

/// A vendor credit with something left to apply.
#[derive(Clone, PartialEq, Eq)]
pub struct OpenCredit {
    /// The credit.
    pub credit: TransactionId,
    /// The vendor.
    pub vendor: Option<ContactId>,
    /// Its date (oldest is applied first).
    pub date: CalendarDate,
    /// Its label (`CM-114`; owner data).
    pub label: String,
    /// What it has left.
    pub open: Money,
}

impl fmt::Debug for OpenCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenCredit")
            .field("credit", &self.credit)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// Which bills are listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowBills {
    /// Due on or before this date (bills without a due date are left out).
    DueOnOrBefore(CalendarDate),
    /// Every open bill.
    All,
}

/// The list order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    /// Soonest due first (the engine's order).
    #[default]
    DueDate,
    /// Vendor name, then due date.
    Vendor,
    /// Largest amount due first.
    AmountDue,
}

/// What the owner set on one bill.
#[derive(Clone, Default, PartialEq, Eq)]
struct Entry {
    selected: bool,
    /// Typed cash (`None`: follows the bill).
    cash: Option<Money>,
    /// Set Discount (`None`: the engine's, when taking discounts).
    discount: Option<Money>,
    /// Credits applied: (credit, amount).
    credits: Vec<(TransactionId, Money)>,
    /// Set Credits chose them (else they are fitted automatically).
    credits_set: bool,
}

/// Why the run can't be paid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PayProblem {
    /// No bill selected.
    NothingSelected,
    /// No bank account.
    AccountMissing,
    /// A negative amount on a bill.
    Negative(TransactionId),
    /// Cash, discount and credits add up to more than the bill owes.
    MoreThanDue(TransactionId),
    /// Nothing applied to a selected bill.
    NothingApplied(TransactionId),
    /// A credit is used beyond what it has left.
    CreditOverused(TransactionId),
    /// A discount needs the engine update (`bill-discounts-v1`).
    DiscountNeedsUpdate(TransactionId),
    /// The amounts add up beyond what money can hold.
    TooLarge,
}

impl PayProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            PayProblem::NothingSelected => "paybills.problem.nothing_selected",
            PayProblem::AccountMissing => "paybills.problem.account_missing",
            PayProblem::Negative(_) => "paybills.problem.negative",
            PayProblem::MoreThanDue(_) => "paybills.problem.more_than_due",
            PayProblem::NothingApplied(_) => "paybills.problem.nothing_applied",
            PayProblem::CreditOverused(_) => "paybills.problem.credit_overused",
            PayProblem::DiscountNeedsUpdate(_) => "paybills.problem.discount_needs_update",
            PayProblem::TooLarge => "paybills.problem.too_large",
        })
    }

    /// The bill it is about, if one.
    #[must_use]
    pub const fn bill(self) -> Option<TransactionId> {
        match self {
            PayProblem::Negative(b)
            | PayProblem::MoreThanDue(b)
            | PayProblem::NothingApplied(b)
            | PayProblem::DiscountNeedsUpdate(b) => Some(b),
            _ => None,
        }
    }
}

/// One bill as it is paid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PlannedBill {
    /// The bill.
    pub bill: TransactionId,
    /// Cash.
    pub cash: Money,
    /// Discount (zero for none).
    pub discount: Money,
}

/// One credit application.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PlannedCredit {
    /// The credit.
    pub credit: TransactionId,
    /// The bill.
    pub bill: TransactionId,
    /// How much.
    pub amount: Money,
}

/// What goes to the engine (the screen maps it to `cl_api::PayBillsRequest`,
/// where each bill's `amount` is its cash plus its credits).
#[derive(Clone, PartialEq, Eq)]
pub struct PayPlan {
    /// The bills, in list order.
    pub bills: Vec<PlannedBill>,
    /// The credits applied.
    pub credits: Vec<PlannedCredit>,
}

impl fmt::Debug for PayPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayPlan")
            .field("bills", &self.bills.len())
            .field("credits", &self.credits.len())
            .finish()
    }
}

impl fmt::Debug for PlannedBill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedBill")
            .field("bill", &self.bill)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for PlannedCredit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedCredit")
            .field("credit", &self.credit)
            .field("bill", &self.bill)
            .finish_non_exhaustive()
    }
}

/// The figures under the grid.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PayTotals {
    /// Selected bills' balance due (Q-P18b as the owner answered it:
    /// every column totals the selected bills).
    pub amount_due: Money,
    /// Selected bills' discounts.
    pub discount: Money,
    /// Selected bills' credits.
    pub credits: Money,
    /// Selected bills' cash: the checks' total.
    pub cash: Money,
    /// Listed bills.
    pub listed: usize,
    /// Selected (and listed) bills.
    pub selected: usize,
    /// Selected bills a filter hides (kept, not paid).
    pub hidden: usize,
}

impl fmt::Debug for PayTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayTotals")
            .field("listed", &self.listed)
            .field("selected", &self.selected)
            .field("hidden", &self.hidden)
            .finish_non_exhaustive()
    }
}

/// The Pay Bills screen's state.
#[derive(Clone)]
pub struct PayBillsForm {
    rows: Vec<PayableRow>,
    credits: Vec<OpenCredit>,
    entries: BTreeMap<TransactionId, Entry>,
    /// Which bills are listed.
    pub show: ShowBills,
    /// One vendor (`None`: all).
    pub vendor: Option<ContactId>,
    /// The order.
    pub sort: SortBy,
    /// Take the engine's discounts on selected bills.
    take_discounts: bool,
}

impl fmt::Debug for PayBillsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayBillsForm")
            .field("rows", &self.rows.len())
            .field("credits", &self.credits.len())
            .field("show", &self.show)
            .field("sort", &self.sort)
            .field("take_discounts", &self.take_discounts)
            .finish_non_exhaustive()
    }
}

fn add(a: Money, b: Money) -> Result<Money, MoneyError> {
    a.checked_add(b)
}

impl PayBillsForm {
    /// An empty form listing bills due on or before `through`, taking
    /// discounts.
    #[must_use]
    pub fn new(through: CalendarDate) -> PayBillsForm {
        PayBillsForm {
            rows: Vec::new(),
            credits: Vec::new(),
            entries: BTreeMap::new(),
            show: ShowBills::DueOnOrBefore(through),
            vendor: None,
            sort: SortBy::DueDate,
            take_discounts: true,
        }
    }

    /// Takes a fresh list from the engine. What the owner set on bills still
    /// listed is kept (a credit no longer open is dropped from them), and
    /// the credits not chosen in Set Credits are fitted again.
    pub fn load(&mut self, rows: Vec<PayableRow>, credits: Vec<OpenCredit>) {
        self.entries
            .retain(|b, _| rows.iter().any(|r| r.bill == *b));
        for e in self.entries.values_mut() {
            e.credits
                .retain(|(c, _)| credits.iter().any(|x| x.credit == *c));
        }
        self.rows = rows;
        self.credits = credits;
        self.refit_credits();
    }

    /// Whether the engine's discounts are taken on selected bills.
    #[must_use]
    pub fn take_discounts(&self) -> bool {
        self.take_discounts
    }

    /// "Take discounts I qualify for": the discounts change, so the
    /// automatic credits are fitted again.
    pub fn set_take_discounts(&mut self, on: bool) {
        if self.take_discounts != on {
            self.take_discounts = on;
            self.refit_credits();
        }
    }

    /// The credits a bill gets automatically: the vendor's open credits,
    /// oldest first, as far as the bill needs them after its discount
    /// (Q-P18i).
    fn fit_credits(&self, bill: TransactionId) -> Vec<(TransactionId, Money)> {
        let Some(row) = self.row(bill) else {
            return Vec::new();
        };
        let mut need = row
            .balance_due
            .checked_sub(self.discount(bill))
            .unwrap_or(Money::ZERO);
        let mut applied = Vec::new();
        for c in self.credits_for(row.vendor) {
            if !need.is_positive() {
                break;
            }
            let left = self.credit_left(c.credit, Some(bill));
            let take = if left < need { left } else { need };
            if take.is_positive() {
                applied.push((c.credit, take));
                need = need.checked_sub(take).unwrap_or(Money::ZERO);
            }
        }
        applied
    }

    /// Fits the automatic credits of every selected bill again, in list
    /// order (Set Credits' choices stay).
    fn refit_credits(&mut self) {
        let auto: Vec<TransactionId> = self
            .rows
            .iter()
            .map(|r| r.bill)
            .filter(|b| {
                self.entries
                    .get(b)
                    .is_some_and(|e| e.selected && !e.credits_set)
            })
            .collect();
        for b in &auto {
            if let Some(e) = self.entries.get_mut(b) {
                e.credits.clear();
            }
        }
        for b in auto {
            let fitted = self.fit_credits(b);
            if let Some(e) = self.entries.get_mut(&b) {
                e.credits = fitted;
            }
        }
    }

    /// Every bill the engine listed.
    #[must_use]
    pub fn all_rows(&self) -> &[PayableRow] {
        &self.rows
    }

    /// The open credits.
    #[must_use]
    pub fn open_credits(&self) -> &[OpenCredit] {
        &self.credits
    }

    /// The vendors of the listed bills (for the vendor filter), by name.
    #[must_use]
    pub fn vendors(&self) -> Vec<(ContactId, String)> {
        let mut out: Vec<(ContactId, String)> = Vec::new();
        for r in &self.rows {
            if let Some(v) = r.vendor
                && !out.iter().any(|(x, _)| *x == v)
            {
                out.push((v, r.vendor_name.clone()));
            }
        }
        out.sort_by_key(|a| a.1.to_lowercase());
        out
    }

    /// The listed rows (filters and order applied).
    #[must_use]
    pub fn visible(&self) -> Vec<&PayableRow> {
        let mut out: Vec<&PayableRow> = self
            .rows
            .iter()
            .filter(|r| self.vendor.is_none() || r.vendor == self.vendor)
            .filter(|r| match self.show {
                ShowBills::All => true,
                ShowBills::DueOnOrBefore(d) => r.due.is_some_and(|due| due <= d),
            })
            .collect();
        match self.sort {
            // The engine's order is soonest due first; keep it stable.
            SortBy::DueDate => out.sort_by_key(|r| (r.due.is_none(), r.due)),
            SortBy::Vendor => out.sort_by(|a, b| {
                a.vendor_name
                    .to_lowercase()
                    .cmp(&b.vendor_name.to_lowercase())
                    .then(a.due.cmp(&b.due))
            }),
            SortBy::AmountDue => out.sort_by_key(|r| std::cmp::Reverse(r.balance_due)),
        }
        out
    }

    fn row(&self, bill: TransactionId) -> Option<&PayableRow> {
        self.rows.iter().find(|r| r.bill == bill)
    }

    /// Whether `bill` is selected.
    #[must_use]
    pub fn is_selected(&self, bill: TransactionId) -> bool {
        self.entries.get(&bill).is_some_and(|e| e.selected)
    }

    /// The discount on `bill`: Set Discount's, else the engine's when
    /// taking discounts and the bill is selected, else zero.
    #[must_use]
    pub fn discount(&self, bill: TransactionId) -> Money {
        let Some(row) = self.row(bill) else {
            return Money::ZERO;
        };
        let e = self.entries.get(&bill);
        match e.and_then(|e| e.discount) {
            Some(d) => d,
            None if self.take_discounts && e.is_some_and(|e| e.selected) => row.discount_available,
            None => Money::ZERO,
        }
    }

    /// The credits applied to `bill`: (credit, amount).
    #[must_use]
    pub fn credits_on(&self, bill: TransactionId) -> &[(TransactionId, Money)] {
        self.entries
            .get(&bill)
            .map_or(&[], |e| e.credits.as_slice())
    }

    /// The total of the credits on `bill`.
    #[must_use]
    pub fn credit_total(&self, bill: TransactionId) -> Money {
        Money::sum(self.credits_on(bill).iter().map(|(_, m)| *m)).unwrap_or(Money::ZERO)
    }

    /// The cash on `bill` ("Amt. to pay"): typed, else balance − discount −
    /// credits for a selected bill (never below zero), else zero.
    #[must_use]
    pub fn cash(&self, bill: TransactionId) -> Money {
        let Some(e) = self.entries.get(&bill) else {
            return Money::ZERO;
        };
        if let Some(c) = e.cash {
            return c;
        }
        if !e.selected {
            return Money::ZERO;
        }
        self.row(bill)
            .and_then(|r| {
                r.balance_due
                    .checked_sub(self.discount(bill))
                    .and_then(|x| x.checked_sub(self.credit_total(bill)))
                    .ok()
            })
            .filter(|m| !m.is_negative())
            .unwrap_or(Money::ZERO)
    }

    /// What a credit has left for `bill`: its open amount minus what other
    /// bills use.
    #[must_use]
    pub fn credit_left(&self, credit: TransactionId, except: Option<TransactionId>) -> Money {
        let open = self
            .credits
            .iter()
            .find(|c| c.credit == credit)
            .map_or(Money::ZERO, |c| c.open);
        let used = Money::sum(
            self.entries
                .iter()
                .filter(|(b, e)| e.selected && Some(**b) != except)
                .flat_map(|(_, e)| e.credits.iter())
                .filter(|(c, _)| *c == credit)
                .map(|(_, m)| *m),
        )
        .unwrap_or(Money::ZERO);
        open.checked_sub(used).unwrap_or(Money::ZERO)
    }

    /// The vendor's open credits, oldest first.
    #[must_use]
    pub fn credits_for(&self, vendor: Option<ContactId>) -> Vec<&OpenCredit> {
        let mut out: Vec<&OpenCredit> = self
            .credits
            .iter()
            .filter(|c| vendor.is_some() && c.vendor == vendor)
            .collect();
        out.sort_by_key(|c| (c.date, c.credit));
        out
    }

    /// Selects or clears `bill`. Selecting applies the vendor's open credits
    /// (as far as the bill needs them) and fills the cash from the bill;
    /// clearing forgets what was set on it.
    pub fn set_selected(&mut self, bill: TransactionId, on: bool) {
        if self.row(bill).is_none() {
            return;
        }
        if !on {
            self.entries.remove(&bill);
            return;
        }
        if self.is_selected(bill) {
            return;
        }
        self.entries.entry(bill).or_default().selected = true;
        let applied = self.fit_credits(bill);
        if let Some(e) = self.entries.get_mut(&bill) {
            e.credits = applied;
        }
    }

    /// Flips `bill` (Space, the checkbox).
    pub fn toggle(&mut self, bill: TransactionId) {
        let on = !self.is_selected(bill);
        self.set_selected(bill, on);
    }

    /// Types the cash on `bill`: a positive amount selects it; zero on a
    /// bill with no credit or discount clears it (nothing would be paid).
    pub fn set_cash(&mut self, bill: TransactionId, cash: Money) {
        if self.row(bill).is_none() {
            return;
        }
        if !self.is_selected(bill) {
            if cash.is_zero() {
                return;
            }
            self.set_selected(bill, true);
        }
        if cash.is_zero() && self.discount(bill).is_zero() && self.credit_total(bill).is_zero() {
            self.set_selected(bill, false);
            return;
        }
        if let Some(e) = self.entries.get_mut(&bill) {
            e.cash = Some(cash);
        }
    }

    /// The cash goes back to following the bill.
    pub fn clear_cash(&mut self, bill: TransactionId) {
        if let Some(e) = self.entries.get_mut(&bill) {
            e.cash = None;
        }
    }

    /// Set Discount: `Some` replaces the engine's, `None` goes back to it.
    /// Selects the bill.
    pub fn set_discount(&mut self, bill: TransactionId, discount: Option<Money>) {
        if self.row(bill).is_none() {
            return;
        }
        self.set_selected(bill, true);
        if let Some(e) = self.entries.get_mut(&bill) {
            e.discount = discount;
        }
        self.refit_credits();
    }

    /// Set Credits: the credits on `bill` (zero amounts dropped). Selects
    /// the bill.
    pub fn set_credits(&mut self, bill: TransactionId, credits: Vec<(TransactionId, Money)>) {
        if self.row(bill).is_none() {
            return;
        }
        self.set_selected(bill, true);
        if let Some(e) = self.entries.get_mut(&bill) {
            e.credits = credits.into_iter().filter(|(_, m)| !m.is_zero()).collect();
            e.credits_set = true;
        }
    }

    /// Selects every listed bill.
    pub fn select_all(&mut self) {
        let bills: Vec<TransactionId> = self.visible().iter().map(|r| r.bill).collect();
        for b in bills {
            self.set_selected(b, true);
        }
    }

    /// Clears every selection ("Clear selections").
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Clears the selection of these bills (the ones just paid).
    pub fn clear_bills(&mut self, bills: &[TransactionId]) {
        for b in bills {
            self.entries.remove(b);
        }
    }

    /// Selected bills a filter hides: kept, not paid, counted apart.
    #[must_use]
    pub fn hidden_selected(&self) -> usize {
        let visible: Vec<TransactionId> = self.visible().iter().map(|r| r.bill).collect();
        self.entries
            .iter()
            .filter(|(b, e)| e.selected && !visible.contains(b) && self.row(**b).is_some())
            .count()
    }

    /// The listed bills that are selected, in list order.
    #[must_use]
    pub fn selected(&self) -> Vec<TransactionId> {
        self.visible()
            .iter()
            .map(|r| r.bill)
            .filter(|b| self.is_selected(*b))
            .collect()
    }

    /// The figures under the grid (`None` only beyond money's range).
    #[must_use]
    pub fn totals(&self) -> Option<PayTotals> {
        let visible = self.visible();
        let selected = self.selected();
        let amount_due = Money::sum(
            selected
                .iter()
                .filter_map(|b| self.row(*b).map(|r| r.balance_due)),
        )
        .ok()?;
        let sum = |f: &dyn Fn(TransactionId) -> Money| Money::sum(selected.iter().map(|b| f(*b)));
        Some(PayTotals {
            amount_due,
            discount: sum(&|b| self.discount(b)).ok()?,
            credits: sum(&|b| self.credit_total(b)).ok()?,
            cash: sum(&|b| self.cash(b)).ok()?,
            listed: visible.len(),
            selected: selected.len(),
            hidden: self.hidden_selected(),
        })
    }

    /// The bank account's balance after the checks: the engine's balance
    /// minus the selected cash (Q-P18c).
    #[must_use]
    pub fn ending_after(&self, engine_balance: Money) -> Option<Money> {
        engine_balance.checked_sub(self.totals()?.cash).ok()
    }

    /// The pay-from account's figure before → after, as shown (Q-P18c):
    /// `engine_balance` is the engine's stored balance (debits − credits).
    /// A bank account shows its balance, which falls by the cash; a credit
    /// card account (`card`) shows the amount owed (the stored balance with
    /// its sign turned, Q-241), which rises by it.
    #[must_use]
    pub fn preview(&self, engine_balance: Money, card: bool) -> Option<(Money, Money)> {
        let cash = self.totals()?.cash;
        if card {
            let owed = engine_balance.checked_neg().ok()?;
            Some((owed, owed.checked_add(cash).ok()?))
        } else {
            Some((engine_balance, engine_balance.checked_sub(cash).ok()?))
        }
    }

    /// Checks the run. `discounts`: the engine takes discounts
    /// (`bill-discounts-v1`).
    ///
    /// # Errors
    /// Every problem found, in list order.
    pub fn plan(&self, has_account: bool, discounts: bool) -> Result<PayPlan, Vec<PayProblem>> {
        let mut problems = Vec::new();
        if !has_account {
            problems.push(PayProblem::AccountMissing);
        }
        let selected = self.selected();
        if selected.is_empty() {
            problems.push(PayProblem::NothingSelected);
        }
        let mut bills = Vec::new();
        let mut credits = Vec::new();
        for b in selected {
            let Some(row) = self.row(b) else { continue };
            let (cash, discount, credit) = (self.cash(b), self.discount(b), self.credit_total(b));
            if cash.is_negative()
                || discount.is_negative()
                || self.credits_on(b).iter().any(|(_, m)| m.is_negative())
            {
                problems.push(PayProblem::Negative(b));
                continue;
            }
            let Ok(applied) = add(cash, discount).and_then(|x| add(x, credit)) else {
                problems.push(PayProblem::TooLarge);
                continue;
            };
            if applied > row.balance_due {
                problems.push(PayProblem::MoreThanDue(b));
            }
            if !applied.is_positive() {
                problems.push(PayProblem::NothingApplied(b));
            }
            if !discount.is_zero() && !discounts {
                problems.push(PayProblem::DiscountNeedsUpdate(b));
            }
            bills.push(PlannedBill {
                bill: b,
                cash,
                discount,
            });
            credits.extend(self.credits_on(b).iter().map(|(c, m)| PlannedCredit {
                credit: *c,
                bill: b,
                amount: *m,
            }));
        }
        let mut seen: Vec<TransactionId> = Vec::new();
        for c in &credits {
            if seen.contains(&c.credit) {
                continue;
            }
            seen.push(c.credit);
            let used = Money::sum(
                credits
                    .iter()
                    .filter(|x| x.credit == c.credit)
                    .map(|x| x.amount),
            );
            let open = self
                .credits
                .iter()
                .find(|x| x.credit == c.credit)
                .map_or(Money::ZERO, |x| x.open);
            if used.map_or(true, |u| u > open) {
                problems.push(PayProblem::CreditOverused(c.credit));
            }
        }
        problems.dedup();
        if problems.is_empty() {
            Ok(PayPlan { bills, credits })
        } else {
            Err(problems)
        }
    }
}

#[cfg(test)]
#[path = "bill_payment/tests.rs"]
mod tests;
