//! Per-user look-and-feel choices that are remembered between runs: colour
//! theme and app zoom (Q-66: 80 to 200 percent). Stored as one small JSON file
//! in the app data folder. Preferences proper (AI keys, defaults) arrive with
//! `cl-settings` in P09; these two are needed by the shell now.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::theme::ThemeChoice;

/// Allowed zoom percentages, smallest to largest.
pub const ZOOM_STEPS: [u16; 9] = [80, 90, 100, 110, 125, 150, 175, 190, 200];

/// Default zoom.
pub const ZOOM_DEFAULT: u16 = 100;

/// What is remembered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiPrefs {
    /// Light, dark, or follow the system.
    pub theme: ThemeChoice,
    /// Zoom in percent; always one of [`ZOOM_STEPS`].
    pub zoom_percent: u16,
}

impl Default for UiPrefs {
    fn default() -> Self {
        UiPrefs {
            theme: ThemeChoice::Auto,
            zoom_percent: ZOOM_DEFAULT,
        }
    }
}

/// The step nearest to `percent`.
#[must_use]
pub fn nearest_step(percent: u16) -> u16 {
    ZOOM_STEPS
        .iter()
        .copied()
        .min_by_key(|s| s.abs_diff(percent))
        .unwrap_or(ZOOM_DEFAULT)
}

/// One step up (no change at the top).
#[must_use]
pub fn zoom_in(percent: u16) -> u16 {
    let cur = nearest_step(percent);
    ZOOM_STEPS.iter().copied().find(|s| *s > cur).unwrap_or(cur)
}

/// One step down (no change at the bottom).
#[must_use]
pub fn zoom_out(percent: u16) -> u16 {
    let cur = nearest_step(percent);
    ZOOM_STEPS
        .iter()
        .rev()
        .copied()
        .find(|s| *s < cur)
        .unwrap_or(cur)
}

impl UiPrefs {
    /// Parse stored text. Unreadable text gives the defaults; a zoom outside
    /// the allowed range is snapped to the nearest step.
    #[must_use]
    pub fn parse(text: &str) -> UiPrefs {
        let mut p: UiPrefs = serde_json::from_str(text).unwrap_or_default();
        p.zoom_percent = nearest_step(p.zoom_percent);
        p
    }

    /// Text to store.
    #[must_use]
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The factor egui wants (1.0 = 100 percent).
    #[must_use]
    pub fn zoom_factor(&self) -> f32 {
        f32::from(self.zoom_percent) / 100.0
    }
}

/// Where UI state is kept: preferences, and one saved tab layout per company.
#[derive(Debug, Clone)]
pub struct UiStore {
    dir: PathBuf,
}

impl UiStore {
    /// A store under `dir` (created on first save).
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> UiStore {
        UiStore { dir: dir.into() }
    }

    /// The store's folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn prefs_path(&self) -> PathBuf {
        self.dir.join("ui-prefs.json")
    }

    fn layout_path(&self, company: &str) -> PathBuf {
        self.dir.join(format!("layout-{}.json", safe_name(company)))
    }

    /// Load preferences (defaults when missing or unreadable).
    #[must_use]
    pub fn load_prefs(&self) -> UiPrefs {
        std::fs::read_to_string(self.prefs_path())
            .map(|t| UiPrefs::parse(&t))
            .unwrap_or_default()
    }

    /// Save preferences.
    ///
    /// # Errors
    /// Any file error.
    pub fn save_prefs(&self, prefs: &UiPrefs) -> io::Result<()> {
        write_atomic(&self.prefs_path(), &prefs.to_text())
    }

    /// Load the tab layout remembered for `company`.
    #[must_use]
    pub fn load_layout(&self, company: &str) -> Option<crate::windows::SessionLayout> {
        let text = std::fs::read_to_string(self.layout_path(company)).ok()?;
        crate::windows::SessionLayout::parse(&text)
    }

    /// Remember the tab layout for `company`.
    ///
    /// # Errors
    /// Any file error.
    pub fn save_layout(
        &self,
        company: &str,
        layout: &crate::windows::SessionLayout,
    ) -> io::Result<()> {
        write_atomic(&self.layout_path(company), &layout.to_text())
    }
}

/// A file name made only of letters, digits, `-` and `_`, so a company id can
/// never point outside the store folder.
#[must_use]
pub fn safe_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if s.is_empty() { "default".into() } else { s }
}

fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{SavedTab, SessionLayout};

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn zoom_steps_stay_inside_80_to_200() {
        assert_eq!(ZOOM_STEPS.first(), Some(&80));
        assert_eq!(ZOOM_STEPS.last(), Some(&200));
        let mut z = 100;
        for _ in 0..20 {
            z = zoom_in(z);
        }
        assert_eq!(z, 200);
        for _ in 0..20 {
            z = zoom_out(z);
        }
        assert_eq!(z, 80);
        assert_eq!(zoom_in(100), 110);
        assert_eq!(zoom_out(100), 90);
    }

    #[test]
    fn odd_values_snap_to_a_step() {
        assert_eq!(nearest_step(0), 80);
        assert_eq!(nearest_step(999), 200);
        assert_eq!(nearest_step(104), 100);
    }

    #[test]
    fn prefs_round_trip_and_bad_text_gives_defaults() {
        let p = UiPrefs {
            theme: ThemeChoice::Dark,
            zoom_percent: 150,
        };
        assert_eq!(UiPrefs::parse(&p.to_text()), p);
        assert_eq!(UiPrefs::parse("garbage"), UiPrefs::default());
        let clamped = UiPrefs::parse(r#"{"theme":"light","zoom_percent":500}"#);
        assert_eq!(clamped.zoom_percent, 200);
        assert!((clamped.zoom_factor() - 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn store_saves_prefs_and_per_company_layouts() {
        let dir = temp_dir("store");
        let store = UiStore::new(&dir);
        assert_eq!(store.load_prefs(), UiPrefs::default());
        let p = UiPrefs {
            theme: ThemeChoice::Light,
            zoom_percent: 125,
        };
        store.save_prefs(&p).unwrap();
        assert_eq!(store.load_prefs(), p);

        let layout = SessionLayout {
            version: SessionLayout::VERSION,
            tabs: vec![SavedTab {
                kind: "nav.home".into(),
                popped_out: false,
            }],
            active: 0,
        };
        store.save_layout("company-a", &layout).unwrap();
        assert_eq!(store.load_layout("company-a"), Some(layout));
        assert_eq!(store.load_layout("company-b"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn company_ids_cannot_escape_the_folder() {
        assert_eq!(safe_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(safe_name(""), "default");
        assert_eq!(safe_name("a1-b_2"), "a1-b_2");
        let store = UiStore::new("/x");
        assert!(store.layout_path("../a").starts_with("/x"));
    }
}
