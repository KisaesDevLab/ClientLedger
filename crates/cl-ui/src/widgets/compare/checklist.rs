//! The check list (A04 D4, S15): every target with the value the form
//! shows, its label, how sure the reading was in words, a snippet of the
//! page where it was read, and "Looks right" / "Find on page". The viewer
//! draws it in place of the page while it is open (docked or popped out).
//!
//! Snippets are parts of textures the viewer already drew (the page shown
//! and its thumbnails; `overlay::snippet`), so a field on a page not drawn
//! yet shows none until "Find on page" turns there. Every button names its
//! field for screen readers and the control channel ("Looks right: Amount
//! due"). No floats here (`cargo xtask nofloat`).

use eframe::egui::{self, RichText, Role, Ui, WidgetInfo};

use super::overlay::{self, Overlay, SnippetSource};
use super::{CompareEvent, TargetLevel};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;

/// The words for a level.
#[must_use]
pub fn level_words(level: TargetLevel) -> String {
    t(match level {
        TargetLevel::Sure => "compare.level.sure",
        TargetLevel::CheckThis => "compare.level.check_this",
        TargetLevel::NotFound => "compare.level.not_found",
    })
}

/// Draws the list for `overlay`'s view; `snippet_of(page)` is a texture
/// the viewer drew of that page, if any. Returns the page to turn to
/// ("Find on page"; the list then closes).
pub fn show(
    ui: &mut Ui,
    overlay: &mut Overlay,
    snippet_of: &dyn Fn(u16) -> Option<SnippetSource>,
    tokens: &Tokens,
) -> Option<u16> {
    let view = overlay.view()?.clone();
    let mut turn = None;
    egui::ScrollArea::vertical()
        .id_salt("compare-checklist")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if view.feed.targets.is_empty() {
                ui.label(RichText::new(t("compare.checklist.none")).color(tokens.text_secondary));
            }
            for (i, target) in view.feed.targets.iter().enumerate() {
                let value = view
                    .values
                    .get(i)
                    .filter(|v| !v.trim().is_empty())
                    .cloned()
                    .unwrap_or_else(|| t("compare.checklist.empty_value"));
                let colour = match target.level {
                    TargetLevel::Sure => tokens.accent,
                    TargetLevel::CheckThis => tokens.warn,
                    TargetLevel::NotFound => tokens.text_secondary,
                };
                ui.horizontal(|ui| {
                    let source = target
                        .spot
                        .and_then(|s| snippet_of(s.page).map(|src| (s, src)));
                    match source {
                        Some((spot, src)) => {
                            let name = t_with("compare.snippet", &[("field", &target.label)]);
                            overlay::snippet(ui, &src, spot, &name);
                        }
                        None => overlay::snippet_space(ui),
                    }
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(&target.label)
                                .family(crate::fonts::bold())
                                .color(tokens.text_primary),
                        );
                        ui.label(RichText::new(&value).color(tokens.text_primary));
                        ui.label(RichText::new(level_words(target.level)).color(colour));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if target.spot.is_some() {
                            let name = t_with("compare.find.named", &[("field", &target.label)]);
                            let r = ui
                                .button(t("compare.find"))
                                .on_hover_text(t("compare.find.tip"));
                            r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
                            if r.clicked() {
                                turn = overlay.find(target.key);
                                overlay.push(CompareEvent::TargetClicked(target.key));
                            }
                        }
                        if target.level != TargetLevel::Sure {
                            let name =
                                t_with("compare.looks_right.named", &[("field", &target.label)]);
                            let r = ui
                                .button(t("compare.looks_right"))
                                .on_hover_text(t("compare.looks_right.tip"));
                            r.widget_info(|| WidgetInfo::labeled(Role::Button, true, &name));
                            if r.clicked() {
                                overlay.push(CompareEvent::LooksRight(target.key));
                            }
                        }
                    });
                });
                ui.separator();
            }
        });
    turn
}
