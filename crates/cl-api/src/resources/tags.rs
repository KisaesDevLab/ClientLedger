//! Tags (`/api/v1/tags`, P05 plan §2.8).
//!
//! Engine quirks kept exactly: the list query is **`snake_case`**
//! (`group_id`, `is_active`), the list answers `{tags, total, limit,
//! offset}`, and DELETE really deletes (409 `TAG_IN_USE` while used; a
//! duplicate name is a 409 without a code).
//!
//! Tag groups (P11 step 5) live under `/api/v1/tags/groups`: the list is
//! `GET /groups/list` (each group with its active tags), deleting a group
//! leaves its tags ungrouped, and groups carry no version (a save is a
//! plain PUT). The engine's `PUT /groups/reorder` is registered after
//! `PUT /groups/{id}` and never runs (U-41), so it is not wrapped. Merge
//! moves header tags only and fails on a tag used on a line or as a
//! default (U-41): callers check [`TagUsage::merge_safe`] first.

use cl_core::Timestamp;
use cl_core::ids::{TagGroupId, TagId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::common::{
    self, Query, Spec, editable_with_active, null_false, null_true, yes,
};
use crate::resources::operations::{
    TAG_DELETE, TAG_GET, TAG_GROUP_DELETE, TAG_GROUP_PUT, TAG_GROUPS_CREATE, TAG_GROUPS_LIST,
    TAG_MERGE, TAG_PUT, TAG_USAGE, TAGS_CREATE, TAGS_LIST,
};
use crate::resources::versioning::{EditableFields, RecordVersion, Versioned};
use crate::session::{Session, decode, json_body};
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

/// The fields of a tag group (also the create and update body).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroupFields {
    /// Name (1-100 characters).
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: Option<String>,
    /// One tag of this group per transaction or line (Q-179).
    #[serde(default, deserialize_with = "null_false")]
    pub is_single_select: bool,
}

/// A tag group as the engine lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagGroup {
    /// Id.
    pub id: TagGroupId,
    /// Fields.
    #[serde(flatten)]
    pub fields: TagGroupFields,
    /// Position among the groups.
    #[serde(default)]
    pub sort_order: Option<i64>,
    /// The group's active tags (list answers only).
    #[serde(default)]
    pub tags: Vec<Tag>,
}

/// Where a tag is used (`GET /tags/{id}/usage`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagUsage {
    /// Transaction lines carrying it.
    #[serde(default)]
    pub transaction_lines: u64,
    /// Transactions tagged with it (header tags).
    #[serde(default)]
    pub transactions: u64,
    /// Budgets.
    #[serde(default)]
    pub budgets: u64,
    /// Items with it as their default tag.
    #[serde(default)]
    pub items: u64,
    /// Vendors with it as their default tag.
    #[serde(default)]
    pub vendor_contacts: u64,
    /// Clients with it as their default tag.
    #[serde(default)]
    pub customer_contacts: u64,
    /// Bank rules that assign it.
    #[serde(default)]
    pub bank_rules: u64,
    /// Every use.
    #[serde(default)]
    pub total: u64,
}

impl TagUsage {
    /// True when the engine's merge can move every use: header tags only
    /// (until U-41 the merge leaves line tags and defaults behind and then
    /// fails on them).
    #[must_use]
    pub fn merge_safe(&self) -> bool {
        self.transaction_lines == 0
            && self.budgets == 0
            && self.items == 0
            && self.vendor_contacts == 0
            && self.customer_contacts == 0
            && self.bank_rules == 0
    }
}

impl Tags<'_> {
    /// Every tag group with its active tags, in the engine's order.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn groups(&self) -> Result<Vec<TagGroup>, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            groups: Vec<TagGroup>,
        }
        let reply = self
            .0
            .send(
                &TAG_GROUPS_LIST,
                Call::at(Method::Get, TAG_GROUPS_LIST.route()),
                None,
            )
            .await?;
        decode::<Body>(&reply, "tag groups").map(|b| b.groups)
    }

    /// Creates a tag group.
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`].
    pub async fn create_group(
        &self,
        fields: &TagGroupFields,
        key: IdempotencyKey,
    ) -> Result<TagGroup, ApiError> {
        let mut call =
            Call::at(Method::Post, TAG_GROUPS_CREATE.route()).json_body(json_body(fields)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&TAG_GROUPS_CREATE, call, None).await?;
        group_from(&reply.body)
    }

    /// Changes a tag group (groups carry no version: the last save wins).
    ///
    /// # Errors
    /// `Validation`; any [`ApiError`].
    pub async fn update_group(
        &self,
        id: TagGroupId,
        fields: &TagGroupFields,
        key: IdempotencyKey,
    ) -> Result<TagGroup, ApiError> {
        let mut call = Call::new(
            Method::Put,
            TAG_GROUP_PUT.route(),
            format!("/api/v1/tags/groups/{id}"),
        )
        .json_body(json_body(fields)?);
        call.idempotency_key = Some(key);
        let reply = self.0.send(&TAG_GROUP_PUT, call, None).await?;
        group_from(&reply.body)
    }

    /// Deletes a tag group; its tags stay, ungrouped.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete_group(&self, id: TagGroupId, key: IdempotencyKey) -> Result<(), ApiError> {
        let mut call = Call::new(
            Method::Delete,
            TAG_GROUP_DELETE.route(),
            format!("/api/v1/tags/groups/{id}"),
        );
        call.idempotency_key = Some(key);
        self.0.send(&TAG_GROUP_DELETE, call, None).await.map(drop)
    }

    /// Where a tag is used.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn usage(&self, id: TagId) -> Result<TagUsage, ApiError> {
        #[derive(Deserialize)]
        struct Body {
            usage: TagUsage,
        }
        let call = Call::new(
            Method::Get,
            TAG_USAGE.route(),
            format!("{}/{id}/usage", SPEC.base),
        );
        let reply = self.0.send(&TAG_USAGE, call, None).await?;
        decode::<Body>(&reply, "tag usage").map(|b| b.usage)
    }

    /// Merges `source` into `target`: header tags move and `source` is
    /// deleted. Until U-41 check [`TagUsage::merge_safe`] first.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a self-merge (nothing sent); any
    /// [`ApiError`].
    pub async fn merge(
        &self,
        source: TagId,
        target: TagId,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            source_tag_id: TagId,
            target_tag_id: TagId,
        }
        if source == target {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::at(Method::Post, TAG_MERGE.route()).json_body(json_body(&Body {
            source_tag_id: source,
            target_tag_id: target,
        })?);
        call.idempotency_key = Some(key);
        self.0.send(&TAG_MERGE, call, None).await.map(drop)
    }
}

fn group_from(body: &[u8]) -> Result<TagGroup, ApiError> {
    #[derive(Deserialize)]
    struct Body {
        group: TagGroup,
    }
    serde_json::from_slice::<Body>(body)
        .map(|b| b.group)
        .map_err(|_| {
            crate::resources::versioning::saved_unreadable(ApiError::Decode { what: "tag group" })
        })
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

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn groups_usage_and_merge() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tags/groups/list"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"groups":[{{"id":"6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Region",
                    "description":null,"isSingleSelect":null,"sortOrder":0,"tags":[{TAG}]}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tags/groups"))
            .respond_with(json(
                201,
                r#"{"group":{"id":"6a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01","name":"Crew","isSingleSelect":true}}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(200, r#"{"nope":1}"#))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .respond_with(json(200, r#"{"message":"Group deleted"}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(
                "/api/v1/tags/5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01/usage",
            ))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"tag":{TAG},"usage":{{"transactionLines":2,"transactions":1,"budgets":0,
                    "items":0,"vendorContacts":0,"customerContacts":0,"bankRules":0,"total":3}}}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tags/merge"))
            .respond_with(json(200, &format!(r#"{{"tag":{TAG}}}"#)))
            .mount(&server)
            .await;
        let s = company_session(&server);
        let groups = s.tags().groups().await.expect("groups");
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].fields.is_single_select, "null reads as false");
        assert_eq!(groups[0].tags.len(), 1);
        let fields = TagGroupFields {
            name: "Crew".into(),
            description: None,
            is_single_select: true,
        };
        let made = s
            .tags()
            .create_group(&fields, IdempotencyKey::new())
            .await
            .expect("create");
        assert!(made.fields.is_single_select);
        let post: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(post["isSingleSelect"], true);
        let unreadable = s
            .tags()
            .update_group(made.id, &fields, IdempotencyKey::new())
            .await
            .expect_err("no group in the answer");
        assert!(matches!(unreadable, ApiError::SavedUnreadable { .. }));
        s.tags()
            .delete_group(made.id, IdempotencyKey::new())
            .await
            .expect("delete");

        let tag: Tag = serde_json::from_str(TAG).expect("tag");
        let usage = s.tags().usage(tag.id).await.expect("usage");
        assert_eq!(usage.transaction_lines, 2);
        assert!(!usage.merge_safe());
        assert!(
            TagUsage {
                transactions: 4,
                total: 4,
                ..TagUsage::default()
            }
            .merge_safe()
        );
        let other = TagId::parse("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a02").expect("id");
        s.tags()
            .merge(tag.id, other, IdempotencyKey::new())
            .await
            .expect("merge");
        let me = s
            .tags()
            .merge(tag.id, tag.id, IdempotencyKey::new())
            .await
            .expect_err("self");
        assert_eq!(me, ApiError::Local(LocalValidation::InvalidValue));
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 2, "nothing sent for a self-merge");
        let merge: Value = serde_json::from_slice(&posts[1].body).expect("json");
        assert_eq!(merge["sourceTagId"], tag.id.to_string());
        assert_eq!(merge["targetTagId"], other.to_string());
    }
}
