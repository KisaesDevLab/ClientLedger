//! Back Up (P08 step 5). "Make Portable Copy" was removed (Q-268):
//! moving a company to another computer is Back Up plus Restore. Older
//! files flagged portable still restore and are never pruned.
//!
//! A `.clbackup` is the engine's encrypted tenant archive byte-for-byte
//! (P02.md §2.5): the app starts an export job with the owner's backup
//! passphrase from the keychain, polls it, moves the finished file from
//! the engine's archive folder to the destination (rename on the same
//! volume, otherwise copy and re-hash), checks the SHA-256 the engine
//! reported, records the file in the backup history, and prunes old files
//! in the destination by the retention rule. Application metadata (app
//! version, kind, name, colour, local flags) goes into `manifest.host`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cl_api::{ArchiveJobStatus, ArchiveRequest, JobId, JobState, Session};
use cl_core::Timestamp;
use cl_core::ids::TenantId;
use cl_settings::{BackupKind, BackupRecord, CompanyFlags, RecentStore, SecretStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CompanyError;
use crate::retention::{KEEP_LAST, KEEP_MONTHS, prune_plan};

/// What the app writes into `manifest.host` (never interpreted by the
/// engine; read back by Restore for the colour and flags).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostMetadata {
    /// Always `ClientLedger`.
    pub app: String,
    /// The app version.
    pub app_version: String,
    /// `backup` or `before-delete` (`portable` in files made before
    /// Q-268; Restore ignores this field).
    pub kind: String,
    /// Company name at export time.
    pub name: String,
    /// Stripe colour `#RRGGBB`.
    pub accent: String,
    /// Local setup flags (Q-P08c).
    pub flags: CompanyFlags,
    /// Company-local preferences that make sense on another computer
    /// (`CompanyPrefs::for_archive`, Q-P09c); absent in P08 archives.
    #[serde(default)]
    pub prefs: cl_settings::CompanyPrefs,
}

impl HostMetadata {
    /// Builds the metadata for a company.
    #[must_use]
    pub fn new(
        kind: BackupKind,
        name: &str,
        accent: cl_settings::Accent,
        flags: CompanyFlags,
        prefs: cl_settings::CompanyPrefs,
    ) -> Self {
        HostMetadata {
            app: "ClientLedger".into(),
            app_version: crate::APP_VERSION.into(),
            kind: match kind {
                BackupKind::Backup => "backup",
                BackupKind::LegacyPortable => "portable",
                BackupKind::BeforeDelete => "before-delete",
            }
            .into(),
            name: name.into(),
            accent: accent.hex(),
            flags,
            prefs,
        }
    }
}

/// Progress of a long company operation, for a progress bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Asking the engine to start.
    Starting,
    /// The engine is working (`percent` 0-100 when known, `step` text).
    Working {
        /// 0-100.
        percent: Option<u8>,
        /// Engine step name.
        step: Option<String>,
    },
    /// Moving the file to its destination.
    Copying,
    /// Hashing the copy.
    Verifying,
    /// Finished.
    Done,
}

/// Where progress goes (the UI stores the latest value for its frame).
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// A progress sink that drops every report.
#[must_use]
pub fn silent() -> ProgressSink {
    Arc::new(|_| {})
}

/// What to back up and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupRequest {
    /// The company.
    pub tenant_id: TenantId,
    /// Regular backup or the one before a delete (never
    /// [`BackupKind::LegacyPortable`], which [`backup`] refuses).
    pub kind: BackupKind,
    /// Destination folder (created when missing).
    pub dest_dir: PathBuf,
    /// Prune old backups of this company in `dest_dir` (regular
    /// backups only).
    pub apply_retention: bool,
}

/// What [`backup`] produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupOutcome {
    /// The record added to the history.
    pub record: BackupRecord,
    /// File size.
    pub bytes: u64,
    /// Older files removed by retention.
    pub pruned: Vec<PathBuf>,
}

/// How often the export job is polled.
pub const POLL_EVERY: Duration = Duration::from_millis(500);
/// How long an export may run.
pub const EXPORT_DEADLINE: Duration = Duration::from_secs(3600);

/// File name for a company's backup: letters, digits and dashes from the
/// name, then the UTC time, then `.clbackup`.
#[must_use]
pub fn backup_file_name(company: &str, at: Timestamp, kind: BackupKind) -> String {
    let stem: String = company
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let stem = if stem.is_empty() {
        "Company".to_string()
    } else {
        stem.chars().take(40).collect()
    };
    let when = at.as_utc().format("%Y%m%d-%H%M%S");
    let suffix = match kind {
        BackupKind::Backup => "",
        BackupKind::LegacyPortable => "-portable",
        BackupKind::BeforeDelete => "-before-delete",
    };
    format!("{stem}-{when}{suffix}.clbackup")
}

/// Lowercase hex SHA-256 of a file (streamed; run off the async worker).
///
/// # Errors
/// Any read error.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The passphrase from the keychain.
pub(crate) fn passphrase(secrets: &dyn SecretStore) -> Result<cl_core::Secret, CompanyError> {
    secrets
        .get(cl_settings::secrets::BACKUP_PASSPHRASE)?
        .filter(|p| !p.is_empty())
        .ok_or(CompanyError::NoPassphrase)
}

/// Polls an archive job (export or import) to its end, reporting the
/// engine's progress and step. A job the engine no longer knows (it
/// restarted) is `JobFailed { JOB_UNKNOWN }`: start again.
///
/// # Errors
/// `JobFailed` with the engine's code, `TIMEOUT` or `JOB_UNKNOWN`; `Api`.
pub async fn wait_for_job(
    session: &Session,
    job: &JobId,
    progress: &ProgressSink,
) -> Result<ArchiveJobStatus, CompanyError> {
    let deadline = tokio::time::Instant::now() + EXPORT_DEADLINE;
    loop {
        let status = match session.tenants().archive_job(job).await {
            Ok(s) => s,
            Err(cl_api::ApiError::NotFound { .. }) => {
                return Err(CompanyError::JobFailed {
                    code: "JOB_UNKNOWN".into(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        match status.status {
            JobState::Succeeded => return Ok(status),
            JobState::Failed => {
                return Err(CompanyError::JobFailed {
                    code: status.error_code(),
                });
            }
            JobState::Queued | JobState::Running => {
                progress(Progress::Working {
                    percent: status.progress,
                    step: status.step,
                });
                if tokio::time::Instant::now() > deadline {
                    return Err(CompanyError::JobFailed {
                        code: "TIMEOUT".into(),
                    });
                }
                tokio::time::sleep(POLL_EVERY).await;
            }
        }
    }
}

/// Moves the finished archive to `dest` and checks its hash. The source is
/// removed only after the copy verified.
pub(crate) async fn place_archive(
    result: &cl_api::ArchiveResult,
    dest: &Path,
    progress: &ProgressSink,
) -> Result<u64, CompanyError> {
    let src = result.path.clone().ok_or(CompanyError::JobFailed {
        code: "NO_PATH".into(),
    })?;
    let expected = result.sha256.to_ascii_lowercase();
    let dest = dest.to_path_buf();
    progress(Progress::Copying);
    let p2 = Arc::clone(progress);
    tokio::task::spawn_blocking(move || -> Result<u64, CompanyError> {
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir).map_err(CompanyError::io("create backup folder"))?;
        }
        let moved = std::fs::rename(&src, &dest).is_ok();
        if !moved {
            std::fs::copy(&src, &dest).map_err(CompanyError::io("copy backup"))?;
        }
        p2(Progress::Verifying);
        let got = sha256_file(&dest).map_err(CompanyError::io("hash backup"))?;
        if got != expected {
            // The bad copy goes; the engine's file stays for another try.
            let _ = std::fs::remove_file(&dest);
            return Err(CompanyError::CopyMismatch);
        }
        if !moved {
            let _ = std::fs::remove_file(&src);
        }
        let bytes = std::fs::metadata(&dest)
            .map(|m| m.len())
            .map_err(CompanyError::io("stat backup"))?;
        Ok(bytes)
    })
    .await
    .map_err(|_| CompanyError::Io {
        what: "copy backup",
        source: std::io::Error::other("worker stopped"),
    })?
}

/// Q-268: the app no longer writes portable copies; that kind exists only
/// so older history entries keep loading.
fn writable(kind: BackupKind) -> Result<(), CompanyError> {
    match kind {
        BackupKind::Backup | BackupKind::BeforeDelete => Ok(()),
        BackupKind::LegacyPortable => Err(CompanyError::Metadata),
    }
}

/// Backs up one company. Works from any session scope.
///
/// # Errors
/// `Metadata` for [`BackupKind::LegacyPortable`] (nothing sent);
/// `NoPassphrase` (nothing sent); `Api` (an engine without
/// `tenant-archive-v1` answers `NotFound`); `ExportFailed`; `Io`;
/// `CopyMismatch`; `Settings`.
pub async fn backup(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &BackupRequest,
    progress: &ProgressSink,
) -> Result<BackupOutcome, CompanyError> {
    writable(req.kind)?;
    let passphrase = passphrase(secrets)?;
    let list = store.load()?;
    let rec = list
        .get(req.tenant_id)
        .cloned()
        .ok_or(CompanyError::UnknownCompany)?;
    let host = HostMetadata::new(
        req.kind,
        &rec.name,
        rec.accent,
        rec.flags,
        rec.prefs.for_archive(),
    );
    progress(Progress::Starting);
    let job = session
        .tenants()
        .export_archive(
            req.tenant_id,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: false,
                omit_document_drafts: false,
                host: serde_json::to_value(&host).map_err(|_| CompanyError::Metadata)?,
                passphrase,
            },
        )
        .await?;
    let result = wait_for_job(session, &job.job_id, progress)
        .await?
        .export_result()
        .ok_or(CompanyError::JobFailed {
            code: "NO_RESULT".into(),
        })?;
    let now = cl_settings::recent::now();
    let dest = req
        .dest_dir
        .join(backup_file_name(&rec.name, now, req.kind));
    let bytes = place_archive(&result, &dest, progress).await?;
    let record = BackupRecord {
        path: dest.clone(),
        sha256: result.sha256.to_ascii_lowercase(),
        archive_id: result.archive_id.clone(),
        created_at: now,
        kind: req.kind,
    };
    let apply = req.apply_retention && req.kind == BackupKind::Backup;
    let dest_dir = req.dest_dir.clone();
    let pruned = store.update(|l| {
        l.record_backup(req.tenant_id, record.clone());
        match (apply, l.get_mut(req.tenant_id)) {
            (true, Some(r)) => prune_company_backups(r, &dest_dir, now),
            _ => Vec::new(),
        }
    })?;
    progress(Progress::Done);
    Ok(BackupOutcome {
        record,
        bytes,
        pruned,
    })
}

/// Applies the retention rule (last 10 plus monthly for 12 months) to a
/// company's **own backup history**: only regular backups recorded under
/// `dir` are candidates (never before-delete copies, never portable copies
/// made before Q-268 removed that command, never a file of another
/// company that happens to share a name stem). Deleted
/// files leave the history; a file already gone is dropped from the
/// history too.
#[must_use]
pub fn prune_company_backups(
    rec: &mut cl_settings::CompanyRecord,
    dir: &Path,
    now: Timestamp,
) -> Vec<PathBuf> {
    let candidates: Vec<(PathBuf, Timestamp)> = rec
        .backups
        .iter()
        .filter(|b| b.kind == BackupKind::Backup && b.path.parent() == Some(dir))
        .map(|b| (b.path.clone(), b.created_at))
        .collect();
    let delete = prune_plan(&candidates, now, KEEP_LAST, KEEP_MONTHS);
    let mut removed = Vec::new();
    for path in delete {
        match std::fs::remove_file(&path) {
            Ok(()) => removed.push(path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => removed.push(path),
            Err(_) => {}
        }
    }
    rec.backups.retain(|b| !removed.contains(&b.path));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_parse_back() {
        let at = Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts");
        let name = backup_file_name(
            "Ozark Ridge Landscaping, LLC / 2026",
            at,
            BackupKind::Backup,
        );
        assert_eq!(
            name,
            "Ozark-Ridge-Landscaping-LLC-2026-20261104-081200.clbackup"
        );
        assert!(backup_file_name("", at, BackupKind::Backup).starts_with("Company-"));
        assert!(
            backup_file_name("x", at, BackupKind::BeforeDelete)
                .ends_with("-before-delete.clbackup")
        );
    }

    #[test]
    fn sha256_and_hex() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"abc").expect("write");
        assert_eq!(
            sha256_file(&p).expect("hash"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn pruning_follows_the_history_not_the_folder() {
        let dir = tempfile::tempdir().expect("tmp");
        let now = Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts");
        let mut rec = cl_settings::CompanyRecord::new(
            cl_core::ids::TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            "Ozark",
            cl_settings::ACCENT_PALETTE[0],
        );
        let add =
            |rec: &mut cl_settings::CompanyRecord, at: Timestamp, kind: BackupKind, sub: &Path| {
                let path = sub.join(backup_file_name("Ozark", at, kind));
                std::fs::write(&path, b"x").expect("write");
                rec.backups.push(BackupRecord {
                    path,
                    sha256: "0".repeat(64),
                    archive_id: at.to_wire(),
                    created_at: at,
                    kind,
                });
            };
        for i in 0..12 {
            let at =
                Timestamp::parse_wire(&format!("2026-10-{:02}T08:00:00.000Z", i + 1)).expect("ts");
            add(&mut rec, at, BackupKind::Backup, dir.path());
        }
        let old = Timestamp::parse_wire("2024-01-01T08:00:00.000Z").expect("ts");
        // A portable copy made before Q-268, older than every backup.
        add(&mut rec, old, BackupKind::LegacyPortable, dir.path());
        // A same-stem file of another company in the folder is not ours.
        std::fs::write(
            dir.path()
                .join(backup_file_name("Ozark", old, BackupKind::Backup)),
            b"x",
        )
        .expect("write");
        // An old backup recorded in another folder is left alone.
        let elsewhere = tempfile::tempdir().expect("tmp");
        add(&mut rec, old, BackupKind::Backup, elsewhere.path());
        let before = rec.backups.len();
        let pruned = prune_company_backups(&mut rec, dir.path(), now);
        assert_eq!(pruned.len(), 2, "{pruned:?}");
        assert_eq!(
            rec.backups.len(),
            before - 2,
            "pruned files leave the history"
        );
        assert!(rec.backups.iter().all(|b| b.path.exists()));
        let left = std::fs::read_dir(dir.path()).expect("dir").count();
        assert_eq!(left, 12, "10 kept + portable + the other company's file");
        assert_eq!(std::fs::read_dir(elsewhere.path()).expect("dir").count(), 1);
        for p in &pruned {
            assert!(
                p.to_string_lossy().contains("Ozark-20261001")
                    || p.to_string_lossy().contains("Ozark-20261002")
            );
        }
    }

    #[test]
    fn portable_copies_are_never_written_q268() {
        assert!(writable(BackupKind::Backup).is_ok());
        assert!(writable(BackupKind::BeforeDelete).is_ok());
        assert!(matches!(
            writable(BackupKind::LegacyPortable),
            Err(CompanyError::Metadata)
        ));
    }

    #[test]
    fn old_portable_copies_are_never_pruned_q268() {
        // Even far past the keep-last and monthly windows, and even when
        // they are the only files in the folder, old portable copies stay.
        let dir = tempfile::tempdir().expect("tmp");
        let now = Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts");
        let mut rec = cl_settings::CompanyRecord::new(
            cl_core::ids::TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            "Ozark",
            cl_settings::ACCENT_PALETTE[0],
        );
        for year in 2010..2030 {
            let at = Timestamp::parse_wire(&format!("{year}-01-01T08:00:00.000Z")).expect("ts");
            let path = dir
                .path()
                .join(backup_file_name("Ozark", at, BackupKind::LegacyPortable));
            std::fs::write(&path, b"x").expect("write");
            rec.backups.push(BackupRecord {
                path,
                sha256: "0".repeat(64),
                archive_id: at.to_wire(),
                created_at: at,
                kind: BackupKind::LegacyPortable,
            });
        }
        let pruned = prune_company_backups(&mut rec, dir.path(), now);
        assert!(pruned.is_empty(), "{pruned:?}");
        assert_eq!(rec.backups.len(), 20);
        assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 20);
    }

    #[test]
    fn host_metadata_round_trips() {
        let h = HostMetadata::new(
            BackupKind::Backup,
            "Shoal Creek Farm",
            cl_settings::ACCENT_PALETTE[1],
            CompanyFlags::default(),
            cl_settings::CompanyPrefs {
                bills_due_days: 14,
                ..cl_settings::CompanyPrefs::default()
            },
        );
        let v = serde_json::to_value(&h).expect("json");
        assert_eq!(v["kind"], "backup");
        assert_eq!(v["prefs"]["bills_due_days"], 14);
        // A P08 archive has no `prefs`: defaults apply.
        let old: HostMetadata = serde_json::from_value(serde_json::json!({
            "app": "ClientLedger", "appVersion": "0.1", "kind": "backup",
            "name": "x", "accent": "#6E8B3D", "flags": {}
        }))
        .expect("old");
        assert_eq!(old.prefs, cl_settings::CompanyPrefs::default());
        assert_eq!(v["accent"], "#6E8B3D");
        let back: HostMetadata = serde_json::from_value(v).expect("back");
        assert_eq!(back, h);
    }
}
