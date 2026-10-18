//! The two grid models of the Chart of Accounts window: the chart itself
//! (one row per account in hierarchy order, the engine's own balance per
//! account) and the quick report's lines. Neither ever adds amounts up
//! (non-negotiable 7).

use std::borrow::Cow;
use std::collections::HashMap;

use cl_api::{Account, AccountReport, AccountReportLine, AccountType, ReportDate};
use cl_core::CalendarDate;
use cl_core::ids::AccountId;
use cl_grid::{
    Align, CellKind, CellValue, Column, ColumnId, GridSpec, Line, RowHeight, RowKey, RowKeyGen,
    RowModel, RowState, SelectionMode, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::lists::accounts::{AccountKind, Chart, ChartEntry, ChartFilter, detail_label};

use crate::i18n::t;

/// Columns of the chart grid.
pub const COL_NUMBER: ColumnId = ColumnId(1);
/// Name, indented under its parent.
pub const COL_NAME: ColumnId = ColumnId(2);
/// Type.
pub const COL_TYPE: ColumnId = ColumnId(3);
/// Detail type.
pub const COL_DETAIL: ColumnId = ColumnId(4);
/// The engine's balance of this account alone.
pub const COL_BALANCE: ColumnId = ColumnId(5);
/// Inactive / sensitive marks.
pub const COL_STATUS: ColumnId = ColumnId(6);

/// Indent levels drawn; deeper accounts stay at this indent.
const MAX_INDENT: usize = 8;

fn column(id: ColumnId, header: &'static str, w: u16, flex: u8) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        help: None,
        kind: CellKind::Display,
        line: Line::First,
        align: Align::Left,
        width: WidthSpec {
            min: w.min(70),
            preferred: w,
            flex,
        },
        editable: false,
        mono: false,
    }
}

/// The chart grid: read-only, one line per account, one row selected.
/// `compact` (the add/edit panel is open beside it) keeps only number,
/// name, type and balance so nothing is cut off.
#[must_use]
pub fn chart_spec(numbers: bool, compact: bool) -> GridSpec {
    let mut cols = Vec::new();
    if numbers {
        cols.push(Column {
            mono: true,
            ..column(COL_NUMBER, "lists.accounts.col.number", 90, 0)
        });
    }
    let name_width = if compact { 220 } else { 280 };
    cols.extend([
        column(COL_NAME, "lists.accounts.col.name", name_width, 2),
        column(COL_TYPE, "lists.accounts.col.type", 140, 0),
    ]);
    if !compact {
        cols.push(column(COL_DETAIL, "lists.accounts.col.detail", 170, 1));
    }
    cols.push(Column {
        kind: CellKind::Amount(AmountRules::MONEY),
        align: Align::Right,
        mono: true,
        help: Some(TextKey("lists.accounts.col.balance_help")),
        ..column(COL_BALANCE, "lists.accounts.col.balance", 140, 0)
    });
    if !compact {
        cols.push(column(COL_STATUS, "lists.accounts.col.status", 130, 0));
    }
    let mut s = GridSpec::browse(cols);
    s.height = RowHeight::Uniform { two_lines: false };
    s.selection = SelectionMode::Single;
    s
}

/// The engine type for a list kind.
#[must_use]
pub fn api_type(kind: AccountKind) -> AccountType {
    match kind {
        AccountKind::Asset => AccountType::Asset,
        AccountKind::Liability => AccountType::Liability,
        AccountKind::Equity => AccountType::Equity,
        AccountKind::Income => AccountType::Revenue,
        AccountKind::Cogs => AccountType::Cogs,
        AccountKind::Expense => AccountType::Expense,
        AccountKind::OtherIncome => AccountType::OtherRevenue,
        AccountKind::OtherExpense => AccountType::OtherExpense,
    }
}

/// The list kind of an engine type.
#[must_use]
pub fn kind_of(t: AccountType) -> AccountKind {
    match t {
        AccountType::Asset => AccountKind::Asset,
        AccountType::Liability => AccountKind::Liability,
        AccountType::Equity => AccountKind::Equity,
        AccountType::Revenue => AccountKind::Income,
        AccountType::Cogs => AccountKind::Cogs,
        AccountType::Expense => AccountKind::Expense,
        AccountType::OtherRevenue => AccountKind::OtherIncome,
        AccountType::OtherExpense => AccountKind::OtherExpense,
    }
}

/// Text for a detail type: the built-in label, or the company's own value.
#[must_use]
pub fn detail_text(detail: Option<&str>) -> String {
    match detail {
        None | Some("") => String::new(),
        Some(d) => detail_label(d).map_or_else(|| d.to_string(), |k| t(k.as_str())),
    }
}

/// The chart as the grid shows it.
#[derive(Debug, Default)]
pub struct ChartModel {
    chart: Chart,
    /// Chart rows the filter keeps.
    visible: Vec<usize>,
    keys: HashMap<AccountId, RowKey>,
    keygen: RowKeyGen,
    /// Per visible row.
    row_keys: Vec<RowKey>,
    by_key: HashMap<RowKey, usize>,
    /// Per chart row: indented name, type, detail, status.
    names: Vec<String>,
    kinds: Vec<String>,
    details: Vec<String>,
    status: Vec<String>,
    revision: u64,
}

impl ChartModel {
    /// The chart of `accounts` (`sensitive` marks, numbers shown or not).
    pub fn set_accounts(
        &mut self,
        accounts: &[Account],
        sensitive: &std::collections::BTreeSet<AccountId>,
        numbers: bool,
        filter: &ChartFilter,
    ) {
        let entries = accounts
            .iter()
            .map(|a| ChartEntry {
                id: a.id,
                parent: a.fields.parent_id,
                number: a.fields.account_number.clone().filter(|n| !n.is_empty()),
                name: a.fields.name.clone(),
                kind: kind_of(a.fields.account_type),
                detail: a.fields.detail_type.clone(),
                description: a.fields.description.clone(),
                active: a.is_active,
                system: a.is_system,
                sensitive: sensitive.contains(&a.id),
                balance: a.balance,
            })
            .collect();
        self.chart = Chart::new(entries, numbers);
        let (inactive, sens, detached) = (
            t("lists.accounts.status.inactive"),
            t("lists.accounts.status.sensitive"),
            t("lists.accounts.status.detached"),
        );
        self.names.clear();
        self.kinds.clear();
        self.details.clear();
        self.status.clear();
        for row in self.chart.rows() {
            let e = &self.chart.entries()[row.entry];
            let indent = "    ".repeat(row.depth.min(MAX_INDENT));
            self.names.push(format!("{indent}{}", e.name));
            self.kinds.push(t(e.kind.label().as_str()));
            self.details.push(detail_text(e.detail.as_deref()));
            let mut marks: Vec<&str> = Vec::new();
            if !e.active {
                marks.push(&inactive);
            }
            if e.sensitive {
                marks.push(&sens);
            }
            if row.detached {
                marks.push(&detached);
            }
            self.status.push(marks.join(" \u{b7} "));
        }
        self.apply_filter(filter);
    }

    /// Re-applies the filter (keys stay with their accounts).
    pub fn apply_filter(&mut self, filter: &ChartFilter) {
        self.visible = self.chart.visible(filter);
        self.row_keys.clear();
        self.by_key.clear();
        for (i, r) in self.visible.iter().enumerate() {
            let id = self.chart.rows()[*r].entry;
            let id = self.chart.entries()[id].id;
            let key = *self
                .keys
                .entry(id)
                .or_insert_with(|| self.keygen.next_key());
            self.row_keys.push(key);
            self.by_key.insert(key, i);
        }
        self.revision += 1;
    }

    /// The arranged chart.
    #[must_use]
    pub fn chart(&self) -> &Chart {
        &self.chart
    }

    /// The account on a grid row key.
    #[must_use]
    pub fn account_at(&self, key: RowKey) -> Option<&ChartEntry> {
        let i = *self.by_key.get(&key)?;
        self.chart.entry_at(self.visible[i])
    }

    /// The grid row key of an account, when it is shown.
    #[must_use]
    pub fn key_of(&self, id: AccountId) -> Option<RowKey> {
        let key = *self.keys.get(&id)?;
        self.by_key.contains_key(&key).then_some(key)
    }

    /// The first shown account whose name or number starts with `text`.
    #[must_use]
    pub fn find_prefix(&self, text: &str) -> Option<RowKey> {
        let q = text.to_lowercase();
        if q.is_empty() {
            return None;
        }
        self.visible.iter().enumerate().find_map(|(i, r)| {
            let e = self.chart.entry_at(*r)?;
            let number = e.number.as_deref().unwrap_or_default().to_lowercase();
            let hit = e.name.to_lowercase().starts_with(&q)
                || (self.chart.numbers_shown() && number.starts_with(&q));
            hit.then(|| self.row_keys[i])
        })
    }

    /// Accounts shown.
    #[must_use]
    pub fn shown(&self) -> usize {
        self.visible.len()
    }
}

impl RowModel for ChartModel {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.visible.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        self.row_keys.get(index).copied()
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        self.by_key.get(&key).copied()
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(&r) = self.visible.get(index) else {
            return CellValue::Empty;
        };
        let Some(e) = self.chart.entry_at(r) else {
            return CellValue::Empty;
        };
        let text = |s: &str| CellValue::Text(Cow::Owned(s.to_string()));
        match col {
            COL_NUMBER => e.number.as_deref().map_or(CellValue::Empty, text),
            COL_NAME => CellValue::Text(Cow::Borrowed(&self.names[r])),
            COL_TYPE => CellValue::Text(Cow::Borrowed(&self.kinds[r])),
            COL_DETAIL => CellValue::Text(Cow::Borrowed(&self.details[r])),
            COL_BALANCE => e.balance.map_or(CellValue::Empty, CellValue::Money),
            COL_STATUS => CellValue::Text(Cow::Borrowed(&self.status[r])),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let Some(&r) = self.visible.get(index) else {
            return String::new();
        };
        let full = &self.chart.rows()[r].full_name;
        let e = &self.chart.entries()[self.chart.rows()[r].entry];
        let mut label = match (&e.number, self.chart.numbers_shown()) {
            (Some(n), true) => format!("{n} {full}"),
            _ => full.clone(),
        };
        if !e.active {
            label.push_str(", ");
            label.push_str(&t("lists.accounts.status.inactive"));
        }
        label
    }
}

/// Columns of the quick report grid.
pub const QR_DATE: ColumnId = ColumnId(1);
/// Transaction type.
pub const QR_TYPE: ColumnId = ColumnId(2);
/// Number.
pub const QR_NUMBER: ColumnId = ColumnId(3);
/// Payee.
pub const QR_NAME: ColumnId = ColumnId(4);
/// Memo.
pub const QR_MEMO: ColumnId = ColumnId(5);
/// Debit.
pub const QR_DEBIT: ColumnId = ColumnId(6);
/// Credit.
pub const QR_CREDIT: ColumnId = ColumnId(7);

/// The quick report grid: read-only lines, newest first.
#[must_use]
pub fn report_spec() -> GridSpec {
    let money = |id, header| Column {
        kind: CellKind::Amount(AmountRules::MONEY),
        align: Align::Right,
        mono: true,
        ..column(id, header, 110, 0)
    };
    let mut s = GridSpec::browse(vec![
        Column {
            kind: CellKind::Date,
            mono: true,
            ..column(QR_DATE, "lists.qr.col.date", 100, 0)
        },
        column(QR_TYPE, "lists.qr.col.type", 130, 0),
        column(QR_NUMBER, "lists.qr.col.number", 80, 0),
        column(QR_NAME, "lists.qr.col.name", 180, 1),
        column(QR_MEMO, "lists.qr.col.memo", 200, 2),
        money(QR_DEBIT, "lists.qr.col.debit"),
        money(QR_CREDIT, "lists.qr.col.credit"),
    ]);
    s.height = RowHeight::Uniform { two_lines: false };
    s.selection = SelectionMode::None;
    s
}

/// The i18n key of an engine transaction type (`journal_entry`, ...).
#[must_use]
pub fn txn_type_key(kind: &str) -> &'static str {
    match kind {
        "invoice" => "lists.qr.type.invoice",
        "customer_payment" | "payment" => "lists.qr.type.payment",
        "cash_sale" => "lists.qr.type.sales_receipt",
        "expense" | "check" => "lists.qr.type.check",
        "card_charge" => "lists.qr.type.card_charge",
        "deposit" => "lists.qr.type.deposit",
        "transfer" => "lists.qr.type.transfer",
        "journal_entry" | "aje" => "lists.qr.type.journal_entry",
        "credit_memo" => "lists.qr.type.credit_memo",
        "customer_refund" => "lists.qr.type.refund",
        "bill" => "lists.qr.type.bill",
        "vendor_credit" => "lists.qr.type.vendor_credit",
        "bill_payment" => "lists.qr.type.bill_payment",
        "daily_sales" => "lists.qr.type.daily_sales",
        _ => "lists.qr.type.other",
    }
}

/// The day of a report row: plain dates as sent; the engine's local
/// midnight read in this computer's time zone (the engine runs here).
#[must_use]
pub fn report_day(d: ReportDate) -> Option<CalendarDate> {
    use chrono::Datelike;
    match d {
        ReportDate::Day(day) => Some(day),
        ReportDate::Midnight(ts) => {
            let local = ts.as_utc().with_timezone(&chrono::Local).date_naive();
            CalendarDate::from_ymd(local.year(), local.month(), local.day()).ok()
        }
    }
}

/// The quick report lines as the grid shows them.
#[derive(Debug, Default)]
pub struct ReportModel {
    lines: Vec<AccountReportLine>,
    days: Vec<Option<CalendarDate>>,
    types: Vec<String>,
    revision: u64,
}

impl ReportModel {
    /// Shows `report`.
    pub fn set(&mut self, report: AccountReport) {
        self.days = report.data.iter().map(|l| report_day(l.txn_date)).collect();
        self.types = report
            .data
            .iter()
            .map(|l| t(txn_type_key(&l.txn_type)))
            .collect();
        self.lines = report.data;
        self.revision += 1;
    }

    /// Empties the report.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.days.clear();
        self.types.clear();
        self.revision += 1;
    }

    /// Lines shown.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// No lines.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

impl RowModel for ReportModel {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.lines.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.lines.len()).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        let i = usize::try_from(key.0.checked_sub(1)?).ok()?;
        (i < self.lines.len()).then_some(i)
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(l) = self.lines.get(index) else {
            return CellValue::Empty;
        };
        let opt = |s: &Option<String>| {
            s.as_deref().map_or(CellValue::Empty, |v| {
                CellValue::Text(Cow::Owned(v.to_string()))
            })
        };
        match col {
            QR_DATE => self.days[index].map_or(CellValue::Empty, CellValue::Date),
            QR_TYPE => CellValue::Text(Cow::Borrowed(&self.types[index])),
            QR_NUMBER => opt(&l.txn_number),
            QR_NAME => opt(&l.payee),
            QR_MEMO => opt(&l.memo),
            QR_DEBIT if !l.debit.is_zero() => CellValue::Money(l.debit),
            QR_CREDIT if !l.credit.is_zero() => CellValue::Money(l.credit),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let day = self
            .days
            .get(index)
            .copied()
            .flatten()
            .map(crate::shell::us_date)
            .unwrap_or_default();
        let kind = self.types.get(index).cloned().unwrap_or_default();
        format!("{day}, {kind}")
    }
}
