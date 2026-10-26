//! The compare tools over the document viewer (A04 D4, Q-291; A06 reuses
//! them): highlights both ways between a form's fields and their spots,
//! click to fill, drag a box, the check list. WP-C builds them here
//! (`overlay.rs` owns all float geometry and hit-testing, `fill.rs` the
//! one-amount rule, `checklist.rs` the list); the model types are
//! [`cl_model::doc_draft`]'s.
//!
//! WP-M froze the control channel's view of them: whoever draws the
//! highlights (the docked pane or the pop-out) calls [`publish_highlight`]
//! each frame it draws, and `state.doc_viewer.highlight` reports
//! [`highlight`]. Field names and levels only: never a value, a word or a
//! file name.

use eframe::egui;
use serde::Serialize;

pub use cl_model::doc_draft::{
    CompareFeed, CompareTarget, FillArm, FillKind, PageBox, TargetKey, TargetLevel,
};

/// What the viewer's highlights show now (`state.doc_viewer.highlight`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HighlightState {
    /// Highlights are on (`viewer.toggle_highlights`).
    pub on: bool,
    /// Targets drawn on the page shown.
    pub drawn: usize,
    /// The outlined field (the form's focused target), by stable name.
    pub focused: Option<String>,
    /// The page shown, from 0.
    pub page: Option<u16>,
    /// The outlined box's level name (`sure`, `check_this`, `not_found`).
    pub level: Option<&'static str>,
    /// The armed field (what a click on a word fills), by stable name.
    pub armed: Option<String>,
}

impl HighlightState {
    /// The control channel's JSON.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

fn highlight_id() -> egui::Id {
    egui::Id::unique("a04/compare/highlight")
}

/// Records what the highlights show (the viewer that draws them, each
/// frame it draws).
pub fn publish_highlight(ctx: &egui::Context, state: HighlightState) {
    ctx.data_mut(|d| d.insert_temp(highlight_id(), state));
}

/// What the highlights showed last; the default (off, nothing drawn) until
/// a viewer published.
#[must_use]
pub fn highlight(ctx: &egui::Context) -> HighlightState {
    ctx.data(|d| d.get_temp::<HighlightState>(highlight_id()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_published_highlight_is_reported() {
        let ctx = egui::Context::default();
        assert_eq!(highlight(&ctx), HighlightState::default());
        assert_eq!(highlight(&ctx).to_json()["on"], false);
        let s = HighlightState {
            on: true,
            drawn: 4,
            focused: Some("amount_due".into()),
            page: Some(0),
            level: Some(TargetLevel::CheckThis.name()),
            armed: Some("amount_due".into()),
        };
        publish_highlight(&ctx, s.clone());
        assert_eq!(highlight(&ctx), s);
        let v = highlight(&ctx).to_json();
        assert_eq!(v["level"], "check_this");
        assert_eq!(v["drawn"], 4);
    }
}
