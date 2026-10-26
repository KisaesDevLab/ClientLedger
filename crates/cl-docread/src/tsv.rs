//! Tesseract TSV parsing (WP-B, step 5; plan D3 "Float exemption", T15).
//! Scanned by `cargo xtask nofloat`: confidence arrives as a decimal string
//! (`"91.734"`) and becomes basis points with integer arithmetic. Garbage,
//! huge or negative input must stay in range or be refused, never panic.
//!
//! **Consumer:** the worker; the signatures here are fixed for WP-B's own
//! use (no other work package calls them).

use crate::model::Confidence;
use std::fmt;

/// A word's box in the **OCR image's** pixels (before mapping back to the
/// original page with `ocr_geom`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBox {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// One level-5 (word) row of the TSV.
#[derive(Clone, PartialEq, Eq)]
pub struct OcrWord {
    pub text: String,
    pub px: PixelBox,
    pub confidence: Confidence,
    /// Tesseract's block, paragraph and line numbers (layout hints).
    pub block: u32,
    pub paragraph: u32,
    pub line: u32,
}

impl fmt::Debug for OcrWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OcrWord")
            .field("chars", &self.text.chars().count())
            .field("px", &self.px)
            .field("confidence", &self.confidence)
            .finish_non_exhaustive()
    }
}

/// The TSV could not be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TsvError {
    #[error("not tesseract TSV output")]
    NotTsv,
    #[error("too many words")]
    TooManyWords,
}

/// The words of a page image `width` × `height` pixels. Boxes are clamped
/// to the image. Stub (step 2): no words.
///
/// # Errors
/// See [`TsvError`].
pub fn parse(tsv: &str, width: u32, height: u32) -> Result<Vec<OcrWord>, TsvError> {
    let _ = (tsv, width, height);
    Ok(Vec::new())
}
