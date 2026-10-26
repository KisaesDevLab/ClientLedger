//! Page clean-up before OCR (WP-B, step 5; feature `worker`): rotate
//! (OSD/90° turns), deskew, contrast, crop, scale into the OCR limit
//! (`protocol::MAX_PGM_WIDTH` × `MAX_PGM_HEIGHT`), and return the
//! [`Transform`] that maps the result back to the original page.
//!
//! One of the three files exempt from `cargo xtask nofloat` (image
//! geometry); it may not mention money types. The crate denies float
//! lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss
)]

use crate::ocr_geom::Transform;
use crate::protocol::PgmError;
use image::GrayImage;

/// A page ready for OCR.
#[derive(Debug, Clone)]
pub struct Cleaned {
    pub image: GrayImage,
    pub transform: Transform,
}

/// Clean up a page image. Stub (step 2): the image unchanged.
#[must_use]
pub fn clean(page: &GrayImage) -> Cleaned {
    Cleaned {
        image: page.clone(),
        transform: Transform::identity(page.width(), page.height()),
    }
}

/// The cleaned page as an 8-bit binary PGM for `NeedOcr`.
///
/// # Errors
/// The image is over the OCR limit.
pub fn to_pgm(image: &GrayImage) -> Result<Vec<u8>, PgmError> {
    crate::protocol::encode_pgm(image.width(), image.height(), image.as_raw())
}
