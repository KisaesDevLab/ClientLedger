//! Region re-read (protocol version 4; A04 D4, Q-A04i, review S9): OCR of
//! one box the owner dragged on a page, in this sandboxed worker only.
//!
//! The page is rendered at OCR resolution and cleaned up exactly as for a
//! full read (contrast, a photographed sheet flattened, deskew), then the
//! host is asked for its orientation, so a quarter-turned scan is turned
//! first. The owner's box (on the original page) is mapped through that
//! clean-up onto the cleaned page, cut out with a small margin, enlarged
//! when small, and OCR'd on its own; each word's box is mapped back onto
//! the original page. The caller keeps only the words whose centre lies in
//! the box (`cl_docread::pipeline::region_reading`).
//!
//! Integer geometry except the clean-up's own point maps (`ocr_geom`).
//! No amount is read or written here: words only.

use crate::batch::{Fatal, Io, numbers_unsure, ocr_raw_image, orientation};
use crate::second_read;
use cl_docread::model::{Bbox, Word};
use cl_docread::ocr_geom;
use cl_docread::preprocess::{self, Cleaned};
use cl_docread::protocol::{JobId, MAX_PGM_HEIGHT, MAX_PGM_WIDTH, OcrMode};
use cl_docread::tsv::PixelBox;
use image::GrayImage;
use image::imageops::FilterType;

/// White border around the cut-out, pixels (tesseract reads text that
/// touches the image edge poorly).
const BORDER: u32 = 16;
/// Least margin added around the box on the cleaned page, pixels. The
/// margin is wider across (a twelfth of the page) than down (a hundredth),
/// so a word the box edge cuts through is read whole, not as a fragment;
/// it is dropped later unless its centre is inside the box.
const MARGIN: u32 = 12;
/// Page width over the side margin.
const SIDE_MARGIN_PARTS: u32 = 12;
/// Page height over the top and bottom margin.
const END_MARGIN_PARTS: u32 = 100;
/// A cut-out shorter than this is enlarged (up to [`MAX_ENLARGE`] times).
const SMALL_HEIGHT: u32 = 160;
const MAX_ENLARGE: u32 = 3;

/// Where a cut-out came from on the cleaned page, to map its words back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cut {
    /// Top-left corner on the cleaned page.
    pub left: u32,
    pub top: u32,
    /// Size on the cleaned page.
    pub width: u32,
    pub height: u32,
    /// Enlargement (1 to [`MAX_ENLARGE`]).
    pub scale: u32,
}

/// OCR the part of the page `grey` inside `region` (ten-thousandths of the
/// original page). `None` when nothing could be read; the words are on the
/// original page, not yet filtered to the box.
///
/// # Errors
/// The pipe broke or the host breached the protocol.
pub(crate) fn ocr_region(
    io: &mut dyn Io,
    (job, page): (JobId, u16),
    grey: &GrayImage,
    region: Bbox,
) -> Result<Option<Vec<Word>>, Fatal> {
    let mut cleaned = preprocess::clean(grey);
    // A small box can't tell which way up the page is: ask for the whole
    // page's orientation every time.
    if let Some(quarters) = orientation(io, job, page, &cleaned)? {
        cleaned = preprocess::clean_turned(grey, quarters);
    }
    let Some(cut) = cut_box(&cleaned, region) else {
        return Ok(None);
    };
    let Some(image) = cut_out(&cleaned.image, cut) else {
        return Ok(None);
    };
    let Some(mut words) = ocr_raw_image(io, (job, page), &image, OcrMode::Read)? else {
        return Ok(None);
    };
    if numbers_unsure(&words)
        && let Some(better) = ocr_raw_image(io, (job, page), &image, OcrMode::ReadBest)?
    {
        // The amounts the second read gives differently are not reported:
        // a region answer carries words only.
        words = second_read::merge(words, better).0;
    }
    Ok(Some(
        words
            .into_iter()
            .map(|w| Word {
                bbox: ocr_geom::to_page(back(w.px, cut), &cleaned.transform),
                text: w.text,
                confidence: w.confidence,
            })
            .collect(),
    ))
}

/// The box on the cleaned page holding `region` (its four corners mapped
/// through the clean-up), grown by its margins and clamped to the page, and
/// the enlargement it needs. `None` when it is empty.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_arithmetic,
    clippy::many_single_char_names
)]
pub(crate) fn cut_box(cleaned: &Cleaned, region: Bbox) -> Option<Cut> {
    let t = &cleaned.transform;
    let (iw, ih) = cleaned.image.dimensions();
    if t.original_width == 0 || t.original_height == 0 || iw == 0 || ih == 0 {
        return None;
    }
    let px = |bp: u16, side: u32| f64::from(bp) * f64::from(side) / f64::from(Bbox::SCALE);
    let (l, r) = (
        px(region.left(), t.original_width),
        px(region.right(), t.original_width),
    );
    let (top, b) = (
        px(region.top(), t.original_height),
        px(region.bottom(), t.original_height),
    );
    let m = t.mapper();
    let corners = [(l, top), (r, top), (l, b), (r, b)].map(|(x, y)| m.to_ocr(x, y));
    let clamp = |v: f64, max: u32| {
        if v.is_finite() {
            v.clamp(0.0, f64::from(max)) as u32
        } else {
            0
        }
    };
    let xs = corners.map(|c| clamp(c.0, iw));
    let ys = corners.map(|c| clamp(c.1, ih));
    let (mx, my) = (
        (iw / SIDE_MARGIN_PARTS).max(MARGIN),
        (ih / END_MARGIN_PARTS).max(MARGIN),
    );
    let x0 = xs.iter().min()?.saturating_sub(mx);
    let y0 = ys.iter().min()?.saturating_sub(my);
    let x1 = xs.iter().max()?.saturating_add(mx).min(iw);
    let y1 = ys.iter().max()?.saturating_add(my).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (width, height) = (x1 - x0, y1 - y0);
    let mut scale = 1;
    while scale < MAX_ENLARGE
        && height * scale < SMALL_HEIGHT
        && (width * (scale + 1) + 2 * BORDER) <= MAX_PGM_WIDTH
        && (height * (scale + 1) + 2 * BORDER) <= MAX_PGM_HEIGHT
    {
        scale += 1;
    }
    Some(Cut {
        left: x0,
        top: y0,
        width,
        height,
        scale,
    })
}

/// The cut-out, enlarged, on a white [`BORDER`]; `None` when it would be
/// over the OCR limit.
pub(crate) fn cut_out(image: &GrayImage, cut: Cut) -> Option<GrayImage> {
    let (iw, ih) = image.dimensions();
    if cut.width == 0
        || cut.height == 0
        || cut.left.checked_add(cut.width)? > iw
        || cut.top.checked_add(cut.height)? > ih
    {
        return None;
    }
    let (sw, sh) = (
        cut.width.checked_mul(cut.scale)?,
        cut.height.checked_mul(cut.scale)?,
    );
    let (ow, oh) = (sw.checked_add(2 * BORDER)?, sh.checked_add(2 * BORDER)?);
    if ow > MAX_PGM_WIDTH || oh > MAX_PGM_HEIGHT {
        return None;
    }
    let crop =
        image::imageops::crop_imm(image, cut.left, cut.top, cut.width, cut.height).to_image();
    let crop = if cut.scale > 1 {
        image::imageops::resize(&crop, sw, sh, FilterType::CatmullRom)
    } else {
        crop
    };
    let mut out = GrayImage::from_pixel(ow, oh, image::Luma([255]));
    image::imageops::overlay(&mut out, &crop, i64::from(BORDER), i64::from(BORDER));
    Some(out)
}

/// A word's box on the cut-out back onto the cleaned page.
pub(crate) fn back(px: PixelBox, cut: Cut) -> PixelBox {
    let s = cut.scale.max(1);
    let left = px.left.saturating_sub(BORDER) / s;
    let top = px.top.saturating_sub(BORDER) / s;
    let right = px
        .left
        .saturating_add(px.width)
        .saturating_sub(BORDER)
        .div_ceil(s)
        .min(cut.width);
    let bottom = px
        .top
        .saturating_add(px.height)
        .saturating_sub(BORDER)
        .div_ceil(s)
        .min(cut.height);
    let (left, top) = (left.min(right), top.min(bottom));
    PixelBox {
        left: cut.left + left,
        top: cut.top + top,
        width: right - left,
        height: bottom - top,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_docread::ocr_geom::Transform;

    fn cleaned(w: u32, h: u32) -> Cleaned {
        Cleaned {
            image: GrayImage::from_pixel(w, h, image::Luma([255])),
            transform: Transform::identity(w, h),
        }
    }

    #[test]
    fn the_box_maps_onto_the_cleaned_page_with_a_margin() {
        let c = cleaned(2_000, 3_000);
        let cut = cut_box(&c, Bbox::new(1_000, 1_000, 2_000, 1_500).unwrap()).unwrap();
        // Margins: a twelfth of the width across, a hundredth of the height.
        assert_eq!((cut.left, cut.top), (200 - 166, 300 - 30));
        assert_eq!((cut.width, cut.height), (200 + 2 * 166, 150 + 2 * 30));
        assert_eq!(cut.scale, 1);
        // A box at the page's corner is clamped to the page.
        let cut = cut_box(&c, Bbox::new(9_000, 9_500, 10_000, 10_000).unwrap()).unwrap();
        assert_eq!(cut.left + cut.width, 2_000);
        assert_eq!(cut.top + cut.height, 3_000);
        // A thin box is enlarged, within the OCR limit.
        let cut = cut_box(&c, Bbox::new(0, 0, 3_000, 100).unwrap()).unwrap();
        assert!(cut.scale > 1, "{cut:?}");
        let img = cut_out(&c.image, cut).unwrap();
        assert!(img.width() <= MAX_PGM_WIDTH && img.height() <= MAX_PGM_HEIGHT);
    }

    #[test]
    fn a_turned_page_maps_the_box_through_the_turn() {
        // A page turned a quarter (its OCR canvas has the sides swapped).
        let mut t = Transform::identity(2_000, 1_000);
        t.rotation_mdeg = 90_000;
        t.ocr_width = 1_000;
        t.ocr_height = 2_000;
        let c = Cleaned {
            image: GrayImage::from_pixel(1_000, 2_000, image::Luma([255])),
            transform: t,
        };
        // The left tenth of the original page.
        let cut = cut_box(&c, Bbox::new(0, 0, 1_000, 10_000).unwrap()).unwrap();
        // Turned clockwise, the left edge becomes the top: a wide, short cut.
        assert!(cut.width > cut.height, "{cut:?}");
        assert!(cut.top <= 20, "{cut:?}");
    }

    #[test]
    fn word_boxes_map_back_off_the_cut_out() {
        let cut = Cut {
            left: 100,
            top: 50,
            width: 300,
            height: 40,
            scale: 2,
        };
        let px = PixelBox {
            left: BORDER + 20,
            top: BORDER + 10,
            width: 60,
            height: 30,
        };
        assert_eq!(
            back(px, cut),
            PixelBox {
                left: 110,
                top: 55,
                width: 30,
                height: 15
            }
        );
        // A hostile box past the cut-out stays inside it.
        let wild = PixelBox {
            left: u32::MAX - 5,
            top: u32::MAX - 5,
            width: u32::MAX,
            height: 7,
        };
        let b = back(wild, cut);
        assert!(b.left + b.width <= cut.left + cut.width);
        assert!(b.top + b.height <= cut.top + cut.height);
        // Inside the border only: an empty box at the cut's corner.
        let edge = back(
            PixelBox {
                left: 0,
                top: 0,
                width: 3,
                height: 3,
            },
            cut,
        );
        assert_eq!((edge.width, edge.height), (0, 0));
    }

    #[test]
    fn an_empty_or_oversized_cut_is_refused() {
        let img = GrayImage::from_pixel(100, 100, image::Luma([255]));
        let base = Cut {
            left: 0,
            top: 0,
            width: 10,
            height: 10,
            scale: 1,
        };
        assert!(cut_out(&img, base).is_some());
        assert!(cut_out(&img, Cut { width: 0, ..base }).is_none());
        assert!(cut_out(&img, Cut { left: 95, ..base }).is_none());
        assert!(
            cut_out(
                &img,
                Cut {
                    left: u32::MAX,
                    ..base
                }
            )
            .is_none()
        );
        let big = GrayImage::from_pixel(3_400, 100, image::Luma([255]));
        assert!(
            cut_out(
                &big,
                Cut {
                    width: 3_400,
                    ..base
                }
            )
            .is_none(),
            "the border would pass the OCR limit"
        );
    }
}
