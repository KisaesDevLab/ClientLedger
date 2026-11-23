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

use std::collections::BTreeSet;
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
use crate::retention::{KEEP_BEFORE_IMPORT, KEEP_LAST, KEEP_MONTHS, prune_plan};

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
    /// The newest transfer files sent to the accountant (P32 "Sent
    /// before"), so a restore keeps them; absent before P32.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transfers: Vec<cl_settings::TransferRecord>,
}

/// How many transfer records a backup carries in `manifest.host` (the
/// engine caps it at 16 KiB).
pub const HOST_TRANSFERS: usize = 20;
/// The most the transfer records may take in `manifest.host`, serialized
/// (review S4): the rest of the 16 KiB host block stays free for the
/// name, flags and preferences.
pub const HOST_TRANSFERS_MAX_BYTES: usize = 12 * 1024;

/// The "Sent before" records a backup carries (review S4): the newest
/// [`HOST_TRANSFERS`], each with its file name only (no folder: the
/// backup may travel to another computer, and `for_archive` keeps no
/// paths), dropping the oldest until they fit in
/// [`HOST_TRANSFERS_MAX_BYTES`].
#[must_use]
pub fn host_transfers(records: &[cl_settings::TransferRecord]) -> Vec<cl_settings::TransferRecord> {
    let skip = records.len().saturating_sub(HOST_TRANSFERS);
    let mut out: Vec<cl_settings::TransferRecord> = records[skip..]
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r.path = r.path.file_name().map(PathBuf::from).unwrap_or_default();
            r
        })
        .collect();
    while !out.is_empty()
        && serde_json::to_vec(&out).map_or(usize::MAX, |b| b.len()) > HOST_TRANSFERS_MAX_BYTES
    {
        out.remove(0);
    }
    out
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
                BackupKind::BeforeImport => "before-import",
            }
            .into(),
            name: name.into(),
            accent: accent.hex(),
            flags,
            prefs,
            transfers: Vec::new(),
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
    /// Prune old backups of this company and kind in `dest_dir`: regular
    /// backups by [`prune_company_backups`], before-import ones by
    /// [`prune_before_import`]; before-delete copies never.
    pub apply_retention: bool,
    /// The before-import backups an import still needs (file names: the
    /// backup a committed, running or failed import batch's "Restore the
    /// backup taken before the import" points at,
    /// `cl_model::imports::history::protected_backups`). Before-import
    /// retention never deletes them, and runs only when this is known:
    /// `None` (the import list could not be read) prunes nothing. Other
    /// kinds ignore it.
    pub protected: Option<BTreeSet<String>>,
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
        BackupKind::BeforeImport => "-before-import",
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
    let status = poll_job(session, job, progress).await?;
    match status.status {
        JobState::Failed => {
            let code = status.error_code();
            // The engine's code only (P33 QA-10: an unlisted code reads
            // "try again" to the owner, so the log must name it).
            tracing::warn!(code = %code, "archive job failed");
            Err(CompanyError::JobFailed { code })
        }
        _ => Ok(status),
    }
}

/// Polls an archive job until it succeeds or fails and returns its last
/// status either way (a failure keeps the engine's `details`).
///
/// # Errors
/// `JobFailed` with `TIMEOUT` or `JOB_UNKNOWN`; `Api`.
pub async fn poll_job(
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
            JobState::Succeeded | JobState::Failed => return Ok(status),
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

/// One attachment file an export could not read (Q-P32k, QA-1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingAttachment {
    /// The attachment id.
    pub attachment_id: String,
    /// Its file name (owner data: shown, never logged).
    pub file_name: Option<String>,
    /// The kind of record it belongs to (`bill`, `transaction`, …).
    pub record_type: Option<String>,
    /// That record's id (for Open record).
    pub record_id: Option<String>,
    /// `FILE_MISSING`, `FILE_SIZE_MISMATCH` or `EXTERNAL_FILE`.
    pub reason: String,
}

/// Each missing file with its name and record, as far as the engine can
/// say (an attachment it can't find keeps only its id).
pub(crate) async fn resolve_missing(
    session: &Session,
    files: Vec<cl_api::MissingFile>,
) -> Vec<MissingAttachment> {
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let found = match cl_api::AttachmentId::parse(&f.row_id) {
            Ok(id) => session.attachments().get(id).await.ok(),
            Err(_) => None,
        };
        out.push(MissingAttachment {
            attachment_id: f.row_id,
            file_name: found.as_ref().map(|a| a.file_name.clone()),
            record_type: found.as_ref().map(|a| a.attachable_type.clone()),
            record_id: found.map(|a| a.attachable_id),
            reason: f.reason,
        });
    }
    out
}

/// The error for a failed backup job: the engine's list of missing or
/// changed attachment files when it gave one (QA-1: desktop backups stop
/// on `FILE_MISSING` / `FILE_SIZE_MISMATCH` and list every file), else
/// `JobFailed` with its code.
pub(crate) async fn job_failure(session: &Session, status: &ArchiveJobStatus) -> CompanyError {
    let code = status.error_code();
    tracing::warn!(code = %code, "backup: export failed");
    if let Some(err) = status.error.as_ref() {
        let files = err.missing_files();
        if !files.is_empty() {
            return CompanyError::MissingFiles(resolve_missing(session, files).await);
        }
    }
    CompanyError::JobFailed { code }
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
        BackupKind::Backup | BackupKind::BeforeDelete | BackupKind::BeforeImport => Ok(()),
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
    let mut host = HostMetadata::new(
        req.kind,
        &rec.name,
        rec.accent,
        rec.flags,
        rec.prefs.for_archive(),
    );
    host.transfers = host_transfers(&rec.transfers);
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
                transfer: false,
                as_of: None,
                host: serde_json::to_value(&host).map_err(|_| CompanyError::Metadata)?,
                passphrase,
            },
        )
        .await?;
    let status = poll_job(session, &job.job_id, progress).await?;
    if status.status == JobState::Failed {
        return Err(job_failure(session, &status).await);
    }
    let result = status.export_result().ok_or(CompanyError::JobFailed {
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
        trigger: None,
        tested: None,
    };
    let dest_dir = req.dest_dir.clone();
    let pruned = store.update(|l| {
        l.record_backup(req.tenant_id, record.clone());
        l.get_mut(req.tenant_id)
            .map(|r| retain(r, req, &dest_dir, now))
            .unwrap_or_default()
    })?;
    progress(Progress::Done);
    Ok(BackupOutcome {
        record,
        bytes,
        pruned,
    })
}

/// The retention a backup asked for: regular backups by
/// [`prune_company_backups`]; before-import ones by [`prune_before_import`]
/// only when the backups imports still need are known; before-delete and
/// old portable copies never.
fn retain(
    rec: &mut cl_settings::CompanyRecord,
    req: &BackupRequest,
    dir: &Path,
    now: Timestamp,
) -> Vec<PathBuf> {
    match (req.apply_retention, req.kind, req.protected.as_ref()) {
        (true, BackupKind::Backup, _) => prune_company_backups(rec, dir, now),
        (true, BackupKind::BeforeImport, Some(keep)) => prune_before_import(rec, dir, now, keep),
        _ => Vec::new(),
    }
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
    prune_kind(
        rec,
        dir,
        now,
        BackupKind::Backup,
        KEEP_LAST,
        KEEP_MONTHS,
        &BTreeSet::new(),
    )
}

/// Retention for the automatic backups taken before an import posts
/// (Q-P30h): the newest [`KEEP_BEFORE_IMPORT`] before-import files of this
/// company recorded under `dir`; no monthly copies. Regular, before-delete
/// and portable files are never candidates, nor is any file named in
/// `protected` (an import batch still points at it), which also takes no
/// place among the newest kept.
#[must_use]
pub fn prune_before_import(
    rec: &mut cl_settings::CompanyRecord,
    dir: &Path,
    now: Timestamp,
    protected: &BTreeSet<String>,
) -> Vec<PathBuf> {
    prune_kind(
        rec,
        dir,
        now,
        BackupKind::BeforeImport,
        KEEP_BEFORE_IMPORT,
        0,
        protected,
    )
}

fn prune_kind(
    rec: &mut cl_settings::CompanyRecord,
    dir: &Path,
    now: Timestamp,
    kind: BackupKind,
    keep_last: usize,
    keep_months: u32,
    protected: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let is_protected = |p: &Path| {
        p.file_name()
            .is_some_and(|n| protected.contains(n.to_string_lossy().as_ref()))
    };
    let candidates: Vec<(PathBuf, Timestamp)> = rec
        .backups
        .iter()
        .filter(|b| b.kind == kind && b.path.parent() == Some(dir) && !is_protected(&b.path))
        .map(|b| (b.path.clone(), b.created_at))
        .collect();
    let delete = prune_plan(&candidates, now, keep_last, keep_months);
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

    /// Review S4: the worst case (20 records, long non-ASCII file names in
    /// deep folders, the largest amounts) stays under 12 KiB of transfers
    /// and the whole host block under the engine's 16 KiB, with no folder.
    #[test]
    fn host_transfers_fit_the_host_limit_and_carry_no_folder() {
        let big = cl_core::Money::parse_wire("-99999999999999.9999").expect("money");
        let deep: PathBuf = std::iter::repeat_n("Ünïcödé-földer-名前-📁", 12).collect();
        let name = format!("{}.clxfer", "Ünïcödé 名前 📁 ".repeat(15));
        let rec = |i: usize| cl_settings::TransferRecord {
            path: Path::new("C:\\Users\\owner").join(&deep).join(&name),
            sha256: "f".repeat(64),
            archive_id: format!("{i:036}"),
            created_at: Timestamp::parse_wire("2026-11-04T08:12:00.000Z").expect("ts"),
            totals: Some(cl_settings::TransferTotals {
                as_of: cl_core::CalendarDate::from_ymd(2026, 11, 4).expect("d"),
                total_assets: big,
                total_liabilities: big,
                total_equity: big,
                net_income: big,
                transactions: u64::MAX,
                attachments: u64::MAX,
            }),
        };
        let all: Vec<_> = (0..40).map(rec).collect();
        let kept = host_transfers(&all);
        assert!(!kept.is_empty() && kept.len() <= HOST_TRANSFERS);
        let bytes = serde_json::to_vec(&kept).expect("json").len();
        assert!(bytes <= HOST_TRANSFERS_MAX_BYTES, "{bytes} bytes");
        // The newest are kept, in order, with the file name only.
        assert_eq!(
            kept.last().map(|r| r.archive_id.clone()),
            Some(format!("{:036}", 39))
        );
        for r in &kept {
            assert_eq!(r.path, PathBuf::from(&name));
            assert!(!serde_json::to_string(r).expect("json").contains("owner"));
        }
        // The whole host block, with every preference, fits the engine's cap.
        let mut host = HostMetadata::new(
            BackupKind::Backup,
            &"N".repeat(255),
            cl_settings::ACCENT_PALETTE[0],
            CompanyFlags::default(),
            cl_settings::CompanyPrefs::default(),
        );
        host.transfers = kept;
        let all = serde_json::to_vec(&host).expect("json").len();
        assert!(all <= cl_api::HOST_METADATA_MAX_BYTES, "{all} bytes");
        // A short history is kept whole.
        let short: Vec<_> = (0..3).map(rec).collect();
        assert_eq!(host_transfers(&short).len(), 3);
        assert_eq!(host_transfers(&[]), Vec::new());
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
                    trigger: None,
                    tested: None,
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

    /// Q-P30h: before-import backups keep only their newest 10 per
    /// company and folder; regular and before-delete files are untouched,
    /// and regular retention never deletes a before-import file.
    #[test]
    fn before_import_backups_keep_the_newest_ten_q_p30h() {
        let dir = tempfile::tempdir().expect("tmp");
        let now = Timestamp::parse_wire("2026-11-17T08:12:00.000Z").expect("ts");
        let mut rec = cl_settings::CompanyRecord::new(
            cl_core::ids::TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            "Ozark",
            cl_settings::ACCENT_PALETTE[0],
        );
        let add = |rec: &mut cl_settings::CompanyRecord, at: Timestamp, kind: BackupKind| {
            let path = dir.path().join(backup_file_name("Ozark", at, kind));
            std::fs::write(&path, b"x").expect("write");
            rec.backups.push(BackupRecord {
                path,
                sha256: "0".repeat(64),
                archive_id: at.to_wire(),
                created_at: at,
                kind,
                trigger: None,
                tested: None,
            });
        };
        // 14 before-import files, one a month apart in 2025 and 2026 (a
        // monthly rule would keep some of the old ones; this one doesn't).
        for i in 0..14u32 {
            let (y, m) = if i < 10 { (2026, i + 1) } else { (2025, i - 9) };
            let at = Timestamp::parse_wire(&format!("{y}-{m:02}-05T08:00:00.000Z")).expect("ts");
            add(&mut rec, at, BackupKind::BeforeImport);
        }
        let old = Timestamp::parse_wire("2020-01-01T08:00:00.000Z").expect("ts");
        add(&mut rec, old, BackupKind::Backup);
        add(&mut rec, old, BackupKind::BeforeDelete);
        assert!(
            prune_company_backups(&mut rec, dir.path(), now).is_empty(),
            "regular retention never touches before-import files (one regular file)"
        );
        let pruned = prune_before_import(&mut rec, dir.path(), now, &BTreeSet::new());
        assert_eq!(pruned.len(), 4, "{pruned:?}");
        assert!(
            pruned
                .iter()
                .all(|p| p.to_string_lossy().contains("Ozark-2025")),
            "the four oldest go: {pruned:?}"
        );
        let kept_imports = rec
            .backups
            .iter()
            .filter(|b| b.kind == BackupKind::BeforeImport)
            .count();
        assert_eq!(kept_imports, KEEP_BEFORE_IMPORT);
        assert_eq!(rec.backups.len(), 12, "10 + regular + before-delete");
        assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 12);
        assert!(
            backup_file_name("x", now, BackupKind::BeforeImport)
                .ends_with("-before-import.clbackup")
        );
        let h = HostMetadata::new(
            BackupKind::BeforeImport,
            "x",
            cl_settings::ACCENT_PALETTE[0],
            CompanyFlags::default(),
            cl_settings::CompanyPrefs::default(),
        );
        assert_eq!(h.kind, "before-import");
    }

    /// P31: the backup a committed import batch's restore points at is
    /// never pruned, however many newer before-import backups (payroll
    /// posts count too) there are; an unprotected old one still goes.
    #[test]
    fn a_backup_an_import_still_needs_is_never_pruned() {
        let dir = tempfile::tempdir().expect("tmp");
        let now = Timestamp::parse_wire("2026-11-18T08:00:00.000Z").expect("ts");
        let mut rec = cl_settings::CompanyRecord::new(
            cl_core::ids::TenantId::parse("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"),
            "Ozark",
            cl_settings::ACCENT_PALETTE[0],
        );
        let mut names = Vec::new();
        // 12 before-import files, the oldest first (2026-01-01 .. 01-12).
        for d in 1..=12u32 {
            let at = Timestamp::parse_wire(&format!("2026-01-{d:02}T08:00:00.000Z")).expect("ts");
            let name = backup_file_name("Ozark", at, BackupKind::BeforeImport);
            let path = dir.path().join(&name);
            std::fs::write(&path, b"x").expect("write");
            rec.backups.push(BackupRecord {
                path,
                sha256: "0".repeat(64),
                archive_id: at.to_wire(),
                created_at: at,
                kind: BackupKind::BeforeImport,
                trigger: None,
                tested: None,
            });
            names.push(name);
        }
        // The oldest is the one a committed QBD conversion points at.
        let keep: BTreeSet<String> = [names[0].clone()].into();
        let pruned = prune_before_import(&mut rec, dir.path(), now, &keep);
        assert_eq!(pruned.len(), 1, "{pruned:?}");
        assert!(
            pruned[0].ends_with(&names[1]),
            "the oldest unprotected one goes: {pruned:?}"
        );
        assert!(
            dir.path().join(&names[0]).is_file(),
            "the protected file stays"
        );
        assert!(rec.backups.iter().any(|b| b.path.ends_with(&names[0])));
        // Ten newer ones besides it are still kept.
        assert_eq!(rec.backups.len(), KEEP_BEFORE_IMPORT + 1);
        // Two more: with the import list unknown nothing is pruned at all.
        for d in 13..=14u32 {
            let at = Timestamp::parse_wire(&format!("2026-01-{d:02}T08:00:00.000Z")).expect("ts");
            let path = dir
                .path()
                .join(backup_file_name("Ozark", at, BackupKind::BeforeImport));
            std::fs::write(&path, b"x").expect("write");
            rec.backups.push(BackupRecord {
                path,
                sha256: "0".repeat(64),
                archive_id: at.to_wire(),
                created_at: at,
                kind: BackupKind::BeforeImport,
                trigger: None,
                tested: None,
            });
        }
        let mut req = BackupRequest {
            tenant_id: rec.tenant_id,
            kind: BackupKind::BeforeImport,
            dest_dir: dir.path().to_path_buf(),
            apply_retention: true,
            protected: None,
        };
        assert!(
            retain(&mut rec, &req, dir.path(), now).is_empty(),
            "unknown: keep all"
        );
        req.protected = Some(keep);
        assert_eq!(retain(&mut rec, &req, dir.path(), now).len(), 2);
        assert!(dir.path().join(&names[0]).is_file());
    }

    #[test]
    fn portable_copies_are_never_written_q268() {
        assert!(writable(BackupKind::Backup).is_ok());
        assert!(writable(BackupKind::BeforeDelete).is_ok());
        assert!(writable(BackupKind::BeforeImport).is_ok());
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
                trigger: None,
                tested: None,
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
