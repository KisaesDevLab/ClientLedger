//! The company logo (Q-92): PNG or JPEG bytes from the caller, checked and
//! prepared for embedding. PNGs are decoded to 8-bit samples (alpha becomes
//! a soft mask); JPEGs are embedded as they are (`DCTDecode`), with only
//! their frame header read for size and colour components.

use crate::PrintError;

/// Largest logo file accepted.
pub const MAX_LOGO_BYTES: usize = 10 * 1024 * 1024;
/// Largest logo side, in pixels.
pub const MAX_LOGO_SIDE: u32 = 10_000;

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

impl Logo {
    /// Read a PNG or JPEG file's bytes.
    ///
    /// # Errors
    /// [`PrintError::Logo`] when the bytes are not a readable PNG or JPEG,
    /// are larger than [`MAX_LOGO_BYTES`], or a side exceeds
    /// [`MAX_LOGO_SIDE`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Logo, PrintError> {
        if bytes.len() > MAX_LOGO_BYTES {
            return Err(PrintError::Logo(
                "the picture file is larger than 10 MB".into(),
            ));
        }
        let logo = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            png_logo(bytes)?
        } else if bytes.starts_with(&[0xFF, 0xD8]) {
            jpeg_logo(bytes)?
        } else {
            return Err(PrintError::Logo(
                "the picture is not a PNG or JPEG file".into(),
            ));
        };
        if logo.width == 0
            || logo.height == 0
            || logo.width > MAX_LOGO_SIDE
            || logo.height > MAX_LOGO_SIDE
        {
            return Err(PrintError::Logo(
                "the picture is empty or larger than 10,000 pixels on a side".into(),
            ));
        }
        Ok(logo)
    }
}

fn png_logo(bytes: &[u8]) -> Result<Logo, PrintError> {
    let bad = |e: png::DecodingError| PrintError::Logo(format!("the PNG could not be read ({e})"));
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(bad)?;
    {
        let info = reader.info();
        if info.width > MAX_LOGO_SIDE || info.height > MAX_LOGO_SIDE {
            return Err(PrintError::Logo(
                "the picture is larger than 10,000 pixels on a side".into(),
            ));
        }
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| PrintError::Logo("the PNG is too large".into()))?;
    let mut buf = vec![0; size];
    let frame = reader.next_frame(&mut buf).map_err(bad)?;
    buf.truncate(frame.buffer_size());
    let (color, channels, has_alpha) = match frame.color_type {
        png::ColorType::Grayscale => (ColorSpace::Gray, 1, false),
        png::ColorType::GrayscaleAlpha => (ColorSpace::Gray, 2, true),
        png::ColorType::Rgb => (ColorSpace::Rgb, 3, false),
        png::ColorType::Rgba => (ColorSpace::Rgb, 4, true),
        png::ColorType::Indexed => {
            return Err(PrintError::Logo(
                "the PNG palette could not be expanded".into(),
            ));
        }
    };
    if frame.bit_depth != png::BitDepth::Eight {
        return Err(PrintError::Logo("the PNG is not 8 bits per sample".into()));
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
fn jpeg_logo(bytes: &[u8]) -> Result<Logo, PrintError> {
    let bad = || PrintError::Logo("the JPEG could not be read".into());
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
    /// enough for the frame reader.
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
    fn jpeg_frame_header_gives_size_and_components() {
        let l = Logo::from_bytes(&jpeg_header(640, 200)).unwrap();
        assert_eq!((l.width, l.height, l.color), (640, 200, ColorSpace::Rgb));
        assert!(matches!(l.data, LogoData::Jpeg(_)));
    }

    #[test]
    fn other_files_and_oversized_pictures_are_refused() {
        assert!(Logo::from_bytes(b"GIF89a").is_err());
        assert!(Logo::from_bytes(&[0xFF, 0xD8, 0xFF]).is_err());
        assert!(Logo::from_bytes(&jpeg_header(20_000, 10)).is_err());
        assert!(Logo::from_bytes(&jpeg_header(0, 10)).is_err());
        let mut truncated = png_rgba(3, 2);
        truncated.truncate(30);
        assert!(Logo::from_bytes(&truncated).is_err());
    }
}
