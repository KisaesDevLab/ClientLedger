//! Memorized reports (U-118, `report-defs-v1`, Q-P28d): definitions and
//! their groups, stored by the engine with a version (409 when stale), a
//! unique name per company and the Q-310 edit rule (only the author, an
//! owner or a super-admin changes one).
//!
//! The definition body is passed through as JSON: `cl-model` owns its
//! shape (`StandardDefinition`), and cl-api may not depend on it.

use cl_core::Timestamp;
use cl_core::ids::UserId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::resources::operations::{
    REPORT_DEF_DELETE, REPORT_DEF_GET, REPORT_DEF_GROUP_DELETE, REPORT_DEF_GROUP_PUT,
    REPORT_DEF_GROUPS_CREATE, REPORT_DEF_GROUPS_LIST, REPORT_DEF_PUT, REPORT_DEFS_CREATE,
    REPORT_DEFS_LIST,
};
use crate::session::Session;
use crate::wire::Capabilities;

/// The engine's cap on a standard definition body.
pub const MAX_DEFINITION_BYTES: usize = 64 * 1024;
/// The engine's cap on a Report Builder definition body (A07, PR-A).
pub const MAX_BUILDER_DEFINITION_BYTES: usize = 256 * 1024;

/// The cap for a body of `kind` (`builder` or `standard`).
#[must_use]
pub fn max_bytes(kind: &str) -> usize {
    if kind == "builder" {
        MAX_BUILDER_DEFINITION_BYTES
    } else {
        MAX_DEFINITION_BYTES
    }
}

/// One memorized report.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionRow {
    /// Its id.
    pub id: String,
    /// Its group (`None`: no group).
    #[serde(default)]
    pub group_id: Option<String>,
    /// Name (owner data).
    pub name: String,
    /// `standard` (A07 adds `builder`).
    pub kind: String,
    /// The body (`{schemaVersion, kind, reportKey, options}`).
    pub definition: Value,
    /// Version for the next change.
    pub version: u64,
    /// Who memorized it.
    #[serde(default)]
    pub created_by: Option<UserId>,
    /// Their name (owner data).
    #[serde(default)]
    pub created_by_name: Option<String>,
    /// Last change.
    #[serde(default)]
    pub updated_at: Option<Timestamp>,
}

/// One group of memorized reports.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionGroup {
    /// Its id.
    pub id: String,
    /// Name (owner data).
    pub name: String,
    /// Order in the catalog.
    #[serde(default)]
    pub sort: i64,
    /// Version for the next change.
    pub version: u64,
}

/// A definition to store.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionWrite {
    /// Name.
    pub name: String,
    /// Group (`None`: no group).
    pub group_id: Option<String>,
    /// `standard`.
    pub kind: String,
    /// The body.
    pub definition: Value,
}

/// Why the engine refused a memorized-report change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionRefusal {
    /// Someone changed it since it was read (409 `STALE_VERSION`).
    Stale,
    /// Another report has that name (409 `NAME_TAKEN`).
    NameTaken,
    /// Only its author, an owner or a super-admin may change it (403
    /// `NOT_AUTHOR`, Q-310).
    NotAuthor,
    /// The body is malformed (400 `DEFINITION_INVALID`).
    Invalid,
    /// Over 64 KB (413 `DEFINITION_TOO_LARGE`).
    TooLarge,
}

/// The memorized-report refusal in `e`, if it is one.
#[must_use]
pub fn definition_refusal(e: &ApiError) -> Option<DefinitionRefusal> {
    let code = match e {
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => return Some(DefinitionRefusal::Stale),
        ApiError::Conflict { code: Some(c), .. } | ApiError::Forbidden { code: Some(c) } => {
            c.as_str()
        }
        ApiError::Validation(v) => v.code().map_or("", |c| c.as_str()),
        ApiError::PayloadTooLarge => return Some(DefinitionRefusal::TooLarge),
        _ => return None,
    };
    Some(match code {
        "STALE_VERSION" => DefinitionRefusal::Stale,
        "NAME_TAKEN" => DefinitionRefusal::NameTaken,
        "NOT_AUTHOR" => DefinitionRefusal::NotAuthor,
        "DEFINITION_INVALID" => DefinitionRefusal::Invalid,
        "DEFINITION_TOO_LARGE" => DefinitionRefusal::TooLarge,
        _ => return None,
    })
}

/// Rows of a list answer: a bare array, or `{data: [...]}`.
fn rows<T: for<'de> Deserialize<'de>>(body: &[u8], what: &'static str) -> Result<Vec<T>, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let list = match v {
        Value::Array(_) => v,
        Value::Object(mut m) => m
            .remove("data")
            .or_else(|| m.remove("definitions"))
            .or_else(|| m.remove("groups"))
            .ok_or(ApiError::Decode { what })?,
        _ => return Err(ApiError::Decode { what }),
    };
    serde_json::from_value(list).map_err(|_| ApiError::Decode { what })
}

/// One row of an answer: bare, or under `data`.
fn one<T: for<'de> Deserialize<'de>>(body: &[u8], what: &'static str) -> Result<T, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let v = match v {
        Value::Object(mut m) if m.contains_key("data") => m.remove("data").unwrap_or_default(),
        v => v,
    };
    serde_json::from_value(v).map_err(|_| ApiError::Decode { what })
}

fn body(v: &impl Serialize, cap: usize) -> Result<Vec<u8>, ApiError> {
    let b = serde_json::to_vec(v).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
    if b.len() > cap {
        return Err(ApiError::PayloadTooLarge);
    }
    Ok(b)
}

/// Memorized reports of a session (`session.report_definitions()`).
#[derive(Clone, Copy, Debug)]
pub struct ReportDefinitions<'a>(&'a Session);

impl Session {
    /// Memorized reports of the open company (P28).
    #[must_use]
    pub fn report_definitions(&self) -> ReportDefinitions<'_> {
        ReportDefinitions(self)
    }
}

impl ReportDefinitions<'_> {
    /// A builder body needs `report-defs-builder-v1` (Q-220: an older
    /// engine would refuse the kind with a bare 400).
    fn require_kind(self, kind: &str) -> Result<(), ApiError> {
        self.require()?;
        if kind == "builder"
            && !self
                .0
                .client()
                .capabilities()
                .has(Capabilities::REPORT_DEFS_BUILDER)
        {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        Ok(())
    }

    fn require(self) -> Result<(), ApiError> {
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::REPORT_DEFS)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// Every memorized report the user can see.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `report-defs-v1`; any [`ApiError`].
    pub async fn list(&self) -> Result<Vec<DefinitionRow>, ApiError> {
        self.require()?;
        let call = Call::at(Method::Get, REPORT_DEFS_LIST.route());
        let reply = self.0.send(&REPORT_DEFS_LIST, call, None).await?;
        rows(&reply.body, "memorized reports")
    }

    /// One memorized report.
    ///
    /// # Errors
    /// As [`ReportDefinitions::list`].
    pub async fn get(&self, id: &str) -> Result<DefinitionRow, ApiError> {
        self.require()?;
        let call = Call::new(
            Method::Get,
            REPORT_DEF_GET.route(),
            REPORT_DEF_GET.route().replace("{id}", id),
        );
        let reply = self.0.send(&REPORT_DEF_GET, call, None).await?;
        one(&reply.body, "memorized report")
    }

    /// Memorizes a report.
    ///
    /// # Errors
    /// `PayloadTooLarge` over 64 KB (nothing sent); [`definition_refusal`]
    /// for the engine's refusals; any [`ApiError`].
    pub async fn create(&self, w: &DefinitionWrite) -> Result<DefinitionRow, ApiError> {
        self.require_kind(&w.kind)?;
        let mut call = Call::at(Method::Post, REPORT_DEFS_CREATE.route());
        call = call.json_body(body(w, max_bytes(&w.kind))?);
        let reply = self.0.send(&REPORT_DEFS_CREATE, call, None).await?;
        one(&reply.body, "memorized report")
    }

    /// Saves changes to the report read at `version`.
    ///
    /// # Errors
    /// As [`ReportDefinitions::create`]; 409 `STALE_VERSION` when it changed.
    pub async fn update(
        &self,
        id: &str,
        version: u64,
        w: &DefinitionWrite,
    ) -> Result<DefinitionRow, ApiError> {
        self.require_kind(&w.kind)?;
        let mut v =
            serde_json::to_value(w).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        if let Value::Object(m) = &mut v {
            m.insert("version".into(), Value::from(version));
        }
        let mut call = Call::new(
            Method::Put,
            REPORT_DEF_PUT.route(),
            REPORT_DEF_PUT.route().replace("{id}", id),
        );
        call = call.json_body(body(&v, max_bytes(&w.kind))?);
        let reply = self.0.send(&REPORT_DEF_PUT, call, None).await?;
        one(&reply.body, "memorized report")
    }

    /// Deletes the report read at `version` (soft delete).
    ///
    /// # Errors
    /// As [`ReportDefinitions::update`].
    pub async fn delete(&self, id: &str, version: u64) -> Result<(), ApiError> {
        self.require()?;
        let mut call = Call::new(
            Method::Delete,
            REPORT_DEF_DELETE.route(),
            REPORT_DEF_DELETE.route().replace("{id}", id),
        );
        call.query = vec![("version".into(), version.to_string())];
        self.0.send(&REPORT_DEF_DELETE, call, None).await?;
        Ok(())
    }

    /// The groups, in order.
    ///
    /// # Errors
    /// As [`ReportDefinitions::list`].
    pub async fn groups(&self) -> Result<Vec<DefinitionGroup>, ApiError> {
        self.require()?;
        let call = Call::at(Method::Get, REPORT_DEF_GROUPS_LIST.route());
        let reply = self.0.send(&REPORT_DEF_GROUPS_LIST, call, None).await?;
        rows(&reply.body, "memorized report groups")
    }

    /// Adds a group.
    ///
    /// # Errors
    /// As [`ReportDefinitions::create`].
    pub async fn create_group(&self, name: &str) -> Result<DefinitionGroup, ApiError> {
        self.require()?;
        let mut call = Call::at(Method::Post, REPORT_DEF_GROUPS_CREATE.route());
        call = call.json_body(body(
            &serde_json::json!({"name": name}),
            MAX_DEFINITION_BYTES,
        )?);
        let reply = self.0.send(&REPORT_DEF_GROUPS_CREATE, call, None).await?;
        one(&reply.body, "memorized report group")
    }

    /// Renames or moves a group read at `version`.
    ///
    /// # Errors
    /// As [`ReportDefinitions::update`].
    pub async fn update_group(
        &self,
        id: &str,
        version: u64,
        name: &str,
        sort: i64,
    ) -> Result<DefinitionGroup, ApiError> {
        self.require()?;
        let mut call = Call::new(
            Method::Put,
            REPORT_DEF_GROUP_PUT.route(),
            REPORT_DEF_GROUP_PUT.route().replace("{id}", id),
        );
        call = call.json_body(body(
            &serde_json::json!({"name": name, "sort": sort, "version": version}),
            MAX_DEFINITION_BYTES,
        )?);
        let reply = self.0.send(&REPORT_DEF_GROUP_PUT, call, None).await?;
        one(&reply.body, "memorized report group")
    }

    /// Deletes a group read at `version`; its reports move to no group.
    ///
    /// # Errors
    /// As [`ReportDefinitions::update`].
    pub async fn delete_group(&self, id: &str, version: u64) -> Result<(), ApiError> {
        self.require()?;
        let mut call = Call::new(
            Method::Delete,
            REPORT_DEF_GROUP_DELETE.route(),
            REPORT_DEF_GROUP_DELETE.route().replace("{id}", id),
        );
        call.query = vec![("version".into(), version.to_string())];
        self.0.send(&REPORT_DEF_GROUP_DELETE, call, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = r#"{
        "id": "d1", "groupId": null, "name": "Monthly P&L for the bank",
        "kind": "standard",
        "definition": {"schemaVersion": 1, "kind": "standard", "reportKey": "profit_loss",
                       "options": {"dates": {"preset": "last_month"}, "columns": "total"}},
        "version": 3, "createdBy": null, "createdByName": "Kurt",
        "updatedAt": "2026-11-04T15:41:00.000Z"
    }"#;

    #[test]
    fn rows_read_bare_or_wrapped() {
        let bare = format!("[{ROW}]");
        let wrapped = format!("{{\"data\": [{ROW}]}}");
        for text in [bare, wrapped] {
            let v: Vec<DefinitionRow> = rows(text.as_bytes(), "x").expect("rows");
            assert_eq!(v.len(), 1);
            assert_eq!(v[0].version, 3);
            assert_eq!(v[0].definition["reportKey"], "profit_loss");
        }
        let r: DefinitionRow = one(format!("{{\"data\": {ROW}}}").as_bytes(), "x").expect("one");
        assert_eq!(r.name, "Monthly P&L for the bank");
    }

    #[test]
    fn refusals_are_recognised() {
        let stale = crate::engine_refusal(
            409,
            &serde_json::json!({"error": {"message": "x", "code": "STALE_VERSION", "details": {"current": 4}}}),
        );
        assert_eq!(definition_refusal(&stale), Some(DefinitionRefusal::Stale));
        let taken = crate::engine_refusal(
            409,
            &serde_json::json!({"error": {"message": "x", "code": "NAME_TAKEN"}}),
        );
        assert_eq!(
            definition_refusal(&taken),
            Some(DefinitionRefusal::NameTaken)
        );
        let author = crate::engine_refusal(
            403,
            &serde_json::json!({"error": {"message": "x", "code": "NOT_AUTHOR"}}),
        );
        assert_eq!(
            definition_refusal(&author),
            Some(DefinitionRefusal::NotAuthor)
        );
        assert_eq!(definition_refusal(&ApiError::Timeout), None);
    }

    #[test]
    fn a_body_over_64_kb_is_refused_before_sending() {
        let w = DefinitionWrite {
            name: "big".into(),
            group_id: None,
            kind: "standard".into(),
            definition: Value::String("x".repeat(MAX_DEFINITION_BYTES)),
        };
        assert!(matches!(
            body(&w, max_bytes(&w.kind)),
            Err(ApiError::PayloadTooLarge)
        ));
    }

    #[test]
    fn a_builder_body_may_be_four_times_larger() {
        let mut w = DefinitionWrite {
            name: "big".into(),
            group_id: None,
            kind: "builder".into(),
            definition: Value::String("x".repeat(MAX_DEFINITION_BYTES)),
        };
        assert!(body(&w, max_bytes(&w.kind)).is_ok());
        w.definition = Value::String("x".repeat(MAX_BUILDER_DEFINITION_BYTES));
        assert!(matches!(
            body(&w, max_bytes(&w.kind)),
            Err(ApiError::PayloadTooLarge)
        ));
    }
}
