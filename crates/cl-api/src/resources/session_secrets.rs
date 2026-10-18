//! Desktop session secrets (E6, P02 plan §2.6; U-09, U-13), typed ahead of
//! the engine merge (capability `session-secrets-v1`).
//!
//! The owner's AI keys and SMTP password live in the OS keychain
//! (`cl-settings`). At sign-in, and whenever a key changes, the app hands
//! them to the engine for the life of the process:
//!
//! - `PUT /api/v1/desktop/session-secrets {ai?:{anthropic?:{apiKey},
//!   openai?:{apiKey}, ollama?:{baseUrl, model}}, smtp?:{password, host,
//!   port, security}}` (engine branch `clientledger/ai-desktop`) — kept in
//!   memory by the engine, registered with its log scrubber, never written
//!   to the database; the mail server named with the password is the only
//!   one the session may reach;
//! - `GET` → `{name: {present, fingerprint}}`, never a value;
//! - `DELETE /api/v1/desktop/session-secrets/{name}`.
//!
//! Until the engine advertises the capability every call is refused
//! locally (`Local(SessionSecretsUnavailable)`) and nothing is sent: the
//! keys stay in the keychain and the AI panel says cloud providers are
//! waiting for engine support (Q-P09a).

use std::collections::BTreeMap;

use cl_core::Secret;
use serde::Deserialize;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};

use crate::client::{Call, Method, TimeoutClass};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::{
    SESSION_SECRETS_DELETE, SESSION_SECRETS_GET, SESSION_SECRETS_PUT,
};
use crate::session::{Session, decode, json_body};
use crate::wire::Capabilities;

/// Names of the engine's session-secret slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SecretSlot {
    /// `ai.anthropic`.
    AiAnthropic,
    /// `ai.openai`.
    AiOpenai,
    /// `ai.ollama.baseUrl`.
    AiOllamaBaseUrl,
    /// `smtp.password`.
    SmtpPassword,
}

impl SecretSlot {
    /// Every slot.
    pub const ALL: [SecretSlot; 4] = [
        SecretSlot::AiAnthropic,
        SecretSlot::AiOpenai,
        SecretSlot::AiOllamaBaseUrl,
        SecretSlot::SmtpPassword,
    ];

    /// The engine's name.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            SecretSlot::AiAnthropic => "ai.anthropic",
            SecretSlot::AiOpenai => "ai.openai",
            SecretSlot::AiOllamaBaseUrl => "ai.ollama.baseUrl",
            SecretSlot::SmtpPassword => "smtp.password",
        }
    }
}

/// Ollama connection details (not secret, but set through the same call
/// so the engine can add the address to its egress allowlist).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OllamaTarget {
    /// `http://localhost:11434` or a LAN address.
    pub base_url: String,
    /// Model name.
    pub model: Option<String>,
}

/// How the SMTP connection is secured (`smtp.security`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurityWire {
    /// Implicit TLS (465).
    Tls,
    /// STARTTLS (587).
    Starttls,
    /// Plain (loopback or private relays only).
    None,
}

/// The SMTP password with the mail server it is for.
#[derive(Clone)]
pub struct SmtpSessionSecret {
    /// The password (omitted when only the server changes).
    pub password: Option<Secret>,
    /// Mail server host.
    pub host: String,
    /// Mail server port.
    pub port: u16,
    /// Connection security.
    pub security: SmtpSecurityWire,
}

impl std::fmt::Debug for SmtpSessionSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpSessionSecret")
            .field("password", &self.password.is_some())
            .field("port", &self.port)
            .field("security", &self.security)
            .finish_non_exhaustive()
    }
}

/// What to hand the engine for this session. `None` fields are left as
/// they are.
#[derive(Clone, Default)]
pub struct SessionSecrets {
    /// Anthropic key.
    pub anthropic_key: Option<Secret>,
    /// `OpenAI` key.
    pub openai_key: Option<Secret>,
    /// Ollama address and model.
    pub ollama: Option<OllamaTarget>,
    /// SMTP password and server.
    pub smtp: Option<SmtpSessionSecret>,
}

impl SessionSecrets {
    /// Nothing to send.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anthropic_key.is_none()
            && self.openai_key.is_none()
            && self.ollama.is_none()
            && self.smtp.is_none()
    }
}

impl std::fmt::Debug for SessionSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSecrets")
            .field("anthropic_key", &self.anthropic_key.is_some())
            .field("openai_key", &self.openai_key.is_some())
            .field("ollama", &self.ollama.is_some())
            .field("smtp", &self.smtp.is_some())
            .finish()
    }
}

impl Serialize for SessionSecrets {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Key<'a> {
            api_key: &'a str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Ollama<'a> {
            base_url: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            model: Option<&'a str>,
        }
        #[derive(Serialize)]
        struct Ai<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            anthropic: Option<Key<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            openai: Option<Key<'a>>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ollama: Option<Ollama<'a>>,
        }
        #[derive(Serialize)]
        struct Smtp<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            password: Option<&'a str>,
            host: &'a str,
            port: u16,
            security: SmtpSecurityWire,
        }
        let ai = Ai {
            anthropic: self.anthropic_key.as_ref().map(|k| Key {
                api_key: k.expose(),
            }),
            openai: self.openai_key.as_ref().map(|k| Key {
                api_key: k.expose(),
            }),
            ollama: self.ollama.as_ref().map(|o| Ollama {
                base_url: &o.base_url,
                model: o.model.as_deref(),
            }),
        };
        let has_ai = ai.anthropic.is_some() || ai.openai.is_some() || ai.ollama.is_some();
        let mut st = s.serialize_struct("SessionSecrets", 2)?;
        if has_ai {
            st.serialize_field("ai", &ai)?;
        }
        if let Some(m) = &self.smtp {
            st.serialize_field(
                "smtp",
                &Smtp {
                    password: m.password.as_ref().map(Secret::expose),
                    host: &m.host,
                    port: m.port,
                    security: m.security,
                },
            )?;
        }
        st.end()
    }
}

/// What the engine says about one slot (never the value).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct SecretPresence {
    /// A value is held for this session.
    pub present: bool,
    /// `sha256:<8 hex>` of the value, for "did it change" checks.
    pub fingerprint: Option<String>,
}

/// `GET /api/v1/desktop/session-secrets`.
pub type SecretPresences = BTreeMap<String, SecretPresence>;

impl Session {
    fn require_session_secrets(&self) -> Result<(), ApiError> {
        if self
            .client()
            .capabilities()
            .has(Capabilities::SESSION_SECRETS)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::SessionSecretsUnavailable))
        }
    }

    /// Hands secrets to the engine for this session. Never retried.
    ///
    /// # Errors
    /// `Local(SessionSecretsUnavailable)` without the capability and
    /// `Local(InvalidValue)` for an empty set (nothing sent); `Forbidden`
    /// for a non-admin; any [`ApiError`].
    pub async fn put_session_secrets(
        &self,
        secrets: &SessionSecrets,
    ) -> Result<SecretPresences, ApiError> {
        self.require_session_secrets()?;
        if secrets.is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::at(Method::Put, SESSION_SECRETS_PUT.route())
            .json_body(json_body(secrets)?)
            .timeout(TimeoutClass::Auth);
        let reply = self.send(&SESSION_SECRETS_PUT, call, None).await?;
        decode(&reply, "session secrets")
    }

    /// Which slots hold a value this session.
    ///
    /// # Errors
    /// `Local(SessionSecretsUnavailable)`; any [`ApiError`].
    pub async fn session_secrets(&self) -> Result<SecretPresences, ApiError> {
        self.require_session_secrets()?;
        let reply = self
            .send(
                &SESSION_SECRETS_GET,
                Call::at(Method::Get, SESSION_SECRETS_GET.route()),
                None,
            )
            .await?;
        decode(&reply, "session secrets")
    }

    /// Forgets one slot for this session.
    ///
    /// # Errors
    /// `Local(SessionSecretsUnavailable)`; any [`ApiError`].
    pub async fn delete_session_secret(&self, slot: SecretSlot) -> Result<(), ApiError> {
        self.require_session_secrets()?;
        let path = SESSION_SECRETS_DELETE
            .route()
            .replace("{name}", slot.wire());
        let call = Call::new(Method::Delete, SESSION_SECRETS_DELETE.route(), path)
            .timeout(TimeoutClass::Auth);
        self.send(&SESSION_SECRETS_DELETE, call, None)
            .await
            .map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests, session_with_caps};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    fn all() -> SessionSecrets {
        SessionSecrets {
            anthropic_key: Some(Secret::new("sk-ant-canary-0001")),
            openai_key: Some(Secret::new("sk-oa-canary-0002")),
            ollama: Some(OllamaTarget {
                base_url: "http://localhost:11434".into(),
                model: Some("llama3.1:8b".into()),
            }),
            smtp: Some(SmtpSessionSecret {
                password: Some(Secret::new("smtp-canary-0003")),
                host: "smtp.example.com".into(),
                port: 587,
                security: SmtpSecurityWire::Starttls,
            }),
        }
    }

    #[test]
    fn body_shape_matches_the_contract_and_debug_hides_values() {
        let v = serde_json::to_value(all()).expect("json");
        assert_eq!(v["ai"]["anthropic"]["apiKey"], "sk-ant-canary-0001");
        assert_eq!(v["ai"]["openai"]["apiKey"], "sk-oa-canary-0002");
        assert_eq!(v["ai"]["ollama"]["baseUrl"], "http://localhost:11434");
        assert_eq!(v["ai"]["ollama"]["model"], "llama3.1:8b");
        assert_eq!(v["smtp"]["password"], "smtp-canary-0003");
        assert_eq!(v["smtp"]["host"], "smtp.example.com");
        assert_eq!(v["smtp"]["port"], 587);
        assert_eq!(v["smtp"]["security"], "starttls");
        let only_smtp = SessionSecrets {
            smtp: Some(SmtpSessionSecret {
                password: None,
                host: "10.0.0.9".into(),
                port: 25,
                security: SmtpSecurityWire::None,
            }),
            ..SessionSecrets::default()
        };
        let v = serde_json::to_value(&only_smtp).expect("json");
        assert!(v.get("ai").is_none());
        assert!(
            v["smtp"].get("password").is_none(),
            "no password key when none"
        );
        assert_eq!(v["smtp"]["security"], "none");
        let dbg = format!("{:?}", all());
        assert!(!dbg.contains("canary"), "{dbg}");
        assert!(
            !dbg.contains("11434") && !dbg.contains("example"),
            "no URL or host in Debug: {dbg}"
        );
        assert!(SessionSecrets::default().is_empty());
        for s in SecretSlot::ALL {
            assert!(s.wire().contains('.'));
        }
    }

    #[tokio::test]
    async fn refused_locally_without_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s.put_session_secrets(&all()).await.expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::SessionSecretsUnavailable)
        );
        assert_eq!(
            s.session_secrets().await.expect_err("gated"),
            ApiError::Local(LocalValidation::SessionSecretsUnavailable)
        );
        assert_eq!(
            s.delete_session_secret(SecretSlot::SmtpPassword)
                .await
                .expect_err("gated"),
            ApiError::Local(LocalValidation::SessionSecretsUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
    }

    #[tokio::test]
    async fn with_the_capability_put_get_delete_are_sent_once() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/desktop/session-secrets"))
            .respond_with(json(
                200,
                r#"{"ai.anthropic":{"present":true,"fingerprint":"sha256:0123abcd"},"smtp.password":{"present":true,"fingerprint":"sha256:89efcdab"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/session-secrets"))
            .respond_with(json(
                200,
                r#"{"ai.anthropic":{"present":true,"fingerprint":"sha256:0123abcd"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/desktop/session-secrets/ai.anthropic"))
            .respond_with(json(200, r#"{"deleted":true}"#))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::SESSION_SECRETS]).await;
        let p = s.put_session_secrets(&all()).await.expect("put");
        assert!(p["ai.anthropic"].present);
        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 1);
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["ai"]["anthropic"]["apiKey"], "sk-ant-canary-0001");
        let got = s.session_secrets().await.expect("get");
        assert_eq!(
            got["ai.anthropic"].fingerprint.as_deref(),
            Some("sha256:0123abcd")
        );
        s.delete_session_secret(SecretSlot::AiAnthropic)
            .await
            .expect("delete");
        assert_eq!(requests(&server, "DELETE").await.len(), 1);
        let e = s
            .put_session_secrets(&SessionSecrets::default())
            .await
            .expect_err("empty");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
        assert_eq!(requests(&server, "PUT").await.len(), 1);
    }
}
