//! The entry row's rules (P12 S3, Q-P12g, Q-P12p): which kind of
//! transaction a typed line becomes, and what stops it from being recorded.
//!
//! The amounts are the owner's typed values, kept as exact [`Money`]. The
//! only question asked of them here is "one side, more than zero"; nothing is
//! added up.

use std::fmt;

use cl_core::CalendarDate;
use cl_core::Money;
use cl_core::ids::{AccountId, ContactId, TagId};

use super::{AccountClass, Side};
use crate::text::TextKey;

/// The engine's `allowedEntryTypes` values the register knows. Unknown
/// values are dropped by the screen and never offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKindTag {
    /// `expense`: a check from a bank, a charge on a card.
    Expense,
    /// `deposit`.
    Deposit,
    /// `transfer`.
    Transfer,
    /// `journal_entry`.
    JournalEntry,
}

impl EntryKindTag {
    /// The engine's value.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            EntryKindTag::Expense => "expense",
            EntryKindTag::Deposit => "deposit",
            EntryKindTag::Transfer => "transfer",
            EntryKindTag::JournalEntry => "journal_entry",
        }
    }

    /// The tag for an engine value; `None` for one this build does not know.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<EntryKindTag> {
        [
            EntryKindTag::Expense,
            EntryKindTag::Deposit,
            EntryKindTag::Transfer,
            EntryKindTag::JournalEntry,
        ]
        .into_iter()
        .find(|t| t.wire() == s)
    }
}

/// What a recorded entry-row line becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A check (`POST /checks`).
    Check,
    /// A deposit.
    Deposit,
    /// A transfer between two bank or card accounts.
    Transfer,
    /// A charge on a credit card.
    CardCharge,
    /// A two-line journal entry.
    JournalEntry,
}

impl EntryKind {
    /// The chip shown in the entry row.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            EntryKind::Check => TextKey("register.kind.check"),
            EntryKind::Deposit => TextKey("register.kind.deposit"),
            EntryKind::Transfer => TextKey("register.kind.transfer"),
            EntryKind::CardCharge => TextKey("register.kind.card_charge"),
            EntryKind::JournalEntry => TextKey("register.kind.journal_entry"),
        }
    }

    /// The `allowedEntryTypes` value that must be present to offer it.
    #[must_use]
    pub const fn needs(self) -> EntryKindTag {
        match self {
            EntryKind::Check | EntryKind::CardCharge => EntryKindTag::Expense,
            EntryKind::Deposit => EntryKindTag::Deposit,
            EntryKind::Transfer => EntryKindTag::Transfer,
            EntryKind::JournalEntry => EntryKindTag::JournalEntry,
        }
    }
}

/// True when the register takes no entries at all: it shows "This register
/// is view-only. Use the forms to add transactions." instead of the row.
#[must_use]
pub fn is_view_only(allowed: &[EntryKindTag]) -> bool {
    allowed.is_empty()
}

/// The kind of transaction for an amount on `side` of a register of class
/// `this`, against an account of class `other` (Q-P12g):
///
/// - bank: the other account a bank or card → Transfer; A/R or A/P → a
///   two-line journal entry; otherwise Payment → Check, Deposit → Deposit;
/// - card: the other account a bank or card → Transfer; A/R or A/P → journal
///   entry; otherwise Charge → Card charge, Payment → journal entry (a card
///   credit);
/// - every other register: a two-line journal entry.
///
/// # Errors
/// [`EntryProblem::AccountMissing`] without another account,
/// [`EntryProblem::KindNotAllowed`] when the engine does not take that kind
/// in this register.
pub fn infer_kind(
    this: AccountClass,
    other: Option<AccountClass>,
    side: Side,
    allowed: &[EntryKindTag],
) -> Result<EntryKind, EntryProblem> {
    let other = other.ok_or(EntryProblem::AccountMissing)?;
    let kind = match this {
        AccountClass::Bank | AccountClass::CreditCard if other.is_money_account() => {
            EntryKind::Transfer
        }
        AccountClass::Bank | AccountClass::CreditCard if other.needs_name() => {
            EntryKind::JournalEntry
        }
        AccountClass::Bank => match side {
            Side::Payment => EntryKind::Check,
            Side::Deposit => EntryKind::Deposit,
        },
        AccountClass::CreditCard => match side {
            Side::Payment => EntryKind::CardCharge,
            Side::Deposit => EntryKind::JournalEntry,
        },
        AccountClass::Receivable
        | AccountClass::Payable
        | AccountClass::OtherAsset
        | AccountClass::OtherLiability
        | AccountClass::Equity
        | AccountClass::IncomeOrExpense => EntryKind::JournalEntry,
    };
    if allowed.contains(&kind.needs()) {
        Ok(kind)
    } else {
        Err(EntryProblem::KindNotAllowed)
    }
}

/// The entry row as typed. `Debug` shows which fields are filled, never
/// their contents.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct EntryDraft {
    /// Date.
    pub date: Option<CalendarDate>,
    /// Check or reference number.
    pub number: String,
    /// Payee (customer, vendor or other name).
    pub payee: Option<ContactId>,
    /// The other account.
    pub account: Option<AccountId>,
    /// Memo.
    pub memo: String,
    /// Amount in the first column.
    pub payment: Option<Money>,
    /// Amount in the second column.
    pub deposit: Option<Money>,
    /// Tags on the line (one at most, Q-233).
    pub tags: Vec<TagId>,
}

impl EntryDraft {
    /// True when nothing but the date is filled: Enter does nothing
    /// (Q-P10v).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.number.trim().is_empty()
            && self.payee.is_none()
            && self.account.is_none()
            && self.memo.trim().is_empty()
            && self.payment.is_none()
            && self.deposit.is_none()
            && self.tags.is_empty()
    }

    /// The side the amount is on, when exactly one side has one.
    #[must_use]
    pub fn side(&self) -> Option<Side> {
        match (self.payment, self.deposit) {
            (Some(_), None) => Some(Side::Payment),
            (None, Some(_)) => Some(Side::Deposit),
            _ => None,
        }
    }

    /// After a save: keep the date, clear the rest.
    pub fn clear_keep_date(&mut self) {
        *self = EntryDraft {
            date: self.date,
            ..EntryDraft::default()
        };
    }
}

impl fmt::Debug for EntryDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EntryDraft")
            .field("date", &self.date.is_some())
            .field("number", &!self.number.is_empty())
            .field("payee", &self.payee.is_some())
            .field("account", &self.account.is_some())
            .field("memo", &!self.memo.is_empty())
            .field("payment", &self.payment.is_some())
            .field("deposit", &self.deposit.is_some())
            .field("tags", &self.tags.len())
            .finish()
    }
}

/// What the entry row is checked against.
#[derive(Debug, Clone, Copy)]
pub struct EntryContext<'a> {
    /// The register's own account.
    pub this_account: AccountId,
    /// Its class.
    pub this_class: AccountClass,
    /// The class of the draft's other account, looked up by the screen;
    /// `None` when no account is chosen or it is no longer in the chart.
    pub other_class: Option<AccountClass>,
    /// The engine's `allowedEntryTypes` for this register.
    pub allowed: &'a [EntryKindTag],
}

/// The entry-row field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryField {
    /// Date.
    Date,
    /// Payee.
    Payee,
    /// Account.
    Account,
    /// Either amount.
    Amount,
    /// Tag.
    Tag,
}

/// Why the entry row cannot be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryProblem {
    /// No date.
    DateMissing,
    /// No amount in either column.
    AmountMissing,
    /// Amounts in both columns.
    BothSides,
    /// Zero or less.
    AmountNotPositive,
    /// No other account.
    AccountMissing,
    /// The chosen account is no longer in the chart.
    AccountGone,
    /// The other account is the register's own.
    SameAccount,
    /// A/R or A/P needs a customer or vendor name (Q-206).
    NameRequired,
    /// More than one tag on the line (Q-233).
    TooManyTags,
    /// The engine does not take this kind in this register.
    KindNotAllowed,
}

impl EntryProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            EntryProblem::DateMissing => TextKey("register.entry.problem.date_missing"),
            EntryProblem::AmountMissing => TextKey("register.entry.problem.amount_missing"),
            EntryProblem::BothSides => TextKey("register.entry.problem.both_sides"),
            EntryProblem::AmountNotPositive => {
                TextKey("register.entry.problem.amount_not_positive")
            }
            EntryProblem::AccountMissing => TextKey("register.entry.problem.account_missing"),
            EntryProblem::AccountGone => TextKey("register.entry.problem.account_gone"),
            EntryProblem::SameAccount => TextKey("register.entry.problem.same_account"),
            EntryProblem::NameRequired => TextKey("register.entry.problem.name_required"),
            EntryProblem::TooManyTags => TextKey("register.entry.problem.too_many_tags"),
            EntryProblem::KindNotAllowed => TextKey("register.entry.problem.kind_not_allowed"),
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> EntryField {
        match self {
            EntryProblem::DateMissing => EntryField::Date,
            EntryProblem::AmountMissing
            | EntryProblem::BothSides
            | EntryProblem::AmountNotPositive => EntryField::Amount,
            EntryProblem::AccountMissing
            | EntryProblem::AccountGone
            | EntryProblem::SameAccount
            | EntryProblem::KindNotAllowed => EntryField::Account,
            EntryProblem::NameRequired => EntryField::Payee,
            EntryProblem::TooManyTags => EntryField::Tag,
        }
    }
}

/// An entry row that passed [`check`], trimmed, ready for the posting
/// builders. `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct CheckedEntry {
    /// What it becomes.
    pub kind: EntryKind,
    /// Which column the amount was in.
    pub side: Side,
    /// The typed amount, more than zero.
    pub amount: Money,
    /// Date.
    pub date: CalendarDate,
    /// The other account.
    pub account: AccountId,
    /// Payee.
    pub payee: Option<ContactId>,
    /// Number, when typed.
    pub number: Option<String>,
    /// Memo, when typed.
    pub memo: Option<String>,
    /// Tag, when chosen.
    pub tag: Option<TagId>,
}

impl fmt::Debug for CheckedEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CheckedEntry")
            .field("kind", &self.kind)
            .field("side", &self.side)
            .finish_non_exhaustive()
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Checks the entry row (one side only, more than zero, an account that is
/// not the register's own, a name for A/R and A/P, one tag per line, a kind
/// the engine takes here).
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    draft: &EntryDraft,
    ctx: &EntryContext<'_>,
) -> Result<CheckedEntry, Vec<EntryProblem>> {
    let mut problems = Vec::new();
    if draft.date.is_none() {
        problems.push(EntryProblem::DateMissing);
    }
    let named =
        ctx.this_class.needs_name() || ctx.other_class.is_some_and(AccountClass::needs_name);
    // Q-206 (owner 2026-10-25): any number of A/R or A/P lines; each
    // still needs a name.
    if named && draft.payee.is_none() {
        problems.push(EntryProblem::NameRequired);
    }
    match draft.account {
        None => problems.push(EntryProblem::AccountMissing),
        Some(a) if a == ctx.this_account => problems.push(EntryProblem::SameAccount),
        Some(_) if ctx.other_class.is_none() => problems.push(EntryProblem::AccountGone),
        Some(_) => {}
    }
    let side = draft.side();
    let amount = match (draft.payment, draft.deposit) {
        (None, None) => {
            problems.push(EntryProblem::AmountMissing);
            None
        }
        (Some(_), Some(_)) => {
            problems.push(EntryProblem::BothSides);
            None
        }
        (Some(m), None) | (None, Some(m)) => {
            if m.is_positive() {
                Some(m)
            } else {
                problems.push(EntryProblem::AmountNotPositive);
                None
            }
        }
    };
    let mut kind = None;
    if let Some(s) = side
        && draft.account.is_some_and(|a| a != ctx.this_account)
        && ctx.other_class.is_some()
    {
        match infer_kind(ctx.this_class, ctx.other_class, s, ctx.allowed) {
            Ok(k) => kind = Some(k),
            Err(p) => problems.push(p),
        }
    }
    if draft.tags.len() > 1 {
        problems.push(EntryProblem::TooManyTags);
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    match (kind, side, amount, draft.date, draft.account) {
        (Some(kind), Some(side), Some(amount), Some(date), Some(account)) => Ok(CheckedEntry {
            kind,
            side,
            amount,
            date,
            account,
            payee: draft.payee,
            number: non_empty(&draft.number),
            memo: non_empty(&draft.memo),
            tag: draft.tags.first().copied(),
        }),
        // Every `None` above pushed a problem, so this arm is unreachable;
        // answer with a problem rather than panic.
        _ => Err(vec![EntryProblem::AccountMissing]),
    }
}
