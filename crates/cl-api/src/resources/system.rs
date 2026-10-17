//! Engine-level operations: the `OpenAPI` document and the SMTP connection
//! test (P05 plan §2.8 `system`). `/health` and `/desktop/info` are on
//! [`Client`].
//!
//! The SMTP test is the route the offline contract test uses: with
//! `CL_OFFLINE=on` the engine answers 503 `OFFLINE_UNAVAILABLE` with
//! `feature: "smtp"`, which maps to `OfflineUnavailable` and is never
//! retried. It is super-admin only and refused from the workspace, so it
//! needs an open company.

use cl_core::Secret;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::StaticAuth;
use crate::resources::operations::{OPENAPI, TEST_SMTP};
use crate::session::{Client, Session, decode, json_body};

/// Input of [`Session::test_smtp`] (`POST /api/v1/admin/test-smtp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmtpTest {
    /// Server host name.
    pub host: String,
    /// Server port.
    pub port: u16,
    /// Sign-in user name (may be empty).
    pub username: String,
    /// Sign-in password (never logged; `Debug` is redacted by `Secret`).
    pub password: Option<Secret>,
    /// Sender address.
    pub from: String,
    /// Sender display name.
    pub from_name: Option<String>,
    /// Where to send a test message, if anywhere.
    pub test_email: Option<String>,
}

impl Serialize for SmtpTest {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("SmtpTest", 7)?;
        st.serialize_field("host", &self.host)?;
        st.serialize_field("port", &self.port)?;
        st.serialize_field("username", &self.username)?;
        st.serialize_field("password", &self.password.as_ref().map(Secret::expose))?;
        st.serialize_field("from", &self.from)?;
        if let Some(name) = &self.from_name {
            st.serialize_field("fromName", name)?;
        }
        if let Some(to) = &self.test_email {
            st.serialize_field("testEmail", to)?;
        }
        st.end()
    }
}

/// Result of an SMTP connection test.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct SmtpTestResult {
    /// The server accepted the connection (and the test message).
    pub success: bool,
    /// The server's error text (may name hosts or addresses: never log it).
    #[serde(default)]
    pub error: Option<String>,
}

impl Session {
    /// Tests an SMTP server. Online-only: offline it fails at once with
    /// `OfflineUnavailable { feature: "smtp" }`.
    ///
    /// # Errors
    /// `OfflineUnavailable`, `FeatureNotEnabled`, `NoCompanyOpen`, or any
    /// [`ApiError`].
    pub async fn test_smtp(&self, test: &SmtpTest) -> Result<SmtpTestResult, ApiError> {
        let call = Call::at(Method::Post, TEST_SMTP.route()).json_body(json_body(test)?);
        let reply = self.send(&TEST_SMTP, call, None).await?;
        decode(&reply, "smtp test")
    }
}

impl Client {
    /// The engine's `OpenAPI` document as raw JSON bytes (for the spec
    /// fixtures; P05 plan §2.8).
    ///
    /// # Errors
    /// Any [`ApiError`] of the request.
    pub async fn openapi_spec(&self) -> Result<Vec<u8>, ApiError> {
        let reply = self
            .exec()
            .run(
                &OPENAPI,
                Call::at(Method::Get, OPENAPI.route()),
                &StaticAuth(crate::client::AttemptAuth::default()),
                None,
            )
            .await?;
        Ok(reply.body.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn smtp() -> SmtpTest {
        SmtpTest {
            host: "smtp.example.test".into(),
            port: 587,
            username: "me".into(),
            password: Some(Secret::new("smtp-pw-canary")),
            from: "me@example.test".into(),
            from_name: None,
            test_email: None,
        }
    }

    #[tokio::test]
    async fn offline_smtp_test_is_typed_and_sent_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/admin/test-smtp"))
            .respond_with(json(
                503,
                r#"{"error":{"message":"Email needs the internet","code":"OFFLINE_UNAVAILABLE","feature":"smtp"}}"#,
            ))
            .mount(&server)
            .await;
        let e = company_session(&server)
            .test_smtp(&smtp())
            .await
            .expect_err("offline");
        assert_eq!(
            e,
            ApiError::OfflineUnavailable {
                feature: Some("smtp".into())
            }
        );
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 1, "never retried");
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["port"], 587);
        assert_eq!(body["password"], "smtp-pw-canary");
        assert!(body.get("fromName").is_none());
        assert!(!format!("{:?}", smtp()).contains("smtp-pw-canary"));
    }

    #[tokio::test]
    async fn smtp_success_and_openapi_bytes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/admin/test-smtp"))
            .respond_with(json(200, r#"{"success":true}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/openapi.json"))
            .respond_with(json(200, r#"{"openapi":"3.0.0"}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let r = s.test_smtp(&smtp()).await.expect("ok");
        assert!(r.success);
        assert_eq!(r.error, None);
        let spec = s.client().openapi_spec().await.expect("spec");
        assert_eq!(spec, br#"{"openapi":"3.0.0"}"#);
        let get = requests(&server, "GET").await;
        assert!(get[0].headers.get("authorization").is_none());
    }
}
