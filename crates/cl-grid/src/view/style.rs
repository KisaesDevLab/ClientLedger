//! Grid colours, fonts and metrics (P10 plan §2.8). cl-ui builds a
//! [`GridStyle`] from its theme tokens for light and dark
//! (`cl_ui::theme::grid_style`); the default here is a neutral light style
//! for tests and the spike.

use egui::{Color32, FontFamily, FontId};

/// Everything the grid paints with.
#[derive(Debug, Clone, PartialEq)]
pub struct GridStyle {
    /// Grid background.
    pub surface: Color32,
    /// Header background.
    pub header_fill: Color32,
    /// Header text.
    pub header_text: Color32,
    /// Cell text.
    pub text: Color32,
    /// Second-line and secondary text.
    pub text_secondary: Color32,
    /// Placeholder text and loading bars.
    pub placeholder: Color32,
    /// Row separators.
    pub row_border: Color32,
    /// Focus outline, selection bar.
    pub accent: Color32,
    /// Cursor row fill.
    pub accent_tint: Color32,
    /// Selection outline.
    pub accent_border: Color32,
    /// Needs-review, entry and suggested rows.
    pub warn_tint: Color32,
    /// Error rows.
    pub warn_surface: Color32,
    /// The error bar and marks.
    pub warn: Color32,
    /// Error and suggestion captions.
    pub warn_text: Color32,
    /// Locked rows.
    pub locked_row: Color32,
    /// Locked row text.
    pub locked_text: Color32,
    /// Caption overlay background.
    pub caption_fill: Color32,
    /// Proportional font.
    pub font: FontId,
    /// Monospaced font (amounts, dates, numbers).
    pub mono: FontId,
    /// Second-line font (12 pt).
    pub small: FontId,
    /// Header font.
    pub header_font: FontId,
    /// One-line row height in points.
    pub row_one: f32,
    /// Two-line row height in points.
    pub row_two: f32,
    /// Header line height in points.
    pub header_line: f32,
    /// Horizontal cell padding in points.
    pub pad: f32,
}

impl Default for GridStyle {
    fn default() -> Self {
        GridStyle {
            surface: Color32::WHITE,
            header_fill: Color32::from_rgb(0xF3, 0xF4, 0xF6),
            header_text: Color32::from_rgb(0x37, 0x41, 0x51),
            text: Color32::from_rgb(0x11, 0x18, 0x27),
            text_secondary: Color32::from_rgb(0x4B, 0x55, 0x63),
            placeholder: Color32::from_rgb(0x8A, 0x8F, 0x98),
            row_border: Color32::from_rgb(0xE5, 0xE7, 0xEB),
            accent: Color32::from_rgb(0x23, 0x6B, 0xB5),
            accent_tint: Color32::from_rgb(0xE8, 0xF0, 0xFA),
            accent_border: Color32::from_rgb(0x9C, 0xBC, 0xE0),
            warn_tint: Color32::from_rgb(0xFF, 0xF7, 0xE0),
            warn_surface: Color32::from_rgb(0xFD, 0xEC, 0xEA),
            warn: Color32::from_rgb(0xB4, 0x23, 0x18),
            warn_text: Color32::from_rgb(0x8A, 0x1C, 0x13),
            locked_row: Color32::from_rgb(0xF1, 0xF1, 0xEE),
            locked_text: Color32::from_rgb(0x55, 0x55, 0x50),
            caption_fill: Color32::from_rgb(0xFF, 0xFF, 0xF4),
            font: FontId::new(13.0, FontFamily::Proportional),
            mono: FontId::new(13.0, FontFamily::Monospace),
            small: FontId::new(12.0, FontFamily::Proportional),
            header_font: FontId::new(12.0, FontFamily::Proportional),
            row_one: 30.0,
            row_two: 44.0,
            header_line: 22.0,
            pad: 6.0,
        }
    }
}
