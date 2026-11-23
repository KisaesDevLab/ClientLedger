//! Send to My Accountant (P32): a `.clxfer` transfer file for the
//! accountant's `MyBooks` Online.
//!
//! A transfer file is the engine's tenant archive written in transfer mode
//! (U-284): users travel as names only, secrets and sign-ins are left out,
//! and the manifest carries verification totals (U-283) that the Online
//! import recomputes and compares. The app adds nothing of its own beyond
//! four fields of `manifest.host` ([`TransferHost`]).
//!
//! Order (plan D3, never changed):
//! 1. the engine must advertise `tenant-archive-transfer-v1` and the
//!    passphrase must pass its rules ([`check_passphrase`], Q-P32f) —
//!    nothing is sent otherwise;
//! 2. a verified **backup** (non-negotiable 9);
//! 3. unposted payroll imports are **discarded** when the owner agreed
//!    (Q-P30c; the backup still holds them);
//! 4. the **export** with `transfer`, attachments, audit trail, both omit
//!    flags and the owner's local date;
//! 5. the result must echo `options.transfer: true` (S3), else the
//!    engine's file is removed and nothing is placed;
//! 6. the file goes to `<dest>.partial`, is hash-checked, then renamed;
//! 7. it is recorded in "Sent before".
//!
//! The passphrase is never stored or logged; [`Secret`] zeroizes it.

use std::path::{Path, PathBuf};

use cl_api::{
    ApiError, ArchiveRequest, Capabilities, ConflictReason, JobState, Session, VerificationTotals,
};
use cl_core::ids::TenantId;
use cl_core::{CalendarDate, Secret};
use cl_settings::{RecentStore, SecretStore, TransferRecord, TransferTotals};
use serde::Serialize;

use crate::backup::{BackupRequest, Progress, ProgressSink};
use crate::error::CompanyError;

/// The file extension.
pub const EXTENSION: &str = crate::restore::TRANSFER_EXTENSION;
/// The shortest transfer passphrase (characters).
pub const MIN_PASSPHRASE_CHARS: usize = 12;
/// The letters a generated passphrase uses: no `I`, `O`, `0` or `1`
/// (32 symbols, 5 bits each).
pub const PASSPHRASE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Exactly what a transfer file's `manifest.host` holds (secrets scan
/// (c)): no colour, flags, preferences, paths or history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferHost {
    /// Always `ClientLedger`.
    pub app: String,
    /// The app version.
    pub app_version: String,
    /// Always `transfer`.
    pub kind: String,
    /// Company name.
    pub name: String,
}

impl TransferHost {
    /// The host metadata for a company.
    #[must_use]
    pub fn new(name: &str) -> TransferHost {
        TransferHost {
            app: "ClientLedger".into(),
            app_version: crate::APP_VERSION.into(),
            kind: "transfer".into(),
            name: name.into(),
        }
    }
}

/// What the Send screen shows before a transfer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Unposted payroll imports (offered for discarding, Q-P30c).
    pub unposted_payroll: u64,
    /// Documents still waiting in Review Documents (left out).
    pub waiting_documents: u64,
    /// The engine can write transfer files.
    pub capability_ok: bool,
}

pub use crate::backup::MissingAttachment;

/// Why a transfer didn't happen.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The engine can't write transfer files (nothing sent).
    #[error("needs engine capability tenant-archive-transfer-v1")]
    Unsupported,
    /// Shorter than [`MIN_PASSPHRASE_CHARS`] (nothing sent).
    #[error("passphrase too short")]
    PassphraseTooShort,
    /// The two entries differ (nothing sent).
    #[error("passphrase mismatch")]
    PassphraseMismatch,
    /// The same as the backup passphrase, which opens every backup
    /// (Q-P32f; nothing sent).
    #[error("passphrase equals the backup passphrase")]
    SameAsBackupPassphrase,
    /// Unposted payroll imports exist and the owner did not agree to
    /// discard them (nothing sent).
    #[error("unposted payroll imports")]
    PayrollDraftsWaiting(u64),
    /// Another backup, transfer or restore is running (409 `TENANT_BUSY`).
    #[error("busy")]
    Busy,
    /// Attachment files are missing or changed; every one is listed.
    #[error("{} attachment files can't be read", .0.len())]
    MissingFiles(Vec<MissingAttachment>),
    /// The engine wrote an ordinary backup instead of a transfer file
    /// (S3). What became of the engine's file is in `file`.
    #[error("the engine did not write a transfer file")]
    NotHonoured {
        /// The engine's file: removed, left (the owner should delete it:
        /// it holds sign-ins), or not named by the engine at all.
        file: EngineFile,
    },
    /// Any error after unposted payroll imports were discarded (review
    /// S7): `source` is what went wrong; the screen also says the imports
    /// are gone and that the backup taken first still holds them.
    #[error("{source} (after discarding payroll imports)")]
    AfterDiscard {
        /// What was discarded and which backup holds it.
        discarded: Discarded,
        /// The error itself.
        source: Box<TransferError>,
    },
    /// Anything else.
    #[error(transparent)]
    Company(#[from] CompanyError),
}

/// What became of the engine's file when it wrote a full archive (S3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineFile {
    /// It was removed.
    Removed,
    /// It could not be removed; this is where it is.
    Left(PathBuf),
    /// The engine's result named no file, so whether one exists is
    /// unknown (review M1).
    Unknown,
}

/// Unposted payroll imports discarded before an error (review S7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discarded {
    /// How many.
    pub count: u64,
    /// When the backup that still holds them was made.
    pub backup_at: cl_core::Timestamp,
}

impl TransferError {
    /// The error itself, under any [`TransferError::AfterDiscard`].
    #[must_use]
    pub fn kind(&self) -> &TransferError {
        match self {
            TransferError::AfterDiscard { source, .. } => source.kind(),
            other => other,
        }
    }

    /// The payroll imports discarded before this error, if any.
    #[must_use]
    pub fn discarded(&self) -> Option<&Discarded> {
        match self {
            TransferError::AfterDiscard { discarded, .. } => Some(discarded),
            _ => None,
        }
    }
}

impl From<ApiError> for TransferError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Conflict {
                reason: ConflictReason::Busy,
                ..
            } => TransferError::Busy,
            other => TransferError::Company(CompanyError::Api(other)),
        }
    }
}

impl cl_core::OwnerFacing for TransferError {
    fn owner_message(&self) -> cl_core::OwnerMessage {
        use cl_core::OwnerMessage as M;
        match self {
            TransferError::Unsupported => {
                M::with_fix("error.transfer.unsupported", "fix.company.unavailable")
            }
            TransferError::PassphraseTooShort => M::with_fix(
                "error.transfer.passphrase_short",
                "fix.transfer.passphrase_short",
            ),
            // QA-11: say which pair (the backup passphrase may be asked
            // on the same screen).
            TransferError::PassphraseMismatch => M::with_fix(
                "error.transfer.passphrase_mismatch",
                "fix.company.passphrase_mismatch",
            ),
            TransferError::SameAsBackupPassphrase => M::with_fix(
                "error.transfer.same_as_backup",
                "fix.transfer.same_as_backup",
            ),
            TransferError::PayrollDraftsWaiting(_) => M::with_fix(
                "error.transfer.payroll_waiting",
                "fix.transfer.payroll_waiting",
            ),
            TransferError::Busy => M::with_fix("error.company.busy", "fix.company.busy"),
            TransferError::MissingFiles(_) => {
                M::with_fix("error.transfer.missing_files", "fix.transfer.missing_files")
            }
            TransferError::NotHonoured {
                file: EngineFile::Removed,
            } => M::with_fix("error.transfer.not_honoured", "fix.transfer.not_honoured"),
            TransferError::NotHonoured { .. } => M::with_fix(
                "error.transfer.not_honoured",
                "fix.transfer.not_honoured_left",
            ),
            TransferError::AfterDiscard { source, .. } => source.owner_message(),
            TransferError::Company(e) => e.owner_message(),
        }
    }

    fn scrubbed_detail(&self) -> String {
        match self {
            TransferError::MissingFiles(f) => format!("missing files: {}", f.len()),
            TransferError::NotHonoured { file } => format!(
                "transfer not honoured; engine file: {}",
                match file {
                    EngineFile::Removed => "removed",
                    EngineFile::Left(_) => "left",
                    EngineFile::Unknown => "unknown",
                }
            ),
            TransferError::AfterDiscard { discarded, source } => format!(
                "{}; payroll drafts discarded: {}",
                source.scrubbed_detail(),
                discarded.count
            ),
            TransferError::PayrollDraftsWaiting(n) => format!("payroll drafts: {n}"),
            TransferError::Company(e) => e.scrubbed_detail(),
            other => format!("{other}"),
        }
    }
}

/// Checks a new transfer passphrase (Q-P32f): at least
/// [`MIN_PASSPHRASE_CHARS`] characters, both entries equal, and never the
/// keychain's backup passphrase.
///
/// # Errors
/// `PassphraseTooShort`, `PassphraseMismatch`, `SameAsBackupPassphrase`;
/// `Company(Secrets)` when the keychain can't be read.
pub fn check_passphrase(
    passphrase: &Secret,
    again: &Secret,
    secrets: &dyn SecretStore,
) -> Result<(), TransferError> {
    if passphrase.expose().chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(TransferError::PassphraseTooShort);
    }
    if passphrase.expose() != again.expose() {
        return Err(TransferError::PassphraseMismatch);
    }
    let backup = secrets
        .get(cl_settings::secrets::BACKUP_PASSPHRASE)
        .map_err(CompanyError::from)?;
    if backup.is_some_and(|b| b.expose() == passphrase.expose()) {
        return Err(TransferError::SameAsBackupPassphrase);
    }
    Ok(())
}

/// "Make one for me": 5 groups of 4 characters from
/// [`PASSPHRASE_ALPHABET`] (100 bits from the OS random source), joined by
/// dashes.
///
/// # Errors
/// `Company(Io)` when the OS random source fails.
pub fn generate_passphrase() -> Result<Secret, TransferError> {
    let mut bytes = zeroize::Zeroizing::new([0u8; 20]);
    getrandom::fill(bytes.as_mut()).map_err(|_| CompanyError::Io {
        what: "random passphrase",
        source: std::io::Error::other("random source"),
    })?;
    let mut s = zeroize::Zeroizing::new(String::with_capacity(24));
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 4 == 0 {
            s.push('-');
        }
        // 256 is a multiple of 32: every symbol is equally likely.
        s.push(char::from(PASSPHRASE_ALPHABET[usize::from(b & 31)]));
    }
    Ok(Secret::new(s.as_str()))
}

/// The file name the Save dialog suggests: `<company>-<local date>.clxfer`.
#[must_use]
pub fn suggested_file_name(company: &str, today: CalendarDate) -> String {
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
    format!("{stem}-{}.{EXTENSION}", today.to_wire())
}

/// What the Send screen needs before a transfer: drafts to offer for
/// discarding, documents left out, and whether the engine can do it.
///
/// # Errors
/// `Api` other than a missing feature (which counts as none).
pub async fn preflight(session: &Session) -> Result<Preflight, TransferError> {
    let capability_ok = session
        .client()
        .capabilities()
        .has(Capabilities::TENANT_ARCHIVE_TRANSFER);
    let unposted_payroll = unposted_payroll(session).await?.len() as u64;
    let waiting_documents = match session
        .document_drafts()
        .list(&cl_api::DraftListQuery {
            statuses: vec![
                cl_api::DraftStatus::New,
                cl_api::DraftStatus::Ready,
                cl_api::DraftStatus::Failed,
            ],
            kind: None,
            limit: 1,
            offset: 0,
            summary: true,
        })
        .await
    {
        Ok(p) => p.total,
        Err(ApiError::Local(_) | ApiError::NotFound { .. }) => 0,
        Err(e) => return Err(e.into()),
    };
    Ok(Preflight {
        unposted_payroll,
        waiting_documents,
        capability_ok,
    })
}

async fn unposted_payroll(
    session: &Session,
) -> Result<Vec<cl_api::payroll::PayrollSessionId>, TransferError> {
    match session.payroll().sessions_all(None).await {
        Ok(page) => Ok(page
            .sessions
            .into_iter()
            .filter(|s| s.status.is_unposted_draft())
            .map(|s| s.id)
            .collect()),
        // An engine without payroll import has no drafts.
        Err(ApiError::Local(_) | ApiError::NotFound { .. }) => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

/// What to send and where.
pub struct TransferRequest {
    /// The company (the open one).
    pub tenant_id: TenantId,
    /// The `.clxfer` file to write (the Save dialog confirmed a replace).
    pub dest: PathBuf,
    /// The new transfer passphrase.
    pub passphrase: Secret,
    /// Typed again.
    pub passphrase_again: Secret,
    /// The owner agreed to discard unposted payroll imports.
    pub discard_payroll: bool,
    /// Where the backup taken first goes.
    pub backup_dir: PathBuf,
    /// The owner's local date (the totals' as-of date).
    pub as_of: CalendarDate,
}

/// Never shows the passphrase or the paths.
impl std::fmt::Debug for TransferRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferRequest")
            .field("tenant_id", &self.tenant_id)
            .field("discard_payroll", &self.discard_payroll)
            .finish_non_exhaustive()
    }
}

/// What [`make_transfer`] produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferOutcome {
    /// The record added to "Sent before".
    pub record: TransferRecord,
    /// The totals in the file (Q-P32l: authoritative).
    pub totals: Option<VerificationTotals>,
    /// File size.
    pub bytes: u64,
    /// Payroll imports discarded first.
    pub discarded_payroll: u64,
}

/// The headline totals kept in the history.
#[must_use]
pub fn history_totals(t: &VerificationTotals) -> TransferTotals {
    TransferTotals {
        as_of: t.as_of,
        total_assets: t.total_assets,
        total_liabilities: t.total_liabilities,
        total_equity: t.total_equity,
        net_income: t.net_income.amount,
        transactions: t.counts.transactions,
        attachments: t.counts.attachments,
    }
}

/// `<dest>.partial`.
#[must_use]
pub fn partial_path(dest: &Path) -> PathBuf {
    crate::iif::fetch::partial_path(dest)
}

/// Makes a transfer file. See the module docs for the order.
///
/// # Errors
/// `Unsupported`, the passphrase errors and `PayrollDraftsWaiting` with
/// nothing sent; `Busy`; `MissingFiles`; `NotHonoured`; `Company` for the
/// backup, the job, the copy and the history.
pub async fn make_transfer(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &TransferRequest,
    progress: &ProgressSink,
) -> Result<TransferOutcome, TransferError> {
    let _gate = crate::backup::GATE.lock().await;
    // 1. Fail closed before anything is sent.
    if !session
        .client()
        .capabilities()
        .has(Capabilities::TENANT_ARCHIVE_TRANSFER)
    {
        return Err(TransferError::Unsupported);
    }
    check_passphrase(&req.passphrase, &req.passphrase_again, secrets)?;
    let drafts = unposted_payroll(session).await?;
    if !drafts.is_empty() && !req.discard_payroll {
        return Err(TransferError::PayrollDraftsWaiting(drafts.len() as u64));
    }
    let list = store.load().map_err(CompanyError::from)?;
    let rec = list
        .get(req.tenant_id)
        .cloned()
        .ok_or(CompanyError::UnknownCompany)?;
    // QA-12: a folder that can't be written fails now, not after the
    // backup and the whole export.
    check_writable(&req.dest)?;

    // 2. Backup first.
    tracing::info!("transfer: backup first");
    let backup = crate::backup::backup_ungated(
        session,
        store,
        secrets,
        &BackupRequest {
            tenant_id: req.tenant_id,
            kind: cl_settings::BackupKind::Backup,
            dest_dir: req.backup_dir.clone(),
            apply_retention: true,
            protected: None,
        },
        &crate::backup::BackupOptions::default(),
        progress,
    )
    .await
    .map_err(|e| match e {
        CompanyError::Api(a) => TransferError::from(a),
        // QA-1: the backup taken first finds the missing files; the same
        // list as the transfer's own check, with Open record.
        CompanyError::MissingFiles(f) => TransferError::MissingFiles(f),
        other => TransferError::Company(other),
    })?;

    // 3.–7. Every error from here on says what was discarded (S7).
    let mut discarded = 0u64;
    let done = after_backup(session, store, req, &rec, &drafts, &mut discarded, progress).await;
    done.map_err(|e| {
        if discarded == 0 {
            e
        } else {
            TransferError::AfterDiscard {
                discarded: Discarded {
                    count: discarded,
                    backup_at: backup.record.created_at,
                },
                source: Box::new(e),
            }
        }
    })
}

/// Writes and removes `<dest>.partial` (the name the file is placed
/// through), so a folder that is missing or can't be written is found
/// before anything is sent (QA-12).
fn check_writable(dest: &Path) -> Result<(), CompanyError> {
    let probe = partial_path(dest);
    // As placing the file does later: a missing folder is made.
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir).map_err(CompanyError::io("check transfer folder"))?;
    }
    let made = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(CompanyError::io("check transfer folder"));
    match made {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        // A `.partial` left by an earlier run: the folder can be written.
        Err(CompanyError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Steps 3 to 7: discard, export, check, place, record. `discarded`
/// counts the payroll imports gone so far, for the caller's error.
#[allow(clippy::too_many_lines, reason = "one linear sequence of steps")]
async fn after_backup(
    session: &Session,
    store: &RecentStore,
    req: &TransferRequest,
    rec: &cl_settings::CompanyRecord,
    drafts: &[cl_api::payroll::PayrollSessionId],
    discarded: &mut u64,
    progress: &ProgressSink,
) -> Result<TransferOutcome, TransferError> {
    // 3. Discard unposted payroll imports (the backup holds them).
    for id in drafts {
        match session.payroll().discard(*id).await {
            Ok(()) => *discarded += 1,
            Err(ApiError::NotFound { .. }) => {}
            Err(e) => return Err(e.into()),
        }
    }
    tracing::info!(discarded = drafts.len(), "transfer: payroll drafts");

    // 4. Export in transfer mode.
    progress(Progress::Starting);
    let job = session
        .tenants()
        .export_archive(
            req.tenant_id,
            &ArchiveRequest {
                include_attachments: true,
                include_audit: true,
                omit_intake_settings: session
                    .client()
                    .capabilities()
                    .has(Capabilities::INTAKE_SETTINGS),
                omit_document_drafts: session
                    .client()
                    .capabilities()
                    .has(Capabilities::DOCUMENT_DRAFTS),
                transfer: true,
                as_of: Some(req.as_of),
                host: serde_json::to_value(TransferHost::new(&rec.name))
                    .map_err(|_| CompanyError::Metadata)?,
                passphrase: req.passphrase.clone(),
            },
        )
        .await?;
    let status = crate::backup::poll_job(session, &job.job_id, progress).await?;
    if status.status == JobState::Failed {
        let code = status.error_code();
        tracing::warn!(code = %code, "transfer: export failed");
        if code == "TENANT_BUSY" {
            return Err(TransferError::Busy);
        }
        if let Some(err) = status.error.as_ref()
            && !err.missing_files().is_empty()
        {
            return Err(TransferError::MissingFiles(
                crate::backup::resolve_missing(session, err.missing_files()).await,
            ));
        }
        return Err(CompanyError::JobFailed { code }.into());
    }
    let result = status.export_result().ok_or(CompanyError::JobFailed {
        code: "NO_RESULT".into(),
    })?;

    // 5. The engine must have written a transfer file (S3).
    if !result.transfer_honoured() {
        tracing::error!("transfer: the engine wrote a full archive; removing it");
        let file = match result.path.clone() {
            None => EngineFile::Unknown,
            Some(p) => {
                let _ = std::fs::remove_file(&p);
                if std::fs::symlink_metadata(&p).is_ok() {
                    EngineFile::Left(p)
                } else {
                    EngineFile::Removed
                }
            }
        };
        return Err(TransferError::NotHonoured { file });
    }

    // 6. Place via `.partial`, check the hash, rename.
    let partial = partial_path(&req.dest);
    let bytes = crate::backup::place_archive(&result, &partial, progress).await?;
    if let Err(e) = std::fs::rename(&partial, &req.dest) {
        let _ = std::fs::remove_file(&partial);
        return Err(CompanyError::Io {
            what: "save transfer file",
            source: e,
        }
        .into());
    }

    // 7. "Sent before".
    let totals = result.verification_totals().cloned();
    let record = TransferRecord {
        path: req.dest.clone(),
        sha256: result.sha256.to_ascii_lowercase(),
        archive_id: result.archive_id.clone(),
        created_at: cl_settings::recent::now(),
        totals: totals.as_ref().map(history_totals),
    };
    store
        .update(|l| l.record_transfer(req.tenant_id, record.clone()))
        .map_err(CompanyError::from)?;
    progress(Progress::Done);
    tracing::info!("transfer: done");
    Ok(TransferOutcome {
        record,
        totals,
        bytes,
        discarded_payroll: *discarded,
    })
}
