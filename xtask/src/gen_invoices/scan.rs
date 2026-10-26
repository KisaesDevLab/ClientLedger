//! "Scanned" and "photographed" pictures of a synthetic PDF (plan D6): the
//! page rendered with the pinned `PDFium`, then turned, blurred, darkened,
//! speckled and, for photos, keystoned and saved as a low-quality JPEG.
//! Floating point here is image geometry and pixel values only.

use super::rng::Rng;
use anyhow::{Context, Result, anyhow};
use image::{GrayImage, Luma};
use pdfium_render::prelude::{PdfRenderConfig, Pdfium};
use std::path::Path;

/// The pinned `PDFium`, bound once.
pub struct Renderer {
    pdfium: Pdfium,
}

impl Renderer {
    /// Bind `target/pdfium/<triple>/` (run `cargo xtask pdfium-fetch`).
    pub fn new(root: &Path) -> Result<Renderer> {
        let host = crate::pdfium_fetch::host()?;
        let lib = root
            .join("target")
            .join("pdfium")
            .join(host.triple)
            .join(host.lib_name);
        if !lib.is_file() {
            return Err(anyhow!(
                "PDFium is missing at {}; run `cargo xtask pdfium-fetch`",
                lib.display()
            ));
        }
        let bindings =
            Pdfium::bind_to_library(&lib).map_err(|e| anyhow!("bind {}: {e}", lib.display()))?;
        Ok(Renderer {
            pdfium: Pdfium::new(bindings),
        })
    }

    /// The first page of `pdf` in grey at `dpi`.
    pub fn first_page(&self, pdf: &[u8], dpi: f32) -> Result<GrayImage> {
        let doc = self
            .pdfium
            .load_pdf_from_byte_slice(pdf, None)
            .map_err(|e| anyhow!("load: {e}"))?;
        let page = doc.pages().get(0).map_err(|e| anyhow!("page: {e}"))?;
        #[allow(clippy::cast_possible_truncation)]
        let (w, h) = (
            (page.width().value * dpi / 72.0).round() as i32,
            (page.height().value * dpi / 72.0).round() as i32,
        );
        let bitmap = page
            .render_with_config(&PdfRenderConfig::new().set_target_size(w, h))
            .map_err(|e| anyhow!("render: {e}"))?;
        let (bw, bh) = (
            u32::try_from(bitmap.width()).context("width")?,
            u32::try_from(bitmap.height()).context("height")?,
        );
        let rgba = bitmap.as_rgba_bytes();
        let mut out = GrayImage::new(bw, bh);
        for (i, px) in out.pixels_mut().enumerate() {
            let p = &rgba[i * 4..i * 4 + 4];
            let l = (u32::from(p[0]) * 299 + u32::from(p[1]) * 587 + u32::from(p[2]) * 114) / 1000;
            *px = Luma([u8::try_from(l.min(255)).unwrap_or(255)]);
        }
        Ok(out)
    }
}

fn clamp(v: f64) -> u8 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let b = v.round().clamp(0.0, 255.0) as u8;
    b
}

fn sample(img: &GrayImage, x: f64, y: f64, background: f64) -> f64 {
    let (w, h) = (f64::from(img.width()), f64::from(img.height()));
    if x < 0.0 || y < 0.0 || x > w - 1.0 || y > h - 1.0 {
        return background;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = (
        (x0 + 1).min(img.width() - 1),
        (y0 + 1).min(img.height() - 1),
    );
    let (fx, fy) = (x - f64::from(x0), y - f64::from(y0));
    let p = |x, y| f64::from(img.get_pixel(x, y)[0]);
    let top = p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx;
    let bottom = p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// A box blur of radius `r` (horizontal then vertical).
fn blur(img: &GrayImage, r: u32) -> GrayImage {
    if r == 0 {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let pass = |src: &GrayImage, horizontal: bool| {
        let mut out = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0u32;
                let mut n = 0u32;
                for d in 0..=2 * r {
                    let o = i64::from(d) - i64::from(r);
                    let (sx, sy) = if horizontal {
                        (i64::from(x) + o, i64::from(y))
                    } else {
                        (i64::from(x), i64::from(y) + o)
                    };
                    if sx >= 0 && sy >= 0 && sx < i64::from(w) && sy < i64::from(h) {
                        sum += u32::from(
                            src.get_pixel(
                                u32::try_from(sx).unwrap_or(0),
                                u32::try_from(sy).unwrap_or(0),
                            )[0],
                        );
                        n += 1;
                    }
                }
                out.put_pixel(x, y, Luma([u8::try_from(sum / n.max(1)).unwrap_or(255)]));
            }
        }
        out
    };
    pass(&pass(img, true), false)
}

/// Settings of one picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Degrade {
    /// Turn in degrees (positive: clockwise).
    pub rotate: f64,
    /// Keystone: how much narrower the top edge is (0..0.14, ~8°).
    pub keystone: f64,
    /// Border of background around the page, as a fraction of its size.
    pub margin: f64,
    pub background: f64,
    pub blur: u32,
    /// Contrast factor and brightness offset.
    pub contrast: f64,
    pub brightness: f64,
    /// Light falling off from one side (0: even).
    pub shade: f64,
    pub noise: f64,
}

impl Degrade {
    /// A flatbed scan: small turn, light blur and speckle.
    pub fn scan(rng: &mut Rng) -> Degrade {
        Degrade {
            rotate: if rng.chance(25) {
                0.0
            } else {
                rng.unit_range(-2.5, 2.5)
            },
            keystone: 0.0,
            margin: 0.0,
            background: 255.0,
            blur: u32::from(rng.chance(50)),
            contrast: rng.unit_range(0.85, 1.0),
            brightness: rng.unit_range(-10.0, 15.0),
            shade: 0.0,
            noise: rng.unit_range(2.0, 7.0),
        }
    }

    /// A phone photo: keystone up to about 8°, a turn, uneven light,
    /// blur and noise.
    pub fn photo(rng: &mut Rng) -> Degrade {
        Degrade {
            rotate: rng.unit_range(-4.0, 4.0),
            keystone: rng.unit_range(0.02, 0.14),
            margin: rng.unit_range(0.03, 0.08),
            background: rng.unit_range(60.0, 120.0),
            blur: u32::try_from(rng.range(1, 2)).unwrap_or(1),
            contrast: rng.unit_range(0.7, 0.92),
            brightness: rng.unit_range(-25.0, 10.0),
            shade: rng.unit_range(0.05, 0.25),
            noise: rng.unit_range(4.0, 11.0),
        }
    }

    /// Apply to `page`.
    pub fn apply(&self, page: &GrayImage, rng: &mut Rng) -> GrayImage {
        let (pw, ph) = (f64::from(page.width()), f64::from(page.height()));
        let (mw, mh) = (pw * self.margin, ph * self.margin);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (ow, oh) = (
            (pw + 2.0 * mw).round() as u32,
            (ph + 2.0 * mh).round() as u32,
        );
        let (cx, cy) = (f64::from(ow) / 2.0, f64::from(oh) / 2.0);
        let (s, c) = (-self.rotate.to_radians()).sin_cos();
        let mut out = GrayImage::new(ow, oh);
        for y in 0..oh {
            for x in 0..ow {
                // Undo the turn about the centre.
                let (dx, dy) = (f64::from(x) - cx, f64::from(y) - cy);
                let (rx, ry) = (cx + dx * c - dy * s, cy + dx * s + dy * c);
                // Into page space, then undo the keystone.
                let (px, py) = (rx - mw, ry - mh);
                let scale = 1.0 - self.keystone * (1.0 - (py / ph).clamp(0.0, 1.0));
                let sx = pw / 2.0 + (px - pw / 2.0) / scale;
                let v = sample(page, sx, py, self.background);
                out.put_pixel(x, y, Luma([clamp(v)]));
            }
        }
        let mut out = blur(&out, self.blur);
        let w = f64::from(ow);
        for (x, _, px) in out.enumerate_pixels_mut() {
            let light = 1.0 - self.shade * (f64::from(x) / w);
            let v = (f64::from(px[0]) * self.contrast + self.brightness) * light
                + rng.noise() * self.noise;
            *px = Luma([clamp(v)]);
        }
        out
    }
}

/// Encode as JPEG at `quality`.
pub fn jpeg(img: &GrayImage, quality: u8) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    enc.encode_image(img).context("encode jpeg")?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_degrade_keeps_the_page_and_a_photo_adds_a_border() {
        let mut page = GrayImage::from_pixel(60, 80, Luma([255]));
        for x in 10..50 {
            page.put_pixel(x, 40, Luma([0]));
        }
        let flat = Degrade {
            rotate: 0.0,
            keystone: 0.0,
            margin: 0.0,
            background: 255.0,
            blur: 0,
            contrast: 1.0,
            brightness: 0.0,
            shade: 0.0,
            noise: 0.0,
        };
        let mut rng = Rng::new(1);
        let same = flat.apply(&page, &mut rng);
        assert_eq!(same, page);
        let photo = Degrade {
            margin: 0.1,
            background: 80.0,
            keystone: 0.1,
            ..flat
        }
        .apply(&page, &mut rng);
        assert_eq!(photo.dimensions(), (72, 96));
        assert_eq!(photo.get_pixel(0, 0)[0], 80);
        assert_ne!(jpeg(&photo, 50).unwrap(), Vec::<u8>::new());
    }
}
