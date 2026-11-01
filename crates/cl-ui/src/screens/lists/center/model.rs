//! The grid models of a center: the contact list (one row per client or
//! vendor, the engine's open balance from its balance report) and a
//! contact's transactions. Neither ever adds amounts up (non-negotiable 7).

use std::borrow::Cow;
use std::collections::HashMap;

use cl_api::{Contact, ContactType, ReportAmount, TransactionSummary};
use cl_core::ids::ContactId;
use cl_core::{MoneyStyle, NegativeStyle};
use cl_grid::{
    Align, CellKind, CellValue, Column, ColumnId, GridSpec, Line, RowHeight, RowKey, RowKeyGen,
    RowModel, RowState, SelectionMode, TextKey, WidthSpec,
};
use cl_model::amount_entry::AmountRules;
use cl_model::lists::contacts::{ContactEntry, ContactFilter, ContactList, Role, Roles};

use crate::i18n::t;

/// Name.
pub const COL_NAME: ColumnId = ColumnId(1);
/// Company name.
pub const COL_COMPANY: ColumnId = ColumnId(2);
/// Phone.
pub const COL_PHONE: ColumnId = ColumnId(3);
/// The engine's open balance from its balance report.
pub const COL_BALANCE: ColumnId = ColumnId(4);
/// Both roles / inactive marks.
pub const COL_STATUS: ColumnId = ColumnId(5);

fn column(id: ColumnId, header: &'static str, w: u16, flex: u8) -> Column {
    Column {
        id,
        header: TextKey(header),
        placeholder: None,
        empty: None,
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

fn money_column(id: ColumnId, header: &'static str, help: &'static str, w: u16) -> Column {
    Column {
        kind: CellKind::Amount(AmountRules::MONEY),
        align: Align::Right,
        mono: true,
        help: Some(TextKey(help)),
        ..column(id, header, w, 0)
    }
}

/// The list grid: read-only, one line per contact, one row selected.
/// `compact` (the add/edit panel is open) keeps name and balance only.
#[must_use]
pub fn list_spec(role: Role, compact: bool) -> GridSpec {
    let balance = match role {
        Role::Client => money_column(
            COL_BALANCE,
            "center.col.balance_client",
            "center.col.balance_client_help",
            110,
        ),
        Role::Vendor => money_column(
            COL_BALANCE,
            "center.col.balance_vendor",
            "center.col.balance_vendor_help",
            110,
        ),
    };
    let mut cols = vec![column(COL_NAME, "center.col.name", 180, 2)];
    if !compact {
        cols.push(column(COL_COMPANY, "center.col.company", 120, 1));
        cols.push(column(COL_PHONE, "center.col.phone", 112, 0));
    }
    cols.push(balance);
    if !compact {
        cols.push(column(COL_STATUS, "center.col.status", 130, 0));
    }
    let mut s = GridSpec::browse(cols);
    s.height = RowHeight::Uniform { two_lines: false };
    s.selection = SelectionMode::Single;
    s
}

/// The roles of an engine contact type.
#[must_use]
pub fn roles_of(t: ContactType) -> Roles {
    match t {
        ContactType::Customer => Roles::Client,
        ContactType::Vendor => Roles::Vendor,
        ContactType::Both => Roles::Both,
    }
}

/// The engine contact type of roles.
#[must_use]
pub fn api_roles(r: Roles) -> ContactType {
    match r {
        Roles::Client => ContactType::Customer,
        Roles::Vendor => ContactType::Vendor,
        Roles::Both => ContactType::Both,
    }
}

/// The list entry of an engine contact.
#[must_use]
pub fn entry_of(c: &Contact) -> ContactEntry {
    ContactEntry {
        id: c.id,
        roles: roles_of(c.fields.contact_type),
        name: c.fields.display_name.clone(),
        company: c.fields.company_name.clone(),
        email: c.fields.email.clone(),
        phone: c.fields.phone.clone(),
        active: c.is_active,
    }
}

/// An engine report figure as text: rounded to cents and laid out like
/// money (thousands separators, `$`, the negative style), but never turned
/// into `Money`, compared or summed (the JSON may be a float until U-14).
#[must_use]
pub fn report_text(a: &ReportAmount, style: MoneyStyle) -> Option<String> {
    Some(layout_plain(&a.display(2)?, style))
}

/// Plain amount text (`-1084.66`) laid out like money: thousands
/// separators, `$`, the negative style. Text only; no arithmetic.
#[must_use]
pub fn layout_plain(shown: &str, style: MoneyStyle) -> String {
    let (negative, digits) = match shown.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, shown),
    };
    let (whole, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let mut body = String::new();
    if style.symbol {
        body.push('$');
    }
    let n = whole.len();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (n - i) % 3 == 0 {
            body.push(',');
        }
        body.push(c);
    }
    if !frac.is_empty() {
        body.push('.');
        body.push_str(frac);
    }
    match (negative, style.negative) {
        (false, _) => body,
        (true, NegativeStyle::Minus) => format!("-{body}"),
        (true, NegativeStyle::Parens) => format!("({body})"),
    }
}

/// True when a report figure shows as zero at cents (display text only).
#[must_use]
pub fn report_is_zero(a: &ReportAmount) -> bool {
    a.display(2)
        .is_some_and(|s| s.bytes().all(|b| matches!(b, b'0' | b'.')))
}

/// A contact's open balance as the report gave it.
#[derive(Debug, Clone, Copy)]
pub enum Balance<'a> {
    /// The report has not arrived.
    Loading,
    /// The report leaves the contact out (nothing open, or not reported).
    NotInReport,
    /// The report's figure.
    Report(&'a ReportAmount),
}

/// The contacts of one center as the grid shows them.
#[derive(Debug)]
pub struct ListModel {
    role: Role,
    list: ContactList,
    visible: Vec<usize>,
    keys: HashMap<ContactId, RowKey>,
    keygen: RowKeyGen,
    row_keys: Vec<RowKey>,
    by_key: HashMap<RowKey, usize>,
    /// Engine open balance per contact (display only, never converted).
    balances: HashMap<ContactId, ReportAmount>,
    /// How money is shown (the grid's style).
    style: MoneyStyle,
    /// The balance report has arrived (absent then means nothing open).
    balances_loaded: bool,
    status: Vec<String>,
    revision: u64,
}

impl ListModel {
    /// An empty list for `role`'s center.
    #[must_use]
    pub fn new(role: Role) -> ListModel {
        ListModel {
            role,
            list: ContactList::default(),
            visible: Vec::new(),
            keys: HashMap::new(),
            keygen: RowKeyGen::default(),
            row_keys: Vec::new(),
            by_key: HashMap::new(),
            balances: HashMap::new(),
            style: MoneyStyle::default(),
            balances_loaded: false,
            status: Vec::new(),
            revision: 0,
        }
    }

    /// The contacts, filtered.
    pub fn set_contacts(&mut self, contacts: &[Contact], filter: &ContactFilter) {
        self.list = ContactList::new(contacts.iter().map(entry_of).collect());
        let (both, inactive) = (t("center.status.both"), t("center.status.inactive"));
        self.status = self
            .list
            .entries()
            .iter()
            .map(|e| {
                let mut marks: Vec<&str> = Vec::new();
                if e.roles == Roles::Both {
                    marks.push(&both);
                }
                if !e.active {
                    marks.push(&inactive);
                }
                marks.join(" \u{b7} ")
            })
            .collect();
        self.apply_filter(filter);
    }

    /// The engine's open balances (display only).
    pub fn set_balances(&mut self, balances: HashMap<ContactId, ReportAmount>) {
        self.balances = balances;
        self.balances_loaded = true;
        self.revision += 1;
    }

    /// Re-applies the filter (keys stay with their contacts).
    pub fn apply_filter(&mut self, filter: &ContactFilter) {
        self.visible = self.list.visible(self.role, filter);
        self.row_keys.clear();
        self.by_key.clear();
        for (i, r) in self.visible.iter().enumerate() {
            let id = self.list.entries()[*r].id;
            let key = *self
                .keys
                .entry(id)
                .or_insert_with(|| self.keygen.next_key());
            self.row_keys.push(key);
            self.by_key.insert(key, i);
        }
        self.revision += 1;
    }

    /// Every loaded contact.
    #[must_use]
    pub fn list(&self) -> &ContactList {
        &self.list
    }

    /// The contact on a row key.
    #[must_use]
    pub fn contact_at(&self, key: RowKey) -> Option<&ContactEntry> {
        let i = *self.by_key.get(&key)?;
        self.list.entries().get(self.visible[i])
    }

    /// The row key of a contact, when it is shown.
    #[must_use]
    pub fn key_of(&self, id: ContactId) -> Option<RowKey> {
        let key = *self.keys.get(&id)?;
        self.by_key.contains_key(&key).then_some(key)
    }

    /// The first shown contact whose name starts with `text`.
    #[must_use]
    pub fn find_prefix(&self, text: &str) -> Option<RowKey> {
        let q = text.to_lowercase();
        if q.is_empty() {
            return None;
        }
        self.visible.iter().enumerate().find_map(|(i, r)| {
            let e = self.list.entries().get(*r)?;
            e.name
                .to_lowercase()
                .starts_with(&q)
                .then(|| self.row_keys[i])
        })
    }

    /// How money is shown; the grid redraws when it changes.
    pub fn set_style(&mut self, style: MoneyStyle) {
        if style != self.style {
            self.style = style;
            self.revision += 1;
        }
    }

    /// The engine's open balance of `id`, as the report gave it.
    #[must_use]
    pub fn balance(&self, id: ContactId) -> Balance<'_> {
        if !self.balances_loaded {
            return Balance::Loading;
        }
        self.balances
            .get(&id)
            .map_or(Balance::NotInReport, Balance::Report)
    }

    /// The open balance of `id` as text; `None` while loading or when the
    /// report leaves the contact out.
    #[must_use]
    pub fn balance_text(&self, id: ContactId) -> Option<String> {
        match self.balance(id) {
            Balance::Report(a) => report_text(a, self.style),
            Balance::Loading | Balance::NotInReport => None,
        }
    }

    /// The balance report has arrived.
    #[must_use]
    pub fn balances_loaded(&self) -> bool {
        self.balances_loaded
    }

    /// Contacts shown.
    #[must_use]
    pub fn shown(&self) -> usize {
        self.visible.len()
    }
}

impl RowModel for ListModel {
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
        let Some(e) = self.list.entries().get(r) else {
            return CellValue::Empty;
        };
        let opt = |s: &Option<String>| {
            s.as_deref().map_or(CellValue::Empty, |v| {
                CellValue::Text(Cow::Owned(v.to_string()))
            })
        };
        match col {
            COL_NAME => CellValue::Text(Cow::Borrowed(&e.name)),
            COL_COMPANY => opt(&e.company),
            COL_PHONE => opt(&e.phone),
            COL_BALANCE if e.active || self.role == Role::Vendor => self
                .balance_text(e.id)
                .map_or(CellValue::Empty, |text| CellValue::Text(Cow::Owned(text))),
            COL_STATUS => CellValue::Text(Cow::Borrowed(&self.status[r])),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let Some(e) = self
            .visible
            .get(index)
            .and_then(|r| self.list.entries().get(*r))
        else {
            return String::new();
        };
        let mut label = e.name.clone();
        if e.roles == Roles::Both {
            label.push_str(", ");
            label.push_str(&t("center.status.both"));
        }
        if !e.active {
            label.push_str(", ");
            label.push_str(&t("center.status.inactive"));
        }
        label
    }
}

/// Date.
pub const H_DATE: ColumnId = ColumnId(1);
/// Transaction type.
pub const H_TYPE: ColumnId = ColumnId(2);
/// Number.
pub const H_NUMBER: ColumnId = ColumnId(3);
/// Memo.
pub const H_MEMO: ColumnId = ColumnId(4);
/// The engine's amount.
pub const H_AMOUNT: ColumnId = ColumnId(5);

/// The transactions grid: read-only, newest first.
#[must_use]
pub fn history_spec() -> GridSpec {
    let mut s = GridSpec::browse(vec![
        Column {
            kind: CellKind::Date,
            mono: true,
            ..column(H_DATE, "center.history.col.date", 88, 0)
        },
        column(H_TYPE, "center.history.col.type", 100, 0),
        column(H_NUMBER, "center.history.col.number", 84, 0),
        column(H_MEMO, "center.history.col.memo", 100, 2),
        Column {
            kind: CellKind::Amount(AmountRules::MONEY),
            align: Align::Right,
            mono: true,
            ..column(H_AMOUNT, "center.history.col.amount", 104, 0)
        },
    ]);
    s.height = RowHeight::Uniform { two_lines: false };
    s.selection = SelectionMode::None;
    s
}

/// The i18n key of a transaction type.
#[must_use]
pub fn txn_key(t: cl_api::TxnType) -> &'static str {
    use cl_api::TxnType as T;
    match t {
        T::Invoice => "lists.qr.type.invoice",
        T::CustomerPayment => "lists.qr.type.payment",
        T::CashSale => "lists.qr.type.sales_receipt",
        T::Expense => "lists.qr.type.check",
        T::Deposit => "lists.qr.type.deposit",
        T::Transfer => "lists.qr.type.transfer",
        T::JournalEntry | T::Aje => "lists.qr.type.journal_entry",
        T::CreditMemo => "lists.qr.type.credit_memo",
        T::CustomerRefund => "lists.qr.type.refund",
        T::Bill => "lists.qr.type.bill",
        T::VendorCredit => "lists.qr.type.vendor_credit",
        T::BillPayment => "lists.qr.type.bill_payment",
        T::DailySales => "lists.qr.type.daily_sales",
        T::ReturnedPayment => "lists.qr.type.returned_payment",
    }
}

/// A contact's transactions as the grid shows them.
#[derive(Debug, Default)]
pub struct HistoryModel {
    rows: Vec<TransactionSummary>,
    types: Vec<String>,
    revision: u64,
}

impl HistoryModel {
    /// Shows `rows`.
    pub fn set(&mut self, rows: Vec<TransactionSummary>) {
        self.types = rows.iter().map(|r| t(txn_key(r.txn_type))).collect();
        self.rows = rows;
        self.revision += 1;
    }

    /// Rows shown.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// No rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The transaction of row `key`.
    #[must_use]
    pub fn row_at(&self, key: RowKey) -> Option<&TransactionSummary> {
        self.index_of(key).and_then(|i| self.rows.get(i))
    }
}

/// The form a history row opens in on Enter or double-click (P21: a
/// customer payment opens in Receive Payment, where it can be edited,
/// voided or bounced).
#[must_use]
pub fn history_form(t: cl_api::TxnType) -> Option<&'static str> {
    match t {
        cl_api::TxnType::CustomerPayment => Some(crate::screens::receive_payment::KIND),
        _ => None,
    }
}

impl RowModel for HistoryModel {
    fn revision(&self) -> u64 {
        self.revision
    }
    fn row_count(&self) -> usize {
        self.rows.len()
    }
    fn row_key(&self, index: usize) -> Option<RowKey> {
        (index < self.rows.len()).then(|| RowKey(index as u64 + 1))
    }
    fn index_of(&self, key: RowKey) -> Option<usize> {
        let i = usize::try_from(key.0.checked_sub(1)?).ok()?;
        (i < self.rows.len()).then_some(i)
    }
    fn row_state(&self, _index: usize) -> RowState {
        RowState::empty()
    }
    fn cell(&self, index: usize, col: ColumnId) -> CellValue<'_> {
        let Some(r) = self.rows.get(index) else {
            return CellValue::Empty;
        };
        let opt = |s: &Option<String>| {
            s.as_deref().map_or(CellValue::Empty, |v| {
                CellValue::Text(Cow::Owned(v.to_string()))
            })
        };
        match col {
            H_DATE => CellValue::Date(r.txn_date),
            H_TYPE => CellValue::Text(Cow::Borrowed(&self.types[index])),
            H_NUMBER => opt(&r.txn_number),
            H_MEMO => opt(&r.memo),
            // The engine's own amount column (the total, or its debit sum).
            H_AMOUNT => r
                .display_total
                .or(r.total)
                .map_or(CellValue::Empty, CellValue::Money),
            _ => CellValue::Empty,
        }
    }
    fn row_label(&self, index: usize) -> String {
        let Some(r) = self.rows.get(index) else {
            return String::new();
        };
        let kind = self.types.get(index).cloned().unwrap_or_default();
        format!("{}, {kind}", crate::shell::us_date(r.txn_date))
    }
}
