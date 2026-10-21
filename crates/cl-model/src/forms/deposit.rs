//! The Make Deposit form (P14 WP-A2): payments taken whole out of Payments
//! Clearing, other lines (a negative line is a fee taken out, Q-84), cash
//! back, and [`check`], the one exact gate before anything is sent.
//!
//! Money rules (P14 plan §4):
//! - A picked payment's amount is the engine's exact figure, carried as it
//!   came: never typed, rounded or split. A display-only amount never
//!   becomes a [`PickedPayment`] (the screen can only build one from an
//!   exact engine amount).
//! - Other lines are the owner's typed `Money` (cents, Q-232), any sign.
//! - The deposit total is `Σpicked + Σlines − cash back` with checked exact
//!   arithmetic; nothing is sent unless it is greater than zero, and cash
//!   back must leave something to deposit.
//! - The checked deposit carries positive line amounts and positive
//!   deductions (the absolute value of each negative line), which cl-api
//!   sends negative and re-checks (`BankPosting::deposit_with`).

use std::fmt;

use cl_core::ids::{AccountId, ContactId, TransactionId};
use cl_core::{CalendarDate, Money};

use crate::forms::lines::{FormLines, PostingLine, trimmed};
use crate::rowkey::RowKey;
use crate::text::TextKey;

/// Most parts one deposit takes (payments, lines and cash back; the
/// engine's `MAX_LINES`).
pub const MAX_PARTS: usize = 500;

/// A pending payment ticked for this deposit, at the engine's exact amount.
#[derive(Clone, PartialEq, Eq)]
pub struct PickedPayment {
    /// The payment's transaction.
    pub source: TransactionId,
    /// Exactly what it holds in Payments Clearing (the engine's figure).
    pub amount: Money,
    /// Received from.
    pub contact: Option<ContactId>,
}

impl fmt::Debug for PickedPayment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickedPayment")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// The cash back row.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CashBackForm {
    /// Where the cash goes.
    pub account: Option<AccountId>,
    /// How much (typed).
    pub amount: Option<Money>,
    /// Memo.
    pub memo: String,
}

impl CashBackForm {
    /// Nothing entered.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.account.is_none() && self.amount.is_none() && self.memo.trim().is_empty()
    }
}

impl fmt::Debug for CashBackForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CashBackForm")
            .field("account", &self.account.is_some())
            .field("amount", &self.amount.is_some())
            .finish_non_exhaustive()
    }
}

/// The Make Deposit form. The other lines live in the shared lines grid;
/// the screen puts them into `lines` to check.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DepositForm {
    /// The account the money goes into.
    pub account: Option<AccountId>,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Memo.
    pub memo: String,
    /// Payments ticked, in list order.
    pub picked: Vec<PickedPayment>,
    /// Other deposits (signed: negative is a fee).
    pub lines: FormLines,
    /// Cash back.
    pub cash_back: CashBackForm,
}

impl fmt::Debug for DepositForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DepositForm")
            .field("account", &self.account)
            .field("date", &self.date)
            .field("picked", &self.picked.len())
            .field("lines", &self.lines.used().count())
            .field("cash_back", &self.cash_back)
            .finish_non_exhaustive()
    }
}

/// The figures under the form: exact sums of the engine's payment amounts
/// and the owner's typed amounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepositTotals {
    /// Σ picked payments.
    pub payments: Money,
    /// Σ other lines (signed).
    pub other: Money,
    /// Cash back (zero when none typed).
    pub cash_back: Money,
    /// What the bank gets: payments + other − cash back.
    pub total: Money,
}

/// What the engine takes (from its capabilities) and the accounts cash
/// back may not go to.
#[derive(Clone, Copy, Debug, Default)]
pub struct DepositContext<'a> {
    /// `deposit-grouping-v1`: payments taken whole.
    pub grouping: bool,
    /// `deposit-cashback-v1`: fees (negative lines) and cash back.
    pub cashback: bool,
    /// Payments Clearing, Accounts Receivable, Accounts Payable and Sales
    /// Tax Payable: the engine refuses cash back to them.
    pub control_accounts: &'a [AccountId],
}

/// Why a deposit can't be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DepositProblem {
    /// No account to deposit into.
    AccountMissing,
    /// No date.
    DateMissing,
    /// No payment ticked and no other line.
    NothingToDeposit,
    /// The total is zero or less.
    TotalNotPositive,
    /// A used line has no account.
    LineAccountMissing(RowKey),
    /// A used line has no amount, or zero.
    LineAmountZero(RowKey),
    /// A line on the deposit account itself.
    LineToDepositAccount(RowKey),
    /// A negative line (a fee) needs `deposit-cashback-v1`.
    NegativeNeedsUpdate(RowKey),
    /// Cash back needs `deposit-cashback-v1`.
    CashBackNeedsUpdate,
    /// Cash back without an account.
    CashBackAccountMissing,
    /// Cash back without an amount, or not above zero.
    CashBackAmountMissing,
    /// Cash back leaves nothing to deposit.
    CashBackTooLarge,
    /// Cash back to the deposit account itself.
    CashBackToDepositAccount,
    /// Cash back to Payments Clearing, A/R, A/P or Sales Tax Payable.
    CashBackToControlAccount,
    /// Payments ticked without `deposit-grouping-v1`.
    GroupingNeedsUpdate,
    /// More than [`MAX_PARTS`] parts.
    TooManyParts,
    /// The amounts add up beyond what money can hold.
    TooLarge,
}

/// Which field a problem belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepositField {
    /// Deposit to.
    Account,
    /// Date.
    Date,
    /// The payments list.
    Payments,
    /// The other lines as a whole.
    Lines,
    /// A line's account.
    LineAccount(RowKey),
    /// A line's amount.
    LineAmount(RowKey),
    /// The cash back account.
    CashBackAccount,
    /// The cash back amount.
    CashBackAmount,
}

impl DepositProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            DepositProblem::AccountMissing => "deposit.problem.account_missing",
            DepositProblem::DateMissing => "deposit.problem.date_missing",
            DepositProblem::NothingToDeposit => "deposit.problem.nothing",
            DepositProblem::TotalNotPositive => "deposit.problem.total_not_positive",
            DepositProblem::LineAccountMissing(_) => "deposit.problem.line_account_missing",
            DepositProblem::LineAmountZero(_) => "deposit.problem.line_amount_zero",
            DepositProblem::LineToDepositAccount(_) => "deposit.problem.line_to_deposit_account",
            DepositProblem::NegativeNeedsUpdate(_) => "deposit.problem.negative_needs_update",
            DepositProblem::CashBackNeedsUpdate => "deposit.problem.cash_back_needs_update",
            DepositProblem::CashBackAccountMissing => "deposit.problem.cash_back_account_missing",
            DepositProblem::CashBackAmountMissing => "deposit.problem.cash_back_amount_missing",
            DepositProblem::CashBackTooLarge => "deposit.problem.cash_back_too_large",
            DepositProblem::CashBackToDepositAccount => "deposit.problem.cash_back_to_deposit",
            DepositProblem::CashBackToControlAccount => "deposit.problem.cash_back_to_control",
            DepositProblem::GroupingNeedsUpdate => "deposit.problem.grouping_needs_update",
            DepositProblem::TooManyParts => "deposit.problem.too_many_parts",
            DepositProblem::TooLarge => "deposit.problem.too_large",
        })
    }

    /// The control-channel name (no amounts).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            DepositProblem::AccountMissing => "account_missing",
            DepositProblem::DateMissing => "date_missing",
            DepositProblem::NothingToDeposit => "nothing_to_deposit",
            DepositProblem::TotalNotPositive => "total_not_positive",
            DepositProblem::LineAccountMissing(_) => "line_account_missing",
            DepositProblem::LineAmountZero(_) => "line_amount_zero",
            DepositProblem::LineToDepositAccount(_) => "line_to_deposit_account",
            DepositProblem::NegativeNeedsUpdate(_) => "negative_needs_update",
            DepositProblem::CashBackNeedsUpdate => "cash_back_needs_update",
            DepositProblem::CashBackAccountMissing => "cash_back_account_missing",
            DepositProblem::CashBackAmountMissing => "cash_back_amount_missing",
            DepositProblem::CashBackTooLarge => "cash_back_too_large",
            DepositProblem::CashBackToDepositAccount => "cash_back_to_deposit_account",
            DepositProblem::CashBackToControlAccount => "cash_back_to_control_account",
            DepositProblem::GroupingNeedsUpdate => "grouping_needs_update",
            DepositProblem::TooManyParts => "too_many_parts",
            DepositProblem::TooLarge => "too_large",
        }
    }

    /// Where the keyboard goes for it.
    #[must_use]
    pub const fn field(self) -> DepositField {
        match self {
            DepositProblem::AccountMissing => DepositField::Account,
            DepositProblem::DateMissing => DepositField::Date,
            DepositProblem::GroupingNeedsUpdate => DepositField::Payments,
            DepositProblem::NothingToDeposit
            | DepositProblem::TotalNotPositive
            | DepositProblem::TooManyParts
            | DepositProblem::TooLarge => DepositField::Lines,
            DepositProblem::LineAccountMissing(k) | DepositProblem::LineToDepositAccount(k) => {
                DepositField::LineAccount(k)
            }
            DepositProblem::LineAmountZero(k) | DepositProblem::NegativeNeedsUpdate(k) => {
                DepositField::LineAmount(k)
            }
            DepositProblem::CashBackAccountMissing
            | DepositProblem::CashBackToDepositAccount
            | DepositProblem::CashBackToControlAccount => DepositField::CashBackAccount,
            DepositProblem::CashBackNeedsUpdate
            | DepositProblem::CashBackAmountMissing
            | DepositProblem::CashBackTooLarge => DepositField::CashBackAmount,
        }
    }
}

/// Checked cash back.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedCashBack {
    /// Where the cash goes.
    pub account: AccountId,
    /// How much (above zero, less than the rest of the deposit).
    pub amount: Money,
    /// Memo, when typed.
    pub memo: Option<String>,
}

impl fmt::Debug for CheckedCashBack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedCashBack")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A deposit that passed [`check`], ready to map onto the engine draft
/// field by field.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedDeposit {
    /// The account the money goes into.
    pub account: AccountId,
    /// Date.
    pub date: CalendarDate,
    /// Memo, when typed.
    pub memo: Option<String>,
    /// Payments taken whole, at the engine's amounts.
    pub payments: Vec<PickedPayment>,
    /// Lines with a positive amount.
    pub lines: Vec<PostingLine>,
    /// Fees: each negative line with its amount made positive.
    pub deductions: Vec<PostingLine>,
    /// Cash back.
    pub cash_back: Option<CheckedCashBack>,
    /// What the bank gets (greater than zero).
    pub total: Money,
}

impl fmt::Debug for CheckedDeposit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedDeposit")
            .field("account", &self.account)
            .field("date", &self.date)
            .field("payments", &self.payments)
            .field("lines", &self.lines.len())
            .field("deductions", &self.deductions.len())
            .field("cash_back", &self.cash_back)
            .finish_non_exhaustive()
    }
}

impl DepositForm {
    /// The figures under the form; `None` when a sum leaves the money
    /// range. Blank amounts count as nothing.
    #[must_use]
    pub fn totals(&self) -> Option<DepositTotals> {
        let payments = Money::sum(self.picked.iter().map(|p| p.amount)).ok()?;
        let other = Money::sum(self.lines.used().filter_map(|l| l.amount)).ok()?;
        let cash_back = self.cash_back.amount.unwrap_or(Money::ZERO);
        let total = payments
            .checked_add(other)
            .and_then(|t| t.checked_sub(cash_back))
            .ok()?;
        Some(DepositTotals {
            payments,
            other,
            cash_back,
            total,
        })
    }

    /// Ticks or unticks a payment (the engine's exact amount comes with
    /// it). Returns whether it is ticked now.
    pub fn toggle(&mut self, p: PickedPayment) -> bool {
        if let Some(i) = self.picked.iter().position(|x| x.source == p.source) {
            self.picked.remove(i);
            false
        } else {
            self.picked.push(p);
            true
        }
    }

    /// Drops the ticked payments that are no longer waiting (deposited in
    /// another window). Returns how many were dropped.
    pub fn keep_only(&mut self, waiting: &[TransactionId]) -> usize {
        let before = self.picked.len();
        self.picked.retain(|p| waiting.contains(&p.source));
        before - self.picked.len()
    }
}

/// The other lines: positive ones and fees (made positive), with their
/// problems.
fn check_lines(
    f: &DepositForm,
    ctx: &DepositContext<'_>,
    problems: &mut Vec<DepositProblem>,
) -> (Vec<PostingLine>, Vec<PostingLine>) {
    let mut lines = Vec::new();
    let mut deductions = Vec::new();
    for l in f.lines.used() {
        let mut ok = true;
        if l.account.is_none() {
            problems.push(DepositProblem::LineAccountMissing(l.key));
            ok = false;
        } else if l.account == f.account {
            problems.push(DepositProblem::LineToDepositAccount(l.key));
            ok = false;
        }
        let amount = match l.amount {
            Some(m) if !m.is_zero() => m,
            _ => {
                problems.push(DepositProblem::LineAmountZero(l.key));
                continue;
            }
        };
        if amount.is_negative() && !ctx.cashback {
            problems.push(DepositProblem::NegativeNeedsUpdate(l.key));
            ok = false;
        }
        let Some(account) = l.account.filter(|_| ok) else {
            continue;
        };
        let p = PostingLine {
            account,
            amount: amount.abs(),
            memo: trimmed(&l.memo),
            contact: l.customer,
            billable: false,
            tag: l.tag,
        };
        if amount.is_negative() {
            deductions.push(p);
        } else {
            lines.push(p);
        }
    }
    (lines, deductions)
}

/// The cash back row, with its problems.
fn check_cash_back(
    f: &DepositForm,
    ctx: &DepositContext<'_>,
    problems: &mut Vec<DepositProblem>,
) -> Option<CheckedCashBack> {
    let cb = &f.cash_back;
    if cb.is_blank() {
        return None;
    }
    if !ctx.cashback {
        problems.push(DepositProblem::CashBackNeedsUpdate);
        return None;
    }
    match cb.account {
        None => problems.push(DepositProblem::CashBackAccountMissing),
        Some(a) if Some(a) == f.account => {
            problems.push(DepositProblem::CashBackToDepositAccount);
        }
        Some(a) if ctx.control_accounts.contains(&a) => {
            problems.push(DepositProblem::CashBackToControlAccount);
        }
        Some(_) => {}
    }
    match cb.amount {
        Some(m) if m.is_positive() => cb.account.map(|account| CheckedCashBack {
            account,
            amount: m,
            memo: trimmed(&cb.memo),
        }),
        _ => {
            problems.push(DepositProblem::CashBackAmountMissing);
            None
        }
    }
}

/// The one exact gate: the checked deposit, or every problem (in field
/// order).
///
/// # Errors
/// See [`DepositProblem`].
pub fn check(
    f: &DepositForm,
    ctx: &DepositContext<'_>,
) -> Result<CheckedDeposit, Vec<DepositProblem>> {
    let mut problems = Vec::new();
    if f.account.is_none() {
        problems.push(DepositProblem::AccountMissing);
    }
    if f.date.is_none() {
        problems.push(DepositProblem::DateMissing);
    }
    if !f.picked.is_empty() && !ctx.grouping {
        problems.push(DepositProblem::GroupingNeedsUpdate);
    }
    let (lines, deductions) = check_lines(f, ctx, &mut problems);
    let cash_back = check_cash_back(f, ctx, &mut problems);
    let cb = &f.cash_back;
    let used = f.lines.used().count();
    if f.picked.is_empty() && used == 0 {
        problems.push(DepositProblem::NothingToDeposit);
    }
    if f.picked.len() + used + usize::from(!cb.is_blank()) > MAX_PARTS {
        problems.push(DepositProblem::TooManyParts);
    }
    let total = match f.totals() {
        None => {
            problems.push(DepositProblem::TooLarge);
            None
        }
        Some(t) => Some(t.total),
    };
    let nothing = problems.contains(&DepositProblem::NothingToDeposit);
    if let Some(t) = total
        && !t.is_positive()
        && !nothing
    {
        // Cash back that leaves nothing is the cash back's problem.
        if cb.amount.is_some_and(Money::is_positive) {
            if ctx.cashback {
                problems.push(DepositProblem::CashBackTooLarge);
            }
        } else {
            problems.push(DepositProblem::TotalNotPositive);
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let (Some(account), Some(date), Some(total)) = (f.account, f.date, total) else {
        // Unreachable: each missing value pushed a problem above.
        return Err(vec![DepositProblem::AccountMissing]);
    };
    Ok(CheckedDeposit {
        account,
        date,
        memo: trimmed(&f.memo),
        payments: f.picked.clone(),
        lines,
        deductions,
        cash_back,
        total,
    })
}

/// The deposit set against a bank-file deposit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BankMatch {
    /// The same to the cent.
    Matches,
    /// The bank's figure minus the deposit total.
    Differs(Money),
}

/// Compares the deposit total with the bank's figure, exactly.
#[must_use]
pub fn bank_match(total: Money, bank: Money) -> Option<BankMatch> {
    let d = bank.checked_sub(total).ok()?;
    Some(if d.is_zero() {
        BankMatch::Matches
    } else {
        BankMatch::Differs(d)
    })
}

/// One row of the deposit summary.
#[derive(Clone, PartialEq, Eq)]
pub struct SummaryRow {
    /// Received from, or the account (owner data).
    pub label: String,
    /// Reference (check number) or memo.
    pub detail: String,
    /// The engine's amount (signed for other lines).
    pub amount: Money,
}

impl fmt::Debug for SummaryRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SummaryRow").finish_non_exhaustive()
    }
}

/// The deposit summary (Q-203; the P19 print model, Q-P14d). Every figure
/// is from the saved record; `total` is the engine's bank amount, never
/// re-added here.
#[derive(Clone, PartialEq, Eq)]
pub struct DepositSummary {
    /// Date.
    pub date: CalendarDate,
    /// Deposit to (display text).
    pub account: String,
    /// Memo.
    pub memo: String,
    /// Payments deposited.
    pub items: Vec<SummaryRow>,
    /// Other lines (fees negative).
    pub other: Vec<SummaryRow>,
    /// Cash back.
    pub cash_back: Option<Money>,
    /// What the bank got (the engine's).
    pub total: Money,
}

impl fmt::Debug for DepositSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DepositSummary")
            .field("date", &self.date)
            .field("items", &self.items.len())
            .field("other", &self.other.len())
            .finish_non_exhaustive()
    }
}

impl DepositSummary {
    /// The rows agree with the engine's total (shown as a quiet check;
    /// the total shown is always the engine's).
    #[must_use]
    pub fn parts_agree(&self) -> bool {
        Money::sum(self.items.iter().chain(&self.other).map(|r| r.amount))
            .and_then(|s| s.checked_sub(self.cash_back.unwrap_or(Money::ZERO)))
            .is_ok_and(|s| s == self.total)
    }
}

#[cfg(test)]
#[path = "deposit/tests.rs"]
mod tests;
