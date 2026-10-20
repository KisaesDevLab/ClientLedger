//! The list chevron (Q-250): a small V at the right end of every list
//! field (grid list cells and form pickers) so the list is discoverable.
//! It is drawn with lines, not a font glyph. A pointer click on the field
//! opens its list at once; a click on the chevron opens or closes it.
//! Keyboard focus (Tab) and Enter or Space never open it.

use egui::{Color32, Painter, PointerButton, Rect, Response, Shape, Stroke, pos2};

/// Width kept free for the chevron at the field's right end, in points.
pub const WIDTH: f32 = 18.0;

/// Where a pointer click on a list field landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    /// On the text part of the field.
    Field,
    /// On the chevron.
    Chevron,
}

/// The chevron's area inside `field`.
#[must_use]
pub fn area(field: Rect) -> Rect {
    Rect::from_min_max(
        pos2((field.max.x - WIDTH).max(field.min.x), field.min.y),
        field.max,
    )
}

/// Paint the chevron at `field`'s right end: pointing down while the list
/// is closed, up while it is open.
pub fn paint(painter: &Painter, field: Rect, color: Color32, open: bool) {
    let c = area(field).center();
    let (w, h) = (3.5, 2.0);
    let (edge, tip) = if open {
        (c.y + h, c.y - h)
    } else {
        (c.y - h, c.y + h)
    };
    painter.add(Shape::line(
        vec![pos2(c.x - w, edge), pos2(c.x, tip), pos2(c.x + w, edge)],
        Stroke::new(1.5, color),
    ));
}

/// A primary-button pointer click on the field this frame, and where.
/// `field` is the rectangle the chevron was painted in. Keyboard "clicks"
/// (Enter or Space on a focused widget) do not count.
#[must_use]
pub fn click(resp: &Response, field: Rect) -> Option<Click> {
    if !resp.clicked_by(PointerButton::Primary) {
        return None;
    }
    let on_chevron = resp
        .interact_pointer_pos()
        .is_some_and(|p| area(field).contains(p));
    Some(if on_chevron {
        Click::Chevron
    } else {
        Click::Field
    })
}

/// The list's new open state after `click`, or `None` to leave it: the
/// field opens a closed list, the chevron toggles it.
#[must_use]
pub fn toggle(click: Option<Click>, open: bool) -> Option<bool> {
    match click {
        Some(Click::Chevron) => Some(!open),
        Some(Click::Field) if !open => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_field_opens_and_the_chevron_toggles() {
        assert_eq!(toggle(Some(Click::Field), false), Some(true));
        assert_eq!(
            toggle(Some(Click::Field), true),
            None,
            "typing area keeps it open"
        );
        assert_eq!(toggle(Some(Click::Chevron), false), Some(true));
        assert_eq!(toggle(Some(Click::Chevron), true), Some(false));
        assert_eq!(toggle(None, false), None, "focus alone never opens");
    }

    #[test]
    fn the_chevron_sits_at_the_right_end() {
        let field = Rect::from_min_max(pos2(10.0, 0.0), pos2(110.0, 20.0));
        let a = area(field);
        assert!((a.min.x - 92.0).abs() < f32::EPSILON);
        assert!((a.max.x - 110.0).abs() < f32::EPSILON);
        let narrow = Rect::from_min_max(pos2(0.0, 0.0), pos2(5.0, 5.0));
        assert!(area(narrow).min.x >= 0.0);
    }
}
