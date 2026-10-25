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
//!
//! Tag numbers (Q-270, U-75, `tag-number-v1`): optional, at most 20
//! characters, unique per company when set (409 `TAG_NUMBER_EXISTS`), sent
//! trimmed, `""` clears, left out of a PUT keeps the stored number (engine
//! PR #168, migration 0228). Without the capability `tagNumber` is never sent:
//! a number on such an engine is refused locally (nothing sent), an empty
//! one is dropped.

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
use crate::resources::reconcile::row_fp;
use crate::resources::versioning::{
    EditableFields, GuardedUpdate, RecordVersion, Versioned, fingerprint_of, pick, update_guarded,
};
use crate::session::{Session, decode, json_body};
use crate::wire::{Capabilities, Page, PageRequest};

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
    /// Tag number (Q-270, `tag-number-v1`): `None` sends nothing (a PUT
    /// keeps the stored number), `Some("")` clears it. Engines without the
    /// capability never receive it. With the capability, forms send the
    /// number they show (`Some`), so a save whose answer was lost can be
    /// confirmed by its fingerprint; with `None` such a save stays
    /// `OutcomeUnknown` (safe, never re-sent blindly).
    #[serde(rename = "tagNumber", default, skip_serializing_if = "Option::is_none")]
    pub tag_number: Option<String>,
}

impl TagFields {
    /// These fields as they go on the wire: the number trimmed; without
    /// `tag-number-v1` no number at all (an empty one is dropped, a real
    /// one refused).
    fn for_engine(&self, numbers: bool) -> Result<TagFields, ApiError> {
        let mut out = self.clone();
        out.tag_number = self.tag_number.as_deref().map(|n| n.trim().to_string());
        if !numbers {
            if out.tag_number.as_deref().is_some_and(|n| !n.is_empty()) {
                return Err(ApiError::Local(LocalValidation::InvalidValue));
            }
            out.tag_number = None;
        }
        Ok(out)
    }
}

/// The editable JSON as the engine will store it: an empty number reads
/// back as `null`.
fn settled(mut v: Value) -> Value {
    if let Some(n) = v.get_mut("tagNumber")
        && n.as_str().is_some_and(|s| s.trim().is_empty())
    {
        *n = Value::Null;
    }
    v
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
    /// `tagNumber` reads as `null` from an engine without numbers (an absent
    /// key and a `None` both pick as `null`), so old records keep their
    /// fingerprint.
    const EDITABLE: &'static [&'static str] = &[
        "name",
        "color",
        "groupId",
        "description",
        "tagNumber",
        "isActive",
    ];

    fn editable(&self) -> Value {
        settled(editable_with_active(
            &self.fields,
            self.is_active,
            Self::EDITABLE,
        ))
    }
}

/// Filters of the tag list (no sort: the engine orders by position, then,
/// with `tag-number-v1`, number as text with unnumbered last, then name).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagFilter {
    /// Tags of this group.
    pub group_id: Option<TagGroupId>,
    /// Active or inactive only.
    pub is_active: Option<bool>,
    /// Name contains (with `tag-number-v1` the engine also matches the
    /// number).
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

    /// True when the engine stores tag numbers (`tag-number-v1`).
    #[must_use]
    pub fn numbers(&self) -> bool {
        self.0.client().capabilities().has(Capabilities::TAG_NUMBER)
    }

    /// Creates a tag.
    ///
    /// # Errors
    /// `Conflict { Unspecified }` for a used name; `Conflict {
    /// TagNumberTaken }` for a used number; `Local(InvalidValue)` for a
    /// number on an engine without `tag-number-v1` (nothing sent); any
    /// [`ApiError`].
    pub async fn create(
        &self,
        new: &TagFields,
        key: IdempotencyKey,
    ) -> Result<Versioned<Tag>, ApiError> {
        let new = new.for_engine(self.numbers())?;
        common::create_named(
            self.0,
            &SPEC,
            "name",
            new.name.as_str(),
            &settled(editable_with_active(&new, true, Tag::EDITABLE)),
            json_body(&new)?,
            key,
        )
        .await
    }

    /// Saves a tag if it is still at `expected` (§2.5).
    ///
    /// # Errors
    /// `Conflict { Stale }` (nothing sent); `Conflict { TagNumberTaken }`;
    /// `Local(InvalidValue)` for a number on an engine without
    /// `tag-number-v1` (nothing sent); any [`ApiError`].
    pub async fn update(
        &self,
        id: TagId,
        expected: &RecordVersion,
        update: &TagUpdate,
        key: IdempotencyKey,
    ) -> Result<Versioned<Tag>, ApiError> {
        let update = TagUpdate {
            fields: update.fields.for_engine(self.numbers())?,
            is_active: update.is_active,
        };
        let body = json_body(&update)?;
        // An empty number clears it: the record then reads `null`.
        let sent: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let requested = fingerprint_of(&settled(pick(&sent, Tag::EDITABLE)));
        let u = GuardedUpdate {
            requested,
            fp: row_fp::<Tag>,
            stamp_covered: false,
            get: SPEC.get,
            put: SPEC.put,
            path: SPEC.path(&id.to_string()),
            key: SPEC.item_key,
            body,
            idempotency_key: key,
        };
        update_guarded(self.0, expected, u).await
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
    /// True when an engine without `merge-closing-date-v1` can move every
    /// use: header tags only (that merge leaves line tags and defaults
    /// behind and then fails on them). With the capability every use moves.
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
    /// deleted. With `merge-closing-date-v1` (U-47) line tags and default
    /// tags move too and a merge that re-tags a transaction dated on or
    /// before the closing date needs `closing_password`; without it check
    /// [`TagUsage::merge_safe`] first.
    ///
    /// # Errors
    /// Nothing is sent for: `Local(InvalidValue)` (a self-merge);
    /// `Local(ClosedPeriodOverrideUnavailable)` (a password without
    /// `merge-closing-date-v1`). From the engine: `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `ClosingPasswordRateLimited`; any
    /// [`ApiError`].
    pub async fn merge(
        &self,
        source: TagId,
        target: TagId,
        closing_password: Option<&cl_core::Secret>,
        key: IdempotencyKey,
    ) -> Result<(), ApiError> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Body<'a> {
            source_tag_id: TagId,
            target_tag_id: TagId,
            #[serde(skip_serializing_if = "Option::is_none")]
            closing_password: Option<&'a str>,
        }
        if source == target {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let closing_password = super::contacts::merge_password(self.0, closing_password)?;
        let mut call = Call::at(Method::Post, TAG_MERGE.route()).json_body(json_body(&Body {
            source_tag_id: source,
            target_tag_id: target,
            closing_password,
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
    use crate::resources::testkit::{company_session, json, query, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    pub(crate) const TAG: &str = r#"{"id":"5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01",
        "name":"Canary Job","color":null,"groupId":null,"description":null,"isActive":true,
        "usageCount":0,"sortOrder":0,"createdAt":"2026-10-01T10:00:00.000Z"}"#;

    fn fields(number: Option<&str>) -> TagFields {
        TagFields {
            name: "Lake Area".into(),
            color: None,
            group_id: None,
            description: None,
            tag_number: number.map(str::to_string),
        }
    }

    #[test]
    fn tag_number_serde() {
        // No number: no key at all, so an engine without `tag-number-v1`
        // sees exactly the old body.
        let v = serde_json::to_value(fields(None)).expect("json");
        assert!(v.get("tagNumber").is_none(), "{v}");
        let v = serde_json::to_value(fields(Some("10"))).expect("json");
        assert_eq!(v["tagNumber"], "10");
        // Old engines send no key; new ones send null or the number.
        let old: Tag = serde_json::from_str(TAG).expect("old");
        assert_eq!(old.fields.tag_number, None);
        let new: Tag = serde_json::from_str(
            &TAG.replace(r#""groupId":null"#, r#""groupId":null,"tagNumber":"10""#),
        )
        .expect("new");
        assert_eq!(new.fields.tag_number.as_deref(), Some("10"));
        let null: Tag = serde_json::from_str(
            &TAG.replace(r#""groupId":null"#, r#""groupId":null,"tagNumber":null"#),
        )
        .expect("null");
        assert_eq!(null.fingerprint(), old.fingerprint(), "null = absent");
        assert_ne!(new.fingerprint(), old.fingerprint());
        assert!(Tag::EDITABLE.contains(&"tagNumber"));
    }

    #[test]
    fn tag_number_on_the_wire() {
        // Trimmed; an empty number clears and fingerprints as none.
        let t = fields(Some(" 10 ")).for_engine(true).expect("ok");
        assert_eq!(t.tag_number.as_deref(), Some("10"));
        let cleared = fields(Some("")).for_engine(true).expect("ok");
        assert_eq!(cleared.tag_number.as_deref(), Some(""));
        assert_eq!(
            settled(editable_with_active(&cleared, true, Tag::EDITABLE)),
            settled(editable_with_active(&fields(None), true, Tag::EDITABLE)),
        );
        // Without the capability: nothing sent, a real number refused.
        assert_eq!(fields(None).for_engine(false), Ok(fields(None)));
        assert_eq!(fields(Some(" ")).for_engine(false), Ok(fields(None)));
        assert_eq!(
            fields(Some("10")).for_engine(false),
            Err(ApiError::Local(LocalValidation::InvalidValue))
        );
    }

    #[tokio::test]
    async fn numbers_follow_the_capability() {
        let saved = TAG.replace(r#""groupId":null"#, r#""groupId":null,"tagNumber":"10""#);
        // Without `tag-number-v1`: no key goes out, a number is refused
        // before anything is sent.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tags"))
            .respond_with(json(200, r#"{"tags":[],"total":0,"limit":500,"offset":0}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tags"))
            .respond_with(json(201, &format!(r#"{{"tag":{TAG}}}"#)))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[]).await;
        assert!(!s.tags().numbers());
        let refused = s
            .tags()
            .create(&fields(Some("10")), IdempotencyKey::new())
            .await
            .expect_err("refused");
        assert_eq!(refused, ApiError::Local(LocalValidation::InvalidValue));
        assert!(requests(&server, "POST").await.is_empty(), "nothing sent");
        s.tags()
            .create(&fields(Some("")), IdempotencyKey::new())
            .await
            .expect("create");
        let posts = requests(&server, "POST").await;
        let body: Value = serde_json::from_slice(&posts[0].body).expect("json");
        assert!(body.get("tagNumber").is_none(), "{body}");

        // With it: the number goes out trimmed; a used one is typed.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tags"))
            .respond_with(json(200, r#"{"tags":[],"total":0,"limit":500,"offset":0}"#))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tags"))
            .respond_with(json(201, &format!(r#"{{"tag":{saved}}}"#)))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/tags"))
            .respond_with(json(
                409,
                r#"{"error":{"message":"Tag number 10 is used by Lake Area","code":"TAG_NUMBER_EXISTS"}}"#,
            ))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::TAG_NUMBER]).await;
        assert!(s.tags().numbers());
        let made = s
            .tags()
            .create(&fields(Some(" 10 ")), IdempotencyKey::new())
            .await
            .expect("create");
        assert_eq!(made.record.fields.tag_number.as_deref(), Some("10"));
        let body: Value =
            serde_json::from_slice(&requests(&server, "POST").await[0].body).expect("json");
        assert_eq!(body["tagNumber"], "10");
        let dup = s
            .tags()
            .create(&fields(Some("10")), IdempotencyKey::new())
            .await
            .expect_err("used");
        assert!(matches!(
            dup,
            ApiError::Conflict {
                reason: ConflictReason::TagNumberTaken,
                ..
            }
        ));
        assert!(!format!("{dup:?} {dup}").contains("Lake"));
    }

    #[tokio::test]
    async fn clearing_a_number_sends_empty_and_settles_as_none() {
        let numbered = TAG.replace(r#""groupId":null"#, r#""groupId":null,"tagNumber":"10""#);
        let cleared = TAG.replace(r#""groupId":null"#, r#""groupId":null,"tagNumber":null"#);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/tags/5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01"))
            .respond_with(json(200, &format!(r#"{{"tag":{numbered}}}"#)))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .respond_with(json(200, &format!(r#"{{"tag":{cleared}}}"#)))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::TAG_NUMBER]).await;
        let got = s
            .tags()
            .get(TagId::parse("5a0c4e8e-6f6e-4c1d-8a37-6c1f4d8e9a01").expect("id"))
            .await
            .expect("get");
        let mut f = got.record.fields.clone();
        f.tag_number = Some(String::new());
        let saved = s
            .tags()
            .update(
                got.record.id,
                &got.version,
                &TagUpdate {
                    fields: f,
                    is_active: true,
                },
                IdempotencyKey::new(),
            )
            .await
            .expect("update");
        assert_eq!(saved.record.fields.tag_number, None);
        let put: Value =
            serde_json::from_slice(&requests(&server, "PUT").await[0].body).expect("json");
        assert_eq!(put["tagNumber"], "");
    }

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
            .merge(tag.id, other, None, IdempotencyKey::new())
            .await
            .expect("merge");
        let me = s
            .tags()
            .merge(tag.id, tag.id, None, IdempotencyKey::new())
            .await
            .expect_err("self");
        assert_eq!(me, ApiError::Local(LocalValidation::InvalidValue));
        let posts = requests(&server, "POST").await;
        assert_eq!(posts.len(), 2, "nothing sent for a self-merge");
        let merge: Value = serde_json::from_slice(&posts[1].body).expect("json");
        assert_eq!(merge["sourceTagId"], tag.id.to_string());
        assert_eq!(merge["targetTagId"], other.to_string());
        assert!(merge.get("closingPassword").is_none());
        let pw = cl_core::Secret::new("pw-canary-123");
        let gated = s
            .tags()
            .merge(tag.id, other, Some(&pw), IdempotencyKey::new())
            .await
            .expect_err("no merge-closing-date-v1");
        assert_eq!(
            gated,
            ApiError::Local(LocalValidation::ClosedPeriodOverrideUnavailable)
        );
        assert_eq!(requests(&server, "POST").await.len(), 2, "nothing sent");
    }
}
