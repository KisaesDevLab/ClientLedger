//! Status bar: who is signed in, the mode, the engine state, the AI line, and
//! context keyboard hints read from the keymap.

use cl_model::commands;
use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui};

use super::{HostView, Shell, chip_text};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::kbd;

/// Status bar height.
pub const HEIGHT: f32 = 26.0;

/// Commands whose shortcut is hinted on the right, with their short labels.
pub const HINTS: [(&str, &str); 4] = [
    ("help.palette", "status.hint.go_to"),
    ("window.next", "status.hint.next_window"),
    ("help.shortcuts", "status.hint.shortcuts"),
    ("help.current", "status.hint.help"),
];

/// The engine text.
#[must_use]
pub fn engine_text(view: &HostView) -> Option<String> {
    match view {
        HostView::Unavailable => None,
        HostView::Starting => Some(t("status.engine.starting")),
        HostView::Running => Some(t("status.engine.running")),
        HostView::Restarting => Some(t("status.engine.restarting")),
        HostView::Failed(reason) => Some(format!("{} {reason}", t("status.engine.failed"))),
        HostView::Stopped => Some(t("status.engine.stopped")),
    }
}

/// Draw the bar.
pub fn show(ui: &mut Ui, shell: &Shell, narrow: bool, tokens: &Tokens) {
    let engine = engine_text(&shell.host_view());
    egui::Panel::bottom("status_bar")
        .exact_size(HEIGHT)
        .frame(
            Frame::new()
                .fill(tokens.surface_panel)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                let small = |s: String| RichText::new(s).size(12.0).color(tokens.text_secondary);
                let user = shell
                    .status
                    .user
                    .clone()
                    .unwrap_or_else(|| t("status.not_signed_in"));
                ui.label(small(user));
                if let Some(mode) = &shell.status.mode {
                    ui.label(small(mode.clone()));
                }
                if let Some(engine) = engine {
                    ui.label(small(engine));
                }
                if let Some(ai) = &shell.status.ai
                    && !narrow
                {
                    ui.label(small(ai.clone()));
                }
                if let Some(toast) = shell.toast_text() {
                    ui.label(
                        RichText::new(toast)
                            .size(12.0)
                            .color(tokens.accent)
                            .strong(),
                    );
                }
                if !narrow {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (id, label) in HINTS.iter().rev() {
                            ui.label(small(t(label)));
                            if let Some(s) = commands::find(id).and_then(|c| c.shortcut) {
                                kbd(ui, &chip_text(s));
                            }
                            ui.add_space(8.0);
                        }
                    });
                }
            });
        });
}
