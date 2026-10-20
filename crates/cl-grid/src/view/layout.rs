//! Column layout (P10 plan §2.8). Widths are logical points. Line-1
//! columns run left to right; each line-2 column sits under the line-1
//! column before it in the spec (Type under Number, Account · Memo under
//! Payee, as in the register mockup). Short of room, the flexible (text)
//! columns shrink toward their minimum first; when that still does not
//! fit, the grid scrolls horizontally instead of squeezing numbers.

use crate::core::model::{Column, ColumnId, Line};

/// Where one column is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnPlace {
    /// The column.
    pub id: ColumnId,
    /// Left edge relative to the grid's content left, in points.
    pub x: f32,
    /// Width in points.
    pub width: f32,
    /// Which line of the row.
    pub line: Line,
}

/// The computed layout.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColumnLayout {
    /// Every column, in spec order.
    pub places: Vec<ColumnPlace>,
    /// Total content width in points.
    pub content_width: f32,
    /// Any column is on line 2.
    pub two_lines: bool,
}

impl ColumnLayout {
    /// Lay out `columns` in `available` points. `widths` are the owner's
    /// resized widths (and grown amount/date widths) by column.
    #[must_use]
    pub fn compute(columns: &[Column], widths: &[(ColumnId, u16)], available: f32) -> ColumnLayout {
        let width_of = |c: &Column| -> (f32, f32, f32) {
            let min = f32::from(c.width.min);
            let set = widths
                .iter()
                .find(|(id, _)| *id == c.id)
                .map(|(_, w)| f32::from(*w));
            let preferred = set.unwrap_or(f32::from(c.width.preferred)).max(min);
            let flex = if set.is_some() {
                0.0
            } else {
                f32::from(c.width.flex)
            };
            (min, preferred, flex)
        };
        let first: Vec<&Column> = columns.iter().filter(|c| c.line == Line::First).collect();
        let specs: Vec<(f32, f32, f32)> = first.iter().map(|c| width_of(c)).collect();
        let preferred_sum: f32 = specs.iter().map(|s| s.1).sum();
        let flex_sum: f32 = specs.iter().map(|s| s.2).sum();
        let extra = (available - preferred_sum).max(0.0);
        // Short of room: the flexible (text) columns give up width first,
        // down to their minimum, in proportion to what each can give; fixed
        // columns (dates, amounts, resized ones) keep theirs.
        let short = (preferred_sum - available).max(0.0);
        let give_sum: f32 = specs.iter().filter(|s| s.2 > 0.0).map(|s| s.1 - s.0).sum();
        let take = short.min(give_sum);
        let mut x = 0.0;
        let mut line1 = Vec::with_capacity(first.len());
        for (c, (min, preferred, flex)) in first.iter().zip(&specs) {
            let w = if *flex > 0.0 && take > 0.0 && give_sum > 0.0 {
                preferred - take * (preferred - min) / give_sum
            } else if flex_sum > 0.0 {
                preferred + extra * flex / flex_sum
            } else {
                *preferred
            };
            line1.push(ColumnPlace {
                id: c.id,
                x,
                width: w,
                line: Line::First,
            });
            x += w;
        }
        let content_width = x;
        let mut places = Vec::with_capacity(columns.len());
        let mut last_line1_x = 0.0;
        let mut l1 = line1.iter();
        for c in columns {
            match c.line {
                Line::First => {
                    if let Some(p) = l1.next() {
                        last_line1_x = p.x;
                        places.push(*p);
                    }
                }
                Line::Second => {
                    let (min, preferred, flex) = width_of(c);
                    let w = if flex > 0.0 {
                        (content_width - last_line1_x).max(min)
                    } else {
                        preferred
                    };
                    places.push(ColumnPlace {
                        id: c.id,
                        x: last_line1_x,
                        width: w,
                        line: Line::Second,
                    });
                }
            }
        }
        ColumnLayout {
            two_lines: columns.iter().any(|c| c.line == Line::Second),
            places,
            content_width,
        }
    }

    /// The place of one column.
    #[must_use]
    pub fn place(&self, id: ColumnId) -> Option<&ColumnPlace> {
        self.places.iter().find(|p| p.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Align, CellKind, WidthSpec};
    use cl_model::TextKey;

    fn col(id: u16, line: Line, min: u16, pref: u16, flex: u8) -> Column {
        Column {
            id: ColumnId(id),
            header: TextKey("h"),
            placeholder: None,
            help: None,
            kind: CellKind::Display,
            line,
            align: Align::Left,
            width: WidthSpec {
                min,
                preferred: pref,
                flex,
            },
            editable: false,
            mono: false,
        }
    }

    #[test]
    fn flex_shares_extra_width_and_line_two_sits_under_its_column() {
        let cols = [
            col(1, Line::First, 80, 100, 0),
            col(2, Line::First, 60, 100, 1),
            col(3, Line::Second, 40, 60, 0),
            col(4, Line::First, 60, 100, 3),
            col(5, Line::Second, 80, 100, 1),
        ];
        let l = ColumnLayout::compute(&cols, &[], 700.0);
        assert!(l.two_lines);
        assert_eq!(l.content_width, 700.0);
        assert_eq!(l.place(ColumnId(2)).unwrap().width, 200.0);
        assert_eq!(l.place(ColumnId(4)).unwrap().width, 400.0);
        assert_eq!(l.place(ColumnId(3)).unwrap().x, 100.0, "under column 2");
        let five = l.place(ColumnId(5)).unwrap();
        assert_eq!(
            (five.x, five.width),
            (300.0, 400.0),
            "under column 4, to the end"
        );
    }

    #[test]
    fn too_narrow_overflows_instead_of_squeezing_and_resized_widths_win() {
        let cols = [
            col(1, Line::First, 80, 100, 1),
            col(2, Line::First, 80, 100, 1),
        ];
        let l = ColumnLayout::compute(&cols, &[], 120.0);
        assert_eq!(
            l.content_width, 160.0,
            "flexible columns shrink to their minimum, then the grid scrolls"
        );
        let l = ColumnLayout::compute(&cols, &[(ColumnId(1), 150)], 400.0);
        assert_eq!(l.place(ColumnId(1)).unwrap().width, 150.0);
        assert_eq!(l.place(ColumnId(2)).unwrap().width, 250.0);
        let l = ColumnLayout::compute(&cols, &[(ColumnId(1), 10)], 400.0);
        assert_eq!(l.place(ColumnId(1)).unwrap().width, 80.0, "never below min");
        assert!(l.place(ColumnId(9)).is_none());
    }

    #[test]
    fn short_of_room_only_flexible_columns_give_width() {
        let cols = [
            col(1, Line::First, 80, 100, 0),
            col(2, Line::First, 80, 240, 1),
            col(3, Line::Second, 80, 240, 1),
            col(4, Line::First, 60, 110, 0),
        ];
        // Preferred 450; 370 available: the flexible column gives 80.
        let l = ColumnLayout::compute(&cols, &[], 370.0);
        assert_eq!(l.content_width, 370.0);
        assert_eq!(l.place(ColumnId(1)).unwrap().width, 100.0);
        assert_eq!(l.place(ColumnId(2)).unwrap().width, 160.0);
        assert_eq!(
            l.place(ColumnId(4)).unwrap().width,
            110.0,
            "amounts keep theirs"
        );
        assert_eq!(
            l.place(ColumnId(3)).unwrap().width,
            270.0,
            "line two runs to the end"
        );
    }
}
