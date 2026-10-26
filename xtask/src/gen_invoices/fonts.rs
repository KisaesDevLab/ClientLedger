//! Test fonts (Q-A03f): IBM Plex from `assets/fonts`, and three pinned OFL
//! fonts from `docread-fonts.lock`, fetched into `target/docread-fonts` and
//! checked by SHA-256. The fetched fonts are never committed or shipped.

use crate::fetch_lock::{Entry, fetch_verified, key_value, parse_entry};
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub const LOCK_FILE: &str = "docread-fonts.lock";

/// A font a layout may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FontId {
    PlexSans,
    PlexSansBold,
    PlexMono,
    CourierPrime,
    CourierPrimeBold,
    Kalam,
    /// The held-out font (only `--holdout` layouts use it).
    PtSerif,
    PtSerifBold,
}

impl FontId {
    pub const ALL: [FontId; 8] = [
        FontId::PlexSans,
        FontId::PlexSansBold,
        FontId::PlexMono,
        FontId::CourierPrime,
        FontId::CourierPrimeBold,
        FontId::Kalam,
        FontId::PtSerif,
        FontId::PtSerifBold,
    ];

    /// The file: in `assets/fonts` for Plex, else the lock's file name.
    pub fn file(self) -> &'static str {
        match self {
            FontId::PlexSans => "IBMPlexSans-Regular.ttf",
            FontId::PlexSansBold => "IBMPlexSans-Bold.ttf",
            FontId::PlexMono => "IBMPlexMono-Regular.ttf",
            FontId::CourierPrime => "CourierPrime-Regular.ttf",
            FontId::CourierPrimeBold => "CourierPrime-Bold.ttf",
            FontId::Kalam => "Kalam-Regular.ttf",
            FontId::PtSerif => "PTSerif-Regular.ttf",
            FontId::PtSerifBold => "PTSerif-Bold.ttf",
        }
    }

    /// In the repository already (IBM Plex ships with the app).
    pub fn committed(self) -> bool {
        matches!(
            self,
            FontId::PlexSans | FontId::PlexSansBold | FontId::PlexMono
        )
    }

    /// The family, as the truth files name it.
    pub fn family(self) -> &'static str {
        match self {
            FontId::PlexSans | FontId::PlexSansBold => "plex-sans",
            FontId::PlexMono => "plex-mono",
            FontId::CourierPrime | FontId::CourierPrimeBold => "courier-prime",
            FontId::Kalam => "kalam",
            FontId::PtSerif | FontId::PtSerifBold => "pt-serif",
        }
    }

    /// The PDF base font name (without the subset tag).
    pub fn ps_name(self) -> &'static str {
        match self {
            FontId::PlexSans => "IBMPlexSans",
            FontId::PlexSansBold => "IBMPlexSans-Bold",
            FontId::PlexMono => "IBMPlexMono",
            FontId::CourierPrime => "CourierPrime",
            FontId::CourierPrimeBold => "CourierPrime-Bold",
            FontId::Kalam => "Kalam-Regular",
            FontId::PtSerif => "PTSerif-Regular",
            FontId::PtSerifBold => "PTSerif-Bold",
        }
    }

    pub fn monospace(self) -> bool {
        matches!(
            self,
            FontId::PlexMono | FontId::CourierPrime | FontId::CourierPrimeBold
        )
    }
}

/// The lock: the commit and one entry per file.
#[derive(Debug, Clone)]
pub struct Lock {
    pub commit: String,
    pub entries: Vec<Entry>,
}

impl Lock {
    /// Parse `docread-fonts.lock`.
    pub fn parse(text: &str) -> Result<Lock> {
        let mut commit = None;
        let mut entries = Vec::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(c) = key_value(LOCK_FILE, i + 1, line, "commit")? {
                commit = Some(c);
                continue;
            }
            entries.push(parse_entry(LOCK_FILE, i + 1, line)?);
        }
        let commit = commit.ok_or_else(|| anyhow!("{LOCK_FILE}: no commit line"))?;
        for e in &entries {
            let prefix = format!("https://raw.githubusercontent.com/google/fonts/{commit}/ofl/");
            if !e.url.starts_with(&prefix) {
                bail!("{LOCK_FILE}: {} does not name commit {commit}", e.target);
            }
        }
        Ok(Lock { commit, entries })
    }
}

/// Loaded font files.
pub struct Fonts {
    data: BTreeMap<FontId, Vec<u8>>,
}

impl Fonts {
    /// Plex from `assets/fonts`, and (unless `plex_only`) the pinned fonts,
    /// fetched when missing.
    pub fn load(root: &Path, plex_only: bool) -> Result<Fonts> {
        let mut data = BTreeMap::new();
        let mut fetched = BTreeMap::new();
        if !plex_only {
            let lock = Lock::parse(
                &std::fs::read_to_string(root.join(LOCK_FILE))
                    .with_context(|| format!("read {LOCK_FILE}"))?,
            )?;
            println!("gen-invoices: test fonts from google/fonts {}", lock.commit);
            let cache = root.join("target").join("docread-fonts");
            for e in &lock.entries {
                let txt = Path::new(&e.target)
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("txt"));
                let ext = if txt { "txt" } else { "ttf" };
                let p = fetch_verified("gen-invoices", LOCK_FILE, e, &cache, ext)?;
                fetched.insert(e.target.clone(), p);
            }
        }
        for id in FontId::ALL {
            let path = if id.committed() {
                root.join("assets").join("fonts").join(id.file())
            } else if let Some(p) = fetched.get(id.file()) {
                p.clone()
            } else {
                continue;
            };
            let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
            ttf_parser::Face::parse(&bytes, 0)
                .map_err(|e| anyhow!("{}: not a font ({e})", id.file()))?;
            data.insert(id, bytes);
        }
        Ok(Fonts { data })
    }

    pub fn has(&self, id: FontId) -> bool {
        self.data.contains_key(&id)
    }

    /// The font's bytes.
    pub fn bytes(&self, id: FontId) -> Result<&[u8]> {
        self.data
            .get(&id)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("font {} is not loaded", id.file()))
    }

    /// The parsed face.
    pub fn face(&self, id: FontId) -> Result<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(self.bytes(id)?, 0).map_err(|e| anyhow!("{}: {e}", id.file()))
    }

    /// Width of `text` at `size` points (no kerning; page geometry only).
    pub fn width(&self, id: FontId, text: &str, size: f32) -> f32 {
        let Ok(face) = self.face(id) else {
            return 0.0;
        };
        let upm = f32::from(face.units_per_em());
        let units: u32 = text
            .chars()
            .map(|c| {
                let g = face
                    .glyph_index(c)
                    .or_else(|| face.glyph_index('?'))
                    .unwrap_or_default();
                u32::from(face.glyph_hor_advance(g).unwrap_or(0))
            })
            .sum();
        #[allow(clippy::cast_precision_loss)]
        let u = units as f32;
        u * size / upm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_lock_pins_three_ofl_fonts_at_one_commit() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let lock = Lock::parse(&std::fs::read_to_string(root.join(LOCK_FILE)).unwrap()).unwrap();
        for id in FontId::ALL.iter().filter(|f| !f.committed()) {
            assert!(
                lock.entries.iter().any(|e| e.target == id.file()),
                "{} missing",
                id.file()
            );
        }
        for lic in ["CourierPrime-OFL.txt", "Kalam-OFL.txt", "PTSerif-OFL.txt"] {
            assert!(lock.entries.iter().any(|e| e.target == lic), "{lic}");
        }
        let bad = "commit = abc\nX.ttf 72f793376f8e2841656bf21d77a5de010f2929bd6956a22ee848ad0c7eb978af https://raw.githubusercontent.com/google/fonts/other/ofl/x/X.ttf\n";
        assert!(Lock::parse(bad).is_err());
    }

    #[test]
    fn plex_loads_from_the_repository_and_measures_text() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let fonts = Fonts::load(&root, true).unwrap();
        assert!(fonts.has(FontId::PlexSans) && !fonts.has(FontId::Kalam));
        let w = fonts.width(FontId::PlexMono, "0000", 10.0);
        assert!((w - 4.0 * fonts.width(FontId::PlexMono, "0", 10.0)).abs() < 0.01);
        assert!(fonts.width(FontId::PlexSans, "Total", 10.0) > 15.0);
    }
}
