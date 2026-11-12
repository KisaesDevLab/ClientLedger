//! The report catalog (plan §5): every catalog report, its route family
//! and the engine capabilities it needs. Groups follow the mockup
//! (Company, Who owes what, Sales & spending, Memorized) plus Q-P28i's
//! Banking, Inventory and Controls.

use cl_core::report::{Extra, Source};

use super::options::ColumnBy;
use super::presets::DatePreset;
use super::spec::{
    BasisMode, CatalogGroup, DateMode, DrillRule, FilterKind, Layout, ReportSpec, SortMode,
};

/// Engine capabilities the reports need (plan §2).
pub mod caps {
    /// PR-A: exact `*Exact` amounts on every catalog route (U-14).
    pub const EXACT: &str = "reports-exact-v1";
    /// PR-B: period, YTD, prior and compare columns, tag split (U-201).
    pub const COLUMNS: &str = "reports-columns-v1";
    /// PR-B: GL and transaction-list drill filters (U-202).
    pub const DRILL: &str = "reports-drill-v1";
    /// PR-D: exact A/P aging (U-121).
    pub const AP_AGING: &str = "ap-aging-exact-v1";
    /// PR-D: as-of open balances.
    pub const OPEN_ASOF: &str = "open-balances-asof-v1";
    /// PR-C: exact budgets and Budget vs. Actual (U-203).
    pub const BUDGETS: &str = "budgets-exact-v1";
    /// PR-E: Closing Date Exceptions (U-204).
    pub const CLOSING: &str = "closing-exceptions-v1";
    /// PR-E: audit-log filters (U-204).
    pub const AUDIT: &str = "audit-filters-v1";
    /// PR-E: report email (U-207).
    pub const EMAIL: &str = "reports-email-v1";
    /// PR-F: memorized reports (U-118).
    pub const DEFINITIONS: &str = "report-defs-v1";
    /// PR-G: ask a question (U-206).
    pub const ASK: &str = "reports-ask-v1";
    /// Exact A/R aging (U-142, shipped).
    pub const AR_AGING: &str = "ar-aging-exact-v1";
    /// Sales tax liability v2 (shipped).
    pub const SALES_TAX: &str = "sales-tax-liability-v2";
    /// Inventory reports (U-160, shipped).
    pub const INVENTORY: &str = "inventory-reports-v1";
    /// Projects (U-35, not built): the Project column stays greyed.
    pub const PROJECTS: &str = "projects-v1";
}

const PL_COLUMNS: &[ColumnBy] = &[
    ColumnBy::Total,
    ColumnBy::Month,
    ColumnBy::FiscalPeriod,
    ColumnBy::Quarter,
    ColumnBy::Year,
    ColumnBy::TagGroup,
    ColumnBy::Client,
    ColumnBy::Project,
];
const BS_COLUMNS: &[ColumnBy] = &[
    ColumnBy::Total,
    ColumnBy::Month,
    ColumnBy::FiscalPeriod,
    ColumnBy::Quarter,
    ColumnBy::Year,
];
const PL_EXTRAS: &[Extra] = &[
    Extra::PctOfIncome,
    Extra::Ytd,
    Extra::PriorPeriod,
    Extra::PriorYear,
];
const BS_EXTRAS: &[Extra] = &[Extra::PriorPeriod, Extra::PriorYear];
const LEDGER_FILTERS: &[FilterKind] = &[
    FilterKind::Accounts,
    FilterKind::Contacts,
    FilterKind::Tags,
    FilterKind::Amount,
    FilterKind::Cleared,
    FilterKind::TxnTypes,
    FilterKind::Adjusting,
];
/// The statements' routes filter by tag and the adjusting lens only; an
/// account or name filter is the ledger's (Q-P28ah).
const STATEMENT_FILTERS: &[FilterKind] = &[FilterKind::Tags, FilterKind::Adjusting];
/// The Balance Sheet and Trial Balance: the adjusting lens only. The
/// engine applies a tag to these reports' lines differently by basis, so a
/// tag-filtered sheet or Trial Balance may not balance and its drills may
/// not sum (PR-6 S2, Q-P28ah amended by Q-PR6b); the tag split is the
/// balance view by tag.
const BALANCE_FILTERS: &[FilterKind] = &[FilterKind::Adjusting];

/// A P&L-like statement.
const STATEMENT: ReportSpec = ReportSpec {
    key: "",
    title: "",
    group: CatalogGroup::Company,
    source: Source::ProfitLoss,
    dates: DateMode::Range,
    default_dates: DatePreset::LastMonth,
    basis: BasisMode::Toggle,
    columns: PL_COLUMNS,
    default_extras: &[],
    extras: PL_EXTRAS,
    filters: STATEMENT_FILTERS,
    tag_split: true,
    sort: SortMode::None,
    drill: DrillRule::Lines,
    capabilities: &[caps::EXACT],
    layout: Layout::Statement,
};

/// A list of rows over dates.
const LIST: ReportSpec = ReportSpec {
    key: "",
    title: "",
    group: CatalogGroup::SalesSpending,
    source: Source::TransactionList,
    dates: DateMode::Range,
    default_dates: DatePreset::ThisMonth,
    basis: BasisMode::Toggle,
    columns: &[ColumnBy::Total],
    default_extras: &[],
    extras: &[],
    filters: &[],
    tag_split: false,
    sort: SortMode::WithinGroups,
    drill: DrillRule::Transaction,
    capabilities: &[caps::EXACT],
    layout: Layout::List,
};

/// Open items as of a date.
const OPEN_ITEMS: ReportSpec = ReportSpec {
    key: "",
    title: "",
    group: CatalogGroup::WhoOwesWhat,
    source: Source::ArAgingSummary,
    dates: DateMode::AsOf,
    default_dates: DatePreset::Today,
    basis: BasisMode::NotApplicable,
    columns: &[ColumnBy::Total],
    default_extras: &[],
    extras: &[],
    // Only the aging details narrow by name (Q-P28ah).
    filters: &[],
    tag_split: false,
    sort: SortMode::WithinGroups,
    drill: DrillRule::Transaction,
    capabilities: &[caps::EXACT],
    layout: Layout::List,
};

/// Every catalog report, in catalog order.
pub static CATALOG: &[ReportSpec] = &[
    // ---------------------------------------------------------- Company
    ReportSpec {
        key: "profit_loss",
        title: "reports.title.profit_loss",
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_by_month",
        title: "reports.title.profit_loss_by_month",
        columns: &[
            ColumnBy::Month,
            ColumnBy::Total,
            ColumnBy::FiscalPeriod,
            ColumnBy::Quarter,
            ColumnBy::Year,
        ],
        default_dates: DatePreset::YearToDate,
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_by_fiscal_period",
        title: "reports.title.profit_loss_by_fiscal_period",
        columns: &[ColumnBy::FiscalPeriod, ColumnBy::Total, ColumnBy::Quarter],
        default_dates: DatePreset::ThisYear,
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_comparative",
        title: "reports.title.profit_loss_comparative",
        default_extras: &[Extra::PriorYear],
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_ytd",
        title: "reports.title.profit_loss_ytd",
        default_extras: &[Extra::PctOfIncome, Extra::Ytd],
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_by_tag_group",
        title: "reports.title.profit_loss_by_tag_group",
        columns: &[ColumnBy::TagGroup],
        tag_split: false,
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "profit_loss_by_client",
        title: "reports.title.profit_loss_by_client",
        columns: &[ColumnBy::Client],
        tag_split: false,
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "balance_sheet",
        title: "reports.title.balance_sheet",
        source: Source::BalanceSheet,
        dates: DateMode::AsOf,
        default_dates: DatePreset::Today,
        columns: BS_COLUMNS,
        extras: BS_EXTRAS,
        drill: DrillRule::Balance,
        filters: BALANCE_FILTERS,
        ..STATEMENT
    },
    ReportSpec {
        key: "balance_sheet_comparative",
        title: "reports.title.balance_sheet_comparative",
        source: Source::BalanceSheet,
        dates: DateMode::AsOf,
        default_dates: DatePreset::EndOfLastMonth,
        columns: BS_COLUMNS,
        extras: BS_EXTRAS,
        default_extras: &[Extra::PriorYear],
        drill: DrillRule::Balance,
        filters: BALANCE_FILTERS,
        capabilities: &[caps::EXACT, caps::COLUMNS],
        ..STATEMENT
    },
    ReportSpec {
        key: "trial_balance",
        title: "reports.title.trial_balance",
        source: Source::TrialBalance,
        // The engine's Trial Balance is a balance at a date (income and
        // expenses from the fiscal year's start).
        dates: DateMode::AsOf,
        default_dates: DatePreset::EndOfLastMonth,
        columns: &[ColumnBy::Total],
        extras: &[],
        drill: DrillRule::Balance,
        filters: BALANCE_FILTERS,
        ..STATEMENT
    },
    ReportSpec {
        key: "general_ledger",
        title: "reports.title.general_ledger",
        group: CatalogGroup::Company,
        source: Source::GeneralLedger,
        filters: LEDGER_FILTERS,
        tag_split: true,
        // A running balance can't be sorted (Q-P28k).
        sort: SortMode::None,
        ..LIST
    },
    ReportSpec {
        key: "transaction_detail_by_account",
        title: "reports.title.transaction_detail_by_account",
        group: CatalogGroup::Company,
        source: Source::GeneralLedger,
        filters: LEDGER_FILTERS,
        sort: SortMode::None,
        ..LIST
    },
    ReportSpec {
        key: "journal",
        title: "reports.title.journal",
        group: CatalogGroup::Company,
        source: Source::Journal,
        filters: LEDGER_FILTERS,
        sort: SortMode::None,
        capabilities: &[caps::EXACT, caps::DRILL],
        ..LIST
    },
    ReportSpec {
        key: "budget_vs_actual",
        title: "reports.title.budget_vs_actual",
        source: Source::BudgetVsActual,
        default_dates: DatePreset::ThisYear,
        // One column group per budget period (whole fiscal periods, 4-4-5
        // aware), then the total (U-203, Q-P28n).
        columns: &[ColumnBy::FiscalPeriod],
        extras: &[],
        filters: &[FilterKind::Budget],
        capabilities: &[caps::BUDGETS],
        ..STATEMENT
    },
    // ---------------------------------------------------- Who owes what
    ReportSpec {
        key: "ar_aging_summary",
        title: "reports.title.ar_aging_summary",
        source: Source::ArAgingSummary,
        drill: DrillRule::Aging,
        layout: Layout::Aging,
        capabilities: &[caps::AR_AGING],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "ar_aging_detail",
        title: "reports.title.ar_aging_detail",
        source: Source::ArAgingDetail,
        filters: &[FilterKind::Contacts],
        capabilities: &[caps::AR_AGING],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "open_invoices",
        title: "reports.title.open_invoices",
        source: Source::InvoiceList,
        capabilities: &[caps::EXACT, caps::OPEN_ASOF],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "invoice_list",
        title: "reports.title.invoice_list",
        source: Source::InvoiceList,
        dates: DateMode::Range,
        default_dates: DatePreset::ThisMonth,
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "customer_balance_summary",
        title: "reports.title.customer_balance_summary",
        source: Source::CustomerBalanceSummary,
        capabilities: &[caps::EXACT, caps::OPEN_ASOF],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "customer_balance_detail",
        title: "reports.title.customer_balance_detail",
        source: Source::CustomerBalanceDetail,
        sort: SortMode::None,
        capabilities: &[caps::EXACT, caps::OPEN_ASOF],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "vendor_balance_summary",
        title: "reports.title.vendor_balance_summary",
        source: Source::VendorBalanceSummary,
        capabilities: &[caps::EXACT, caps::OPEN_ASOF],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "vendor_balance_detail",
        title: "reports.title.vendor_balance_detail",
        source: Source::VendorBalanceDetail,
        sort: SortMode::None,
        capabilities: &[caps::EXACT, caps::OPEN_ASOF],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "ap_aging_summary",
        title: "reports.title.ap_aging_summary",
        source: Source::ApAgingSummary,
        drill: DrillRule::Aging,
        layout: Layout::Aging,
        capabilities: &[caps::EXACT, caps::AP_AGING],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "ap_aging_detail",
        title: "reports.title.ap_aging_detail",
        source: Source::ApAgingDetail,
        filters: &[FilterKind::Contacts],
        capabilities: &[caps::EXACT, caps::AP_AGING],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "unpaid_bills",
        title: "reports.title.unpaid_bills",
        source: Source::UnpaidBills,
        capabilities: &[caps::EXACT, caps::AP_AGING],
        ..OPEN_ITEMS
    },
    ReportSpec {
        key: "vendor_statement",
        title: "reports.title.vendor_statement",
        source: Source::VendorStatement,
        dates: DateMode::Range,
        default_dates: DatePreset::ThisMonth,
        filters: &[FilterKind::Vendor],
        // A running balance can't be sorted (Q-P28k).
        sort: SortMode::None,
        ..OPEN_ITEMS
    },
    // ------------------------------------------------- Sales & spending
    ReportSpec {
        key: "sales_by_client",
        title: "reports.title.sales_by_client",
        source: Source::SalesByCustomer,
        // The route takes the adjusting lens, not a client (Q-P28ah).
        filters: &[FilterKind::Adjusting],
        drill: DrillRule::Lines,
        capabilities: &[caps::EXACT, caps::DRILL],
        ..LIST
    },
    ReportSpec {
        key: "sales_by_item",
        title: "reports.title.sales_by_item",
        source: Source::SalesByItem,
        // The route takes the adjusting lens, not an item (Q-P28ah).
        filters: &[FilterKind::Adjusting],
        drill: DrillRule::Lines,
        capabilities: &[caps::EXACT, caps::DRILL],
        ..LIST
    },
    ReportSpec {
        key: "revenue_by_category",
        title: "reports.title.revenue_by_category",
        source: Source::RevenueByCategory,
        drill: DrillRule::Lines,
        ..LIST
    },
    ReportSpec {
        key: "expenses_by_category",
        title: "reports.title.expenses_by_category",
        source: Source::ExpenseByCategory,
        drill: DrillRule::Lines,
        ..LIST
    },
    ReportSpec {
        key: "expenses_by_vendor",
        title: "reports.title.expenses_by_vendor",
        source: Source::ExpenseByVendor,
        drill: DrillRule::Lines,
        ..LIST
    },
    ReportSpec {
        key: "transactions_by_vendor",
        title: "reports.title.transactions_by_vendor",
        source: Source::TransactionListByVendor,
        basis: BasisMode::NotApplicable,
        filters: &[FilterKind::Vendor],
        ..LIST
    },
    ReportSpec {
        key: "bill_payment_history",
        title: "reports.title.bill_payment_history",
        source: Source::BillPaymentHistory,
        basis: BasisMode::NotApplicable,
        ..LIST
    },
    ReportSpec {
        key: "sales_tax_liability",
        title: "reports.title.sales_tax_liability",
        source: Source::SalesTaxLiability,
        default_dates: DatePreset::LastQuarter,
        // The company's sales tax basis decides (P23).
        basis: BasisMode::NotApplicable,
        sort: SortMode::None,
        drill: DrillRule::None,
        capabilities: &[caps::SALES_TAX],
        ..LIST
    },
    ReportSpec {
        key: "vendor_1099_summary",
        title: "reports.title.vendor_1099_summary",
        source: Source::Vendor1099Summary,
        dates: DateMode::Year,
        default_dates: DatePreset::LastYear,
        basis: BasisMode::NotApplicable,
        drill: DrillRule::None,
        ..LIST
    },
    ReportSpec {
        key: "ap_1099_prep",
        title: "reports.title.ap_1099_prep",
        source: Source::Ap1099Prep,
        dates: DateMode::Year,
        default_dates: DatePreset::LastYear,
        basis: BasisMode::NotApplicable,
        ..LIST
    },
    // ------------------------------------------------------------ Banking
    ReportSpec {
        key: "deposit_detail",
        title: "reports.title.deposit_detail",
        group: CatalogGroup::Banking,
        source: Source::DepositDetail,
        basis: BasisMode::NotApplicable,
        ..LIST
    },
    ReportSpec {
        key: "check_register",
        title: "reports.title.check_register",
        group: CatalogGroup::Banking,
        source: Source::CheckRegister,
        basis: BasisMode::NotApplicable,
        filters: &[FilterKind::BankAccount],
        ..LIST
    },
    ReportSpec {
        key: "bank_balances",
        title: "reports.title.bank_balances",
        group: CatalogGroup::Banking,
        source: Source::BankBalances,
        dates: DateMode::AsOf,
        default_dates: DatePreset::Today,
        basis: BasisMode::NotApplicable,
        drill: DrillRule::Balance,
        ..LIST
    },
    ReportSpec {
        key: "account_activity_summary",
        title: "reports.title.account_activity_summary",
        group: CatalogGroup::Banking,
        source: Source::AccountActivitySummary,
        basis: BasisMode::NotApplicable,
        drill: DrillRule::Lines,
        ..LIST
    },
    // ---------------------------------------------------------- Inventory
    ReportSpec {
        key: "inventory_valuation",
        title: "reports.title.inventory_valuation",
        group: CatalogGroup::Inventory,
        source: Source::InventoryValuation,
        dates: DateMode::AsOf,
        default_dates: DatePreset::Today,
        basis: BasisMode::NotApplicable,
        drill: DrillRule::None,
        capabilities: &[caps::INVENTORY],
        ..LIST
    },
    ReportSpec {
        key: "inventory_valuation_detail",
        title: "reports.title.inventory_valuation_detail",
        group: CatalogGroup::Inventory,
        source: Source::InventoryValuationDetail,
        default_dates: DatePreset::YearToDate,
        basis: BasisMode::NotApplicable,
        filters: &[FilterKind::Items],
        // A running quantity and value can't be sorted.
        sort: SortMode::None,
        capabilities: &[caps::INVENTORY],
        ..LIST
    },
    ReportSpec {
        key: "inventory_stock_status",
        title: "reports.title.inventory_stock_status",
        group: CatalogGroup::Inventory,
        source: Source::InventoryStockStatus,
        dates: DateMode::AsOf,
        default_dates: DatePreset::Today,
        basis: BasisMode::NotApplicable,
        drill: DrillRule::None,
        capabilities: &[caps::INVENTORY],
        ..LIST
    },
    // ----------------------------------------------------------- Controls
    ReportSpec {
        key: "audit_trail",
        title: "reports.title.audit_trail",
        group: CatalogGroup::Controls,
        source: Source::AuditLog,
        basis: BasisMode::NotApplicable,
        capabilities: &[caps::AUDIT],
        ..LIST
    },
    ReportSpec {
        key: "closing_date_exceptions",
        title: "reports.title.closing_date_exceptions",
        group: CatalogGroup::Controls,
        source: Source::ClosingDateExceptions,
        default_dates: DatePreset::ThisYear,
        basis: BasisMode::NotApplicable,
        capabilities: &[caps::CLOSING],
        ..LIST
    },
    ReportSpec {
        key: "duplicate_finder",
        title: "reports.title.duplicate_finder",
        group: CatalogGroup::Controls,
        source: Source::Duplicates,
        // The engine scans by date; dates are always sent.
        default_dates: DatePreset::ThisQuarter,
        basis: BasisMode::NotApplicable,
        sort: SortMode::None,
        // Exact today (Q-P28o): a list of pairs, no capability.
        capabilities: &[],
        ..LIST
    },
];

/// The report window's stand-in spec for a Report Builder run over a range
/// (A07): not in the catalog; the window takes its dates, basis and period
/// columns from it and runs the definition instead of a catalog route.
pub static BUILDER_RANGE: ReportSpec = ReportSpec {
    key: "builder",
    title: "reports.builder.title",
    group: CatalogGroup::Company,
    source: Source::ProfitLoss,
    dates: DateMode::Range,
    default_dates: DatePreset::ThisMonth,
    basis: BasisMode::Toggle,
    columns: &[
        ColumnBy::Total,
        ColumnBy::Month,
        ColumnBy::Quarter,
        ColumnBy::Year,
    ],
    default_extras: &[],
    extras: &[],
    filters: &[],
    tag_split: false,
    sort: SortMode::None,
    drill: DrillRule::Lines,
    capabilities: &[],
    layout: Layout::Statement,
};

/// As [`BUILDER_RANGE`], for a definition dated as of one day.
pub static BUILDER_AS_OF: ReportSpec = ReportSpec {
    dates: DateMode::AsOf,
    default_dates: DatePreset::Today,
    ..BUILDER_RANGE
};

/// As [`BUILDER_AS_OF`] with no Columns choice: aging buckets refuse period
/// and compare ranges (U-119e, review #2).
pub static BUILDER_AS_OF_TOTAL: ReportSpec = ReportSpec {
    columns: &[ColumnBy::Total],
    ..BUILDER_AS_OF
};

/// As [`BUILDER_RANGE`] with no Columns choice: a running balance takes no
/// period columns (`RUNNING_BALANCE`).
pub static BUILDER_RANGE_TOTAL: ReportSpec = ReportSpec {
    columns: &[ColumnBy::Total],
    ..BUILDER_RANGE
};
/// The report with `key`.
#[must_use]
pub fn find(key: &str) -> Option<&'static ReportSpec> {
    CATALOG.iter().find(|s| s.key == key)
}

/// Reports whose title (as `title_of` renders it) or key contains every
/// word of `query`, case-insensitively, in catalog order.
pub fn search<'a>(
    query: &str,
    title_of: impl Fn(&ReportSpec) -> String + 'a,
) -> Vec<&'static ReportSpec> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    CATALOG
        .iter()
        .filter(|s| {
            let hay = format!("{} {}", title_of(s), s.key.replace('_', " ")).to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// The capability a column choice needs beyond its report's.
#[must_use]
pub const fn column_capability(c: ColumnBy) -> Option<&'static str> {
    match c {
        ColumnBy::Total => None,
        ColumnBy::Project => Some(caps::PROJECTS),
        _ => Some(caps::COLUMNS),
    }
}

/// The capability an extra column needs.
#[must_use]
pub const fn extra_capability(_: Extra) -> &'static str {
    caps::COLUMNS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_and_follow_the_engine_pattern() {
        let mut seen = HashSet::new();
        for s in CATALOG {
            assert!(super::super::definition::valid_key(s.key), "{}", s.key);
            assert!(seen.insert(s.key), "duplicate {}", s.key);
            assert_eq!(s.title, format!("reports.title.{}", s.key));
            assert!(!s.columns.is_empty(), "{}", s.key);
            assert!(
                s.default_extras.iter().all(|e| s.extras.contains(e)),
                "{}",
                s.key
            );
            assert_ne!(s.group, CatalogGroup::Memorized);
        }
    }

    #[test]
    fn the_mockup_reports_are_in_their_groups() {
        for (key, group) in [
            ("profit_loss", CatalogGroup::Company),
            ("profit_loss_by_month", CatalogGroup::Company),
            ("balance_sheet", CatalogGroup::Company),
            ("trial_balance", CatalogGroup::Company),
            ("general_ledger", CatalogGroup::Company),
            ("budget_vs_actual", CatalogGroup::Company),
            ("ar_aging_summary", CatalogGroup::WhoOwesWhat),
            ("open_invoices", CatalogGroup::WhoOwesWhat),
            ("ap_aging_summary", CatalogGroup::WhoOwesWhat),
            ("unpaid_bills", CatalogGroup::WhoOwesWhat),
            ("sales_by_client", CatalogGroup::SalesSpending),
            ("expenses_by_vendor", CatalogGroup::SalesSpending),
            ("sales_tax_liability", CatalogGroup::SalesSpending),
            ("vendor_1099_summary", CatalogGroup::SalesSpending),
        ] {
            assert_eq!(find(key).map(|s| s.group), Some(group), "{key}");
        }
    }

    #[test]
    fn open_items_have_no_basis_and_running_balances_no_sort() {
        for s in CATALOG {
            if matches!(
                s.source,
                Source::ArAgingSummary
                    | Source::ArAgingDetail
                    | Source::ApAgingSummary
                    | Source::ApAgingDetail
                    | Source::UnpaidBills
                    | Source::CustomerBalanceSummary
                    | Source::VendorBalanceSummary
            ) {
                assert_eq!(s.basis, BasisMode::NotApplicable, "{}", s.key);
            }
            if s.source == Source::GeneralLedger {
                assert_eq!(s.sort, SortMode::None, "{}", s.key);
            }
        }
    }

    #[test]
    fn availability_follows_the_capabilities() {
        let pl = find("profit_loss").expect("p&l");
        assert!(!pl.available(|_| false));
        assert!(pl.available(|c| c == caps::EXACT));
        let by_month = find("profit_loss_by_month").expect("by month");
        assert!(!by_month.available(|c| c == caps::EXACT));
        // Exact today: no engine update needed.
        let dup = find("duplicate_finder").expect("dups");
        assert!(dup.available(|_| false));
        let ar = find("ar_aging_summary").expect("ar");
        assert!(ar.available(|c| c == caps::AR_AGING));
        assert_eq!(column_capability(ColumnBy::Project), Some(caps::PROJECTS));
    }

    /// Q-P28z: the reports that list one vendor or bank account wait for
    /// the pick; the others run.
    #[test]
    fn vendor_and_bank_lists_wait_for_their_pick() {
        use super::super::options::Filters;
        let mut f = Filters::default();
        for (key, kind) in [
            ("transactions_by_vendor", FilterKind::Vendor),
            ("vendor_statement", FilterKind::Vendor),
            ("check_register", FilterKind::BankAccount),
        ] {
            assert_eq!(find(key).expect(key).missing_pick(&f), Some(kind), "{key}");
        }
        assert_eq!(find("unpaid_bills").expect("ub").missing_pick(&f), None);
        f.contact_ids = vec![
            cl_core::ids::ContactId::parse("7f3a2c10-5b6d-4e8f-9a0b-1c2d3e4f5a60").expect("id"),
        ];
        assert_eq!(find("vendor_statement").expect("vs").missing_pick(&f), None);
        assert_eq!(
            find("check_register").expect("cr").missing_pick(&f),
            Some(FilterKind::BankAccount)
        );
    }

    #[test]
    fn search_matches_every_word() {
        let title = |s: &ReportSpec| s.key.replace('_', " ");
        let hits: Vec<_> = search("profit month", title)
            .iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(hits, vec!["profit_loss_by_month"]);
        assert_eq!(search("", title).len(), CATALOG.len());
        assert!(
            search("AGING", title)
                .iter()
                .all(|s| s.key.contains("aging"))
        );
    }
}
