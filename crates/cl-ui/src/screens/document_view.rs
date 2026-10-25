//! A document in its own tab (P17, `nav.document_view`, plan §3 WP1,
//! Q-P17l): "Open full size" from a preview, with zoom and page controls.
//!
//! # Handoff (WP0 skeleton, WP1 fills this file)
//!
//! A preview's screen pushes `CompanyEvent::OpenWith(nav.document_view,
//! OpenPayload::Document(source))`; [`DocumentViewScreen::open_with`] takes
//! it. The tab reports `Scope::Viewer` while the viewer has the keyboard,
//! so the `viewer.*` keys reach [`ViewerState::command`]. Opened bare (the
//! palette, a restored layout) it shows the viewer's empty state. WP1 adds
//! the layout and the empty-state text (`i18n/viewer.rs`).

use std::sync::Arc;

use cl_core::CalendarDate;
use cl_core::ids::TenantId;
use cl_model::{CommandId, Scope};
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::company::CompanyLink;
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::pdf_viewer::{DocViewer, ViewerState};
use crate::windows::{OpenPayload, Screen};

/// The command and window kind.
pub const KIND: &str = "nav.document_view";

/// The document tab for `kind`, if it is one.
#[must_use]
pub fn make(
    kind: CommandId,
    link: &Arc<CompanyLink>,
    open: Option<(TenantId, String)>,
    today: CalendarDate,
) -> Option<Box<dyn Screen>> {
    let _ = (link, open, today);
    (kind.as_str() == KIND).then(|| Box::new(DocumentViewScreen::default()) as Box<_>)
}

/// The document tab.
#[derive(Debug, Default)]
pub struct DocumentViewScreen {
    viewer: ViewerState,
}

impl Screen for DocumentViewScreen {
    fn title(&self) -> String {
        t(&format!("cmd.{KIND}"))
    }

    fn ui(&mut self, ui: &mut Ui) {
        let tokens = Tokens::of(ui.ctx());
        let _ = DocViewer::show(ui, &mut self.viewer, &tokens);
    }

    fn scope(&self) -> Scope {
        if self.viewer.has_focus() {
            Scope::Viewer
        } else {
            Scope::Window
        }
    }

    fn command(&mut self, id: CommandId) -> bool {
        self.viewer.command(id)
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::Document(source) => {
                self.viewer.open(source);
                true
            }
            OpenPayload::BillPrefill(_) | OpenPayload::InboxDraft(_) => false,
        }
    }

    fn state(&self) -> Value {
        json!({ "screen": "document_view", "viewer": self.viewer.state_json() })
    }
}

#[cfg(test)]
mod skeleton_tests {
    use super::*;
    use crate::widgets::pdf_viewer::DocSource;

    #[test]
    fn takes_a_document_and_nothing_else() {
        let mut s = DocumentViewScreen::default();
        assert_eq!(s.scope(), Scope::Window);
        assert_eq!(s.state()["viewer"]["open"], false);
        assert!(s.open_with(OpenPayload::Document(DocSource {
            name: "a.pdf".into(),
            bytes: Arc::from(&b"%PDF-1.7"[..]),
        })));
        assert_eq!(s.state()["viewer"]["open"], true);
        assert_eq!(s.title(), "Document Preview");
    }
}
