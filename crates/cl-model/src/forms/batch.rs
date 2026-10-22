//! Bulk Entry (P15A): sections, rows and local checks, with no egui and no
//! engine types.
//!
//! WP0 skeleton: the types the screen and the demo agree on. Lane B fills
//! in the behavior (`target/plans/P15A.md` §3 WP-B1–B3): columns per
//! section kind, `RowDraft::hash`, `local_problems`, the batch state and
//! Save all gating, totals, and the paste and mapping helpers.
//!
//! Money is exact: every amount is a [`Money`] rounded to the cent when it
//! is typed or pasted (Q-232). Totals are sums of the owner's own amounts;
//! balances are never derived here (non-negotiable 7).

use cl_core::ids::{AccountId, ContactId, TagId};
use cl_core::{CalendarDate, Money};

/// The most rows one batch holds (Q-P15Ag).
pub const MAX_ROWS: usize = 2_000;

/// A section's type (Q-P15Ab, Q-263).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SectionKind {
    /// Checks from a bank account.
    Checks,
    /// Expenses (no check number) from a bank account.
    Expenses,
    /// Charges on a credit card account.
    CardCharges,
    /// Credits (refunds) on a credit card account.
    CardCredits,
    /// Deposits into a bank account.
    Deposits,
    /// Journal entries (no context account).
    JournalEntries,
}

impl SectionKind {
    /// Every kind, in the order the Type picker lists them.
    pub const ALL: [SectionKind; 6] = [
        SectionKind::Checks,
        SectionKind::Expenses,
        SectionKind::CardCharges,
        SectionKind::CardCredits,
        SectionKind::Deposits,
        SectionKind::JournalEntries,
    ];
}

/// A cell of a Bulk Entry row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    /// Date.
    Date,
    /// Check number, Ref no. or Entry No.
    Number,
    /// Print later (checks).
    PrintLater,
    /// Pay to, Payee, Received from or Name.
    Contact,
    /// The line account.
    Account,
    /// Amount.
    Amount,
    /// Debit (journal entries).
    Debit,
    /// Credit (journal entries).
    Credit,
    /// Memo.
    Memo,
    /// Tag (one per line, Q-233).
    Tag,
}

/// A row's identity inside the window: stable across inserts, deletes and
/// sorting (the engine's row numbers are assigned at send time).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RowKey(pub u64);

/// One row as the owner typed or pasted it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RowDraft {
    /// The row's identity.
    pub key: Option<RowKey>,
    /// Date.
    pub date: Option<CalendarDate>,
    /// Check number, Ref no. or Entry No.
    pub number: String,
    /// Print later (checks).
    pub print_later: bool,
    /// The name, when picked from the list.
    pub contact: Option<ContactId>,
    /// The name as typed.
    pub contact_text: String,
    /// The line account, when picked from the list.
    pub account: Option<AccountId>,
    /// The line account as typed.
    pub account_text: String,
    /// Amount (every kind but journal entries).
    pub amount: Option<Money>,
    /// Debit (journal entries).
    pub debit: Option<Money>,
    /// Credit (journal entries).
    pub credit: Option<Money>,
    /// Memo.
    pub memo: String,
    /// Tag.
    pub tag: Option<TagId>,
}

/// The key and which cells are filled only: no name, memo or amount.
impl std::fmt::Debug for RowDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowDraft")
            .field("key", &self.key)
            .field("date", &self.date.is_some())
            .field("contact", &self.contact)
            .field("account", &self.account)
            .field("amount", &self.amount.is_some())
            .field("debit", &self.debit.is_some())
            .field("credit", &self.credit.is_some())
            .finish_non_exhaustive()
    }
}

/// One section: rows of one kind on one context account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    /// What the rows become.
    pub kind: SectionKind,
    /// The bank or card account (`None` for journal entries).
    pub context: Option<AccountId>,
    /// The rows, in grid order.
    pub rows: Vec<RowDraft>,
}

/// A journal entry group: the rows with the same date and Entry No.
/// (Q-P15Aj).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct JeGroupKey {
    /// The entry date.
    pub date: CalendarDate,
    /// The Entry No.
    pub number: String,
}

/// What the app itself finds wrong with a row, before the engine checks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// A required cell is empty.
    Missing(Field),
    /// An amount is zero or negative.
    NotPositive,
    /// A journal entry row has both a debit and a credit.
    BothDebitCredit,
    /// The check number is used twice in this batch.
    DuplicateNumber,
    /// A journal entry group's debits and credits differ by `difference`.
    JeUnbalanced {
        /// The group.
        group: JeGroupKey,
        /// Debits minus credits (exact).
        difference: Money,
    },
    /// The date is on or before the closing date.
    Closed,
}
