//! IIF export (P32, Q-126, Q-P32c, Q-P32j): accounts, clients, vendors,
//! items and every posted transaction as a QuickBooks Desktop general
//! journal entry, for an owner who wants to go back to (or hand off to)
//! QuickBooks Desktop.
//!
//! This module is pure: it takes lists and transactions already read
//! from the engine ([`fetch`] does that) and plans the file. A plan is
//! made in full before anything is written, so a stop writes nothing.
//!
//! # Rules
//! - **Every transaction becomes a GENERAL JOURNAL** (`TRNSTYPE`), whatever
//!   its kind in ClientLedger. The first line is the `TRNS` row, the
//!   others are `SPL` rows, then `ENDTRNS`. Amounts are signed: debits
//!   positive, credits negative; a block sums to `0.00`.
//! - **Cents per line (Q-P32c, Q-P32j).** Each line is rounded to cents,
//!   half away from zero (as `display_amount`). When a transaction's
//!   rounded lines don't sum to zero it goes on the stop list; nothing is
//!   written and no plug line is ever added. A line that rounds to `0.00`
//!   is left out (the transaction still foots).
//! - **Names (Q-P32j).** An A/R line uses the client's name and an A/P
//!   line the vendor's; either without one is a stop. A contact that is
//!   both a client and a vendor is written twice, the vendor entry with
//!   ` (V)` added, because QuickBooks keeps one list of names for
//!   customers and vendors. Other lines carry the name that suits the
//!   transaction's side (sales: the client; purchases: the vendor).
//! - **Text.** Windows-1252, CRLF line ends, no tab, line break or `"` in
//!   a field, `:` only between an account and its sub-account
//!   ([`encode`]). Names are unique without regard to case after those
//!   substitutions and the length cut.
//! - **Accounts** carry no opening balance (the transactions make the
//!   balances). Sub-accounts are `Parent:Child`.
//!
//! # QuickBooks rules this file relies on (R5)
//! The plan asked for each rule to be checked against Intuit's published
//! IIF reference before it was written. On 2026-11-19 neither Intuit's
//! help pages nor an archived copy could be read from the build machine,
//! so **none is confirmed from the source**; each follows the
//! conservative Q-P32j default and the unconfirmed ones are listed as
//! limitations ([`Limitation::Unconfirmed`], QUESTIONS Q-P32v):
//! - headers `!ACCNT`, `!CUST`, `!VEND`, `!INVITEM`, `!TRNS`, `!SPL`,
//!   `!ENDTRNS` and the field names used here; dates `MM/DD/YYYY`;
//! - the `ACCNTTYPE` codes (`BANK`, `AR`, `OCASSET`, `FIXASSET`, `OASSET`,
//!   `AP`, `CCARD`, `OCLIAB`, `LTLIAB`, `EQUITY`, `INC`, `COGS`, `EXP`,
//!   `EXINC`, `EXEXP`) and `INVITEMTYPE` codes (`SERV`, `PART`, `OTHC`);
//! - name lengths: 31 characters per account segment and item, 41 per
//!   client or vendor, 11 for the entry number;
//! - one name list shared by customers and vendors;
//! - whether QuickBooks accepts more than one A/R or A/P line in a general
//!   journal entry (not a stop; listed);
//! - whether an IIF import is all or nothing. It is assumed **not** to be,
//!   so the dialog always says to back up the QuickBooks file first.
//!
//! # Limitations (also in `docs/user/iif-export.md`)
//! Voided, cash-basis-only and draft entries are left out; every
//! transaction is a journal entry (no invoices, bills or payments as
//! such, so no open-invoice matching); no opening balances; no inventory
//! quantities (inventory parts become non-inventory parts), sales tax
//! setup, payroll items, budgets, attachments or audit trail; inactive
//! status is not carried; discount and subtotal items are left out.

#![allow(
    clippy::doc_markdown,
    reason = "QuickBooks and ClientLedger are product names, not code"
)]
pub mod encode;
pub mod fetch;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use cl_core::{CalendarDate, Money};
use rust_decimal::{Decimal, RoundingStrategy};

use encode::{FieldKind, Fields, unique};

/// Longest account name segment and item name (characters).
pub const ACCOUNT_SEGMENT_MAX: usize = 31;
/// Longest item name.
pub const ITEM_NAME_MAX: usize = 31;
/// Longest client or vendor name.
pub const NAME_MAX: usize = 41;
/// Longest entry number.
pub const DOCNUM_MAX: usize = 11;
/// Longest memo or description.
pub const MEMO_MAX: usize = 4095;
/// What a vendor entry of a client-and-vendor contact ends with.
pub const VENDOR_SUFFIX: &str = " (V)";

/// An account as the engine lists it (ids are opaque keys).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceAccount {
    /// Account id.
    pub id: String,
    /// Its own name (not the full path).
    pub name: String,
    /// Account number.
    pub number: Option<String>,
    /// Parent account id.
    pub parent_id: Option<String>,
    /// Engine account type (`asset`, `liability`, …).
    pub account_type: String,
    /// Engine detail type.
    pub detail_type: Option<String>,
    /// Engine system role (`accounts_receivable`, …).
    pub system_tag: Option<String>,
    /// Description.
    pub description: Option<String>,
}

/// Whether a contact is a client, a vendor or both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactKind {
    /// A client (customer).
    Client,
    /// A vendor.
    Vendor,
    /// Both.
    Both,
}

impl ContactKind {
    /// From the engine's `contactType`.
    #[must_use]
    pub fn parse(s: &str) -> Option<ContactKind> {
        match s {
            "customer" => Some(ContactKind::Client),
            "vendor" => Some(ContactKind::Vendor),
            "both" => Some(ContactKind::Both),
            _ => None,
        }
    }

    fn client(self) -> bool {
        matches!(self, ContactKind::Client | ContactKind::Both)
    }

    fn vendor(self) -> bool {
        matches!(self, ContactKind::Vendor | ContactKind::Both)
    }
}

/// A client or vendor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceContact {
    /// Contact id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Kind.
    pub kind: ContactKind,
}

/// An item as the engine lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceItem {
    /// Item id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Engine `itemType` (`service`, `non_inventory`, …).
    pub item_type: String,
    /// Description.
    pub description: Option<String>,
    /// Income account id.
    pub income_account_id: Option<String>,
}

/// One journal line (from the exact transaction list).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceLine {
    /// Journal line id.
    pub line_id: String,
    /// Account id.
    pub account_id: String,
    /// The line's contact (else the transaction's).
    pub contact_id: Option<String>,
    /// Debit minus credit, exact.
    pub amount: Money,
    /// Line description.
    pub memo: Option<String>,
}

/// One posted transaction with its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceTxn {
    /// Transaction id.
    pub id: String,
    /// Date.
    pub date: CalendarDate,
    /// Engine `txnType`.
    pub txn_type: String,
    /// Number shown to the owner.
    pub number: Option<String>,
    /// Memo.
    pub memo: Option<String>,
    /// The transaction's contact.
    pub contact_id: Option<String>,
    /// Lines in engine order.
    pub lines: Vec<SourceLine>,
}

/// Everything the file is made from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Books {
    /// Accounts, inactive ones too.
    pub accounts: Vec<SourceAccount>,
    /// Clients and vendors, inactive ones too.
    pub contacts: Vec<SourceContact>,
    /// Items.
    pub items: Vec<SourceItem>,
    /// Posted transactions.
    pub txns: Vec<SourceTxn>,
}

/// What to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// The account, client, vendor and item lists.
    pub include_lists: bool,
    /// The transactions.
    pub include_transactions: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            include_lists: true,
            include_transactions: true,
        }
    }
}

/// IIF account type (`ACCNTTYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[allow(missing_docs, reason = "IIF codes")]
pub enum AccountKind {
    Bank,
    Ar,
    OtherCurrentAsset,
    FixedAsset,
    OtherAsset,
    Ap,
    CreditCard,
    OtherCurrentLiability,
    LongTermLiability,
    Equity,
    Income,
    Cogs,
    Expense,
    OtherIncome,
    OtherExpense,
}

impl AccountKind {
    /// The IIF code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            AccountKind::Bank => "BANK",
            AccountKind::Ar => "AR",
            AccountKind::OtherCurrentAsset => "OCASSET",
            AccountKind::FixedAsset => "FIXASSET",
            AccountKind::OtherAsset => "OASSET",
            AccountKind::Ap => "AP",
            AccountKind::CreditCard => "CCARD",
            AccountKind::OtherCurrentLiability => "OCLIAB",
            AccountKind::LongTermLiability => "LTLIAB",
            AccountKind::Equity => "EQUITY",
            AccountKind::Income => "INC",
            AccountKind::Cogs => "COGS",
            AccountKind::Expense => "EXP",
            AccountKind::OtherIncome => "EXINC",
            AccountKind::OtherExpense => "EXEXP",
        }
    }
}

/// The IIF type of an engine account, and whether it was a fallback (an
/// unknown or tenant-defined detail type mapped by its top-level type).
/// `None` for an unknown top-level type.
#[must_use]
pub fn account_iif_type(
    account_type: &str,
    detail_type: Option<&str>,
    system_tag: Option<&str>,
) -> Option<(AccountKind, bool)> {
    match system_tag {
        Some("accounts_receivable") => return Some((AccountKind::Ar, false)),
        Some("accounts_payable") => return Some((AccountKind::Ap, false)),
        _ => {}
    }
    let d = detail_type.unwrap_or("");
    let exact = |k| Some((k, false));
    let fallback = |k| Some((k, !d.is_empty()));
    match account_type {
        "asset" => match d {
            "bank" | "checking" | "savings" | "cash" | "cash_on_hand" | "petty_cash"
            | "money_market" => exact(AccountKind::Bank),
            "accounts_receivable" => exact(AccountKind::Ar),
            "other_current_asset" | "undeposited_funds" | "inventory" | "prepaid_expenses" => {
                exact(AccountKind::OtherCurrentAsset)
            }
            "fixed_asset" => exact(AccountKind::FixedAsset),
            "other_asset" => exact(AccountKind::OtherAsset),
            _ => fallback(AccountKind::OtherCurrentAsset),
        },
        "liability" => match d {
            "accounts_payable" => exact(AccountKind::Ap),
            "credit_card" => exact(AccountKind::CreditCard),
            "other_current_liability"
            | "line_of_credit"
            | "sales_tax_payable"
            | "payroll_liabilities" => exact(AccountKind::OtherCurrentLiability),
            "long_term_liability" => exact(AccountKind::LongTermLiability),
            _ => fallback(AccountKind::OtherCurrentLiability),
        },
        "equity" => Some((AccountKind::Equity, !known(account_type, d))),
        "revenue" => Some((AccountKind::Income, !known(account_type, d))),
        "cogs" => Some((AccountKind::Cogs, !known(account_type, d))),
        "expense" => Some((AccountKind::Expense, !known(account_type, d))),
        "other_revenue" => Some((AccountKind::OtherIncome, !known(account_type, d))),
        "other_expense" => Some((AccountKind::OtherExpense, !known(account_type, d))),
        _ => None,
    }
}

/// The engine's built-in detail types (`fixtures/iif/detail_types.txt`,
/// a copy of `DETAIL_TYPES` in `engine/packages/shared`), with an empty
/// detail type counted as known.
const DETAIL_TYPES: &str = include_str!("../../../../fixtures/iif/detail_types.txt");

fn known(account_type: &str, detail: &str) -> bool {
    detail.is_empty()
        || DETAIL_TYPES.lines().any(|l| {
            let mut p = l.split_whitespace();
            p.next() == Some(account_type) && p.next() == Some(detail)
        })
}

/// Which side of the business a transaction is on, for the name on lines
/// other than A/R and A/P.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Sales: the client's name.
    Sales,
    /// Purchases: the vendor's name.
    Purchases,
    /// Neither: the contact's name as a client when it is one, else as a
    /// vendor.
    Other,
}

/// The side of an engine transaction kind.
#[must_use]
pub fn side_of(txn_type: &str) -> Side {
    match txn_type {
        "invoice" | "customer_payment" | "cash_sale" | "credit_memo" | "customer_refund"
        | "daily_sales" | "returned_payment" | "bad_debt_recovery" => Side::Sales,
        "expense" | "bill" | "vendor_credit" | "bill_payment" | "sales_tax_payment" => {
            Side::Purchases
        }
        _ => Side::Other,
    }
}

/// Why a transaction stops the export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// Its lines rounded to cents don't sum to zero; `off` is the sum.
    NotFooting {
        /// The rounded lines' sum.
        off: Decimal,
    },
    /// An A/R line has no client.
    ArWithoutClient,
    /// An A/P line has no vendor.
    ApWithoutVendor,
}

/// A transaction the export can't write. Any stop means nothing is
/// written (Q-P32c).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    /// Transaction id (for Open).
    pub txn_id: String,
    /// Date.
    pub date: CalendarDate,
    /// Engine `txnType`.
    pub txn_type: String,
    /// Number.
    pub number: Option<String>,
    /// Why.
    pub reason: StopReason,
}

/// Something the file can't carry, shown after a finished export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Limitation {
    /// Accounts whose detail type was unknown, typed by their top-level
    /// type.
    FallbackAccountTypes(u64),
    /// Items left out (discount, subtotal, or no income account).
    ItemsLeftOut(u64),
    /// Inventory parts written as non-inventory parts.
    InventoryAsNonInventory(u64),
    /// Characters written as `?`.
    Unmappable(u64),
    /// Names changed to stay unique.
    Renamed(u64),
    /// Transactions with more than one A/R or A/P line (QuickBooks may
    /// refuse them; unconfirmed).
    SeveralArApLines(u64),
    /// QuickBooks rules that could not be checked against Intuit's
    /// reference (R5).
    Unconfirmed,
}

/// What a plan holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Accounts.
    pub accounts: u64,
    /// Clients.
    pub clients: u64,
    /// Vendors.
    pub vendors: u64,
    /// Items.
    pub items: u64,
    /// Transactions.
    pub transactions: u64,
    /// Lines.
    pub lines: u64,
}

/// Why the export could not be planned or written.
#[derive(Debug, thiserror::Error)]
pub enum IifError {
    /// Transactions that can't be written; nothing was written.
    #[error("{} transactions can't be written", .0.len())]
    Stopped(Vec<Stop>),
    /// A line names an account or contact that isn't in the lists.
    #[error("a transaction refers to a missing {what}")]
    Inconsistent {
        /// `account` or `contact`.
        what: &'static str,
    },
    /// The same journal line came twice, or a transaction in two windows.
    #[error("the transaction list repeated a line")]
    Duplicate,
    /// The books changed while the file was read.
    #[error("the books changed while the file was made")]
    BooksChanged,
    /// The user can't see hidden accounts (Q-P32i).
    #[error("needs sight of every account")]
    HiddenAccounts,
    /// Cancelled by the owner.
    #[error("cancelled")]
    Cancelled,
    /// Writing the file.
    #[error("could not write the file")]
    Io(#[from] std::io::Error),
    /// Reading from the engine.
    #[error(transparent)]
    Api(#[from] cl_api::ApiError),
}

/// Collects transaction-list rows window by window, grouping lines by
/// transaction id (never by consecutive rows: the engine interleaves the
/// lines of transactions with the same date and time, S10a).
#[derive(Debug, Default)]
pub struct TxnCollector {
    txns: BTreeMap<String, SourceTxn>,
    lines: BTreeSet<String>,
    /// Transaction ids of earlier windows.
    done: BTreeSet<String>,
}

impl TxnCollector {
    /// Adds one window's rows (each a line with its transaction header).
    ///
    /// # Errors
    /// `Duplicate` for a line seen before or a transaction seen in an
    /// earlier window.
    pub fn add_window(&mut self, rows: Vec<(SourceTxn, SourceLine)>) -> Result<(), IifError> {
        let mut here = BTreeSet::new();
        for (head, line) in rows {
            if self.done.contains(&head.id) || !self.lines.insert(line.line_id.clone()) {
                return Err(IifError::Duplicate);
            }
            here.insert(head.id.clone());
            self.txns
                .entry(head.id.clone())
                .or_insert_with(|| SourceTxn {
                    lines: Vec::new(),
                    ..head
                })
                .lines
                .push(line);
        }
        self.done.extend(here);
        Ok(())
    }

    /// The transactions, oldest first (then by id).
    #[must_use]
    pub fn finish(self) -> Vec<SourceTxn> {
        let mut v: Vec<SourceTxn> = self.txns.into_values().collect();
        v.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.id.cmp(&b.id)));
        v
    }
}

/// One written list row.
#[derive(Clone, Debug, PartialEq, Eq)]
struct AccountRow {
    name: String,
    kind: AccountKind,
    desc: String,
    number: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ItemRow {
    name: String,
    kind: &'static str,
    desc: String,
    account: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LineRow {
    account: String,
    name: String,
    amount: Decimal,
    memo: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TxnRow {
    date: CalendarDate,
    number: String,
    lines: Vec<LineRow>,
}

/// A file ready to write: every check has passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    accounts: Vec<AccountRow>,
    clients: Vec<String>,
    vendors: Vec<String>,
    items: Vec<ItemRow>,
    txns: Vec<TxnRow>,
    /// What the file holds.
    pub counts: Counts,
    /// What it can't carry.
    pub limitations: Vec<Limitation>,
    /// Characters written as `?`.
    pub unmappable_chars: u64,
}

/// A line's cents: half away from zero, never `-0`.
#[must_use]
pub fn cents(m: Money) -> Decimal {
    let d = m
        .as_decimal()
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    if d.is_zero() { Decimal::ZERO } else { d }
}

/// An amount as IIF text: two decimals, debits positive, never `-0.00`.
#[must_use]
pub fn amount_text(d: Decimal) -> String {
    let mut d = d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    if d.is_zero() {
        d = Decimal::ZERO;
    }
    d.rescale(2);
    d.to_string()
}

/// Plans the file: names, types, rounding and every stop rule.
///
/// # Errors
/// `Stopped` with every transaction that can't be written (nothing is
/// written); `Inconsistent` when a line names an account or contact that
/// isn't in the lists.
#[allow(
    clippy::too_many_lines,
    reason = "one pass over the lists, then the transactions"
)]
pub fn plan(books: &Books, opts: Options) -> Result<Plan, IifError> {
    let mut f = Fields::default();
    let mut renamed = 0u64;
    let mut limitations = Vec::new();

    // Accounts: full `Parent:Child` names, parents first.
    let by_id: BTreeMap<&str, &SourceAccount> =
        books.accounts.iter().map(|a| (a.id.as_str(), a)).collect();
    let mut full: BTreeMap<String, String> = BTreeMap::new();
    let mut kinds: BTreeMap<String, AccountKind> = BTreeMap::new();
    let mut fallbacks = 0u64;
    let mut ordered: Vec<&SourceAccount> = books.accounts.iter().collect();
    ordered.sort_by_key(|a| (depth(a, &by_id), a.name.to_lowercase(), a.id.clone()));
    let mut taken_accounts = BTreeSet::new();
    let mut account_rows = Vec::new();
    for a in ordered {
        let Some((kind, fell_back)) = account_iif_type(
            &a.account_type,
            a.detail_type.as_deref(),
            a.system_tag.as_deref(),
        ) else {
            return Err(IifError::Inconsistent { what: "account" });
        };
        fallbacks += u64::from(fell_back);
        let seg = f.clean(&a.name, FieldKind::ListName, ACCOUNT_SEGMENT_MAX);
        let parent = a.parent_id.as_deref().and_then(|p| full.get(p)).cloned();
        // Every segment goes through `unique` (review M4): a name that
        // cleans to nothing becomes `?`, never an empty segment, and the
        // last segment is unique under its parent (the full name).
        let unique_seg = unique_under(&seg, parent.as_deref(), &taken_accounts);
        renamed += u64::from(unique_seg != seg);
        let name = match &parent {
            Some(p) => format!("{p}:{unique_seg}"),
            None => unique_seg,
        };
        taken_accounts.insert(name.to_lowercase());
        full.insert(a.id.clone(), name.clone());
        kinds.insert(a.id.clone(), kind);
        account_rows.push(AccountRow {
            name,
            kind,
            desc: f.clean(
                a.description.as_deref().unwrap_or(""),
                FieldKind::Text,
                MEMO_MAX,
            ),
            number: f.clean(a.number.as_deref().unwrap_or(""), FieldKind::Text, 7),
        });
    }
    if fallbacks > 0 {
        limitations.push(Limitation::FallbackAccountTypes(fallbacks));
    }

    // Clients and vendors share one name list; clients first.
    let mut taken_names = BTreeSet::new();
    let mut client_name: BTreeMap<&str, String> = BTreeMap::new();
    let mut vendor_name: BTreeMap<&str, String> = BTreeMap::new();
    let mut contacts: Vec<&SourceContact> = books.contacts.iter().collect();
    contacts.sort_by_key(|c| (c.name.to_lowercase(), c.id.clone()));
    for c in contacts.iter().filter(|c| c.kind.client()) {
        let clean = f.clean(&c.name, FieldKind::ListName, NAME_MAX);
        let n = unique(&clean, NAME_MAX, &taken_names);
        renamed += u64::from(n != clean);
        taken_names.insert(n.to_lowercase());
        client_name.insert(c.id.as_str(), n);
    }
    for c in contacts.iter().filter(|c| c.kind.vendor()) {
        let clean = f.clean(&c.name, FieldKind::ListName, NAME_MAX);
        let wanted = if c.kind == ContactKind::Both {
            let keep = NAME_MAX - VENDOR_SUFFIX.len();
            let stem: String = clean.chars().take(keep).collect();
            format!("{}{VENDOR_SUFFIX}", stem.trim_end())
        } else {
            clean.clone()
        };
        let n = unique(&wanted, NAME_MAX, &taken_names);
        renamed += u64::from(n != wanted);
        taken_names.insert(n.to_lowercase());
        vendor_name.insert(c.id.as_str(), n);
    }
    let kind_of: BTreeMap<&str, ContactKind> = books
        .contacts
        .iter()
        .map(|c| (c.id.as_str(), c.kind))
        .collect();

    // Items (their own list).
    let mut taken_items = BTreeSet::new();
    let mut item_rows = Vec::new();
    let (mut left_out, mut as_part) = (0u64, 0u64);
    let mut items: Vec<&SourceItem> = books.items.iter().collect();
    items.sort_by_key(|i| (i.name.to_lowercase(), i.id.clone()));
    for i in items {
        let kind = match i.item_type.as_str() {
            "service" => "SERV",
            "non_inventory" => "PART",
            "inventory_part" => {
                as_part += 1;
                "PART"
            }
            "other_charge" => "OTHC",
            _ => {
                left_out += 1;
                continue;
            }
        };
        let Some(account) = i.income_account_id.as_deref().and_then(|a| full.get(a)) else {
            left_out += 1;
            continue;
        };
        let clean = f.clean(&i.name, FieldKind::ListName, ITEM_NAME_MAX);
        let n = unique(&clean, ITEM_NAME_MAX, &taken_items);
        renamed += u64::from(n != clean);
        taken_items.insert(n.to_lowercase());
        item_rows.push(ItemRow {
            name: n,
            kind,
            desc: f.clean(
                i.description.as_deref().unwrap_or(""),
                FieldKind::Text,
                MEMO_MAX,
            ),
            account: account.clone(),
        });
    }
    if left_out > 0 {
        limitations.push(Limitation::ItemsLeftOut(left_out));
    }
    if as_part > 0 {
        limitations.push(Limitation::InventoryAsNonInventory(as_part));
    }

    // Transactions.
    let mut stops = Vec::new();
    let mut txn_rows = Vec::new();
    let mut several_arap = 0u64;
    let mut lines_written = 0u64;
    if opts.include_transactions {
        for t in &books.txns {
            let side = side_of(&t.txn_type);
            let mut rows = Vec::new();
            let mut sum = Decimal::ZERO;
            let mut reason = None;
            let mut arap = 0u32;
            for l in &t.lines {
                let account = full
                    .get(&l.account_id)
                    .ok_or(IifError::Inconsistent { what: "account" })?;
                let kind = kinds
                    .get(&l.account_id)
                    .copied()
                    .ok_or(IifError::Inconsistent { what: "account" })?;
                let contact = l.contact_id.as_deref().or(t.contact_id.as_deref());
                let ck = match contact {
                    Some(c) => Some(
                        kind_of
                            .get(c)
                            .copied()
                            .ok_or(IifError::Inconsistent { what: "contact" })?,
                    ),
                    None => None,
                };
                let client = || contact.and_then(|c| client_name.get(c)).cloned();
                let vendor = || contact.and_then(|c| vendor_name.get(c)).cloned();
                let name = match kind {
                    AccountKind::Ar => {
                        arap += 1;
                        let n = client();
                        if n.is_none() {
                            reason.get_or_insert(StopReason::ArWithoutClient);
                        }
                        n.unwrap_or_default()
                    }
                    AccountKind::Ap => {
                        arap += 1;
                        let n = vendor();
                        if n.is_none() {
                            reason.get_or_insert(StopReason::ApWithoutVendor);
                        }
                        n.unwrap_or_default()
                    }
                    _ => match (side, ck) {
                        (_, None) => String::new(),
                        (Side::Sales, Some(k)) if k.client() => client().unwrap_or_default(),
                        (Side::Purchases, Some(k)) if k.vendor() => vendor().unwrap_or_default(),
                        (_, Some(k)) if k.client() => client().unwrap_or_default(),
                        (_, Some(_)) => vendor().unwrap_or_default(),
                    },
                };
                let c = cents(l.amount);
                sum += c;
                if c.is_zero() {
                    continue;
                }
                rows.push(LineRow {
                    account: account.clone(),
                    name,
                    amount: c,
                    memo: f.clean(
                        l.memo.as_deref().or(t.memo.as_deref()).unwrap_or(""),
                        FieldKind::Text,
                        MEMO_MAX,
                    ),
                });
            }
            if !sum.is_zero() {
                reason = Some(StopReason::NotFooting { off: sum });
            }
            if let Some(reason) = reason {
                stops.push(Stop {
                    txn_id: t.id.clone(),
                    date: t.date,
                    txn_type: t.txn_type.clone(),
                    number: t.number.clone(),
                    reason,
                });
                continue;
            }
            if rows.is_empty() {
                // Every line rounded to zero: nothing to write.
                continue;
            }
            several_arap += u64::from(arap > 1);
            lines_written += rows.len() as u64;
            txn_rows.push(TxnRow {
                date: t.date,
                number: f.clean(
                    t.number.as_deref().unwrap_or(""),
                    FieldKind::Text,
                    DOCNUM_MAX,
                ),
                lines: rows,
            });
        }
    }
    if !stops.is_empty() {
        return Err(IifError::Stopped(stops));
    }
    if several_arap > 0 {
        limitations.push(Limitation::SeveralArApLines(several_arap));
    }
    if renamed > 0 {
        limitations.push(Limitation::Renamed(renamed));
    }
    if f.unmappable > 0 {
        limitations.push(Limitation::Unmappable(f.unmappable));
    }
    limitations.push(Limitation::Unconfirmed);

    let sorted = |m: BTreeMap<&str, String>| {
        let mut v: Vec<String> = m.into_values().collect();
        v.sort_by_key(|n| n.to_lowercase());
        v
    };
    let (clients, vendors) = (sorted(client_name), sorted(vendor_name));
    let mut plan = Plan {
        counts: Counts {
            accounts: account_rows.len() as u64,
            clients: clients.len() as u64,
            vendors: vendors.len() as u64,
            items: item_rows.len() as u64,
            transactions: txn_rows.len() as u64,
            lines: lines_written,
        },
        accounts: account_rows,
        clients,
        vendors,
        items: item_rows,
        txns: txn_rows,
        limitations,
        unmappable_chars: f.unmappable,
    };
    if !opts.include_lists {
        plan.accounts.clear();
        plan.clients.clear();
        plan.vendors.clear();
        plan.items.clear();
        plan.counts.accounts = 0;
        plan.counts.clients = 0;
        plan.counts.vendors = 0;
        plan.counts.items = 0;
    }
    Ok(plan)
}

fn depth(a: &SourceAccount, by_id: &BTreeMap<&str, &SourceAccount>) -> usize {
    let mut d = 0;
    let mut at = a;
    while let Some(p) = at.parent_id.as_deref().and_then(|p| by_id.get(p)) {
        d += 1;
        at = p;
        if d > by_id.len() {
            break;
        }
    }
    d
}

fn unique_under(seg: &str, parent: Option<&str>, taken: &BTreeSet<String>) -> String {
    let prefix = parent
        .map(|p| format!("{p}:"))
        .unwrap_or_default()
        .to_lowercase();
    let under: BTreeSet<String> = taken
        .iter()
        .filter_map(|t| t.strip_prefix(&prefix))
        .filter(|rest| !rest.contains(':'))
        .map(str::to_string)
        .collect();
    unique(seg, ACCOUNT_SEGMENT_MAX, &under)
}

fn date_text(d: CalendarDate) -> String {
    format!("{:02}/{:02}/{:04}", d.month(), d.day(), d.year())
}

impl Plan {
    /// Writes the file (Windows-1252, CRLF).
    ///
    /// # Errors
    /// Any write error.
    pub fn write_to<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        let mut row = |fields: &[&str]| -> std::io::Result<()> {
            for (i, f) in fields.iter().enumerate() {
                if i > 0 {
                    w.write_all(b"\t")?;
                }
                w.write_all(&encode::encode(f))?;
            }
            w.write_all(b"\r\n")
        };
        if !self.accounts.is_empty() {
            row(&["!ACCNT", "NAME", "ACCNTTYPE", "DESC", "ACCNUM"])?;
            for a in &self.accounts {
                row(&["ACCNT", &a.name, a.kind.code(), &a.desc, &a.number])?;
            }
        }
        if !self.clients.is_empty() {
            row(&["!CUST", "NAME"])?;
            for c in &self.clients {
                row(&["CUST", c])?;
            }
        }
        if !self.vendors.is_empty() {
            row(&["!VEND", "NAME"])?;
            for v in &self.vendors {
                row(&["VEND", v])?;
            }
        }
        if !self.items.is_empty() {
            row(&["!INVITEM", "NAME", "INVITEMTYPE", "DESC", "ACCNT"])?;
            for i in &self.items {
                row(&["INVITEM", &i.name, i.kind, &i.desc, &i.account])?;
            }
        }
        if !self.txns.is_empty() {
            let head = [
                "TRNSID", "TRNSTYPE", "DATE", "ACCNT", "NAME", "AMOUNT", "DOCNUM", "MEMO",
            ];
            row(&[&["!TRNS"][..], &head[..]].concat())?;
            let mut spl = head;
            spl[0] = "SPLID";
            row(&[&["!SPL"][..], &spl[..]].concat())?;
            row(&["!ENDTRNS"])?;
            for t in &self.txns {
                let date = date_text(t.date);
                for (i, l) in t.lines.iter().enumerate() {
                    let amount = amount_text(l.amount);
                    let tag = if i == 0 { "TRNS" } else { "SPL" };
                    row(&[
                        tag,
                        "",
                        "GENERAL JOURNAL",
                        &date,
                        &l.account,
                        &l.name,
                        &amount,
                        &t.number,
                        &l.memo,
                    ])?;
                }
                row(&["ENDTRNS"])?;
            }
        }
        Ok(())
    }

    /// The whole file as bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::new();
        // Writing to a Vec never fails.
        let _ = self.write_to(&mut v);
        v
    }
}

#[cfg(test)]
mod tests;
