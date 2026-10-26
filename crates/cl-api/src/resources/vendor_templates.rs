//! Vendor layout templates (A03, engine U-92, capability
//! `document-drafts-v1`): `/api/v1/vendor-layout-templates`.
//!
//! - A template records where one vendor's documents of one layout print
//!   each field, learned on this computer from accepted readings (A04). The
//!   engine stores it as bounded JSON keyed by (company, vendor,
//!   `fingerprint`) and never reads it.
//! - `fingerprint` is the desktop's storage key of the layout
//!   ([`layout_key`]); the layout's own tokens travel inside
//!   `field_regions`.
//! - `PUT` creates (`expected_updated_at: None`, 409 `STALE` when it
//!   exists) or replaces (the stored token, else 409 `STALE`); at most 500
//!   per company ([`crate::DraftRefusal::TemplateLimit`]).
//! - Without `document-drafts-v1` nothing is sent and every call answers
//!   `Local(Unsupported)`.
//! - Layout anchors may hold printed text: `Debug` shows ids and counts.

use std::fmt;
use std::fmt::Write as _;
use std::str::FromStr;

use cl_core::ids::{ContactId, IdError, parse_uuid};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::attachments::uuid_id;
use crate::resources::document_drafts::{bounded, supported};
use crate::resources::operations::{
    VENDOR_TEMPLATE_DELETE, VENDOR_TEMPLATES_LIST, VENDOR_TEMPLATES_PUT,
};
use crate::resources::versioning::saved_unreadable;
use crate::session::{Session, decode, json_body};

uuid_id!(
    /// One vendor layout template (`vendor_layout_templates.id`).
    VendorTemplateId
);

/// `fieldRegions`, as JSON bytes.
pub const TEMPLATE_REGIONS_BYTES: usize = 64 * 1024;
/// `lineTable`, as JSON bytes.
pub const TEMPLATE_LINE_TABLE_BYTES: usize = 32 * 1024;
/// Templates a company may have.
pub const TEMPLATES_PER_COMPANY: usize = 500;

/// The storage key of a layout: `l1:` and the SHA-256 of `parts` (each
/// followed by a line feed), 67 characters of `[a-z0-9:]`. The same parts
/// give the same key on every computer.
#[must_use]
pub fn layout_key<S: AsRef<str>>(parts: &[S]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_ref().as_bytes());
        h.update(b"\n");
    }
    let mut out = String::from("l1:");
    for b in h.finalize() {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A key the engine takes: 8 to 128 of `[A-Za-z0-9:_.-]`.
fn key_ok(k: &str) -> bool {
    (8..=128).contains(&k.len())
        && k.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'_' | b'.' | b'-'))
}

/// A stored vendor layout template.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VendorTemplate {
    /// Id.
    pub id: VendorTemplateId,
    /// The vendor.
    pub contact_id: ContactId,
    /// The layout's storage key.
    pub fingerprint: String,
    /// Where each field prints (desktop JSON).
    #[serde(default)]
    pub field_regions: Map<String, Value>,
    /// The line table (desktop JSON).
    #[serde(default)]
    pub line_table: Option<Map<String, Value>>,
    /// Accepted documents merged into it.
    #[serde(default)]
    pub samples: u32,
    /// Last applied (an opaque timestamp).
    #[serde(default)]
    pub last_used_at: Option<String>,
    /// The template's change token: pass it back unchanged.
    pub updated_at: String,
}

impl fmt::Debug for VendorTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VendorTemplate")
            .field("id", &self.id)
            .field("contact_id", &self.contact_id)
            .field("field_regions", &self.field_regions.len())
            .field("line_table", &self.line_table.is_some())
            .field("samples", &self.samples)
            .finish_non_exhaustive()
    }
}

/// A template to store.
#[derive(Clone, PartialEq, Eq)]
pub struct TemplateUpsert {
    /// The vendor.
    pub contact: ContactId,
    /// The layout's storage key ([`layout_key`]).
    pub fingerprint: String,
    /// Where each field prints (at most 64 KiB; integer numbers only).
    pub field_regions: Map<String, Value>,
    /// The line table (at most 32 KiB).
    pub line_table: Option<Map<String, Value>>,
    /// Accepted documents merged into it (1 to 1,000,000).
    pub samples: u32,
    /// The stored template's `updated_at` (replace), or `None` (create).
    pub expected_updated_at: Option<String>,
}

impl fmt::Debug for TemplateUpsert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TemplateUpsert")
            .field("contact", &self.contact)
            .field("field_regions", &self.field_regions.len())
            .field("line_table", &self.line_table.is_some())
            .field("samples", &self.samples)
            .field("replace", &self.expected_updated_at.is_some())
            .finish_non_exhaustive()
    }
}

impl TemplateUpsert {
    fn body(&self) -> Result<Value, ApiError> {
        let regions = Value::Object(self.field_regions.clone());
        let table = self.line_table.clone().map(Value::Object);
        if !key_ok(&self.fingerprint)
            || !(1..=1_000_000).contains(&self.samples)
            || !bounded(&regions, TEMPLATE_REGIONS_BYTES)
            || table
                .as_ref()
                .is_some_and(|t| !bounded(t, TEMPLATE_LINE_TABLE_BYTES))
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        Ok(json!({
            "contactId": self.contact,
            "fingerprint": self.fingerprint,
            "fieldRegions": regions,
            "lineTable": table,
            "samples": self.samples,
            "expectedUpdatedAt": self.expected_updated_at,
        }))
    }
}

#[derive(Deserialize)]
struct One {
    template: VendorTemplate,
}

/// Vendor layout template operations (`session.vendor_templates()`).
#[derive(Clone, Copy, Debug)]
pub struct VendorTemplates<'a>(pub(crate) &'a Session);

impl Session {
    /// Vendor layout templates of the open company (`document-drafts-v1`).
    #[must_use]
    pub fn vendor_templates(&self) -> VendorTemplates<'_> {
        VendorTemplates(self)
    }
}

impl VendorTemplates<'_> {
    /// The company's templates (at most 500), newest first; one vendor's
    /// with `contact`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `document-drafts-v1` (nothing sent);
    /// any [`ApiError`].
    pub async fn list(&self, contact: Option<ContactId>) -> Result<Vec<VendorTemplate>, ApiError> {
        #[derive(Deserialize)]
        struct All {
            templates: Vec<VendorTemplate>,
        }
        supported(self.0)?;
        let mut call = Call::at(Method::Get, VENDOR_TEMPLATES_LIST.route());
        if let Some(c) = contact {
            call.query = vec![("contactId".into(), c.to_string())];
        }
        let reply = self.0.send(&VENDOR_TEMPLATES_LIST, call, None).await?;
        decode::<All>(&reply, "vendor templates").map(|a| a.templates)
    }

    /// Creates or replaces the template of (vendor, `fingerprint`).
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a bad key, sample
    /// count or oversized JSON (nothing sent); `Conflict { Stale }`;
    /// [`crate::DraftRefusal::TemplateLimit`]; `NotFound` for a vendor that
    /// is not the company's; any [`ApiError`].
    pub async fn put(&self, template: &TemplateUpsert) -> Result<VendorTemplate, ApiError> {
        supported(self.0)?;
        let body = json_body(&template.body()?)?;
        let call = Call::at(Method::Put, VENDOR_TEMPLATES_PUT.route()).json_body(body);
        let reply = self.0.send(&VENDOR_TEMPLATES_PUT, call, None).await?;
        decode::<One>(&reply, "vendor template")
            .map(|o| o.template)
            .map_err(saved_unreadable)
    }

    /// Deletes a template; with `expected_updated_at`, only if it is still
    /// that version.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Conflict { Stale }`; `NotFound`; any
    /// [`ApiError`].
    pub async fn delete(
        &self,
        id: VendorTemplateId,
        expected_updated_at: Option<&str>,
    ) -> Result<(), ApiError> {
        supported(self.0)?;
        let mut call = Call::new(
            Method::Delete,
            VENDOR_TEMPLATE_DELETE.route(),
            VENDOR_TEMPLATE_DELETE
                .route()
                .replace("{id}", &id.to_string()),
        );
        if let Some(t) = expected_updated_at {
            call.query = vec![("expectedUpdatedAt".into(), t.to_string())];
        }
        self.0.send(&VENDOR_TEMPLATE_DELETE, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{company_session, json as reply, requests, session_with_caps};
    use crate::wire::Capabilities;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ROUTE: &str = "/api/v1/vendor-layout-templates";
    const VENDOR: &str = "00000000-0000-4000-8000-0000000000a1";
    const ID: &str = "00000000-0000-4000-8000-0000000000b1";
    const TOKEN: &str = "2026-11-04T10:00:00.123Z";

    fn stored() -> String {
        format!(
            r#"{{"template":{{"id":"{ID}","companyId":"0b0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
            "contactId":"{VENDOR}","fingerprint":"{}","fieldRegions":{{"v":1,"label":"Invoice Canary"}},
            "lineTable":null,"samples":2,"lastUsedAt":null,"createdAt":"{TOKEN}","updatedAt":"{TOKEN}"}}}}"#,
            layout_key(&["a", "b"])
        )
    }

    fn upsert() -> TemplateUpsert {
        let mut regions = Map::new();
        regions.insert("v".into(), json!(1));
        regions.insert("label".into(), json!("Invoice Canary"));
        TemplateUpsert {
            contact: ContactId::parse(VENDOR).expect("id"),
            fingerprint: layout_key(&["a", "b"]),
            field_regions: regions,
            line_table: None,
            samples: 2,
            expected_updated_at: None,
        }
    }

    async fn served(server: &MockServer) -> Session {
        session_with_caps(server, &[Capabilities::DOCUMENT_DRAFTS]).await
    }

    #[test]
    fn layout_keys_are_stable_and_accepted() {
        let k = layout_key(&["invoice", "#", "aspect:12941"]);
        assert_eq!(k, layout_key(&["invoice", "#", "aspect:12941"]));
        assert_ne!(k, layout_key(&["invoice#", "aspect:12941"]));
        assert_eq!(k.len(), 67);
        assert!(key_ok(&k));
        assert!(!key_ok("short"));
        assert!(!key_ok("has space in it"));
    }

    #[tokio::test]
    async fn put_list_delete_round_trip() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(ROUTE))
            .respond_with(reply(201, &stored()))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(ROUTE))
            .respond_with(reply(
                200,
                &format!(r#"{{"templates":[{}]}}"#, {
                    let s = stored();
                    s[12..s.len() - 1].to_string()
                }),
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
        let s = served(&server).await;
        let t = s.vendor_templates().put(&upsert()).await.expect("put");
        assert_eq!(t.samples, 2);
        assert_eq!(t.updated_at, TOKEN);
        let all = s
            .vendor_templates()
            .list(Some(ContactId::parse(VENDOR).expect("id")))
            .await
            .expect("list");
        assert_eq!(all, vec![t.clone()]);
        s.vendor_templates()
            .delete(t.id, Some(&t.updated_at))
            .await
            .expect("delete");

        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["expectedUpdatedAt"], Value::Null, "create only");
        assert_eq!(put["contactId"], VENDOR);
        assert_eq!(put["lineTable"], Value::Null);
        let gets = requests(&server, "GET").await;
        let get = gets.iter().find(|r| r.url.path() == ROUTE).expect("list");
        assert_eq!(
            get.url.query(),
            Some(format!("contactId={VENDOR}").as_str())
        );
        let del = &requests(&server, "DELETE").await[0];
        assert!(
            del.url
                .query()
                .is_some_and(|q| q.starts_with("expectedUpdatedAt="))
        );
    }

    #[tokio::test]
    async fn bad_input_and_no_capability_send_nothing() {
        let server = MockServer::start().await;
        let s = served(&server).await;
        let mut float = upsert();
        float.field_regions.insert(
            "x".into(),
            serde_json::from_str::<Value>("0.5").expect("a fractional JSON number"),
        );
        let mut deep = upsert();
        deep.field_regions
            .insert("d".into(), json!({"a":{"b":{"c":{"d":{"e":{"f":1}}}}}}));
        let mut key = upsert();
        key.fingerprint = "no".into();
        let mut none = upsert();
        none.samples = 0;
        for bad in [float, deep, key, none] {
            assert_eq!(
                s.vendor_templates().put(&bad).await,
                Err(ApiError::Local(LocalValidation::InvalidValue)),
                "{bad:?}"
            );
        }
        let off = company_session(&server);
        assert_eq!(
            off.vendor_templates().list(None).await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        assert_eq!(
            off.vendor_templates().put(&upsert()).await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        );
        let sent = server.received_requests().await.expect("recorded");
        assert!(sent.iter().all(|r| r.url.path() != ROUTE));
    }

    #[tokio::test]
    async fn the_template_limit_is_typed() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(reply(
                409,
                r#"{"error":{"message":"At most 500","code":"TEMPLATE_LIMIT"}}"#,
            ))
            .mount(&server)
            .await;
        let e = served(&server)
            .await
            .vendor_templates()
            .put(&upsert())
            .await
            .expect_err("limit");
        assert_eq!(
            e,
            ApiError::DocumentDraft(crate::resources::document_drafts::DraftRefusal::TemplateLimit)
        );
    }

    #[test]
    fn debug_never_shows_layout_text() {
        let t: One = serde_json::from_str(&stored()).expect("json");
        let text = format!("{:?} {:?}", t.template, upsert());
        assert!(!text.contains("Canary"), "{text}");
    }
}
