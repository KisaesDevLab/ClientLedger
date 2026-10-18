//! Tag cells (Q-179): chips, one per tag group, then the text being typed.
//!
//! Typing ranks the tag list into a type-ahead popup. Enter or Tab adds the
//! highlighted or exactly matching tag as a chip. A tag from a group that
//! already has one replaces it in place, with the `grid.tags.replaced`
//! caption. Backspace in an empty editor removes the last chip. The rules
//! are `cl_model::tags` and `core::state`. This module draws the chips (Plex
//! has no chip glyphs, so they are painted) and names them for AccessKit.
//!
//! Engine note (S14): engine lines hold one `tagId`. P13 refuses to save a
//! line with more than one tag until the multi-tag UPSTREAM item lands.

use cl_model::tags::TagChip;
use egui::{Align2, CornerRadius, Painter, Rect, Stroke, StrokeKind, pos2, vec2};

use crate::view::style::GridStyle;

/// The chips as one line of text ("Install · Crew B"). The grid shows the
/// same text in a tag cell that is not being edited.
#[must_use]
pub fn chips_text(chips: &[TagChip]) -> String {
    chips
        .iter()
        .map(|c| c.label.as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Paint the chips at the left of `rect`, using at most two thirds of its
/// width (a "+N" marks chips that do not fit). Returns the width used.
#[must_use = "the editor starts after the chips"]
pub fn paint_chips(painter: &Painter, rect: Rect, chips: &[TagChip], style: &GridStyle) -> f32 {
    if chips.is_empty() {
        return 0.0;
    }
    let limit = rect.width() * 2.0 / 3.0;
    let chip_h = (rect.height() - 8.0).clamp(12.0, 22.0);
    let chip_top = rect.center().y - chip_h / 2.0;
    let mut x = rect.left() + 4.0;
    for (i, chip) in chips.iter().enumerate() {
        let galley = painter.layout_no_wrap(chip.label.clone(), style.small.clone(), style.text);
        let chip_w = galley.size().x + 12.0;
        if x + chip_w - rect.left() > limit {
            let more = format!("+{}", chips.len() - i);
            painter.text(
                pos2(x + 2.0, rect.center().y),
                Align2::LEFT_CENTER,
                &more,
                style.small.clone(),
                style.text_secondary,
            );
            x += 24.0;
            break;
        }
        let r = Rect::from_min_size(pos2(x, chip_top), vec2(chip_w, chip_h));
        painter.rect_filled(r, CornerRadius::same(4), style.accent_tint);
        painter.rect_stroke(
            r,
            CornerRadius::same(4),
            Stroke::new(1.0, style.accent_border),
            StrokeKind::Inside,
        );
        let pos = pos2(r.left() + 6.0, r.center().y - galley.size().y / 2.0);
        painter.galley(pos, galley, style.text);
        x += chip_w + 4.0;
    }
    x - rect.left()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_joins_labels() {
        let chips = vec![
            TagChip {
                id: 1,
                label: "Install".into(),
                group: None,
            },
            TagChip {
                id: 3,
                label: "Crew B".into(),
                group: Some(70),
            },
        ];
        assert_eq!(chips_text(&chips), "Install · Crew B");
        assert_eq!(chips_text(&[]), "");
    }
}
