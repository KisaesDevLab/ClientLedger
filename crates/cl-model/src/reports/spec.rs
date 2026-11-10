//! A report's static description (plan §4): what it reads, which dates,
//! basis, columns, extras and filters it offers, how it drills and which
//! engine capabilities it needs. The catalog is a table of these.

use cl_core::report::{Extra, Source};

use super::options::ColumnBy;
use super::presets::DatePreset;

/// The catalog's groups (mockup, plus Q-P28i's extras).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CatalogGroup {
    /// Company.
    Company,
    /// Who owes what.
    WhoOwesWhat,
    /// Sales & spending.
    SalesSpending,
    /// Banking (Q-P28i).
    Banking,
    /// Inventory (Q-P28i).
    Inventory,
    /// Controls (Q-P28i).
    Controls,
    /// The owner's memorized reports.
    Memorized,
}

impl CatalogGroup {
    /// The fixed groups in catalog order.
    pub const ALL: [CatalogGroup; 7] = [
        CatalogGroup::Company,
        CatalogGroup::WhoOwesWhat,
        CatalogGroup::SalesSpending,
        CatalogGroup::Banking,
        CatalogGroup::Inventory,
        CatalogGroup::Controls,
        CatalogGroup::Memorized,
    ];

    /// Its i18n key.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            CatalogGroup::Company => "reports.group.company",
            CatalogGroup::WhoOwesWhat => "reports.group.who_owes_what",
            CatalogGroup::SalesSpending => "reports.group.sales_spending",
            CatalogGroup::Banking => "reports.group.banking",
            CatalogGroup::Inventory => "reports.group.inventory",
            CatalogGroup::Controls => "reports.group.controls",
            CatalogGroup::Memorized => "reports.group.memorized",
        }
    }
}

/// Which dates a report takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateMode {
    /// From and To.
    Range,
    /// One as-of date.
    AsOf,
    /// A whole year (1099).
    Year,
    /// No dates.
    None,
}

/// Whether the basis toggle applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BasisMode {
    /// Accrual / Cash.
    Toggle,
    /// Open items: basis doesn't apply (Q-308 note).
    NotApplicable,
}

/// Filters a report offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs, reason = "filter kinds, as named")]
pub enum FilterKind {
    Accounts,
    Contacts,
    Tags,
    Items,
    Amount,
    Cleared,
    TxnTypes,
    Adjusting,
}

/// Sorting (Q-P28k).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    /// Not sortable (statements, running balances).
    None,
    /// Detail rows sort within their group; totals stay pinned.
    WithinGroups,
}

/// What clicking a cell opens (plan §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrillRule {
    /// Period amounts open the lines behind them.
    Lines,
    /// Balances open the GL from the first entry to the column's date.
    Balance,
    /// A row opens its transaction's form.
    Transaction,
    /// An aging cell opens the detail for that contact and bucket.
    Aging,
    /// No drill-down.
    None,
}

/// How the window lays the table out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A financial statement (sections, totals, indent).
    Statement,
    /// A list of rows (transactions, documents).
    List,
    /// An aging grid (contacts × buckets).
    Aging,
}

/// One catalog report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportSpec {
    /// Stable key (`^[a-z0-9_]{1,64}$`, the A07 starter id).
    pub key: &'static str,
    /// Title i18n key.
    pub title: &'static str,
    /// Catalog group.
    pub group: CatalogGroup,
    /// The engine route family.
    pub source: Source,
    /// Dates it takes.
    pub dates: DateMode,
    /// The default date choice.
    pub default_dates: DatePreset,
    /// Basis toggle or not.
    pub basis: BasisMode,
    /// Column choices (the first is the default).
    pub columns: &'static [ColumnBy],
    /// Extras on by default (a subset of `extras`).
    pub default_extras: &'static [Extra],
    /// Extras offered.
    pub extras: &'static [Extra],
    /// Filters offered.
    pub filters: &'static [FilterKind],
    /// Tag split offered.
    pub tag_split: bool,
    /// Sorting.
    pub sort: SortMode,
    /// Drill-down.
    pub drill: DrillRule,
    /// Engine capabilities it needs (all of them). Missing: the entry
    /// shows greyed as "Needs an engine update" (Q-220).
    pub capabilities: &'static [&'static str],
    /// Layout.
    pub layout: Layout,
}

impl ReportSpec {
    /// True when `has` reports every capability this report needs.
    pub fn available(&self, has: impl Fn(&str) -> bool) -> bool {
        self.capabilities.iter().all(|c| has(c))
    }

    /// The default column choice.
    #[must_use]
    pub fn default_columns(&self) -> ColumnBy {
        self.columns.first().copied().unwrap_or(ColumnBy::Total)
    }
}
