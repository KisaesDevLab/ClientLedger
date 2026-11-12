//! The Report Builder editor's state (ADD-03 A08, plan §3.1): the
//! definition being edited, the memorized row it came from, where it
//! started, the selection, undo and redo, and the problems to show.
//!
//! No egui, no I/O. Every change is one operation that pushes an undo
//! snapshot and re-validates; the engine computes every figure.
//!
//! The seam commit declares the types other code constructs ([`Origin`],
//! [`Saved`], [`SavedKind`]); WP-A1 adds `BuilderEditor` and its
//! operations.

use cl_core::Timestamp;

use super::BuilderDefinition;

/// Where an editor's report started (the hint's "Started from ..." link).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A blank report in Advanced.
    Blank,
    /// Guided mode's questions (Q-A08a).
    Guided,
    /// A ready-made report (`/starters` key).
    Starter(String),
    /// Customize on a standard report: its catalog key (P28), opened on
    /// its mapped starter (Q-A08g).
    Standard(String),
    /// A memorized Report Builder report.
    Memorized,
}

/// The kind of the memorized row an editor came from. The engine never
/// changes a row's kind (PUT refuses), so Save changes works only on
/// `Builder` rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavedKind {
    /// A Report Builder row (`kind: 'builder'`).
    Builder,
    /// A memorized standard report (`kind: 'standard'`): Customize opens
    /// its starter, and only Memorize saves (a new row).
    Standard,
}

/// The memorized row behind an editor, as last read or saved.
#[derive(Clone, PartialEq, Eq)]
pub struct Saved {
    /// The row's id.
    pub id: String,
    /// Its version, sent with the next change (409 when stale).
    pub version: u64,
    /// Its kind.
    pub kind: SavedKind,
    /// True when this user may change it (the engine's `canEdit`: the
    /// author, an owner or a super admin, Q-310).
    pub can_edit: bool,
    /// The definition as saved (dirty compares with it). For a
    /// [`SavedKind::Standard`] row, whose body is not a builder definition,
    /// it is the mapped starter's definition the editor opened on (with the
    /// row's dates and basis applied), so an untouched Customize is not
    /// dirty; Save changes stays off for that kind.
    pub def: BuilderDefinition,
    /// The memorized name (owner data).
    pub name: String,
    /// Its memorized-report group.
    pub group_id: Option<String>,
    /// Who memorized it (owner data).
    pub author: Option<String>,
    /// Last change (the hint's "Saved hh:mm").
    pub updated_at: Option<Timestamp>,
}

// Names and captions are owner data: Debug shows only the row's identity.
impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("id", &self.id)
            .field("version", &self.version)
            .field("kind", &self.kind)
            .field("can_edit", &self.can_edit)
            .finish_non_exhaustive()
    }
}
