//! Per-company SMTP settings (P09 step 4, Q-34): `GET/PUT
//! /api/v1/company/smtp` and `POST /api/v1/company/smtp/test`.
//!
//! The engine's writer accepts an `smtpPass` column; the desktop never
//! sends it (the password lives in the keychain and reaches the engine per
//! session, U-13 / `session_secrets`), and the reader never surfaces it:
//! [`SmtpSettings`] has no password field, only `password_configured`. The
//! connection test takes the password in its body (the engine does not
//! keep it) and is credential-carrying, so the listener is re-verified.

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{COMPANY_SMTP, COMPANY_SMTP_PUT, COMPANY_SMTP_TEST};
use crate::resources::system::{SmtpTest, SmtpTestResult};
use crate::session::{Session, decode, json_body};

/// `GET /api/v1/company/smtp` (the password column is never read).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SmtpSettings {
    /// Server host name (empty when not set).
    pub smtp_host: String,
    /// Port.
    pub smtp_port: u16,
    /// Sign-in user name.
    pub smtp_user: String,
    /// A password is stored in the engine (server mode) or was given this
    /// session (desktop, U-13).
    pub password_configured: bool,
    /// Sender address.
    pub smtp_from: String,
    /// Sender display name.
    pub smtp_from_name: String,
    /// A host is set.
    pub configured: bool,
}

/// What Preferences writes (`PUT /api/v1/company/smtp`). No password.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmtpSettingsUpdate {
    /// Server host name (1-255 characters).
    pub host: String,
    /// Port (1-65535).
    pub port: u16,
    /// Sign-in user name (may be empty).
    pub user: String,
    /// Sender address.
    pub from: String,
    /// Sender display name (may be empty).
    pub from_name: String,
}

impl Serialize for SmtpSettingsUpdate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("SmtpSettingsUpdate", 5)?;
        st.serialize_field("smtpHost", &self.host)?;
        st.serialize_field("smtpPort", &self.port)?;
        st.serialize_field("smtpUser", &self.user)?;
        st.serialize_field("smtpFrom", &self.from)?;
        st.serialize_field("smtpFromName", &self.from_name)?;
        st.end()
    }
}

impl Session {
    /// The open company's SMTP settings.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn smtp_settings(&self) -> Result<SmtpSettings, ApiError> {
        let reply = self
            .send(
                &COMPANY_SMTP,
                Call::at(Method::Get, COMPANY_SMTP.route()),
                None,
            )
            .await?;
        decode(&reply, "smtp settings")
    }

    /// Saves host, port, user and sender (never a password).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty host, a zero port or a sender
    /// without `@` (nothing sent); `Validation`; any [`ApiError`].
    pub async fn update_smtp_settings(
        &self,
        update: &SmtpSettingsUpdate,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        if update.host.trim().is_empty()
            || update.host.len() > 255
            || update.port == 0
            || !update.from.contains('@')
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Put, COMPANY_SMTP_PUT.route()).json_body(json_body(update)?);
        call.idempotency_key = Some(key);
        self.send(&COMPANY_SMTP_PUT, call, None).await.map(drop)
    }

    /// Tests a connection with the given settings and password (the engine
    /// keeps nothing). Online-only: offline it fails at once with
    /// `OfflineUnavailable { feature: "smtp" }`.
    ///
    /// # Errors
    /// `OfflineUnavailable`, `NoCompanyOpen`, any [`ApiError`].
    pub async fn test_company_smtp(&self, test: &SmtpTest) -> Result<SmtpTestResult, ApiError> {
        let call = Call::at(Method::Post, COMPANY_SMTP_TEST.route()).json_body(json_body(test)?);
        let reply = self.send(&COMPANY_SMTP_TEST, call, None).await?;
        decode(&reply, "smtp test")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use cl_core::Secret;
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    #[tokio::test]
    async fn read_write_and_test_never_carry_a_stored_password() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/smtp"))
            .respond_with(json(
                200,
                r#"{"smtpHost":"smtp.example.test","smtpPort":587,"smtpUser":"books","smtpPass":"",
                    "passwordConfigured":true,"smtpFrom":"books@example.test","smtpFromName":"Ozark","configured":true}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company/smtp"))
            .respond_with(json(200, r#"{"message":"SMTP settings saved"}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/smtp/test"))
            .respond_with(json(200, r#"{"success":false,"error":"ECONNREFUSED"}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let st = s.smtp_settings().await.expect("get");
        assert_eq!(st.smtp_port, 587);
        assert!(st.password_configured && st.configured);
        assert!(!format!("{st:?}").contains("smtpPass"));
        let u = SmtpSettingsUpdate {
            host: "smtp.example.test".into(),
            port: 465,
            user: "books".into(),
            from: "books@example.test".into(),
            from_name: String::new(),
        };
        s.update_smtp_settings(&u, IdempotencyKey::new())
            .await
            .expect("put");
        let puts = requests(&server, "PUT").await;
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["smtpPort"], 465);
        assert!(
            body.get("smtpPass").is_none(),
            "no password column is ever written"
        );
        assert!(puts[0].headers.get("idempotency-key").is_some());
        let bad = SmtpSettingsUpdate {
            from: "nope".into(),
            ..u
        };
        assert_eq!(
            s.update_smtp_settings(&bad, IdempotencyKey::new())
                .await
                .expect_err("from"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        assert_eq!(requests(&server, "PUT").await.len(), 1);
        let r = s
            .test_company_smtp(&SmtpTest {
                host: "smtp.example.test".into(),
                port: 587,
                username: "books".into(),
                password: Some(Secret::new("smtp-pw-canary")),
                from: "books@example.test".into(),
                from_name: None,
                test_email: None,
            })
            .await
            .expect("test");
        assert!(!r.success);
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["password"], "smtp-pw-canary");
    }
}
