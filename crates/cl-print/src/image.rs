//! The company logo (Q-92): PNG or JPEG bytes from the caller, checked and
//! prepared for embedding. PNGs are decoded to 8-bit samples (alpha becomes
//! a soft mask); JPEGs are embedded in PDFs as they are (`DCTDecode`), with
//! only their frame header read for size and colour components, and decoded
//! only when a printer needs pixels ([`Logo::rgb_on_white`]).
//!
//! A logo that can't be used (`WebP`, which `MyBooks` Online accepts; over
//! 2 MB or 16 megapixels; unreadable) is a [`LogoProblem`]: the caller
//! prints without the logo and shows a notice (Q-P19af, Q-P19v). It never
//! stops a print.

/// Largest logo file accepted (Q-P19v).
pub const MAX_LOGO_BYTES: usize = 2 * 1024 * 1024;
/// Largest logo, in pixels (16 megapixels).
pub const MAX_LOGO_PIXELS: u64 = 16_000_000;
/// Largest logo side, in pixels.
pub const MAX_LOGO_SIDE: u32 = 10_000;

/// Why a logo can't be printed. The caller prints without it and tells the
/// owner to use a PNG or JPEG in Company Information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LogoProblem {
    /// A WebP picture.
    #[error("the logo is a WebP picture; use a PNG or JPEG")]
    WebP,
    /// Neither PNG nor JPEG.
    #[error("the logo is not a PNG or JPEG picture")]
    NotPngOrJpeg,
    /// The file is over 2 MB.
    #[error("the logo file is larger than 2 MB")]
    TooLarge,
    /// Over 16 megapixels or 10,000 pixels on a side, or empty.
    #[error("the logo has too many pixels (over 16 megapixels)")]
    TooManyPixels,
    /// The file could not be read.
    #[error("the logo file could not be read")]
    Unreadable,
}

/// Colour model of the samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSpace {
    /// One component.
    Gray,
    /// Three components.
    Rgb,
    /// Four components (JPEG only).
    Cmyk,
}

/// How the picture is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogoData {
    /// JPEG bytes, embedded unchanged.
    Jpeg(Vec<u8>),
    /// Decoded 8-bit samples, with an optional 8-bit alpha channel.
    Raw {
        /// Colour samples, row-major.
        samples: Vec<u8>,
        /// Alpha samples, if the PNG had transparency.
        alpha: Option<Vec<u8>>,
    },
}

/// A logo ready to embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logo {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour model.
    pub color: ColorSpace,
    /// The picture.
    pub data: LogoData,
}

/// Opaque 8-bit RGB pixels, row-major from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 3` bytes.
    pub rgb: Vec<u8>,
}

fn check_size(width: u32, height: u32) -> Result<(), LogoProblem> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_LOGO_SIDE
        || height > MAX_LOGO_SIDE
        || pixels > MAX_LOGO_PIXELS
    {
        return Err(LogoProblem::TooManyPixels);
    }
    Ok(())
}

impl Logo {
    /// Read a PNG or JPEG file's bytes.
    ///
    /// # Errors
    /// The [`LogoProblem`] to tell the owner about.
    pub fn from_bytes(bytes: &[u8]) -> Result<Logo, LogoProblem> {
        if bytes.len() > MAX_LOGO_BYTES {
            return Err(LogoProblem::TooLarge);
        }
        if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            return Err(LogoProblem::WebP);
        }
        let logo = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            png_logo(bytes)?
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            jpeg_logo(bytes)?
        } else {
            return Err(LogoProblem::NotPngOrJpeg);
        };
        check_size(logo.width, logo.height)?;
        Ok(logo)
    }

    /// The picture as opaque RGB over white (for printers that take
    /// pixels, such as the Windows print path).
    ///
    /// # Errors
    /// [`LogoProblem::Unreadable`] when the JPEG data can't be decoded.
    pub fn rgb_on_white(&self) -> Result<RgbImage, LogoProblem> {
        let n = usize::try_from(u64::from(self.width) * u64::from(self.height))
            .map_err(|_| LogoProblem::TooManyPixels)?;
        let rgb = match &self.data {
            LogoData::Jpeg(bytes) => decode_jpeg_rgb(bytes, n)?,
            LogoData::Raw { samples, alpha } => {
                let ch = if self.color == ColorSpace::Gray { 1 } else { 3 };
                if samples.len() != n * ch {
                    return Err(LogoProblem::Unreadable);
                }
                let mut out = Vec::with_capacity(n * 3);
                for i in 0..n {
                    let a = alpha
                        .as_ref()
                        .and_then(|a| a.get(i))
                        .copied()
                        .unwrap_or(255);
                    for k in 0..3 {
                        let v = samples[i * ch + if ch == 1 { 0 } else { k }];
                        out.push(over_white(v, a));
                    }
                }
                out
            }
        };
        Ok(RgbImage {
            width: self.width,
            height: self.height,
            rgb,
        })
    }
}

/// `v` with opacity `a` over white, exactly in integers.
fn over_white(v: u8, a: u8) -> u8 {
    let (v, a) = (u32::from(v), u32::from(a));
    // (v*a + 255*(255-a)) / 255, rounded; always <= 255.
    u8::try_from((v * a + 255 * (255 - a) + 127) / 255).unwrap_or(255)
}

fn decode_jpeg_rgb(bytes: &[u8], pixels: usize) -> Result<Vec<u8>, LogoProblem> {
    use zune_jpeg::zune_core::colorspace::ColorSpace as Zcs;
    use zune_jpeg::zune_core::options::DecoderOptions;
    let side = usize::try_from(MAX_LOGO_SIDE).unwrap_or(10_000);
    let opts = DecoderOptions::default()
        .jpeg_set_out_colorspace(Zcs::RGB)
        .set_max_width(side)
        .set_max_height(side);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(std::io::Cursor::new(bytes), opts);
    let rgb = dec.decode().map_err(|_| LogoProblem::Unreadable)?;
    if rgb.len() != pixels * 3 {
        return Err(LogoProblem::Unreadable);
    }
    Ok(rgb)
}

fn png_logo(bytes: &[u8]) -> Result<Logo, LogoProblem> {
    let bad = |_: png::DecodingError| LogoProblem::Unreadable;
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(bad)?;
    {
        let info = reader.info();
        check_size(info.width, info.height)?;
    }
    let size = reader
        .output_buffer_size()
        .ok_or(LogoProblem::TooManyPixels)?;
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).map_err(bad)?;
    buf.truncate(frame.buffer_size());
    let (color, channels, has_alpha) = match frame.color_type {
        png::ColorType::Grayscale => (ColorSpace::Gray, 1, false),
        png::ColorType::GrayscaleAlpha => (ColorSpace::Gray, 2, true),
        png::ColorType::Rgb => (ColorSpace::Rgb, 3, false),
        png::ColorType::Rgba => (ColorSpace::Rgb, 4, true),
        png::ColorType::Indexed => return Err(LogoProblem::Unreadable),
    };
    if frame.bit_depth != png::BitDepth::Eight {
        return Err(LogoProblem::Unreadable);
    }
    let (samples, alpha) = if has_alpha {
        let n_colour = channels - 1;
        let mut s = Vec::with_capacity(buf.len() / channels * n_colour);
        let mut a = Vec::with_capacity(buf.len() / channels);
        for px in buf.chunks_exact(channels) {
            s.extend_from_slice(&px[..n_colour]);
            a.push(px[n_colour]);
        }
        (s, Some(a))
    } else {
        (buf, None)
    };
    Ok(Logo {
        width: frame.width,
        height: frame.height,
        color,
        data: LogoData::Raw { samples, alpha },
    })
}

/// Width, height and components from the JPEG frame header (`SOFn`).
fn jpeg_logo(bytes: &[u8]) -> Result<Logo, LogoProblem> {
    let bad = || LogoProblem::Unreadable;
    let mut i = 2usize;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return Err(bad());
        }
        let marker = bytes[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // Stand-alone markers carry no length.
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let seg = bytes.get(i + 4..i + 2 + len).ok_or_else(bad)?;
            if seg.len() < 6 {
                return Err(bad());
            }
            let height = u32::from(u16::from_be_bytes([seg[1], seg[2]]));
            let width = u32::from(u16::from_be_bytes([seg[3], seg[4]]));
            let color = match seg[5] {
                1 => ColorSpace::Gray,
                3 => ColorSpace::Rgb,
                4 => ColorSpace::Cmyk,
                _ => return Err(bad()),
            };
            return Ok(Logo {
                width,
                height,
                color,
                data: LogoData::Jpeg(bytes.to_vec()),
            });
        }
        if len < 2 {
            return Err(bad());
        }
        i += 2 + len;
    }
    Err(bad())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny RGBA PNG.
    pub(crate) fn png_rgba(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().unwrap();
            let data: Vec<u8> = (0..w * h)
                .flat_map(|i| [u8::try_from(i % 256).unwrap(), 10, 20, 128])
                .collect();
            wr.write_image_data(&data).unwrap();
        }
        out
    }

    /// A minimal JPEG header (SOI, APP0, SOF0 for 3 components, EOI) —
    /// enough for the frame reader, not for decoding.
    pub(crate) fn jpeg_header(w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn png_with_alpha_splits_samples_and_mask() {
        let l = Logo::from_bytes(&png_rgba(3, 2)).unwrap();
        assert_eq!((l.width, l.height, l.color), (3, 2, ColorSpace::Rgb));
        let LogoData::Raw { samples, alpha } = l.data else {
            panic!()
        };
        assert_eq!(samples.len(), 18);
        assert_eq!(alpha.unwrap(), vec![128; 6]);
    }

    #[test]
    fn png_pixels_are_composited_over_white() {
        let img = Logo::from_bytes(&png_rgba(3, 2))
            .unwrap()
            .rgb_on_white()
            .unwrap();
        assert_eq!((img.width, img.height, img.rgb.len()), (3, 2, 18));
        // Pixel 0: (0, 10, 20) at alpha 128 over white.
        assert_eq!(&img.rgb[..3], &[127, 132, 137]);
        assert_eq!(over_white(0, 255), 0);
        assert_eq!(over_white(0, 0), 255);
    }

    #[test]
    fn jpeg_frame_header_gives_size_and_components() {
        let l = Logo::from_bytes(&jpeg_header(640, 200)).unwrap();
        assert_eq!((l.width, l.height, l.color), (640, 200, ColorSpace::Rgb));
        assert!(matches!(l.data, LogoData::Jpeg(_)));
        // A header without image data can't be decoded to pixels.
        assert_eq!(l.rgb_on_white(), Err(LogoProblem::Unreadable));
    }

    #[test]
    fn unusable_logos_give_a_reason() {
        assert_eq!(Logo::from_bytes(b"GIF89a"), Err(LogoProblem::NotPngOrJpeg));
        assert_eq!(
            Logo::from_bytes(b"RIFF\0\0\0\0WEBPVP8 "),
            Err(LogoProblem::WebP)
        );
        assert_eq!(
            Logo::from_bytes(&[0xFF, 0xD8, 0xFF]),
            Err(LogoProblem::Unreadable)
        );
        assert_eq!(
            Logo::from_bytes(&jpeg_header(20_000, 10)),
            Err(LogoProblem::TooManyPixels)
        );
        assert_eq!(
            Logo::from_bytes(&jpeg_header(5000, 4000)),
            Err(LogoProblem::TooManyPixels)
        );
        assert!(Logo::from_bytes(&jpeg_header(4000, 4000)).is_ok());
        assert_eq!(
            Logo::from_bytes(&jpeg_header(0, 10)),
            Err(LogoProblem::TooManyPixels)
        );
        let mut big = jpeg_header(10, 10);
        big.resize(MAX_LOGO_BYTES + 1, 0);
        assert_eq!(Logo::from_bytes(&big), Err(LogoProblem::TooLarge));
        let mut truncated = png_rgba(3, 2);
        truncated.truncate(30);
        assert_eq!(Logo::from_bytes(&truncated), Err(LogoProblem::Unreadable));
    }
}
