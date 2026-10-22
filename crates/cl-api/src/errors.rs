//! Typed engine errors and the status/code mapping (P05 plan §2.4).
//!
//! The engine answers errors as `{error: {message, code?, details?,
//! feature?}}`. Engine messages and details can contain names and amounts
//! ("debits (x) != credits (y)"), so they are kept in [`UserText`], whose
//! `Debug` shows only the length and which has no `Display`. `ApiError`'s own
//! `Display` is technical and PII-free (status, code, request id) and is the
//! only thing logs and "Copy details" ever see.

use std::fmt;
use std::time::Duration;

use cl_core::ids::RequestId;
use cl_core::{CalendarDate, OwnerFacing, OwnerMessage, Timestamp};
use serde_json::Value;

use crate::idempotency::IdempotencyKey;
use crate::resources::versioning::RecordVersion;

/// An engine error code such as `VALIDATION_ERROR`.
///
/// Only upper-case ASCII letters, digits and `_` (1–64 characters) are kept,
/// so a code is always safe to log; anything else is treated as "no code".
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ErrorCode(String);

impl ErrorCode {
    /// Accepts an engine-style code.
    #[must_use]
    pub fn parse(s: &str) -> Option<ErrorCode> {
        let ok = (1..=64).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        ok.then(|| ErrorCode(s.to_string()))
    }

    /// The code text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErrorCode({})", self.0)
    }
}

/// Engine message text for display to the owner. It may contain names or
/// amounts, so it never reaches logs: `Debug` shows only the length and there
/// is no `Display`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserText(String);

impl UserText {
    pub(crate) fn new(s: impl Into<String>) -> UserText {
        UserText(s.into())
    }

    /// The text, for the UI only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for UserText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} chars>", self.0.chars().count())
    }
}

/// A request field path such as `lines.0.debit`.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct FieldPath(String);

impl FieldPath {
    /// The dotted path.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FieldPath({})", self.0)
    }
}

/// One field problem reported by the engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    /// The field, when the engine named one.
    pub path: Option<FieldPath>,
    /// What is wrong, for the owner.
    pub message: UserText,
}

/// The engine refused the input (400 or 422).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationError {
    code: Option<ErrorCode>,
    message: UserText,
    issues: Vec<ValidationIssue>,
}

impl ValidationError {
    /// The first field the engine named (`lines`, `incomeAccountId`, …).
    #[must_use]
    pub fn field(&self) -> Option<&FieldPath> {
        self.issues.iter().find_map(|i| i.path.as_ref())
    }

    /// The engine's message for the owner.
    #[must_use]
    pub fn message(&self) -> &UserText {
        &self.message
    }

    /// The engine code, if any (the balance error has none until U-21).
    #[must_use]
    pub fn code(&self) -> Option<&ErrorCode> {
        self.code.as_ref()
    }

    /// Every field problem.
    #[must_use]
    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    /// A refusal naming one field, as the engine's object-shaped
    /// `details: {field, ...}` reads (demo twins build the same).
    pub(crate) fn for_field(code: &str, field: &str, message: &str) -> ValidationError {
        ValidationError {
            code: ErrorCode::parse(code),
            message: UserText::new(message),
            issues: vec![ValidationIssue {
                path: Some(FieldPath(field.to_string())),
                message: UserText::new(message),
            }],
        }
    }
}

/// Input refused by cl-api before anything was sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalValidation {
    /// A closed-period authorization was given but the engine offers no
    /// closing-date password yet (U-15).
    ClosedPeriodOverrideUnavailable,
    /// A journal entry needs at least two lines.
    TooFewLines,
    /// A journal entry may have at most 500 lines.
    TooManyLines,
    /// Every line amount must be greater than zero.
    NonPositiveAmount,
    /// Debits and credits differ.
    Unbalanced,
    /// A page size outside 1..=500.
    PageLimit,
    /// A value that cannot be sent (for example a malformed name or id).
    InvalidValue,
    /// A void needs a reason.
    MissingReason,
    /// The closing date cannot be written until the engine gates it with
    /// the closing-date password (U-15, `closing-password-v1`).
    ClosingDateWriteUnavailable,
    /// A tenant delete needs a verified backup file first (Q-185).
    BackupRequired,
    /// Session secrets cannot be handed to the engine until it advertises
    /// `session-secrets-v1` (E6); keys stay in the keychain meanwhile.
    SessionSecretsUnavailable,
    /// The integer usage summary needs `ai-desktop-v1` (E6).
    AiUsageSummaryUnavailable,
    /// A range whose start is after its end (dates or amounts).
    BadRange,
    /// The register's cleared and tag filters need `register-exact-v1`
    /// (P12 R3); nothing was sent.
    RegisterFilterUnavailable,
    /// The split lines do not add up to the amount of the transaction.
    TotalMismatch,
    /// The same account is on both sides (a transfer to itself, or a split
    /// line to the register's own account).
    SameAccount,
    /// Setting or changing the closing-date password needs
    /// `closing-password-v1` (U-15); nothing was sent.
    ClosingPasswordUnavailable,
    /// Deleting a transaction needs `txn-delete-v1` (R7); nothing was
    /// sent. Void it instead.
    TxnDeleteUnavailable,
    // ---- P13 WP2: attachments and suggestions ----
    /// An attachment is empty or larger than 10 MB; nothing was sent.
    AttachmentTooLarge,
    /// An attachment is not a PDF, PNG or JPEG (by its first bytes);
    /// nothing was sent.
    AttachmentType,
    /// Rule suggestions need `rules-evaluate-v1`; nothing was sent.
    RulesEvaluateUnavailable,
    // ---- P13 WP1: write check ----
    /// A customer or a billable mark on a check or expense line needs
    /// `line-billable-v1`; nothing was sent.
    LineBillableUnavailable,
    /// Editing a check's own fields (`PUT /checks/{id}`) needs
    /// `check-edit-v1`; nothing was sent. The P12 expense path still
    /// changes its date, payee, memo and lines.
    CheckEditUnavailable,
    // ---- P14: deposits, daily sales, owner helpers ----
    /// Depositing pending payments as a group needs `deposit-grouping-v1`;
    /// nothing was sent.
    DepositGroupingUnavailable,
    /// Cash back and fee (negative) lines on a deposit need
    /// `deposit-cashback-v1`; nothing was sent.
    DepositCashBackUnavailable,
    /// A vendor refund against a vendor credit needs `vendor-refund-v1`;
    /// nothing was sent.
    VendorRefundUnavailable,
    /// Bank-file deposit groupings need `feed-deposit-groupings-v1`;
    /// nothing was sent.
    FeedGroupingsUnavailable,
    /// The equity role accounts need `equity-roles-v1`; nothing was sent.
    EquityRolesUnavailable,
    /// Posting or voiding a daily-sales day in a closed period needs
    /// `daily-sales-lock-v1`; nothing was sent.
    DailySalesLockUnavailable,
    // ---- P15: journal entries, templates, catch-up ----
    /// A name on a journal entry line needs `je-names-v1`; nothing was
    /// sent.
    JeNamesUnavailable,
    /// A journal entry number (or its suggestion) needs `je-number-v1`;
    /// nothing was sent.
    JeNumberUnavailable,
    /// The Adjusting flag needs `je-adjusting-v1`; nothing was sent.
    JeAdjustingUnavailable,
    /// Reverse on needs `je-reversing-v1`; nothing was sent.
    JeReversingUnavailable,
    /// Reverse on must be after the entry's date; nothing was sent.
    ReverseNotAfter,
    /// A journal entry line on Accounts Receivable needs a customer, and
    /// one on Accounts Payable a vendor; nothing was sent.
    ArApNameMissing,
    /// A journal entry may use only one Accounts Receivable or Accounts
    /// Payable account; nothing was sent.
    ArApTwoAccounts,
    /// Templates need `templates-v1`; nothing was sent.
    TemplatesUnavailable,
    /// A deposit that groups customer payments cannot be a template;
    /// nothing was sent.
    TemplateGrouping,
    /// The catch-up run needs `recurring-run-due-v1`; nothing was sent.
    RunDueUnavailable,
    /// Bulk Entry needs a `batch-*-v1` capability the engine does not
    /// advertise (P15A §2); nothing was sent.
    BatchUnavailable,
    /// A Bulk Entry amount has digits below the cent (cl-model rounds
    /// before building a row, Q-232); nothing was sent.
    NotWholeCents,
    /// A Bulk Entry CSV or paste is empty or larger than 10 MB; nothing
    /// was sent.
    BatchFileSize,
}

/// A save was refused because its date is on or before the closing date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedPeriod {
    /// The closing date, when the engine sent a valid `YYYY-MM-DD`.
    pub lock_date: Option<CalendarDate>,
    /// Whether this user could override (there is no override until U-15).
    pub can_override: bool,
    /// Whether a closing-date password is required (U-15
    /// `details.closingPasswordRequired`; `None` from engines without it).
    pub password_required: Option<bool>,
    /// The engine's message.
    pub message: UserText,
}

/// Why the engine refused a sign-in or a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFailure {
    /// Wrong email or password.
    InvalidCredentials,
    /// The session ended; sign in again.
    SessionExpired,
    /// An administrator action needs a fresh sign-in.
    ReauthRequired,
    /// The two-step verification code or challenge is not valid.
    TfaInvalid,
    /// Too many failed attempts.
    AccountLocked {
        /// When the lock ends, if the engine said.
        until: Option<Timestamp>,
    },
    /// The user was deactivated.
    Deactivated,
}

/// Kind of a 409 conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictReason {
    /// The record changed since it was read (U-17 `STALE` or the interim
    /// fingerprint check).
    Stale,
    /// The record is still used elsewhere.
    InUse,
    /// The record already exists.
    AlreadyExists,
    /// The engine is busy with this resource; try again shortly.
    Busy,
    /// The transaction is in a finished reconciliation (409
    /// `RECONCILED_TRANSACTION`); it can't be deleted or changed that way.
    Reconciled,
    /// The transaction was already deleted (409 `TXN_ALREADY_DELETED`).
    AlreadyDeleted,
    /// This kind of transaction can't be deleted; void it instead (409
    /// `TXN_DELETE_UNSUPPORTED`, Q-245).
    DeleteUnsupported,
    /// Other records depend on this transaction (a deposit that groups
    /// payments, or a payment inside one; 409 `TXN_DELETE_LINKED`).
    DeleteLinked,
    /// Any other conflict.
    Unspecified,
}

/// Every way an engine call can fail.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ApiError {
    /// The engine refused the input.
    #[error("validation failed ({})", code_text(.0.code.as_ref()))]
    Validation(ValidationError),
    /// Bulk Entry rows the engine refused (P15A, `batch-atomic-v1`): the
    /// save posted nothing. Each entry names its group and row number.
    #[error("{} bulk entry rows refused", .0.len())]
    BatchRows(Vec<crate::resources::batch::BatchRowProblem>),
    /// cl-api refused the input before sending anything.
    #[error("refused before sending ({0:?})")]
    Local(LocalValidation),
    /// The record changed, is in use, already exists, or the engine is busy.
    #[error("conflict {reason:?} ({})", code_text(code.as_ref()))]
    Conflict {
        /// What kind of conflict.
        reason: ConflictReason,
        /// The record's current version, when known.
        current_version: Option<RecordVersion>,
        /// The engine code.
        code: Option<ErrorCode>,
        /// The engine's message.
        message: UserText,
    },
    /// The date is in a closed period.
    #[error("closed period (TB_PERIOD_LOCKED)")]
    ClosedPeriod(ClosedPeriod),
    /// The closing-date password was wrong (403 `CLOSING_PASSWORD_INVALID`,
    /// U-15). Nothing changed. The password is never echoed.
    #[error("closing-date password rejected (CLOSING_PASSWORD_INVALID)")]
    ClosingPasswordRejected,
    /// A closing-date password is set and this change needs it (403
    /// `CLOSING_PASSWORD_REQUIRED`, U-15).
    #[error("closing-date password required (CLOSING_PASSWORD_REQUIRED)")]
    ClosingPasswordRequired,
    /// Only the owner may set the first closing-date password (403
    /// `CLOSING_PASSWORD_OWNER_REQUIRED`, U-15).
    #[error("closing-date password: owner required (CLOSING_PASSWORD_OWNER_REQUIRED)")]
    ClosingPasswordOwnerRequired,
    /// Too many wrong closing-date passwords (429
    /// `CLOSING_PASSWORD_RATE_LIMIT`: 10 in 15 minutes per user).
    #[error("closing-date password attempts rate limited")]
    ClosingPasswordRateLimited {
        /// How long to wait, if the engine said.
        retry_after: Option<Duration>,
    },
    /// The owner's own sign-in password was wrong when resetting the
    /// closing-date password (403 `ACCOUNT_PASSWORD_INVALID`, Q-239).
    /// Never echoed.
    #[error("sign-in password rejected (ACCOUNT_PASSWORD_INVALID)")]
    AccountPasswordRejected,
    /// The engine sent money as a JSON number where cl-api needs the exact
    /// decimal string (for example a register from an engine without
    /// `register-exact-v1`). Amounts that may have been rounded are never
    /// shown or used.
    #[error("the engine sent inexact amounts ({what})")]
    InexactMoney {
        /// Which part (never the content).
        what: &'static str,
    },
    /// The feature needs the internet and the app works offline.
    #[error("offline: feature unavailable ({})", feature.as_deref().unwrap_or("-"))]
    OfflineUnavailable {
        /// Engine feature name (`smtp`, `ai`, …).
        feature: Option<String>,
    },
    /// The online feature is not turned on (503 `EGRESS_BLOCKED`).
    #[error("feature not turned on ({})", feature.as_deref().unwrap_or("-"))]
    FeatureNotEnabled {
        /// Engine feature name.
        feature: Option<String>,
    },
    /// AI suggestions are off: AI is not turned on, the company has not
    /// agreed to the AI notice, or the task is switched off (refused
    /// locally with nothing sent, or by the engine's AI gate).
    #[error("AI suggestions are off")]
    AiUnavailable,
    /// Sign-in or session problem.
    #[error("unauthorized ({0:?})")]
    Unauthorized(AuthFailure),
    /// The user may not do this.
    #[error("forbidden ({})", code_text(code.as_ref()))]
    Forbidden {
        /// The engine code.
        code: Option<ErrorCode>,
    },
    /// The record or route does not exist.
    #[error("not found ({})", code_text(code.as_ref()))]
    NotFound {
        /// The engine code.
        code: Option<ErrorCode>,
    },
    /// The engine does not offer this in the desktop app.
    #[error("not available in the desktop app ({code})")]
    Unsupported {
        /// The engine code.
        code: ErrorCode,
    },
    /// A company must be open first.
    #[error("no company is open")]
    NoCompanyOpen,
    /// Too many requests; wait and try again.
    #[error("rate limited")]
    RateLimited {
        /// How long to wait, if the engine said.
        retry_after: Option<Duration>,
    },
    /// The request is too large.
    #[error("payload too large")]
    PayloadTooLarge,
    /// The engine is not running or did not answer as the engine.
    #[error("engine unavailable (permanent: {permanent})")]
    EngineUnavailable {
        /// The engine will not come back without the owner.
        permanent: bool,
    },
    /// A save may or may not have been recorded; check before trying again.
    #[error("outcome unknown, request {request_id}")]
    OutcomeUnknown {
        /// The request's `X-Request-Id`.
        request_id: RequestId,
        /// The idempotency key the save used.
        idempotency_key: Option<IdempotencyKey>,
    },
    /// The engine did not answer in time.
    #[error("timed out")]
    Timeout,
    /// The engine failed.
    #[error("engine error {status} ({})", code_text(code.as_ref()))]
    Server {
        /// HTTP status.
        status: u16,
        /// The engine code.
        code: Option<ErrorCode>,
    },
    /// A request cl-api should never send (a bug on our side).
    #[error("protocol error {status} ({})", code_text(code.as_ref()))]
    Protocol {
        /// HTTP status.
        status: u16,
        /// The engine code.
        code: Option<ErrorCode>,
    },
    /// A response could not be read.
    #[error("could not read the engine's answer ({what})")]
    Decode {
        /// Which part (never the content).
        what: &'static str,
    },
    /// The engine answered a save with 2xx, so it **is** recorded, but its
    /// answer could not be read (for example a newer engine sent a value
    /// this version does not know). Owner wording: "Saved — reload to see
    /// it". Never "try again": that would enter it twice.
    #[error("saved, but the answer could not be read ({what})")]
    SavedUnreadable {
        /// Which part (never the content).
        what: &'static str,
    },
    /// A read result from before a company switch (queue only).
    #[error("result from a previous company")]
    StaleScope,
    /// A read was cancelled because its window closed (queue only; writes
    /// are never cancelled).
    #[error("cancelled")]
    Cancelled,
    /// A background job failed unexpectedly (queue only).
    #[error("internal error")]
    Internal,
}

fn code_text(code: Option<&ErrorCode>) -> &str {
    code.map_or("no code", ErrorCode::as_str)
}

impl OwnerFacing for ApiError {
    fn owner_message(&self) -> OwnerMessage {
        let (key, fix) = match self {
            ApiError::Validation(_) => ("error.api.validation", Some("fix.api.validation")),
            ApiError::BatchRows(_) => ("error.api.batch_rows", Some("fix.api.batch_rows")),
            ApiError::Local(_) => ("error.api.local", Some("fix.api.validation")),
            ApiError::Conflict { reason, .. } => match reason {
                ConflictReason::Stale => ("error.api.conflict_stale", Some("fix.api.reload")),
                ConflictReason::InUse => ("error.api.conflict_in_use", None),
                ConflictReason::AlreadyExists => ("error.api.conflict_exists", None),
                ConflictReason::Busy => ("error.api.busy", Some("fix.api.try_again_soon")),
                ConflictReason::Reconciled => ("error.api.conflict_reconciled", None),
                ConflictReason::AlreadyDeleted => {
                    ("error.api.conflict_deleted", Some("fix.api.reload"))
                }
                ConflictReason::DeleteUnsupported => {
                    ("error.api.delete_unsupported", Some("fix.api.void_instead"))
                }
                ConflictReason::DeleteLinked => {
                    ("error.api.delete_linked", Some("fix.api.void_instead"))
                }
                ConflictReason::Unspecified => ("error.api.conflict", None),
            },
            ApiError::ClosedPeriod(_) => ("error.api.closed_period", Some("fix.api.closed_period")),
            ApiError::ClosingPasswordRejected => ("error.api.closing_password_wrong", None),
            ApiError::ClosingPasswordRequired => (
                "error.api.closing_password_required",
                Some("fix.api.closing_password_required"),
            ),
            ApiError::ClosingPasswordOwnerRequired => (
                "error.api.closing_password_owner",
                Some("fix.api.ask_owner"),
            ),
            ApiError::ClosingPasswordRateLimited { .. } => {
                ("error.api.closing_password_attempts", Some("fix.api.wait"))
            }
            ApiError::AccountPasswordRejected => ("error.api.account_password_wrong", None),
            ApiError::InexactMoney { .. } => ("error.api.inexact_money", Some("fix.api.report")),
            ApiError::OfflineUnavailable { .. } => ("error.api.offline", Some("fix.api.offline")),
            ApiError::FeatureNotEnabled { .. } => {
                ("error.api.feature_off", Some("fix.api.feature_off"))
            }
            ApiError::AiUnavailable => ("error.api.ai_unavailable", Some("fix.api.ai_unavailable")),
            ApiError::Unauthorized(f) => match f {
                AuthFailure::InvalidCredentials => ("error.api.bad_credentials", None),
                AuthFailure::SessionExpired | AuthFailure::ReauthRequired => {
                    ("error.api.session_expired", Some("fix.api.sign_in"))
                }
                AuthFailure::TfaInvalid => ("error.api.tfa_invalid", None),
                AuthFailure::AccountLocked { .. } => ("error.api.locked", Some("fix.api.wait")),
                AuthFailure::Deactivated => ("error.api.deactivated", None),
            },
            ApiError::Forbidden { .. } => ("error.api.forbidden", None),
            ApiError::NotFound { .. } => ("error.api.not_found", Some("fix.api.reload")),
            ApiError::Unsupported { .. } => ("error.api.unsupported", None),
            ApiError::NoCompanyOpen => ("error.api.no_company", Some("fix.api.open_company")),
            ApiError::RateLimited { .. } => ("error.api.busy", Some("fix.api.try_again_soon")),
            ApiError::PayloadTooLarge => ("error.api.too_large", None),
            ApiError::EngineUnavailable { .. } | ApiError::Timeout => (
                "error.api.engine_unavailable",
                Some("fix.api.try_again_soon"),
            ),
            ApiError::OutcomeUnknown { .. } => {
                ("error.api.outcome_unknown", Some("fix.api.check_again"))
            }
            ApiError::Server { .. }
            | ApiError::Protocol { .. }
            | ApiError::Decode { .. }
            | ApiError::Internal => ("error.api.internal", Some("fix.api.report")),
            ApiError::SavedUnreadable { .. } => {
                ("error.api.saved_unreadable", Some("fix.api.reload_to_see"))
            }
            ApiError::StaleScope => ("error.api.stale_scope", None),
            ApiError::Cancelled => ("error.api.cancelled", None),
        };
        OwnerMessage {
            key,
            suggestion_key: fix,
        }
    }

    fn scrubbed_detail(&self) -> String {
        self.to_string()
    }
}

/// Which kind of route a failure came from (affects 400/401 mapping).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RouteKind {
    /// No user session (`/health`, `/desktop/info`).
    Public,
    /// `/auth/login`.
    Login,
    /// `/auth/tfa/*` (TFA token in `Authorization`).
    Tfa,
    /// `/auth/refresh`.
    Refresh,
    /// Every route behind the engine's `authenticate`.
    Session,
}

/// What the executor should do with a non-2xx answer.
#[derive(Debug)]
pub(crate) enum Mapped {
    /// Return this error (or retry it per the operation's class).
    Final(ApiError),
    /// 401 `DESKTOP_TOKEN_REQUIRED` or 421: the desktop gate refused before
    /// any handler ran. Re-read the endpoint and replay once.
    DesktopGate,
    /// A code-less or non-JSON 401 where the engine always codes its 401s:
    /// not the engine's `authenticate`, possibly a foreign program (B4). Never
    /// refresh; re-read the endpoint and replay only on a changed endpoint.
    NotEngine,
    /// An engine-shaped 401 `UNAUTHORIZED` from `authenticate`: the session
    /// layer may refresh and replay (§2.3).
    SessionRejected,
}

/// The parsed `{error: {...}}` body. `None` when the body is not that shape.
struct EngineErrorBody {
    message: String,
    code: Option<ErrorCode>,
    raw: Value,
}

impl EngineErrorBody {
    fn parse(body: &[u8]) -> Option<EngineErrorBody> {
        let v: Value = serde_json::from_slice(body).ok()?;
        let err = v.get("error")?.as_object()?;
        let message = err.get("message")?.as_str()?.to_string();
        let code = err
            .get("code")
            .and_then(Value::as_str)
            .and_then(ErrorCode::parse);
        Some(EngineErrorBody {
            message,
            code,
            raw: v,
        })
    }

    fn pointer(&self, p: &str) -> Option<&Value> {
        self.raw.pointer(p)
    }

    fn is(&self, code: &str) -> bool {
        self.code.as_ref().is_some_and(|c| c.as_str() == code)
    }

    fn text(&self) -> UserText {
        UserText::new(self.message.clone())
    }
}

fn validation(body: &EngineErrorBody) -> ValidationError {
    let issues = body
        .pointer("/error/details")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|issue| {
                    let message = issue.get("message")?.as_str()?;
                    let path = issue.get("path").and_then(Value::as_array).and_then(|p| {
                        let parts: Vec<String> = p
                            .iter()
                            .filter_map(|seg| match seg {
                                Value::String(s) => Some(s.clone()),
                                Value::Number(n) => n.as_u64().map(|n| n.to_string()),
                                _ => None,
                            })
                            .collect();
                        (!parts.is_empty()).then(|| FieldPath(parts.join(".")))
                    });
                    Some(ValidationIssue {
                        path,
                        message: UserText::new(message),
                    })
                })
                .collect()
        })
        .or_else(|| {
            // Object-shaped details naming one field (`{field, earliest}`).
            let field = body.pointer("/error/details/field")?.as_str()?;
            Some(vec![ValidationIssue {
                path: Some(FieldPath(field.to_string())),
                message: body.text(),
            }])
        })
        .unwrap_or_default();
    ValidationError {
        code: body.code.clone(),
        message: body.text(),
        issues,
    }
}

fn conflict(body: &EngineErrorBody) -> ApiError {
    let reason = match body.code.as_ref().map(ErrorCode::as_str) {
        Some("STALE") => ConflictReason::Stale,
        Some("TAG_IN_USE") => ConflictReason::InUse,
        Some("EMAIL_EXISTS" | "ACCOUNT_NUMBER_EXISTS") => ConflictReason::AlreadyExists,
        // `BATCH_BUSY` (P15A): another batch save of the company is running,
        // or a row lock was not free in time. Nothing was posted by this
        // request; a retry with the same idempotency key replays a finished
        // save.
        Some("TENANT_BUSY" | "IDEMPOTENCY_IN_PROGRESS" | "BATCH_BUSY") => ConflictReason::Busy,
        Some("RECONCILED_TRANSACTION") => ConflictReason::Reconciled,
        Some("TXN_ALREADY_DELETED") => ConflictReason::AlreadyDeleted,
        Some("TXN_DELETE_UNSUPPORTED") => ConflictReason::DeleteUnsupported,
        Some("TXN_DELETE_LINKED") => ConflictReason::DeleteLinked,
        _ => ConflictReason::Unspecified,
    };
    let current_version = if reason == ConflictReason::Stale {
        body.pointer("/error/details/currentVersion")
            .and_then(Value::as_u64)
            .map(RecordVersion::Server)
    } else {
        None
    };
    ApiError::Conflict {
        reason,
        current_version,
        code: body.code.clone(),
        message: body.text(),
    }
}

fn closed_period(body: &EngineErrorBody) -> ClosedPeriod {
    ClosedPeriod {
        // Only a plain `YYYY-MM-DD` is accepted, so a timestamp can never
        // shift the date through a time zone.
        lock_date: body
            .pointer("/error/details/lockDate")
            .and_then(Value::as_str)
            .and_then(|s| CalendarDate::parse_wire(s).ok()),
        can_override: body
            .pointer("/error/details/canOverride")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        password_required: body
            .pointer("/error/details/closingPasswordRequired")
            .or_else(|| body.pointer("/error/details/passwordRequired"))
            .and_then(Value::as_bool),
        message: body.text(),
    }
}

fn feature(body: Option<&EngineErrorBody>) -> Option<String> {
    body.and_then(|b| b.pointer("/error/feature"))
        .and_then(Value::as_str)
        .filter(|f| {
            f.len() <= 64
                && f.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
        .map(str::to_string)
}

/// A Bulk Entry refusal naming its rows (`details.rowNumbers`, P15A
/// `batch-atomic-v1`): any 4xx except the ones with their own meaning —
/// closed periods and the closing-date password (423, 403, 429), a reused
/// idempotency key, and a whole-request schema error.
fn batch_rows(
    status: u16,
    body: &EngineErrorBody,
) -> Option<Vec<crate::resources::batch::BatchRowProblem>> {
    if !(400..500).contains(&status) || matches!(status, 401 | 423 | 429) {
        return None;
    }
    if [
        "VALIDATION_ERROR",
        "IDEMPOTENCY_KEY_REUSED",
        "IDEMPOTENCY_KEY_INVALID",
        "IDEMPOTENCY_UNSUPPORTED",
        "CLOSING_PASSWORD_INVALID",
        "CLOSING_PASSWORD_REQUIRED",
        "CLOSING_PASSWORD_OWNER_REQUIRED",
        "DESKTOP_TOKEN_REQUIRED",
        "WORKSPACE_TENANT",
    ]
    .iter()
    .any(|c| body.is(c))
    {
        return None;
    }
    let details = body.pointer("/error/details")?;
    crate::resources::batch::rows_from_details(
        details,
        &body.message,
        body.code.as_ref().map(ErrorCode::as_str),
    )
}

/// Maps a non-2xx answer: Bulk Entry rows first, then the §2.4 table.
pub(crate) fn map_failure(
    status: u16,
    body: &[u8],
    retry_after: Option<Duration>,
    route: RouteKind,
) -> Mapped {
    if let Some(rows) = EngineErrorBody::parse(body).and_then(|b| batch_rows(status, &b)) {
        return Mapped::Final(ApiError::BatchRows(rows));
    }
    map_status(status, body, retry_after, route)
}

/// The §2.4 table.
fn map_status(status: u16, body: &[u8], retry_after: Option<Duration>, route: RouteKind) -> Mapped {
    let parsed = EngineErrorBody::parse(body);
    let code = parsed.as_ref().and_then(|b| b.code.clone());
    let has = |c: &str| parsed.as_ref().is_some_and(|b| b.is(c));
    let fin = Mapped::Final;
    match status {
        400 => match &parsed {
            Some(b) if b.is("VALIDATION_ERROR") => fin(ApiError::Validation(validation(b))),
            Some(b) if b.is("INVALID_JSON") => fin(ApiError::Protocol { status, code }),
            Some(b)
                if route == RouteKind::Tfa
                    && b.code.is_none()
                    && b.pointer("/error/locked_until")
                        .is_some_and(|v| !v.is_null()) =>
            {
                let until = b
                    .pointer("/error/locked_until")
                    .and_then(Value::as_str)
                    .and_then(|s| Timestamp::parse_wire(s).ok());
                fin(ApiError::Unauthorized(AuthFailure::AccountLocked { until }))
            }
            Some(b) if route == RouteKind::Tfa && b.code.is_none() => {
                fin(ApiError::Unauthorized(AuthFailure::TfaInvalid))
            }
            Some(b) => fin(ApiError::Validation(validation(b))),
            None => fin(ApiError::Protocol { status, code: None }),
        },
        401 => {
            if has("DESKTOP_TOKEN_REQUIRED") {
                return Mapped::DesktopGate;
            }
            if has("INVALID_CREDENTIALS") {
                return fin(ApiError::Unauthorized(AuthFailure::InvalidCredentials));
            }
            if has("ADMIN_SESSION_EXPIRED") {
                return fin(ApiError::Unauthorized(AuthFailure::ReauthRequired));
            }
            match route {
                // Any 401 from /auth/refresh (missing, reused or expired
                // refresh token) ends the session; never refresh again.
                RouteKind::Refresh => fin(ApiError::Unauthorized(AuthFailure::SessionExpired)),
                RouteKind::Tfa => fin(ApiError::Unauthorized(AuthFailure::TfaInvalid)),
                _ if code.is_none() => Mapped::NotEngine,
                RouteKind::Session if has("UNAUTHORIZED") => Mapped::SessionRejected,
                RouteKind::Login | RouteKind::Public => {
                    fin(ApiError::Unauthorized(AuthFailure::InvalidCredentials))
                }
                RouteKind::Session => fin(ApiError::Unauthorized(AuthFailure::SessionExpired)),
            }
        }
        403 if has("ACCOUNT_LOCKED") => fin(ApiError::Unauthorized(AuthFailure::AccountLocked {
            until: None,
        })),
        403 if has("ACCOUNT_DEACTIVATED") => fin(ApiError::Unauthorized(AuthFailure::Deactivated)),
        403 if has("ORIGIN_NOT_ALLOWED") => fin(ApiError::Protocol { status, code }),
        403 if has("CLOSING_PASSWORD_INVALID") => fin(ApiError::ClosingPasswordRejected),
        403 if has("CLOSING_PASSWORD_REQUIRED") => fin(ApiError::ClosingPasswordRequired),
        403 if has("CLOSING_PASSWORD_OWNER_REQUIRED") => {
            fin(ApiError::ClosingPasswordOwnerRequired)
        }
        403 if has("ACCOUNT_PASSWORD_INVALID") => fin(ApiError::AccountPasswordRejected),
        403 => fin(ApiError::Forbidden { code }),
        404 => match code {
            Some(c) if c.as_str() == "NOT_AVAILABLE_IN_DESKTOP" => {
                fin(ApiError::Unsupported { code: c })
            }
            code => fin(ApiError::NotFound { code }),
        },
        409 if has("WORKSPACE_TENANT") => fin(ApiError::NoCompanyOpen),
        409 => match &parsed {
            Some(b) => fin(conflict(b)),
            None => fin(ApiError::Conflict {
                reason: ConflictReason::Unspecified,
                current_version: None,
                code: None,
                message: UserText::default(),
            }),
        },
        413 => fin(ApiError::PayloadTooLarge),
        421 => Mapped::DesktopGate,
        422 if has("IDEMPOTENCY_KEY_REUSED") => fin(ApiError::Protocol { status, code }),
        422 => match &parsed {
            Some(b) => fin(ApiError::Validation(validation(b))),
            None => fin(ApiError::Protocol { status, code: None }),
        },
        423 => match &parsed {
            Some(b) if b.is("TB_PERIOD_LOCKED") => fin(ApiError::ClosedPeriod(closed_period(b))),
            _ => fin(ApiError::Forbidden { code }),
        },
        429 if has("CLOSING_PASSWORD_RATE_LIMIT") => {
            fin(ApiError::ClosingPasswordRateLimited { retry_after })
        }
        429 => fin(ApiError::RateLimited { retry_after }),
        503 if has("OFFLINE_UNAVAILABLE") => fin(ApiError::OfflineUnavailable {
            feature: feature(parsed.as_ref()),
        }),
        503 if has("EGRESS_BLOCKED") => fin(ApiError::FeatureNotEnabled {
            feature: feature(parsed.as_ref()),
        }),
        500..=599 => fin(ApiError::Server { status, code }),
        _ => fin(ApiError::Protocol { status, code }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{TOKEN, fast_config};
    use crate::client::{AttemptAuth, Call, Method, Transport};
    use crate::endpoint::FixedEndpoint;
    use cl_core::Secret;
    use std::sync::Arc;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn map(status: u16, body: &str, route: RouteKind) -> Mapped {
        map_failure(status, body.as_bytes(), None, route)
    }

    fn err(status: u16, body: &str, route: RouteKind) -> ApiError {
        match map(status, body, route) {
            Mapped::Final(e) => e,
            other => panic!("expected a final error, got {other:?}"),
        }
    }

    fn code(c: &str) -> Option<ErrorCode> {
        ErrorCode::parse(c)
    }

    const S: RouteKind = RouteKind::Session;

    #[test]
    fn validation_error_reads_issues_and_first_field() {
        let body = r#"{"error":{"message":"lines: At least 2 lines required","code":"VALIDATION_ERROR",
            "details":[{"path":["lines"],"message":"At least 2 lines required","code":"too_small"},
                       {"path":["lines",0,"debit"],"message":"Required"}]}}"#;
        let ApiError::Validation(v) = err(400, body, S) else {
            panic!("validation")
        };
        assert_eq!(v.field().map(FieldPath::as_str), Some("lines"));
        assert_eq!(
            v.issues()[1].path.as_ref().map(FieldPath::as_str),
            Some("lines.0.debit")
        );
        assert_eq!(v.code().map(ErrorCode::as_str), Some("VALIDATION_ERROR"));
        assert_eq!(v.message().as_str(), "lines: At least 2 lines required");
    }

    #[test]
    fn validation_error_reads_an_object_detail_field() {
        let body = r#"{"error":{"message":"Auto schedules can start no earlier than 2025-11-03","code":"VALIDATION_ERROR",
            "details":{"field":"schedule.nextDate","earliest":"2025-11-03"}}}"#;
        let e = err(400, body, S);
        let ApiError::Validation(v) = &e else {
            panic!("validation")
        };
        assert_eq!(v.field().map(FieldPath::as_str), Some("schedule.nextDate"));
        assert!(crate::is_auto_start_too_early(&e));
        let twin =
            crate::auto_start_too_early(cl_core::CalendarDate::from_ymd(2025, 11, 3).expect("d"));
        assert!(crate::is_auto_start_too_early(&twin));
        // Other refusals are not it.
        let other = r#"{"error":{"message":"x","code":"VALIDATION_ERROR","details":{"lockDate":"2026-01-01"}}}"#;
        assert!(!crate::is_auto_start_too_early(&err(400, other, S)));
    }

    #[test]
    #[allow(clippy::too_many_lines, reason = "one row per table line")]
    fn mapping_table_rows() {
        type Row = (u16, &'static str, RouteKind, ApiError);
        let rows: Vec<Row> = vec![
            (
                400,
                r#"{"error":{"message":"bad","code":"INVALID_JSON"}}"#,
                S,
                ApiError::Protocol {
                    status: 400,
                    code: code("INVALID_JSON"),
                },
            ),
            (
                401,
                r#"{"error":{"message":"x","code":"INVALID_CREDENTIALS"}}"#,
                RouteKind::Login,
                ApiError::Unauthorized(AuthFailure::InvalidCredentials),
            ),
            (
                401,
                r#"{"error":{"message":"x","code":"ADMIN_SESSION_EXPIRED"}}"#,
                S,
                ApiError::Unauthorized(AuthFailure::ReauthRequired),
            ),
            (
                401,
                r#"{"error":{"message":"Missing TFA token"}}"#,
                RouteKind::Tfa,
                ApiError::Unauthorized(AuthFailure::TfaInvalid),
            ),
            (
                401,
                r#"{"error":{"message":"Missing refresh token"}}"#,
                RouteKind::Refresh,
                ApiError::Unauthorized(AuthFailure::SessionExpired),
            ),
            (
                401,
                r#"{"error":{"message":"Invalid refresh token","code":"UNAUTHORIZED"}}"#,
                RouteKind::Refresh,
                ApiError::Unauthorized(AuthFailure::SessionExpired),
            ),
            (
                403,
                r#"{"error":{"message":"x","code":"ACCOUNT_LOCKED"}}"#,
                RouteKind::Login,
                ApiError::Unauthorized(AuthFailure::AccountLocked { until: None }),
            ),
            (
                403,
                r#"{"error":{"message":"x","code":"ACCOUNT_DEACTIVATED"}}"#,
                RouteKind::Login,
                ApiError::Unauthorized(AuthFailure::Deactivated),
            ),
            (
                403,
                r#"{"error":{"message":"x","code":"ORIGIN_NOT_ALLOWED"}}"#,
                S,
                ApiError::Protocol {
                    status: 403,
                    code: code("ORIGIN_NOT_ALLOWED"),
                },
            ),
            (
                403,
                r#"{"error":{"message":"x","code":"PERMISSION_DENIED"}}"#,
                S,
                ApiError::Forbidden {
                    code: code("PERMISSION_DENIED"),
                },
            ),
            (
                404,
                r#"{"error":{"message":"x","code":"NOT_AVAILABLE_IN_DESKTOP"}}"#,
                S,
                ApiError::Unsupported {
                    code: ErrorCode("NOT_AVAILABLE_IN_DESKTOP".into()),
                },
            ),
            (
                404,
                r#"{"error":{"message":"x","code":"TENANT_NOT_FOUND"}}"#,
                S,
                ApiError::NotFound {
                    code: code("TENANT_NOT_FOUND"),
                },
            ),
            (
                404,
                r#"{"error":{"message":"Not found"}}"#,
                S,
                ApiError::NotFound { code: None },
            ),
            (
                409,
                r#"{"error":{"message":"x","code":"WORKSPACE_TENANT"}}"#,
                S,
                ApiError::NoCompanyOpen,
            ),
            (
                413,
                r#"{"error":{"message":"x","code":"PAYLOAD_TOO_LARGE"}}"#,
                S,
                ApiError::PayloadTooLarge,
            ),
            (
                422,
                r#"{"error":{"message":"x","code":"IDEMPOTENCY_KEY_REUSED"}}"#,
                S,
                ApiError::Protocol {
                    status: 422,
                    code: code("IDEMPOTENCY_KEY_REUSED"),
                },
            ),
            (
                429,
                r#"{"error":{"message":"x","code":"TOO_MANY_REQUESTS"}}"#,
                S,
                ApiError::RateLimited { retry_after: None },
            ),
            (
                500,
                r#"{"error":{"message":"x","code":"INTERNAL_ERROR"}}"#,
                S,
                ApiError::Server {
                    status: 500,
                    code: code("INTERNAL_ERROR"),
                },
            ),
            (
                502,
                "bad gateway",
                S,
                ApiError::Server {
                    status: 502,
                    code: None,
                },
            ),
            (
                504,
                "",
                S,
                ApiError::Server {
                    status: 504,
                    code: None,
                },
            ),
            (
                503,
                r#"{"error":{"message":"x","code":"OFFLINE_UNAVAILABLE","feature":"smtp"}}"#,
                S,
                ApiError::OfflineUnavailable {
                    feature: Some("smtp".into()),
                },
            ),
            (
                503,
                r#"{"error":{"message":"x","code":"EGRESS_BLOCKED","feature":"ai"}}"#,
                S,
                ApiError::FeatureNotEnabled {
                    feature: Some("ai".into()),
                },
            ),
            (
                503,
                r#"{"error":{"message":"x","code":"SETUP_BUSY"}}"#,
                S,
                ApiError::Server {
                    status: 503,
                    code: code("SETUP_BUSY"),
                },
            ),
            (
                405,
                "",
                S,
                ApiError::Protocol {
                    status: 405,
                    code: None,
                },
            ),
        ];
        for (status, body, route, want) in rows {
            assert_eq!(err(status, body, route), want, "{status} {body}");
        }
    }

    #[test]
    fn code_less_400_is_validation_without_code() {
        let ApiError::Validation(v) = err(
            400,
            r#"{"error":{"message":"Transaction does not balance: debits (1.00) != credits (2.00)"}}"#,
            S,
        ) else {
            panic!("validation")
        };
        assert_eq!(v.code(), None);
        assert_eq!(v.field(), None);
        let ApiError::Validation(v) = err(422, r#"{"error":{"message":"x","code":"NOPE"}}"#, S)
        else {
            panic!("422 validation")
        };
        assert_eq!(v.code().map(ErrorCode::as_str), Some("NOPE"));
        assert_eq!(
            err(400, "<html>", S),
            ApiError::Protocol {
                status: 400,
                code: None
            }
        );
    }

    #[test]
    fn tfa_lock_is_account_locked_s6() {
        let body = r#"{"error":{"message":"Account temporarily locked","remaining_attempts":0,
            "locked_until":"2026-10-15T12:30:00.000Z"}}"#;
        let ApiError::Unauthorized(AuthFailure::AccountLocked { until }) =
            err(400, body, RouteKind::Tfa)
        else {
            panic!("locked")
        };
        assert_eq!(
            until.map(Timestamp::to_wire).as_deref(),
            Some("2026-10-15T12:30:00.000Z")
        );
        let wrong = r#"{"error":{"message":"Invalid code","remaining_attempts":3}}"#;
        assert_eq!(
            err(400, wrong, RouteKind::Tfa),
            ApiError::Unauthorized(AuthFailure::TfaInvalid)
        );
        let null_lock = r#"{"error":{"message":"Invalid code","locked_until":null}}"#;
        assert_eq!(
            err(400, null_lock, RouteKind::Tfa),
            ApiError::Unauthorized(AuthFailure::TfaInvalid)
        );
        // Outside TFA routes the same body is ordinary validation.
        assert!(matches!(err(400, body, S), ApiError::Validation(_)));
    }

    #[test]
    fn b4_only_engine_shaped_unauthorized_may_refresh() {
        assert!(matches!(
            map(
                401,
                r#"{"error":{"message":"Token expired","code":"UNAUTHORIZED"}}"#,
                S
            ),
            Mapped::SessionRejected
        ));
        for body in [
            "",
            "Unauthorized",
            "{}",
            r#"{"error":{"message":"no code"}}"#,
            r#"{"error":"x"}"#,
            r#"{"error":{"code":"UNAUTHORIZED"}}"#,
        ] {
            assert!(matches!(map(401, body, S), Mapped::NotEngine), "{body:?}");
        }
        // Public routes have no authenticate: never a refresh.
        assert!(matches!(map(401, "", RouteKind::Public), Mapped::NotEngine));
        assert_eq!(
            err(
                401,
                r#"{"error":{"message":"x","code":"UNAUTHORIZED"}}"#,
                RouteKind::Public
            ),
            ApiError::Unauthorized(AuthFailure::InvalidCredentials)
        );
        assert!(matches!(
            map(
                401,
                r#"{"error":{"message":"x","code":"DESKTOP_TOKEN_REQUIRED"}}"#,
                S
            ),
            Mapped::DesktopGate
        ));
        assert!(matches!(
            map(421, r#"{"error":{"code":"HOST_NOT_ALLOWED"}}"#, S),
            Mapped::DesktopGate
        ));
    }

    #[test]
    fn conflicts_and_stale_version() {
        let stale = r#"{"error":{"code":"STALE","message":"changed",
            "details":{"entity":"account","id":"x","currentVersion":8,"updatedAt":"2026-01-01T00:00:00.000Z"}}}"#;
        assert!(matches!(
            err(409, stale, S),
            ApiError::Conflict {
                reason: ConflictReason::Stale,
                current_version: Some(RecordVersion::Server(8)),
                ..
            }
        ));
        for (c, want) in [
            ("TAG_IN_USE", ConflictReason::InUse),
            ("EMAIL_EXISTS", ConflictReason::AlreadyExists),
            ("TENANT_BUSY", ConflictReason::Busy),
            ("IDEMPOTENCY_IN_PROGRESS", ConflictReason::Busy),
            ("BATCH_BUSY", ConflictReason::Busy),
            ("SOMETHING", ConflictReason::Unspecified),
        ] {
            let body = format!(r#"{{"error":{{"message":"m","code":"{c}"}}}}"#);
            let ApiError::Conflict {
                reason,
                current_version,
                ..
            } = err(409, &body, S)
            else {
                panic!("conflict")
            };
            assert_eq!((reason, current_version), (want, None), "{c}");
        }
        let ApiError::Conflict { reason, code, .. } =
            err(409, r#"{"error":{"message":"Tag name already exists"}}"#, S)
        else {
            panic!("conflict")
        };
        assert_eq!((reason, code), (ConflictReason::Unspecified, None));
    }

    #[test]
    fn closed_period_reads_details_and_rejects_shifted_dates() {
        let body = r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED",
            "details":{"lockDate":"2026-03-31","canOverride":true}}}"#;
        let ApiError::ClosedPeriod(cp) = err(423, body, S) else {
            panic!("closed")
        };
        assert_eq!(
            cp.lock_date.map(CalendarDate::to_wire).as_deref(),
            Some("2026-03-31")
        );
        assert!(cp.can_override);
        assert_eq!(cp.password_required, None);
        let ts = r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED",
            "details":{"lockDate":"2026-03-31T00:00:00.000Z","canOverride":false,"passwordRequired":true}}}"#;
        let ApiError::ClosedPeriod(cp) = err(423, ts, S) else {
            panic!("closed")
        };
        assert_eq!(cp.lock_date, None);
        assert!(!cp.can_override);
        assert_eq!(cp.password_required, Some(true));
        // U-15 (engine PR #153) names it `closingPasswordRequired`.
        let u15 = r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED",
            "details":{"lockDate":"2026-03-31","canOverride":false,"closingPasswordRequired":true}}}"#;
        let ApiError::ClosedPeriod(cp) = err(423, u15, S) else {
            panic!("closed")
        };
        assert_eq!(cp.password_required, Some(true));
        assert_eq!(
            cp.lock_date.map(CalendarDate::to_wire).as_deref(),
            Some("2026-03-31")
        );
    }

    #[test]
    fn closing_password_refusals_are_typed_and_never_echo() {
        let body = |code: &str| {
            format!(
                r#"{{"error":{{"message":"Closing password hunter2 is wrong","code":"{code}"}}}}"#
            )
        };
        assert_eq!(
            err(403, &body("CLOSING_PASSWORD_INVALID"), S),
            ApiError::ClosingPasswordRejected
        );
        assert_eq!(
            err(403, &body("CLOSING_PASSWORD_REQUIRED"), S),
            ApiError::ClosingPasswordRequired
        );
        assert_eq!(
            err(403, &body("CLOSING_PASSWORD_OWNER_REQUIRED"), S),
            ApiError::ClosingPasswordOwnerRequired
        );
        let Mapped::Final(e) = map_failure(
            429,
            body("CLOSING_PASSWORD_RATE_LIMIT").as_bytes(),
            Some(Duration::from_mins(15)),
            S,
        ) else {
            panic!("final")
        };
        assert_eq!(
            e,
            ApiError::ClosingPasswordRateLimited {
                retry_after: Some(Duration::from_mins(15))
            }
        );
        // Other 403s and 429s are unchanged.
        assert!(matches!(
            err(403, &body("FORBIDDEN"), S),
            ApiError::Forbidden { .. }
        ));
        assert!(matches!(
            err(429, &body("RATE_LIMIT"), S),
            ApiError::RateLimited { .. }
        ));
        for code in [
            "CLOSING_PASSWORD_INVALID",
            "CLOSING_PASSWORD_REQUIRED",
            "CLOSING_PASSWORD_OWNER_REQUIRED",
        ] {
            let e = err(403, &body(code), S);
            let shown = format!("{e} | {e:?} | {}", e.scrubbed_detail());
            assert!(!shown.contains("hunter2"), "{shown}");
            assert!(matches!(
                e.owner_message().key,
                "error.api.closing_password_wrong"
                    | "error.api.closing_password_required"
                    | "error.api.closing_password_owner"
            ));
        }
    }

    #[test]
    fn delete_refusals_and_the_reset_password_are_typed() {
        let body = |code: &str| {
            format!(r#"{{"error":{{"message":"Acme Ltd check 1001 hunter2","code":"{code}"}}}}"#)
        };
        for (code, reason, key) in [
            (
                "RECONCILED_TRANSACTION",
                ConflictReason::Reconciled,
                "error.api.conflict_reconciled",
            ),
            (
                "TXN_ALREADY_DELETED",
                ConflictReason::AlreadyDeleted,
                "error.api.conflict_deleted",
            ),
            (
                "TXN_DELETE_UNSUPPORTED",
                ConflictReason::DeleteUnsupported,
                "error.api.delete_unsupported",
            ),
            (
                "TXN_DELETE_LINKED",
                ConflictReason::DeleteLinked,
                "error.api.delete_linked",
            ),
        ] {
            let e = err(409, &body(code), S);
            assert!(
                matches!(&e, ApiError::Conflict { reason: r, .. } if *r == reason),
                "{code}: {e:?}"
            );
            assert_eq!(e.owner_message().key, key);
            let shown = format!("{e} | {}", e.scrubbed_detail());
            assert!(!shown.contains("Acme"), "{shown}");
        }
        let e = err(403, &body("ACCOUNT_PASSWORD_INVALID"), S);
        assert_eq!(e, ApiError::AccountPasswordRejected);
        assert!(!format!("{e} | {e:?}").contains("hunter2"));
        assert_eq!(e.owner_message().key, "error.api.account_password_wrong");
    }

    #[test]
    fn display_and_debug_never_carry_engine_text() {
        let body = r#"{"error":{"message":"Transaction does not balance: debits (1234.56) != credits (99.00) for Acme Ltd",
            "code":"VALIDATION_ERROR","details":[{"path":["memo"],"message":"Acme Ltd memo 1234.56"}]}}"#;
        let e = err(400, body, S);
        let shown = format!("{e} | {e:?} | {}", e.scrubbed_detail());
        for canary in ["1234.56", "Acme", "memo 1"] {
            assert!(!shown.contains(canary), "{canary} leaked: {shown}");
        }
        assert_eq!(e.owner_message().key, "error.api.validation");
        let e = err(
            409,
            r#"{"error":{"message":"Tag Acme is used","code":"TAG_IN_USE","details":{"name":"Acme"}}}"#,
            S,
        );
        assert!(!format!("{e} {e:?}").contains("Acme"));
    }

    #[test]
    fn codes_are_sanitized() {
        assert!(ErrorCode::parse("VALIDATION_ERROR").is_some());
        for bad in ["", "lower", "WITH SPACE", "A\nB", &"X".repeat(65)] {
            assert!(ErrorCode::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(
            err(
                403,
                r#"{"error":{"message":"x","code":"evil <script>"}}"#,
                S
            ),
            ApiError::Forbidden { code: None }
        );
        assert_eq!(
            err(
                503,
                r#"{"error":{"message":"x","code":"OFFLINE_UNAVAILABLE","feature":"a b"}}"#,
                S
            ),
            ApiError::OfflineUnavailable { feature: None }
        );
    }

    #[test]
    fn every_variant_has_an_owner_message() {
        let all = [
            ApiError::Local(LocalValidation::Unbalanced),
            ApiError::NoCompanyOpen,
            ApiError::Timeout,
            ApiError::StaleScope,
            ApiError::Internal,
            ApiError::Decode { what: "x" },
            ApiError::SavedUnreadable { what: "x" },
            ApiError::EngineUnavailable { permanent: true },
            ApiError::OutcomeUnknown {
                request_id: RequestId::from_uuid(uuid::Uuid::nil()),
                idempotency_key: Some(IdempotencyKey::new()),
            },
            ApiError::Unauthorized(AuthFailure::SessionExpired),
        ];
        for e in all {
            assert!(e.owner_message().key.starts_with("error.api."), "{e:?}");
            assert_ne!(e.scrubbed_detail(), "");
        }
    }

    /// The same rows through a live HTTP stub: status, headers and body as
    /// the engine sends them.
    #[tokio::test]
    async fn stub_rows_through_the_transport() {
        let server = MockServer::start().await;
        let rows: [(&str, u16, &str); 4] = [
            (
                "/v",
                400,
                r#"{"error":{"message":"name: Required","code":"VALIDATION_ERROR","details":[{"path":["name"],"message":"Required"}]}}"#,
            ),
            (
                "/off",
                503,
                r#"{"error":{"message":"Needs internet","code":"OFFLINE_UNAVAILABLE","feature":"smtp"}}"#,
            ),
            (
                "/lock",
                423,
                r#"{"error":{"message":"locked","code":"TB_PERIOD_LOCKED","details":{"lockDate":"2025-12-31","canOverride":true}}}"#,
            ),
            (
                "/rl",
                429,
                r#"{"error":{"message":"slow down","code":"TOO_MANY_REQUESTS"}}"#,
            ),
        ];
        for (p, status, body) in rows {
            let mut t = ResponseTemplate::new(status).set_body_raw(body, "application/json");
            if status == 429 {
                t = t.insert_header("retry-after", "3");
            }
            Mock::given(path(p)).respond_with(t).mount(&server).await;
        }
        let src = Arc::new(FixedEndpoint::new(
            server.address().port(),
            Secret::new(TOKEN),
            1,
        ));
        let t = Transport::new(src, fast_config()).expect("client");
        let mut got = Vec::new();
        for (p, ..) in rows {
            let call = Call::new(Method::Get, "/stub", p);
            let r = t
                .attempt(
                    &call,
                    &AttemptAuth::default(),
                    1,
                    tokio::time::Instant::now() + Duration::from_secs(3),
                )
                .await
                .expect("reply");
            got.push(map_failure(r.status, &r.body, r.retry_after, S));
        }
        let finals: Vec<ApiError> = got
            .into_iter()
            .map(|m| match m {
                Mapped::Final(e) => e,
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(
            matches!(&finals[0], ApiError::Validation(v) if v.field().map(FieldPath::as_str) == Some("name"))
        );
        assert_eq!(
            finals[1],
            ApiError::OfflineUnavailable {
                feature: Some("smtp".into())
            }
        );
        assert!(matches!(&finals[2], ApiError::ClosedPeriod(cp) if cp.can_override));
        assert_eq!(
            finals[3],
            ApiError::RateLimited {
                retry_after: Some(Duration::from_secs(3))
            }
        );
    }
}
