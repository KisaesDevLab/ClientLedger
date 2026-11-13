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
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{
    REPORT_DEF_DELETE, REPORT_DEF_GET, REPORT_DEF_GROUP_DELETE, REPORT_DEF_GROUP_PUT,
    REPORT_DEF_GROUPS_CREATE, REPORT_DEF_GROUPS_LIST, REPORT_DEF_PUT, REPORT_DEFS_CREATE,
    REPORT_DEFS_EXPORT, REPORT_DEFS_IMPORT_PREVIEW, REPORT_DEFS_LIST,
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
    /// The user may change or delete it (its author, an owner or a super
    /// admin, Q-310). `false` when an older engine doesn't say: Save
    /// changes then stays off and a refusal is never invited (A08).
    #[serde(default)]
    pub can_edit: bool,
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
    /// Where it came from, for the engine's audit row: `Some("import")`
    /// for a report read from a `.clreport` file (A09, U-220); `None`
    /// otherwise (not sent).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// A change to some fields of a memorized report (A09: the Memorized
/// Reports list renames and moves without resending what it doesn't
/// change; the engine's PUT takes every field but `version` as optional).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefinitionPatch {
    /// A new name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new group (`Some(None)`: no group).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<Option<String>>,
    /// A new body, of the row's own kind (a builder rename sends its body
    /// with `title` = the new name, Q-A08r).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<Value>,
}

/// Why the engine refused a memorized-report change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionRefusal {
    /// Someone changed it since it was read (409 `STALE_VERSION`).
    Stale,
    /// Another report has that name (409 `NAME_TAKEN`).
    NameTaken,
    /// Only its author, an owner or a super-admin may change it (403
    /// `NOT_AUTHOR`, Q-310).
    NotAuthor,
    /// The body is malformed (400 `DEFINITION_INVALID {path, code}`; a
    /// builder body says where, which the editor shows as a problem, A08).
    Invalid {
        /// Where, from `definition`, scrubbed: an input key is `<key>`
        /// (S-L3).
        path: Option<String>,
        /// The rule's code.
        code: Option<String>,
    },
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
        ApiError::Validation(v) if v.code().is_some_and(|c| c.as_str() == "DEFINITION_INVALID") => {
            return Some(DefinitionRefusal::Invalid {
                path: v
                    .details()
                    .text("path")
                    .map(cl_core::report::scrub_definition_path),
                code: v.details().text("code").map(str::to_string),
            });
        }
        ApiError::Validation(v) => v.code().map_or("", |c| c.as_str()),
        ApiError::PayloadTooLarge => return Some(DefinitionRefusal::TooLarge),
        _ => return None,
    };
    Some(match code {
        "STALE_VERSION" => DefinitionRefusal::Stale,
        "NAME_TAKEN" => DefinitionRefusal::NameTaken,
        "NOT_AUTHOR" => DefinitionRefusal::NotAuthor,
        "DEFINITION_TOO_LARGE" => DefinitionRefusal::TooLarge,
        _ => return None,
    })
}

// ------------------------------------------------ A09: `.clreport` (U-220)

/// The most reports one export or preview carries (the engine's cap).
pub const MAX_TRANSFER_REPORTS: usize = 200;
/// The cap on a preview body: a 2 MB file plus the owner's choices.
pub const MAX_TRANSFER_BYTES: usize = 3 * 1024 * 1024;

/// What to export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportWhat {
    /// These memorized reports, in this order (1 to 200).
    Ids(Vec<String>),
    /// Every report of this group (by name; the engine refuses over 200).
    Group(String),
}

/// One report as the engine exports it, and as a preview takes it back:
/// no ids (each is the token `"$ref:N"` in `definition`), the group by
/// name, and what each token named. `Debug` shows its kind only.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferReport {
    /// Its memorized name (owner data).
    pub name: String,
    /// Its group's name (`None`: no group).
    #[serde(default)]
    pub group_name: Option<String>,
    /// `standard` or `builder`.
    pub kind: String,
    /// The definition with `"$ref:N"` tokens.
    pub definition: Value,
    /// `references[N]`: `{kind, name, number?, accountType?, contactType?,
    /// tagGroupName?, fiscalYear?}` or `{kind, missing: true}`. Passed
    /// through as the engine wrote it (`cl-model` reads it for display).
    #[serde(default)]
    pub references: Vec<Value>,
}

impl std::fmt::Debug for TransferReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransferReport")
            .field("kind", &self.kind)
            .field("references", &self.references.len())
            .finish_non_exhaustive()
    }
}

/// A row the owner may pick for an unresolved reference.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Candidate {
    /// Its id.
    pub id: String,
    /// What the engine shows for it (`6100 · Shop supplies`).
    pub label: String,
}

impl std::fmt::Debug for Candidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Candidate").finish_non_exhaustive()
    }
}

/// A reference no single row of this company matched.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Unresolved {
    /// `$ref:N`.
    #[serde(rename = "ref")]
    pub reference_token: String,
    /// What the file named (as in [`TransferReport::references`]).
    pub reference: Value,
    /// The rows that could be meant (≤ 20; none when nothing matched).
    #[serde(default)]
    pub candidates: Vec<Candidate>,
}

/// A reference the owner left out, and what that did to the report.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Dropped {
    /// `$ref:N`.
    #[serde(rename = "ref")]
    pub reference_token: String,
    /// `removed_from_list`, `filter_removed`, `group_by_removed`,
    /// `tag_split_removed` or `columns_total`.
    pub effect: String,
}

/// The live report that already has the name.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameTaken {
    /// Its id.
    pub id: String,
    /// Its version (a Replace sends it).
    pub version: u64,
    /// Its kind (a Replace can't change it).
    pub kind: String,
    /// The user may change it (Q-310).
    #[serde(default)]
    pub can_edit: bool,
}

/// Why a report with every reference settled still can't be saved (the
/// engine's save check: `MIXED_POLARITY` after a leave-out, …).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct PreviewProblem {
    /// Where (scrubbed on read).
    #[serde(default)]
    pub path: Option<String>,
    /// The rule's code.
    #[serde(default)]
    pub code: Option<String>,
}

/// One report of a preview. `Debug` shows no names.
#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewReport {
    /// The file's name for it.
    pub name: String,
    /// `standard` or `builder`.
    pub kind: String,
    /// Every reference is settled and the save check passed.
    pub ready: bool,
    /// The definition with this company's ids (tokens left for what is
    /// unresolved).
    pub definition: Value,
    /// References still to settle.
    #[serde(default)]
    pub unresolved: Vec<Unresolved>,
    /// References left out, with their effect.
    #[serde(default)]
    pub dropped: Vec<Dropped>,
    /// This company's group with the file's group name.
    #[serde(default)]
    pub group_id: Option<String>,
    /// The live report that has the name already.
    #[serde(default)]
    pub name_taken: Option<NameTaken>,
    /// The save check's refusal, when nothing is unresolved.
    #[serde(default)]
    pub problem: Option<PreviewProblem>,
}

impl std::fmt::Debug for PreviewReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewReport")
            .field("kind", &self.kind)
            .field("ready", &self.ready)
            .field("unresolved", &self.unresolved.len())
            .field("dropped", &self.dropped)
            .field("name_taken", &self.name_taken.is_some())
            .field("problem", &self.problem)
            .finish_non_exhaustive()
    }
}

/// The owner's choices: `"<report index>:$ref:N"` → an id of this company,
/// or `None` to leave it out (sent as `null`).
pub type Choices = std::collections::BTreeMap<String, Option<String>>;

/// The choice key for reference `token` of report `index`.
#[must_use]
pub fn choice_key(index: usize, token: &str) -> String {
    format!("{index}:{token}")
}

/// The export body.
fn export_body(what: &ExportWhat) -> Value {
    match what {
        ExportWhat::Ids(ids) => serde_json::json!({ "ids": ids }),
        ExportWhat::Group(id) => serde_json::json!({ "groupId": id }),
    }
}

/// The preview body (`choices` left out when there are none).
fn preview_body(reports: &[TransferReport], choices: &Choices) -> Result<Value, ApiError> {
    let mut v = serde_json::json!({ "reports": reports });
    if !choices.is_empty()
        && let Value::Object(m) = &mut v
    {
        m.insert(
            "choices".into(),
            serde_json::to_value(choices)
                .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?,
        );
    }
    Ok(v)
}

/// The `reports` of an export or preview answer (bare, or under `data`).
fn reports_of<T: for<'de> Deserialize<'de>>(
    body: &[u8],
    what: &'static str,
) -> Result<Vec<T>, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| ApiError::Decode { what })?;
    let v = match v {
        Value::Object(mut m) if m.contains_key("data") => m.remove("data").unwrap_or_default(),
        v => v,
    };
    let list = match v {
        Value::Object(mut m) => m.remove("reports").ok_or(ApiError::Decode { what })?,
        _ => return Err(ApiError::Decode { what }),
    };
    serde_json::from_value(list).map_err(|_| ApiError::Decode { what })
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

    /// Memorizes a report. `key` is made once per owner action (a retry of
    /// the same Memorize sends the same key, so the engine stores one row).
    ///
    /// # Errors
    /// `PayloadTooLarge` over the kind's cap (nothing sent);
    /// [`definition_refusal`] for the engine's refusals; any [`ApiError`].
    pub async fn create(
        &self,
        w: &DefinitionWrite,
        key: IdempotencyKey,
    ) -> Result<DefinitionRow, ApiError> {
        self.require_kind(&w.kind)?;
        let mut call = Call::at(Method::Post, REPORT_DEFS_CREATE.route());
        call = call.json_body(body(w, max_bytes(&w.kind))?);
        call.idempotency_key = Some(key);
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

    /// Changes some fields of the report read at `version` (rename, move
    /// to a group). `kind` is the row's own kind: it sets the size cap and
    /// the builder capability check, and is not sent.
    ///
    /// # Errors
    /// As [`ReportDefinitions::update`].
    pub async fn patch(
        &self,
        id: &str,
        version: u64,
        kind: &str,
        p: &DefinitionPatch,
    ) -> Result<DefinitionRow, ApiError> {
        self.require_kind(kind)?;
        let mut v =
            serde_json::to_value(p).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        if let Value::Object(m) = &mut v {
            m.insert("version".into(), Value::from(version));
        }
        let mut call = Call::new(
            Method::Put,
            REPORT_DEF_PUT.route(),
            REPORT_DEF_PUT.route().replace("{id}", id),
        );
        call = call.json_body(body(&v, max_bytes(kind))?);
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

    /// `.clreport` routes need `report-defs-transfer-v1` (U-220, Q-220).
    fn require_transfer(self) -> Result<(), ApiError> {
        self.require()?;
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::REPORT_DEFS_TRANSFER)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// Memorized reports without ids, for a `.clreport` file (A09). The
    /// engine audits one `download` row per report.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `report-defs-transfer-v1`;
    /// `Local(InvalidValue)` for no ids or more than 200; 404 when a report
    /// or the group isn't live in this company; any [`ApiError`].
    pub async fn export(&self, what: &ExportWhat) -> Result<Vec<TransferReport>, ApiError> {
        self.require_transfer()?;
        if let ExportWhat::Ids(ids) = what
            && (ids.is_empty() || ids.len() > MAX_TRANSFER_REPORTS)
        {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::at(Method::Post, REPORT_DEFS_EXPORT.route());
        call = call.json_body(body(&export_body(what), MAX_DEFINITION_BYTES)?);
        let reply = self.0.send(&REPORT_DEFS_EXPORT, call, None).await?;
        reports_of(&reply.body, "report file export")
    }

    /// Matches a file's references in this company, with the owner's
    /// `choices` so far (A09). Writes nothing.
    ///
    /// # Errors
    /// As [`ReportDefinitions::export`]; `PayloadTooLarge` over 3 MB
    /// (nothing sent); 400 `DEFINITION_INVALID` (`SCHEMA_NEWER`, a kind
    /// that differs, a bad token, see [`definition_refusal`]) and 400
    /// `VALIDATION_ERROR` for a choice that isn't this company's row.
    pub async fn import_preview(
        &self,
        reports: &[TransferReport],
        choices: &Choices,
    ) -> Result<Vec<PreviewReport>, ApiError> {
        self.require_transfer()?;
        if reports.is_empty() || reports.len() > MAX_TRANSFER_REPORTS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call = Call::at(Method::Post, REPORT_DEFS_IMPORT_PREVIEW.route());
        call = call.json_body(body(&preview_body(reports, choices)?, MAX_TRANSFER_BYTES)?);
        let reply = self.0.send(&REPORT_DEFS_IMPORT_PREVIEW, call, None).await?;
        let mut out: Vec<PreviewReport> = reports_of(&reply.body, "report file preview")?;
        // A problem's path may hold an input key: scrubbed as a refusal's
        // is (S-L3).
        for r in &mut out {
            if let Some(p) = r.problem.as_mut() {
                p.path = p
                    .path
                    .as_deref()
                    .map(cl_core::report::scrub_definition_path);
            }
        }
        Ok(out)
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
    fn a_patch_sends_only_what_changes() {
        let rename = DefinitionPatch {
            name: Some("For the bank".into()),
            ..DefinitionPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&rename).expect("json"),
            serde_json::json!({"name": "For the bank"})
        );
        let ungroup = DefinitionPatch {
            group_id: Some(None),
            ..DefinitionPatch::default()
        };
        assert_eq!(
            serde_json::to_value(&ungroup).expect("json"),
            serde_json::json!({"groupId": null})
        );
    }

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
        let invalid = crate::engine_refusal(
            400,
            &serde_json::json!({"error": {"message": "x", "code": "DEFINITION_INVALID",
                "details": {"path": "definition.nodes.2.terms.0.nodeId", "code": "FORMULA_FORWARD_REF"}}}),
        );
        assert_eq!(
            definition_refusal(&invalid),
            Some(DefinitionRefusal::Invalid {
                path: Some("definition.nodes.2.terms.0.nodeId".into()),
                code: Some("FORMULA_FORWARD_REF".into()),
            })
        );
        // An input key in the path is scrubbed (S-L3).
        let header = crate::engine_refusal(
            400,
            &serde_json::json!({"error": {"message": "x", "code": "DEFINITION_INVALID",
                "details": {"path": "definition.columns.headers.Rivera Remodel", "code": "SCHEMA"}}}),
        );
        assert!(matches!(
            definition_refusal(&header),
            Some(DefinitionRefusal::Invalid { path: Some(p), .. }) if !p.contains("Rivera")
        ));
    }

    #[test]
    fn can_edit_is_read_and_defaults_to_false() {
        let r: DefinitionRow = one(ROW.as_bytes(), "x").expect("row");
        assert!(!r.can_edit);
        let with = ROW.replace("\"version\": 3,", "\"version\": 3, \"canEdit\": true,");
        let r: DefinitionRow = one(with.as_bytes(), "x").expect("row");
        assert!(r.can_edit);
    }

    #[test]
    fn export_and_preview_bodies_follow_the_u220_contract() {
        assert_eq!(
            export_body(&ExportWhat::Ids(vec!["a".into(), "b".into()])),
            serde_json::json!({"ids": ["a", "b"]})
        );
        assert_eq!(
            export_body(&ExportWhat::Group("g".into())),
            serde_json::json!({"groupId": "g"})
        );
        let r = TransferReport {
            name: "Shop costs".into(),
            group_name: Some("For the bank".into()),
            kind: "standard".into(),
            definition: serde_json::json!({"kind": "standard"}),
            references: vec![serde_json::json!({"kind": "account", "name": "Sales"})],
        };
        let mut choices = Choices::new();
        assert_eq!(
            preview_body(std::slice::from_ref(&r), &choices).expect("body"),
            serde_json::json!({"reports": [{"name": "Shop costs", "groupName": "For the bank",
                "kind": "standard", "definition": {"kind": "standard"},
                "references": [{"kind": "account", "name": "Sales"}]}]})
        );
        // A leave-out is sent as null, never skipped.
        choices.insert(choice_key(0, "$ref:0"), None);
        choices.insert(choice_key(0, "$ref:1"), Some("id-1".into()));
        let b = preview_body(&[r], &choices).expect("body");
        assert_eq!(
            b["choices"],
            serde_json::json!({"0:$ref:0": null, "0:$ref:1": "id-1"})
        );
    }

    #[test]
    fn preview_answers_are_read() {
        let body = serde_json::json!({"reports": [{
            "name": "Shop costs", "kind": "builder", "ready": false,
            "definition": {"kind": "builder"},
            "unresolved": [{"ref": "$ref:0", "reference": {"kind": "contact", "name": "Cosmos", "contactType": "vendor"},
                            "candidates": [{"id": "c1", "label": "Cosmos (vendor)"}, {"id": "c2", "label": "Cosmos (both)"}]}],
            "dropped": [{"ref": "$ref:1", "effect": "group_by_removed"}],
            "groupId": null,
            "nameTaken": {"id": "d1", "version": 4, "kind": "builder", "canEdit": true},
            "problem": null
        }]});
        let v: Vec<PreviewReport> =
            reports_of(&serde_json::to_vec(&body).expect("b"), "x").expect("read");
        assert_eq!(v.len(), 1);
        assert!(!v[0].ready);
        assert_eq!(v[0].unresolved[0].reference_token, "$ref:0");
        assert_eq!(v[0].unresolved[0].candidates.len(), 2);
        assert_eq!(v[0].dropped[0].effect, "group_by_removed");
        assert_eq!(v[0].name_taken.as_ref().map(|n| n.version), Some(4));
        // Debug names nothing the owner typed.
        let d = format!("{:?}", v[0]);
        assert!(!d.contains("Shop") && !d.contains("Cosmos"), "{d}");
        // An export answer, wrapped or not.
        let export = serde_json::json!({"data": {"reports": [{"name": "P&L", "groupName": null,
            "kind": "standard", "definition": {}, "references": []}]}});
        let r: Vec<TransferReport> =
            reports_of(&serde_json::to_vec(&export).expect("b"), "x").expect("export");
        assert_eq!(r[0].group_name, None);
        assert!(reports_of::<TransferReport>(b"[]", "x").is_err());
    }

    #[test]
    fn a_body_over_64_kb_is_refused_before_sending() {
        let w = DefinitionWrite {
            name: "big".into(),
            group_id: None,
            kind: "standard".into(),
            definition: Value::String("x".repeat(MAX_DEFINITION_BYTES)),
            origin: None,
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
            origin: None,
        };
        assert!(body(&w, max_bytes(&w.kind)).is_ok());
        w.definition = Value::String("x".repeat(MAX_BUILDER_DEFINITION_BYTES));
        assert!(matches!(
            body(&w, max_bytes(&w.kind)),
            Err(ApiError::PayloadTooLarge)
        ));
    }
}
