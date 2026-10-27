//! The demo "engine" for document drafts (A04 step 6, WP-D): the engine's
//! `document-drafts-v1` (U-92, U-92b) and `document-drafts-attach-v1`
//! (U-95) rules, synthetic, `--demo` only. The parity test
//! (`drafts_tests.rs`) runs `fixtures/docs/draft-cases.json` against it;
//! the live engine runs the same cases (T16).
//!
//! What it plays, as `services/document-draft.service.ts` does:
//! - **The claim:** [`create`] hashes the stored file (SHA-256); a company
//!   holding the same bytes as a draft that is not discarded answers 409
//!   `DOCUMENT_ALREADY_CLAIMED` naming it; a file filed on a record is 422
//!   `ATTACHMENT_NOT_DRAFT`. Discarding frees the claim; converting keeps it.
//! - **Stale and closed:** every change names the `updatedAt` it last read
//!   (409 `STALE` otherwise); a converted or discarded draft answers 409
//!   `DOCUMENT_DRAFT_CLOSED` (with the record it became).
//! - **Convert** ([`convert`]): the closing date (423, then the password),
//!   the duplicate rule (posted bills and expenses of the vendor with the
//!   same normalized reference, or the same total within 60 days, block
//!   until acknowledged: 409 `POSSIBLE_DUPLICATE`; other live drafts are
//!   listed, never blocking), then the posting through the demo's own
//!   services and the guarded relink of the draft's file. The demo can't
//!   roll a posting back, so it checks the duplicates on the values it is
//!   about to post; the answer is the engine's.
//! - **Attach** ([`attach`], U-95): the draft's file moves onto a posted
//!   record (404 gone, 409 `TRANSACTION_NOT_POSTED` void, 422
//!   `TARGET_NOT_ATTACHABLE`, `DOCUMENT_FILE_MISSING`,
//!   `ATTACHMENT_NOT_DRAFT`); nothing posts and no password is needed.
//! - **The guard** (U-95 b): plain relinks (`checks::link_draft`) leave the
//!   files live drafts read on their key, and `checks::link_attachment`
//!   refuses them (409 `ATTACHMENT_IN_USE`).
//! - **Keys:** create, convert and attach are keyed. The same key with the
//!   same request replays the first answer; with another request it is 422
//!   `IDEMPOTENCY_KEY_REUSED`. Refusals are never stored, so a refused
//!   convert sent again with the closing-date password (not part of the
//!   request's identity) under the same key posts once.
//!
//! Refusals are built as the engine's JSON error bodies and read through
//! [`cl_api::engine_refusal`], so the desktop sees exactly the
//! [`ApiError`]s a live engine gives. Every operation is a function on
//! [`DemoState`] (the files, ledger, bills and contacts live there);
//! [`DemoDrafts`] is the store. Not played: permissions (the demo has one
//! user) and the engine's row locks (the demo is single-threaded).
//!
//! Money is exact (`Money`, decimal strings on the wire); nothing here
//! logs, and `Debug` shows counts only.

use std::collections::BTreeSet;
use std::fmt;

use cl_api::{
    ApiError, AttachTo, AttachmentId, BankPosting, BillDraft, CardChargeDraft, CheckDraft,
    ConvertedDraft, DocumentDraft, DocumentDraftId, DraftKind, DraftListQuery, DraftPage,
    DraftReading, DraftStatus, DuplicateMatch, IdempotencyKey, LocalValidation, MatchReason,
    MatchSource, TransactionSummary, TxnStatus, TxnType, engine_refusal,
};
use cl_core::ids::{AccountId, ContactId, TenantId, TransactionId};
use cl_core::{CalendarDate, Money, Secret, Timestamp};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::DemoState;
use super::register::{DemoLedger, DemoPosting, check_closing};

/// The window of the vendor + amount duplicate rule, in days either side
/// (the engine's `DUPLICATE_WINDOW_DAYS`).
pub const DUPLICATE_WINDOW_DAYS: i64 = 60;

/// At most this many matches of each kind (the engine's `LIMIT 20`).
const MAX_MATCHES: usize = 20;

/// The engine's `TXN_ATTACHABLE_TYPES`.
const ATTACHABLE: [&str; 14] = [
    "invoice",
    "customer_payment",
    "cash_sale",
    "expense",
    "deposit",
    "transfer",
    "journal_entry",
    "credit_memo",
    "customer_refund",
    "bill",
    "vendor_credit",
    "bill_payment",
    "daily_sales",
    "aje",
];

/// One stored draft.
#[derive(Clone)]
struct Row {
    tenant: TenantId,
    /// Creation order (the list is newest first).
    seq: u64,
    draft: DocumentDraft,
}

/// What a keyed request was, for the replay rule (the closing-date
/// password is not part of it, as the engine's request hash drops it).
#[derive(Clone, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "a handful of keyed requests kept in memory"
)]
enum Request {
    Create {
        tenant: TenantId,
        attachment: AttachmentId,
        hint: DraftKind,
    },
    Convert {
        tenant: TenantId,
        id: DocumentDraftId,
        conversion: DemoConversion,
    },
    Attach {
        tenant: TenantId,
        id: DocumentDraftId,
        transaction: TransactionId,
        expected: String,
    },
}

/// The first answer of a keyed request.
#[derive(Clone)]
enum Reply {
    Draft(DocumentDraft),
    Converted(ConvertedDraft),
    Attached(AttachedDraft),
}

impl Reply {
    fn draft_id(&self) -> DocumentDraftId {
        match self {
            Reply::Draft(d) => d.id,
            Reply::Converted(c) => c.draft.id,
            Reply::Attached(a) => a.draft.id,
        }
    }
}

/// The demo engine's document drafts, every company's.
#[derive(Clone, Default)]
pub struct DemoDrafts {
    /// The demo engine advertises `document-drafts-v1` and
    /// `document-drafts-attach-v1` (on in `DemoState::new`; off by
    /// default, as an engine without them).
    pub enabled: bool,
    rows: Vec<Row>,
    /// Keyed requests that succeeded: key → (request, first answer).
    keys: Vec<(String, Request, Reply)>,
    seq: u64,
    /// Requests that reached the demo engine (replays included). Tests use
    /// it to prove that a refused call sent nothing.
    pub requests: u32,
    /// The next convert or attach commits, then its answer is lost
    /// (`OutcomeUnknown`, keyed): tests of resending under the same key.
    pub lose_next_reply: bool,
    /// The next keyed request answers 409 `IDEMPOTENCY_IN_PROGRESS`
    /// (nothing done).
    pub busy_next: bool,
    /// The next [`search_amount`] fails (`Timeout`, nothing read): tests
    /// that a failed register search never posts (review B2).
    pub fail_next_search: bool,
    /// A04 WP-F2: the vendors' saved layouts (the demo's
    /// `vendor_document_templates`), learned on accept and used by the
    /// inbox's reading. No amounts.
    pub layouts: Vec<(ContactId, cl_docread::model::Template)>,
    /// Records the converts posted (not the ones attach filed on), so the
    /// demo's inbox can void them when it lays its files out again: every
    /// scenario run starts with the books as found (A04 step 15).
    pub created: Vec<TransactionId>,
}

impl fmt::Debug for DemoDrafts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DemoDrafts")
            .field("enabled", &self.enabled)
            .field("drafts", &self.rows.len())
            .field("keys", &self.keys.len())
            .field("layouts", &self.layouts.len())
            .finish_non_exhaustive()
    }
}

impl DemoDrafts {
    /// An empty store of a demo engine with drafts and attach.
    #[must_use]
    pub fn on() -> DemoDrafts {
        DemoDrafts {
            enabled: true,
            ..DemoDrafts::default()
        }
    }

    /// Whether the demo engine has document drafts now.
    #[must_use]
    pub fn supported(&self) -> bool {
        self.enabled
    }

    /// The files live drafts read (other than `except`'s): the guard keeps
    /// them on their draft key.
    #[must_use]
    pub(crate) fn live_files(&self, except: Option<DocumentDraftId>) -> BTreeSet<AttachmentId> {
        self.rows
            .iter()
            .filter(|r| r.draft.status.is_live() && Some(r.draft.id) != except)
            .filter_map(|r| r.draft.attachment_id)
            .collect()
    }

    /// The live draft reading `file`, if any.
    #[must_use]
    pub(crate) fn reader_of(&self, file: AttachmentId) -> Option<DocumentDraftId> {
        self.rows
            .iter()
            .find(|r| r.draft.status.is_live() && r.draft.attachment_id == Some(file))
            .map(|r| r.draft.id)
    }

    fn find(&self, tenant: TenantId, id: DocumentDraftId) -> Option<&Row> {
        self.rows
            .iter()
            .find(|r| r.tenant == tenant && r.draft.id == id)
    }

    fn find_mut(&mut self, tenant: TenantId, id: DocumentDraftId) -> Option<&mut Row> {
        self.rows
            .iter_mut()
            .find(|r| r.tenant == tenant && r.draft.id == id)
    }

    /// The next change token: an ISO timestamp in milliseconds, always
    /// later than every earlier one (the engine's `NEXT_UPDATED_AT`).
    fn stamp(&mut self) -> String {
        self.seq += 1;
        let base = DemoState::now().unix_millis();
        let ms = base.saturating_add(i64::try_from(self.seq).unwrap_or(i64::MAX));
        Timestamp::from_unix_millis(ms)
            .unwrap_or_else(|_| DemoState::now())
            .to_wire()
    }

    fn next_id(&mut self) -> Result<DocumentDraftId, ApiError> {
        let n = self.rows.len() + 1;
        DocumentDraftId::parse(&format!("d0000000-0000-4000-8000-dd{n:010}"))
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))
    }
}

// ---------------------------------------------------------------- refusals

/// An engine refusal: its JSON error body, read as cl-api reads it.
#[allow(
    clippy::needless_pass_by_value,
    reason = "callers build the details in place"
)]
fn refusal(status: u16, code: &str, details: Value) -> ApiError {
    engine_refusal(
        status,
        &json!({"error": {"message": "", "code": code, "details": details}}),
    )
}

fn not_found() -> ApiError {
    refusal(404, "NOT_FOUND", Value::Null)
}

fn unsupported<T>() -> Result<T, ApiError> {
    Err(ApiError::Local(LocalValidation::Unsupported))
}

fn invalid() -> ApiError {
    ApiError::Local(LocalValidation::InvalidValue)
}

fn closed(d: &DocumentDraft) -> ApiError {
    refusal(
        409,
        cl_api::DOCUMENT_DRAFT_CLOSED,
        json!({"status": d.status.as_str(), "transactionId": d.converted_transaction_id}),
    )
}

fn stale(d: &DocumentDraft) -> ApiError {
    refusal(409, "STALE", json!({"updatedAt": d.updated_at}))
}

/// The draft as it stands for a change: live and not stale.
fn changeable(
    state: &DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
    expected: &str,
) -> Result<DocumentDraft, ApiError> {
    let row = state.drafts.find(tenant, id).ok_or_else(not_found)?;
    if !row.draft.status.is_live() {
        return Err(closed(&row.draft));
    }
    if row.draft.updated_at != expected {
        return Err(stale(&row.draft));
    }
    Ok(row.draft.clone())
}

// -------------------------------------------------------------------- keys

/// The first answer of `key` for the same request, or 422
/// `IDEMPOTENCY_KEY_REUSED` for another one; `None` for a new key.
fn replay(
    state: &DemoState,
    key: &IdempotencyKey,
    request: &Request,
) -> Result<Option<Reply>, ApiError> {
    let header = key.as_header();
    let Some((_, first, reply)) = state.drafts.keys.iter().find(|(k, _, _)| *k == header) else {
        return Ok(None);
    };
    if first == request {
        return Ok(Some(reply.clone()));
    }
    Err(refusal(
        422,
        "IDEMPOTENCY_KEY_REUSED",
        json!({"entityType": "document_draft", "entityId": reply.draft_id()}),
    ))
}

fn remember(state: &mut DemoState, key: &IdempotencyKey, request: Request, reply: Reply) {
    state.drafts.keys.push((key.as_header(), request, reply));
}

/// The demo's lost answer: it committed, the reply never arrived.
fn lost(key: &IdempotencyKey) -> ApiError {
    match cl_core::ids::RequestId::parse("d0000000-0000-4000-8000-f00000000002") {
        Ok(request_id) => ApiError::OutcomeUnknown {
            request_id,
            idempotency_key: Some(key.clone()),
            keyed: true,
        },
        Err(_) => ApiError::Timeout,
    }
}

fn busy(state: &mut DemoState) -> Result<(), ApiError> {
    if std::mem::take(&mut state.drafts.busy_next) {
        return Err(refusal(409, "IDEMPOTENCY_IN_PROGRESS", Value::Null));
    }
    Ok(())
}

// ------------------------------------------------------------------- files

/// The attachable type of a file filed on `to`: the engine stores an
/// adjusting entry's files as `journal_entry` (U-95 review M-1).
#[must_use]
pub fn file_type(to: AttachTo) -> &'static str {
    match to.attachable_type() {
        "aje" => "journal_entry",
        t => t,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

// ------------------------------------------------------------- read, claim

/// `POST /document-drafts`: claims an uploaded draft file as a document
/// draft (status `new`, kind `kind_hint`). Keyed.
///
/// # Errors
/// `Local(Unsupported)` without drafts; `NotFound` (no such file);
/// [`cl_api::DraftRefusal::NotADraftFile`] (filed on a record);
/// [`cl_api::DraftRefusal::AlreadyClaimed`] (the same bytes are already a
/// draft that is not discarded); `IdempotencyKeyReused`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the key is taken as cl_api::DocumentDrafts takes it"
)]
pub fn create(
    state: &mut DemoState,
    tenant: TenantId,
    attachment: AttachmentId,
    kind_hint: DraftKind,
    key: IdempotencyKey,
) -> Result<DocumentDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    state.drafts.requests += 1;
    let request = Request::Create {
        tenant,
        attachment,
        hint: kind_hint,
    };
    if let Some(Reply::Draft(d)) = replay(state, &key, &request)? {
        return Ok(d);
    }
    busy(state)?;
    let file = super::checks::find_attachment(state, attachment).ok_or_else(not_found)?;
    if file.attachable_type != "draft" {
        return Err(refusal(422, cl_api::ATTACHMENT_NOT_DRAFT, Value::Null));
    }
    let sha = sha256_hex(&super::checks::download(state, attachment)?);
    if let Some(holder) = state.drafts.rows.iter().find(|r| {
        r.tenant == tenant
            && r.draft.content_sha256 == sha
            && r.draft.status != DraftStatus::Discarded
    }) {
        return Err(refusal(
            409,
            cl_api::DOCUMENT_ALREADY_CLAIMED,
            json!({"draftId": holder.draft.id}),
        ));
    }
    let id = state.drafts.next_id()?;
    let updated_at = state.drafts.stamp();
    let draft = DocumentDraft {
        id,
        attachment_id: Some(attachment),
        content_sha256: sha,
        kind: kind_hint,
        status: DraftStatus::New,
        reader: None,
        fields: Some(Value::Object(Map::new())),
        lines: Some(Value::Array(Vec::new())),
        checks: Some(Value::Array(Vec::new())),
        failure_reason: None,
        suggested_contact_id: None,
        doc_reference: None,
        doc_total: None,
        doc_date: None,
        converted_transaction_id: None,
        converted_type: None,
        updated_at,
    };
    let seq = state.drafts.seq;
    state.drafts.rows.push(Row {
        tenant,
        seq,
        draft: draft.clone(),
    });
    remember(state, &key, request, Reply::Draft(draft.clone()));
    Ok(draft)
}

/// `GET /document-drafts`: one page, newest first.
///
/// # Errors
/// `Local(Unsupported)`; `Local(InvalidValue)` for a page size outside 1
/// to 200 or an offset over 10,000 (as cl-api refuses it).
pub fn list(
    state: &DemoState,
    tenant: TenantId,
    query: &DraftListQuery,
) -> Result<DraftPage, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    if query.limit == 0
        || query.limit > cl_api::draft_limits::MAX_PAGE
        || query.offset > cl_api::draft_limits::MAX_OFFSET
    {
        return Err(invalid());
    }
    let mut rows: Vec<&Row> = state
        .drafts
        .rows
        .iter()
        .filter(|r| r.tenant == tenant)
        .filter(|r| query.statuses.is_empty() || query.statuses.contains(&r.draft.status))
        .filter(|r| query.kind.is_none_or(|k| k == r.draft.kind))
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.seq));
    let total = rows.len() as u64;
    let drafts = rows
        .into_iter()
        .skip(query.offset as usize)
        .take(query.limit as usize)
        .map(|r| {
            let mut d = r.draft.clone();
            if query.summary {
                d.fields = None;
                d.lines = None;
                d.checks = None;
            }
            d
        })
        .collect();
    Ok(DraftPage { drafts, total })
}

/// `GET /document-drafts/{id}`.
///
/// # Errors
/// `Local(Unsupported)`; `NotFound` (also another company's).
pub fn get(
    state: &DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
) -> Result<DocumentDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    state
        .drafts
        .find(tenant, id)
        .map(|r| r.draft.clone())
        .ok_or_else(not_found)
}

/// A JSON value the engine stores (cl-api's own check: integers within
/// ±10⁹, bounded strings and keys, at most 6 deep; never a fraction).
fn json_ok(v: &Value, depth: u32) -> bool {
    use cl_api::draft_limits as l;
    match v {
        Value::Null | Value::Bool(_) => true,
        Value::String(s) => s.encode_utf16().count() <= l::MAX_STRING,
        Value::Number(n) => {
            n.as_u64().is_some_and(|u| u <= l::MAX_INTEGER)
                || n.as_i64()
                    .is_some_and(|i| i.unsigned_abs() <= l::MAX_INTEGER)
        }
        Value::Array(a) => depth < l::MAX_DEPTH && a.iter().all(|x| json_ok(x, depth + 1)),
        Value::Object(m) => {
            depth < l::MAX_DEPTH
                && m.iter()
                    .all(|(k, x)| k.encode_utf16().count() <= l::MAX_KEY && json_ok(x, depth + 1))
        }
    }
}

fn bounded(v: &Value, max: usize) -> bool {
    json_ok(v, 0) && serde_json::to_vec(v).is_ok_and(|b| b.len() <= max)
}

fn contact_known(state: &mut DemoState, id: ContactId) -> bool {
    state.contacts_mut().iter().any(|c| c.id == id)
}

/// `PUT /document-drafts/{id}` with a reading (status `ready`).
///
/// # Errors
/// `Local(Unsupported)`; `Local(InvalidValue)` over the bounds (nothing
/// sent); `NotFound` (the draft, or a vendor that is not the company's);
/// `Conflict { Stale }`; [`cl_api::DraftRefusal::Closed`].
pub fn store_reading(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
    expected_updated_at: &str,
    reading: &DraftReading,
) -> Result<DocumentDraft, ApiError> {
    use cl_api::draft_limits as l;
    if !state.drafts.enabled {
        return unsupported();
    }
    let fields = Value::Object(reading.fields.clone());
    let lines = Value::Array(reading.lines.clone());
    let checks = Value::Array(reading.checks.clone());
    if reading.lines.len() > l::MAX_LINES
        || reading.checks.len() > l::MAX_CHECKS
        || !bounded(&fields, l::FIELDS_BYTES)
        || !bounded(&lines, l::LINES_BYTES)
        || !bounded(&checks, l::CHECKS_BYTES)
    {
        return Err(invalid());
    }
    state.drafts.requests += 1;
    changeable(state, tenant, id, expected_updated_at)?;
    if let Some(c) = reading.suggested_contact
        && !contact_known(state, c)
    {
        return Err(not_found());
    }
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    let d = &mut row.draft;
    d.status = DraftStatus::Ready;
    d.kind = reading.kind;
    d.reader = Some(reading.reader);
    d.fields = Some(fields);
    d.lines = Some(lines);
    d.checks = Some(checks);
    d.failure_reason = None;
    d.suggested_contact_id = reading.suggested_contact;
    d.doc_reference = reading
        .doc_reference
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty() && r.chars().count() <= l::MAX_REFERENCE)
        .map(str::to_string);
    d.doc_total = reading.doc_total.filter(|t| !t.is_negative());
    d.doc_date = reading.doc_date;
    d.updated_at = updated_at;
    Ok(d.clone())
}

/// `PUT /document-drafts/{id}` with a failure (status `failed`); the rest
/// of the draft is kept.
///
/// # Errors
/// `Local(InvalidValue)` for a reason outside `[a-z0-9_]{1,40}`; otherwise
/// as [`store_reading`].
pub fn store_failure(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
    expected_updated_at: &str,
    reason: &str,
) -> Result<DocumentDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    let reason_ok = (1..=40).contains(&reason.len())
        && reason
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !reason_ok {
        return Err(invalid());
    }
    state.drafts.requests += 1;
    changeable(state, tenant, id, expected_updated_at)?;
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    row.draft.status = DraftStatus::Failed;
    row.draft.failure_reason = Some(reason.to_string());
    row.draft.updated_at = updated_at;
    Ok(row.draft.clone())
}

/// `POST /document-drafts/{id}/discard`: frees the claim (the file stays).
/// Already discarded: the draft as it is, nothing written.
///
/// # Errors
/// `Local(Unsupported)`; `NotFound`; [`cl_api::DraftRefusal::Closed`] for
/// a converted draft.
pub fn discard(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
) -> Result<DocumentDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    state.drafts.requests += 1;
    let row = state.drafts.find(tenant, id).ok_or_else(not_found)?;
    match row.draft.status {
        DraftStatus::Discarded => return Ok(row.draft.clone()),
        DraftStatus::Converted => return Err(closed(&row.draft)),
        _ => {}
    }
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    row.draft.status = DraftStatus::Discarded;
    row.draft.updated_at = updated_at;
    Ok(row.draft.clone())
}

/// Another computer changes the draft (a reading stored elsewhere): its
/// token moves on, so a form holding the old one gets 409 `STALE`. Tests
/// and scenarios use it. Returns the new token.
///
/// # Errors
/// `NotFound`; [`cl_api::DraftRefusal::Closed`].
pub fn touch(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
) -> Result<String, ApiError> {
    let row = state.drafts.find(tenant, id).ok_or_else(not_found)?;
    if !row.draft.status.is_live() {
        return Err(closed(&row.draft));
    }
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    row.draft.updated_at.clone_from(&updated_at);
    Ok(updated_at)
}

// -------------------------------------------------------------- duplicates

/// What a duplicate check compares (the engine's `DuplicateProbe`).
struct Probe {
    vendor: Option<ContactId>,
    reference: Option<String>,
    total: Option<Money>,
    date: Option<CalendarDate>,
    except_txn: Option<TransactionId>,
    except_draft: Option<DocumentDraftId>,
}

fn normalized(r: Option<&str>) -> String {
    r.map(cl_docread::dupes::normalize_reference)
        .unwrap_or_default()
}

fn within_window(a: CalendarDate, b: CalendarDate) -> bool {
    a.days_until(b).abs() <= DUPLICATE_WINDOW_DAYS
}

/// The names the demo ledger knows contact `id` by (its name now and its
/// sample name).
fn names_of(state: &mut DemoState, id: ContactId) -> Vec<String> {
    let mut names: Vec<String> = state
        .contacts_mut()
        .iter()
        .filter(|c| c.id == id)
        .map(|c| c.fields.display_name.clone())
        .collect();
    if let Some(n) = state.contact_names.get(&id) {
        names.push(n.clone());
    }
    names
}

/// The engine's type of a demo ledger record (`expense` for checks and
/// card charges).
fn engine_type(kind: &str) -> TxnType {
    super::register::txn_type(kind)
}

/// The reference a posted record carries: a bill's vendor invoice number,
/// an expense's own number (not a check number).
fn txn_reference(
    state: &mut DemoState,
    t: &cl_company::sample::SampleTransaction,
) -> Option<String> {
    match t.kind.as_str() {
        "bill" => super::bills::get(state, DemoLedger::id_of(t))
            .and_then(|v| v.record.vendor_invoice_number),
        "check" => None,
        _ => Some(t.number.clone()).filter(|n| !n.is_empty()),
    }
}

/// Posted bills and expenses of the vendor, not void, that match by
/// normalized reference or by exact total within the window.
fn transaction_matches(state: &mut DemoState, p: &Probe) -> Vec<DuplicateMatch> {
    let Some(vendor) = p.vendor else {
        return Vec::new();
    };
    let names = names_of(state, vendor);
    let want = normalized(p.reference.as_deref());
    let mut out = Vec::new();
    for t in state.ledger.transactions() {
        let ty = match engine_type(&t.kind) {
            TxnType::Bill => "bill",
            TxnType::Expense if matches!(t.kind.as_str(), "check" | "card_charge" | "expense") => {
                "expense"
            }
            _ => continue,
        };
        let id = DemoLedger::id_of(&t);
        if state.ledger.is_void(id)
            || Some(id) == p.except_txn
            || !t.contact.as_ref().is_some_and(|c| names.contains(c))
        {
            continue;
        }
        let total = Money::sum(t.lines.iter().map(|l| l.debit)).ok();
        let reference = txn_reference(state, &t);
        let by_reference = !want.is_empty() && normalized(reference.as_deref()) == want;
        let by_amount = matches!(
            (p.total, p.date, total),
            (Some(a), Some(d), Some(b)) if a == b && within_window(d, t.date)
        );
        if by_reference || by_amount {
            out.push(DuplicateMatch {
                source: MatchSource::Transaction(id),
                reason: if by_reference {
                    MatchReason::Reference
                } else {
                    MatchReason::AmountDate
                },
                txn_type: Some(ty.to_string()),
                kind: None,
                date: Some(t.date),
                total,
                reference,
            });
        }
    }
    out.sort_by(|a, b| b.date.cmp(&a.date).then(key_of(a).cmp(&key_of(b))));
    out.truncate(MAX_MATCHES);
    out
}

fn key_of(m: &DuplicateMatch) -> String {
    match m.source {
        MatchSource::Transaction(id) => id.to_string(),
        MatchSource::Draft(id) => id.to_string(),
    }
}

/// Other live drafts of the vendor in the company, by the same two rules.
fn draft_matches(state: &DemoState, tenant: TenantId, p: &Probe) -> Vec<DuplicateMatch> {
    let Some(vendor) = p.vendor else {
        return Vec::new();
    };
    let want = normalized(p.reference.as_deref());
    let mut rows: Vec<&Row> = state
        .drafts
        .rows
        .iter()
        .filter(|r| {
            let d = &r.draft;
            r.tenant == tenant
                && d.status.is_live()
                && d.suggested_contact_id == Some(vendor)
                && Some(d.id) != p.except_draft
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.seq));
    rows.into_iter()
        .filter_map(|r| {
            let d = &r.draft;
            let by_reference = !want.is_empty() && normalized(d.doc_reference.as_deref()) == want;
            let by_amount = matches!(
                (p.total, p.date, d.doc_total, d.doc_date),
                (Some(a), Some(pd), Some(b), Some(dd)) if a == b && within_window(pd, dd)
            );
            (by_reference || by_amount).then(|| DuplicateMatch {
                source: MatchSource::Draft(d.id),
                reason: if by_reference {
                    MatchReason::Reference
                } else {
                    MatchReason::AmountDate
                },
                txn_type: None,
                kind: Some(d.kind),
                date: d.doc_date,
                total: d.doc_total,
                reference: d.doc_reference.clone(),
            })
        })
        .take(MAX_MATCHES)
        .collect()
}

/// A match on the wire (`{source, id, reason, txnType, kind, date, total,
/// reference}`), amounts as decimal strings.
fn match_wire(m: &DuplicateMatch) -> Value {
    let (source, id) = match m.source {
        MatchSource::Transaction(id) => ("transaction", id.to_string()),
        MatchSource::Draft(id) => ("draft", id.to_string()),
    };
    json!({
        "source": source,
        "id": id,
        "reason": match m.reason {
            MatchReason::Reference => "reference",
            _ => "amount_date",
        },
        "txnType": m.txn_type,
        "kind": m.kind,
        "date": m.date.map(CalendarDate::to_wire),
        "total": m.total.map(Money::to_wire),
        "reference": m.reference,
    })
}

/// `GET /document-drafts/{id}/duplicates`: posted records and other live
/// drafts of the draft's vendor that may be the same document. Empty
/// without a vendor.
///
/// # Errors
/// `Local(Unsupported)`; `NotFound`.
pub fn duplicates(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
) -> Result<Vec<DuplicateMatch>, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    let d = state
        .drafts
        .find(tenant, id)
        .map(|r| r.draft.clone())
        .ok_or_else(not_found)?;
    let probe = Probe {
        vendor: d.suggested_contact_id,
        reference: d.doc_reference.clone(),
        total: d.doc_total,
        date: d.doc_date,
        except_txn: d.converted_transaction_id,
        except_draft: Some(d.id),
    };
    let mut out = transaction_matches(state, &probe);
    out.extend(draft_matches(state, tenant, &probe));
    Ok(out)
}

// ----------------------------------------------------------------- convert

/// What a draft is posted as in the demo: the inputs of the same typed
/// builders the forms use (cl-api's `ConvertBody` holds checked
/// `BankPosting`s the demo can't read back). `draft_attachment` is ignored:
/// convert moves the draft's own file.
#[derive(Clone, PartialEq)]
pub enum DemoConvertBody {
    /// A bill (`POST /bills`' body).
    Bill(BillDraft),
    /// A card charge paid from the card.
    CardCharge(CardChargeDraft),
    /// A cash expense paid from cash or a bank account.
    CashExpense(CardChargeDraft),
    /// A check already written.
    Check(CheckDraft),
}

impl fmt::Debug for DemoConvertBody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DemoConvertBody({})", self.kind().as_str())
    }
}

impl DemoConvertBody {
    /// The kind it posts as.
    #[must_use]
    pub const fn kind(&self) -> DraftKind {
        match self {
            DemoConvertBody::Bill(_) => DraftKind::Bill,
            DemoConvertBody::CardCharge(_) => DraftKind::CardCharge,
            DemoConvertBody::CashExpense(_) => DraftKind::CashExpense,
            DemoConvertBody::Check(_) => DraftKind::Check,
        }
    }

    fn date(&self) -> CalendarDate {
        match self {
            DemoConvertBody::Bill(d) => d.date,
            DemoConvertBody::CardCharge(d) | DemoConvertBody::CashExpense(d) => d.date,
            DemoConvertBody::Check(d) => d.date,
        }
    }

    fn vendor(&self) -> Option<ContactId> {
        match self {
            DemoConvertBody::Bill(d) => Some(d.vendor),
            DemoConvertBody::CardCharge(d) | DemoConvertBody::CashExpense(d) => d.payee,
            DemoConvertBody::Check(d) => d.payee,
        }
    }

    fn total(&self) -> Option<Money> {
        match self {
            DemoConvertBody::Bill(d) => Some(d.amount_due),
            DemoConvertBody::CardCharge(d) | DemoConvertBody::CashExpense(d) => {
                Money::sum(d.lines.iter().map(|l| l.amount)).ok()
            }
            DemoConvertBody::Check(d) => Some(d.amount),
        }
    }

    /// The reference the posted record carries (a check carries none).
    fn reference(&self) -> Option<String> {
        match self {
            DemoConvertBody::Bill(d) => d.vendor_invoice_number.clone(),
            DemoConvertBody::CardCharge(d) | DemoConvertBody::CashExpense(d) => d.number.clone(),
            DemoConvertBody::Check(_) => None,
        }
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty())
    }
}

/// A convert request (cl-api's `DraftConversion` over [`DemoConvertBody`]).
#[derive(Clone, Debug, PartialEq)]
pub struct DemoConversion {
    /// What to post.
    pub body: DemoConvertBody,
    /// The draft's `updated_at` as last read.
    pub expected_updated_at: String,
    /// Posted records the owner saw and wants to enter this anyway.
    pub acknowledge: Vec<TransactionId>,
}

fn closing_of(state: &DemoState, tenant: TenantId) -> Option<CalendarDate> {
    state
        .companies
        .iter()
        .find(|c| c.tenant_id == tenant)
        .and_then(|c| c.closed_through)
}

/// The checks cl-api makes before sending (nothing is sent when they
/// fail).
fn check_body(state: &DemoState, body: &DemoConvertBody) -> Result<(), ApiError> {
    let billable = !state.checks.line_billable_off;
    match body {
        DemoConvertBody::Bill(_) => Ok(()),
        DemoConvertBody::CardCharge(d) | DemoConvertBody::CashExpense(d) => {
            BankPosting::card_charge_with(d.clone(), billable)
                .map(|_| ())
                .map_err(ApiError::Local)
        }
        DemoConvertBody::Check(d) => BankPosting::check_with(d.clone(), billable)
            .map(|_| ())
            .map_err(ApiError::Local),
    }
}

fn tag_name(state: &mut DemoState, id: Option<cl_core::ids::TagId>) -> Option<String> {
    let id = id?;
    state
        .tags_mut()
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.fields.name.clone())
}

/// Posts a card charge or cash expense as the demo's Record Card Charge
/// does (`kit::demo_save`): the card (or bank) credited, the lines debited.
fn post_expense(
    state: &mut DemoState,
    d: &CardChargeDraft,
    kind: &'static str,
) -> Result<TransactionId, ApiError> {
    let chart = state.chart.get_or_insert_with(super::chart).clone();
    let contact = d.payee.and_then(|p| {
        state
            .contacts_mut()
            .iter()
            .find(|c| c.id == p)
            .map(|c| c.fields.display_name.clone())
    });
    let posting = DemoPosting {
        kind,
        date: d.date,
        number: d.number.clone().unwrap_or_default(),
        contact,
        memo: d.memo.clone().unwrap_or_default(),
        own: d.pay_from,
        own_debit: false,
        lines: d.lines.iter().map(|l| (l.account, l.amount)).collect(),
    };
    let id = state
        .ledger
        .save(&chart, &posting, None)
        .ok_or_else(invalid)?;
    let details: Vec<(Option<String>, Option<String>)> = d
        .lines
        .iter()
        .map(|l| (l.memo.clone(), tag_name(state, l.tag)))
        .collect();
    state
        .ledger
        .set_line_details(&chart, id, d.pay_from, &details);
    Ok(id)
}

/// Posts `body` through the demo's own services (the record only; the
/// file moves in [`convert`]).
fn post(
    state: &mut DemoState,
    body: &DemoConvertBody,
    password: Option<&Secret>,
) -> Result<TransactionId, ApiError> {
    match body {
        DemoConvertBody::Bill(d) => {
            let mut d = d.clone();
            d.draft_attachment = None;
            super::bills::create_bill(state, &d, password)
        }
        DemoConvertBody::Check(d) => {
            let mut d = d.clone();
            d.draft_attachment = None;
            super::checks::create(state, &d, password)
        }
        DemoConvertBody::CardCharge(d) => post_expense(state, d, "card_charge"),
        DemoConvertBody::CashExpense(d) => post_expense(state, d, "expense"),
    }
}

/// The draft's own file, when it is still on a draft key.
fn draft_key_of(state: &DemoState, file: Option<AttachmentId>) -> Option<String> {
    let a = super::checks::find_attachment(state, file?)?;
    (a.attachable_type == "draft").then_some(a.attachable_id)
}

/// `POST /document-drafts/{id}/convert`: posts the draft as `conversion`
/// says, moves its file onto the new record (the guard keeps files other
/// live drafts read), marks it converted. Keyed. `password` is the
/// closing-date password (never part of the request's identity).
///
/// # Errors
/// Nothing changes on any error. `Local(Unsupported)`; `Local(…)` for a
/// body cl-api refuses before sending, or more than 50 acknowledged;
/// `IdempotencyKeyReused`; `Conflict { Busy }`; `NotFound` (the draft, or a
/// vendor not in the company); [`cl_api::DraftRefusal::Closed`];
/// `Conflict { Stale }`; `ClosedPeriod` / `ClosingPasswordRejected`;
/// [`cl_api::DraftRefusal::PossibleDuplicate`]; the posting's own refusals.
/// With `lose_next_reply`: `OutcomeUnknown` (keyed) after committing.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the key is taken as cl_api::DocumentDrafts takes it"
)]
pub fn convert(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
    conversion: &DemoConversion,
    password: Option<&Secret>,
    key: IdempotencyKey,
) -> Result<ConvertedDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    if conversion.acknowledge.len() > cl_api::draft_limits::MAX_ACKNOWLEDGED {
        return Err(invalid());
    }
    check_body(state, &conversion.body)?;
    state.drafts.requests += 1;
    let request = Request::Convert {
        tenant,
        id,
        conversion: conversion.clone(),
    };
    if let Some(Reply::Converted(c)) = replay(state, &key, &request)? {
        return Ok(c);
    }
    busy(state)?;
    let before = changeable(state, tenant, id, &conversion.expected_updated_at)?;
    let body = &conversion.body;
    let vendor = body.vendor();
    if let Some(v) = vendor
        && !contact_known(state, v)
    {
        return Err(not_found());
    }
    // The posting's closing-date check comes first (423 before 409).
    check_closing(closing_of(state, tenant), &[body.date()], password)?;
    let probe = Probe {
        vendor,
        reference: body.reference().or_else(|| before.doc_reference.clone()),
        total: body.total(),
        date: Some(body.date()),
        except_txn: None,
        except_draft: Some(id),
    };
    let open: Vec<DuplicateMatch> = transaction_matches(state, &probe)
        .into_iter()
        .filter(|m| match m.source {
            MatchSource::Transaction(t) => !conversion.acknowledge.contains(&t),
            MatchSource::Draft(_) => true,
        })
        .collect();
    if !open.is_empty() {
        let drafts = draft_matches(state, tenant, &probe);
        return Err(refusal(
            409,
            cl_api::POSSIBLE_DUPLICATE,
            json!({
                "matches": open.iter().map(match_wire).collect::<Vec<_>>(),
                "draftMatches": drafts.iter().map(match_wire).collect::<Vec<_>>(),
            }),
        ));
    }
    let txn = post(state, body, password)?;
    state.drafts.created.push(txn);
    let to = match body {
        DemoConvertBody::Bill(_) => AttachTo::Bill(txn),
        _ => AttachTo::Expense(txn),
    };
    if let Some(k) = draft_key_of(state, before.attachment_id) {
        super::checks::move_draft_key(state, &k, to, Some(id));
    }
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    row.draft.status = DraftStatus::Converted;
    row.draft.kind = body.kind();
    row.draft.converted_transaction_id = Some(txn);
    row.draft.converted_type = Some(to.attachable_type().to_string());
    row.draft.updated_at = updated_at;
    let draft = row.draft.clone();
    let posted = Probe {
        except_txn: Some(txn),
        ..probe
    };
    let converted = ConvertedDraft {
        draft_matches: draft_matches(state, tenant, &posted),
        draft,
        transaction: txn,
    };
    remember(state, &key, request, Reply::Converted(converted.clone()));
    if std::mem::take(&mut state.drafts.lose_next_reply) {
        return Err(lost(&key));
    }
    Ok(converted)
}

// ------------------------------------------------------------------ attach

/// An attached draft (`{draft, transaction}`, U-95).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachedDraft {
    /// The draft, now `Converted` into the record it was attached to (its
    /// kind unchanged).
    pub draft: DocumentDraft,
    /// The record that now has the document.
    pub transaction: TransactionId,
}

/// `POST /document-drafts/{id}/attach` (U-95): files the draft's document
/// on a record that is already posted instead of posting a new one. Keyed;
/// nothing posts, so no closing-date password.
///
/// # Errors
/// Nothing changes on any error. `Local(Unsupported)`;
/// `IdempotencyKeyReused`; `Conflict { Busy }`; `NotFound` (the draft, or a
/// record that is gone); [`cl_api::DraftRefusal::Closed`];
/// `Conflict { Stale }`; `Validation` 422 `TARGET_NOT_ATTACHABLE` or
/// `DOCUMENT_FILE_MISSING`; `Conflict` 409 `TRANSACTION_NOT_POSTED` (void);
/// [`cl_api::DraftRefusal::NotADraftFile`] (the file is on another record);
/// `Conflict { InUse }`. With `lose_next_reply`: `OutcomeUnknown` (keyed)
/// after committing.
#[allow(
    clippy::needless_pass_by_value,
    reason = "the key is taken as cl_api::DocumentDrafts takes it"
)]
pub fn attach(
    state: &mut DemoState,
    tenant: TenantId,
    id: DocumentDraftId,
    transaction: TransactionId,
    expected_updated_at: &str,
    key: IdempotencyKey,
) -> Result<AttachedDraft, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    state.drafts.requests += 1;
    let request = Request::Attach {
        tenant,
        id,
        transaction,
        expected: expected_updated_at.to_string(),
    };
    if let Some(Reply::Attached(a)) = replay(state, &key, &request)? {
        return Ok(a);
    }
    busy(state)?;
    let before = changeable(state, tenant, id, expected_updated_at)?;
    let kind = state
        .ledger
        .transactions()
        .into_iter()
        .find(|t| DemoLedger::id_of(t) == transaction)
        .map(|t| t.kind)
        .ok_or_else(|| refusal(404, "NOT_FOUND", Value::Null))?;
    let to = AttachTo::Txn(engine_type(&kind), transaction);
    // The engine's `txn_type` (an adjusting entry is `aje`; its files are
    // stored as `journal_entry`, see [`file_type`]).
    let type_name = to.attachable_type();
    if !ATTACHABLE.contains(&type_name) {
        return Err(refusal(
            422,
            "TARGET_NOT_ATTACHABLE",
            json!({"txnType": type_name}),
        ));
    }
    if state.ledger.is_void(transaction) {
        return Err(refusal(
            409,
            "TRANSACTION_NOT_POSTED",
            json!({"status": "void"}),
        ));
    }
    let missing = || refusal(422, "DOCUMENT_FILE_MISSING", Value::Null);
    let file = before.attachment_id.ok_or_else(missing)?;
    let found = super::checks::find_attachment(state, file).ok_or_else(missing)?;
    let on_target =
        found.attachable_type == file_type(to) && found.attachable_id == to.attachable_id();
    if !on_target {
        if found.attachable_type != "draft" {
            return Err(refusal(422, cl_api::ATTACHMENT_NOT_DRAFT, Value::Null));
        }
        let moved = super::checks::move_draft_key(state, &found.attachable_id, to, Some(id));
        if !moved.contains(&file) {
            return Err(refusal(
                409,
                cl_api::ATTACHMENT_IN_USE,
                json!({"draftId": state.drafts.reader_of(file)}),
            ));
        }
    }
    let updated_at = state.drafts.stamp();
    let row = state.drafts.find_mut(tenant, id).ok_or_else(not_found)?;
    row.draft.status = DraftStatus::Converted;
    row.draft.converted_transaction_id = Some(transaction);
    row.draft.converted_type = Some(type_name.to_string());
    row.draft.updated_at = updated_at;
    let attached = AttachedDraft {
        draft: row.draft.clone(),
        transaction,
    };
    remember(state, &key, request, Reply::Attached(attached.clone()));
    if std::mem::take(&mut state.drafts.lose_next_reply) {
        return Err(lost(&key));
    }
    Ok(attached)
}

// ---------------------------------------------------- the register search

/// The register search of Q-282 (`GET /transactions` with the exact
/// `amount` filter, U-95 c): posted records whose total is exactly
/// `amount`, dated `start` to `end`, optionally only those with a line on
/// `account`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AmountSearch {
    /// The exact total.
    pub amount: Money,
    /// First date.
    pub start: CalendarDate,
    /// Last date.
    pub end: CalendarDate,
    /// Only records with a line on this account.
    pub account: Option<AccountId>,
}

/// Never shows the amount (owner data).
impl fmt::Debug for AmountSearch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AmountSearch")
            .field("start", &self.start)
            .field("end", &self.end)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

/// The demo ledger's answer to an [`AmountSearch`], newest first: posted
/// only (void and deleted records left out); totals compared exactly
/// (`10.1` is `10.10`, never `10.01`).
///
/// # Errors
/// `Local(Unsupported)` without drafts (the filter comes with
/// `document-drafts-attach-v1`); `Timeout` once after `fail_next_search`.
pub fn search_amount(
    state: &mut DemoState,
    q: &AmountSearch,
) -> Result<Vec<TransactionSummary>, ApiError> {
    if !state.drafts.enabled {
        return unsupported();
    }
    if std::mem::take(&mut state.drafts.fail_next_search) {
        return Err(ApiError::Timeout);
    }
    let chart = state.chart.get_or_insert_with(super::chart).clone();
    let number = q.account.and_then(|a| {
        chart
            .iter()
            .find(|x| x.id == a)
            .and_then(|x| x.fields.account_number.clone())
    });
    if q.account.is_some() && number.is_none() {
        return Ok(Vec::new());
    }
    let contacts = state.contacts_mut().clone();
    let mut txns: Vec<_> = state
        .ledger
        .transactions()
        .into_iter()
        .filter(|t| t.date >= q.start && t.date <= q.end)
        .filter(|t| !state.ledger.is_void(DemoLedger::id_of(t)))
        .filter(|t| {
            number
                .as_ref()
                .is_none_or(|n| t.lines.iter().any(|l| l.account == *n))
        })
        .filter(|t| Money::sum(t.lines.iter().map(|l| l.debit)).ok() == Some(q.amount))
        .collect();
    txns.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    Ok(txns
        .into_iter()
        .map(|t| {
            let contact_id = t.contact.as_ref().and_then(|n| {
                contacts
                    .iter()
                    .find(|c| c.fields.display_name == *n)
                    .map(|c| c.id)
            });
            TransactionSummary {
                id: DemoLedger::id_of(&t),
                txn_type: engine_type(&t.kind),
                txn_number: Some(t.number.clone()).filter(|n| !n.is_empty() && t.kind != "check"),
                txn_date: t.date,
                status: TxnStatus::Posted,
                contact_id,
                contact_name: t.contact.clone(),
                memo: Some(t.memo.clone()).filter(|m| !m.is_empty()),
                total: Some(q.amount),
                display_total: Some(q.amount),
                created_at: None,
                check_number: if t.kind == "check" {
                    t.number.parse().ok()
                } else {
                    None
                },
            }
        })
        .collect())
}

#[cfg(test)]
#[path = "drafts_tests.rs"]
mod tests;
