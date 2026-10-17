//! The open company's profile and settings (P05 plan §2.8).
//!
//! - `GET /api/v1/company` and `PUT /api/v1/company` carry the profile
//!   (name, address, contact details, entity type). The engine's
//!   `updateCompanySchema` has no `lockDate`, and [`CompanyProfileUpdate`]
//!   has no such field either (NN8; Q-P05g).
//! - `GET /api/v1/company/settings` is read-only in P05. A typed settings
//!   writer comes with P08/P09 and must omit `lockDate` until U-15 (§2.7):
//!   [`CompanySettings`] deliberately does not implement `Serialize`.
//! - Company list, create, open and close live on [`Session`] because they
//!   change the session's scope.

use cl_core::ids::CompanyId;
use cl_core::{CalendarDate, Rate, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::requested_fp;
use crate::resources::operations::{COMPANY_GET, COMPANY_PUT, COMPANY_SETTINGS};
use crate::resources::reconcile::row_fp;
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, get_versioned, pick, update_guarded,
};
use crate::session::{Scope, Session, decode, json_body};

/// Legal form of the business (`entityType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    /// Sole proprietorship.
    SoleProp,
    /// Single-member LLC.
    SingleMemberLlc,
    /// S corporation.
    SCorp,
    /// C corporation.
    CCorp,
    /// Partnership.
    Partnership,
}

/// The profile fields the company form edits. Sent whole on save: `None`
/// clears a nullable field. `country` and `entity_type` cannot be cleared
/// (the engine refuses `null` there), so `None` leaves them unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyProfileUpdate {
    /// Business name (required, 1-255 characters).
    pub business_name: String,
    /// Legal name.
    #[serde(default)]
    pub legal_name: Option<String>,
    /// Employer identification number.
    #[serde(default)]
    pub ein: Option<String>,
    /// Address line 1.
    #[serde(default)]
    pub address_line1: Option<String>,
    /// Address line 2.
    #[serde(default)]
    pub address_line2: Option<String>,
    /// City.
    #[serde(default)]
    pub city: Option<String>,
    /// State.
    #[serde(default)]
    pub state: Option<String>,
    /// ZIP code.
    #[serde(default)]
    pub zip: Option<String>,
    /// Two- or three-letter country code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Phone.
    #[serde(default)]
    pub phone: Option<String>,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Website.
    #[serde(default)]
    pub website: Option<String>,
    /// Industry.
    #[serde(default)]
    pub industry: Option<String>,
    /// Legal form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<EntityType>,
}

/// `GET /api/v1/company` (fields the app uses; others, including the
/// encrypted SMTP password column, are never read).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanyProfile {
    /// The company row.
    pub id: CompanyId,
    /// The editable profile.
    #[serde(flatten)]
    pub profile: CompanyProfileUpdate,
    /// Whether first-run setup finished.
    #[serde(default)]
    pub setup_complete: Option<bool>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl CompanyProfile {
    /// Wire names of the fingerprinted fields.
    pub const EDITABLE_FIELDS: &'static [&'static str] = &[
        "businessName",
        "legalName",
        "ein",
        "addressLine1",
        "addressLine2",
        "city",
        "state",
        "zip",
        "country",
        "phone",
        "email",
        "website",
        "industry",
        "entityType",
    ];
}

impl EditableFields for CompanyProfile {
    const EDITABLE: &'static [&'static str] = CompanyProfile::EDITABLE_FIELDS;

    fn editable(&self) -> Value {
        let v = serde_json::to_value(&self.profile).unwrap_or(Value::Null);
        pick(&v, Self::EDITABLE)
    }
}

/// `GET /api/v1/company/settings` (read-only; no `Serialize`, so it can
/// never be sent back with a `lockDate`).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySettings {
    /// First month of the fiscal year (1-12).
    #[serde(default)]
    pub fiscal_year_start_month: Option<u8>,
    /// `cash` or `accrual`.
    #[serde(default)]
    pub accounting_method: Option<String>,
    /// Default payment terms key.
    #[serde(default)]
    pub default_payment_terms: Option<String>,
    /// Invoice number prefix.
    #[serde(default)]
    pub invoice_prefix: Option<String>,
    /// Next invoice number.
    #[serde(default)]
    pub invoice_next_number: Option<u64>,
    /// Default sales tax rate (exact, `decimal(5,4)`).
    #[serde(default)]
    pub default_sales_tax_rate: Option<Rate>,
    /// Currency code.
    #[serde(default)]
    pub currency: Option<String>,
    /// Date format preference.
    #[serde(default)]
    pub date_format: Option<String>,
    /// The closing date (shown only; there is no writer until U-15).
    #[serde(default)]
    pub lock_date: Option<CalendarDate>,
}

impl Session {
    fn require_company(&self) -> Result<(), ApiError> {
        match self.scope() {
            Scope::Company { .. } => Ok(()),
            Scope::Workspace => Err(ApiError::NoCompanyOpen),
        }
    }

    /// The open company's profile with its version.
    ///
    /// # Errors
    /// `NoCompanyOpen` in the workspace (nothing sent); any [`ApiError`].
    pub async fn company_profile(&self) -> Result<Versioned<CompanyProfile>, ApiError> {
        self.require_company()?;
        get_versioned(
            self,
            &COMPANY_GET,
            COMPANY_GET.route().to_string(),
            "company",
        )
        .await
    }

    /// Saves the profile if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` when it changed since it was read (nothing
    /// sent); `NoCompanyOpen`; any [`ApiError`].
    pub async fn update_company_profile(
        &self,
        expected: &RecordVersion,
        update: &CompanyProfileUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<CompanyProfile>, ApiError> {
        self.require_company()?;
        let body = json_body(update)?;
        let u = GuardedUpdate {
            get: &COMPANY_GET,
            put: &COMPANY_PUT,
            path: COMPANY_PUT.route().to_string(),
            key: "company",
            requested: requested_fp::<CompanyProfile>(&body),
            body,
            idempotency_key: key,
            fp: row_fp::<CompanyProfile>,
            stamp_covered: false,
        };
        update_guarded(self, expected, u).await
    }

    /// The open company's settings (read-only in P05).
    ///
    /// # Errors
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn company_settings(&self) -> Result<CompanySettings, ApiError> {
        #[derive(Deserialize)]
        struct Wrapper {
            settings: CompanySettings,
        }
        let call = Call::at(Method::Get, COMPANY_SETTINGS.route());
        let reply = self.send(&COMPANY_SETTINGS, call, None).await?;
        Ok(decode::<Wrapper>(&reply, "company settings")?.settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer};

    const PROFILE: &str = r#"{"company":{"id":"0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "tenantId":"1b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","businessName":"Canary Co",
        "legalName":null,"ein":null,"addressLine1":"1 Main","addressLine2":null,"city":"Town",
        "state":"WI","zip":"53000","country":"US","phone":null,"email":null,"website":null,
        "industry":null,"entityType":"single_member_llc","smtpPass":"enc:secret",
        "lockDate":"2026-01-31","setupComplete":true,
        "createdAt":"2026-10-01T10:00:00.000Z","updatedAt":"2026-10-02T10:00:00.000Z"}}"#;

    #[tokio::test]
    async fn profile_read_and_guarded_update_never_send_lock_date() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company"))
            .respond_with(json(200, PROFILE))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company"))
            .and(header(
                "x-company-id",
                "0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            ))
            .respond_with(json(200, &PROFILE.replace("Canary Co", "New Name")))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let v = s.company_profile().await.expect("profile");
        assert_eq!(v.record.profile.business_name, "Canary Co");
        assert_eq!(
            v.record.profile.entity_type,
            Some(EntityType::SingleMemberLlc)
        );
        assert!(matches!(v.version, RecordVersion::Fingerprint(_)));

        let mut update = v.record.profile.clone();
        update.business_name = "New Name".into();
        let saved = s
            .update_company_profile(&v.version, &update, IdempotencyKey::new())
            .await
            .expect("saved");
        assert_eq!(saved.record.profile.business_name, "New Name");
        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 1);
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["businessName"], "New Name");
        assert_eq!(
            body["legalName"],
            Value::Null,
            "nullable fields are sent to clear"
        );
        assert!(body.get("lockDate").is_none());
        assert!(body.get("smtpPass").is_none());
        assert!(puts[0].headers.get("idempotency-key").is_some());
    }

    #[tokio::test]
    async fn changed_profile_is_a_conflict_without_a_put() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company"))
            .respond_with(json(200, &PROFILE.replace("1 Main", "2 Main")))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let stale = RecordVersion::Fingerprint([7; 32]);
        let e = s
            .update_company_profile(
                &stale,
                &CompanyProfileUpdate::default(),
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
        assert_eq!(requests(&server, "PUT").await.len(), 0);
    }

    #[tokio::test]
    async fn settings_are_read_with_an_exact_rate_and_the_workspace_sends_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(
                200,
                r#"{"settings":{"fiscalYearStartMonth":7,"accountingMethod":"accrual",
                    "defaultSalesTaxRate":"0.0550","invoiceNextNumber":1001,
                    "lockDate":"2026-06-30","chatSupportEnabled":false}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let st = s.company_settings().await.expect("settings");
        assert_eq!(st.fiscal_year_start_month, Some(7));
        assert_eq!(
            st.default_sales_tax_rate.map(Rate::to_wire).as_deref(),
            Some("0.0550")
        );
        assert_eq!(
            st.lock_date.map(CalendarDate::to_wire).as_deref(),
            Some("2026-06-30")
        );

        let ws = crate::session::tests::session_on(&server, Scope::Workspace);
        let before = server.received_requests().await.expect("r").len();
        assert_eq!(
            ws.company_profile().await.expect_err("ws"),
            ApiError::NoCompanyOpen
        );
        assert_eq!(
            ws.company_settings().await.expect_err("ws"),
            ApiError::NoCompanyOpen
        );
        assert_eq!(server.received_requests().await.expect("r").len(), before);
    }

    #[tokio::test]
    async fn settings_with_a_json_number_rate_are_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(200, r#"{"settings":{"defaultSalesTaxRate":0.055}}"#))
            .mount(&server)
            .await;
        let e = company_session(&server)
            .company_settings()
            .await
            .expect_err("float");
        assert!(matches!(e, ApiError::Decode { .. }));
    }
}
