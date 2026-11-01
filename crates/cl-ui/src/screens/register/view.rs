//! The register grid (P12 WP5): engine register lines mapped to rows, the
//! page window behind the grid, and the column spec (mockup `Register.html`:
//! Date · Number/Type · Payee/Account·Memo · Payment · ✓ · Deposit ·
//! Balance, two lines a row, or one line on request).
//!
//! Every amount here is text made from what the engine sent
//! ([`RegisterAmount::display`]): exact amounts and display-only ones look
//! the same and neither is ever added up (non-negotiable 7). The Balance
//! column is the engine's running balance as sent (it is already in the
//! account's natural sign, Q-P12h), or a dash while a filter makes it
//! unreliable (Q-P12c). The first amount column always holds the credit and
//! the second the debit; the engine's `payment` / `deposit` fields are
//! placed by [`AccountClass::engine_payment_side`].

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use cl_api::{
    Category, ClearState, RegisterAmount, RegisterLine, RegisterPage, TxnStatus, TxnType,
};
use cl_core::ids::{AccountId, JournalLineId, TransactionId};
use cl_core::{CalendarDate, MoneyStyle};
use cl_grid::{
    Align, CellKind, CellValue, Column, ColumnId, GridSpec, Line, MarkKind, RowHeight, RowKey,
    RowKeyGen, RowModel, RowState, TextKey, WidthSpec,
};
use cl_model::register::{
    AccountClass, BalanceMode, BalanceSource, ColumnLabels, DisplaySign, PAGE, PageNo, PageRequest,
    Reply, RowView, Side, WindowCache,
};

use crate::i18n::t;
use crate::screens::lists::center::model::layout_plain;

/// Date.
pub const COL_DATE: ColumnId = ColumnId(1);
/// Number (line 1).
pub const COL_NUMBER: ColumnId = ColumnId(2);
/// Payee (line 1).
pub const COL_PAYEE: ColumnId = ColumnId(3);
/// Payment (the credit side).
pub const COL_PAYMENT: ColumnId = ColumnId(4);
/// Cleared / reconciled mark.
pub const COL_MARK: ColumnId = ColumnId(5);
/// Deposit (the debit side).
pub const COL_DEPOSIT: ColumnId = ColumnId(6);
/// Type (line 2).
pub const COL_TYPE: ColumnId = ColumnId(7);
/// Account · Memo (line 2).
pub const COL_ACCOUNT: ColumnId = ColumnId(8);
/// Balance (read-only, engine value).
pub const COL_BALANCE: ColumnId = ColumnId(9);

/// Row keys kept before the ones of lines no longer loaded are dropped:
/// four full page windows, so the cursor's key survives ordinary reloads
/// and query changes while a long session cannot grow the maps forever.
// 200 rows a page (`PAGE`) times `MAX_PAGES`, four times over.
const KEY_CAP: usize = 4 * cl_model::register::MAX_PAGES * 200;
const _: () = assert!(PAGE == 200, "KEY_CAP assumes 200 rows a page");

/// The dash shown for a balance that is hidden or unreadable.
const DASH: &str = "\u{2014}";

fn column(id: ColumnId, header: &'static str, line: Line, w: u16) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        empty: None,
        help: None,
        kind: CellKind::Display,
        line,
        align: Align::Left,
        width: WidthSpec {
            min: w.min(80),
            preferred: w,
            flex: u8::from(w > 150),
        },
        editable: false,
        mono: false,
    }
}

/// The register spec for an account of `class`. With `one_line` the second
/// line folds into the first: Type gets its own column and the Payee cell
/// reads "Payee — Account · Memo".
#[must_use]
pub fn spec(one_line: bool, class: AccountClass, balance: BalanceMode) -> GridSpec {
    let labels = ColumnLabels::for_class(class);
    let second = if one_line { Line::First } else { Line::Second };
    let money = |id: ColumnId, header: TextKey, w: u16| Column {
        align: Align::Right,
        mono: true,
        header,
        ..column(id, "", Line::First, w)
    };
    let mut columns = vec![
        Column {
            mono: true,
            ..column(COL_DATE, "register.col.date", Line::First, 100)
        },
        Column {
            mono: true,
            ..column(COL_NUMBER, "register.col.number", Line::First, 90)
        },
        column(COL_TYPE, "register.col.type", second, 90),
        column(
            COL_PAYEE,
            if one_line {
                "register.col.payee_account"
            } else {
                "register.col.payee"
            },
            Line::First,
            240,
        ),
    ];
    if !one_line {
        columns.push(column(
            COL_ACCOUNT,
            "register.col.account_memo",
            Line::Second,
            240,
        ));
    }
    columns.extend([
        money(COL_PAYMENT, labels.payment, 110),
        Column {
            align: Align::Center,
            kind: CellKind::Mark,
            help: Some(TextKey("register.col.mark_help")),
            ..column(COL_MARK, "register.col.mark", Line::First, 40)
        },
        money(COL_DEPOSIT, labels.deposit, 110),
        Column {
            help: Some(
                balance
                    .tooltip()
                    .unwrap_or(TextKey("register.col.balance_help")),
            ),
            ..money(COL_BALANCE, TextKey("register.col.balance"), 120)
        },
    ]);
    let mut s = GridSpec::browse(columns);
    s.height = RowHeight::Uniform {
        two_lines: !one_line,
    };
    s
}

/// The short type code shown under the number (i18n key). `debit` is true
/// when the line is a debit to this account: on a credit card an expense
/// that debits the card pays it down (a payment), one that credits it is a
/// charge.
#[must_use]
pub fn type_key(t: TxnType, class: AccountClass, debit: bool) -> &'static str {
    match t {
        TxnType::Expense if class == AccountClass::CreditCard && debit => {
            "register.type.card_payment"
        }
        TxnType::Expense if class == AccountClass::CreditCard => "register.type.card_charge",
        TxnType::Expense => "register.type.check",
        TxnType::Deposit => "register.type.deposit",
        TxnType::Transfer => "register.type.transfer",
        TxnType::JournalEntry => "register.type.journal",
        TxnType::Aje => "register.type.adjustment",
        TxnType::Invoice => "register.type.invoice",
        TxnType::CustomerPayment => "register.type.payment",
        TxnType::CashSale => "register.type.sales_receipt",
        TxnType::CreditMemo => "register.type.credit_memo",
        TxnType::CustomerRefund => "register.type.refund",
        TxnType::Bill => "register.type.bill",
        TxnType::VendorCredit => "register.type.vendor_credit",
        TxnType::BillPayment => "register.type.bill_payment",
        TxnType::DailySales => "register.type.daily_sales",
        TxnType::ReturnedPayment => "register.type.returned_payment",
    }
}

/// One register row as the grid shows it: text only, made from one engine
/// line. `Debug` shows no amounts, names or memo text.
#[derive(Clone)]
pub struct RegRow {
    /// The journal line (selection follows it across reloads).
    pub line_id: JournalLineId,
    /// Its transaction (edits start from `GET /transactions/{id}`, WP6).
    pub txn_id: TransactionId,
    /// Its kind (Delete is offered only for the kinds the engine deletes,
    /// Q-245).
    pub txn_type: TxnType,
    /// Date.
    pub date: CalendarDate,
    /// Number text.
    pub number: String,
    /// Type code text.
    pub kind: String,
    /// Payee (owner data).
    pub payee: String,
    /// "Account · Memo" (owner data).
    pub account_memo: String,
    /// Plain text (`1084.66`) of the first amount column (the credit:
    /// Payment, Charge, Decrease or Increase), as the engine sent it.
    pub payment: Option<String>,
    /// Plain text of the second amount column (the debit).
    pub deposit: Option<String>,
    /// Plain running balance text, as the engine sent it; `None` when the
    /// engine's value could not be read (shown as a dash).
    pub balance: Option<String>,
    /// Cleared state.
    pub cleared: ClearState,
    /// A void transaction.
    pub void: bool,
    /// A draft (not posted yet).
    pub draft: bool,
    /// The engine allows editing it.
    pub editable: bool,
    /// Every amount on the line was exact.
    pub exact: bool,
}

impl std::fmt::Debug for RegRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegRow")
            .field("line_id", &self.line_id)
            .field("date", &self.date)
            .field("cleared", &self.cleared)
            .field("void", &self.void)
            .finish_non_exhaustive()
    }
}

/// How lines are turned into rows: the register's class and sign, and the
/// chart's names for the other account.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    /// The register's account class.
    pub class: Option<AccountClass>,
    /// "1010 · Operating Checking" by account.
    pub names: HashMap<AccountId, String>,
}

impl Mapping {
    /// The row for one engine line.
    #[must_use]
    pub fn row(&self, l: &RegisterLine) -> RegRow {
        let class = self.class.unwrap_or(AccountClass::Bank);
        // Running balances are register figures: in the natural sign.
        let negate = DisplaySign::for_source(class, BalanceSource::Register).negates();
        let text = |a: Option<&RegisterAmount>| a.and_then(|a| a.display(false));
        let (credit, debit) = match class.engine_payment_side() {
            Side::Payment => (l.payment.as_ref(), l.deposit.as_ref()),
            Side::Deposit => (l.deposit.as_ref(), l.payment.as_ref()),
        };
        let account = match &l.category {
            Category::Account { id, name } => self
                .names
                .get(id)
                .cloned()
                .or_else(|| name.clone())
                .unwrap_or_default(),
            Category::Split => t("register.split"),
            Category::None => String::new(),
        };
        let void = l.status == TxnStatus::Void;
        let draft = l.status == TxnStatus::Draft;
        let mut parts: Vec<String> = Vec::new();
        if void {
            parts.push(t("register.void_prefix"));
        }
        if !account.is_empty() {
            parts.push(account);
        }
        if let Some(m) = l.memo.as_deref().filter(|m| !m.is_empty()) {
            parts.push(m.to_string());
        }
        if draft {
            parts.push(t("register.draft"));
        }
        let amounts = [l.payment.as_ref(), l.deposit.as_ref(), Some(&l.balance)];
        RegRow {
            line_id: l.line_id,
            txn_id: l.txn_id,
            txn_type: l.txn_type,
            date: l.date,
            number: l
                .number
                .clone()
                .or_else(|| l.check_number.map(|n| n.to_string()))
                .unwrap_or_default(),
            kind: t(type_key(l.txn_type, class, debit.is_some())),
            payee: l.payee.clone().unwrap_or_default(),
            account_memo: parts.join(" \u{b7} "),
            payment: text(credit),
            deposit: text(debit),
            balance: l.balance.display(negate),
            cleared: l.cleared,
            void,
            draft,
            editable: l.engine_editable && !void,
            exact: amounts.into_iter().flatten().all(RegisterAmount::is_exact),
        }
    }
}

/// The rows behind the register grid: the page window plus a stable row key
/// per journal line, so the cursor and selection survive reloads (Q-P10n).
#[derive(Debug)]
pub struct RegisterModel {
    window: WindowCache<RegRow>,
    keys: HashMap<JournalLineId, RowKey>,
    lines: HashMap<RowKey, JournalLineId>,
    keygen: RowKeyGen,
    row_of: HashMap<JournalLineId, u64>,
    line_at: BTreeMap<u64, JournalLineId>,
    failed: BTreeSet<u64>,
    /// Rows dated on or before this are locked (NN8).
    pub closing: Option<CalendarDate>,
    /// How the Balance column is filled.
    pub balance: BalanceMode,
    /// The money layout in force.
    pub style: MoneyStyle,
    /// One-line display.
    pub one_line: bool,
    revision: u64,
    visible: Range<usize>,
    display_only: bool,
}

impl Default for RegisterModel {
    fn default() -> Self {
        RegisterModel::new(0)
    }
}

impl RegisterModel {
    /// An empty register for the query with fingerprint `query_fp`.
    #[must_use]
    pub fn new(query_fp: u64) -> RegisterModel {
        RegisterModel {
            window: WindowCache::new(query_fp),
            keys: HashMap::new(),
            lines: HashMap::new(),
            keygen: RowKeyGen::new(),
            row_of: HashMap::new(),
            line_at: BTreeMap::new(),
            failed: BTreeSet::new(),
            closing: None,
            balance: BalanceMode::Engine,
            style: MoneyStyle::default(),
            one_line: false,
            revision: 0,
            visible: 0..0,
            display_only: false,
        }
    }

    /// The page window.
    #[must_use]
    pub fn window(&self) -> &WindowCache<RegRow> {
        &self.window
    }

    /// Switches to another query; false when it is the same one. Keys stay:
    /// a line still in the new range keeps its row key.
    pub fn set_query(&mut self, query_fp: u64) -> bool {
        if !self.window.set_query(query_fp) {
            return false;
        }
        self.row_of.clear();
        self.line_at.clear();
        self.failed.clear();
        self.display_only = false;
        self.revision += 1;
        true
    }

    /// The ledger changed: rows stay on screen until their pages return.
    pub fn invalidate(&mut self) {
        self.window.invalidate();
        self.failed.clear();
        self.revision += 1;
    }

    /// The pages to fetch for the rows drawn last frame (or `rows`).
    pub fn wanted(&mut self, rows: Option<Range<u64>>) -> Vec<PageRequest> {
        let r = rows.unwrap_or(self.visible.start as u64..self.visible.end as u64);
        self.window.wanted(r)
    }

    /// True while `req` still matters.
    #[must_use]
    pub fn is_current(&self, req: &PageRequest) -> bool {
        self.window.is_current(req)
    }

    /// Stores an engine page for `req`, mapped by `mapping`.
    pub fn accept(&mut self, req: PageRequest, page: &RegisterPage, mapping: &Mapping) -> Reply {
        if !self.window.is_current(&req) {
            return Reply::Dropped;
        }
        let rows: Vec<RegRow> = page.lines.iter().map(|l| mapping.row(l)).collect();
        let first = req.page.first_row();
        let stale: Vec<u64> = self
            .line_at
            .range(first..first + PAGE)
            .map(|(r, _)| *r)
            .collect();
        for r in stale {
            if let Some(id) = self.line_at.remove(&r)
                && self.row_of.get(&id) == Some(&r)
            {
                self.row_of.remove(&id);
            }
        }
        for (i, row) in rows.iter().enumerate() {
            let at = first + i as u64;
            if !self.keys.contains_key(&row.line_id) {
                let k = self.keygen.next_key();
                self.keys.insert(row.line_id, k);
                self.lines.insert(k, row.line_id);
            }
            self.row_of.insert(row.line_id, at);
            self.line_at.insert(at, row.line_id);
        }
        if !page.exact {
            self.display_only = true;
        }
        self.failed.remove(&req.page.0);
        self.revision += 1;
        self.prune_keys(KEY_CAP);
        self.window.accept(req, page.total_rows, rows)
    }

    #[cfg(test)]
    pub(super) fn prune_keys_for_test(&mut self, cap: usize) {
        self.prune_keys(cap);
    }

    /// Past `cap` keys, keeps only the keys of lines loaded now.
    fn prune_keys(&mut self, cap: usize) {
        if self.keys.len() <= cap {
            return;
        }
        let row_of = &self.row_of;
        self.keys.retain(|line, _| row_of.contains_key(line));
        self.lines.retain(|_, line| row_of.contains_key(line));
    }

    /// A page failed to load.
    pub fn fail(&mut self, req: PageRequest) -> Reply {
        let r = self.window.fail(req);
        if r == Reply::Taken {
            self.failed.insert(req.page.0);
            self.revision += 1;
        }
        r
    }

    /// "Try again" on every failed page.
    pub fn retry_failed(&mut self) {
        for p in std::mem::take(&mut self.failed) {
            self.window.retry(PageNo(p));
        }
        self.revision += 1;
    }

    /// Pages that failed and wait for "Try again".
    #[must_use]
    pub fn failed_pages(&self) -> usize {
        self.failed.len()
    }

    /// True when a page held display-only amounts (the engine sent JSON
    /// numbers).
    #[must_use]
    pub fn display_only(&self) -> bool {
        self.display_only
    }

    /// Forces a repaint of every cell (style, mode or closing date changed).
    pub fn touch(&mut self) {
        self.revision += 1;
    }

    /// The row on screen at `index`, if loaded.
    #[must_use]
    pub fn row_at(&self, index: usize) -> Option<&RegRow> {
        match self.window.row(index as u64)? {
            RowView::Ready(r) | RowView::Stale(r) => Some(r),
            RowView::Loading | RowView::Failed => None,
        }
    }

    /// The key of a journal line, once seen.
    #[must_use]
    pub fn key_of(&self, line: JournalLineId) -> Option<RowKey> {
        self.keys.get(&line).copied()
    }

    /// The journal line of a key.
    #[must_use]
    pub fn line_of(&self, key: RowKey) -> Option<JournalLineId> {
        self.lines.get(&key).copied()
    }

    /// Rows in the register (0 until the first page arrives).
    #[must_use]
    pub fn total(&self) -> Option<u64> {
        self.window.total_rows()
    }

    /// The rows drawn last frame.
    #[must_use]
    pub fn visible(&self) -> Range<usize> {
        self.visible.clone()
    }

    /// The loaded row behind a key (edits start from its transaction).
    #[must_use]
    pub fn row_for_key(&self, key: RowKey) -> Option<&RegRow> {
        let line = self.line_of(key)?;
        let index = *self.row_of.get(&line)?;
        self.row_at(usize::try_from(index).ok()?)
            .filter(|r| r.line_id == line)
    }

    /// The row is dated on or before the closing date (NN8).
    #[must_use]
    pub fn locked(&self, r: &RegRow) -> bool {
        self.closing.is_some_and(|c| r.date <= c)
    }

    fn money(&self, plain: Option<&String>) -> CellValue<'_> {
        plain.map_or(CellValue::Empty, |p| {
            CellValue::Text(Cow::Owned(layout_plain(p, self.style)))
        })
    }
}

impl RowModel for RegisterModel {
    fn revision(&self) -> u64 {
        self.revision
    }

    fn row_count(&self) -> usize {
        usize::try_from(self.window.total_rows().unwrap_or(0)).unwrap_or(usize::MAX)
    }

    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.row_at(index).and_then(|r| self.key_of(r.line_id))
    }

    fn index_of(&self, key: RowKey) -> Option<usize> {
        let line = self.lines.get(&key)?;
        let at = *self.row_of.get(line)?;
        // Only while the row still holds that line.
        (self.row_at(usize::try_from(at).ok()?)?.line_id == *line)
            .then(|| usize::try_from(at).ok())
            .flatten()
    }

    fn row_state(&self, index: usize) -> RowState {
        let Some(r) = self.row_at(index) else {
            return RowState::empty();
        };
        let mut s = match r.cleared {
            ClearState::Reconciled => RowState::RECONCILED,
            ClearState::Cleared => RowState::CLEARED,
            ClearState::None => RowState::empty(),
        };
        if self.locked(r) {
            s = s | RowState::LOCKED;
        }
        if r.void {
            s = s | RowState::MUTED;
        }
        s
    }

    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let r = match self.window.row(index as u64) {
            Some(RowView::Ready(r) | RowView::Stale(r)) => r,
            Some(RowView::Loading) => return CellValue::Loading,
            Some(RowView::Failed) if col == COL_PAYEE => {
                return CellValue::Text(Cow::Owned(t("register.row_failed")));
            }
            Some(RowView::Failed) | None => return CellValue::Empty,
        };
        match col {
            COL_DATE => CellValue::Date(r.date),
            COL_NUMBER => CellValue::Text(Cow::Borrowed(&r.number)),
            COL_TYPE => CellValue::Text(Cow::Borrowed(&r.kind)),
            COL_PAYEE if self.one_line && !r.account_memo.is_empty() => {
                CellValue::Text(Cow::Owned(if r.payee.is_empty() {
                    r.account_memo.clone()
                } else {
                    format!("{} \u{2014} {}", r.payee, r.account_memo)
                }))
            }
            COL_PAYEE => CellValue::Text(Cow::Borrowed(&r.payee)),
            COL_ACCOUNT => CellValue::Text(Cow::Borrowed(&r.account_memo)),
            COL_PAYMENT => self.money(r.payment.as_ref()),
            COL_DEPOSIT => self.money(r.deposit.as_ref()),
            COL_MARK => CellValue::Mark(match r.cleared {
                ClearState::Reconciled => MarkKind::Reconciled,
                ClearState::Cleared => MarkKind::Cleared,
                ClearState::None => MarkKind::None,
            }),
            COL_BALANCE => match (self.balance, r.balance.as_ref()) {
                (BalanceMode::Engine, Some(b)) => {
                    CellValue::Text(Cow::Owned(layout_plain(b, self.style)))
                }
                _ => CellValue::Text(Cow::Borrowed(DASH)),
            },
            _ => CellValue::Empty,
        }
    }

    fn row_label(&self, index: usize) -> String {
        let Some(r) = self.row_at(index) else {
            return t("register.row_loading");
        };
        let mut label = format!("{}, {}", crate::shell::us_date(r.date), r.payee);
        if !r.number.is_empty() {
            label.push_str(", ");
            label.push_str(&r.number);
        }
        let mut notes: Vec<&str> = Vec::new();
        if self.locked(r) {
            notes.push("register.state.locked");
        }
        if r.void {
            notes.push("register.state.void");
        }
        if r.draft {
            notes.push("register.state.draft");
        }
        match r.cleared {
            ClearState::Reconciled => notes.push("register.state.reconciled"),
            ClearState::Cleared => notes.push("register.state.cleared"),
            ClearState::None => {}
        }
        if !r.editable && !r.void {
            notes.push("register.state.not_editable");
        }
        for n in notes {
            label.push_str(", ");
            label.push_str(&t(n));
        }
        label
    }

    fn visible_range(&mut self, range: Range<usize>) {
        self.visible = range;
    }
}
