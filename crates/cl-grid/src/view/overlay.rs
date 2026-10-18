//! Captions under the open cell (P10 plan Rev 2 §2.6 item 7): drawn in a
//! foreground `Area` so they never change a row's height, with a status
//! node so screen readers and the control channel can read them.

use egui::{
    Align2, Area, Context, CornerRadius, Frame, Id, Label, Margin, Order, Pos2, Rect, RichText,
    Role, Sense, Stroke, UiBuilder, vec2,
};

use super::style::GridStyle;
use crate::core::model::Suggestion;

/// What a popup shows this frame (from `core::state::Popup`).
pub struct PopupView<'a> {
    /// The entries.
    pub items: &'a [Suggestion],
    /// The highlighted entry.
    pub highlight: Option<usize>,
    /// The first entry shown (the list is a window of `rows` entries).
    pub top: usize,
    /// Entries shown at once.
    pub rows: usize,
    /// "Add {text}" row text, already resolved.
    pub add_new: Option<String>,
    /// A status line instead of entries ("Loading…", "No matches").
    pub status: Option<String>,
    /// The list's accessible label.
    pub label: String,
    /// Width in points.
    pub width: f32,
}

/// A press on a popup row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupHit {
    /// An entry, by index into `items`.
    Item(usize),
    /// The "Add …" row.
    AddNew,
}

/// The first entry to show so `highlight` stays inside the window.
#[must_use]
pub fn window_top(top: usize, highlight: Option<usize>, rows: usize, len: usize) -> usize {
    let rows = rows.max(1);
    let mut top = top.min(len.saturating_sub(rows));
    if let Some(h) = highlight {
        if h < top {
            top = h;
        } else if h >= top + rows {
            top = h + 1 - rows;
        }
    }
    top
}

/// The id of a popup entry's node (`list` is the popup's id).
#[must_use]
pub fn item_id(list: Id, index: usize) -> Id {
    list.with(("item", index))
}

/// The id of the "Add …" row's node.
#[must_use]
pub fn add_new_id(list: Id) -> Id {
    list.with("add_new")
}

/// Draw a list popup under a cell whose bottom-left corner is `at`. Rows
/// are click targets, never focusable, so keyboard focus stays on the
/// grid's one focus widget (B1). Returns the press and the popup's rect.
#[must_use]
pub fn popup(
    ctx: &Context,
    id: Id,
    at: Pos2,
    view: &PopupView<'_>,
    style: &GridStyle,
) -> (Option<PopupHit>, Rect) {
    let row_h = style.row_one.max(18.0);
    let mut hit = None;
    let shown = Area::new(id.with("area"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(style.surface)
                .stroke(Stroke::new(1.0, style.accent_border))
                .inner_margin(Margin::same(2))
                .show(ui, |ui| {
                    // This Ui is the ListBox node; rows are its children.
                    ui.scope_builder(
                        UiBuilder::new()
                            .scope_id(id)
                            .accessibility_role(Role::ListBox)
                            .accessibility_label(view.label.clone()),
                        |ui| {
                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            let row = |ui: &mut egui::Ui,
                                       rid: Id,
                                       text: &str,
                                       highlighted: bool,
                                       secondary: bool|
                             -> bool {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(view.width, row_h), Sense::hover());
                                let resp = ui.interact(r, rid, Sense::CLICK);
                                if highlighted {
                                    ui.painter().rect_filled(
                                        r,
                                        CornerRadius::same(2),
                                        style.accent_tint,
                                    );
                                }
                                let color = if secondary {
                                    style.text_secondary
                                } else {
                                    style.text
                                };
                                let mut job = egui::text::LayoutJob::simple_singleline(
                                    text.to_string(),
                                    style.font.clone(),
                                    color,
                                );
                                job.wrap = egui::text::TextWrapping::truncate_at_width(
                                    (view.width - 2.0 * style.pad).max(1.0),
                                );
                                let galley = ui.painter().layout_job(job);
                                let pos = Align2::LEFT_CENTER
                                    .anchor_size(
                                        egui::pos2(r.left() + style.pad, r.center().y),
                                        galley.size(),
                                    )
                                    .min;
                                ui.painter().galley(pos, galley, color);
                                ctx.accesskit_node_builder(rid, |node| {
                                    node.set_bounds(egui::accesskit::Rect {
                                        x0: r.min.x.into(),
                                        y0: r.min.y.into(),
                                        x1: r.max.x.into(),
                                        y1: r.max.y.into(),
                                    });
                                    node.set_role(Role::ListBoxOption);
                                    node.set_label(text);
                                    node.set_selected(highlighted);
                                    node.add_action(egui::accesskit::Action::Click);
                                });
                                resp.clicked()
                            };
                            if let Some(status) = &view.status {
                                let _ = row(ui, id.with("status"), status, false, true);
                            }
                            let end = (view.top + view.rows.max(1)).min(view.items.len());
                            for i in view.top.min(end)..end {
                                let s = &view.items[i];
                                if row(
                                    ui,
                                    item_id(id, i),
                                    &s.label,
                                    view.highlight == Some(i),
                                    false,
                                ) {
                                    hit = Some(PopupHit::Item(i));
                                }
                            }
                            if let Some(add) = &view.add_new
                                && row(ui, add_new_id(id), add, false, true)
                            {
                                hit = Some(PopupHit::AddNew);
                            }
                        },
                    );
                });
        });
    // An AccessKit Click on a row arrives as `clicked()` (egui fakes the
    // click for `Sense::CLICK` widgets).
    (hit, shown.response.rect)
}

/// Show `text` under a cell whose bottom-left corner is `at`.
pub fn caption(ctx: &Context, id: Id, at: Pos2, text: &str, style: &GridStyle) {
    Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(at)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(style.caption_fill)
                .stroke(Stroke::new(1.0, style.row_border))
                .inner_margin(Margin::symmetric(6, 3))
                .show(ui, |ui| {
                    let r = ui.add(
                        Label::new(
                            RichText::new(text)
                                .font(style.small.clone())
                                .color(style.warn_text),
                        )
                        .selectable(false),
                    );
                    ctx.accesskit_node_builder(r.id, |node| {
                        node.set_role(Role::Status);
                        node.set_label(text);
                    });
                });
        });
}
