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
    /// Mutations once the engine advertises `idempotency-v1` (U-18): retried
    /// like reads, always with the same key.
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
            RetryClass::KeyedOnce => "keyed-once",
            RetryClass::Never => "never",
        }
    }

    /// The class in effect, given whether the engine offers request
    /// idempotency (U-18).
    #[must_use]
    pub const fn effective(self, idempotency_v1: bool) -> RetryClass {
        match self {
            RetryClass::PreHandlerOnly if idempotency_v1 => RetryClass::KeyedOnce,
            other => other,
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
pub(crate) const ACCOUNTS_CREATE: Operation = write(Method::Post, "/api/v1/accounts");
pub(crate) const ACCOUNT_GET: Operation = read("/api/v1/accounts/{id}");
pub(crate) const ACCOUNT_PUT: Operation = write(Method::Put, "/api/v1/accounts/{id}");
pub(crate) const ACCOUNT_DELETE: Operation = write(Method::Delete, "/api/v1/accounts/{id}");
pub(crate) const CONTACTS_LIST: Operation = read("/api/v1/contacts");
pub(crate) const CONTACTS_CREATE: Operation = write(Method::Post, "/api/v1/contacts");
pub(crate) const CONTACT_GET: Operation = read("/api/v1/contacts/{id}");
pub(crate) const CONTACT_PUT: Operation = write(Method::Put, "/api/v1/contacts/{id}");
pub(crate) const CONTACT_DELETE: Operation = write(Method::Delete, "/api/v1/contacts/{id}");
pub(crate) const ITEMS_LIST: Operation = read("/api/v1/items");
pub(crate) const ITEMS_CREATE: Operation = write(Method::Post, "/api/v1/items");
pub(crate) const ITEM_GET: Operation = read("/api/v1/items/{id}");
pub(crate) const ITEM_PUT: Operation = write(Method::Put, "/api/v1/items/{id}");
pub(crate) const ITEM_DELETE: Operation = write(Method::Delete, "/api/v1/items/{id}");
pub(crate) const TAGS_LIST: Operation = read("/api/v1/tags");
pub(crate) const TAGS_CREATE: Operation = write(Method::Post, "/api/v1/tags");
pub(crate) const TAG_GET: Operation = read("/api/v1/tags/{id}");
pub(crate) const TAG_PUT: Operation = write(Method::Put, "/api/v1/tags/{id}");
pub(crate) const TAG_DELETE: Operation = write(Method::Delete, "/api/v1/tags/{id}");
pub(crate) const TXNS_LIST: Operation = read("/api/v1/transactions");
pub(crate) const TXNS_CREATE: Operation = write(Method::Post, "/api/v1/transactions");
pub(crate) const TXN_GET: Operation = read("/api/v1/transactions/{id}");
pub(crate) const TXN_PUT: Operation = write(Method::Put, "/api/v1/transactions/{id}");
pub(crate) const TXN_VOID: Operation = write(Method::Post, "/api/v1/transactions/{id}/void");
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
pub(crate) const INVOICES_CREATE: Operation = write(Method::Post, "/api/v1/invoices");
pub(crate) const BILLS_CREATE: Operation = write(Method::Post, "/api/v1/bills");
pub(crate) const REPORT_BALANCE_SHEET: Operation = read("/api/v1/reports/balance-sheet");
pub(crate) const REPORT_TRIAL_BALANCE: Operation = read("/api/v1/reports/trial-balance");
pub(crate) const REPORT_PROFIT_LOSS: Operation = read("/api/v1/reports/profit-loss");
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

#[cfg(test)]
/// Mutations outside `/api/v1/auth/` that are never retried: each creates
/// or destroys a company, or changes a credential.
const NEVER_RETRIED: [(&str, &str); 3] = [
    ("POST", "/api/v1/tenants"),
    ("POST", "/api/v1/tenants/{id}/delete"),
    ("POST", "/api/v1/desktop/recover-password"),
];

/// Every covered operation, in `operations.txt` order.
pub const OPERATIONS: [Operation; 62] = [
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
    fn classification_rules_hold() {
        let mut seen = HashSet::new();
        for o in &OPERATIONS {
            assert!(seen.insert((o.method(), o.route())), "duplicate {o:?}");
            let never = (o.route().starts_with("/api/v1/auth/") && !o.method().is_read())
                || NEVER_RETRIED.contains(&(o.method().as_str(), o.route()));
            match (o.method().is_read(), never) {
                (true, _) => assert_eq!(o.retry(), RetryClass::Idempotent, "{o:?}"),
                (false, true) => assert_eq!(o.retry(), RetryClass::Never, "{o:?}"),
                (false, false) => assert_eq!(o.retry(), RetryClass::PreHandlerOnly, "{o:?}"),
            }
            if o.company_scoped() {
                assert_eq!(o.kind, RouteKind::Session);
            }
        }
        assert_eq!(
            find(Method::Post, "/api/v1/auth/create-tenant").map(|o| o.timeout),
            Some(TimeoutClass::CreateTenant)
        );
        assert!(find(Method::Get, "/nope").is_none());
    }

    #[test]
    fn idempotency_capability_upgrades_only_mutations() {
        assert_eq!(
            RetryClass::PreHandlerOnly.effective(true),
            RetryClass::KeyedOnce
        );
        assert_eq!(
            RetryClass::PreHandlerOnly.effective(false),
            RetryClass::PreHandlerOnly
        );
        assert_eq!(RetryClass::Never.effective(true), RetryClass::Never);
        assert_eq!(
            RetryClass::Idempotent.effective(true),
            RetryClass::Idempotent
        );
    }
}
