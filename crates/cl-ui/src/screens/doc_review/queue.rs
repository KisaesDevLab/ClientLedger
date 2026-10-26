//! The Review Documents list (A04 D6). WP-F1 (step 11) builds it: pages
//! of [`cl_model::doc_draft::QUEUE_PAGE`] from the engine's draft list
//! (`status=new,ready,failed`), status in words in each row's AccessKit
//! label, lazy duplicate checks, the filter, Read all with its progress,
//! Discard, and Home's count.
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
//! amount, a name, a reference or a file name.

use cl_api::DocumentDraftId;
use cl_model::CommandId;
use cl_model::doc_draft::{DuplicateCheck, QueueCursor, QueueRow, Step};
use serde_json::{Value, json};

use crate::reading::queue::BatchProgress;

/// The list's rows and selection.
#[derive(Debug, Clone, Default)]
pub struct QueueView {
    /// The rows shown, newest first.
    pub(crate) rows: Vec<QueueRow<DocumentDraftId>>,
    /// The selection, by draft id.
    pub(crate) cursor: QueueCursor<DocumentDraftId>,
    /// The engine's `total` of live drafts (Home's count).
    pub(crate) total: u64,
    /// Read all's progress, while it runs.
    pub(crate) reading: Option<BatchProgress>,
}

impl QueueView {
    /// The rows shown.
    #[must_use]
    pub fn rows(&self) -> &[QueueRow<DocumentDraftId>] {
        &self.rows
    }

    /// The selected draft.
    #[must_use]
    pub fn selected(&self) -> Option<DocumentDraftId> {
        self.cursor.selected().copied()
    }

    /// The ids in row order.
    #[must_use]
    pub fn ids(&self) -> Vec<DocumentDraftId> {
        self.rows.iter().map(|r| r.id).collect()
    }

    /// A new page of rows: the selection stays on the same draft
    /// ([`QueueCursor::refresh`]).
    pub fn set_rows(&mut self, rows: Vec<QueueRow<DocumentDraftId>>, total: u64) {
        self.rows = rows;
        self.total = total;
        let ids = self.ids();
        self.cursor.refresh(&ids);
    }

    /// The list's commands. J/K move the selection here; `queue.open`,
    /// `queue.discard` and `queue.read_all` are WP-F1's. Returns whether
    /// the command was handled (false on an empty list).
    pub fn command(&mut self, id: CommandId) -> bool {
        let step = match id.as_str() {
            "queue.next" => Step::Next,
            "queue.prev" => Step::Prev,
            _ => return false,
        };
        let ids = self.ids();
        self.cursor.step(&ids, step).is_some()
    }

    /// `state.queue` (see the module comment).
    #[must_use]
    pub fn state(&self) -> Value {
        let ids = self.ids();
        let selected = self.cursor.position(&ids);
        let rows: Vec<Value> = self
            .rows
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
            "rows": rows,
        })
    }
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

#[cfg(test)]
mod tests {
    use cl_model::doc_draft::{DocKind, DraftState};

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
        assert!(!q.command(CommandId::new("queue.open")), "WP-F1's");
        let s = q.state();
        assert_eq!(s["total"], 3);
        assert_eq!(s["selected"], 1);
        assert_eq!(s["reading"], Value::Null);
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
}
