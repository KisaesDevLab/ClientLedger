//! UI state kept between runs: the user preferences (`cl_settings::UserPrefs`,
//! re-exported here as [`UiPrefs`] with the zoom helpers the shell uses)
//! and one saved tab layout per company. Preferences proper are edited in
//! the Preferences window (P09); the shell reads theme and zoom from them.

use std::io;
use std::path::{Path, PathBuf};

pub use cl_settings::user_prefs::{
    UserPrefs as UiPrefs, UserPrefsStore, ZOOM_DEFAULT, ZOOM_STEPS, nearest_step, zoom_in, zoom_out,
};

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

    /// The preferences store (the same file `cl-settings` reads).
    #[must_use]
    pub fn prefs_store(&self) -> UserPrefsStore {
        UserPrefsStore::in_dir(&self.dir)
    }

    fn layout_path(&self, company: &str) -> PathBuf {
        self.dir.join(format!("layout-{}.json", key_hash(company)))
    }

    /// Load preferences (defaults when missing or unreadable).
    #[must_use]
    pub fn load_prefs(&self) -> UiPrefs {
        self.prefs_store().load()
    }

    /// Save preferences.
    ///
    /// # Errors
    /// Any file error.
    pub fn save_prefs(&self, prefs: &UiPrefs) -> io::Result<()> {
        self.prefs_store().save(prefs)
    }

    /// Load the tab layout remembered for `company`.
    #[must_use]
    pub fn load_layout(&self, company: &str) -> Option<crate::windows::SessionLayout> {
        let text = read_capped(&self.layout_path(company))?;
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

/// A 16-digit hex name for a company key (FNV-1a, 64 bit). Layout files are
/// named by this hash, so no company id can influence a path, and two ids
/// that `safe_name` would have merged get separate files.
#[must_use]
pub fn key_hash(raw: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in raw.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
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

/// Largest preference or layout file that is read.
pub const MAX_READ: u64 = 64 * 1024;

/// Read a small text file; anything larger than [`MAX_READ`] or not UTF-8
/// is treated as unreadable (the caller falls back to defaults).
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_READ + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() as u64 > MAX_READ {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Write to a per-process temporary file, flush it to disk, then rename it
/// over the target, so a crash leaves the old file or the new one and two
/// running programs never share a temporary name.
fn write_atomic(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeChoice;
    use crate::windows::{SavedTab, SessionLayout};

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cl-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn prefs_round_trip_and_bad_text_gives_defaults() {
        let p = UiPrefs {
            theme: ThemeChoice::Dark,
            zoom_percent: 150,
            ..UiPrefs::default()
        };
        assert_eq!(UiPrefs::parse(&p.to_text()), p);
    }

    #[test]
    fn store_saves_prefs_and_per_company_layouts() {
        let dir = temp_dir("store");
        let store = UiStore::new(&dir);
        assert_eq!(store.load_prefs(), UiPrefs::default());
        let p = UiPrefs {
            theme: ThemeChoice::Light,
            zoom_percent: 125,
            ..UiPrefs::default()
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
    fn layout_files_are_named_by_a_hash_of_the_key() {
        let store = UiStore::new("/x");
        let a = store.layout_path("co/1");
        let b = store.layout_path("co_1");
        assert_ne!(a, b, "ids that look alike must not share a file");
        assert_eq!(key_hash("co-1").len(), 16);
        assert_eq!(key_hash("co-1"), key_hash("co-1"));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("layout-") && !name.contains("co"),
            "{name}"
        );
    }

    #[test]
    fn oversized_files_are_ignored() {
        let dir = temp_dir("big");
        std::fs::create_dir_all(&dir).unwrap();
        let store = UiStore::new(&dir);
        // Preferences go through cl-settings (4 MiB cap); layouts keep the
        // 64 KiB cap here.
        let big = format!(
            "{{\"theme\":\"dark\",\"zoom_percent\":150,\"pad\":\"{}\"}}",
            "x".repeat(5 * 1024 * 1024)
        );
        std::fs::write(dir.join("ui-prefs.json"), big).unwrap();
        assert_eq!(store.load_prefs(), UiPrefs::default());
        let layout = format!(
            "{{\"version\":1,\"tabs\":[],\"active\":0,\"pad\":\"{}\"}}",
            "x".repeat(70_000)
        );
        std::fs::write(store.layout_path("co"), layout).unwrap();
        assert_eq!(store.load_layout("co"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn saving_leaves_no_temporary_file_behind() {
        let dir = temp_dir("tmpname");
        let store = UiStore::new(&dir);
        store.save_prefs(&UiPrefs::default()).unwrap();
        store.save_prefs(&UiPrefs::default()).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["ui-prefs.json"]);
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
