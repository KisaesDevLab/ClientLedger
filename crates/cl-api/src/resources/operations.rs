//! The engine operations cl-api covers and how each may be retried
//! (P05 plan §2.6, §2.8). `crates/cl-api/operations.txt` lists the same rows
//! and a test keeps the two in step.

use crate::client::{Method, TimeoutClass};
use crate::errors::RouteKind;

/// How an operation may be retried automatically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RetryClass {
    /// GET: retried on timeouts, connect errors and 502/503/504 (up to three
    /// attempts with full jitter, within the deadline).
    Idempotent,
    /// Mutations: retried only when the request provably never reached a
    /// handler (connect refused, desktop-gate refusal, an expired-token 401),
    /// or once with the same key when outcome reconciliation proves the first
    /// attempt did not commit.
    PreHandlerOnly,
    /// A mutation in one of the engine's keyed route sets (U-18, engine
    /// `docs/DESKTOP.md` "Request idempotency"; `x-idempotency: true` in its
    /// `openapi.json`): `PreHandlerOnly` unless the engine advertises that
    /// set's capability ([`KeySet::capability`]), `KeyedOnce` when it does.
    /// Only these routes ever get `KeyedOnce`.
    Keyable(KeySet),
    /// A keyable mutation while the engine advertises its set's capability
    /// (never written in the table; see [`RetryClass::effective`]): retried
    /// like reads, always with the same key, which the engine replays.
    KeyedOnce,
    /// Auth and company creation: never retried (rate limits, token
    /// rotation, duplicate companies).
    Never,
}

impl RetryClass {
    /// Name used in `operations.txt`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            RetryClass::Idempotent => "idempotent",
            RetryClass::PreHandlerOnly => "pre-handler-only",
            RetryClass::Keyable(KeySet::V1) => "keyable",
            RetryClass::Keyable(KeySet::ApV1) => "keyable-ap",
            RetryClass::Keyable(KeySet::DocsV1) => "keyable-docs",
            RetryClass::Keyable(KeySet::DocsAttachV1) => "keyable-docs-attach",
            RetryClass::Keyable(KeySet::PayBillsV1) => "keyable-pay-bills",
            RetryClass::Keyable(KeySet::CheckPrintV1) => "keyable-check-print",
            RetryClass::Keyable(KeySet::InvoiceV1) => "keyable-invoice",
            RetryClass::Keyable(KeySet::InvoiceEmailV1) => "keyable-invoice-email",
            RetryClass::Keyable(KeySet::SalesDocEmailV1) => "keyable-sales-doc-email",
            RetryClass::Keyable(KeySet::ReceivePaymentV1) => "keyable-receive-payment",
            RetryClass::Keyable(KeySet::CreditMemoV1) => "keyable-credit-memo",
            RetryClass::Keyable(KeySet::CustomerStatementsV1) => "keyable-customer-statements",
            RetryClass::Keyable(KeySet::FinanceChargesV1) => "keyable-finance-charges",
            RetryClass::Keyable(KeySet::SalesTaxSetupV1) => "keyable-sales-tax",
            RetryClass::Keyable(KeySet::SalesTaxPayV1) => "keyable-sales-tax-pay",
            RetryClass::Keyable(KeySet::RecoveryV1) => "keyable-recovery",
            RetryClass::Keyable(KeySet::InventoryV1) => "keyable-inventory",
            RetryClass::Keyable(KeySet::UnitSetsV1) => "keyable-unit-sets",
            RetryClass::Keyable(KeySet::ReportDefsV1) => "keyable-report-defs",
            RetryClass::Keyable(KeySet::BudgetsV1) => "keyable-budgets",
            RetryClass::Keyable(KeySet::ReportsEmailV1) => "keyable-reports-email",
            RetryClass::Keyable(KeySet::BankImportV1) => "keyable-bank-import",
            RetryClass::Keyable(KeySet::FeedV1) => "keyable-feed",
            RetryClass::Keyable(KeySet::FeedPostV1) => "keyable-feed-post",
            RetryClass::Keyable(KeySet::FeedAcceptV1) => "keyable-feed-accept",
            RetryClass::Keyable(KeySet::FeedUndoV1) => "keyable-feed-undo",
            RetryClass::KeyedOnce => "keyed-once",
            RetryClass::Never => "never",
        }
    }

    /// The class in effect, given whether the engine replays this request's
    /// key (U-18: it advertises the route's set capability and the key is
    /// young enough, see [`crate::IdempotencyKey`]). Only
    /// [`RetryClass::Keyable`] routes are upgraded; every other mutation
    /// keeps its class, so it is never blind-retried (the engine does not
    /// replay it).
    #[must_use]
    pub const fn effective(self, keyed: bool) -> RetryClass {
        match self {
            RetryClass::Keyable(_) if keyed => RetryClass::KeyedOnce,
            RetryClass::Keyable(_) => RetryClass::PreHandlerOnly,
            other => other,
        }
    }
}

/// The engine's keyed route sets. Each capability name fixes its routes
/// (routes added later get a new name), so a route replays its key only
/// while the engine advertises the capability of the set it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeySet {
    /// `idempotency-v1` (U-18): transactions, checks, invoices, bill
    /// create, daily sales, templates and the list creates.
    V1,
    /// `idempotency-ap-v1` (U-83 to U-87): bill edit and void, vendor credit
    /// create, edit and void, bill-payment void.
    ApV1,
    /// `idempotency-docs-v1` (U-92a): the document draft claim and convert.
    DocsV1,
    /// `document-drafts-attach-v1` (U-95): the document draft attach, keyed
    /// with `idempotency-docs-v1`'s contract under its own capability.
    DocsAttachV1,
    /// `idempotency-pay-bills-v1` (U-102): Pay Bills (`POST /bill-payments`).
    PayBillsV1,
    /// `idempotency-check-print-v1` (U-116, PR-2 B2): the check print run
    /// (`POST /checks/print`).
    CheckPrintV1,
    /// `idempotency-invoice-v1` (U-124): invoice edit, void and sent.
    InvoiceV1,
    /// `invoice-email-v1` (U-127): invoice email and email-later, keyed
    /// with `idempotency-v1`'s contract under their own capability.
    InvoiceEmailV1,
    /// `sales-doc-email-v1` (U-144): credit memo and sales receipt email
    /// and email-later, keyed under their own capability.
    SalesDocEmailV1,
    /// `idempotency-receive-payment-v1` (U-132, P21): payment receive,
    /// edit, void, unapply, apply and bounce.
    ReceivePaymentV1,
    /// `idempotency-credit-memo-v1` (U-137, P22): credit memo create,
    /// edit, apply and unapply; sales receipt create and edit.
    CreditMemoV1,
    /// `idempotency-customer-statements-v1` (U-140, P22): statement runs,
    /// statement email and email-later.
    CustomerStatementsV1,
    /// `idempotency-finance-charges-v1` (U-141, P22): finance-charge
    /// assess.
    FinanceChargesV1,
    /// `idempotency-sales-tax-v1` (U-147, P23): the sales tax setup writes
    /// (agencies, items and groups, codes, exemption certificates).
    SalesTaxSetupV1,
    /// `idempotency-sales-tax-pay-v1` (U-150, P23): Pay Sales Tax and
    /// linking an outside payment.
    SalesTaxPayV1,
    /// `idempotency-recovery-v1` (U-154, P23): bad-debt recoveries.
    RecoveryV1,
    /// Keyed under `inventory-adjust-v1` (U-159, P24; no separate capability): inventory adjustments
    /// (create, edit, delete, void) and posting a count.
    InventoryV1,
    /// Keyed under `units-of-measure-v1` (U-161, P24) with `idempotency-v1`'s
    /// contract: creating a unit set.
    UnitSetsV1,
    /// Keyed under `report-defs-v1` (U-118, P28) with `idempotency-v1`'s
    /// contract: creating and changing memorized reports and their groups.
    ReportDefsV1,
    /// `budgets-exact-v1` (U-203, P28): budget create and change, lines,
    /// fill from actuals, copy and adjust by percent.
    BudgetsV1,
    /// `reports-email-v1` (U-207, P28): emailing a report file.
    ReportsEmailV1,
    /// `bank-import-batch-v1` (U-06, P25): a bank import commit (its
    /// preview on the same route takes no key, [`crate::client`] `no_key`)
    /// and undoing an import.
    BankImportV1,
    /// `feed-idempotency-v1` (U-173, P26 E2): bank feed categorize,
    /// approve and match (key optional) and the review decisions, duplicate
    /// and review again (key required).
    FeedV1,
    /// `feed-match-post-v1` (U-171, P26 E4): a feed transfer and a feed
    /// deposit (key required).
    FeedPostV1,
    /// `feed-accept-v1` (U-170, P26 E3): the one accept route (key
    /// required).
    FeedAcceptV1,
    /// `feed-undo-v1` (U-169, P26 E3): the one undo route (key required).
    FeedUndoV1,
}

impl KeySet {
    /// The capability the engine advertises while it replays this set.
    #[must_use]
    pub const fn capability(self) -> &'static str {
        match self {
            KeySet::V1 => crate::wire::Capabilities::IDEMPOTENCY,
            KeySet::ApV1 => crate::wire::Capabilities::IDEMPOTENCY_AP,
            KeySet::DocsV1 => crate::wire::Capabilities::IDEMPOTENCY_DOCS,
            KeySet::DocsAttachV1 => crate::wire::Capabilities::DOCUMENT_DRAFTS_ATTACH,
            KeySet::PayBillsV1 => crate::wire::Capabilities::IDEMPOTENCY_PAY_BILLS,
            KeySet::CheckPrintV1 => crate::wire::Capabilities::IDEMPOTENCY_CHECK_PRINT,
            KeySet::InvoiceV1 => crate::wire::Capabilities::IDEMPOTENCY_INVOICE,
            KeySet::InvoiceEmailV1 => crate::wire::Capabilities::INVOICE_EMAIL,
            KeySet::SalesDocEmailV1 => crate::wire::Capabilities::SALES_DOC_EMAIL,
            KeySet::ReceivePaymentV1 => crate::wire::Capabilities::IDEMPOTENCY_RECEIVE_PAYMENT,
            KeySet::CreditMemoV1 => crate::wire::Capabilities::IDEMPOTENCY_CREDIT_MEMO,
            KeySet::CustomerStatementsV1 => {
                crate::wire::Capabilities::IDEMPOTENCY_CUSTOMER_STATEMENTS
            }
            KeySet::FinanceChargesV1 => crate::wire::Capabilities::IDEMPOTENCY_FINANCE_CHARGES,
            KeySet::SalesTaxSetupV1 => crate::wire::Capabilities::IDEMPOTENCY_SALES_TAX,
            KeySet::SalesTaxPayV1 => crate::wire::Capabilities::IDEMPOTENCY_SALES_TAX_PAY,
            KeySet::RecoveryV1 => crate::wire::Capabilities::IDEMPOTENCY_RECOVERY,
            KeySet::InventoryV1 => crate::wire::Capabilities::INVENTORY_ADJUST,
            KeySet::UnitSetsV1 => crate::wire::Capabilities::UNITS_OF_MEASURE,
            KeySet::ReportDefsV1 => crate::wire::Capabilities::REPORT_DEFS,
            KeySet::BudgetsV1 => crate::wire::Capabilities::BUDGETS_EXACT,
            KeySet::ReportsEmailV1 => crate::wire::Capabilities::REPORTS_EMAIL,
            KeySet::BankImportV1 => crate::wire::Capabilities::BANK_IMPORT_BATCH,
            KeySet::FeedV1 => crate::wire::Capabilities::FEED_IDEMPOTENCY,
            KeySet::FeedPostV1 => crate::wire::Capabilities::FEED_MATCH_POST,
            KeySet::FeedAcceptV1 => crate::wire::Capabilities::FEED_ACCEPT,
            KeySet::FeedUndoV1 => crate::wire::Capabilities::FEED_UNDO,
        }
    }
}

/// One covered engine operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operation {
    method: Method,
    route: &'static str,
    retry: RetryClass,
    pub(crate) kind: RouteKind,
    pub(crate) timeout: TimeoutClass,
    company_scoped: bool,
}

impl Operation {
    /// HTTP method.
    #[must_use]
    pub const fn method(&self) -> Method {
        self.method
    }

    /// Route template (`/api/v1/accounts/{id}`).
    #[must_use]
    pub const fn route(&self) -> &'static str {
        self.route
    }

    /// Retry class before capability upgrades.
    #[must_use]
    pub const fn retry(&self) -> RetryClass {
        self.retry
    }

    /// Needs an open company (sends `X-Company-Id`).
    #[must_use]
    pub const fn company_scoped(&self) -> bool {
        self.company_scoped
    }

    /// In one of the engine's keyed route sets (U-18).
    #[must_use]
    pub const fn is_keyable(&self) -> bool {
        matches!(self.retry, RetryClass::Keyable(_))
    }

    /// The engine's keyed route set this route is in, if any.
    #[must_use]
    pub const fn key_set(&self) -> Option<KeySet> {
        match self.retry {
            RetryClass::Keyable(set) => Some(set),
            _ => None,
        }
    }

    /// Which failures the route reads beyond the §2.4 table: refused rows
    /// and groups only on `/batch/validate` and `/batch/save`.
    pub(crate) fn failure_shape(&self) -> crate::errors::FailureShape {
        if *self == BATCH_SAVE || *self == BATCH_VALIDATE {
            crate::errors::FailureShape::Batch
        } else {
            crate::errors::FailureShape::Plain
        }
    }
}

const fn op(
    method: Method,
    route: &'static str,
    retry: RetryClass,
    kind: RouteKind,
    timeout: TimeoutClass,
    company_scoped: bool,
) -> Operation {
    Operation {
        method,
        route,
        retry,
        kind,
        timeout,
        company_scoped,
    }
}

const fn read(route: &'static str) -> Operation {
    op(
        Method::Get,
        route,
        RetryClass::Idempotent,
        RouteKind::Session,
        TimeoutClass::Read,
        true,
    )
}

const fn write(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::PreHandlerOnly,
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-v1` route set (U-18). The set is
/// fixed for that capability name (routes added later get a new name), and
/// `tests/spec_fixtures.rs` ties these rows to the engine's `x-idempotency`.
const fn keyable(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::V1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-ap-v1` route set (U-83 to U-87):
/// keyed only while the engine advertises that capability (an engine with
/// only `idempotency-v1` does not replay these).
const fn keyable_ap(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::ApV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-docs-v1` route set (U-92a):
/// keyed only while the engine advertises that capability.
const fn keyable_docs(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::DocsV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `document-drafts-attach-v1` route set (U-95):
/// keyed only while the engine advertises that capability.
const fn keyable_docs_attach(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::DocsAttachV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-pay-bills-v1` route set (U-102):
/// keyed only while the engine advertises that capability.
const fn keyable_pay_bills(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::PayBillsV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-check-print-v1` route set
/// (U-116): keyed only while the engine advertises that capability.
const fn keyable_check_print(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::CheckPrintV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `invoice-email-v1` keyed set (U-127): keyed
/// only while the engine advertises that capability.
const fn keyable_invoice_email(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::InvoiceEmailV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-invoice-v1` route set (U-124):
/// keyed only while the engine advertises that capability.
const fn keyable_invoice(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::InvoiceV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in the engine's `idempotency-receive-payment-v1` route set
/// (U-132): keyed only while the engine advertises that capability.
const fn keyable_receive_payment(method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(KeySet::ReceivePaymentV1),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

/// A mutation in one of the P22 keyed route sets (U-137, U-140, U-141):
/// keyed only while the engine advertises that set's capability.
const fn keyable_in(set: KeySet, method: Method, route: &'static str) -> Operation {
    op(
        method,
        route,
        RetryClass::Keyable(set),
        RouteKind::Session,
        TimeoutClass::Write,
        true,
    )
}

const fn auth(route: &'static str, kind: RouteKind) -> Operation {
    op(
        Method::Post,
        route,
        RetryClass::Never,
        kind,
        TimeoutClass::Auth,
        false,
    )
}

const fn public_read(route: &'static str, timeout: TimeoutClass) -> Operation {
    op(
        Method::Get,
        route,
        RetryClass::Idempotent,
        RouteKind::Public,
        timeout,
        false,
    )
}

const fn session_read(route: &'static str) -> Operation {
    op(
        Method::Get,
        route,
        RetryClass::Idempotent,
        RouteKind::Session,
        TimeoutClass::Read,
        false,
    )
}

pub(crate) const HEALTH: Operation = public_read("/health", TimeoutClass::Probe);
pub(crate) const DESKTOP_INFO: Operation = public_read("/api/v1/desktop/info", TimeoutClass::Probe);
pub(crate) const OPENAPI: Operation = public_read("/openapi.json", TimeoutClass::Read);
/// Super-admin only and refused from the workspace, so it is company-scoped.
pub(crate) const TEST_SMTP: Operation = write(Method::Post, "/api/v1/admin/test-smtp");
pub(crate) const LOGIN: Operation = auth("/api/v1/auth/login", RouteKind::Login);
pub(crate) const TFA_VERIFY: Operation = auth("/api/v1/auth/tfa/verify", RouteKind::Tfa);
pub(crate) const REFRESH: Operation = auth("/api/v1/auth/refresh", RouteKind::Refresh);
/// Logout is not behind `authenticate`; it only reads the refresh cookie.
pub(crate) const LOGOUT: Operation = auth("/api/v1/auth/logout", RouteKind::Public);
pub(crate) const SWITCH_TENANT: Operation = auth("/api/v1/auth/switch-tenant", RouteKind::Session);
pub(crate) const CREATE_TENANT: Operation = op(
    Method::Post,
    "/api/v1/auth/create-tenant",
    RetryClass::Never,
    RouteKind::Session,
    TimeoutClass::CreateTenant,
    false,
);
pub(crate) const AUTH_ME: Operation = session_read("/api/v1/auth/me");
/// Read without `X-Company-Id`: used while opening a company to learn its id.
pub(crate) const COMPANY_GET: Operation = session_read("/api/v1/company");

pub(crate) const COMPANY_PUT: Operation = write(Method::Put, "/api/v1/company");
pub(crate) const COMPANY_SETTINGS: Operation = read("/api/v1/company/settings");
pub(crate) const ACCOUNTS_LIST: Operation = read("/api/v1/accounts");
pub(crate) const ACCOUNTS_CREATE: Operation = keyable(Method::Post, "/api/v1/accounts");
pub(crate) const ACCOUNT_GET: Operation = read("/api/v1/accounts/{id}");
pub(crate) const ACCOUNT_PUT: Operation = write(Method::Put, "/api/v1/accounts/{id}");
pub(crate) const ACCOUNT_DELETE: Operation = write(Method::Delete, "/api/v1/accounts/{id}");
pub(crate) const CONTACTS_LIST: Operation = read("/api/v1/contacts");
pub(crate) const CONTACTS_CREATE: Operation = keyable(Method::Post, "/api/v1/contacts");
pub(crate) const CONTACT_GET: Operation = read("/api/v1/contacts/{id}");
pub(crate) const CONTACT_PUT: Operation = write(Method::Put, "/api/v1/contacts/{id}");
pub(crate) const CONTACT_DELETE: Operation = write(Method::Delete, "/api/v1/contacts/{id}");
pub(crate) const ITEMS_LIST: Operation = read("/api/v1/items");
pub(crate) const ITEMS_CREATE: Operation = keyable(Method::Post, "/api/v1/items");
pub(crate) const ITEM_GET: Operation = read("/api/v1/items/{id}");
pub(crate) const ITEM_PUT: Operation = write(Method::Put, "/api/v1/items/{id}");
pub(crate) const ITEM_DELETE: Operation = write(Method::Delete, "/api/v1/items/{id}");
pub(crate) const TAGS_LIST: Operation = read("/api/v1/tags");
pub(crate) const TAGS_CREATE: Operation = keyable(Method::Post, "/api/v1/tags");
pub(crate) const TAG_GET: Operation = read("/api/v1/tags/{id}");
pub(crate) const TAG_PUT: Operation = write(Method::Put, "/api/v1/tags/{id}");
pub(crate) const TAG_DELETE: Operation = write(Method::Delete, "/api/v1/tags/{id}");
pub(crate) const TXNS_LIST: Operation = read("/api/v1/transactions");
pub(crate) const TXNS_CREATE: Operation = keyable(Method::Post, "/api/v1/transactions");
pub(crate) const TXN_GET: Operation = read("/api/v1/transactions/{id}");
pub(crate) const TXN_PUT: Operation = keyable(Method::Put, "/api/v1/transactions/{id}");
pub(crate) const TXN_VOID: Operation = keyable(Method::Post, "/api/v1/transactions/{id}/void");
pub(crate) const LEDGER_VERSION: Operation = read("/api/v1/ledger-version");

// ---- P08 company lifecycle ------------------------------------------------

/// A session-level write that is never company-scoped (tenant management).
const fn workspace_write(method: Method, route: &'static str, timeout: TimeoutClass) -> Operation {
    op(
        method,
        route,
        RetryClass::PreHandlerOnly,
        RouteKind::Session,
        timeout,
        false,
    )
}

/// A session-level write that is never retried (it creates or destroys a
/// company, or changes a credential).
const fn workspace_once(route: &'static str, kind: RouteKind, timeout: TimeoutClass) -> Operation {
    op(Method::Post, route, RetryClass::Never, kind, timeout, false)
}

pub(crate) const COMPANY_SETTINGS_PUT: Operation = write(Method::Put, "/api/v1/company/settings");
pub(crate) const SETUP_COMPLETE: Operation = write(Method::Post, "/api/v1/company/setup-complete");
pub(crate) const COA_TEMPLATE_OPTIONS: Operation =
    public_read("/api/v1/coa-templates/options", TimeoutClass::Read);
pub(crate) const ACCOUNTS_IMPORT: Operation = write(Method::Post, "/api/v1/accounts/import");
pub(crate) const OPENING_BALANCES: Operation =
    write(Method::Post, "/api/v1/export/opening-balances");
pub(crate) const INVOICES_CREATE: Operation = keyable(Method::Post, "/api/v1/invoices");
pub(crate) const BILLS_CREATE: Operation = keyable(Method::Post, "/api/v1/bills");
pub(crate) const REPORT_BALANCE_SHEET: Operation = read("/api/v1/reports/balance-sheet");
pub(crate) const REPORT_TRIAL_BALANCE: Operation = read("/api/v1/reports/trial-balance");
pub(crate) const REPORT_PROFIT_LOSS: Operation = read("/api/v1/reports/profit-loss");
/// The quick report of one account (P11).
pub(crate) const REPORT_ACCOUNT: Operation = read("/api/v1/reports/account-report");
pub(crate) const EXPORT_FULL: Operation = read("/api/v1/export/full");
pub(crate) const EXPORT_FULL_FILE: Operation = read("/api/v1/export/full/{fileName}");
/// E5a (U-05): the companies the user can open, never the workspace.
pub(crate) const TENANTS_LIST: Operation = session_read("/api/v1/tenants");
pub(crate) const TENANTS_CREATE: Operation = workspace_once(
    "/api/v1/tenants",
    RouteKind::Session,
    TimeoutClass::CreateTenant,
);
pub(crate) const TENANT_RENAME: Operation =
    workspace_write(Method::Patch, "/api/v1/tenants/{id}", TimeoutClass::Write);
pub(crate) const TENANT_ARCHIVE_EXPORT: Operation = workspace_write(
    Method::Post,
    "/api/v1/tenants/{id}/archives",
    TimeoutClass::Archive,
);
pub(crate) const TENANT_ARCHIVE_JOB: Operation =
    session_read("/api/v1/tenant-archives/jobs/{jobId}");
pub(crate) const TENANT_ARCHIVE_DOWNLOAD: Operation =
    session_read("/api/v1/tenant-archives/{fileName}/download");
pub(crate) const TENANT_ARCHIVE_IMPORT: Operation = workspace_write(
    Method::Post,
    "/api/v1/tenant-archives/import",
    TimeoutClass::Archive,
);
/// E5b: the safe delete (archive first, typed name).
pub(crate) const TENANT_DELETE: Operation = workspace_once(
    "/api/v1/tenants/{id}/delete",
    RouteKind::Session,
    TimeoutClass::Archive,
);
/// U-23: reset the owner password with the recovery key (launch token only).
pub(crate) const RECOVER_PASSWORD: Operation = workspace_once(
    "/api/v1/desktop/recover-password",
    RouteKind::Public,
    TimeoutClass::Auth,
);
/// U-23: a new recovery key, shown once.
pub(crate) const RECOVERY_KEY_ROTATE: Operation = workspace_once(
    "/api/v1/auth/recovery-key/rotate",
    RouteKind::Session,
    TimeoutClass::Auth,
);

// ---- P09 preferences, AI and SMTP ---------------------------------------

/// AI admin routes are super-admin + session; the owner always has a company
/// open in Preferences, so they are company-scoped like the consent routes.
pub(crate) const AI_CONFIG: Operation = read("/api/v1/ai/admin/config");
pub(crate) const AI_CONFIG_PUT: Operation = write(Method::Put, "/api/v1/ai/admin/config");
pub(crate) const AI_DISCLOSURE: Operation = read("/api/v1/ai/admin/disclosure");
pub(crate) const AI_DISCLOSURE_ACCEPT: Operation =
    write(Method::Post, "/api/v1/ai/admin/disclosure/accept");
pub(crate) const AI_CONSENT: Operation = read("/api/v1/ai/consent");
pub(crate) const AI_CONSENT_DISCLOSURE: Operation =
    read("/api/v1/ai/consent/{companyId}/disclosure");
pub(crate) const AI_CONSENT_ACCEPT: Operation =
    write(Method::Post, "/api/v1/ai/consent/{companyId}/accept");
pub(crate) const AI_CONSENT_REVOKE: Operation =
    write(Method::Post, "/api/v1/ai/consent/{companyId}/revoke");
pub(crate) const AI_CONSENT_TASKS: Operation =
    write(Method::Patch, "/api/v1/ai/consent/{companyId}/tasks");
pub(crate) const AI_TEST_PROVIDER: Operation =
    write(Method::Post, "/api/v1/ai/admin/test/{provider}");
pub(crate) const AI_MODELS: Operation = read("/api/v1/ai/admin/models/{provider}");
pub(crate) const AI_STATUS: Operation = read("/api/v1/ai/status");
pub(crate) const AI_USAGE: Operation = read("/api/v1/ai/usage");
pub(crate) const AI_USAGE_SUMMARY: Operation = read("/api/v1/ai/usage/summary");
pub(crate) const COMPANY_SMTP: Operation = read("/api/v1/company/smtp");
pub(crate) const COMPANY_SMTP_PUT: Operation = write(Method::Put, "/api/v1/company/smtp");
pub(crate) const COMPANY_SMTP_TEST: Operation = write(Method::Post, "/api/v1/company/smtp/test");
pub(crate) const SESSION_SECRETS_PUT: Operation = workspace_write(
    Method::Put,
    "/api/v1/desktop/session-secrets",
    TimeoutClass::Auth,
);
/// E6 session secrets: session-level, launch-token gated, never company-scoped.
pub(crate) const SESSION_SECRETS_GET: Operation = session_read("/api/v1/desktop/session-secrets");
pub(crate) const SESSION_SECRETS_DELETE: Operation = workspace_write(
    Method::Delete,
    "/api/v1/desktop/session-secrets/{name}",
    TimeoutClass::Auth,
);

// ---- P11 contacts and centers -----------------------------------------------

/// Merge two contacts (moves their transactions, deactivates the source).
pub(crate) const CONTACTS_MERGE: Operation = write(Method::Post, "/api/v1/contacts/merge");
/// A contact's transactions (the ledger list filtered to it).
pub(crate) const CONTACT_TRANSACTIONS: Operation = read("/api/v1/contacts/{id}/transactions");
/// Open invoice balance per client.
pub(crate) const REPORT_CUSTOMER_BALANCE: Operation =
    read("/api/v1/reports/customer-balance-summary");
/// Open bill balance per vendor, by age.
pub(crate) const REPORT_AP_AGING: Operation = read("/api/v1/reports/ap-aging-summary");
/// One vendor's statement from the books: opening balance, activity and
/// closing balance for a period (A06 balance line, Q-297).
pub(crate) const REPORT_VENDOR_STATEMENT: Operation = read("/api/v1/reports/vendor-statement");
/// A vendor's 1099 profile (W-9 on file, tax ID type, backup withholding).
pub(crate) const VENDOR_1099_PROFILE: Operation =
    read("/api/v1/practice/1099/vendors/{contactId}/profile");
/// Changes a vendor's 1099 profile.
pub(crate) const VENDOR_1099_PROFILE_PUT: Operation = write(
    Method::Put,
    "/api/v1/practice/1099/vendors/{contactId}/profile",
);

/// Replaces the accounts mapped to one 1099 form box (the 1099 Center).
pub(crate) const FORM_1099_ACCOUNT_MAPPINGS_PUT: Operation = write(
    Method::Put,
    "/api/v1/practice/1099/account-mappings/{formBox}",
);

// ---- P11 tags ----------------------------------------------------------------

/// Tag groups with their active tags.
pub(crate) const TAG_GROUPS_LIST: Operation = read("/api/v1/tags/groups/list");
/// New tag group.
pub(crate) const TAG_GROUPS_CREATE: Operation = write(Method::Post, "/api/v1/tags/groups");
/// Changes a tag group.
pub(crate) const TAG_GROUP_PUT: Operation = write(Method::Put, "/api/v1/tags/groups/{id}");
/// Deletes a tag group (its tags stay, ungrouped).
pub(crate) const TAG_GROUP_DELETE: Operation = write(Method::Delete, "/api/v1/tags/groups/{id}");
/// Where a tag is used.
pub(crate) const TAG_USAGE: Operation = read("/api/v1/tags/{id}/usage");
/// Merges one tag into another (header tags only until U-41).
pub(crate) const TAG_MERGE: Operation = write(Method::Post, "/api/v1/tags/merge");

// ---- P11 list import and export ------------------------------------------------

/// The Chart of Accounts as CSV.
pub(crate) const ACCOUNTS_EXPORT: Operation = read("/api/v1/accounts/export");
/// Clients and vendors as CSV.
pub(crate) const CONTACTS_EXPORT: Operation = read("/api/v1/contacts/export");
/// Creates contacts from mapped CSV rows.
pub(crate) const CONTACTS_IMPORT: Operation = write(Method::Post, "/api/v1/contacts/import");
/// Items as CSV.
pub(crate) const ITEMS_EXPORT: Operation = read("/api/v1/items/export");

// ---- P12 register, bank postings, audit, closing-date password ---------------

/// One page of an account register.
pub(crate) const REGISTER: Operation = read("/api/v1/accounts/{id}/register");
/// The register footer.
pub(crate) const REGISTER_SUMMARY: Operation = read("/api/v1/accounts/{id}/register/summary");
/// Write a check (posts an `expense` with a check number).
pub(crate) const CHECKS_CREATE: Operation = keyable(Method::Post, "/api/v1/checks");
/// The audit trail (filtered by entity type and text).
pub(crate) const AUDIT_LOG: Operation = read("/api/v1/audit-log");
/// The closing date and whether a closing-date password is set (U-15).
pub(crate) const CLOSING_PASSWORD_GET: Operation = read("/api/v1/company/closing-password");
/// Sets, changes or clears the closing-date password (U-15): a credential
/// change, never retried.
pub(crate) const CLOSING_PASSWORD_PUT: Operation = op(
    Method::Put,
    "/api/v1/company/closing-password",
    RetryClass::Never,
    RouteKind::Session,
    TimeoutClass::Auth,
    true,
);
/// Soft-deletes a transaction (R7, `txn-delete-v1`).
pub(crate) const TXN_DELETE: Operation = keyable(Method::Delete, "/api/v1/transactions/{id}");
/// The owner resets a forgotten closing-date password after re-entering
/// their sign-in password (U-15, Q-239): a credential change, never retried.
pub(crate) const CLOSING_PASSWORD_RESET: Operation = op(
    Method::Post,
    "/api/v1/company/closing-password/reset",
    RetryClass::Never,
    RouteKind::Session,
    TimeoutClass::Auth,
    true,
);

// ---- P13 WP2: payee panel, attachments, suggestions ---------------------------

/// A vendor's open bills in brief (count and total owed, overdue part).
pub(crate) const BILLS_VENDOR_SUMMARY: Operation = read("/api/v1/bills/vendor-summary");
/// Unpaid bills with their balance due.
pub(crate) const BILLS_PAYABLE: Operation = read("/api/v1/bills/payable");
/// The account to prefill for a contact (its default, else the most recent).
pub(crate) const CONTACT_SUGGEST_ACCOUNT: Operation = read("/api/v1/contacts/{id}/suggest-account");
/// Uploads a file (multipart) to a record or to a draft.
pub(crate) const ATTACHMENTS_UPLOAD: Operation = write(Method::Post, "/api/v1/attachments");
/// The files on a record.
pub(crate) const ATTACHMENTS_LIST: Operation = read("/api/v1/attachments");
/// One file's bytes.
pub(crate) const ATTACHMENT_DOWNLOAD: Operation = read("/api/v1/attachments/{id}/download");
/// AI account suggestions for typed rows (nothing is stored).
pub(crate) const AI_CATEGORIZE_PREVIEW: Operation =
    write(Method::Post, "/api/v1/ai/categorize/preview");
/// The owner's rule that matches a form (`rules-evaluate-v1`; read-only in
/// the engine, but a POST, so classed as a write).
pub(crate) const RULES_EVALUATE: Operation =
    write(Method::Post, "/api/v1/practice/conditional-rules/evaluate");

// ---- P13 WP1: write check ------------------------------------------------------

/// Checks of a bank account, newest first (Prev/Next on Write Check).
pub(crate) const CHECKS_LIST: Operation = read("/api/v1/checks");
/// Check settings (the next check number per bank account).
pub(crate) const CHECK_SETTINGS: Operation = read("/api/v1/checks/settings");
/// Edit a saved check, its number, printed name, address and Print later
/// included (`check-edit-v1`).
pub(crate) const CHECK_PUT: Operation = keyable(Method::Put, "/api/v1/checks/{id}");

// ---- P14: deposits, daily sales, owner helpers ---------------------------------

/// Payments waiting in Payments Clearing (`?exact=1` for string amounts
/// with `deposit-grouping-v1`).
pub(crate) const PENDING_DEPOSITS: Operation = read("/api/v1/payments/pending-deposits");
/// The bank feed (bank-file deposits waiting for an account).
pub(crate) const FEED_LIST: Operation = read("/api/v1/banking/feed");
/// Groups of pending payments that add up to a bank-file deposit
/// (`feed-deposit-groupings-v1`).
pub(crate) const FEED_DEPOSIT_GROUPINGS: Operation =
    read("/api/v1/banking/feed/{id}/deposit-groupings");
/// The deposit detail report (what each deposit grouped).
pub(crate) const REPORT_DEPOSIT_DETAIL: Operation = read("/api/v1/reports/deposit-detail");
/// The company's entity type and equity role accounts (`equity-roles-v1`).
pub(crate) const EQUITY_ROLES: Operation = read("/api/v1/company/equity-roles");
/// Daily-sales layouts.
pub(crate) const DAILY_TEMPLATES: Operation = read("/api/v1/daily-sales/templates");
/// Creates a daily-sales layout (blank or from a preset).
pub(crate) const DAILY_TEMPLATES_CREATE: Operation =
    write(Method::Post, "/api/v1/daily-sales/templates");
/// One daily-sales layout with its lines.
pub(crate) const DAILY_TEMPLATE_GET: Operation = read("/api/v1/daily-sales/templates/{id}");
/// Renames a layout or changes its default tag.
pub(crate) const DAILY_TEMPLATE_PUT: Operation =
    write(Method::Put, "/api/v1/daily-sales/templates/{id}");
/// Deletes a layout.
pub(crate) const DAILY_TEMPLATE_DELETE: Operation =
    write(Method::Delete, "/api/v1/daily-sales/templates/{id}");
/// Replaces a layout's lines.
pub(crate) const DAILY_TEMPLATE_LINES: Operation =
    write(Method::Put, "/api/v1/daily-sales/templates/{id}/lines");
/// Daily-sales days (by status and date).
pub(crate) const DAILY_ENTRIES: Operation = read("/api/v1/daily-sales/entries");
/// Saves a day as a draft.
pub(crate) const DAILY_ENTRIES_CREATE: Operation =
    keyable(Method::Post, "/api/v1/daily-sales/entries");
/// The posting a day would make, with its over/short (nothing is stored;
/// a POST, so classed as a write).
pub(crate) const DAILY_ENTRY_PREVIEW: Operation =
    write(Method::Post, "/api/v1/daily-sales/entries/preview");
/// One day.
pub(crate) const DAILY_ENTRY_GET: Operation = read("/api/v1/daily-sales/entries/{id}");
/// Changes a draft day.
pub(crate) const DAILY_ENTRY_PUT: Operation =
    write(Method::Put, "/api/v1/daily-sales/entries/{id}");
/// Deletes a draft day.
pub(crate) const DAILY_ENTRY_DELETE: Operation =
    write(Method::Delete, "/api/v1/daily-sales/entries/{id}");
/// Posts a day (`closingPassword` with `daily-sales-lock-v1`).
pub(crate) const DAILY_ENTRY_POST: Operation =
    keyable(Method::Post, "/api/v1/daily-sales/entries/{id}/post");
/// Voids a posted day (`closingPassword` with `daily-sales-lock-v1`).
pub(crate) const DAILY_ENTRY_VOID: Operation =
    keyable(Method::Post, "/api/v1/daily-sales/entries/{id}/void");

// ---- P15: journal entries, templates, catch-up ----------------------------------
// Client-declared (outside the pinned spec) until the P15 engine PRs land.

/// The suggested next journal entry number (`je-number-v1`).
pub(crate) const JE_NEXT_NUMBER: Operation = read("/api/v1/journal-entries/next-number");
/// Templates, by group and kind (`templates-v1`).
pub(crate) const TEMPLATES_LIST: Operation = read("/api/v1/templates");
/// Stores a template (a form's create body minus the date and numbers).
pub(crate) const TEMPLATES_CREATE: Operation = keyable(Method::Post, "/api/v1/templates");
/// Stores a template from a posted transaction.
pub(crate) const TEMPLATES_FROM_TXN: Operation =
    keyable(Method::Post, "/api/v1/templates/from-transaction");
/// Template groups.
pub(crate) const TEMPLATE_GROUPS: Operation = read("/api/v1/templates/groups");
/// Creates a template group.
pub(crate) const TEMPLATE_GROUPS_CREATE: Operation =
    write(Method::Post, "/api/v1/templates/groups");
/// Renames a template group.
pub(crate) const TEMPLATE_GROUP_PUT: Operation =
    write(Method::Put, "/api/v1/templates/groups/{id}");
/// Deletes a template group (its templates move to no group).
pub(crate) const TEMPLATE_GROUP_DELETE: Operation =
    write(Method::Delete, "/api/v1/templates/groups/{id}");
/// One template with its draft and schedule.
pub(crate) const TEMPLATE_GET: Operation = read("/api/v1/templates/{id}");
/// Changes a template (`version`; 409 `STALE`).
pub(crate) const TEMPLATE_PUT: Operation = write(Method::Put, "/api/v1/templates/{id}");
/// Deletes a template (`?version=`).
pub(crate) const TEMPLATE_DELETE: Operation = write(Method::Delete, "/api/v1/templates/{id}");
/// A template's draft for a date, with warnings (posts nothing).
pub(crate) const TEMPLATE_DRAFT: Operation = read("/api/v1/templates/{id}/draft");
/// Posts one occurrence now (`closingPassword` in a closed period).
pub(crate) const TEMPLATE_ENTER: Operation = keyable(Method::Post, "/api/v1/templates/{id}/enter");
/// Dismisses one occurrence.
pub(crate) const TEMPLATE_SKIP: Operation = keyable(
    Method::Post,
    "/api/v1/templates/{id}/occurrences/{date}/skip",
);
/// Links an occurrence to a transaction the owner saved in a form.
pub(crate) const TEMPLATE_RESOLVE: Operation = keyable(
    Method::Post,
    "/api/v1/templates/{id}/occurrences/{date}/resolve",
);
/// Posts the due automatic occurrences (`recurring-run-due-v1`; repeatable
/// with the same `runId`).
pub(crate) const RECURRING_RUN_DUE: Operation = write(Method::Post, "/api/v1/recurring/run-due");
/// Due reminders and open skipped occurrences (no side effects).
pub(crate) const RECURRING_ATTENTION: Operation = read("/api/v1/recurring/attention");

// ---- P15A: Bulk Entry -------------------------------------------------------------
// In the pinned spec; the `batch-*-v1` behavior is client-declared until the
// P15A engine PR lands (lane A adds the calls).

/// Resolves names and accounts and reports problems per row (posts
/// nothing; a POST, so classed as a write).
pub(crate) const BATCH_VALIDATE: Operation = write(Method::Post, "/api/v1/batch/validate");
/// Posts every row in one transaction (`batch-atomic-v1`).
pub(crate) const BATCH_SAVE: Operation = write(Method::Post, "/api/v1/batch/save");
/// The default or most recent account for a name (`?contact_id=`).
pub(crate) const BATCH_SUGGEST_ACCOUNT: Operation = read("/api/v1/batch/suggest-account");
/// Reads a CSV or pasted file (multipart) into rows of text.
pub(crate) const BATCH_PARSE_CSV: Operation = write(Method::Post, "/api/v1/batch/parse-csv");

// ---- P16 WP6a: Home ----------------------------------------------------------------
// In the pinned spec; exact money and `asOf` (`dashboard-exact-v1`) arrive with
// engine PR #168.

/// Bank, cash and card balances the engine stores (`exact=1`, `asOf`).
pub(crate) const DASHBOARD_CASH: Operation = read("/api/v1/dashboard/cash-position");
/// Open and overdue invoices.
pub(crate) const DASHBOARD_RECEIVABLES: Operation = read("/api/v1/dashboard/receivables");
/// Open, overdue and soon-due bills (`dueWithinDays`).
pub(crate) const DASHBOARD_PAYABLES: Operation = read("/api/v1/dashboard/payables");
/// Counts of things waiting on a person.
pub(crate) const DASHBOARD_ACTIONS: Operation = read("/api/v1/dashboard/action-items");
/// Every dashboard panel in one request, with the `asOf` it used.
pub(crate) const DASHBOARD_SUMMARY: Operation = read("/api/v1/dashboard/summary");
/// Bills by status and due date (Home counts read `total` only).
pub(crate) const BILLS_LIST: Operation = read("/api/v1/bills");
/// Payroll imports by status (Home counts the unposted ones).
pub(crate) const PAYROLL_SESSIONS: Operation = read("/api/v1/payroll-import/sessions");
/// One account's reconciliations, newest statement first.
pub(crate) const RECONCILIATIONS: Operation = read("/api/v1/banking/reconciliations");
/// Imported bank statements.
pub(crate) const BANK_STATEMENTS: Operation = read("/api/v1/banking/statements");

// ---- P17: Enter Bill, vendor credits, receipt inbox, document reading ---------
// `BILLS_LIST` (P16) and `BILLS_CREATE` (P08) are above. Engine PR-A
// (U-83 to U-87) is in the pin: the vendor credit edit, the terms dates and
// the payment terms list are in its spec, and the A/P document mutations are
// keyed under `idempotency-ap-v1`. Bill and credit delete use `TXN_DELETE`
// (U-84b).

/// One bill with its lines.
pub(crate) const BILL_GET: Operation = read("/api/v1/bills/{id}");
/// Changes a bill (`expectedUpdatedAt` and `closingPassword` with
/// `ap-edit-v1` / `ap-closing-v1`).
pub(crate) const BILL_PUT: Operation = keyable_ap(Method::Put, "/api/v1/bills/{id}");
/// Voids an unpaid bill.
pub(crate) const BILL_VOID: Operation = keyable_ap(Method::Post, "/api/v1/bills/{id}/void");
/// The due and discount dates a bill would get (`bill-terms-v1`, U-86d).
pub(crate) const BILL_TERMS_DATES: Operation = read("/api/v1/bills/terms-dates");
/// The fixed payment terms list (`bill-terms-v1`, U-86e).
pub(crate) const PAYMENT_TERMS: Operation = read("/api/v1/payment-terms");
/// Links the files of a draft to a saved bill.
pub(crate) const BILL_ATTACH_DRAFT: Operation =
    write(Method::Post, "/api/v1/bills/{id}/attach-draft");
/// Reads a bill out of an uploaded file with the owner's AI provider (only
/// on the owner's click, Q-73; a POST, so classed as a write).
pub(crate) const BILL_EXTRACT: Operation = write(
    Method::Post,
    "/api/v1/bills/extract-from-attachment/{attachmentId}",
);
/// Vendor credits by vendor, date and text.
pub(crate) const VENDOR_CREDITS_LIST: Operation = read("/api/v1/vendor-credits");
/// Records a vendor credit.
pub(crate) const VENDOR_CREDITS_CREATE: Operation =
    keyable_ap(Method::Post, "/api/v1/vendor-credits");
/// One vendor credit with its lines.
pub(crate) const VENDOR_CREDIT_GET: Operation = read("/api/v1/vendor-credits/{id}");
/// Changes an unapplied vendor credit (`ap-edit-v1`, U-84a).
pub(crate) const VENDOR_CREDIT_PUT: Operation =
    keyable_ap(Method::Put, "/api/v1/vendor-credits/{id}");
/// Voids a vendor credit.
pub(crate) const VENDOR_CREDIT_VOID: Operation =
    keyable_ap(Method::Post, "/api/v1/vendor-credits/{id}/void");
/// A vendor's credits with an unused balance.
pub(crate) const VENDOR_CREDITS_AVAILABLE: Operation =
    read("/api/v1/vendor-credits/available/{vendorId}");
/// One file's details.
pub(crate) const ATTACHMENT_GET: Operation = read("/api/v1/attachments/{id}");
/// Deletes a file.
pub(crate) const ATTACHMENT_DELETE: Operation = write(Method::Delete, "/api/v1/attachments/{id}");
/// Links a file to a record.
pub(crate) const ATTACHMENT_LINK: Operation = write(Method::Post, "/api/v1/attachments/{id}/link");
/// Files not linked to any record.
pub(crate) const ATTACHMENTS_UNLINKED: Operation = read("/api/v1/attachments/unlinked");
/// Bill payments by vendor and date (statement match).
pub(crate) const BILL_PAYMENTS_LIST: Operation = read("/api/v1/bill-payments");
/// Voids a bill payment and takes its applications off the bills and
/// credits it paid (the register's A/P void guard, review B1).
pub(crate) const BILL_PAYMENT_VOID: Operation =
    keyable_ap(Method::Post, "/api/v1/bill-payments/{id}/void");

// ---- A01: intake folders (engine U-93, `intake-settings-v1`) ---------------
// In the engine spec since pin a88337e3 (engine PR #177).

/// The company's intake folders and their change token.
pub(crate) const INTAKE_SETTINGS_GET: Operation = read("/api/v1/company/intake-settings");
/// Replaces the folder list (`expectedUpdatedAt`, 409 `STALE`). Not in an
/// idempotency set: a retried full replace either matches or is `STALE`.
pub(crate) const INTAKE_SETTINGS_PUT: Operation =
    write(Method::Put, "/api/v1/company/intake-settings");

// ---- A03: document drafts and vendor layouts (engine U-92, `document-drafts-v1`)
// In the engine spec since pin 38cf97eb (engine PR #178).

/// Claims an uploaded draft file as a document draft (keyed under
/// `idempotency-docs-v1`; 409 `DOCUMENT_ALREADY_CLAIMED`).
pub(crate) const DOCUMENT_DRAFTS_CREATE: Operation =
    keyable_docs(Method::Post, "/api/v1/document-drafts");
/// Document drafts by status and kind, newest first (`view=summary`).
pub(crate) const DOCUMENT_DRAFTS_LIST: Operation = read("/api/v1/document-drafts");
/// One document draft with its reading.
pub(crate) const DOCUMENT_DRAFT_GET: Operation = read("/api/v1/document-drafts/{id}");
/// Posted records and live drafts that look like the same document.
pub(crate) const DOCUMENT_DRAFT_DUPLICATES: Operation =
    read("/api/v1/document-drafts/{id}/duplicates");
/// Stores a reading or a failure (`expectedUpdatedAt`, 409 `STALE`). Not
/// in an idempotency set: a repeat is `STALE` or stores the same reading.
pub(crate) const DOCUMENT_DRAFT_PUT: Operation = write(Method::Put, "/api/v1/document-drafts/{id}");
/// Discards a draft (frees the claim; an already discarded one is 200).
pub(crate) const DOCUMENT_DRAFT_DISCARD: Operation =
    write(Method::Post, "/api/v1/document-drafts/{id}/discard");
/// Posts a draft as a bill, card charge, cash expense or check in one
/// database transaction (keyed under `idempotency-docs-v1`).
pub(crate) const DOCUMENT_DRAFT_CONVERT: Operation =
    keyable_docs(Method::Post, "/api/v1/document-drafts/{id}/convert");
/// The company's vendor layout templates (`?contactId=`).
pub(crate) const VENDOR_TEMPLATES_LIST: Operation = read("/api/v1/vendor-layout-templates");
/// Creates or replaces a vendor layout template (`expectedUpdatedAt`).
pub(crate) const VENDOR_TEMPLATES_PUT: Operation =
    write(Method::Put, "/api/v1/vendor-layout-templates");
/// Deletes a vendor layout template.
pub(crate) const VENDOR_TEMPLATE_DELETE: Operation =
    write(Method::Delete, "/api/v1/vendor-layout-templates/{id}");

// ---- A03: document reading assist (engine U-92d, `document-assist-v1`)
// In the engine spec since pin 38cf97eb (engine PR #179).

/// A second reading of a document's page text (and page images) by the
/// company's AI provider. Nothing is stored, but it spends tokens: never
/// retried after it may have been sent.
pub(crate) const DOCUMENT_READING_ASSIST: Operation =
    write(Method::Post, "/api/v1/document-reading/assist");

// ---- A04: attach a document draft (engine U-95, `document-drafts-attach-v1`)
// In the engine spec since pin 4e0593c3 (engine PR #180).

/// Files a live draft's document on a posted transaction and closes the
/// draft, in one database transaction (keyed under
/// `document-drafts-attach-v1`; nothing posts).
pub(crate) const DOCUMENT_DRAFT_ATTACH: Operation =
    keyable_docs_attach(Method::Post, "/api/v1/document-drafts/{id}/attach");

// ---- P18: Pay Bills (engine U-101 to U-104) --------------------------------
// Both routes are in the pinned spec; U-101 to U-103 add body fields under
// their capabilities and U-102 keys the pay (`idempotency-pay-bills-v1`).

/// Pays bills: one bill payment per vendor in one database transaction
/// (keyed only under `idempotency-pay-bills-v1`).
pub(crate) const BILL_PAYMENTS_CREATE: Operation =
    keyable_pay_bills(Method::Post, "/api/v1/bill-payments");
/// One bill payment with its bill and credit applications.
pub(crate) const BILL_PAYMENT_GET: Operation = read("/api/v1/bill-payments/{id}");

// ---- P19: form printing (engine U-106 to U-111, U-113) ------------------------
// None of these routes is in an idempotency set: the print routes are plain
// mutations (retried only when provably not sent).

/// Checks waiting to print (`check-reprint-audit-v1` adds the mailing rows).
pub(crate) const CHECKS_PRINT_QUEUE: Operation = read("/api/v1/checks/print-queue");
/// Numbers queued checks and marks them printed (the numbers are recorded
/// before anything goes to the printer).
pub(crate) const CHECKS_PRINT: Operation =
    keyable_check_print(Method::Post, "/api/v1/checks/print");
/// Sends printed checks back to the queue (numbers spoiled and logged).
pub(crate) const CHECKS_REQUEUE: Operation = write(Method::Post, "/api/v1/checks/requeue");
/// Sends a whole printed batch back to the queue.
pub(crate) const CHECKS_REPRINT: Operation =
    write(Method::Post, "/api/v1/checks/reprint/{batchId}");
/// Check numbers spoiled by reprints (reusable; listing only).
pub(crate) const CHECKS_SPOILED: Operation = read("/api/v1/checks/spoiled");
/// Check settings update (the per-account check style, U-109).
pub(crate) const CHECK_SETTINGS_PUT: Operation = write(Method::Put, "/api/v1/checks/settings");
/// The company's uploaded logo bytes (`company-logo-v1`).
pub(crate) const COMPANY_LOGO: Operation = read("/api/v1/company/logo");
/// Uploads the company logo (multipart field `logo`).
pub(crate) const COMPANY_LOGO_UPLOAD: Operation = write(Method::Post, "/api/v1/company/logo");
/// The company's form template with the footer each form type prints.
pub(crate) const INVOICE_TEMPLATE: Operation = read("/api/v1/company/invoice-template");
/// Updates the footer text and the per-type overrides.
pub(crate) const INVOICE_TEMPLATE_PUT: Operation =
    write(Method::Put, "/api/v1/company/invoice-template");
/// Prefix and next number of every document type (`document-numbers-v1`).
pub(crate) const DOCUMENT_NUMBERS: Operation = read("/api/v1/document-numbers");
/// The number the next document of a type would get (nothing taken).
pub(crate) const DOCUMENT_NUMBER_NEXT: Operation = read("/api/v1/document-numbers/next");
/// Sets the prefix and/or next number of one type.
pub(crate) const DOCUMENT_NUMBER_PUT: Operation =
    write(Method::Put, "/api/v1/document-numbers/{type}");
/// Takes the next number of a type (statements).
pub(crate) const DOCUMENT_NUMBER_CLAIM: Operation =
    write(Method::Post, "/api/v1/document-numbers/claim");
/// Invoices, sales receipts and credit memos marked Print later (U-113).
pub(crate) const FORMS_PRINT_QUEUE: Operation = read("/api/v1/forms/print-queue");
/// Clears the Print later mark after printing; one audit row each.
pub(crate) const FORMS_MARK_PRINTED: Operation = write(Method::Post, "/api/v1/forms/mark-printed");

// ---- P20 Create Invoice (engine U-122 to U-129, U-38) ------------------------

/// Invoices (the transaction list filters plus `sent`, U-126).
pub(crate) const INVOICES_LIST: Operation = read("/api/v1/invoices");
/// One invoice with its lines, fields and send history.
pub(crate) const INVOICE_GET: Operation = read("/api/v1/invoices/{id}");
/// Edits an invoice (`invoice-edit-v1`; keyed under `idempotency-invoice-v1`).
pub(crate) const INVOICE_PUT: Operation = keyable_invoice(Method::Put, "/api/v1/invoices/{id}");
/// Voids an invoice (409 `INVOICE_HAS_PAYMENTS` while payments are applied).
pub(crate) const INVOICE_VOID: Operation =
    keyable_invoice(Method::Post, "/api/v1/invoices/{id}/void");
/// Records that an invoice was printed, emailed or sent as a PDF (U-126).
pub(crate) const INVOICE_SENT: Operation =
    keyable_invoice(Method::Post, "/api/v1/invoices/{id}/sent");
/// Every figure of an invoice, posting nothing (U-129; contract per P20
/// plan §2, verify when E2 lands).
pub(crate) const INVOICE_PREVIEW: Operation = write(Method::Post, "/api/v1/invoices/preview");
/// Emails an invoice through the company SMTP (U-127; keyed under
/// `invoice-email-v1`: a retry with the same key sends once).
pub(crate) const INVOICE_EMAIL: Operation =
    keyable_invoice_email(Method::Post, "/api/v1/invoices/{id}/email");
/// Puts an email in the Outbox (U-127, Q-P20q; keyed under
/// `invoice-email-v1`).
pub(crate) const INVOICE_EMAIL_LATER: Operation =
    keyable_invoice_email(Method::Post, "/api/v1/invoices/{id}/email-later");
/// The Outbox (U-127; contract, verify with E4).
pub(crate) const EMAIL_QUEUE: Operation = read("/api/v1/forms/email-queue");
/// Takes one message out of the Outbox (U-127; contract, verify with E4).
pub(crate) const EMAIL_QUEUE_DELETE: Operation =
    write(Method::Delete, "/api/v1/forms/email-queue/{id}");
/// A client's unbilled costs (U-128; contract, verify when E3 lands).
pub(crate) const BILLABLE_COSTS: Operation = read("/api/v1/billable-costs");
/// Invoice print layouts, default first (U-130, `invoice-layouts-v1`).
pub(crate) const INVOICE_LAYOUTS_LIST: Operation = read("/api/v1/company/invoice-layouts");
/// Creates an invoice layout (U-130).
pub(crate) const INVOICE_LAYOUTS_CREATE: Operation =
    write(Method::Post, "/api/v1/company/invoice-layouts");
/// One invoice layout (U-130).
pub(crate) const INVOICE_LAYOUT_GET: Operation = read("/api/v1/company/invoice-layouts/{id}");
/// Edits, archives or unarchives an invoice layout (U-130).
pub(crate) const INVOICE_LAYOUT_PUT: Operation =
    write(Method::Put, "/api/v1/company/invoice-layouts/{id}");
/// Makes an invoice layout the company default (U-130).
pub(crate) const INVOICE_LAYOUT_DEFAULT: Operation =
    write(Method::Post, "/api/v1/company/invoice-layouts/{id}/default");
/// Price levels (U-40; contract per P20 plan §2, verify when E2 lands).
pub(crate) const PRICE_LEVELS_LIST: Operation = read("/api/v1/price-levels");
/// Creates a price level (contract, verify with E2).
pub(crate) const PRICE_LEVELS_CREATE: Operation = write(Method::Post, "/api/v1/price-levels");
/// One price level with its item prices (contract, verify with E2).
pub(crate) const PRICE_LEVEL_GET: Operation = read("/api/v1/price-levels/{id}");
/// Replaces a price level (contract, verify with E2).
pub(crate) const PRICE_LEVEL_PUT: Operation = write(Method::Put, "/api/v1/price-levels/{id}");
/// Deletes a price level (contract, verify with E2).
pub(crate) const PRICE_LEVEL_DELETE: Operation = write(Method::Delete, "/api/v1/price-levels/{id}");

// ---- P21 Receive Payment (engine U-132 to U-136) ----------------------------

/// Every figure of a customer payment, posting nothing (U-132; contract per
/// the P21 plan §2.2, verify at the pin bump).
pub(crate) const PAYMENT_PREVIEW: Operation = write(Method::Post, "/api/v1/payments/preview");
/// Receives a customer payment (U-132: one transaction, exact amounts).
pub(crate) const PAYMENT_RECEIVE: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/payments/receive");
/// Customer payments by client and date (contract, verify at the pin bump).
pub(crate) const PAYMENTS_LIST: Operation = read("/api/v1/payments");
/// One customer payment with its applications (contract).
pub(crate) const PAYMENT_GET: Operation = read("/api/v1/payments/{id}");
/// Edits a customer payment (Q-P21o; contract).
pub(crate) const PAYMENT_PUT: Operation =
    keyable_receive_payment(Method::Put, "/api/v1/payments/{id}");
/// Voids a customer payment and reopens its invoices (contract).
pub(crate) const PAYMENT_VOID: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/payments/{id}/void");
/// Takes one invoice off a payment.
pub(crate) const PAYMENT_UNAPPLY: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/payments/unapply");
/// Applies a credit or a held deposit to invoices (U-133, U-135; contract).
pub(crate) const PAYMENT_APPLY: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/payments/{id}/apply");
/// Records a bounced check (U-132; contract).
pub(crate) const PAYMENT_BOUNCE: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/payments/{id}/bounce");
/// A client's open invoices.
pub(crate) const PAYMENT_OPEN_INVOICES: Operation =
    read("/api/v1/payments/open-invoices/{customerId}");
/// A client's held deposits with what is left of each (U-135; contract).
pub(crate) const PAYMENT_DEPOSITS_ON_HAND: Operation = read("/api/v1/payments/deposits-on-hand");
/// Refunds a client's credit or held deposit (U-133: the canonical
/// `customer_refund` route, Receive Payment permission; contract).
pub(crate) const CUSTOMER_REFUND_CREATE: Operation =
    keyable_receive_payment(Method::Post, "/api/v1/customer-refunds");

// ---- P22 credit memos, sales receipts, refunds, statements, finance charges --
// Contract per the P22 plan §2.2-§2.7 (U-137 to U-143, U-112); verify at
// the D8 pin bump.

const fn keyable_credit_memo(method: Method, route: &'static str) -> Operation {
    keyable_in(KeySet::CreditMemoV1, method, route)
}

/// A client's open credits: payment credits and credit memos (U-137).
pub(crate) const PAYMENT_CREDITS: Operation = read("/api/v1/payments/credits");
/// Every figure of a credit memo, posting nothing (U-138).
pub(crate) const CREDIT_MEMO_PREVIEW: Operation =
    write(Method::Post, "/api/v1/credit-memos/preview");
/// Creates a credit memo, applying it in the same step (U-137, U-138).
pub(crate) const CREDIT_MEMO_CREATE: Operation =
    keyable_credit_memo(Method::Post, "/api/v1/credit-memos");
/// One credit memo with its applications and refunds.
pub(crate) const CREDIT_MEMO_GET: Operation = read("/api/v1/credit-memos/{id}");
/// Edits a credit memo (`expectedUpdatedAt`).
pub(crate) const CREDIT_MEMO_PUT: Operation =
    keyable_credit_memo(Method::Put, "/api/v1/credit-memos/{id}");
/// Applies a credit memo to invoices.
pub(crate) const CREDIT_MEMO_APPLY: Operation =
    keyable_credit_memo(Method::Post, "/api/v1/credit-memos/{id}/apply");
/// Takes one invoice off a credit memo.
pub(crate) const CREDIT_MEMO_UNAPPLY: Operation =
    keyable_credit_memo(Method::Post, "/api/v1/credit-memos/{id}/unapply");
/// Every figure of a sales receipt, posting nothing (U-143).
pub(crate) const SALES_RECEIPT_PREVIEW: Operation =
    write(Method::Post, "/api/v1/sales-receipts/preview");
/// Creates a sales receipt (U-143; keyed under `idempotency-credit-memo-v1`,
/// a desktop-side assumption until the pin bump).
pub(crate) const SALES_RECEIPT_CREATE: Operation =
    keyable_credit_memo(Method::Post, "/api/v1/sales-receipts");
/// One sales receipt.
pub(crate) const SALES_RECEIPT_GET: Operation = read("/api/v1/sales-receipts/{id}");
/// Edits a sales receipt.
pub(crate) const SALES_RECEIPT_PUT: Operation =
    keyable_credit_memo(Method::Put, "/api/v1/sales-receipts/{id}");
/// One customer refund with its source (U-139).
pub(crate) const CUSTOMER_REFUND_GET: Operation = read("/api/v1/customer-refunds/{id}");
/// One client's statement as of a date (U-112).
pub(crate) const REPORT_CUSTOMER_STATEMENT: Operation = read("/api/v1/reports/customer-statement");
/// The statement batch picker (U-112).
pub(crate) const REPORT_CUSTOMER_STATEMENTS: Operation =
    read("/api/v1/reports/customer-statements");
/// The A/R aging summary (U-142: exact string fields and client rows).
pub(crate) const REPORT_AR_AGING: Operation = read("/api/v1/reports/ar-aging-summary");
/// Issues numbered statement snapshots (U-140).
pub(crate) const STATEMENT_RUN_CREATE: Operation = keyable_in(
    KeySet::CustomerStatementsV1,
    Method::Post,
    "/api/v1/customer-statement-runs",
);
/// A client's statement history, newest first (U-140).
pub(crate) const STATEMENTS_LIST: Operation = read("/api/v1/customer-statements");
/// One issued statement (its snapshot).
pub(crate) const STATEMENT_GET: Operation = read("/api/v1/customer-statements/{id}");
/// Records that a statement was printed, emailed or saved as a PDF.
pub(crate) const STATEMENT_SENT: Operation = keyable_in(
    KeySet::CustomerStatementsV1,
    Method::Post,
    "/api/v1/customer-statements/{id}/sent",
);
/// Emails a statement through the company SMTP (U-140).
pub(crate) const STATEMENT_EMAIL: Operation = keyable_in(
    KeySet::CustomerStatementsV1,
    Method::Post,
    "/api/v1/customer-statements/{id}/email",
);
/// Puts a statement email in the Outbox (U-140).
pub(crate) const STATEMENT_EMAIL_LATER: Operation = keyable_in(
    KeySet::CustomerStatementsV1,
    Method::Post,
    "/api/v1/customer-statements/{id}/email-later",
);
/// Every finance charge the date would assess, posting nothing (U-141).
pub(crate) const FINANCE_CHARGE_PREVIEW: Operation =
    write(Method::Post, "/api/v1/finance-charges/preview");
/// Assesses finance charges, all or nothing (U-141).
pub(crate) const FINANCE_CHARGE_ASSESS: Operation = keyable_in(
    KeySet::FinanceChargesV1,
    Method::Post,
    "/api/v1/finance-charges/assess",
);
/// Finance-charge assessments of a period (U-141).
pub(crate) const FINANCE_CHARGES_LIST: Operation = read("/api/v1/finance-charges");
/// Emails a credit memo through the company SMTP (U-144).
pub(crate) const CREDIT_MEMO_EMAIL: Operation = keyable_in(
    KeySet::SalesDocEmailV1,
    Method::Post,
    "/api/v1/credit-memos/{id}/email",
);
/// Puts a credit memo email in the Outbox (U-144).
pub(crate) const CREDIT_MEMO_EMAIL_LATER: Operation = keyable_in(
    KeySet::SalesDocEmailV1,
    Method::Post,
    "/api/v1/credit-memos/{id}/email-later",
);
/// Emails a sales receipt through the company SMTP (U-144).
pub(crate) const SALES_RECEIPT_EMAIL: Operation = keyable_in(
    KeySet::SalesDocEmailV1,
    Method::Post,
    "/api/v1/sales-receipts/{id}/email",
);
/// Puts a sales receipt email in the Outbox (U-144).
pub(crate) const SALES_RECEIPT_EMAIL_LATER: Operation = keyable_in(
    KeySet::SalesDocEmailV1,
    Method::Post,
    "/api/v1/sales-receipts/{id}/email-later",
);

// ---- P23 sales tax ----------------------------------------------------------------
// Contract per the P23 plan §2.4-§2.8 (U-147 to U-154, U-25); verify at the
// WP-4 pin bump.
/// The sales tax agencies (U-147).
pub(crate) const SALES_TAX_AGENCIES: Operation = read("/api/v1/sales-tax/agencies");
/// Makes a vendor a sales tax agency (U-147).
pub(crate) const SALES_TAX_AGENCY_CREATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/agencies",
);
/// Edits an agency (`expectedUpdatedAt`).
pub(crate) const SALES_TAX_AGENCY_PUT: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Put,
    "/api/v1/sales-tax/agencies/{id}",
);
/// Deactivates an agency (409 `AGENCY_IN_USE`).
pub(crate) const SALES_TAX_AGENCY_DEACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/agencies/{id}/deactivate",
);
/// Reactivates an agency.
pub(crate) const SALES_TAX_AGENCY_REACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/agencies/{id}/reactivate",
);
/// Sales tax items and groups (U-147).
pub(crate) const SALES_TAX_ITEMS: Operation = read("/api/v1/sales-tax/items");
/// Creates a sales tax item or group.
pub(crate) const SALES_TAX_ITEM_CREATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/items",
);
/// Edits a sales tax item or group (`expectedUpdatedAt`).
pub(crate) const SALES_TAX_ITEM_PUT: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Put,
    "/api/v1/sales-tax/items/{id}",
);
/// Deactivates a sales tax item or group (409 `ITEM_IN_GROUP`).
pub(crate) const SALES_TAX_ITEM_DEACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/items/{id}/deactivate",
);
/// Reactivates a sales tax item or group.
pub(crate) const SALES_TAX_ITEM_REACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/items/{id}/reactivate",
);
/// Tax codes (U-25).
pub(crate) const SALES_TAX_CODES: Operation = read("/api/v1/sales-tax/codes");
/// Creates a tax code.
pub(crate) const SALES_TAX_CODE_CREATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/codes",
);
/// Edits a tax code (409 `SYSTEM_CODE` for Tax and Non).
pub(crate) const SALES_TAX_CODE_PUT: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Put,
    "/api/v1/sales-tax/codes/{id}",
);
/// Deactivates a tax code (409 `CODE_IN_USE`).
pub(crate) const SALES_TAX_CODE_DEACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/codes/{id}/deactivate",
);
/// Reactivates a tax code.
pub(crate) const SALES_TAX_CODE_REACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/codes/{id}/reactivate",
);
/// Sales tax owed as of a date by agency and item (U-149).
pub(crate) const SALES_TAX_OWED: Operation = read("/api/v1/sales-tax/owed");
/// Exemption certificates (U-151).
pub(crate) const SALES_TAX_EXEMPTIONS: Operation = read("/api/v1/sales-tax/exemptions");
/// Records an exemption certificate.
pub(crate) const SALES_TAX_EXEMPTION_CREATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/exemptions",
);
/// Edits an exemption certificate (`expectedUpdatedAt`).
pub(crate) const SALES_TAX_EXEMPTION_PUT: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Put,
    "/api/v1/sales-tax/exemptions/{id}",
);
/// Deactivates an exemption certificate.
pub(crate) const SALES_TAX_EXEMPTION_DEACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/exemptions/{id}/deactivate",
);
/// Reactivates an exemption certificate.
pub(crate) const SALES_TAX_EXEMPTION_REACTIVATE: Operation = keyable_in(
    KeySet::SalesTaxSetupV1,
    Method::Post,
    "/api/v1/sales-tax/exemptions/{id}/reactivate",
);
/// Certificates expired or expiring within a window (U-151).
pub(crate) const REPORT_EXEMPTIONS_EXPIRING: Operation =
    read("/api/v1/reports/exemptions-expiring");
/// Every figure of a sales tax payment, posting nothing (U-150).
pub(crate) const SALES_TAX_PAY_PREVIEW: Operation =
    write(Method::Post, "/api/v1/sales-tax/payments/preview");
/// Pays sales tax: a `sales_tax_payment` check (U-150).
pub(crate) const SALES_TAX_PAY_CREATE: Operation = keyable_in(
    KeySet::SalesTaxPayV1,
    Method::Post,
    "/api/v1/sales-tax/payments",
);
/// Links an outside payment that debited Sales Tax Payable (U-150).
pub(crate) const SALES_TAX_PAY_LINK: Operation = keyable_in(
    KeySet::SalesTaxPayV1,
    Method::Post,
    "/api/v1/sales-tax/payments/link",
);
/// Undoes a link (U-150).
pub(crate) const SALES_TAX_PAY_UNLINK: Operation = keyable_in(
    KeySet::SalesTaxPayV1,
    Method::Post,
    "/api/v1/sales-tax/payments/{id}/unlink",
);
/// Records tax paid outside these books through a date (U-150).
pub(crate) const SALES_TAX_PAY_MARK_PAID: Operation = keyable_in(
    KeySet::SalesTaxPayV1,
    Method::Post,
    "/api/v1/sales-tax/payments/mark-paid-through",
);
/// Undoes one "Mark paid through" run (U-150 review fix; a second undo of
/// the same run is 409 `ALREADY_UNDONE`).
pub(crate) const SALES_TAX_PAY_MARK_PAID_UNDO: Operation = keyable_in(
    KeySet::SalesTaxPayV1,
    Method::Post,
    "/api/v1/sales-tax/payments/mark-paid-through/{runId}/undo",
);
/// One sales tax payment with its lines.
pub(crate) const SALES_TAX_PAYMENT_GET: Operation = read("/api/v1/sales-tax/payments/{id}");
/// Sales tax payments.
pub(crate) const SALES_TAX_PAYMENTS_LIST: Operation = read("/api/v1/sales-tax/payments");
/// Recovers part of a written-off invoice (U-154).
pub(crate) const WRITE_OFF_RECOVERY_CREATE: Operation = keyable_in(
    KeySet::RecoveryV1,
    Method::Post,
    "/api/v1/payments/write-off-recoveries",
);
/// Written-off invoices and what is left to recover (U-154).
pub(crate) const WRITE_OFFS_LIST: Operation = read("/api/v1/payments/write-offs");
/// The sales tax liability report (v2 string columns, U-149).
pub(crate) const REPORT_SALES_TAX_LIABILITY: Operation =
    read("/api/v1/reports/sales-tax-liability");
/// The sales tax payments report.
pub(crate) const REPORT_SALES_TAX_PAYMENTS: Operation = read("/api/v1/reports/sales-tax-payments");

// ---- P24 inventory (U-159, U-160, U-78; client-declared until the pin
// bump, plan §2.4).
/// Records an inventory adjustment (U-159).
pub(crate) const INVENTORY_ADJUSTMENT_CREATE: Operation = keyable_in(
    KeySet::InventoryV1,
    Method::Post,
    "/api/v1/inventory/adjustments",
);
/// One inventory adjustment.
pub(crate) const INVENTORY_ADJUSTMENT_GET: Operation = read("/api/v1/inventory/adjustments/{id}");
/// Edits an inventory adjustment (`expectedUpdatedAt`).
pub(crate) const INVENTORY_ADJUSTMENT_PUT: Operation = keyable_in(
    KeySet::InventoryV1,
    Method::Put,
    "/api/v1/inventory/adjustments/{id}",
);
/// Deletes an inventory adjustment.
pub(crate) const INVENTORY_ADJUSTMENT_DELETE: Operation = keyable_in(
    KeySet::InventoryV1,
    Method::Delete,
    "/api/v1/inventory/adjustments/{id}",
);
/// Voids an inventory adjustment.
pub(crate) const INVENTORY_ADJUSTMENT_VOID: Operation = keyable_in(
    KeySet::InventoryV1,
    Method::Post,
    "/api/v1/inventory/adjustments/{id}/void",
);
/// The physical count sheet as of a date.
pub(crate) const INVENTORY_COUNT_SHEET: Operation = read("/api/v1/inventory/count-sheet");
/// Posts a count as one adjustment (409 `STALE` when on hand moved).
pub(crate) const INVENTORY_COUNT_POST: Operation = keyable_in(
    KeySet::InventoryV1,
    Method::Post,
    "/api/v1/inventory/counts",
);
/// Inventory valuation summary as of a date, with the GL check (U-160).
pub(crate) const REPORT_INVENTORY_VALUATION: Operation =
    read("/api/v1/reports/inventory-valuation");
/// Inventory valuation detail over dates (U-160).
pub(crate) const REPORT_INVENTORY_VALUATION_DETAIL: Operation =
    read("/api/v1/reports/inventory-valuation-detail");
/// Inventory stock status as of a date (U-160).
pub(crate) const REPORT_INVENTORY_STOCK_STATUS: Operation =
    read("/api/v1/reports/inventory-stock-status");
/// Items at or below their reorder point (U-78).
pub(crate) const ITEMS_REORDER_ALERTS: Operation = read("/api/v1/items/reorder-alerts");
/// The reorder report (U-78).
pub(crate) const REPORT_REORDER: Operation = read("/api/v1/reports/reorder");
/// Unit sets (U-161).
pub(crate) const UNIT_SETS_LIST: Operation = read("/api/v1/unit-sets");
/// Creates a unit set (U-161; keyed while the engine advertises
/// `units-of-measure-v1`).
pub(crate) const UNIT_SET_CREATE: Operation =
    keyable_in(KeySet::UnitSetsV1, Method::Post, "/api/v1/unit-sets");
/// A bank file import, preview or commit (U-06): the Import time class.
/// The commit is keyed under `bank-import-batch-v1`; the preview is sent
/// with no key.
pub(crate) const FEED_IMPORT_BATCH: Operation = op(
    Method::Post,
    "/api/v1/banking/feed/import-batch",
    RetryClass::Keyable(KeySet::BankImportV1),
    RouteKind::Session,
    TimeoutClass::Import,
    true,
);
/// Earlier bank file imports, newest first (U-06).
pub(crate) const FEED_IMPORT_BATCHES: Operation = read("/api/v1/banking/feed/import-batches");
/// Undoes a bank file import (U-06; keyed). The Import time class: while
/// the import is being prepared the engine waits up to 20 s for the
/// preparation to stop before it undoes (bank-import-progress-v1).
pub(crate) const FEED_IMPORT_BATCH_UNDO: Operation = op(
    Method::Delete,
    "/api/v1/banking/feed/import-batches/{id}",
    RetryClass::Keyable(KeySet::BankImportV1),
    RouteKind::Session,
    TimeoutClass::Import,
    true,
);
/// Which bank connection a file's account belongs to (U-06). A POST so
/// the account number travels in the body, never in a URL or a log
/// (Q-P25u). It changes nothing; as a POST it is retried only before
/// the engine's handler ran, like any unkeyed write.
pub(crate) const BANK_CONNECTION_LOOKUP: Operation =
    write(Method::Post, "/api/v1/banking/connections/lookup");
/// Bank feed counts per review bucket and state (`feed-review-v1`, U-174).
pub(crate) const FEED_COUNTS: Operation = read("/api/v1/banking/feed/counts");
/// The journal lines a feed item may match (`feed-match-v2`, U-166).
pub(crate) const FEED_MATCH_CANDIDATES: Operation =
    read("/api/v1/banking/feed/{id}/match-candidates");
/// Payroll bank lines a money-out item may already be (`feed-match-v2`).
pub(crate) const FEED_PAYROLL_OVERLAP: Operation =
    read("/api/v1/banking/feed/{id}/payroll-overlap");
/// Bank lines on another account that may be a transfer's other side
/// (`feed-match-post-v1`, U-171).
pub(crate) const FEED_TRANSFER_CANDIDATES: Operation =
    read("/api/v1/banking/feed/{id}/transfer-candidates");
/// Posts a feed item (exact, optional split; `feed-exact-v1`), keyed under
/// `feed-idempotency-v1`.
pub(crate) const FEED_CATEGORIZE: Operation = keyable_in(
    KeySet::FeedV1,
    Method::Put,
    "/api/v1/banking/feed/{id}/categorize",
);
/// Stages an account (and optional split) without posting.
pub(crate) const FEED_ASSIGN: Operation = write(Method::Put, "/api/v1/banking/feed/{id}/assign");
/// Posts what is staged on a feed item (keyed under `feed-idempotency-v1`).
pub(crate) const FEED_APPROVE: Operation = keyable_in(
    KeySet::FeedV1,
    Method::Post,
    "/api/v1/banking/feed/{id}/approve",
);
/// Matches a feed item to a line of a posted entry (`feed-match-v2`).
pub(crate) const FEED_MATCH: Operation = keyable_in(
    KeySet::FeedV1,
    Method::Put,
    "/api/v1/banking/feed/{id}/match",
);
/// Keep both or remove this one on a possible duplicate (key required).
pub(crate) const FEED_DUPLICATE: Operation = keyable_in(
    KeySet::FeedV1,
    Method::Post,
    "/api/v1/banking/feed/{id}/duplicate",
);
/// Puts a row whose entry was voided back to waiting (key required).
pub(crate) const FEED_REVIEW_AGAIN: Operation = keyable_in(
    KeySet::FeedV1,
    Method::Post,
    "/api/v1/banking/feed/{id}/review-again",
);
/// Excludes a waiting feed item.
pub(crate) const FEED_EXCLUDE: Operation = write(Method::Put, "/api/v1/banking/feed/{id}/exclude");
/// Restores an excluded feed item.
pub(crate) const FEED_UNEXCLUDE: Operation =
    write(Method::Put, "/api/v1/banking/feed/{id}/unexclude");
/// One transfer for a bank line and its other side (key required).
pub(crate) const FEED_TRANSFER: Operation = keyable_in(
    KeySet::FeedPostV1,
    Method::Post,
    "/api/v1/banking/feed/{id}/transfer",
);
/// One deposit of waiting payments, net of fees, for a money-in bank line
/// (key required).
pub(crate) const FEED_DEPOSIT: Operation = keyable_in(
    KeySet::FeedPostV1,
    Method::Post,
    "/api/v1/banking/feed/{id}/deposit",
);
/// Accepts a selection or every row one rule staged, in one engine
/// transaction (`feed-accept-v1`, U-170; key required).
pub(crate) const FEED_ACCEPT: Operation = keyable_in(
    KeySet::FeedAcceptV1,
    Method::Post,
    "/api/v1/banking/feed/accept",
);
/// An accept's answer again (a replay over the 512 KB cap).
pub(crate) const FEED_ACCEPT_GROUP: Operation = read("/api/v1/banking/feed/accept-groups/{id}");
/// Takes accepted rows or an accept group back (`feed-undo-v1`, U-169;
/// key required).
pub(crate) const FEED_UNDO: Operation = keyable_in(
    KeySet::FeedUndoV1,
    Method::Post,
    "/api/v1/banking/feed/undo",
);
/// The Bank Feed rules (conditional rules; P26 DI-2).
pub(crate) const FEED_RULE_LIST: Operation = read("/api/v1/practice/conditional-rules");
/// Creates a rule.
pub(crate) const FEED_RULE_CREATE: Operation =
    write(Method::Post, "/api/v1/practice/conditional-rules");
/// Replaces a rule.
pub(crate) const FEED_RULE_UPDATE: Operation =
    write(Method::Put, "/api/v1/practice/conditional-rules/{id}");
/// Deletes a rule.
pub(crate) const FEED_RULE_DELETE: Operation =
    write(Method::Delete, "/api/v1/practice/conditional-rules/{id}");
/// Rules suggested from repeated choices (Q-141, Q-P26k).
pub(crate) const FEED_RULE_SUGGESTIONS: Operation =
    read("/api/v1/practice/conditional-rules/suggestions");
/// Dismisses a rule suggestion.
pub(crate) const FEED_RULE_SUGGESTION_DISMISS: Operation = write(
    Method::Post,
    "/api/v1/practice/conditional-rules/suggestions/dismiss",
);
// ---- P27: Reconcile (U-188 to U-191). Not keyed: a retried complete or
// undo gets a 409 and the screen reads the reconciliation again.
/// Begin a reconciliation (charges, pre-check, later lines; U-188/U-189).
pub(crate) const RECON_START: Operation = write(Method::Post, "/api/v1/banking/reconciliations");
/// One reconciliation with its lines and the engine's totals.
pub(crate) const RECON_GET: Operation = read("/api/v1/banking/reconciliations/{id}");
/// Ticks in one batch (`?view=summary`: totals only).
pub(crate) const RECON_LINES: Operation =
    write(Method::Put, "/api/v1/banking/reconciliations/{id}/lines");
/// Change statement info (date, ending balance, charges).
pub(crate) const RECON_PATCH: Operation =
    write(Method::Patch, "/api/v1/banking/reconciliations/{id}");
/// Pull in new lines (pre-checked) and drop orphans.
pub(crate) const RECON_REFRESH: Operation =
    write(Method::Post, "/api/v1/banking/reconciliations/{id}/refresh");
/// Finish: posts the charges, marks the lines reconciled.
pub(crate) const RECON_COMPLETE: Operation = write(
    Method::Post,
    "/api/v1/banking/reconciliations/{id}/complete",
);
/// Undo the latest finished reconciliation (U-190 modes, Admin only).
pub(crate) const RECON_UNDO: Operation =
    write(Method::Post, "/api/v1/banking/reconciliations/{id}/undo");
/// Discard one in progress ("Start over").
pub(crate) const RECON_CANCEL: Operation =
    write(Method::Post, "/api/v1/banking/reconciliations/{id}/cancel");
/// The detail report (the summary PDF reads it too).
pub(crate) const RECON_DETAIL_REPORT: Operation = read("/api/v1/reports/reconciliation-detail");
/// The discrepancy report (U-191).
pub(crate) const RECON_DISCREPANCY: Operation = read("/api/v1/reports/reconciliation-discrepancy");
/// Runs the rules again over waiting bank lines (P26 QA-10: a rule saved
/// in the Bank Feed sets up the lines already waiting). Desktop engines
/// only stage what a rule sets (Q-P26o); nothing posts.
pub(crate) const FEED_REPROCESS_RULES: Operation =
    write(Method::Post, "/api/v1/banking/feed/bulk-reprocess-rules");
/// The linked bank accounts (one connection per account a bank file was
/// imported into).
pub(crate) const BANK_CONNECTIONS: Operation = read("/api/v1/banking/connections");
/// Books and latest bank-file balance per bank and card account (U-77).
pub(crate) const DASHBOARD_ACCOUNT_BALANCES: Operation = read("/api/v1/dashboard/account-balances");
/// One unit set.
pub(crate) const UNIT_SET_GET: Operation = read("/api/v1/unit-sets/{id}");
/// Stores a unit set (`expectedUpdatedAt`).
pub(crate) const UNIT_SET_PUT: Operation = write(Method::Put, "/api/v1/unit-sets/{id}");
/// Deletes a unit set no item or line names.
pub(crate) const UNIT_SET_DELETE: Operation = write(Method::Delete, "/api/v1/unit-sets/{id}");
// P28: memorized reports (U-118, `report-defs-v1`). Not keyed yet: a
// retried save is caught by the name rule (409 `NAME_TAKEN`) or the version
// (409 `STALE_VERSION`).
/// Memorized reports.
pub(crate) const REPORT_DEFS_LIST: Operation = read("/api/v1/report-builder/definitions");
/// Memorizes a report.
pub(crate) const REPORT_DEFS_CREATE: Operation = keyable_in(
    KeySet::ReportDefsV1,
    Method::Post,
    "/api/v1/report-builder/definitions",
);
/// One memorized report.
pub(crate) const REPORT_DEF_GET: Operation = read("/api/v1/report-builder/definitions/{id}");
/// Saves changes to a memorized report (`version`).
pub(crate) const REPORT_DEF_PUT: Operation = keyable_in(
    KeySet::ReportDefsV1,
    Method::Put,
    "/api/v1/report-builder/definitions/{id}",
);
/// Deletes a memorized report (`version`).
pub(crate) const REPORT_DEF_DELETE: Operation =
    write(Method::Delete, "/api/v1/report-builder/definitions/{id}");
/// Memorized report groups.
pub(crate) const REPORT_DEF_GROUPS_LIST: Operation =
    read("/api/v1/report-builder/definition-groups");
/// Adds a group.
pub(crate) const REPORT_DEF_GROUPS_CREATE: Operation = keyable_in(
    KeySet::ReportDefsV1,
    Method::Post,
    "/api/v1/report-builder/definition-groups",
);
/// Renames or moves a group (`version`).
pub(crate) const REPORT_DEF_GROUP_PUT: Operation = keyable_in(
    KeySet::ReportDefsV1,
    Method::Put,
    "/api/v1/report-builder/definition-groups/{id}",
);
/// Deletes a group; its reports move to no group (`version`).
pub(crate) const REPORT_DEF_GROUP_DELETE: Operation = write(
    Method::Delete,
    "/api/v1/report-builder/definition-groups/{id}",
);
// A09 (U-220, `report-defs-transfer-v1`): `.clreport` files. Not keyed:
// export only reads (and writes a `download` audit row per report); the
// preview writes nothing. Outside the pinned spec until U-220's pin bump.
/// Memorized reports without ids (`"$ref:N"` tokens and references).
pub(crate) const REPORT_DEFS_EXPORT: Operation =
    write(Method::Post, "/api/v1/report-builder/definitions/export");
/// Matches a file's references in this company (no writes).
pub(crate) const REPORT_DEFS_IMPORT_PREVIEW: Operation = write(
    Method::Post,
    "/api/v1/report-builder/definitions/import/preview",
);
// A07: Report Builder runs (U-119, `report-builder-v1`). Read-only POSTs,
// never blind-retried (a 503 `REPORT_TIMEOUT` already took 30 s), with the
// long timeout.
/// Runs a builder definition.
pub(crate) const REPORT_BUILDER_RUN: Operation = op(
    Method::Post,
    "/api/v1/report-builder/run",
    RetryClass::PreHandlerOnly,
    RouteKind::Session,
    TimeoutClass::Import,
    true,
);
/// One page of a path's detail.
pub(crate) const REPORT_BUILDER_DETAIL: Operation = op(
    Method::Post,
    "/api/v1/report-builder/run/detail",
    RetryClass::PreHandlerOnly,
    RouteKind::Session,
    TimeoutClass::Import,
    true,
);
/// The field catalogue.
pub(crate) const REPORT_BUILDER_FIELDS: Operation = read("/api/v1/report-builder/fields");
/// The starters.
pub(crate) const REPORT_BUILDER_STARTERS: Operation = read("/api/v1/report-builder/starters");
/// The General Ledger and Transaction Detail by Account (U-14 exact).
pub(crate) const REPORT_GENERAL_LEDGER: Operation = read("/api/v1/reports/general-ledger");
/// Revenue by Category (U-14 exact).
pub(crate) const REPORT_REVENUE_BY_CATEGORY: Operation =
    read("/api/v1/reports/revenue-by-category");
/// Expenses by Category (U-14 exact).
pub(crate) const REPORT_EXPENSE_BY_CATEGORY: Operation =
    read("/api/v1/reports/expense-by-category");
/// `ar-aging-detail` (P28 WP3).
pub(crate) const REPORT_AR_AGING_DETAIL: Operation = read("/api/v1/reports/ar-aging-detail");
/// `invoice-list` (P28 WP3).
pub(crate) const REPORT_INVOICE_LIST: Operation = read("/api/v1/reports/invoice-list");
/// `customer-balance-detail` (P28 WP3).
pub(crate) const REPORT_CUSTOMER_BALANCE_DETAIL: Operation =
    read("/api/v1/reports/customer-balance-detail");
/// `vendor-balance-summary` (P28 WP3).
pub(crate) const REPORT_VENDOR_BALANCE_SUMMARY: Operation =
    read("/api/v1/reports/vendor-balance-summary");
/// `vendor-balance-detail` (P28 WP3).
pub(crate) const REPORT_VENDOR_BALANCE_DETAIL: Operation =
    read("/api/v1/reports/vendor-balance-detail");
/// `ap-aging-detail` (P28 WP3).
pub(crate) const REPORT_AP_AGING_DETAIL: Operation = read("/api/v1/reports/ap-aging-detail");
/// `unpaid-bills` (P28 WP3).
pub(crate) const REPORT_UNPAID_BILLS: Operation = read("/api/v1/reports/unpaid-bills");
/// `sales-by-customer` (P28 WP3).
pub(crate) const REPORT_SALES_BY_CUSTOMER: Operation = read("/api/v1/reports/sales-by-customer");
/// `sales-by-item` (P28 WP3).
pub(crate) const REPORT_SALES_BY_ITEM: Operation = read("/api/v1/reports/sales-by-item");
/// `expense-by-vendor` (P28 WP3).
pub(crate) const REPORT_EXPENSE_BY_VENDOR: Operation = read("/api/v1/reports/expense-by-vendor");
/// `transaction-list-by-vendor` (P28 WP3).
pub(crate) const REPORT_TRANSACTION_LIST_BY_VENDOR: Operation =
    read("/api/v1/reports/transaction-list-by-vendor");
/// `bill-payment-history` (P28 WP3).
pub(crate) const REPORT_BILL_PAYMENT_HISTORY: Operation =
    read("/api/v1/reports/bill-payment-history");
/// `vendor-1099-summary` (P28 WP3).
pub(crate) const REPORT_VENDOR_1099_SUMMARY: Operation =
    read("/api/v1/reports/vendor-1099-summary");
/// `ap-1099-prep` (P28 WP3).
pub(crate) const REPORT_AP_1099_PREP: Operation = read("/api/v1/reports/ap-1099-prep");
/// `check-register` (P28 WP3).
pub(crate) const REPORT_CHECK_REGISTER: Operation = read("/api/v1/reports/check-register");
/// `bank-balances` (P28 WP3).
pub(crate) const REPORT_BANK_BALANCES: Operation = read("/api/v1/reports/bank-balances");
/// `account-activity-summary` (P28 WP3).
pub(crate) const REPORT_ACCOUNT_ACTIVITY_SUMMARY: Operation =
    read("/api/v1/reports/account-activity-summary");
/// `closing-date-exceptions` (P28 WP3).
pub(crate) const REPORT_CLOSING_DATE_EXCEPTIONS: Operation =
    read("/api/v1/reports/closing-date-exceptions");
/// The duplicate pairs scan (P28 WP3, Duplicate Finder).
pub(crate) const DUPLICATES_SCAN: Operation = read("/api/v1/duplicates");
// P28 WP4: budgets (U-203, `budgets-exact-v1`), report email (U-207,
// `reports-email-v1`) and ask a question (U-206, `reports-ask-v1`).
/// The company's budgets.
pub(crate) const BUDGETS_LIST: Operation = read("/api/v1/budgets");
/// Creates a budget for a fiscal year.
pub(crate) const BUDGETS_CREATE: Operation =
    keyable_in(KeySet::BudgetsV1, Method::Post, "/api/v1/budgets");
/// Renames or changes a budget (`version`).
pub(crate) const BUDGET_PUT: Operation =
    keyable_in(KeySet::BudgetsV1, Method::Put, "/api/v1/budgets/{id}");
/// Deletes a budget and its lines.
pub(crate) const BUDGET_DELETE: Operation = write(Method::Delete, "/api/v1/budgets/{id}");
/// A budget's lines with the engine's totals.
pub(crate) const BUDGET_LINES: Operation = read("/api/v1/budgets/{id}/lines");
/// Saves budget lines (`version`).
pub(crate) const BUDGET_LINES_PUT: Operation =
    keyable_in(KeySet::BudgetsV1, Method::Put, "/api/v1/budgets/{id}/lines");
/// Fills a budget from last year's actuals.
pub(crate) const BUDGET_FILL: Operation = keyable_in(
    KeySet::BudgetsV1,
    Method::Post,
    "/api/v1/budgets/{id}/fill-from-actuals",
);
/// Copies another budget's lines.
pub(crate) const BUDGET_COPY: Operation = keyable_in(
    KeySet::BudgetsV1,
    Method::Post,
    "/api/v1/budgets/{id}/copy-from/{sourceId}",
);
/// Adjusts every cell by a percent.
pub(crate) const BUDGET_ADJUST: Operation = keyable_in(
    KeySet::BudgetsV1,
    Method::Post,
    "/api/v1/budgets/{id}/adjust-by-percent",
);
/// Budget vs. Actual (`exact=1`).
pub(crate) const BUDGET_VS_ACTUAL: Operation = read("/api/v1/budgets/{id}/vs-actual");
/// Emails an exported report file through the company mail server.
pub(crate) const REPORT_EMAIL: Operation = keyable_in(
    KeySet::ReportsEmailV1,
    Method::Post,
    "/api/v1/reports/email",
);
/// Maps a question to a catalog report (changes nothing; not keyed; a
/// model call can take a while, so the long class).
pub(crate) const REPORT_ASK: Operation = op(
    Method::Post,
    "/api/v1/reports/ask",
    RetryClass::PreHandlerOnly,
    RouteKind::Session,
    TimeoutClass::Import,
    true,
);

#[cfg(test)]
/// Mutations outside `/api/v1/auth/` that are never retried: each creates
/// or destroys a company, or changes a credential.
const NEVER_RETRIED: [(&str, &str); 5] = [
    ("POST", "/api/v1/tenants"),
    ("POST", "/api/v1/tenants/{id}/delete"),
    ("POST", "/api/v1/desktop/recover-password"),
    ("PUT", "/api/v1/company/closing-password"),
    ("POST", "/api/v1/company/closing-password/reset"),
];

/// Every covered operation, in `operations.txt` order.
pub const OPERATIONS: [Operation; 420] = [
    HEALTH,
    DESKTOP_INFO,
    OPENAPI,
    TEST_SMTP,
    LOGIN,
    TFA_VERIFY,
    REFRESH,
    LOGOUT,
    SWITCH_TENANT,
    CREATE_TENANT,
    AUTH_ME,
    COMPANY_GET,
    COMPANY_PUT,
    COMPANY_SETTINGS,
    ACCOUNTS_LIST,
    ACCOUNTS_CREATE,
    ACCOUNT_GET,
    ACCOUNT_PUT,
    ACCOUNT_DELETE,
    CONTACTS_LIST,
    CONTACTS_CREATE,
    CONTACT_GET,
    CONTACT_PUT,
    CONTACT_DELETE,
    ITEMS_LIST,
    ITEMS_CREATE,
    ITEM_GET,
    ITEM_PUT,
    ITEM_DELETE,
    TAGS_LIST,
    TAGS_CREATE,
    TAG_GET,
    TAG_PUT,
    TAG_DELETE,
    TXNS_LIST,
    TXNS_CREATE,
    TXN_GET,
    TXN_PUT,
    TXN_VOID,
    LEDGER_VERSION,
    COMPANY_SETTINGS_PUT,
    SETUP_COMPLETE,
    COA_TEMPLATE_OPTIONS,
    ACCOUNTS_IMPORT,
    OPENING_BALANCES,
    INVOICES_CREATE,
    BILLS_CREATE,
    REPORT_BALANCE_SHEET,
    REPORT_TRIAL_BALANCE,
    REPORT_PROFIT_LOSS,
    REPORT_ACCOUNT,
    EXPORT_FULL,
    EXPORT_FULL_FILE,
    TENANTS_LIST,
    TENANTS_CREATE,
    TENANT_RENAME,
    TENANT_ARCHIVE_EXPORT,
    TENANT_ARCHIVE_JOB,
    TENANT_ARCHIVE_DOWNLOAD,
    TENANT_ARCHIVE_IMPORT,
    TENANT_DELETE,
    RECOVER_PASSWORD,
    RECOVERY_KEY_ROTATE,
    AI_CONFIG,
    AI_CONFIG_PUT,
    AI_DISCLOSURE,
    AI_DISCLOSURE_ACCEPT,
    AI_CONSENT,
    AI_CONSENT_DISCLOSURE,
    AI_CONSENT_ACCEPT,
    AI_CONSENT_REVOKE,
    AI_CONSENT_TASKS,
    AI_TEST_PROVIDER,
    AI_MODELS,
    AI_STATUS,
    AI_USAGE,
    AI_USAGE_SUMMARY,
    COMPANY_SMTP,
    COMPANY_SMTP_PUT,
    COMPANY_SMTP_TEST,
    SESSION_SECRETS_PUT,
    SESSION_SECRETS_GET,
    SESSION_SECRETS_DELETE,
    CONTACTS_MERGE,
    CONTACT_TRANSACTIONS,
    REPORT_CUSTOMER_BALANCE,
    REPORT_AP_AGING,
    REPORT_VENDOR_STATEMENT,
    VENDOR_1099_PROFILE,
    VENDOR_1099_PROFILE_PUT,
    FORM_1099_ACCOUNT_MAPPINGS_PUT,
    TAG_GROUPS_LIST,
    TAG_GROUPS_CREATE,
    TAG_GROUP_PUT,
    TAG_GROUP_DELETE,
    TAG_USAGE,
    TAG_MERGE,
    ACCOUNTS_EXPORT,
    CONTACTS_EXPORT,
    CONTACTS_IMPORT,
    ITEMS_EXPORT,
    REGISTER,
    REGISTER_SUMMARY,
    CHECKS_CREATE,
    AUDIT_LOG,
    CLOSING_PASSWORD_GET,
    CLOSING_PASSWORD_PUT,
    TXN_DELETE,
    CLOSING_PASSWORD_RESET,
    // P13 WP2
    BILLS_VENDOR_SUMMARY,
    BILLS_PAYABLE,
    CONTACT_SUGGEST_ACCOUNT,
    ATTACHMENTS_UPLOAD,
    ATTACHMENTS_LIST,
    ATTACHMENT_DOWNLOAD,
    AI_CATEGORIZE_PREVIEW,
    RULES_EVALUATE,
    // P13 WP1
    CHECKS_LIST,
    CHECK_SETTINGS,
    CHECK_PUT,
    // P14
    PENDING_DEPOSITS,
    FEED_LIST,
    FEED_DEPOSIT_GROUPINGS,
    REPORT_DEPOSIT_DETAIL,
    EQUITY_ROLES,
    DAILY_TEMPLATES,
    DAILY_TEMPLATES_CREATE,
    DAILY_TEMPLATE_GET,
    DAILY_TEMPLATE_PUT,
    DAILY_TEMPLATE_DELETE,
    DAILY_TEMPLATE_LINES,
    DAILY_ENTRIES,
    DAILY_ENTRIES_CREATE,
    DAILY_ENTRY_PREVIEW,
    DAILY_ENTRY_GET,
    DAILY_ENTRY_PUT,
    DAILY_ENTRY_DELETE,
    DAILY_ENTRY_POST,
    DAILY_ENTRY_VOID,
    // P15
    JE_NEXT_NUMBER,
    TEMPLATES_LIST,
    TEMPLATES_CREATE,
    TEMPLATES_FROM_TXN,
    TEMPLATE_GROUPS,
    TEMPLATE_GROUPS_CREATE,
    TEMPLATE_GROUP_PUT,
    TEMPLATE_GROUP_DELETE,
    TEMPLATE_GET,
    TEMPLATE_PUT,
    TEMPLATE_DELETE,
    TEMPLATE_DRAFT,
    TEMPLATE_ENTER,
    TEMPLATE_SKIP,
    TEMPLATE_RESOLVE,
    RECURRING_RUN_DUE,
    RECURRING_ATTENTION,
    // P15A
    BATCH_VALIDATE,
    BATCH_SAVE,
    BATCH_SUGGEST_ACCOUNT,
    BATCH_PARSE_CSV,
    // P16 WP6a
    DASHBOARD_CASH,
    DASHBOARD_RECEIVABLES,
    DASHBOARD_PAYABLES,
    DASHBOARD_ACTIONS,
    DASHBOARD_SUMMARY,
    BILLS_LIST,
    PAYROLL_SESSIONS,
    RECONCILIATIONS,
    BANK_STATEMENTS,
    // P17
    BILL_GET,
    BILL_PUT,
    BILL_VOID,
    BILL_TERMS_DATES,
    PAYMENT_TERMS,
    BILL_ATTACH_DRAFT,
    BILL_EXTRACT,
    VENDOR_CREDITS_LIST,
    VENDOR_CREDITS_CREATE,
    VENDOR_CREDIT_GET,
    VENDOR_CREDIT_PUT,
    VENDOR_CREDIT_VOID,
    VENDOR_CREDITS_AVAILABLE,
    ATTACHMENT_GET,
    ATTACHMENT_DELETE,
    ATTACHMENT_LINK,
    ATTACHMENTS_UNLINKED,
    BILL_PAYMENTS_LIST,
    BILL_PAYMENT_VOID,
    // A01
    INTAKE_SETTINGS_GET,
    INTAKE_SETTINGS_PUT,
    // A03
    DOCUMENT_DRAFTS_CREATE,
    DOCUMENT_DRAFTS_LIST,
    DOCUMENT_DRAFT_GET,
    DOCUMENT_DRAFT_DUPLICATES,
    DOCUMENT_DRAFT_PUT,
    DOCUMENT_DRAFT_DISCARD,
    DOCUMENT_DRAFT_CONVERT,
    VENDOR_TEMPLATES_LIST,
    VENDOR_TEMPLATES_PUT,
    VENDOR_TEMPLATE_DELETE,
    DOCUMENT_READING_ASSIST,
    // A04
    DOCUMENT_DRAFT_ATTACH,
    // P18
    BILL_PAYMENTS_CREATE,
    BILL_PAYMENT_GET,
    // P19
    CHECKS_PRINT_QUEUE,
    CHECKS_PRINT,
    CHECKS_REQUEUE,
    CHECKS_REPRINT,
    CHECKS_SPOILED,
    CHECK_SETTINGS_PUT,
    COMPANY_LOGO,
    COMPANY_LOGO_UPLOAD,
    INVOICE_TEMPLATE,
    INVOICE_TEMPLATE_PUT,
    DOCUMENT_NUMBERS,
    DOCUMENT_NUMBER_NEXT,
    DOCUMENT_NUMBER_PUT,
    DOCUMENT_NUMBER_CLAIM,
    FORMS_PRINT_QUEUE,
    FORMS_MARK_PRINTED,
    // P20
    INVOICES_LIST,
    INVOICE_GET,
    INVOICE_PUT,
    INVOICE_VOID,
    INVOICE_SENT,
    INVOICE_PREVIEW,
    INVOICE_EMAIL,
    INVOICE_EMAIL_LATER,
    EMAIL_QUEUE,
    EMAIL_QUEUE_DELETE,
    BILLABLE_COSTS,
    PRICE_LEVELS_LIST,
    PRICE_LEVELS_CREATE,
    PRICE_LEVEL_GET,
    PRICE_LEVEL_PUT,
    PRICE_LEVEL_DELETE,
    INVOICE_LAYOUTS_LIST,
    INVOICE_LAYOUTS_CREATE,
    INVOICE_LAYOUT_GET,
    INVOICE_LAYOUT_PUT,
    INVOICE_LAYOUT_DEFAULT,
    // P21
    PAYMENT_PREVIEW,
    PAYMENT_RECEIVE,
    PAYMENTS_LIST,
    PAYMENT_GET,
    PAYMENT_PUT,
    PAYMENT_VOID,
    PAYMENT_UNAPPLY,
    PAYMENT_APPLY,
    PAYMENT_BOUNCE,
    PAYMENT_OPEN_INVOICES,
    PAYMENT_DEPOSITS_ON_HAND,
    CUSTOMER_REFUND_CREATE,
    PAYMENT_CREDITS,
    CREDIT_MEMO_PREVIEW,
    CREDIT_MEMO_CREATE,
    CREDIT_MEMO_GET,
    CREDIT_MEMO_PUT,
    CREDIT_MEMO_APPLY,
    CREDIT_MEMO_UNAPPLY,
    SALES_RECEIPT_PREVIEW,
    SALES_RECEIPT_CREATE,
    SALES_RECEIPT_GET,
    SALES_RECEIPT_PUT,
    CUSTOMER_REFUND_GET,
    REPORT_CUSTOMER_STATEMENT,
    REPORT_CUSTOMER_STATEMENTS,
    REPORT_AR_AGING,
    STATEMENT_RUN_CREATE,
    STATEMENTS_LIST,
    STATEMENT_GET,
    STATEMENT_SENT,
    STATEMENT_EMAIL,
    STATEMENT_EMAIL_LATER,
    FINANCE_CHARGE_PREVIEW,
    FINANCE_CHARGE_ASSESS,
    FINANCE_CHARGES_LIST,
    CREDIT_MEMO_EMAIL,
    CREDIT_MEMO_EMAIL_LATER,
    SALES_RECEIPT_EMAIL,
    SALES_RECEIPT_EMAIL_LATER,
    // P23
    SALES_TAX_AGENCIES,
    SALES_TAX_AGENCY_CREATE,
    SALES_TAX_AGENCY_PUT,
    SALES_TAX_AGENCY_DEACTIVATE,
    SALES_TAX_AGENCY_REACTIVATE,
    SALES_TAX_ITEMS,
    SALES_TAX_ITEM_CREATE,
    SALES_TAX_ITEM_PUT,
    SALES_TAX_ITEM_DEACTIVATE,
    SALES_TAX_ITEM_REACTIVATE,
    SALES_TAX_CODES,
    SALES_TAX_CODE_CREATE,
    SALES_TAX_CODE_PUT,
    SALES_TAX_CODE_DEACTIVATE,
    SALES_TAX_CODE_REACTIVATE,
    SALES_TAX_OWED,
    SALES_TAX_EXEMPTIONS,
    SALES_TAX_EXEMPTION_CREATE,
    SALES_TAX_EXEMPTION_PUT,
    SALES_TAX_EXEMPTION_DEACTIVATE,
    SALES_TAX_EXEMPTION_REACTIVATE,
    REPORT_EXEMPTIONS_EXPIRING,
    SALES_TAX_PAY_PREVIEW,
    SALES_TAX_PAY_CREATE,
    SALES_TAX_PAY_LINK,
    SALES_TAX_PAY_UNLINK,
    SALES_TAX_PAY_MARK_PAID,
    SALES_TAX_PAY_MARK_PAID_UNDO,
    SALES_TAX_PAYMENT_GET,
    SALES_TAX_PAYMENTS_LIST,
    WRITE_OFF_RECOVERY_CREATE,
    REPORT_SALES_TAX_LIABILITY,
    REPORT_SALES_TAX_PAYMENTS,
    WRITE_OFFS_LIST,
    INVENTORY_ADJUSTMENT_CREATE,
    INVENTORY_ADJUSTMENT_GET,
    INVENTORY_ADJUSTMENT_PUT,
    INVENTORY_ADJUSTMENT_DELETE,
    INVENTORY_ADJUSTMENT_VOID,
    INVENTORY_COUNT_SHEET,
    INVENTORY_COUNT_POST,
    REPORT_INVENTORY_VALUATION,
    REPORT_INVENTORY_VALUATION_DETAIL,
    REPORT_INVENTORY_STOCK_STATUS,
    ITEMS_REORDER_ALERTS,
    REPORT_REORDER,
    UNIT_SETS_LIST,
    UNIT_SET_CREATE,
    UNIT_SET_GET,
    UNIT_SET_PUT,
    UNIT_SET_DELETE,
    FEED_IMPORT_BATCH,
    FEED_IMPORT_BATCHES,
    FEED_IMPORT_BATCH_UNDO,
    BANK_CONNECTION_LOOKUP,
    FEED_COUNTS,
    FEED_MATCH_CANDIDATES,
    FEED_PAYROLL_OVERLAP,
    FEED_TRANSFER_CANDIDATES,
    FEED_CATEGORIZE,
    FEED_ASSIGN,
    FEED_APPROVE,
    FEED_MATCH,
    FEED_DUPLICATE,
    FEED_REVIEW_AGAIN,
    FEED_EXCLUDE,
    FEED_UNEXCLUDE,
    FEED_TRANSFER,
    FEED_DEPOSIT,
    DASHBOARD_ACCOUNT_BALANCES,
    BANK_CONNECTIONS,
    FEED_ACCEPT,
    FEED_ACCEPT_GROUP,
    FEED_UNDO,
    FEED_RULE_LIST,
    FEED_RULE_CREATE,
    FEED_RULE_UPDATE,
    FEED_RULE_DELETE,
    FEED_RULE_SUGGESTIONS,
    FEED_RULE_SUGGESTION_DISMISS,
    // P27
    RECON_START,
    RECON_GET,
    RECON_LINES,
    RECON_PATCH,
    RECON_REFRESH,
    RECON_COMPLETE,
    RECON_UNDO,
    RECON_CANCEL,
    RECON_DETAIL_REPORT,
    RECON_DISCREPANCY,
    // P28
    REPORT_DEFS_LIST,
    REPORT_DEFS_CREATE,
    REPORT_DEF_GET,
    REPORT_DEF_PUT,
    REPORT_DEF_DELETE,
    REPORT_DEF_GROUPS_LIST,
    REPORT_DEF_GROUPS_CREATE,
    REPORT_DEF_GROUP_PUT,
    REPORT_DEF_GROUP_DELETE,
    // A07
    REPORT_BUILDER_RUN,
    REPORT_BUILDER_DETAIL,
    REPORT_BUILDER_FIELDS,
    REPORT_BUILDER_STARTERS,
    REPORT_GENERAL_LEDGER,
    REPORT_REVENUE_BY_CATEGORY,
    REPORT_EXPENSE_BY_CATEGORY,
    REPORT_AR_AGING_DETAIL,
    REPORT_INVOICE_LIST,
    REPORT_CUSTOMER_BALANCE_DETAIL,
    REPORT_VENDOR_BALANCE_SUMMARY,
    REPORT_VENDOR_BALANCE_DETAIL,
    REPORT_AP_AGING_DETAIL,
    REPORT_UNPAID_BILLS,
    REPORT_SALES_BY_CUSTOMER,
    REPORT_SALES_BY_ITEM,
    REPORT_EXPENSE_BY_VENDOR,
    REPORT_TRANSACTION_LIST_BY_VENDOR,
    REPORT_BILL_PAYMENT_HISTORY,
    REPORT_VENDOR_1099_SUMMARY,
    REPORT_AP_1099_PREP,
    REPORT_CHECK_REGISTER,
    REPORT_BANK_BALANCES,
    REPORT_ACCOUNT_ACTIVITY_SUMMARY,
    REPORT_CLOSING_DATE_EXCEPTIONS,
    DUPLICATES_SCAN,
    BUDGETS_LIST,
    BUDGETS_CREATE,
    BUDGET_PUT,
    BUDGET_DELETE,
    BUDGET_LINES,
    BUDGET_LINES_PUT,
    BUDGET_FILL,
    BUDGET_COPY,
    BUDGET_ADJUST,
    BUDGET_VS_ACTUAL,
    REPORT_EMAIL,
    REPORT_ASK,
    // A09 (U-220)
    REPORT_DEFS_EXPORT,
    REPORT_DEFS_IMPORT_PREVIEW,
];

/// Looks up a covered operation.
#[must_use]
pub fn find(method: Method, route: &str) -> Option<&'static Operation> {
    OPERATIONS
        .iter()
        .find(|o| o.method == method && o.route == route)
}

/// Raw operations for the contract harness (feature `test-support`): never
/// company-scoped locally (the harness adds `X-Company-Id` itself) so the
/// engine's own scope checks can be observed.
#[cfg(feature = "test-support")]
pub(crate) const fn raw(method: Method) -> Operation {
    match method {
        Method::Get => op(
            method,
            "/raw",
            RetryClass::Idempotent,
            RouteKind::Session,
            TimeoutClass::Read,
            false,
        ),
        _ => op(
            method,
            "/raw",
            RetryClass::PreHandlerOnly,
            RouteKind::Session,
            TimeoutClass::Write,
            false,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_matches_operations_txt() {
        let txt = include_str!("../../operations.txt");
        let rows: Vec<(String, String, String)> = txt
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| {
                let mut it = l.split_whitespace();
                let m = it.next().expect("method").to_string();
                let r = it.next().expect("route").to_string();
                let c = it.next().expect("class").to_string();
                assert_eq!(it.next(), None, "extra column in {l:?}");
                (m, r, c)
            })
            .collect();
        let table: Vec<(String, String, String)> = OPERATIONS
            .iter()
            .map(|o| {
                (
                    o.method().as_str().to_string(),
                    o.route().to_string(),
                    o.retry().as_str().to_string(),
                )
            })
            .collect();
        assert_eq!(rows, table);
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one branch per keyed route set")]
    fn classification_rules_hold() {
        let mut seen = HashSet::new();
        for o in &OPERATIONS {
            assert!(seen.insert((o.method(), o.route())), "duplicate {o:?}");
            let never = (o.route().starts_with("/api/v1/auth/") && !o.method().is_read())
                || NEVER_RETRIED.contains(&(o.method().as_str(), o.route()));
            let route = (o.method().as_str(), o.route());
            let set = if KEYABLE.contains(&route) {
                Some(KeySet::V1)
            } else if KEYABLE_AP.contains(&route) {
                Some(KeySet::ApV1)
            } else if KEYABLE_DOCS.contains(&route) {
                Some(KeySet::DocsV1)
            } else if KEYABLE_DOCS_ATTACH.contains(&route) {
                Some(KeySet::DocsAttachV1)
            } else if KEYABLE_PAY_BILLS.contains(&route) {
                Some(KeySet::PayBillsV1)
            } else if KEYABLE_CHECK_PRINT.contains(&route) {
                Some(KeySet::CheckPrintV1)
            } else if KEYABLE_INVOICE.contains(&route) {
                Some(KeySet::InvoiceV1)
            } else if KEYABLE_INVOICE_EMAIL.contains(&route) {
                Some(KeySet::InvoiceEmailV1)
            } else if KEYABLE_RECEIVE_PAYMENT.contains(&route) {
                Some(KeySet::ReceivePaymentV1)
            } else if KEYABLE_CREDIT_MEMO.contains(&route) {
                Some(KeySet::CreditMemoV1)
            } else if KEYABLE_CUSTOMER_STATEMENTS.contains(&route) {
                Some(KeySet::CustomerStatementsV1)
            } else if KEYABLE_FINANCE_CHARGES.contains(&route) {
                Some(KeySet::FinanceChargesV1)
            } else if KEYABLE_SALES_DOC_EMAIL.contains(&route) {
                Some(KeySet::SalesDocEmailV1)
            } else if KEYABLE_SALES_TAX.contains(&route) {
                Some(KeySet::SalesTaxSetupV1)
            } else if KEYABLE_SALES_TAX_PAY.contains(&route) {
                Some(KeySet::SalesTaxPayV1)
            } else if KEYABLE_RECOVERY.contains(&route) {
                Some(KeySet::RecoveryV1)
            } else if KEYABLE_INVENTORY.contains(&route) {
                Some(KeySet::InventoryV1)
            } else if KEYABLE_UNIT_SETS.contains(&route) {
                Some(KeySet::UnitSetsV1)
            } else if KEYABLE_REPORT_DEFS.contains(&route) {
                Some(KeySet::ReportDefsV1)
            } else if KEYABLE_BUDGETS.contains(&route) {
                Some(KeySet::BudgetsV1)
            } else if route == ("POST", "/api/v1/reports/email") {
                Some(KeySet::ReportsEmailV1)
            } else if KEYABLE_BANK_IMPORT.contains(&route) {
                Some(KeySet::BankImportV1)
            } else if KEYABLE_FEED.contains(&route) {
                Some(KeySet::FeedV1)
            } else if KEYABLE_FEED_POST.contains(&route) {
                Some(KeySet::FeedPostV1)
            } else if route == ("POST", "/api/v1/banking/feed/accept") {
                Some(KeySet::FeedAcceptV1)
            } else if route == ("POST", "/api/v1/banking/feed/undo") {
                Some(KeySet::FeedUndoV1)
            } else {
                None
            };
            match (o.method().is_read(), never, set) {
                (true, _, _) => assert_eq!(o.retry(), RetryClass::Idempotent, "{o:?}"),
                (false, true, _) => assert_eq!(o.retry(), RetryClass::Never, "{o:?}"),
                (false, false, Some(set)) => {
                    assert_eq!(o.retry(), RetryClass::Keyable(set), "{o:?}");
                    assert_eq!(o.key_set(), Some(set), "{o:?}");
                }
                (false, false, None) => {
                    assert_eq!(o.retry(), RetryClass::PreHandlerOnly, "{o:?}");
                }
            }
            assert_ne!(o.retry(), RetryClass::KeyedOnce, "never in the table");
            if o.company_scoped() {
                assert_eq!(o.kind, RouteKind::Session);
            }
        }
        assert_eq!(
            OPERATIONS.iter().filter(|o| o.is_keyable()).count(),
            KEYABLE.len()
                + KEYABLE_AP.len()
                + KEYABLE_DOCS.len()
                + KEYABLE_DOCS_ATTACH.len()
                + KEYABLE_PAY_BILLS.len()
                + KEYABLE_CHECK_PRINT.len()
                + KEYABLE_INVOICE.len()
                + KEYABLE_INVOICE_EMAIL.len()
                + KEYABLE_RECEIVE_PAYMENT.len()
                + KEYABLE_CREDIT_MEMO.len()
                + KEYABLE_CUSTOMER_STATEMENTS.len()
                + KEYABLE_FINANCE_CHARGES.len()
                + KEYABLE_SALES_DOC_EMAIL.len()
                + KEYABLE_SALES_TAX.len()
                + KEYABLE_SALES_TAX_PAY.len()
                + KEYABLE_RECOVERY.len()
                + KEYABLE_INVENTORY.len()
                + KEYABLE_UNIT_SETS.len()
                + KEYABLE_REPORT_DEFS.len()
                + KEYABLE_BUDGETS.len()
                + KEYABLE_BANK_IMPORT.len()
                + KEYABLE_FEED.len()
                + KEYABLE_FEED_POST.len()
                + 3,
            "every idempotency-v1, idempotency-ap-v1, idempotency-docs-v1, document-drafts-attach-v1 and idempotency-pay-bills-v1 route is covered"
        );
        assert_eq!(
            find(Method::Post, "/api/v1/auth/create-tenant").map(|o| o.timeout),
            Some(TimeoutClass::CreateTenant)
        );
        assert!(find(Method::Get, "/nope").is_none());
    }

    /// The engine's `idempotency-v1` route set, as `docs/DESKTOP.md`
    /// ("Request idempotency") lists it. `tests/spec_fixtures.rs` checks the
    /// same rows against the `x-idempotency` marks of its `openapi.json`.
    const KEYABLE: [(&str, &str); 20] = [
        ("POST", "/api/v1/transactions"),
        ("PUT", "/api/v1/transactions/{id}"),
        ("POST", "/api/v1/transactions/{id}/void"),
        ("DELETE", "/api/v1/transactions/{id}"),
        ("POST", "/api/v1/checks"),
        ("PUT", "/api/v1/checks/{id}"),
        ("POST", "/api/v1/invoices"),
        ("POST", "/api/v1/bills"),
        ("POST", "/api/v1/daily-sales/entries"),
        ("POST", "/api/v1/daily-sales/entries/{id}/post"),
        ("POST", "/api/v1/daily-sales/entries/{id}/void"),
        ("POST", "/api/v1/templates"),
        ("POST", "/api/v1/templates/from-transaction"),
        ("POST", "/api/v1/templates/{id}/enter"),
        ("POST", "/api/v1/templates/{id}/occurrences/{date}/skip"),
        ("POST", "/api/v1/templates/{id}/occurrences/{date}/resolve"),
        ("POST", "/api/v1/accounts"),
        ("POST", "/api/v1/contacts"),
        ("POST", "/api/v1/items"),
        ("POST", "/api/v1/tags"),
    ];

    /// The engine's `idempotency-ap-v1` route set (U-83 to U-87), as
    /// `tests/fixtures/idempotency_ap_v1_routes.txt` records it.
    const KEYABLE_AP: [(&str, &str); 6] = [
        ("PUT", "/api/v1/bills/{id}"),
        ("POST", "/api/v1/bills/{id}/void"),
        ("POST", "/api/v1/vendor-credits"),
        ("PUT", "/api/v1/vendor-credits/{id}"),
        ("POST", "/api/v1/vendor-credits/{id}/void"),
        ("POST", "/api/v1/bill-payments/{id}/void"),
    ];

    /// The engine's `idempotency-docs-v1` route set (U-92a), as
    /// `tests/fixtures/idempotency_docs_v1_routes.txt` records it.
    const KEYABLE_DOCS: [(&str, &str); 2] = [
        ("POST", "/api/v1/document-drafts"),
        ("POST", "/api/v1/document-drafts/{id}/convert"),
    ];

    /// The engine's `document-drafts-attach-v1` route set (U-95), as
    /// `tests/fixtures/idempotency_docs_attach_v1_routes.txt` records it.
    const KEYABLE_DOCS_ATTACH: [(&str, &str); 1] =
        [("POST", "/api/v1/document-drafts/{id}/attach")];

    /// The engine's `idempotency-pay-bills-v1` route set (U-102), as
    /// `tests/fixtures/idempotency_pay_bills_v1_routes.txt` records it.
    const KEYABLE_PAY_BILLS: [(&str, &str); 1] = [("POST", "/api/v1/bill-payments")];

    /// The engine's `idempotency-check-print-v1` route set (U-116), as
    /// `tests/fixtures/idempotency_check_print_v1_routes.txt` records it.
    const KEYABLE_CHECK_PRINT: [(&str, &str); 1] = [("POST", "/api/v1/checks/print")];

    /// The engine's `idempotency-invoice-v1` route set (U-124), as
    /// `tests/fixtures/idempotency_invoice_v1_routes.txt` records it.
    const KEYABLE_INVOICE: [(&str, &str); 3] = [
        ("PUT", "/api/v1/invoices/{id}"),
        ("POST", "/api/v1/invoices/{id}/void"),
        ("POST", "/api/v1/invoices/{id}/sent"),
    ];

    /// The engine's `invoice-email-v1` keyed set (U-127), as
    /// `tests/fixtures/idempotency_invoice_email_v1_routes.txt` records it.
    const KEYABLE_INVOICE_EMAIL: [(&str, &str); 2] = [
        ("POST", "/api/v1/invoices/{id}/email"),
        ("POST", "/api/v1/invoices/{id}/email-later"),
    ];

    /// The engine's `idempotency-receive-payment-v1` keyed set (U-132), as
    /// `tests/fixtures/idempotency_receive_payment_v1_routes.txt` records it.
    const KEYABLE_RECEIVE_PAYMENT: [(&str, &str); 7] = [
        ("POST", "/api/v1/payments/receive"),
        ("POST", "/api/v1/customer-refunds"),
        ("PUT", "/api/v1/payments/{id}"),
        ("POST", "/api/v1/payments/{id}/void"),
        ("POST", "/api/v1/payments/unapply"),
        ("POST", "/api/v1/payments/{id}/apply"),
        ("POST", "/api/v1/payments/{id}/bounce"),
    ];

    /// The P22 key sets (U-137, U-140, U-141).
    const KEYABLE_CREDIT_MEMO: [(&str, &str); 6] = [
        ("POST", "/api/v1/credit-memos"),
        ("PUT", "/api/v1/credit-memos/{id}"),
        ("POST", "/api/v1/credit-memos/{id}/apply"),
        ("POST", "/api/v1/credit-memos/{id}/unapply"),
        ("POST", "/api/v1/sales-receipts"),
        ("PUT", "/api/v1/sales-receipts/{id}"),
    ];
    const KEYABLE_CUSTOMER_STATEMENTS: [(&str, &str); 4] = [
        ("POST", "/api/v1/customer-statement-runs"),
        ("POST", "/api/v1/customer-statements/{id}/email"),
        ("POST", "/api/v1/customer-statements/{id}/email-later"),
        ("POST", "/api/v1/customer-statements/{id}/sent"),
    ];
    const KEYABLE_FINANCE_CHARGES: [(&str, &str); 1] = [("POST", "/api/v1/finance-charges/assess")];
    /// The engine's `sales-doc-email-v1` keyed set (U-144), as
    /// `tests/fixtures/idempotency_sales_doc_email_v1_routes.txt` records it.
    const KEYABLE_SALES_DOC_EMAIL: [(&str, &str); 4] = [
        ("POST", "/api/v1/credit-memos/{id}/email"),
        ("POST", "/api/v1/credit-memos/{id}/email-later"),
        ("POST", "/api/v1/sales-receipts/{id}/email"),
        ("POST", "/api/v1/sales-receipts/{id}/email-later"),
    ];
    /// The P23 key sets (U-147, U-150, U-154), as
    /// `tests/fixtures/idempotency_sales_tax*_routes.txt` and
    /// `idempotency_recovery_v1_routes.txt` record them.
    const KEYABLE_SALES_TAX: [(&str, &str); 16] = [
        ("POST", "/api/v1/sales-tax/agencies"),
        ("PUT", "/api/v1/sales-tax/agencies/{id}"),
        ("POST", "/api/v1/sales-tax/agencies/{id}/deactivate"),
        ("POST", "/api/v1/sales-tax/agencies/{id}/reactivate"),
        ("POST", "/api/v1/sales-tax/items"),
        ("PUT", "/api/v1/sales-tax/items/{id}"),
        ("POST", "/api/v1/sales-tax/items/{id}/deactivate"),
        ("POST", "/api/v1/sales-tax/items/{id}/reactivate"),
        ("POST", "/api/v1/sales-tax/codes"),
        ("PUT", "/api/v1/sales-tax/codes/{id}"),
        ("POST", "/api/v1/sales-tax/codes/{id}/deactivate"),
        ("POST", "/api/v1/sales-tax/codes/{id}/reactivate"),
        ("POST", "/api/v1/sales-tax/exemptions"),
        ("PUT", "/api/v1/sales-tax/exemptions/{id}"),
        ("POST", "/api/v1/sales-tax/exemptions/{id}/deactivate"),
        ("POST", "/api/v1/sales-tax/exemptions/{id}/reactivate"),
    ];
    const KEYABLE_SALES_TAX_PAY: [(&str, &str); 5] = [
        ("POST", "/api/v1/sales-tax/payments"),
        ("POST", "/api/v1/sales-tax/payments/link"),
        ("POST", "/api/v1/sales-tax/payments/{id}/unlink"),
        ("POST", "/api/v1/sales-tax/payments/mark-paid-through"),
        (
            "POST",
            "/api/v1/sales-tax/payments/mark-paid-through/{runId}/undo",
        ),
    ];
    const KEYABLE_RECOVERY: [(&str, &str); 1] = [("POST", "/api/v1/payments/write-off-recoveries")];
    /// The P24 key set (U-159), as `idempotency_inventory_v1_routes.txt`
    /// records it.
    const KEYABLE_INVENTORY: [(&str, &str); 5] = [
        ("POST", "/api/v1/inventory/adjustments"),
        ("PUT", "/api/v1/inventory/adjustments/{id}"),
        ("DELETE", "/api/v1/inventory/adjustments/{id}"),
        ("POST", "/api/v1/inventory/adjustments/{id}/void"),
        ("POST", "/api/v1/inventory/counts"),
    ];

    const KEYABLE_UNIT_SETS: [(&str, &str); 1] = [("POST", "/api/v1/unit-sets")];

    /// `budgets-exact-v1` (U-203); `tests/fixtures/idempotency_budgets_v1_routes.txt`.
    const KEYABLE_BUDGETS: [(&str, &str); 6] = [
        ("POST", "/api/v1/budgets"),
        ("POST", "/api/v1/budgets/{id}/adjust-by-percent"),
        ("POST", "/api/v1/budgets/{id}/copy-from/{sourceId}"),
        ("POST", "/api/v1/budgets/{id}/fill-from-actuals"),
        ("PUT", "/api/v1/budgets/{id}"),
        ("PUT", "/api/v1/budgets/{id}/lines"),
    ];

    const KEYABLE_REPORT_DEFS: [(&str, &str); 4] = [
        ("POST", "/api/v1/report-builder/definitions"),
        ("PUT", "/api/v1/report-builder/definitions/{id}"),
        ("POST", "/api/v1/report-builder/definition-groups"),
        ("PUT", "/api/v1/report-builder/definition-groups/{id}"),
    ];

    const KEYABLE_BANK_IMPORT: [(&str, &str); 2] = [
        ("POST", "/api/v1/banking/feed/import-batch"),
        ("DELETE", "/api/v1/banking/feed/import-batches/{id}"),
    ];

    /// `feed-idempotency-v1` (U-173, P26 E2).
    const KEYABLE_FEED: [(&str, &str); 5] = [
        ("PUT", "/api/v1/banking/feed/{id}/categorize"),
        ("POST", "/api/v1/banking/feed/{id}/approve"),
        ("PUT", "/api/v1/banking/feed/{id}/match"),
        ("POST", "/api/v1/banking/feed/{id}/duplicate"),
        ("POST", "/api/v1/banking/feed/{id}/review-again"),
    ];

    /// `feed-match-post-v1` (U-171, P26 E4).
    const KEYABLE_FEED_POST: [(&str, &str); 2] = [
        ("POST", "/api/v1/banking/feed/{id}/transfer"),
        ("POST", "/api/v1/banking/feed/{id}/deposit"),
    ];

    #[test]
    fn feed_key_sets_name_their_capabilities() {
        assert_eq!(KeySet::FeedV1.capability(), "feed-idempotency-v1");
        assert_eq!(KeySet::FeedPostV1.capability(), "feed-match-post-v1");
        assert_eq!(KeySet::FeedAcceptV1.capability(), "feed-accept-v1");
        assert_eq!(KeySet::FeedUndoV1.capability(), "feed-undo-v1");
        assert_eq!(FEED_ACCEPT.key_set(), Some(KeySet::FeedAcceptV1));
        assert_eq!(FEED_UNDO.key_set(), Some(KeySet::FeedUndoV1));
        assert_eq!(FEED_CATEGORIZE.key_set(), Some(KeySet::FeedV1));
        assert_eq!(FEED_DEPOSIT.key_set(), Some(KeySet::FeedPostV1));
        assert_eq!(FEED_ASSIGN.key_set(), None);
        assert_eq!(FEED_EXCLUDE.key_set(), None);
    }

    #[test]
    fn bank_import_key_set_names_its_capability() {
        assert_eq!(KeySet::BankImportV1.capability(), "bank-import-batch-v1");
        assert_eq!(FEED_IMPORT_BATCH.key_set(), Some(KeySet::BankImportV1));
        assert_eq!(FEED_IMPORT_BATCH.timeout, TimeoutClass::Import);
        assert_eq!(FEED_IMPORT_BATCH_UNDO.key_set(), Some(KeySet::BankImportV1));
        // The engine may wait 20 s for the preparation to stop.
        assert_eq!(FEED_IMPORT_BATCH_UNDO.timeout, TimeoutClass::Import);
        assert_eq!(FEED_IMPORT_BATCHES.key_set(), None);
        assert_eq!(BANK_CONNECTION_LOOKUP.key_set(), None);
    }

    #[test]
    fn p24_key_set_names_its_capability() {
        assert_eq!(KeySet::UnitSetsV1.capability(), "units-of-measure-v1");
        assert_eq!(UNIT_SET_CREATE.key_set(), Some(KeySet::UnitSetsV1));
        assert_eq!(UNIT_SET_PUT.key_set(), None);
        assert_eq!(KeySet::InventoryV1.capability(), "inventory-adjust-v1");
        assert_eq!(INVENTORY_COUNT_POST.key_set(), Some(KeySet::InventoryV1));
        assert_eq!(INVENTORY_COUNT_SHEET.key_set(), None);
        assert_eq!(
            RetryClass::Keyable(KeySet::InventoryV1).as_str(),
            "keyable-inventory"
        );
    }

    #[test]
    fn p23_key_sets_name_their_capabilities() {
        assert_eq!(
            KeySet::SalesTaxSetupV1.capability(),
            "idempotency-sales-tax-v1"
        );
        assert_eq!(
            KeySet::SalesTaxPayV1.capability(),
            "idempotency-sales-tax-pay-v1"
        );
        assert_eq!(KeySet::RecoveryV1.capability(), "idempotency-recovery-v1");
        assert_eq!(
            SALES_TAX_ITEM_CREATE.key_set(),
            Some(KeySet::SalesTaxSetupV1)
        );
        assert_eq!(SALES_TAX_PAY_CREATE.key_set(), Some(KeySet::SalesTaxPayV1));
        assert_eq!(SALES_TAX_PAY_PREVIEW.key_set(), None);
        assert_eq!(
            WRITE_OFF_RECOVERY_CREATE.key_set(),
            Some(KeySet::RecoveryV1)
        );
        assert_eq!(REPORT_SALES_TAX_LIABILITY.key_set(), None);
        assert_eq!(
            RetryClass::Keyable(KeySet::SalesTaxPayV1).as_str(),
            "keyable-sales-tax-pay"
        );
    }

    #[test]
    fn key_sets_name_their_capabilities() {
        assert_eq!(
            KeySet::CreditMemoV1.capability(),
            "idempotency-credit-memo-v1"
        );
        assert_eq!(
            KeySet::CustomerStatementsV1.capability(),
            "idempotency-customer-statements-v1"
        );
        assert_eq!(
            KeySet::FinanceChargesV1.capability(),
            "idempotency-finance-charges-v1"
        );
        assert_eq!(CREDIT_MEMO_CREATE.key_set(), Some(KeySet::CreditMemoV1));
        assert_eq!(CREDIT_MEMO_GET.key_set(), None);
        assert_eq!(
            STATEMENT_RUN_CREATE.key_set(),
            Some(KeySet::CustomerStatementsV1)
        );
        assert_eq!(
            FINANCE_CHARGE_ASSESS.key_set(),
            Some(KeySet::FinanceChargesV1)
        );
        assert_eq!(KeySet::V1.capability(), "idempotency-v1");
        assert_eq!(KeySet::ApV1.capability(), "idempotency-ap-v1");
        assert_eq!(RetryClass::Keyable(KeySet::V1).as_str(), "keyable");
        assert_eq!(RetryClass::Keyable(KeySet::ApV1).as_str(), "keyable-ap");
        assert_eq!(KeySet::DocsV1.capability(), "idempotency-docs-v1");
        assert_eq!(RetryClass::Keyable(KeySet::DocsV1).as_str(), "keyable-docs");
        assert_eq!(DOCUMENT_DRAFT_CONVERT.key_set(), Some(KeySet::DocsV1));
        assert_eq!(DOCUMENT_DRAFT_PUT.key_set(), None);
        assert_eq!(
            KeySet::DocsAttachV1.capability(),
            "document-drafts-attach-v1"
        );
        assert_eq!(
            RetryClass::Keyable(KeySet::DocsAttachV1).as_str(),
            "keyable-docs-attach"
        );
        assert_eq!(DOCUMENT_DRAFT_ATTACH.key_set(), Some(KeySet::DocsAttachV1));
        assert_eq!(BILLS_CREATE.key_set(), Some(KeySet::V1));
        assert_eq!(BILL_PUT.key_set(), Some(KeySet::ApV1));
        assert_eq!(BILL_GET.key_set(), None);
        assert_eq!(KeySet::PayBillsV1.capability(), "idempotency-pay-bills-v1");
        assert_eq!(
            RetryClass::Keyable(KeySet::PayBillsV1).as_str(),
            "keyable-pay-bills"
        );
        assert_eq!(BILL_PAYMENTS_CREATE.key_set(), Some(KeySet::PayBillsV1));
        assert_eq!(BILL_PAYMENT_GET.key_set(), None);
        assert_eq!(
            KeySet::CheckPrintV1.capability(),
            "idempotency-check-print-v1"
        );
        assert_eq!(
            RetryClass::Keyable(KeySet::CheckPrintV1).as_str(),
            "keyable-check-print"
        );
        assert_eq!(CHECKS_PRINT.key_set(), Some(KeySet::CheckPrintV1));
        assert_eq!(CHECKS_REQUEUE.key_set(), None);
        assert_eq!(KeySet::InvoiceV1.capability(), "idempotency-invoice-v1");
        assert_eq!(
            RetryClass::Keyable(KeySet::InvoiceV1).as_str(),
            "keyable-invoice"
        );
        assert_eq!(INVOICE_PUT.key_set(), Some(KeySet::InvoiceV1));
        assert_eq!(INVOICES_CREATE.key_set(), Some(KeySet::V1));
        assert_eq!(INVOICE_PREVIEW.key_set(), None);
        assert_eq!(KeySet::InvoiceEmailV1.capability(), "invoice-email-v1");
        assert_eq!(
            RetryClass::Keyable(KeySet::InvoiceEmailV1).as_str(),
            "keyable-invoice-email"
        );
        assert_eq!(INVOICE_EMAIL.key_set(), Some(KeySet::InvoiceEmailV1));
        assert_eq!(INVOICE_EMAIL_LATER.key_set(), Some(KeySet::InvoiceEmailV1));
        assert_eq!(EMAIL_QUEUE_DELETE.key_set(), None);
        assert_eq!(
            KeySet::ReceivePaymentV1.capability(),
            "idempotency-receive-payment-v1"
        );
        assert_eq!(
            RetryClass::Keyable(KeySet::ReceivePaymentV1).as_str(),
            "keyable-receive-payment"
        );
        assert_eq!(PAYMENT_BOUNCE.key_set(), Some(KeySet::ReceivePaymentV1));
        assert_eq!(PAYMENT_PREVIEW.key_set(), None);
    }

    /// C1 (U-18 plan §10.1): `idempotency-v1` upgrades only `keyable`
    /// routes. Every other mutation keeps its class: never blind-retried,
    /// reconciled after a lost reply.
    #[test]
    fn idempotency_capability_upgrades_only_keyable_routes() {
        for set in [
            KeySet::V1,
            KeySet::ApV1,
            KeySet::DocsV1,
            KeySet::DocsAttachV1,
            KeySet::PayBillsV1,
            KeySet::CheckPrintV1,
            KeySet::InvoiceV1,
            KeySet::InvoiceEmailV1,
            KeySet::ReceivePaymentV1,
        ] {
            assert_eq!(
                RetryClass::Keyable(set).effective(true),
                RetryClass::KeyedOnce
            );
            assert_eq!(
                RetryClass::Keyable(set).effective(false),
                RetryClass::PreHandlerOnly
            );
        }
        assert_eq!(
            RetryClass::PreHandlerOnly.effective(true),
            RetryClass::PreHandlerOnly
        );
        assert_eq!(RetryClass::Never.effective(true), RetryClass::Never);
        assert_eq!(
            RetryClass::Idempotent.effective(true),
            RetryClass::Idempotent
        );
        // Routes the engine does not cover stay unkeyed under the capability.
        for (m, r) in [
            (Method::Post, "/api/v1/attachments"),
            (Method::Post, "/api/v1/contacts/merge"),
            (Method::Post, "/api/v1/daily-sales/templates"),
            (Method::Put, "/api/v1/contacts/{id}"),
            (Method::Post, "/api/v1/batch/save"),
            (Method::Post, "/api/v1/recurring/run-due"),
        ] {
            let o = find(m, r).expect("covered");
            assert_eq!(o.retry().effective(true), RetryClass::PreHandlerOnly, "{r}");
        }
        for (m, r) in [
            (Method::Post, "/api/v1/auth/create-tenant"),
            (Method::Post, "/api/v1/tenants"),
        ] {
            let o = find(m, r).expect("covered");
            assert_eq!(o.retry().effective(true), RetryClass::Never, "{r}");
        }
    }
}
