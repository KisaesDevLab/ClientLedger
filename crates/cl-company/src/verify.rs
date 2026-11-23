//! Test restore (P34 B2, U-296, Q-P34a): proves a backup file restores
//! cleanly without creating a company. The engine restores it inside one
//! transaction, verifies it (counts, content, the trial balance compared
//! string for string, the verification totals) and always rolls back.
//!
//! The verdict and every amount are the engine's; this module only stores
//! them on the backup's history entry (`tested`) and gives the owner plain
//! words. A file the app refuses before sending (changed after it was
//! written, not a backup) is recorded as a failed test with its reason,
//! not shown as an error.

use std::path::{Path, PathBuf};

use cl_api::{JobState, Session, TestRestoreRequest};
use cl_core::Timestamp;
use cl_settings::{BackupTest, RecentStore, SecretStore, TestTrialBalance};

use crate::backup::{GATE, ProgressSink};
use crate::error::CompanyError;

/// Reasons a test restore failed (codes; the UI has the words).
pub mod reason {
    /// The file changed after this app wrote it.
    pub const TAMPERED: &str = "TAMPERED";
    /// Not a `ClientLedger` backup (or damaged beyond reading).
    pub const NOT_A_BACKUP: &str = "NOT_A_BACKUP";
    /// A transfer file, not a backup.
    pub const TRANSFER_FILE: &str = "TRANSFER_FILE";
    /// Written by a newer `ClientLedger`.
    pub const TOO_NEW: &str = "ARCHIVE_TOO_NEW";

    /// The restored books did not match the file (trial balance, counts,
    /// content or totals).
    pub const MISMATCH: &str = "VERIFY_MISMATCH";
}

/// What a test restore found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOutcome {
    /// The file tested.
    pub path: PathBuf,
    /// Its SHA-256, as read now.
    pub sha256: Option<String>,
    /// The result stored on the backup's history entry.
    pub test: BackupTest,
    /// The engine's failed checks (names only).
    pub failures: Vec<String>,
    /// True when the history had this file and now holds the result.
    pub recorded: bool,
}

fn refused(path: &Path, now: Timestamp, why: &str) -> TestOutcome {
    TestOutcome {
        path: path.to_path_buf(),
        sha256: None,
        test: BackupTest {
            at: now,
            passed: false,
            reason: Some(why.into()),
            tb: None,
        },
        failures: Vec::new(),
        recorded: false,
    }
}

/// The reason code for an error the app gave before sending anything, or
/// `None` when the error is not about the file (then it is returned).
#[must_use]
pub fn local_refusal(e: &CompanyError) -> Option<&'static str> {
    match e {
        CompanyError::Tampered => Some(reason::TAMPERED),
        CompanyError::NotABackup => Some(reason::NOT_A_BACKUP),
        CompanyError::TransferNotABackup => Some(reason::TRANSFER_FILE),
        CompanyError::ArchiveTooNew => Some(reason::TOO_NEW),
        // A read error (a drive gone, a cloud-sync lock) says nothing
        // about the file: an error, never a failed test.
        _ => None,
    }
}

/// True for an engine job code that is a verdict about the file itself
/// (`ARCHIVE_CORRUPT`, `ARCHIVE_PASSPHRASE_INVALID`, `ARCHIVE_TOO_NEW`,
/// `ARCHIVE_VERIFY_FAILED`, …): every such code starts `ARCHIVE_`.
/// `INSUFFICIENT_STORAGE`, `TENANT_BUSY`, `TIMEOUT` and the like are not.
#[must_use]
pub fn is_file_verdict(code: &str) -> bool {
    code.starts_with("ARCHIVE_")
}

/// Test-restores a backup file. Waits for [`GATE`].
///
/// # Errors
/// `Unavailable` without `tenant-archive-test-restore-v1`; `NoPassphrase`;
/// `Io` when the file cannot be read; `Api` and `JobFailed` (`TIMEOUT`,
/// `JOB_UNKNOWN`, `INSUFFICIENT_STORAGE`, `TENANT_BUSY`) when the engine
/// could not run it; nothing is recorded then. A file the app or the
/// engine refuses (changed, damaged, wrong passphrase, too new, did not
/// verify) is a failed test, not an error.
pub async fn test_restore(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    path: &Path,
    progress: &ProgressSink,
) -> Result<TestOutcome, CompanyError> {
    let _gate = GATE.lock().await;
    test_restore_ungated(session, store, secrets, path, progress).await
}

/// [`test_restore`] for a caller that holds [`GATE`] (the scheduler).
///
/// # Errors
/// As [`test_restore`].
#[allow(clippy::too_many_lines, reason = "one linear sequence of steps")]
pub(crate) async fn test_restore_ungated(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    path: &Path,
    progress: &ProgressSink,
) -> Result<TestOutcome, CompanyError> {
    if !session
        .client()
        .capabilities()
        .has(cl_api::Capabilities::TENANT_ARCHIVE_TEST_RESTORE)
    {
        return Err(CompanyError::Unavailable(
            cl_api::Capabilities::TENANT_ARCHIVE_TEST_RESTORE,
        ));
    }
    let path = std::path::absolute(path).map_err(CompanyError::io("resolve archive"))?;
    let list = store.load()?;
    let info = {
        let (p, l) = (path.clone(), list.clone());
        tokio::task::spawn_blocking(move || crate::restore::inspect_archive(&p, &l))
            .await
            .map_err(|_| CompanyError::Io {
                what: "read archive",
                source: std::io::Error::other("worker stopped"),
            })?
    };
    let info = match info {
        Ok(i) => i,
        Err(e) => {
            let Some(why) = local_refusal(&e) else {
                return Err(e);
            };
            tracing::info!(reason = why, "test restore: refused before sending");
            let mut out = refused(&path, cl_settings::recent::now(), why);
            // A changed file still has its history entry (same archive id):
            // record the failure on the entry this app wrote.
            out.recorded = record_by_path(store, &path, &out.test)?;
            return Ok(out);
        }
    };
    let passphrase = crate::backup::passphrase(secrets)?;
    progress(crate::Progress::Starting);
    let job = session
        .tenants()
        .test_restore(&TestRestoreRequest {
            path: path.clone(),
            passphrase,
        })
        .await?;
    let status = crate::backup::poll_job(session, &job.job_id, progress).await?;
    let now = cl_settings::recent::now();
    let mut out = TestOutcome {
        path: path.clone(),
        sha256: Some(info.sha256.clone()),
        test: BackupTest {
            at: now,
            passed: false,
            reason: None,
            tb: None,
        },
        failures: Vec::new(),
        recorded: false,
    };
    if status.status == JobState::Failed {
        let code = status.error_code();
        if !is_file_verdict(&code) {
            // No disk space, busy, an engine fault: nothing is known about
            // the file, so nothing is recorded (Q-P34bi).
            tracing::warn!(code = %code, "test restore: could not run");
            return Err(CompanyError::JobFailed { code });
        }
        // Wrong passphrase, damaged, too new, did not verify: the
        // engine's code is the reason (never a path).
        tracing::info!(code = %code, "test restore: the engine refused the file");
        out.test.reason = Some(code);
    } else {
        let result = status
            .test_restore_result()?
            .ok_or(CompanyError::JobFailed {
                code: "NO_RESULT".into(),
            })?;
        out.test.passed = result.passed;
        out.test.tb = result.trial_balance.as_ref().map(|t| TestTrialBalance {
            rows_equal: t.rows_equal,
            totals_equal: t.totals_equal,
            expected_debit: t.expected.debit_exact,
            expected_credit: t.expected.credit_exact,
            actual_debit: t.actual.debit_exact,
            actual_credit: t.actual.credit_exact,
        });
        out.failures = result
            .failures
            .iter()
            .map(|f| match &f.table {
                Some(t) => format!("{}:{t}", f.check),
                None => f.check.clone(),
            })
            .collect();
        if !result.passed {
            out.test.reason = Some(reason::MISMATCH.into());
        }
        tracing::info!(
            passed = result.passed,
            failures = result.failures.len(),
            "test restore: done"
        );
    }
    let test = out.test.clone();
    let sha = info.sha256.clone();
    out.recorded = store.update(|l| l.record_test(&sha, test))?;
    progress(crate::Progress::Done);
    Ok(out)
}

/// Records a test result on the history entry for `path` (by path: a
/// changed file no longer has the recorded hash).
fn record_by_path(
    store: &RecentStore,
    path: &Path,
    test: &BackupTest,
) -> Result<bool, CompanyError> {
    Ok(store.update(|l| {
        for c in &mut l.companies {
            if let Some(b) =
                c.backups.iter_mut().rev().find(|b| {
                    b.path == path || std::path::absolute(&b.path).is_ok_and(|p| p == path)
                })
            {
                b.tested = Some(test.clone());
                return true;
            }
        }
        false
    })?)
}
