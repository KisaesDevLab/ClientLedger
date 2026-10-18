//! AI settings, consent and usage (P09 step 3; `target/plans/P09-scout.md`
//! §1 and §2, P02 plan §2.6 E6).
//!
//! - `GET/PUT /api/v1/ai/admin/config`: the engine's global AI config. The
//!   writer [`AiConfigUpdate`] has **no key fields at all**: in the desktop
//!   app keys live in the OS keychain and reach the engine only through the
//!   per-session secrets (`session_secrets`), never the database
//!   (Q-P09a; the engine refuses them with `DESKTOP_KEYS_NOT_STORED` once
//!   E6 lands). The provider choice goes as `fallbackChain: ["<chosen>"]`
//!   (Q-33).
//! - Disclosure and consent (Q-136, Q-140): the system disclosure (super
//!   admin), the company disclosure, accept, revoke and the per-task
//!   toggles. Every accept writes an engine audit row.
//! - `POST /api/v1/ai/admin/test/{provider}` (10/min), `GET
//!   /api/v1/ai/admin/models/{provider}`, `GET /api/v1/ai/status`.
//! - `GET /api/v1/ai/usage`: call and token counts (integers) plus a dollar
//!   *estimate* the engine sums as floats; it is read as a display-only
//!   [`ReportAmount`], never as money (Q-P02e, non-negotiable 6).
//! - `GET /api/v1/ai/usage/summary` (E6, capability `ai-desktop-v1`):
//!   integer token totals; refused locally until the engine advertises it.
//! - [`AiUsageStamp`]: the `usage` object E6 adds to AI replies, which
//!   feeds the client-side token cap (`cl_settings::ai`).

use cl_core::Timestamp;
use cl_core::ids::CompanyId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{
    AI_CONFIG, AI_CONFIG_PUT, AI_CONSENT, AI_CONSENT_ACCEPT, AI_CONSENT_DISCLOSURE,
    AI_CONSENT_REVOKE, AI_CONSENT_TASKS, AI_DISCLOSURE, AI_DISCLOSURE_ACCEPT, AI_MODELS, AI_STATUS,
    AI_TEST_PROVIDER, AI_USAGE, AI_USAGE_SUMMARY,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Scope, Session, decode, json_body};
use crate::wire::{Capabilities, ReportAmount};

/// `GET /api/v1/ai/admin/config` (the fields the app uses; key values are
/// never in the reply).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    /// AI on at the system level.
    #[serde(default)]
    pub is_enabled: bool,
    /// Provider order; the desktop sends one entry.
    #[serde(default)]
    pub fallback_chain: Vec<String>,
    /// Provider for categorization.
    #[serde(default)]
    pub categorization_provider: Option<String>,
    /// Model for categorization.
    #[serde(default)]
    pub categorization_model: Option<String>,
    /// Provider for OCR.
    #[serde(default)]
    pub ocr_provider: Option<String>,
    /// Model for OCR.
    #[serde(default)]
    pub ocr_model: Option<String>,
    /// Provider for document classification.
    #[serde(default)]
    pub document_classification_provider: Option<String>,
    /// Model for document classification.
    #[serde(default)]
    pub document_classification_model: Option<String>,
    /// An Anthropic key is stored in the engine (server mode only).
    #[serde(default)]
    pub has_anthropic_key: bool,
    /// An `OpenAI` key is stored in the engine (server mode only).
    #[serde(default)]
    pub has_openai_key: bool,
    /// Ollama address.
    #[serde(default)]
    pub ollama_base_url: Option<String>,
    /// `strict`, `standard`, `permissive`.
    #[serde(default)]
    pub pii_protection_level: Option<String>,
    /// Usage rows are written.
    #[serde(default)]
    pub track_usage: Option<bool>,
    /// The engine's dollar cap: a float estimate, display only (Q-P02e).
    #[serde(default)]
    pub monthly_budget_limit: Option<ReportAmount>,
    /// When the super admin accepted the system disclosure.
    #[serde(default)]
    pub admin_disclosure_accepted_at: Option<Timestamp>,
    /// Current disclosure version.
    #[serde(default)]
    pub disclosure_version: Option<u32>,
}

/// The non-secret fields the Preferences AI panel writes
/// (`PUT /api/v1/ai/admin/config`). Only present fields are sent. There is
/// deliberately no key field and no dollar budget here (floats).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigUpdate {
    /// Turn AI on or off (the engine refuses `true` before the system
    /// disclosure is accepted).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_enabled: Option<bool>,
    /// The chosen provider, alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_chain: Option<Vec<String>>,
    /// Provider for categorization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categorization_provider: Option<String>,
    /// Model for categorization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categorization_model: Option<String>,
    /// Provider for OCR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_provider: Option<String>,
    /// Model for OCR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_model: Option<String>,
    /// Provider for document classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_classification_provider: Option<String>,
    /// Model for document classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_classification_model: Option<String>,
    /// Ollama address (loopback or LAN).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ollama_base_url: Option<String>,
    /// `strict` in the desktop app (Q-139).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pii_protection_level: Option<String>,
    /// Usage rows on or off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_usage: Option<bool>,
}

impl AiConfigUpdate {
    /// Wire keys of credential fields the desktop must never send.
    pub const FORBIDDEN_KEYS: [&'static str; 9] = [
        "anthropicApiKey",
        "openaiApiKey",
        "geminiApiKey",
        "openaiCompatApiKey",
        "digitaloceanApiKey",
        "glmOcrApiKey",
        "routerToken",
        "monthlyBudgetLimit",
        "apiKey",
    ];

    /// Everything a provider choice needs: the chain, the three task
    /// providers, strict redaction, usage tracking.
    #[must_use]
    pub fn for_provider(provider: &str, enabled: bool) -> AiConfigUpdate {
        AiConfigUpdate {
            is_enabled: Some(enabled),
            fallback_chain: Some(vec![provider.to_string()]),
            categorization_provider: Some(provider.to_string()),
            ocr_provider: Some(provider.to_string()),
            document_classification_provider: Some(provider.to_string()),
            pii_protection_level: Some("strict".into()),
            track_usage: Some(true),
            ..AiConfigUpdate::default()
        }
    }
}

/// The system (super admin) disclosure, `GET /api/v1/ai/admin/disclosure`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiDisclosure {
    /// Disclosure version the engine enforces.
    pub version: u32,
    /// Version of the text.
    #[serde(default)]
    pub text_version: Option<u32>,
    /// The disclosure text (markdown).
    #[serde(default)]
    pub text: String,
    /// Accepted when.
    #[serde(default)]
    pub accepted_at: Option<Timestamp>,
    /// Accepted by (user id).
    #[serde(default)]
    pub accepted_by: Option<String>,
}

/// Per-task switches (Q-140), `PATCH /api/v1/ai/consent/{companyId}/tasks`.
/// Only present fields are sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiTaskToggles {
    /// Bank Feed suggestions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub categorization: Option<bool>,
    /// Read bills and receipts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ocr: Option<bool>,
    /// Statement parsing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statement_parsing: Option<bool>,
    /// Document classification.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_classification: Option<bool>,
    /// Vendor enrichment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enrich_vendor: Option<bool>,
    /// Judgment review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judgment_review: Option<bool>,
    /// Ask a question in Reports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_summary: Option<bool>,
}

/// The company disclosure and consent state,
/// `GET /api/v1/ai/consent/{companyId}/disclosure` and the accept reply.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyConsent {
    /// The company.
    pub company_id: CompanyId,
    /// Current system disclosure version.
    #[serde(default)]
    pub system_version: Option<u32>,
    /// Version this company accepted.
    #[serde(default)]
    pub accepted_version: Option<u32>,
    /// When.
    #[serde(default)]
    pub accepted_at: Option<Timestamp>,
    /// By whom (user id).
    #[serde(default)]
    pub accepted_by: Option<String>,
    /// AI on for this company.
    #[serde(default)]
    pub ai_enabled: bool,
    /// The task switches as stored.
    #[serde(default)]
    pub enabled_tasks: AiTaskToggles,
    /// The disclosure text shown to the owner.
    #[serde(default)]
    pub text: Option<String>,
}

/// One company row of `GET /api/v1/ai/consent`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentRow {
    /// The company.
    pub id: CompanyId,
    /// AI on.
    #[serde(default)]
    pub ai_enabled: bool,
    /// Version accepted.
    #[serde(default)]
    pub accepted_version: Option<u32>,
    /// When.
    #[serde(default)]
    pub accepted_at: Option<Timestamp>,
    /// Task switches.
    #[serde(default)]
    pub tasks: Option<AiTaskToggles>,
    /// Accepted an older disclosure.
    #[serde(default)]
    pub is_stale: bool,
}

/// `GET /api/v1/ai/consent`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsentStatus {
    /// System AI on.
    #[serde(default)]
    pub system_enabled: Option<bool>,
    /// System disclosure version.
    #[serde(default)]
    pub system_version: Option<u32>,
    /// Companies of this tenant.
    #[serde(default)]
    pub companies: Vec<ConsentRow>,
}

/// `POST /api/v1/ai/admin/test/{provider}`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTest {
    /// Reached and answered.
    pub success: bool,
    /// The engine's error text (may name hosts; never log it).
    #[serde(default)]
    pub error: Option<String>,
    /// Model details, if any.
    #[serde(default)]
    pub model_info: Option<Value>,
}

/// `GET /api/v1/ai/admin/models/{provider}`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ProviderModels {
    /// Model names (empty when the provider cannot be reached).
    #[serde(default)]
    pub models: Vec<String>,
    /// Why the list is empty, if it is.
    #[serde(default)]
    pub error: Option<String>,
}

/// `GET /api/v1/ai/status` (any user): six readiness flags, as the engine
/// sends them.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiStatus {
    /// AI on, a credential present, and a provider configured.
    pub is_enabled: bool,
    /// Bill OCR ready.
    pub has_bill_ocr: bool,
    /// Receipt OCR ready.
    pub has_receipt_ocr: bool,
    /// Categorization ready.
    pub has_categorization: bool,
    /// Statement parser ready.
    pub has_statement_parser: bool,
    /// Document classifier ready.
    pub has_document_classifier: bool,
}

/// `GET /api/v1/ai/usage?months=N`: integer counts plus the engine's float
/// cost estimate (display only).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiUsage {
    /// Calls made.
    pub total_calls: u64,
    /// Input tokens.
    pub total_input: u64,
    /// Output tokens.
    pub total_output: u64,
    /// Estimated dollars (float in the engine; never money here).
    pub total_cost: Option<ReportAmount>,
    /// Per provider, as sent.
    pub by_provider: Value,
    /// Per job type, as sent.
    pub by_job_type: Value,
}

/// One row of the E6 usage summary.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageSummaryRow {
    /// Provider.
    pub provider: String,
    /// Model.
    pub model: Option<String>,
    /// Feature (job type).
    pub feature: Option<String>,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}

/// `GET /api/v1/ai/usage/summary?from&to` (E6, `ai-desktop-v1`): integer
/// tokens only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiUsageSummary {
    /// Total input tokens.
    pub input_tokens: u64,
    /// Total output tokens.
    pub output_tokens: u64,
    /// Breakdown.
    pub rows: Vec<UsageSummaryRow>,
}

/// The `usage` object E6 adds to every AI reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AiUsageStamp {
    /// Provider that answered.
    pub provider: String,
    /// Model that answered.
    pub model: Option<String>,
    /// Input tokens.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
}

impl AiUsageStamp {
    /// Reads `usage` from an AI reply body, if present (older engines send
    /// none).
    #[must_use]
    pub fn from_reply(body: &Value) -> Option<AiUsageStamp> {
        serde_json::from_value(body.get("usage")?.clone()).ok()
    }
}

/// Engine provider names the desktop may use (Q-33).
pub const DESKTOP_PROVIDERS: [&str; 3] = ["anthropic", "openai", "ollama"];

impl Session {
    fn company_id(&self) -> Result<CompanyId, ApiError> {
        match self.scope() {
            Scope::Company { company_id, .. } => Ok(company_id),
            Scope::Workspace => Err(ApiError::NoCompanyOpen),
        }
    }

    /// The engine's AI config (never key values).
    ///
    /// # Errors
    /// `Forbidden` for a non-admin; any [`ApiError`].
    pub async fn ai_config(&self) -> Result<AiConfig, ApiError> {
        let reply = self
            .send(&AI_CONFIG, Call::at(Method::Get, AI_CONFIG.route()), None)
            .await?;
        decode(&reply, "ai config")
    }

    /// Writes non-secret AI config. A provider outside
    /// [`DESKTOP_PROVIDERS`] is refused locally (nothing sent).
    ///
    /// # Errors
    /// `Local(InvalidValue)`; `Validation` when the disclosure was not
    /// accepted; any [`ApiError`].
    pub async fn update_ai_config(
        &self,
        update: &AiConfigUpdate,
        key: IdempotencyKey,
    ) -> Result<AiConfig, ApiError> {
        let providers = update
            .fallback_chain
            .iter()
            .flatten()
            .map(String::as_str)
            .chain(update.categorization_provider.as_deref())
            .chain(update.ocr_provider.as_deref())
            .chain(update.document_classification_provider.as_deref());
        for p in providers {
            if !DESKTOP_PROVIDERS.contains(&p) {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
        }
        let mut call = Call::at(Method::Put, AI_CONFIG_PUT.route()).json_body(json_body(update)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&AI_CONFIG_PUT, call, None).await?;
        decode(&reply, "ai config").map_err(saved_unreadable)
    }

    /// The system disclosure.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn ai_disclosure(&self) -> Result<AiDisclosure, ApiError> {
        let reply = self
            .send(
                &AI_DISCLOSURE,
                Call::at(Method::Get, AI_DISCLOSURE.route()),
                None,
            )
            .await?;
        decode(&reply, "ai disclosure")
    }

    /// Accepts the system disclosure (audit row `ai_config_system_disclosure`).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn accept_ai_disclosure(
        &self,
        key: IdempotencyKey,
    ) -> Result<AiDisclosure, ApiError> {
        let mut call = Call::at(Method::Post, AI_DISCLOSURE_ACCEPT.route());
        call.idempotency_key = Some(key);
        let reply = self.send(&AI_DISCLOSURE_ACCEPT, call, None).await?;
        decode(&reply, "ai disclosure").map_err(saved_unreadable)
    }

    /// Consent state of every company in the tenant.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn ai_consent(&self) -> Result<ConsentStatus, ApiError> {
        let reply = self
            .send(&AI_CONSENT, Call::at(Method::Get, AI_CONSENT.route()), None)
            .await?;
        decode(&reply, "ai consent")
    }

    /// The open company's disclosure text and consent state.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn company_ai_disclosure(&self) -> Result<CompanyConsent, ApiError> {
        let id = self.company_id()?;
        let path = AI_CONSENT_DISCLOSURE
            .route()
            .replace("{companyId}", &id.to_string());
        let reply = self
            .send(
                &AI_CONSENT_DISCLOSURE,
                Call::new(Method::Get, AI_CONSENT_DISCLOSURE.route(), path),
                None,
            )
            .await?;
        decode(&reply, "company ai consent")
    }

    /// Accepts the disclosure for the open company (Q-136: the engine
    /// writes `company_ai_consent_accepted` with date, user and version).
    ///
    /// # Errors
    /// `NoCompanyOpen`; `Validation` when the system side is not ready;
    /// any [`ApiError`].
    pub async fn accept_company_ai_consent(
        &self,
        key: IdempotencyKey,
    ) -> Result<CompanyConsent, ApiError> {
        let id = self.company_id()?;
        let path = AI_CONSENT_ACCEPT
            .route()
            .replace("{companyId}", &id.to_string());
        let mut call = Call::new(Method::Post, AI_CONSENT_ACCEPT.route(), path);
        call.idempotency_key = Some(key);
        let reply = self.send(&AI_CONSENT_ACCEPT, call, None).await?;
        decode(&reply, "company ai consent").map_err(saved_unreadable)
    }

    /// Revokes the open company's consent (AI off for it).
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn revoke_company_ai_consent(&self, key: IdempotencyKey) -> Result<(), ApiError> {
        let id = self.company_id()?;
        let path = AI_CONSENT_REVOKE
            .route()
            .replace("{companyId}", &id.to_string());
        let mut call = Call::new(Method::Post, AI_CONSENT_REVOKE.route(), path);
        call.idempotency_key = Some(key);
        self.send(&AI_CONSENT_REVOKE, call, None).await.map(drop)
    }

    /// Sets per-task switches for the open company (Q-140). Returns the
    /// stored switches.
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn set_ai_tasks(
        &self,
        toggles: &AiTaskToggles,
        key: IdempotencyKey,
    ) -> Result<AiTaskToggles, ApiError> {
        #[derive(Deserialize)]
        struct W {
            tasks: AiTaskToggles,
        }
        let id = self.company_id()?;
        let path = AI_CONSENT_TASKS
            .route()
            .replace("{companyId}", &id.to_string());
        let mut call =
            Call::new(Method::Patch, AI_CONSENT_TASKS.route(), path).json_body(json_body(toggles)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&AI_CONSENT_TASKS, call, None).await?;
        decode::<W>(&reply, "ai tasks")
            .map(|w| w.tasks)
            .map_err(saved_unreadable)
    }

    /// Tests a provider connection (rate limited 10/min; offline the cloud
    /// ones answer `OfflineUnavailable`).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a provider outside the allowlist (nothing
    /// sent); `RateLimited`; `OfflineUnavailable`; any [`ApiError`].
    pub async fn test_ai_provider(&self, provider: &str) -> Result<ProviderTest, ApiError> {
        if !DESKTOP_PROVIDERS.contains(&provider) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let path = AI_TEST_PROVIDER.route().replace("{provider}", provider);
        let reply = self
            .send(
                &AI_TEST_PROVIDER,
                Call::new(Method::Post, AI_TEST_PROVIDER.route(), path),
                None,
            )
            .await?;
        decode(&reply, "provider test")
    }

    /// Models a provider offers (empty when unreachable).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a provider outside the allowlist; any
    /// [`ApiError`].
    pub async fn ai_models(&self, provider: &str) -> Result<ProviderModels, ApiError> {
        if !DESKTOP_PROVIDERS.contains(&provider) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let path = AI_MODELS.route().replace("{provider}", provider);
        let reply = self
            .send(
                &AI_MODELS,
                Call::new(Method::Get, AI_MODELS.route(), path),
                None,
            )
            .await?;
        decode(&reply, "provider models")
    }

    /// What AI features are ready.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn ai_status(&self) -> Result<AiStatus, ApiError> {
        let reply = self
            .send(&AI_STATUS, Call::at(Method::Get, AI_STATUS.route()), None)
            .await?;
        decode(&reply, "ai status")
    }

    /// Usage over the last `months` months (1-24).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for months outside 1-24; any [`ApiError`].
    pub async fn ai_usage(&self, months: u8) -> Result<AiUsage, ApiError> {
        if !(1..=24).contains(&months) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::at(Method::Get, AI_USAGE.route());
        call.query = vec![("months".into(), months.to_string())];
        let reply = self.send(&AI_USAGE, call, None).await?;
        decode(&reply, "ai usage")
    }

    /// Integer token totals between two dates (E6). Refused locally until
    /// the engine advertises `ai-desktop-v1` (nothing sent).
    ///
    /// # Errors
    /// `Local(AiUsageSummaryUnavailable)`; any [`ApiError`].
    pub async fn ai_usage_summary(
        &self,
        from: cl_core::CalendarDate,
        to: cl_core::CalendarDate,
    ) -> Result<AiUsageSummary, ApiError> {
        if !self.client().capabilities().has(Capabilities::AI_DESKTOP) {
            return Err(ApiError::Local(LocalValidation::AiUsageSummaryUnavailable));
        }
        let mut call = Call::at(Method::Get, AI_USAGE_SUMMARY.route());
        call.query = vec![("from".into(), from.to_wire()), ("to".into(), to.to_wire())];
        let reply = self.send(&AI_USAGE_SUMMARY, call, None).await?;
        decode(&reply, "ai usage summary")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const CONFIG: &str = r#"{"isEnabled":true,"fallbackChain":["ollama"],"categorizationProvider":"ollama",
        "categorizationModel":"llama3.1:8b","ocrProvider":null,"hasAnthropicKey":false,"hasOpenaiKey":false,
        "ollamaBaseUrl":"http://localhost:11434","piiProtectionLevel":"strict","trackUsage":true,
        "monthlyBudgetLimit":10.5,"adminDisclosureAcceptedAt":"2026-10-01T10:00:00.000Z","disclosureVersion":1,
        "providerTestHistory":{},"taskOptions":{}}"#;

    #[tokio::test]
    async fn config_round_trip_sends_no_key_and_refuses_other_providers() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/admin/config"))
            .respond_with(json(200, CONFIG))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/ai/admin/config"))
            .respond_with(json(200, CONFIG))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let c = s.ai_config().await.expect("config");
        assert!(c.is_enabled);
        assert_eq!(c.fallback_chain, vec!["ollama"]);
        assert_eq!(
            c.monthly_budget_limit
                .as_ref()
                .and_then(|m| m.display(2))
                .as_deref(),
            Some("10.50")
        );
        let u = AiConfigUpdate::for_provider("anthropic", true);
        let saved = s
            .update_ai_config(&u, IdempotencyKey::new())
            .await
            .expect("put");
        assert!(saved.is_enabled);
        let puts = requests(&server, "PUT").await;
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["fallbackChain"], serde_json::json!(["anthropic"]));
        assert_eq!(body["piiProtectionLevel"], "strict");
        for k in AiConfigUpdate::FORBIDDEN_KEYS {
            assert!(body.get(k).is_none(), "{k} must never be sent");
        }
        assert!(puts[0].headers.get("idempotency-key").is_some());
        let e = s
            .update_ai_config(
                &AiConfigUpdate::for_provider("gemini", true),
                IdempotencyKey::new(),
            )
            .await
            .expect_err("gemini");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
        assert_eq!(requests(&server, "PUT").await.len(), 1);
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    async fn disclosure_consent_tasks_test_status_and_usage() {
        let server = MockServer::start().await;
        let company = crate::resources::testkit::COMPANY;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/admin/disclosure"))
            .respond_with(json(
                200,
                r#"{"version":2,"textVersion":3,"text":"Notice","acceptedAt":null,"acceptedBy":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/ai/admin/disclosure/accept"))
            .respond_with(json(
                200,
                r#"{"version":2,"textVersion":3,"text":"Notice","acceptedAt":"2026-10-17T10:00:00.000Z","acceptedBy":"u1"}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/v1/ai/consent/{company}/disclosure")))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"companyId":"{company}","companyName":"Canary Co","systemVersion":2,"acceptedVersion":null,
                        "acceptedAt":null,"acceptedBy":null,"aiEnabled":false,"enabledTasks":{{"categorization":false}},"text":"x"}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/ai/consent/{company}/accept")))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"companyId":"{company}","systemVersion":2,"acceptedVersion":2,"acceptedAt":"2026-10-17T10:00:00.000Z",
                        "aiEnabled":true,"enabledTasks":{{"categorization":true,"receipt_ocr":true}}}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/v1/ai/consent/{company}/revoke")))
            .respond_with(json(200, r#"{"revoked":true}"#))
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path(format!("/api/v1/ai/consent/{company}/tasks")))
            .respond_with(json(
                200,
                r#"{"tasks":{"categorization":true,"report_summary":false}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/consent"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"systemEnabled":true,"systemVersion":2,"companies":[{{"id":"{company}","name":"Canary Co",
                        "aiEnabled":true,"acceptedVersion":1,"acceptedAt":null,"tasks":null,"isStale":true}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/ai/admin/test/ollama"))
            .respond_with(json(
                200,
                r#"{"success":false,"error":"connect ECONNREFUSED 127.0.0.1:11434"}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/admin/models/ollama"))
            .respond_with(json(200, r#"{"models":["llama3.1:8b"]}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/status"))
            .respond_with(json(200, r#"{"isEnabled":true,"hasCategorization":true}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/ai/usage"))
            .respond_with(json(
                200,
                r#"{"totalCalls":3,"totalInput":1200,"totalOutput":300,"totalCost":0.0123,"byProvider":{},"byJobType":{}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let d = s.ai_disclosure().await.expect("disclosure");
        assert_eq!((d.version, d.text_version), (2, Some(3)));
        assert!(d.accepted_at.is_none());
        let d = s
            .accept_ai_disclosure(IdempotencyKey::new())
            .await
            .expect("accept");
        assert!(d.accepted_at.is_some());
        let c = s.company_ai_disclosure().await.expect("company");
        assert!(!c.ai_enabled);
        assert_eq!(c.enabled_tasks.categorization, Some(false));
        let c = s
            .accept_company_ai_consent(IdempotencyKey::new())
            .await
            .expect("accept");
        assert!(c.ai_enabled);
        assert_eq!(c.accepted_version, Some(2));
        s.revoke_company_ai_consent(IdempotencyKey::new())
            .await
            .expect("revoke");
        let t = s
            .set_ai_tasks(
                &AiTaskToggles {
                    report_summary: Some(false),
                    ..AiTaskToggles::default()
                },
                IdempotencyKey::new(),
            )
            .await
            .expect("tasks");
        assert_eq!(t.report_summary, Some(false));
        let patches = requests(&server, "PATCH").await;
        let body: Value = serde_json::from_slice(&patches[0].body).expect("json");
        assert_eq!(body, serde_json::json!({"report_summary": false}));
        let status = s.ai_consent().await.expect("consent");
        assert!(status.companies[0].is_stale);
        let t = s.test_ai_provider("ollama").await.expect("test");
        assert!(!t.success);
        assert!(t.error.is_some());
        assert_eq!(
            s.test_ai_provider("gemini").await.expect_err("gemini"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        let m = s.ai_models("ollama").await.expect("models");
        assert_eq!(m.models, vec!["llama3.1:8b"]);
        let st = s.ai_status().await.expect("status");
        assert!(st.is_enabled && st.has_categorization && !st.has_bill_ocr);
        let u = s.ai_usage(1).await.expect("usage");
        assert_eq!(
            (u.total_calls, u.total_input, u.total_output),
            (3, 1200, 300)
        );
        assert_eq!(
            u.total_cost.as_ref().and_then(|c| c.display(2)).as_deref(),
            Some("0.01")
        );
        let gets = requests(&server, "GET").await;
        let usage_req = gets
            .iter()
            .find(|r| r.url.path() == "/api/v1/ai/usage")
            .expect("usage request");
        assert_eq!(query(usage_req), "months=1");
        assert_eq!(
            s.ai_usage(0).await.expect_err("months"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
    }

    #[tokio::test]
    async fn usage_summary_is_gated_and_usage_stamps_parse() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let d = cl_core::CalendarDate::from_ymd(2026, 10, 1).expect("d");
        let e = s.ai_usage_summary(d, d).await.expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::AiUsageSummaryUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
        let stamp = AiUsageStamp::from_reply(&serde_json::json!({
            "result": {}, "usage": {"provider":"anthropic","model":"m","inputTokens":12,"outputTokens":3}
        }))
        .expect("stamp");
        assert_eq!((stamp.input_tokens, stamp.output_tokens), (12, 3));
        assert_eq!(
            AiUsageStamp::from_reply(&serde_json::json!({"result": {}})),
            None
        );
    }

    #[test]
    fn update_type_has_no_credential_field() {
        let v = serde_json::to_value(AiConfigUpdate::for_provider("openai", false)).expect("json");
        let text = v.to_string().to_ascii_lowercase();
        assert!(!text.contains("key") && !text.contains("token") && !text.contains("budget"));
    }
}
