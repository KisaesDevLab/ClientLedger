//! Page clean-up before OCR (WP-B, step 5; feature `worker`): contrast and
//! scale into the OCR limit (`protocol::MAX_PGM_WIDTH` × `MAX_PGM_HEIGHT`),
//! returning the [`Transform`] that maps the result back to the original
//! page.
//!
//! **Deferred to WP-A2 (plan R2):** orientation detection (OSD needs a
//! tesseract run the frozen protocol has no frame for), deskew and
//! cropping. The [`Transform`] and `ocr_geom` already carry crop and
//! rotation, so adding them changes only this file.
//!
//! One of the three files exempt from `cargo xtask nofloat` (image
//! geometry); it may not mention money types. The crate denies float
//! lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::ocr_geom::Transform;
use crate::protocol::{MAX_PGM_HEIGHT, MAX_PGM_WIDTH, PgmError};
use image::GrayImage;
use image::imageops::FilterType;

/// Pages narrower than this are enlarged (up to twice) so small print
/// reaches the size tesseract reads best.
pub const MIN_OCR_WIDTH: u32 = 1_200;

/// A page ready for OCR.
#[derive(Debug, Clone)]
pub struct Cleaned {
    pub image: GrayImage,
    pub transform: Transform,
}

/// Clean up a page image: stretch its contrast, then scale it into the OCR
/// limit (or up, when it is small). An empty image is returned as is.
#[must_use]
pub fn clean(page: &GrayImage) -> Cleaned {
    let (w, h) = page.dimensions();
    if w == 0 || h == 0 {
        return Cleaned {
            image: page.clone(),
            transform: Transform::identity(w, h),
        };
    }
    let mut image = page.clone();
    stretch_contrast(&mut image);
    let factor = scale_factor(w, h);
    let mut transform = Transform::identity(w, h);
    if (factor - 1.0).abs() > f64::EPSILON {
        let nw = ((f64::from(w) * factor).round() as u32).clamp(1, MAX_PGM_WIDTH);
        let nh = ((f64::from(h) * factor).round() as u32).clamp(1, MAX_PGM_HEIGHT);
        image = image::imageops::resize(&image, nw, nh, FilterType::Triangle);
        transform.scale = (factor * 10_000.0).round().max(1.0) as u32;
        transform.ocr_width = nw;
        transform.ocr_height = nh;
    }
    Cleaned { image, transform }
}

/// How much to scale a `w` × `h` page: down to fit the OCR limit, up (at
/// most twice) when narrower than [`MIN_OCR_WIDTH`], else 1.
#[must_use]
pub fn scale_factor(w: u32, h: u32) -> f64 {
    let (fw, fh) = (f64::from(w.max(1)), f64::from(h.max(1)));
    let fit = (f64::from(MAX_PGM_WIDTH) / fw).min(f64::from(MAX_PGM_HEIGHT) / fh);
    if fit < 1.0 {
        return fit;
    }
    if w < MIN_OCR_WIDTH {
        return (f64::from(MIN_OCR_WIDTH) / fw).min(2.0).min(fit);
    }
    1.0
}

/// Stretch grey levels so the darkest 1 % become black and the lightest 1 %
/// white. A nearly flat page (a blank one) is left alone.
pub fn stretch_contrast(image: &mut GrayImage) {
    let mut hist = [0u64; 256];
    for p in image.pixels() {
        hist[usize::from(p.0[0])] += 1;
    }
    let total: u64 = hist.iter().sum();
    let cut = total / 100;
    let level = |from_dark: bool| -> u8 {
        let mut seen = 0u64;
        for i in 0..256usize {
            let idx = if from_dark { i } else { 255 - i };
            seen += hist[idx];
            if seen > cut {
                return u8::try_from(idx).unwrap_or(0);
            }
        }
        if from_dark { 0 } else { 255 }
    };
    let (lo, hi) = (level(true), level(false));
    if hi <= lo || hi - lo < 32 || (lo == 0 && hi == 255) {
        return;
    }
    let (lo, span) = (u32::from(lo), u32::from(hi - lo));
    for p in image.pixels_mut() {
        let v = u32::from(p.0[0]).saturating_sub(lo);
        p.0[0] = u8::try_from((v * 255 / span).min(255)).unwrap_or(255);
    }
}

/// The cleaned page as an 8-bit binary PGM for `NeedOcr`.
///
/// # Errors
/// The image is over the OCR limit.
pub fn to_pgm(image: &GrayImage) -> Result<Vec<u8>, PgmError> {
    crate::protocol::encode_pgm(image.width(), image.height(), image.as_raw())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::validate_pgm;

    #[test]
    fn large_pages_shrink_into_the_limit_and_map_back() {
        let page = GrayImage::from_pixel(6_800, 8_800, image::Luma([255]));
        let c = clean(&page);
        assert_eq!(c.image.dimensions(), (3_400, 4_400));
        assert_eq!(c.transform.scale, 5_000);
        assert_eq!(
            (c.transform.ocr_width, c.transform.ocr_height),
            (3_400, 4_400)
        );
        assert_eq!(c.transform.original_width, 6_800);
        let pgm = to_pgm(&c.image).unwrap();
        assert_eq!(validate_pgm(&pgm), Ok((3_400, 4_400)));
    }

    #[test]
    fn small_pages_grow_and_letter_pages_stay() {
        assert_eq!(scale_factor(2_550, 3_300), 1.0);
        assert_eq!(scale_factor(400, 900), 2.0);
        assert!((scale_factor(1_000, 1_000) - 1.2).abs() < 1e-9);
        // Long and thin (a receipt): growing must still fit.
        let f = scale_factor(600, 4_000);
        assert!(f64::from(4_000u32) * f <= f64::from(MAX_PGM_HEIGHT));
        let c = clean(&GrayImage::from_pixel(500, 700, image::Luma([200])));
        assert_eq!(c.image.dimensions(), (1_000, 1_400));
        assert_eq!(c.transform.scale, 20_000);
    }

    #[test]
    fn contrast_is_stretched_but_a_blank_page_is_left_alone() {
        let mut faint = GrayImage::from_fn(100, 100, |x, _| {
            image::Luma([if x < 10 { 120 } else { 200 }])
        });
        stretch_contrast(&mut faint);
        assert_eq!(faint.get_pixel(0, 0).0[0], 0);
        assert_eq!(faint.get_pixel(50, 0).0[0], 255);
        let mut blank = GrayImage::from_pixel(50, 50, image::Luma([250]));
        stretch_contrast(&mut blank);
        assert!(blank.pixels().all(|p| p.0[0] == 250));
        let empty = clean(&GrayImage::new(0, 0));
        assert_eq!(empty.image.dimensions(), (0, 0));
    }
}
