//! Captions under the open cell (P10 plan Rev 2 §2.6 item 7): drawn in a
//! foreground `Area` so they never change a row's height, with a status
//! node so screen readers and the control channel can read them.

use egui::{Area, Context, Frame, Id, Label, Margin, Order, Pos2, RichText, Role, Stroke};

use super::style::GridStyle;

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
