//! The selected transaction's documents (A02, Q-A02f): the register lists
//! the files attached to the transaction under the cursor
//! (`GET /attachments` by its type and id), "Show document" opens the first
//! one in the pop-out viewer, and while the viewer is out it follows the
//! cursor ([`crate::windows::Screen::document`]). There is no docked pane
//! (the grid has the room) and no shortcut. Nothing is written.
//!
//! Review S3: the list is asked for once the cursor has rested on a row for
//! [`REST`] (Show document asks at once), and at most one list and one
//! download are in flight; an answer for a row the cursor has left is
//! dropped and the row under the cursor is asked for next. A list that
//! failed says so instead of looking like "no documents".

use std::sync::Arc;

use cl_api::{AttachTo, Attachment, AttachmentId, TxnType};
use cl_core::ids::TransactionId;
use eframe::egui::{self, Ui};
use serde_json::{Value, json};

use super::RegisterScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::widgets::pdf_viewer::DocSource;
use crate::windows::doc_viewer::{self, DocRef, PaneRequest};

/// How long the cursor rests on a row before its files are listed.
pub(super) const REST: f64 = 0.25;

/// A downloaded file: its id, name and bytes.
type Download = (AttachmentId, String, Vec<u8>);

/// What the register knows about the cursor row's files.
#[derive(Default)]
pub(super) struct RowDocs {
    /// The transaction they are for.
    pub txn: Option<TransactionId>,
    /// When the cursor came to it (egui seconds).
    since: f64,
    /// The list in flight, and the row it is for.
    list_task: Option<(TransactionId, Task<Vec<Attachment>>)>,
    /// The files, once listed.
    pub list: Option<Vec<Attachment>>,
    /// The list failed: shown, never taken for "no documents".
    pub list_failed: bool,
    /// The download in flight, and the row it is for.
    bytes_task: Option<(TransactionId, Task<Download>)>,
    /// The first file's bytes (what the viewer shows).
    source: Option<DocSource>,
    /// "Show document" was clicked: pop out once the bytes are here.
    pop: bool,
}

impl RowDocs {
    pub(super) fn busy(&self) -> bool {
        self.list_task.is_some() || self.bytes_task.is_some()
    }

    fn count(&self) -> usize {
        self.list.as_ref().map_or(0, Vec::len)
    }

    /// The control channel's view (no file names).
    pub(super) fn state_json(&self) -> Value {
        json!({
            "row": self.txn.is_some(),
            "listed": self.list.is_some(),
            "count": self.count(),
            "loaded": self.source.is_some(),
            "list_failed": self.list_failed,
        })
    }
}

impl RegisterScreen {
    /// The transaction under the cursor (`None` while it is loading).
    fn docs_row(&self) -> Option<(TxnType, TransactionId)> {
        self.cursor_row().map(|r| (r.txn_type, r.txn_id))
    }

    /// Each frame: list the files of the row the cursor rests on, fetch
    /// the first one's bytes while the viewer is out (or Show document
    /// asked), and pop the viewer out once they are here.
    pub(super) fn sync_docs(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let row = self.docs_row();
        if row.map(|(_, id)| id) != self.docs.txn {
            // Answers in flight finish; they are dropped when they come.
            let list_task = self.docs.list_task.take();
            let bytes_task = self.docs.bytes_task.take();
            self.docs = RowDocs {
                txn: row.map(|(_, id)| id),
                since: now,
                list_task,
                bytes_task,
                ..RowDocs::default()
            };
        }
        if let Some((for_id, task)) = self.docs.list_task.as_mut()
            && let Some(r) = task.try_take()
        {
            let for_id = *for_id;
            self.docs.list_task = None;
            if self.docs.txn == Some(for_id) {
                match r {
                    Ok(list) => self.docs.list = Some(list),
                    Err(_) => self.docs.list_failed = true,
                }
            }
        }
        if self.docs.list.is_none()
            && !self.docs.list_failed
            && self.docs.list_task.is_none()
            && let Some((kind, id)) = row
        {
            let waited = now - self.docs.since;
            if self.docs.pop || waited >= REST {
                self.list_docs(id, AttachTo::Txn(kind, id));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(REST - waited));
            }
        }
        let wanted = self.docs.pop || doc_viewer::is_popped(ctx);
        if wanted
            && self.docs.source.is_none()
            && self.docs.bytes_task.is_none()
            && let Some(id) = self.docs.txn
            && let Some(first) = self.docs.list.as_ref().and_then(|l| l.first())
        {
            let (file, name) = (first.id, first.file_name.clone());
            self.fetch_doc(id, file, name);
        }
        if let Some((for_id, task)) = self.docs.bytes_task.as_mut()
            && let Some(r) = task.try_take()
        {
            let for_id = *for_id;
            self.docs.bytes_task = None;
            if self.docs.txn == Some(for_id)
                && let Ok((_, name, bytes)) = r
            {
                self.docs.source = Some(DocSource {
                    name,
                    bytes: Arc::from(bytes),
                });
            }
        }
        if self.docs.pop && self.docs.source.is_some() {
            self.docs.pop = false;
            if !doc_viewer::is_popped(ctx) {
                doc_viewer::request(ctx, PaneRequest::PopOut);
            }
        }
        if self.docs.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    fn list_docs(&mut self, row: TransactionId, of: AttachTo) {
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let list = crate::company::demo::checks::attachments(&demo, of);
            drop(demo);
            self.docs.list = Some(list);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.docs.list_failed = true;
            return;
        };
        match group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(50, 0)?;
            Ok(s.attachments().list(of, page).await?.data)
        }) {
            Ok(p) => self.docs.list_task = Some((row, Task::Pending(p))),
            Err(_) => self.docs.list_failed = true,
        }
    }

    fn fetch_doc(&mut self, row: TransactionId, id: AttachmentId, name: String) {
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::checks::download(&demo, id);
            drop(demo);
            if let Ok(bytes) = r {
                self.docs.bytes_task = Some((row, Task::ready((id, name, bytes))));
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok((id, name, s.attachments().download(id).await?))
        }) {
            self.docs.bytes_task = Some((row, Task::Pending(p)));
        }
    }

    /// Tests: a list answer in flight for `row`.
    #[cfg(test)]
    pub(super) fn docs_list_in_flight(&mut self, row: TransactionId, task: Task<Vec<Attachment>>) {
        self.docs.list_task = Some((row, task));
    }

    /// "Show document" (the toolbar): on when the selected transaction has
    /// files; a list that failed says so in its tooltip.
    pub(super) fn docs_button(&mut self, ui: &mut Ui) {
        let n = self.docs.count();
        let tip = if self.docs.list_failed {
            t("register.doc.list_failed")
        } else if n > 0 {
            format!(
                "{} ({})",
                t("register.doc.show.tip"),
                t_with("register.doc.count", &[("n", &n.to_string())])
            )
        } else {
            t("register.doc.show.tip")
        };
        let r = ui
            .add_enabled(n > 0, egui::Button::new(t("register.doc.show")))
            .on_hover_text(tip.clone())
            .on_disabled_hover_text(tip);
        if r.clicked() {
            self.show_document(ui.ctx());
        }
    }

    /// Show the selected transaction's first document in its own window.
    pub(super) fn show_document(&mut self, ctx: &egui::Context) {
        if self.docs.count() == 0 {
            return;
        }
        self.docs.pop = true;
        self.sync_docs(ctx);
    }

    /// What the pop-out viewer follows.
    pub(super) fn row_document(&self) -> Option<DocRef> {
        self.docs.source.clone().map(DocRef::new)
    }
}
