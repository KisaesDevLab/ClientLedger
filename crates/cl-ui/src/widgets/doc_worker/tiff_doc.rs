//! Multi-page TIFF for the document worker (A02, plan D1, Q-A02k).
//!
//! `image` reads only a TIFF's first page, so the worker reads TIFFs with the
//! `tiff` crate directly: opening walks the page list (at most
//! [`MAX_PAGES`], which also ends a page list that loops back on itself) and
//! decodes the first page; later pages are decoded one at a time when asked
//! for, and only the last page drawn is kept.
//!
//! Each page is checked before its pixels are read: at most
//! [`MAX_IMAGE_SIDE`] px a side and [`MAX_IMAGE_ALLOC`] bytes, so a header
//! that claims a huge page (a "TIFF bomb") is refused without allocating.
//! The decoder's own buffers are held to the same limit. Grey (1, 8 or 16
//! bit, with or without alpha), RGB and RGBA (8 or 16 bit) and CMYK (8 bit)
//! convert to RGBA; CCITT G4 fax pages come through the `fax` crate as 1-bit
//! grey. Anything else (palette, Lab, YCbCr, planar, BigTIFF images over the
//! limits, a damaged strip) makes that page unreadable.
//!
//! Only the compressions in [`compression_taken`] are read. JPEG-in-TIFF is
//! not (review B1): its decoder sizes its buffer from the JPEG header inside
//! the strip, which the page-size check above never sees, so a few hundred
//! bytes could ask for gigabytes. The `tiff` crate is built without its
//! `jpeg` feature as well; the check here keeps such a page out even if that
//! feature comes back.

use std::io::Cursor;
use std::sync::Arc;

use ::tiff::ColorType;
use ::tiff::decoder::{Decoder, DecodingResult, Limits};
use ::tiff::tags::Tag;
use image::RgbaImage;

use super::{MAX_IMAGE_ALLOC, MAX_IMAGE_SIDE, MAX_PAGES, shrink};

/// One open TIFF.
pub(super) struct TiffDoc {
    decoder: Decoder<Cursor<Arc<[u8]>>>,
    pages: u32,
    /// The last page drawn (page number, pixels at most `MAX_SIDE`).
    cache: Option<(u32, RgbaImage)>,
}

/// The decoder's limits: its buffers at most [`MAX_IMAGE_ALLOC`] bytes, a
/// tag's values at most 1 MiB.
fn limits() -> Limits {
    // `Limits` is `non_exhaustive`: start from its defaults.
    let mut l = Limits::default();
    let alloc = usize::try_from(MAX_IMAGE_ALLOC).unwrap_or(usize::MAX);
    l.decoding_buffer_size = alloc;
    l.intermediate_buffer_size = alloc;
    l.ifd_value_size = 1024 * 1024;
    l
}

impl TiffDoc {
    /// Open `bytes`: the page count and whether there are more than
    /// [`MAX_PAGES`]. `None` when the file is not a TIFF this can read, or its
    /// first page is refused.
    pub(super) fn open(bytes: &Arc<[u8]>) -> Option<(TiffDoc, bool)> {
        let decoder = Decoder::new(Cursor::new(Arc::clone(bytes)))
            .ok()?
            .with_limits(limits());
        let mut doc = TiffDoc {
            decoder,
            pages: 1,
            cache: None,
        };
        let truncated = doc.count_pages();
        // The first page must show, as for a PNG or JPEG.
        doc.with_page(0, true, |_| ())?;
        Some((doc, truncated))
    }

    /// Walk the page list. A page whose directory reads counts even if its
    /// pixels can't be decoded (it shows as unreadable on its own); a
    /// damaged or looping list ends the walk. True when pages were left out.
    fn count_pages(&mut self) -> bool {
        while self.decoder.more_images() {
            if self.pages >= MAX_PAGES {
                return true;
            }
            let before = self.decoder.ifd_pointer();
            // An error after the directory was read (an unsupported page)
            // still moves to that page; an error reading the list does not.
            let _ = self.decoder.next_image();
            if self.decoder.ifd_pointer() == before {
                break;
            }
            self.pages += 1;
        }
        false
    }

    /// Pages shown (at most [`MAX_PAGES`]).
    pub(super) fn pages(&self) -> u32 {
        self.pages
    }

    /// Run `f` on page `page` (decoded now unless it is the kept page).
    /// `keep`: keep the decoded page for next time (drawn pages are kept,
    /// thumbnails are not, so they never push out the page on screen).
    pub(super) fn with_page<R>(
        &mut self,
        page: u32,
        keep: bool,
        f: impl FnOnce(&RgbaImage) -> R,
    ) -> Option<R> {
        if page >= self.pages {
            return None;
        }
        if let Some((p, img)) = &self.cache
            && *p == page
        {
            return Some(f(img));
        }
        let img = self.decode(page)?;
        let out = f(&img);
        if keep {
            self.cache = Some((page, img));
        }
        Some(out)
    }

    /// Decode one page, within the limits, at most `MAX_SIDE` on its long
    /// side.
    fn decode(&mut self, page: u32) -> Option<RgbaImage> {
        let d = &mut self.decoder;
        d.seek_to_image(usize::try_from(page).ok()?).ok()?;
        let (w, h) = d.dimensions().ok()?;
        if w == 0 || h == 0 || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
            return None;
        }
        // No tag means uncompressed (TIFF 6.0).
        let method = d
            .find_tag_unsigned::<u16>(Tag::Compression)
            .ok()?
            .unwrap_or(1);
        if !compression_taken(method) {
            return None;
        }
        let colour = d.colortype().ok()?;
        // The decoded samples and the RGBA copy each stay within the limit
        // (checked before any pixel is read).
        let px = u64::from(w) * u64::from(h);
        let sample_bytes = u64::from(colour.bit_depth().div_ceil(8));
        let decoded = px * u64::from(colour.num_samples()) * sample_bytes;
        if decoded > MAX_IMAGE_ALLOC || px * 4 > MAX_IMAGE_ALLOC {
            return None;
        }
        let data = d.read_image().ok()?;
        shrink(to_rgba(colour, w, h, &data)?)
    }
}

/// The TIFF compressions read: none, CCITT fax, LZW, Deflate (both tag
/// values) and PackBits. Each one's output is bounded by the page size
/// checked first. JPEG (6 and 7) is not among them.
#[must_use]
pub(super) fn compression_taken(method: u16) -> bool {
    matches!(method, 1 | 3 | 4 | 5 | 8 | 0x80B2 | 0x8005)
}

/// Convert decoded samples to RGBA8. The decoder has already turned
/// "white is zero" pages (fax) the right way round. `None` for a colour
/// this does not take or a buffer of the wrong size (a planar page, of
/// which only the first plane is read).
pub(super) fn to_rgba(
    colour: ColorType,
    width: u32,
    height: u32,
    data: &DecodingResult,
) -> Option<RgbaImage> {
    let (wu, hu) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    let pixels = wu.checked_mul(hu)?;
    let mut out = Vec::with_capacity(pixels.checked_mul(4)?);
    let gray = |out: &mut Vec<u8>, v: u8, a: u8| out.extend_from_slice(&[v, v, v, a]);
    // The high byte of a 16-bit sample.
    let hi = |v: u16| v.to_be_bytes()[0];
    match (colour, data) {
        (ColorType::Gray(1), DecodingResult::U8(b)) => {
            let stride = wu.div_ceil(8);
            if b.len() != stride.checked_mul(hu)? {
                return None;
            }
            for row in b.chunks_exact(stride) {
                for x in 0..wu {
                    let bit = (row[x / 8] >> (7 - x % 8)) & 1;
                    gray(&mut out, if bit == 1 { 255 } else { 0 }, 255);
                }
            }
        }
        (ColorType::Gray(8), DecodingResult::U8(b)) if b.len() == pixels => {
            for v in b {
                gray(&mut out, *v, 255);
            }
        }
        (ColorType::Gray(16), DecodingResult::U16(b)) if b.len() == pixels => {
            for v in b {
                gray(&mut out, hi(*v), 255);
            }
        }
        (
            ColorType::Multiband {
                bit_depth: 8,
                num_samples: 2,
            },
            DecodingResult::U8(b),
        ) if b.len() == pixels * 2 => {
            for p in b.as_chunks::<2>().0 {
                gray(&mut out, p[0], p[1]);
            }
        }
        (
            ColorType::Multiband {
                bit_depth: 16,
                num_samples: 2,
            },
            DecodingResult::U16(b),
        ) if b.len() == pixels * 2 => {
            for p in b.as_chunks::<2>().0 {
                gray(&mut out, hi(p[0]), hi(p[1]));
            }
        }
        (ColorType::RGB(8), DecodingResult::U8(b)) if b.len() == pixels * 3 => {
            for p in b.as_chunks::<3>().0 {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        (ColorType::RGB(16), DecodingResult::U16(b)) if b.len() == pixels * 3 => {
            for p in b.as_chunks::<3>().0 {
                out.extend_from_slice(&[hi(p[0]), hi(p[1]), hi(p[2]), 255]);
            }
        }
        (ColorType::RGBA(8), DecodingResult::U8(b)) if b.len() == pixels * 4 => {
            out.extend_from_slice(b);
        }
        (ColorType::RGBA(16), DecodingResult::U16(b)) if b.len() == pixels * 4 => {
            out.extend(b.iter().map(|v| hi(*v)));
        }
        (ColorType::CMYK(8), DecodingResult::U8(b)) if b.len() == pixels * 4 => {
            for p in b.as_chunks::<4>().0 {
                let k = 255 - u16::from(p[3]);
                // (255 − c)(255 − k) / 255 is at most 255.
                let ch = |c: u8| u8::try_from((255 - u16::from(c)) * k / 255).unwrap_or(255);
                out.extend_from_slice(&[ch(p[0]), ch(p[1]), ch(p[2]), 255]);
            }
        }
        _ => return None,
    }
    RgbaImage::from_raw(width, height, out)
}
