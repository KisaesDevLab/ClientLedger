//! The grid's context menu (P16 D6): a foreground popup at the pointer (or
//! under the cursor cell for Shift+F10 and the Menu key), with an AccessKit
//! `Menu` node and one `MenuItem` per entry. Entries are click targets,
//! never focusable, so keyboard focus stays on the grid's one focus widget
//! (B1); the core moves the highlight and picks.

use egui::{
    Align2, Area, Context, CornerRadius, Frame, Id, Margin, Order, Pos2, Rect, Role, Sense, Stroke,
    UiBuilder, vec2,
};

use super::style::GridStyle;
use crate::core::context::ContextItem;

/// What the menu shows this frame (from `core::context::ContextMenu`).
pub struct MenuView<'a> {
    /// The entries.
    pub items: &'a [ContextItem],
    /// The highlighted entry.
    pub highlight: Option<usize>,
    /// The menu's accessible label.
    pub label: String,
}

/// The id of an entry's node (`menu` is the menu's id).
#[must_use]
pub fn item_id(menu: Id, index: usize) -> Id {
    menu.with(("item", index))
}

/// Narrowest menu, in points.
const MIN_WIDTH: f32 = 160.0;
/// Room between a label and its shortcut, in points.
const GAP: f32 = 24.0;

/// Draw the menu with its top-left corner at `at`. Returns the index of an
/// enabled entry that was clicked this frame and the menu's rect.
#[must_use]
pub fn show(
    ctx: &Context,
    id: Id,
    at: Pos2,
    view: &MenuView<'_>,
    style: &GridStyle,
) -> (Option<usize>, Rect) {
    let row_h = style.row_one.max(18.0);
    let mut hit = None;
    let shown = Area::new(id.with("area"))
        .order(Order::Foreground)
        .fixed_pos(at)
        .constrain(true)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(style.surface)
                .stroke(Stroke::new(1.0, style.accent_border))
                .inner_margin(Margin::same(2))
                .show(ui, |ui| {
                    // This Ui is the Menu node; entries are its children.
                    ui.scope_builder(
                        UiBuilder::new()
                            .scope_id(id)
                            .accessibility_role(Role::Menu)
                            .accessibility_label(view.label.clone()),
                        |ui| {
                            ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                            let painter = ui.painter().clone();
                            let galleys: Vec<_> = view
                                .items
                                .iter()
                                .map(|item| {
                                    let color = if item.enabled {
                                        style.text
                                    } else {
                                        style.placeholder
                                    };
                                    let label = painter.layout_no_wrap(
                                        item.label.clone(),
                                        style.font.clone(),
                                        color,
                                    );
                                    let keys = item.shortcut.as_ref().map(|s| {
                                        painter.layout_no_wrap(
                                            s.clone(),
                                            style.small.clone(),
                                            style.text_secondary,
                                        )
                                    });
                                    (label, keys)
                                })
                                .collect();
                            let width = galleys
                                .iter()
                                .map(|(l, k)| {
                                    l.size().x
                                        + k.as_ref().map_or(0.0, |k| GAP + k.size().x)
                                        + 2.0 * style.pad
                                })
                                .fold(MIN_WIDTH, f32::max);
                            for (i, (item, (label, keys))) in
                                view.items.iter().zip(galleys).enumerate()
                            {
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(width, row_h), Sense::hover());
                                let rid = item_id(id, i);
                                let mut resp = ui.interact(r, rid, Sense::CLICK);
                                let highlighted = view.highlight == Some(i) && item.enabled;
                                if highlighted || (item.enabled && resp.hovered()) {
                                    painter.rect_filled(
                                        r,
                                        CornerRadius::same(2),
                                        style.accent_tint,
                                    );
                                }
                                let pos = Align2::LEFT_CENTER
                                    .anchor_size(
                                        egui::pos2(r.left() + style.pad, r.center().y),
                                        label.size(),
                                    )
                                    .min;
                                painter.galley(pos, label, style.text);
                                if let Some(k) = keys {
                                    let pos = Align2::RIGHT_CENTER
                                        .anchor_size(
                                            egui::pos2(r.right() - style.pad, r.center().y),
                                            k.size(),
                                        )
                                        .min;
                                    painter.galley(pos, k, style.text_secondary);
                                }
                                ctx.accesskit_node_builder(rid, |node| {
                                    node.set_bounds(super::a11y::ak_rect(r));
                                    super::a11y::menu_item_node(node, item, highlighted);
                                });
                                if let Some(t) = &item.tooltip {
                                    resp = resp.on_hover_text(t.as_str());
                                }
                                if resp.clicked() && item.enabled {
                                    hit = Some(i);
                                }
                            }
                        },
                    );
                });
        });
    // An AccessKit Click on an entry arrives as `clicked()` (egui fakes the
    // click for `Sense::CLICK` widgets).
    (hit, shown.response.rect)
}
