//! Back Up and Portable copy (P08 step 5).
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

use cl_api::{ArchiveRequest, ArchiveResult, JobState, Session};
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
    /// `backup`, `portable` or `before-delete`.
    pub kind: String,
    /// Company name at export time.
    pub name: String,
    /// Stripe colour `#RRGGBB`.
    pub accent: String,
    /// Local setup flags (Q-P08c).
    pub flags: CompanyFlags,
}

impl HostMetadata {
    /// Builds the metadata for a company.
    #[must_use]
    pub fn new(
        kind: BackupKind,
        name: &str,
        accent: cl_settings::Accent,
        flags: CompanyFlags,
    ) -> Self {
        HostMetadata {
            app: "ClientLedger".into(),
            app_version: crate::APP_VERSION.into(),
            kind: match kind {
                BackupKind::Backup => "backup",
                BackupKind::Portable => "portable",
                BackupKind::BeforeDelete => "before-delete",
            }
            .into(),
            name: name.into(),
            accent: accent.hex(),
            flags,
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
    /// Regular backup, portable copy, or the one before a delete.
    pub kind: BackupKind,
    /// Destination folder (created when missing).
    pub dest_dir: PathBuf,
    /// Prune old backups of this company in `dest_dir` (never for a
    /// portable copy).
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
        BackupKind::Portable => "-portable",
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

/// Polls an export job to its end.
pub(crate) async fn wait_for_export(
    session: &Session,
    job: &cl_api::JobId,
    progress: &ProgressSink,
) -> Result<ArchiveResult, CompanyError> {
    let deadline = tokio::time::Instant::now() + EXPORT_DEADLINE;
    loop {
        let status = match session.tenants().archive_job(job).await {
            Ok(s) => s,
            Err(cl_api::ApiError::NotFound { .. }) => {
                return Err(CompanyError::ExportFailed {
                    code: "JOB_UNKNOWN".into(),
                });
            }
            Err(e) => return Err(e.into()),
        };
        match status.status {
            JobState::Done => {
                return status.result.ok_or(CompanyError::ExportFailed {
                    code: "NO_RESULT".into(),
                });
            }
            JobState::Failed => {
                return Err(CompanyError::ExportFailed {
                    code: status.code.unwrap_or_else(|| "FAILED".into()),
                });
            }
            JobState::Queued | JobState::Running => {
                progress(Progress::Working {
                    percent: status.progress,
                    step: status.step,
                });
                if tokio::time::Instant::now() > deadline {
                    return Err(CompanyError::ExportFailed {
                        code: "TIMEOUT".into(),
                    });
                }
                tokio::time::sleep(POLL_EVERY).await;
            }
        }
    }
}

/// Moves the finished archive to `dest` and checks its hash.
pub(crate) async fn place_archive(
    result: &ArchiveResult,
    dest: &Path,
    progress: &ProgressSink,
) -> Result<u64, CompanyError> {
    let src = result.path.clone().ok_or(CompanyError::ExportFailed {
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
        if std::fs::rename(&src, &dest).is_err() {
            std::fs::copy(&src, &dest).map_err(CompanyError::io("copy backup"))?;
            let _ = std::fs::remove_file(&src);
        }
        p2(Progress::Verifying);
        let got = sha256_file(&dest).map_err(CompanyError::io("hash backup"))?;
        if got != expected {
            let _ = std::fs::remove_file(&dest);
            return Err(CompanyError::CopyMismatch);
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

/// Backs up one company. Works from any session scope.
///
/// # Errors
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
    let passphrase = passphrase(secrets)?;
    let list = store.load()?;
    let rec = list
        .get(req.tenant_id)
        .cloned()
        .ok_or(CompanyError::UnknownCompany)?;
    let host = HostMetadata::new(req.kind, &rec.name, rec.accent, rec.flags);
    progress(Progress::Starting);
    let job = session
        .tenants()
        .export_archive(
            req.tenant_id,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                host: serde_json::to_value(&host).map_err(|_| CompanyError::Metadata)?,
                passphrase,
            },
        )
        .await?;
    let result = wait_for_export(session, &job.job_id, progress).await?;
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
    store.update(|l| l.record_backup(req.tenant_id, record.clone()))?;
    let pruned = if req.apply_retention && req.kind == BackupKind::Backup {
        prune_company_backups(&req.dest_dir, &rec.name, now)
    } else {
        Vec::new()
    };
    progress(Progress::Done);
    Ok(BackupOutcome {
        record,
        bytes,
        pruned,
    })
}

/// Deletes older `<company>-*.clbackup` files in `dir` by the retention
/// rule (last 10 plus monthly for 12 months). Portable and before-delete
/// copies are never pruned.
#[must_use]
pub fn prune_company_backups(dir: &Path, company: &str, now: Timestamp) -> Vec<PathBuf> {
    let prefix = backup_file_name(company, now, BackupKind::Backup);
    let Some(prefix) = prefix
        .rsplit_once('-')
        .and_then(|(a, _)| a.rsplit_once('-').map(|(p, _)| p.to_string()))
    else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with(&format!("{prefix}-")) || !name.ends_with(".clbackup") {
            continue;
        }
        if name.contains("-portable.") || name.contains("-before-delete.") {
            continue;
        }
        let Some(at) = parse_stamp(name, &prefix) else {
            continue;
        };
        files.push((path, at));
    }
    let delete = prune_plan(&files, now, KEEP_LAST, KEEP_MONTHS);
    delete
        .into_iter()
        .filter(|p| std::fs::remove_file(p).is_ok())
        .collect()
}

fn parse_stamp(name: &str, prefix: &str) -> Option<Timestamp> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('-')?;
    let stamp = rest.get(..15)?;
    let (date, time) = stamp.split_once('-')?;
    if date.len() != 8 || time.len() != 6 {
        return None;
    }
    Timestamp::parse_wire(&format!(
        "{}-{}-{}T{}:{}:{}.000Z",
        &date[..4],
        &date[4..6],
        &date[6..8],
        &time[..2],
        &time[2..4],
        &time[4..6]
    ))
    .ok()
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
        assert_eq!(
            parse_stamp(&name, "Ozark-Ridge-Landscaping-LLC-2026"),
            Some(at)
        );
        assert!(backup_file_name("", at, BackupKind::Portable).starts_with("Company-"));
        assert!(backup_file_name("x", at, BackupKind::Portable).ends_with("-portable.clbackup"));
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
    fn pruning_touches_only_this_companys_regular_backups() {
        let dir = tempfile::tempdir().expect("tmp");
        let now = Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts");
        for i in 0..12 {
            let at =
                Timestamp::parse_wire(&format!("2026-10-{:02}T08:00:00.000Z", i + 1)).expect("ts");
            std::fs::write(
                dir.path()
                    .join(backup_file_name("Ozark", at, BackupKind::Backup)),
                b"x",
            )
            .expect("write");
        }
        let old = Timestamp::parse_wire("2024-01-01T08:00:00.000Z").expect("ts");
        std::fs::write(
            dir.path()
                .join(backup_file_name("Ozark", old, BackupKind::Portable)),
            b"x",
        )
        .expect("write");
        std::fs::write(
            dir.path()
                .join(backup_file_name("Other", old, BackupKind::Backup)),
            b"x",
        )
        .expect("write");
        let pruned = prune_company_backups(dir.path(), "Ozark", now);
        assert_eq!(pruned.len(), 2, "{pruned:?}");
        let left = std::fs::read_dir(dir.path()).expect("dir").count();
        assert_eq!(left, 12);
        for p in &pruned {
            assert!(
                p.to_string_lossy().contains("Ozark-20261001")
                    || p.to_string_lossy().contains("Ozark-20261002")
            );
        }
    }

    #[test]
    fn host_metadata_round_trips() {
        let h = HostMetadata::new(
            BackupKind::Portable,
            "Shoal Creek Farm",
            cl_settings::ACCENT_PALETTE[1],
            CompanyFlags::default(),
        );
        let v = serde_json::to_value(&h).expect("json");
        assert_eq!(v["kind"], "portable");
        assert_eq!(v["accent"], "#6E8B3D");
        let back: HostMetadata = serde_json::from_value(v).expect("back");
        assert_eq!(back, h);
    }
}
