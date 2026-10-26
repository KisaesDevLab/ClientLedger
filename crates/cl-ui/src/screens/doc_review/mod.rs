//! Review Documents (A04 D6, `vendors.review_documents`): the
//! company-wide queue of documents waiting to be entered. Drafts open in
//! the real forms in draft mode; Accept is the form's Save & Close or Save
//! & New (Q-A04b), so nothing posts from this list.
//!
//! Scaffold (WP-M). Who builds what:
//! - [`queue`]: the list, its keys and `state.queue` (WP-F1, step 11).
//! - [`review`]: draft → form → convert, Page Up/Down through the queue,
//!   learning, and the control channel's review digests (WP-F2, steps 12
//!   and 14).
//! - [`attach`]: attach to an existing transaction (WP-F2, step 13).
//!
//! Frozen here: [`KIND`], [`make`], the window's [`Scope::List`] while its
//! list has the keyboard (J/K/Enter/Delete, Q-A04c), its
//! [`Screen::keeps_text_key`] (only a focused text field keeps Delete, not
//! a focused row) and [`queue_state`].

pub mod attach;
pub mod queue;
pub mod review;

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::{self, RichText, Ui};
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::theme::Tokens;
use crate::windows::{Screen, Windows};

/// The command and window kind.
pub const KIND: &str = "vendors.review_documents";

/// The Review Documents window for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    (kind.as_str() == KIND)
        .then(|| Box::new(DocReviewScreen::new(Arc::clone(link), open, today)) as Box<_>)
}

/// The Review Documents window.
pub struct DocReviewScreen {
    /// The open company.
    #[expect(dead_code, reason = "WP-F1 (step 11) loads the queue through it")]
    pub(crate) link: Arc<CompanyLink>,
    /// The company's tenant and name, when known.
    #[expect(dead_code, reason = "WP-F1 (step 11) keys the queue by company")]
    pub(crate) tenant: Option<(TenantId, String)>,
    /// The owner's today.
    #[expect(dead_code, reason = "WP-F2 (step 12) opens drafts with it")]
    pub(crate) today: CalendarDate,
    /// The list (WP-F1).
    pub(crate) queue: queue::QueueView,
}

impl std::fmt::Debug for DocReviewScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocReviewScreen")
            .field("queue", &self.queue)
            .finish_non_exhaustive()
    }
}

impl DocReviewScreen {
    /// A queue for the open company.
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        tenant: Option<(TenantId, String)>,
        today: CalendarDate,
    ) -> DocReviewScreen {
        DocReviewScreen {
            link,
            tenant,
            today,
            queue: queue::QueueView::default(),
        }
    }
}

impl Screen for DocReviewScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        ui.add_space(8.0);
        ui.heading(self.title());
        ui.add_space(6.0);
        if self.queue.rows().is_empty() {
            ui.label(RichText::new(t("doc_review.empty")).color(tokens.text_secondary));
        }
    }

    /// The list's keys while the window has focus (WP-F1 narrows this to
    /// "while the list has the keyboard"); the shell never runs J/K/Enter
    /// while a text field such as the filter has focus (S8).
    fn scope(&self) -> Scope {
        Scope::List
    }

    fn command(&mut self, id: CommandId) -> bool {
        self.queue.command(id)
    }

    fn keeps_text_key(&self, ctx: &egui::Context, s: cl_model::Shortcut) -> bool {
        let _ = s;
        // A focused row is not a text field: Delete there discards
        // (`queue.discard`), as Delete deletes in other lists.
        ctx.text_edit_focused()
    }

    fn state(&self) -> Value {
        json!({ "queue": self.queue.state() })
    }
}

/// `state.queue`: the Review Documents window's queue, whether or not it is
/// in front ([`queue::QueueView::state`]); `null` when it isn't open.
#[must_use]
pub fn queue_state(windows: &Windows) -> Value {
    windows
        .all()
        .iter()
        .find(|w| w.kind.as_str() == KIND)
        .map_or(Value::Null, |w| {
            w.screen
                .state()
                .get("queue")
                .cloned()
                .unwrap_or(Value::Null)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link() -> (tempfile::TempDir, Arc<CompanyLink>) {
        let dir = tempfile::tempdir().expect("tmp");
        let link = Arc::new(CompanyLink::demo(dir.path()));
        (dir, link)
    }

    fn today() -> CalendarDate {
        CalendarDate::from_ymd(2026, 11, 4).expect("date")
    }

    #[test]
    fn opens_its_own_kind_only_with_the_list_scope() {
        let (_dir, link) = link();
        let cmd = cl_model::commands::find(KIND).expect("a command");
        assert!(cmd.opens && cmd.arrives.is_none());
        let mut s = make(cmd.id, &link, None, today()).expect("its screen");
        assert_eq!(s.title(), "Review Documents");
        assert_eq!(s.scope(), Scope::List);
        assert!(!s.dirty());
        let other = cl_model::commands::find("vendors.write_check").expect("cmd");
        assert!(make(other.id, &link, None, today()).is_none());
        // A built screen reports its state.
        assert_eq!(s.state()["queue"]["rows"], json!([]));
        assert_eq!(s.state()["queue"]["selected"], Value::Null);
        // J/K on an empty list do nothing and say so.
        assert!(!s.command(CommandId::new("queue.next")));
        assert!(!s.command(CommandId::new("form.save_close")));
    }

    #[test]
    fn the_queue_state_follows_the_window_wherever_it_is() {
        let (_dir, link) = link();
        let mut windows = Windows::default();
        assert_eq!(queue_state(&windows), Value::Null);
        let kind = CommandId::new(KIND);
        let screen = make(kind, &link, None, today()).expect("screen");
        windows.open_new(kind, screen);
        windows.open_new(
            CommandId::new("vendors.enter_bill"),
            Box::new(crate::windows::Placeholder::for_command(CommandId::new(
                "vendors.enter_bill",
            ))),
        );
        assert_eq!(queue_state(&windows)["total"], 0);
    }

    #[test]
    fn the_empty_queue_draws_its_note() {
        let (_dir, link) = link();
        let mut s = DocReviewScreen::new(link, None, today());
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| s.ui(ui));
        // Nothing paints here; egui insists the texture updates are consumed.
        out.textures_delta.clear();
        assert_eq!(s.queue.rows().len(), 0);
        assert!(format!("{s:?}").contains("DocReviewScreen"));
    }
}
