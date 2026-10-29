//! Standard check stock: three checks per sheet at a fixed 88.9 mm pitch,
//! always from position 1; positions left over on the last sheet stay
//! blank (owner 2026-10-29).

use super::{CheckData, CheckOptions, place_check};
use crate::PrintError;
use crate::layout::Page;

/// Top of each check position on the sheet.
pub const POSITION_TOPS: [f32; 3] = [0.0, 88.9, 177.8];

/// Pages for `checks`, three per sheet.
pub(super) fn pages(checks: &[CheckData], opts: &CheckOptions) -> Result<Vec<Page>, PrintError> {
    checks
        .chunks(POSITION_TOPS.len())
        .map(|sheet| {
            let mut page = Page::default();
            for (slot, c) in sheet.iter().enumerate() {
                let pos = u8::try_from(slot + 1).unwrap_or(1);
                place_check(&mut page, c, pos, POSITION_TOPS[slot], opts)?;
            }
            Ok(page)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::tests::{assert_check_at, origin, sample};
    use super::super::*;
    use super::POSITION_TOPS;
    use crate::fonts::MM_PER_PT;
    use crate::layout::{LETTER_H_MM, resolve};
    use crate::pdf::read_back::{content_streams, text_origins};
    use crate::pdf::{pt, render_resolved};

    fn standard(off: AlignmentOffset) -> CheckOptions {
        CheckOptions {
            style: CheckStyle::Standard,
            offset: off,
            ..CheckOptions::default()
        }
    }

    fn assert_positions(off: AlignmentOffset) {
        let checks: Vec<CheckData> = (0..3).map(sample).collect();
        let doc = layout_checks(&checks, &standard(off)).unwrap();
        assert_eq!(doc.pages.len(), 1);
        let page = &resolve(&doc).unwrap()[0];
        for (i, top) in POSITION_TOPS.iter().enumerate() {
            assert_check_at(page, i + 1, *top, off, WINDOW_ON_STANDARD);
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
        let doc = layout_checks(&[sample(1)], &standard(AlignmentOffset::NONE)).unwrap();
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
        assert!((px * MM_PER_PT - x).abs() <= 0.5);
        assert!(((pt(LETTER_H_MM) - py) * MM_PER_PT - y).abs() <= 0.5);
    }

    #[test]
    fn sheets_always_start_at_position_one_and_leftovers_stay_blank() {
        let checks: Vec<CheckData> = (0..4).map(sample).collect();
        let doc = layout_checks(&checks, &standard(AlignmentOffset::NONE)).unwrap();
        assert_eq!(doc.pages.len(), 2);
        for pos in 1..=3 {
            assert!(doc.pages[0].find(&format!("check{pos}.payee")).is_some());
        }
        assert!(doc.pages[1].find("check1.payee").is_some());
        assert!(doc.pages[1].find("check2.payee").is_none());
        assert!(doc.pages[1].find("check3.payee").is_none());
    }

    #[test]
    fn a_negative_amount_cannot_be_printed() {
        let mut c = sample(1);
        c.amount = -c.amount;
        assert_eq!(
            layout_checks(&[c], &standard(AlignmentOffset::NONE)),
            Err(PrintError::NegativeAmount)
        );
    }
}
