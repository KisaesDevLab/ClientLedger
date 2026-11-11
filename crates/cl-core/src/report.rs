//! Reports as data (P28 plan §0, §4): what is asked of the engine
//! ([`ReportRequest`]) and the neutral table every report becomes
//! ([`ReportTable`]), which the report window, collapse, drill-down, the
//! CSV, xlsx and PDF exports and the golden files all read without knowing
//! which report it is.
//!
//! The rows follow ADD-03's U-119 run rows (`kind`, `level`, `caption`,
//! `cells`, `drill`, `expandable`), so the report builder's output is one
//! more adapter.
//!
//! The engine computes every amount, subtotal, percent and roll-up; the
//! table only carries them. Nothing here adds, subtracts or divides money:
//! amounts are the engine's exact decimal strings parsed at full scale,
//! and [`display_amount`] rounds a copy for the screen only.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

use crate::dates::CalendarDate;
use crate::ids::{AccountId, ContactId, ItemId, TagGroupId, TagId, TransactionId};
use crate::money::{MoneyStyle, NegativeStyle};

/// Accrual or cash (Q-308: some reports have no basis).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// Cash basis (`both` and `cash` entries).
    Cash,
    /// Accrual basis (`both` and `accrual` entries).
    Accrual,
}

impl Basis {
    /// The engine's query value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Basis::Cash => "cash",
            Basis::Accrual => "accrual",
        }
    }
}

/// The engine routes the report window reads (plan §5), one per family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(
    missing_docs,
    reason = "one variant per engine report route, named after it"
)]
pub enum Source {
    ProfitLoss,
    BalanceSheet,
    TrialBalance,
    GeneralLedger,
    Journal,
    TransactionList,
    BudgetVsActual,
    ArAgingSummary,
    ArAgingDetail,
    InvoiceList,
    CustomerBalanceSummary,
    CustomerBalanceDetail,
    VendorBalanceSummary,
    ApAgingSummary,
    ApAgingDetail,
    UnpaidBills,
    VendorStatement,
    SalesByCustomer,
    SalesByItem,
    RevenueByCategory,
    ExpenseByCategory,
    ExpenseByVendor,
    TransactionListByVendor,
    BillPaymentHistory,
    SalesTaxLiability,
    Vendor1099Summary,
    Ap1099Prep,
    DepositDetail,
    CheckRegister,
    BankBalances,
    AccountActivitySummary,
    InventoryValuation,
    InventoryValuationDetail,
    InventoryStockStatus,
    AuditLog,
    ClosingDateExceptions,
    Duplicates,
}

impl Source {
    /// Every source, in catalog order.
    pub const ALL: [Source; 37] = [
        Source::ProfitLoss,
        Source::BalanceSheet,
        Source::TrialBalance,
        Source::GeneralLedger,
        Source::Journal,
        Source::TransactionList,
        Source::BudgetVsActual,
        Source::ArAgingSummary,
        Source::ArAgingDetail,
        Source::InvoiceList,
        Source::CustomerBalanceSummary,
        Source::CustomerBalanceDetail,
        Source::VendorBalanceSummary,
        Source::ApAgingSummary,
        Source::ApAgingDetail,
        Source::UnpaidBills,
        Source::VendorStatement,
        Source::SalesByCustomer,
        Source::SalesByItem,
        Source::RevenueByCategory,
        Source::ExpenseByCategory,
        Source::ExpenseByVendor,
        Source::TransactionListByVendor,
        Source::BillPaymentHistory,
        Source::SalesTaxLiability,
        Source::Vendor1099Summary,
        Source::Ap1099Prep,
        Source::DepositDetail,
        Source::CheckRegister,
        Source::BankBalances,
        Source::AccountActivitySummary,
        Source::InventoryValuation,
        Source::InventoryValuationDetail,
        Source::InventoryStockStatus,
        Source::AuditLog,
        Source::ClosingDateExceptions,
        Source::Duplicates,
    ];

    /// The engine route (`/api/v1/...`). Budget vs. Actual's `:id` is the
    /// request's budget.
    #[must_use]
    pub const fn route(self) -> &'static str {
        match self {
            Source::ProfitLoss => "/api/v1/reports/profit-loss",
            Source::BalanceSheet => "/api/v1/reports/balance-sheet",
            Source::TrialBalance => "/api/v1/reports/trial-balance",
            // The Journal is the ledger grouped by transaction (U-202).
            Source::GeneralLedger | Source::Journal => "/api/v1/reports/general-ledger",
            Source::TransactionList => "/api/v1/reports/transaction-list",
            Source::BudgetVsActual => "/api/v1/budgets/:id/vs-actual",
            Source::ArAgingSummary => "/api/v1/reports/ar-aging-summary",
            Source::ArAgingDetail => "/api/v1/reports/ar-aging-detail",
            Source::InvoiceList => "/api/v1/reports/invoice-list",
            Source::CustomerBalanceSummary => "/api/v1/reports/customer-balance-summary",
            Source::CustomerBalanceDetail => "/api/v1/reports/customer-balance-detail",
            Source::VendorBalanceSummary => "/api/v1/reports/vendor-balance-summary",
            Source::ApAgingSummary => "/api/v1/reports/ap-aging-summary",
            Source::ApAgingDetail => "/api/v1/reports/ap-aging-detail",
            Source::UnpaidBills => "/api/v1/reports/unpaid-bills",
            Source::VendorStatement => "/api/v1/reports/vendor-statement",
            Source::SalesByCustomer => "/api/v1/reports/sales-by-customer",
            Source::SalesByItem => "/api/v1/reports/sales-by-item",
            Source::RevenueByCategory => "/api/v1/reports/revenue-by-category",
            Source::ExpenseByCategory => "/api/v1/reports/expense-by-category",
            Source::ExpenseByVendor => "/api/v1/reports/expense-by-vendor",
            Source::TransactionListByVendor => "/api/v1/reports/transaction-list-by-vendor",
            Source::BillPaymentHistory => "/api/v1/reports/bill-payment-history",
            Source::SalesTaxLiability => "/api/v1/reports/sales-tax-liability",
            Source::Vendor1099Summary => "/api/v1/reports/vendor-1099-summary",
            Source::Ap1099Prep => "/api/v1/reports/ap-1099-prep",
            Source::DepositDetail => "/api/v1/reports/deposit-detail",
            Source::CheckRegister => "/api/v1/reports/check-register",
            Source::BankBalances => "/api/v1/reports/bank-balances",
            Source::AccountActivitySummary => "/api/v1/reports/account-activity-summary",
            Source::InventoryValuation => "/api/v1/reports/inventory-valuation",
            Source::InventoryValuationDetail => "/api/v1/reports/inventory-valuation-detail",
            Source::InventoryStockStatus => "/api/v1/reports/inventory-stock-status",
            Source::AuditLog => "/api/v1/audit-log",
            Source::ClosingDateExceptions => "/api/v1/reports/closing-date-exceptions",
            Source::Duplicates => "/api/v1/duplicates",
        }
    }
}

/// An inclusive date range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DateRange {
    /// First day.
    pub from: CalendarDate,
    /// Last day.
    pub to: CalendarDate,
}

/// The dates a report runs on, always explicit (4-4-5 ranges are sent as
/// ranges, never as a count).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportDates {
    /// A period.
    Range(DateRange),
    /// A balance date.
    AsOf(CalendarDate),
    /// Period columns (months, fiscal periods, quarters, custom), up to 36;
    /// on a balance report each range's end is its as-of date.
    Ranges(Vec<DateRange>),
    /// Lists with no dates.
    None,
}

impl ReportDates {
    /// The first and last day covered (`None` for [`ReportDates::None`];
    /// an as-of date is both).
    #[must_use]
    pub fn span(&self) -> Option<DateRange> {
        match self {
            ReportDates::Range(r) => Some(*r),
            ReportDates::AsOf(d) => Some(DateRange { from: *d, to: *d }),
            ReportDates::Ranges(v) => Some(DateRange {
                from: v.first()?.from,
                to: v.last()?.to,
            }),
            ReportDates::None => None,
        }
    }
}

/// Columns compared side by side (U-201).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "by")]
pub enum Compare {
    /// No compare columns.
    #[default]
    None,
    /// One column per client with activity.
    ByContact,
    /// One column per tag of the group.
    ByTag {
        /// The tag group.
        tag_group_id: TagGroupId,
    },
}

/// Extra columns the engine adds (U-201).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extra {
    /// % of income (P&L) beside each amount column.
    PctOfIncome,
    /// A year-to-date column.
    Ytd,
    /// The prior period and the % change.
    PriorPeriod,
    /// The same period last year and the % change.
    PriorYear,
}

/// Cleared state filter (U-202).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "the engine's cleared filter values")]
pub enum Cleared {
    Cleared,
    Uncleared,
    Reconciled,
}

/// Which entries count (the adjusting lens, U-202).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adjusting {
    /// Every entry.
    Include,
    /// Leave adjusting entries out.
    Exclude,
    /// Adjusting entries only.
    Only,
}

impl Adjusting {
    /// The engine's query value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Adjusting::Include => "include",
            Adjusting::Exclude => "exclude",
            Adjusting::Only => "only",
        }
    }
}

/// Report filters by id and kind (amounts are the owner's input, sent as
/// decimal strings; AI never sets them, Q-P28g).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFilters {
    /// Accounts (at most 500).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_ids: Vec<AccountId>,
    /// Clients or vendors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contact_ids: Vec<ContactId>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_ids: Vec<TagId>,
    /// Items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub item_ids: Vec<ItemId>,
    /// Smallest line amount (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_dec_str")]
    pub amount_min: Option<Decimal>,
    /// Largest line amount (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_dec_str")]
    pub amount_max: Option<Decimal>,
    /// Cleared state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared: Option<Cleared>,
    /// Transaction types (the engine's names).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub txn_types: Vec<String>,
    /// The adjusting lens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjusting: Option<Adjusting>,
    /// Lines without a tag of this group (U-202).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub untagged_in_group: Option<TagGroupId>,
}

/// What one report run asks of the engine: the resolved, concrete form of
/// the owner's options (relative presets already turned into dates).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRequest {
    /// The route family.
    pub source: Source,
    /// Explicit dates.
    pub dates: ReportDates,
    /// The basis, where the report has one.
    pub basis: Option<Basis>,
    /// Compare columns.
    pub columns: Compare,
    /// Extra columns.
    pub extras: BTreeSet<Extra>,
    /// Filters.
    pub filters: ReportFilters,
    /// Split each account by the tags of this group (U-201 step 5b).
    pub tag_split: Option<TagGroupId>,
    /// The fiscal year's first day (YTD columns).
    pub fiscal_year_start: Option<CalendarDate>,
    /// The budget (Budget vs. Actual).
    pub budget_id: Option<String>,
}

impl ReportRequest {
    /// A request for `source` on `dates` with nothing else set.
    #[must_use]
    pub fn new(source: Source, dates: ReportDates) -> ReportRequest {
        ReportRequest {
            source,
            dates,
            basis: None,
            columns: Compare::None,
            extras: BTreeSet::new(),
            filters: ReportFilters::default(),
            tag_split: None,
            fiscal_year_start: None,
            budget_id: None,
        }
    }
}

// ------------------------------------------------------------------ table

/// Decimals travel as their exact text (never a JSON number).
pub mod dec_str {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    /// Writes the exact text.
    ///
    /// # Errors
    /// The serializer's.
    pub fn serialize<S: Serializer>(d: &Decimal, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&d.to_string())
    }

    /// Reads exact text only.
    ///
    /// # Errors
    /// Text that is not a decimal.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Decimal, D::Error> {
        let s = String::deserialize(d)?;
        Decimal::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// [`dec_str`] for an optional decimal (`null` is `None`).
pub mod opt_dec_str {
    use rust_decimal::Decimal;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::str::FromStr;

    /// Writes the exact text or `null`.
    ///
    /// # Errors
    /// The serializer's.
    #[allow(clippy::ref_option, reason = "serde `with` passes a reference")]
    pub fn serialize<S: Serializer>(d: &Option<Decimal>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(d) => s.serialize_str(&d.to_string()),
            None => s.serialize_none(),
        }
    }

    /// Reads exact text or `null`.
    ///
    /// # Errors
    /// Text that is not a decimal.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Decimal>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| Decimal::from_str(&s).map_err(serde::de::Error::custom))
            .transpose()
    }
}

/// What a column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "column kinds, as named")]
pub enum ColumnKind {
    Amount,
    Percent,
    Quantity,
    Text,
    Date,
    Count,
}

impl ColumnKind {
    /// Right-aligned (numbers).
    #[must_use]
    pub const fn is_numeric(self) -> bool {
        matches!(
            self,
            ColumnKind::Amount | ColumnKind::Percent | ColumnKind::Quantity | ColumnKind::Count
        )
    }
}

/// One column heading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnHead {
    /// Stable key (`total`, `p1`, `ytd`, `pct_total`, ...).
    pub key: String,
    /// The heading as shown (owner text or a formatted period).
    pub caption: String,
    /// What it holds.
    pub kind: ColumnKind,
    /// The dates behind an amount column (drill-down uses them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<DateRange>,
    /// What a column of a split adds to a cell's drill (U-201: a client or
    /// tag column), or that its cells have none ($ and % change).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<ColumnDrill>,
}

/// A column's part of a cell's drill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnDrill {
    /// The lines of one client.
    Contact(ContactId),
    /// The lines of one tag, or without a tag of a group.
    Tag(TagScope),
    /// The column's cells have no lines behind them (a change, "All
    /// others", lines without a client).
    None,
}

/// What a row is (U-119 run-row kinds plus the tag split's).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowKind {
    /// A section heading (no amounts).
    Header,
    /// A list row (a transaction, a document, a client).
    Detail,
    /// An account row.
    Account,
    /// A tag row under its account (tag split).
    TagRow,
    /// An account's untagged plus tags (tag split, optional).
    AccountTotal,
    /// A section subtotal.
    Subtotal,
    /// A section total.
    Total,
    /// A grand line (gross profit, net income).
    Grand,
    /// A note line.
    Note,
}

impl RowKind {
    /// Totals stay where they are when rows are sorted (Q-P28k).
    #[must_use]
    pub const fn is_pinned(self) -> bool {
        matches!(
            self,
            RowKind::Header | RowKind::Subtotal | RowKind::Total | RowKind::Grand | RowKind::Note
        )
    }

    /// The canonical name (CSV, control channel).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RowKind::Header => "header",
            RowKind::Detail => "detail",
            RowKind::Account => "account",
            RowKind::TagRow => "tag",
            RowKind::AccountTotal => "account_total",
            RowKind::Subtotal => "subtotal",
            RowKind::Total => "total",
            RowKind::Grand => "grand",
            RowKind::Note => "note",
        }
    }
}

/// Where a row sits in the report's tree: its ancestors' keys and its own.
/// Collapsing a row hides the rows under its path (Q-P28j).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RowPath(pub Vec<String>);

impl RowPath {
    /// A path from its parts.
    #[must_use]
    pub fn of<I: IntoIterator<Item = S>, S: Into<String>>(parts: I) -> RowPath {
        RowPath(parts.into_iter().map(Into::into).collect())
    }

    /// The child path `self / key`.
    #[must_use]
    pub fn child(&self, key: impl Into<String>) -> RowPath {
        let mut v = self.0.clone();
        v.push(key.into());
        RowPath(v)
    }

    /// True when `self` lies strictly under `parent`.
    #[must_use]
    pub fn is_under(&self, parent: &RowPath) -> bool {
        self.0.len() > parent.0.len() && self.0.starts_with(&parent.0)
    }

    /// `a/b/c`, for display and the control channel.
    #[must_use]
    pub fn joined(&self) -> String {
        self.0.join("/")
    }
}

/// One cell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cell {
    /// Nothing.
    Empty,
    /// Text (names, memos, numbers of documents).
    Text(String),
    /// An exact amount from the engine.
    Amount(#[serde(with = "dec_str")] Decimal),
    /// A percent at the engine's 1 dp; `None` on a zero base (shown "—").
    Percent(#[serde(with = "opt_dec_str")] Option<Decimal>),
    /// A quantity (inventory).
    Quantity(#[serde(with = "dec_str")] Decimal),
    /// A date.
    Date(CalendarDate),
    /// A count.
    Count(u64),
}

/// How a hidden-row drill-down finds a tag (U-202).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagScope {
    /// Any tag.
    #[default]
    Any,
    /// Lines with this tag.
    Tag(TagId),
    /// Lines with no tag at all.
    Untagged,
    /// Lines with no tag of this group (Q-P28h).
    UntaggedInGroup(TagGroupId),
}

/// An aging bucket (Q-105).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs, reason = "the five aging buckets and the total")]
pub enum AgingBucket {
    Current,
    Days1To30,
    Days31To60,
    Days61To90,
    Over90,
    Total,
}

/// What clicking a cell opens (plan §4 drill rules).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Drill {
    /// The lines behind an amount: Transaction Detail by Account (the GL)
    /// filtered to them. A column with its own dates replaces `from`/`to`
    /// ([`Drill::at`]).
    Lines {
        /// Accounts (at most 500; a subtotal's section).
        account_ids: Vec<AccountId>,
        /// First day (`None`: from the first entry, for balances).
        from: Option<CalendarDate>,
        /// Last day.
        to: CalendarDate,
        /// One client (a client column).
        contact: Option<ContactId>,
        /// The tag scope (tag split rows).
        tag: TagScope,
        /// The report's basis.
        basis: Option<Basis>,
        /// The report's adjusting lens.
        adjusting: Option<Adjusting>,
    },
    /// One transaction's form.
    Transaction {
        /// The transaction.
        id: TransactionId,
        /// Its type, as the engine names it.
        txn_type: String,
    },
    /// The aging detail for one contact and bucket.
    Aging {
        /// The contact (`None`: the "(no client)" row).
        contact: Option<ContactId>,
        /// The bucket clicked.
        bucket: AgingBucket,
        /// As of.
        as_of: CalendarDate,
    },
    /// The Profit & Loss over a period: the Balance Sheet's current-year
    /// net income opens the fiscal year to date (plan §4).
    ProfitLoss {
        /// First day (the fiscal year's).
        from: CalendarDate,
        /// Last day (the balance date).
        to: CalendarDate,
        /// The basis the sheet ran on.
        basis: Option<Basis>,
    },
}

impl Drill {
    /// The drill of the cell in `column`, or `None` when the column's cells
    /// have no lines behind them: [`Drill::at`] plus the column's client or
    /// tag.
    #[must_use]
    pub fn for_cell(&self, column: &ColumnHead) -> Option<Drill> {
        let mut d = self.at(column);
        match (column.drill, &mut d) {
            (Some(ColumnDrill::None), _) => return None,
            (Some(ColumnDrill::Contact(c)), Drill::Lines { contact, .. }) => *contact = Some(c),
            (Some(ColumnDrill::Tag(t)), Drill::Lines { tag, .. }) => {
                // A row already scoped to a tag can't take a second one.
                if *tag != TagScope::Any {
                    return None;
                }
                *tag = t;
            }
            _ => {}
        }
        Some(d)
    }

    /// The drill for a cell in `column`: a column with its own dates
    /// narrows a `Lines` drill to them (a balance keeps `from: None`).
    #[must_use]
    pub fn at(&self, column: &ColumnHead) -> Drill {
        match (self, column.range) {
            (
                Drill::Lines {
                    account_ids,
                    from,
                    contact,
                    tag,
                    basis,
                    adjusting,
                    ..
                },
                Some(r),
            ) => Drill::Lines {
                account_ids: account_ids.clone(),
                from: from.map(|_| r.from),
                to: r.to,
                contact: *contact,
                tag: *tag,
                basis: *basis,
                adjusting: *adjusting,
            },
            (Drill::Aging { contact, as_of, .. }, _) => match AgingBucket::of_column(&column.key) {
                Some(bucket) => Drill::Aging {
                    contact: *contact,
                    bucket,
                    as_of: *as_of,
                },
                None => self.clone(),
            },
            _ => self.clone(),
        }
    }
}

impl AgingBucket {
    /// The aging columns' keys, in order.
    pub const KEYS: [&'static str; 6] = [
        "current",
        "days_1_30",
        "days_31_60",
        "days_61_90",
        "over_90",
        "total",
    ];

    /// The bucket of an aging column key.
    #[must_use]
    pub fn of_column(key: &str) -> Option<AgingBucket> {
        Some(match key {
            "current" => AgingBucket::Current,
            "days_1_30" => AgingBucket::Days1To30,
            "days_31_60" => AgingBucket::Days31To60,
            "days_61_90" => AgingBucket::Days61To90,
            "over_90" => AgingBucket::Over90,
            "total" => AgingBucket::Total,
            _ => return None,
        })
    }
}

/// One row of a report.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportRow {
    /// What it is.
    pub kind: RowKind,
    /// Indent level (0 = section).
    pub level: u8,
    /// Its place in the tree.
    pub path: RowPath,
    /// The first column's text. A caption starting with `@` is an i18n key
    /// the window translates (fixed labels such as "Total"); anything else
    /// is owner data shown as is.
    pub caption: String,
    /// One cell per [`ReportTable::columns`] entry.
    pub cells: Vec<Cell>,
    /// What clicking an amount opens (`None`: no drill, e.g. a hidden
    /// account, Retained earnings).
    pub drill: Option<Drill>,
    /// Rows lie under it (it can collapse).
    pub expandable: bool,
}

/// A row-count cap the engine applied (Q-P28l).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Truncated {
    /// Rows sent.
    pub shown: u64,
    /// Rows there are.
    pub total: u64,
}

/// What the report says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportMeta {
    /// The basis it ran on.
    pub basis: Option<Basis>,
    /// The dates it ran on.
    pub dates: Option<ReportDates>,
    /// The engine's cash-basis rules note (Q-P23z).
    pub cash_basis_rules: Option<String>,
    /// Rows were capped.
    pub truncated: Option<Truncated>,
    /// Rows of accounts the user can't see were left out (Q-157 hook).
    pub hidden_notice: bool,
}

/// A report as data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportTable {
    /// The columns after the caption column.
    pub columns: Vec<ColumnHead>,
    /// The rows in order.
    pub rows: Vec<ReportRow>,
    /// About the run.
    pub meta: ReportMeta,
}

impl ReportTable {
    /// The rows still shown when the rows at `collapsed` paths are folded:
    /// everything under a collapsed row is hidden; the collapsed row and
    /// its total stay (the engine's roll-up, Q-P28j). Indexes into `rows`.
    #[must_use]
    pub fn visible(&self, collapsed: &[RowPath]) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, r)| !collapsed.iter().any(|c| r.path.is_under(c)))
            .map(|(i, _)| i)
            .collect()
    }

    /// The canonical CSV of the golden files (plan §6): a header line of
    /// `kind,level,path,caption` and the column keys, then one line per
    /// row with the engine's exact strings (amounts unrounded, percents at
    /// 1 dp, null percents empty, dates `YYYY-MM-DD`; trailing zeros dropped). No run time, no
    /// formatting: equal tables give equal text.
    #[must_use]
    pub fn to_canonical_csv(&self) -> String {
        let mut out = String::new();
        let mut head = vec![
            "kind".to_string(),
            "level".into(),
            "path".into(),
            "caption".into(),
        ];
        head.extend(self.columns.iter().map(|c| c.key.clone()));
        push_csv_line(&mut out, &head);
        for r in &self.rows {
            let mut line = vec![
                r.kind.as_str().to_string(),
                r.level.to_string(),
                r.path.joined(),
                r.caption.clone(),
            ];
            line.extend(r.cells.iter().map(canonical_cell));
            push_csv_line(&mut out, &line);
        }
        out
    }
}

/// A cell's canonical text.
#[must_use]
pub fn canonical_cell(c: &Cell) -> String {
    match c {
        Cell::Empty | Cell::Percent(None) => String::new(),
        Cell::Text(s) => s.clone(),
        Cell::Amount(d) | Cell::Quantity(d) | Cell::Percent(Some(d)) => canonical_decimal(*d),
        Cell::Date(d) => d.to_wire(),
        Cell::Count(n) => n.to_string(),
    }
}

/// A decimal's canonical text: no trailing zeros, no negative zero, so an
/// engine string and the same value read through `Money` agree.
#[must_use]
pub fn canonical_decimal(d: Decimal) -> String {
    if d.is_zero() {
        return "0".to_string();
    }
    d.normalize().to_string()
}

/// Appends one CSV line (RFC 4180 quoting) ending in `\n`.
pub fn push_csv_line(out: &mut String, cells: &[String]) {
    for (i, c) in cells.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if c.contains([',', '"', '\n', '\r']) {
            out.push('"');
            out.push_str(&c.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(c);
        }
    }
    out.push('\n');
}

/// An amount as the screen, PDF and print show it: 2 dp, half away from
/// zero, thousands separators. Display only (plan §6): the exact value
/// stays in the table.
#[must_use]
pub fn display_amount(d: Decimal, style: MoneyStyle) -> String {
    let r = d.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    let neg = r.is_sign_negative() && !r.is_zero();
    let abs = r.abs();
    let whole = abs.trunc();
    let frac = abs - whole;
    let mut body = String::new();
    if style.symbol {
        body.push('$');
    }
    body.push_str(&crate::money::group_thousands(
        u128::try_from(whole.mantissa()).unwrap_or(0) / 10u128.pow(whole.scale()),
    ));
    let mut cents = frac;
    cents.rescale(2);
    let _ = write!(body, ".{:02}", cents.mantissa());
    match (neg, style.negative) {
        (false, _) => body,
        (true, NegativeStyle::Minus) => format!("-{body}"),
        (true, NegativeStyle::Parens) => format!("({body})"),
    }
}

/// A percent as shown: `63.7%`; `—` on a zero base.
#[must_use]
pub fn display_percent(p: Option<Decimal>) -> String {
    match p {
        Some(d) => {
            let r = d.round_dp_with_strategy(1, RoundingStrategy::MidpointAwayFromZero);
            let mut s = r;
            s.rescale(1);
            format!("{s}%")
        }
        None => "\u{2014}".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Decimal {
        Decimal::from_str(s).expect("decimal")
    }

    fn day(y: i32, m: u32, dd: u32) -> CalendarDate {
        CalendarDate::from_ymd(y, m, dd).expect("date")
    }

    fn table() -> ReportTable {
        let income = RowPath::of(["income"]);
        ReportTable {
            columns: vec![
                ColumnHead {
                    key: "total".into(),
                    caption: "Oct 2026".into(),
                    kind: ColumnKind::Amount,
                    range: Some(DateRange {
                        from: day(2026, 10, 1),
                        to: day(2026, 10, 31),
                    }),
                    drill: None,
                },
                ColumnHead {
                    key: "pct_total".into(),
                    caption: "% of income".into(),
                    kind: ColumnKind::Percent,
                    range: None,
                    drill: None,
                },
            ],
            rows: vec![
                ReportRow {
                    kind: RowKind::Header,
                    level: 0,
                    path: income.clone(),
                    caption: "Income".into(),
                    cells: vec![Cell::Empty, Cell::Empty],
                    drill: None,
                    expandable: true,
                },
                ReportRow {
                    kind: RowKind::Account,
                    level: 1,
                    path: income.child("4000"),
                    caption: "4000 · Landscape, \"installation\"".into(),
                    cells: vec![
                        Cell::Amount(d("38420.0000")),
                        Cell::Percent(Some(d("63.7"))),
                    ],
                    drill: None,
                    expandable: false,
                },
                ReportRow {
                    kind: RowKind::Total,
                    level: 0,
                    path: RowPath::of(["income_total"]),
                    caption: "Total income".into(),
                    cells: vec![Cell::Amount(d("60280.00")), Cell::Percent(None)],
                    drill: None,
                    expandable: false,
                },
            ],
            meta: ReportMeta::default(),
        }
    }

    #[test]
    fn canonical_csv_keeps_the_exact_strings() {
        let csv = table().to_canonical_csv();
        assert_eq!(
            csv,
            "kind,level,path,caption,total,pct_total\n\
             header,0,income,Income,,\n\
             account,1,income/4000,\"4000 · Landscape, \"\"installation\"\"\",38420,63.7\n\
             total,0,income_total,Total income,60280,\n"
        );
    }

    #[test]
    fn collapse_hides_only_the_rows_under_the_path() {
        let t = table();
        assert_eq!(t.visible(&[]), vec![0, 1, 2]);
        assert_eq!(t.visible(&[RowPath::of(["income"])]), vec![0, 2]);
        assert!(!RowPath::of(["income"]).is_under(&RowPath::of(["income"])));
        assert!(!RowPath::of(["income_total"]).is_under(&RowPath::of(["income"])));
    }

    #[test]
    fn amounts_display_at_two_places_half_away_from_zero() {
        let plain = MoneyStyle::default();
        assert_eq!(display_amount(d("38420"), plain), "38,420.00");
        assert_eq!(display_amount(d("0.005"), plain), "0.01");
        assert_eq!(display_amount(d("-0.005"), plain), "-0.01");
        assert_eq!(display_amount(d("-0.004"), plain), "0.00");
        assert_eq!(display_amount(d("1234567.8949"), plain), "1,234,567.89");
        let parens = MoneyStyle {
            negative: NegativeStyle::Parens,
            symbol: true,
        };
        assert_eq!(display_amount(d("-12.5"), parens), "($12.50)");
    }

    #[test]
    fn percents_display_with_a_dash_for_no_base() {
        assert_eq!(display_percent(Some(d("63.7"))), "63.7%");
        assert_eq!(display_percent(Some(d("0"))), "0.0%");
        assert_eq!(display_percent(None), "\u{2014}");
    }

    #[test]
    fn a_column_narrows_a_lines_drill_to_its_dates() {
        let drill = Drill::Lines {
            account_ids: vec![],
            from: Some(day(2026, 1, 1)),
            to: day(2026, 10, 31),
            contact: None,
            tag: TagScope::Any,
            basis: Some(Basis::Accrual),
            adjusting: None,
        };
        let col = &table().columns[0];
        match drill.at(col) {
            Drill::Lines { from, to, .. } => {
                assert_eq!(from, Some(day(2026, 10, 1)));
                assert_eq!(to, day(2026, 10, 31));
            }
            other => panic!("{other:?}"),
        }
        // A balance drill keeps "from the first entry".
        let balance = Drill::Lines {
            account_ids: vec![],
            from: None,
            to: day(2026, 10, 31),
            contact: None,
            tag: TagScope::Any,
            basis: None,
            adjusting: None,
        };
        match balance.at(col) {
            Drill::Lines { from, .. } => assert_eq!(from, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_source_has_a_versioned_route() {
        for s in Source::ALL {
            assert!(s.route().starts_with("/api/v1/"), "{s:?}");
        }
        let mut seen = std::collections::HashSet::new();
        assert!(
            Source::ALL
                .iter()
                .filter(|s| **s != Source::Journal)
                .all(|s| seen.insert(s.route()))
        );
    }

    #[test]
    fn a_table_round_trips_through_json() {
        let t = table();
        let j = serde_json::to_string(&t).expect("json");
        let back: ReportTable = serde_json::from_str(&j).expect("back");
        assert_eq!(back, t);
    }
}
