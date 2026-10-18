//! Company (tenant) lifecycle and tenant archives: the E5 contract from
//! `target/plans/P02.md` §2.5 (U-05), typed ahead of the engine merge.
//!
//! - `GET /api/v1/tenants` lists the companies the user can open, never the
//!   workspace (capability `tenants-list-v1`). Until the engine advertises
//!   it, [`crate::Session::companies`] keeps using `accessibleTenants`.
//! - `POST /api/v1/tenants` creates a company; `PATCH /api/v1/tenants/{id}`
//!   renames it with `expectedUpdatedAt` (409 `STALE` → `Conflict { Stale }`).
//! - `POST /api/v1/tenants/{id}/archives` starts an encrypted export job,
//!   `GET /api/v1/tenant-archives/jobs/{jobId}` polls it (a restarted engine
//!   answers 404 `JOB_UNKNOWN`: failed, retry), and
//!   `POST /api/v1/tenant-archives/import` imports a `.clbackup` by absolute
//!   path as a **new** tenant and returns the engine's verification
//!   (capability `tenant-archive-v1`).
//! - `POST /api/v1/tenants/{id}/delete` is the safe delete (E5b, capability
//!   `tenant-delete-v1`): it needs the typed company name, a fresh archive
//!   with its SHA-256 and the passphrase, and never runs without them.
//!
//! Passphrases and the closing-date password travel in [`Secret`]s and are
//! never logged; the request bodies are credential-carrying
//! ([`crate::client`] re-verifies the engine listener for them).

use std::path::{Path, PathBuf};

use cl_core::Secret;
use cl_core::Timestamp;
use cl_core::ids::{CompanyId, TenantId};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{
    TENANT_ARCHIVE_DOWNLOAD, TENANT_ARCHIVE_EXPORT, TENANT_ARCHIVE_IMPORT, TENANT_ARCHIVE_JOB,
    TENANT_DELETE, TENANT_RENAME, TENANTS_CREATE, TENANTS_LIST,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};

/// One row of `GET /api/v1/tenants`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantSummary {
    /// The engine tenant.
    pub id: TenantId,
    /// Company label (owner data: never logged).
    pub name: String,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed; the rename guard (`expectedUpdatedAt`).
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
    /// The user's role there.
    #[serde(default)]
    pub role: Option<String>,
}

/// Input of [`Tenants::create`] (`POST /api/v1/tenants`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTenant {
    /// Company name.
    pub name: String,
    /// Engine COA template slug (`default`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub business_type: Option<String>,
    /// Alias the E5 route accepts for the template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coa_template: Option<String>,
    /// Seed only the system accounts; the app imports its own chart.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_accounts_only: Option<bool>,
}

/// Answer of [`Tenants::create`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedTenant {
    /// The new tenant.
    pub tenant_id: TenantId,
    /// Its company row, when the engine reports it.
    pub company_id: Option<CompanyId>,
    /// Its name.
    pub name: String,
}

impl<'de> Deserialize<'de> for CreatedTenant {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire {
            #[serde(default)]
            id: Option<TenantId>,
            #[serde(default)]
            tenant_id: Option<TenantId>,
            #[serde(default)]
            company_id: Option<CompanyId>,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            tenant_name: Option<String>,
        }
        let w = Wire::deserialize(d)?;
        let tenant_id = w
            .tenant_id
            .or(w.id)
            .ok_or_else(|| serde::de::Error::missing_field("tenantId"))?;
        Ok(CreatedTenant {
            tenant_id,
            company_id: w.company_id,
            name: w.name.or(w.tenant_name).unwrap_or_default(),
        })
    }
}

/// What a backup contains (`POST /api/v1/tenants/{id}/archives`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveRequest {
    /// Bundle attachment files.
    pub include_attachments: bool,
    /// Bundle the audit trail.
    pub include_audit: bool,
    /// Opaque application metadata kept in `manifest.host` (≤ 16 KiB).
    pub host: Value,
    /// The owner's backup passphrase (never logged).
    pub passphrase: Secret,
}

impl Serialize for ArchiveRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("ArchiveRequest", 4)?;
        st.serialize_field("includeAttachments", &self.include_attachments)?;
        st.serialize_field("includeAudit", &self.include_audit)?;
        st.serialize_field("host", &self.host)?;
        st.serialize_field("passphrase", self.passphrase.expose())?;
        st.end()
    }
}

/// Maximum `host` metadata size the engine accepts.
pub const HOST_METADATA_MAX_BYTES: usize = 16 * 1024;

/// A started export job (202).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveJob {
    /// Poll handle (not durable across engine restarts).
    pub job_id: JobId,
    /// The archive this job writes.
    pub archive_id: String,
}

/// A job id: path-safe by construction.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct JobId(String);

impl JobId {
    /// Accepts `[A-Za-z0-9_-]{1,128}`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` otherwise.
    pub fn parse(s: &str) -> Result<JobId, ApiError> {
        if path_safe(s, 128) {
            Ok(JobId(s.to_string()))
        } else {
            Err(ApiError::Local(LocalValidation::InvalidValue))
        }
    }

    /// The id text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for JobId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        JobId::parse(&s).map_err(|_| serde::de::Error::custom("job id"))
    }
}

/// A file name the engine hands out: one path segment, no traversal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchiveFileName(String);

impl ArchiveFileName {
    /// Accepts `[A-Za-z0-9._-]{1,200}` that is not `.` or `..`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` otherwise.
    pub fn parse(s: &str) -> Result<ArchiveFileName, ApiError> {
        let ok = (1..=200).contains(&s.len())
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
        if ok {
            Ok(ArchiveFileName(s.to_string()))
        } else {
            Err(ApiError::Local(LocalValidation::InvalidValue))
        }
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ArchiveFileName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ArchiveFileName::parse(&s).map_err(|_| serde::de::Error::custom("file name"))
    }
}

fn path_safe(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Where an archive job stands (`jobs.ts` `JobStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    /// Waiting to start.
    Queued,
    /// Running.
    Running,
    /// Finished; `result` is present.
    #[serde(alias = "done")]
    Succeeded,
    /// Failed; `error` says why and nothing was written.
    Failed,
}

/// Why a job failed (`error: {code, message}`); only the code is used.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct JobError {
    /// Stable code (`ARCHIVE_PASSPHRASE_INVALID`, `ARCHIVE_CORRUPT`, ...).
    pub code: String,
    /// The engine's message (owner data may be absent; never logged).
    #[serde(default)]
    pub message: String,
}

/// `GET /api/v1/tenant-archives/jobs/{jobId}`: an export or import job.
/// `result` is the job's own shape; use [`ArchiveJobStatus::export_result`]
/// or [`ArchiveJobStatus::import_result`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveJobStatus {
    /// The job.
    #[serde(default)]
    pub job_id: Option<JobId>,
    /// `export` or `import`.
    #[serde(default)]
    pub kind: Option<String>,
    /// State.
    pub status: JobState,
    /// 0-100, when reported.
    #[serde(default)]
    pub progress: Option<u8>,
    /// Current step name (`queued`, `tables`, `files`, `verify`, `done`).
    #[serde(default)]
    pub step: Option<String>,
    /// The result, once `Succeeded`.
    #[serde(default)]
    pub result: Option<Value>,
    /// The failure, once `Failed`.
    #[serde(default)]
    pub error: Option<JobError>,
}

impl ArchiveJobStatus {
    /// The finished export (a succeeded export job).
    #[must_use]
    pub fn export_result(&self) -> Option<ArchiveResult> {
        self.result
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    /// The finished import (a succeeded import job).
    #[must_use]
    pub fn import_result(&self) -> Option<ImportResult> {
        self.result
            .clone()
            .and_then(|v| serde_json::from_value(v).ok())
    }

    /// The failure code, or `FAILED` when none was given.
    #[must_use]
    pub fn error_code(&self) -> String {
        self.error
            .as_ref()
            .map_or_else(|| "FAILED".to_string(), |e| e.code.clone())
    }
}

/// A started import job (202 `{jobId}`); poll with
/// [`Tenants::archive_job`] and read [`ArchiveJobStatus::import_result`].
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportJob {
    /// Poll handle.
    pub job_id: JobId,
}

/// A finished export.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveResult {
    /// The archive id (equals the manifest's).
    pub archive_id: String,
    /// File name under the engine's archive folder.
    pub file_name: ArchiveFileName,
    /// Absolute path of the file (desktop: the app moves it).
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// SHA-256 of the ciphertext file, lowercase hex.
    pub sha256: String,
    /// File size.
    #[serde(default)]
    pub bytes: Option<u64>,
    /// Schema version written.
    #[serde(default)]
    pub schema_version: Option<u64>,
}

/// Input of [`Tenants::import_archive`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRequest {
    /// Absolute path to a `.clbackup` / `.zip` on this computer.
    pub path: PathBuf,
    /// The passphrase the file was written with.
    pub passphrase: Secret,
    /// Create a scratch tenant (test-restore, E5b).
    pub scratch: bool,
}

impl Serialize for ImportRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("ImportRequest", 3)?;
        st.serialize_field("path", &self.path.to_string_lossy())?;
        st.serialize_field("passphrase", self.passphrase.expose())?;
        if self.scratch {
            st.serialize_field("scratch", &true)?;
        }
        st.end()
    }
}

/// The engine's own verification of an import (manifest counts, content
/// digests, trial balance, files, identity). The app shows it; it never
/// recomputes a trial balance in Rust (non-negotiable 7).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportVerification {
    /// Per-table row counts matched the manifest.
    #[serde(default)]
    pub tables: Option<u64>,
    /// Content digests matched.
    #[serde(default)]
    pub content_equal: Option<bool>,
    /// The trial balance matched string-for-string.
    #[serde(default)]
    pub trial_balance_equal: Option<bool>,
    /// Files written and hashed.
    #[serde(default)]
    pub files: Option<u64>,
    /// Everything else the engine reported.
    #[serde(flatten)]
    pub other: Value,
}

/// Result of an import job (`import.service.ts` `ImportResult`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    /// The new tenant.
    pub tenant_id: TenantId,
    /// Its name (from the archive).
    #[serde(default)]
    pub name: String,
    /// Whether it is a scratch tenant.
    #[serde(default)]
    pub scratch: bool,
    /// The archive that was imported.
    #[serde(default)]
    pub archive_id: Option<String>,
    /// The tenant the archive was exported from.
    #[serde(default)]
    pub source_tenant_id: Option<TenantId>,
    /// The engine's verification report.
    #[serde(default)]
    pub verification: Option<ImportVerification>,
    /// Non-fatal notes.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The manifest's `host` object (what the exporting app wrote), echoed
    /// by the engine so a restore on another computer keeps the app's
    /// colour and flags.
    #[serde(default)]
    pub host: Option<Value>,
}

/// Input of [`Tenants::delete`] (E5b safe delete, Q-185).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeleteRequest {
    /// The company name as the owner typed it.
    pub confirm_name: String,
    /// A fresh archive of this tenant.
    pub archive_path: PathBuf,
    /// Its SHA-256 (lowercase hex), from the export job.
    pub archive_sha256: String,
    /// The passphrase the archive was written with.
    pub passphrase: Secret,
    /// Required once the company has a closing-date password (U-15).
    pub closing_password: Option<Secret>,
}

impl Serialize for DeleteRequest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Archive<'a> {
            path: String,
            sha256: &'a str,
        }
        let mut st = s.serialize_struct("DeleteRequest", 4)?;
        st.serialize_field("confirmName", &self.confirm_name)?;
        st.serialize_field(
            "archive",
            &Archive {
                path: self.archive_path.to_string_lossy().into_owned(),
                sha256: &self.archive_sha256,
            },
        )?;
        st.serialize_field("passphrase", self.passphrase.expose())?;
        if let Some(cp) = &self.closing_password {
            st.serialize_field("closingPassword", cp.expose())?;
        }
        st.end()
    }
}

/// The engine's delete receipt (kept with the backup history).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReceipt {
    /// The deleted tenant.
    pub tenant_id: TenantId,
    /// Archive the delete was verified against.
    #[serde(default)]
    pub archive_id: Option<String>,
    /// When.
    #[serde(default)]
    pub deleted_at: Option<Timestamp>,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn require_absolute_file(p: &Path) -> Result<(), ApiError> {
    if p.is_absolute() && p.file_name().is_some() {
        Ok(())
    } else {
        Err(ApiError::Local(LocalValidation::InvalidValue))
    }
}

/// Company lifecycle and archives.
pub struct Tenants<'a>(pub(crate) &'a Session);

impl Session {
    /// Company (tenant) management: works from the workspace.
    #[must_use]
    pub fn tenants(&self) -> Tenants<'_> {
        Tenants(self)
    }
}

impl Tenants<'_> {
    /// `GET /api/v1/tenants`: the companies the user can open (never the
    /// workspace). Needs `tenants-list-v1`; an older engine answers 404.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(&self) -> Result<Vec<TenantSummary>, ApiError> {
        #[derive(Deserialize)]
        struct Wrapper {
            tenants: Vec<TenantSummary>,
        }
        let reply = self
            .0
            .send(
                &TENANTS_LIST,
                Call::at(Method::Get, TENANTS_LIST.route()),
                None,
            )
            .await?;
        // The route may answer a bare array or `{tenants: [...]}`.
        if let Ok(list) = serde_json::from_slice::<Vec<TenantSummary>>(&reply.body) {
            return Ok(list);
        }
        Ok(decode::<Wrapper>(&reply, "tenants")?.tenants)
    }

    /// `POST /api/v1/tenants`: a new company. Never retried.
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`]. A 2xx whose answer cannot be read is
    /// `SavedUnreadable` (the company exists).
    pub async fn create(&self, new: &NewTenant) -> Result<CreatedTenant, ApiError> {
        let call = Call::at(Method::Post, TENANTS_CREATE.route()).json_body(json_body(new)?);
        let reply = self.0.send(&TENANTS_CREATE, call, None).await?;
        decode(&reply, "created tenant").map_err(saved_unreadable)
    }

    /// `PATCH /api/v1/tenants/{id}`: rename a company. The engine compares
    /// `expectedUpdatedAt` and answers 409 `STALE` when the row moved.
    ///
    /// # Errors
    /// `Conflict { Stale }`; `Validation`; any [`ApiError`].
    pub async fn rename(
        &self,
        id: TenantId,
        name: &str,
        expected_updated_at: Timestamp,
        key: IdempotencyKey,
    ) -> Result<TenantSummary, ApiError> {
        #[derive(Deserialize)]
        struct RenamedWrapper {
            tenant: TenantSummary,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            name: &'a str,
            expected_updated_at: String,
        }
        let name = name.trim();
        if name.is_empty() || name.len() > 255 {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::new(
            Method::Patch,
            TENANT_RENAME.route(),
            TENANT_RENAME.route().replace("{id}", &id.to_string()),
        )
        .json_body(json_body(&Body {
            name,
            expected_updated_at: expected_updated_at.to_wire(),
        })?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&TENANT_RENAME, call, None).await?;
        if let Ok(w) = serde_json::from_slice::<RenamedWrapper>(&reply.body) {
            return Ok(w.tenant);
        }
        decode(&reply, "renamed tenant").map_err(saved_unreadable)
    }

    /// `POST /api/v1/tenants/{id}/archives`: start an encrypted export of a
    /// company (202). Poll with [`Tenants::archive_job`].
    ///
    /// # Errors
    /// `Local(InvalidValue)` when `host` exceeds 16 KiB or the passphrase
    /// is empty (nothing sent); any [`ApiError`].
    pub async fn export_archive(
        &self,
        id: TenantId,
        request: &ArchiveRequest,
    ) -> Result<ArchiveJob, ApiError> {
        if request.passphrase.is_empty()
            || serde_json::to_vec(&request.host).is_ok_and(|b| b.len() > HOST_METADATA_MAX_BYTES)
            || serde_json::to_vec(&request.host).is_err()
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::new(
            Method::Post,
            TENANT_ARCHIVE_EXPORT.route(),
            TENANT_ARCHIVE_EXPORT
                .route()
                .replace("{id}", &id.to_string()),
        )
        .json_body(json_body(request)?);
        let reply = self.0.send(&TENANT_ARCHIVE_EXPORT, call, None).await?;
        decode(&reply, "archive job")
    }

    /// `GET /api/v1/tenant-archives/jobs/{jobId}`. A 404 `JOB_UNKNOWN`
    /// (engine restarted) is `NotFound`: treat the export as failed and
    /// start again.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn archive_job(&self, job: &JobId) -> Result<ArchiveJobStatus, ApiError> {
        let call = Call::new(
            Method::Get,
            TENANT_ARCHIVE_JOB.route(),
            TENANT_ARCHIVE_JOB.route().replace("{jobId}", job.as_str()),
        );
        let reply = self.0.send(&TENANT_ARCHIVE_JOB, call, None).await?;
        decode(&reply, "archive job status")
    }

    /// `GET /api/v1/tenant-archives/{fileName}/download`: the encrypted file
    /// bytes (LAN clients; on this computer the app moves the file instead).
    ///
    /// # Errors
    /// `NotFound`; `PayloadTooLarge` beyond the client's body cap; any
    /// [`ApiError`].
    pub async fn download_archive(&self, file: &ArchiveFileName) -> Result<Vec<u8>, ApiError> {
        let call = Call::new(
            Method::Get,
            TENANT_ARCHIVE_DOWNLOAD.route(),
            TENANT_ARCHIVE_DOWNLOAD
                .route()
                .replace("{fileName}", file.as_str()),
        );
        let reply = self.0.send(&TENANT_ARCHIVE_DOWNLOAD, call, None).await?;
        Ok(reply.body.to_vec())
    }

    /// `POST /api/v1/tenant-archives/import`: start importing a `.clbackup`
    /// as a new tenant (202 `{jobId}`). The job verifies counts, digests,
    /// the trial balance and files before it commits; a failure leaves
    /// nothing behind and shows as `Failed` with an `error` code
    /// (`ARCHIVE_PASSPHRASE_INVALID`, `ARCHIVE_CORRUPT`, `ARCHIVE_TOO_NEW`).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a relative path or empty passphrase
    /// (nothing sent); `Validation` for a path the engine refuses; any
    /// [`ApiError`].
    pub async fn import_archive(&self, request: &ImportRequest) -> Result<ImportJob, ApiError> {
        require_absolute_file(&request.path)?;
        if request.passphrase.is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::at(Method::Post, TENANT_ARCHIVE_IMPORT.route())
            .json_body(json_body(request)?)
            .timeout(crate::client::TimeoutClass::Archive);
        let reply = self.0.send(&TENANT_ARCHIVE_IMPORT, call, None).await?;
        decode(&reply, "import job").map_err(saved_unreadable)
    }

    /// `POST /api/v1/tenants/{id}/delete`: the safe delete (Q-185). The
    /// session must not be in the target company. Never retried.
    ///
    /// # Errors
    /// `Local(BackupRequired)` without an absolute archive path and a
    /// SHA-256, `Local(InvalidValue)` for an empty name or passphrase
    /// (nothing sent); `Conflict` for `ARCHIVE_STALE`, `TENANT_BUSY`,
    /// `SWITCH_TENANT_FIRST`; `Forbidden`; any [`ApiError`].
    pub async fn delete(
        &self,
        id: TenantId,
        request: &DeleteRequest,
    ) -> Result<DeleteReceipt, ApiError> {
        if require_absolute_file(&request.archive_path).is_err()
            || !is_sha256_hex(&request.archive_sha256)
        {
            return Err(ApiError::Local(LocalValidation::BackupRequired));
        }
        if request.confirm_name.trim().is_empty() || request.passphrase.is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        if let crate::session::Scope::Company { tenant_id, .. } = self.0.scope()
            && tenant_id == id
        {
            return Err(ApiError::Conflict {
                reason: crate::errors::ConflictReason::Busy,
                current_version: None,
                code: crate::errors::ErrorCode::parse("SWITCH_TENANT_FIRST"),
                message: crate::errors::UserText::default(),
            });
        }
        let call = Call::new(
            Method::Post,
            TENANT_DELETE.route(),
            TENANT_DELETE.route().replace("{id}", &id.to_string()),
        )
        .json_body(json_body(request)?)
        .timeout(crate::client::TimeoutClass::Archive);
        let reply = self.0.send(&TENANT_DELETE, call, None).await?;
        decode(&reply, "delete receipt").map_err(saved_unreadable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{TENANT, company_session, json, requests};
    use crate::session::Scope;
    use crate::session::tests::session_on;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn ws(server: &MockServer) -> Session {
        session_on(server, Scope::Workspace)
    }

    #[tokio::test]
    async fn list_accepts_a_bare_array_and_an_object() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tenants"))
            .respond_with(json(
                200,
                r#"[{"id":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Canary Co",
                     "createdAt":"2026-10-01T10:00:00.000Z","updatedAt":"2026-10-02T10:00:00.000Z","role":"owner"}]"#,
            ))
            .mount(&server)
            .await;
        let list = ws(&server).tenants().list().await.expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Canary Co");
        assert!(list[0].updated_at.is_some());
    }

    #[tokio::test]
    async fn create_and_rename_send_the_e5_bodies() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tenants"))
            .respond_with(json(
                201,
                r#"{"tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
                    "companyId":"0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","tenantName":"New Co"}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/tenants/{TENANT}")))
            .respond_with(json(
                200,
                r#"{"tenant":{"id":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Renamed","updatedAt":"2026-10-03T10:00:00.000Z"}}"#,
            ))
            .mount(&server)
            .await;
        let s = ws(&server);
        let made = s
            .tenants()
            .create(&NewTenant {
                name: "New Co".into(),
                business_type: Some("default".into()),
                coa_template: None,
                system_accounts_only: Some(true),
            })
            .await
            .expect("create");
        assert_eq!(made.name, "New Co");
        assert!(made.company_id.is_some());
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["systemAccountsOnly"], true);
        assert!(body.get("coaTemplate").is_none());

        let at = Timestamp::parse_wire("2026-10-02T10:00:00.000Z").expect("ts");
        let renamed = s
            .tenants()
            .rename(made.tenant_id, "Renamed", at, IdempotencyKey::new())
            .await
            .expect("rename");
        assert_eq!(renamed.name, "Renamed");
        let patches = requests(&server, "PATCH").await;
        let body: Value = serde_json::from_slice(&patches[0].body).expect("json");
        assert_eq!(body["expectedUpdatedAt"], "2026-10-02T10:00:00.000Z");
        assert!(patches[0].headers.get("idempotency-key").is_some());
    }

    #[tokio::test]
    async fn stale_rename_is_a_conflict() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/tenants/{TENANT}")))
            .respond_with(json(
                409,
                r#"{"error":{"message":"changed","code":"STALE"}}"#,
            ))
            .mount(&server)
            .await;
        let e = ws(&server)
            .tenants()
            .rename(
                TenantId::parse(TENANT).expect("id"),
                "x",
                Timestamp::parse_wire("2026-10-02T10:00:00.000Z").expect("ts"),
                IdempotencyKey::new(),
            )
            .await
            .expect_err("stale");
        assert!(matches!(
            e,
            ApiError::Conflict {
                reason: crate::ConflictReason::Stale,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn export_job_and_poll() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/tenants/{TENANT}/archives")))
            .respond_with(json(202, r#"{"jobId":"job_1","archiveId":"a-1"}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tenant-archives/jobs/job_1"))
            .respond_with(json(
                200,
                r#"{"jobId":"job_1","kind":"export","status":"succeeded","progress":100,"step":"done",
                    "result":{"archiveId":"a-1","fileName":"a-1.zip","path":"/tmp/x/a-1.zip","sha256":"ab","bytes":10}}"#,
            ))
            .mount(&server)
            .await;
        let s = ws(&server);
        let req = ArchiveRequest {
            include_attachments: true,
            include_audit: true,
            host: serde_json::json!({"app":"ClientLedger","kind":"backup"}),
            passphrase: Secret::new("pass-canary-123"),
        };
        let job = s
            .tenants()
            .export_archive(TenantId::parse(TENANT).expect("id"), &req)
            .await
            .expect("job");
        assert_eq!(job.job_id.as_str(), "job_1");
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["passphrase"], "pass-canary-123");
        assert_eq!(body["host"]["kind"], "backup");
        assert!(!format!("{req:?}").contains("pass-canary"));
        let st = s.tenants().archive_job(&job.job_id).await.expect("status");
        assert_eq!(st.status, JobState::Succeeded);
        assert!(
            st.import_result().is_none(),
            "an export job has no import result"
        );
        let r = st.export_result().expect("result");
        assert_eq!(r.file_name.as_str(), "a-1.zip");
        assert_eq!(r.path.as_deref(), Some(Path::new("/tmp/x/a-1.zip")));
    }

    #[tokio::test]
    async fn a_failed_job_carries_its_error_code() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tenant-archives/jobs/bad"))
            .respond_with(json(
                200,
                r#"{"jobId":"bad","kind":"import","status":"failed","progress":30,"step":"verify",
                    "error":{"code":"ARCHIVE_PASSPHRASE_INVALID","message":"wrong passphrase"}}"#,
            ))
            .mount(&server)
            .await;
        let st = ws(&server)
            .tenants()
            .archive_job(&JobId::parse("bad").expect("id"))
            .await
            .expect("status");
        assert_eq!(st.status, JobState::Failed);
        assert_eq!(st.error_code(), "ARCHIVE_PASSPHRASE_INVALID");
        assert!(st.export_result().is_none() && st.import_result().is_none());
    }

    #[tokio::test]
    async fn job_unknown_after_restart_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tenant-archives/jobs/gone"))
            .respond_with(json(
                404,
                r#"{"error":{"message":"unknown","code":"JOB_UNKNOWN"}}"#,
            ))
            .mount(&server)
            .await;
        let e = ws(&server)
            .tenants()
            .archive_job(&JobId::parse("gone").expect("id"))
            .await
            .expect_err("404");
        assert!(matches!(e, ApiError::NotFound { .. }));
    }

    #[tokio::test]
    async fn import_sends_an_absolute_path_and_reads_the_job_result() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tenant-archives/import"))
            .respond_with(json(202, r#"{"jobId":"imp_1"}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tenant-archives/jobs/imp_1"))
            .respond_with(json(
                200,
                r##"{"jobId":"imp_1","kind":"import","status":"succeeded","progress":100,"step":"done",
                    "result":{"tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Restored","scratch":false,
                    "archiveId":"a-9","sourceTenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02",
                    "verification":{"tables":42,"rows":900,"contentEqual":true,"trialBalanceEqual":true,"files":3,
                    "identity":{"matched":1,"created":0},"warnings":[]},"warnings":[],
                    "host":{"app":"ClientLedger","accent":"#3E7CB1"}}}"##,
            ))
            .mount(&server)
            .await;
        let s = ws(&server);
        let abs = if cfg!(windows) {
            PathBuf::from(r"C:\backups\co.clbackup")
        } else {
            PathBuf::from("/backups/co.clbackup")
        };
        let job = s
            .tenants()
            .import_archive(&ImportRequest {
                path: abs,
                passphrase: Secret::new("pw"),
                scratch: false,
            })
            .await
            .expect("import");
        let st = s.tenants().archive_job(&job.job_id).await.expect("job");
        assert_eq!(st.status, JobState::Succeeded);
        let r = st.import_result().expect("import result");
        assert_eq!(r.archive_id.as_deref(), Some("a-9"));
        assert_eq!(
            r.host.as_ref().map(|h| h["accent"].clone()),
            Some(serde_json::json!("#3E7CB1"))
        );
        let v = r.verification.expect("verification");
        assert_eq!(v.trial_balance_equal, Some(true));
        assert_eq!(v.other["identity"]["matched"], 1);
        let before = server.received_requests().await.expect("r").len();
        let e = s
            .tenants()
            .import_archive(&ImportRequest {
                path: PathBuf::from("relative.clbackup"),
                passphrase: Secret::new("pw"),
                scratch: false,
            })
            .await
            .expect_err("relative");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
        assert_eq!(server.received_requests().await.expect("r").len(), before);
    }

    #[tokio::test]
    async fn delete_refuses_without_a_backup_and_from_inside_the_company() {
        let server = MockServer::start().await;
        let s = ws(&server);
        let id = TenantId::parse(TENANT).expect("id");
        let abs = if cfg!(windows) {
            PathBuf::from(r"C:\b\a.clbackup")
        } else {
            PathBuf::from("/b/a.clbackup")
        };
        let good = DeleteRequest {
            confirm_name: "Canary Co".into(),
            archive_path: abs.clone(),
            archive_sha256: "a".repeat(64),
            passphrase: Secret::new("pw"),
            closing_password: None,
        };
        let e = s
            .tenants()
            .delete(
                id,
                &DeleteRequest {
                    archive_sha256: "nope".into(),
                    ..good.clone()
                },
            )
            .await
            .expect_err("no sha");
        assert_eq!(e, ApiError::Local(LocalValidation::BackupRequired));
        let inside = company_session(&server);
        let e = inside
            .tenants()
            .delete(id, &good)
            .await
            .expect_err("inside");
        assert!(matches!(e, ApiError::Conflict { .. }), "{e:?}");
        assert!(server.received_requests().await.expect("r").is_empty());

        Mock::given(method("POST"))
            .and(path(format!("/api/v1/tenants/{TENANT}/delete")))
            .respond_with(json(
                200,
                r#"{"tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","archiveId":"a-1"}"#,
            ))
            .mount(&server)
            .await;
        let receipt = s.tenants().delete(id, &good).await.expect("deleted");
        assert_eq!(receipt.archive_id.as_deref(), Some("a-1"));
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["confirmName"], "Canary Co");
        assert_eq!(body["archive"]["sha256"], "a".repeat(64));
        assert!(body.get("closingPassword").is_none());
    }

    #[test]
    fn ids_and_file_names_are_path_safe() {
        assert!(JobId::parse("ok_1-2").is_ok());
        assert!(JobId::parse("../x").is_err());
        assert!(JobId::parse("").is_err());
        assert!(ArchiveFileName::parse("a-1.zip").is_ok());
        assert!(ArchiveFileName::parse("..").is_err());
        assert!(ArchiveFileName::parse("a/b.zip").is_err());
    }
}
