//! A vendor's 1099 profile (`/api/v1/practice/1099/vendors/{id}/profile`,
//! Q-53, P11): whether a W-9 is on file, the tax ID type, backup
//! withholding. The engine keeps the tax ID from a W-9 encrypted and only
//! ever returns it masked (`***-**-1234`).
//!
//! Profile data is owner data (names, tax IDs, addresses): never logged.
//! `Debug` of the update body never shows a tax ID.

use cl_core::ids::{AccountId, ContactId};
use cl_core::{Money, Timestamp};
use serde::{Deserialize, Serialize};

use crate::client::{Call, Method};
use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{
    COMPANY_1099_THRESHOLDS, COMPANY_1099_THRESHOLDS_PUT, FORM_1099_ACCOUNT_MAPPINGS_PUT,
    VENDOR_1099_PROFILE, VENDOR_1099_PROFILE_PUT,
};
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

impl Form1099<'_> {
    /// Marks `vendor` as one who gets a 1099 (or not).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn set_eligible(
        &self,
        vendor: ContactId,
        eligible: bool,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        let body = serde_json::json!({ "is1099Eligible": eligible });
        let mut call = Call::new(
            Method::Put,
            VENDOR_1099_PROFILE_PUT.route(),
            profile_path(vendor),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        self.0.send(&VENDOR_1099_PROFILE_PUT, call, None).await?;
        Ok(())
    }

    /// Replaces the accounts mapped to the 1099 form box `form_box` (the
    /// engine's value, like `NEC-1`).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a box that isn't letters, digits and a
    /// dash; any [`ApiError`].
    pub async fn set_account_mappings(
        &self,
        form_box: &str,
        accounts: &[AccountId],
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        if form_box.is_empty()
            || !form_box
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(ApiError::Local(
                crate::errors::LocalValidation::InvalidValue,
            ));
        }
        let body = serde_json::json!({ "accountIds": accounts });
        let mut call = Call::new(
            Method::Put,
            FORM_1099_ACCOUNT_MAPPINGS_PUT.route(),
            format!("/api/v1/practice/1099/account-mappings/{form_box}"),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        self.0
            .send(&FORM_1099_ACCOUNT_MAPPINGS_PUT, call, None)
            .await?;
        Ok(())
    }
}

// ---- P29 WP8 (U-215): the company's own 1099 threshold per box ----

/// The engine capability for [`Form1099::thresholds`] and
/// [`Form1099::set_thresholds`] (U-215).
pub const THRESHOLDS_1099: &str = "1099-thresholds-v1";

/// One 1099 box and its reporting threshold, as the engine reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Threshold1099Box {
    /// The engine's box code (`NEC-1`, `MISC-1`, ...). Kept as text so a
    /// newer engine's extra box still reads.
    pub code: String,
    /// The form (`1099-NEC`, `1099-MISC`).
    pub form: String,
    /// The box number on the form (`1`, `10`).
    pub box_number: String,
    /// The engine's label for the box.
    pub label: String,
    /// The company's own amount; `None` means the year's standard amount
    /// applies.
    pub amount: Option<Money>,
    /// The year's standard amount, when the engine reports it.
    pub standard: Option<Money>,
    /// The amount the 1099 reports use, when the engine reports it.
    pub effective: Option<Money>,
}

/// The company's 1099 thresholds for one year.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thresholds1099 {
    /// The year the standard amounts are for.
    pub year: i32,
    /// One row per box, in the engine's order.
    pub boxes: Vec<Threshold1099Box>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireThresholdBox {
    #[serde(rename = "box")]
    code: String,
    #[serde(default)]
    form: String,
    #[serde(default)]
    box_number: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    default_exact: Option<String>,
    #[serde(default)]
    effective_exact: Option<String>,
}

#[derive(Deserialize)]
struct WireThresholds {
    year: i32,
    boxes: Vec<WireThresholdBox>,
}

fn threshold_money(s: Option<&str>) -> Result<Option<Money>, ApiError> {
    s.map(|s| {
        Money::parse_wire(s).map_err(|_| ApiError::Decode {
            what: "1099 threshold",
        })
    })
    .transpose()
}

impl TryFrom<WireThresholds> for Thresholds1099 {
    type Error = ApiError;

    fn try_from(w: WireThresholds) -> Result<Self, ApiError> {
        let boxes = w
            .boxes
            .into_iter()
            .map(|b| {
                Ok(Threshold1099Box {
                    amount: threshold_money(b.amount.as_deref())?,
                    standard: threshold_money(b.default_exact.as_deref())?,
                    effective: threshold_money(b.effective_exact.as_deref())?,
                    code: b.code,
                    form: b.form,
                    box_number: b.box_number,
                    label: b.label,
                })
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        Ok(Thresholds1099 {
            year: w.year,
            boxes,
        })
    }
}

/// The body of a thresholds change: box code → amount (`None` clears it).
/// Only boxes in `changes` are sent; the engine keeps the others.
///
/// # Errors
/// `Local(InvalidValue)` for a negative amount or a box code that isn't
/// letters, digits and a dash.
fn thresholds_body(
    changes: &std::collections::BTreeMap<String, Option<Money>>,
) -> Result<serde_json::Value, ApiError> {
    let mut map = serde_json::Map::new();
    for (code, amount) in changes {
        let ok_code =
            !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !ok_code || amount.is_some_and(Money::is_negative) {
            return Err(ApiError::Local(
                crate::errors::LocalValidation::InvalidValue,
            ));
        }
        map.insert(
            code.clone(),
            amount.map_or(serde_json::Value::Null, |m| {
                serde_json::Value::String(m.to_wire())
            }),
        );
    }
    Ok(serde_json::json!({ "thresholds": map }))
}

impl Form1099<'_> {
    /// The company's 1099 thresholds for the current year (U-215,
    /// [`THRESHOLDS_1099`]).
    ///
    /// # Errors
    /// `Forbidden` without company-settings rights; any [`ApiError`].
    pub async fn thresholds(&self) -> Result<Thresholds1099, ApiError> {
        let call = Call::new(
            Method::Get,
            COMPANY_1099_THRESHOLDS.route(),
            COMPANY_1099_THRESHOLDS.route().to_owned(),
        );
        let reply = self.0.send(&COMPANY_1099_THRESHOLDS, call, None).await?;
        decode::<WireThresholds>(&reply, "1099 thresholds")?.try_into()
    }

    /// Sets or clears the company's own threshold for the boxes in
    /// `changes` (`None` = back to the year's standard amount) and returns
    /// what the engine now holds. Setting the same amounts again changes
    /// nothing, so the write may be resent.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a negative amount or a bad box code;
    /// `Forbidden` (`OWNER_REQUIRED`) for anyone but an Admin;
    /// `Validation`; any [`ApiError`].
    pub async fn set_thresholds(
        &self,
        changes: &std::collections::BTreeMap<String, Option<Money>>,
        key: IdempotencyKey,
    ) -> Result<Thresholds1099, ApiError> {
        let body = thresholds_body(changes)?;
        let mut call = Call::new(
            Method::Put,
            COMPANY_1099_THRESHOLDS_PUT.route(),
            COMPANY_1099_THRESHOLDS_PUT.route().to_owned(),
        )
        .json_body(json_body(&body)?);
        call.idempotency_key = Some(key);
        let reply = self
            .0
            .send(&COMPANY_1099_THRESHOLDS_PUT, call, None)
            .await?;
        decode::<WireThresholds>(&reply, "1099 thresholds")?.try_into()
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

    const THRESHOLDS: &str = r#"{"year":2026,"boxes":[
        {"box":"NEC-1","form":"1099-NEC","boxNumber":"1","label":"Nonemployee compensation",
         "amount":"1000.0000","defaultExact":"2000.0000","effectiveExact":"1000.0000"},
        {"box":"MISC-2","form":"1099-MISC","boxNumber":"2","label":"Royalties",
         "amount":null,"defaultExact":"10.0000","effectiveExact":"10.0000"},
        {"box":"MISC-99","form":"1099-MISC","boxNumber":"99","label":"Future box","amount":null}]}"#;

    #[tokio::test]
    async fn thresholds_read_as_money_and_only_changes_are_sent() {
        let server = MockServer::start().await;
        let route = "/api/v1/company/1099-thresholds";
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(json(200, THRESHOLDS))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path(route))
            .respond_with(json(200, THRESHOLDS))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let t = s.form1099().thresholds().await.expect("read");
        assert_eq!(t.year, 2026);
        assert_eq!(t.boxes.len(), 3, "an unknown box still reads");
        let nec = &t.boxes[0];
        assert_eq!(nec.code, "NEC-1");
        assert_eq!(nec.amount, Some(Money::parse_wire("1000").expect("m")));
        assert_eq!(nec.standard, Some(Money::parse_wire("2000").expect("m")));
        assert_eq!(t.boxes[1].amount, None);
        assert_eq!(t.boxes[2].standard, None, "never invented");

        let mut changes = std::collections::BTreeMap::new();
        changes.insert(
            "NEC-1".to_owned(),
            Some(Money::parse_wire("750.5").expect("m")),
        );
        changes.insert("MISC-2".to_owned(), None);
        s.form1099()
            .set_thresholds(&changes, IdempotencyKey::new())
            .await
            .expect("put");
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(
            put,
            serde_json::json!({"thresholds": {"NEC-1": "750.5000", "MISC-2": null}})
        );
    }

    #[test]
    fn thresholds_body_refuses_negatives_and_bad_codes() {
        let mut c = std::collections::BTreeMap::new();
        c.insert(
            "NEC-1".to_owned(),
            Some(Money::parse_wire("-1").expect("m")),
        );
        assert!(thresholds_body(&c).is_err());
        let mut c = std::collections::BTreeMap::new();
        c.insert("NEC 1".to_owned(), None);
        assert!(thresholds_body(&c).is_err());
        let mut c = std::collections::BTreeMap::new();
        c.insert("NEC-1".to_owned(), Some(Money::ZERO));
        assert_eq!(
            thresholds_body(&c).expect("ok"),
            serde_json::json!({"thresholds": {"NEC-1": "0.0000"}})
        );
    }

    #[test]
    fn a_malformed_amount_is_a_decode_error() {
        let w: WireThresholds =
            serde_json::from_str(r#"{"year":2026,"boxes":[{"box":"NEC-1","amount":"1e3"}]}"#)
                .expect("json");
        assert!(Thresholds1099::try_from(w).is_err());
    }
}
