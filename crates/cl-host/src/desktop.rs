//! The engine's first-run endpoints (`/api/v1/desktop/info` and `/initialize`).
//!
//! No UI is built here; `Supervisor::initialize_engine` is the host-side hook
//! the Open Company window (P08) will call.

use crate::http;
use cl_pg::Secret;
use serde_json::{Value, json};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_mins(1);

/// `GET /api/v1/desktop/info`, the parts the host cares about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopInfo {
    /// A user exists, so first-run setup is done.
    pub initialized: bool,
    /// The hidden owner workspace tenant, once initialized (Q-P02h).
    pub workspace_tenant_id: Option<String>,
    /// Whether the engine says its database role is a superuser (it must not be); `None` when unknown.
    pub db_superuser: Option<bool>,
    /// Engine source commit, when reported.
    pub engine_commit: Option<String>,
}

/// The owner account created at first run.
#[derive(Clone)]
pub struct AdminAccount {
    /// Login email.
    pub email: String,
    /// Name shown in the app.
    pub display_name: String,
    /// At least 12 characters. Never logged.
    pub password: Secret,
}

impl std::fmt::Debug for AdminAccount {
    /// Shows nothing personal: no email, no name, no password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminAccount").finish_non_exhaustive()
    }
}

/// The first company, created together with the owner (optional).
#[derive(Debug, Clone, Default)]
pub struct CompanySeed {
    /// Company name.
    pub name: String,
    /// Chart-of-accounts template key, for example `default`.
    pub business_type: Option<String>,
    /// Entity type such as `sole_prop`.
    pub entity_type: Option<String>,
    /// Industry, free text.
    pub industry: Option<String>,
}

/// What first-run setup created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Initialized {
    /// The hidden workspace tenant that holds user rows.
    pub workspace_tenant_id: Option<String>,
    /// The owner's user id.
    pub user_id: String,
    /// The first company, when one was requested and could be created.
    pub company_tenant_id: Option<String>,
    /// Non-fatal notes such as `COMPANY_NOT_CREATED`.
    pub warnings: Vec<String>,
    /// The owner's recovery key, returned once by an engine with U-23
    /// (`recovery-key-v1`); `None` before that. Never logged.
    pub recovery_key: Option<Secret>,
}

/// Why a desktop endpoint call failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DesktopError {
    /// Could not talk to the engine.
    #[error("could not reach the engine: {0}")]
    Unreachable(String),
    /// The engine refused with an error code (never the message text).
    #[error("the engine refused the request ({status}): {code}")]
    Refused {
        /// HTTP status.
        status: u16,
        /// The engine's error code, such as `ALREADY_INITIALIZED`.
        code: String,
    },
    /// The answer was not understood.
    #[error("the engine's answer was not understood")]
    Malformed,
    /// The engine is up but cannot reach its database.
    #[error("the engine cannot reach its database")]
    EngineDatabaseUnavailable,
    /// The request failed local validation before being sent.
    #[error("{0}")]
    Invalid(String),
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/code")
                .or_else(|| v.get("code"))
                .and_then(Value::as_str)
                .map(|c| c.chars().take(60).collect::<String>())
        })
        .unwrap_or_else(|| "UNKNOWN".into())
}

/// Ask the engine whether first-run setup has happened.
///
/// # Errors
/// See [`DesktopError`].
pub fn info(port: u16, token: &Secret) -> Result<DesktopInfo, DesktopError> {
    let r = http::request(port, "GET", "/api/v1/desktop/info", token, None, TIMEOUT)
        .map_err(|e| DesktopError::Unreachable(e.kind().to_string()))?;
    if r.status != 200 {
        return Err(DesktopError::Refused {
            status: r.status,
            code: error_code(&r.body),
        });
    }
    parse_info(&r.body)
}

pub(crate) fn parse_info(body: &str) -> Result<DesktopInfo, DesktopError> {
    let v: Value = serde_json::from_str(body).map_err(|_| DesktopError::Malformed)?;
    let v = v.get("data").unwrap_or(&v);
    Ok(DesktopInfo {
        initialized: match v.get("initialized") {
            Some(Value::Bool(b)) => *b,
            // `null`: the engine could not read its own database.
            Some(Value::Null) => return Err(DesktopError::EngineDatabaseUnavailable),
            _ => return Err(DesktopError::Malformed),
        },
        workspace_tenant_id: v
            .get("workspaceTenantId")
            .and_then(Value::as_str)
            .map(str::to_string),
        db_superuser: v.get("dbSuperuser").and_then(Value::as_bool),
        engine_commit: v
            .get("engineCommit")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Build the JSON body for `/initialize`.
pub(crate) fn initialize_body(
    admin: &AdminAccount,
    company: Option<&CompanySeed>,
) -> Result<String, DesktopError> {
    if admin.password.expose().chars().count() < 12 {
        return Err(DesktopError::Invalid(
            "the password must be at least 12 characters".into(),
        ));
    }
    if admin.email.trim().is_empty() || admin.display_name.trim().is_empty() {
        return Err(DesktopError::Invalid(
            "an email address and a name are required".into(),
        ));
    }
    let mut body = json!({
        "admin": {
            "email": admin.email.trim(),
            "displayName": admin.display_name.trim(),
            "password": admin.password.expose(),
        }
    });
    if let Some(c) = company {
        if c.name.trim().is_empty() {
            return Err(DesktopError::Invalid("the company needs a name".into()));
        }
        let mut company_json = json!({ "name": c.name.trim() });
        for (key, value) in [
            ("businessType", &c.business_type),
            ("entityType", &c.entity_type),
            ("industry", &c.industry),
        ] {
            if let Some(v) = value {
                company_json[key] = Value::String(v.clone());
            }
        }
        body["company"] = company_json;
    }
    Ok(body.to_string())
}

/// `POST /api/v1/desktop/initialize`.
///
/// # Errors
/// See [`DesktopError`]; `ALREADY_INITIALIZED` comes back as `Refused`.
pub fn initialize(
    port: u16,
    token: &Secret,
    admin: &AdminAccount,
    company: Option<&CompanySeed>,
) -> Result<Initialized, DesktopError> {
    let body = initialize_body(admin, company)?;
    let r = http::request(
        port,
        "POST",
        "/api/v1/desktop/initialize",
        token,
        Some(&body),
        TIMEOUT,
    )
    .map_err(|e| DesktopError::Unreachable(e.kind().to_string()))?;
    if r.status != 201 && r.status != 200 {
        return Err(DesktopError::Refused {
            status: r.status,
            code: error_code(&r.body),
        });
    }
    parse_initialized(&r.body)
}

pub(crate) fn parse_initialized(body: &str) -> Result<Initialized, DesktopError> {
    let v: Value = serde_json::from_str(body).map_err(|_| DesktopError::Malformed)?;
    let v = v.get("data").unwrap_or(&v);
    let text = |key: &str| v.get(key).and_then(Value::as_str).map(str::to_string);
    Ok(Initialized {
        workspace_tenant_id: text("workspaceTenantId"),
        user_id: text("userId").ok_or(DesktopError::Malformed)?,
        // An engine from before the workspace tenant (Q-P02h) answers
        // `{tenantId, userId}`, where the tenant is the first company.
        company_tenant_id: text("companyTenantId").or_else(|| text("tenantId")),
        warnings: v
            .get("warnings")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        recovery_key: text("recoveryKey").map(Secret::new),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin(pw: &str) -> AdminAccount {
        AdminAccount {
            email: " Owner@Example.com ".into(),
            display_name: "Pat Owner".into(),
            password: Secret::new(pw),
        }
    }

    #[test]
    fn info_is_parsed() {
        let i = parse_info(
            r#"{"mode":"desktop","initialized":true,"workspaceTenantId":"w1","dbSuperuser":false,"engineCommit":"abc"}"#,
        )
        .unwrap();
        assert!(i.initialized);
        assert_eq!(i.workspace_tenant_id.as_deref(), Some("w1"));
        assert_eq!(i.db_superuser, Some(false));
        let before = parse_info(r#"{"initialized":false,"workspaceTenantId":null}"#).unwrap();
        assert!(!before.initialized);
        assert_eq!(before.workspace_tenant_id, None);
        assert_eq!(parse_info("{}"), Err(DesktopError::Malformed));
    }

    #[test]
    fn the_initialize_body_is_trimmed_and_includes_the_company() {
        let company = CompanySeed {
            name: " Acme ".into(),
            business_type: Some("default".into()),
            ..CompanySeed::default()
        };
        let body = initialize_body(&admin("correct horse battery"), Some(&company)).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["admin"]["email"], "Owner@Example.com");
        assert_eq!(v["company"]["name"], "Acme");
        assert_eq!(v["company"]["businessType"], "default");
        assert!(v["company"].get("industry").is_none());
        let none = initialize_body(&admin("correct horse battery"), None).unwrap();
        assert!(!none.contains("company"));
    }

    #[test]
    fn short_passwords_and_blank_names_fail_locally() {
        assert!(matches!(
            initialize_body(&admin("short"), None),
            Err(DesktopError::Invalid(_))
        ));
        let blank = CompanySeed::default();
        assert!(matches!(
            initialize_body(&admin("correct horse battery"), Some(&blank)),
            Err(DesktopError::Invalid(_))
        ));
    }

    #[test]
    fn errors_never_echo_the_password() {
        let err = initialize_body(&admin("tiny"), None).unwrap_err();
        assert!(!err.to_string().contains("tiny"));
        let shown = format!("{:?}", admin("tiny"));
        for private in ["tiny", "owner@example", "Owner@Example", "Pat Owner"] {
            assert!(!shown.contains(private), "{shown}");
        }
    }

    #[test]
    fn the_result_ids_are_parsed() {
        let i = parse_initialized(
            r#"{"workspaceTenantId":"w","userId":"u","companyTenantId":null,"warnings":["COMPANY_NOT_CREATED"]}"#,
        )
        .unwrap();
        assert_eq!(i.workspace_tenant_id.as_deref(), Some("w"));
        assert_eq!(i.company_tenant_id, None);
        assert_eq!(i.warnings, ["COMPANY_NOT_CREATED"]);
        assert_eq!(
            parse_initialized(r#"{"workspaceTenantId":"w"}"#),
            Err(DesktopError::Malformed)
        );
    }

    #[test]
    fn an_engine_from_before_the_workspace_tenant_is_understood() {
        let i = parse_initialized(r#"{"tenantId":"t1","userId":"u1"}"#).unwrap();
        assert_eq!(i.workspace_tenant_id, None);
        assert_eq!(i.company_tenant_id.as_deref(), Some("t1"));
        assert_eq!(i.user_id, "u1");
    }

    #[test]
    fn an_engine_without_its_database_says_so() {
        assert_eq!(
            parse_info(r#"{"initialized":null,"dbSuperuser":null}"#),
            Err(DesktopError::EngineDatabaseUnavailable)
        );
        assert_eq!(
            parse_info(r#"{"initialized":"yes"}"#),
            Err(DesktopError::Malformed)
        );
    }

    #[test]
    fn engine_error_codes_are_extracted_without_the_message() {
        assert_eq!(
            error_code(r#"{"error":{"code":"ALREADY_INITIALIZED","message":"secret detail"}}"#),
            "ALREADY_INITIALIZED"
        );
        assert_eq!(error_code(r#"{"code":"X"}"#), "X");
        assert_eq!(error_code("<html>"), "UNKNOWN");
    }
}
