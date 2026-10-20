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

use cl_core::ids::{CompanyId, UserId};
use cl_core::{CalendarDate, Rate, Secret, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::errors::LocalValidation;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::requested_fp;
use crate::resources::operations::{
    CLOSING_PASSWORD_GET, CLOSING_PASSWORD_PUT, CLOSING_PASSWORD_RESET, COMPANY_GET, COMPANY_PUT,
    COMPANY_SETTINGS, COMPANY_SETTINGS_PUT,
};
use crate::resources::reconcile::row_fp;
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, get_versioned, pick, update_guarded,
};
use crate::session::{Scope, Session, decode, json_body};
use crate::wire::Capabilities;

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

/// Accounting basis of the company (`accountingMethod`, Q-44).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingMethod {
    /// Accrual (the default).
    Accrual,
    /// Cash.
    Cash,
}

/// The settings the setup interview and Preferences write
/// (`PUT /api/v1/company/settings`). Every field is optional and only
/// present fields are sent. The closing date is deliberately not here:
/// [`ClosingDateChange`] is the only writer and it is capability-gated.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompanySettingsUpdate {
    /// First month of the fiscal year (1-12).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fiscal_year_start_month: Option<u8>,
    /// Basis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accounting_method: Option<AccountingMethod>,
    /// Default payment terms key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_payment_terms: Option<String>,
    /// Invoice prefix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoice_prefix: Option<String>,
    /// Next invoice number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invoice_next_number: Option<u64>,
    /// Default sales tax rate (exact).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_sales_tax_rate: Option<Rate>,
    /// Currency code (USD, Q-41).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Date format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_format: Option<String>,
}

/// A closing-date change (Q-48, Q-P02d). Sent only when the engine
/// advertises `closing-password-v1` (U-15), because before that the engine
/// accepts the lock date without any password.
#[derive(Clone)]
pub struct ClosingDateChange {
    /// The new closing date, or `None` to clear it.
    pub closing_date: Option<CalendarDate>,
    /// The closing-date password (set or confirm). Never logged.
    pub closing_password: Option<Secret>,
}

impl std::fmt::Debug for ClosingDateChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosingDateChange")
            .field("closing_date", &self.closing_date)
            .finish_non_exhaustive()
    }
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

    /// Writes company settings (never the closing date).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a month outside 1-12 (nothing sent);
    /// `NoCompanyOpen`; `Validation`; any [`ApiError`].
    pub async fn update_company_settings(
        &self,
        update: &CompanySettingsUpdate,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        self.require_company()?;
        if update
            .fiscal_year_start_month
            .is_some_and(|m| !(1..=12).contains(&m))
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(json_body(update)?);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
    }

    /// Sets or clears the closing date with its password (U-15). Refused
    /// locally until the engine advertises `closing-password-v1`: until
    /// then nothing in the typed API writes `lockDate` (NN8).
    ///
    /// # Errors
    /// `Local(ClosingDateWriteUnavailable)` without the capability (nothing
    /// sent); `Unauthorized(InvalidCredentials)` for a wrong password;
    /// `NoCompanyOpen`; any [`ApiError`].
    pub async fn set_closing_date(
        &self,
        change: &ClosingDateChange,
        key: IdempotencyKey,
    ) -> Result<CompanySettings, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            lock_date: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            closing_password: Option<&'a str>,
        }
        self.require_company()?;
        if !self
            .client()
            .capabilities()
            .has(Capabilities::CLOSING_PASSWORD)
        {
            return Err(ApiError::Local(
                LocalValidation::ClosingDateWriteUnavailable,
            ));
        }
        let body = json_body(&Body {
            lock_date: change.closing_date.map(CalendarDate::to_wire),
            closing_password: change.closing_password.as_ref().map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Put, COMPANY_SETTINGS_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&COMPANY_SETTINGS_PUT, call, None).await?;
        settings_from(&reply)
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

/// The closing date and whether a closing-date password is set
/// (`GET /company/closing-password`, U-15). Never the password or its hash.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClosingPasswordStatus {
    /// The closing date.
    #[serde(default)]
    pub closing_date: Option<CalendarDate>,
    /// Who set the closing date.
    #[serde(default)]
    pub set_by: Option<UserId>,
    /// When it was set.
    #[serde(default)]
    pub set_at: Option<Timestamp>,
    /// A closing-date password is set.
    #[serde(rename = "closingPasswordSet")]
    pub password_set: bool,
}

/// Shortest closing-date password (characters).
pub const CLOSING_PASSWORD_MIN_CHARS: usize = 4;
/// Longest closing-date password (UTF-8 bytes).
pub const CLOSING_PASSWORD_MAX_BYTES: usize = 72;

impl Session {
    fn require_closing_password(&self) -> Result<(), ApiError> {
        self.require_company()?;
        if self
            .client()
            .capabilities()
            .has(Capabilities::CLOSING_PASSWORD)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::ClosingPasswordUnavailable))
        }
    }

    /// The closing date and whether a closing-date password is set.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`
    /// (nothing sent); `NoCompanyOpen`; any [`ApiError`].
    pub async fn closing_password_status(&self) -> Result<ClosingPasswordStatus, ApiError> {
        self.require_closing_password()?;
        let call = Call::at(Method::Get, CLOSING_PASSWORD_GET.route());
        let reply = self.send(&CLOSING_PASSWORD_GET, call, None).await?;
        decode(&reply, "closing password status")
    }

    /// Sets, changes (`current` required once one is set) or clears (`new`
    /// = `None`) the closing-date password. Returns whether one is set now.
    /// Never retried; the passwords are never logged.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`,
    /// `Local(InvalidValue)` for a new password under 4 characters or over
    /// 72 bytes (nothing sent); `ClosingPasswordRejected` (wrong current
    /// password), `ClosingPasswordOwnerRequired`,
    /// `ClosingPasswordRateLimited`; any [`ApiError`].
    pub async fn set_closing_password(
        &self,
        current: Option<&Secret>,
        new: Option<&Secret>,
        key: IdempotencyKey,
    ) -> Result<bool, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            #[serde(skip_serializing_if = "Option::is_none")]
            current_password: Option<&'a str>,
            new_password: Option<&'a str>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer {
            closing_password_set: bool,
        }
        self.require_closing_password()?;
        if let Some(n) = new {
            let n = n.expose();
            if n.chars().count() < CLOSING_PASSWORD_MIN_CHARS
                || n.len() > CLOSING_PASSWORD_MAX_BYTES
            {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
        }
        let body = json_body(&Body {
            current_password: current.map(Secret::expose),
            new_password: new.map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Put, CLOSING_PASSWORD_PUT.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&CLOSING_PASSWORD_PUT, call, None).await?;
        decode::<Answer>(&reply, "closing password")
            .map(|a| a.closing_password_set)
            .map_err(crate::resources::versioning::saved_unreadable)
    }

    /// The owner's reset of a forgotten closing-date password (Q-239): with
    /// their own sign-in password they clear it (`new` = `None`) or replace
    /// it. The engine records the reset in the audit trail. Returns whether
    /// a closing-date password is set now. Never retried; neither password
    /// is ever logged.
    ///
    /// # Errors
    /// `Local(ClosingPasswordUnavailable)` without `closing-password-v1`,
    /// `Local(InvalidValue)` for an empty sign-in password or a new
    /// password under 4 characters or over 72 bytes (nothing sent);
    /// `AccountPasswordRejected` (wrong sign-in password),
    /// `ClosingPasswordOwnerRequired` (not the owner),
    /// `ClosingPasswordRateLimited`; any [`ApiError`].
    pub async fn reset_closing_password(
        &self,
        account_password: &Secret,
        new: Option<&Secret>,
        key: IdempotencyKey,
    ) -> Result<bool, ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            account_password: &'a str,
            new_password: Option<&'a str>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Answer {
            closing_password_set: bool,
        }
        self.require_closing_password()?;
        if account_password.expose().is_empty() {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        if let Some(n) = new {
            let n = n.expose();
            if n.chars().count() < CLOSING_PASSWORD_MIN_CHARS
                || n.len() > CLOSING_PASSWORD_MAX_BYTES
            {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
        }
        let body = json_body(&Body {
            account_password: account_password.expose(),
            new_password: new.map(Secret::expose),
        })?;
        let mut call = Call::at(Method::Post, CLOSING_PASSWORD_RESET.route()).json_body(body);
        call.idempotency_key = Some(key);
        let reply = self.send(&CLOSING_PASSWORD_RESET, call, None).await?;
        decode::<Answer>(&reply, "closing password reset")
            .map(|a| a.closing_password_set)
            .map_err(crate::resources::versioning::saved_unreadable)
    }
}

/// Reads `{settings}` from a settings write; a 2xx that cannot be read is
/// `SavedUnreadable` (the change is recorded).
fn settings_from(reply: &crate::client::EngineReply) -> Result<CompanySettings, ApiError> {
    #[derive(Deserialize)]
    struct Wrapper {
        settings: CompanySettings,
    }
    decode::<Wrapper>(reply, "company settings")
        .map(|w| w.settings)
        .map_err(crate::resources::versioning::saved_unreadable)
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

    #[tokio::test]
    async fn settings_writer_sends_only_present_fields_and_never_the_lock_date() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company/settings"))
            .respond_with(json(
                200,
                r#"{"settings":{"fiscalYearStartMonth":7,"accountingMethod":"cash","lockDate":null}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let st = s
            .update_company_settings(
                &CompanySettingsUpdate {
                    fiscal_year_start_month: Some(7),
                    accounting_method: Some(AccountingMethod::Cash),
                    ..CompanySettingsUpdate::default()
                },
                IdempotencyKey::new(),
            )
            .await
            .expect("settings");
        assert_eq!(st.fiscal_year_start_month, Some(7));
        let puts = requests(&server, "PUT").await;
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["fiscalYearStartMonth"], 7);
        assert_eq!(body["accountingMethod"], "cash");
        assert!(body.get("lockDate").is_none());
        assert!(body.get("currency").is_none());
        let e = s
            .update_company_settings(
                &CompanySettingsUpdate {
                    fiscal_year_start_month: Some(13),
                    ..CompanySettingsUpdate::default()
                },
                IdempotencyKey::new(),
            )
            .await
            .expect_err("month");
        assert_eq!(e, ApiError::Local(LocalValidation::InvalidValue));
    }

    #[tokio::test]
    async fn closing_date_is_refused_locally_without_the_capability() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        let e = s
            .set_closing_date(
                &ClosingDateChange {
                    closing_date: Some(CalendarDate::from_ymd(2026, 9, 30).expect("d")),
                    closing_password: Some(Secret::new("closing-canary")),
                },
                IdempotencyKey::new(),
            )
            .await
            .expect_err("gated");
        assert_eq!(
            e,
            ApiError::Local(LocalValidation::ClosingDateWriteUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());
        let change = ClosingDateChange {
            closing_date: None,
            closing_password: Some(Secret::new("closing-canary")),
        };
        assert!(!format!("{change:?}").contains("canary"));
    }

    #[tokio::test]
    async fn closing_password_status_and_set() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        assert_eq!(
            s.closing_password_status().await.expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert_eq!(
            s.set_closing_password(None, Some(&Secret::new("pw-canary")), IdempotencyKey::new())
                .await
                .expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/company/closing-password"))
            .respond_with(json(
                200,
                r#"{"closingDate":"2026-09-30","setBy":"5b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
                    "setAt":"2026-10-01T10:00:00.000Z","closingPasswordSet":true}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/company/closing-password"))
            .respond_with(json(200, r#"{"closingPasswordSet":false}"#))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[Capabilities::CLOSING_PASSWORD],
        )
        .await;
        let st = s.closing_password_status().await.expect("status");
        assert!(st.password_set);
        assert_eq!(st.closing_date, CalendarDate::from_ymd(2026, 9, 30).ok());
        assert_eq!(
            s.set_closing_password(None, Some(&Secret::new("abc")), IdempotencyKey::new())
                .await
                .expect_err("short"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        assert_eq!(
            s.set_closing_password(
                None,
                Some(&Secret::new("x".repeat(73))),
                IdempotencyKey::new()
            )
            .await
            .expect_err("long"),
            ApiError::Local(LocalValidation::InvalidValue)
        );
        let set = s
            .set_closing_password(
                Some(&Secret::new("old-canary")),
                None,
                IdempotencyKey::new(),
            )
            .await
            .expect("cleared");
        assert!(!set);
        let puts = requests(&server, "PUT").await;
        assert_eq!(puts.len(), 1);
        let body: Value = serde_json::from_slice(&puts[0].body).expect("json");
        assert_eq!(body["currentPassword"], "old-canary");
        assert_eq!(body["newPassword"], Value::Null);
    }

    #[tokio::test]
    async fn owner_reset_is_gated_checked_and_typed() {
        let server = MockServer::start().await;
        let s = company_session(&server);
        assert_eq!(
            s.reset_closing_password(&Secret::new("signin-canary"), None, IdempotencyKey::new())
                .await
                .expect_err("gated"),
            ApiError::Local(LocalValidation::ClosingPasswordUnavailable)
        );
        assert!(server.received_requests().await.expect("r").is_empty());

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/closing-password/reset"))
            .respond_with(json(200, r#"{"closingPasswordSet":true}"#))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/company/closing-password/reset"))
            .respond_with(json(
                403,
                r#"{"error":{"message":"bad signin-canary","code":"ACCOUNT_PASSWORD_INVALID"}}"#,
            ))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::session_with_caps(
            &server,
            &[Capabilities::CLOSING_PASSWORD],
        )
        .await;
        for (account, new) in [("", None), ("signin", Some("abc"))] {
            let new = new.map(Secret::new);
            assert_eq!(
                s.reset_closing_password(
                    &Secret::new(account),
                    new.as_ref(),
                    IdempotencyKey::new()
                )
                .await
                .expect_err("local"),
                ApiError::Local(LocalValidation::InvalidValue)
            );
        }
        let set = s
            .reset_closing_password(
                &Secret::new("signin-canary"),
                Some(&Secret::new("new-canary")),
                IdempotencyKey::new(),
            )
            .await
            .expect("reset");
        assert!(set);
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 1);
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert_eq!(body["accountPassword"], "signin-canary");
        assert_eq!(body["newPassword"], "new-canary");
        let e = s
            .reset_closing_password(&Secret::new("signin-canary"), None, IdempotencyKey::new())
            .await
            .expect_err("wrong");
        assert_eq!(e, ApiError::AccountPasswordRejected);
        assert!(!format!("{e} {e:?}").contains("canary"));
        // A credential change is never retried.
        assert_eq!(requests(&server, "POST").await.len(), 2);
    }
}
