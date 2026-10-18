//! The single-line text editor (text, memo, picker and date cells).
//!
//! It is the grid's one focus widget while editing (B1): it carries the
//! grid's focus id and the full event filter, and Enter never makes it give
//! up focus (`return_key(None)`, spike R2).

use egui::{Align2, FontId, Frame, Id, Margin, TextEdit};

use crate::grid::FULL_FILTER;

/// The `TextEdit` for the cursor cell.
pub fn editor(
    buffer: &mut String,
    id: Id,
    font: FontId,
    align: Align2,
    width: f32,
    pad: f32,
    max_chars: Option<usize>,
) -> TextEdit<'_> {
    // Geometry: the cell padding in whole points.
    #[allow(clippy::cast_possible_truncation)]
    let pad = pad.clamp(0.0, 40.0).round() as i8;
    let mut te = TextEdit::singleline(buffer)
        .id(id)
        .event_filter(FULL_FILTER)
        .return_key(None)
        .frame(Frame::NONE)
        .margin(Margin::symmetric(pad, 0))
        .font(font)
        .align(align)
        .desired_width(width);
    if let Some(n) = max_chars {
        te = te.char_limit(n);
    }
    te
}
