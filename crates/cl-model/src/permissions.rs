//! Who may do what in the open company (P29, plan §1).
//!
//! The engine is the gate: every route checks the signed-in user's
//! effective permission map. The desktop reads the same map from
//! `GET /auth/me` and hides only what the engine would refuse, so menus,
//! the rail, the palette, shortcuts, Home's lanes and the Reports list
//! never offer a door that ends in "you don't have permission".
//!
//! - [`Rights`] is the map for one company (the role is per company, so it
//!   is read again after every open or switch).
//! - [`need`] has a row for every command in [`crate::commands::COMMANDS`];
//!   a unit test fails when one is missing, so nothing is allowed by
//!   default.
//! - [`report_area`] mirrors the engine's report areas (U-227): a report
//!   needs `reports` and the area its data belongs to.
//! - Admin is the engine `owner` or the super admin (Q-P29k).
//!
//! Nothing here does I/O.

use std::collections::BTreeMap;
use std::sync::Arc;

pub use cl_core::sensitive_doors::SensitiveDoors;

use cl_core::report::Source;

use crate::commands::CommandId;

/// An access level of one resource (the engine's `none`, `view`, `full`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Level {
    /// No access.
    #[default]
    None,
    /// Read only.
    View,
    /// Read and change.
    Full,
}

impl Level {
    /// The engine's word.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Level::None => "none",
            Level::View => "view",
            Level::Full => "full",
        }
    }

    /// Reads the engine's word; anything else is `None` (unknown values
    /// never grant).
    #[must_use]
    pub fn parse(s: &str) -> Level {
        match s {
            "full" => Level::Full,
            "view" => Level::View,
            _ => Level::None,
        }
    }
}

/// What a command does with a resource (the engine maps GET to read and
/// every other method to a change).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// Look.
    Read,
    /// Create, change, delete.
    Write,
}

macro_rules! resources {
    ($( $(#[$m:meta])* $v:ident = $key:literal, $writable:literal, $inherits:expr; )*) => {
        /// A row of the engine's permission matrix (`PERMISSION_RESOURCES`),
        /// plus the P29 report areas (U-227, U-228).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum Resource {
            $( $(#[$m])* $v, )*
        }

        impl Resource {
            /// Every resource, in the engine's matrix order.
            pub const ALL: &'static [Resource] = &[$(Resource::$v,)*];

            /// The engine key.
            #[must_use]
            pub const fn key(self) -> &'static str {
                match self { $(Resource::$v => $key,)* }
            }

            /// False for a view-only surface (only none and view apply).
            #[must_use]
            pub const fn writable(self) -> bool {
                match self { $(Resource::$v => $writable,)* }
            }

            /// A key an older engine does not send takes this resource's
            /// level (the engine's `inheritsFrom`, U-227).
            #[must_use]
            pub const fn inherits_from(self) -> Option<Resource> {
                match self { $(Resource::$v => $inherits,)* }
            }
        }
    };
}

resources! {
    /// Journal entries, sales receipts, expenses, transfers.
    Transactions = "transactions", true, None;
    /// Bulk Entry.
    BatchEntry = "batch_entry", true, None;
    /// Templates and recurring entries.
    Recurring = "recurring", true, None;
    /// The duplicate finder.
    Duplicates = "duplicates", true, None;
    /// Bulk imports.
    BulkImport = "bulk_import", true, None;
    /// Invoices, credit memos, statements, finance charges.
    Invoices = "invoices", true, None;
    /// Receive Payment and refunds.
    ReceivePayment = "receive_payment", true, None;
    /// Estimates.
    Estimates = "estimates", true, None;
    /// Items, price levels, unit sets, inventory.
    Items = "items", true, None;
    /// Sales tax.
    SalesTax = "sales_tax", true, None;
    /// Bills.
    Bills = "bills", true, None;
    /// Pay Bills.
    PayBills = "pay_bills", true, None;
    /// Vendor credits.
    VendorCredits = "vendor_credits", true, None;
    /// Payroll import.
    PayrollImport = "payroll_import", true, None;
    /// Checks.
    Checks = "checks", true, None;
    /// Bank feed, deposits, reconcile.
    Banking = "banking", true, None;
    /// Daily sales.
    DailySales = "daily_sales", true, None;
    /// Reports (every report needs it).
    Reports = "reports", false, None;
    /// Budgets.
    Budgets = "budgets", true, None;
    /// Home's figures.
    Dashboard = "dashboard", false, None;
    /// The accountant's trial balance module.
    TrialBalance = "trial_balance", true, None;
    /// The tax estimator.
    TaxEstimator = "tax_estimator", true, None;
    /// Chart of accounts.
    Accounts = "accounts", true, None;
    /// Customers and vendors.
    Contacts = "contacts", true, None;
    /// Attachments.
    Attachments = "attachments", true, None;
    /// Tags.
    Tags = "tags", true, None;
    /// Company settings.
    CompanySettings = "company_settings", true, None;
    /// The AI assistant.
    AiChat = "ai_chat", true, None;
    /// The audit trail.
    AuditLog = "audit_log", false, None;
    /// U-227: the P&L, Balance Sheet, ledger reports, packs and Ask.
    FinancialReports = "financial_reports", false, Some(Resource::Reports);
}
// No `item_costs`: U-228 was cut, and the engine's report areas put the
// inventory value reports on `items` (services/report-areas.ts).

impl Resource {
    /// The resource with this engine key.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Resource> {
        Resource::ALL.iter().copied().find(|r| r.key() == key)
    }

    fn index(self) -> usize {
        // `ALL` lists every variant in declaration order.
        self as usize
    }
}

const N: usize = 30;

/// The rights of the signed-in user in the open company.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rights {
    levels: [Level; N],
    admin: bool,
    /// Sensitive accounts are hidden from this user (U-33, the sensitive
    /// plan). Always false until the engine reports it.
    pub hides_sensitive: bool,
    /// The routes the engine still refuses this user while an account is
    /// hidden (`sensitive-accounts-doors-v1`, P29 review B2). Nothing
    /// refused for an Admin, an older engine, or when nothing is marked.
    doors: Arc<SensitiveDoors>,
}

impl Default for Rights {
    /// No company open: nothing.
    fn default() -> Rights {
        Rights::none()
    }
}

/// A command's requirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// Every signed-in user (app commands, keys inside a screen that is
    /// itself gated).
    Always,
    /// Only an Admin (Q-P29k).
    AdminOnly,
    /// Read access to the resource.
    Read(Resource),
    /// Change access to the resource.
    Write(Resource),
    /// A catalog report: `reports` and the report's area.
    Report(Source),
}

impl Rights {
    /// Everything, as an Admin (the demo; the owner who made the company).
    #[must_use]
    pub fn all() -> Rights {
        Rights {
            levels: [Level::Full; N],
            admin: true,
            hides_sensitive: false,
            doors: Arc::default(),
        }
    }

    /// Nothing.
    #[must_use]
    pub fn none() -> Rights {
        Rights {
            levels: [Level::None; N],
            admin: false,
            hides_sensitive: false,
            doors: Arc::default(),
        }
    }

    /// The engine's `readonly` role: view everything, change nothing.
    #[must_use]
    pub fn view_only() -> Rights {
        Rights {
            levels: [Level::View; N],
            admin: false,
            hides_sensitive: false,
            doors: Arc::default(),
        }
    }

    /// The rights the engine reported: the role in this company, the super
    /// admin flag and the effective map. Unknown keys are ignored; a key the
    /// map lacks takes its `inherits_from` level, else none.
    #[must_use]
    pub fn from_engine<S: AsRef<str>>(
        role: &str,
        is_super_admin: bool,
        map: &BTreeMap<String, S>,
    ) -> Rights {
        let mut sent = [None::<Level>; N];
        for (k, v) in map {
            if let Some(r) = Resource::from_key(k) {
                sent[r.index()] = Some(Level::parse(v.as_ref()));
            }
        }
        let mut levels = [Level::None; N];
        for r in Resource::ALL {
            let own = sent[r.index()]
                .or_else(|| r.inherits_from().and_then(|p| sent[p.index()]))
                .unwrap_or(Level::None);
            levels[r.index()] = if r.writable() {
                own
            } else {
                own.min(Level::View)
            };
        }
        let admin = is_admin_role(role, is_super_admin);
        Rights {
            levels,
            admin,
            hides_sensitive: hides_sensitive(admin, map),
            doors: Arc::default(),
        }
    }

    /// The same rights without the "See sensitive accounts" right (the
    /// demo's restricted Bookkeeper, U-33); an Admin always sees them.
    #[must_use]
    pub fn hiding_sensitive(mut self) -> Rights {
        self.hides_sensitive = !self.admin;
        self
    }

    /// The least rights a role can have when the engine's map could not be
    /// read: an Admin keeps everything; any other role is view-only at most
    /// and sees no sensitive account (fail closed; the engine still
    /// decides; P29 review M1) — except a `bookkeeper`, who sees them
    /// (owner 2026-11-17: the engine's default for the role).
    #[must_use]
    pub fn from_role_only(role: &str, is_super_admin: bool) -> Rights {
        if is_admin_role(role, is_super_admin) {
            Rights::all()
        } else if role == "bookkeeper" {
            Rights::view_only()
        } else if matches!(role, "readonly" | "accountant") {
            Rights::view_only().hiding_sensitive()
        } else {
            Rights::none().hiding_sensitive()
        }
    }

    /// The level of one resource.
    #[must_use]
    pub fn level(&self, r: Resource) -> Level {
        self.levels[r.index()]
    }

    /// May the user do `action` with `r`.
    #[must_use]
    pub fn can(&self, r: Resource, action: Action) -> bool {
        match action {
            Action::Read => self.level(r) >= Level::View,
            Action::Write => self.level(r) == Level::Full,
        }
    }

    /// Owner or super admin (Q-P29k).
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// The built-in role template these rights are exactly (P29 QA-4: the
    /// status bar names the role the Users list shows). `None` for an
    /// Admin, view-only, or a custom role or overrides.
    #[must_use]
    pub fn builtin_role(&self) -> Option<BuiltinRole> {
        if self.admin {
            return None;
        }
        BuiltinRole::ALL
            .iter()
            .copied()
            .find(|b| b.rights().levels == self.levels)
    }

    /// May the user run the catalog report with this source.
    #[must_use]
    pub fn allows_report(&self, source: Source) -> bool {
        self.allows_report_area(source) && !self.refuses("GET", source.route())
    }

    /// The same without the sensitive-doors check (a panel that reads
    /// another route than the report's own, [`ReportUse::reads`]).
    fn allows_report_area(&self, source: Source) -> bool {
        let area = report_area(source);
        let base = report_needs_reports(source);
        // P29 review S6: Budget vs. Actual is served by the engine's budgets
        // router, which checks `budgets` read as well.
        let budgets =
            !matches!(source, Source::BudgetVsActual) || self.can(Resource::Budgets, Action::Read);
        (!base || self.can(Resource::Reports, Action::Read))
            && self.can(area, Action::Read)
            && budgets
    }

    /// May the user run this need.
    #[must_use]
    pub fn allows_need(&self, need: Need) -> bool {
        match need {
            Need::Always => true,
            Need::AdminOnly => self.admin,
            Need::Read(r) => self.can(r, Action::Read),
            Need::Write(r) => self.can(r, Action::Write),
            Need::Report(s) => self.allows_report(s),
        }
    }

    /// May the user run this command. A command without a row is refused
    /// (a unit test keeps every shipped command in the table).
    #[must_use]
    pub fn allows(&self, id: CommandId) -> bool {
        need(id).is_some_and(|n| self.allows_need(n))
            && command_door(id).is_none_or(|d| self.door_open(d))
    }

    /// The engine's list of refused routes for this user (P29 review B2).
    /// An Admin keeps nothing refused.
    #[must_use]
    pub fn with_doors(mut self, doors: impl Into<Arc<SensitiveDoors>>) -> Rights {
        self.doors = if self.admin {
            Arc::default()
        } else {
            doors.into()
        };
        self
    }

    /// The list kept by [`Rights::with_doors`].
    #[must_use]
    pub fn doors(&self) -> &SensitiveDoors {
        &self.doors
    }

    /// Would the engine refuse `method route` because an account is hidden
    /// from this user (the matcher, [`SensitiveDoors::refuses`]).
    #[must_use]
    pub fn refuses(&self, method: &str, route: &str) -> bool {
        !self.admin && self.doors.refuses(method, route)
    }

    /// Is every route behind this door served for this user.
    #[must_use]
    pub fn door_open(&self, door: Door) -> bool {
        !door.routes().iter().any(|(m, r)| self.refuses(m, r))
    }

    /// May the user add, merge, delete or turn off rows in the list window
    /// `window` (P29). A list the role opens with read access only keeps
    /// its New/Merge/Delete buttons disabled; any other window is not a
    /// read-gated list and is left alone.
    #[must_use]
    pub fn can_change_list(&self, window: CommandId) -> bool {
        match need(window) {
            Some(Need::Read(r)) => self.can(r, Action::Write),
            _ => true,
        }
    }

    /// The same rights without Admin (the demo's non-owner roles).
    #[must_use]
    pub fn without_admin(mut self) -> Rights {
        self.admin = false;
        self
    }

    /// Sets one level (Custom roles, tests).
    pub fn set(&mut self, r: Resource, level: Level) {
        self.levels[r.index()] = if r.writable() {
            level
        } else {
            level.min(Level::View)
        };
    }
}

/// U-33: an engine with `sensitive-accounts-v1` sends `sensitive_accounts`
/// (none or view); without the right, accounts the Admin marked are hidden.
/// An older engine sends no key and hides nothing. Whether any account is
/// marked is the engine's `hiddenNotice`, not this.
fn hides_sensitive<S: AsRef<str>>(admin: bool, map: &BTreeMap<String, S>) -> bool {
    !admin
        && map
            .get(crate::roles::SENSITIVE_KEY)
            .is_some_and(|v| Level::parse(v.as_ref()) == Level::None)
}

/// Owner or super admin. An engine `accountant` (only a `MyBooks` transfer
/// brings one in) has full rights but is not an Admin (Q-P29k).
#[must_use]
pub fn is_admin_role(role: &str, is_super_admin: bool) -> bool {
    is_super_admin || matches!(role.trim(), "owner" | "super_admin")
}

/// The area a catalog report's data belongs to (mirrors the engine's
/// U-227 table, plan §2 "U-227 applies the area gate to").
#[must_use]
pub const fn report_area(source: Source) -> Resource {
    match source {
        Source::ProfitLoss
        | Source::BalanceSheet
        | Source::TrialBalance
        | Source::GeneralLedger
        | Source::Journal
        | Source::TransactionList
        | Source::BudgetVsActual
        | Source::RevenueByCategory
        | Source::ExpenseByCategory
        | Source::AccountActivitySummary => Resource::FinancialReports,
        Source::ArAgingSummary
        | Source::ArAgingDetail
        | Source::InvoiceList
        | Source::CustomerBalanceSummary
        | Source::CustomerBalanceDetail
        | Source::SalesByCustomer
        | Source::SalesByItem => Resource::Invoices,
        Source::VendorBalanceSummary
        | Source::VendorBalanceDetail
        | Source::ApAgingSummary
        | Source::ApAgingDetail
        | Source::UnpaidBills
        | Source::VendorStatement
        | Source::ExpenseByVendor
        | Source::TransactionListByVendor
        | Source::BillPaymentHistory
        | Source::Vendor1099Summary
        | Source::Ap1099Prep => Resource::Bills,
        Source::SalesTaxLiability => Resource::SalesTax,
        Source::DepositDetail | Source::CheckRegister | Source::BankBalances => Resource::Banking,
        Source::InventoryValuation
        | Source::InventoryValuationDetail
        | Source::InventoryStockStatus => Resource::Items,
        Source::AuditLog | Source::ClosingDateExceptions => Resource::AuditLog,
        Source::Duplicates => Resource::Duplicates,
    }
}

/// The audit trail (`/audit-log`) needs `audit_log` alone. The closing-date
/// exceptions route lives under the engine's reports router, which checks
/// `reports` read before the route's own `audit_log` check, so it needs
/// both.
const fn report_needs_reports(source: Source) -> bool {
    !matches!(source, Source::AuditLog)
}

/// A screen outside the Reports window that reads a `/reports/*` route
/// (plan WP1): the panel or figure is hidden without its area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReportUse {
    /// Home's Clients owe figure and the A/R aging link.
    HomeReceivables,
    /// Home's Bills due figure.
    HomePayables,
    /// Home's income and expense chart and bank balances.
    HomeCharts,
    /// Home's Sales tax owed footer.
    HomeSalesTax,
    /// A customer's statement (Statements, the customer center).
    CustomerStatement,
    /// A vendor's statement (vendor center, statement match).
    VendorStatement,
    /// The 1099 Center's summary and prep.
    Vendor1099,
    /// Reconcile's detail and discrepancy reports.
    ReconcileReports,
    /// Sales Tax Owed and the payments list.
    SalesTaxReports,
    /// Items to Reorder and the inventory reports.
    InventoryReports,
    /// An account's quick report (Chart of Accounts, registers).
    AccountReport,
    /// Pay Bills' unpaid bills list.
    UnpaidBills,
}

impl ReportUse {
    /// Every use.
    pub const ALL: &'static [ReportUse] = &[
        ReportUse::HomeReceivables,
        ReportUse::HomePayables,
        ReportUse::HomeCharts,
        ReportUse::HomeSalesTax,
        ReportUse::CustomerStatement,
        ReportUse::VendorStatement,
        ReportUse::Vendor1099,
        ReportUse::ReconcileReports,
        ReportUse::SalesTaxReports,
        ReportUse::InventoryReports,
        ReportUse::AccountReport,
        ReportUse::UnpaidBills,
    ];

    /// The report whose area it needs.
    #[must_use]
    pub const fn source(self) -> Source {
        match self {
            ReportUse::HomeReceivables | ReportUse::CustomerStatement => Source::ArAgingSummary,
            ReportUse::HomePayables | ReportUse::UnpaidBills => Source::UnpaidBills,
            ReportUse::HomeCharts | ReportUse::AccountReport => Source::ProfitLoss,
            ReportUse::HomeSalesTax | ReportUse::SalesTaxReports => Source::SalesTaxLiability,
            ReportUse::VendorStatement => Source::VendorStatement,
            ReportUse::Vendor1099 => Source::Vendor1099Summary,
            ReportUse::ReconcileReports => Source::BankBalances,
            ReportUse::InventoryReports => Source::InventoryStockStatus,
        }
    }

    /// The engine routes it reads (for the "every caller has an area" test
    /// and the contract test).
    #[must_use]
    pub const fn routes(self) -> &'static [&'static str] {
        match self {
            ReportUse::HomeReceivables => &["/api/v1/reports/ar-aging-summary"],
            ReportUse::HomePayables => &["/api/v1/reports/ap-aging-summary"],
            ReportUse::HomeCharts => &[
                "/api/v1/reports/profit-loss",
                "/api/v1/reports/balance-sheet",
                "/api/v1/reports/revenue-by-category",
                "/api/v1/reports/expense-by-category",
                "/api/v1/reports/trial-balance",
                "/api/v1/reports/general-ledger",
                "/api/v1/reports/account-activity-summary",
            ],
            ReportUse::HomeSalesTax => &["/api/v1/reports/sales-tax-liability"],
            ReportUse::CustomerStatement => &[
                "/api/v1/reports/customer-statement",
                "/api/v1/reports/customer-statements",
                "/api/v1/reports/customer-balance-summary",
                "/api/v1/reports/customer-balance-detail",
                "/api/v1/reports/ar-aging-detail",
                "/api/v1/reports/invoice-list",
                "/api/v1/reports/sales-by-customer",
                "/api/v1/reports/sales-by-item",
            ],
            ReportUse::VendorStatement => &[
                "/api/v1/reports/vendor-statement",
                "/api/v1/reports/vendor-balance-summary",
                "/api/v1/reports/vendor-balance-detail",
                "/api/v1/reports/ap-aging-detail",
                "/api/v1/reports/expense-by-vendor",
                "/api/v1/reports/transaction-list-by-vendor",
                "/api/v1/reports/bill-payment-history",
            ],
            ReportUse::Vendor1099 => &[
                "/api/v1/reports/vendor-1099-summary",
                "/api/v1/reports/ap-1099-prep",
            ],
            ReportUse::ReconcileReports => &[
                "/api/v1/reports/reconciliation-detail",
                "/api/v1/reports/reconciliation-discrepancy",
                "/api/v1/reports/deposit-detail",
                "/api/v1/reports/check-register",
                "/api/v1/reports/bank-balances",
            ],
            ReportUse::SalesTaxReports => &[
                "/api/v1/reports/sales-tax-payments",
                "/api/v1/reports/exemptions-expiring",
            ],
            ReportUse::InventoryReports => &[
                "/api/v1/reports/inventory-stock-status",
                "/api/v1/reports/reorder",
                "/api/v1/reports/inventory-valuation",
                "/api/v1/reports/inventory-valuation-detail",
            ],
            ReportUse::AccountReport => &["/api/v1/reports/account-report"],
            ReportUse::UnpaidBills => &["/api/v1/reports/unpaid-bills"],
        }
    }
}

impl Rights {
    /// May a screen outside the Reports window show this report panel.
    #[must_use]
    pub fn allows_use(&self, u: ReportUse) -> bool {
        self.allows_report_area(u.source()) && !u.reads().iter().any(|r| self.refuses("GET", r))
    }
}

impl ReportUse {
    /// The routes the panel itself reads (narrower than [`ReportUse::routes`],
    /// which lists the area's reports): the sensitive-doors check uses
    /// these, so a panel that reads only served routes stays.
    #[must_use]
    pub const fn reads(self) -> &'static [&'static str] {
        match self {
            ReportUse::HomeReceivables => &["/api/v1/reports/ar-aging-detail"],
            ReportUse::HomePayables => &["/api/v1/reports/ap-aging-summary"],
            // charts::read asks for the P&L only.
            ReportUse::HomeCharts => &["/api/v1/reports/profit-loss"],
            // The footer reads `/sales-tax/owed`, not the liability report.
            ReportUse::HomeSalesTax => &["/api/v1/sales-tax/owed"],
            ReportUse::CustomerStatement => &[
                "/api/v1/reports/customer-statement",
                "/api/v1/reports/customer-statements",
            ],
            ReportUse::VendorStatement => &["/api/v1/reports/vendor-statement"],
            ReportUse::Vendor1099 => &[
                "/api/v1/reports/vendor-1099-summary",
                "/api/v1/reports/ap-1099-prep",
            ],
            ReportUse::ReconcileReports => &[
                "/api/v1/reports/reconciliation-detail",
                "/api/v1/reports/reconciliation-discrepancy",
            ],
            ReportUse::SalesTaxReports => &["/api/v1/reports/sales-tax-payments"],
            ReportUse::InventoryReports => &["/api/v1/reports/reorder"],
            ReportUse::AccountReport => &["/api/v1/reports/account-report"],
            ReportUse::UnpaidBills => &["/api/v1/reports/unpaid-bills"],
        }
    }
}

/// A door outside the Reports list that reads a route the engine may still
/// refuse while an account is hidden (P29 review B2). Each screen asks
/// [`Rights::door_open`] and hides or disables the piece; a unit test in
/// `cl-ui` keeps every refused desktop operation claimed by a door, a
/// catalog report, or an Admin-only screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Door {
    /// File › Export All Data.
    ExportAll,
    /// Opening balances sent from company setup.
    OpeningBalances,
    /// A vendor's 1099 box and tax details (vendor center and form).
    Vendor1099Profile,
    /// Preferences › 1099 account mappings.
    Box1099Mappings,
    /// Email a report.
    ReportEmail,
    /// Budgets' Fill from actuals, Copy and Adjust by percent.
    BudgetTools,
    /// Reconcile's detail and discrepancy reports.
    ReconcileReports,
    /// Sales tax payments list.
    SalesTaxPayments,
    /// Exemption certificates expiring soon.
    ExemptionsExpiring,
    /// Items to Reorder's report.
    ReorderReport,
    /// AI usage counts and the AI task switches.
    AiUsage,
    /// Preferences › AI: the company's AI settings, models and test.
    AiSettings,
    /// Banking › Import Payroll, its History, Home's unposted-payroll link
    /// (U-247: every payroll route is refused while an account is hidden).
    PayrollImport,
}

impl Door {
    /// Every door.
    pub const ALL: &'static [Door] = &[
        Door::ExportAll,
        Door::OpeningBalances,
        Door::Vendor1099Profile,
        Door::Box1099Mappings,
        Door::ReportEmail,
        Door::BudgetTools,
        Door::ReconcileReports,
        Door::SalesTaxPayments,
        Door::ExemptionsExpiring,
        Door::ReorderReport,
        Door::AiUsage,
        Door::AiSettings,
        Door::PayrollImport,
    ];

    /// The engine routes behind it (method, route template).
    #[must_use]
    pub const fn routes(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Door::ExportAll => &[
                ("GET", "/api/v1/export/full"),
                ("GET", "/api/v1/export/full/{fileName}"),
            ],
            Door::OpeningBalances => &[("POST", "/api/v1/export/opening-balances")],
            Door::Vendor1099Profile => &[
                ("GET", "/api/v1/practice/1099/vendors/{contactId}/profile"),
                ("PUT", "/api/v1/practice/1099/vendors/{contactId}/profile"),
            ],
            Door::Box1099Mappings => &[("PUT", "/api/v1/practice/1099/account-mappings/{formBox}")],
            Door::ReportEmail => &[("POST", "/api/v1/reports/email")],
            Door::BudgetTools => &[
                ("POST", "/api/v1/budgets/{id}/fill-from-actuals"),
                ("POST", "/api/v1/budgets/{id}/copy-from/{sourceId}"),
                ("POST", "/api/v1/budgets/{id}/adjust-by-percent"),
            ],
            Door::ReconcileReports => &[
                ("GET", "/api/v1/reports/reconciliation-detail"),
                ("GET", "/api/v1/reports/reconciliation-discrepancy"),
            ],
            Door::SalesTaxPayments => &[("GET", "/api/v1/reports/sales-tax-payments")],
            Door::ExemptionsExpiring => &[("GET", "/api/v1/reports/exemptions-expiring")],
            Door::ReorderReport => &[("GET", "/api/v1/reports/reorder")],
            Door::AiUsage => &[
                ("GET", "/api/v1/ai/usage"),
                ("GET", "/api/v1/ai/usage/summary"),
                ("PATCH", "/api/v1/ai/consent/{companyId}/tasks"),
            ],
            Door::AiSettings => &[
                ("GET", "/api/v1/ai/admin/config"),
                ("PUT", "/api/v1/ai/admin/config"),
                ("GET", "/api/v1/ai/admin/disclosure"),
                ("POST", "/api/v1/ai/admin/disclosure/accept"),
                ("POST", "/api/v1/ai/admin/test/{provider}"),
                ("GET", "/api/v1/ai/admin/models/{provider}"),
            ],
            Door::PayrollImport => &[
                ("GET", "/api/v1/payroll-import/sessions"),
                ("POST", "/api/v1/payroll-import/upload"),
                ("GET", "/api/v1/payroll-import/sessions/{id}"),
                ("DELETE", "/api/v1/payroll-import/sessions/{id}"),
                ("GET", "/api/v1/payroll-import/sessions/{id}/preview"),
                ("POST", "/api/v1/payroll-import/sessions/{id}/apply-mapping"),
                (
                    "GET",
                    "/api/v1/payroll-import/sessions/{id}/description-map",
                ),
                (
                    "PUT",
                    "/api/v1/payroll-import/sessions/{id}/description-map",
                ),
                ("POST", "/api/v1/payroll-import/sessions/{id}/validate"),
                ("POST", "/api/v1/payroll-import/sessions/{id}/generate-je"),
                ("POST", "/api/v1/payroll-import/sessions/{id}/post"),
                ("POST", "/api/v1/payroll-import/sessions/{id}/reverse"),
                ("GET", "/api/v1/payroll-import/templates"),
                ("POST", "/api/v1/payroll-import/templates"),
                ("PUT", "/api/v1/payroll-import/templates/{id}"),
                ("DELETE", "/api/v1/payroll-import/templates/{id}"),
                ("GET", "/api/v1/payroll-import/account-mappings/{companyId}"),
                ("PUT", "/api/v1/payroll-import/account-mappings/{companyId}"),
                (
                    "POST",
                    "/api/v1/payroll-import/account-mappings/{companyId}/auto-map",
                ),
            ],
        }
    }
}

/// The door a whole command depends on (its window shows nothing else).
#[must_use]
pub fn command_door(id: CommandId) -> Option<Door> {
    match id.as_str() {
        "file.export_all" => Some(Door::ExportAll),
        "payroll.import" => Some(Door::PayrollImport),
        _ => None,
    }
}

/// Admin-only desktop screens whose routes may stay refused for others (an
/// Admin is never refused). Preferences › AI is offered to every user, so
/// its routes are [`Door::AiSettings`]. P31: the import screens
/// (`file.import_qbd`, `file.import_system`, `file.import_transactions`)
/// are Admin-only; the engine refuses `/api/v1/imports` to a viewer with a
/// hidden account (U-263).
pub const ADMIN_ONLY_ROUTES: &[(&str, &str)] = &[
    ("POST", "/api/v1/imports/upload"),
    ("GET", "/api/v1/imports/{id}"),
    ("DELETE", "/api/v1/imports/{id}"),
    ("POST", "/api/v1/imports/batches"),
    ("GET", "/api/v1/imports/batches"),
    ("GET", "/api/v1/imports/batches/{id}"),
    ("POST", "/api/v1/imports/batches/{id}/commit"),
    ("POST", "/api/v1/imports/batches/{id}/undo"),
    ("POST", "/api/v1/imports/batches/{id}/tieout"),
    ("POST", "/api/v1/imports/batches/{id}/verify"),
    ("PUT", "/api/v1/imports/batches/{id}/spot-check"),
    ("GET", "/api/v1/imports/batches/{id}/transactions"),
];

/// Built-in roles the Users screen offers (plan §1 table, Q-P29v). Admin
/// and View-only are engine roles; the others are a `bookkeeper` with a
/// permission template of this name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltinRole {
    /// Everything except Admin tools.
    Bookkeeper,
    /// Customers and sales.
    Receivables,
    /// Vendors and bills.
    Payables,
}

impl BuiltinRole {
    /// Every built-in template.
    pub const ALL: &'static [BuiltinRole] = &[
        BuiltinRole::Bookkeeper,
        BuiltinRole::Receivables,
        BuiltinRole::Payables,
    ];

    /// The template's name on the engine (how the app finds it again).
    #[must_use]
    pub const fn template_name(self) -> &'static str {
        match self {
            BuiltinRole::Bookkeeper => "Bookkeeper",
            BuiltinRole::Receivables => "Accounts receivable",
            BuiltinRole::Payables => "Accounts payable",
        }
    }

    /// The full key map (every key listed: an absent key is `none` on the
    /// engine).
    #[must_use]
    pub fn levels(self) -> Vec<(Resource, Level)> {
        Resource::ALL.iter().map(|r| (*r, self.level(*r))).collect()
    }

    #[allow(clippy::match_same_arms)] // the plan's table, row by row
    fn level(self, r: Resource) -> Level {
        use Level::{Full, None as No, View};
        use Resource as R;
        match (self, r) {
            (_, R::TrialBalance | R::TaxEstimator | R::AiChat) => No,
            (
                BuiltinRole::Bookkeeper,
                R::CompanySettings | R::Reports | R::FinancialReports | R::Dashboard | R::AuditLog,
            ) => View,
            (BuiltinRole::Bookkeeper, _) => Full,
            // A/R
            (
                BuiltinRole::Receivables,
                R::Transactions
                | R::Recurring
                | R::Contacts
                | R::Attachments
                | R::Invoices
                | R::ReceivePayment
                | R::Estimates
                | R::SalesTax
                | R::DailySales,
            ) => Full,
            (
                BuiltinRole::Receivables,
                R::Tags | R::Items | R::Accounts | R::Reports | R::Dashboard | R::CompanySettings,
            ) => View,
            (BuiltinRole::Receivables, _) => No,
            // A/P
            (
                BuiltinRole::Payables,
                R::Contacts
                | R::Attachments
                | R::Recurring
                | R::Checks
                | R::Bills
                | R::PayBills
                | R::VendorCredits,
            ) => Full,
            (
                BuiltinRole::Payables,
                R::Tags | R::Items | R::Accounts | R::Reports | R::Dashboard | R::CompanySettings,
            ) => View,
            (BuiltinRole::Payables, _) => No,
        }
    }

    /// The "See sensitive accounts" level the template carries (owner
    /// 2026-11-17, Q-P29c changed): the Bookkeeper sees them; A/R and A/P
    /// don't.
    #[must_use]
    pub const fn sensitive(self) -> Level {
        match self {
            BuiltinRole::Bookkeeper => Level::View,
            BuiltinRole::Receivables | BuiltinRole::Payables => Level::None,
        }
    }

    /// The rights a `bookkeeper` on this template has (no overrides).
    #[must_use]
    pub fn rights(self) -> Rights {
        let map: BTreeMap<String, &str> = self
            .levels()
            .into_iter()
            .map(|(r, l)| (r.key().to_owned(), l.key()))
            .collect();
        Rights::from_engine("bookkeeper", false, &map)
    }
}

/// What a command needs. `None` for an id with no row: [`Rights::allows`]
/// refuses it, and a unit test keeps every shipped command in the table.
#[must_use]
#[allow(clippy::too_many_lines, clippy::match_same_arms)] // one row per command, grouped by menu
pub fn need(id: CommandId) -> Option<Need> {
    // Keys that act inside a window: the window itself is gated.
    const INSIDE: &[&str] = &[
        "form.",
        "check.",
        "batch.",
        "grid.",
        "bill.",
        "invoice.",
        "payment.",
        "credit_memo.",
        "viewer.",
        "queue.",
        "feed.",
        "reconcile.",
        "builder.",
        "list.",
        "register.",
        "window.",
        "view.",
        "help.",
        "edit.",
    ];
    use Need::{AdminOnly, Always, Read, Report, Write};
    use Resource as R;
    let id = id.as_str();
    let n = match id {
        // File
        "file.open_company" | "file.new_company" | "file.close_company" | "file.open_practice"
        | "file.sign_out" | "file.lock" | "file.restore" | "file.print" | "file.print_forms"
        | "file.printer_setup" | "file.close_window" | "file.quit" => Always,
        // Q-P32i: an IIF file needs the backup right (and sight of every
        // hidden account, checked by the screen).
        "file.backup" | "file.export_all" | "file.export_iif" => Read(R::CompanySettings),
        // P31 (S4): commit, undo and verify are the Admin's; the wizards are too
        // (D5: the spreadsheet import's commit and undo, U-263).
        "file.full_backup"
        | "file.delete_company"
        | "file.import_qbd"
        | "file.import_system"
        | "file.import_transactions" => AdminOnly,
        // Edit (Preferences hold personal settings too; company tabs check
        // their own rights)
        "edit.preferences" => Always,
        // Lists
        "lists.chart_of_accounts" => Read(R::Accounts),
        "lists.items" | "lists.price_levels" | "lists.unit_sets" => Read(R::Items),
        "lists.tags" => Read(R::Tags),
        "lists.terms" | "lists.payment_methods" => Read(R::Contacts),
        "lists.templates" | "lists.templates_due" => Read(R::Recurring),
        "lists.sales_tax" => Read(R::SalesTax),
        "lists.import_export" => Write(R::Contacts),
        // Customers
        "clients.center" => Read(R::Contacts),
        "clients.create_invoice"
        | "clients.credit_memo"
        | "clients.finance_charges"
        | "clients.recover_bad_debt" => Write(R::Invoices),
        "clients.receive_payment" | "clients.refund" => Write(R::ReceivePayment),
        "clients.sales_receipt" => Write(R::Transactions),
        "clients.daily_sales" => Write(R::DailySales),
        "clients.statements" | "clients.outbox" => Read(R::Invoices),
        "clients.tax_exemptions" => Read(R::SalesTax),
        // Vendors
        "vendors.center" => Read(R::Contacts),
        "vendors.enter_bill" => Write(R::Bills),
        "vendors.pay_bills" => Write(R::PayBills),
        "vendors.bill_payment" => Read(R::PayBills),
        "vendors.write_check" => Write(R::Checks),
        "vendors.card_charge" | "vendors.personal_funds" => Write(R::Transactions),
        "vendors.sales_tax_owed" => Read(R::SalesTax),
        "vendors.pay_sales_tax" => Write(R::SalesTax),
        "vendors.adjust_inventory" | "vendors.physical_count" => Write(R::Items),
        "vendors.items_to_reorder" => Read(R::Items),
        // ADD-02:84: reading a statement or a receipt is `bills:read`.
        "nav.receipt_inbox" | "nav.statement_match" | "vendors.review_documents" => Read(R::Bills),
        "nav.document_view" => Read(R::Attachments),
        // Banking
        "banking.register" => Read(R::Accounts),
        "banking.deposit" | "banking.reconcile" | "banking.import_file" => Write(R::Banking),
        "banking.feed" => Read(R::Banking),
        "banking.transfer" | "banking.owner_money" | "banking.loan_payment" => {
            Write(R::Transactions)
        }
        "banking.bulk_entry" => Write(R::BatchEntry),
        // Payroll
        "payroll.import" => Write(R::PayrollImport),
        // Reports
        "reports.center"
        | "reports.memorized_list"
        | "reports.report_file"
        | "reports.builder_editor" => Read(R::Reports),
        "reports.profit_loss" => Report(Source::ProfitLoss),
        "reports.balance_sheet" => Report(Source::BalanceSheet),
        "reports.trial_balance" => Report(Source::TrialBalance),
        "reports.ar_aging" => Report(Source::ArAgingSummary),
        "reports.inventory_value" => Report(Source::InventoryValuation),
        "reports.inventory_value_detail" => Report(Source::InventoryValuationDetail),
        "reports.stock_status" => Report(Source::InventoryStockStatus),
        "reports.closing_date_exceptions" => Report(Source::ClosingDateExceptions),
        "reports.refresh"
        | "reports.memorize"
        | "reports.collapse"
        | "reports.memorized_open"
        | "reports.memorized_rename"
        | "reports.memorized_delete"
        | "reports.export_report_file"
        | "reports.import_report_file" => Always,
        // Company
        "company.journal_entry" => Write(R::Transactions),
        "company.information" => Read(R::CompanySettings),
        "company.closing_date" | "company.users" | "company.lan_setup" => AdminOnly,
        "company.send_to_accountant" => Read(R::CompanySettings),
        "company.audit_trail" => Read(R::AuditLog),
        // Your own password, automatic sign-in and idle lock.
        "company.my_sign_in" => Always,
        "nav.home" => Always,
        other if INSIDE.iter().any(|p| other.starts_with(p)) => Always,
        _ => return None,
    };
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::COMMANDS;
    use proptest::prelude::*;

    /// The pinned engine's answer for a viewer with a hidden account.
    pub(crate) fn engine_doors() -> SensitiveDoors {
        serde_json::from_str(cl_core::sensitive_doors::ENGINE_ANSWER_JSON).expect("fixture")
    }

    #[test]
    fn refused_doors_are_not_offered_to_a_restricted_user() {
        let bk = BuiltinRole::Bookkeeper.rights().with_doors(engine_doors());
        // Reports the engine filters stay; the ones it still refuses go.
        assert!(bk.allows_report(Source::ArAgingSummary));
        assert!(bk.allows_report(Source::ExpenseByCategory));
        assert!(
            bk.allows_report(Source::ClosingDateExceptions)
                || !bk.can(Resource::AuditLog, Action::Read)
        );
        for s in [
            Source::SalesTaxLiability,
            Source::Vendor1099Summary,
            Source::Ap1099Prep,
            Source::ExpenseByVendor,
            Source::VendorBalanceDetail,
            Source::RevenueByCategory,
            Source::DepositDetail,
            Source::AccountActivitySummary,
            Source::InventoryValuation,
            Source::InventoryValuationDetail,
            Source::InventoryStockStatus,
            Source::BudgetVsActual,
        ] {
            assert!(!bk.allows_report(s), "{s:?} is refused by the engine");
        }
        // Commands follow: the report commands and Export All Data.
        assert!(!bk.allows(CommandId::new("reports.inventory_value")));
        assert!(!bk.allows(CommandId::new("file.export_all")));
        assert!(!bk.door_open(Door::Vendor1099Profile));
        assert!(!bk.door_open(Door::ReportEmail));
        // Home: the sales tax footer reads `/sales-tax/owed` (served); the
        // chart reads the P&L (served).
        assert!(bk.allows_use(ReportUse::HomeSalesTax));
        assert!(
            bk.allows_use(ReportUse::HomeCharts)
                || !bk.can(Resource::FinancialReports, Action::Read)
        );
        assert!(!bk.allows_use(ReportUse::Vendor1099));
    }

    #[test]
    fn admins_and_unmarked_companies_keep_every_door() {
        let admin = Rights::all().with_doors(engine_doors());
        assert!(admin.allows_report(Source::SalesTaxLiability));
        assert!(admin.allows(CommandId::new("file.export_all")));
        assert!(Door::ALL.iter().all(|d| admin.door_open(*d)));
        // Nothing marked: the engine answers false and empty lists.
        let bk = BuiltinRole::Bookkeeper
            .rights()
            .with_doors(SensitiveDoors::none());
        let before = BuiltinRole::Bookkeeper.rights();
        for s in Source::ALL {
            assert_eq!(bk.allows_report(s), before.allows_report(s), "{s:?}");
        }
        assert!(Door::ALL.iter().all(|d| bk.door_open(*d)));
        // An older engine: no list read at all.
        assert!(before.allows(CommandId::new("file.export_all")));
    }

    #[test]
    fn every_door_route_is_refused_by_the_pinned_engine() {
        // A door whose routes were all converted is stale: drop it.
        let d = engine_doors();
        for door in Door::ALL {
            assert!(d.refuses_any(door.routes()), "{door:?} is served now");
        }
        for (m, r) in ADMIN_ONLY_ROUTES {
            assert!(d.refuses(m, r), "{m} {r}");
        }
    }

    #[test]
    fn the_resource_table_matches_the_engine() {
        assert_eq!(Resource::ALL.len(), N);
        for (i, r) in Resource::ALL.iter().enumerate() {
            assert_eq!(r.index(), i, "{r:?}");
            assert_eq!(Resource::from_key(r.key()), Some(*r));
        }
        assert_eq!(Resource::from_key("payroll"), None);
    }

    /// P29 review S6: Budget vs. Actual also needs `budgets` read (the
    /// engine's budgets router serves it); other financial reports don't.
    #[test]
    fn budget_vs_actual_needs_budgets_read() {
        let mut r = Rights::none();
        r.set(Resource::Reports, Level::View);
        r.set(Resource::FinancialReports, Level::View);
        assert!(r.allows_report(Source::ProfitLoss));
        assert!(!r.allows_report(Source::BudgetVsActual));
        r.set(Resource::Budgets, Level::View);
        assert!(r.allows_report(Source::BudgetVsActual));
        // The built-in roles without budgets don't get it.
        assert!(
            !BuiltinRole::Payables
                .rights()
                .allows_report(Source::BudgetVsActual)
        );
    }

    /// P29 review M1: when the engine's map can't be read, a non-Admin is
    /// treated as not seeing sensitive accounts (fail closed); a Bookkeeper
    /// sees them (owner 2026-11-17).
    #[test]
    fn role_only_rights_hide_sensitive_for_non_admins() {
        assert!(!Rights::from_role_only("bookkeeper", false).hides_sensitive);
        for role in ["readonly", "accountant", "someone"] {
            assert!(
                Rights::from_role_only(role, false).hides_sensitive,
                "{role}"
            );
        }
        assert!(!Rights::from_role_only("owner", false).hides_sensitive);
        assert!(!Rights::from_role_only("bookkeeper", true).hides_sensitive);
    }

    #[test]
    fn every_command_has_a_row() {
        let missing: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| need(c.id).is_none())
            .map(|c| c.id.as_str())
            .collect();
        assert!(
            missing.is_empty(),
            "commands without a rights row: {missing:?}"
        );
        assert_eq!(need(CommandId::new("nope.nothing")), None);
        assert!(!Rights::all().allows(CommandId::new("nope.nothing")));
    }

    #[test]
    fn admin_is_the_owner_or_the_super_admin() {
        let view = Rights::view_only();
        assert!(!view.can_change_list(CommandId::new("lists.chart_of_accounts")));
        assert!(!view.can_change_list(CommandId::new("clients.center")));
        assert!(Rights::all().can_change_list(CommandId::new("lists.tags")));
        // A window that is not a read-gated list is left alone.
        assert!(view.can_change_list(CommandId::new("nav.home")));
        assert!(is_admin_role("owner", false));
        assert!(is_admin_role("bookkeeper", true));
        assert!(!is_admin_role("accountant", false));
        assert!(!is_admin_role("bookkeeper", false));
        let mut full: BTreeMap<String, &str> = BTreeMap::new();
        for r in Resource::ALL {
            full.insert(r.key().into(), "full");
        }
        let acct = Rights::from_engine("accountant", false, &full);
        assert!(!acct.is_admin());
        assert!(acct.allows(CommandId::new("clients.create_invoice")));
        assert!(!acct.allows(CommandId::new("company.closing_date")));
        assert!(Rights::all().allows(CommandId::new("company.closing_date")));
        // A view-only surface never reads as full.
        assert_eq!(acct.level(Resource::Reports), Level::View);
    }

    #[test]
    fn payables_hides_invoices_and_receivable_reports() {
        let ap = BuiltinRole::Payables.rights();
        let yes = |id: &'static str| ap.allows(CommandId::new(id));
        assert!(!yes("clients.create_invoice"));
        assert!(!yes("clients.receive_payment"));
        assert!(!yes("reports.ar_aging"));
        assert!(!yes("reports.profit_loss"), "financial reports are none");
        assert!(!yes("company.closing_date"));
        assert!(!yes("company.audit_trail"));
        assert!(yes("vendors.enter_bill"));
        assert!(yes("vendors.pay_bills"));
        assert!(yes("vendors.write_check"));
        assert!(yes("reports.center"));
        assert!(ap.allows_report(Source::ApAgingSummary));
        assert!(!ap.allows_report(Source::ArAgingDetail));
        assert!(!ap.allows_report(Source::InvoiceList));
        assert!(
            ap.allows_report(Source::InventoryValuation),
            "A/P sees costs"
        );
        assert!(!ap.is_admin());
    }

    #[test]
    fn receivables_hides_bills() {
        let ar = BuiltinRole::Receivables.rights();
        assert!(ar.allows(CommandId::new("clients.create_invoice")));
        assert!(ar.allows(CommandId::new("clients.sales_receipt")));
        assert!(!ar.allows(CommandId::new("vendors.enter_bill")));
        assert!(!ar.allows(CommandId::new("banking.deposit")), "Q-P29x");
        assert!(!ar.allows_report(Source::ApAgingSummary));
        // The engine puts inventory value on `items` (A/R views items).
        assert!(ar.allows_report(Source::InventoryValuation));
        assert!(ar.allows_report(Source::InventoryValuationDetail));
        assert!(
            !ar.allows_report(Source::ProfitLoss),
            "financial reports none"
        );
        assert!(ar.allows_report(Source::ArAgingSummary));
    }

    #[test]
    fn a_missing_area_key_inherits_reports() {
        // An engine before U-227 sends no `financial_reports`: the P&L
        // follows `reports`, as the engine does.
        let mut m: BTreeMap<String, &str> = BTreeMap::new();
        m.insert("reports".into(), "view");
        m.insert("items".into(), "full");
        let r = Rights::from_engine("bookkeeper", false, &m);
        assert_eq!(r.level(Resource::FinancialReports), Level::View);
        assert!(
            r.allows_report(Source::InventoryValuation),
            "inventory value follows items"
        );
        assert!(r.allows(CommandId::new("reports.profit_loss")));
        assert!(
            !r.allows(CommandId::new("reports.ar_aging")),
            "invoices none"
        );
        // Unknown keys and words are ignored; nothing is granted by them.
        m.insert("payroll_runs".into(), "full");
        m.insert("bills".into(), "admin");
        let r = Rights::from_engine("bookkeeper", false, &m);
        assert_eq!(r.level(Resource::Bills), Level::None);
    }

    #[test]
    fn the_audit_reports_need_only_the_audit_right() {
        let mut r = Rights::none();
        r.set(Resource::AuditLog, Level::View);
        assert!(r.allows_report(Source::AuditLog));
        // Under the engine's reports router: `reports` read too.
        assert!(!r.allows_report(Source::ClosingDateExceptions));
        r.set(Resource::Reports, Level::View);
        assert!(r.allows_report(Source::ClosingDateExceptions));
        r.set(Resource::Reports, Level::None);
        assert!(!r.allows_report(Source::ProfitLoss));
        assert!(r.allows(CommandId::new("company.audit_trail")));
    }

    #[test]
    fn without_the_map_only_admins_keep_more_than_view() {
        assert_eq!(Rights::from_role_only("owner", false), Rights::all());
        assert_eq!(
            Rights::from_role_only("bookkeeper", false),
            Rights::view_only()
        );
        assert_eq!(
            Rights::from_role_only("readonly", false),
            Rights::view_only().hiding_sensitive()
        );
        assert_eq!(
            Rights::from_role_only("client_viewer", false),
            Rights::none().hiding_sensitive()
        );
        let v = Rights::view_only();
        assert!(v.allows(CommandId::new("reports.profit_loss")));
        assert!(!v.allows(CommandId::new("vendors.write_check")));
        assert!(Rights::default().allows(CommandId::new("nav.home")));
        assert!(!Rights::default().allows(CommandId::new("lists.chart_of_accounts")));
    }

    #[test]
    fn every_report_caller_has_an_area() {
        let mut routes: Vec<&str> = ReportUse::ALL
            .iter()
            .flat_map(|u| u.routes())
            .copied()
            .collect();
        let n = routes.len();
        routes.sort_unstable();
        routes.dedup();
        assert_eq!(routes.len(), n, "a route belongs to one use");
        let ap = BuiltinRole::Payables.rights();
        assert!(!ap.allows_use(ReportUse::HomeReceivables));
        assert!(ap.allows_use(ReportUse::HomePayables));
        assert!(!ap.allows_use(ReportUse::HomeCharts));
        assert!(Rights::all().allows_use(ReportUse::HomeCharts));
    }

    #[test]
    fn builtins_list_every_key() {
        for b in BuiltinRole::ALL {
            assert_eq!(b.levels().len(), N, "{b:?}");
            for (r, l) in b.levels() {
                assert!(r.writable() || l != Level::Full, "{b:?} {r:?}");
            }
        }
        let bk = BuiltinRole::Bookkeeper.rights();
        assert!(bk.allows(CommandId::new("clients.create_invoice")));
        assert!(bk.allows(CommandId::new("company.audit_trail")));
        assert!(!bk.allows(CommandId::new("company.users")));
    }

    /// P29 QA-4: rights are named by the built-in template they are.
    #[test]
    fn rights_name_their_builtin_role() {
        for b in BuiltinRole::ALL {
            assert_eq!(b.rights().builtin_role(), Some(*b), "{b:?}");
        }
        assert_eq!(Rights::all().builtin_role(), None, "an Admin");
        assert_eq!(Rights::view_only().builtin_role(), None);
        let mut custom = BuiltinRole::Payables.rights();
        custom.set(Resource::Reports, Level::None);
        assert_eq!(custom.builtin_role(), None, "a custom role");
    }

    #[test]
    fn sensitive_accounts_hide_only_without_the_right() {
        let mut m: BTreeMap<String, &str> = BTreeMap::new();
        m.insert("reports".into(), "view");
        // An engine before U-33 sends no key: nothing is hidden.
        assert!(!Rights::from_engine("bookkeeper", false, &m).hides_sensitive);
        m.insert("sensitive_accounts".into(), "none");
        assert!(Rights::from_engine("bookkeeper", false, &m).hides_sensitive);
        assert!(!Rights::from_engine("owner", false, &m).hides_sensitive);
        assert!(!Rights::from_engine("bookkeeper", true, &m).hides_sensitive);
        m.insert("sensitive_accounts".into(), "view");
        assert!(!Rights::from_engine("bookkeeper", false, &m).hides_sensitive);
        // The demo's restricted viewer (View-only); an Admin never hides.
        let vo = Rights::view_only().hiding_sensitive();
        assert!(vo.hides_sensitive);
        // Owner 2026-11-17: the built-in Bookkeeper sees them; A/R, A/P don't.
        assert!(!BuiltinRole::Bookkeeper.rights().hides_sensitive);
        assert_eq!(BuiltinRole::Bookkeeper.sensitive(), Level::View);
        assert_eq!(BuiltinRole::Receivables.sensitive(), Level::None);
        assert_eq!(BuiltinRole::Payables.sensitive(), Level::None);
        assert!(!Rights::all().hiding_sensitive().hides_sensitive);
    }

    fn arb_rights() -> impl Strategy<Value = Rights> {
        (proptest::collection::vec(0u8..3, N), any::<bool>()).prop_map(|(ls, admin)| {
            let mut r = Rights::none();
            for (res, l) in Resource::ALL.iter().zip(ls) {
                r.set(
                    *res,
                    match l {
                        0 => Level::None,
                        1 => Level::View,
                        _ => Level::Full,
                    },
                );
            }
            r.admin = admin;
            r
        })
    }

    proptest! {
        /// `allows` never grants what `can` denies.
        #[test]
        fn allows_is_within_can(r in arb_rights()) {
            for c in COMMANDS {
                let Some(n) = need(c.id) else { continue };
                if !r.allows(c.id) {
                    continue;
                }
                match n {
                    Need::Read(res) => prop_assert!(r.can(res, Action::Read)),
                    Need::Write(res) => prop_assert!(r.can(res, Action::Write)),
                    Need::Report(s) => {
                        prop_assert!(r.can(report_area(s), Action::Read));
                    }
                    Need::AdminOnly => prop_assert!(r.is_admin()),
                    Need::Always => {}
                }
            }
        }
    }
}
