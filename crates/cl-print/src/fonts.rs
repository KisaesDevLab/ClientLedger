//! The fonts printed forms use: IBM Plex Sans and Mono from `assets/fonts`
//! (SIL OFL 1.1, listed in ATTRIBUTION.md), compiled in so a printed form
//! never depends on what the computer has installed (DESIGN.md "Tokens":
//! Sans for text, Mono for numbers, dates, check numbers and amounts).
//!
//! Metrics come from the font files through `skrifa`, so right-aligned
//! amounts and asterisk fills land where the PDF draws them. Geometry is the
//! only floating point here; money never passes through these functions.

use crate::PrintError;
use skrifa::MetadataProvider as _;
use skrifa::instance::{LocationRef, Size};

const SANS_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const SANS_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Bold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");

/// Millimetres per PostScript point.
pub const MM_PER_PT: f32 = 25.4 / 72.0;

/// A font a layout may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Font {
    /// IBM Plex Sans 400.
    Sans,
    /// IBM Plex Sans 600 (labels, column headings).
    SansSemiBold,
    /// IBM Plex Sans 700 (document titles).
    SansBold,
    /// IBM Plex Mono 400 (amounts, dates, numbers).
    Mono,
}

impl Font {
    /// Every font, in a fixed order.
    pub const ALL: [Font; 4] = [Font::Sans, Font::SansSemiBold, Font::SansBold, Font::Mono];

    /// The PDF base font name (without the subset tag).
    #[must_use]
    pub fn ps_name(self) -> &'static str {
        match self {
            Font::Sans => "IBMPlexSans",
            Font::SansSemiBold => "IBMPlexSans-SemiBold",
            Font::SansBold => "IBMPlexSans-Bold",
            Font::Mono => "IBMPlexMono",
        }
    }

    /// True for the fixed-pitch face.
    #[must_use]
    pub fn monospace(self) -> bool {
        matches!(self, Font::Mono)
    }

    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Font::Sans => SANS_REGULAR,
            Font::SansSemiBold => SANS_SEMIBOLD,
            Font::SansBold => SANS_BOLD,
            Font::Mono => MONO_REGULAR,
        }
    }

    /// The parsed face.
    pub(crate) fn face(self) -> Result<Face, PrintError> {
        skrifa::FontRef::new(self.bytes())
            .map(|font| Face { font })
            .map_err(|e| PrintError::Font(format!("{}: {e}", self.ps_name())))
    }

    /// Width of `text` at `size_pt`, in millimetres (advance widths, no
    /// kerning, the same glyphs the PDF draws).
    #[must_use]
    pub fn width_mm(self, text: &str, size_pt: f32) -> f32 {
        let Ok(face) = self.face() else {
            return 0.0;
        };
        let upm = f32::from(face.units_per_em());
        let units: f32 = text.chars().map(|c| face.advance(face.glyph(c))).sum();
        units * size_pt / upm * MM_PER_PT
    }

    /// Distance from the top of a line box to the baseline, in millimetres
    /// (the font's ascender at `size_pt`).
    #[must_use]
    pub fn ascent_mm(self, size_pt: f32) -> f32 {
        let Ok(face) = self.face() else {
            return size_pt * 0.8 * MM_PER_PT;
        };
        face.ascender() / f32::from(face.units_per_em()) * size_pt * MM_PER_PT
    }
}

/// A parsed font (metrics in font units).
pub(crate) struct Face {
    font: skrifa::FontRef<'static>,
}

impl Face {
    fn metrics(&self) -> skrifa::metrics::Metrics {
        self.font.metrics(Size::unscaled(), LocationRef::default())
    }

    pub(crate) fn units_per_em(&self) -> u16 {
        self.metrics().units_per_em
    }

    /// The glyph for `c`, else for `?`, else `.notdef`.
    pub(crate) fn glyph(&self, c: char) -> u16 {
        let map = self.font.charmap();
        map.map(c)
            .or_else(|| map.map('?'))
            .and_then(|g| u16::try_from(g.to_u32()).ok())
            .unwrap_or(0)
    }

    /// Advance width of glyph `gid`.
    pub(crate) fn advance(&self, gid: u16) -> f32 {
        self.font
            .glyph_metrics(Size::unscaled(), LocationRef::default())
            .advance_width(skrifa::GlyphId::new(u32::from(gid)))
            .unwrap_or(0.0)
    }

    /// Font bounding box: x min, y min, x max, y max.
    pub(crate) fn bbox(&self) -> [f32; 4] {
        self.metrics()
            .bounds
            .map_or([0.0, -200.0, 1000.0, 900.0], |b| {
                [b.x_min, b.y_min, b.x_max, b.y_max]
            })
    }

    pub(crate) fn ascender(&self) -> f32 {
        self.metrics().ascent
    }

    pub(crate) fn descender(&self) -> f32 {
        self.metrics().descent
    }

    pub(crate) fn cap_height(&self) -> f32 {
        let m = self.metrics();
        m.cap_height.unwrap_or(m.ascent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_font_parses_and_measures() {
        for f in Font::ALL {
            assert!(f.face().is_ok(), "{f:?}");
            assert!(f.width_mm("Total", 10.0) > 5.0, "{f:?}");
            let a = f.ascent_mm(10.0);
            assert!(a > 2.5 && a < 4.0, "{f:?} ascent {a}");
        }
    }

    #[test]
    fn mono_digits_share_one_width_and_width_scales_with_size() {
        let one = Font::Mono.width_mm("0", 10.0);
        assert!((Font::Mono.width_mm("0123456789", 10.0) - 10.0 * one).abs() < 0.001);
        assert!(
            (Font::Sans.width_mm("Pay", 20.0) - 2.0 * Font::Sans.width_mm("Pay", 10.0)).abs()
                < 0.001
        );
    }
}
