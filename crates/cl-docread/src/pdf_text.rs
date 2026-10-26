//! The PDF text layer (WP-B, step 4; feature `worker`): a page's words with
//! positions through `PDFium`, mapped to [`Bbox`] ten-thousandths of the page
//! (y flipped from PDF user space), in reading order; plus a quality score
//! that sends pages with a poor text layer to OCR (R6).
//!
//! One of the three files exempt from `cargo xtask nofloat` (`PDFium` reports
//! points as floats); it may not mention money types. The crate denies
//! float lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use crate::model::{Bbox, Word};
use pdfium_render::prelude::PdfPage;

/// Below this many basis points the page is read with OCR instead.
pub const MIN_TEXT_QUALITY_BP: u16 = 5_000;

/// The page's words, in reading order. Stub (step 2): none.
#[must_use]
pub fn page_words(page: &PdfPage<'_>) -> Vec<Word> {
    let _ = page;
    Vec::new()
}

/// How usable a text layer is, in basis points (share of plausible words,
/// coverage of the page). Stub (step 2): 0, so every page goes to OCR.
#[must_use]
pub fn text_quality(words: &[Word]) -> u16 {
    let _ = (words, Bbox::PAGE);
    0
}
