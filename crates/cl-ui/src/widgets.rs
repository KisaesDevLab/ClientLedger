//! Small shared widgets: keyboard chips, count badges, dirty dots, eyebrows,
//! primary buttons. Marks are drawn with the painter because the embedded
//! fonts carry no symbol fallback.

use eframe::egui::{
    self, Color32, CornerRadius, Margin, Pos2, Response, RichText, Role, Sense, Stroke, StrokeKind,
    Ui, Vec2, WidgetInfo,
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

/// A button with its shortcut chip inside, after the label ("Go To
/// `Ctrl F`"). The chip is painted, so the accessible name stays the label
/// alone. No chip when `chip` is empty.
pub fn button_with_chip(ui: &mut Ui, label: &str, chip: &str) -> Response {
    if chip.is_empty() {
        return ui.button(label);
    }
    chip_button(ui, RichText::new(label), chip, false, |b| b)
}

/// A form action button with its shortcut chip inside ("Save & New
/// `Alt N`"), `min_height` tall; `primary` fills it with the accent and
/// draws the chip to read on it. The accessible name stays the label.
pub fn action_button_with_chip(
    ui: &mut Ui,
    label: &str,
    chip: &str,
    primary: bool,
    min_height: f32,
) -> Response {
    let t = Tokens::of(ui.ctx());
    let text = if primary {
        RichText::new(label).color(t.on_accent).strong()
    } else {
        RichText::new(label)
    };
    chip_button(ui, text, chip, primary, |b| {
        let b = b.min_size(Vec2::new(0.0, min_height));
        if primary {
            b.fill(t.accent).stroke(Stroke::NONE)
        } else {
            b
        }
    })
}

/// A button holding `label` then a painted chip `chip`; `style` finishes
/// the button (fill, size).
fn chip_button(
    ui: &mut Ui,
    label: RichText,
    chip: &str,
    on_accent: bool,
    style: impl FnOnce(egui::Button<'_>) -> egui::Button<'_>,
) -> Response {
    let t = Tokens::of(ui.ctx());
    let (fill, stroke, text) = if on_accent {
        (
            t.on_accent.gamma_multiply(0.18),
            Stroke::new(1.0, t.on_accent.gamma_multiply(0.45)),
            t.on_accent,
        )
    } else {
        (
            t.surface_window,
            Stroke::new(1.0, t.border_strong),
            t.text_secondary,
        )
    };
    let font = styles::kbd().resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(chip.to_string(), font, text);
    let size = galley.size() + Vec2::new(10.0, 2.0);
    let id = egui::IdSalt::new(("button-chip", label.text().to_string()));
    let r = style(egui::Button::new((label, egui::Atom::custom(id, size)))).atom_ui(ui);
    if let Some(rect) = r.rect(id) {
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(3),
            fill,
            stroke,
            StrokeKind::Inside,
        );
        p.galley(rect.center() - galley.size() / 2.0, galley, text);
    }
    r.response
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

/// The outline of `rect` with rounded corners of `radius`, as points (the
/// first repeated at the end, so the path closes).
#[must_use]
pub fn rounded_outline(rect: egui::Rect, radius: f32) -> Vec<Pos2> {
    use std::f32::consts::FRAC_PI_2;
    const STEPS: u8 = 6;
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    // Corner centres, clockwise from the top left, and each arc's start.
    let corners = [
        (Pos2::new(rect.left() + r, rect.top() + r), 2.0),
        (Pos2::new(rect.right() - r, rect.top() + r), 3.0),
        (Pos2::new(rect.right() - r, rect.bottom() - r), 0.0),
        (Pos2::new(rect.left() + r, rect.bottom() - r), 1.0),
    ];
    let mut points = Vec::with_capacity(4 * usize::from(STEPS + 1) + 1);
    for (c, quarter) in corners {
        for i in 0..=STEPS {
            let a = (quarter + f32::from(i) / f32::from(STEPS)) * FRAC_PI_2;
            points.push(Pos2::new(c.x + r * a.cos(), c.y + r * a.sin()));
        }
    }
    if let Some(first) = points.first().copied() {
        points.push(first);
    }
    points
}

/// A dashed rounded border around `rect` (a drop zone, DESIGN.md).
pub fn paint_dashed_border(painter: &egui::Painter, rect: egui::Rect, radius: f32, stroke: Stroke) {
    painter.extend(egui::Shape::dashed_line(
        &rounded_outline(rect, radius),
        stroke,
        4.0,
        3.0,
    ));
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
/// widget area, and an accessible label (which may say more than the text,
/// for example "unsaved changes"). Returns the row response and the
/// rectangle the trailing content may use.
pub fn row_button(
    ui: &mut Ui,
    label: &str,
    accessible: &str,
    selected: bool,
    height: f32,
) -> (Response, egui::Rect) {
    row_button_with_room(ui, label, accessible, selected, height, 0.0)
}

/// [`row_button`] keeping `room` points free at the right (for a dot or a
/// count): a label too long for the rest ends in an ellipsis instead of
/// running under it.
pub fn row_button_with_room(
    ui: &mut Ui,
    label: &str,
    accessible: &str,
    selected: bool,
    height: f32,
    room: f32,
) -> (Response, egui::Rect) {
    let t = Tokens::of(ui.ctx());
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
    response.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, accessible));
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
        let mut job = egui::text::LayoutJob::single_section(
            label.to_owned(),
            egui::TextFormat::simple(egui::TextStyle::Body.resolve(ui.style()), color),
        );
        job.wrap =
            egui::text::TextWrapping::truncate_at_width((rect.width() - 16.0 - room).max(0.0));
        let galley = ui.painter().layout_job(job);
        let pos = Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, color);
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

#[cfg(test)]
mod tests {
    use eframe::egui::{self, accesskit::Role};

    #[test]
    fn the_rounded_outline_closes_and_stays_on_the_rect() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 40.0));
        let pts = super::rounded_outline(rect, 6.0);
        assert_eq!(pts.first(), pts.last());
        for p in &pts {
            assert!(rect.expand(0.01).contains(*p), "{p:?}");
        }
        // Each side is reached: the path goes all the way round.
        assert!(pts.iter().any(|p| (p.x - 10.0).abs() < 0.01));
        assert!(pts.iter().any(|p| (p.x - 110.0).abs() < 0.01));
        assert!(pts.iter().any(|p| (p.y - 20.0).abs() < 0.01));
        assert!(pts.iter().any(|p| (p.y - 60.0).abs() < 0.01));
    }

    /// The chip is painted inside the button, wider than the label alone,
    /// and the accessible name stays the label ("Go To", not "Go To Ctrl F").
    #[test]
    fn button_with_chip_keeps_the_label_as_its_name() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, crate::theme::ThemeChoice::Light);
        crate::fonts::install(&ctx);
        ctx.enable_accesskit();
        let mut widths = (0.0, 0.0);
        let mut out = None;
        for _ in 0..2 {
            let mut o = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    widths.0 = super::button_with_chip(ui, "Go To", "Ctrl F").rect.width();
                    widths.1 = super::button_with_chip(ui, "Go To", "").rect.width();
                });
            });
            o.textures_delta.clear();
            out = Some(o);
        }
        assert!(
            widths.0 > widths.1 + 20.0,
            "chip widens the button: {widths:?}"
        );
        let update = out
            .and_then(|o| o.platform_output.accesskit_update)
            .expect("accesskit is on");
        let names: Vec<String> = update
            .nodes
            .iter()
            .filter(|(_, n)| n.role() == Role::Button)
            .filter_map(|(_, n)| n.label().map(str::to_string))
            .collect();
        assert_eq!(names, ["Go To", "Go To"]);
    }
}
