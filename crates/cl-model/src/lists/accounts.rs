//! Chart of Accounts rules (P11 step 1): the hierarchy built from each
//! account's `parentId` (as deep as the engine allows, Q-208), the order
//! accounts are listed in, account numbers shown by preference (Q-79) and
//! kept unique (Q-209), the active/inactive and search filters, and the
//! add/edit form's checks.
//!
//! Balances are carried as the engine reported them, one per account. A
//! parent's own balance is never combined with its children's here
//! (non-negotiable 7); the engine total of a subtree waits for U-32.

use std::cmp::Ordering;
use std::collections::HashMap;

use cl_core::Money;
use cl_core::ids::AccountId;

use crate::lists::engine_len;
use crate::text::TextKey;

/// Longest name the engine takes.
pub const NAME_MAX: usize = 255;
/// Longest account number the engine takes.
pub const NUMBER_MAX: usize = 20;
/// Longest detail type the engine takes.
pub const DETAIL_MAX: usize = 100;
/// Levels a full name spells out; deeper accounts show `…:` and their
/// last levels, so a pathological chain cannot build huge names.
pub const FULL_NAME_LEVELS: usize = 32;

/// The eight account types, in chart order (balance sheet first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AccountKind {
    /// Asset (bank, receivables, fixed assets).
    Asset,
    /// Liability (payables, credit cards, loans).
    Liability,
    /// Equity.
    Equity,
    /// Income.
    Income,
    /// Cost of goods sold.
    Cogs,
    /// Expense.
    Expense,
    /// Other income.
    OtherIncome,
    /// Other expense.
    OtherExpense,
}

impl AccountKind {
    /// Every kind, in chart order.
    pub const ALL: [AccountKind; 8] = [
        AccountKind::Asset,
        AccountKind::Liability,
        AccountKind::Equity,
        AccountKind::Income,
        AccountKind::Cogs,
        AccountKind::Expense,
        AccountKind::OtherIncome,
        AccountKind::OtherExpense,
    ];

    /// The engine's `accountType` value.
    #[must_use]
    pub const fn wire(self) -> &'static str {
        match self {
            AccountKind::Asset => "asset",
            AccountKind::Liability => "liability",
            AccountKind::Equity => "equity",
            AccountKind::Income => "revenue",
            AccountKind::Cogs => "cogs",
            AccountKind::Expense => "expense",
            AccountKind::OtherIncome => "other_revenue",
            AccountKind::OtherExpense => "other_expense",
        }
    }

    /// The kind for an engine `accountType` value.
    #[must_use]
    pub fn from_wire(s: &str) -> Option<AccountKind> {
        AccountKind::ALL.into_iter().find(|k| k.wire() == s)
    }

    /// Owner-facing name.
    #[must_use]
    pub const fn label(self) -> TextKey {
        match self {
            AccountKind::Asset => TextKey("lists.accounts.kind.asset"),
            AccountKind::Liability => TextKey("lists.accounts.kind.liability"),
            AccountKind::Equity => TextKey("lists.accounts.kind.equity"),
            AccountKind::Income => TextKey("lists.accounts.kind.income"),
            AccountKind::Cogs => TextKey("lists.accounts.kind.cogs"),
            AccountKind::Expense => TextKey("lists.accounts.kind.expense"),
            AccountKind::OtherIncome => TextKey("lists.accounts.kind.other_income"),
            AccountKind::OtherExpense => TextKey("lists.accounts.kind.other_expense"),
        }
    }

    /// The engine's built-in detail types for this kind (`DETAIL_TYPES` in
    /// the engine's shared account types).
    #[must_use]
    pub const fn detail_types(self) -> &'static [&'static str] {
        match self {
            AccountKind::Asset => &[
                "bank",
                "accounts_receivable",
                "other_current_asset",
                "fixed_asset",
                "other_asset",
            ],
            AccountKind::Liability => &[
                "accounts_payable",
                "credit_card",
                "other_current_liability",
                "long_term_liability",
            ],
            AccountKind::Equity => &["owners_equity", "retained_earnings", "opening_balance"],
            AccountKind::Income => &["service", "sales_of_product"],
            AccountKind::Cogs => &["cost_of_goods_sold", "other_cost_of_service"],
            AccountKind::Expense => &[
                "advertising",
                "bank_charges",
                "insurance",
                "meals_entertainment",
                "office_supplies",
                "legal_professional",
                "rent_or_lease",
                "repairs_maintenance",
                "utilities",
                "travel",
                "payroll_expenses",
            ],
            AccountKind::OtherIncome => &["other_income", "interest_earned"],
            AccountKind::OtherExpense => &[
                "other_expense",
                "interest_expense",
                "depreciation",
                "amortization",
            ],
        }
    }
}

/// Owner-facing name of a built-in detail type; `None` for a company's own
/// detail type, which is shown as the engine stores it.
#[must_use]
pub fn detail_label(detail: &str) -> Option<TextKey> {
    Some(TextKey(match detail {
        "bank" => "lists.accounts.detail.bank",
        "accounts_receivable" => "lists.accounts.detail.accounts_receivable",
        "other_current_asset" => "lists.accounts.detail.other_current_asset",
        "fixed_asset" => "lists.accounts.detail.fixed_asset",
        "other_asset" => "lists.accounts.detail.other_asset",
        "accounts_payable" => "lists.accounts.detail.accounts_payable",
        "credit_card" => "lists.accounts.detail.credit_card",
        "other_current_liability" => "lists.accounts.detail.other_current_liability",
        "long_term_liability" => "lists.accounts.detail.long_term_liability",
        "owners_equity" => "lists.accounts.detail.owners_equity",
        "retained_earnings" => "lists.accounts.detail.retained_earnings",
        "opening_balance" => "lists.accounts.detail.opening_balance",
        "service" => "lists.accounts.detail.service",
        "sales_of_product" => "lists.accounts.detail.sales_of_product",
        "cost_of_goods_sold" => "lists.accounts.detail.cost_of_goods_sold",
        "other_cost_of_service" => "lists.accounts.detail.other_cost_of_service",
        "advertising" => "lists.accounts.detail.advertising",
        "bank_charges" => "lists.accounts.detail.bank_charges",
        "insurance" => "lists.accounts.detail.insurance",
        "meals_entertainment" => "lists.accounts.detail.meals_entertainment",
        "office_supplies" => "lists.accounts.detail.office_supplies",
        "legal_professional" => "lists.accounts.detail.legal_professional",
        "rent_or_lease" => "lists.accounts.detail.rent_or_lease",
        "repairs_maintenance" => "lists.accounts.detail.repairs_maintenance",
        "utilities" => "lists.accounts.detail.utilities",
        "travel" => "lists.accounts.detail.travel",
        "payroll_expenses" => "lists.accounts.detail.payroll_expenses",
        "other_income" => "lists.accounts.detail.other_income",
        "interest_earned" => "lists.accounts.detail.interest_earned",
        "other_expense" => "lists.accounts.detail.other_expense",
        "interest_expense" => "lists.accounts.detail.interest_expense",
        "depreciation" => "lists.accounts.detail.depreciation",
        "amortization" => "lists.accounts.detail.amortization",
        _ => return None,
    }))
}

/// True for the accounts that carry a bank or card number (Q-154): bank
/// accounts and credit cards.
#[must_use]
pub fn holds_bank_or_card_number(kind: AccountKind, detail: Option<&str>) -> bool {
    matches!(
        (kind, detail),
        (AccountKind::Asset, Some("bank")) | (AccountKind::Liability, Some("credit_card"))
    )
}

/// The detail type of a credit card account: the only accounts that take
/// "Card ends in" digits (A03; the engine's rule, `document-drafts-v1`).
pub const CREDIT_CARD_DETAIL: &str = "credit_card";

/// True when an account of this detail type takes card digits.
#[must_use]
pub fn takes_card_digits(detail: &str) -> bool {
    detail.trim() == CREDIT_CARD_DETAIL
}

/// The last four digits of a credit card as typed or loaded ("Card ends
/// in", A03). Empty means none. `Debug` never shows the digits.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct CardEnding(pub String);

impl CardEnding {
    /// Exactly four ASCII digits (after trimming).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        let t = self.0.trim();
        t.len() == 4 && t.bytes().all(|b| b.is_ascii_digit())
    }

    /// Nothing typed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// The digits, trimmed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.trim()
    }
}

impl std::fmt::Debug for CardEnding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CardEnding(<redacted>)")
    }
}

/// One account as the screen loaded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartEntry {
    /// Id.
    pub id: AccountId,
    /// Parent account, when it is a sub-account.
    pub parent: Option<AccountId>,
    /// Account number.
    pub number: Option<String>,
    /// Name (one level, without the parent's).
    pub name: String,
    /// Type.
    pub kind: AccountKind,
    /// Detail type.
    pub detail: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Active.
    pub active: bool,
    /// Created by the engine (cannot be removed).
    pub system: bool,
    /// Marked sensitive (Q-157; kept locally until U-33).
    pub sensitive: bool,
    /// The account's own balance, as the engine reported it.
    pub balance: Option<Money>,
    /// "Card ends in" of a credit card account (A03; empty when not set or
    /// the engine lacks `document-drafts-v1`).
    pub card_last4: CardEnding,
}

/// One account in chart order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRow {
    /// Index into [`Chart::entries`].
    pub entry: usize,
    /// 0 for a top-level account, 1 for its sub-accounts, and so on.
    pub depth: usize,
    /// `Parent:Child` name.
    pub full_name: String,
    /// The parent link was dropped: the parent is missing, or the accounts
    /// point at each other in a loop.
    pub detached: bool,
}

/// What the list shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChartFilter {
    /// Also list inactive accounts.
    pub show_inactive: bool,
    /// Name or number contains (case-insensitive).
    pub search: String,
}

/// The chart, in display order.
#[derive(Debug, Clone, Default)]
pub struct Chart {
    entries: Vec<ChartEntry>,
    rows: Vec<ChartRow>,
    row_of: HashMap<AccountId, usize>,
    /// Lower-cased full name and number of each row, for the filter.
    search: Vec<(String, String)>,
    numbers: bool,
}

/// Splits a number into runs of ASCII digits and runs of anything else.
fn number_chunks(s: &str) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let digit = first.is_ascii_digit();
        let end = rest
            .char_indices()
            .find(|(_, c)| c.is_ascii_digit() != digit)
            .map_or(rest.len(), |(i, _)| i);
        let (chunk, tail) = rest.split_at(end);
        rest = tail;
        Some(chunk)
    })
}

/// One run of a number: digit runs compare by value (shorter after trimming
/// leading zeros is smaller; `10` before `010`), and come before text runs;
/// text runs compare ignoring case.
fn chunk_cmp(a: &str, b: &str) -> Ordering {
    let digit = |s: &str| s.bytes().next().is_some_and(|c| c.is_ascii_digit());
    match (digit(a), digit(b)) {
        (true, true) => {
            let (ta, tb) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
            ta.len()
                .cmp(&tb.len())
                .then_with(|| ta.cmp(tb))
                .then_with(|| a.len().cmp(&b.len()))
        }
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => a
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(b.chars().flat_map(char::to_lowercase)),
    }
}

/// Account and tag numbers in "natural" order: runs of digits compare as
/// numbers (`900` before `1000`, `1000` before `1000-A` before `2000`), other
/// text ignoring case. A total order (equal only for the same text), so it is
/// safe for every sort.
pub(crate) fn number_cmp(a: &str, b: &str) -> Ordering {
    let mut ca = number_chunks(a);
    let mut cb = number_chunks(b);
    loop {
        match (ca.next(), cb.next()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => match chunk_cmp(x, y) {
                Ordering::Equal => {}
                other => return other,
            },
        }
    }
}

/// The order of siblings: type, then number (numbers on), then name.
fn sibling_cmp(
    entries: &[ChartEntry],
    names: &[String],
    numbers: bool,
    a: usize,
    b: usize,
) -> Ordering {
    let (ea, eb) = (&entries[a], &entries[b]);
    let by_number = if numbers {
        match (&ea.number, &eb.number) {
            (Some(x), Some(y)) => number_cmp(x, y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    } else {
        Ordering::Equal
    };
    ea.kind
        .cmp(&eb.kind)
        .then(by_number)
        .then_with(|| names[a].cmp(&names[b]))
        .then_with(|| ea.id.to_string().cmp(&eb.id.to_string()))
}

/// Builds the rows from the parent links.
struct Tree<'a> {
    entries: &'a [ChartEntry],
    by_id: HashMap<AccountId, usize>,
    children: Vec<Vec<usize>>,
    rows: Vec<ChartRow>,
    visited: Vec<bool>,
}

impl Tree<'_> {
    fn parent_of(&self, i: usize) -> Option<usize> {
        self.entries[i]
            .parent
            .and_then(|p| self.by_id.get(&p))
            .copied()
            .filter(|p| *p != i)
    }

    /// Lists `start` and everything below it. An explicit stack, because
    /// chains can be as deep as the engine lets them.
    fn place(&mut self, start: usize, detached: bool) {
        let mut stack: Vec<(usize, usize, Option<usize>)> = vec![(start, 0, None)];
        while let Some((i, depth, parent_row)) = stack.pop() {
            if self.visited[i] {
                continue;
            }
            self.visited[i] = true;
            let name = &self.entries[i].name;
            let full_name = match parent_row {
                None => name.clone(),
                Some(r) => {
                    let parent = &self.rows[r].full_name;
                    if depth >= FULL_NAME_LEVELS {
                        let tail = parent.rsplit(':').next().unwrap_or_default();
                        format!("\u{2026}:{tail}:{name}")
                    } else {
                        format!("{parent}:{name}")
                    }
                }
            };
            let row = self.rows.len();
            self.rows.push(ChartRow {
                entry: i,
                depth,
                full_name,
                detached: detached && parent_row.is_none(),
            });
            for &c in self.children[i].iter().rev() {
                if !self.visited[c] {
                    stack.push((c, depth + 1, Some(row)));
                }
            }
        }
    }

    /// Accounts left over sit in, or below, a loop of parent links. Walks
    /// up from `from` until an account repeats: that account and the
    /// steps after it form the loop.
    fn loop_members(&self, from: usize) -> Vec<usize> {
        let mut seen: Vec<usize> = Vec::new();
        let mut marked = vec![false; self.entries.len()];
        let mut cur = from;
        while !marked[cur] {
            marked[cur] = true;
            seen.push(cur);
            match self.parent_of(cur) {
                Some(p) if !self.visited[p] => cur = p,
                _ => break,
            }
        }
        let start = seen.iter().position(|s| *s == cur).unwrap_or(0);
        seen.split_off(start)
    }
}

impl Chart {
    /// Arranges `entries`. With `numbers` (Q-79) siblings sort by account
    /// number (numbered accounts first), otherwise by name; types always
    /// come in chart order. Accounts whose parent is missing, or that point
    /// at each other in a loop, are listed at the top and marked detached.
    #[must_use]
    pub fn new(entries: Vec<ChartEntry>, numbers: bool) -> Chart {
        let n = entries.len();
        let names: Vec<String> = entries.iter().map(|e| e.name.to_lowercase()).collect();
        let cmp = |a: &usize, b: &usize| sibling_cmp(&entries, &names, numbers, *a, *b);
        let mut tree = Tree {
            entries: &entries,
            by_id: entries.iter().enumerate().map(|(i, e)| (e.id, i)).collect(),
            children: vec![Vec::new(); n],
            rows: Vec::with_capacity(n),
            visited: vec![false; n],
        };
        let mut roots: Vec<usize> = Vec::new();
        for i in 0..n {
            match tree.parent_of(i) {
                Some(p) => tree.children[p].push(i),
                None => roots.push(i),
            }
        }
        for c in &mut tree.children {
            c.sort_by(cmp);
        }
        roots.sort_by(cmp);
        for &r in &roots {
            tree.place(r, entries[r].parent.is_some());
        }
        // Break each loop at its first account in chart order.
        while let Some(first) = (0..n).filter(|i| !tree.visited[*i]).min_by(cmp) {
            let mut members = tree.loop_members(first);
            members.sort_by(cmp);
            tree.place(members[0], true);
        }
        let rows = tree.rows;
        let row_of = rows
            .iter()
            .enumerate()
            .map(|(r, row)| (entries[row.entry].id, r))
            .collect();
        let search = rows
            .iter()
            .map(|row| {
                let number = entries[row.entry].number.as_deref().unwrap_or_default();
                (row.full_name.to_lowercase(), number.to_lowercase())
            })
            .collect();
        Chart {
            entries,
            rows,
            row_of,
            search,
            numbers,
        }
    }

    /// Accounts as loaded.
    #[must_use]
    pub fn entries(&self) -> &[ChartEntry] {
        &self.entries
    }

    /// Every account in chart order.
    #[must_use]
    pub fn rows(&self) -> &[ChartRow] {
        &self.rows
    }

    /// Number of accounts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No accounts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Account numbers are shown (Q-79).
    #[must_use]
    pub fn numbers_shown(&self) -> bool {
        self.numbers
    }

    /// The row of an account.
    #[must_use]
    pub fn row_of(&self, id: AccountId) -> Option<usize> {
        self.row_of.get(&id).copied()
    }

    /// One account.
    #[must_use]
    pub fn entry(&self, id: AccountId) -> Option<&ChartEntry> {
        self.row_of(id).map(|r| &self.entries[self.rows[r].entry])
    }

    /// The entry of a row.
    #[must_use]
    pub fn entry_at(&self, row: usize) -> Option<&ChartEntry> {
        self.rows.get(row).map(|r| &self.entries[r.entry])
    }

    /// `Parent:Child` name.
    #[must_use]
    pub fn full_name(&self, id: AccountId) -> Option<&str> {
        self.row_of(id).map(|r| self.rows[r].full_name.as_str())
    }

    /// The name lists and pickers show: `1000 · Checking` with numbers on
    /// and a number set, else the full name.
    #[must_use]
    pub fn display_name(&self, id: AccountId) -> Option<String> {
        let r = self.row_of(id)?;
        let full = &self.rows[r].full_name;
        let e = &self.entries[self.rows[r].entry];
        Some(match (&e.number, self.numbers) {
            (Some(n), true) if !n.is_empty() => format!("{n} \u{b7} {full}"),
            _ => full.clone(),
        })
    }

    /// The rows the filter keeps, in chart order.
    #[must_use]
    pub fn visible(&self, filter: &ChartFilter) -> Vec<usize> {
        let q = filter.search.trim().to_lowercase();
        self.rows
            .iter()
            .enumerate()
            .filter(|(r, row)| {
                let e = &self.entries[row.entry];
                if !filter.show_inactive && !e.active {
                    return false;
                }
                if q.is_empty() {
                    return true;
                }
                let (name, number) = &self.search[*r];
                name.contains(&q) || (self.numbers && number.contains(&q))
            })
            .map(|(r, _)| r)
            .collect()
    }

    /// True when `id` is `ancestor` or lies below it.
    #[must_use]
    pub fn is_within(&self, id: AccountId, ancestor: AccountId) -> bool {
        let mut cur = Some(id);
        // At most one step per account: a loop cannot spin forever.
        for _ in 0..=self.entries.len() {
            match cur {
                Some(c) if c == ancestor => return true,
                Some(c) => cur = self.entry(c).and_then(|e| e.parent),
                None => return false,
            }
        }
        false
    }

    /// True when some account has `id` as its parent.
    #[must_use]
    pub fn has_children(&self, id: AccountId) -> bool {
        self.entries
            .iter()
            .any(|e| e.parent == Some(id) && e.id != id)
    }

    /// Rows that may be the parent of an account of `kind`: active accounts
    /// of the same type, never the account being edited or one below it.
    #[must_use]
    pub fn parent_choices(&self, kind: AccountKind, editing: Option<AccountId>) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let e = &self.entries[row.entry];
                e.active && e.kind == kind && editing.is_none_or(|me| !self.is_within(e.id, me))
            })
            .map(|(r, _)| r)
            .collect()
    }

    /// The account that already uses `number` (case-insensitive, trimmed),
    /// other than `except`.
    #[must_use]
    pub fn number_owner(&self, number: &str, except: Option<AccountId>) -> Option<AccountId> {
        let want = number.trim().to_lowercase();
        if want.is_empty() {
            return None;
        }
        self.entries
            .iter()
            .find(|e| {
                Some(e.id) != except
                    && e.number
                        .as_deref()
                        .is_some_and(|n| n.trim().to_lowercase() == want)
            })
            .map(|e| e.id)
    }
}

/// The add/edit account form's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountDraft {
    /// Name.
    pub name: String,
    /// Account number (empty = none).
    pub number: String,
    /// Type.
    pub kind: AccountKind,
    /// Detail type (empty = none).
    pub detail: String,
    /// Description.
    pub description: String,
    /// Parent, for a sub-account.
    pub parent: Option<AccountId>,
    /// Sensitive (Q-157).
    pub sensitive: bool,
    /// "Card ends in" (A03): credit card accounts only; empty clears it.
    pub card_last4: CardEnding,
}

impl AccountDraft {
    /// A blank account of `kind`.
    #[must_use]
    pub fn new(kind: AccountKind) -> AccountDraft {
        AccountDraft {
            name: String::new(),
            number: String::new(),
            kind,
            detail: kind
                .detail_types()
                .first()
                .map(|d| (*d).to_string())
                .unwrap_or_default(),
            description: String::new(),
            parent: None,
            sensitive: false,
            card_last4: CardEnding::default(),
        }
    }

    /// The form for an existing account.
    #[must_use]
    pub fn from_entry(e: &ChartEntry) -> AccountDraft {
        AccountDraft {
            name: e.name.clone(),
            number: e.number.clone().unwrap_or_default(),
            kind: e.kind,
            detail: e.detail.clone().unwrap_or_default(),
            description: e.description.clone().unwrap_or_default(),
            parent: e.parent,
            sensitive: e.sensitive,
            card_last4: e.card_last4.clone(),
        }
    }

    /// Changes the type: the detail type and parent follow (a parent must
    /// be of the same type).
    pub fn set_kind(&mut self, kind: AccountKind) {
        if kind == self.kind {
            return;
        }
        self.kind = kind;
        self.detail = kind
            .detail_types()
            .first()
            .map(|d| (*d).to_string())
            .unwrap_or_default();
        self.parent = None;
        // Card digits belong to a credit card account only.
        self.card_last4 = CardEnding::default();
    }
}

/// A draft that passed the checks, trimmed, ready for the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedAccount {
    /// Name.
    pub name: String,
    /// Number.
    pub number: Option<String>,
    /// Type.
    pub kind: AccountKind,
    /// Detail type.
    pub detail: Option<String>,
    /// Description.
    pub description: Option<String>,
    /// Parent.
    pub parent: Option<AccountId>,
    /// Sensitive.
    pub sensitive: bool,
    /// Card digits, checked (`None`: clear, or not a credit card account).
    pub card_last4: Option<CardEnding>,
}

/// The form field a problem belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftField {
    /// Name.
    Name,
    /// Number.
    Number,
    /// Type.
    Kind,
    /// Detail type.
    Detail,
    /// Parent.
    Parent,
    /// "Card ends in" (A03).
    Card,
}

/// Why a draft cannot be saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftProblem {
    /// No name.
    NameMissing,
    /// Longer than the engine takes.
    NameTooLong,
    /// `:` separates an account from its sub-accounts.
    NameColon,
    /// Longer than the engine takes.
    NumberTooLong,
    /// Another account has this number (Q-209).
    NumberTaken,
    /// Longer than the engine takes.
    DetailTooLong,
    /// The chosen parent is gone.
    ParentMissing,
    /// A parent must be of the same type.
    ParentOtherKind,
    /// The parent is this account or one below it.
    ParentLoop,
    /// The parent is inactive.
    ParentInactive,
    /// The type of an account with sub-accounts cannot change.
    KindHasSubaccounts,
    /// "Card ends in" is not exactly four digits (A03).
    CardDigits,
}

impl DraftProblem {
    /// Plain-language message.
    #[must_use]
    pub const fn key(self) -> TextKey {
        match self {
            DraftProblem::NameMissing => TextKey("lists.accounts.problem.name_missing"),
            DraftProblem::NameTooLong => TextKey("lists.accounts.problem.name_too_long"),
            DraftProblem::NameColon => TextKey("lists.accounts.problem.name_colon"),
            DraftProblem::NumberTooLong => TextKey("lists.accounts.problem.number_too_long"),
            DraftProblem::NumberTaken => TextKey("lists.accounts.problem.number_taken"),
            DraftProblem::DetailTooLong => TextKey("lists.accounts.problem.detail_too_long"),
            DraftProblem::ParentMissing => TextKey("lists.accounts.problem.parent_missing"),
            DraftProblem::ParentOtherKind => TextKey("lists.accounts.problem.parent_other_kind"),
            DraftProblem::ParentLoop => TextKey("lists.accounts.problem.parent_loop"),
            DraftProblem::ParentInactive => TextKey("lists.accounts.problem.parent_inactive"),
            DraftProblem::KindHasSubaccounts => {
                TextKey("lists.accounts.problem.kind_has_subaccounts")
            }
            DraftProblem::CardDigits => TextKey("lists.accounts.problem.card_digits"),
        }
    }

    /// The field to focus.
    #[must_use]
    pub const fn field(self) -> DraftField {
        match self {
            DraftProblem::NameMissing | DraftProblem::NameTooLong | DraftProblem::NameColon => {
                DraftField::Name
            }
            DraftProblem::NumberTooLong | DraftProblem::NumberTaken => DraftField::Number,
            DraftProblem::DetailTooLong => DraftField::Detail,
            DraftProblem::KindHasSubaccounts => DraftField::Kind,
            DraftProblem::CardDigits => DraftField::Card,
            DraftProblem::ParentMissing
            | DraftProblem::ParentOtherKind
            | DraftProblem::ParentLoop
            | DraftProblem::ParentInactive => DraftField::Parent,
        }
    }
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Checks a draft against the loaded chart. `editing` is the account being
/// changed (`None` for a new one). The local number check only spares a
/// round trip: the engine's 409 `ACCOUNT_NUMBER_EXISTS` stays the rule.
///
/// # Errors
/// Every problem found, in field order.
pub fn check(
    draft: &AccountDraft,
    chart: &Chart,
    editing: Option<AccountId>,
) -> Result<CheckedAccount, Vec<DraftProblem>> {
    let mut problems = Vec::new();
    let name = draft.name.trim();
    if name.is_empty() {
        problems.push(DraftProblem::NameMissing);
    } else if engine_len(name) > NAME_MAX {
        problems.push(DraftProblem::NameTooLong);
    }
    if name.contains(':') {
        problems.push(DraftProblem::NameColon);
    }
    let number = non_empty(&draft.number);
    if let Some(n) = &number {
        if engine_len(n) > NUMBER_MAX {
            problems.push(DraftProblem::NumberTooLong);
        } else if chart.number_owner(n, editing).is_some() {
            problems.push(DraftProblem::NumberTaken);
        }
    }
    if let Some(me) = editing
        && chart.entry(me).is_some_and(|e| e.kind != draft.kind)
        && chart.has_children(me)
    {
        problems.push(DraftProblem::KindHasSubaccounts);
    }
    let detail = non_empty(&draft.detail);
    if detail
        .as_deref()
        .is_some_and(|d| engine_len(d) > DETAIL_MAX)
    {
        problems.push(DraftProblem::DetailTooLong);
    }
    if let Some(p) = draft.parent {
        match chart.entry(p) {
            None => problems.push(DraftProblem::ParentMissing),
            Some(_) if editing.is_some_and(|me| chart.is_within(p, me)) => {
                problems.push(DraftProblem::ParentLoop);
            }
            Some(e) if e.kind != draft.kind => problems.push(DraftProblem::ParentOtherKind),
            Some(e) if !e.active => problems.push(DraftProblem::ParentInactive),
            Some(_) => {}
        }
    }
    // Card digits: credit card accounts only (the engine clears them when
    // an account stops being one); exactly four digits, or empty to clear.
    let card = if takes_card_digits(&draft.detail) && !draft.card_last4.is_empty() {
        if draft.card_last4.is_valid() {
            Some(CardEnding(draft.card_last4.as_str().to_string()))
        } else {
            problems.push(DraftProblem::CardDigits);
            None
        }
    } else {
        None
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    Ok(CheckedAccount {
        name: name.to_string(),
        number,
        kind: draft.kind,
        detail,
        description: non_empty(&draft.description),
        parent: draft.parent,
        sensitive: draft.sensitive,
        card_last4: card,
    })
}

#[cfg(test)]
mod tests;
