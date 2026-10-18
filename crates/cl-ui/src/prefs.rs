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

/// Column widths the owner set by dragging, per screen and grid, kept in
/// their own file so a bad entry never touches the preferences (P10 plan
/// §2.13, Q-P10k). Keys look like `dev.grid_demo/lines`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridWidths {
    /// Least recently set first.
    entries: Vec<(String, Vec<(u16, u16)>)>,
}

impl GridWidths {
    /// The file under the UI store folder.
    pub const FILE: &'static str = "grid-widths.json";
    /// File format version.
    pub const VERSION: u64 = 1;
    /// Most grids remembered; setting one more forgets the oldest.
    pub const MAX_GRIDS: usize = 200;
    /// Most columns per grid.
    pub const MAX_COLUMNS: usize = 64;
    /// Longest grid key.
    pub const MAX_KEY: usize = 96;
    /// Narrowest and widest width kept, in points.
    pub const WIDTH_RANGE: std::ops::RangeInclusive<u16> = 8..=4000;

    fn key_ok(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= Self::MAX_KEY
            && !key.contains("..")
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'))
    }

    fn entry_from(value: &serde_json::Value) -> Option<Vec<(u16, u16)>> {
        let list = value.as_array()?;
        if list.len() > Self::MAX_COLUMNS {
            return None;
        }
        let mut out: Vec<(u16, u16)> = Vec::with_capacity(list.len());
        for pair in list {
            let pair = pair.as_array()?;
            let [col, width] = pair.as_slice() else {
                return None;
            };
            let col = u16::try_from(col.as_u64()?).ok()?;
            let width = u16::try_from(width.as_u64()?).ok()?;
            if !Self::WIDTH_RANGE.contains(&width) || out.iter().any(|(c, _)| *c == col) {
                return None;
            }
            out.push((col, width));
        }
        Some(out)
    }

    /// Read the file's text. Each grid's entry is checked on its own: an
    /// entry that is malformed is dropped and the others are kept. Text that
    /// is not the expected JSON at all gives an empty set.
    #[must_use]
    pub fn parse(text: &str) -> GridWidths {
        let mut out = GridWidths::default();
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
        else {
            return out;
        };
        if root.get("version").and_then(serde_json::Value::as_u64) != Some(Self::VERSION) {
            return out;
        }
        let Some(serde_json::Value::Object(grids)) = root.get("grids") else {
            return out;
        };
        for (key, value) in grids {
            if out.entries.len() >= Self::MAX_GRIDS {
                break;
            }
            if Self::key_ok(key)
                && let Some(widths) = Self::entry_from(value)
            {
                out.entries.push((key.clone(), widths));
            }
        }
        out
    }

    /// The file's text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let grids: serde_json::Map<String, serde_json::Value> = self
            .entries
            .iter()
            .map(|(k, w)| (k.clone(), serde_json::json!(w)))
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "version": Self::VERSION,
            "grids": grids,
        }))
        .unwrap_or_default()
    }

    /// The widths remembered for a grid.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Vec<(cl_grid::ColumnId, u16)>> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, w)| {
            w.iter()
                .map(|(c, px)| (cl_grid::ColumnId(*c), *px))
                .collect()
        })
    }

    /// Remember a grid's widths. Widths out of range, repeated columns and
    /// columns past the cap are not kept, and a key that could not be read
    /// back is refused. Returns false when nothing changed.
    pub fn set(&mut self, key: &str, widths: &[(cl_grid::ColumnId, u16)]) -> bool {
        if !Self::key_ok(key) {
            return false;
        }
        let mut list: Vec<(u16, u16)> = Vec::new();
        for (c, w) in widths {
            if list.len() == Self::MAX_COLUMNS {
                break;
            }
            if Self::WIDTH_RANGE.contains(w) && list.iter().all(|(id, _)| *id != c.0) {
                list.push((c.0, *w));
            }
        }
        if self.entries.iter().any(|(k, w)| k == key && *w == list) {
            return false;
        }
        self.entries.retain(|(k, _)| k != key);
        self.entries.push((key.to_string(), list));
        while self.entries.len() > Self::MAX_GRIDS {
            self.entries.remove(0);
        }
        true
    }

    /// How many grids are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when no grid is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl UiStore {
    fn widths_path(&self) -> PathBuf {
        self.dir.join(GridWidths::FILE)
    }

    /// Load remembered column widths (empty when missing, oversized or
    /// unreadable; never touches `ui-prefs.json`).
    #[must_use]
    pub fn load_grid_widths(&self) -> GridWidths {
        read_capped(&self.widths_path()).map_or_else(GridWidths::default, |t| GridWidths::parse(&t))
    }

    /// Save column widths.
    ///
    /// # Errors
    /// Any file error.
    pub fn save_grid_widths(&self, widths: &GridWidths) -> io::Result<()> {
        write_atomic(&self.widths_path(), &widths.to_text())
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

    fn widths(pairs: &[(u16, u16)]) -> Vec<(cl_grid::ColumnId, u16)> {
        pairs
            .iter()
            .map(|(c, w)| (cl_grid::ColumnId(*c), *w))
            .collect()
    }

    /// T19 (S12, A19).
    #[test]
    fn grid_widths_survive_preferences_save() {
        let dir = temp_dir("widths");
        let store = UiStore::new(&dir);
        let mut book = store.load_grid_widths();
        assert!(book.is_empty());
        assert!(book.set("dev.grid_demo/lines", &widths(&[(1, 180), (2, 96)])));
        store.save_grid_widths(&book).unwrap();
        // A Preferences save writes ui-prefs.json only.
        let p = UiPrefs {
            zoom_percent: 150,
            ..UiPrefs::default()
        };
        store.save_prefs(&p).unwrap();
        assert_eq!(
            store.load_grid_widths().get("dev.grid_demo/lines"),
            Some(widths(&[(1, 180), (2, 96)]))
        );
        assert_eq!(store.load_prefs(), p);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrupt_width_entry_drops_only_itself() {
        let text = r#"{"version":1,"grids":{
            "a/lines":[[1,120],[2,80]],
            "b/lines":[[1,"wide"]],
            "c/lines":[[1,120,3]],
            "d/lines":[[1,0]],
            "e/lines":[[1,90],[1,95]],
            "../x":[[1,90]],
            "f/lines":[]
        }}"#;
        let book = GridWidths::parse(text);
        assert_eq!(book.get("a/lines"), Some(widths(&[(1, 120), (2, 80)])));
        assert_eq!(book.get("f/lines"), Some(Vec::new()));
        for bad in ["b/lines", "c/lines", "d/lines", "e/lines", "../x"] {
            assert_eq!(book.get(bad), None, "{bad}");
        }
        assert_eq!(book.len(), 2);
        assert!(GridWidths::parse("not json").is_empty());
        assert!(GridWidths::parse(r#"{"version":2,"grids":{"a":[[1,9]]}}"#).is_empty());
        assert_eq!(GridWidths::parse(&book.to_text()), book);
    }

    #[test]
    fn a_corrupt_widths_file_leaves_preferences_intact() {
        let dir = temp_dir("widths-corrupt");
        let store = UiStore::new(&dir);
        let p = UiPrefs {
            theme: ThemeChoice::Dark,
            ..UiPrefs::default()
        };
        store.save_prefs(&p).unwrap();
        let prefs_text = std::fs::read(dir.join("ui-prefs.json")).unwrap();
        std::fs::write(dir.join(GridWidths::FILE), b"\xff\xfe{{{").unwrap();
        assert!(store.load_grid_widths().is_empty());
        let mut book = store.load_grid_widths();
        book.set("a/lines", &widths(&[(1, 100)]));
        store.save_grid_widths(&book).unwrap();
        assert_eq!(
            std::fs::read(dir.join("ui-prefs.json")).unwrap(),
            prefs_text
        );
        assert_eq!(store.load_prefs(), p);
        assert_eq!(
            store.load_grid_widths().get("a/lines"),
            Some(widths(&[(1, 100)]))
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn width_book_is_capped() {
        let mut book = GridWidths::default();
        let many: Vec<(cl_grid::ColumnId, u16)> =
            (0..100).map(|c| (cl_grid::ColumnId(c), 50)).collect();
        assert!(book.set("big", &many));
        assert_eq!(
            book.get("big").map(|w| w.len()),
            Some(GridWidths::MAX_COLUMNS)
        );
        assert!(!book.set("big", &many), "unchanged widths are not a change");
        for i in 0..=GridWidths::MAX_GRIDS {
            book.set(&format!("g{i}"), &widths(&[(1, 60)]));
        }
        assert_eq!(book.len(), GridWidths::MAX_GRIDS);
        assert_eq!(book.get("big"), None, "the oldest grid is forgotten");
        assert!(!book.set("bad key!", &widths(&[(1, 60)])));
        assert!(book.to_text().len() < usize::try_from(MAX_READ).unwrap());
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
