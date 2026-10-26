//! The demo "engine" for document drafts (A04 step 6, WP-D): the engine's
//! `document-drafts-v1` and U-95 rules, synthetic, `--demo` only. WP-D
//! builds it: the content-hash claim, stale tokens, closed drafts, the
//! duplicate rule, the closing date, the relink on convert, attach, the
//! claimed-file guard and keyed replay; the parity test runs
//! `fixtures/docs/draft-cases.json` against it and the live engine (T16).
//!
//! Frozen by WP-M: [`DemoDrafts`] lives on `DemoState::drafts`; its
//! methods mirror `cl_api::DocumentDrafts` method for method (same names,
//! same arguments after the tenant, same `Result<_, cl_api::ApiError>`
//! answers), synchronous, so the desktop's draft code takes the demo or the
//! live engine alike. Posting (`convert`) and `attach` need the demo ledger
//! and bills, so WP-D adds them as functions taking `&mut DemoState`. Until
//! WP-D, [`DemoDrafts::supported`] is false and every call answers
//! `Local(Unsupported)`, as an engine without `document-drafts-v1` does
//! (nothing changes).

use cl_api::{
    ApiError, AttachmentId, DocumentDraft, DocumentDraftId, DraftKind, DraftListQuery, DraftPage,
    DraftReading, DuplicateMatch, IdempotencyKey, LocalValidation,
};
use cl_core::ids::TenantId;

/// The demo engine's document drafts, every company's.
#[derive(Clone, Debug, Default)]
pub struct DemoDrafts {
    /// The demo engine advertises `document-drafts-v1` and
    /// `document-drafts-attach-v1` (WP-D turns it on).
    pub enabled: bool,
}

/// What an engine without the capability answers: nothing sent, nothing
/// changed.
fn unsupported<T>() -> Result<T, ApiError> {
    Err(ApiError::Local(LocalValidation::Unsupported))
}

impl DemoDrafts {
    /// Whether the demo engine has document drafts now.
    #[must_use]
    pub fn supported(&self) -> bool {
        self.enabled
    }

    /// `DocumentDrafts::create`: claims an uploaded draft file.
    ///
    /// # Errors
    /// As the engine (see `cl_api::DocumentDrafts::create`).
    pub fn create(
        &mut self,
        tenant: TenantId,
        attachment: AttachmentId,
        kind_hint: DraftKind,
        key: IdempotencyKey,
    ) -> Result<DocumentDraft, ApiError> {
        let _ = (tenant, attachment, kind_hint, key);
        unsupported()
    }

    /// `DocumentDrafts::list`.
    ///
    /// # Errors
    /// As the engine.
    pub fn list(&self, tenant: TenantId, query: &DraftListQuery) -> Result<DraftPage, ApiError> {
        let _ = (tenant, query);
        unsupported()
    }

    /// `DocumentDrafts::get`.
    ///
    /// # Errors
    /// As the engine.
    pub fn get(&self, tenant: TenantId, id: DocumentDraftId) -> Result<DocumentDraft, ApiError> {
        let _ = (tenant, id);
        unsupported()
    }

    /// `DocumentDrafts::duplicates`.
    ///
    /// # Errors
    /// As the engine.
    pub fn duplicates(
        &self,
        tenant: TenantId,
        id: DocumentDraftId,
    ) -> Result<Vec<DuplicateMatch>, ApiError> {
        let _ = (tenant, id);
        unsupported()
    }

    /// `DocumentDrafts::store_reading`.
    ///
    /// # Errors
    /// As the engine.
    pub fn store_reading(
        &mut self,
        tenant: TenantId,
        id: DocumentDraftId,
        expected_updated_at: &str,
        reading: &DraftReading,
    ) -> Result<DocumentDraft, ApiError> {
        let _ = (tenant, id, expected_updated_at, reading);
        unsupported()
    }

    /// `DocumentDrafts::store_failure`.
    ///
    /// # Errors
    /// As the engine.
    pub fn store_failure(
        &mut self,
        tenant: TenantId,
        id: DocumentDraftId,
        expected_updated_at: &str,
        reason: &str,
    ) -> Result<DocumentDraft, ApiError> {
        let _ = (tenant, id, expected_updated_at, reason);
        unsupported()
    }

    /// `DocumentDrafts::discard`.
    ///
    /// # Errors
    /// As the engine.
    pub fn discard(
        &mut self,
        tenant: TenantId,
        id: DocumentDraftId,
    ) -> Result<DocumentDraft, ApiError> {
        let _ = (tenant, id);
        unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sections of `fixtures/docs/draft-cases.json` (WP-M froze them;
    /// later work packages only append cases).
    const SECTIONS: [&str; 7] = [
        "claim",
        "store",
        "discard",
        "convert",
        "attach",
        "guard",
        "amount_filter",
    ];

    #[test]
    fn the_shared_cases_keep_their_shape() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/docs/draft-cases.json");
        let text = std::fs::read_to_string(path).expect("draft-cases.json");
        let v: serde_json::Value = serde_json::from_str(&text).expect("json");
        let ops: Vec<&str> = v["_schema"]["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        let mut names = std::collections::HashSet::new();
        for section in SECTIONS {
            for case in v[section].as_array().expect(section) {
                let name = case["name"].as_str().expect("name");
                assert!(names.insert(name.to_owned()), "{name} twice");
                assert!(
                    matches!(case["owner"].as_str(), Some("WP-D" | "WP-E")),
                    "{name}"
                );
                for step in case["steps"].as_array().expect("steps") {
                    let op = step["op"].as_str().expect("op");
                    assert!(ops.contains(&op), "{name}: {op}");
                    assert!(step["expect"]["status"].is_u64(), "{name}: {op}");
                }
            }
        }
        let keys: Vec<&String> = v.as_object().expect("object").keys().collect();
        assert_eq!(keys.len(), SECTIONS.len() + 2, "{keys:?}");
    }

    #[test]
    fn without_drafts_every_call_is_unsupported_and_changes_nothing() {
        let mut d = DemoDrafts::default();
        assert!(!d.supported());
        let tenant = TenantId::parse("00000000-0000-4000-8000-000000000001").expect("tenant");
        let file = AttachmentId::parse("00000000-0000-4000-8000-000000000002").expect("file");
        let id = DocumentDraftId::parse("00000000-0000-4000-8000-000000000003").expect("id");
        let key = IdempotencyKey::new();
        let un = |e: ApiError| matches!(e, ApiError::Local(LocalValidation::Unsupported));
        assert!(un(d
            .create(tenant, file, DraftKind::Bill, key)
            .expect_err("no")));
        assert!(un(d
            .list(tenant, &DraftListQuery::default())
            .expect_err("no")));
        assert!(un(d.get(tenant, id).expect_err("no")));
        assert!(un(d.duplicates(tenant, id).expect_err("no")));
        assert!(un(d
            .store_failure(tenant, id, "t", "gave_up")
            .expect_err("no")));
        assert!(un(d.discard(tenant, id).expect_err("no")));
    }
}
