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
/// A vendor's 1099 profile (W-9 on file, tax ID type, backup withholding).
pub(crate) const VENDOR_1099_PROFILE: Operation =
    read("/api/v1/practice/1099/vendors/{contactId}/profile");
/// Changes a vendor's 1099 profile.
pub(crate) const VENDOR_1099_PROFILE_PUT: Operation = write(
    Method::Put,
    "/api/v1/practice/1099/vendors/{contactId}/profile",
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
pub(crate) const CHECKS_CREATE: Operation = write(Method::Post, "/api/v1/checks");
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

#[cfg(test)]
/// Mutations outside `/api/v1/auth/` that are never retried: each creates
/// or destroys a company, or changes a credential.
const NEVER_RETRIED: [(&str, &str); 4] = [
    ("POST", "/api/v1/tenants"),
    ("POST", "/api/v1/tenants/{id}/delete"),
    ("POST", "/api/v1/desktop/recover-password"),
    ("PUT", "/api/v1/company/closing-password"),
];

/// Every covered operation, in `operations.txt` order.
pub const OPERATIONS: [Operation; 105] = [
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
    VENDOR_1099_PROFILE,
    VENDOR_1099_PROFILE_PUT,
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
