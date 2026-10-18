//! Delete company (Q-185): Admin only (the engine checks the role), the
//! name typed exactly, and a verified backup taken first. The engine's safe
//! delete (E5b, capability `tenant-delete-v1`) re-checks the archive hash
//! and the typed name itself; until it lands, delete is refused before any
//! backup is taken.

use std::path::PathBuf;

use cl_api::{Capabilities, DeleteReceipt, Session};
use cl_core::Secret;
use cl_core::ids::TenantId;
use cl_settings::{BackupKind, BackupRecord, RecentStore, SecretStore};

use crate::backup::{BackupRequest, ProgressSink, backup};
use crate::error::CompanyError;

/// What to delete.
#[derive(Clone)]
pub struct DeleteRequest {
    /// The company.
    pub tenant_id: TenantId,
    /// The name as the owner typed it (must match exactly).
    pub typed_name: String,
    /// Where the before-delete backup goes.
    pub backup_dir: PathBuf,
    /// The closing-date password, when the company has one (U-15).
    pub closing_password: Option<Secret>,
}

impl std::fmt::Debug for DeleteRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeleteRequest")
            .field("tenant_id", &self.tenant_id)
            .finish_non_exhaustive()
    }
}

/// What [`delete_company`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteOutcome {
    /// The backup taken first (kept).
    pub backup: BackupRecord,
    /// The engine's receipt.
    pub receipt: DeleteReceipt,
}

/// Checks the typed name against the company's name: exact after
/// trimming, case-sensitive.
#[must_use]
pub fn name_matches(typed: &str, actual: &str) -> bool {
    !typed.trim().is_empty() && typed.trim() == actual.trim()
}

/// Deletes a company: switches the session out of it, takes a
/// before-delete backup, asks the engine's safe delete with that file,
/// then forgets the company locally. The backup file is kept.
///
/// # Errors
/// `Unavailable("tenant-delete-v1")` or `NameMismatch` (nothing sent, no
/// backup taken); `UnknownCompany`; everything [`backup`] can fail with;
/// `Api` from the delete (the company and the backup remain).
pub async fn delete_company(
    session: &Session,
    store: &RecentStore,
    secrets: &dyn SecretStore,
    req: &DeleteRequest,
    progress: &ProgressSink,
) -> Result<DeleteOutcome, CompanyError> {
    if !session
        .client()
        .capabilities()
        .has(Capabilities::TENANT_DELETE)
    {
        return Err(CompanyError::Unavailable(Capabilities::TENANT_DELETE));
    }
    let list = store.load()?;
    let rec = list
        .get(req.tenant_id)
        .cloned()
        .ok_or(CompanyError::UnknownCompany)?;
    if !name_matches(&req.typed_name, &rec.name) {
        return Err(CompanyError::NameMismatch);
    }
    if let cl_api::Scope::Company { tenant_id, .. } = session.scope()
        && tenant_id == req.tenant_id
    {
        session.close_company().await?;
    }
    let taken = backup(
        session,
        store,
        secrets,
        &BackupRequest {
            tenant_id: req.tenant_id,
            kind: BackupKind::BeforeDelete,
            dest_dir: req.backup_dir.clone(),
            apply_retention: false,
        },
        progress,
    )
    .await?;
    let passphrase = crate::backup::passphrase(secrets)?;
    let receipt = session
        .tenants()
        .delete(
            req.tenant_id,
            &cl_api::DeleteRequest {
                confirm_name: req.typed_name.trim().to_string(),
                archive_path: std::path::absolute(&taken.record.path)
                    .map_err(CompanyError::io("resolve backup"))?,
                archive_sha256: taken.record.sha256.clone(),
                passphrase,
                closing_password: req.closing_password.clone(),
            },
        )
        .await?;
    let now = cl_settings::recent::now();
    let receipt_json = serde_json::to_value(&receipt).unwrap_or_default();
    store.update(|l| l.tombstone(req.tenant_id, now, receipt_json))?;
    Ok(DeleteOutcome {
        backup: taken.record,
        receipt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_name_must_match_exactly() {
        assert!(name_matches(
            " Ozark Ridge Landscaping LLC ",
            "Ozark Ridge Landscaping LLC"
        ));
        assert!(!name_matches(
            "ozark ridge landscaping llc",
            "Ozark Ridge Landscaping LLC"
        ));
        assert!(!name_matches("", ""));
        assert!(!name_matches("Ozark", "Ozark Ridge"));
    }
}
