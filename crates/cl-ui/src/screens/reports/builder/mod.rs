//! The Report Builder editor (ADD-03 A08, plan §3.8; mockup
//! `docs/design/mockups/ReportBuilder.html`): one window per report, with
//! the outline, the row/columns/report properties and a live preview the
//! engine runs. Guided mode asks five questions instead (Q-A08a).
//!
//! The UI adds up nothing: every figure in the preview is the engine's.
//!
//! The seam commit declares what other screens hand the editor
//! ([`BuilderSeed`], [`open`]); WP-A2 builds the screen.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cl_core::CalendarDate;
use cl_core::report::{Adjusting, Basis};
use cl_model::CommandId;
use cl_model::report_def::BuilderDefinition;
use cl_model::report_def::editor::{Origin, Saved};
use cl_model::reports::DateChoice;
use eframe::egui::Ui;

use crate::company::{CompanyEvent, CompanyLink};
use crate::i18n::t;
use crate::windows::{OpenPayload, Screen};

/// The editor's window kind.
pub const KIND: &str = "reports.builder_editor";

/// One editor window's identity: its result tab and a Customize from that
/// tab find it by this (Q-A08x).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EditorId(u64);

/// Hands out [`EditorId`]s.
static EDITORS: AtomicU64 = AtomicU64::new(1);

impl EditorId {
    /// A new editor's id.
    #[must_use]
    pub(crate) fn next() -> EditorId {
        EditorId(EDITORS.fetch_add(1, Ordering::Relaxed))
    }
}

/// What the editor opens on. `Debug` shows no captions or names (owner
/// data).
#[derive(Clone, PartialEq, Eq)]
pub enum BuilderSeed {
    /// Guided mode, blank (Q-A08a; Reports list "New report...").
    Guided,
    /// A blank report in Advanced.
    Blank,
    /// A ready-made report (`/starters`), in Advanced.
    Starter {
        /// Its key.
        key: String,
        /// Its definition.
        def: Box<BuilderDefinition>,
        /// Its title (the Report name when the definition has none).
        title: String,
    },
    /// Customize on a report window (Q-A08g, Q-A08k).
    Customize(Box<CustomizeSeed>),
}

/// Customize on a standard report (its mapped starter), a builder run or a
/// memorized report.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomizeSeed {
    /// The definition to edit.
    pub def: BuilderDefinition,
    /// The Report name to start with (owner data): the memorized name, else
    /// the window's report title. The editor makes it the definition's
    /// title too (Q-A08r).
    pub title: String,
    /// Where it started.
    pub origin: Origin,
    /// The memorized row, as re-read for this action (Q-A07u(1)).
    pub saved: Option<Saved>,
    /// The window's dates (a preset, or session-only fixed dates).
    pub dates: Option<DateChoice>,
    /// The window's basis (`None`: the definition's).
    pub basis: Option<Basis>,
    /// The window's adjusting lens (`None`: the definition's).
    pub adjusting: Option<Adjusting>,
    /// The window had filters, which Customize doesn't carry: the editor
    /// says so (Q-A08k).
    pub filters_dropped: bool,
    /// The editor whose result tab this is: it takes the seed back in
    /// place (Q-A08x). `None`: a new editor.
    pub editor: Option<EditorId>,
}

impl std::fmt::Debug for BuilderSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderSeed::Guided => f.write_str("Guided"),
            BuilderSeed::Blank => f.write_str("Blank"),
            BuilderSeed::Starter { key, .. } => f
                .debug_struct("Starter")
                .field("key", key)
                .finish_non_exhaustive(),
            BuilderSeed::Customize(c) => f
                .debug_struct("Customize")
                .field("origin", &c.origin)
                .field("saved", &c.saved)
                .field("editor", &c.editor)
                .finish_non_exhaustive(),
        }
    }
}

/// Opens the editor on `seed` (a new window, or the editor named in a
/// Customize seed).
#[allow(
    dead_code,
    reason = "A08 seam: WP-B3's Customize and WP-A2's New report call it"
)]
pub(crate) fn open(link: &CompanyLink, seed: BuilderSeed) {
    link.push(CompanyEvent::OpenWith(
        CommandId::new(KIND),
        Box::new(OpenPayload::BuilderEditor(Box::new(seed))),
    ));
}

/// The editor window.
pub struct BuilderScreen {
    #[allow(dead_code, reason = "A08 seam: WP-A2 runs the preview through it")]
    link: Arc<CompanyLink>,
    #[allow(dead_code, reason = "A08 seam: WP-A2 resolves dates with it")]
    today: CalendarDate,
    /// The open company (header name; the key of its fiscal calendar,
    /// `window::company_calendar`).
    #[allow(dead_code, reason = "A08 seam: WP-A2 reads the fiscal year with it")]
    open: Option<(cl_core::ids::TenantId, String)>,
    id: EditorId,
    seed: Option<BuilderSeed>,
}

impl BuilderScreen {
    /// A new editor (Guided until a payload says otherwise).
    #[must_use]
    pub fn new(
        link: Arc<CompanyLink>,
        open: Option<(cl_core::ids::TenantId, String)>,
        today: CalendarDate,
    ) -> BuilderScreen {
        BuilderScreen {
            link,
            today,
            open,
            id: EditorId::next(),
            seed: None,
        }
    }
}

impl Screen for BuilderScreen {
    fn title(&self) -> String {
        t("builder.title")
    }

    fn ui(&mut self, ui: &mut Ui) {
        ui.label(t("builder.title"));
    }

    fn open_with(&mut self, payload: OpenPayload) -> bool {
        match payload {
            OpenPayload::BuilderEditor(seed) => {
                self.seed = Some(*seed);
                true
            }
            _ => false,
        }
    }

    fn state(&self) -> serde_json::Value {
        serde_json::json!({
            "screen": "report_builder",
            "editor": self.id.0,
        })
    }

    fn takes_in_place(&self, payload: &OpenPayload) -> bool {
        matches!(
            payload,
            OpenPayload::BuilderEditor(seed)
                if matches!(&**seed, BuilderSeed::Customize(c) if c.editor == Some(self.id))
        )
    }
}

impl std::fmt::Debug for BuilderScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuilderScreen")
            .field("id", &self.id)
            .field("seed", &self.seed)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
