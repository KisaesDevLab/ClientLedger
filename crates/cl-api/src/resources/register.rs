//! The account register (`GET /api/v1/accounts/{id}/register` and
//! `/register/summary`, P12 plan WP1, §4).
//!
//! - The engine is the ledger: every balance here is a value the engine
//!   sent (NN7). Nothing is added up on this side.
//! - Money is exact or display-only. Requests always ask for `exact=1`;
//!   engines with `register-exact-v1` answer every amount as a decimal
//!   string, which becomes [`Money`] ([`RegisterAmount::Exact`]). An engine
//!   without it (the pinned engine) answers JSON numbers; each is kept as
//!   the number text exactly as sent ([`RegisterAmount::DisplayOnly`], the
//!   P05 [`ReportAmount`] pattern) and the page is marked `exact: false`.
//!   Display-only amounts are never turned into `Money`, compared, summed
//!   or used for an edit: edits always start from `GET /transactions/{id}`,
//!   so postings stay exact either way (P12 plan WP6, Q-P12b).
//! - Both dates are always sent: [`RegisterQuery`] cannot be built without
//!   them, so the engine's "last 90 days" default can never apply silently.
//! - `Debug` of a line or a page shows no amounts, names or memo text.

use std::fmt;

use cl_core::ids::{AccountId, ContactId, JournalLineId, TagId, TransactionId};
use cl_core::{CalendarDate, Money};
use rust_decimal::RoundingStrategy;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::accounts::AccountType;
use crate::resources::common::{Query, SortDir};
use crate::resources::operations::{REGISTER, REGISTER_SUMMARY};
use crate::resources::transactions::{TxnStatus, TxnType};
use crate::session::Session;
use crate::wire::{Capabilities, ReportAmount};

/// The most rows the engine serves in one register page.
pub const REGISTER_MAX_PER_PAGE: u16 = 200;

/// The cleared-state filter (`reconciled=`, P12 R3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClearedFilter {
    /// Lines cleared in an open reconciliation, not yet reconciled.
    Cleared,
    /// Lines not cleared anywhere.
    Uncleared,
    /// Lines in a finished reconciliation.
    Reconciled,
}

impl ClearedFilter {
    const fn as_str(self) -> &'static str {
        match self {
            ClearedFilter::Cleared => "cleared",
            ClearedFilter::Uncleared => "uncleared",
            ClearedFilter::Reconciled => "reconciled",
        }
    }
}

/// Sort keys of the register (`sort_by=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum RegisterSort {
    /// Date, then entry order (the default).
    #[default]
    Date,
    /// Amount.
    Amount,
    /// Transaction type.
    Type,
    /// Number (`ref_no`).
    Number,
}

impl RegisterSort {
    const fn as_str(self) -> &'static str {
        match self {
            RegisterSort::Date => "date",
            RegisterSort::Amount => "amount",
            RegisterSort::Type => "type",
            RegisterSort::Number => "ref_no",
        }
    }
}

/// What a register page shows. Both dates are required fields.
#[derive(Clone, PartialEq, Eq)]
pub struct RegisterQuery {
    /// First date shown (inclusive).
    pub start: CalendarDate,
    /// Last date shown (inclusive).
    pub end: CalendarDate,
    /// Only this kind of transaction.
    pub txn_type: Option<TxnType>,
    /// Payee name contains (owner data: never logged).
    pub payee: Option<String>,
    /// Number, payee or memo contains (owner data: never logged).
    pub search: Option<String>,
    /// Cleared state (needs `register-exact-v1`).
    pub cleared: Option<ClearedFilter>,
    /// Lines with this tag (needs `register-exact-v1`).
    pub tag: Option<TagId>,
    /// Smallest amount (not below zero).
    pub min: Option<Money>,
    /// Largest amount (not below zero).
    pub max: Option<Money>,
    /// Also list void transactions.
    pub include_void: bool,
    /// Sort key.
    pub sort: RegisterSort,
    /// Sort direction.
    pub dir: SortDir,
}

impl fmt::Debug for RegisterQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterQuery")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("txn_type", &self.txn_type)
            .field("payee", &self.payee.is_some())
            .field("search", &self.search.is_some())
            .field("cleared", &self.cleared)
            .field("tag", &self.tag.is_some())
            .field("amount_filter", &(self.min.is_some() || self.max.is_some()))
            .field("include_void", &self.include_void)
            .field("sort", &self.sort)
            .field("dir", &self.dir)
            .finish()
    }
}

impl RegisterQuery {
    /// All lines dated `start..=end`, oldest first, voids included.
    #[must_use]
    pub fn new(start: CalendarDate, end: CalendarDate) -> RegisterQuery {
        RegisterQuery {
            start,
            end,
            txn_type: None,
            payee: None,
            search: None,
            cleared: None,
            tag: None,
            min: None,
            max: None,
            include_void: true,
            sort: RegisterSort::Date,
            dir: SortDir::Asc,
        }
    }

    /// Whether any filter beyond the dates is on.
    #[must_use]
    pub fn filters_active(&self) -> bool {
        self.txn_type.is_some()
            || self.payee.as_deref().is_some_and(|p| !p.is_empty())
            || self.search.as_deref().is_some_and(|p| !p.is_empty())
            || self.cleared.is_some()
            || self.tag.is_some()
            || self.min.is_some()
            || self.max.is_some()
    }

    /// The query pairs for one page (`page` counts from 1).
    ///
    /// # Errors
    /// `BadRange` (start after end, or min above max), `InvalidValue` (a
    /// negative amount bound), `PageLimit` (page 0, or `per_page` outside
    /// 1..=200), `RegisterFilterUnavailable` (cleared or tag filter without
    /// `register-exact-v1`).
    pub fn to_query(
        &self,
        caps: &Capabilities,
        page: u32,
        per_page: u16,
    ) -> Result<Vec<(String, String)>, LocalValidation> {
        if self.start > self.end {
            return Err(LocalValidation::BadRange);
        }
        if self.min.is_some_and(Money::is_negative) || self.max.is_some_and(Money::is_negative) {
            return Err(LocalValidation::InvalidValue);
        }
        if let (Some(lo), Some(hi)) = (self.min, self.max)
            && lo > hi
        {
            return Err(LocalValidation::BadRange);
        }
        if page == 0 || per_page == 0 || per_page > REGISTER_MAX_PER_PAGE {
            return Err(LocalValidation::PageLimit);
        }
        if (self.cleared.is_some() || self.tag.is_some()) && !caps.has(Capabilities::REGISTER_EXACT)
        {
            return Err(LocalValidation::RegisterFilterUnavailable);
        }
        let mut q = Query::default();
        q.put("exact", "1");
        q.put("start_date", self.start.to_wire());
        q.put("end_date", self.end.to_wire());
        if let Some(t) = self.txn_type {
            q.put("txn_type", t.as_str());
        }
        q.opt_str("payee", self.payee.as_deref());
        q.opt_str("search", self.search.as_deref());
        if let Some(c) = self.cleared {
            q.put("reconciled", c.as_str());
        }
        if let Some(t) = self.tag {
            q.put("tag_id", t.to_string());
        }
        if let Some(m) = self.min {
            q.put("min_amount", m.to_wire());
        }
        if let Some(m) = self.max {
            q.put("max_amount", m.to_wire());
        }
        if self.include_void {
            q.put("include_void", "true");
        }
        q.put("sort_by", self.sort.as_str());
        q.put("sort_dir", self.dir.as_str());
        q.put("page", page.to_string());
        q.put("per_page", per_page.to_string());
        Ok(q.0)
    }
}

/// An inline entry kind the engine allows in this register
/// (`allowedEntryTypes`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EntryType {
    /// A payment out (check or card charge; engine `expense`).
    Expense,
    /// A deposit.
    Deposit,
    /// A transfer between two balance-sheet accounts.
    Transfer,
    /// A two-line journal entry.
    JournalEntry,
    /// A kind this version does not know. Never offered.
    Other(String),
}

impl EntryType {
    fn parse(s: &str) -> EntryType {
        match s {
            "expense" => EntryType::Expense,
            "deposit" => EntryType::Deposit,
            "transfer" => EntryType::Transfer,
            "journal_entry" => EntryType::JournalEntry,
            other => EntryType::Other(other.to_string()),
        }
    }
}

/// The other side of a register line.
#[derive(Clone, PartialEq, Eq)]
pub enum Category {
    /// One other account.
    Account {
        /// The account.
        id: AccountId,
        /// Its name (display only).
        name: Option<String>,
    },
    /// Several other accounts ("— Split —").
    Split,
    /// No other account (for example a one-sided line).
    None,
}

impl fmt::Debug for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Category::Account { id, .. } => write!(f, "Account({id})"),
            Category::Split => f.write_str("Split"),
            Category::None => f.write_str("None"),
        }
    }
}

/// Cleared state of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClearState {
    /// Not cleared.
    None,
    /// Cleared in an open reconciliation.
    Cleared,
    /// In a finished reconciliation.
    Reconciled,
}

/// A register amount as the engine sent it.
///
/// There is deliberately no `PartialEq` or arithmetic: an exact value is
/// reached through [`RegisterAmount::exact`], and a display-only value has
/// no path into `Money` at all. `Debug` prints no digits.
#[derive(Clone)]
pub enum RegisterAmount {
    /// A decimal string (`register-exact-v1`): exact.
    Exact(Money),
    /// A JSON number from an engine without `register-exact-v1`: the text
    /// as sent, for display only.
    DisplayOnly(ReportAmount),
}

impl RegisterAmount {
    /// The exact value, `None` for a display-only amount.
    #[must_use]
    pub fn exact(&self) -> Option<Money> {
        match self {
            RegisterAmount::Exact(m) => Some(*m),
            RegisterAmount::DisplayOnly(_) => None,
        }
    }

    /// True for an exact amount.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        matches!(self, RegisterAmount::Exact(_))
    }

    /// Plain display text at cents (`-1084.66`), half away from zero,
    /// never `-0.00`. `negated` flips the shown sign (the display flip of
    /// a credit-normal account's stored balance, Q-P12h); for either kind
    /// that is a change of the text's sign, not arithmetic on the value. `None` when the
    /// engine's number text is not readable as a decimal.
    #[must_use]
    pub fn display(&self, negated: bool) -> Option<String> {
        let plain = match self {
            RegisterAmount::Exact(m) => {
                let mut d = m
                    .as_decimal()
                    .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
                if d.is_zero() {
                    d.set_sign_positive(true);
                }
                format!("{d:.2}")
            }
            RegisterAmount::DisplayOnly(r) => r.display(2)?,
        };
        Some(if negated { flip_sign(&plain) } else { plain })
    }
}

/// Flips the sign of plain display text; zero stays unsigned.
fn flip_sign(plain: &str) -> String {
    if let Some(rest) = plain.strip_prefix('-') {
        rest.to_string()
    } else if plain.bytes().all(|b| matches!(b, b'0' | b'.')) {
        plain.to_string()
    } else {
        format!("-{plain}")
    }
}

impl fmt::Debug for RegisterAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            RegisterAmount::Exact(_) => "RegisterAmount::Exact(..)",
            RegisterAmount::DisplayOnly(_) => "RegisterAmount::DisplayOnly(..)",
        })
    }
}

/// One register line: one journal line of this account. Amounts are the
/// engine's values (exact or display-only); `Debug` prints none of them.
#[derive(Clone)]
pub struct RegisterLine {
    /// The journal line.
    pub line_id: JournalLineId,
    /// Its transaction.
    pub txn_id: TransactionId,
    /// Transaction kind.
    pub txn_type: TxnType,
    /// Transaction number (`txnNumber`).
    pub number: Option<String>,
    /// Check number.
    pub check_number: Option<u64>,
    /// Date.
    pub date: CalendarDate,
    /// Payee (owner data).
    pub payee: Option<String>,
    /// Payee contact.
    pub contact: Option<ContactId>,
    /// The other side.
    pub category: Category,
    /// Memo (owner data).
    pub memo: Option<String>,
    /// Money out of the account (the credit side for an asset).
    pub payment: Option<RegisterAmount>,
    /// Money into the account.
    pub deposit: Option<RegisterAmount>,
    /// The account's balance after this line, as the engine computed it
    /// (filter-independent with `register-exact-v1`).
    pub balance: RegisterAmount,
    /// Cleared state.
    pub cleared: ClearState,
    /// The transaction has more than two lines.
    pub has_splits: bool,
    /// The engine allows editing it (not reconciled, not void).
    pub engine_editable: bool,
    /// Status.
    pub status: TxnStatus,
}

impl fmt::Debug for RegisterLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterLine")
            .field("line_id", &self.line_id)
            .field("txn_id", &self.txn_id)
            .field("txn_type", &self.txn_type)
            .field("date", &self.date)
            .field("category", &self.category)
            .field("cleared", &self.cleared)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The register's account header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegisterAccount {
    /// Account.
    pub id: AccountId,
    /// Type.
    pub account_type: AccountType,
    /// Detail type (`bank`, `credit_card`, …).
    pub detail_type: Option<String>,
}

/// One page of the register.
#[derive(Clone)]
pub struct RegisterPage {
    /// The account.
    pub account: RegisterAccount,
    /// True when every amount on the page is exact; false when the engine
    /// sent JSON numbers (display only).
    pub exact: bool,
    /// Balance before `start` (engine value).
    pub balance_forward: RegisterAmount,
    /// Unfiltered balance at `end` (engine value; an engine without
    /// `register-exact-v1` does not send it).
    pub ending_balance: Option<RegisterAmount>,
    /// Page number (from 1).
    pub page: u32,
    /// Rows per page.
    pub per_page: u16,
    /// Rows matching the query in all pages.
    pub total_rows: u64,
    /// Pages in all.
    pub total_pages: u64,
    /// Entry kinds this register allows inline.
    pub allowed: Vec<EntryType>,
    /// The rows of this page.
    pub lines: Vec<RegisterLine>,
}

impl fmt::Debug for RegisterPage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterPage")
            .field("account", &self.account)
            .field("exact", &self.exact)
            .field("page", &self.page)
            .field("per_page", &self.per_page)
            .field("total_rows", &self.total_rows)
            .field("total_pages", &self.total_pages)
            .field("allowed", &self.allowed)
            .field("lines", &self.lines.len())
            .finish_non_exhaustive()
    }
}

/// The register footer (`/register/summary`).
#[derive(Clone)]
pub struct RegisterSummary {
    /// True when every amount is exact (see [`RegisterPage::exact`]).
    pub exact: bool,
    /// The account's stored balance.
    pub current_balance: RegisterAmount,
    /// Unfiltered balance of posted lines through `as_of` (P12 R4).
    pub ending_balance: Option<RegisterAmount>,
    /// The part of it cleared in any reconciliation (P12 R4).
    pub cleared_balance: Option<RegisterAmount>,
    /// The date the ending balance is at (`None`: all dates).
    pub as_of: Option<CalendarDate>,
    /// Posted lines not cleared.
    pub uncleared_count: u64,
    /// Statement date of the last finished reconciliation.
    pub last_reconciled: Option<CalendarDate>,
}

impl fmt::Debug for RegisterSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegisterSummary")
            .field("exact", &self.exact)
            .field("as_of", &self.as_of)
            .field("uncleared_count", &self.uncleared_count)
            .field("last_reconciled", &self.last_reconciled)
            .finish_non_exhaustive()
    }
}

// ------------------------------------------------------------------ wire

/// A money field as sent: a decimal string becomes exact `Money`; a JSON
/// number is kept as its text for display only (never parsed into money).
fn amount(raw: &RawValue, what: &'static str) -> Result<RegisterAmount, ApiError> {
    let text = raw.get();
    match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
        Some(s) => Money::parse_wire(s)
            .map(RegisterAmount::Exact)
            .map_err(|_| ApiError::Decode { what }),
        None if text == "null" => Err(ApiError::Decode { what }),
        None => serde_json::from_str::<ReportAmount>(text)
            .map(RegisterAmount::DisplayOnly)
            .map_err(|_| ApiError::Decode { what }),
    }
}

fn amount_opt(
    raw: Option<&RawValue>,
    what: &'static str,
) -> Result<Option<RegisterAmount>, ApiError> {
    match raw {
        None => Ok(None),
        Some(r) if r.get() == "null" => Ok(None),
        Some(r) => amount(r, what).map(Some),
    }
}

fn all_exact<'a>(amounts: impl IntoIterator<Item = Option<&'a RegisterAmount>>) -> bool {
    amounts.into_iter().flatten().all(RegisterAmount::is_exact)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAccount {
    id: AccountId,
    account_type: AccountType,
    #[serde(default)]
    detail_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePagination {
    page: u32,
    per_page: u16,
    total_rows: u64,
    total_pages: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireLine {
    line_id: JournalLineId,
    transaction_id: TransactionId,
    txn_type: TxnType,
    #[serde(default)]
    txn_number: Option<String>,
    #[serde(default)]
    check_number: Option<u64>,
    txn_date: CalendarDate,
    #[serde(default)]
    payee_name: Option<String>,
    #[serde(default)]
    contact_id: Option<ContactId>,
    #[serde(default)]
    category_name: Option<String>,
    #[serde(default)]
    category_account_id: Option<AccountId>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    payment: Option<Box<RawValue>>,
    #[serde(default)]
    deposit: Option<Box<RawValue>>,
    running_balance: Box<RawValue>,
    reconciliation_status: String,
    #[serde(default)]
    has_splits: bool,
    #[serde(default)]
    is_editable: bool,
    status: TxnStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePage {
    account: WireAccount,
    balance_forward: Box<RawValue>,
    #[serde(default)]
    ending_balance: Option<Box<RawValue>>,
    pagination: WirePagination,
    #[serde(default)]
    allowed_entry_types: Vec<String>,
    lines: Vec<WireLine>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSummary {
    current_balance: Box<RawValue>,
    #[serde(default)]
    ending_balance: Option<Box<RawValue>>,
    #[serde(default)]
    cleared_balance: Option<Box<RawValue>>,
    #[serde(default)]
    as_of_date: Option<CalendarDate>,
    #[serde(default)]
    uncleared_count: u64,
    #[serde(default)]
    last_reconciliation_date: Option<CalendarDate>,
}

/// The category the engine names "— Split —" when several other accounts
/// share the transaction.
const SPLIT_NAME: &str = "— Split —";

impl WireLine {
    fn into_line(self) -> Result<RegisterLine, ApiError> {
        let category = match (self.category_account_id, self.category_name) {
            (Some(id), name) => Category::Account { id, name },
            (None, Some(n)) if n == SPLIT_NAME => Category::Split,
            (None, _) if self.has_splits => Category::Split,
            (None, _) => Category::None,
        };
        let cleared = match self.reconciliation_status.as_str() {
            "cleared" => ClearState::Cleared,
            "reconciled" => ClearState::Reconciled,
            "uncleared" => ClearState::None,
            _ => {
                return Err(ApiError::Decode {
                    what: "register reconciliation status",
                });
            }
        };
        Ok(RegisterLine {
            line_id: self.line_id,
            txn_id: self.transaction_id,
            txn_type: self.txn_type,
            number: self.txn_number,
            check_number: self.check_number,
            date: self.txn_date,
            payee: self.payee_name,
            contact: self.contact_id,
            category,
            memo: self.memo,
            payment: amount_opt(self.payment.as_deref(), "register payment")?,
            deposit: amount_opt(self.deposit.as_deref(), "register deposit")?,
            balance: amount(&self.running_balance, "register balance")?,
            cleared,
            has_splits: self.has_splits,
            engine_editable: self.is_editable,
            status: self.status,
        })
    }
}

/// Reads a register page answer.
///
/// # Errors
/// `Decode` for any shape problem (a money string outside the engine
/// grammar, a `null` balance, an unknown cleared state, an exact page
/// without its ending balance).
pub fn page_from_json(body: &[u8]) -> Result<RegisterPage, ApiError> {
    let w: WirePage =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: "register" })?;
    let balance_forward = amount(&w.balance_forward, "register balance forward")?;
    let ending_balance = amount_opt(w.ending_balance.as_deref(), "register ending balance")?;
    // An exact engine (R1/R4) always sends the ending balance.
    if balance_forward.is_exact() && ending_balance.is_none() {
        return Err(ApiError::Decode {
            what: "register ending balance",
        });
    }
    let lines = w
        .lines
        .into_iter()
        .map(WireLine::into_line)
        .collect::<Result<Vec<_>, _>>()?;
    let exact = all_exact(
        [Some(&balance_forward), ending_balance.as_ref()]
            .into_iter()
            .chain(
                lines
                    .iter()
                    .flat_map(|l| [l.payment.as_ref(), l.deposit.as_ref(), Some(&l.balance)]),
            ),
    );
    Ok(RegisterPage {
        account: RegisterAccount {
            id: w.account.id,
            account_type: w.account.account_type,
            detail_type: w.account.detail_type,
        },
        exact,
        balance_forward,
        ending_balance,
        page: w.pagination.page,
        per_page: w.pagination.per_page,
        total_rows: w.pagination.total_rows,
        total_pages: w.pagination.total_pages,
        allowed: w
            .allowed_entry_types
            .iter()
            .map(|s| EntryType::parse(s))
            .collect(),
        lines,
    })
}

/// Reads a register summary answer.
///
/// # Errors
/// `Decode` for any shape problem.
pub fn summary_from_json(body: &[u8]) -> Result<RegisterSummary, ApiError> {
    let w: WireSummary = serde_json::from_slice(body).map_err(|_| ApiError::Decode {
        what: "register summary",
    })?;
    let current_balance = amount(&w.current_balance, "register current balance")?;
    let ending_balance = amount_opt(w.ending_balance.as_deref(), "register ending balance")?;
    let cleared_balance = amount_opt(w.cleared_balance.as_deref(), "register cleared balance")?;
    let exact = all_exact([
        Some(&current_balance),
        ending_balance.as_ref(),
        cleared_balance.as_ref(),
    ]);
    Ok(RegisterSummary {
        exact,
        current_balance,
        ending_balance,
        cleared_balance,
        as_of: w.as_of_date,
        uncleared_count: w.uncleared_count,
        last_reconciled: w.last_reconciliation_date,
    })
}

// ------------------------------------------------------------- resource

/// Register operations of a session (`session.register()`).
#[derive(Clone, Copy, Debug)]
pub struct Register<'a>(pub(crate) &'a Session);

impl Session {
    /// Account registers of the open company.
    #[must_use]
    pub fn register(&self) -> Register<'_> {
        Register(self)
    }
}

impl Register<'_> {
    /// One page of an account's register (`page` counts from 1, at most
    /// 200 rows a page).
    ///
    /// # Errors
    /// `Local(..)` from [`RegisterQuery::to_query`] (nothing sent);
    /// `NotFound`; any [`ApiError`].
    pub async fn page(
        &self,
        account: AccountId,
        query: &RegisterQuery,
        page: u32,
        per_page: u16,
    ) -> Result<RegisterPage, ApiError> {
        let caps = self.0.client().capabilities();
        let pairs = query
            .to_query(&caps, page, per_page)
            .map_err(ApiError::Local)?;
        let mut call = Call::new(
            Method::Get,
            REGISTER.route(),
            format!("/api/v1/accounts/{account}/register"),
        );
        call.query = pairs;
        let reply = self.0.send(&REGISTER, call, None).await?;
        page_from_json(&reply.body)
    }

    /// The register footer. `end` limits the ending balance to lines dated
    /// on or before it (all dates when `None`).
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn summary(
        &self,
        account: AccountId,
        end: Option<CalendarDate>,
    ) -> Result<RegisterSummary, ApiError> {
        let mut call = Call::new(
            Method::Get,
            REGISTER_SUMMARY.route(),
            format!("/api/v1/accounts/{account}/register/summary"),
        );
        call.query.push(("exact".into(), "1".into()));
        if let Some(d) = end {
            call.query.push(("end_date".into(), d.to_wire()));
        }
        let reply = self.0.send(&REGISTER_SUMMARY, call, None).await?;
        summary_from_json(&reply.body)
    }
}

#[cfg(test)]
#[path = "register_tests.rs"]
mod tests;
