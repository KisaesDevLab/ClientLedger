//! Verify Data (P34 B3, U-12, Q-170–Q-172): the engine's integrity checks
//! for the open company, and the one way the app fixes what they find.
//!
//! - [`quick_check_due`]: the weekly quick check is due when the stored
//!   last quick run is 7 or more days old (company-wide, Q-P34f).
//! - [`run_check`]: one check run (owner or admin; the engine refuses
//!   others).
//! - [`fix_after_backup`]: a verified before-fix backup first (trigger
//!   `before_fix`), then the fix guarded by the run's ledger version and
//!   naming that backup, then the same level again. No backup, no fix
//!   (Q-170). The caller takes [`crate::backup::GATE`] first (refusing
//!   when it is busy), then runs this under a routine maintenance hold in
//!   LAN mode (Q-P34d), so the hold never waits behind a test restore.
//!
//! Every amount and verdict is the engine's; logs carry codes and counts.

use cl_api::{FixResult, IntegrityId, IntegrityLevel, IntegrityRun, IntegritySummary, Session};
use cl_core::Timestamp;
use cl_core::ids::TenantId;
use cl_settings::{BackupKind, BackupRecord, BackupTrigger, RecentStore, SecretStore};

use crate::backup::{BackupOptions, BackupRequest, GateGuard, ProgressSink};
use crate::error::CompanyError;

/// Days between weekly quick checks (Q-171).
pub const QUICK_CHECK_DAYS: i64 = 7;

/// True when the weekly quick check should run: never run, or the last
/// one finished 7 or more days ago. A last run in the future (the clock
/// went back) counts as recent.
#[must_use]
pub fn quick_check_due(summary: &IntegritySummary, now: Timestamp) -> bool {
    match &summary.last_quick {
        None => true,
        Some(r) => {
            now.unix_millis()
                .saturating_sub(r.finished_at.unix_millis())
                >= QUICK_CHECK_DAYS * 86_400_000
        }
    }
}

/// Runs the checks of `level` for the open company.
///
/// # Errors
/// `Api` (`Local(Unsupported)` without `integrity-v1`, `Forbidden` for a
/// user who is not an owner or admin).
pub async fn run_check(
    session: &Session,
    level: IntegrityLevel,
) -> Result<IntegrityRun, CompanyError> {
    let run = session.integrity().check(level).await?;
    tracing::info!(
        findings = run.findings.len(),
        truncated = run.truncated,
        "integrity check done"
    );
    Ok(run)
}

/// What a fix did.
#[derive(Debug)]
pub struct FixOutcome {
    /// The verified backup made first.
    pub backup: BackupRecord,
    /// The engine's answer.
    pub result: FixResult,
    /// The same level checked again (an error is shown with "check
    /// again"; the fix itself stands).
    pub recheck: Result<IntegrityRun, CompanyError>,
}

/// What to fix and where the backup before it goes.
#[derive(Clone, Copy, Debug)]
pub struct FixRequest<'a> {
    /// The open company.
    pub tenant_id: TenantId,
    /// Where the before-fix backup goes.
    pub backup_dir: &'a std::path::Path,
    /// The run the findings came from (its ledger version guards the fix).
    pub run: &'a IntegrityRun,
    /// The chosen findings.
    pub finding_ids: &'a [IntegrityId],
}

/// Fixes the chosen findings of a run, after a verified backup. `gate` is
/// the caller's held [`crate::backup::GATE`] (see [`crate::backup::try_gate`]).
///
/// # Errors
/// Any backup error (nothing fixed); `Api` from the fix, where
/// `cl_api::is_integrity_stale` means the books changed since the check
/// (check again); `Api(Local(InvalidValue))` for no findings.
pub async fn fix_after_backup(
    gate: GateGuard,
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &FixRequest<'_>,
    progress: &ProgressSink,
) -> Result<FixOutcome, CompanyError> {
    let FixRequest {
        tenant_id,
        backup_dir,
        run,
        finding_ids,
    } = *req;
    if finding_ids.is_empty() {
        return Err(CompanyError::Api(cl_api::ApiError::Local(
            cl_api::LocalValidation::InvalidValue,
        )));
    }
    let _gate = gate;
    // 1. The backup. No retention runs here; a later regular backup into
    //    the same folder counts it toward that folder's rule (Q-P34bj).
    let backup = crate::backup::backup_ungated(
        session,
        store,
        secrets,
        &BackupRequest {
            tenant_id,
            kind: BackupKind::Backup,
            dest_dir: backup_dir.to_path_buf(),
            apply_retention: false,
            protected: None,
        },
        &BackupOptions {
            trigger: Some(BackupTrigger::BeforeFix),
            ..BackupOptions::default()
        },
        progress,
    )
    .await?;
    // 2. The fix, guarded by the run's ledger version, naming the backup.
    let result = session
        .integrity()
        .fix(run, finding_ids, Some(&backup.record.archive_id))
        .await?;
    tracing::info!(
        fixed = result.fixed.len(),
        skipped = result.skipped.len(),
        "integrity fix done"
    );
    // 3. Check again at the same level.
    let recheck = run_check(session, run.level).await;
    Ok(FixOutcome {
        backup: backup.record,
        result,
        recheck,
    })
}
