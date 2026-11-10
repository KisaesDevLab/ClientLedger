//! The one accept route and the one undo route of the bank feed (P26 E3,
//! U-170 / U-169).
//!
//! - `POST /banking/feed/accept` posts or matches a selection (1–500 rows)
//!   or every waiting row one auto-accept rule staged ("Accept all from
//!   this rule", 500 a call, paged with `afterId`). One database
//!   transaction, one required `Idempotency-Key`; a replay answers the
//!   same body. Rows the engine leaves alone come back in `skipped`, rows
//!   that failed in `failures` (`TB_PERIOD_LOCKED`: send the password and
//!   retry **those rows under a fresh key**).
//! - `POST /banking/feed/undo` takes rows or a whole accept group back:
//!   a posted entry is deleted, a match only unlinked. 409
//!   `FEED_UNDO_CHANGED {ids}` (edited since) undoes nothing: ask the
//!   owner, then send again with `force`.
//!
//! A05 (U-181, `feed-accept-draft-v1`): an item may carry a document
//! draft ([`AcceptItem::Receipt`]); the engine attaches its file to the
//! entry it posts or matches and closes the draft in the same transaction.
//! A05 (U-180, `document-drafts-reopen-v1`): an Undo that deletes an entry
//! returns its drafts to the review queue ([`UndoOutcome::reopened_drafts`]).
//!
//! Amounts: a split travels as in [`choice_body`]; nothing here totals.

use std::fmt;

use cl_core::ids::{JournalLineId, TransactionId};
use serde::Deserialize;

use crate::resources::document_drafts::DocumentDraftId;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictDetails, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::feed::{Feed, FeedItemId};
use crate::resources::feed_review::{BankConnectionId, FeedChoice, choice_body};
use crate::resources::operations::{FEED_ACCEPT, FEED_ACCEPT_GROUP, FEED_UNDO};
use crate::resources::posting::ClosedPeriodAuthorization;
use crate::wire::Capabilities;

/// The most rows one accept or undo takes (the engine's `FEED_ACCEPT_MAX`).
pub const ACCEPT_MAX: usize = 500;

/// An accept group: what one accept (or one transfer, deposit or match)
/// posted, undone together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UndoGroupId(uuid::Uuid);

impl UndoGroupId {
    /// Parses a group id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<UndoGroupId, ApiError> {
        cl_core::ids::parse_uuid(s)
            .map(UndoGroupId)
            .map_err(|_| ApiError::Decode {
                what: "accept group id",
            })
    }

    /// A random id (the demo's groups).
    #[must_use]
    pub fn new_random() -> UndoGroupId {
        UndoGroupId(uuid::Uuid::new_v4())
    }
}

impl fmt::Display for UndoGroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A document draft an accept files on the row's entry (U-181).
#[derive(Clone, PartialEq, Eq)]
pub struct DraftLink {
    /// The draft.
    pub id: DocumentDraftId,
    /// Its change token exactly as read (`updatedAt`), never reformatted.
    pub expected_updated_at: String,
    /// PR-A1 B1 (Q-PRA1b): the posted entries the owner confirmed this
    /// receipt is not (after a `DOCUMENT_POSSIBLE_DUPLICATE`, "Use it
    /// anyway"). Empty: the engine's duplicate check refuses any match.
    pub acknowledged_duplicates: Vec<TransactionId>,
}

impl fmt::Debug for DraftLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DraftLink")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// One row of an accept.
#[derive(Clone, PartialEq, Eq)]
pub enum AcceptItem {
    /// As proposed: what is staged on the row, else its suggestion.
    AsProposed(FeedItemId),
    /// The owner's values (account, payee, tag, memo, split).
    Values(FeedItemId, FeedChoice),
    /// Match it to a line of a posted entry (`line` may be left out when
    /// only one line fits).
    Match {
        /// The row.
        id: FeedItemId,
        /// The entry.
        transaction: TransactionId,
        /// Its bank line.
        line: Option<JournalLineId>,
    },
    /// `item` (as proposed, values or a match) with a document draft whose
    /// file is attached to the entry and which closes with it (U-181,
    /// `feed-accept-draft-v1`). Never nested.
    Receipt {
        /// What posts or matches.
        item: Box<AcceptItem>,
        /// The draft.
        draft: DraftLink,
    },
}

impl AcceptItem {
    /// The row.
    #[must_use]
    pub fn id(&self) -> FeedItemId {
        match self {
            AcceptItem::AsProposed(id)
            | AcceptItem::Values(id, _)
            | AcceptItem::Match { id, .. } => *id,
            AcceptItem::Receipt { item, .. } => item.id(),
        }
    }

    /// The draft this item files, if any.
    #[must_use]
    pub fn draft(&self) -> Option<&DraftLink> {
        match self {
            AcceptItem::Receipt { draft, .. } => Some(draft),
            _ => None,
        }
    }

    /// What posts or matches (the item without its draft).
    #[must_use]
    pub fn inner(&self) -> &AcceptItem {
        match self {
            AcceptItem::Receipt { item, .. } => item,
            other => other,
        }
    }
}

impl fmt::Debug for AcceptItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AcceptItem::AsProposed(id) => write!(f, "AsProposed({id})"),
            AcceptItem::Values(id, c) => write!(f, "Values({id}, {c:?})"),
            AcceptItem::Match {
                id, transaction, ..
            } => {
                write!(f, "Match({id} -> {transaction})")
            }
            AcceptItem::Receipt { item, draft } => {
                write!(f, "Receipt({item:?}, {})", draft.id)
            }
        }
    }
}

/// "Accept all from this rule": the rule as the list showed it.
#[derive(Clone, PartialEq, Eq)]
pub struct RuleAccept {
    /// The rule's id.
    pub rule_id: String,
    /// `conditional` or `legacy`.
    pub kind: String,
    /// The rule's `updatedAt` exactly as listed (`suggestedRule.updatedAt`).
    pub updated_at: String,
    /// One bank account only.
    pub connection: Option<BankConnectionId>,
    /// The next page starts after this row (the last answer's `nextAfterId`).
    pub after: Option<FeedItemId>,
}

impl fmt::Debug for RuleAccept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuleAccept")
            .field("kind", &self.kind)
            .field("after", &self.after)
            .finish_non_exhaustive()
    }
}

/// What one accept asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcceptRequest {
    /// The owner's selection (1–500 rows).
    Items(Vec<AcceptItem>),
    /// Every waiting row one rule staged.
    Rule(RuleAccept),
}

/// Why the engine left a row alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AcceptSkip {
    /// No such row.
    NotFound,
    /// Another request holds it (refresh it).
    InUse,
    /// Already accepted, excluded, or its entry was voided.
    NotPending,
    /// A possible duplicate waiting for Keep both / Remove.
    PossibleDuplicate,
    /// It has a match signal and no values or match were sent.
    HasMatch,
    /// No account.
    Incomplete,
    /// Rule mode: the row's suggestion isn't what the rule stages now.
    StagingDiffers,
    /// A reason this client doesn't know.
    Other,
}

impl AcceptSkip {
    fn parse(s: &str) -> AcceptSkip {
        match s {
            "not_found" => AcceptSkip::NotFound,
            "in_use" => AcceptSkip::InUse,
            "not_pending" => AcceptSkip::NotPending,
            "possible_duplicate" => AcceptSkip::PossibleDuplicate,
            "has_match" => AcceptSkip::HasMatch,
            "incomplete" => AcceptSkip::Incomplete,
            "staging_differs" => AcceptSkip::StagingDiffers,
            _ => AcceptSkip::Other,
        }
    }

    /// The row changed under the owner (another computer): refresh it and
    /// drop it from the selection.
    #[must_use]
    pub const fn is_stale(self) -> bool {
        matches!(
            self,
            AcceptSkip::NotFound
                | AcceptSkip::InUse
                | AcceptSkip::NotPending
                | AcceptSkip::StagingDiffers
        )
    }
}

/// A row that failed (the others still posted).
#[derive(Clone, PartialEq, Eq)]
pub struct AcceptFailure {
    /// The row.
    pub id: FeedItemId,
    /// The engine's code (`TB_PERIOD_LOCKED`, `SPLITS_DONT_BALANCE`,
    /// `FEED_UNDO_RECONCILED`, …).
    pub code: String,
    /// Its details.
    pub details: ConflictDetails,
}

impl fmt::Debug for AcceptFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcceptFailure")
            .field("id", &self.id)
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

impl AcceptFailure {
    /// `TB_PERIOD_LOCKED`: dated on or before the closing date.
    pub const PERIOD_LOCKED: &'static str = "TB_PERIOD_LOCKED";
    /// Undo: in a completed reconciliation.
    pub const UNDO_RECONCILED: &'static str = "FEED_UNDO_RECONCILED";
    /// Undo: a deposit holding a bounced payment.
    pub const PAYMENT_BOUNCED: &'static str = "PAYMENT_BOUNCED";
    /// Undo: the row isn't accepted (a stale Undo).
    pub const NOT_ACCEPTED: &'static str = "FEED_ITEM_NOT_ACCEPTED";
    /// Another request holds the row.
    pub const IN_USE: &'static str = "FEED_ITEM_IN_USE";
    /// U-181: the receipt was already used or thrown away
    /// (`details.transactionId` when it was used). The row didn't post.
    pub const DRAFT_CLOSED: &'static str = "DOCUMENT_DRAFT_CLOSED";
    /// U-181: the receipt changed since it was read (`details.updatedAt`).
    /// The row didn't post.
    pub const DRAFT_STALE: &'static str = "STALE";
    /// U-181: the receipt's file was deleted. The row didn't post.
    pub const DRAFT_FILE_MISSING: &'static str = "DOCUMENT_FILE_MISSING";
    /// U-181: the draft is a vendor statement (never filed on an entry).
    pub const DRAFT_INVALID_KIND: &'static str = "INVALID_KIND";
    /// U-181: the receipt's file is already filed on a record.
    pub const DRAFT_FILE_FILED: &'static str = crate::ATTACHMENT_NOT_DRAFT;
    /// U-181: the matched entry can't take files.
    pub const DRAFT_TARGET: &'static str = crate::TARGET_NOT_ATTACHABLE;
    /// U-181: the user may not file this receipt (the locked rows'
    /// recheck).
    pub const DRAFT_DENIED: &'static str = "PERMISSION_DENIED";
    /// U-181: not this company's draft. (A row that isn't found is
    /// skipped, never a failure, so on an accept this is the receipt.)
    pub const DRAFT_NOT_FOUND: &'static str = "NOT_FOUND";
    /// PR-A1 B1 (`feed-accept-draft-duplicates-v1`): the receipt looks
    /// like an entry already in the books (`details.matches`, as
    /// [`crate::DuplicateMatch`]). The row didn't post; send it again with
    /// the matches in [`DraftLink::acknowledged_duplicates`] after the
    /// owner confirms. Distinct from the bank line's own duplicate.
    pub const DRAFT_POSSIBLE_DUPLICATE: &'static str = "DOCUMENT_POSSIBLE_DUPLICATE";

    /// Dated on or before the closing date: needs the password.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.code == Self::PERIOD_LOCKED
    }

    /// The entries a `DOCUMENT_POSSIBLE_DUPLICATE` names (posted records
    /// only; anything this version can't read is left out). Empty for
    /// any other failure.
    #[must_use]
    pub fn duplicates(&self) -> Vec<crate::DuplicateMatch> {
        if self.code != Self::DRAFT_POSSIBLE_DUPLICATE {
            return Vec::new();
        }
        feed_duplicates_from(self.details.raw("matches"))
    }

    /// A refusal of the row's receipt (U-181); the row didn't post.
    #[must_use]
    pub fn is_receipt(&self) -> bool {
        matches!(
            self.code.as_str(),
            Self::DRAFT_CLOSED
                | Self::DRAFT_STALE
                | Self::DRAFT_FILE_MISSING
                | Self::DRAFT_INVALID_KIND
                | Self::DRAFT_FILE_FILED
                | Self::DRAFT_TARGET
                | Self::DRAFT_DENIED
                | Self::DRAFT_NOT_FOUND
                | Self::DRAFT_POSSIBLE_DUPLICATE
        )
    }
}

/// What an accept did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptOutcome {
    /// Undo takes this whole accept back.
    pub undo_group: UndoGroupId,
    /// Rows posted as new entries.
    pub posted: u32,
    /// Rows matched to entries in the books.
    pub matched: u32,
    /// Rows left alone, with why.
    pub skipped: Vec<(FeedItemId, AcceptSkip)>,
    /// Rows that failed.
    pub failures: Vec<AcceptFailure>,
    /// Rule mode: the rule's rows after this page.
    pub remaining: u64,
    /// Rule mode: send as `after` to go on.
    pub next_after: Option<FeedItemId>,
}

impl AcceptOutcome {
    /// The rows that need the closing-date password.
    #[must_use]
    pub fn locked(&self) -> Vec<FeedItemId> {
        self.failures
            .iter()
            .filter(|f| f.is_locked())
            .map(|f| f.id)
            .collect()
    }
}

/// What Undo takes back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UndoTarget {
    /// These rows (1–500).
    Items(Vec<FeedItemId>),
    /// A whole accept group.
    Group(UndoGroupId),
}

/// One row Undo took back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undone {
    /// The row (back to its state before the accept).
    pub id: FeedItemId,
    /// What the accept had done (`categorize`, `match`, `transfer`, …).
    pub kind: String,
    /// The entry.
    pub transaction: TransactionId,
    /// The entry was deleted (else only unlinked and kept).
    pub deleted: bool,
    /// The accept group it came from.
    pub group: UndoGroupId,
}

/// What an undo did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoOutcome {
    /// Rows taken back.
    pub undone: Vec<Undone>,
    /// Rows that stayed (`FEED_UNDO_RECONCILED`, `TB_PERIOD_LOCKED`, …).
    pub failures: Vec<AcceptFailure>,
    /// Drafts whose entry was deleted: back in the review queue with their
    /// files (U-180, `document-drafts-reopen-v1`; empty without it).
    pub reopened_drafts: Vec<DocumentDraftId>,
}

impl UndoOutcome {
    /// The rows that need the closing-date password.
    #[must_use]
    pub fn locked(&self) -> Vec<FeedItemId> {
        self.failures
            .iter()
            .filter(|f| f.is_locked())
            .map(|f| f.id)
            .collect()
    }
}

// ---------------------------------------------------------------- wire

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFeedDuplicate {
    transaction_id: String,
    #[serde(default, rename = "type")]
    txn_type: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    total: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    no_vendor: bool,
}

/// The posted entries of a `DOCUMENT_POSSIBLE_DUPLICATE` (`{transactionId,
/// type, date, total, reference}` each; the convert's own `{source, id,
/// …}` shape is read too). Entries this version can't read are left out;
/// draft matches never block and are dropped.
pub(crate) fn feed_duplicates_from(v: Option<&Value>) -> Vec<crate::DuplicateMatch> {
    use crate::{MatchReason, MatchSource};
    let Some(list) = v.and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|m| {
            if m.get("transactionId").is_none() {
                return crate::resources::document_drafts::matches_from(Some(&json!([m])))
                    .into_iter()
                    .find(|d| matches!(d.source, MatchSource::Transaction(_)));
            }
            let w: WireFeedDuplicate = serde_json::from_value(m.clone()).ok()?;
            Some(crate::DuplicateMatch {
                source: MatchSource::Transaction(TransactionId::parse(&w.transaction_id).ok()?),
                reason: MatchReason::Other,
                txn_type: w.txn_type,
                kind: None,
                date: w
                    .date
                    .and_then(|d| cl_core::CalendarDate::parse_wire(&d).ok()),
                total: w.total.and_then(|t| cl_core::Money::parse_wire(&t).ok()),
                reference: w.reference,
                no_vendor: w.no_vendor,
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct WireSkip {
    id: String,
    reason: String,
}

#[derive(Deserialize)]
struct WireFailure {
    id: String,
    code: String,
    #[serde(default)]
    details: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAccept {
    undo_group_id: String,
    #[serde(default)]
    posted: u32,
    #[serde(default)]
    matched: u32,
    #[serde(default)]
    skipped: Vec<WireSkip>,
    #[serde(default)]
    failures: Vec<WireFailure>,
    #[serde(default)]
    remaining: u64,
    #[serde(default)]
    next_after_id: Option<String>,
}

fn failures_of(w: Vec<WireFailure>) -> Result<Vec<AcceptFailure>, ApiError> {
    w.into_iter()
        .map(|f| {
            Ok(AcceptFailure {
                id: FeedItemId::parse(&f.id)?,
                code: f.code,
                details: f.details.map(ConflictDetails::new).unwrap_or_default(),
            })
        })
        .collect()
}

/// Reads an accept answer (also `GET /banking/feed/accept-groups/:id`).
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn accept_outcome_from(body: &[u8]) -> Result<AcceptOutcome, ApiError> {
    const WHAT: &str = "feed accept";
    let w: WireAccept =
        serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut skipped = Vec::with_capacity(w.skipped.len());
    for s in w.skipped {
        skipped.push((FeedItemId::parse(&s.id)?, AcceptSkip::parse(&s.reason)));
    }
    Ok(AcceptOutcome {
        undo_group: UndoGroupId::parse(&w.undo_group_id)?,
        posted: w.posted,
        matched: w.matched,
        skipped,
        failures: failures_of(w.failures)?,
        remaining: w.remaining,
        next_after: match w.next_after_id.as_deref() {
            Some(s) => Some(FeedItemId::parse(s)?),
            None => None,
        },
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUndone {
    id: String,
    #[serde(default)]
    kind: Option<String>,
    transaction_id: String,
    entry: String,
    undo_group_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUndo {
    #[serde(default)]
    undone: Vec<WireUndone>,
    #[serde(default)]
    failures: Vec<WireFailure>,
    #[serde(default)]
    reopened_drafts: Vec<String>,
}

/// Reads an undo answer.
///
/// # Errors
/// `Decode` for an unreadable answer.
pub fn undo_outcome_from(body: &[u8]) -> Result<UndoOutcome, ApiError> {
    const WHAT: &str = "feed undo";
    let w: WireUndo = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what: WHAT })?;
    let mut undone = Vec::with_capacity(w.undone.len());
    for u in w.undone {
        undone.push(Undone {
            id: FeedItemId::parse(&u.id)?,
            kind: u.kind.unwrap_or_default(),
            transaction: u
                .transaction_id
                .parse()
                .map_err(|_| ApiError::Decode { what: WHAT })?,
            deleted: u.entry == "deleted",
            group: UndoGroupId::parse(&u.undo_group_id)?,
        });
    }
    let reopened_drafts = w
        .reopened_drafts
        .iter()
        .map(|d| DocumentDraftId::parse(d))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::Decode { what: WHAT })?;
    Ok(UndoOutcome {
        undone,
        failures: failures_of(w.failures)?,
        reopened_drafts,
    })
}

/// The `undoGroupId` of a transfer, deposit or match answer.
pub(crate) fn undo_group_of(body: &[u8]) -> Option<UndoGroupId> {
    let v: Value = serde_json::from_slice(body).ok()?;
    UndoGroupId::parse(v.get("undoGroupId")?.as_str()?).ok()
}

// ---------------------------------------------------------------- bodies

/// The JSON of an accept (without the password).
///
/// # Errors
/// `Local(InvalidValue)` for no rows, more than 500, a repeated row, or an
/// empty rule id or change token; [`choice_body`]'s checks for values.
pub fn accept_body(req: &AcceptRequest) -> Result<Value, ApiError> {
    let bad = || ApiError::Local(LocalValidation::InvalidValue);
    match req {
        AcceptRequest::Items(items) => {
            if items.is_empty() || items.len() > ACCEPT_MAX {
                return Err(bad());
            }
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::with_capacity(items.len());
            for i in items {
                if !seen.insert(i.id()) {
                    return Err(bad());
                }
                let mut m = match i.inner() {
                    AcceptItem::AsProposed(_) => Map::new(),
                    AcceptItem::Values(_, c) => match choice_body(c)? {
                        Value::Object(m) => m,
                        _ => return Err(bad()),
                    },
                    AcceptItem::Match {
                        transaction, line, ..
                    } => {
                        let mut m = Map::new();
                        m.insert("matchTransactionId".into(), json!(transaction.to_string()));
                        if let Some(l) = line {
                            m.insert("journalLineId".into(), json!(l.to_string()));
                        }
                        m
                    }
                    // A receipt inside a receipt.
                    AcceptItem::Receipt { .. } => return Err(bad()),
                };
                if let Some(d) = i.draft() {
                    if d.expected_updated_at.trim().is_empty() {
                        return Err(bad());
                    }
                    let mut dm = Map::new();
                    dm.insert("id".into(), json!(d.id.to_string()));
                    dm.insert("expectedUpdatedAt".into(), json!(d.expected_updated_at));
                    if !d.acknowledged_duplicates.is_empty() {
                        dm.insert(
                            "acknowledgeDuplicateOf".into(),
                            Value::Array(
                                d.acknowledged_duplicates
                                    .iter()
                                    .map(|t| json!(t.to_string()))
                                    .collect(),
                            ),
                        );
                    }
                    m.insert("documentDraft".into(), Value::Object(dm));
                }
                m.insert("id".into(), json!(i.id().to_string()));
                out.push(Value::Object(m));
            }
            Ok(json!({ "items": out }))
        }
        AcceptRequest::Rule(r) => {
            if r.rule_id.trim().is_empty() || r.updated_at.trim().is_empty() {
                return Err(bad());
            }
            let kind = match r.kind.as_str() {
                "legacy" => "legacy",
                _ => "conditional",
            };
            let mut m = Map::new();
            m.insert("ruleId".into(), json!(r.rule_id));
            m.insert("ruleKind".into(), json!(kind));
            m.insert("ruleUpdatedAt".into(), json!(r.updated_at));
            if let Some(c) = r.connection {
                m.insert("bankConnectionId".into(), json!(c.to_string()));
            }
            if let Some(a) = r.after {
                m.insert("afterId".into(), json!(a.to_string()));
            }
            Ok(Value::Object(m))
        }
    }
}

/// The JSON of an undo (without the password).
///
/// # Errors
/// `Local(InvalidValue)` for no rows, more than 500 or a repeat.
pub fn undo_body(target: &UndoTarget, force: bool) -> Result<Value, ApiError> {
    let mut m = Map::new();
    match target {
        UndoTarget::Items(ids) => {
            let mut seen = std::collections::HashSet::new();
            if ids.is_empty() || ids.len() > ACCEPT_MAX || !ids.iter().all(|i| seen.insert(*i)) {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            m.insert(
                "feedItemIds".into(),
                Value::Array(ids.iter().map(|i| json!(i.to_string())).collect()),
            );
        }
        UndoTarget::Group(g) => {
            m.insert("undoGroupId".into(), json!(g.to_string()));
        }
    }
    if force {
        m.insert("force".into(), json!(true));
    }
    Ok(Value::Object(m))
}

// ---------------------------------------------------------------- calls

impl Feed<'_> {
    /// Accepts a selection or every row one rule staged, in one engine
    /// transaction. A retry of the same accept reuses `key`; the password
    /// retry of the locked rows takes a **fresh** key.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-accept-v1`, or without
    /// `feed-accept-draft-v1` for an item with a receipt (nothing sent);
    /// [`accept_body`]'s checks; 409 `RULE_CHANGED` / `RULE_NOT_AUTO_ACCEPT`
    /// and 404 `RULE_NOT_FOUND` ([`crate::feed_refusal`]); a wrong password;
    /// any [`ApiError`].
    pub async fn accept(
        &self,
        req: &AcceptRequest,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<AcceptOutcome, ApiError> {
        self.accept_in(req, None, auth, key).await
    }

    /// [`Feed::accept`] with the owner's transfer window (1-10 days) for
    /// the engine's transfer-twin hold (`transferWindowDays`,
    /// `feed-guards-v1`, U-200); left out on an engine without it.
    ///
    /// # Errors
    /// As [`Feed::accept`]; `Local(InvalidValue)` for a window outside 1-10.
    pub async fn accept_in(
        &self,
        req: &AcceptRequest,
        transfer_window_days: Option<u8>,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<AcceptOutcome, ApiError> {
        self.need(Capabilities::FEED_ACCEPT)?;
        if transfer_window_days.is_some_and(|d| !(1..=10).contains(&d)) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let window = transfer_window_days.filter(|_| self.has(Capabilities::FEED_GUARDS));
        if let AcceptRequest::Items(items) = req
            && items.iter().any(|i| i.draft().is_some())
        {
            self.need(Capabilities::FEED_ACCEPT_DRAFT)?;
            if items.iter().any(|i| {
                i.draft()
                    .is_some_and(|d| !d.acknowledged_duplicates.is_empty())
            }) {
                self.need(Capabilities::FEED_ACCEPT_DRAFT_DUPLICATES)?;
            }
        }
        let mut body = accept_body(req)?;
        if let (Some(d), Value::Object(m)) = (window, &mut body) {
            m.insert("transferWindowDays".into(), json!(d));
        }
        let body = self.with_password(body, auth)?;
        let mut call = Call::at(Method::Post, FEED_ACCEPT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_ACCEPT, call, None).await?;
        accept_outcome_from(&reply.body)
    }

    /// An accept's answer again (a replay over the 512 KB cap).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-accept-v1`; `NotFound`; any
    /// [`ApiError`].
    pub async fn accept_group(&self, id: UndoGroupId) -> Result<AcceptOutcome, ApiError> {
        self.need(Capabilities::FEED_ACCEPT)?;
        let call = Call::new(
            Method::Get,
            FEED_ACCEPT_GROUP.route(),
            format!("/api/v1/banking/feed/accept-groups/{id}"),
        );
        let reply = self.0.send(&FEED_ACCEPT_GROUP, call, None).await?;
        accept_outcome_from(&reply.body)
    }

    /// Takes accepted rows or a whole accept group back. `force` after the
    /// owner confirmed a `FEED_UNDO_CHANGED`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `feed-undo-v1`; [`undo_body`]'s checks;
    /// 409 `FEED_UNDO_CHANGED {ids}` ([`crate::FeedRefusal::UndoChanged`]);
    /// `NotFound` for an unknown group; any [`ApiError`].
    pub async fn undo(
        &self,
        target: &UndoTarget,
        force: bool,
        auth: Option<&ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<UndoOutcome, ApiError> {
        self.need(Capabilities::FEED_UNDO)?;
        let body = self.with_password(undo_body(target, force)?, auth)?;
        let mut call = Call::at(Method::Post, FEED_UNDO.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&FEED_UNDO, call, None).await?;
        undo_outcome_from(&reply.body)
    }
}

#[cfg(test)]
#[path = "feed_accept_tests.rs"]
mod tests;
