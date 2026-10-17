//! Small shared widgets: keyboard chips, count badges, dirty dots, eyebrows,
//! primary buttons. Marks are drawn with the painter because the embedded
//! fonts carry no symbol fallback.

use eframe::egui::{
    self, Align2, Color32, CornerRadius, Margin, Pos2, Response, RichText, Role, Sense, Stroke,
    StrokeKind, Ui, Vec2, WidgetInfo,
};

use crate::theme::{Tokens, styles};

/// A keyboard chip such as `Ctrl K`.
pub fn kbd(ui: &mut Ui, text: &str) -> Response {
    let t = Tokens::of(ui.ctx());
    egui::Frame::new()
        .fill(t.surface_window)
        .stroke(Stroke::new(1.0, t.border_strong))
        .corner_radius(CornerRadius::same(3))
        .inner_margin(Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .text_style(styles::kbd())
                        .color(t.text_secondary),
                )
                .selectable(false),
            );
        })
        .response
}

/// A pill with a count, in the warning colour ("37" on Bank Feed).
pub fn badge(ui: &mut Ui, count: u32) -> Response {
    let t = Tokens::of(ui.ctx());
    let text = count.to_string();
    let response = egui::Frame::new()
        .fill(t.warn_badge)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(&text)
                        .text_style(styles::kbd())
                        .strong()
                        .color(t.on_warn),
                )
                .selectable(false),
            );
        })
        .response;
    response.widget_info(|| WidgetInfo::labeled(Role::Label, true, format!("{count} waiting")));
    response
}

/// A filled dot, the "unsaved changes" mark on tabs and rail entries.
pub fn paint_dot(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32) {
    painter.circle_filled(center, radius, color);
}

/// Small uppercase section label (WORKSPACE, TOOLS, OPEN WINDOWS, MONEY IN).
pub fn eyebrow(ui: &mut Ui, text: &str) {
    let t = Tokens::of(ui.ctx());
    ui.add(
        egui::Label::new(
            RichText::new(text.to_uppercase())
                .text_style(styles::eyebrow())
                .color(t.text_tertiary),
        )
        .selectable(false),
    );
}

/// Primary action button: accent fill, at least 44 px tall (DESIGN.md).
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let t = Tokens::of(ui.ctx());
    ui.add(
        egui::Button::new(RichText::new(label).color(t.on_accent).strong())
            .fill(t.accent)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(96.0, 44.0)),
    )
}

/// A full-width selectable row with painted text, an optional trailing
/// widget area, and an accessible label. Returns the row response and the
/// rectangle the trailing content may use.
pub fn row_button(ui: &mut Ui, label: &str, selected: bool, height: f32) -> (Response, egui::Rect) {
    let t = Tokens::of(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, label));
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            t.accent
        } else if response.hovered() {
            t.accent_tint
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        let color = if selected {
            t.on_accent
        } else {
            t.text_primary
        };
        ui.painter().text(
            Pos2::new(rect.left() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            egui::TextStyle::Body.resolve(ui.style()),
            color,
        );
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(2.0, t.accent),
                StrokeKind::Inside,
            );
        }
    }
    (response, rect)
}
