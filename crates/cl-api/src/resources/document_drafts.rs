//! Document drafts (A03, engine U-92 / U-92b, capability
//! `document-drafts-v1`; keys under `idempotency-docs-v1`, U-92a):
//! `/api/v1/document-drafts`.
//!
//! - A draft is the desktop's reading of one vendor document (a bill, a card
//!   charge, a check or a cash expense) waiting for the owner. It is
//!   **claimed** from an uploaded draft file ([`DocumentDrafts::create`]):
//!   the engine hashes the stored bytes, and a file the company already has
//!   as a live or converted draft is refused with
//!   [`DraftRefusal::AlreadyClaimed`] naming that draft (Q-A03i).
//! - The reading itself (`fields`, `lines`, `checks`) is the desktop's JSON:
//!   the engine bounds it and never computes with it. Amounts in it are
//!   decimal strings; JSON numbers must be integers (basis points, page
//!   boxes). [`DraftReading`] is checked here against the engine's bounds
//!   before anything is sent.
//! - Every change names the `updatedAt` it last read (an opaque token,
//!   echoed back exactly as the engine sent it); a different stored value is
//!   409 `STALE` (`Conflict { Stale }`).
//! - **Convert** ([`DocumentDrafts::convert`]) posts the draft through the
//!   kind's own service in one database transaction. Its body is built from
//!   the same typed cl-api builders as manual entry ([`BillDraft`],
//!   [`BankPosting`]), so a draft posts exactly what the form would. The
//!   closing-date password goes at the top level only (U-15). Posted
//!   records that look like the same document refuse the convert with
//!   [`DraftRefusal::PossibleDuplicate`] until the owner acknowledges them.
//! - **Attach** ([`DocumentDrafts::attach`], A04, engine U-95,
//!   `document-drafts-attach-v1`) files the draft's document on a record
//!   that is already posted and closes the draft; nothing posts. Without
//!   that capability it answers `Local(Unsupported)` and sends nothing.
//! - Without `document-drafts-v1` nothing is sent and every call answers
//!   `Local(Unsupported)` (S12: the desktop reads with its rules only).
//! - Document data (references, totals, names, the reading) is owner data:
//!   `Debug` shows ids, kinds and states only, and nothing here logs.

use std::fmt;
use std::str::FromStr;

use cl_core::ids::{ContactId, IdError, TransactionId, parse_uuid};
use cl_core::{CalendarDate, Money};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::attachments::{AttachmentId, uuid_id};
use crate::resources::bank_postings::{BankPosting, BankPostingKind};
use crate::resources::bills::BillDraft;
use crate::resources::operations::{
    DOCUMENT_DRAFT_ATTACH, DOCUMENT_DRAFT_CONVERT, DOCUMENT_DRAFT_DISCARD,
    DOCUMENT_DRAFT_DUPLICATES, DOCUMENT_DRAFT_GET, DOCUMENT_DRAFT_PUT, DOCUMENT_DRAFTS_CREATE,
    DOCUMENT_DRAFTS_LIST, Operation,
};
use crate::resources::posting::{PostingOptions, posting_body};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

uuid_id!(
    /// One document draft (`document_drafts.id`).
    DocumentDraftId
);

/// Bounds of a stored reading (the engine's `DOCUMENT_DRAFT_LIMITS`).
pub mod limits {
    /// `fields`, as JSON bytes.
    pub const FIELDS_BYTES: usize = 64 * 1024;
    /// `lines`, as JSON bytes.
    pub const LINES_BYTES: usize = 256 * 1024;
    /// `checks`, as JSON bytes.
    pub const CHECKS_BYTES: usize = 64 * 1024;
    /// Entries in `lines`.
    pub const MAX_LINES: usize = 500;
    /// Entries in `checks`.
    pub const MAX_CHECKS: usize = 200;
    /// Nesting below the top value.
    pub const MAX_DEPTH: u32 = 6;
    /// Characters (UTF-16 code units) of one string.
    pub const MAX_STRING: usize = 2000;
    /// Characters of one object key.
    pub const MAX_KEY: usize = 100;
    /// The largest integer, either sign.
    pub const MAX_INTEGER: u64 = 1_000_000_000;
    /// Characters of `docReference`.
    pub const MAX_REFERENCE: usize = 100;
    /// Duplicates a convert may acknowledge.
    pub const MAX_ACKNOWLEDGED: usize = 50;
    /// The page size cap of the list.
    pub const MAX_PAGE: u32 = 200;
    /// The deepest `offset` of the list.
    pub const MAX_OFFSET: u32 = 10_000;
}

/// What a document is, or becomes (`document_drafts.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftKind {
    /// A vendor bill (A/P).
    Bill,
    /// A card charge (an expense paid from a card account).
    CardCharge,
    /// A check already written.
    Check,
    /// A cash expense (an expense paid from cash or a bank account).
    CashExpense,
    /// Not known yet.
    Unknown,
}

impl DraftKind {
    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DraftKind::Bill => "bill",
            DraftKind::CardCharge => "card_charge",
            DraftKind::Check => "check",
            DraftKind::CashExpense => "cash_expense",
            DraftKind::Unknown => "unknown",
        }
    }
}

/// Where a draft stands (`document_drafts.status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftStatus {
    /// Claimed, not read yet.
    New,
    /// Read, waiting for the owner.
    Ready,
    /// The reading failed (`failure_reason`).
    Failed,
    /// Posted (`converted_transaction_id`).
    Converted,
    /// Thrown away; the file's claim is free again.
    Discarded,
}

impl DraftStatus {
    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DraftStatus::New => "new",
            DraftStatus::Ready => "ready",
            DraftStatus::Failed => "failed",
            DraftStatus::Converted => "converted",
            DraftStatus::Discarded => "discarded",
        }
    }

    /// A live draft holds its file (it can't be deleted) and can still
    /// change.
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(
            self,
            DraftStatus::New | DraftStatus::Ready | DraftStatus::Failed
        )
    }
}

/// Which reader made the stored reading (`document_drafts.reader`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftReader {
    /// The PDF's own text layer.
    TextPdf,
    /// OCR on this computer.
    Ocr,
    /// A vendor layout template.
    Template,
    /// A local Ollama model.
    Ollama,
    /// An online AI service.
    Cloud,
    /// Typed by the owner.
    Manual,
}

/// A document draft as the engine returns it. `fields`, `lines` and
/// `checks` are absent in the list's summary view.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDraft {
    /// Id.
    pub id: DocumentDraftId,
    /// The file it reads (`None` once the file was deleted).
    #[serde(default)]
    pub attachment_id: Option<AttachmentId>,
    /// SHA-256 of the file's bytes (lowercase hex): the claim.
    #[serde(default)]
    pub content_sha256: String,
    /// What it is (a reader's guess until converted).
    pub kind: DraftKind,
    /// Where it stands.
    pub status: DraftStatus,
    /// The reader of the stored reading.
    #[serde(default)]
    pub reader: Option<DraftReader>,
    /// The reading's header fields (desktop JSON).
    #[serde(default)]
    pub fields: Option<Value>,
    /// The reading's lines (desktop JSON).
    #[serde(default)]
    pub lines: Option<Value>,
    /// The reading's checks (desktop JSON).
    #[serde(default)]
    pub checks: Option<Value>,
    /// Why the reading failed (`[a-z0-9_]{1,40}`).
    #[serde(default)]
    pub failure_reason: Option<String>,
    /// The vendor the desktop matched.
    #[serde(default)]
    pub suggested_contact_id: Option<ContactId>,
    /// The document's reference as read (duplicate check).
    #[serde(default)]
    pub doc_reference: Option<String>,
    /// The document's total as read (duplicate check).
    #[serde(default)]
    pub doc_total: Option<Money>,
    /// The document's date as read (duplicate check).
    #[serde(default)]
    pub doc_date: Option<CalendarDate>,
    /// The record it was posted as.
    #[serde(default)]
    pub converted_transaction_id: Option<TransactionId>,
    /// That record's engine type (`bill`, `expense`).
    #[serde(default)]
    pub converted_type: Option<String>,
    /// When the document arrived (the claim).
    #[serde(default)]
    pub created_at: Option<cl_core::Timestamp>,
    /// The draft's own change token: pass it back unchanged as
    /// `expected_updated_at`. Opaque, never parsed or reformatted.
    pub updated_at: String,
}

impl fmt::Debug for DocumentDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocumentDraft")
            .field("id", &self.id)
            .field("attachment_id", &self.attachment_id)
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("reader", &self.reader)
            .field("has_reading", &self.fields.is_some())
            .field("failure_reason", &self.failure_reason)
            .field("suggested_contact_id", &self.suggested_contact_id)
            .field("converted_transaction_id", &self.converted_transaction_id)
            .finish_non_exhaustive()
    }
}

/// One page of drafts.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DraftPage {
    /// Newest first.
    pub drafts: Vec<DocumentDraft>,
    /// All matching drafts.
    pub total: u64,
}

/// Filters of the draft list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftListQuery {
    /// Any of these (empty: every status).
    pub statuses: Vec<DraftStatus>,
    /// Only this kind.
    pub kind: Option<DraftKind>,
    /// Page size, 1 to [`limits::MAX_PAGE`].
    pub limit: u32,
    /// Rows skipped, at most [`limits::MAX_OFFSET`].
    pub offset: u32,
    /// Leave each draft's reading out (`view=summary`).
    pub summary: bool,
}

impl Default for DraftListQuery {
    fn default() -> Self {
        DraftListQuery {
            statuses: Vec::new(),
            kind: None,
            limit: 50,
            offset: 0,
            summary: false,
        }
    }
}

impl DraftListQuery {
    fn pairs(&self) -> Result<Vec<(String, String)>, ApiError> {
        if self.limit == 0 || self.limit > limits::MAX_PAGE || self.offset > limits::MAX_OFFSET {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut q = Vec::new();
        if !self.statuses.is_empty() {
            let s: Vec<&str> = self.statuses.iter().map(|s| s.as_str()).collect();
            q.push(("status".to_string(), s.join(",")));
        }
        if let Some(k) = self.kind {
            q.push(("kind".to_string(), k.as_str().to_string()));
        }
        q.push(("limit".to_string(), self.limit.to_string()));
        q.push(("offset".to_string(), self.offset.to_string()));
        if self.summary {
            q.push(("view".to_string(), "summary".to_string()));
        }
        Ok(q)
    }
}

/// Where a possible duplicate is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchSource {
    /// A posted bill, expense or check (acknowledge it to convert anyway).
    Transaction(TransactionId),
    /// Another live draft (shown, never blocks a convert).
    Draft(DocumentDraftId),
}

/// Why it looks like the same document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchReason {
    /// Same vendor and the same normalized reference.
    Reference,
    /// Same vendor and total within 60 days.
    AmountDate,
    /// A reason this version does not know.
    Other,
}

/// A record or draft that may be the same document (U-92b).
#[derive(Clone, PartialEq, Eq)]
pub struct DuplicateMatch {
    /// Where it is.
    pub source: MatchSource,
    /// Why it matched.
    pub reason: MatchReason,
    /// A posted record's engine type (`bill`, `expense`).
    pub txn_type: Option<String>,
    /// A draft's kind.
    pub kind: Option<DraftKind>,
    /// Its date.
    pub date: Option<CalendarDate>,
    /// Its total.
    pub total: Option<Money>,
    /// Its reference as stored (owner data).
    pub reference: Option<String>,
}

impl fmt::Debug for DuplicateMatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DuplicateMatch")
            .field("source", &self.source)
            .field("reason", &self.reason)
            .field("txn_type", &self.txn_type)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatchWire {
    source: String,
    id: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    txn_type: Option<String>,
    #[serde(default)]
    kind: Option<DraftKind>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    reference: Option<String>,
}

impl DuplicateMatch {
    fn from_wire(w: MatchWire) -> Option<DuplicateMatch> {
        let source = match w.source.as_str() {
            "transaction" => MatchSource::Transaction(TransactionId::parse(&w.id).ok()?),
            "draft" => MatchSource::Draft(DocumentDraftId::parse(&w.id).ok()?),
            _ => return None,
        };
        let reason = match w.reason.as_deref() {
            Some("reference") => MatchReason::Reference,
            Some("amount_date") => MatchReason::AmountDate,
            _ => MatchReason::Other,
        };
        Some(DuplicateMatch {
            source,
            reason,
            txn_type: w.txn_type,
            kind: w.kind,
            // A plain `YYYY-MM-DD` only; anything else is left out.
            date: w.date.and_then(|d| CalendarDate::parse_wire(&d).ok()),
            total: w.total.and_then(|t| Money::parse_wire(&t).ok()),
            reference: w.reference,
        })
    }
}

/// The matches in a JSON array; entries this version can't read are left
/// out.
pub(crate) fn matches_from(v: Option<&Value>) -> Vec<DuplicateMatch> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| serde_json::from_value::<MatchWire>(m.clone()).ok())
                .filter_map(DuplicateMatch::from_wire)
                .collect()
        })
        .unwrap_or_default()
}

/// Why the engine refused a document draft call (beyond the ordinary
/// errors). Carried by [`ApiError::DocumentDraft`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftRefusal {
    /// 409 `DOCUMENT_ALREADY_CLAIMED`: the company already has this file
    /// (the same bytes) as a draft that is not discarded. `draft` is it.
    AlreadyClaimed {
        /// The draft that holds the file, when the engine said.
        draft: Option<DocumentDraftId>,
    },
    /// 409 `DOCUMENT_DRAFT_CLOSED`: the draft was converted (into
    /// `transaction`) or discarded; it can't change any more.
    Closed {
        /// The record it was posted as.
        transaction: Option<TransactionId>,
    },
    /// 409 `POSSIBLE_DUPLICATE`: posted records that look like the same
    /// document. Nothing was posted; convert again acknowledging them.
    PossibleDuplicate {
        /// Posted bills, expenses and checks (these block).
        matches: Vec<DuplicateMatch>,
        /// Other live drafts (shown, never block).
        draft_matches: Vec<DuplicateMatch>,
    },
    /// 422 `ATTACHMENT_NOT_DRAFT`: the file is already filed on a record.
    NotADraftFile,
    /// 409 `TEMPLATE_LIMIT`: the company has 500 vendor layouts.
    TemplateLimit,
}

/// Engine codes of [`DraftRefusal`].
pub const DOCUMENT_ALREADY_CLAIMED: &str = "DOCUMENT_ALREADY_CLAIMED";
/// See [`DraftRefusal::Closed`].
pub const DOCUMENT_DRAFT_CLOSED: &str = "DOCUMENT_DRAFT_CLOSED";
/// See [`DraftRefusal::PossibleDuplicate`].
pub const POSSIBLE_DUPLICATE: &str = "POSSIBLE_DUPLICATE";
/// See [`DraftRefusal::NotADraftFile`].
pub const ATTACHMENT_NOT_DRAFT: &str = "ATTACHMENT_NOT_DRAFT";
/// See [`DraftRefusal::TemplateLimit`].
pub const TEMPLATE_LIMIT: &str = "TEMPLATE_LIMIT";
/// 409 on `DELETE /attachments/{id}` of a file a live draft reads
/// (`Conflict { InUse }`, `details.draftId`): discard the draft first.
pub const ATTACHMENT_IN_USE: &str = "ATTACHMENT_IN_USE";

/// 422 on attach (U-95): the record is a type files can't be filed on, or
/// an adjusting entry the user can't reach (`Validation`, this code).
pub const TARGET_NOT_ATTACHABLE: &str = "TARGET_NOT_ATTACHABLE";
/// 409 on attach (U-95): the record is void or a draft
/// (`Conflict { Unspecified }`, this code, `details.status`).
pub const TRANSACTION_NOT_POSTED: &str = "TRANSACTION_NOT_POSTED";
/// 422 on attach (U-95): the draft's file was deleted (`Validation`, this
/// code).
pub const DOCUMENT_FILE_MISSING: &str = "DOCUMENT_FILE_MISSING";

/// Why the engine refused an attach (U-95), read from the [`ApiError`] the
/// call answered. Every U-95 refusal maps onto the ordinary errors (so the
/// demo engine, which answers through [`crate::engine_refusal`], gives the
/// same ones); this names them for the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachProblem {
    /// [`DraftRefusal::Closed`]: converted, attached or discarded already
    /// (`transaction` is the record it went to).
    Closed {
        /// The record it was posted as or attached to.
        transaction: Option<TransactionId>,
    },
    /// `Conflict { Stale }`: the draft changed since it was read.
    Stale,
    /// `NotFound`: the draft or the record is gone, or is another
    /// company's.
    NotFound,
    /// 422 [`TARGET_NOT_ATTACHABLE`].
    NotAttachable,
    /// 409 [`TRANSACTION_NOT_POSTED`]: void (or a draft record).
    NotPosted,
    /// 422 [`DOCUMENT_FILE_MISSING`]: the draft's file was deleted.
    FileMissing,
    /// [`DraftRefusal::NotADraftFile`]: the file is filed on another record.
    FileOnAnotherRecord,
    /// 409 [`ATTACHMENT_IN_USE`] (`Conflict { InUse }`): another live draft
    /// reads the file.
    FileInUse,
    /// 403: the user may not change the draft's kind or the record.
    Forbidden,
}

/// The [`AttachProblem`] an attach error stands for; `None` for errors
/// that are not refusals of the attach itself (network, session, a busy
/// key, an unknown outcome).
#[must_use]
pub fn attach_problem(e: &ApiError) -> Option<AttachProblem> {
    use crate::errors::ConflictReason;
    let code =
        |c: Option<&crate::errors::ErrorCode>, want: &str| c.is_some_and(|c| c.as_str() == want);
    Some(match e {
        ApiError::DocumentDraft(DraftRefusal::Closed { transaction }) => AttachProblem::Closed {
            transaction: *transaction,
        },
        ApiError::DocumentDraft(DraftRefusal::NotADraftFile) => AttachProblem::FileOnAnotherRecord,
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => AttachProblem::Stale,
        ApiError::Conflict {
            reason: ConflictReason::InUse,
            ..
        } => AttachProblem::FileInUse,
        ApiError::Conflict { code: c, .. } if code(c.as_ref(), TRANSACTION_NOT_POSTED) => {
            AttachProblem::NotPosted
        }
        ApiError::Validation(v) if code(v.code(), TARGET_NOT_ATTACHABLE) => {
            AttachProblem::NotAttachable
        }
        ApiError::Validation(v) if code(v.code(), DOCUMENT_FILE_MISSING) => {
            AttachProblem::FileMissing
        }
        ApiError::NotFound { .. } => AttachProblem::NotFound,
        ApiError::Forbidden { .. } => AttachProblem::Forbidden,
        _ => return None,
    })
}

/// The refusal an engine error body names, if any (`code`, `details`).
pub(crate) fn refusal(status: u16, code: &str, details: Option<&Value>) -> Option<DraftRefusal> {
    let id = |k: &str| details.and_then(|d| d.get(k)).and_then(Value::as_str);
    match (status, code) {
        (409, DOCUMENT_ALREADY_CLAIMED) => Some(DraftRefusal::AlreadyClaimed {
            draft: id("draftId").and_then(|s| DocumentDraftId::parse(s).ok()),
        }),
        (409, DOCUMENT_DRAFT_CLOSED) => Some(DraftRefusal::Closed {
            transaction: id("transactionId").and_then(|s| TransactionId::parse(s).ok()),
        }),
        (409, POSSIBLE_DUPLICATE) => Some(DraftRefusal::PossibleDuplicate {
            matches: matches_from(details.and_then(|d| d.get("matches"))),
            draft_matches: matches_from(details.and_then(|d| d.get("draftMatches"))),
        }),
        (422, ATTACHMENT_NOT_DRAFT) => Some(DraftRefusal::NotADraftFile),
        (409, TEMPLATE_LIMIT) => Some(DraftRefusal::TemplateLimit),
        _ => None,
    }
}

/// A reading to store (`PUT /document-drafts/{id}`, status `ready`). The
/// whole reading is replaced: every typed field is written, `None` as
/// `null` (cleared).
#[derive(Clone, PartialEq, Eq)]
pub struct DraftReading {
    /// What the document is.
    pub kind: DraftKind,
    /// Which reader read it.
    pub reader: DraftReader,
    /// Header fields (a JSON object; decimal-string amounts, integer
    /// numbers only).
    pub fields: Map<String, Value>,
    /// Lines (at most [`limits::MAX_LINES`]).
    pub lines: Vec<Value>,
    /// Checks for the owner (at most [`limits::MAX_CHECKS`]).
    pub checks: Vec<Value>,
    /// The vendor the desktop matched.
    pub suggested_contact: Option<ContactId>,
    /// The reference (trimmed; longer than 100 characters is left out).
    pub doc_reference: Option<String>,
    /// The total (a negative total, such as a credit, is left out: the
    /// duplicate check compares totals of zero or more).
    pub doc_total: Option<Money>,
    /// The document date.
    pub doc_date: Option<CalendarDate>,
}

impl fmt::Debug for DraftReading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftReading")
            .field("kind", &self.kind)
            .field("reader", &self.reader)
            .field("fields", &self.fields.len())
            .field("lines", &self.lines.len())
            .field("checks", &self.checks.len())
            .field("suggested_contact", &self.suggested_contact)
            .finish_non_exhaustive()
    }
}

/// A JSON value the engine stores: integers within ±10⁹, strings and keys
/// within bounds, nested at most [`limits::MAX_DEPTH`] deep. A fractional
/// number is refused, so no amount is ever stored as a float.
fn json_ok(v: &Value, depth: u32) -> bool {
    match v {
        Value::Null | Value::Bool(_) => true,
        Value::String(s) => s.encode_utf16().count() <= limits::MAX_STRING,
        Value::Number(n) => {
            n.as_u64().is_some_and(|u| u <= limits::MAX_INTEGER)
                || n.as_i64()
                    .is_some_and(|i| i.unsigned_abs() <= limits::MAX_INTEGER)
        }
        Value::Array(a) => depth < limits::MAX_DEPTH && a.iter().all(|x| json_ok(x, depth + 1)),
        Value::Object(m) => {
            depth < limits::MAX_DEPTH
                && m.iter().all(|(k, x)| {
                    k.encode_utf16().count() <= limits::MAX_KEY && json_ok(x, depth + 1)
                })
        }
    }
}

/// `v` passes [`json_ok`] and is at most `max_bytes` as JSON.
pub(crate) fn bounded(v: &Value, max_bytes: usize) -> bool {
    json_ok(v, 0) && serde_json::to_vec(v).is_ok_and(|b| b.len() <= max_bytes)
}

fn clean_reference(r: Option<&str>) -> Option<String> {
    r.map(str::trim)
        .filter(|r| !r.is_empty() && r.chars().count() <= limits::MAX_REFERENCE)
        .map(str::to_string)
}

impl DraftReading {
    fn body(&self, expected_updated_at: &str) -> Result<Value, ApiError> {
        let fields = Value::Object(self.fields.clone());
        let lines = Value::Array(self.lines.clone());
        let checks = Value::Array(self.checks.clone());
        if self.lines.len() > limits::MAX_LINES
            || self.checks.len() > limits::MAX_CHECKS
            || !bounded(&fields, limits::FIELDS_BYTES)
            || !bounded(&lines, limits::LINES_BYTES)
            || !bounded(&checks, limits::CHECKS_BYTES)
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(json!({
            "expectedUpdatedAt": expected_updated_at,
            "status": "ready",
            "kind": self.kind,
            "reader": self.reader,
            "fields": fields,
            "lines": lines,
            "checks": checks,
            "failureReason": Value::Null,
            "suggestedContactId": self.suggested_contact,
            "docReference": clean_reference(self.doc_reference.as_deref()),
            "docTotal": self.doc_total.filter(|t| !t.is_negative()).map(Money::to_wire),
            "docDate": self.doc_date,
        }))
    }
}

/// A failure reason the engine stores: `[a-z0-9_]{1,40}`.
fn reason_ok(r: &str) -> bool {
    (1..=40).contains(&r.len())
        && r.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// What a draft is posted as, built with the same typed builders as manual
/// entry.
#[derive(Clone, Debug)]
pub enum ConvertBody {
    /// A bill (`POST /bills`' body).
    Bill(BillDraft),
    /// A card charge: a [`BankPosting::card_charge`] paid from the card.
    CardCharge(BankPosting),
    /// A cash expense: a [`BankPosting::card_charge`] paid from cash or a
    /// bank account.
    CashExpense(BankPosting),
    /// A check already written: a [`BankPosting::check`].
    Check(BankPosting),
}

impl ConvertBody {
    /// The kind it posts as.
    #[must_use]
    pub const fn kind(&self) -> DraftKind {
        match self {
            ConvertBody::Bill(_) => DraftKind::Bill,
            ConvertBody::CardCharge(_) => DraftKind::CardCharge,
            ConvertBody::CashExpense(_) => DraftKind::CashExpense,
            ConvertBody::Check(_) => DraftKind::Check,
        }
    }

    /// The `body` member [`DocumentDrafts::convert`] sends for this posting
    /// under `caps` (the same function builds both): the desktop digests it
    /// to prove the posting carries what the owner reviewed (A04 AC1).
    /// Owner data: never log it.
    ///
    /// # Errors
    /// As [`DocumentDrafts::convert`]'s local checks (nothing is sent then).
    pub fn wire_body(&self, caps: &Capabilities) -> Result<Value, ApiError> {
        self.value(caps)
    }

    /// The kind's create body, without `draftAttachmentId` (convert moves
    /// the draft's own file) and the expense's `txnType`.
    fn value(&self, caps: &Capabilities) -> Result<Value, ApiError> {
        let mut v = match self {
            ConvertBody::Bill(d) => {
                let parts = d.parts(false);
                crate::resources::ap_doc::check_doc(&parts, caps)?;
                crate::resources::ap_doc::doc_body(&parts, caps, false)
            }
            ConvertBody::CardCharge(p) | ConvertBody::CashExpense(p) => {
                if p.kind() != BankPostingKind::CardCharge {
                    return Err(ApiError::Local(LocalValidation::InvalidValue));
                }
                p.create_value()
            }
            ConvertBody::Check(p) => {
                if p.kind() != BankPostingKind::Check {
                    return Err(ApiError::Local(LocalValidation::InvalidValue));
                }
                p.create_value()
            }
        };
        if let Value::Object(m) = &mut v {
            m.remove("draftAttachmentId");
            m.remove("txnType");
        }
        Ok(v)
    }
}

/// A convert request.
#[derive(Clone, Debug)]
pub struct DraftConversion {
    /// What to post.
    pub body: ConvertBody,
    /// The draft's `updated_at` as last read.
    pub expected_updated_at: String,
    /// Posted records the owner saw and wants to enter this anyway (at
    /// most [`limits::MAX_ACKNOWLEDGED`]).
    pub acknowledge: Vec<TransactionId>,
}

/// A converted draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvertedDraft {
    /// The draft, now `Converted`.
    pub draft: DocumentDraft,
    /// The record it was posted as.
    pub transaction: TransactionId,
    /// Other live drafts that look like the same document (shown, never
    /// blocking).
    pub draft_matches: Vec<DuplicateMatch>,
}

/// Document draft operations of a session (`session.document_drafts()`).
#[derive(Clone, Copy, Debug)]
pub struct DocumentDrafts<'a>(pub(crate) &'a Session);

impl Session {
    /// Document drafts of the open company (`document-drafts-v1`).
    #[must_use]
    pub fn document_drafts(&self) -> DocumentDrafts<'_> {
        DocumentDrafts(self)
    }
}

/// `Ok` with `document-drafts-v1`, else `Local(Unsupported)`.
pub(crate) fn supported(s: &Session) -> Result<(), ApiError> {
    if s.client().capabilities().has(Capabilities::DOCUMENT_DRAFTS) {
        Ok(())
    } else {
        Err(ApiError::Local(LocalValidation::Unsupported))
    }
}

fn at(op: &'static Operation, id: DocumentDraftId) -> Call {
    Call::new(
        op.method(),
        op.route(),
        op.route().replace("{id}", &id.to_string()),
    )
}

#[derive(Deserialize)]
struct One {
    draft: DocumentDraft,
}

impl DocumentDrafts<'_> {
    /// Claims an uploaded draft file (`AttachTo::Draft`) as a document
    /// draft (status `new`). Keyed under `idempotency-docs-v1`: a retry
    /// with the same key gets the same draft.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-drafts-v1` (nothing sent);
    /// [`DraftRefusal::AlreadyClaimed`] when the company already has these
    /// bytes as a draft that is not discarded; [`DraftRefusal::NotADraftFile`]
    /// for a file filed on a record; `NotFound` for another company's file;
    /// `Forbidden`; any [`ApiError`].
    pub async fn create(
        &self,
        attachment: AttachmentId,
        kind_hint: DraftKind,
        key: IdempotencyKey,
    ) -> Result<DocumentDraft, ApiError> {
        supported(self.0)?;
        let body = json_body(&json!({
            "attachmentId": attachment,
            "kindHint": kind_hint,
        }))?;
        let mut call = Call::at(Method::Post, DOCUMENT_DRAFTS_CREATE.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DOCUMENT_DRAFTS_CREATE, call, None).await?;
        decode::<One>(&reply, "document draft")
            .map(|o| o.draft)
            .map_err(saved_unreadable)
    }

    /// One page of drafts, newest first.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a page size outside
    /// 1 to 200 or an offset over 10,000 (nothing sent); any [`ApiError`].
    pub async fn list(&self, query: &DraftListQuery) -> Result<DraftPage, ApiError> {
        supported(self.0)?;
        let mut call = Call::at(Method::Get, DOCUMENT_DRAFTS_LIST.route());
        call.query = query.pairs()?;
        let reply = self.0.send(&DOCUMENT_DRAFTS_LIST, call, None).await?;
        decode(&reply, "document drafts")
    }

    /// One draft with its reading.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound` (also another company's); any
    /// [`ApiError`].
    pub async fn get(&self, id: DocumentDraftId) -> Result<DocumentDraft, ApiError> {
        supported(self.0)?;
        let reply = self
            .0
            .send(&DOCUMENT_DRAFT_GET, at(&DOCUMENT_DRAFT_GET, id), None)
            .await?;
        decode::<One>(&reply, "document draft").map(|o| o.draft)
    }

    /// Posted records and other live drafts of the draft's vendor that look
    /// like the same document (same normalized reference, or the same total
    /// within 60 days). Empty without a vendor.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound`; any [`ApiError`].
    pub async fn duplicates(&self, id: DocumentDraftId) -> Result<Vec<DuplicateMatch>, ApiError> {
        supported(self.0)?;
        let reply = self
            .0
            .send(
                &DOCUMENT_DRAFT_DUPLICATES,
                at(&DOCUMENT_DRAFT_DUPLICATES, id),
                None,
            )
            .await?;
        let v: Value = decode(&reply, "duplicates")?;
        Ok(matches_from(v.get("matches")))
    }

    async fn put(&self, id: DocumentDraftId, body: &Value) -> Result<DocumentDraft, ApiError> {
        let call = at(&DOCUMENT_DRAFT_PUT, id).json_body(json_body(body)?);
        let reply = self.0.send(&DOCUMENT_DRAFT_PUT, call, None).await?;
        decode::<One>(&reply, "document draft")
            .map(|o| o.draft)
            .map_err(saved_unreadable)
    }

    /// Stores a reading (status `ready`), replacing the stored one.
    /// `expected_updated_at` is the draft's `updated_at` as last read.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` when the reading is over
    /// the engine's bounds or holds a fractional JSON number (nothing
    /// sent); `Conflict { Stale }`; [`DraftRefusal::Closed`]; `NotFound`
    /// (also a vendor that is not the company's); any [`ApiError`].
    pub async fn store_reading(
        &self,
        id: DocumentDraftId,
        expected_updated_at: &str,
        reading: &DraftReading,
    ) -> Result<DocumentDraft, ApiError> {
        supported(self.0)?;
        let body = reading.body(expected_updated_at)?;
        self.put(id, &body).await
    }

    /// Records that the document could not be read (status `failed`).
    /// `reason` is a short class such as `unreadable` (`[a-z0-9_]{1,40}`).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for another reason form (nothing sent);
    /// otherwise as [`DocumentDrafts::store_reading`].
    pub async fn store_failure(
        &self,
        id: DocumentDraftId,
        expected_updated_at: &str,
        reason: &str,
    ) -> Result<DocumentDraft, ApiError> {
        supported(self.0)?;
        if !reason_ok(reason) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = json!({
            "expectedUpdatedAt": expected_updated_at,
            "status": "failed",
            "failureReason": reason,
        });
        self.put(id, &body).await
    }

    /// Discards a draft: its file's claim is free again (the file itself
    /// stays; delete it afterwards). Already discarded: nothing changes.
    ///
    /// # Errors
    /// `Local(Unsupported)`; [`DraftRefusal::Closed`] for a converted
    /// draft; `NotFound`; any [`ApiError`].
    pub async fn discard(&self, id: DocumentDraftId) -> Result<DocumentDraft, ApiError> {
        supported(self.0)?;
        let call = at(&DOCUMENT_DRAFT_DISCARD, id).json_body(b"{}".to_vec());
        let reply = self.0.send(&DOCUMENT_DRAFT_DISCARD, call, None).await?;
        decode::<One>(&reply, "document draft")
            .map(|o| o.draft)
            .map_err(saved_unreadable)
    }

    /// Posts a draft in one database transaction: the record through its
    /// kind's own service, the draft's file moved onto it, the draft marked
    /// converted. Keyed under `idempotency-docs-v1` (a retry with the same
    /// key after it committed gets the same record). The closing-date
    /// password goes at the top level (`opts.closed_period`).
    ///
    /// # Errors
    /// Nothing is sent for `Local(…)`: `Unsupported` (no
    /// `document-drafts-v1`, or a password without `closing-password-v1`),
    /// `InvalidValue` (more than 50 acknowledged, a card charge or check
    /// posting of the wrong kind), and the bill's own checks (see
    /// [`crate::Bills::create`]). From the engine:
    /// [`DraftRefusal::PossibleDuplicate`] (nothing posted),
    /// [`DraftRefusal::Closed`], `Conflict { Stale }`, `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `Validation` (paths like
    /// `body.lines.0.amount`), `NotFound`, `Forbidden`, any [`ApiError`]. A
    /// 2xx whose answer can't be read is `SavedUnreadable` (it posted).
    pub async fn convert(
        &self,
        id: DocumentDraftId,
        conversion: &DraftConversion,
        opts: &PostingOptions,
        key: IdempotencyKey,
    ) -> Result<ConvertedDraft, ApiError> {
        supported(self.0)?;
        if conversion.acknowledge.len() > limits::MAX_ACKNOWLEDGED {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let caps = self.0.client().capabilities();
        let body = json!({
            "kind": conversion.body.kind(),
            "body": conversion.body.value(&caps)?,
            "expectedUpdatedAt": conversion.expected_updated_at,
            "acknowledgeDuplicateOf": conversion.acknowledge,
        });
        // The password goes at the top level only: the engine drops one
        // inside `body`.
        let bytes = posting_body(self.0, body, opts)?;
        let mut call = at(&DOCUMENT_DRAFT_CONVERT, id).json_body(bytes);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DOCUMENT_DRAFT_CONVERT, call, None).await?;
        converted_from(&reply.body).map_err(saved_unreadable)
    }

    /// Files the draft's document on a record that is already posted
    /// instead of posting a new one (U-95, Q-282): in one database
    /// transaction the draft's file moves onto `transaction` and the draft
    /// is closed as `Converted` into it (its kind unchanged). Nothing posts,
    /// so no closing-date password is asked for. There is no undo (Q-A04e).
    /// Keyed under `document-drafts-attach-v1`: a retry with the same key
    /// after it committed gets the same answer.
    ///
    /// # Errors
    /// Nothing is sent for `Local(Unsupported)` (no `document-drafts-v1` or
    /// no `document-drafts-attach-v1`: an older engine has no attach, and a
    /// plain link would let the draft post the same document again). From
    /// the engine, read with [`attach_problem`]:
    /// [`DraftRefusal::Closed`], `Conflict { Stale }`, `NotFound` (the
    /// draft or the record is gone or another company's), `Validation`
    /// [`TARGET_NOT_ATTACHABLE`] or [`DOCUMENT_FILE_MISSING`], `Conflict`
    /// [`TRANSACTION_NOT_POSTED`], [`DraftRefusal::NotADraftFile`],
    /// `Forbidden`; and any [`ApiError`] (`OutcomeUnknown { keyed }` after a
    /// lost reply: retry with the same key). A 2xx whose answer can't be
    /// read is `SavedUnreadable` (it was attached).
    pub async fn attach(
        &self,
        id: DocumentDraftId,
        transaction: TransactionId,
        expected_updated_at: &str,
        key: IdempotencyKey,
    ) -> Result<AttachedDraft, ApiError> {
        attach_supported(self.0)?;
        let body = json_body(&json!({
            "transactionId": transaction,
            "expectedUpdatedAt": expected_updated_at,
        }))?;
        let mut call = at(&DOCUMENT_DRAFT_ATTACH, id).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&DOCUMENT_DRAFT_ATTACH, call, None).await?;
        attached_from(&reply.body).map_err(saved_unreadable)
    }
}

/// An attached draft (`{draft, transaction}`, U-95).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedDraft {
    /// The draft, now `Converted` into the record (`converted_type` is the
    /// record's type: `aje` for an adjusting entry, whose files are stored
    /// as `journal_entry`).
    pub draft: DocumentDraft,
    /// The record that now has the document.
    pub transaction: TransactionId,
}

/// `Ok` with `document-drafts-v1` and `document-drafts-attach-v1`, else
/// `Local(Unsupported)`.
pub(crate) fn attach_supported(s: &Session) -> Result<(), ApiError> {
    supported(s)?;
    if s.client()
        .capabilities()
        .has(Capabilities::DOCUMENT_DRAFTS_ATTACH)
    {
        Ok(())
    } else {
        Err(ApiError::Local(LocalValidation::Unsupported))
    }
}

/// Reads an attach answer `{draft, transaction}`.
pub(crate) fn attached_from(body: &[u8]) -> Result<AttachedDraft, ApiError> {
    let c = converted_from(body).map_err(|_| ApiError::Decode {
        what: "attached draft",
    })?;
    Ok(AttachedDraft {
        draft: c.draft,
        transaction: c.transaction,
    })
}

/// Reads a convert answer `{draft, transaction, draftMatches}`.
pub(crate) fn converted_from(body: &[u8]) -> Result<ConvertedDraft, ApiError> {
    let decode_err = || ApiError::Decode {
        what: "converted draft",
    };
    let v: Value = serde_json::from_slice(body).map_err(|_| decode_err())?;
    let draft: DocumentDraft =
        serde_json::from_value(v.get("draft").cloned().unwrap_or(Value::Null))
            .map_err(|_| decode_err())?;
    let transaction = v
        .pointer("/transaction/id")
        .and_then(Value::as_str)
        .and_then(|s| TransactionId::parse(s).ok())
        .or(draft.converted_transaction_id)
        .ok_or_else(decode_err)?;
    Ok(ConvertedDraft {
        draft_matches: matches_from(v.get("draftMatches")),
        draft,
        transaction,
    })
}

#[cfg(test)]
#[path = "document_drafts_tests.rs"]
mod tests;
