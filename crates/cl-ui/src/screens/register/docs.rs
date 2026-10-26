//! The selected transaction's documents (A02, Q-A02f): the register lists
//! the files attached to the transaction under the cursor
//! (`GET /attachments` by its type and id), "Show document" opens the first
//! one in the pop-out viewer, and while the viewer is out it follows the
//! cursor ([`crate::windows::Screen::document`]). There is no docked pane
//! (the grid has the room) and no shortcut. Nothing is written.

use std::sync::Arc;

use cl_api::{AttachTo, Attachment, AttachmentId};
use cl_core::ids::TransactionId;
use eframe::egui::{self, Ui};
use serde_json::{Value, json};

use super::RegisterScreen;
use crate::company::Task;
use crate::i18n::{t, t_with};
use crate::widgets::pdf_viewer::DocSource;
use crate::windows::doc_viewer::{self, DocRef, PaneRequest};

/// What the register knows about the cursor row's files.
#[derive(Default)]
pub(super) struct RowDocs {
    /// The transaction they are for.
    pub txn: Option<TransactionId>,
    list_task: Option<Task<Vec<Attachment>>>,
    /// The files, once listed.
    pub list: Option<Vec<Attachment>>,
    bytes_task: Option<Task<(AttachmentId, String, Vec<u8>)>>,
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
        })
    }
}

impl RegisterScreen {
    /// The transaction under the cursor (`None` while it is loading).
    fn docs_row(&self) -> Option<(cl_api::TxnType, TransactionId)> {
        #[cfg(test)]
        if let Some(key) = self.actions.cursor {
            return self.model.row_for_key(key).map(|r| (r.txn_type, r.txn_id));
        }
        let key = self.grid.core().cursor()?.row;
        self.model.row_for_key(key).map(|r| (r.txn_type, r.txn_id))
    }

    /// Each frame: list the files of a newly selected transaction, fetch
    /// the first one's bytes while the viewer is out (or Show document
    /// asked), and pop the viewer out once they are here.
    pub(super) fn sync_docs(&mut self, ctx: &egui::Context) {
        let row = self.docs_row();
        if row.map(|(_, id)| id) != self.docs.txn {
            self.docs = RowDocs {
                txn: row.map(|(_, id)| id),
                ..RowDocs::default()
            };
            if let Some((kind, id)) = row {
                self.list_docs(AttachTo::Txn(kind, id));
            }
        }
        if let Some(r) = self.docs.list_task.as_mut().and_then(Task::try_take) {
            self.docs.list_task = None;
            self.docs.list = Some(r.unwrap_or_default());
        }
        let wanted = self.docs.pop || doc_viewer::is_popped(ctx);
        if wanted
            && self.docs.source.is_none()
            && self.docs.bytes_task.is_none()
            && let Some(first) = self.docs.list.as_ref().and_then(|l| l.first())
        {
            let (id, name) = (first.id, first.file_name.clone());
            self.fetch_doc(id, name);
        }
        if let Some(r) = self.docs.bytes_task.as_mut().and_then(Task::try_take) {
            self.docs.bytes_task = None;
            if let Ok((_, name, bytes)) = r {
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

    fn list_docs(&mut self, of: AttachTo) {
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let list = crate::company::demo::checks::attachments(&demo, of);
            drop(demo);
            self.docs.list = Some(list);
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            self.docs.list = Some(Vec::new());
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            let page = cl_api::PageRequest::new(50, 0)?;
            Ok(s.attachments().list(of, page).await?.data)
        }) {
            self.docs.list_task = Some(Task::Pending(p));
        }
    }

    fn fetch_doc(&mut self, id: AttachmentId, name: String) {
        let link = Arc::clone(&self.link);
        if let Some(demo) = link.demo_state() {
            let r = crate::company::demo::checks::download(&demo, id);
            drop(demo);
            if let Ok(bytes) = r {
                self.docs.bytes_task = Some(Task::ready((id, name, bytes)));
            }
            return;
        }
        let (Some(session), Some(group)) = (self.link.session(), self.group.as_ref()) else {
            return;
        };
        if let Ok(p) = group.read(&session, move |s| async move {
            Ok((id, name, s.attachments().download(id).await?))
        }) {
            self.docs.bytes_task = Some(Task::Pending(p));
        }
    }

    /// "Show document" (the toolbar): on when the selected transaction has
    /// files.
    pub(super) fn docs_button(&mut self, ui: &mut Ui) {
        let n = self.docs.count();
        let r = ui
            .add_enabled(n > 0, egui::Button::new(t("register.doc.show")))
            .on_hover_text(if n > 0 {
                format!(
                    "{} ({})",
                    t("register.doc.show.tip"),
                    t_with("register.doc.count", &[("n", &n.to_string())])
                )
            } else {
                t("register.doc.show.tip")
            });
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
