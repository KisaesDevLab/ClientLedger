//! Small form pieces the company screens share: cards, labelled inputs
//! (each with an AccessKit label), a date field with the DESIGN.md date
//! keys, a money field, choice rows, and the action row. The real grid and
//! form widgets arrive with P10; these are deliberately plain.

use cl_core::{CalendarDate, Money, MoneyInputError, MoneyStyle, UserInput};
use cl_model::date_entry::{self, DateOrder, US_FIRST_WEEKDAY};
use cl_model::date_rules::DateKey;
use eframe::egui::{self, Color32, CornerRadius, Margin, Response, RichText, Stroke, Ui, Vec2};

use crate::i18n::t;
use crate::theme::Tokens;
use crate::widgets::primary_button;

/// Text typed into a secret field: zeroized when dropped or cleared, so a
/// key or password does not linger in freed memory (security low).
///
/// Limit: egui's `TextEdit` keeps its own undo history of the text in
/// `Memory` (plain `String`s), so a typed secret can outlive this buffer
/// inside egui until that state is dropped. Follow-up: clear the undoer
/// for secret fields, or give them a `TextEdit` without undo (Q-P09h).
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

/// A plain-language heading for a group of fields (its response takes a
/// tooltip).
pub fn heading(ui: &mut Ui, tokens: &Tokens, text: &str) -> Response {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .text_style(crate::theme::styles::section())
                .color(tokens.text_primary),
        )
        .selectable(false),
    )
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
/// W, K) applied when typed at the end of the text. Parsing and the keys are
/// the shared `cl_model::date_entry` rules the grid uses (P10, Q-P10f).
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

    /// Re-parses the text (`M/D` uses `today`'s year).
    pub fn reparse(&mut self, today: CalendarDate) {
        self.value = date_entry::parse(&self.text, DateOrder::Mdy, today).ok();
    }

    /// Shows a parsed date in full (`1/5/99` becomes `01/05/2099`, N2), so
    /// the owner sees which century a short year meant. Unparsed text stays.
    pub fn settle(&mut self) {
        if let Some(d) = self.value {
            self.text = date_entry::format(d, DateOrder::Mdy);
        }
    }

    /// Sets a date and its text.
    pub fn set(&mut self, d: CalendarDate) {
        self.text = crate::shell::us_date(d);
        self.value = Some(d);
    }

    /// The text changed from `before`: a date key (+, −, T, M, H, Y, R, W,
    /// K) is a command, not text. Typed at the end it always is (the date
    /// so far, else the field's date, else today, is the base); typed
    /// anywhere else, wherever the caret was, it is when `before` is a
    /// date. Other typing re-parses the text.
    pub fn typed(&mut self, before: &str, today: CalendarDate) {
        let last = self.text.chars().last();
        let command = if let Some(key) = last.and_then(DateKey::from_char) {
            let mut base = self.text.clone();
            base.pop();
            Some((key, base))
        } else if date_entry::parse(before, DateOrder::Mdy, today).is_ok() {
            single_insert(before, &self.text)
                .and_then(|(_, c)| DateKey::from_char(c))
                .map(|key| (key, before.to_string()))
        } else {
            None
        };
        match command {
            Some((key, base)) => {
                // The week starts on Sunday for US businesses (Q-41, Q-P10f).
                match date_entry::apply_key(
                    key,
                    &base,
                    self.value,
                    DateOrder::Mdy,
                    today,
                    US_FIRST_WEEKDAY,
                ) {
                    Some(d) => self.set(d),
                    // Out of range: the field keeps its text.
                    None => self.text = base,
                }
            }
            None => self.reparse(today),
        }
    }
}

/// The one character `after` adds to `before` (and where), when that is the
/// whole change.
fn single_insert(before: &str, after: &str) -> Option<(usize, char)> {
    let (b, a): (Vec<char>, Vec<char>) = (before.chars().collect(), after.chars().collect());
    if a.len() != b.len() + 1 {
        return None;
    }
    let i = b
        .iter()
        .zip(&a)
        .position(|(x, y)| x != y)
        .unwrap_or(b.len());
    (a[i + 1..] == b[i..]).then_some((i, a[i]))
}

/// Draws a date field and applies date keys.
pub fn date_field(
    ui: &mut Ui,
    label: &str,
    input: &mut DateInput,
    today: CalendarDate,
) -> Response {
    let before = input.text.clone();
    let response = ui
        .horizontal(|ui| {
            let l = ui.add(egui::Label::new(label).selectable(false));
            ui.add_sized([120.0, 24.0], egui::TextEdit::singleline(&mut input.text))
                .labelled_by(l.id)
        })
        .inner;
    if response.changed() {
        input.typed(&before, today);
    }
    if response.lost_focus() {
        input.settle();
    }
    response
}

/// The text an amount field shows once the owner leaves it: the parsed
/// amount in the display style (thousands separators, two decimals,
/// DESIGN.md "Amounts"). `None` when the text is empty or not an amount,
/// so the owner's typing stays as it was.
#[must_use]
pub fn settled_money_text(text: &str, style: MoneyStyle) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    Money::parse_user(text, UserInput::MONEY)
        .ok()
        .map(|m| m.format(style))
}

/// A money input, right-aligned in mono like every amount; the text settles
/// to the display style when focus leaves. Returns the parsed amount
/// (exact) when the text is one.
///
/// # Errors
/// The owner-facing parse error when the text is not an amount.
pub fn money_field(
    ui: &mut Ui,
    label: &str,
    text: &mut String,
    style: MoneyStyle,
) -> Result<Option<Money>, MoneyInputError> {
    let response = ui
        .horizontal(|ui| {
            let l = ui.add(egui::Label::new(label).selectable(false));
            ui.add_sized(
                [140.0, 24.0],
                egui::TextEdit::singleline(text)
                    .font(egui::TextStyle::Monospace)
                    .align(egui::Align2::RIGHT_CENTER),
            )
            .labelled_by(l.id)
        })
        .inner;
    if response.lost_focus()
        && let Some(settled) = settled_money_text(text, style)
    {
        *text = settled;
    }
    if text.trim().is_empty() {
        return Ok(None);
    }
    Money::parse_user(text, UserInput::MONEY).map(Some)
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

    /// An amount settles to the display style with thousands separators,
    /// and the settled text parses back to the very same amount.
    #[test]
    fn money_text_settles_to_the_display_style_exactly() {
        let minus = MoneyStyle::default();
        let parens = MoneyStyle {
            negative: cl_core::NegativeStyle::Parens,
            symbol: true,
        };
        assert_eq!(
            settled_money_text("1286.40", minus).as_deref(),
            Some("1,286.40")
        );
        assert_eq!(
            settled_money_text("1234567.5", minus).as_deref(),
            Some("1,234,567.50")
        );
        assert_eq!(settled_money_text("", minus), None);
        // Typed money rounds to the cent, half away from zero (Q-232).
        assert_eq!(
            settled_money_text("12.345", minus).as_deref(),
            Some("12.35")
        );
        assert_eq!(
            settled_money_text("-1.005", minus).as_deref(),
            Some("-1.01")
        );
        assert_eq!(settled_money_text("abc", minus), None);
        for typed in ["1286.40", "-42.07", "0.01", "999999.99"] {
            for style in [minus, parens] {
                let shown = settled_money_text(typed, style).expect("settles");
                assert_eq!(
                    Money::parse_user(&shown, UserInput::MONEY),
                    Money::parse_user(typed, UserInput::MONEY),
                    "{typed} -> {shown}"
                );
            }
        }
    }

    #[test]
    fn date_keys_work_wherever_the_caret_is() {
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let typed = |before: &str, after: &str| {
            let mut input = DateInput::new(today);
            input.text = before.into();
            input.reparse(today);
            input.text = after.into();
            input.typed(before, today);
            (input.text, input.value.map(CalendarDate::to_wire))
        };
        let day = |s: &str| {
            (
                s.to_string(),
                Some(format!("{}-{}-{}", &s[6..], &s[..2], &s[3..5])),
            )
        };
        // At the end, as before.
        assert_eq!(typed("11/04/2026", "11/04/2026+"), day("11/05/2026"));
        // Mid-text: the caret after the month, at the start, in the year.
        assert_eq!(typed("11/04/2026", "11+/04/2026"), day("11/05/2026"));
        assert_eq!(typed("11/04/2026", "-11/04/2026"), day("11/03/2026"));
        assert_eq!(typed("11/04/2026", "11/04/20t26"), day("11/04/2026"));
        assert_eq!(typed("11/10/2026", "11/1T0/2026"), day("11/04/2026"));
        assert_eq!(typed("11/04/2026", "11/0m4/2026"), day("11/01/2026"));
        assert_eq!(typed("11/04/2026", "11/04/2y026"), day("01/01/2026"));
        assert_eq!(typed("11/04/2026", "1k1/04/2026"), day("11/07/2026"));
        // Not a date before: the key is text (the owner is still typing).
        assert_eq!(typed("11/0", "1-1/0"), ("1-1/0".to_string(), None));
        // A replaced selection is not a key.
        assert_eq!(
            typed("11/04/2026", "3/7"),
            ("3/7".to_string(), Some("2026-03-07".into()))
        );
        assert_eq!(single_insert("ab", "aXb"), Some((1, 'X')));
        assert_eq!(single_insert("ab", "abX"), Some((2, 'X')));
        assert_eq!(single_insert("ab", "XYb"), None);
    }

    #[test]
    fn date_input_uses_the_shared_rules() {
        let today = CalendarDate::from_ymd(2026, 11, 4).expect("date");
        let mut input = DateInput::new(today);
        assert_eq!(input.text, "11/04/2026");
        for (text, want) in [
            ("1/4/26", Some("2026-01-04")),
            ("2026-11-04", Some("2026-11-04")),
            ("3/7", Some("2026-03-07")),
            ("13/40/2026", None),
            ("", None),
            ("bad", None),
        ] {
            input.text = text.into();
            input.reparse(today);
            assert_eq!(
                input.value.map(CalendarDate::to_wire).as_deref(),
                want,
                "{text}"
            );
        }
        input.text = "1/5/99".into();
        input.reparse(today);
        input.settle();
        assert_eq!(input.text, "01/05/2099");
        input.text = "bad".into();
        input.reparse(today);
        input.settle();
        assert_eq!(input.text, "bad");
    }
}
