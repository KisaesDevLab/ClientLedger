//! Small form pieces the company screens share: cards, labelled inputs
//! (each with an AccessKit label), a date field with the DESIGN.md date
//! keys, a money field, choice rows, and the action row. The real grid and
//! form widgets arrive with P10; these are deliberately plain.

use cl_core::{CalendarDate, Money, MoneyInputError, UserInput};
use cl_model::date_rules::{DateKey, apply_date_key};
use eframe::egui::{self, Color32, CornerRadius, Margin, Response, RichText, Stroke, Ui, Vec2};

use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// Text typed into a secret field: zeroized when dropped or cleared, so a
/// key or password does not linger in freed memory (security low).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SecretText(zeroize::Zeroizing<String>);

impl SecretText {
    /// Empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text without surrounding whitespace (borrowed; never copied).
    #[must_use]
    pub fn trimmed(&self) -> &str {
        self.0.trim()
    }

    /// Wipes the text.
    pub fn clear(&mut self) {
        zeroize::Zeroize::zeroize(&mut *self.0);
    }
}

impl std::fmt::Debug for SecretText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretText(..)")
    }
}

impl From<&str> for SecretText {
    fn from(s: &str) -> SecretText {
        SecretText(zeroize::Zeroizing::new(s.to_string()))
    }
}

impl egui::TextBuffer for SecretText {
    fn is_mutable(&self) -> bool {
        true
    }

    fn as_str(&self) -> &str {
        self.0.as_str()
    }

    fn insert_text(&mut self, text: &str, char_index: egui::text::CharIndex) -> usize {
        egui::TextBuffer::insert_text(&mut *self.0, text, char_index)
    }

    fn delete_char_range(&mut self, char_range: std::ops::Range<egui::text::CharIndex>) {
        egui::TextBuffer::delete_char_range(&mut *self.0, char_range);
    }

    fn type_id(&self) -> std::any::TypeId {
        std::any::TypeId::of::<SecretText>()
    }
}

/// A password input over a zeroizing buffer (masked on screen and in the
/// accessibility tree).
pub fn password_secret(ui: &mut Ui, label: &str, value: &mut SecretText) -> Response {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        // A secret: masked on screen and in the accessibility tree.
        ui.add_sized(
            [ui.available_width().min(360.0), 24.0],
            egui::TextEdit::singleline(value).password(true),
        )
        .labelled_by(l.id)
    })
    .inner
}

/// A single-line input that takes the whole row (paths).
pub fn field_wide(ui: &mut Ui, label: &str, value: &mut String) -> Response {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        ui.add_sized(
            [ui.available_width(), 24.0],
            egui::TextEdit::singleline(value).hint_text(""),
        )
        .labelled_by(l.id)
    })
    .inner
}

/// True when a focusable choice was clicked, or has keyboard focus and
/// Space or Enter was pressed.
#[must_use]
pub fn activated(r: &Response) -> bool {
    r.clicked()
        || (r.has_focus()
            && r.ctx
                .input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter)))
}

/// A bordered section on the subtle surface.
pub fn card<R>(ui: &mut Ui, tokens: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(tokens.surface_subtle)
        .stroke(Stroke::new(1.0, tokens.border_default))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A bordered section with less vertical padding (dense panels).
pub fn card_tight<R>(ui: &mut Ui, tokens: &Tokens, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(tokens.surface_subtle)
        .stroke(Stroke::new(1.0, tokens.border_default))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(14, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            add(ui)
        })
        .inner
}

/// A plain-language heading for a group of fields.
pub fn heading(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .text_style(crate::theme::styles::section())
                .color(tokens.text_primary),
        )
        .selectable(false),
    );
}

/// Secondary explanatory text.
pub fn hint(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(tokens.text_secondary)).wrap());
}

/// A single-line text input with its label (the label is the accessible
/// name, so scripts and screen readers find it).
pub fn field(ui: &mut Ui, label: &str, value: &mut String) -> Response {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        ui.add_sized(
            [ui.available_width().min(360.0), 24.0],
            egui::TextEdit::singleline(value).hint_text(""),
        )
        .labelled_by(l.id)
    })
    .inner
}

/// A password input (masked on screen and in the accessibility tree).
pub fn password(ui: &mut Ui, label: &str, value: &mut String) -> Response {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        // A secret: masked on screen and in the accessibility tree.
        ui.add_sized(
            [ui.available_width().min(360.0), 24.0],
            egui::TextEdit::singleline(value).password(true),
        )
        .labelled_by(l.id)
    })
    .inner
}

/// A date input in `MM/DD/YYYY` with the date keys (+, -, T, M, H, Y, R,
/// W, K) applied when typed at the end of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DateInput {
    /// The text as typed.
    pub text: String,
    /// The parsed date, when the text is a date.
    pub value: Option<CalendarDate>,
}

impl DateInput {
    /// Starts at `d`.
    #[must_use]
    pub fn new(d: CalendarDate) -> DateInput {
        DateInput {
            text: crate::shell::us_date(d),
            value: Some(d),
        }
    }

    /// Re-parses the text.
    pub fn reparse(&mut self) {
        self.value = parse_us_date(&self.text);
    }

    /// Sets a date and its text.
    pub fn set(&mut self, d: CalendarDate) {
        self.text = crate::shell::us_date(d);
        self.value = Some(d);
    }
}

/// Parses `M/D/YYYY`, `MM/DD/YYYY` or `YYYY-MM-DD`.
#[must_use]
pub fn parse_us_date(text: &str) -> Option<CalendarDate> {
    let s = text.trim();
    if let Ok(d) = CalendarDate::parse_wire(s) {
        return Some(d);
    }
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 3 {
        return None;
    }
    let m: u32 = parts[0].parse().ok()?;
    let d: u32 = parts[1].parse().ok()?;
    let y: i32 = parts[2].parse().ok()?;
    let y = if (0..100).contains(&y) { y + 2000 } else { y };
    CalendarDate::from_ymd(y, m, d).ok()
}

/// Draws a date field and applies date keys.
pub fn date_field(
    ui: &mut Ui,
    label: &str,
    input: &mut DateInput,
    today: CalendarDate,
) -> Response {
    let response = ui
        .horizontal(|ui| {
            let l = ui.add(egui::Label::new(label).selectable(false));
            ui.add_sized([120.0, 24.0], egui::TextEdit::singleline(&mut input.text))
                .labelled_by(l.id)
        })
        .inner;
    if response.changed() {
        let last = input.text.chars().last();
        if let Some(key) = last.and_then(DateKey::from_char) {
            input.text.pop();
            let base = parse_us_date(&input.text).or(input.value).unwrap_or(today);
            if let Some(d) = apply_date_key(key, base, today, cl_core::Weekday::Mon) {
                input.set(d);
            }
        } else {
            input.reparse();
        }
    }
    response
}

/// A money input; returns the parsed amount (exact) when the text is one.
///
/// # Errors
/// The owner-facing parse error when the text is not an amount.
pub fn money_field(
    ui: &mut Ui,
    label: &str,
    text: &mut String,
) -> Result<Option<Money>, MoneyInputError> {
    ui.horizontal(|ui| {
        let l = ui.add(egui::Label::new(label).selectable(false));
        ui.add_sized([140.0, 24.0], egui::TextEdit::singleline(text))
            .labelled_by(l.id);
    });
    if text.trim().is_empty() {
        return Ok(None);
    }
    Money::parse_user(text, UserInput::CENTS).map(Some)
}

/// One selectable card with a title and a description (a radio choice).
/// Allocated like a button, so every card is in the Tab order, the arrow
/// keys move focus between cards (egui's own focus movement) and Space or
/// Enter selects the focused one; it shows a radio mark, left-aligned
/// text and a focus ring.
pub fn choice(
    ui: &mut Ui,
    tokens: &Tokens,
    selected: bool,
    title: &str,
    description: &str,
) -> Response {
    let width = ui.available_width();
    let pad = Vec2::new(12.0, 8.0);
    let mark = 14.0;
    let text_width = (width - pad.x * 2.0 - mark - 6.0).max(40.0);
    let title_galley = ui.painter().layout(
        title.to_string(),
        egui::FontId::proportional(14.0),
        tokens.text_primary,
        text_width,
    );
    let desc_galley = ui.painter().layout(
        description.to_string(),
        egui::FontId::proportional(12.0),
        tokens.text_secondary,
        width - pad.x * 2.0,
    );
    let height = pad.y * 2.0 + title_galley.size().y + 4.0 + desc_galley.size().y;
    let (rect, response) = ui.allocate_at_least(Vec2::new(width, height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if selected {
            tokens.accent_tint
        } else {
            tokens.surface_window
        };
        let stroke = if selected {
            tokens.accent
        } else {
            tokens.border_default
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(6),
            fill,
            Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(2.0),
                CornerRadius::same(8),
                Stroke::new(2.0, tokens.accent),
                egui::StrokeKind::Outside,
            );
        }
        let title_top = rect.top() + pad.y;
        let center = egui::Pos2::new(
            rect.left() + pad.x + mark / 2.0,
            title_top + title_galley.size().y / 2.0,
        );
        painter.circle_stroke(
            center,
            6.0,
            Stroke::new(
                1.5,
                if selected {
                    tokens.accent
                } else {
                    tokens.border_strong
                },
            ),
        );
        if selected {
            painter.circle_filled(center, 3.0, tokens.accent);
        }
        painter.galley(
            egui::Pos2::new(rect.left() + pad.x + mark + 6.0, title_top),
            title_galley.clone(),
            tokens.text_primary,
        );
        painter.galley(
            egui::Pos2::new(rect.left() + pad.x, title_top + title_galley.size().y + 4.0),
            desc_galley,
            tokens.text_secondary,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, title.to_string())
    });
    response
}

/// A coloured swatch button for the company colour picker.
pub fn swatch(ui: &mut Ui, color: [u8; 3], selected: bool, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, name.to_string())
    });
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(6),
        Color32::from_rgb(color[0], color[1], color[2]),
    );
    if selected || response.has_focus() {
        painter.rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(8),
            Stroke::new(2.0, Tokens::of(ui.ctx()).accent),
            egui::StrokeKind::Outside,
        );
    }
    response
}

/// The action row: a primary button and plain ones. Returns the index of
/// the clicked button (0 = primary).
pub fn actions(ui: &mut Ui, primary: &str, others: &[&str]) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        if primary_button(ui, primary).clicked() {
            clicked = Some(0);
        }
        for (i, label) in others.iter().enumerate() {
            if ui
                .add(egui::Button::new(*label).min_size(Vec2::new(0.0, 44.0)))
                .clicked()
            {
                clicked = Some(i + 1);
            }
        }
    });
    clicked
}

/// Shows an owner-facing error inline (message plus fix).
pub fn inline_error(ui: &mut Ui, tokens: &Tokens, error: &dyn cl_core::OwnerFacing) {
    let m = error.owner_message();
    let mut text = t(m.key);
    if let Some(fix) = m.suggestion_key {
        text.push(' ');
        text.push_str(&t(fix));
    }
    egui::Frame::new()
        .fill(tokens.warn_surface)
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(text).color(tokens.warn_text)).wrap());
        });
}

/// A short progress line.
pub fn progress_line(ui: &mut Ui, tokens: &Tokens, text: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0));
        ui.add(
            egui::Label::new(RichText::new(text).color(tokens.text_secondary)).selectable(false),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_parse_in_the_us_forms() {
        let d = parse_us_date("11/04/2026").expect("date");
        assert_eq!(d.to_wire(), "2026-11-04");
        assert_eq!(
            parse_us_date("1/4/26")
                .map(CalendarDate::to_wire)
                .as_deref(),
            Some("2026-01-04")
        );
        assert_eq!(
            parse_us_date("2026-11-04")
                .map(CalendarDate::to_wire)
                .as_deref(),
            Some("2026-11-04")
        );
        assert_eq!(parse_us_date("13/40/2026"), None);
        assert_eq!(parse_us_date(""), None);
        let mut input = DateInput::new(d);
        assert_eq!(input.text, "11/04/2026");
        input.text = "bad".into();
        input.reparse();
        assert_eq!(input.value, None);
    }
}
