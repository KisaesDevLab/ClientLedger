//! Tags (`/api/v1/tags`, P05 plan §2.8).
//!
//! Engine quirks kept exactly: the list query is **`snake_case`**
//! (`group_id`, `is_active`), the list answers `{tags, total, limit,
//! offset}`, and DELETE really deletes (409 `TAG_IN_USE` while used; a
//! duplicate name is a 409 without a code).

use cl_core::Timestamp;
use cl_core::ids::{TagGroupId, TagId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::ApiError;
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{self, Query, Spec, editable_with_active, null_true, yes};
use crate::resources::operations::{TAG_DELETE, TAG_GET, TAG_PUT, TAGS_CREATE, TAGS_LIST};
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned};
use crate::session::{Session, json_body};
use crate::wire::{Page, PageRequest};

pub(crate) const SPEC: Spec = Spec {
    list: &TAGS_LIST,
    create: &TAGS_CREATE,
    get: &TAG_GET,
    put: &TAG_PUT,
    delete: &TAG_DELETE,
    base: "/api/v1/tags",
    item_key: "tag",
    list_key: "tags",
};

/// The fields the tag form edits (also the create body).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagFields {
    /// Name (unique, 1-100 characters).
    pub name: String,
    /// `#rrggbb`.
    #[serde(default)]
    pub color: Option<String>,
    /// Tag group.
    #[serde(default)]
    pub group_id: Option<TagGroupId>,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Body of a tag update: every editable field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagUpdate {
    /// Form fields.
    #[serde(flatten)]
    pub fields: TagFields,
    /// Active.
    pub is_active: bool,
}

/// A tag as the engine returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    /// Id.
    pub id: TagId,
    /// Form fields.
    #[serde(flatten)]
    pub fields: TagFields,
    /// Active.
    #[serde(default = "yes", deserialize_with = "null_true")]
    pub is_active: bool,
    /// How often it is used (engine-maintained).
    #[serde(default)]
    pub usage_count: Option<i64>,
    /// Position in its group.
    #[serde(default)]
    pub sort_order: Option<i64>,
    /// Created.
    #[serde(default)]
    pub created_at: Option<Timestamp>,
    /// Last changed.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

impl EditableFields for Tag {
    const EDITABLE: &'static [&'static str] =
        &["name", "color", "groupId", "description", "isActive"];

    fn editable(&self) -> Value {
        editable_with_active(&self.fields, self.is_active, Self::EDITABLE)
    }
}

/// Filters of the tag list (no sort: the engine orders by position, then
/// name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagFilter {
    /// Tags of this group.
    pub group_id: Option<TagGroupId>,
    /// Active or inactive only.
    pub is_active: Option<bool>,
    /// Name contains.
    pub search: Option<String>,
}

impl TagFilter {
    /// Query pairs (**`snake_case`**, as the engine reads them).
    #[must_use]
    pub fn to_query(&self) -> Vec<(String, String)> {
        self.query().0
    }

    pub(crate) fn query(&self) -> Query {
        let mut q = Query::default();
        if let Some(g) = self.group_id {
            q.put("group_id", g.to_string());
        }
        q.opt_bool("is_active", self.is_active);
        q.opt_str("search", self.search.as_deref());
        q
    }
}

/// Tag operations of a session (`session.tags()`).
#[derive(Clone, Copy, Debug)]
pub struct Tags<'a>(pub(crate) &'a Session);

impl Session {
    /// Tags of the open company.
    #[must_use]
    pub fn tags(&self) -> Tags<'_> {
        Tags(self)
    }
}

impl Tags<'_> {
    /// One page of tags.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn list(&self, filter: &TagFilter, page: PageRequest) -> Result<Page<Tag>, ApiError> {
        common::list_page(self.0, &SPEC, filter.query(), page).await
    }

    /// Every matching tag (pages of 500, at most 100,000 rows).
    ///
    /// # Errors
    /// `Decode` beyond the cap; any [`ApiError`].
    pub async fn list_all(&self, filter: &TagFilter) -> Result<Vec<Tag>, ApiError> {
        common::list_all(self.0, &SPEC, &filter.query()).await
    }

    /// One tag with its version.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn get(&self, id: TagId) -> Result<Versioned<Tag>, ApiError> {
        common::get(self.0, &SPEC, &id.to_string()).await
    }

    /// Creates a tag.
    ///
    /// # Errors
    /// `Conflict { Unspecified }` for a used name; any [`ApiError`].
    pub async fn create(
        &self,
        new: &TagFields,
        key: IdempotencyKey,
    ) -> Result<Versioned<Tag>, ApiError> {
        common::create_named(
            self.0,
            &SPEC,
            "name",
            new.name.as_str(),
            &editable_with_active(new, true, Tag::EDITABLE),
            json_body(new)?,
            key,
        )
        .await
    }

    /// Saves a tag if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent); any [`ApiError`].
    pub async fn update(
        &self,
        id: TagId,
        expected: &RecordVersion,
        update: &TagUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Tag>, ApiError> {
        common::update(
            self.0,
            &SPEC,
            &id.to_string(),
            expected,
            json_body(update)?,
            key,
        )
        .await
    }

    /// Deletes a tag.
    ///
    /// # Errors
    /// `Conflict { InUse }` while transactions use it; any [`ApiError`].
    pub async fn delete(&self, id: TagId, key: IdempotencyKey) -> Result<(), ApiError> {
        common::delete_record(self.0, &SPEC, &id.to_string(), key)
            .await
            .map(drop)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ConflictReason;
    use crate::resources::testkit::{company_session, json, query, requests};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const TAG: &str = r#"{"id":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "name":"Canary Job","color":null,"groupId":null,"description":null,"isActive":true,
        "usageCount":0,"sortOrder":0,"createdAt":"2026-10-01T10:00:00.000Z"}"#;

    #[test]
    fn filter_query_is_snake_case() {
        let f = TagFilter {
            group_id: Some(TagGroupId::parse("6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("g")),
            is_active: Some(true),
            search: Some("job".into()),
        };
        let pairs: Vec<String> = f
            .to_query()
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        assert_eq!(
            pairs,
            [
                "group_id=6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
                "is_active=true",
                "search=job"
            ]
        );
    }

    #[tokio::test]
    async fn tags_page_shape_and_conflicts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tags"))
            .respond_with(json(
                200,
                &format!(r#"{{"tags":[{TAG}],"total":1,"limit":200,"offset":0}}"#),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"A tag with this name already exists"}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"in use","code":"TAG_IN_USE","details":{"tagName":"Canary Job"}}}"#,
            ))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let page = s
            .tags()
            .list(
                &TagFilter {
                    is_active: Some(false),
                    ..TagFilter::default()
                },
                PageRequest::first_max(),
            )
            .await
            .expect("list");
        assert_eq!(page.total, 1);
        assert_eq!(page.data[0].fields.name, "Canary Job");
        assert_eq!(
            query(&requests(&server, "GET").await[0]),
            "is_active=false&limit=500&offset=0"
        );
        let dup = s
            .tags()
            .create(&page.data[0].fields, IdempotencyKey::new())
            .await
            .expect_err("dup");
        assert!(matches!(
            dup,
            ApiError::Conflict {
                reason: ConflictReason::Unspecified,
                code: None,
                ..
            }
        ));
        let used = s
            .tags()
            .delete(page.data[0].id, IdempotencyKey::new())
            .await
            .expect_err("in use");
        assert!(matches!(
            used,
            ApiError::Conflict {
                reason: ConflictReason::InUse,
                ..
            }
        ));
        assert!(!format!("{used:?} {used}").contains("Canary Job"));
    }
}
