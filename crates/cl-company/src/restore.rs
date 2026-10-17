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
    let names = zipfile::entry_names(path).map_err(CompanyError::io("read archive"))?;
    if !names.iter().any(|n| n == "manifest.json.enc") {
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

/// Restores a backup as a new company and records it. The session ends
/// in the workspace; the caller opens the restored company.
///
/// # Errors
/// `NotABackup`, `Tampered`, `Io` (nothing sent); `NoPassphrase`; `Api`
/// (a wrong passphrase or a corrupt file is `Validation`; an engine
/// without `tenant-archive-v1` answers `NotFound`); `Settings`.
pub async fn restore(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &RestoreRequest,
    progress: &ProgressSink,
) -> Result<RestoreOutcome, CompanyError> {
    let list = store.load()?;
    let info = inspect_archive(&req.path, &list)?;
    let passphrase = match &req.passphrase {
        Some(p) if !p.is_empty() => p.clone(),
        _ => crate::backup::passphrase(secrets)?,
    };
    let path = std::path::absolute(&req.path).map_err(CompanyError::io("resolve archive"))?;
    session.close_company().await?;
    progress(Progress::Starting);
    let imported = session
        .tenants()
        .import_archive(&ImportRequest {
            path,
            passphrase,
            scratch: false,
        })
        .await?;
    progress(Progress::Verifying);

    // The company the file came from, if it still exists here.
    let previous = info
        .known
        .as_ref()
        .map(|(id, _)| *id)
        .or_else(|| {
            list.companies
                .iter()
                .find(|c| c.backup_by_archive_id(&info.archive_id).is_some())
                .map(|c| c.tenant_id)
        })
        .filter(|id| *id != imported.tenant_id);
    let previous_rec = previous.and_then(|id| list.get(id).cloned());
    let original_name = previous_rec
        .as_ref()
        .map_or_else(|| imported.name.clone(), |r| r.name.clone());
    let when = cl_settings::recent::now();
    let mut names_swapped = false;
    let mut final_name = imported.name.clone();
    if let (Some(prev_id), Some(prev)) = (previous, previous_rec.as_ref()) {
        let stamp = when.as_utc().format("%m/%d/%Y");
        let old_name = format!("{} (before restore {stamp})", prev.name);
        match session
            .tenants()
            .rename(prev_id, &old_name, None, IdempotencyKey::new())
            .await
        {
            Ok(_) => {
                session
                    .tenants()
                    .rename(
                        imported.tenant_id,
                        &original_name,
                        None,
                        IdempotencyKey::new(),
                    )
                    .await?;
                names_swapped = true;
                final_name = original_name.clone();
            }
            Err(ApiError::NotFound { .. }) => {
                // No rename route yet: both names stay as the engine set
                // them (the restored copy carries the archive's name).
            }
            Err(e) => return Err(e.into()),
        }
    }

    store.update(|l| {
        if let (Some(prev_id), true) = (previous, names_swapped)
            && let Some(p) = l.get_mut(prev_id)
        {
            p.name = format!("{} (before restore)", p.name);
        }
        let accent = previous_rec
            .as_ref()
            .map_or_else(|| cl_settings::recent::next_accent(&[]), |r| r.accent);
        let flags = previous_rec.as_ref().map(|r| r.flags).unwrap_or_default();
        let rec = l.upsert(imported.tenant_id, &final_name);
        rec.accent = accent;
        rec.flags = flags;
        rec.source = Some("restore".into());
    })?;
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
