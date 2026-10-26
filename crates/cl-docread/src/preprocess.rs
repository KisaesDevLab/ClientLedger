//! Page clean-up before OCR (WP-B step 5, WP-A2 step 13b; feature
//! `worker`): contrast; for a photographed sheet, the sheet found against
//! its background and flattened (keystone and turn, a perspective map);
//! deskew; and scale into the OCR limit (`protocol::MAX_PGM_WIDTH` ×
//! `MAX_PGM_HEIGHT`). Returns the [`Transform`] that maps the result back
//! to the original page, so every word's box lands where the viewer shows
//! it.
//!
//! The geometry is measured on a small copy (at most [`PROBE`] pixels on
//! its longer side) and the full page is resampled once.
//!
//! Quarter turns (a page photographed sideways) are found by tesseract's
//! orientation pass, which the worker asks the host for only when a first
//! read comes back poor (`protocol::OcrMode::Orient`).
//!
//! One of the three files exempt from `cargo xtask nofloat` (image
//! geometry); it may not mention money types. The crate denies float
//! lints, so this module allows them.
#![allow(
    clippy::float_arithmetic,
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use crate::ocr_geom::Transform;
use crate::protocol::{MAX_PGM_HEIGHT, MAX_PGM_WIDTH, PgmError};
use image::GrayImage;
use image::imageops::FilterType;

/// Pages narrower than this are enlarged (up to twice) so small print
/// reaches the size tesseract reads best.
pub const MIN_OCR_WIDTH: u32 = 1_200;

/// The analysis copy's longer side, pixels.
pub const PROBE: u32 = 800;

/// Turns searched by deskew, either way (millidegrees).
const MAX_SKEW_MDEG: i32 = 6_000;
/// A smaller measured turn is left alone.
const MIN_SKEW_MDEG: i32 = 150;

/// A page ready for OCR.
#[derive(Debug, Clone)]
pub struct Cleaned {
    pub image: GrayImage,
    pub transform: Transform,
}

/// Clean up a page image: stretch its contrast, flatten a photographed
/// sheet, straighten a turned page, then scale into the OCR limit (or up,
/// when it is small). An empty image is returned as is.
#[must_use]
pub fn clean(page: &GrayImage) -> Cleaned {
    clean_turned(page, 0)
}

/// [`clean`] with the page first turned by `quarters` quarter turns
/// clockwise (from tesseract's orientation pass).
#[must_use]
pub fn clean_turned(page: &GrayImage, quarters: i32) -> Cleaned {
    let (w, h) = page.dimensions();
    if w == 0 || h == 0 {
        return Cleaned {
            image: page.clone(),
            transform: Transform::identity(w, h),
        };
    }
    let mut image = page.clone();
    stretch_contrast(&mut image);
    let probe = probe_of(&image);
    let k = f64::from(probe.width()) / f64::from(w);
    let mut transform = Transform::identity(w, h);
    if let Some(q) = sheet(&probe) {
        let to_page = |(x, y): (f64, f64)| {
            [
                ((x / k).round().max(0.0) as u32).min(w),
                ((y / k).round().max(0.0) as u32).min(h),
            ]
        };
        let quad = q.map(to_page);
        let (qw, qh) = quad_size(&quad);
        if qw > 0 && qh > 0 {
            transform.quad = Some(quad);
            transform.crop_width = qw;
            transform.crop_height = qh;
        }
    }
    // Deskew, measured on the probe seen through the sheet's flattening.
    let flat_probe = if transform.quad.is_some() {
        let mut t = probe_transform(&transform, k, probe.dimensions());
        t.rotation_mdeg = 0;
        warp(&probe, &t)
    } else {
        probe
    };
    let skew = skew_mdeg(&flat_probe);
    transform.rotation_mdeg = skew + quarters.rem_euclid(4) * 90_000;
    let (cw, ch) = transform.canvas();
    let factor = scale_factor(cw, ch);
    transform.scale = (factor * 10_000.0).round().max(1.0) as u32;
    transform.ocr_width = ((f64::from(cw) * factor).round() as u32).clamp(1, MAX_PGM_WIDTH);
    transform.ocr_height = ((f64::from(ch) * factor).round() as u32).clamp(1, MAX_PGM_HEIGHT);
    if transform.quad.is_none() && transform.rotation_mdeg == 0 {
        if transform.scale == 10_000 {
            transform.ocr_width = w;
            transform.ocr_height = h;
        } else {
            image = image::imageops::resize(
                &image,
                transform.ocr_width,
                transform.ocr_height,
                FilterType::Triangle,
            );
        }
        return Cleaned { image, transform };
    }
    let image = warp(&image, &transform);
    Cleaned { image, transform }
}

/// `t` (made for the full page) for the probe copy, `k` probe pixels per
/// page pixel.
fn probe_transform(t: &Transform, k: f64, (pw, ph): (u32, u32)) -> Transform {
    let s = |v: u32| (f64::from(v) * k).round() as u32;
    let mut p = Transform::identity(pw, ph);
    p.quad = t.quad.map(|q| q.map(|[x, y]| [s(x), s(y)]));
    p.crop_left = s(t.crop_left);
    p.crop_top = s(t.crop_top);
    p.crop_width = s(t.crop_width).max(1);
    p.crop_height = s(t.crop_height).max(1);
    p.rotation_mdeg = t.rotation_mdeg;
    let (cw, ch) = p.canvas();
    p.ocr_width = cw;
    p.ocr_height = ch;
    p
}

/// The small analysis copy.
fn probe_of(image: &GrayImage) -> GrayImage {
    let (w, h) = image.dimensions();
    let long = w.max(h);
    if long <= PROBE {
        return image.clone();
    }
    let pw = ((u64::from(w) * u64::from(PROBE)) / u64::from(long)).max(1) as u32;
    let ph = ((u64::from(h) * u64::from(PROBE)) / u64::from(long)).max(1) as u32;
    image::imageops::resize(image, pw, ph, FilterType::Triangle)
}

/// The rectangle a quad flattens to: its longer top/bottom edge by its
/// longer left/right edge (no detail is lost).
fn quad_size(q: &[[u32; 2]; 4]) -> (u32, u32) {
    let d = |a: [u32; 2], b: [u32; 2]| {
        let dx = f64::from(a[0]) - f64::from(b[0]);
        let dy = f64::from(a[1]) - f64::from(b[1]);
        dx.hypot(dy).round() as u32
    };
    let w = d(q[0], q[1]).max(d(q[3], q[2]));
    let h = d(q[0], q[3]).max(d(q[1], q[2]));
    (w, h)
}

/// Otsu's threshold of an image.
fn otsu(image: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in image.pixels() {
        hist[usize::from(p.0[0])] += 1;
    }
    let total: u64 = hist.iter().sum();
    let sum_all: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &n)| i as f64 * n as f64)
        .sum();
    let (mut w0, mut sum0, mut best, mut best_t) = (0u64, 0.0f64, -1.0f64, 128u8);
    for (t, &n) in hist.iter().enumerate() {
        w0 += n;
        if w0 == 0 {
            continue;
        }
        let w1 = total - w0;
        if w1 == 0 {
            break;
        }
        sum0 += t as f64 * n as f64;
        let m0 = sum0 / w0 as f64;
        let m1 = (sum_all - sum0) / w1 as f64;
        let between = w0 as f64 * w1 as f64 * (m0 - m1) * (m0 - m1);
        if between > best {
            best = between;
            best_t = t as u8;
        }
    }
    best_t
}

/// A photographed sheet: the largest bright region, when the picture's
/// edge is mostly darker background and the region covers at least a
/// fifth of it. Its four corners (top left, top right, bottom right,
/// bottom left) in probe pixels, pulled in slightly so no background edge
/// is kept.
#[must_use]
pub fn sheet(probe: &GrayImage) -> Option<[(f64, f64); 4]> {
    let (w, h) = probe.dimensions();
    if w < 16 || h < 16 {
        return None;
    }
    let t = otsu(probe);
    let bright = |x: u32, y: u32| probe.get_pixel(x, y).0[0] > t;
    // The edge ring: a scan's white paper fills it; a photo's background
    // shows there.
    let ring = (w.min(h) / 50).max(2);
    let (mut edge, mut edge_bright) = (0u64, 0u64);
    for y in 0..h {
        for x in 0..w {
            if x < ring || y < ring || x >= w - ring || y >= h - ring {
                edge += 1;
                edge_bright += u64::from(bright(x, y));
            }
        }
    }
    if edge_bright * 10 > edge * 4 {
        return None;
    }
    // The largest 4-connected bright region.
    let idx = |x: u32, y: u32| (y * w + x) as usize;
    let mut label = vec![0u32; (w * h) as usize];
    let (mut best, mut best_size, mut next) = (0u32, 0usize, 0u32);
    let mut stack: Vec<(u32, u32)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if label[idx(x, y)] != 0 || !bright(x, y) {
                continue;
            }
            next += 1;
            let mut size = 0usize;
            label[idx(x, y)] = next;
            stack.push((x, y));
            while let Some((cx, cy)) = stack.pop() {
                size += 1;
                let mut visit = |nx: u32, ny: u32| {
                    if label[idx(nx, ny)] == 0 && bright(nx, ny) {
                        label[idx(nx, ny)] = next;
                        stack.push((nx, ny));
                    }
                };
                if cx > 0 {
                    visit(cx - 1, cy);
                }
                if cx + 1 < w {
                    visit(cx + 1, cy);
                }
                if cy > 0 {
                    visit(cx, cy - 1);
                }
                if cy + 1 < h {
                    visit(cx, cy + 1);
                }
            }
            if size > best_size {
                best_size = size;
                best = next;
            }
        }
    }
    if best == 0 || best_size * 5 < (w * h) as usize {
        return None;
    }
    // Extreme points along the diagonals.
    let mut c = [(0i64, 0i64); 4];
    let mut score = [i64::MIN; 4];
    for y in 0..h {
        for x in 0..w {
            if label[idx(x, y)] != best {
                continue;
            }
            let (xi, yi) = (i64::from(x), i64::from(y));
            for (k, s) in [-xi - yi, xi - yi, xi + yi, yi - xi]
                .into_iter()
                .enumerate()
            {
                if s > score[k] {
                    score[k] = s;
                    c[k] = (xi, yi);
                }
            }
        }
    }
    let quad = c.map(|(x, y)| (x as f64 + 0.5, y as f64 + 0.5));
    // Pull each corner a little towards the middle.
    let (mx, my) = (
        quad.iter().map(|p| p.0).sum::<f64>() / 4.0,
        quad.iter().map(|p| p.1).sum::<f64>() / 4.0,
    );
    let inset = 2.0;
    let quad = quad.map(|(x, y)| {
        let (dx, dy) = (mx - x, my - y);
        let len = dx.hypot(dy).max(1.0);
        (x + dx / len * inset, y + dy / len * inset)
    });
    // A sane, convex quad of reasonable size.
    let area = shoelace(&quad);
    if area < f64::from(w) * f64::from(h) / 5.0 || !convex(&quad) {
        return None;
    }
    Some(quad)
}

fn shoelace(q: &[(f64, f64); 4]) -> f64 {
    let mut s = 0.0;
    for i in 0..4 {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        s += a.0 * b.1 - b.0 * a.1;
    }
    (s / 2.0).abs()
}

fn convex(q: &[(f64, f64); 4]) -> bool {
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (a, b, c) = (q[i], q[(i + 1) % 4], q[(i + 2) % 4]);
        let cross = (b.0 - a.0) * (c.1 - b.1) - (b.1 - a.1) * (c.0 - b.0);
        if cross.abs() < 1e-9 {
            return false;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// The turn (millidegrees, as [`Transform::rotation_mdeg`]) that makes the
/// text lines of `image` level: the turn whose row profile of dark pixels
/// is sharpest. 0 when the page has too little text or the gain is small.
#[must_use]
pub fn skew_mdeg(image: &GrayImage) -> i32 {
    let (w, h) = image.dimensions();
    if w < 16 || h < 16 {
        return 0;
    }
    let t = otsu(image);
    // Ink: the dark class, and darker than mid grey (not shade).
    let dark_max = t.min(160);
    let mut pts: Vec<(f64, f64)> = Vec::new();
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    for (x, y, p) in image.enumerate_pixels() {
        if p.0[0] <= dark_max {
            pts.push((f64::from(x) - cx, f64::from(y) - cy));
        }
    }
    // Large dark areas (a background, a filled box) are not text lines.
    if pts.len() < 200 || pts.len() * 3 > (w * h) as usize {
        return 0;
    }
    let rows = (f64::from(w).hypot(f64::from(h)) as usize) + 4;
    let off = rows as f64 / 2.0;
    let mut hist = vec![0u32; rows];
    let mut sharpness = |mdeg: i32| -> f64 {
        hist.iter_mut().for_each(|v| *v = 0);
        let (s, c) = (f64::from(mdeg) / 1000.0).to_radians().sin_cos();
        for &(dx, dy) in &pts {
            let ry = dx * s + dy * c + off;
            let i = (ry as usize).min(rows - 1);
            hist[i] += 1;
        }
        hist.iter().map(|&v| f64::from(v) * f64::from(v)).sum()
    };
    let level = sharpness(0);
    let (mut best, mut best_s) = (0, level);
    let mut a = -MAX_SKEW_MDEG;
    while a <= MAX_SKEW_MDEG {
        let s = sharpness(a);
        if s > best_s {
            best = a;
            best_s = s;
        }
        a += 250;
    }
    let centre = best;
    let mut a = centre - 250;
    while a <= centre + 250 {
        let s = sharpness(a);
        if s > best_s {
            best = a;
            best_s = s;
        }
        a += 50;
    }
    if best.abs() < MIN_SKEW_MDEG || best_s < level * 1.05 {
        return 0;
    }
    best
}

/// Resample `src` (the original page) into the OCR image `t` describes:
/// every OCR pixel's centre mapped back to the page, bilinear, white
/// outside the page. A large reduction first shrinks the page smoothly.
#[must_use]
pub fn warp(src: &GrayImage, t: &Transform) -> GrayImage {
    let (sw, sh) = src.dimensions();
    let (ow, oh) = (t.ocr_width.max(1), t.ocr_height.max(1));
    let factor = f64::from(t.scale.max(1)) / 10_000.0;
    let shrunk;
    let (img, kx, ky) = if factor < 0.7 {
        let nw = ((f64::from(sw) * factor).round() as u32).max(1);
        let nh = ((f64::from(sh) * factor).round() as u32).max(1);
        shrunk = image::imageops::resize(src, nw, nh, FilterType::Triangle);
        (
            &shrunk,
            f64::from(nw) / f64::from(sw),
            f64::from(nh) / f64::from(sh),
        )
    } else {
        (src, 1.0, 1.0)
    };
    let (iw, ih) = img.dimensions();
    let raw = img.as_raw();
    let m = t.mapper();
    let mut out = GrayImage::new(ow, oh);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let (px, py) = m.to_original(f64::from(x) + 0.5, f64::from(y) + 0.5);
        p.0[0] = bilinear(raw, iw, ih, px * kx - 0.5, py * ky - 0.5);
    }
    out
}

fn bilinear(raw: &[u8], w: u32, h: u32, x: f64, y: f64) -> u8 {
    if !(x.is_finite() && y.is_finite()) || x < -0.5 || y < -0.5 {
        return 255;
    }
    let (fw, fh) = (f64::from(w), f64::from(h));
    if x > fw - 0.5 || y > fh - 0.5 {
        return 255;
    }
    let x = x.clamp(0.0, fw - 1.0);
    let y = y.clamp(0.0, fh - 1.0);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - f64::from(x0), y - f64::from(y0));
    let at = |xx: u32, yy: u32| f64::from(raw[(yy * w + xx) as usize]);
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    (top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8
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

    /// A white page `w` × `h` with dark text bars every 30 px.
    fn lined(w: u32, h: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let ink = y % 30 < 6 && x > w / 10 && x < w * 9 / 10 && (x / 9) % 4 != 3;
            image::Luma([if ink { 20 } else { 235 }])
        })
    }

    /// `page` turned `deg` clockwise about its centre on a white canvas.
    fn turned(page: &GrayImage, deg: f64, bg: u8, margin: u32) -> GrayImage {
        let (w, h) = page.dimensions();
        let (ow, oh) = (w + 2 * margin, h + 2 * margin);
        let (cx, cy) = (f64::from(ow) / 2.0, f64::from(oh) / 2.0);
        let (s, c) = (-deg).to_radians().sin_cos();
        GrayImage::from_fn(ow, oh, |x, y| {
            let (dx, dy) = (f64::from(x) - cx, f64::from(y) - cy);
            let (rx, ry) = (cx + dx * c - dy * s, cy + dx * s + dy * c);
            let (px, py) = (rx - f64::from(margin), ry - f64::from(margin));
            if px < 0.0 || py < 0.0 || px >= f64::from(w) || py >= f64::from(h) {
                image::Luma([bg])
            } else {
                *page.get_pixel(px as u32, py as u32)
            }
        })
    }

    /// Row sharpness of an image's dark pixels (higher: levelled lines).
    fn row_profile(img: &GrayImage) -> f64 {
        let mut rows = vec![0f64; img.height() as usize];
        for (_, y, p) in img.enumerate_pixels() {
            if p.0[0] < 100 {
                rows[y as usize] += 1.0;
            }
        }
        rows.iter().map(|v| v * v).sum()
    }

    #[test]
    fn a_turned_scan_is_straightened_and_its_words_map_back() {
        let page = lined(600, 760);
        for deg in [2.5, -3.0] {
            let scan = turned(&page, deg, 255, 0);
            let mdeg = skew_mdeg(&scan);
            // Undo the turn: about `-deg`, to within a quarter degree.
            assert!(
                (f64::from(mdeg) / 1000.0 + deg).abs() <= 0.3,
                "{deg}: {mdeg}"
            );
            let c = clean(&scan);
            assert_eq!(c.transform.rotation_mdeg, mdeg);
            let mut unturned = c.transform;
            unturned.rotation_mdeg = 0;
            let before = row_profile(&warp(&scan, &unturned));
            assert!(row_profile(&c.image) > before * 1.3);
            // A point on the cleaned image maps back to the original.
            let m = c.transform.mapper();
            let (x, y) = m.to_ocr(300.0, 380.0);
            let (bx, by) = m.to_original(x, y);
            assert!((bx - 300.0).abs() < 1e-6 && (by - 380.0).abs() < 1e-6);
        }
        // A level page is left level.
        assert_eq!(skew_mdeg(&page), 0);
        assert_eq!(clean(&page).transform.rotation_mdeg, 0);
    }

    #[test]
    fn a_photographed_sheet_is_found_and_flattened() {
        let page = lined(500, 640);
        let photo = turned(&page, 3.0, 70, 40);
        let q = sheet(&photo).expect("a sheet on a dark background");
        // Corners near the turned page's corners, in order.
        assert!(q[0].0 < q[1].0 && q[3].0 < q[2].0 && q[0].1 < q[3].1);
        let c = clean(&photo);
        let quad = c.transform.quad.expect("flattened");
        assert!(quad.iter().all(|p| p[0] <= 580 && p[1] <= 720));
        let (cw, ch) = (c.transform.crop_width, c.transform.crop_height);
        assert!(cw.abs_diff(500) < 25 && ch.abs_diff(640) < 25, "{cw}x{ch}");
        // No background left at the corners of the cleaned image.
        let img = &c.image;
        let corner = img.get_pixel(3, 3).0[0];
        assert!(corner > 150, "{corner}");
        // A scan (white to the edges) has no sheet to find.
        assert!(sheet(&turned(&page, 2.0, 255, 0)).is_none());
        assert!(sheet(&GrayImage::new(8, 8)).is_none());
    }

    #[test]
    fn a_quarter_turn_swaps_the_canvas() {
        let page = lined(300, 400);
        let c = clean_turned(&page, 1);
        assert_eq!(c.transform.rotation_mdeg, 90_000);
        assert!(c.image.width() > c.image.height());
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
