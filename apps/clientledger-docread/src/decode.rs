//! Images in Rust (plan D2): PNG and JPEG through `image`, multi-page TIFF
//! through `tiff`, with the A02 viewer's limits: at most
//! [`MAX_IMAGE_SIDE`] px a side and [`MAX_IMAGE_ALLOC`] bytes to decode,
//! checked before pixels are read. Every page becomes 8-bit grey (alpha
//! over white) for the clean-up and OCR.
//!
//! TIFF: the compressions in [`compression_taken`] only (no JPEG-in-TIFF,
//! review B1 of A02: its decoder sizes buffers from a header the page check
//! never sees). Grey 1/8/16-bit with or without alpha, RGB, RGBA and CMYK.

use image::{DynamicImage, GrayImage, ImageReader, Limits};
use std::io::Cursor;
use tiff::ColorType;
use tiff::decoder::{Decoder, DecodingResult, Limits as TiffLimits};
use tiff::tags::Tag;

/// Largest image side in pixels (A02).
pub const MAX_IMAGE_SIDE: u32 = 12_000;
/// Largest decode buffer in bytes (A02).
pub const MAX_IMAGE_ALLOC: u64 = 256 * 1024 * 1024;
/// TIFF pages walked to count them, at most (a looping or endless page
/// list ends here).
pub const MAX_TIFF_PAGES_COUNTED: u16 = 1_000;

/// What a file's first bytes say it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Pdf,
    Png,
    Jpeg,
    Tiff,
    Unknown,
}

/// Sniff the type from the content, never a name. A PDF header may sit
/// after up to 1 KiB of junk (as readers allow).
#[must_use]
pub fn sniff(bytes: &[u8]) -> Kind {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Kind::Png
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Kind::Jpeg
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        Kind::Tiff
    } else if bytes[..bytes.len().min(1024)]
        .windows(5)
        .any(|w| w == b"%PDF-")
    {
        Kind::Pdf
    } else {
        Kind::Unknown
    }
}

/// Why an image page could not be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    /// Over a size limit.
    TooLarge,
    /// Damaged or a kind of image not read.
    Unreadable,
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_IMAGE_SIDE);
    l.max_image_height = Some(MAX_IMAGE_SIDE);
    l.max_alloc = Some(MAX_IMAGE_ALLOC);
    l
}

/// Decode a PNG or JPEG to grey within the limits.
///
/// # Errors
/// See [`ImageError`].
pub fn decode_image(bytes: &[u8]) -> Result<GrayImage, ImageError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ImageError::Unreadable)?;
    let (w, h) = reader
        .into_dimensions()
        .map_err(|_| ImageError::Unreadable)?;
    if w == 0 || h == 0 {
        return Err(ImageError::Unreadable);
    }
    if w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
        return Err(ImageError::TooLarge);
    }
    reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ImageError::Unreadable)?;
    reader.limits(limits());
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(_) => ImageError::TooLarge,
        _ => ImageError::Unreadable,
    })?;
    Ok(grey(img))
}

/// Integer luma (ITU-R BT.601).
fn luma(r: u8, g: u8, b: u8) -> u8 {
    let v = (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b) + 500) / 1000;
    u8::try_from(v).unwrap_or(u8::MAX)
}

/// `v` with alpha `a` over white.
fn over_white(v: u8, a: u8) -> u8 {
    let (v, a) = (u32::from(v), u32::from(a));
    u8::try_from((v * a + 255 * (255 - a) + 127) / 255).unwrap_or(u8::MAX)
}

/// Any decoded image as grey, without an RGBA copy of large pictures.
fn grey(img: DynamicImage) -> GrayImage {
    match img {
        DynamicImage::ImageLuma8(g) => g,
        DynamicImage::ImageLumaA8(g) => GrayImage::from_fn(g.width(), g.height(), |x, y| {
            let p = g.get_pixel(x, y).0;
            image::Luma([over_white(p[0], p[1])])
        }),
        DynamicImage::ImageRgb8(c) => GrayImage::from_fn(c.width(), c.height(), |x, y| {
            let p = c.get_pixel(x, y).0;
            image::Luma([luma(p[0], p[1], p[2])])
        }),
        DynamicImage::ImageRgba8(c) => GrayImage::from_fn(c.width(), c.height(), |x, y| {
            let p = c.get_pixel(x, y).0;
            image::Luma([over_white(luma(p[0], p[1], p[2]), p[3])])
        }),
        other => other.to_luma8(),
    }
}

/// RGBA bytes (a rendered PDF page) to grey, alpha over white. `None` when
/// the buffer does not match the size.
#[must_use]
pub fn grey_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<GrayImage> {
    let n = usize::try_from(u64::from(width) * u64::from(height)).ok()?;
    if rgba.len() != n.checked_mul(4)? {
        return None;
    }
    let px: Vec<u8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| over_white(luma(p[0], p[1], p[2]), p[3]))
        .collect();
    GrayImage::from_raw(width, height, px)
}

// ---------------------------------------------------------------------------
// TIFF
// ---------------------------------------------------------------------------

/// An open TIFF: pages decoded one at a time.
pub struct Tiff {
    decoder: Decoder<Cursor<Vec<u8>>>,
    pages: u16,
}

fn tiff_limits() -> TiffLimits {
    let mut l = TiffLimits::default();
    let alloc = usize::try_from(MAX_IMAGE_ALLOC).unwrap_or(usize::MAX);
    l.decoding_buffer_size = alloc;
    l.intermediate_buffer_size = alloc;
    l.ifd_value_size = 1024 * 1024;
    l
}

impl Tiff {
    /// Open and count pages (at most [`MAX_TIFF_PAGES_COUNTED`]).
    ///
    /// # Errors
    /// Not a TIFF this reads.
    pub fn open(bytes: Vec<u8>) -> Result<Tiff, ImageError> {
        let decoder = Decoder::new(Cursor::new(bytes))
            .map_err(|_| ImageError::Unreadable)?
            .with_limits(tiff_limits());
        let mut t = Tiff { decoder, pages: 1 };
        while t.decoder.more_images() && t.pages < MAX_TIFF_PAGES_COUNTED {
            let before = t.decoder.ifd_pointer();
            let _ = t.decoder.next_image();
            if t.decoder.ifd_pointer() == before {
                break;
            }
            t.pages += 1;
        }
        Ok(t)
    }

    /// Pages counted.
    #[must_use]
    pub fn pages(&self) -> u16 {
        self.pages
    }

    /// Decode page `page` to grey.
    ///
    /// # Errors
    /// See [`ImageError`].
    pub fn page(&mut self, page: u16) -> Result<GrayImage, ImageError> {
        let d = &mut self.decoder;
        d.seek_to_image(usize::from(page))
            .map_err(|_| ImageError::Unreadable)?;
        let (w, h) = d.dimensions().map_err(|_| ImageError::Unreadable)?;
        if w == 0 || h == 0 {
            return Err(ImageError::Unreadable);
        }
        if w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
            return Err(ImageError::TooLarge);
        }
        let method = d
            .find_tag_unsigned::<u16>(Tag::Compression)
            .map_err(|_| ImageError::Unreadable)?
            .unwrap_or(1);
        if !compression_taken(method) {
            return Err(ImageError::Unreadable);
        }
        let colour = d.colortype().map_err(|_| ImageError::Unreadable)?;
        let px = u64::from(w) * u64::from(h);
        let sample_bytes = u64::from(colour.bit_depth().div_ceil(8));
        if px * u64::from(colour.num_samples()) * sample_bytes > MAX_IMAGE_ALLOC {
            return Err(ImageError::TooLarge);
        }
        let data = d.read_image().map_err(|_| ImageError::Unreadable)?;
        tiff_grey(colour, w, h, &data).ok_or(ImageError::Unreadable)
    }
}

/// The TIFF compressions read: none, CCITT fax, LZW, Deflate (both tag
/// values) and `PackBits`. Not JPEG (6, 7).
#[must_use]
pub fn compression_taken(method: u16) -> bool {
    matches!(method, 1 | 3 | 4 | 5 | 8 | 0x80B2 | 0x8005)
}

/// Decoded TIFF samples to grey. `None` for a colour this does not take or
/// a buffer of the wrong size.
fn tiff_grey(
    colour: ColorType,
    width: u32,
    height: u32,
    data: &DecodingResult,
) -> Option<GrayImage> {
    let (wu, hu) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    let pixels = wu.checked_mul(hu)?;
    let hi = |v: u16| v.to_be_bytes()[0];
    let out: Vec<u8> = match (colour, data) {
        (ColorType::Gray(1), DecodingResult::U8(b)) => {
            let stride = wu.div_ceil(8);
            if b.len() != stride.checked_mul(hu)? {
                return None;
            }
            let mut out = Vec::with_capacity(pixels);
            for row in b.chunks_exact(stride) {
                for x in 0..wu {
                    let bit = (row[x / 8] >> (7 - x % 8)) & 1;
                    out.push(if bit == 1 { 255 } else { 0 });
                }
            }
            out
        }
        (ColorType::Gray(8), DecodingResult::U8(b)) if b.len() == pixels => b.clone(),
        (ColorType::Gray(16), DecodingResult::U16(b)) if b.len() == pixels => {
            b.iter().map(|v| hi(*v)).collect()
        }
        (
            ColorType::Multiband {
                bit_depth: 8,
                num_samples: 2,
            },
            DecodingResult::U8(b),
        ) if b.len() == pixels * 2 => b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| over_white(p[0], p[1]))
            .collect(),
        (ColorType::RGB(8), DecodingResult::U8(b)) if b.len() == pixels * 3 => b
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| luma(p[0], p[1], p[2]))
            .collect(),
        (ColorType::RGB(16), DecodingResult::U16(b)) if b.len() == pixels * 3 => b
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| luma(hi(p[0]), hi(p[1]), hi(p[2])))
            .collect(),
        (ColorType::RGBA(8), DecodingResult::U8(b)) if b.len() == pixels * 4 => b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| over_white(luma(p[0], p[1], p[2]), p[3]))
            .collect(),
        (ColorType::CMYK(8), DecodingResult::U8(b)) if b.len() == pixels * 4 => b
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let k = 255 - u16::from(p[3]);
                let ch = |c: u8| u8::try_from((255 - u16::from(c)) * k / 255).unwrap_or(255);
                luma(ch(p[0]), ch(p[1]), ch(p[2]))
            })
            .collect(),
        _ => return None,
    };
    GrayImage::from_raw(width, height, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/docs")
                .join(name),
        )
        .expect("fixture")
    }

    #[test]
    fn types_are_sniffed_from_content() {
        assert_eq!(sniff(&fixture("receipt.png")), Kind::Png);
        assert_eq!(sniff(&fixture("receipt.jpg")), Kind::Jpeg);
        assert_eq!(sniff(&fixture("three_pages.tif")), Kind::Tiff);
        assert_eq!(sniff(&fixture("two_pages.pdf")), Kind::Pdf);
        assert_eq!(sniff(b"junk\n%PDF-1.7"), Kind::Pdf);
        assert_eq!(sniff(b"MZ\x90\0"), Kind::Unknown);
        assert_eq!(sniff(b""), Kind::Unknown);
    }

    #[test]
    fn pictures_decode_to_grey_and_bombs_are_refused() {
        let png = decode_image(&fixture("receipt.png")).unwrap();
        assert!(png.width() > 0 && png.height() > 0);
        assert!(decode_image(&fixture("receipt.jpg")).is_ok());
        assert_eq!(
            decode_image(&fixture("bomb_30000.png")),
            Err(ImageError::TooLarge)
        );
        assert_eq!(
            decode_image(b"\x89PNG\r\n\x1a\nbroken"),
            Err(ImageError::Unreadable)
        );
    }

    #[test]
    fn tiff_pages_decode_one_at_a_time() {
        let mut t = Tiff::open(fixture("three_pages.tif")).unwrap();
        assert_eq!(t.pages(), 3);
        for p in 0..3 {
            assert!(t.page(p).is_ok(), "page {p}");
        }
        assert!(t.page(3).is_err());
        let mut fax = Tiff::open(fixture("g4_fax.tif")).unwrap();
        let g = fax.page(0).unwrap();
        assert!(g.pixels().any(|p| p.0[0] == 0) && g.pixels().any(|p| p.0[0] == 255));
        for bomb in ["bomb_tiff.tif", "bomb_jpeg_tiff.tif"] {
            let r = Tiff::open(fixture(bomb)).and_then(|mut t| t.page(0));
            assert!(r.is_err(), "{bomb}");
        }
    }

    #[test]
    fn transparency_is_laid_over_white() {
        let g = grey_from_rgba(2, 1, &[0, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        assert_eq!(g.as_raw(), &[255, 0]);
        assert!(grey_from_rgba(2, 1, &[0; 7]).is_none());
        assert_eq!(luma(255, 255, 255), 255);
        assert_eq!(over_white(0, 128), 127);
    }
}
