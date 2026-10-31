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
/// Emails an invoice through the company SMTP (U-127; contract per P20
/// plan §2, verify when E4 lands).
pub(crate) const INVOICE_EMAIL: Operation = write(Method::Post, "/api/v1/invoices/{id}/email");
/// Puts an email in the Outbox (U-127, Q-P20q; contract, verify with E4).
pub(crate) const INVOICE_EMAIL_LATER: Operation =
    write(Method::Post, "/api/v1/invoices/{id}/email-later");
/// The Outbox (U-127; contract, verify with E4).
pub(crate) const EMAIL_QUEUE: Operation = read("/api/v1/forms/email-queue");
/// Takes one message out of the Outbox (U-127; contract, verify with E4).
pub(crate) const EMAIL_QUEUE_DELETE: Operation =
    write(Method::Delete, "/api/v1/forms/email-queue/{id}");
/// A client's unbilled costs (U-128; contract, verify when E3 lands).
pub(crate) const BILLABLE_COSTS: Operation = read("/api/v1/billable-costs");
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
pub const OPERATIONS: [Operation; 235] = [
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
                + KEYABLE_INVOICE.len(),
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

    #[test]
    fn key_sets_name_their_capabilities() {
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
