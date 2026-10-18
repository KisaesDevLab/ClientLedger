//! A vendor's 1099 profile (`/api/v1/practice/1099/vendors/{id}/profile`,
//! Q-53, P11): whether a W-9 is on file, the tax ID type, backup
//! withholding. The engine keeps the tax ID from a W-9 encrypted and only
//! ever returns it masked (`***-**-1234`).
//!
//! Profile data is owner data (names, tax IDs, addresses): never logged.
//! `Debug` of the update body never shows a tax ID.

use cl_core::Timestamp;
use cl_core::ids::ContactId;
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{VENDOR_1099_PROFILE, VENDOR_1099_PROFILE_PUT};
use crate::session::{Session, decode, json_body};

/// Kind of taxpayer number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TinType {
    /// Social Security number (a person).
    #[serde(rename = "SSN")]
    Ssn,
    /// Employer identification number (a business).
    #[serde(rename = "EIN")]
    Ein,
}

/// A vendor's 1099 profile as the engine returns it.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vendor1099Profile {
    /// The vendor.
    pub contact_id: ContactId,
    /// Payments are tracked for a 1099 (the contact's flag).
    #[serde(default)]
    pub is1099_eligible: bool,
    /// A W-9 is on file.
    #[serde(default)]
    pub w9_on_file: bool,
    /// When the W-9 was captured.
    #[serde(default)]
    pub w9_captured_at: Option<Timestamp>,
    /// The tax ID from the W-9, masked by the engine (last four only).
    #[serde(default)]
    pub tin_masked: Option<String>,
    /// SSN or EIN.
    #[serde(default)]
    pub tin_type: Option<TinType>,
    /// Backup withholding applies.
    #[serde(default)]
    pub backup_withholding: bool,
    /// Why the vendor is left off 1099s (`corporation`, ...), if it is.
    #[serde(default)]
    pub exclusion_reason: Option<String>,
}

impl std::fmt::Debug for Vendor1099Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vendor1099Profile")
            .field("contact_id", &self.contact_id)
            .field("w9_on_file", &self.w9_on_file)
            .field("has_tin", &self.tin_masked.is_some())
            .finish_non_exhaustive()
    }
}

/// Changes to a vendor's 1099 profile; `None` leaves a field as it is.
/// The tax ID itself is kept on the contact (`ContactFields::tax_id`), the
/// field the engine's 1099 reports read.
#[derive(Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Vendor1099Update {
    /// SSN or EIN (`Some(None)` clears it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tin_type: Option<Option<TinType>>,
    /// Backup withholding applies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_withholding: Option<bool>,
}

impl std::fmt::Debug for Vendor1099Update {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vendor1099Update")
            .field("tin_type", &self.tin_type)
            .field("backup_withholding", &self.backup_withholding)
            .finish()
    }
}

impl Vendor1099Update {
    /// True when nothing would change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tin_type.is_none() && self.backup_withholding.is_none()
    }
}

#[derive(Deserialize)]
struct ProfileReply {
    profile: Vendor1099Profile,
}

/// 1099 operations of a session (`session.form1099()`).
#[derive(Clone, Copy, Debug)]
pub struct Form1099<'a>(pub(crate) &'a Session);

impl Session {
    /// Vendors' 1099 profiles of the open company.
    #[must_use]
    pub fn form1099(&self) -> Form1099<'_> {
        Form1099(self)
    }
}

fn profile_path(vendor: ContactId) -> String {
    format!("/api/v1/practice/1099/vendors/{vendor}/profile")
}

impl Form1099<'_> {
    /// The vendor's 1099 profile (an empty one when none was saved yet).
    ///
    /// # Errors
    /// `NotFound` for an unknown contact; any [`ApiError`].
    pub async fn profile(&self, vendor: ContactId) -> Result<Vendor1099Profile, ApiError> {
        let call = Call::new(
            Method::Get,
            VENDOR_1099_PROFILE.route(),
            profile_path(vendor),
        );
        let reply = self.0.send(&VENDOR_1099_PROFILE, call, None).await?;
        decode::<ProfileReply>(&reply, "1099 profile").map(|r| r.profile)
    }

    /// Saves changes to the vendor's 1099 profile. Setting the same values
    /// twice changes nothing, so the write may be resent.
    ///
    /// # Errors
    /// `NotFound`; `Validation`; any [`ApiError`].
    pub async fn update_profile(
        &self,
        vendor: ContactId,
        update: &Vendor1099Update,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let mut call = Call::new(
            Method::Put,
            VENDOR_1099_PROFILE_PUT.route(),
            profile_path(vendor),
        )
        .json_body(json_body(update)?);
        call.idempotency_key = Some(key);
        self.0.send(&VENDOR_1099_PROFILE_PUT, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json, requests};
    use serde_json::Value;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const VENDOR: &str = "3a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01";

    #[tokio::test]
    async fn profile_reads_masked_and_update_sends_only_changes() {
        let server = MockServer::start().await;
        let route = format!("/api/v1/practice/1099/vendors/{VENDOR}/profile");
        Mock::given(method("GET"))
            .and(path(route.as_str()))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"profile":{{"contactId":"{VENDOR}","is1099Eligible":true,"w9OnFile":true,
                    "w9CapturedAt":"2026-03-01T10:00:00.000Z","w9ExpiresAt":null,
                    "tinMasked":"***-**-6789","tinType":"EIN","backupWithholding":false,
                    "notes":null,"legalName":"Galena Gravel LLC","businessName":null,
                    "mailingAddress":{{"line1":null,"city":null,"state":null,"zip":null}},
                    "exclusionReason":null}}}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(route.as_str()))
            .respond_with(json(200, r#"{"ok":true}"#))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let id = ContactId::parse(VENDOR).expect("id");
        let p = s.form1099().profile(id).await.expect("profile");
        assert!(p.w9_on_file && p.is1099_eligible);
        assert_eq!(p.tin_type, Some(TinType::Ein));
        assert_eq!(p.tin_masked.as_deref(), Some("***-**-6789"));
        assert!(!format!("{p:?}").contains("6789"), "never in Debug");

        let update = Vendor1099Update {
            tin_type: Some(None),
            backup_withholding: Some(true),
        };
        assert!(!update.is_empty());
        assert!(Vendor1099Update::default().is_empty());
        s.form1099()
            .update_profile(id, &update, IdempotencyKey::new())
            .await
            .expect("put");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(
            put,
            serde_json::json!({"tinType": null, "backupWithholding": true})
        );
    }
}
