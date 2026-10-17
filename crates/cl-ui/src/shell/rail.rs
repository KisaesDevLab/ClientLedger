//! The left rail: Workspace, Tools, and the list of open windows
//! (DESIGN.md window anatomy item 3). Accountant tools are post-beta and have
//! no entry.

use cl_model::{CommandId, commands};
use eframe::egui::{self, Align, Frame, Layout, Margin, RichText, Ui, UiBuilder, Vec2};

use super::{RailCounts, Shell, chip_text};
use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::{badge, eyebrow, kbd, paint_dot, row_button};

/// Rail width in logical points (DESIGN.md: 200 to 220).
pub const WIDTH: f32 = 212.0;
const ROW: f32 = 30.0;

/// Draw the rail.
pub fn show(ui: &mut Ui, shell: &mut Shell, tokens: &Tokens) {
    let active_kind = shell.windows.active().map(|w| w.kind);
    let counts = shell.counts;
    let active_id = shell.windows.active_id();
    let open: Vec<(crate::windows::WindowId, String, bool)> = shell
        .windows
        .all()
        .iter()
        .map(|w| (w.id, w.title(), w.dirty()))
        .collect();
    let mut queued: Vec<CommandId> = Vec::new();
    let mut activate = None;
    egui::Panel::left("rail")
        .exact_size(WIDTH)
        .resizable(false)
        .frame(
            Frame::new()
                .fill(tokens.surface_panel)
                .inner_margin(Margin::symmetric(8, 12)),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 2.0);
            section(ui, "shell.rail.workspace", 0.0);
            for id in cl_model::commands::ALL.rail_workspace {
                entry(ui, id, active_kind, counts, &mut queued);
            }
            section(ui, "shell.rail.tools", 14.0);
            for id in cl_model::commands::ALL.rail_tools {
                entry(ui, id, active_kind, counts, &mut queued);
            }
            section(ui, "shell.rail.open_windows", 14.0);
            for (id, title, dirty) in &open {
                let active = active_id == Some(*id);
                let (resp, rect) = row_button(ui, title, &title_label(title, *dirty), active, ROW);
                if *dirty {
                    paint_dot(
                        ui.painter(),
                        egui::pos2(rect.right() - 12.0, rect.center().y),
                        4.0,
                        if active {
                            tokens.on_accent
                        } else {
                            tokens.text_primary
                        },
                    );
                }
                if resp.clicked() {
                    activate = Some(*id);
                }
            }
        });
    for id in queued {
        shell.queue(id);
    }
    if let Some(id) = activate {
        shell.windows.activate(id);
    }
}

fn title_label(title: &str, dirty: bool) -> String {
    if dirty {
        crate::i18n::t_with("tabs.label_dirty", &[("title", title)])
    } else {
        title.to_string()
    }
}

fn section(ui: &mut Ui, key: &str, top_space: f32) {
    ui.add_space(top_space);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        eyebrow(ui, &t(key));
    });
    ui.add_space(4.0);
}

fn entry(
    ui: &mut Ui,
    id: &str,
    active_kind: Option<CommandId>,
    counts: RailCounts,
    queued: &mut Vec<CommandId>,
) {
    let Some(cmd) = commands::find(id) else {
        return;
    };
    let tokens = Tokens::of(ui.ctx());
    let selected = active_kind == Some(cmd.id);
    let label = t(&format!("cmd.{}", cmd.id));
    let (resp, rect) = row_button(ui, &label, &label, selected, ROW);
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(8.0, 2.0)))
            .layout(Layout::right_to_left(Align::Center)),
    );
    let count = match id {
        "clients.center" => counts.clients,
        "vendors.center" => counts.vendors,
        _ => None,
    };
    if id == "banking.feed" {
        if let Some(n) = counts.bank_feed {
            badge(&mut child, n);
        }
    } else if let Some(n) = count {
        child.label(
            RichText::new(n.to_string())
                .family(egui::FontFamily::Monospace)
                .size(12.0)
                .color(if selected {
                    tokens.on_accent
                } else {
                    tokens.text_secondary
                }),
        );
    } else if let Some(s) = cmd.shortcut {
        kbd(&mut child, &chip_text(s));
    }
    if resp.clicked() {
        queued.push(cmd.id);
    }
}
