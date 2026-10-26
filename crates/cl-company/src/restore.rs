//! Restore (P08 step 5): verify the file, import it as a **new** tenant
//! (the engine verifies counts, digests, the trial balance and files
//! before it commits), then swap names with the company it came from and
//! keep that older company until the owner confirms (Q-P02i, acceptance:
//! "restore never removes the existing company before the restored one
//! verifies").

use std::path::{Path, PathBuf};

use cl_api::{ApiError, IdempotencyKey, ImportRequest, ImportVerification, Session};
use cl_core::ids::TenantId;
use cl_core::{Secret, Timestamp};
use cl_settings::{BackupRecord, RecentCompanies, RecentStore, SecretStore};
use serde::Deserialize;

use crate::backup::{Progress, ProgressSink, sha256_file};
use crate::error::CompanyError;
use crate::zipfile;

/// The archive format the engine writes.
pub const ARCHIVE_FORMAT: &str = "mybooks-tenant-archive";
/// The newest format version this app reads.
pub const MAX_FORMAT_VERSION: u64 = 1;

/// The plaintext envelope of an archive (never names the company).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    format: String,
    format_version: u64,
    archive_id: String,
    #[serde(default)]
    created_at: Option<Timestamp>,
}

/// What can be learned about a `.clbackup` without the passphrase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveInfo {
    /// The archive id.
    pub archive_id: String,
    /// When it was written.
    pub created_at: Option<Timestamp>,
    /// Format version.
    pub format_version: u64,
    /// SHA-256 of the file.
    pub sha256: String,
    /// The backup record, when this app wrote the file (same hash).
    pub known: Option<(TenantId, BackupRecord)>,
    /// File size.
    pub bytes: u64,
}

/// Reads the envelope and hashes the file. A file whose archive id is in
/// the backup history but whose hash differs from the recorded one was
/// changed after it was written and is refused ([`CompanyError::Tampered`]);
/// the engine would refuse it too, but this stops before anything is sent.
///
/// # Errors
/// `Io`; `NotABackup`; `Tampered`.
pub fn inspect_archive(path: &Path, list: &RecentCompanies) -> Result<ArchiveInfo, CompanyError> {
    let bytes = zipfile::read_entry(path, "envelope.json")
        .map_err(CompanyError::io("read archive"))?
        .ok_or(CompanyError::NotABackup)?;
    let env: Envelope = serde_json::from_slice(&bytes).map_err(|_| CompanyError::NotABackup)?;
    if env.format != ARCHIVE_FORMAT || env.format_version > MAX_FORMAT_VERSION {
        return Err(CompanyError::NotABackup);
    }
    if !zipfile::has_entry(path, "manifest.json.enc").map_err(CompanyError::io("read archive"))? {
        return Err(CompanyError::NotABackup);
    }
    let sha256 = sha256_file(path).map_err(CompanyError::io("hash archive"))?;
    let size = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(CompanyError::io("stat archive"))?;
    let by_hash = list
        .find_backup_by_sha256(&sha256)
        .map(|(c, b)| (c.tenant_id, b.clone()));
    if by_hash.is_none()
        && list
            .companies
            .iter()
            .any(|c| c.backup_by_archive_id(&env.archive_id).is_some())
    {
        return Err(CompanyError::Tampered);
    }
    Ok(ArchiveInfo {
        archive_id: env.archive_id,
        created_at: env.created_at,
        format_version: env.format_version,
        sha256,
        known: by_hash,
        bytes: size,
    })
}

/// What to restore.
#[derive(Clone)]
pub struct RestoreRequest {
    /// The `.clbackup` file (absolute path on this computer).
    pub path: PathBuf,
    /// The passphrase, when typed (a file from another computer);
    /// otherwise the keychain's.
    pub passphrase: Option<Secret>,
}

impl std::fmt::Debug for RestoreRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoreRequest").finish_non_exhaustive()
    }
}

/// What [`restore`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreOutcome {
    /// The restored company (a new tenant).
    pub new_tenant: TenantId,
    /// Its name after the swap.
    pub name: String,
    /// The engine's verification.
    pub verification: Option<ImportVerification>,
    /// The company the file came from, renamed and kept for the owner to
    /// confirm before it is removed (never deleted here).
    pub kept_previous: Option<TenantId>,
    /// The file was one this app wrote (hash in the history).
    pub file_known: bool,
    /// Names could be swapped (needs the E5a rename route).
    pub names_swapped: bool,
}

/// Restores a backup as a new company and records it. The session must
/// be in the workspace (the shell closes the open company first, which
/// saves or prompts for open forms and waits for pending writes); the
/// caller opens the restored company afterwards.
///
/// Order: the file is checked locally, the import job runs and the engine
/// verifies it, the local record is written (colour and flags from the
/// archive's `host`, else from the company it came from), and only then
/// are names swapped, which needs the previous company's `updatedAt` from
/// `GET /api/v1/tenants`. A failed swap is not fatal: the restored
/// company keeps the archive's name and `names_swapped` is false.
///
/// # Errors
/// `NotABackup`, `Tampered`, `Io` (nothing sent); `NoPassphrase`;
/// `Api(NoCompanyOpen)` when a company is still open; `JobFailed` with
/// the engine's code (`ARCHIVE_PASSPHRASE_INVALID`, `ARCHIVE_CORRUPT`,
/// `ARCHIVE_TOO_NEW`); `Api`; `Settings`.
#[allow(clippy::too_many_lines, reason = "one linear sequence of steps")]
pub async fn restore(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &RestoreRequest,
    progress: &ProgressSink,
) -> Result<RestoreOutcome, CompanyError> {
    let list = store.load()?;
    let path = std::path::absolute(&req.path).map_err(CompanyError::io("resolve archive"))?;
    let info = {
        let (path, list) = (path.clone(), list.clone());
        tokio::task::spawn_blocking(move || inspect_archive(&path, &list))
            .await
            .map_err(|_| CompanyError::Io {
                what: "read archive",
                source: std::io::Error::other("worker stopped"),
            })??
    };
    let passphrase = match &req.passphrase {
        Some(p) if !p.is_empty() => p.clone(),
        _ => crate::backup::passphrase(secrets)?,
    };
    if session.scope() != cl_api::Scope::Workspace {
        return Err(CompanyError::Api(ApiError::NoCompanyOpen));
    }
    progress(Progress::Starting);
    let job = session
        .tenants()
        .import_archive(&ImportRequest {
            path,
            passphrase,
            scratch: false,
        })
        .await?;
    let imported = crate::backup::wait_for_job(session, &job.job_id, progress)
        .await?
        .import_result()
        .ok_or(CompanyError::JobFailed {
            code: "NO_RESULT".into(),
        })?;
    progress(Progress::Verifying);

    // The company the file came from, if it still exists here.
    let previous = info
        .known
        .as_ref()
        .map(|(id, _)| *id)
        .or(imported.source_tenant_id)
        .or_else(|| {
            list.companies
                .iter()
                .find(|c| c.backup_by_archive_id(&info.archive_id).is_some())
                .map(|c| c.tenant_id)
        })
        .filter(|id| *id != imported.tenant_id && list.get(*id).is_some());
    let previous_rec = previous.and_then(|id| list.get(id).cloned());
    // Read what the exporting app wrote, field by field (another version
    // may have written more or less than today's `HostMetadata`).
    let host_accent = imported
        .host
        .as_ref()
        .and_then(|h| h.get("accent"))
        .and_then(serde_json::Value::as_str)
        .and_then(cl_settings::Accent::parse);
    let host_flags: Option<cl_settings::CompanyFlags> = imported
        .host
        .as_ref()
        .and_then(|h| h.get("flags"))
        .and_then(|f| serde_json::from_value(f.clone()).ok());
    let host_prefs: Option<cl_settings::CompanyPrefs> = imported
        .host
        .as_ref()
        .and_then(|h| h.get("prefs"))
        .and_then(|f| serde_json::from_value(f.clone()).ok());
    let original_name = previous_rec
        .as_ref()
        .map_or_else(|| imported.name.clone(), |r| r.name.clone());

    // Local record first, so a failed name swap still leaves a company
    // the window can list and open.
    store.update(|l| {
        let accent = host_accent
            .or_else(|| previous_rec.as_ref().map(|r| r.accent))
            .unwrap_or_else(|| {
                cl_settings::recent::next_accent(
                    &l.companies.iter().map(|c| c.accent).collect::<Vec<_>>(),
                )
            });
        let flags = host_flags
            .or_else(|| previous_rec.as_ref().map(|r| r.flags))
            .unwrap_or_default();
        // Machine-local fields (receipt inbox, this month's token count,
        // the consent note) come from this computer's record, never the
        // archive; the token cap is the stricter of the two (M5).
        let local = previous_rec.as_ref().map(|r| &r.prefs);
        let prefs = if let Some(p) = host_prefs {
            cl_settings::CompanyPrefs::merged_from_archive(&p, local)
        } else {
            let mut p = local.cloned().unwrap_or_default();
            p.normalize();
            p
        };
        let rec = l.upsert(imported.tenant_id, &imported.name);
        rec.accent = accent;
        rec.flags = flags;
        rec.prefs = prefs;
        rec.source = Some("restore".into());
    })?;

    let mut names_swapped = false;
    let mut final_name = imported.name.clone();
    if let (Some(prev_id), Some(prev)) = (previous, previous_rec.as_ref()) {
        let stamp = cl_settings::recent::now().as_utc().format("%m/%d/%Y");
        let old_name = format!("{} (before restore {stamp})", prev.name);
        names_swapped = swap_names(
            session,
            prev_id,
            &old_name,
            imported.tenant_id,
            &original_name,
        )
        .await?;
        if names_swapped {
            final_name = original_name.clone();
            store.update(|l| {
                if let Some(p) = l.get_mut(prev_id) {
                    p.name.clone_from(&old_name);
                }
                if let Some(n) = l.get_mut(imported.tenant_id) {
                    n.name.clone_from(&final_name);
                }
            })?;
        }
    }
    progress(Progress::Done);
    Ok(RestoreOutcome {
        new_tenant: imported.tenant_id,
        name: final_name,
        verification: imported.verification,
        kept_previous: previous,
        file_known: info.known.is_some(),
        names_swapped,
    })
}

/// Renames the previous company out of the way, then the restored one to
/// the original name. Each rename sends the row's current `updatedAt`
/// from `GET /api/v1/tenants` (409 `STALE` otherwise). Returns `Ok(false)`
/// when the engine has no rename route, the previous company changed
/// meanwhile (`STALE`), or the second rename failed after the first
/// succeeded (the restored company keeps the archive name).
///
/// # Errors
/// `Api` when the first rename fails for a reason other than a missing
/// route.
async fn swap_names(
    session: &Session,
    prev_id: TenantId,
    old_name: &str,
    new_id: TenantId,
    original_name: &str,
) -> Result<bool, CompanyError> {
    let updated_at = |id: TenantId, rows: &[cl_api::TenantSummary]| {
        rows.iter().find(|t| t.id == id).and_then(|t| t.updated_at)
    };
    let rows = match session.tenants().list().await {
        Ok(r) => r,
        Err(ApiError::NotFound { .. }) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let Some(prev_at) = updated_at(prev_id, &rows) else {
        return Ok(false);
    };
    match session
        .tenants()
        .rename(prev_id, old_name, prev_at, IdempotencyKey::new())
        .await
    {
        Ok(_) => {}
        Err(ApiError::NotFound { .. }) => return Ok(false),
        Err(ApiError::Conflict {
            reason: cl_api::ConflictReason::Stale,
            ..
        }) => {
            // Someone changed the previous company meanwhile: the restore
            // itself succeeded, so keep both names as they are.
            tracing::warn!("previous company changed; names not swapped");
            return Ok(false);
        }
        Err(e) => return Err(e.into()),
    }
    let rows = session.tenants().list().await.unwrap_or_default();
    let Some(new_at) = updated_at(new_id, &rows) else {
        tracing::warn!("restored company could not be renamed to the original name");
        return Ok(false);
    };
    match session
        .tenants()
        .rename(new_id, original_name, new_at, IdempotencyKey::new())
        .await
    {
        Ok(_) => Ok(true),
        Err(e) => {
            tracing::warn!(error = %e, "restored company keeps the archive name");
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cl_settings::BackupKind;

    fn id(n: u8) -> TenantId {
        TenantId::parse(&format!("1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a{n:02x}")).expect("id")
    }

    fn write_archive(dir: &Path, name: &str, archive_id: &str, extra: &[u8]) -> PathBuf {
        let p = dir.join(name);
        let envelope = format!(
            r#"{{"format":"mybooks-tenant-archive","formatVersion":1,"archiveId":"{archive_id}","createdAt":"2026-11-04T08:12:00.000Z","encryption":{{"kdf":"pbkdf2-sha512"}},"keyCheck":"x"}}"#
        );
        zipfile::write_stored(
            &p,
            &[
                ("envelope.json".into(), envelope.into_bytes()),
                ("manifest.json.enc".into(), extra.to_vec()),
            ],
        )
        .expect("zip");
        p
    }

    #[test]
    fn inspect_reads_the_envelope_and_knows_its_own_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write_archive(dir.path(), "a.clbackup", "arch-1", b"enc");
        let mut list = RecentCompanies::default();
        let info = inspect_archive(&p, &list).expect("info");
        assert_eq!(info.archive_id, "arch-1");
        assert_eq!(info.format_version, 1);
        assert!(info.known.is_none());
        assert!(info.created_at.is_some());
        list.upsert(id(1), "Ozark");
        list.record_backup(
            id(1),
            BackupRecord {
                path: p.clone(),
                sha256: info.sha256.clone(),
                archive_id: "arch-1".into(),
                created_at: Timestamp::from_unix_millis(1).expect("ts"),
                kind: BackupKind::Backup,
            },
        );
        let info = inspect_archive(&p, &list).expect("info");
        assert_eq!(info.known.as_ref().map(|(t, _)| *t), Some(id(1)));
    }

    #[test]
    fn a_changed_file_with_a_known_archive_id_is_refused() {
        let dir = tempfile::tempdir().expect("tmp");
        let p = write_archive(dir.path(), "a.clbackup", "arch-1", b"enc");
        let mut list = RecentCompanies::default();
        list.upsert(id(1), "Ozark");
        list.record_backup(
            id(1),
            BackupRecord {
                path: p.clone(),
                sha256: "0".repeat(64),
                archive_id: "arch-1".into(),
                created_at: Timestamp::from_unix_millis(1).expect("ts"),
                kind: BackupKind::Backup,
            },
        );
        let e = inspect_archive(&p, &list).expect_err("tampered");
        assert!(matches!(e, CompanyError::Tampered), "{e:?}");
    }

    #[test]
    fn not_a_backup_is_refused_before_anything_is_sent() {
        let dir = tempfile::tempdir().expect("tmp");
        let txt = dir.path().join("notes.clbackup");
        std::fs::write(&txt, b"hello").expect("write");
        let list = RecentCompanies::default();
        assert!(matches!(
            inspect_archive(&txt, &list).expect_err("text"),
            CompanyError::NotABackup
        ));
        let wrong = dir.path().join("w.clbackup");
        zipfile::write_stored(
            &wrong,
            &[(
                "envelope.json".into(),
                br#"{"format":"other","formatVersion":1,"archiveId":"a"}"#.to_vec(),
            )],
        )
        .expect("zip");
        assert!(matches!(
            inspect_archive(&wrong, &list).expect_err("format"),
            CompanyError::NotABackup
        ));
        let newer = dir.path().join("n.clbackup");
        zipfile::write_stored(
            &newer,
            &[
                (
                    "envelope.json".into(),
                    br#"{"format":"mybooks-tenant-archive","formatVersion":2,"archiveId":"a"}"#
                        .to_vec(),
                ),
                ("manifest.json.enc".into(), b"x".to_vec()),
            ],
        )
        .expect("zip");
        assert!(matches!(
            inspect_archive(&newer, &list).expect_err("version"),
            CompanyError::NotABackup
        ));
    }
}
