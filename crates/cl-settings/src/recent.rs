//! The recent-companies store (Q-P08a, Q-P08c).
//!
//! One JSON file, `companies.json`, in the app's settings folder. It holds
//! what the Open Company window shows beyond the engine's tenant list and
//! what the engine cannot store yet: accent colour, last opened and last
//! backed up, the backup history, and the local setup flags (fiscal
//! calendar, sales tax, inventory) that wait for U-25. It never holds
//! amounts, secrets or transaction data; the transaction count shown on
//! the Open Company card is a plain count the engine reported.

use std::path::{Path, PathBuf};

use cl_core::Timestamp;
use cl_core::ids::TenantId;
use serde::{Deserialize, Serialize};

use crate::store::{read_capped, write_atomic};

/// An accent colour (Q-184), as RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Accent(pub [u8; 3]);

impl Accent {
    /// `#RRGGBB`.
    #[must_use]
    pub fn hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0[0], self.0[1], self.0[2])
    }

    /// Parses `#RRGGBB` or `RRGGBB`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Accent> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Accent([
            u8::try_from((v >> 16) & 0xff).ok()?,
            u8::try_from((v >> 8) & 0xff).ok()?,
            u8::try_from(v & 0xff).ok()?,
        ]))
    }
}

/// The eight stripe colours offered at setup, all original (not from any
/// other product) and readable as a 6 px stripe on light and dark chrome.
/// The first three are the ones the mockups use.
pub const ACCENT_PALETTE: [Accent; 8] = [
    Accent([0x3E, 0x7C, 0xB1]), // river blue
    Accent([0x6E, 0x8B, 0x3D]), // field green
    Accent([0x8A, 0x5A, 0x9E]), // plum
    Accent([0xB5, 0x65, 0x1D]), // clay
    Accent([0x0F, 0x5E, 0x5A]), // teal (the app accent)
    Accent([0xC2, 0x3B, 0x4A]), // brick
    Accent([0x5B, 0x6B, 0x7A]), // slate
    Accent([0xC9, 0xA2, 0x27]), // wheat
];

/// The least-used palette colour among `used`, so companies made in turn
/// get different stripes (Q-184).
#[must_use]
pub fn next_accent(used: &[Accent]) -> Accent {
    ACCENT_PALETTE
        .iter()
        .copied()
        .min_by_key(|c| used.iter().filter(|u| *u == c).count())
        .unwrap_or(ACCENT_PALETTE[0])
}

/// Fiscal calendar choice kept locally until U-25 (Q-P08c).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiscalCalendar {
    /// Calendar months.
    #[default]
    Standard,
    /// 4-4-5 weeks per quarter.
    Retail445,
    /// 4-5-4.
    Retail454,
    /// 5-4-4.
    Retail544,
}

/// Setup flags the engine does not store yet (U-25).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanyFlags {
    /// Standard months or a retail calendar.
    pub fiscal_calendar: FiscalCalendar,
    /// Sales tax features on.
    pub sales_tax_enabled: bool,
    /// Inventory features on.
    pub inventory_enabled: bool,
}

/// What a backup file was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupKind {
    /// A regular backup.
    Backup,
    /// A "portable copy" made before Q-268 removed that command. Read
    /// only: the app never writes this kind any more, but older history
    /// entries still load, and retention never deletes them (Q-268).
    #[serde(rename = "portable")]
    LegacyPortable,
    /// The backup taken before a delete or a replace-with-restore.
    BeforeDelete,
}

/// One backup file this app wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupRecord {
    /// Where the file was written.
    pub path: PathBuf,
    /// SHA-256 of the file, lowercase hex.
    pub sha256: String,
    /// The engine's archive id (also in the manifest).
    pub archive_id: String,
    /// When.
    pub created_at: Timestamp,
    /// Why.
    pub kind: BackupKind,
}

/// One company as the Open Company window knows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompanyRecord {
    /// The engine tenant.
    pub tenant_id: TenantId,
    /// Display name as last seen.
    pub name: String,
    /// Stripe colour.
    pub accent: Accent,
    /// Last opened on this computer.
    #[serde(default)]
    pub last_opened: Option<Timestamp>,
    /// Last successful backup.
    #[serde(default)]
    pub last_backup: Option<Timestamp>,
    /// Backups, newest last (capped at [`BACKUP_HISTORY`]).
    #[serde(default)]
    pub backups: Vec<BackupRecord>,
    /// Local setup flags.
    #[serde(default)]
    pub flags: CompanyFlags,
    /// Company-local preferences (P09).
    #[serde(default)]
    pub prefs: crate::CompanyPrefs,
    /// Transactions the engine reported when last opened (a count only).
    #[serde(default)]
    pub transaction_count: Option<u64>,
    /// Where the books came from, when not typed in (`qbd` after P31).
    #[serde(default)]
    pub source: Option<String>,
    /// The practice company (reset any time, never backed up by schedule).
    #[serde(default)]
    pub is_sample: bool,
    /// Deleted through the safe delete: the record stays as a tombstone
    /// with the before-delete backup and the engine's receipt (Q-185).
    #[serde(default)]
    pub deleted_at: Option<Timestamp>,
    /// The engine's delete receipt (ids and times only).
    #[serde(default)]
    pub delete_receipt: Option<serde_json::Value>,
}

/// Backups remembered per company.
pub const BACKUP_HISTORY: usize = 200;

impl CompanyRecord {
    /// A fresh record.
    #[must_use]
    pub fn new(tenant_id: TenantId, name: impl Into<String>, accent: Accent) -> CompanyRecord {
        CompanyRecord {
            tenant_id,
            name: name.into(),
            accent,
            last_opened: None,
            last_backup: None,
            backups: Vec::new(),
            flags: CompanyFlags::default(),
            prefs: crate::CompanyPrefs::default(),
            transaction_count: None,
            source: None,
            is_sample: false,
            deleted_at: None,
            delete_receipt: None,
        }
    }

    /// True for a deleted company kept as a tombstone.
    #[must_use]
    pub fn is_tombstone(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// The backup with this SHA-256, if the app wrote it.
    #[must_use]
    pub fn backup_by_sha256(&self, sha256: &str) -> Option<&BackupRecord> {
        self.backups
            .iter()
            .rev()
            .find(|b| b.sha256.eq_ignore_ascii_case(sha256))
    }

    /// The backup with this archive id, if the app wrote it.
    #[must_use]
    pub fn backup_by_archive_id(&self, archive_id: &str) -> Option<&BackupRecord> {
        self.backups
            .iter()
            .rev()
            .find(|b| b.archive_id == archive_id)
    }
}

/// The file's content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecentCompanies {
    /// Format version.
    pub version: u32,
    /// Companies, most recently opened first.
    pub companies: Vec<CompanyRecord>,
    /// The company to reopen at start, when the owner chose one.
    pub last_open: Option<TenantId>,
    /// Where backups go by default.
    pub backup_folder: Option<PathBuf>,
    /// Back up when a company is closed (Q-217 family).
    pub backup_on_close: bool,
}

/// Format version written today.
pub const FORMAT_VERSION: u32 = 1;

impl RecentCompanies {
    /// Parses the file text; a damaged file yields an empty list (the
    /// engine's tenant list is the source of truth for what exists).
    #[must_use]
    pub fn parse(text: &str) -> RecentCompanies {
        match serde_json::from_str::<RecentCompanies>(text) {
            Ok(v) if v.version <= FORMAT_VERSION => v,
            Ok(_) => {
                tracing::warn!("companies.json is from a newer ClientLedger; ignoring it");
                RecentCompanies::default()
            }
            Err(_) => {
                tracing::warn!("companies.json could not be read; starting with an empty list");
                RecentCompanies::default()
            }
        }
    }

    /// JSON text.
    #[must_use]
    pub fn to_text(&self) -> String {
        let mut v = self.clone();
        v.version = FORMAT_VERSION;
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".into())
    }

    /// A company by tenant id.
    #[must_use]
    pub fn get(&self, id: TenantId) -> Option<&CompanyRecord> {
        self.companies.iter().find(|c| c.tenant_id == id)
    }

    /// A company by tenant id, mutable.
    pub fn get_mut(&mut self, id: TenantId) -> Option<&mut CompanyRecord> {
        self.companies.iter_mut().find(|c| c.tenant_id == id)
    }

    /// Inserts or updates the name of a company (keeps everything else).
    pub fn upsert(&mut self, id: TenantId, name: &str) -> &mut CompanyRecord {
        if let Some(i) = self.companies.iter().position(|c| c.tenant_id == id) {
            self.companies[i].name = name.to_string();
            &mut self.companies[i]
        } else {
            let accent = next_accent(&self.companies.iter().map(|c| c.accent).collect::<Vec<_>>());
            self.companies.push(CompanyRecord::new(id, name, accent));
            let last = self.companies.len() - 1;
            &mut self.companies[last]
        }
    }

    /// Marks a company as opened now and moves it to the front.
    pub fn touch_opened(&mut self, id: TenantId, now: Timestamp) {
        if let Some(i) = self.companies.iter().position(|c| c.tenant_id == id) {
            let mut c = self.companies.remove(i);
            c.last_opened = Some(now);
            self.companies.insert(0, c);
            self.last_open = Some(id);
        }
    }

    /// Records a backup the app wrote.
    pub fn record_backup(&mut self, id: TenantId, backup: BackupRecord) {
        if let Some(c) = self.get_mut(id) {
            c.last_backup = Some(backup.created_at);
            c.backups.push(backup);
            if c.backups.len() > BACKUP_HISTORY {
                let drop = c.backups.len() - BACKUP_HISTORY;
                c.backups.drain(..drop);
            }
        }
    }

    /// Forgets a company (after a delete).
    pub fn remove(&mut self, id: TenantId) -> Option<CompanyRecord> {
        let i = self.companies.iter().position(|c| c.tenant_id == id)?;
        if self.last_open == Some(id) {
            self.last_open = None;
        }
        Some(self.companies.remove(i))
    }

    /// Marks a company deleted: the record stays with its backup history,
    /// the delete time and the engine's receipt, and leaves every listing.
    pub fn tombstone(&mut self, id: TenantId, at: Timestamp, receipt: serde_json::Value) {
        if self.last_open == Some(id) {
            self.last_open = None;
        }
        if let Some(c) = self.get_mut(id) {
            c.deleted_at = Some(at);
            c.delete_receipt = Some(receipt);
        }
    }

    /// Companies that exist (not tombstones), in file order.
    pub fn live(&self) -> impl Iterator<Item = &CompanyRecord> {
        self.companies.iter().filter(|c| !c.is_tombstone())
    }

    /// Drops companies the engine no longer lists (keeps the practice
    /// company record only while it exists in the engine too). Tombstones
    /// stay.
    pub fn retain_existing(&mut self, existing: &[TenantId]) {
        self.companies
            .retain(|c| c.is_tombstone() || existing.contains(&c.tenant_id));
        if self.last_open.is_some_and(|l| !existing.contains(&l)) {
            self.last_open = None;
        }
    }

    /// The backup that produced a file with this hash, across companies.
    #[must_use]
    pub fn find_backup_by_sha256(&self, sha256: &str) -> Option<(&CompanyRecord, &BackupRecord)> {
        self.companies
            .iter()
            .find_map(|c| c.backup_by_sha256(sha256).map(|b| (c, b)))
    }
}

/// Errors of the store file.
#[derive(Debug, thiserror::Error)]
pub enum RecentError {
    /// Reading or writing the file failed.
    #[error("the companies list could not be {action}: {source}")]
    Io {
        /// `read` or `saved`.
        action: &'static str,
        /// The error.
        #[source]
        source: std::io::Error,
    },
}

/// The file on disk.
#[derive(Debug, Clone)]
pub struct RecentStore {
    path: PathBuf,
}

impl RecentStore {
    /// `<settings dir>/companies.json`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> RecentStore {
        RecentStore {
            path: dir.join("companies.json"),
        }
    }

    /// The file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the list (empty when the file is missing or unreadable).
    ///
    /// # Errors
    /// `Io` when the file exists but cannot be read.
    pub fn load(&self) -> Result<RecentCompanies, RecentError> {
        let text = read_capped(&self.path).map_err(|source| RecentError::Io {
            action: "read",
            source,
        })?;
        Ok(text.map(|t| RecentCompanies::parse(&t)).unwrap_or_default())
    }

    /// Saves the list atomically.
    ///
    /// # Errors
    /// `Io` when the file cannot be written.
    pub fn save(&self, list: &RecentCompanies) -> Result<(), RecentError> {
        write_atomic(&self.path, &list.to_text()).map_err(|source| RecentError::Io {
            action: "saved",
            source,
        })
    }

    /// Loads, applies `f`, saves.
    ///
    /// # Errors
    /// `Io` from either side.
    pub fn update<T>(&self, f: impl FnOnce(&mut RecentCompanies) -> T) -> Result<T, RecentError> {
        let mut list = self.load()?;
        let out = f(&mut list);
        self.save(&list)?;
        Ok(out)
    }
}

/// The current time as a [`Timestamp`] (the only clock in this crate).
#[must_use]
pub fn now() -> Timestamp {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp::from_unix_millis(ms).unwrap_or_else(|_| {
        Timestamp::from_unix_millis(0).unwrap_or_else(|_| unreachable!("epoch is in range"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> TenantId {
        TenantId::parse(&format!("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_millis(ms).expect("ts")
    }

    #[test]
    fn accents_round_trip_and_rotate() {
        assert_eq!(Accent::parse("#3E7CB1"), Some(ACCENT_PALETTE[0]));
        assert_eq!(ACCENT_PALETTE[0].hex(), "#3E7CB1");
        assert_eq!(Accent::parse("nope"), None);
        let a = next_accent(&[]);
        let b = next_accent(&[a]);
        let c = next_accent(&[a, b]);
        assert!(a != b && b != c && a != c);
        let all: Vec<Accent> = ACCENT_PALETTE.to_vec();
        assert_eq!(next_accent(&all), ACCENT_PALETTE[0], "wraps around");
    }

    #[test]
    fn upsert_touch_backup_remove_and_text_round_trip() {
        let mut list = RecentCompanies::default();
        list.upsert(id(1), "Ozark Ridge");
        list.upsert(id(2), "Shoal Creek");
        list.upsert(id(1), "Ozark Ridge Landscaping LLC");
        assert_eq!(list.companies.len(), 2);
        assert_eq!(list.companies[0].name, "Ozark Ridge Landscaping LLC");
        assert_ne!(list.companies[0].accent, list.companies[1].accent);
        list.touch_opened(id(2), ts(1_000));
        assert_eq!(list.companies[0].tenant_id, id(2));
        assert_eq!(list.last_open, Some(id(2)));
        list.record_backup(
            id(2),
            BackupRecord {
                path: PathBuf::from("b.clbackup"),
                sha256: "AB".repeat(32),
                archive_id: "a-1".into(),
                created_at: ts(2_000),
                kind: BackupKind::Backup,
            },
        );
        assert_eq!(list.get(id(2)).expect("c").last_backup, Some(ts(2_000)));
        let (c, b) = list
            .find_backup_by_sha256(&"ab".repeat(32))
            .expect("found case-insensitively");
        assert_eq!(c.tenant_id, id(2));
        assert_eq!(b.archive_id, "a-1");
        let text = list.to_text();
        let back = RecentCompanies::parse(&text);
        assert_eq!(back, {
            let mut l = list.clone();
            l.version = FORMAT_VERSION;
            l
        });
        assert!(list.remove(id(2)).is_some());
        assert_eq!(list.last_open, None);
        list.retain_existing(&[]);
        assert_eq!(list.companies.len(), 0);
        // A tombstone survives retain_existing and leaves `live()`.
        list.upsert(id(3), "Gone Co");
        list.touch_opened(id(3), ts(5));
        list.tombstone(id(3), ts(9), serde_json::json!({"archiveId": "a-3"}));
        assert_eq!(list.last_open, None);
        list.retain_existing(&[]);
        assert_eq!(list.companies.len(), 1);
        assert_eq!(list.live().count(), 0);
        assert!(list.get(id(3)).expect("kept").is_tombstone());
    }

    #[test]
    fn damaged_or_newer_files_start_empty() {
        assert_eq!(RecentCompanies::parse("not json").companies.len(), 0);
        assert_eq!(
            RecentCompanies::parse("{\"version\": 99, \"companies\": []}")
                .companies
                .len(),
            0
        );
        assert_eq!(RecentCompanies::parse("{}"), RecentCompanies::default());
    }

    #[test]
    fn old_portable_history_entries_still_load_q268() {
        // History written before Q-268 removed "Make Portable Copy".
        let old: BackupRecord = serde_json::from_value(serde_json::json!({
            "path": "Ozark-20240101-080000-portable.clbackup",
            "sha256": "0".repeat(64),
            "archive_id": "a-old",
            "created_at": "2024-01-01T08:00:00.000Z",
            "kind": "portable"
        }))
        .expect("old portable record loads");
        assert_eq!(old.kind, BackupKind::LegacyPortable);
        // Saving the list again keeps the old flag as written.
        let v = serde_json::to_value(&old).expect("json");
        assert_eq!(v["kind"], "portable");
    }

    #[test]
    fn backup_history_is_capped() {
        let mut list = RecentCompanies::default();
        list.upsert(id(1), "x");
        for i in 0..(BACKUP_HISTORY + 5) {
            list.record_backup(
                id(1),
                BackupRecord {
                    path: PathBuf::from(format!("{i}.clbackup")),
                    sha256: format!("{i:064x}"),
                    archive_id: i.to_string(),
                    created_at: ts(i64::try_from(i).expect("i")),
                    kind: BackupKind::Backup,
                },
            );
        }
        let c = list.get(id(1)).expect("c");
        assert_eq!(c.backups.len(), BACKUP_HISTORY);
        assert_eq!(c.backups.last().expect("last").archive_id, "204");
    }

    #[test]
    fn store_round_trip_in_a_folder() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = RecentStore::in_dir(&dir.path().join("settings"));
        assert_eq!(store.load().expect("empty").companies.len(), 0);
        store
            .update(|l| {
                l.upsert(id(3), "Kurt - Rental Duplex")
                    .flags
                    .sales_tax_enabled = true;
            })
            .expect("update");
        let l = store.load().expect("load");
        assert!(l.get(id(3)).expect("c").flags.sales_tax_enabled);
        assert!(now().unix_millis() > 1_700_000_000_000);
    }
}
