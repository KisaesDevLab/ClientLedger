//! OCR geometry (WP-B, step 5; plan D3 "Spots map back"): maps every TSV
//! box from the cleaned-up OCR image back to the **original** page, through
//! the [`Transform`] `preprocess` returns, so A04's highlights land where
//! the viewer shows the page.
//!
//! One of the three files exempt from `cargo xtask nofloat` (float
//! geometry); it may not mention money types (an xtask test checks that).
//! The crate denies float lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use crate::model::Bbox;
use crate::tsv::PixelBox;

/// What the clean-up did to the original page image, integers only (plan
/// D3): applied in the order crop → rotate → scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    /// The original page image's size in pixels.
    pub original_width: u32,
    pub original_height: u32,
    /// Crop applied first, in original pixels.
    pub crop_left: u32,
    pub crop_top: u32,
    pub crop_width: u32,
    pub crop_height: u32,
    /// Rotation about the crop's centre, millidegrees clockwise (deskew and
    /// 90° turns).
    pub rotation_mdeg: i32,
    /// OCR image pixels per original pixel, per 10,000.
    pub scale: u32,
    /// The OCR image's size in pixels.
    pub ocr_width: u32,
    pub ocr_height: u32,
}

impl Transform {
    /// No change: the OCR image is the original page image.
    #[must_use]
    pub const fn identity(width: u32, height: u32) -> Transform {
        Transform {
            original_width: width,
            original_height: height,
            crop_left: 0,
            crop_top: 0,
            crop_width: width,
            crop_height: height,
            rotation_mdeg: 0,
            scale: 10_000,
            ocr_width: width,
            ocr_height: height,
        }
    }
}

/// The box on the original page for a box on the OCR image. Stub (step 2):
/// the whole page.
#[must_use]
pub fn to_page(px: PixelBox, transform: &Transform) -> Bbox {
    let _ = (px, transform);
    Bbox::PAGE
}
