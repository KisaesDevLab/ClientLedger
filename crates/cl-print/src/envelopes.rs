//! #10 envelopes (Q-P19aa): one envelope per page, 241.3 × 104.8 mm
//! (9.5 × 4.125 in) landscape, fed through the printer's envelope slot.
//!
//! - Return address (company name and address; logo optional, off by
//!   default) in the top-left corner, 3/8 in from the edges.
//! - Recipient address 4 in from the left and 2 in from the top. That keeps
//!   it inside the USPS OCR read area (at least 1/2 in from the sides, 5/8 in
//!   above the bottom and no more than 2 3/4 in above it) and clear of the
//!   barcode strip along the bottom.

use crate::PrintError;
use crate::fonts::Font;
use crate::forms::CompanyInfo;
use crate::image::Logo;
use crate::layout::{Document, Element, Page, PageSize, Rect, TextBox};

/// The return-address block.
pub const RETURN_ADDRESS: Rect = Rect::new(9.5, 9.5, 95.0, 22.0);
/// The logo box, when the logo is on.
pub const RETURN_LOGO: Rect = Rect::new(9.5, 9.5, 20.0, 12.0);
/// The return address's left edge beside the logo.
pub const RETURN_TEXT_X_WITH_LOGO: f32 = 32.0;
/// The recipient address block.
pub const RECIPIENT: Rect = Rect::new(101.6, 50.8, 120.0, 25.0);
/// The USPS OCR read area on a #10 envelope.
pub const OCR_AREA: Rect = Rect::new(12.7, 104.8 - 69.85, 241.3 - 25.4, 69.85 - 15.875);

/// What prints on every envelope of a batch.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EnvelopeOptions {
    /// The return address; empty name and lines print nothing.
    pub return_address: CompanyInfo,
    /// Print the logo beside the return address (off by default).
    pub show_logo: bool,
    /// The company logo.
    pub logo: Option<Logo>,
}

/// One envelope.
///
/// # Errors
/// As [`layout_envelopes`].
pub fn layout_envelope(
    recipient: &[String],
    opts: &EnvelopeOptions,
) -> Result<Document, PrintError> {
    layout_envelopes(&[recipient.to_vec()], opts)
}

/// One envelope per recipient address (name first).
///
/// # Errors
/// [`PrintError::NoPages`] for no recipients.
pub fn layout_envelopes(
    recipients: &[Vec<String>],
    opts: &EnvelopeOptions,
) -> Result<Document, PrintError> {
    if recipients.is_empty() {
        return Err(PrintError::NoPages);
    }
    let logo = if opts.show_logo {
        opts.logo.clone()
    } else {
        None
    };
    let pages = recipients
        .iter()
        .map(|r| {
            let mut p = Page::default();
            let x = if logo.is_some() {
                p.elements.push(Element::Logo(RETURN_LOGO));
                RETURN_TEXT_X_WITH_LOGO
            } else {
                RETURN_ADDRESS.x
            };
            let w = RETURN_ADDRESS.right() - x;
            p.text(TextBox::new(
                "return.name",
                Rect::new(x, RETURN_ADDRESS.y, w, 4.5),
                Font::SansSemiBold,
                9.0,
                opts.return_address.name.clone(),
            ));
            p.text(TextBox::new(
                "return.lines",
                Rect::new(x, RETURN_ADDRESS.y + 4.5, w, RETURN_ADDRESS.h - 4.5),
                Font::Sans,
                8.5,
                opts.return_address.lines.join("\n"),
            ));
            p.text(TextBox::new(
                "recipient",
                RECIPIENT,
                Font::Sans,
                11.0,
                r.join("\n"),
            ));
            p
        })
        .collect();
    Ok(Document {
        title: "Envelopes".into(),
        size: PageSize::ENVELOPE_10,
        pages,
        logo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::MM_PER_PT;
    use crate::layout::resolve;
    use crate::pdf::read_back::{content_streams, text_origins};
    use crate::pdf::{pt, render_resolved};

    fn opts() -> EnvelopeOptions {
        EnvelopeOptions {
            return_address: CompanyInfo {
                name: "Kisaes Garden Supply".into(),
                lines: vec!["12 Elm Street".into(), "Bloomington, IN 47401".into()],
            },
            ..EnvelopeOptions::default()
        }
    }

    fn addr(n: usize) -> Vec<String> {
        vec![
            format!("Customer {n}"),
            "5 Oak Lane".into(),
            "Columbus, OH 43004-1234".into(),
        ]
    }

    #[test]
    fn addresses_are_within_half_a_millimetre_and_inside_the_ocr_area() {
        let doc = layout_envelopes(&[addr(1), addr(2)], &opts()).unwrap();
        assert_eq!(doc.size, PageSize::ENVELOPE_10);
        let pages = resolve(&doc).unwrap();
        assert_eq!(pages.len(), 2);
        for p in &pages {
            let r: Vec<_> = p.runs_of("recipient").collect();
            assert_eq!(r.len(), 3);
            assert!((r[0].x - 101.6).abs() <= 0.5);
            assert!((r[0].baseline - (50.8 + Font::Sans.ascent_mm(11.0))).abs() <= 0.5);
            for l in &r {
                let right = l.x + Font::Sans.width_mm(&l.text, 11.0);
                let top = l.baseline - Font::Sans.ascent_mm(11.0);
                assert!(OCR_AREA.contains(Rect::new(
                    l.x,
                    top,
                    right - l.x,
                    l.baseline - top + 1.0
                )));
            }
            let n = p.runs_of("return.name").next().unwrap();
            assert!((n.x - 9.5).abs() <= 0.5);
            assert!((n.baseline - (9.5 + Font::SansSemiBold.ascent_mm(9.0))).abs() <= 0.5);
            assert_eq!(p.runs_of("return.lines").count(), 2);
            assert_eq!(p.logos.len(), 0);
        }
    }

    #[test]
    fn the_pdf_page_is_envelope_sized_and_text_lands_where_laid_out() {
        let doc = layout_envelope(&addr(1), &opts()).unwrap();
        let pages = resolve(&doc).unwrap();
        let bytes = render_resolved(&pages, None, "Envelopes").unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(
            s.contains(&format!(
                "/MediaBox [0 0 {} {}]",
                fmt(pt(241.3)),
                fmt(pt(104.8))
            )),
            "media box"
        );
        let origins = text_origins(&content_streams(&bytes)[0]);
        for ((x, y), run) in origins.iter().zip(&pages[0].runs) {
            assert!((x * MM_PER_PT - run.x).abs() < 0.01);
            assert!(((pt(104.8) - y) * MM_PER_PT - run.baseline).abs() < 0.01);
        }
    }

    /// pdf-writer's number format for these values.
    fn fmt(v: f32) -> String {
        format!("{v}")
    }

    #[test]
    fn the_logo_is_off_by_default_and_moves_the_return_address_when_on() {
        let mut o = opts();
        o.logo = Some(Logo::from_bytes(&crate::image::tests::png_rgba(4, 2)).unwrap());
        let off = resolve(&layout_envelope(&addr(1), &o).unwrap()).unwrap();
        assert_eq!(off[0].logos.len(), 0);
        o.show_logo = true;
        let p = &resolve(&layout_envelope(&addr(1), &o).unwrap()).unwrap()[0];
        assert_eq!(p.logos.len(), 1);
        assert!(
            (p.runs_of("return.name").next().unwrap().x - RETURN_TEXT_X_WITH_LOGO).abs() < 0.001
        );
        assert_eq!(layout_envelopes(&[], &o), Err(PrintError::NoPages));
    }
}
