//! Shared wire types: engine identity, capabilities and health.

use std::collections::BTreeSet;

use cl_core::Timestamp;
use cl_core::ids::TenantId;
use serde::Deserialize;

/// Capability strings the engine advertises in `/desktop/info.capabilities`
/// (introduced by U-21; a missing array means none).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(BTreeSet<String>);

impl Capabilities {
    /// U-17 record versions and `If-Match`.
    pub const RECORD_VERSION: &'static str = "record-version-v1";
    /// U-18 request idempotency.
    pub const IDEMPOTENCY: &'static str = "idempotency-v1";
    /// U-19 strict money validation.
    pub const MONEY_STRICT: &'static str = "money-strict-v1";
    /// U-21 stable error codes.
    pub const ERROR_CODES: &'static str = "error-codes-v1";
    /// E5a `GET /api/v1/tenants`.
    pub const TENANTS_LIST: &'static str = "tenants-list-v1";
    /// U-15 closing-date password.
    pub const CLOSING_PASSWORD: &'static str = "closing-password-v1";

    /// True when the engine advertises `name`.
    #[must_use]
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }

    /// Every advertised capability.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl<S: Into<String>> FromIterator<S> for Capabilities {
    fn from_iter<I: IntoIterator<Item = S>>(iter: I) -> Self {
        Capabilities(iter.into_iter().map(Into::into).collect())
    }
}

/// `GET /api/v1/desktop/info` (the fields cl-api uses; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInfo {
    /// Engine version string.
    #[serde(default)]
    pub version: Option<String>,
    /// Engine source commit.
    #[serde(default)]
    pub engine_commit: Option<String>,
    /// Whether a user exists (`null` while the database is unavailable).
    #[serde(default)]
    pub initialized: Option<bool>,
    /// The hidden owner workspace tenant (Q-P02h); `null` on older engines.
    #[serde(default)]
    pub workspace_tenant_id: Option<TenantId>,
    /// Advertised capabilities.
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// `GET /health` (desktop fields; others are ignored).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`, `degraded`, …
    #[serde(default)]
    pub status: Option<String>,
    /// Internet reachability; `None` means unknown, not offline (E4).
    #[serde(default)]
    pub online: Option<bool>,
    /// When reachability was last checked.
    #[serde(default)]
    pub last_online_check: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_info_tolerates_missing_and_extra_fields() {
        let info: DesktopInfo = serde_json::from_str(
            r#"{"mode":"desktop","version":"1.2.3","engineCommit":null,"offline":"auto",
                "initialized":true,"workspaceTenantId":"6f1c1e0e-1d7f-4c55-9e3e-1b2f7f3f9a10",
                "capabilities":["idempotency-v1","error-codes-v1"]}"#,
        )
        .expect("info");
        assert!(info.capabilities.has(Capabilities::IDEMPOTENCY));
        assert!(!info.capabilities.has(Capabilities::RECORD_VERSION));
        assert_eq!(info.capabilities.iter().count(), 2);
        assert!(info.workspace_tenant_id.is_some());
        let old: DesktopInfo = serde_json::from_str(r#"{"workspaceTenantId":null}"#).expect("old");
        assert_eq!(old.capabilities, Capabilities::default());
        assert_eq!(old.workspace_tenant_id, None);
    }

    #[test]
    fn health_online_null_is_unknown() {
        let h: Health = serde_json::from_str(r#"{"status":"ok","online":null}"#).expect("h");
        assert_eq!(h.online, None);
        let h: Health = serde_json::from_str(
            r#"{"status":"ok","online":false,"lastOnlineCheck":"2026-10-15T08:00:00.000Z"}"#,
        )
        .expect("h");
        assert_eq!(h.online, Some(false));
        assert!(h.last_online_check.is_some());
        let caps: Capabilities = ["a", "b"].into_iter().collect();
        assert!(caps.has("a"));
    }
}
