//! A01 WP-E boundary: where the Document intake panel reads and writes
//! the company's intake folders.
//!
//! - **Demo**: the demo engine store (`company::demo::intake`), answered
//!   at once on the UI thread.
//! - **Engine**: `GET`/`PUT /api/v1/company/intake-settings` (U-93), on
//!   the request queue. Only reached when the engine advertises
//!   [`CAPABILITY`]; the pinned engine does not yet, so the panel shows
//!   "Document intake needs the latest engine" (Q-A01q).
//!
//! The engine side is `cl_api`'s `session.intake()` (A01 WP-E). A `409
//! STALE` arrives as `ApiError::Conflict { reason: Stale, .. }`
//! ([`is_stale`]).

use cl_api::{ApiError, Capabilities, ConflictReason, Session};
use cl_company::CompanyError;
use cl_core::ids::TenantId;
use cl_core::intake::IntakeFolder;

use crate::company::CompanyLink;
use crate::company::demo::intake::DemoIntakeError;

/// The engine capability for intake settings (U-93).
pub const CAPABILITY: &str = Capabilities::INTAKE_SETTINGS;

/// The engine role that is ClientLedger's Admin until P29 (Q-A01k).
pub const ADMIN_ROLE: &str = "owner";

/// The company's folders as the engine stores them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stored {
    /// The folders, in order.
    pub folders: Vec<IntakeFolder>,
    /// The engine's stamp, echoed back as `expectedUpdatedAt`.
    pub updated_at: Option<String>,
}

/// What opening the panel read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fetched {
    /// The folders and stamp.
    pub stored: Stored,
    /// The user's engine role in the company (`None`: not known).
    pub role: Option<String>,
}

/// Whether this company can keep intake folders at all.
#[must_use]
pub fn supported(link: &CompanyLink) -> bool {
    link.is_demo() || link.capabilities().has(CAPABILITY)
}

/// The engine's folders (`GET /company/intake-settings`).
///
/// # Errors
/// Any [`ApiError`].
pub async fn engine_get(session: &Session) -> Result<Stored, ApiError> {
    session.intake().get().await.map(stored)
}

fn stored(s: cl_api::IntakeSettings) -> Stored {
    Stored {
        folders: s.folders,
        updated_at: s.updated_at,
    }
}

/// Replaces the engine's folders when its stamp is still `expected`
/// (`PUT /company/intake-settings`).
///
/// # Errors
/// Any [`ApiError`]; a changed stamp is the `409 STALE` [`is_stale`] knows.
pub async fn engine_put(
    session: &Session,
    folders: &[IntakeFolder],
    expected: Option<&str>,
) -> Result<Stored, ApiError> {
    session.intake().put(folders, expected).await.map(stored)
}

/// The user's engine role in `tenant`, from the company list.
pub async fn engine_role(session: &Session, tenant: TenantId) -> Option<String> {
    session
        .companies()
        .await
        .ok()?
        .into_iter()
        .find(|c| c.tenant_id == tenant)
        .map(|c| c.role)
}

/// The `409 STALE` answer to a write: someone saved the folders since
/// they were read.
#[must_use]
pub fn is_stale(e: &CompanyError) -> bool {
    matches!(
        e,
        CompanyError::Api(ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        })
    )
}

/// The demo engine's folders and the demo user's role.
#[must_use]
pub fn demo_get(link: &CompanyLink, tenant: TenantId) -> Option<Fetched> {
    let demo = link.demo_state()?;
    let s = demo.intake.get(tenant);
    Some(Fetched {
        stored: Stored {
            folders: s.folders,
            updated_at: s.updated_at,
        },
        role: Some(demo.intake.role()),
    })
}

/// Why a demo write failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoPutError {
    /// No demo engine.
    NoDemo,
    /// `409 STALE`.
    Stale,
    /// `400 VALIDATION`.
    Invalid,
}

/// Writes the folders to the demo engine.
///
/// # Errors
/// [`DemoPutError`].
pub fn demo_put(
    link: &CompanyLink,
    tenant: TenantId,
    folders: Vec<IntakeFolder>,
    expected: Option<&str>,
) -> Result<Stored, DemoPutError> {
    let mut demo = link.demo_state().ok_or(DemoPutError::NoDemo)?;
    match demo.intake.put(tenant, folders, expected) {
        Ok(s) => Ok(Stored {
            folders: s.folders,
            updated_at: s.updated_at,
        }),
        Err(DemoIntakeError::Stale) => Err(DemoPutError::Stale),
        Err(DemoIntakeError::Invalid(_)) => Err(DemoPutError::Invalid),
    }
}
