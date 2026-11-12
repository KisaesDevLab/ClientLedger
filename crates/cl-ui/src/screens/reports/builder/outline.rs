//! The outline tree (plan §3.8): S/G/=/T/␣/⤓ icons, sub-captions ("by
//! account", "Income − COGS"), "N of 200 rows", the Add menu, in-place
//! rename (F2) and the warning tint on rows with a problem.
//!
//! Each row is a tree item in the accessibility tree; the selected row
//! has the keyboard while the tree does, so the outline's keys (Insert,
//! Delete, Tab, F2, arrows) act on it (`Scope::Outline`).

use cl_model::report_def::editor::{Op, RowKind};
use cl_model::report_def::validate::limits;
use cl_model::report_def::{Node, outline as tree};
use eframe::egui::{self, RichText, Sense, Ui};

use super::{BuilderScreen, Pane};
use crate::i18n::{t, t_with};
use crate::theme::Tokens;
use crate::widgets::kbd;
use crate::windows::Screen;

/// A depth's indent in points (depth is at most 8).
fn depth_px(depth: usize) -> f32 {
    f32::from(u8::try_from(depth.saturating_sub(1)).unwrap_or(u8::MAX)) * 16.0
}

/// The outline's row height.
const ROW_H: f32 = 26.0;

/// A row's icon letter.
const fn icon(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Section => "S",
        RowKind::Group => "G",
        RowKind::Formula => "=",
        RowKind::Text => "T",
        RowKind::Blank => "\u{2423}",
        RowKind::PageBreak => "\u{2913}",
    }
}

/// The i18n key naming a kind.
pub(crate) const fn kind_key(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Section => "builder.kind.section",
        RowKind::Group => "builder.kind.group",
        RowKind::Formula => "builder.kind.formula",
        RowKind::Text => "builder.kind.text",
        RowKind::Blank => "builder.kind.blank",
        RowKind::PageBreak => "builder.kind.page_break",
    }
}

/// The caption a row shows (its kind's name when it has none).
pub(crate) fn caption_of(s: &BuilderScreen, id: &str) -> String {
    match tree::find(&s.ed.definition().nodes, id) {
        Some(Node::Section(x)) => x.caption.clone(),
        Some(Node::Group(x)) => x.caption.clone(),
        Some(Node::Formula(x)) => x.caption.clone(),
        Some(Node::Text(x)) if !x.text.is_empty() => x.text.clone(),
        Some(n) => t(kind_key(RowKind::of(n))),
        None => String::new(),
    }
}

/// The sub-caption: "by account", "Income − COGS", "group".
fn sub_caption(s: &BuilderScreen, node: &Node) -> String {
    match node {
        Node::Section(x) => x
            .group_by
            .first()
            .map(|g| {
                t_with(
                    "builder.outline.by",
                    &[("key", &t(&format!("builder.group.{}", g.key)))],
                )
            })
            .unwrap_or_default(),
        Node::Group(_) => t("builder.outline.group"),
        Node::Formula(f) => {
            let mut out = String::new();
            for (i, term) in f.terms.iter().enumerate() {
                let c = caption_of(s, &term.node_id);
                let short: String = c.chars().take(10).collect();
                if i > 0 || term.sign < 0 {
                    out.push_str(if term.sign < 0 { " \u{2212} " } else { " + " });
                }
                out.push_str(&short);
            }
            out
        }
        _ => String::new(),
    }
}

/// The Add menu's kinds, in order.
fn add_menu(s: &mut BuilderScreen, ui: &mut Ui) {
    let mut close = ui.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Frame::group(ui.style()).show(ui, |ui| {
        for (n, kind) in RowKind::ALL.iter().enumerate() {
            let r = ui.button(t(kind_key(*kind)));
            if n == 0 && !ui.memory(|m| m.focused().is_some_and(|f| f == r.id)) && s.add_menu_new {
                r.request_focus();
            }
            if r.clicked() {
                let caption = match kind {
                    RowKind::Section => t("builder.new.section"),
                    RowKind::Group => t("builder.new.group"),
                    RowKind::Formula => t("builder.new.formula"),
                    RowKind::Text => t("builder.new.text"),
                    RowKind::Blank | RowKind::PageBreak => String::new(),
                };
                s.apply(Op::Add {
                    kind: *kind,
                    caption,
                });
                close = true;
                s.focus_next = Some(Pane::Outline);
                ui.ctx().request_repaint();
            }
        }
    });
    s.add_menu_new = false;
    if close {
        s.add_menu = false;
        s.focus_to = Some(Pane::Outline);
    }
}

/// The outline pane.
pub(crate) fn draw(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    egui::Frame::new()
        .fill(tokens.surface_subtle)
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(t("builder.outline.title"))
                        .size(12.0)
                        .strong()
                        .color(tokens.text_secondary),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(t_with(
                            "builder.outline.count",
                            &[
                                ("n", &s.ed.row_count().to_string()),
                                ("max", &limits::NODES.to_string()),
                            ],
                        ))
                        .size(12.0)
                        .color(tokens.text_secondary),
                    );
                });
            });
            toolbar(s, ui);
            if s.ed.row_count() >= limits::NODES - 10 {
                crate::screens::company::form::inline_note(
                    ui,
                    tokens,
                    &t("builder.outline.near_cap"),
                );
            }
            if s.add_menu {
                add_menu(s, ui);
            }
            egui::ScrollArea::vertical()
                .id_salt(("builder-outline", s.id.0))
                .auto_shrink([false, false])
                .max_height(ui.available_height() - 40.0)
                .show(ui, |ui| rows(s, ui, tokens));
            ui.label(
                RichText::new(t("builder.outline.keys"))
                    .size(11.0)
                    .color(tokens.text_secondary),
            );
        });
}

fn toolbar(s: &mut BuilderScreen, ui: &mut Ui) {
    let sel = s.ed.selection().map(str::to_string);
    ui.horizontal_wrapped(|ui| {
        let add = ui.add_enabled_ui(s.ed.can_add(), |ui| {
            ui.horizontal(|ui| {
                let r = ui.button(t("builder.outline.add"));
                kbd(ui, "Ins");
                r
            })
            .inner
        });
        if add
            .inner
            .on_disabled_hover_text(t("builder.refused.too_many_rows"))
            .clicked()
        {
            s.add_menu = !s.add_menu;
            s.add_menu_new = s.add_menu;
        }
        let has = sel.is_some();
        let del = ui.add_enabled(has, egui::Button::new(t("builder.outline.delete")));
        if del.clicked() {
            s.command(cl_model::CommandId::new("builder.remove"));
        }
        for (key, cmd) in [
            ("builder.outline.up", "builder.move_up"),
            ("builder.outline.down", "builder.move_down"),
            ("builder.outline.indent", "builder.indent"),
            ("builder.outline.outdent", "builder.outdent"),
        ] {
            let label = match cmd {
                "builder.move_up" => "\u{2191}",
                "builder.move_down" => "\u{2193}",
                "builder.indent" => "\u{2192}",
                _ => "\u{2190}",
            };
            let r = ui.add_enabled(has, egui::Button::new(label));
            let name = t(key);
            r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, has, &name));
            if r.on_hover_text(&name).clicked() {
                s.command(cl_model::CommandId::new(cmd));
            }
        }
    });
}

fn rows(s: &mut BuilderScreen, ui: &mut Ui, tokens: &Tokens) {
    let list = s.ed.visible_rows();
    let selected = s.ed.selection().map(str::to_string);
    let mut any_focus = false;
    let mut click = None;
    let mut toggle = None;
    for row in &list {
        let is_sel = selected.as_deref() == Some(row.id.as_str());
        let Some(node) = tree::find(&s.ed.definition().nodes, &row.id).cloned() else {
            continue;
        };
        let warn = s.ed.row_has_problem(&row.id);
        if let Some((rid, _)) = &s.rename
            && *rid == row.id
        {
            rename_row(s, ui, row.depth);
            continue;
        }
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, ROW_H), Sense::click());
        let caption = caption_of(s, &row.id);
        let sub = sub_caption(s, &node);
        resp.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::TreeItem,
                true,
                is_sel,
                format!("{} {caption}", t(kind_key(row.kind))),
            )
        });
        if ui.is_rect_visible(rect) {
            let p = ui.painter();
            let fill = if warn {
                tokens.warn_tint
            } else if is_sel {
                tokens.accent_tint
            } else {
                egui::Color32::TRANSPARENT
            };
            p.rect_filled(rect, 3.0, fill);
            if is_sel {
                p.rect_stroke(
                    rect,
                    3.0,
                    egui::Stroke::new(1.0, tokens.accent_border),
                    egui::StrokeKind::Inside,
                );
            }
            if resp.has_focus() {
                p.rect_stroke(
                    rect.expand(1.0),
                    3.0,
                    egui::Stroke::new(2.0, tokens.accent),
                    egui::StrokeKind::Outside,
                );
            }
            #[allow(clippy::cast_precision_loss, reason = "depth is at most 8")]
            let indent = (row.depth - 1) as f32 * 16.0;
            let mut x = rect.left() + 4.0 + indent;
            if row.has_children {
                let mark = if row.folded { "+" } else { "\u{2212}" };
                p.text(
                    egui::pos2(x, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mark,
                    egui::FontId::proportional(12.0),
                    tokens.text_secondary,
                );
            }
            x += 12.0;
            let icon_rect = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - 8.0),
                egui::vec2(16.0, 16.0),
            );
            p.rect_stroke(
                icon_rect,
                2.0,
                egui::Stroke::new(1.0, tokens.border_strong),
                egui::StrokeKind::Inside,
            );
            p.text(
                icon_rect.center(),
                egui::Align2::CENTER_CENTER,
                icon(row.kind),
                egui::FontId::monospace(10.0),
                tokens.text_secondary,
            );
            x += 22.0;
            let muted = matches!(row.kind, RowKind::Blank | RowKind::PageBreak);
            p.text(
                egui::pos2(x, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &caption,
                egui::FontId::proportional(13.0),
                if muted {
                    tokens.text_secondary
                } else {
                    tokens.text_primary
                },
            );
            if !sub.is_empty() {
                p.text(
                    egui::pos2(rect.right() - 4.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    &sub,
                    egui::FontId::proportional(11.0),
                    tokens.text_secondary,
                );
            }
        }
        if is_sel && s.focus_to == Some(Pane::Outline) {
            resp.request_focus();
            resp.scroll_to_me(None);
        }
        if resp.has_focus() {
            any_focus = true;
            if !is_sel {
                // Tab focus landed here: it is the selection.
                click = Some(row.id.clone());
            }
        }
        if resp.clicked() {
            click = Some(row.id.clone());
            if row.has_children
                && resp
                    .interact_pointer_pos()
                    .is_some_and(|pos| pos.x < rect.left() + 18.0 + depth_px(row.depth))
            {
                toggle = Some((row.id.clone(), !row.folded));
            }
        }
        if resp.double_clicked() {
            s.ed.select(Some(&row.id));
            s.start_rename();
        }
    }
    if list.is_empty() {
        ui.label(RichText::new(t("builder.outline.empty")).color(tokens.text_secondary));
        if s.focus_to == Some(Pane::Outline) {
            // An empty tree: the Add button takes the keyboard.
            s.add_menu = true;
            s.add_menu_new = true;
        }
    }
    if let Some(id) = click {
        s.ed.select(Some(&id));
        s.pane = Pane::Outline;
    }
    if let Some((id, folded)) = toggle {
        s.ed.set_folded(&id, folded);
    }
    s.outline_focused = any_focus;
    if any_focus {
        s.pane = Pane::Outline;
    }
}

/// The row being renamed: a text field; Enter commits, Esc cancels.
fn rename_row(s: &mut BuilderScreen, ui: &mut Ui, depth: usize) {
    let Some((id, mut text)) = s.rename.clone() else {
        return;
    };
    let mut done = None;
    ui.horizontal(|ui| {
        #[allow(clippy::cast_precision_loss, reason = "depth is at most 8")]
        ui.add_space((depth - 1) as f32 * 16.0 + 16.0);
        let r = ui.add(
            egui::TextEdit::singleline(&mut text)
                .id(ui.make_persistent_id(("builder-rename", &id)))
                .desired_width(ui.available_width() - 8.0),
        );
        let label = t("builder.outline.rename");
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::TextInput, true, &label));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() {
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            done = Some(!esc);
        }
    });
    match done {
        // The field goes away this frame: the row takes the keyboard on
        // the next one.
        Some(true) => {
            s.rename = None;
            s.apply(Op::Rename {
                id: id.clone(),
                text,
            });
            s.focus_next = Some(Pane::Outline);
            ui.ctx().request_repaint();
        }
        Some(false) => {
            s.rename = None;
            s.focus_next = Some(Pane::Outline);
            ui.ctx().request_repaint();
        }
        None => s.rename = Some((id, text)),
    }
}
