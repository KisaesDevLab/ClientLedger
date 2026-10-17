//! The application icon (Q-122: option 1 until the owner picks).
//!
//! Option 1 is a teal rounded square (the accent colour) holding three white
//! ledger rows of different lengths and one amber mark, drawn here from a few
//! rounded rectangles so there is no image file to bundle and the window icon
//! always matches `assets/icon/option-1.svg`. Options 2 and 3 are drafts in
//! `assets/icon/` only. All three are original; none uses any other product's
//! artwork.

// Pixel maths on small image sizes: the casts below are bounded (sizes up to a
// few hundred pixels, channel values clamped to 0..=255).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::sync::Arc;

use eframe::egui::IconData;

const TEAL: [u8; 3] = [0x0F, 0x5E, 0x5A];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
const AMBER: [u8; 3] = [0xD9, 0x8A, 0x45];

/// A rounded rectangle in a 256-unit design space.
struct Shape {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    r: f32,
    color: [u8; 3],
    alpha: f32,
}

/// Option 1, in the same coordinates as `assets/icon/option-1.svg`.
const OPTION_1: [Shape; 5] = [
    Shape {
        x: 0.0,
        y: 0.0,
        w: 256.0,
        h: 256.0,
        r: 56.0,
        color: TEAL,
        alpha: 1.0,
    },
    Shape {
        x: 52.0,
        y: 68.0,
        w: 152.0,
        h: 18.0,
        r: 9.0,
        color: WHITE,
        alpha: 1.0,
    },
    Shape {
        x: 52.0,
        y: 119.0,
        w: 108.0,
        h: 18.0,
        r: 9.0,
        color: WHITE,
        alpha: 0.85,
    },
    Shape {
        x: 52.0,
        y: 170.0,
        w: 128.0,
        h: 18.0,
        r: 9.0,
        color: WHITE,
        alpha: 0.7,
    },
    Shape {
        x: 176.0,
        y: 113.0,
        w: 28.0,
        h: 30.0,
        r: 8.0,
        color: AMBER,
        alpha: 1.0,
    },
];

fn coverage(s: &Shape, px: f32, py: f32) -> bool {
    let (cx, cy) = (s.x + s.w / 2.0, s.y + s.h / 2.0);
    let (dx, dy) = (
        (px - cx).abs() - (s.w / 2.0 - s.r),
        (py - cy).abs() - (s.h / 2.0 - s.r),
    );
    let (qx, qy) = (dx.max(0.0), dy.max(0.0));
    (qx * qx + qy * qy).sqrt() + dx.max(dy).min(0.0) <= s.r
}

/// RGBA pixels (straight alpha) of option 1 at `size` by `size`.
#[must_use]
pub fn rgba(size: u32) -> Vec<u8> {
    const SUB: u32 = 4; // samples per axis
    let scale = 256.0 / size as f32;
    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
            for sy in 0..SUB {
                for sx in 0..SUB {
                    let px = (x as f32 + (sx as f32 + 0.5) / SUB as f32) * scale;
                    let py = (y as f32 + (sy as f32 + 0.5) / SUB as f32) * scale;
                    // Paint shapes in order over a transparent sample.
                    let mut sample = [0.0_f32; 4];
                    for s in &OPTION_1 {
                        if coverage(s, px, py) {
                            let sa = s.alpha;
                            for (i, c) in s.color.iter().enumerate() {
                                sample[i] = sample[i] * (1.0 - sa) + f32::from(*c) * sa;
                            }
                            sample[3] = sample[3] * (1.0 - sa) + sa;
                        }
                    }
                    r += sample[0] * sample[3];
                    g += sample[1] * sample[3];
                    b += sample[2] * sample[3];
                    a += sample[3];
                }
            }
            let n = (SUB * SUB) as f32;
            let alpha = a / n;
            let un = |v: f32| {
                if a > 0.0 {
                    (v / a).round().clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            out.extend_from_slice(&[un(r), un(g), un(b), (alpha * 255.0).round() as u8]);
        }
    }
    out
}

/// The icon for the window and task bar.
#[must_use]
pub fn viewport_icon() -> Option<Arc<IconData>> {
    const SIZE: u32 = 128;
    Some(Arc::new(IconData {
        rgba: rgba(SIZE),
        width: SIZE,
        height: SIZE,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(data: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [data[i], data[i + 1], data[i + 2], data[i + 3]]
    }

    #[test]
    fn size_and_corners() {
        let d = rgba(64);
        assert_eq!(d.len(), 64 * 64 * 4);
        assert_eq!(
            px(&d, 64, 0, 0)[3],
            0,
            "the corner is outside the rounded square"
        );
        assert_eq!(px(&d, 64, 63, 63)[3], 0);
        // A teal pixel in the open area.
        assert_eq!(px(&d, 64, 8, 32), [0x0F, 0x5E, 0x5A, 255]);
    }

    #[test]
    fn the_top_row_is_white_and_the_mark_is_amber() {
        let d = rgba(256);
        assert_eq!(px(&d, 256, 100, 77), [255, 255, 255, 255]);
        let amber = px(&d, 256, 190, 128);
        assert_eq!(amber, [0xD9, 0x8A, 0x45, 255]);
    }

    #[test]
    fn the_viewport_icon_is_square_rgba() {
        let i = viewport_icon().expect("icon");
        assert_eq!(i.rgba.len(), (i.width * i.height * 4) as usize);
    }

    #[test]
    fn the_svg_draft_uses_the_same_geometry() {
        let svg = include_str!("../../../assets/icon/option-1.svg");
        for needle in ["viewBox=\"0 0 256 256\"", "#0F5E5A", "#D98A45", "rx=\"56\""] {
            assert!(svg.contains(needle), "{needle}");
        }
    }
}
