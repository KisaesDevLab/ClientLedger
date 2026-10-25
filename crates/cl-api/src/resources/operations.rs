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
/// Soft-deletes a transaction (R7, `txn-delete-v1`).
pub(crate) const TXN_DELETE: Operation = write(Method::Delete, "/api/v1/transactions/{id}");
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
pub(crate) const CHECK_PUT: Operation = write(Method::Put, "/api/v1/checks/{id}");

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
    write(Method::Post, "/api/v1/daily-sales/entries");
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
    write(Method::Post, "/api/v1/daily-sales/entries/{id}/post");
/// Voids a posted day (`closingPassword` with `daily-sales-lock-v1`).
pub(crate) const DAILY_ENTRY_VOID: Operation =
    write(Method::Post, "/api/v1/daily-sales/entries/{id}/void");

// ---- P15: journal entries, templates, catch-up ----------------------------------
// Client-declared (outside the pinned spec) until the P15 engine PRs land.

/// The suggested next journal entry number (`je-number-v1`).
pub(crate) const JE_NEXT_NUMBER: Operation = read("/api/v1/journal-entries/next-number");
/// Templates, by group and kind (`templates-v1`).
pub(crate) const TEMPLATES_LIST: Operation = read("/api/v1/templates");
/// Stores a template (a form's create body minus the date and numbers).
pub(crate) const TEMPLATES_CREATE: Operation = write(Method::Post, "/api/v1/templates");
/// Stores a template from a posted transaction.
pub(crate) const TEMPLATES_FROM_TXN: Operation =
    write(Method::Post, "/api/v1/templates/from-transaction");
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
pub(crate) const TEMPLATE_ENTER: Operation = write(Method::Post, "/api/v1/templates/{id}/enter");
/// Dismisses one occurrence.
pub(crate) const TEMPLATE_SKIP: Operation = write(
    Method::Post,
    "/api/v1/templates/{id}/occurrences/{date}/skip",
);
/// Links an occurrence to a transaction the owner saved in a form.
pub(crate) const TEMPLATE_RESOLVE: Operation = write(
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
// `BILLS_LIST` (P16) and `BILLS_CREATE` (P08) are above. The vendor credit
// edit, the terms dates and the payment terms list are client-declared
// (engine PR-A, U-84/U-86) until the pin moves; bill and credit delete use
// `TXN_DELETE` (U-84b).

/// One bill with its lines.
pub(crate) const BILL_GET: Operation = read("/api/v1/bills/{id}");
/// Changes a bill (`expectedUpdatedAt` and `closingPassword` with
/// `ap-edit-v1` / `ap-closing-v1`).
pub(crate) const BILL_PUT: Operation = write(Method::Put, "/api/v1/bills/{id}");
/// Voids an unpaid bill.
pub(crate) const BILL_VOID: Operation = write(Method::Post, "/api/v1/bills/{id}/void");
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
pub(crate) const VENDOR_CREDITS_CREATE: Operation = write(Method::Post, "/api/v1/vendor-credits");
/// One vendor credit with its lines.
pub(crate) const VENDOR_CREDIT_GET: Operation = read("/api/v1/vendor-credits/{id}");
/// Changes an unapplied vendor credit (`ap-edit-v1`, U-84a).
pub(crate) const VENDOR_CREDIT_PUT: Operation = write(Method::Put, "/api/v1/vendor-credits/{id}");
/// Voids a vendor credit.
pub(crate) const VENDOR_CREDIT_VOID: Operation =
    write(Method::Post, "/api/v1/vendor-credits/{id}/void");
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
    write(Method::Post, "/api/v1/bill-payments/{id}/void");

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
pub const OPERATIONS: [Operation; 186] = [
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
