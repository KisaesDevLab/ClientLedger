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
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use crate::model::Bbox;
use crate::tsv::PixelBox;

/// What the clean-up did to the original page image, integers only (plan
/// D3): applied in the order crop → rotate → scale.
///
/// The crop is a rectangle, or, with [`Transform::quad`], a four-sided
/// region (a photographed sheet of paper, keystoned and turned) stretched
/// onto a `crop_width` × `crop_height` rectangle by a perspective map.
///
/// Rotation turns the crop about its centre into a canvas whose centre is
/// the crop's: the canvas is the crop's size, or its sides swapped when the
/// rotation is an odd number of quarter turns (rounded to the nearest
/// quarter). Clockwise as the page is seen (y down).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    /// The original page image's size in pixels.
    pub original_width: u32,
    pub original_height: u32,
    /// Crop applied first, in original pixels (ignored with a `quad`).
    pub crop_left: u32,
    pub crop_top: u32,
    pub crop_width: u32,
    pub crop_height: u32,
    /// The paper's corners in original pixels — top left, top right,
    /// bottom right, bottom left — mapped onto the crop rectangle's
    /// corners.
    pub quad: Option<[[u32; 2]; 4]>,
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
            quad: None,
            rotation_mdeg: 0,
            scale: 10_000,
            ocr_width: width,
            ocr_height: height,
        }
    }

    /// Whether the rotation swaps the canvas sides.
    #[must_use]
    pub fn swaps(&self) -> bool {
        let quarters = (f64::from(self.rotation_mdeg) / 90_000.0).round() as i64;
        quarters.rem_euclid(2) == 1
    }

    /// The rotated canvas size in (crop) pixels.
    #[must_use]
    pub fn canvas(&self) -> (u32, u32) {
        if self.swaps() {
            (self.crop_height, self.crop_width)
        } else {
            (self.crop_width, self.crop_height)
        }
    }

    /// The point maps, computed once (sine, cosine, perspective).
    #[must_use]
    pub fn mapper(&self) -> Mapper {
        let (rw, rh) = self.canvas();
        let (sin, cos) = (f64::from(self.rotation_mdeg) / 1000.0)
            .to_radians()
            .sin_cos();
        let (cw, ch) = (f64::from(self.crop_width), f64::from(self.crop_height));
        let quad = self.quad.and_then(|q| {
            let p = q.map(|[x, y]| (f64::from(x), f64::from(y)));
            Homography::square_to_quad(p).map(|h| (h, h.inverse()))
        });
        Mapper {
            canvas: (f64::from(rw), f64::from(rh)),
            crop: (cw, ch),
            origin: (f64::from(self.crop_left), f64::from(self.crop_top)),
            sin,
            cos,
            scale: f64::from(self.scale.max(1)) / 10_000.0,
            quad,
        }
    }

    /// A point on the original image (pixels) to the OCR image.
    #[must_use]
    pub fn to_ocr(&self, x: f64, y: f64) -> (f64, f64) {
        self.mapper().to_ocr(x, y)
    }

    /// A point on the OCR image to the original image (pixels).
    #[must_use]
    pub fn to_original(&self, x: f64, y: f64) -> (f64, f64) {
        self.mapper().to_original(x, y)
    }
}

/// A projective map of the unit square (Heckbert's square-to-quad), as
/// the 3 × 3 matrix `[a b c; d e f; g h 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography([f64; 9]);

impl Homography {
    /// The map taking (0,0), (1,0), (1,1), (0,1) to `p[0..4]`; `None` for a
    /// degenerate quad.
    #[must_use]
    pub fn square_to_quad(p: [(f64, f64); 4]) -> Option<Homography> {
        let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = p;
        let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
        let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
        let m = if dx3.abs() < 1e-9 && dy3.abs() < 1e-9 {
            [x1 - x0, x2 - x1, x0, y1 - y0, y2 - y1, y0, 0.0, 0.0, 1.0]
        } else {
            let det = dx1 * dy2 - dx2 * dy1;
            if det.abs() < 1e-12 {
                return None;
            }
            let g = (dx3 * dy2 - dx2 * dy3) / det;
            let h = (dx1 * dy3 - dx3 * dy1) / det;
            [
                x1 - x0 + g * x1,
                x3 - x0 + h * x3,
                x0,
                y1 - y0 + g * y1,
                y3 - y0 + h * y3,
                y0,
                g,
                h,
                1.0,
            ]
        };
        let hm = Homography(m);
        (hm.det().abs() > 1e-12 && m.iter().all(|v| v.is_finite())).then_some(hm)
    }

    fn det(&self) -> f64 {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
    }

    /// The inverse map (adjugate; the scale cancels in [`Self::apply`]).
    #[must_use]
    pub fn inverse(&self) -> Homography {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        Homography([
            e * i - f * h,
            c * h - b * i,
            b * f - c * e,
            f * g - d * i,
            a * i - c * g,
            c * d - a * f,
            d * h - e * g,
            b * g - a * h,
            a * e - b * d,
        ])
    }

    /// Map a point.
    #[must_use]
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f, g, h, i] = self.0;
        let w = g * x + h * y + i;
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        ((a * x + b * y + c) / w, (d * x + e * y + f) / w)
    }
}

/// [`Transform`]'s point maps with everything per-transform computed once
/// (the clean-up maps every OCR pixel).
#[derive(Debug, Clone, Copy)]
pub struct Mapper {
    canvas: (f64, f64),
    crop: (f64, f64),
    origin: (f64, f64),
    sin: f64,
    cos: f64,
    scale: f64,
    /// Unit square → paper, and back.
    quad: Option<(Homography, Homography)>,
}

impl Mapper {
    /// A point on the original image (pixels) to the OCR image.
    #[must_use]
    pub fn to_ocr(&self, x: f64, y: f64) -> (f64, f64) {
        let (cw, ch) = self.crop;
        let (u, v) = match &self.quad {
            Some((_, back)) => {
                let (s, t) = back.apply(x, y);
                (s * cw, t * ch)
            }
            None => (x - self.origin.0, y - self.origin.1),
        };
        let (dx, dy) = (u - cw / 2.0, v - ch / 2.0);
        let (s, c) = (self.sin, self.cos);
        let rx = self.canvas.0 / 2.0 + dx * c - dy * s;
        let ry = self.canvas.1 / 2.0 + dx * s + dy * c;
        (rx * self.scale, ry * self.scale)
    }

    /// A point on the OCR image to the original image (pixels).
    #[must_use]
    pub fn to_original(&self, x: f64, y: f64) -> (f64, f64) {
        let rx = x / self.scale - self.canvas.0 / 2.0;
        let ry = y / self.scale - self.canvas.1 / 2.0;
        let (s, c) = (self.sin, self.cos);
        let dx = rx * c + ry * s;
        let dy = -rx * s + ry * c;
        let (cw, ch) = self.crop;
        let (u, v) = (cw / 2.0 + dx, ch / 2.0 + dy);
        match &self.quad {
            Some((to, _)) if cw > 0.0 && ch > 0.0 => to.apply(u / cw, v / ch),
            _ => (self.origin.0 + u, self.origin.1 + v),
        }
    }
}

/// The box on the original page for a box on the OCR image: the four
/// corners mapped back, their bounds, in ten-thousandths of the original
/// page, clamped to it.
#[must_use]
pub fn to_page(px: PixelBox, transform: &Transform) -> Bbox {
    if transform.original_width == 0 || transform.original_height == 0 {
        return Bbox::PAGE;
    }
    let (l, t) = (f64::from(px.left), f64::from(px.top));
    let (r, b) = (l + f64::from(px.width), t + f64::from(px.height));
    let m = transform.mapper();
    let corners = [(l, t), (r, t), (l, b), (r, b)].map(|(x, y)| m.to_original(x, y));
    let (ow, oh) = (
        f64::from(transform.original_width),
        f64::from(transform.original_height),
    );
    let bp = |v: f64, size: f64| {
        let v = (v / size * f64::from(Bbox::SCALE)).round();
        if v.is_finite() {
            v.clamp(0.0, f64::from(Bbox::SCALE)) as u16
        } else {
            0
        }
    };
    let xs = corners.map(|c| bp(c.0, ow));
    let ys = corners.map(|c| bp(c.1, oh));
    let min = |a: [u16; 4]| a.into_iter().min().unwrap_or(0);
    let max = |a: [u16; 4]| a.into_iter().max().unwrap_or(0);
    Bbox::new(min(xs), min(ys), max(xs), max(ys)).unwrap_or(Bbox::PAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn turned(w: u32, h: u32, mdeg: i32, scale: u32) -> Transform {
        let mut t = Transform::identity(w, h);
        t.rotation_mdeg = mdeg;
        t.scale = scale;
        let (cw, ch) = t.canvas();
        t.ocr_width = cw * scale / 10_000;
        t.ocr_height = ch * scale / 10_000;
        t
    }

    /// The OCR-image box covering an original-image box (pixels).
    fn forward(t: &Transform, l: f64, top: f64, r: f64, b: f64) -> PixelBox {
        let c = [(l, top), (r, top), (l, b), (r, b)].map(|(x, y)| t.to_ocr(x, y));
        let x0 = c.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
        let x1 = c.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
        let y0 = c.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let y1 = c.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        PixelBox {
            left: x0.round() as u32,
            top: y0.round() as u32,
            width: (x1 - x0).round() as u32,
            height: (y1 - y0).round() as u32,
        }
    }

    fn near(a: Bbox, b: Bbox, tol: u16) -> bool {
        a.left().abs_diff(b.left()) <= tol
            && a.top().abs_diff(b.top()) <= tol
            && a.right().abs_diff(b.right()) <= tol
            && a.bottom().abs_diff(b.bottom()) <= tol
    }

    #[test]
    fn identity_maps_pixels_to_ten_thousandths() {
        let t = Transform::identity(2000, 1000);
        let b = to_page(
            PixelBox {
                left: 200,
                top: 100,
                width: 400,
                height: 50,
            },
            &t,
        );
        assert_eq!(b, Bbox::new(1_000, 1_000, 3_000, 1_500).unwrap());
    }

    #[test]
    fn quarter_turns_and_scaling_map_back_within_one_unit() {
        // A 2550 × 3300 page; a word near the top left.
        let want = (300.0, 400.0, 900.0, 470.0);
        let expect = Bbox::new(1_176, 1_212, 3_529, 1_424).unwrap();
        for mdeg in [0, 90_000, 180_000, 270_000, -90_000] {
            for scale in [10_000, 5_000, 13_333] {
                let t = turned(2550, 3300, mdeg, scale);
                let px = forward(&t, want.0, want.1, want.2, want.3);
                let got = to_page(px, &t);
                assert!(near(got, expect, 2), "{mdeg} {scale}: {got:?}");
            }
        }
        assert!(turned(2550, 3300, 90_000, 10_000).swaps());
        assert!(!turned(2550, 3300, 3_000, 10_000).swaps());
        assert_eq!(turned(2550, 3300, 90_000, 10_000).canvas(), (3300, 2550));
    }

    #[test]
    fn deskew_and_crop_points_round_trip() {
        let mut t = turned(3000, 4000, 3_000, 8_000);
        t.crop_left = 100;
        t.crop_top = 150;
        t.crop_width = 2800;
        t.crop_height = 3700;
        for (x, y) in [
            (100.0, 150.0),
            (1500.0, 2000.0),
            (2899.0, 3849.0),
            (640.5, 77.25),
        ] {
            let (ox, oy) = t.to_ocr(x, y);
            let (bx, by) = t.to_original(ox, oy);
            assert!((bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6, "{x},{y}");
        }
        // A small box: the deskewed box grows a little, never moves.
        let px = forward(&t, 1000.0, 1000.0, 1004.0, 1004.0);
        let got = to_page(px, &t);
        let centre = |a: u16, b: u16| u32::midpoint(u32::from(a), u32::from(b));
        assert!(
            centre(got.left(), got.right()).abs_diff(3_340) <= 3,
            "{got:?}"
        );
        assert!(
            centre(got.top(), got.bottom()).abs_diff(2_505) <= 3,
            "{got:?}"
        );
    }

    #[test]
    fn a_keystoned_sheet_maps_onto_its_rectangle_and_back() {
        // A photo 2000 × 2600 with the sheet's corners inside it, narrower
        // at the top, turned a little.
        let mut t = Transform::identity(2000, 2600);
        t.quad = Some([[260, 180], [1700, 230], [1880, 2450], [90, 2400]]);
        t.crop_width = 1800;
        t.crop_height = 2250;
        t.rotation_mdeg = -1_500;
        t.scale = 12_000;
        let (cw, ch) = t.canvas();
        t.ocr_width = cw * 12 / 10;
        t.ocr_height = ch * 12 / 10;
        let m = t.mapper();
        // The sheet's corners land on the rectangle's corners (before the
        // turn): with no turn, exactly.
        let mut flat = t;
        flat.rotation_mdeg = 0;
        flat.scale = 10_000;
        let (x, y) = flat.to_ocr(1700.0, 230.0);
        assert!((x - 1800.0).abs() < 1e-6 && y.abs() < 1e-6, "{x},{y}");
        let (x, y) = flat.to_original(0.0, 2250.0);
        assert!((x - 90.0).abs() < 1e-6 && (y - 2400.0).abs() < 1e-6);
        for (x, y) in [(300.0, 400.0), (1000.0, 1300.0), (1800.0, 2400.0)] {
            let (ox, oy) = m.to_ocr(x, y);
            let (bx, by) = m.to_original(ox, oy);
            assert!((bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6, "{x},{y}");
        }
        assert!(Homography::square_to_quad([(0.0, 0.0); 4]).is_none());
    }

    #[test]
    fn degenerate_transforms_stay_on_the_page() {
        let mut t = Transform::identity(0, 0);
        let px = PixelBox {
            left: 1,
            top: 1,
            width: 1,
            height: 1,
        };
        assert_eq!(to_page(px, &t), Bbox::PAGE);
        t = Transform::identity(10, 10);
        t.scale = 0;
        let b = to_page(px, &t);
        assert!(b.right() <= Bbox::SCALE);
    }

    proptest! {
        // T15: the inverse lands in 0..=10000 for any transform and box.
        #[test]
        fn any_box_maps_inside_the_page(
            ow in 1u32..12_000, oh in 1u32..12_000,
            cl in 0u32..12_000, ct in 0u32..12_000,
            cw in 0u32..12_000, ch in 0u32..12_000,
            mdeg in -400_000i32..400_000,
            scale in 0u32..40_000,
            quad in proptest::bool::ANY,
            l in 0u32..5_000, t in 0u32..5_000, w in 0u32..5_000, h in 0u32..5_000,
        ) {
            let tr = Transform {
                original_width: ow, original_height: oh,
                crop_left: cl, crop_top: ct, crop_width: cw, crop_height: ch,
                quad: if quad { Some([[cl, ct], [cl + cw, ct + 7], [cl + cw + 3, ct + ch], [cl / 2, ct + ch]]) } else { None },
                rotation_mdeg: mdeg, scale, ocr_width: 3400, ocr_height: 4400,
            };
            let b = to_page(PixelBox { left: l, top: t, width: w, height: h }, &tr);
            prop_assert!(b.left() <= b.right() && b.right() <= Bbox::SCALE);
            prop_assert!(b.top() <= b.bottom() && b.bottom() <= Bbox::SCALE);
        }
    }
}
