//! Bank feed review (P26): the review queue's list, counts, candidates and
//! the single-item actions the desktop sends (E1 U-164, E2 U-166/U-168/
//! U-172–U-174, E4 U-171/U-77).
//!
//! - Every amount is an exact decimal string in the **feed sign** (positive
//!   = money out); a JSON number where an amount belongs leaves the row out
//!   (counted in [`FeedPage::skipped`]), it could never post exactly.
//! - Split and fee amounts are sent with at most 2 decimals and 12 integer
//!   digits ([`crate::wire_decimal`]); the engine re-checks every total.
//! - Categorize, approve and match send an `Idempotency-Key` the caller
//!   keeps (a retry reuses it); duplicate, review again, transfer and
//!   deposit need one (keyed under `feed-idempotency-v1` and
//!   `feed-match-post-v1`).
//! - 409s stay [`ApiError::Conflict`] (`FEED_ITEM_NOT_PENDING {status}`,
//!   `LINE_ALREADY_MATCHED`, `FEED_ITEM_IN_USE`, …); [`feed_refusal`] names
//!   them and the 400s for the screen. A 403 on a write is a view-only user
//!   ([`is_view_only`]).
//! - The one accept and one undo route (E3, U-170/U-169) are in
//!   [`crate::resources::feed_accept`]; transfer, deposit and match answer
//!   their accept group ([`FeedPosted::undo_group`]) for Undo.

use std::fmt;
use std::str::FromStr;

use cl_core::ids::{AccountId, ContactId, JournalLineId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::bank_import::wire_decimal;
use crate::resources::customer_payments::error_code;
use crate::resources::feed::{Feed, FeedItemId};
use crate::resources::feed_accept::{UndoGroupId, undo_group_of};
use crate::resources::operations::{
    BANK_CONNECTIONS, DASHBOARD_ACCOUNT_BALANCES, FEED_APPROVE, FEED_ASSIGN, FEED_CATEGORIZE,
    FEED_COUNTS, FEED_DEPOSIT, FEED_DUPLICATE, FEED_EXCLUDE, FEED_LIST, FEED_MATCH,
    FEED_MATCH_CANDIDATES, FEED_PAYROLL_OVERLAP, FEED_REVIEW_AGAIN, FEED_TRANSFER,
    FEED_TRANSFER_CANDIDATES, FEED_UNEXCLUDE, Operation,
};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::session::json_body;
use crate::wire::Capabilities;

/// The most rows one list page asks for (the engine allows 500).
pub const FEED_PAGE_MAX: u16 = 500;
/// The most split lines (the engine allows 2-50).
pub const SPLIT_LINES_MAX: usize = 50;
/// The most fee lines on a deposit.
pub const FEE_LINES_MAX: usize = 10;
/// The most payments in one deposit.
pub const DEPOSIT_PAYMENTS_MAX: usize = 100;
/// The transfer window the screen uses (Q-P26l); the engine takes 1-10.
pub const TRANSFER_WINDOW_DAYS: u8 = 3;

/// A bank connection's id (one per linked bank account).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BankConnectionId(uuid::Uuid);

impl BankConnectionId {
    /// Parses a connection id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<BankConnectionId, ApiError> {
        cl_core::ids::parse_uuid(s)
            .map(BankConnectionId)
            .map_err(|_| ApiError::Decode {
                what: "bank connection id",
            })
    }
}

impl fmt::Display for BankConnectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A waiting row's review bucket (U-174; first match wins: duplicate and
/// review, then matches, then ready).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeedBucket {
    /// Ready to accept.
    Ready,
    /// Needs the owner (no account, low confidence, a possible duplicate).
    Review,
    /// Has a match signal.
    Matches,
}

impl FeedBucket {
    /// The wire word.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FeedBucket::Ready => "ready",
            FeedBucket::Review => "review",
            FeedBucket::Matches => "matches",
        }
    }

    fn parse(s: &str) -> Option<FeedBucket> {
        match s {
            "ready" => Some(FeedBucket::Ready),
            "review" => Some(FeedBucket::Review),
            "matches" => Some(FeedBucket::Matches),
            _ => None,
        }
    }
}

/// A feed item's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FeedStatus {
    /// Waiting, nothing staged.
    Pending,
    /// Waiting with an account staged.
    Assigned,
    /// Being posted right now.
    Categorizing,
    /// Matched to an entry already in the books.
    Matched,
    /// Posted as a new entry.
    Categorized,
    /// Left out by the owner.
    Excluded,
    /// A state this client doesn't know.
    Other,
}

impl FeedStatus {
    /// The wire word (`other` for an unknown one).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FeedStatus::Pending => "pending",
            FeedStatus::Assigned => "assigned",
            FeedStatus::Categorizing => "categorizing",
            FeedStatus::Matched => "matched",
            FeedStatus::Categorized => "categorized",
            FeedStatus::Excluded => "excluded",
            FeedStatus::Other => "other",
        }
    }

    /// Parses the wire word (anything unknown is [`FeedStatus::Other`]).
    #[must_use]
    pub fn parse(s: &str) -> FeedStatus {
        match s {
            "pending" => FeedStatus::Pending,
            "assigned" => FeedStatus::Assigned,
            "categorizing" => FeedStatus::Categorizing,
            "matched" => FeedStatus::Matched,
            "categorized" => FeedStatus::Categorized,
            "excluded" => FeedStatus::Excluded,
            _ => FeedStatus::Other,
        }
    }

    /// Pending or assigned: waiting for the owner.
    #[must_use]
    pub const fn is_waiting(self) -> bool {
        matches!(self, FeedStatus::Pending | FeedStatus::Assigned)
    }

    /// Matched or categorized: in the books.
    #[must_use]
    pub const fn is_accepted(self) -> bool {
        matches!(self, FeedStatus::Matched | FeedStatus::Categorized)
    }
}

/// What a list page asks for. `Default` is every row, 50 at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedQuery {
    /// One state.
    pub status: Option<FeedStatus>,
    /// Only waiting rows (pending and assigned); ignored with `status`.
    pub waiting_only: bool,
    /// Every accepted row, categorized and matched, voided entries
    /// included (`status=accepted`, `feed-accept-v1`, Q-P26y); ignored
    /// with `status`.
    pub accepted: bool,
    /// One review bucket.
    pub bucket: Option<FeedBucket>,
    /// Possible duplicates waiting for a decision (true) or the others.
    pub possible_duplicate: Option<bool>,
    /// Rows whose entry was voided (true) or the others.
    pub entry_voided: Option<bool>,
    /// One bank connection (account).
    pub connection: Option<BankConnectionId>,
    /// Description or amount text (owner data: sent, never logged).
    pub search: Option<String>,
    /// Page size, 1..=500.
    pub limit: u16,
    /// First row.
    pub offset: u32,
}

impl Default for FeedQuery {
    fn default() -> FeedQuery {
        FeedQuery {
            status: None,
            waiting_only: false,
            accepted: false,
            bucket: None,
            possible_duplicate: None,
            entry_voided: None,
            connection: None,
            search: None,
            limit: 50,
            offset: 0,
        }
    }
}

impl FeedQuery {
    /// The query string, in a fixed order.
    ///
    /// # Errors
    /// `Local(PageLimit)` for a page size outside 1..=500.
    pub fn to_query(&self) -> Result<Vec<(String, String)>, ApiError> {
        if self.limit == 0 || self.limit > FEED_PAGE_MAX {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        let mut q: Vec<(String, String)> = Vec::new();
        let flag = |b: bool| if b { "true" } else { "false" }.to_string();
        if let Some(s) = self.status {
            q.push(("status".into(), s.as_str().into()));
        } else if self.accepted {
            q.push(("status".into(), "accepted".into()));
        } else if self.waiting_only {
            q.push(("actionableOnly".into(), "true".into()));
        }
        if let Some(b) = self.bucket {
            q.push(("bucket".into(), b.as_str().into()));
        }
        if let Some(b) = self.possible_duplicate {
            q.push(("possibleDuplicate".into(), flag(b)));
        }
        if let Some(b) = self.entry_voided {
            q.push(("entryVoided".into(), flag(b)));
        }
        if let Some(c) = self.connection {
            q.push(("bankConnectionId".into(), c.to_string()));
        }
        if let Some(s) = self
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            q.push(("search".into(), s.to_string()));
        }
        q.push(("sortBy".into(), "feedDate".into()));
        q.push(("sortDir".into(), "desc".into()));
        q.push(("limit".into(), self.limit.to_string()));
        q.push(("offset".into(), self.offset.to_string()));
        Ok(q)
    }
}

/// An id with its display name (an account, a contact, a tag).
#[derive(Clone, PartialEq, Eq)]
pub struct Named<T> {
    /// The id.
    pub id: T,
    /// The name (owner data).
    pub name: String,
}

impl<T: fmt::Debug> fmt::Debug for Named<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Named")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The rule that staged a row's suggestion (U-168).
#[derive(Clone, PartialEq, Eq)]
pub struct SuggestedRule {
    /// The rule's id (conditional or legacy).
    pub id: String,
    /// `conditional` or `legacy`.
    pub kind: String,
    /// The rule's name (owner data).
    pub name: String,
    /// The rule may accept without review (Q-P26n).
    pub auto_accept: bool,
    /// The rule's change token (`ruleUpdatedAt` for "Accept all").
    pub updated_at: Option<String>,
}

impl fmt::Debug for SuggestedRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuggestedRule")
            .field("kind", &self.kind)
            .field("auto_accept", &self.auto_accept)
            .finish_non_exhaustive()
    }
}

/// The row a possible duplicate looks like (U-174).
#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateOf {
    /// The other row.
    pub id: FeedItemId,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// Its amount (feed sign), when exact.
    pub amount: Option<Money>,
    /// Its description (owner data).
    pub description: Option<String>,
    /// Its state.
    pub status: Option<FeedStatus>,
}

impl fmt::Debug for DuplicateOf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateOf")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// The one transfer counterpart of a waiting row (`transferCandidate`,
/// Q-P26y): a bank line on another account within 3 days.
#[derive(Clone, PartialEq, Eq)]
pub struct TransferHint {
    /// The other bank line.
    pub feed_item: FeedItemId,
    /// Its account.
    pub account: Option<AccountId>,
    /// The account's name.
    pub account_name: Option<String>,
}

impl fmt::Debug for TransferHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferHint")
            .field("feed_item", &self.feed_item)
            .finish_non_exhaustive()
    }
}

/// One bank feed row. `Debug` prints ids, the state and the bucket only.
#[derive(Clone, PartialEq, Eq)]
pub struct FeedRow {
    /// The feed item.
    pub id: FeedItemId,
    /// Its bank connection.
    pub connection: Option<BankConnectionId>,
    /// The bank account's name in the books.
    pub bank_account_name: Option<String>,
    /// The bank's date.
    pub date: CalendarDate,
    /// The amount in the feed sign (positive = money out), exactly.
    pub amount: Money,
    /// The bank's (cleaned) description.
    pub description: String,
    /// The bank's raw description.
    pub original_description: Option<String>,
    /// The bank's memo.
    pub memo: Option<String>,
    /// The check number.
    pub check_number: Option<String>,
    /// The state.
    pub status: FeedStatus,
    /// The review bucket of a waiting row.
    pub bucket: Option<FeedBucket>,
    /// The suggested account.
    pub suggested_account: Option<Named<AccountId>>,
    /// The suggested payee.
    pub suggested_contact: Option<Named<ContactId>>,
    /// The suggested tag.
    pub suggested_tag: Option<Named<TagId>>,
    /// The suggestion's confidence, 0 to 1, exactly as sent.
    pub confidence: Option<Decimal>,
    /// Where the suggestion came from (`rule`, `history`, `ai`, …).
    pub match_type: Option<String>,
    /// The staged account of an assigned row.
    pub assigned_account: Option<Named<AccountId>>,
    /// The staged payee.
    pub assigned_contact: Option<Named<ContactId>>,
    /// The staged tag.
    pub assigned_tag: Option<Named<TagId>>,
    /// The staged memo.
    pub assigned_memo: Option<String>,
    /// The rule that staged the suggestion.
    pub suggested_rule: Option<SuggestedRule>,
    /// Flagged as a possible duplicate on import.
    pub possible_duplicate: bool,
    /// The row it looks like.
    pub duplicate_of: Option<DuplicateOf>,
    /// The entry it posted or matched.
    pub matched_transaction: Option<TransactionId>,
    /// The bank line of that entry (U-166).
    pub matched_line: Option<JournalLineId>,
    /// The linked entry was voided (Q-P26u).
    pub entry_voided: bool,
    /// When it was voided (engine timestamp text).
    pub entry_voided_at: Option<String>,
    /// Why an excluded row was excluded (`duplicate`, …).
    pub exclusion_reason: Option<String>,
    /// Journal lines it may match.
    pub match_candidate_count: u32,
    /// The one transfer counterpart, when exactly one exists (Q-P26y).
    pub transfer_candidate: Option<TransferHint>,
    /// The row's change token.
    pub updated_at: Option<String>,
}

impl fmt::Debug for FeedRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedRow")
            .field("id", &self.id)
            .field("date", &self.date)
            .field("status", &self.status)
            .field("bucket", &self.bucket)
            .finish_non_exhaustive()
    }
}

impl FeedRow {
    /// The amount as the register shows it (money in positive): −feed.
    #[must_use]
    pub fn display_amount(&self) -> Money {
        self.amount.checked_neg().unwrap_or(self.amount)
    }

    /// Money out of the bank account.
    #[must_use]
    pub fn is_money_out(&self) -> bool {
        self.amount.is_positive()
    }
}

/// One page of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedPage {
    /// The rows, newest first.
    pub rows: Vec<FeedRow>,
    /// Rows the query matches in all.
    pub total: u64,
    /// Rows left out because an amount wasn't an exact string.
    pub skipped: u32,
}

/// Counts per review bucket and state (`GET /banking/feed/counts`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedCounts {
    /// Waiting (ready + review + matches).
    pub waiting: u64,
    /// Ready to accept.
    pub ready: u64,
    /// Needs review.
    pub review: u64,
    /// With a match signal.
    pub matches: u64,
    /// Possible duplicates waiting for a decision.
    pub possible_duplicate: u64,
    /// In the books (entry-voided rows included).
    pub accepted: u64,
    /// Accepted rows whose entry was voided.
    pub entry_voided: u64,
    /// Excluded.
    pub excluded: u64,
    /// Being posted.
    pub in_progress: u64,
}

/// A journal line a feed item may match (U-166).
#[derive(Clone, PartialEq, Eq)]
pub struct MatchCandidate {
    /// The entry.
    pub transaction: TransactionId,
    /// Its line on the bank account.
    pub line: JournalLineId,
    /// The entry's kind (`check`, `bill_payment`, `deposit`, …).
    pub txn_type: Option<String>,
    /// Its number.
    pub txn_number: Option<String>,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// The line's amount in the feed sign, exactly.
    pub amount: Money,
    /// Its memo (owner data).
    pub memo: Option<String>,
    /// The payee (owner data).
    pub contact_name: Option<String>,
    /// The check number.
    pub check_number: Option<String>,
}

impl fmt::Debug for MatchCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MatchCandidate")
            .field("transaction", &self.transaction)
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

/// A payroll bank line a money-out item may already be (U-172).
#[derive(Clone, PartialEq, Eq)]
pub struct PayrollOverlap {
    /// The payroll entry.
    pub transaction: TransactionId,
    /// Its bank line.
    pub line: JournalLineId,
    /// Its memo (owner data).
    pub memo: Option<String>,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// The line's amount in the feed sign.
    pub amount: Money,
}

impl fmt::Debug for PayrollOverlap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayrollOverlap")
            .field("transaction", &self.transaction)
            .field("line", &self.line)
            .finish_non_exhaustive()
    }
}

/// A bank line on another account that may be a transfer's other side.
#[derive(Clone, PartialEq, Eq)]
pub struct TransferCandidate {
    /// The other feed item.
    pub feed_item: FeedItemId,
    /// Its account.
    pub account: AccountId,
    /// The account's name.
    pub account_name: String,
    /// Its date.
    pub date: CalendarDate,
    /// Its amount in the feed sign (the opposite of this row's).
    pub amount: Money,
}

impl fmt::Debug for TransferCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransferCandidate")
            .field("feed_item", &self.feed_item)
            .field("date", &self.date)
            .finish_non_exhaustive()
    }
}

/// One line of the owner's split (2 to 50; positive, at most 2 decimals).
#[derive(Clone, PartialEq, Eq)]
pub struct FeedSplitLine {
    /// The account.
    pub account: AccountId,
    /// The amount (more than 0), exactly.
    pub amount: Money,
    /// A tag (`None` sends nothing).
    pub tag: Option<TagId>,
    /// A memo.
    pub memo: Option<String>,
}

impl fmt::Debug for FeedSplitLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedSplitLine")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// The owner's choice for one row: categorize posts it, assign stages it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FeedChoice {
    /// The account (with a split: the first line's account).
    pub account: Option<AccountId>,
    /// The payee.
    pub contact: Option<ContactId>,
    /// A tag (`None` sends nothing; the engine falls back to the suggested
    /// tag).
    pub tag: Option<TagId>,
    /// A memo.
    pub memo: Option<String>,
    /// The owner's split (empty: one line on `account`).
    pub splits: Vec<FeedSplitLine>,
}

impl fmt::Debug for FeedChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedChoice")
            .field("account", &self.account)
            .field("splits", &self.splits.len())
            .finish_non_exhaustive()
    }
}

/// Keep both or remove this one (a possible duplicate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DuplicateDecision {
    /// Clears the flag; the row waits as usual.
    Keep,
    /// Excludes the row with reason `duplicate`.
    Remove,
}

/// A transfer's other side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferTarget {
    /// A waiting bank line on another account.
    OtherItem(FeedItemId),
    /// An account in the books with no bank line yet.
    Account(AccountId),
}

/// A processor fee taken out of a deposit (Q-P26t).
#[derive(Clone, PartialEq, Eq)]
pub struct FeeLine {
    /// The fee account.
    pub account: AccountId,
    /// The fee (more than 0, at most 2 decimals).
    pub amount: Money,
    /// A memo (at most 500 characters).
    pub memo: Option<String>,
}

impl fmt::Debug for FeeLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeeLine")
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A transfer or deposit posted from the feed.
#[derive(Clone, PartialEq, Eq)]
pub struct FeedPosted {
    /// The entry posted.
    pub transaction: Option<TransactionId>,
    /// Each matched feed item and its line.
    pub items: Vec<(FeedItemId, JournalLineId)>,
    /// The payments deposited and their amounts.
    pub payments: Vec<(TransactionId, Money)>,
    /// Undo takes it back as one group (`feed-undo-v1`).
    pub undo_group: Option<UndoGroupId>,
}

impl fmt::Debug for FeedPosted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FeedPosted")
            .field("transaction", &self.transaction)
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

/// A linked bank account (`GET /banking/connections`).
#[derive(Clone, PartialEq, Eq)]
pub struct BankConnection {
    /// The connection.
    pub id: BankConnectionId,
    /// Its account in the books.
    pub account: Option<AccountId>,
    /// The account's name.
    pub account_name: Option<String>,
}

impl fmt::Debug for BankConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BankConnection")
            .field("id", &self.id)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// A bank or card account's books balance next to its latest bank file
/// (U-77). Register sign: a card's positive balance is owed.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountBalance {
    /// The account.
    pub account: AccountId,
    /// Its name.
    pub name: String,
    /// Its number.
    pub number: Option<String>,
    /// A card account (else a bank or cash account).
    pub is_card: bool,
    /// The books balance.
    pub books: Money,
    /// The latest bank file's ending balance.
    pub bank_file: Option<Money>,
    /// That file's date.
    pub bank_file_date: Option<CalendarDate>,
    /// The books on that date.
    pub books_at_file_date: Option<Money>,
}

impl fmt::Debug for AccountBalance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountBalance")
            .field("account", &self.account)
            .field("is_card", &self.is_card)
            .finish_non_exhaustive()
    }
}

/// Why the engine refused a feed action (for the screen's words).
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FeedRefusal {
    /// 409 `FEED_ITEM_NOT_PENDING`: the row moved on (another computer).
    NotPending {
        /// Its state now.
        status: Option<FeedStatus>,
    },
    /// 409 `FEED_ITEM_IN_USE`: another request holds the row.
    InUse,
    /// 400 `SPLITS_DONT_BALANCE`.
    SplitsDontBalance {
        /// Amount − split total.
        difference: Option<Money>,
    },
    /// 400 `AMOUNT_NOT_CENTS`: an amount with more than 2 decimals can't be
    /// split.
    AmountNotCents,
    /// 400 `LINE_REQUIRED`: more than one line of the entry fits.
    LineRequired {
        /// The lines that fit.
        lines: Vec<JournalLineId>,
    },
    /// 400 `MATCH_MISMATCH`.
    MatchMismatch {
        /// `transactionId`, `journalLineId`, `account` or `amount`.
        field: Option<String>,
    },
    /// 409 `LINE_ALREADY_MATCHED`: another row holds the line.
    LineAlreadyMatched,
    /// 409 `NOT_A_POSSIBLE_DUPLICATE`.
    NotAPossibleDuplicate,
    /// 409 `ENTRY_NOT_VOIDED`: Review again on a row that isn't voided.
    EntryNotVoided,
    /// 400 `TRANSFER_MISMATCH`.
    TransferMismatch {
        /// The field that doesn't fit.
        field: Option<String>,
    },
    /// 400 `FEED_ITEM_MISMATCH`: payments − fees ≠ bank amount, or not money in.
    ItemMismatch {
        /// Bank amount − (payments − fees).
        difference: Option<Money>,
        /// `not_money_in`, when that is why.
        reason: Option<String>,
    },
    /// 400 `FEE_ACCOUNT_INVALID`.
    FeeAccountInvalid {
        /// The account.
        account: Option<AccountId>,
    },
    /// 400 `PAYMENT_NOT_PENDING_DEPOSIT`.
    PaymentNotPendingDeposit,
    /// 409 `PAYMENT_BOUNCED`.
    PaymentBounced,
    /// 409 `RULE_CHANGED`: the rule changed since the list was read.
    RuleChanged {
        /// Its change token now.
        updated_at: Option<String>,
    },
    /// 409 `RULE_NOT_AUTO_ACCEPT`: the rule doesn't allow Accept all.
    RuleNotAutoAccept,
    /// 404 `RULE_NOT_FOUND`: the rule was deleted.
    RuleNotFound,
    /// 409 `FEED_UNDO_CHANGED`: these rows' entries were edited since they
    /// were accepted (nothing undone; ask, then send with `force`).
    UndoChanged {
        /// The rows.
        ids: Vec<FeedItemId>,
    },
}

impl fmt::Debug for FeedRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            FeedRefusal::NotPending { status } => {
                return write!(f, "NotPending({status:?})");
            }
            FeedRefusal::InUse => "InUse",
            FeedRefusal::SplitsDontBalance { .. } => "SplitsDontBalance(..)",
            FeedRefusal::AmountNotCents => "AmountNotCents",
            FeedRefusal::LineRequired { lines } => {
                return write!(f, "LineRequired({})", lines.len());
            }
            FeedRefusal::MatchMismatch { field } => {
                return write!(f, "MatchMismatch({field:?})");
            }
            FeedRefusal::LineAlreadyMatched => "LineAlreadyMatched",
            FeedRefusal::NotAPossibleDuplicate => "NotAPossibleDuplicate",
            FeedRefusal::EntryNotVoided => "EntryNotVoided",
            FeedRefusal::TransferMismatch { field } => {
                return write!(f, "TransferMismatch({field:?})");
            }
            FeedRefusal::ItemMismatch { .. } => "ItemMismatch(..)",
            FeedRefusal::FeeAccountInvalid { .. } => "FeeAccountInvalid(..)",
            FeedRefusal::PaymentNotPendingDeposit => "PaymentNotPendingDeposit",
            FeedRefusal::PaymentBounced => "PaymentBounced",
            FeedRefusal::RuleChanged { .. } => "RuleChanged(..)",
            FeedRefusal::RuleNotAutoAccept => "RuleNotAutoAccept",
            FeedRefusal::RuleNotFound => "RuleNotFound",
            FeedRefusal::UndoChanged { ids } => {
                return write!(f, "UndoChanged({})", ids.len());
            }
        };
        f.write_str(name)
    }
}

/// Classifies a feed refusal (`None` for anything else).
#[must_use]
pub fn feed_refusal(e: &ApiError) -> Option<FeedRefusal> {
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        ApiError::Validation(v) => Some(v.details()),
        _ => None,
    };
    let text = |k: &str| details.and_then(|d| d.text(k)).map(str::to_string);
    let money = |k: &str| details.and_then(|d| d.money(k));
    Some(match error_code(e)? {
        "FEED_ITEM_NOT_PENDING" => FeedRefusal::NotPending {
            status: text("status").map(|s| FeedStatus::parse(&s)),
        },
        "FEED_ITEM_IN_USE" => FeedRefusal::InUse,
        "SPLITS_DONT_BALANCE" => FeedRefusal::SplitsDontBalance {
            difference: money("difference"),
        },
        "AMOUNT_NOT_CENTS" => FeedRefusal::AmountNotCents,
        "LINE_REQUIRED" => FeedRefusal::LineRequired {
            lines: details.map(|d| d.ids("journalLineIds")).unwrap_or_default(),
        },
        "MATCH_MISMATCH" => FeedRefusal::MatchMismatch {
            field: text("field"),
        },
        "LINE_ALREADY_MATCHED" => FeedRefusal::LineAlreadyMatched,
        "NOT_A_POSSIBLE_DUPLICATE" => FeedRefusal::NotAPossibleDuplicate,
        "ENTRY_NOT_VOIDED" => FeedRefusal::EntryNotVoided,
        "TRANSFER_MISMATCH" => FeedRefusal::TransferMismatch {
            field: text("field"),
        },
        "FEED_ITEM_MISMATCH" => FeedRefusal::ItemMismatch {
            difference: money("difference"),
            reason: text("reason"),
        },
        "FEE_ACCOUNT_INVALID" => FeedRefusal::FeeAccountInvalid {
            account: details.and_then(|d| d.id("accountId")),
        },
        "PAYMENT_NOT_PENDING_DEPOSIT" => FeedRefusal::PaymentNotPendingDeposit,
        "PAYMENT_BOUNCED" => FeedRefusal::PaymentBounced,
        "RULE_CHANGED" => FeedRefusal::RuleChanged {
            updated_at: text("updatedAt"),
        },
        "RULE_NOT_AUTO_ACCEPT" => FeedRefusal::RuleNotAutoAccept,
        "RULE_NOT_FOUND" => FeedRefusal::RuleNotFound,
        "FEED_UNDO_CHANGED" => FeedRefusal::UndoChanged {
            ids: details.map(|d| d.ids("ids")).unwrap_or_default(),
        },
        _ => return None,
    })
}

/// A 403 on a feed write that isn't about the closing-date password: the
/// user may only look (`readonly` role, practice routes).
#[must_use]
pub fn is_view_only(e: &ApiError) -> bool {
    matches!(e, ApiError::Forbidden { .. })
}

/// A row that changed under the owner (another computer): the screen
/// refreshes it and drops it from the selection (plan §5 LAN).
#[must_use]
pub fn is_stale_row(e: &ApiError) -> bool {
    matches!(
        feed_refusal(e),
        Some(
            FeedRefusal::NotPending { .. }
                | FeedRefusal::InUse
                | FeedRefusal::LineAlreadyMatched
                | FeedRefusal::NotAPossibleDuplicate
                | FeedRefusal::EntryNotVoided
        )
    ) || matches!(e, ApiError::NotFound { .. })
}

// ---------------------------------------------------------------- wire

/// An exact amount string; `None` for a number or anything else.
fn exact(v: Option<&RawValue>) -> Option<Money> {
    let t = v?.get();
    let s = t.strip_prefix('"')?.strip_suffix('"')?;
    Money::parse_wire(s).ok()
}

fn named<T: FromStr>(id: Option<&str>, name: Option<&str>) -> Option<Named<T>> {
    Some(Named {
        id: id?.parse().ok()?,
        name: name.unwrap_or_default().to_string(),
    })
}

fn opt_date(s: Option<&str>) -> Option<CalendarDate> {
    let s = s?;
    let head = s.get(..10).unwrap_or(s);
    CalendarDate::parse_wire(head).ok()
}

/// A confidence score sent as a decimal string (a number is read through
/// its text, never as f64).
fn confidence(v: Option<&RawValue>) -> Option<Decimal> {
    let t = v?.get();
    let s = t
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(t);
    Decimal::from_str(s).ok()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRule {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    auto_accept: bool,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireDup {
    id: String,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    amount: Option<Box<RawValue>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireRow {
    id: String,
    #[serde(default)]
    bank_connection_id: Option<String>,
    #[serde(default)]
    bank_account_name: Option<String>,
    feed_date: String,
    amount: Box<RawValue>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    original_description: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    check_number: Option<Value>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    suggested_account_id: Option<String>,
    #[serde(default)]
    suggested_account_name: Option<String>,
    #[serde(default)]
    suggested_contact_id: Option<String>,
    #[serde(default)]
    suggested_contact_name: Option<String>,
    #[serde(default)]
    suggested_tag_id: Option<String>,
    #[serde(default)]
    suggested_tag_name: Option<String>,
    #[serde(default)]
    confidence_score: Option<Box<RawValue>>,
    #[serde(default)]
    match_type: Option<String>,
    #[serde(default)]
    assigned_account_id: Option<String>,
    #[serde(default)]
    assigned_account_name: Option<String>,
    #[serde(default)]
    assigned_contact_id: Option<String>,
    #[serde(default)]
    assigned_contact_name: Option<String>,
    #[serde(default)]
    assigned_tag_id: Option<String>,
    #[serde(default)]
    assigned_tag_name: Option<String>,
    #[serde(default)]
    assigned_memo: Option<String>,
    #[serde(default)]
    suggested_rule: Option<WireRule>,
    #[serde(default)]
    possible_duplicate: Option<bool>,
    #[serde(default)]
    duplicate_of: Option<WireDup>,
    #[serde(default)]
    matched_transaction_id: Option<String>,
    #[serde(default)]
    matched_journal_line_id: Option<String>,
    #[serde(default)]
    entry_voided: Option<bool>,
    #[serde(default)]
    entry_voided_at: Option<String>,
    #[serde(default)]
    exclusion_reason: Option<String>,
    #[serde(default)]
    match_candidate_count: Option<u64>,
    #[serde(default)]
    transfer_candidate: Option<WireTransferHint>,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTransferHint {
    feed_item_id: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    account_name: Option<String>,
}

impl WireRow {
    fn into_row(self) -> Result<Option<FeedRow>, ApiError> {
        const WHAT: &str = "bank feed row";
        let Some(amount) = exact(Some(&self.amount)) else {
            return Ok(None);
        };
        let date = opt_date(Some(&self.feed_date)).ok_or(ApiError::Decode { what: WHAT })?;
        let check_number = match self.check_number {
            Some(Value::String(s)) if !s.is_empty() => Some(s),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        };
        let duplicate_of = match self.duplicate_of {
            Some(d) => Some(DuplicateOf {
                id: FeedItemId::parse(&d.id)?,
                date: opt_date(d.date.as_deref()),
                amount: exact(d.amount.as_deref()),
                description: d.description,
                status: d.status.as_deref().map(FeedStatus::parse),
            }),
            None => None,
        };
        Ok(Some(FeedRow {
            id: FeedItemId::parse(&self.id)?,
            connection: self
                .bank_connection_id
                .as_deref()
                .and_then(|c| BankConnectionId::parse(c).ok()),
            bank_account_name: self.bank_account_name,
            date,
            amount,
            description: self.description.unwrap_or_default(),
            original_description: self.original_description,
            memo: self.memo,
            check_number,
            status: self
                .status
                .as_deref()
                .map_or(FeedStatus::Other, FeedStatus::parse),
            bucket: self.bucket.as_deref().and_then(FeedBucket::parse),
            suggested_account: named(
                self.suggested_account_id.as_deref(),
                self.suggested_account_name.as_deref(),
            ),
            suggested_contact: named(
                self.suggested_contact_id.as_deref(),
                self.suggested_contact_name.as_deref(),
            ),
            suggested_tag: named(
                self.suggested_tag_id.as_deref(),
                self.suggested_tag_name.as_deref(),
            ),
            confidence: confidence(self.confidence_score.as_deref()),
            match_type: self.match_type,
            assigned_account: named(
                self.assigned_account_id.as_deref(),
                self.assigned_account_name.as_deref(),
            ),
            assigned_contact: named(
                self.assigned_contact_id.as_deref(),
                self.assigned_contact_name.as_deref(),
            ),
            assigned_tag: named(
                self.assigned_tag_id.as_deref(),
                self.assigned_tag_name.as_deref(),
            ),
            assigned_memo: self.assigned_memo,
            suggested_rule: self.suggested_rule.map(|r| SuggestedRule {
                id: r.id,
                kind: r.kind.unwrap_or_default(),
                name: r.name.unwrap_or_default(),
                auto_accept: r.auto_accept,
                updated_at: r.updated_at,
            }),
            possible_duplicate: self.possible_duplicate.unwrap_or(false),
            duplicate_of,
            matched_transaction: self
                .matched_transaction_id
                .as_deref()
                .and_then(|t| t.parse().ok()),
            matched_line: self
                .matched_journal_line_id
                .as_deref()
                .and_then(|t| t.parse().ok()),
            entry_voided: self.entry_voided.unwrap_or(false),
            entry_voided_at: self.entry_voided_at,
            exclusion_reason: self.exclusion_reason,
            match_candidate_count: self
                .match_candidate_count
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0),
            transfer_candidate: match self.transfer_candidate {
                Some(t) => Some(TransferHint {
                    feed_item: FeedItemId::parse(&t.feed_item_id)?,
                    account: t.account_id.as_deref().and_then(|a| a.parse().ok()),
                    account_name: t.account_name,
                }),
                None => None,
            },
            updated_at: self.updated_at,
        }))
    }
}

#[derive(Deserialize)]
struct WirePage {
    data: Vec<WireRow>,
    #[serde(default)]
    total: Option<u64>,
}

/// Reads a `GET /banking/feed` page.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn feed_page_from(body: &[u8]) -> Result<FeedPage, ApiError> {
    const WHAT: &str = "bank feed";
    let w: WirePage = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut rows = Vec::with_capacity(w.data.len());
    let mut skipped = 0u32;
    for r in w.data {
        match r.into_row()? {
            Some(row) => rows.push(row),
            None => skipped = skipped.saturating_add(1),
        }
    }
    let total = w.total.unwrap_or(rows.len() as u64);
    Ok(FeedPage {
        rows,
        total,
        skipped,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCounts {
    #[serde(default)]
    waiting: u64,
    #[serde(default)]
    ready: u64,
    #[serde(default)]
    review: u64,
    #[serde(default)]
    matches: u64,
    #[serde(default)]
    possible_duplicate: u64,
    #[serde(default)]
    accepted: u64,
    #[serde(default)]
    entry_voided: u64,
    #[serde(default)]
    excluded: u64,
    #[serde(default)]
    in_progress: u64,
}

/// Reads `GET /banking/feed/counts`.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn feed_counts_from(body: &[u8]) -> Result<FeedCounts, ApiError> {
    let w: WireCounts = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "bank feed counts",
    })?;
    Ok(FeedCounts {
        waiting: w.waiting,
        ready: w.ready,
        review: w.review,
        matches: w.matches,
        possible_duplicate: w.possible_duplicate,
        accepted: w.accepted,
        entry_voided: w.entry_voided,
        excluded: w.excluded,
        in_progress: w.in_progress,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCandidate {
    id: String,
    journal_line_id: String,
    #[serde(default)]
    txn_type: Option<String>,
    #[serde(default)]
    txn_number: Option<Value>,
    #[serde(default)]
    txn_date: Option<String>,
    amount: Box<RawValue>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    contact_name: Option<String>,
    #[serde(default)]
    check_number: Option<Value>,
}

fn text_of(v: Option<Value>) -> Option<String> {
    match v? {
        Value::String(s) if !s.is_empty() => Some(s),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Deserialize)]
struct WireCandidates {
    candidates: Vec<WireCandidate>,
}

/// Reads `match-candidates` (a row with an inexact amount is left out).
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn match_candidates_from(body: &[u8]) -> Result<Vec<MatchCandidate>, ApiError> {
    const WHAT: &str = "match candidates";
    let w: WireCandidates =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut out = Vec::new();
    for c in w.candidates {
        let Some(amount) = exact(Some(&c.amount)) else {
            continue;
        };
        out.push(MatchCandidate {
            transaction: c.id.parse().map_err(|_| ApiError::Decode { what: WHAT })?,
            line: c
                .journal_line_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            txn_type: c.txn_type,
            txn_number: text_of(c.txn_number),
            date: opt_date(c.txn_date.as_deref()),
            amount,
            memo: c.memo,
            contact_name: c.contact_name,
            check_number: text_of(c.check_number),
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireOverlap {
    txn_id: String,
    journal_line_id: String,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    date: Option<String>,
    amount: Box<RawValue>,
}

#[derive(Deserialize)]
struct WireOverlaps {
    overlaps: Vec<WireOverlap>,
}

/// Reads `payroll-overlap`.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn payroll_overlaps_from(body: &[u8]) -> Result<Vec<PayrollOverlap>, ApiError> {
    const WHAT: &str = "payroll overlap";
    let w: WireOverlaps =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut out = Vec::new();
    for o in w.overlaps {
        let Some(amount) = exact(Some(&o.amount)) else {
            continue;
        };
        out.push(PayrollOverlap {
            transaction: o
                .txn_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            line: o
                .journal_line_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            memo: o.memo,
            date: opt_date(o.date.as_deref()),
            amount,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTransferCandidate {
    feed_item_id: String,
    account_id: String,
    #[serde(default)]
    account_name: Option<String>,
    date: String,
    amount: Box<RawValue>,
}

#[derive(Deserialize)]
struct WireTransferCandidates {
    candidates: Vec<WireTransferCandidate>,
}

/// Reads `transfer-candidates`.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn transfer_candidates_from(body: &[u8]) -> Result<Vec<TransferCandidate>, ApiError> {
    const WHAT: &str = "transfer candidates";
    let w: WireTransferCandidates =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut out = Vec::new();
    for c in w.candidates {
        let Some(amount) = exact(Some(&c.amount)) else {
            continue;
        };
        out.push(TransferCandidate {
            feed_item: FeedItemId::parse(&c.feed_item_id)?,
            account: c
                .account_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            account_name: c.account_name.unwrap_or_default(),
            date: opt_date(Some(&c.date)).ok_or(ApiError::Decode { what: WHAT })?,
            amount,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePostedItem {
    feed_item_id: String,
    journal_line_id: String,
}

#[derive(Deserialize)]
struct WirePayment {
    id: String,
    amount: Box<RawValue>,
}

#[derive(Deserialize)]
struct WirePosted {
    #[serde(default)]
    transaction: Option<Value>,
    #[serde(default)]
    items: Vec<WirePostedItem>,
    #[serde(default)]
    payments: Vec<WirePayment>,
}

/// Reads a transfer or deposit answer.
///
/// # Errors
/// `Decode` for an unreadable answer or a payment amount that isn't exact.
pub fn posted_from(body: &[u8]) -> Result<FeedPosted, ApiError> {
    const WHAT: &str = "feed posting";
    let w: WirePosted =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let transaction = w
        .transaction
        .as_ref()
        .and_then(|t| t.get("id"))
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok());
    let mut items = Vec::with_capacity(w.items.len());
    for i in w.items {
        items.push((
            FeedItemId::parse(&i.feed_item_id)?,
            i.journal_line_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
        ));
    }
    let mut payments = Vec::with_capacity(w.payments.len());
    for p in w.payments {
        payments.push((
            p.id.parse().map_err(|_| ApiError::Decode { what: WHAT })?,
            exact(Some(&p.amount)).ok_or(ApiError::Decode { what: WHAT })?,
        ));
    }
    Ok(FeedPosted {
        transaction,
        items,
        payments,
        undo_group: undo_group_of(body),
    })
}

/// The transaction id of a `{transaction}` answer (categorize, approve).
fn transaction_of(body: &[u8]) -> Result<Option<TransactionId>, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "feed posting",
    })?;
    Ok(v.pointer("/transaction/id")
        .and_then(Value::as_str)
        .and_then(|s| s.parse().ok()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireConnection {
    id: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    account_name: Option<String>,
}

#[derive(Deserialize)]
struct WireConnections {
    connections: Vec<WireConnection>,
}

/// Reads `GET /banking/connections`.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn connections_from(body: &[u8]) -> Result<Vec<BankConnection>, ApiError> {
    let w: WireConnections = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "bank connections",
    })?;
    let mut out = Vec::with_capacity(w.connections.len());
    for c in w.connections {
        out.push(BankConnection {
            id: BankConnectionId::parse(&c.id)?,
            account: c.account_id.as_deref().and_then(|a| a.parse().ok()),
            account_name: c.account_name,
        });
    }
    Ok(out)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireBalance {
    account_id: String,
    name: String,
    #[serde(default)]
    account_number: Option<String>,
    kind: String,
    books_balance: Box<RawValue>,
    #[serde(default)]
    bank_file_balance: Option<Box<RawValue>>,
    #[serde(default)]
    bank_file_date: Option<String>,
    #[serde(default)]
    books_balance_at_file_date: Option<Box<RawValue>>,
}

/// Reads `GET /dashboard/account-balances` (an inexact balance is an error:
/// balances are engine values, never guessed).
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn account_balances_from(body: &[u8]) -> Result<Vec<AccountBalance>, ApiError> {
    const WHAT: &str = "account balances";
    let w: Vec<WireBalance> =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let opt = |v: Option<&RawValue>| -> Result<Option<Money>, ApiError> {
        match v {
            Some(r) if r.get() != "null" => exact(Some(r))
                .map(Some)
                .ok_or(ApiError::Decode { what: WHAT }),
            _ => Ok(None),
        }
    };
    let mut out = Vec::with_capacity(w.len());
    for b in w {
        out.push(AccountBalance {
            account: b
                .account_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            name: b.name,
            number: b.account_number,
            is_card: b.kind == "card",
            books: exact(Some(&b.books_balance)).ok_or(ApiError::Decode { what: WHAT })?,
            bank_file: opt(b.bank_file_balance.as_deref())?,
            bank_file_date: opt_date(b.bank_file_date.as_deref()),
            books_at_file_date: opt(b.books_balance_at_file_date.as_deref())?,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------- bodies

/// A positive amount with at most 2 decimals, as the engine reads it.
fn cents(m: Money) -> Result<String, ApiError> {
    if !m.is_positive() {
        return Err(ApiError::Local(LocalValidation::NonPositiveAmount));
    }
    wire_decimal(m.as_decimal()).map_err(ApiError::Local)
}

/// The JSON of the owner's choice (categorize and assign).
///
/// # Errors
/// `Local(InvalidValue)` without an account; `Local(TooFewLines)` for one
/// split line; `Local(TooManyLines)` above 50; a split amount that isn't
/// positive or has a third decimal.
pub fn choice_body(choice: &FeedChoice) -> Result<Value, ApiError> {
    let account = choice
        .account
        .or_else(|| choice.splits.first().map(|l| l.account))
        .ok_or(ApiError::Local(LocalValidation::InvalidValue))?;
    let mut m = Map::new();
    m.insert("accountId".into(), json!(account.to_string()));
    if let Some(c) = choice.contact {
        m.insert("contactId".into(), json!(c.to_string()));
    }
    if let Some(t) = choice.tag {
        m.insert("tagId".into(), json!(t.to_string()));
    }
    if let Some(memo) = &choice.memo {
        m.insert("memo".into(), json!(memo));
    }
    if !choice.splits.is_empty() {
        if choice.splits.len() < 2 {
            return Err(ApiError::Local(LocalValidation::TooFewLines));
        }
        if choice.splits.len() > SPLIT_LINES_MAX {
            return Err(ApiError::Local(LocalValidation::TooManyLines));
        }
        let mut lines = Vec::with_capacity(choice.splits.len());
        for l in &choice.splits {
            let mut line = Map::new();
            line.insert("accountId".into(), json!(l.account.to_string()));
            line.insert("amount".into(), json!(cents(l.amount)?));
            if let Some(t) = l.tag {
                line.insert("tagId".into(), json!(t.to_string()));
            }
            if let Some(memo) = &l.memo {
                line.insert("memo".into(), json!(memo));
            }
            lines.push(Value::Object(line));
        }
        m.insert("splits".into(), Value::Array(lines));
    }
    Ok(Value::Object(m))
}

/// The JSON of a deposit.
///
/// # Errors
/// `Local(InvalidValue)` for no payments, more than 100, a repeat, or more
/// than 10 fee lines; a fee that isn't positive or has a third decimal.
pub fn deposit_body(payments: &[TransactionId], fees: &[FeeLine]) -> Result<Value, ApiError> {
    if payments.is_empty() || payments.len() > DEPOSIT_PAYMENTS_MAX || fees.len() > FEE_LINES_MAX {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let mut seen = std::collections::HashSet::new();
    if !payments.iter().all(|p| seen.insert(*p)) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    let mut m = Map::new();
    m.insert(
        "sourceTransactionIds".into(),
        Value::Array(payments.iter().map(|p| json!(p.to_string())).collect()),
    );
    if !fees.is_empty() {
        let mut lines = Vec::with_capacity(fees.len());
        for f in fees {
            let mut line = Map::new();
            line.insert("accountId".into(), json!(f.account.to_string()));
            line.insert("amount".into(), json!(cents(f.amount)?));
            if let Some(memo) = &f.memo {
                line.insert("memo".into(), json!(memo));
            }
            lines.push(Value::Object(line));
        }
        m.insert("feeLines".into(), Value::Array(lines));
    }
    Ok(Value::Object(m))
}

// ---------------------------------------------------------------- calls

fn item_call(op: &Operation, id: FeedItemId, tail: &str) -> Call {
    Call::new(
        op.method(),
        op.route(),
        format!("/api/v1/banking/feed/{id}/{tail}"),
    )
}

impl Feed<'_> {
    pub(crate) fn has(self, cap: &str) -> bool {
        self.0.client().capabilities().has(cap)
    }

    pub(crate) fn need(self, cap: &str) -> Result<(), ApiError> {
        if self.has(cap) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// The body with the closing-date password applied (feed routes take it
    /// under `feed-closing-date-v1`).
    pub(crate) fn with_password(
        self,
        mut body: Value,
        auth: Option<&ClosedPeriodAuthorization>,
    ) -> Result<Vec<u8>, ApiError> {
        if let Some(a) = auth {
            if !self.has(Capabilities::FEED_CLOSING_DATE) {
                return Err(ApiError::Local(
                    LocalValidation::ClosedPeriodOverrideUnavailable,
                ));
            }
            if let Value::Object(m) = &mut body {
                m.insert("closingPassword".into(), json!(a.expose()));
            }
        }
        json_body(&body)
    }

    /// One page of the review list.
    ///
    /// # Errors
    /// `Local(Unsupported)` for a bucket, duplicate or voided filter without
    /// `feed-review-v1` (nothing sent); `Local(PageLimit)`; any [`ApiError`].
    pub async fn list(&self, q: &FeedQuery) -> Result<FeedPage, ApiError> {
        if q.bucket.is_some() || q.possible_duplicate.is_some() || q.entry_voided.is_some() {
            self.need(Capabilities::FEED_REVIEW)?;
        }
        if q.accepted && q.status.is_none() {
            self.need(Capabilities::FEED_ACCEPT)?;
        }
        let mut call = Call::at(Method::Get, FEED_LIST.route());
        call.query = q.to_query()?;
        let reply = self.0.send(&FEED_LIST, call, None).await?;
        feed_page_from(&reply.body)
    }

    /// Counts per bucket and state, for one connection or all.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-review-v1`; any [`ApiError`].
    pub async fn counts(
        &self,
        connection: Option<BankConnectionId>,
    ) -> Result<FeedCounts, ApiError> {
        self.need(Capabilities::FEED_REVIEW)?;
        let mut call = Call::at(Method::Get, FEED_COUNTS.route());
        if let Some(c) = connection {
            call.query = vec![("bankConnectionId".into(), c.to_string())];
        }
        let reply = self.0.send(&FEED_COUNTS, call, None).await?;
        feed_counts_from(&reply.body)
    }

    /// The linked bank accounts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn connections(&self) -> Result<Vec<BankConnection>, ApiError> {
        let call = Call::at(Method::Get, BANK_CONNECTIONS.route());
        let reply = self.0.send(&BANK_CONNECTIONS, call, None).await?;
        connections_from(&reply.body)
    }

    /// The journal lines a row may match, one per line.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-v2`; any [`ApiError`]
    /// (`Forbidden` for a view-only user).
    pub async fn match_candidates(&self, id: FeedItemId) -> Result<Vec<MatchCandidate>, ApiError> {
        self.need(Capabilities::FEED_MATCH_V2)?;
        let call = item_call(&FEED_MATCH_CANDIDATES, id, "match-candidates");
        let reply = self.0.send(&FEED_MATCH_CANDIDATES, call, None).await?;
        match_candidates_from(&reply.body)
    }

    /// Payroll bank lines a money-out row may already be.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-v2`; any [`ApiError`].
    pub async fn payroll_overlap(&self, id: FeedItemId) -> Result<Vec<PayrollOverlap>, ApiError> {
        self.need(Capabilities::FEED_MATCH_V2)?;
        let call = item_call(&FEED_PAYROLL_OVERLAP, id, "payroll-overlap");
        let reply = self.0.send(&FEED_PAYROLL_OVERLAP, call, None).await?;
        payroll_overlaps_from(&reply.body)
    }

    /// Bank lines on another account within `window_days` (1-10) that may
    /// be this row's transfer partner.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-post-v1`;
    /// `Local(InvalidValue)` for a window outside 1-10; any [`ApiError`].
    pub async fn transfer_candidates(
        &self,
        id: FeedItemId,
        window_days: u8,
    ) -> Result<Vec<TransferCandidate>, ApiError> {
        self.need(Capabilities::FEED_MATCH_POST)?;
        if !(1..=10).contains(&window_days) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = item_call(&FEED_TRANSFER_CANDIDATES, id, "transfer-candidates");
        call.query = vec![("windowDays".into(), window_days.to_string())];
        let reply = self.0.send(&FEED_TRANSFER_CANDIDATES, call, None).await?;
        transfer_candidates_from(&reply.body)
    }

    /// Posts a row with the owner's choice (exact; an optional split).
    /// Returns the entry posted.
    ///
    /// # Errors
    /// `Local(Unsupported)` with a split and without `feed-exact-v1`; the
    /// body's local checks ([`choice_body`]); `ClosedPeriod` (423) and the
    /// closing-password errors; [`feed_refusal`] cases; any [`ApiError`].
    pub async fn categorize(
        &self,
        id: FeedItemId,
        choice: &FeedChoice,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<Option<TransactionId>, ApiError> {
        self.need(Capabilities::FEED_EXACT)?;
        let body = self.with_password(choice_body(choice)?, auth)?;
        let mut call = item_call(&FEED_CATEGORIZE, id, "categorize").json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_CATEGORIZE, call, None).await?;
        transaction_of(&reply.body)
    }

    /// Stages the owner's choice on a row without posting.
    ///
    /// # Errors
    /// As [`Feed::categorize`], without the closed period.
    pub async fn assign(&self, id: FeedItemId, choice: &FeedChoice) -> Result<(), ApiError> {
        self.need(Capabilities::FEED_EXACT)?;
        let body = json_body(&choice_body(choice)?)?;
        let call = item_call(&FEED_ASSIGN, id, "assign").json_body(body);
        self.0.send(&FEED_ASSIGN, call, None).await?;
        Ok(())
    }

    /// Posts what is staged on an assigned row.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-exact-v1`; as [`Feed::categorize`].
    pub async fn approve(
        &self,
        id: FeedItemId,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<Option<TransactionId>, ApiError> {
        self.need(Capabilities::FEED_EXACT)?;
        let body = self.with_password(json!({}), auth)?;
        let mut call = item_call(&FEED_APPROVE, id, "approve").json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_APPROVE, call, None).await?;
        transaction_of(&reply.body)
    }

    /// Matches a row to a line of a posted entry (`line` may be left out
    /// when only one line fits). Returns its accept group (`feed-undo-v1`).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-v2`; [`feed_refusal`] cases
    /// (`LineRequired`, `MatchMismatch`, `LineAlreadyMatched`,
    /// `NotPending`); any [`ApiError`].
    pub async fn match_line(
        &self,
        id: FeedItemId,
        transaction: TransactionId,
        line: Option<JournalLineId>,
        key: IdempotencyKey,
    ) -> Result<Option<UndoGroupId>, ApiError> {
        self.need(Capabilities::FEED_MATCH_V2)?;
        let mut body = json!({ "transactionId": transaction.to_string() });
        if let (Some(l), Value::Object(m)) = (line, &mut body) {
            m.insert("journalLineId".into(), json!(l.to_string()));
        }
        let mut call = item_call(&FEED_MATCH, id, "match").json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_MATCH, call, None).await?;
        Ok(undo_group_of(&reply.body))
    }

    /// Keep both or remove this one, on a possible duplicate.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-review-v1`; `NotAPossibleDuplicate`;
    /// any [`ApiError`].
    pub async fn decide_duplicate(
        &self,
        id: FeedItemId,
        decision: DuplicateDecision,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        self.need(Capabilities::FEED_REVIEW)?;
        let word = match decision {
            DuplicateDecision::Keep => "keep",
            DuplicateDecision::Remove => "remove",
        };
        let body = json_body(&json!({ "decision": word }))?;
        let mut call = item_call(&FEED_DUPLICATE, id, "duplicate").json_body(body);
        call.idempotency_key = Some(key);
        self.0.send(&FEED_DUPLICATE, call, None).await?;
        Ok(())
    }

    /// Puts a row whose entry was voided back to waiting (Q-P26u).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-v2`; `EntryNotVoided`; any
    /// [`ApiError`].
    pub async fn review_again(&self, id: FeedItemId, key: IdempotencyKey) -> Result<(), ApiError> {
        self.need(Capabilities::FEED_MATCH_V2)?;
        let mut call =
            item_call(&FEED_REVIEW_AGAIN, id, "review-again").json_body(json_body(&json!({}))?);
        call.idempotency_key = Some(key);
        self.0.send(&FEED_REVIEW_AGAIN, call, None).await?;
        Ok(())
    }

    /// Excludes a waiting row.
    ///
    /// # Errors
    /// `NotPending` (409 with `feed-idempotency-v1`); any [`ApiError`].
    pub async fn exclude(&self, id: FeedItemId) -> Result<(), ApiError> {
        let call = item_call(&FEED_EXCLUDE, id, "exclude");
        self.0.send(&FEED_EXCLUDE, call, None).await?;
        Ok(())
    }

    /// Restores an excluded row.
    ///
    /// # Errors
    /// `NotPending`; any [`ApiError`].
    pub async fn unexclude(&self, id: FeedItemId) -> Result<(), ApiError> {
        let call = item_call(&FEED_UNEXCLUDE, id, "unexclude");
        self.0.send(&FEED_UNEXCLUDE, call, None).await?;
        Ok(())
    }

    /// Posts one transfer for a row and its other side and matches both.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-post-v1`; `TransferMismatch`,
    /// `NotPending`, `InUse`; the closed period; any [`ApiError`].
    pub async fn transfer(
        &self,
        id: FeedItemId,
        target: TransferTarget,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<FeedPosted, ApiError> {
        self.need(Capabilities::FEED_MATCH_POST)?;
        let body = match target {
            TransferTarget::OtherItem(o) => json!({ "otherFeedItemId": o.to_string() }),
            TransferTarget::Account(a) => json!({ "toAccountId": a.to_string() }),
        };
        let body = self.with_password(body, auth)?;
        let mut call = item_call(&FEED_TRANSFER, id, "transfer").json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_TRANSFER, call, None).await?;
        posted_from(&reply.body)
    }

    /// Posts one deposit of waiting payments, less fee lines, for a
    /// money-in row and matches it. The engine reads the payments' live
    /// amounts and checks payments − fees = bank amount exactly.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-match-post-v1`; [`deposit_body`]'s
    /// checks; `ItemMismatch`, `FeeAccountInvalid`, `PaymentNotPendingDeposit`,
    /// `PaymentBounced`; the closed period; any [`ApiError`].
    pub async fn deposit(
        &self,
        id: FeedItemId,
        payments: &[TransactionId],
        fees: &[FeeLine],
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<FeedPosted, ApiError> {
        self.need(Capabilities::FEED_MATCH_POST)?;
        let body = self.with_password(deposit_body(payments, fees)?, auth)?;
        let mut call = item_call(&FEED_DEPOSIT, id, "deposit").json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_DEPOSIT, call, None).await?;
        posted_from(&reply.body)
    }

    /// Books and latest bank-file balance per bank and card account.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `account-balances-v1`; any [`ApiError`].
    pub async fn account_balances(&self) -> Result<Vec<AccountBalance>, ApiError> {
        self.need(Capabilities::ACCOUNT_BALANCES)?;
        let call = Call::at(Method::Get, DASHBOARD_ACCOUNT_BALANCES.route());
        let reply = self.0.send(&DASHBOARD_ACCOUNT_BALANCES, call, None).await?;
        account_balances_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "feed_review_tests.rs"]
mod tests;
