//! The document viewer (P17, plan §3 WP1): one widget for the Enter Bill
//! source-document card, its full-size tab (`screens::document_view`), the
//! receipt inbox preview and Write Check's attachment preview.
//!
//! # Handoff (WP0 skeleton, WP1 fills this file)
//!
//! Fixed by WP0, called by WP4, WP5 and the full-size tab (WP1 may add, not
//! change):
//! - [`DocViewer::show`]`(ui, &mut ViewerState, &Tokens) -> ViewerEvent`,
//!   drawn every frame the preview is visible.
//! - [`ViewerState`] (`Default` = nothing open): [`ViewerState::open`],
//!   [`ViewerState::clear`], [`ViewerState::source`], [`ViewerState::page`]
//!   (from 0), [`ViewerState::page_count`], [`ViewerState::zoom`],
//!   [`ViewerState::has_focus`] (the screen then reports
//!   `Scope::Viewer`, so the `viewer.*` keys reach it),
//!   [`ViewerState::set_full_size_button`], [`ViewerState::command`] (runs
//!   `viewer.zoom_in`, `viewer.zoom_out`, `viewer.fit`, `viewer.page_next`,
//!   `viewer.page_prev`; returns true when handled) and
//!   [`ViewerState::state_json`] (no file names).
//! - [`ViewerEvent`]: `OpenFullSize` (the screen pushes
//!   `CompanyEvent::OpenWith(nav.document_view, OpenPayload::Document(..))`)
//!   and `SaveCopy` (the screen offers Save a copy, P13).
//! - [`DocSource`]: the file name (owner data: never logged, `Debug`
//!   redacted) and its bytes.
//!
//! WP1 adds the worker plumbing ([`super::doc_worker`]), textures (freed on
//! drop), Fit width / Fit page / 25–400 % zoom (UI geometry, `f32`, not
//! money), page navigation and the AccessKit labels ("Page 1 of 1", "Zoom
//! in", "Zoom out", "Fit", "Previous page", "Next page", "Open full size";
//! the page image is role img "Preview of <file name>"). Strings go in
//! `i18n/viewer.rs`.

use std::sync::Arc;

use cl_model::CommandId;
use eframe::egui::Ui;
use serde_json::{Value, json};

use crate::theme::Tokens;

/// A document to show.
#[derive(Clone, PartialEq, Eq)]
pub struct DocSource {
    /// The file name as the owner knows it (never logged).
    pub name: String,
    /// The file's bytes.
    pub bytes: Arc<[u8]>,
}

/// The size only: the name is owner data.
impl std::fmt::Debug for DocSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocSource")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// How the page is sized.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Zoom {
    /// The page fills the width.
    #[default]
    FitWidth,
    /// The whole page shows.
    FitPage,
    /// A fixed size, 25–400 %.
    Percent(u16),
}

/// What the viewer asks its screen to do this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewerEvent {
    /// Nothing.
    #[default]
    None,
    /// "Open full size": show the document in its own tab.
    OpenFullSize,
    /// "Save a copy" (the preview is unavailable or the PDF is protected).
    SaveCopy,
}

/// One viewer's state.
#[derive(Debug, Default)]
pub struct ViewerState {
    source: Option<DocSource>,
    page: u32,
    pages: u32,
    zoom: Zoom,
    focused: bool,
    full_size_button: bool,
}

impl ViewerState {
    /// Show `source` from its first page.
    pub fn open(&mut self, source: DocSource) {
        *self = ViewerState {
            source: Some(source),
            full_size_button: self.full_size_button,
            ..ViewerState::default()
        };
    }

    /// Show nothing.
    pub fn clear(&mut self) {
        *self = ViewerState {
            full_size_button: self.full_size_button,
            ..ViewerState::default()
        };
    }

    /// The document shown.
    #[must_use]
    pub fn source(&self) -> Option<&DocSource> {
        self.source.as_ref()
    }

    /// The page shown, from 0.
    #[must_use]
    pub fn page(&self) -> u32 {
        self.page
    }

    /// Pages known (0 until the worker opened the document).
    #[must_use]
    pub fn page_count(&self) -> u32 {
        self.pages
    }

    /// The zoom setting.
    #[must_use]
    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    /// True while the keyboard is in the viewer (last frame).
    #[must_use]
    pub fn has_focus(&self) -> bool {
        self.focused
    }

    /// Offer "Open full size" (the card does; the full-size tab does not).
    pub fn set_full_size_button(&mut self, on: bool) {
        self.full_size_button = on;
    }

    /// Run a `viewer.*` command. Returns true when handled.
    pub fn command(&mut self, id: CommandId) -> bool {
        // Skeleton (WP1 replaces): nothing to zoom or turn yet.
        let _ = id;
        false
    }

    /// The control channel's view (no file names).
    #[must_use]
    pub fn state_json(&self) -> Value {
        json!({
            "open": self.source.is_some(),
            "page": self.page,
            "pages": self.pages,
            "focused": self.focused,
        })
    }
}

/// The viewer widget.
pub struct DocViewer;

impl DocViewer {
    /// Draw the viewer for `state`.
    pub fn show(ui: &mut Ui, state: &mut ViewerState, tokens: &Tokens) -> ViewerEvent {
        // Skeleton (WP1 replaces): draws nothing yet.
        let _ = (ui, tokens);
        state.focused = false;
        ViewerEvent::None
    }
}

#[cfg(test)]
mod skeleton_tests {
    use super::*;

    #[test]
    fn open_and_clear_keep_the_button_choice() {
        let mut v = ViewerState::default();
        v.set_full_size_button(true);
        v.open(DocSource {
            name: "invoice.pdf".into(),
            bytes: Arc::from(&b"%PDF-1.7"[..]),
        });
        assert!(v.source().is_some() && v.full_size_button);
        assert!(!format!("{:?}", v.source()).contains("invoice"));
        v.clear();
        assert!(v.source().is_none() && v.full_size_button);
        assert_eq!(v.state_json()["open"], false);
    }
}
