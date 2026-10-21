//! The Make Journal Entry form (P15 lane J2): date, entry number, memo,
//! the Adjusting flag, Reverse on and the debit/credit lines, and
//! [`check`], the gate before anything is sent.
//!
//! **Signed amounts.** Inside this module, and only here, a line's
//! [`FormLine::amount`] is signed: a debit is positive, a credit negative.
//! The grid shows two columns and typing in one replaces the other
//! ([`set_debit`], [`set_credit`]); [`side_of`] turns the signed value back
//! into a [`JeSide`] for the engine. The owner's typed amounts are kept as
//! exact [`Money`] (cents, Q-232); nothing here is a ledger figure
//! (non-negotiable 7).
//!
//! **Totals and the balance helper.** [`totals`] sums the owner's own
//! debits and credits exactly ([`Money::sum`], an overflow is a problem,
//! never a rounded figure) and [`balance_suggestion`] offers the amount
//! that balances on the opposite side (Q-P15n). Like Write Check's
//! "remaining", it is input arithmetic; the owner accepts it.
//!
//! The A/R-A/P rule (Q-206, Q-P15s): an entry may have one line on an
//! Accounts Receivable or Accounts Payable account, and that line needs a
//! customer (A/R) or a vendor (A/P). Without `je-names-v1` such lines are
//! refused with "needs an engine update".

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};

use crate::forms::amount_words::MAX_CHECK_UNITS;
use crate::forms::lines::{FormLine, FormLines, trimmed};
use crate::rowkey::RowKey;
use crate::text::TextKey;

/// The most lines an entry may have (engine `MAX_LINES`).
pub const MAX_LINES: usize = 500;

/// The longest entry number, after trimming (engine `txnNumber`, shapes-a).
pub const NUMBER_MAX_CHARS: usize = 20;

/// Debit or credit, with the amount (above zero on a checked line).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum JeSide {
    /// Debit.
    Debit(Money),
    /// Credit.
    Credit(Money),
}

impl JeSide {
    /// The amount.
    #[must_use]
    pub const fn amount(self) -> Money {
        match self {
            JeSide::Debit(m) | JeSide::Credit(m) => m,
        }
    }

    /// The signed form value (debit positive, credit negative). `None`
    /// only for an amount outside the money range when negated, which a
    /// `Money` cannot be.
    #[must_use]
    pub fn signed(self) -> Option<Money> {
        match self {
            JeSide::Debit(m) => Some(m),
            JeSide::Credit(m) => m.checked_neg().ok(),
        }
    }
}

impl fmt::Debug for JeSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            JeSide::Debit(_) => "Debit(..)",
            JeSide::Credit(_) => "Credit(..)",
        })
    }
}

/// The side a signed form amount is on: positive is a debit, negative a
/// credit, zero or blank is neither.
#[must_use]
pub fn side_of(amount: Option<Money>) -> Option<JeSide> {
    let m = amount?;
    if m.is_positive() {
        Some(JeSide::Debit(m))
    } else if m.is_negative() {
        Some(JeSide::Credit(m.abs()))
    } else {
        None
    }
}

/// The Debit column's value of a line (blank for a credit).
#[must_use]
pub fn debit_of(l: &FormLine) -> Option<Money> {
    match side_of(l.amount) {
        Some(JeSide::Debit(m)) => Some(m),
        _ => None,
    }
}

/// The Credit column's value of a line (blank for a debit).
#[must_use]
pub fn credit_of(l: &FormLine) -> Option<Money> {
    match side_of(l.amount) {
        Some(JeSide::Credit(m)) => Some(m),
        _ => None,
    }
}

/// Typing in Debit: the line becomes this debit and its credit is cleared.
/// A blank, zero or negative entry clears the line's amount (the grid's
/// amount field refuses a minus sign; this keeps the rule total).
pub fn set_debit(l: &mut FormLine, typed: Option<Money>) {
    l.amount = typed.filter(|m| m.is_positive());
}

/// Typing in Credit: the line becomes this credit and its debit is cleared.
pub fn set_credit(l: &mut FormLine, typed: Option<Money>) {
    l.amount = typed
        .filter(|m| m.is_positive())
        .and_then(|m| m.checked_neg().ok());
}

/// Clearing the Debit cell leaves a credit alone, and the reverse.
pub fn clear_debit(l: &mut FormLine) {
    if debit_of(l).is_some() {
        l.amount = None;
    }
}

/// Clearing the Credit cell leaves a debit alone.
pub fn clear_credit(l: &mut FormLine) {
    if credit_of(l).is_some() {
        l.amount = None;
    }
}

/// The Make Journal Entry form. `Debug` is redacted (amounts, memos and
/// numbers are owner data).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct JournalForm {
    /// Entry date.
    pub date: Option<CalendarDate>,
    /// Entry No. (free text; suggested from the engine's next number).
    pub number: String,
    /// Memo.
    pub memo: String,
    /// An adjusting entry.
    pub adjusting: bool,
    /// Post a reversing entry on this date (after the entry date).
    pub reverse_on: Option<CalendarDate>,
    /// The lines; [`FormLine::amount`] is signed (debit positive) and
    /// [`FormLine::customer`] holds the line's name (any kind, Q-P15o).
    pub lines: FormLines,
}

impl fmt::Debug for JournalForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalForm")
            .field("date", &self.date)
            .field("adjusting", &self.adjusting)
            .field("reverse_on", &self.reverse_on.is_some())
            .field("lines", &self.lines.lines.len())
            .finish_non_exhaustive()
    }
}

/// The owner's typed debits and credits, summed exactly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct JeTotals {
    /// Σ debits.
    pub debits: Money,
    /// Σ credits.
    pub credits: Money,
    /// Σ debits − Σ credits (zero when balanced).
    pub difference: Money,
}

impl JeTotals {
    /// True when debits equal credits exactly.
    #[must_use]
    pub fn balanced(self) -> bool {
        self.difference.is_zero()
    }
}

impl fmt::Debug for JeTotals {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JeTotals")
            .field("balanced", &self.balanced())
            .finish_non_exhaustive()
    }
}

/// The totals of the typed lines, or `None` when a sum leaves the money
/// range (a save is refused with [`JournalProblem::TooLarge`]).
#[must_use]
pub fn totals(lines: &FormLines) -> Option<JeTotals> {
    let mut debits = Vec::new();
    let mut credits = Vec::new();
    for l in &lines.lines {
        match side_of(l.amount) {
            Some(JeSide::Debit(m)) => debits.push(m),
            Some(JeSide::Credit(m)) => credits.push(m),
            None => {}
        }
    }
    let debits = Money::sum(debits).ok()?;
    let credits = Money::sum(credits).ok()?;
    let difference = debits.checked_sub(credits).ok()?;
    Some(JeTotals {
        debits,
        credits,
        difference,
    })
}

/// The amount that balances the entry, on the side it goes (Q-P15n): more
/// debits → a credit of the difference, more credits → a debit. `None` when
/// balanced or when the totals leave the money range.
#[must_use]
pub fn balance_suggestion(lines: &FormLines) -> Option<JeSide> {
    let t = totals(lines)?;
    if t.difference.is_positive() {
        Some(JeSide::Credit(t.difference))
    } else if t.difference.is_negative() {
        Some(JeSide::Debit(t.difference.abs()))
    } else {
        None
    }
}

/// Puts the balancing amount on `key`'s line (the owner accepted the
/// suggestion or pressed "Balance on this line"). The line's own amount is
/// left out of the totals first, so it can be re-balanced. Returns false
/// (and changes nothing) when the other lines balance on their own, or for
/// an unknown key.
pub fn apply_balance(lines: &mut FormLines, key: RowKey) -> bool {
    let Some(i) = lines.lines.iter().position(|l| l.key == key) else {
        return false;
    };
    let mut without = lines.clone();
    without.lines[i].amount = None;
    let Some(side) = balance_suggestion(&without) else {
        return false;
    };
    let Some(signed) = side.signed() else {
        return false;
    };
    lines.lines[i].amount = Some(signed);
    true
}

/// What a name is, for the A/R-A/P rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct NameKind {
    /// A customer or a customer's job.
    pub customer: bool,
    /// A vendor.
    pub vendor: bool,
}

/// What [`check`] needs to know about the company and the engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "one flag per engine capability"
)]
pub struct JournalContext {
    /// Accounts Receivable accounts (system tag or detail type).
    pub receivable: BTreeSet<AccountId>,
    /// Accounts Payable accounts.
    pub payable: BTreeSet<AccountId>,
    /// The kind of each known name; a name not listed is not kind-checked
    /// (the engine decides).
    pub names: BTreeMap<ContactId, NameKind>,
    /// `je-names-v1`: names on lines, A/R and A/P lines.
    pub names_supported: bool,
    /// `je-number-v1`: an entry number.
    pub number_supported: bool,
    /// `je-adjusting-v1`: the Adjusting flag.
    pub adjusting_supported: bool,
    /// `je-reversing-v1`: Reverse on.
    pub reversing_supported: bool,
}

impl JournalContext {
    fn is_ar_ap(&self, a: AccountId) -> bool {
        self.receivable.contains(&a) || self.payable.contains(&a)
    }
}

/// Which part of the form a problem is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JournalField {
    /// Date.
    Date,
    /// Entry No.
    Number,
    /// Reverse on.
    ReverseOn,
    /// Adjusting entry.
    Adjusting,
    /// The lines as a whole (and the totals line).
    Lines,
    /// One line's account.
    LineAccount(RowKey),
    /// One line's debit or credit.
    LineAmount(RowKey),
    /// One line's name.
    LineName(RowKey),
}

/// Why an entry cannot be saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JournalProblem {
    /// No date.
    DateMissing,
    /// Entry No. is more than [`NUMBER_MAX_CHARS`] characters.
    NumberTooLong,
    /// An entry number needs an engine update (`je-number-v1`).
    NumberNeedsUpdate,
    /// The Adjusting flag needs an engine update.
    AdjustingNeedsUpdate,
    /// Reverse on needs an engine update.
    ReversingNeedsUpdate,
    /// Reverse on is on or before the entry date.
    ReverseNotAfter,
    /// Fewer than two lines with something on them.
    TooFewLines,
    /// More than [`MAX_LINES`] lines.
    TooManyLines,
    /// A used line has no account.
    LineAccountMissing(RowKey),
    /// A used line has neither a debit nor a credit.
    LineAmountMissing(RowKey),
    /// A line's amount is more than 999,999,999,999.99.
    LineAmountTooLarge(RowKey),
    /// The totals leave the money range.
    TooLarge,
    /// Debits and credits differ; `difference` = Σ debits − Σ credits.
    Unbalanced {
        /// Σ debits − Σ credits, exact.
        difference: Money,
    },
    /// An A/R or A/P line without a name.
    ArApNameMissing(RowKey),
    /// An A/R line whose name is not a customer.
    ArNeedsCustomer(RowKey),
    /// An A/P line whose name is not a vendor.
    ApNeedsVendor(RowKey),
    /// More than one A/R or A/P line.
    ArApTwoLines,
    /// A/R and A/P lines need an engine update (`je-names-v1`).
    ArApNeedsUpdate(RowKey),
    /// Names on lines need an engine update (`je-names-v1`).
    NameNeedsUpdate(RowKey),
}

impl JournalProblem {
    /// Plain-language message (strings live in `cl-ui::i18n`).
    #[must_use]
    pub const fn key(self) -> TextKey {
        TextKey(match self {
            JournalProblem::DateMissing => "journal.problem.date_missing",
            JournalProblem::NumberTooLong => "journal.problem.number_too_long",
            JournalProblem::NumberNeedsUpdate => "journal.problem.number_needs_update",
            JournalProblem::AdjustingNeedsUpdate => "journal.problem.adjusting_needs_update",
            JournalProblem::ReversingNeedsUpdate => "journal.problem.reversing_needs_update",
            JournalProblem::ReverseNotAfter => "journal.problem.reverse_not_after",
            JournalProblem::TooFewLines => "journal.problem.too_few_lines",
            JournalProblem::TooManyLines => "journal.problem.too_many_lines",
            JournalProblem::LineAccountMissing(_) => "journal.problem.line_account_missing",
            JournalProblem::LineAmountMissing(_) => "journal.problem.line_amount_missing",
            JournalProblem::LineAmountTooLarge(_) => "journal.problem.line_amount_too_large",
            JournalProblem::TooLarge => "journal.problem.too_large",
            JournalProblem::Unbalanced { .. } => "journal.problem.unbalanced",
            JournalProblem::ArApNameMissing(_) => "journal.problem.ar_ap_name_missing",
            JournalProblem::ArNeedsCustomer(_) => "journal.problem.ar_needs_customer",
            JournalProblem::ApNeedsVendor(_) => "journal.problem.ap_needs_vendor",
            JournalProblem::ArApTwoLines => "journal.problem.ar_ap_two_lines",
            JournalProblem::ArApNeedsUpdate(_) => "journal.problem.ar_ap_needs_update",
            JournalProblem::NameNeedsUpdate(_) => "journal.problem.name_needs_update",
        })
    }

    /// The control-channel name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            JournalProblem::DateMissing => "date_missing",
            JournalProblem::NumberTooLong => "number_too_long",
            JournalProblem::NumberNeedsUpdate => "number_needs_update",
            JournalProblem::AdjustingNeedsUpdate => "adjusting_needs_update",
            JournalProblem::ReversingNeedsUpdate => "reversing_needs_update",
            JournalProblem::ReverseNotAfter => "reverse_not_after",
            JournalProblem::TooFewLines => "too_few_lines",
            JournalProblem::TooManyLines => "too_many_lines",
            JournalProblem::LineAccountMissing(_) => "line_account_missing",
            JournalProblem::LineAmountMissing(_) => "line_amount_missing",
            JournalProblem::LineAmountTooLarge(_) => "line_amount_too_large",
            JournalProblem::TooLarge => "too_large",
            JournalProblem::Unbalanced { .. } => "unbalanced",
            JournalProblem::ArApNameMissing(_) => "ar_ap_name_missing",
            JournalProblem::ArNeedsCustomer(_) => "ar_needs_customer",
            JournalProblem::ApNeedsVendor(_) => "ap_needs_vendor",
            JournalProblem::ArApTwoLines => "ar_ap_two_lines",
            JournalProblem::ArApNeedsUpdate(_) => "ar_ap_needs_update",
            JournalProblem::NameNeedsUpdate(_) => "name_needs_update",
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> JournalField {
        match self {
            JournalProblem::DateMissing => JournalField::Date,
            JournalProblem::NumberTooLong | JournalProblem::NumberNeedsUpdate => {
                JournalField::Number
            }
            JournalProblem::AdjustingNeedsUpdate => JournalField::Adjusting,
            JournalProblem::ReversingNeedsUpdate | JournalProblem::ReverseNotAfter => {
                JournalField::ReverseOn
            }
            JournalProblem::TooFewLines
            | JournalProblem::TooManyLines
            | JournalProblem::TooLarge
            | JournalProblem::Unbalanced { .. }
            | JournalProblem::ArApTwoLines => JournalField::Lines,
            JournalProblem::LineAccountMissing(k) | JournalProblem::ArApNeedsUpdate(k) => {
                JournalField::LineAccount(k)
            }
            JournalProblem::LineAmountMissing(k) | JournalProblem::LineAmountTooLarge(k) => {
                JournalField::LineAmount(k)
            }
            JournalProblem::ArApNameMissing(k)
            | JournalProblem::ArNeedsCustomer(k)
            | JournalProblem::ApNeedsVendor(k)
            | JournalProblem::NameNeedsUpdate(k) => JournalField::LineName(k),
        }
    }
}

/// One checked line, ready to map onto the engine's journal line.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedJeLine {
    /// Account.
    pub account: AccountId,
    /// Debit or credit, above zero.
    pub side: JeSide,
    /// Line memo, when typed (trimmed).
    pub memo: Option<String>,
    /// Name.
    pub contact: Option<ContactId>,
    /// Tag (one per line, Q-233).
    pub tag: Option<TagId>,
}

impl fmt::Debug for CheckedJeLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedJeLine")
            .field("side", &self.side)
            .field("contact", &self.contact.is_some())
            .field("tag", &self.tag.is_some())
            .finish_non_exhaustive()
    }
}

/// An entry that passed [`check`]: balanced exactly, at least two lines,
/// each with one side above zero. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedJournal {
    /// Date.
    pub date: CalendarDate,
    /// Entry No., trimmed (`None` when blank).
    pub number: Option<String>,
    /// Memo, when typed (trimmed).
    pub memo: Option<String>,
    /// Adjusting.
    pub adjusting: bool,
    /// Reverse on (after `date`).
    pub reverse_on: Option<CalendarDate>,
    /// The lines in order.
    pub lines: Vec<CheckedJeLine>,
}

impl fmt::Debug for CheckedJournal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedJournal")
            .field("date", &self.date)
            .field("adjusting", &self.adjusting)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// The reason Save is disabled, if any: the first problem whose fix is on
/// the lines' totals (unbalanced, too few lines, too large). The form uses
/// it for the disabled Save buttons' tooltip and the status bar.
#[must_use]
pub fn save_blocker(form: &JournalForm) -> Option<JournalProblem> {
    let used = form.lines.used().count();
    if used < 2 {
        return Some(JournalProblem::TooFewLines);
    }
    match totals(&form.lines) {
        None => Some(JournalProblem::TooLarge),
        Some(t) if !t.balanced() => Some(JournalProblem::Unbalanced {
            difference: t.difference,
        }),
        Some(_) => None,
    }
}

/// Checks the form.
///
/// # Errors
/// Every problem found, in field order: date, number, adjusting, reverse
/// on, then the lines (each line's account, amount and name), then the
/// lines as a whole (count, A/R-A/P, balance).
#[allow(
    clippy::too_many_lines,
    reason = "one rule after another, in field order"
)]
pub fn check(
    form: &JournalForm,
    ctx: &JournalContext,
) -> Result<CheckedJournal, Vec<JournalProblem>> {
    let mut problems = Vec::new();
    if form.date.is_none() {
        problems.push(JournalProblem::DateMissing);
    }
    let number = trimmed(&form.number);
    if let Some(n) = &number {
        if n.chars().count() > NUMBER_MAX_CHARS {
            problems.push(JournalProblem::NumberTooLong);
        } else if !ctx.number_supported {
            problems.push(JournalProblem::NumberNeedsUpdate);
        }
    }
    if form.adjusting && !ctx.adjusting_supported {
        problems.push(JournalProblem::AdjustingNeedsUpdate);
    }
    if let Some(r) = form.reverse_on {
        if !ctx.reversing_supported {
            problems.push(JournalProblem::ReversingNeedsUpdate);
        } else if form.date.is_some_and(|d| r <= d) {
            problems.push(JournalProblem::ReverseNotAfter);
        }
    }
    let used: Vec<&FormLine> = form.lines.used().collect();
    let mut lines = Vec::with_capacity(used.len());
    let mut ar_ap_lines = 0_usize;
    for l in &used {
        let side = side_of(l.amount);
        match l.account {
            None => problems.push(JournalProblem::LineAccountMissing(l.key)),
            Some(a) if ctx.is_ar_ap(a) => {
                ar_ap_lines += 1;
                if ctx.names_supported {
                    match l.customer {
                        None => problems.push(JournalProblem::ArApNameMissing(l.key)),
                        Some(c) => {
                            let kind = ctx.names.get(&c);
                            if ctx.receivable.contains(&a) && kind.is_some_and(|k| !k.customer) {
                                problems.push(JournalProblem::ArNeedsCustomer(l.key));
                            }
                            if ctx.payable.contains(&a) && kind.is_some_and(|k| !k.vendor) {
                                problems.push(JournalProblem::ApNeedsVendor(l.key));
                            }
                        }
                    }
                } else {
                    problems.push(JournalProblem::ArApNeedsUpdate(l.key));
                }
            }
            Some(_) => {}
        }
        match side {
            None => problems.push(JournalProblem::LineAmountMissing(l.key)),
            Some(s) if s.amount().to_ten_thousandths() > MAX_CHECK_UNITS => {
                problems.push(JournalProblem::LineAmountTooLarge(l.key));
            }
            Some(_) => {}
        }
        let is_ar_ap = l.account.is_some_and(|a| ctx.is_ar_ap(a));
        if l.customer.is_some() && !ctx.names_supported && !is_ar_ap {
            problems.push(JournalProblem::NameNeedsUpdate(l.key));
        }
        if let (Some(account), Some(side)) = (l.account, side) {
            lines.push(CheckedJeLine {
                account,
                side,
                memo: trimmed(&l.memo),
                contact: l.customer,
                tag: l.tag,
            });
        }
    }
    if used.len() < 2 {
        problems.push(JournalProblem::TooFewLines);
    }
    if used.len() > MAX_LINES {
        problems.push(JournalProblem::TooManyLines);
    }
    if ar_ap_lines > 1 {
        problems.push(JournalProblem::ArApTwoLines);
    }
    match totals(&form.lines) {
        None => problems.push(JournalProblem::TooLarge),
        Some(t) if !t.balanced() => problems.push(JournalProblem::Unbalanced {
            difference: t.difference,
        }),
        Some(_) => {}
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match form.date {
        Some(date) => Ok(CheckedJournal {
            date,
            number,
            memo: trimmed(&form.memo),
            adjusting: form.adjusting,
            reverse_on: form.reverse_on,
            lines,
        }),
        // A missing date pushed a problem above; answer with it rather
        // than panic.
        None => Err(vec![JournalProblem::DateMissing]),
    }
}

/// The form for a saved entry's lines (Prev/Next, edit, Use template):
/// each `(account, side, memo, name, tag)` becomes a signed form line.
#[must_use]
pub fn line_from_side(
    key: RowKey,
    account: AccountId,
    side: JeSide,
    memo: Option<&str>,
    contact: Option<ContactId>,
    tag: Option<TagId>,
) -> FormLine {
    FormLine {
        account: Some(account),
        amount: side.signed(),
        memo: memo.unwrap_or_default().to_string(),
        customer: contact,
        tag,
        ..FormLine::blank(key)
    }
}

#[cfg(test)]
mod tests;
