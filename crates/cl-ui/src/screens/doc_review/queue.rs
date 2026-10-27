//! The Review Documents list (A04 D6, WP-F1 step 11): pages of
//! [`QUEUE_PAGE`] from the engine's draft list (`status=new,ready,failed`),
//! status in words in each row's AccessKit label, lazy duplicate checks
//! (two at a time, cached by draft id and change token), the filter, Read
//! all's progress, and the keys J/K (move), Enter (open) and Delete
//! (discard, confirmed).
//!
//! Frozen by WP-M: [`QueueView`]'s selection model
//! ([`cl_model::doc_draft::QueueCursor`] by draft id, so J/K stay on the
//! same draft across a refresh) and the shape of [`QueueView::state`], the
//! control channel's `state.queue`:
//!
//! ```json
//! { "total": 3, "selected": 0, "reading": {"done": 1, "total": 4},
//!   "rows": [ { "kind": "bill", "status": "ready",
//!               "duplicate": "clear", "selected": true } ] }
//! ```
//!
//! `selected` is the selected row's index or `null`; `reading` is `null`
//! when no batch is reading. Kinds, statuses and indexes only: never an
//! amount, a name, a reference or a file name. What a row shows the owner
//! (date, reference, amount) lives in [`RowView`], whose `Debug` says
//! nothing, and never reaches the state.

use std::collections::HashMap;

use cl_api::AttachmentId;
use cl_api::{
    ApiError, DocumentDraft, DocumentDraftId, DraftKind, DraftListQuery, DraftPage, DraftStatus,
    DuplicateMatch, LocalValidation, MatchSource,
};
use cl_core::{CalendarDate, Money};
use cl_model::CommandId;
use cl_model::doc_draft::{
    DUPLICATE_CHECKS_AT_ONCE, DocKind, DraftState, DuplicateCheck, QUEUE_PAGE, QueueCursor,
    QueueRow, Step,
};
use serde_json::{Value, json};

use crate::i18n::t;
use crate::reading::queue::BatchProgress;

/// The live statuses the queue lists.
pub const QUEUE_STATUSES: [DraftStatus; 3] =
    [DraftStatus::New, DraftStatus::Ready, DraftStatus::Failed];

/// One queue page from `offset`, with each draft's reading (the status
/// needs the read fields' confidence).
#[must_use]
pub fn page_query(offset: u32) -> DraftListQuery {
    DraftListQuery {
        statuses: QUEUE_STATUSES.to_vec(),
        kind: None,
        limit: QUEUE_PAGE,
        offset,
        summary: false,
    }
}

/// Home's count (D6): the engine's `total` of live drafts, one summary row.
#[must_use]
pub fn count_query() -> DraftListQuery {
    DraftListQuery {
        statuses: QUEUE_STATUSES.to_vec(),
        kind: None,
        limit: 1,
        offset: 0,
        summary: true,
    }
}

/// The engine's `total` from a count read; an engine without document
/// drafts has none to review (0, so Home shows no line).
///
/// # Errors
/// Any other [`ApiError`].
pub fn count_of(r: Result<DraftPage, ApiError>) -> Result<u64, ApiError> {
    match r {
        Ok(p) => Ok(p.total),
        Err(ApiError::Local(LocalValidation::Unsupported)) => Ok(0),
        Err(e) => Err(e),
    }
}

/// The model's kind for the engine's.
#[must_use]
pub const fn doc_kind(k: DraftKind) -> DocKind {
    match k {
        DraftKind::Bill => DocKind::Bill,
        DraftKind::CardCharge => DocKind::CardCharge,
        DraftKind::Check => DocKind::Check,
        DraftKind::CashExpense => DocKind::CashExpense,
        DraftKind::Unknown => DocKind::Unknown,
    }
}

/// A live draft's queue state; `None` for converted and discarded ones.
#[must_use]
pub const fn draft_state(s: DraftStatus) -> Option<DraftState> {
    match s {
        DraftStatus::New => Some(DraftState::New),
        DraftStatus::Ready => Some(DraftState::Ready),
        DraftStatus::Failed => Some(DraftState::Failed),
        DraftStatus::Converted | DraftStatus::Discarded => None,
    }
}

/// "Check these fields": the stored reading has a check for the owner, or
/// a field the form fills (vendor, date, reference, due date, total or
/// balance due) read below sure. A reading this version can't read asks
/// for a look too.
#[must_use]
pub fn stored_needs_check(d: &DocumentDraft) -> bool {
    if d.checks
        .as_ref()
        .and_then(Value::as_array)
        .is_some_and(|c| !c.is_empty())
    {
        return true;
    }
    let Some(fields) = d.fields.clone() else {
        return d.status == DraftStatus::Ready;
    };
    let Ok(h) = serde_json::from_value::<cl_docread::model::Header>(fields) else {
        return true;
    };
    let low = |c: Option<cl_docread::model::Confidence>| {
        c.is_some_and(|c| cl_model::doc_draft::needs_check(crate::reading::map::level(c)))
    };
    low(h.vendor.as_ref().map(|f| f.confidence))
        || low(h.date.as_ref().map(|f| f.confidence))
        || low(h.reference.as_ref().map(|f| f.confidence))
        || low(h.due_date.as_ref().map(|f| f.confidence))
        || low(h.total.as_ref().map(|f| f.confidence))
        || low(h.balance_due.as_ref().map(|f| f.confidence))
}

/// A queue row for a live draft (`None` for a closed one).
#[must_use]
pub fn row_of(d: &DocumentDraft) -> Option<QueueRow<DocumentDraftId>> {
    let state = draft_state(d.status)?;
    Some(QueueRow {
        id: d.id,
        updated_at: d.updated_at.clone(),
        kind: doc_kind(d.kind),
        state,
        needs_check: state == DraftState::Ready && stored_needs_check(d),
        duplicate: DuplicateCheck::NotChecked,
    })
}

/// A duplicate check's answer: a posted record that may be this document
/// is "Possible duplicate"; other live drafts alone are not (convert shows
/// them and never blocks on them, D3); a failed check says nothing.
#[must_use]
pub fn duplicate_of(r: &Result<Vec<DuplicateMatch>, ApiError>) -> DuplicateCheck {
    match r {
        Ok(m)
            if m.iter()
                .any(|m| matches!(m.source, MatchSource::Transaction(_))) =>
        {
            DuplicateCheck::Possible
        }
        Ok(_) => DuplicateCheck::Clear,
        Err(_) => DuplicateCheck::Failed,
    }
}

/// What a row shows the owner: owner data, never in `state.queue`, a log
/// or `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct RowView {
    /// The document's date as read.
    pub date: Option<CalendarDate>,
    /// Its reference as read.
    pub reference: Option<String>,
    /// Its total as read.
    pub total: Option<Money>,
    /// The day it arrived (UI check item 5).
    pub arrived: Option<CalendarDate>,
    /// Its file (the name is fetched once, [`QueueView::names`]).
    pub file: Option<AttachmentId>,
}

impl std::fmt::Debug for RowView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowView").finish_non_exhaustive()
    }
}

impl RowView {
    fn of(d: &DocumentDraft) -> RowView {
        RowView {
            date: d.doc_date,
            reference: d.doc_reference.clone(),
            total: d.doc_total,
            arrived: d.created_at.and_then(|t| {
                let local = t.as_utc().with_timezone(&chrono::Local).date_naive();
                CalendarDate::from_naive(local).ok()
            }),
            file: d.attachment_id,
        }
    }
}

/// What the owner asked of the list with a key or a button; the window
/// carries it out (it has the egui context and the engine).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    /// Enter: open the draft in its form.
    Open(DocumentDraftId),
    /// Delete: ask, then discard.
    Discard(DocumentDraftId),
    /// Read all documents not read yet.
    ReadAll,
}

/// The list's rows and selection.
#[derive(Clone, Default)]
pub struct QueueView {
    /// The rows of this page, newest first.
    pub(crate) rows: Vec<QueueRow<DocumentDraftId>>,
    /// The selection, by draft id.
    pub(crate) cursor: QueueCursor<DocumentDraftId>,
    /// The engine's `total` of live drafts (Home's count).
    pub(crate) total: u64,
    /// Read all's progress, while it runs.
    pub(crate) reading: Option<BatchProgress>,
    /// What each row shows (owner data).
    pub(crate) views: HashMap<DocumentDraftId, RowView>,
    /// The filter as typed (owner text, never in the state).
    pub(crate) filter: String,
    /// File names by upload (owner data: on screen only, never in the
    /// state, a log or `Debug`).
    pub(crate) names: HashMap<AttachmentId, String>,
    /// Duplicate answers by draft and change token.
    dup_cache: HashMap<(DocumentDraftId, String), DuplicateCheck>,
    /// The last key or button the window hasn't carried out yet.
    pub(crate) action: Option<QueueAction>,
}

/// Counts and positions only: the filter is owner text.
impl std::fmt::Debug for QueueView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueueView")
            .field("rows", &self.rows.len())
            .field("selected", &self.cursor.position(&self.ids()))
            .field("total", &self.total)
            .field("reading", &self.reading)
            .field("filtered", &!self.filter.trim().is_empty())
            .field("action", &self.action)
            .finish_non_exhaustive()
    }
}

impl QueueView {
    /// The rows shown (the filter applied).
    #[must_use]
    pub fn rows(&self) -> Vec<&QueueRow<DocumentDraftId>> {
        self.rows.iter().filter(|r| self.shown(r)).collect()
    }

    /// Every row of the page, filtered or not.
    #[must_use]
    pub fn all_rows(&self) -> &[QueueRow<DocumentDraftId>] {
        &self.rows
    }

    /// The selected draft.
    #[must_use]
    pub fn selected(&self) -> Option<DocumentDraftId> {
        self.cursor.selected().copied()
    }

    /// The selected row.
    #[must_use]
    pub fn selected_row(&self) -> Option<&QueueRow<DocumentDraftId>> {
        let id = self.selected()?;
        self.rows.iter().find(|r| r.id == id)
    }

    /// The ids of the rows shown, in order.
    #[must_use]
    pub fn ids(&self) -> Vec<DocumentDraftId> {
        self.rows().iter().map(|r| r.id).collect()
    }

    /// A row's words for the filter: its kind, its status and its
    /// reference.
    fn shown(&self, r: &QueueRow<DocumentDraftId>) -> bool {
        let needle = self.filter.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let reference = self
            .views
            .get(&r.id)
            .and_then(|v| v.reference.as_deref())
            .unwrap_or_default();
        [
            kind_words(r.kind),
            t(r.status().key().0),
            reference.to_string(),
        ]
        .iter()
        .any(|w| w.to_lowercase().contains(&needle))
    }

    /// The filter changed: the selection stays when its row is still
    /// shown, else the first shown row is selected.
    pub fn set_filter(&mut self, text: &str) {
        self.filter = text.to_string();
        let ids = self.ids();
        if self.cursor.position(&ids).is_none() {
            self.cursor = QueueCursor::new();
        }
        self.cursor.refresh(&ids);
    }

    /// A new page of rows: the selection stays on the same draft
    /// ([`QueueCursor::refresh`]).
    pub fn set_rows(&mut self, rows: Vec<QueueRow<DocumentDraftId>>, total: u64) {
        self.rows = rows;
        for r in &mut self.rows {
            if let Some(d) = self.dup_cache.get(&(r.id, r.updated_at.clone())) {
                r.duplicate = *d;
            }
        }
        self.total = total;
        let ids = self.ids();
        self.cursor.refresh(&ids);
    }

    /// Uploads of this page whose file name isn't known yet.
    #[must_use]
    pub fn names_missing(&self) -> Vec<AttachmentId> {
        let mut out: Vec<AttachmentId> = self
            .views
            .values()
            .filter_map(|v| v.file)
            .filter(|a| !self.names.contains_key(a))
            .collect();
        out.sort_by_key(ToString::to_string);
        out.dedup();
        out
    }

    /// A page from the engine.
    pub fn set_page(&mut self, page: &DraftPage) {
        self.views = page.drafts.iter().map(|d| (d.id, RowView::of(d))).collect();
        let rows = page.drafts.iter().filter_map(row_of).collect();
        self.set_rows(rows, page.total);
    }

    /// The read rows to check for duplicates now, at most
    /// [`DUPLICATE_CHECKS_AT_ONCE`] with `running` already out; each is
    /// marked "Checking…".
    pub fn next_checks(&mut self, running: usize) -> Vec<(DocumentDraftId, String)> {
        let room = DUPLICATE_CHECKS_AT_ONCE.saturating_sub(running);
        let mut out = Vec::new();
        for r in &mut self.rows {
            if out.len() >= room {
                break;
            }
            if r.state == DraftState::Ready && r.duplicate == DuplicateCheck::NotChecked {
                r.duplicate = DuplicateCheck::Checking;
                out.push((r.id, r.updated_at.clone()));
            }
        }
        out
    }

    /// A duplicate check answered for `id` as it was at `updated_at`.
    pub fn duplicate_done(&mut self, id: DocumentDraftId, updated_at: &str, check: DuplicateCheck) {
        if check != DuplicateCheck::Failed {
            self.dup_cache.insert((id, updated_at.to_string()), check);
        }
        if let Some(r) = self
            .rows
            .iter_mut()
            .find(|r| r.id == id && r.updated_at == updated_at)
        {
            r.duplicate = check;
        }
    }

    /// Selects `id` (a click on its row).
    pub fn select(&mut self, id: DocumentDraftId) -> bool {
        let ids = self.ids();
        self.cursor.select(&ids, &id)
    }

    /// The list's commands: J/K move the selection; Enter, Delete and Read
    /// all are left in [`QueueView::action`] for the window. Returns
    /// whether the command was handled (false on an empty list).
    pub fn command(&mut self, id: CommandId) -> bool {
        let step = match id.as_str() {
            "queue.next" => Step::Next,
            "queue.prev" => Step::Prev,
            "queue.open" => return self.act(QueueAction::Open),
            "queue.discard" => return self.act(QueueAction::Discard),
            "queue.read_all" => {
                self.action = Some(QueueAction::ReadAll);
                return true;
            }
            _ => return false,
        };
        let ids = self.ids();
        self.cursor.step(&ids, step).is_some()
    }

    fn act(&mut self, f: fn(DocumentDraftId) -> QueueAction) -> bool {
        let ids = self.ids();
        match self.cursor.position(&ids).and_then(|at| ids.get(at)) {
            Some(id) => {
                self.action = Some(f(*id));
                true
            }
            None => false,
        }
    }

    /// `state.queue` (see the module comment).
    #[must_use]
    pub fn state(&self) -> Value {
        let ids = self.ids();
        let selected = self.cursor.position(&ids);
        let rows: Vec<Value> = self
            .rows()
            .iter()
            .enumerate()
            .map(|(i, r)| {
                json!({
                    "kind": r.kind.name(),
                    "status": r.status().name(),
                    "duplicate": duplicate_name(r.duplicate),
                    "selected": selected == Some(i),
                })
            })
            .collect();
        json!({
            "total": self.total,
            "selected": selected,
            "reading": self.reading.map(|p| json!({"done": p.done, "total": p.total})),
            "filtered": !self.filter.trim().is_empty(),
            "rows": rows,
        })
    }
}

/// A kind in words ("Bill", "Card charge").
#[must_use]
pub fn kind_words(k: DocKind) -> String {
    t(&format!("doc_review.kind.{}", k.name()))
}

/// A duplicate check's stable name (`state.queue`).
#[must_use]
pub fn duplicate_name(d: DuplicateCheck) -> &'static str {
    match d {
        DuplicateCheck::NotChecked => "not_checked",
        DuplicateCheck::Checking => "checking",
        DuplicateCheck::Clear => "clear",
        DuplicateCheck::Possible => "possible",
        DuplicateCheck::Failed => "failed",
    }
}

/// A row's AccessKit label: kind, place and status in words, e.g. "Bill,
/// document 2 of 6, Check these fields". No date, name or amount.
#[must_use]
pub fn row_label(r: &QueueRow<DocumentDraftId>, at: usize, of: u64) -> String {
    let mut status = t(r.status().key().0);
    if r.duplicate == DuplicateCheck::Checking {
        status = format!("{status}, {}", t("doc_review.checking"));
    }
    crate::i18n::t_with(
        "doc_review.row.label",
        &[
            ("kind", &kind_words(r.kind)),
            ("n", &(at + 1).to_string()),
            ("of", &of.to_string()),
            ("status", &status),
        ],
    )
}

#[cfg(test)]
mod tests {
    use cl_api::MatchReason;
    use cl_model::doc_draft::QueueStatus;

    use super::*;

    fn id(n: u8) -> DocumentDraftId {
        DocumentDraftId::parse(&format!("00000000-0000-4000-8000-0000000000{n:02}")).expect("id")
    }

    fn row(n: u8, state: DraftState, dup: DuplicateCheck) -> QueueRow<DocumentDraftId> {
        QueueRow {
            id: id(n),
            updated_at: format!("t{n}"),
            kind: DocKind::Bill,
            state,
            needs_check: false,
            duplicate: dup,
        }
    }

    fn draft(n: u8, status: &str, fields: Value, checks: Value) -> DocumentDraft {
        let mut v = json!({
            "id": id(n).to_string(),
            "kind": "bill",
            "status": status,
            "docReference": "INV-7",
            "docTotal": "12.50",
            "updatedAt": format!("t{n}"),
        });
        v["fields"] = fields;
        v["checks"] = checks;
        serde_json::from_value(v).expect("draft")
    }

    /// A read total as the engine stores it (the reader's own JSON).
    fn found(value: &str, confidence: u16) -> Value {
        use cl_docread::model::{Bbox, Confidence, Found, RuleId, Spot};
        let total = Money::parse_wire(value).expect("money");
        serde_json::to_value(Found {
            value: total,
            spot: Spot {
                page: 0,
                bbox: Bbox::new(10, 10, 20, 20).expect("bbox"),
            },
            rule: RuleId::new("test"),
            confidence: Confidence::new(confidence).expect("bp"),
        })
        .expect("json")
    }

    #[test]
    fn j_and_k_move_and_the_state_shows_words_only() {
        let mut q = QueueView::default();
        q.set_rows(
            vec![
                row(1, DraftState::Ready, DuplicateCheck::Clear),
                row(2, DraftState::Ready, DuplicateCheck::Possible),
                row(3, DraftState::Failed, DuplicateCheck::NotChecked),
            ],
            3,
        );
        assert_eq!(
            q.selected(),
            Some(id(1)),
            "a loaded list selects its first row"
        );
        assert!(q.command(CommandId::new("queue.next")));
        assert_eq!(q.selected(), Some(id(2)));
        assert!(q.command(CommandId::new("queue.open")));
        assert_eq!(q.action.take(), Some(QueueAction::Open(id(2))));
        assert!(q.command(CommandId::new("queue.discard")));
        assert_eq!(q.action.take(), Some(QueueAction::Discard(id(2))));
        assert!(q.command(CommandId::new("queue.read_all")));
        assert_eq!(q.action.take(), Some(QueueAction::ReadAll));
        assert!(!q.command(CommandId::new("form.save_close")));
        let s = q.state();
        assert_eq!(s["total"], 3);
        assert_eq!(s["selected"], 1);
        assert_eq!(s["reading"], Value::Null);
        assert_eq!(s["filtered"], false);
        assert_eq!(s["rows"][1]["status"], "possible_duplicate");
        assert_eq!(s["rows"][1]["selected"], true);
        assert_eq!(s["rows"][2]["status"], "couldnt_read");
        assert_eq!(s["rows"][0]["duplicate"], "clear");
        assert!(!s.to_string().contains("00000000"), "no ids either");
        // The selected draft was entered: the next one is selected.
        q.set_rows(
            vec![
                row(1, DraftState::Ready, DuplicateCheck::Clear),
                row(3, DraftState::Failed, DuplicateCheck::NotChecked),
            ],
            2,
        );
        assert_eq!(q.selected(), Some(id(3)));
        q.reading = Some(BatchProgress { done: 1, total: 4 });
        assert_eq!(q.state()["reading"]["total"], 4);
        for d in [
            DuplicateCheck::NotChecked,
            DuplicateCheck::Checking,
            DuplicateCheck::Failed,
        ] {
            assert_ne!(duplicate_name(d), "");
        }
    }

    #[test]
    fn an_empty_list_takes_no_keys() {
        let mut q = QueueView::default();
        for c in ["queue.next", "queue.prev", "queue.open", "queue.discard"] {
            assert!(!q.command(CommandId::new(c)), "{c}");
        }
        assert_eq!(q.action, None);
    }

    /// AC1: J/K stay on the same draft across a refresh that reorders and
    /// adds rows.
    #[test]
    fn the_selection_survives_a_refresh() {
        let mut q = QueueView::default();
        let r = |n| row(n, DraftState::Ready, DuplicateCheck::NotChecked);
        q.set_rows(vec![r(1), r(2), r(3)], 3);
        q.command(CommandId::new("queue.next"));
        q.command(CommandId::new("queue.next"));
        assert_eq!(q.selected(), Some(id(3)));
        q.set_rows(vec![r(4), r(3), r(1), r(2)], 4);
        assert_eq!(q.selected(), Some(id(3)));
        q.command(CommandId::new("queue.next"));
        assert_eq!(q.selected(), Some(id(1)));
        q.command(CommandId::new("queue.prev"));
        q.command(CommandId::new("queue.prev"));
        q.command(CommandId::new("queue.prev"));
        assert_eq!(q.selected(), Some(id(4)), "K stops at the top");
    }

    #[test]
    fn each_status_in_words() {
        let ready = draft(
            1,
            "ready",
            json!({"total": found("12.50", 9_500)}),
            json!([]),
        );
        assert!(!stored_needs_check(&ready));
        assert_eq!(row_of(&ready).expect("row").status(), QueueStatus::Ready);
        let unsure = draft(
            2,
            "ready",
            json!({"total": found("12.50", 7_000)}),
            json!([]),
        );
        assert_eq!(
            row_of(&unsure).expect("row").status(),
            QueueStatus::CheckFields
        );
        let checked = draft(3, "ready", json!({}), json!([{"kind": "totals_mismatch"}]));
        assert_eq!(
            row_of(&checked).expect("row").status(),
            QueueStatus::CheckFields
        );
        let odd = draft(4, "ready", json!({"surprise": 1}), json!([]));
        assert!(
            stored_needs_check(&odd),
            "an unknown reading asks for a look"
        );
        let failed = draft(5, "failed", Value::Null, Value::Null);
        assert_eq!(
            row_of(&failed).expect("row").status(),
            QueueStatus::CouldntRead
        );
        let new = draft(6, "new", Value::Null, Value::Null);
        assert_eq!(
            row_of(&new).expect("row").status(),
            QueueStatus::WaitingToBeRead
        );
        assert!(row_of(&draft(7, "converted", Value::Null, Value::Null)).is_none());
        assert!(row_of(&draft(8, "discarded", Value::Null, Value::Null)).is_none());
        // A possible duplicate wins over everything but a failed reading.
        let mut r = row_of(&unsure).expect("row");
        r.duplicate = DuplicateCheck::Possible;
        assert_eq!(r.status(), QueueStatus::PossibleDuplicate);
        for k in DocKind::ALL {
            assert!(crate::i18n::has(&format!("doc_review.kind.{}", k.name())));
        }
        assert_eq!(doc_kind(DraftKind::CashExpense), DocKind::CashExpense);
    }

    #[test]
    fn duplicates_two_at_a_time_cached_by_token() {
        let mut q = QueueView::default();
        let r = |n| row(n, DraftState::Ready, DuplicateCheck::NotChecked);
        let mut waiting = row(9, DraftState::New, DuplicateCheck::NotChecked);
        waiting.kind = DocKind::Unknown;
        q.set_rows(vec![waiting, r(1), r(2), r(3)], 4);
        let first = q.next_checks(0);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].0, id(1), "unread drafts are not checked");
        assert!(q.next_checks(2).is_empty(), "two at a time");
        assert_eq!(q.state()["rows"][1]["duplicate"], "checking");
        let txn =
            cl_core::ids::TransactionId::parse("00000000-0000-4000-8000-0000000000aa").expect("id");
        let possible = Ok(vec![DuplicateMatch {
            source: MatchSource::Transaction(txn),
            reason: MatchReason::Reference,
            txn_type: None,
            kind: None,
            date: None,
            total: None,
            reference: None,
        }]);
        assert_eq!(duplicate_of(&possible), DuplicateCheck::Possible);
        let draft_only = Ok(vec![DuplicateMatch {
            source: MatchSource::Draft(id(5)),
            reason: MatchReason::AmountDate,
            txn_type: None,
            kind: None,
            date: None,
            total: None,
            reference: None,
        }]);
        assert_eq!(duplicate_of(&draft_only), DuplicateCheck::Clear);
        assert_eq!(
            duplicate_of(&Err(ApiError::Internal)),
            DuplicateCheck::Failed
        );
        q.duplicate_done(id(1), "t1", DuplicateCheck::Possible);
        q.duplicate_done(id(2), "t2", DuplicateCheck::Failed);
        assert_eq!(q.next_checks(0).len(), 1, "the third, then nothing");
        // A refresh keeps answers for unchanged drafts; a changed token
        // asks again; a failed check is asked again next time.
        let mut changed = r(1);
        changed.updated_at = "t1b".into();
        q.set_rows(vec![r(1), changed.clone(), r(2)], 3);
        assert_eq!(q.rows[0].duplicate, DuplicateCheck::Possible);
        assert_eq!(q.rows[1].duplicate, DuplicateCheck::NotChecked);
        assert_eq!(q.rows[2].duplicate, DuplicateCheck::NotChecked);
    }

    /// T18 (model side): the filter narrows the rows and J/K walk the
    /// rows shown; the typed text never reaches the state.
    #[test]
    fn the_filter_narrows_rows_and_keys() {
        let mut q = QueueView::default();
        let page = DraftPage {
            drafts: vec![
                draft(1, "ready", json!({}), json!([])),
                draft(2, "failed", Value::Null, Value::Null),
                draft(3, "new", Value::Null, Value::Null),
            ],
            total: 3,
        };
        q.set_page(&page);
        assert_eq!(q.rows().len(), 3);
        q.set_filter("couldn");
        assert_eq!(q.ids(), vec![id(2)]);
        assert_eq!(q.selected(), Some(id(2)));
        assert!(!q.command(CommandId::new("queue.next")) || q.selected() == Some(id(2)));
        q.set_filter("inv-7");
        assert_eq!(q.rows().len(), 3, "the reference matches");
        q.set_filter("jk");
        assert_eq!(q.rows().len(), 0);
        assert_eq!(q.selected(), None);
        let s = q.state().to_string();
        assert!(!s.contains("jk") && !s.contains("INV") && !s.contains("12.5"));
        assert_eq!(q.state()["filtered"], true);
        q.set_filter("INV-7");
        let debug = format!("{q:?}");
        assert!(
            debug.contains("filtered: true") && !debug.contains("INV"),
            "{debug}"
        );
        q.set_filter("");
        assert_eq!(q.selected(), Some(id(1)));
        assert!(!format!("{q:?}").contains("INV-7"), "Debug says nothing");
    }

    #[test]
    fn row_labels_say_kind_place_and_status() {
        let mut r = row(1, DraftState::Ready, DuplicateCheck::Checking);
        r.needs_check = true;
        assert_eq!(
            row_label(&r, 1, 6),
            "Bill, document 2 of 6, Check these fields, checking for duplicates"
        );
    }

    #[test]
    fn counts_and_queries() {
        assert_eq!(
            count_of(Ok(DraftPage {
                drafts: vec![],
                total: 7
            }))
            .ok(),
            Some(7)
        );
        assert_eq!(
            count_of(Err(ApiError::Local(LocalValidation::Unsupported))).ok(),
            Some(0)
        );
        assert!(count_of(Err(ApiError::Internal)).is_err());
        assert!(count_query().summary && count_query().limit == 1);
        let p = page_query(50);
        assert_eq!((p.limit, p.offset, p.summary), (QUEUE_PAGE, 50, false));
        assert_eq!(p.statuses, QUEUE_STATUSES.to_vec());
    }

    /// UI check item 5: a page's file names are fetched once, shown on
    /// screen only, never in `Debug` or the state.
    #[test]
    fn file_names_are_fetched_once_and_kept_out_of_the_state() {
        let a = AttachmentId::parse("a0000000-0000-4000-8000-000000000001").expect("id");
        let id = DocumentDraftId::parse("d0000000-0000-4000-8000-dd0000000001").expect("id");
        let mut q = QueueView::default();
        q.views.insert(
            id,
            RowView {
                file: Some(a),
                ..RowView::default()
            },
        );
        assert_eq!(q.names_missing(), vec![a]);
        q.names.insert(a, "acme-invoice-0412.pdf".into());
        assert_eq!(q.names_missing(), Vec::<AttachmentId>::new());
        assert!(!format!("{q:?}").contains("acme"));
        assert!(!format!("{:?}", q.views[&id]).contains("acme"));
    }
}
