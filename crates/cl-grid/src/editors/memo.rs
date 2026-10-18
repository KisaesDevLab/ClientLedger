//! The memo editor (Q-200). A single-line memo is the text editor. A
//! multiline memo is a multiline `TextEdit`: Shift+Enter inserts a line
//! break, Enter still records the row, the grid consumes Tab before the
//! editor sees it, and the arrows move the caret while the text has several
//! lines. Measured rows grow with the text, up to [`MAX_LINES`].

use egui::{FontId, Frame, Id, Key, KeyboardShortcut, Margin, Modifiers, TextEdit};

use crate::grid::FULL_FILTER;

/// The most lines a memo row grows to; longer text scrolls inside.
pub const MAX_LINES: u64 = 8;

/// Extra lines a memo value needs beyond the first (capped).
#[must_use]
pub fn extra_lines(text: &str) -> u64 {
    let breaks = u64::try_from(text.matches('\n').count()).unwrap_or(u64::MAX);
    breaks.min(MAX_LINES - 1)
}

/// The multiline `TextEdit` for the cursor cell.
pub fn editor(
    buffer: &mut String,
    id: Id,
    font: FontId,
    width: f32,
    rows: usize,
    max_chars: usize,
) -> TextEdit<'_> {
    TextEdit::multiline(buffer)
        .id(id)
        .event_filter(FULL_FILTER)
        .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
        .frame(Frame::NONE)
        .margin(Margin::symmetric(6, 2))
        .font(font)
        .desired_width(width)
        .desired_rows(rows.max(1))
        .char_limit(max_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        assert_eq!(extra_lines("one"), 0);
        assert_eq!(extra_lines("one\ntwo\nthree"), 2);
        assert_eq!(extra_lines(&"x\n".repeat(30)), MAX_LINES - 1);
    }
}
