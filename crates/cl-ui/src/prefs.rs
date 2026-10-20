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

/// The register's Show range, Sort by and one-line choice per company and
/// account (P12, Q-186), in their own capped file like [`GridWidths`].
/// Keys are `<company hash>/<account id>`, so no company name or id is
/// written in clear.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegisterPrefsBook {
    /// Least recently set first.
    entries: Vec<(String, cl_model::register::RegisterPrefs)>,
}

impl RegisterPrefsBook {
    /// The file under the UI store folder.
    pub const FILE: &'static str = "register-prefs.json";
    /// File format version.
    pub const VERSION: u64 = 1;
    /// Most registers remembered; one more forgets the oldest.
    pub const MAX_ENTRIES: usize = 300;

    /// The key for `company`'s register of `account`.
    #[must_use]
    pub fn key(company: &str, account: &str) -> String {
        format!("{}/{}", key_hash(company), safe_name(account))
    }

    fn key_ok(key: &str) -> bool {
        GridWidths::key_ok(key)
    }

    /// Read the file's text; a malformed entry drops only itself.
    #[must_use]
    pub fn parse(text: &str) -> RegisterPrefsBook {
        let mut out = RegisterPrefsBook::default();
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
        else {
            return out;
        };
        if root.get("version").and_then(serde_json::Value::as_u64) != Some(Self::VERSION) {
            return out;
        }
        let Some(serde_json::Value::Object(list)) = root.get("registers") else {
            return out;
        };
        for (key, value) in list {
            if out.entries.len() >= Self::MAX_ENTRIES {
                break;
            }
            if Self::key_ok(key)
                && let Ok(p) = serde_json::from_value(value.clone())
            {
                out.entries.push((key.clone(), p));
            }
        }
        out
    }

    /// The file's text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let list: serde_json::Map<String, serde_json::Value> = self
            .entries
            .iter()
            .filter_map(|(k, p)| Some((k.clone(), serde_json::to_value(p).ok()?)))
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "version": Self::VERSION,
            "registers": list,
        }))
        .unwrap_or_default()
    }

    /// The choices remembered for a register.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<cl_model::register::RegisterPrefs> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, p)| *p)
    }

    /// Remember a register's choices. Returns false when nothing changed or
    /// the key could not be read back.
    pub fn set(&mut self, key: &str, prefs: cl_model::register::RegisterPrefs) -> bool {
        if !Self::key_ok(key) || self.get(key) == Some(prefs) {
            return false;
        }
        self.entries.retain(|(k, _)| k != key);
        self.entries.push((key.to_string(), prefs));
        while self.entries.len() > Self::MAX_ENTRIES {
            self.entries.remove(0);
        }
        true
    }

    /// How many registers are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when none is remembered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl UiStore {
    fn register_prefs_path(&self) -> PathBuf {
        self.dir.join(RegisterPrefsBook::FILE)
    }

    /// Load the remembered register choices (empty when missing, oversized
    /// or unreadable).
    #[must_use]
    pub fn load_register_prefs(&self) -> RegisterPrefsBook {
        read_capped(&self.register_prefs_path())
            .map_or_else(RegisterPrefsBook::default, |t| RegisterPrefsBook::parse(&t))
    }

    /// Save the register choices.
    ///
    /// # Errors
    /// Any file error.
    pub fn save_register_prefs(&self, book: &RegisterPrefsBook) -> io::Result<()> {
        write_atomic(&self.register_prefs_path(), &book.to_text())
    }
}

/// How often the owner accepted the same suggestion (Q-141): per company,
/// payee, account and source (`rule`, `ai`). Enough repeats offer a local
/// rule. Ids and counts only (no names or amounts); capped, in its own
/// file next to the company list, so a bad entry drops only itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SuggestionAccepts {
    /// Least recently counted first.
    entries: Vec<(String, u32)>,
}

impl SuggestionAccepts {
    /// The file in the settings folder.
    pub const FILE: &'static str = "suggestion-accepts.json";
    /// File format version.
    pub const VERSION: u64 = 1;
    /// Most entries kept; one more forgets the oldest.
    pub const MAX_ENTRIES: usize = 500;
    /// Longest key.
    pub const MAX_KEY: usize = 160;

    /// The key for one (company, payee, account, source).
    #[must_use]
    pub fn key(company: &str, payee: &str, account: &str, source: &str) -> String {
        format!(
            "{}/{}/{}/{}",
            key_hash(company),
            safe_name(payee),
            safe_name(account),
            safe_name(source)
        )
    }

    fn key_ok(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= Self::MAX_KEY
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/'))
    }

    /// Read the file's text; a malformed entry drops only itself.
    #[must_use]
    pub fn parse(text: &str) -> SuggestionAccepts {
        let mut out = SuggestionAccepts::default();
        let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(text)
        else {
            return out;
        };
        if root.get("version").and_then(serde_json::Value::as_u64) != Some(Self::VERSION) {
            return out;
        }
        // A list of `[key, count]`, least recently counted first.
        let Some(serde_json::Value::Array(list)) = root.get("accepts") else {
            return out;
        };
        for pair in list {
            if out.entries.len() >= Self::MAX_ENTRIES {
                break;
            }
            let Some([key, value]) = pair.as_array().map(Vec::as_slice) else {
                continue;
            };
            if let Some(key) = key.as_str().filter(|k| Self::key_ok(k))
                && let Some(n) = value.as_u64().and_then(|n| u32::try_from(n).ok())
                && !out.entries.iter().any(|(k, _)| k == key)
            {
                out.entries.push((key.to_string(), n));
            }
        }
        out
    }

    /// The file's text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let list: Vec<serde_json::Value> = self
            .entries
            .iter()
            .map(|(k, n)| serde_json::json!([k, n]))
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "version": Self::VERSION,
            "accepts": list,
        }))
        .unwrap_or_default()
    }

    /// How often `key` was accepted.
    #[must_use]
    pub fn count(&self, key: &str) -> u32 {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map_or(0, |(_, n)| *n)
    }

    /// Counts one more accept; returns the new count (0 for a bad key).
    pub fn bump(&mut self, key: &str) -> u32 {
        if !Self::key_ok(key) {
            return 0;
        }
        let n = self.count(key).saturating_add(1);
        self.entries.retain(|(k, _)| k != key);
        self.entries.push((key.to_string(), n));
        while self.entries.len() > Self::MAX_ENTRIES {
            self.entries.remove(0);
        }
        n
    }

    /// How many entries are kept.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True when none is kept.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Load from `dir` (empty when missing, oversized or unreadable).
    #[must_use]
    pub fn load(dir: &Path) -> SuggestionAccepts {
        read_capped(&dir.join(Self::FILE))
            .map_or_else(SuggestionAccepts::default, |t| SuggestionAccepts::parse(&t))
    }

    /// Save into `dir`.
    ///
    /// # Errors
    /// Any file error.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        write_atomic(&dir.join(Self::FILE), &self.to_text())
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
    fn register_prefs_round_trip_capped_and_keyed_by_hash() {
        use cl_model::register::{RangePreset, RegisterPrefs, SortChoice};
        let dir = temp_dir("register-prefs");
        let store = UiStore::new(&dir);
        assert!(store.load_register_prefs().is_empty());
        let key = RegisterPrefsBook::key("tenant-1", "acct-1");
        assert!(!key.contains("tenant"), "{key}");
        let p = RegisterPrefs {
            range: RangePreset::AllDates,
            sort: SortChoice::Number,
            one_line: true,
        };
        let mut book = RegisterPrefsBook::default();
        assert!(book.set(&key, p));
        assert!(!book.set(&key, p), "unchanged choices are not a change");
        assert!(!book.set("../x", p));
        store.save_register_prefs(&book).unwrap();
        assert_eq!(store.load_register_prefs().get(&key), Some(p));
        // Preferences proper are untouched by it.
        assert!(!dir.join("ui-prefs.json").exists());
        for i in 0..=RegisterPrefsBook::MAX_ENTRIES {
            book.set(&RegisterPrefsBook::key("t", &format!("a{i}")), p);
        }
        assert_eq!(book.len(), RegisterPrefsBook::MAX_ENTRIES);
        assert_eq!(book.get(&key), None, "the oldest register is forgotten");
        assert!(book.to_text().len() < usize::try_from(MAX_READ).unwrap());
        assert!(RegisterPrefsBook::parse("not json").is_empty());
        let bad = format!(
            r#"{{"version":1,"registers":{{"{key}":{{"v":1,"one_line":"yes"}},"ok/a":{{}}}}}}"#
        );
        let parsed = RegisterPrefsBook::parse(&bad);
        assert_eq!(parsed.get(&key), None);
        assert_eq!(parsed.get("ok/a"), Some(RegisterPrefs::default()));
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

    #[test]
    fn suggestion_accepts_count_round_trip_and_stay_capped() {
        let mut book = SuggestionAccepts::default();
        let key = SuggestionAccepts::key(
            "d0000000-0000-4000-8000-000000000001",
            "d0000000-0000-4000-8000-c00000000040",
            "d0000000-0000-4000-8000-a00000005110",
            "ai",
        );
        assert_eq!(book.bump(&key), 1);
        assert_eq!(book.bump(&key), 2);
        assert_eq!(book.bump("../bad key"), 0, "a bad key is not kept");
        let back = SuggestionAccepts::parse(&book.to_text());
        assert_eq!(back.count(&key), 2);
        assert!(
            !book
                .to_text()
                .contains("d0000000-0000-4000-8000-000000000001")
        );
        for n in 0..(SuggestionAccepts::MAX_ENTRIES + 10) {
            book.bump(&format!("k/{n}"));
        }
        assert_eq!(book.len(), SuggestionAccepts::MAX_ENTRIES);
        assert_eq!(book.count(&key), 0, "the oldest is forgotten");
        assert!(SuggestionAccepts::parse("{\"version\":9}").is_empty());
        let dir = temp_dir("accepts");
        book.save(&dir).expect("save");
        assert_eq!(SuggestionAccepts::load(&dir), book);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
