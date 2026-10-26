//! The one document pane every attachment site draws (A02, plan D4).
//!
//! [`DocPane::show`]`(ui, &mut ViewerState, &Tokens) -> ViewerEvent`:
//! - Docked: the viewer with "Pop out" (Ctrl+Shift+D). A click asks the
//!   shell to move the document to its window and answers
//!   [`ViewerEvent::PopOut`]; the site need not do anything with it.
//! - Popped out: a one-line placeholder with "Dock"; the form's own viewer
//!   lets go of its page texture (the window shows its own copy). A click
//!   answers [`ViewerEvent::Dock`], also handled by the shell.
//! - F6 with the viewer docked moves the keyboard between the form and this
//!   pane (the form widget it left gets it back).
//!
//! The site reports the same document through
//! [`crate::windows::Screen::document`], so the window follows it.
//! `OpenFullSize` and `SaveCopy` come back as before for the site to handle.

use eframe::egui::{Id, RichText, Ui};

use super::pdf_viewer::{DocViewer, ViewerEvent, ViewerState};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::windows::doc_viewer::{self, PaneRequest};

/// The pane.
pub struct DocPane;

fn form_focus_id() -> Id {
    Id::unique("cl-doc-pane-form-focus")
}

impl DocPane {
    /// Draw the pane for `state` (every frame the form shows it).
    pub fn show(ui: &mut Ui, state: &mut ViewerState, tokens: &Tokens) -> ViewerEvent {
        let ctx = ui.ctx().clone();
        if doc_viewer::take_focus_toggle(&ctx) {
            if state.has_focus() {
                let back = ctx.data_mut(|d| {
                    let id = d.get_temp::<Id>(form_focus_id());
                    d.remove::<Id>(form_focus_id());
                    id
                });
                ctx.memory_mut(|m| match back {
                    Some(id) => m.request_focus(id),
                    // Nothing had the keys before: the viewer just lets go.
                    None => {
                        if let Some(f) = m.focused() {
                            m.surrender_focus(f);
                        }
                    }
                });
            } else {
                if let Some(id) = ctx.memory(eframe::egui::Memory::focused) {
                    ctx.data_mut(|d| d.insert_temp(form_focus_id(), id));
                }
                state.focus();
            }
        }
        if doc_viewer::is_popped(&ctx) && state.source().is_some() {
            // The window draws its own copy; this one keeps no pixels.
            state.release_pixels();
            let mut event = ViewerEvent::None;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(t("docpane.popped")).color(tokens.text_secondary));
                if ui.button(t("docviewer.dock")).clicked() {
                    doc_viewer::request(&ctx, PaneRequest::Dock);
                    event = ViewerEvent::Dock;
                }
            });
            return event;
        }
        state.set_pop_out_button(true);
        let event = DocViewer::show(ui, state, tokens);
        if event == ViewerEvent::PopOut {
            doc_viewer::request(&ctx, PaneRequest::PopOut);
        }
        event
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use eframe::egui::{self, Context, RawInput};

    use super::*;
    use crate::widgets::pdf_viewer::DocSource;

    fn png() -> DocSource {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/docs/receipt.png");
        DocSource {
            name: "receipt.png".into(),
            bytes: Arc::from(std::fs::read(path).unwrap_or_default()),
        }
    }

    fn run(ctx: &Context, state: &mut ViewerState) -> (ViewerEvent, Vec<String>) {
        let mut event = ViewerEvent::None;
        let mut out = ctx.run_ui(RawInput::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                event = DocPane::show(ui, state, &Tokens::of(ui.ctx()));
            });
        });
        out.textures_delta.clear();
        let names = out
            .platform_output
            .accesskit_update
            .map(|u| {
                u.nodes
                    .iter()
                    .filter_map(|(_, n)| n.label().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        (event, names)
    }

    #[test]
    fn docked_offers_pop_out_and_popped_shows_dock() {
        let ctx = Context::default();
        ctx.enable_accesskit();
        let mut state = ViewerState::default();
        state.open(png());
        let (_, names) = run(&ctx, &mut state);
        assert!(names.iter().any(|n| n == "Pop out"), "{names:?}");
        ctx.data_mut(|d| d.insert_temp(Id::unique("cl-doc-viewer-popped"), true));
        assert!(doc_viewer::is_popped(&ctx));
        let (_, names) = run(&ctx, &mut state);
        assert!(names.iter().any(|n| n == "Dock"), "{names:?}");
        assert!(!names.iter().any(|n| n == "Pop out"), "{names:?}");
        assert_eq!(state.state_json()["rendered"], false, "no pixels kept");
        assert!(state.source().is_some(), "the form keeps its document");
    }

    #[test]
    fn f6_moves_the_keyboard_into_the_pane() {
        let ctx = Context::default();
        let mut state = ViewerState::default();
        state.open(png());
        for _ in 0..400 {
            if state.state_json()["rendered"] == true {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            let _ = run(&ctx, &mut state);
        }
        doc_viewer::ask_focus_toggle(&ctx, egui::ViewportId::ROOT);
        let _ = run(&ctx, &mut state);
        let _ = run(&ctx, &mut state);
        assert!(state.has_focus(), "{:?}", state.state_json());
    }
}
