//! Standard check stock: three checks per sheet at a fixed 88.9 mm pitch.

use super::{CheckData, CheckOptions, place_check};
use crate::PrintError;
use crate::layout::Page;

/// Top of each check position on the sheet.
pub const POSITION_TOPS: [f32; 3] = [0.0, 88.9, 177.8];

/// Pages for `checks`, starting at `opts.first_position` on the first
/// sheet.
pub(super) fn pages(checks: &[CheckData], opts: &CheckOptions) -> Result<Vec<Page>, PrintError> {
    if !(1..=3).contains(&opts.first_position) {
        return Err(PrintError::BadPosition);
    }
    let mut pages = Vec::new();
    let mut page = Page::default();
    let mut slot = usize::from(opts.first_position - 1);
    for c in checks {
        if slot == POSITION_TOPS.len() {
            pages.push(std::mem::take(&mut page));
            slot = 0;
        }
        let pos = u8::try_from(slot + 1).unwrap_or(1);
        place_check(&mut page, c, pos, POSITION_TOPS[slot], opts.offset)?;
        slot += 1;
    }
    pages.push(page);
    Ok(pages)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{origin, sample};
    use super::super::*;
    use super::POSITION_TOPS;
    use crate::fonts::MM_PER_PT;
    use crate::layout::LETTER_H_MM;
    use crate::layout::resolve;
    use crate::pdf::read_back::{content_streams, text_origins};
    use crate::pdf::{pt, render_resolved};

    const TOL: f32 = 0.5;

    /// Every field's first run, at every standard position, against the
    /// spec plus the offset.
    fn assert_positions(off: AlignmentOffset) {
        let checks: Vec<CheckData> = (0..3).map(sample).collect();
        let doc = layout_checks(
            &checks,
            &CheckOptions {
                offset: off,
                ..CheckOptions::default()
            },
        )
        .unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &resolve(&doc).unwrap()[0];
        for (i, top) in POSITION_TOPS.iter().enumerate() {
            let pos = i + 1;
            // The spec, independent of the layout code: spec + top + offset.
            let spec = |r: crate::Rect| r.moved(off.dx_mm(), top + off.dy_mm());
            for (name, font, size, rect) in [
                ("date", Font::Mono, 10.0, FIELDS.date),
                ("payee", Font::Sans, 10.0, FIELDS.payee),
                ("words", Font::Sans, 9.5, FIELDS.words),
                ("address", Font::Sans, 10.0, FIELDS.address),
                ("memo", Font::Sans, 9.0, FIELDS.memo),
            ] {
                let r = page
                    .runs_of(&format!("check{pos}.{name}"))
                    .next()
                    .unwrap_or_else(|| panic!("{pos}.{name}"));
                let (x, y) = origin(spec(rect), font, size);
                assert!((r.x - x).abs() <= TOL, "check{pos}.{name} x {} vs {x}", r.x);
                assert!(
                    (r.baseline - y).abs() <= TOL,
                    "check{pos}.{name} y {} vs {y}",
                    r.baseline
                );
            }
            // The amount ends at the box's right edge.
            let a = page.runs_of(&format!("check{pos}.amount")).next().unwrap();
            let right = a.x + Font::Mono.width_mm(&a.text, 10.0);
            let s = spec(FIELDS.amount);
            assert!(
                (right - s.right()).abs() <= TOL && a.x >= s.x - 0.001,
                "amount {right}"
            );
            assert!((a.baseline - (s.y + Font::Mono.ascent_mm(10.0))).abs() <= TOL);
            assert!(a.text.ends_with("1,234.56") && a.text.starts_with('*'));
            // Every address line lies inside the window (moved with the check).
            let window = WINDOW_ON_STANDARD.moved(off.dx_mm(), top + off.dy_mm());
            let lines: Vec<_> = page.runs_of(&format!("check{pos}.address")).collect();
            assert_eq!(lines.len(), 3);
            for l in lines {
                assert!(
                    l.x >= window.x
                        && l.baseline <= window.bottom()
                        && l.baseline - Font::Sans.ascent_mm(10.0) >= window.y - 0.001
                );
            }
        }
    }

    #[test]
    fn every_field_at_every_position_is_within_half_a_millimetre() {
        assert_positions(AlignmentOffset::NONE);
    }

    #[test]
    fn the_offset_moves_every_field() {
        assert_positions(AlignmentOffset::new(-3.25, 4.5).unwrap());
        assert_positions(AlignmentOffset::new(10.0, -10.0).unwrap());
    }

    #[test]
    fn the_pdf_draws_every_run_where_the_layout_put_it() {
        let doc = layout_checks(&[sample(1)], &CheckOptions::default()).unwrap();
        let pages = resolve(&doc).unwrap();
        let bytes = render_resolved(&pages, None, "Checks").unwrap();
        let streams = content_streams(&bytes);
        assert_eq!(streams.len(), 1);
        let origins = text_origins(&streams[0]);
        assert_eq!(origins.len(), pages[0].runs.len());
        for ((x, y), run) in origins.iter().zip(&pages[0].runs) {
            let (xm, ym) = (x * MM_PER_PT, (pt(LETTER_H_MM) - y) * MM_PER_PT);
            assert!(
                (xm - run.x).abs() < 0.01 && (ym - run.baseline).abs() < 0.01,
                "{}",
                run.tag
            );
        }
        // And against the spec directly: the payee in position 1.
        let payee = pages[0]
            .runs
            .iter()
            .position(|r| r.tag == "check1.payee")
            .unwrap();
        let (x, y) = origin(FIELDS.payee, Font::Sans, 10.0);
        let (px, py) = origins[payee];
        assert!(
            (px * MM_PER_PT - x).abs() <= TOL
                && ((pt(LETTER_H_MM) - py) * MM_PER_PT - y).abs() <= TOL
        );
    }

    #[test]
    fn checks_fill_from_the_first_free_position_and_continue_on_new_sheets() {
        let checks: Vec<CheckData> = (0..5).map(sample).collect();
        let doc = layout_checks(
            &checks,
            &CheckOptions {
                first_position: 3,
                ..CheckOptions::default()
            },
        )
        .unwrap();
        assert_eq!(doc.pages.len(), 3);
        assert!(
            doc.pages[0].find("check3.payee").is_some()
                && doc.pages[0].find("check1.payee").is_none()
        );
        assert!(doc.pages[1].find("check3.payee").is_some());
        assert!(
            doc.pages[2].find("check1.payee").is_some()
                && doc.pages[2].find("check2.payee").is_none()
        );
        for bad in [0, 4] {
            assert_eq!(
                layout_checks(
                    &checks,
                    &CheckOptions {
                        first_position: bad,
                        ..CheckOptions::default()
                    }
                ),
                Err(PrintError::BadPosition)
            );
        }
    }

    #[test]
    fn a_negative_amount_cannot_be_printed() {
        let mut c = sample(1);
        c.amount = -c.amount;
        assert_eq!(
            layout_checks(&[c], &CheckOptions::default()),
            Err(PrintError::NegativeAmount)
        );
    }
}
