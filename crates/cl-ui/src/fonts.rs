//! Embedded fonts: IBM Plex Sans for the interface and IBM Plex Mono for
//! numbers, dates, check numbers and amounts (DESIGN.md typography).
//! Both are SIL OFL 1.1; see `ATTRIBUTION.md` and `assets/fonts/OFL.txt`.
//!
//! Plex has no emoji or symbol fallback and egui's default fonts are not
//! bundled (their licenses are outside the allowlist), so the UI draws its
//! own marks (dots, check marks, chevrons) with the painter instead of
//! relying on glyphs.

use std::collections::BTreeMap;
use std::sync::Arc;

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};

const SANS_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Regular.ttf");
const SANS_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Medium.ttf");
const SANS_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-SemiBold.ttf");
const SANS_BOLD: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexSans-Bold.ttf");
const MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Regular.ttf");
const MONO_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/IBMPlexMono-Medium.ttf");

/// Sans 500.
#[must_use]
pub fn medium() -> FontFamily {
    FontFamily::Name("sans-medium".into())
}

/// Sans 600.
#[must_use]
pub fn semibold() -> FontFamily {
    FontFamily::Name("sans-semibold".into())
}

/// Sans 700.
#[must_use]
pub fn bold() -> FontFamily {
    FontFamily::Name("sans-bold".into())
}

/// Mono 500.
#[must_use]
pub fn mono_medium() -> FontFamily {
    FontFamily::Name("mono-medium".into())
}

/// The font set: Proportional is Plex Sans 400, Monospace is Plex Mono 400,
/// and named families carry the heavier weights.
#[must_use]
pub fn definitions() -> FontDefinitions {
    let faces: [(&str, &[u8]); 6] = [
        ("plex-sans-400", SANS_REGULAR),
        ("plex-sans-500", SANS_MEDIUM),
        ("plex-sans-600", SANS_SEMIBOLD),
        ("plex-sans-700", SANS_BOLD),
        ("plex-mono-400", MONO_REGULAR),
        ("plex-mono-500", MONO_MEDIUM),
    ];
    let mut font_data = BTreeMap::new();
    for (name, bytes) in faces {
        font_data.insert(name.to_string(), Arc::new(FontData::from_static(bytes)));
    }
    let mut families = BTreeMap::new();
    families.insert(FontFamily::Proportional, vec!["plex-sans-400".to_string()]);
    families.insert(FontFamily::Monospace, vec!["plex-mono-400".to_string()]);
    families.insert(medium(), vec!["plex-sans-500".to_string()]);
    families.insert(semibold(), vec!["plex-sans-600".to_string()]);
    families.insert(bold(), vec!["plex-sans-700".to_string()]);
    families.insert(mono_medium(), vec!["plex-mono-500".to_string()]);
    FontDefinitions {
        font_data,
        families,
    }
}

/// Install the fonts. Call once at start-up.
pub fn install(ctx: &Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_points_at_an_embedded_face() {
        let d = definitions();
        for (family, names) in &d.families {
            assert!(!names.is_empty(), "{family:?}");
            for n in names {
                assert!(d.font_data.contains_key(n), "{family:?} -> {n}");
            }
        }
        assert_eq!(d.font_data.len(), 6);
    }

    #[test]
    fn embedded_files_are_truetype() {
        for bytes in [
            SANS_REGULAR,
            SANS_MEDIUM,
            SANS_SEMIBOLD,
            SANS_BOLD,
            MONO_REGULAR,
            MONO_MEDIUM,
        ] {
            assert!(bytes.len() > 100_000);
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "TrueType header");
        }
    }
}
