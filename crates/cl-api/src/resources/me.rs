//! Who the signed-in user is in the open company (`GET /auth/me`, P29).
//!
//! The engine answers with the effective permission map of the active
//! company (`permissions`: resource key → `none` / `view` / `full`), the
//! companies the user can reach with the role in each, and the user's super
//! admin flag. The map's words are kept as text: `cl-model` reads them
//! (unknown keys ignored, unknown words never grant). The user's own
//! `user.role` is the role in the user's home company, so the role in the
//! open company comes from the `accessibleTenants` row of `activeTenantId`.
//!
//! Nothing here is owner data except the email, which is not kept.

use std::collections::BTreeMap;

use cl_core::ids::TenantId;
use serde::Deserialize;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::resources::operations::{AUTH_ME, SENSITIVE_DOORS};
use crate::session::{Session, decode};

/// What `/auth/me` says about the user in the active company.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Me {
    /// The company the session is in (`activeTenantId`).
    pub active_tenant: Option<TenantId>,
    /// The role there (`owner`, `bookkeeper`, `readonly`, …); empty when
    /// the engine did not list the company.
    pub role: String,
    /// The user is the super admin (the desktop's first user).
    pub is_super_admin: bool,
    /// The effective map, as the engine words it.
    pub permissions: BTreeMap<String, String>,
    /// U-230: the user must choose a new password before anything else
    /// works (false on engines without it).
    pub password_change_required: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    #[serde(default)]
    user: Option<WireUser>,
    #[serde(default)]
    accessible_tenants: Vec<WireTenant>,
    #[serde(default)]
    active_tenant_id: Option<TenantId>,
    #[serde(default)]
    permissions: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireUser {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    is_super_admin: Option<bool>,
    #[serde(default)]
    password_change_required: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireTenant {
    tenant_id: TenantId,
    #[serde(default)]
    role: Option<String>,
}

impl Session {
    /// The signed-in user's role and rights in the open company.
    ///
    /// # Errors
    /// Any [`ApiError`] of the request; `Decode` for an unreadable answer.
    pub async fn me(&self) -> Result<Me, ApiError> {
        let reply = self
            .send(&AUTH_ME, Call::at(Method::Get, AUTH_ME.route()), None)
            .await?;
        let wire: Wire = decode(&reply, "who am I")?;
        Ok(from_wire(wire))
    }

    /// The routes the engine still refuses the signed-in user because an
    /// account is hidden from them (`GET /sensitive-doors`, P29 review B2).
    /// `Ok(None)` on an engine without `sensitive-accounts-doors-v1`: the
    /// caller behaves as before.
    ///
    /// # Errors
    /// Any [`ApiError`] of the request; `Decode` for an unreadable answer.
    pub async fn sensitive_doors(
        &self,
    ) -> Result<Option<cl_core::sensitive_doors::SensitiveDoors>, ApiError> {
        if !self
            .client()
            .capabilities()
            .has(crate::wire::Capabilities::SENSITIVE_DOORS)
        {
            return Ok(None);
        }
        let reply = self
            .send(
                &SENSITIVE_DOORS,
                Call::at(Method::Get, SENSITIVE_DOORS.route()),
                None,
            )
            .await?;
        decode(&reply, "sensitive doors").map(Some)
    }
}

fn from_wire(wire: Wire) -> Me {
    let is_super_admin = wire
        .user
        .as_ref()
        .and_then(|u| u.is_super_admin)
        .unwrap_or(false);
    let role = wire
        .active_tenant_id
        .and_then(|t| {
            wire.accessible_tenants
                .iter()
                .find(|a| a.tenant_id == t)
                .and_then(|a| a.role.clone())
        })
        // An engine that lists no tenants: the user's own role.
        .or_else(|| {
            wire.accessible_tenants
                .is_empty()
                .then(|| wire.user.as_ref().and_then(|u| u.role.clone()))
                .flatten()
        })
        .unwrap_or_default();
    let password_change_required = wire
        .user
        .as_ref()
        .and_then(|u| u.password_change_required)
        .unwrap_or(false);
    let permissions = wire
        .permissions
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_owned())))
        .collect();
    Me {
        active_tenant: wire.active_tenant_id,
        role,
        is_super_admin,
        permissions,
        password_change_required,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const T1: &str = "d0000000-0000-4000-8000-000000000001";
    const T2: &str = "d0000000-0000-4000-8000-000000000002";

    #[tokio::test]
    async fn the_role_is_the_open_companys() {
        let server = MockServer::start().await;
        let body = format!(
            r#"{{"user":{{"id":"u","email":"a@example.test","role":"owner","isSuperAdmin":false}},
               "accessibleTenants":[{{"tenantId":"{T1}","tenantName":"Home","role":"owner"}},
                                    {{"tenantId":"{T2}","tenantName":"Other","role":"bookkeeper"}}],
               "activeTenantId":"{T2}",
               "permissions":{{"bills":"full","invoices":"none","reports":"view","future_key":"full","odd":3}}}}"#
        );
        Mock::given(method("GET"))
            .and(path("/api/v1/auth/me"))
            .respond_with(json(200, &body))
            .mount(&server)
            .await;
        let me = company_session(&server).me().await.expect("me");
        assert_eq!(me.role, "bookkeeper");
        assert!(!me.is_super_admin);
        assert_eq!(me.active_tenant, TenantId::parse(T2).ok());
        assert_eq!(
            me.permissions.get("bills").map(String::as_str),
            Some("full")
        );
        assert_eq!(
            me.permissions.get("future_key").map(String::as_str),
            Some("full")
        );
        assert!(
            !me.permissions.contains_key("odd"),
            "non-text words dropped"
        );
        assert!(!format!("{me:?}").contains("example.test"));
    }

    #[test]
    fn an_engine_without_tenants_uses_the_users_role() {
        let wire: Wire = serde_json::from_str(
            r#"{"user":{"role":"readonly","isSuperAdmin":true},"permissions":{}}"#,
        )
        .expect("wire");
        let me = from_wire(wire);
        assert_eq!(me.role, "readonly");
        assert!(me.is_super_admin);
        assert_eq!(me.active_tenant, None);
    }

    #[tokio::test]
    async fn a_403_is_counted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(json(
                403,
                r#"{"error":{"message":"no","code":"PERMISSION_DENIED"}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        assert_eq!(s.forbidden_seen(), 0);
        let _ = s.me().await;
        assert_eq!(s.forbidden_seen(), 1);
    }
}
