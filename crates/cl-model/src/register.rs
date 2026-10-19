//! Register rules without egui (P12): the date range and sort choices, the
//! quick filters, the per-account memory (Q-186), the page window over the
//! engine's register, the column labels and display sign for each kind of
//! account, and the entry-row rules.
//!
//! The engine is the ledger. Nothing here adds amounts up: every balance a
//! register shows is a value the engine sent, carried as it came
//! (non-negotiable 7, Q-187). The only arithmetic on an engine balance is the
//! display sign flip for credit-normal accounts (Q-P12h), which is
//! [`Money::checked_neg`] and nothing else.
//!
//! No cl-api types appear here (layering): cl-ui maps the API's register
//! lines into its own row type and hands them to [`WindowCache`].

use std::fmt;

use cl_core::fiscal::FiscalYearSpec;
use cl_core::ids::TagId;
use cl_core::{CalendarDate, Money, MoneyError};
use serde::{Deserialize, Serialize};

use crate::lists::accounts::AccountKind;
use crate::text::TextKey;

mod entry;
mod window;

pub use entry::{
    CheckedEntry, EntryContext, EntryDraft, EntryField, EntryKind, EntryKindTag, EntryProblem,
    check, infer_kind, is_view_only,
};
pub use window::{MAX_PAGES, PAGE, PageNo, PageRequest, Reply, RowView, WindowCache};

// ---------------------------------------------------------------------------
// Date ranges
// ---------------------------------------------------------------------------

/// A resolved, inclusive date range. Both ends are always sent to the
/// engine, so its own "last 90 days by server UTC" default never applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DateRange {
    /// First day shown.
    pub start: CalendarDate,
    /// Last day shown.
    pub end: CalendarDate,
}

/// The "Show" choice in the register toolbar.
///
/// Relative presets stay relative: what is remembered is "this month", not
/// the dates it meant on the day it was chosen (Q-P12l).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RangePreset {
    /// The 90 days up to today (the default, Q-186).
    #[default]
    Last90Days,
    /// The calendar month containing today.
    ThisMonth,
    /// The calendar month before this one.
    LastMonth,
    /// The fiscal quarter containing today.
    ThisQuarter,
    /// The fiscal year containing today.
    ThisYear,
    /// The fiscal year before this one.
    LastYear,
    /// Every date the app supports.
    AllDates,
    /// Dates the owner typed; `from <= to` (see [`RangePreset::custom`]).
    Custom {
        /// First day.
        from: CalendarDate,
        /// Last day.
        to: CalendarDate,
    },
}

/// Why a custom range is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeProblem {
    /// From is after To.
    FromAfterTo,
}

impl RangeProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            RangeProblem::FromAfterTo => TextKey("register.range.problem.from_after_to"),
        }
    }
}

impl RangePreset {
    /// The presets offered in the picker, in order (Custom is offered as
    /// "Custom dates…" after them).
    pub const PRESETS: [RangePreset; 7] = [
        RangePreset::Last90Days,
        RangePreset::ThisMonth,
        RangePreset::LastMonth,
        RangePreset::ThisQuarter,
        RangePreset::ThisYear,
        RangePreset::LastYear,
        RangePreset::AllDates,
    ];

    /// A custom range.
    ///
    /// # Errors
    /// [`RangeProblem::FromAfterTo`] when `from` is after `to`.
    pub fn custom(from: CalendarDate, to: CalendarDate) -> Result<RangePreset, RangeProblem> {
        if from > to {
            return Err(RangeProblem::FromAfterTo);
        }
        Ok(RangePreset::Custom { from, to })
    }

    /// Picker label.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            RangePreset::Last90Days => TextKey("register.range.last_90_days"),
            RangePreset::ThisMonth => TextKey("register.range.this_month"),
            RangePreset::LastMonth => TextKey("register.range.last_month"),
            RangePreset::ThisQuarter => TextKey("register.range.this_quarter"),
            RangePreset::ThisYear => TextKey("register.range.this_year"),
            RangePreset::LastYear => TextKey("register.range.last_year"),
            RangePreset::AllDates => TextKey("register.range.all_dates"),
            RangePreset::Custom { .. } => TextKey("register.range.custom"),
        }
    }

    /// The dates this choice means on `today` (the local clock, never the
    /// engine's). Quarters and years follow the company's fiscal year.
    ///
    /// Never fails: at the edges of the supported dates (1900, 2199) the
    /// range is clipped to them, and a fiscal setup the calendar cannot
    /// place falls back to calendar quarters and years.
    #[must_use]
    pub fn resolve(self, today: CalendarDate, fiscal: &FiscalYearSpec) -> DateRange {
        let range = |start: CalendarDate, end: CalendarDate| DateRange { start, end };
        match self {
            RangePreset::Last90Days => range(
                today.add_days(-90).unwrap_or_else(|_| CalendarDate::min()),
                today,
            ),
            RangePreset::ThisMonth => range(today.start_of_month(), today.end_of_month()),
            RangePreset::LastMonth => match today.start_of_month().add_days(-1) {
                Ok(prev) => range(prev.start_of_month(), prev.end_of_month()),
                // January 1900: there is no earlier month; show the first one.
                Err(_) => range(today.start_of_month(), today.end_of_month()),
            },
            RangePreset::ThisQuarter => fiscal
                .year_containing(today)
                .and_then(|fy| fiscal.quarters(&fy))
                .ok()
                .and_then(|qs| qs.into_iter().find(|q| q.contains(today)))
                .map_or_else(
                    || range(today.start_of_quarter(), today.end_of_quarter()),
                    |q| range(q.start, q.end),
                ),
            RangePreset::ThisYear => fiscal.year_containing(today).map_or_else(
                |_| range(today.start_of_year(), today.end_of_year()),
                |fy| range(fy.start, fy.end),
            ),
            RangePreset::LastYear => fiscal
                .year_containing(today)
                .and_then(|fy| fiscal.year(fy.label - 1))
                .map_or_else(
                    |_| {
                        let before = today
                            .start_of_year()
                            .add_days(-1)
                            .unwrap_or_else(|_| today.start_of_year());
                        range(before.start_of_year(), before.end_of_year())
                    },
                    |fy| range(fy.start, fy.end),
                ),
            RangePreset::AllDates => range(CalendarDate::min(), CalendarDate::max()),
            RangePreset::Custom { from, to } => range(from.min(to), from.max(to)),
        }
    }

    /// The remembered form: a preset name, or `custom:<from>..<to>`.
    #[must_use]
    pub fn to_pref(self) -> String {
        match self {
            RangePreset::Last90Days => "last_90_days".into(),
            RangePreset::ThisMonth => "this_month".into(),
            RangePreset::LastMonth => "last_month".into(),
            RangePreset::ThisQuarter => "this_quarter".into(),
            RangePreset::ThisYear => "this_year".into(),
            RangePreset::LastYear => "last_year".into(),
            RangePreset::AllDates => "all_dates".into(),
            RangePreset::Custom { from, to } => {
                format!("custom:{}..{}", from.to_wire(), to.to_wire())
            }
        }
    }

    /// Reads [`RangePreset::to_pref`] text; `None` for anything else (a
    /// later version's preset, a hand-edited file).
    #[must_use]
    pub fn from_pref(s: &str) -> Option<RangePreset> {
        if let Some(rest) = s.strip_prefix("custom:") {
            let (from, to) = rest.split_once("..")?;
            let from = CalendarDate::parse_wire(from).ok()?;
            let to = CalendarDate::parse_wire(to).ok()?;
            return RangePreset::custom(from, to).ok();
        }
        RangePreset::PRESETS.into_iter().find(|p| p.to_pref() == s)
    }
}

// ---------------------------------------------------------------------------
// Sort
// ---------------------------------------------------------------------------

/// The "Sort by" choice. Balances never depend on it: the engine reports
/// each line's balance in date order whatever the sort (R1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SortChoice {
    /// Date, oldest first (the default, Q-186).
    #[default]
    DateOldest,
    /// Date, newest first.
    DateNewest,
    /// Check or reference number.
    Number,
    /// Amount.
    Amount,
    /// Transaction type.
    Type,
}

impl SortChoice {
    /// Every choice, in picker order.
    pub const ALL: [SortChoice; 5] = [
        SortChoice::DateOldest,
        SortChoice::DateNewest,
        SortChoice::Number,
        SortChoice::Amount,
        SortChoice::Type,
    ];

    /// The engine's `sort_by` value.
    #[must_use]
    pub const fn sort_by_wire(self) -> &'static str {
        match self {
            SortChoice::DateOldest | SortChoice::DateNewest => "date",
            SortChoice::Number => "ref_no",
            SortChoice::Amount => "amount",
            SortChoice::Type => "type",
        }
    }

    /// True for `sort_dir=desc`.
    #[must_use]
    pub const fn descending(self) -> bool {
        matches!(self, SortChoice::DateNewest)
    }

    /// True for either date order (Go To can find an offset by date).
    #[must_use]
    pub const fn is_date(self) -> bool {
        matches!(self, SortChoice::DateOldest | SortChoice::DateNewest)
    }

    /// Picker label.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            SortChoice::DateOldest => TextKey("register.sort.date_oldest"),
            SortChoice::DateNewest => TextKey("register.sort.date_newest"),
            SortChoice::Number => TextKey("register.sort.number"),
            SortChoice::Amount => TextKey("register.sort.amount"),
            SortChoice::Type => TextKey("register.sort.type"),
        }
    }

    const fn pref(self) -> &'static str {
        match self {
            SortChoice::DateOldest => "date_oldest",
            SortChoice::DateNewest => "date_newest",
            SortChoice::Number => "number",
            SortChoice::Amount => "amount",
            SortChoice::Type => "type",
        }
    }

    fn from_pref(s: &str) -> Option<SortChoice> {
        SortChoice::ALL.into_iter().find(|c| c.pref() == s)
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// The quick filters (Q-189): uncleared, payee, tag, amount range.
///
/// `Debug` says which filters are on, never the payee or the amounts.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// Only lines not yet cleared (needs the engine's R3).
    pub uncleared: bool,
    /// Payee name text (the engine matches it).
    pub payee: Option<String>,
    /// Lines with this tag (needs the engine's R3).
    pub tag: Option<TagId>,
    /// Lowest amount, typed exactly.
    pub min: Option<Money>,
    /// Highest amount, typed exactly.
    pub max: Option<Money>,
}

/// Why the filters cannot be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterProblem {
    /// The lowest amount is above the highest.
    MinAboveMax,
}

impl FilterProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            FilterProblem::MinAboveMax => TextKey("register.filter.problem.min_above_max"),
        }
    }
}

impl Filters {
    /// The payee text trimmed, `None` when blank.
    #[must_use]
    pub fn payee_text(&self) -> Option<&str> {
        self.payee
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    /// True when any filter narrows the register.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.uncleared
            || self.payee_text().is_some()
            || self.tag.is_some()
            || self.min.is_some()
            || self.max.is_some()
    }

    /// True when a filter that needs the engine's `register-exact-v1` (R3)
    /// is on; without it those filters are switched off (Q-P12d).
    #[must_use]
    pub fn needs_register_exact(&self) -> bool {
        self.uncleared || self.tag.is_some()
    }

    /// Checks the amount range.
    ///
    /// # Errors
    /// [`FilterProblem::MinAboveMax`].
    pub fn check(&self) -> Result<(), FilterProblem> {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) if lo > hi => Err(FilterProblem::MinAboveMax),
            _ => Ok(()),
        }
    }

    /// "Clear filters".
    pub fn clear(&mut self) {
        *self = Filters::default();
    }
}

impl fmt::Debug for Filters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Filters")
            .field("uncleared", &self.uncleared)
            .field("payee", &self.payee_text().is_some())
            .field("tag", &self.tag.is_some())
            .field("min", &self.min.is_some())
            .field("max", &self.max.is_some())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Per-account memory
// ---------------------------------------------------------------------------

/// The version written by this build.
pub const PREFS_VERSION: u32 = 1;

/// What a register remembers per company and account (Q-186, Q-P12l). The
/// UI stores it; this type only says what is kept and how it reads back.
///
/// Reading is lenient: unknown fields are ignored, and a value this build
/// does not know (a later version's preset) falls back to its default
/// instead of failing the whole record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(from = "StoredPrefs", into = "StoredPrefs")]
pub struct RegisterPrefs {
    /// The "Show" range, kept relative.
    pub range: RangePreset,
    /// The "Sort by" choice.
    pub sort: SortChoice,
    /// One-line display.
    pub one_line: bool,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct StoredPrefs {
    v: u32,
    range: Option<String>,
    sort: Option<String>,
    one_line: bool,
}

impl From<StoredPrefs> for RegisterPrefs {
    fn from(s: StoredPrefs) -> RegisterPrefs {
        RegisterPrefs {
            range: s
                .range
                .as_deref()
                .and_then(RangePreset::from_pref)
                .unwrap_or_default(),
            sort: s
                .sort
                .as_deref()
                .and_then(SortChoice::from_pref)
                .unwrap_or_default(),
            one_line: s.one_line,
        }
    }
}

impl From<RegisterPrefs> for StoredPrefs {
    fn from(p: RegisterPrefs) -> StoredPrefs {
        StoredPrefs {
            v: PREFS_VERSION,
            range: Some(p.range.to_pref()),
            sort: Some(p.sort.pref().to_string()),
            one_line: p.one_line,
        }
    }
}

// ---------------------------------------------------------------------------
// Balances, accounts, labels
// ---------------------------------------------------------------------------

/// How the Balance column is filled (Q-P12c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BalanceMode {
    /// Each cell shows the engine's running balance for that line.
    Engine,
    /// A filter is on and the engine's balances still follow the filter
    /// (before R1), so the cells show a dash instead of a wrong figure.
    HiddenFiltered,
}

impl BalanceMode {
    /// The mode for the current filters and engine capability
    /// (`register-exact-v1`).
    #[must_use]
    pub const fn choose(filters_active: bool, register_exact: bool) -> BalanceMode {
        if filters_active && !register_exact {
            BalanceMode::HiddenFiltered
        } else {
            BalanceMode::Engine
        }
    }

    /// Header tooltip when the balances are hidden.
    #[must_use]
    pub const fn tooltip(self) -> Option<TextKey> {
        match self {
            BalanceMode::Engine => None,
            BalanceMode::HiddenFiltered => Some(TextKey("register.balance.hidden_filtered")),
        }
    }
}

/// What kind of register an account has. The chart's eight types are not
/// enough: a bank and a credit card are both told apart by detail type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountClass {
    /// Asset with detail type `bank`.
    Bank,
    /// Liability with detail type `credit_card`.
    CreditCard,
    /// Asset with detail type `accounts_receivable`.
    Receivable,
    /// Liability with detail type `accounts_payable`.
    Payable,
    /// Any other asset.
    OtherAsset,
    /// Any other liability.
    OtherLiability,
    /// Equity.
    Equity,
    /// Income, cost of goods sold or expense (no register).
    IncomeOrExpense,
}

impl AccountClass {
    /// The class of an account of `kind` with engine detail type `detail`.
    #[must_use]
    pub fn of(kind: AccountKind, detail: Option<&str>) -> AccountClass {
        match (kind, detail) {
            (AccountKind::Asset, Some("bank")) => AccountClass::Bank,
            (AccountKind::Asset, Some("accounts_receivable")) => AccountClass::Receivable,
            (AccountKind::Asset, _) => AccountClass::OtherAsset,
            (AccountKind::Liability, Some("credit_card")) => AccountClass::CreditCard,
            (AccountKind::Liability, Some("accounts_payable")) => AccountClass::Payable,
            (AccountKind::Liability, _) => AccountClass::OtherLiability,
            (AccountKind::Equity, _) => AccountClass::Equity,
            (
                AccountKind::Income
                | AccountKind::Cogs
                | AccountKind::Expense
                | AccountKind::OtherIncome
                | AccountKind::OtherExpense,
                _,
            ) => AccountClass::IncomeOrExpense,
        }
    }

    /// True for balance-sheet accounts, which have a register.
    #[must_use]
    pub const fn has_register(self) -> bool {
        !matches!(self, AccountClass::IncomeOrExpense)
    }

    /// True when the account normally carries a credit balance.
    #[must_use]
    pub const fn is_credit_normal(self) -> bool {
        matches!(
            self,
            AccountClass::CreditCard
                | AccountClass::Payable
                | AccountClass::OtherLiability
                | AccountClass::Equity
        )
    }

    /// True for A/R and A/P: an entry on them needs a customer or vendor
    /// name (Q-206).
    #[must_use]
    pub const fn needs_name(self) -> bool {
        matches!(self, AccountClass::Receivable | AccountClass::Payable)
    }

    /// True for bank and credit card accounts (transfers between them).
    #[must_use]
    pub const fn is_money_account(self) -> bool {
        matches!(self, AccountClass::Bank | AccountClass::CreditCard)
    }
}

/// Which amount column a line's amount sits in. The first column holds
/// credits to the register's account, the second debits, whatever their
/// labels say (WP1: payment = credit, deposit = debit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// The first amount column (credit): Payment, Charge, Decrease or
    /// Increase by account.
    Payment,
    /// The second amount column (debit): Deposit, Payment, Increase or
    /// Decrease by account.
    Deposit,
}

/// The two amount column headings for a register (Q-P12g).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColumnLabels {
    /// First amount column (credits).
    pub payment: TextKey,
    /// Second amount column (debits).
    pub deposit: TextKey,
}

impl ColumnLabels {
    /// The headings for a register of `class`.
    #[must_use]
    pub const fn for_class(class: AccountClass) -> ColumnLabels {
        match class {
            AccountClass::Bank | AccountClass::IncomeOrExpense => ColumnLabels {
                payment: TextKey("register.col.payment"),
                deposit: TextKey("register.col.deposit"),
            },
            AccountClass::CreditCard => ColumnLabels {
                payment: TextKey("register.col.charge"),
                deposit: TextKey("register.col.card_payment"),
            },
            AccountClass::Receivable | AccountClass::OtherAsset => ColumnLabels {
                payment: TextKey("register.col.decrease"),
                deposit: TextKey("register.col.increase"),
            },
            AccountClass::Payable | AccountClass::OtherLiability | AccountClass::Equity => {
                ColumnLabels {
                    payment: TextKey("register.col.increase"),
                    deposit: TextKey("register.col.decrease"),
                }
            }
        }
    }
}

/// How an engine balance is shown (Q-P12h). The engine reports register
/// balances as debits minus credits; a credit-normal account shows them
/// with its normal sign, so a card's balance owed reads positive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplaySign {
    /// Shown as sent.
    AsSent,
    /// Shown negated.
    Negated,
}

impl DisplaySign {
    /// The sign rule for a register of `class`.
    #[must_use]
    pub const fn for_class(class: AccountClass) -> DisplaySign {
        if class.is_credit_normal() {
            DisplaySign::Negated
        } else {
            DisplaySign::AsSent
        }
    }

    /// The value to show for the engine's `value`. This is the only
    /// operation ever applied to an engine balance.
    ///
    /// # Errors
    /// [`MoneyError`] if the negation leaves the supported range.
    pub fn apply(self, value: Money) -> Result<Money, MoneyError> {
        match self {
            DisplaySign::AsSent => Ok(value),
            DisplaySign::Negated => value.checked_neg(),
        }
    }
}

#[cfg(test)]
mod tests;
