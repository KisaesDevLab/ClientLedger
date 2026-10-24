//! The Ko-fi support button (P16 step 6, Q-278, D2): a small "Support
//! ClientLedger on Ko-fi" button at the bottom right of Home. It opens
//! [`KOFI_URL`] in the owner's browser only when clicked or activated from
//! the keyboard; the app makes no request of its own and adds no tracking
//! parameters. The heart is painted from theme tokens (original artwork; no
//! Ko-fi logo), and the tooltip shows the address.

use eframe::egui::{
    self, CornerRadius, Pos2, Response, Role, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    WidgetInfo,
};

use crate::i18n::t;
use crate::theme::Tokens;

/// The owner's Ko-fi page (Q-278). No query parameters.
pub const KOFI_URL: &str = "https://ko-fi.com/I3D3227TTP";

/// Padding inside the button, and the heart's width.
const PAD: Vec2 = Vec2::new(10.0, 5.0);
const HEART: f32 = 12.0;

/// Draws the button; opens the page when it was activated. Returns the
/// button's response.
pub(super) fn show(ui: &mut Ui, tokens: &Tokens) -> Response {
    let label = t("home.kofi");
    let font = egui::TextStyle::Small.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.clone(), font, tokens.text_secondary);
    let size = Vec2::new(
        PAD.x * 2.0 + HEART + 6.0 + galley.size().x,
        (PAD.y * 2.0 + galley.size().y).max(28.0),
    );
    let (rect, r) = ui.allocate_exact_size(size, Sense::click());
    r.widget_info(|| WidgetInfo::labeled(Role::Button, ui.is_enabled(), &label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let hovered = r.hovered() || r.has_focus();
        p.rect(
            rect,
            CornerRadius::same(4),
            if hovered {
                tokens.accent_tint
            } else {
                tokens.surface_window
            },
            Stroke::new(
                1.0,
                if hovered {
                    tokens.accent_border
                } else {
                    tokens.border_default
                },
            ),
            StrokeKind::Inside,
        );
        let c = Pos2::new(rect.left() + PAD.x + HEART / 2.0, rect.center().y);
        heart(p, c, HEART, tokens.accent);
        let text_pos = Pos2::new(
            rect.left() + PAD.x + HEART + 6.0,
            rect.center().y - galley.size().y / 2.0,
        );
        p.galley(text_pos, galley, tokens.text_secondary);
        if r.has_focus() {
            p.rect_stroke(
                rect,
                CornerRadius::same(4),
                Stroke::new(2.0, tokens.accent),
                StrokeKind::Inside,
            );
        }
    }
    // `clicked` is a pointer click, or Enter or Space while focused.
    if r.clicked() {
        crate::links::open_external(ui.ctx(), KOFI_URL);
    }
    r.on_hover_text(KOFI_URL)
}

/// A heart `size` wide centred on `c`: two discs over a downward triangle.
fn heart(p: &egui::Painter, c: Pos2, size: f32, color: egui::Color32) {
    let r = size * 0.27;
    let top = c.y - size * 0.12;
    p.circle_filled(Pos2::new(c.x - r * 0.95, top), r, color);
    p.circle_filled(Pos2::new(c.x + r * 0.95, top), r, color);
    p.add(Shape::convex_polygon(
        vec![
            Pos2::new(c.x - r * 1.9, top + r * 0.25),
            Pos2::new(c.x + r * 1.9, top + r * 0.25),
            Pos2::new(c.x, c.y + size * 0.42),
        ],
        color,
        Stroke::NONE,
    ));
}
