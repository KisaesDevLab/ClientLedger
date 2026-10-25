//! Typed client for the local bookkeeping engine API.
//!
//! - Plain HTTP to `127.0.0.1` only (the host is a constant); no proxy, no
//!   TLS (P33 adds it for LAN), no redirects, no cookie jar.
//! - The engine endpoint (port, launch token, generation) comes from an
//!   [`EndpointSource`] the host implements and is re-read for every attempt.
//! - Money is exact (CLAUDE.md non-negotiable 6): money values on the wire are
//!   `cl_core::Money`/`Quantity`/`Rate` strings. The float lints below are
//!   deny-level, `clippy.toml` bans every float conversion, and
//!   `tests/float_ban.rs` proves each ban fires; the xtask `nofloat` scan
//!   covers `src/`.
//! - Nothing secret or personal is logged: no tokens, cookies, passwords,
//!   amounts, names or memo text.
#![forbid(unsafe_code)]
#![deny(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

// Test hooks never reach an optimised build: feature unification
// (`--workspace --all-targets`) can switch `test-support` on for the apps,
// so a release build with it fails here (xtask `release-features` checks
// the normal app build too).
#[cfg(all(feature = "test-support", not(debug_assertions)))]
compile_error!(
    "the `test-support` feature is for tests only and must not be enabled in a release build"
);

mod client;
pub mod endpoint;
pub mod errors;
pub mod idempotency;
pub mod queue;
pub mod resources;
pub mod session;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod wire;

pub use client::{Budget, ClientConfig, LOOPBACK_HOST, Method, Timeouts};
pub use endpoint::{EndpointSource, EndpointState, EngineEndpoint, FixedEndpoint, Purpose};
pub use errors::{
    ApiError, AuthFailure, ClosedPeriod, ConflictReason, ErrorCode, FieldPath, LocalValidation,
    UserText, ValidationError, ValidationIssue,
};
pub use idempotency::IdempotencyKey;
pub use queue::{
    ApiQueue, OrphanLabel, OrphanOutcome, OrphanResult, Pending, QueueConfig, QueueError,
    RequestGroup, ScopeSource, ShutdownReport,
};
pub use resources::accounts::{
    Account, AccountFields, AccountFilter, AccountSort, AccountType, AccountUpdate, Accounts,
};
pub use resources::ai::{
    AiConfig, AiConfigUpdate, AiDisclosure, AiStatus, AiTaskToggles, AiUsage, AiUsageStamp,
    AiUsageSummary, CompanyConsent, ConsentRow, ConsentStatus, DESKTOP_PROVIDERS, ProviderModels,
    ProviderTest, UsageSummaryRow, UsageTotals,
};
pub use resources::audit::{Audit, AuditAction, AuditEntity, AuditEntry, AuditImage, AuditLine};
pub use resources::bank_postings::{
    BankPosting, BankPostingKind, CASH_BACK_MEMO_MAX, CardChargeDraft, CashBack, CheckDraft,
    DepositCaps, DepositDraft, DepositSource, GroupedPayment, PAYEE_ADDRESS_MAX, PAYEE_NAME_MAX,
    PRINTED_MEMO_MAX, SplitLine, TransferDraft,
};
// P13 WP1: write check.
pub use resources::checks::{
    CHECK_PRINTED, CheckEditOptions, CheckRecord, CheckSettings, CheckSummary, Checks, LINE_BILLED,
    PrintStatus, PrintedCheckChange, check_settings_from_json, checks_from_json, is_check_printed,
    is_line_billed, is_number_used,
};
pub use resources::common::SortDir;
pub use resources::company::{
    AccountingMethod, CLOSING_PASSWORD_MAX_BYTES, CLOSING_PASSWORD_MIN_CHARS, ClosingDateChange,
    ClosingPasswordStatus, CompanyProfile, CompanyProfileUpdate, CompanySettings,
    CompanySettingsUpdate, EntityType, EquityRole, EquityRoles,
};
pub use resources::contacts::{
    Contact, ContactFields, ContactFilter, ContactSort, ContactType, ContactUpdate, Contacts,
    NewContact,
};
pub use resources::export::{ExportFile, ExportManifest};
pub use resources::form1099::{Form1099, TinType, Vendor1099Profile, Vendor1099Update};
pub use resources::items::{Item, ItemFields, ItemFilter, ItemSort, ItemUpdate, Items};
pub use resources::ledger::LedgerStamp;
pub use resources::list_csv::{CONTACT_IMPORT_MAX_ROWS, ContactImportResult, ContactImportRow};
pub use resources::operations::{OPERATIONS, Operation, RetryClass};
pub use resources::posting::{ClosedPeriodAuthorization, PostingOptions};
pub use resources::recovery::{PASSWORD_MIN_CHARS, PasswordRecovery, normalize_recovery_key};
pub use resources::register::{
    Category, ClearState, ClearedFilter, EntryType, REGISTER_MAX_PER_PAGE, Register,
    RegisterAccount, RegisterAmount, RegisterLine, RegisterPage, RegisterQuery, RegisterSort,
    RegisterSummary,
};
pub use resources::reports::{
    AccountReport, AccountReportLine, ApAgingSummary, ApAgingVendor, BalanceSheet,
    BalanceSheetEntry, CustomerBalanceRow, CustomerBalanceSummary, ProfitLoss, ReportDate, Reports,
    TrialBalance, TrialBalanceRow,
};
pub use resources::sales::{BillLine, InvoiceLine, NewBill, NewInvoice, Posted};
pub use resources::session_secrets::{
    OllamaTarget, SecretPresence, SecretPresences, SecretSlot, SessionSecrets, SmtpSecurityWire,
    SmtpSessionSecret,
};
pub use resources::setup::{
    AccountImportResult, AccountImportRow, AccountRef, CoaTemplateOption, IMPORT_MAX_ROWS,
    OpeningBalance, OpeningBalancesPosted, SkippedAccount,
};
pub use resources::smtp::{SmtpSettings, SmtpSettingsUpdate};
pub use resources::system::{SmtpTest, SmtpTestResult};
pub use resources::tags::{
    Tag, TagFields, TagFilter, TagGroup, TagGroupFields, TagUpdate, TagUsage, Tags,
};
pub use resources::tenants::{
    ArchiveFileName, ArchiveJob, ArchiveJobStatus, ArchiveRequest, ArchiveResult, CreatedTenant,
    DeleteReceipt, DeleteRequest, HOST_METADATA_MAX_BYTES, ImportJob, ImportRequest, ImportResult,
    ImportVerification, JobError, JobId, JobState, NewTenant, TenantSummary, Tenants,
};
pub use resources::transactions::{
    AccountRoles, Basis, BilledStatus, DELETE_REASON_MAX_CHARS, DeleteOptions,
    JE_AR_AP_NAME_REQUIRED, JE_AR_AP_ONE_PER_ENTRY, JE_NUMBER_MAX_CHARS, JeLine, JeOptions,
    JournalEntries, LINE_ROLE_CASH_BACK, MAX_LINES, NewJournalEntry, REVERSAL_EDITED, ReportBasis,
    Side, Transaction, TransactionFilter, TransactionLine, TransactionSummary, Transactions,
    TxnSort, TxnStatus, TxnType, has_code, next_number_from_json,
};
pub use resources::versioning::{EditableFields, RecordVersion, Versioned};
pub use session::{
    Client, CompanySummary, CreatedCompany, LoginOutcome, NewCompany, OpenCompany, Scope, Session,
    TfaChallenge, TfaMethod,
};
pub use wire::{Capabilities, DesktopInfo, Health, Page, PageRequest, ReportAmount};
// P13 WP2: payee panel, attachments, suggestions.
pub use resources::ai::{
    AI_PREVIEW_DESCRIPTION_MAX_CHARS, AI_PREVIEW_MAX_ROWS, AiGate, AiPreviewRow, AiSuggestion,
};
pub use resources::attachments::{
    ATTACHMENT_MAX_BYTES, AttachTo, Attachment, AttachmentId, Attachments, DraftKey, FileKind,
    check_file,
};
pub use resources::bills::{Bills, OpenBill, VendorSummary};
pub use resources::contacts::{SuggestSource, SuggestedAccount};
pub use resources::rules::{RULE_DESCRIPTION_MAX_CHARS, RuleMatch, RuleQuery, Rules};

// P14 lane A: pending deposits and bank-file groupings.
pub use resources::payments::{
    ALREADY_DEPOSITED, DepositRecord, DepositedCashBack, DepositedLine, DepositedPayment, Payments,
    PendingAmount, PendingDeposits, PendingPayment, SavedDeposit, VENDOR_CREDIT_EXCEEDED,
    VendorRefundDraft, is_already_deposited, is_vendor_credit_exceeded,
};

// P14 lane A: the bank feed.
pub use resources::feed::{
    Feed, FeedDeposit, FeedGrouping, FeedGroupings, FeedItemId, GROUPINGS_LIMIT,
};

// P14 lane B: daily sales.
pub use resources::daily_sales::{
    DAILY_LABEL_MAX_CHARS, DAILY_NAME_MAX_CHARS, DAILY_NOTES_MAX_CHARS, DailyEntry,
    DailyEntryDraft, DailyEntryQuery, DailyEntrySummary, DailyLine, DailyLineInput,
    DailyPostOptions, DailyPreview, DailySales, DailyTemplate, DailyTotals, DailyValue,
    EntryStatus, MAX_DAILY_LINES, MAX_DAILY_VALUES, NormalSide, Preset, Section, entry_from_json,
    preview_from_json, template_from_json,
};

// P15 lane T: templates and the catch-up run.
pub use resources::recurring::{
    Attention, DueReminder, FailedTemplate, PostedOccurrence, RUN_LIMIT_DEFAULT, RUN_LIMIT_MAX,
    Recurring, RunDue, SkipReason, SkippedOccurrence, attention_from_json, new_run_id,
    run_due_from_json,
};
pub use resources::templates::{
    AUTO_START_FIELD, COUNT_MAX, DraftWarning, DraftWarningCode, Entered, Frequency, INTERVAL_MAX,
    MISSING_ACCOUNT, MISSING_LAYOUT, NewTemplate, OCCURRENCE_TAKEN, ScheduleEnd, ScheduleMode,
    TEMPLATE_GROUP_NAME_MAX, TEMPLATE_NAME_MAX, TEMPLATE_NAME_TAKEN, TEMPLATE_PAGE_MAX, Template,
    TemplateChange, TemplateDraft, TemplateGroup, TemplateKind, TemplateSchedule, TemplateUse,
    Templates, auto_start_too_early, client_form, is_auto_start_too_early, is_name_taken,
    is_occurrence_taken, use_from_json,
};

// P15A: Bulk Entry.
pub use resources::batch::{
    Batch, BatchAmount, BatchCaps, BatchField, BatchGroup, BatchGroupProblem, BatchKind,
    BatchRowIn, BatchRowProblem, BatchSaved, BatchValidation, ColumnMapping, CsvField,
    ENGINE_MAX_GROUPS, ENGINE_MAX_ROWS, FieldProblem, PARSE_CSV_MAX_BYTES, ParsedRow, ParsedRows,
    ProblemKind, RowCheck, SavedTxn, is_key_reused,
};

// P16 WP6a: Home reads.
pub use resources::dashboard::{
    ActionItems, CashAccount, CashPosition, DUE_WITHIN_DAYS_MAX, Dashboard, DashboardSummary,
    Payables, Receivables, StaleReconciliation, action_items_from_json, cash_position_from_json,
    payables_from_json, receivables_from_json, summary_from_json,
};
pub use resources::home_queries::{
    BILLS_PAGE, BILLS_PAGES_MAX, BillQuery, HomeQueries, LastReconciled, OpenBillStatus,
    PayrollStatus, RECONCILE_ACCOUNTS_MAX, ReconcileState,
};
pub use wire::money_str_strict;
