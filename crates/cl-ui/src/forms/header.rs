//! The top of every transaction form, laid out one way (Write Check, P13):
//! the toolbar with ◀ Prev / Next ▶ and the "Unsaved changes" note on the
//! right, then the title row with the form's account picker (when it has
//! one) on the left and the window title on the right.
//!
//! Both wrap instead of drawing over the controls when the window is narrow
//! or zoomed (960 × 640 at 200 %): the note drops to the next toolbar row,
//! and the title moves onto its own row above the picker.

use eframe::egui::{self, Align, Layout, RichText, Ui, Vec2};

use crate::theme::Tokens;

/// Gap kept between the title row's content and the title.
const TITLE_GAP: f32 = 16.0;

/// Which record button was clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    /// The record before this one.
    Prev,
    /// The record after this one.
    Next,
}

impl Nav {
    /// The command it runs.
    #[must_use]
    pub fn command(self) -> &'static str {
        match self {
            Nav::Prev => "form.prev_record",
            Nav::Next => "form.next_record",
        }
    }

    /// True for Prev (the forms' `navigate(older)`).
    #[must_use]
    pub fn older(self) -> bool {
        self == Nav::Prev
    }
}

/// One record button: a painted triangle (the UI fonts have no ◀ ▶) and
/// its label, the triangle on the side it points to.
fn arrow_button(ui: &mut Ui, label: &str, left: bool, tip: &str) -> egui::Response {
    let id = egui::IdSalt::new(("form-nav-arrow", left));
    let size = Vec2::new(8.0, 10.0);
    let text = RichText::new(label);
    let button = if left {
        egui::Button::new((egui::Atom::custom(id, size), text))
    } else {
        egui::Button::new((text, egui::Atom::custom(id, size)))
    };
    let r = button.atom_ui(ui);
    if let Some(rect) = r.rect(id) {
        let color = ui.style().interact(&r.response).text_color();
        let (tip_x, base_x) = if left {
            (rect.left(), rect.right())
        } else {
            (rect.right(), rect.left())
        };
        let c = rect.center().y;
        let h = rect.height() / 2.0;
        ui.painter().add(egui::Shape::convex_polygon(
            vec![
                egui::pos2(tip_x, c),
                egui::pos2(base_x, c - h),
                egui::pos2(base_x, c + h),
            ],
            color,
            egui::Stroke::NONE,
        ));
    }
    r.response.on_hover_text(tip)
}

/// ◀ Prev · Next ▶ over the saved records, the tooltips naming what they
/// open and their shortcuts. Returns the one clicked.
pub fn nav_buttons(
    ui: &mut Ui,
    enabled: bool,
    prev: (&str, &str),
    next: (&str, &str),
) -> Option<Nav> {
    let mut clicked = None;
    ui.add_enabled_ui(enabled, |ui| {
        if arrow_button(ui, prev.0, true, prev.1).clicked() {
            clicked = Some(Nav::Prev);
        }
        if arrow_button(ui, next.0, false, next.1).clicked() {
            clicked = Some(Nav::Next);
        }
    });
    clicked
}

/// The "Unsaved changes" note, last in a `horizontal_wrapped` toolbar:
/// at the right when the row has room for it, else on the next row (never
/// over the buttons).
pub fn unsaved_note(ui: &mut Ui, tokens: &Tokens, dirty: bool, text: &str) {
    if !dirty {
        return;
    }
    let rich = RichText::new(text).size(12.0).color(tokens.text_tertiary);
    let width = ui
        .painter()
        .layout_no_wrap(
            text.to_owned(),
            egui::FontId::proportional(12.0),
            tokens.text_tertiary,
        )
        .size()
        .x;
    let label = egui::Label::new(rich)
        .wrap_mode(egui::TextWrapMode::Extend)
        .selectable(false);
    if ui.available_size_before_wrap().x >= width + ui.spacing().item_spacing.x {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(label);
        });
    } else {
        ui.add(label);
    }
}

/// The toolbar: `add` draws the buttons (start with [`nav_buttons`]), then
/// the unsaved note; a separator under it.
pub fn toolbar<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    dirty: bool,
    unsaved: &str,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let out = ui
        .horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let out = add(ui);
            unsaved_note(ui, tokens, dirty, unsaved);
            out
        })
        .inner;
    ui.separator();
    out
}

/// The window title in the heading style.
fn title_label(tokens: &Tokens, title: &str) -> egui::Label {
    egui::Label::new(
        RichText::new(title)
            .text_style(egui::TextStyle::Heading)
            .color(tokens.text_primary),
    )
    .selectable(false)
}

/// The title row: `content` (the account picker and its balance, or
/// nothing) on the left and `title` on the right. When the two don't fit
/// side by side the title goes on its own row above, and `content` wraps.
pub fn title_row<R>(
    ui: &mut Ui,
    tokens: &Tokens,
    salt: &str,
    title: &str,
    content: impl FnOnce(&mut Ui) -> R,
) -> R {
    let id = ui.scope_id().with(("form-title-row", salt));
    let font = egui::TextStyle::Heading.resolve(ui.style());
    let title_width = ui
        .painter()
        .layout_no_wrap(title.to_owned(), font, tokens.text_primary)
        .size()
        .x;
    let room = ui.available_width();
    let last: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    let beside = |content_width: f32| content_width + TITLE_GAP + title_width <= room;
    let side = beside(last);
    if !side {
        ui.add(title_label(tokens, title).wrap());
        ui.add_space(4.0);
    }
    let (out, width) = ui
        .horizontal_wrapped(|ui| {
            let r = ui.scope(content);
            let width = r.response.rect.width();
            if side {
                ui.with_layout(Layout::right_to_left(Align::BOTTOM), |ui| {
                    ui.add(title_label(tokens, title));
                });
            }
            (r.inner, width)
        })
        .inner;
    if (width - last).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, width));
        if beside(width) != side {
            // Lay the frame out again with the title where it fits.
            ui.ctx().request_discard("form title row");
        }
    }
    out
}

/// The title alone at the right (a form with no picker above its card);
/// it wraps when the window is narrower than the title.
pub fn title_only(ui: &mut Ui, tokens: &Tokens, title: &str) {
    ui.with_layout(Layout::top_down(Align::Max), |ui| {
        ui.add(title_label(tokens, title).wrap());
    });
}

/// One group of a wrapping row (a label above its input, a balance line):
/// laid out at its natural size, so the row moves the whole group to the
/// next line when it doesn't fit instead of squeezing it (egui wraps only
/// items of a known size; the size is last frame's).
pub fn wrap_item<R>(
    ui: &mut Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    add: impl FnOnce(&mut Ui) -> R,
) -> R {
    let id = ui.scope_id().with(("form-wrap-item", salt));
    let last: Option<Vec2> = ui.data(|d| d.get_temp(id));
    // The first time, the full width: the group's labels must not wrap to
    // the end of a row before its natural width is known.
    let size = last.unwrap_or(Vec2::new(ui.available_width(), 0.0));
    let r = ui.allocate_ui_with_layout(size, Layout::top_down(Align::Min), add);
    let got = r.response.rect.size();
    if last.is_none_or(|l| (l - got).length() > 0.5) {
        ui.data_mut(|d| d.insert_temp(id, got));
        ui.ctx().request_discard("form wrap item");
    }
    r.inner
}
