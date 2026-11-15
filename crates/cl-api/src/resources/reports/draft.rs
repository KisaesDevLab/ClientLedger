//! Describe the report (A10, U-120, `report-builder-ai-v1`, Q-A10b–i): the
//! company's AI provider drafts a Report Builder definition from the
//! owner's description. Only names from the books reach the model; the
//! engine masks any amount in the description, resolves names to ids by
//! lookup (never an id from the model) and leaves a `"$ref:N"` placeholder
//! where a name matched none or several rows.
//!
//! The reply is checked again here ([`validate`]), as Ask a question's is:
//! a JSON number other than `schemaVersion: 1` and a formula term's ±1, a
//! digit of any script in text or in an object key other than an id or a
//! placeholder at an id field, an aging bucket key where bucket keys go or
//! the preset that was sent at `dates.preset`, an amount filter, a
//! placeholder that is not listed as unresolved (or the other way round)
//! are refused as
//! [`ApiError::ReportShape`], and the builder shows nothing from it
//! (non-negotiable 4: the AI never produces a number).

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::resources::operations::REPORT_BUILDER_DRAFT;
use crate::session::Session;
use crate::wire::Capabilities;

/// The longest description the engine takes (UTF-16 units, as zod counts).
pub const MAX_DESCRIPTION_CHARS: usize = 1000;
/// The most date presets one request may send.
pub const MAX_DATE_PRESETS: usize = 40;
/// The most candidates the engine offers for one name.
pub const MAX_CANDIDATES: usize = 20;
/// The aging bucket keys, the only digits a definition's structure has
/// besides ids, placeholders and presets (`DRAFT_STRUCTURAL_DIGITS`).
pub const AGING_KEYS: [&str; 4] = ["d1_30", "d31_60", "d61_90", "over_90"];

/// What is sent: the owner's words and the date preset keys the app
/// offers. One-shot (Q-A10g): never a definition to refine.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftRequest {
    /// The owner's description (1–1,000 characters).
    pub description: String,
    /// Preset keys the draft may use (≤ 40).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub date_presets: Vec<String>,
}

/// The description is owner text: `Debug` shows its length only.
impl std::fmt::Debug for DraftRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftRequest")
            .field("description_len", &self.description.chars().count())
            .field("date_presets", &self.date_presets.len())
            .finish()
    }
}

/// True when `description` may be sent: 1–1,000 UTF-16 units once
/// trimmed.
#[must_use]
pub fn description_ok(description: &str) -> bool {
    let d = description.trim();
    !d.is_empty() && d.encode_utf16().count() <= MAX_DESCRIPTION_CHARS
}

/// What an unresolved name was meant to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DraftKind {
    /// An account.
    Account,
    /// A customer or vendor.
    Contact,
    /// An item.
    Item,
    /// A tag.
    Tag,
    /// A tag group (a "by tag" grouping).
    TagGroup,
}

impl DraftKind {
    fn of(s: &str) -> Option<DraftKind> {
        Some(match s {
            "account" => DraftKind::Account,
            "contact" => DraftKind::Contact,
            "item" => DraftKind::Item,
            "tag" => DraftKind::Tag,
            "tagGroup" => DraftKind::TagGroup,
            _ => return None,
        })
    }

    /// The wire name (`tagGroup` for a tag group).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DraftKind::Account => "account",
            DraftKind::Contact => "contact",
            DraftKind::Item => "item",
            DraftKind::Tag => "tag",
            DraftKind::TagGroup => "tagGroup",
        }
    }
}

/// A row the owner may pick for a name.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftCandidate {
    /// Its id.
    pub id: String,
    /// What the books call it (`6100 · Rent`).
    pub label: String,
}

impl std::fmt::Debug for DraftCandidate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftCandidate").finish_non_exhaustive()
    }
}

/// Where a placeholder sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftSite {
    /// The section it is in.
    pub node_id: String,
    /// `definition.nodes.0.filters.0.ids.0` (informational: a resolve
    /// works by token, as removing one shifts the indexes after it).
    pub path: String,
}

/// A name the engine couldn't match to exactly one row. `Debug` names no
/// owner data.
#[derive(Clone, PartialEq, Eq)]
pub struct DraftUnresolved {
    /// `$ref:N`.
    pub token: String,
    /// What it was meant to be.
    pub kind: DraftKind,
    /// The name (a books name or the owner's words; `None` otherwise).
    pub name: Option<String>,
    /// `ambiguous` (several rows match: see `candidates`) or `not_found`.
    pub reason: String,
    /// Where it sits.
    pub sites: Vec<DraftSite>,
    /// The rows that could be meant (≤ 20; ambiguous only).
    pub candidates: Vec<DraftCandidate>,
}

impl std::fmt::Debug for DraftUnresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftUnresolved")
            .field("token", &self.token)
            .field("kind", &self.kind)
            .field("reason", &self.reason)
            .field("sites", &self.sites.len())
            .field("candidates", &self.candidates.len())
            .finish_non_exhaustive()
    }
}

/// An engine note on the draft. Only the code is kept: the app words it
/// (the engine's message is never shown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteCode {
    /// Names are left for the owner to pick.
    PickUnresolved,
    /// An amount in the description was not turned into a filter: the
    /// owner adds it (Q-A10e).
    AmountFilterLeftOut,
    /// A date range the app has no preset for was left out.
    PresetLeftOut,
    /// Numbers in the description were not used.
    NumbersNotUsed,
}

impl NoteCode {
    fn of(s: &str) -> Option<NoteCode> {
        Some(match s {
            "pick_unresolved" => NoteCode::PickUnresolved,
            "amount_filter_left_out" => NoteCode::AmountFilterLeftOut,
            "preset_left_out" => NoteCode::PresetLeftOut,
            "numbers_not_used" => NoteCode::NumbersNotUsed,
            _ => return None,
        })
    }

    /// The wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            NoteCode::PickUnresolved => "pick_unresolved",
            NoteCode::AmountFilterLeftOut => "amount_filter_left_out",
            NoteCode::PresetLeftOut => "preset_left_out",
            NoteCode::NumbersNotUsed => "numbers_not_used",
        }
    }
}

/// One note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftNote {
    /// What it says.
    pub code: NoteCode,
    /// The row it is about, when known.
    pub node_id: Option<String>,
}

/// A checked draft. `Debug` shows counts only (captions and names are
/// owner data).
#[derive(Clone, PartialEq)]
pub struct DraftReply {
    /// The definition (schema v1; may hold `$ref:N` placeholders).
    pub definition: Value,
    /// No placeholder is left: it saves as is.
    pub savable: bool,
    /// The names to pick or remove.
    pub unresolved: Vec<DraftUnresolved>,
    /// The engine's notes.
    pub notes: Vec<DraftNote>,
    /// `self_hosted` (Ollama) or `cloud`.
    pub reader: Option<String>,
    /// `anthropic`, `openai` or `ollama`.
    pub provider: Option<String>,
}

impl std::fmt::Debug for DraftReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DraftReply")
            .field("savable", &self.savable)
            .field("unresolved", &self.unresolved)
            .field("notes", &self.notes)
            .field("reader", &self.reader)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

/// Why a description got no draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftRefusal {
    /// AI is off for the company, or its report opt-in is (403
    /// `AI_CONSENT_REQUIRED`; no model was called, Q-A10b).
    ConsentRequired,
    /// The description couldn't be turned into a report, or the model's
    /// answer was refused (422 `DRAFT_UNMAPPED`; `reason` from the engine).
    Unmapped {
        /// `no_report`, `not_json`, `amount_filter`, … (a code, never text
        /// from the description).
        reason: Option<String>,
    },
    /// The AI provider can't be reached or isn't set up (503
    /// `AI_UNAVAILABLE`).
    Unavailable,
    /// The request was refused as malformed (400 `VALIDATION_ERROR`).
    Invalid,
}

/// The draft refusal in `e`, if it is one.
#[must_use]
pub fn draft_refusal(e: &ApiError) -> Option<DraftRefusal> {
    let code = match e {
        ApiError::Forbidden { code: Some(c) } | ApiError::Server { code: Some(c), .. } => {
            c.as_str()
        }
        ApiError::Validation(v) => {
            let c = v.code().map_or("", crate::errors::ErrorCode::as_str);
            if c == "DRAFT_UNMAPPED" {
                // A code from a closed list: kept only when it looks like one.
                let reason = v
                    .details()
                    .text("reason")
                    .filter(|r| {
                        r.len() <= 40 && r.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                    })
                    .map(str::to_string);
                return Some(DraftRefusal::Unmapped { reason });
            }
            c
        }
        // A model call that took too long or whose answer was lost: it
        // saves nothing, so it is "couldn't reach the AI service", never
        // an unknown save (A10 QA-1).
        ApiError::OfflineUnavailable { .. }
        | ApiError::Timeout
        | ApiError::EngineUnavailable { permanent: false }
        | ApiError::OutcomeUnknown { .. } => return Some(DraftRefusal::Unavailable),
        _ => return None,
    };
    match code {
        "AI_CONSENT_REQUIRED" => Some(DraftRefusal::ConsentRequired),
        "DRAFT_UNMAPPED" => Some(DraftRefusal::Unmapped { reason: None }),
        "AI_UNAVAILABLE" => Some(DraftRefusal::Unavailable),
        "VALIDATION_ERROR" => Some(DraftRefusal::Invalid),
        _ => None,
    }
}

fn bad() -> ApiError {
    ApiError::ReportShape { what: "draft" }
}

/// A whole uuid, any case.
fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// `$ref:N` (N: 1–4 ASCII digits; the same rule as
/// `cl_model::report_def::draft::is_placeholder`).
fn is_token(s: &str) -> bool {
    s.strip_prefix("$ref:")
        .is_some_and(|n| !n.is_empty() && n.len() <= 4 && n.bytes().all(|c| c.is_ascii_digit()))
}

/// True when `s` holds a digit of any script (fullwidth, Arabic-Indic,
/// Roman numerals and other Unicode numbers too, as the engine's `\p{N}`).
#[must_use]
pub fn has_digit(s: &str) -> bool {
    s.chars().any(char::is_numeric)
}

/// True when the object keys on `path` end with `tail`.
fn at(path: &[&str], tail: &[&str]) -> bool {
    path.ends_with(tail)
}

/// A renamed header's key may name an aging bucket (`d1_30` or
/// `bucket:d1_30`); no other key in a draft holds a digit.
fn header_key_ok(k: &str) -> bool {
    AGING_KEYS.contains(&k.strip_prefix("bucket:").unwrap_or(k))
}

/// Walks every value of the definition. `path` holds the object keys from
/// the definition down to `v` (array indexes are left out). Numbers only
/// where the structure has them (`schemaVersion: 1`, a term's sign ±1);
/// a digit in text only where the schema puts one: an id or a placeholder
/// in `filters.*.ids.*` or `groupBy.*.tagGroupId`, an aging bucket key in
/// `filters.*.values.*`, the preset sent at `dates.preset`. Object keys
/// hold no digit (an aging bucket header key aside). Every placeholder is
/// gathered.
fn check_value(
    v: &Value,
    path: &mut Vec<String>,
    presets: &[String],
    tokens: &mut BTreeSet<String>,
) -> Result<(), ApiError> {
    let p: Vec<&str> = path.iter().map(String::as_str).collect();
    match v {
        Value::Number(n) => {
            let ok = if p == ["schemaVersion"] {
                n.as_u64() == Some(1)
            } else if at(&p, &["terms", "sign"]) {
                matches!(n.as_i64(), Some(1 | -1))
            } else {
                false
            };
            if ok { Ok(()) } else { Err(bad()) }
        }
        Value::String(s) => {
            let id_site = at(&p, &["filters", "ids"]) || at(&p, &["groupBy", "tagGroupId"]);
            if id_site && is_token(s) {
                tokens.insert(s.clone());
                return Ok(());
            }
            let ok = !has_digit(s)
                || (id_site && is_uuid(s))
                || (at(&p, &["filters", "values"]) && AGING_KEYS.contains(&s.as_str()))
                || (p == ["dates", "preset"] && presets.iter().any(|x| x == s));
            if ok { Ok(()) } else { Err(bad()) }
        }
        Value::Array(a) => a
            .iter()
            .try_for_each(|x| check_value(x, path, presets, tokens)),
        Value::Object(m) => {
            // No amount filter of any shape (non-negotiable 4, Q-A10e).
            if p.last() == Some(&"filters") || (m.contains_key("op") && m.contains_key("field")) {
                let op = m.get("op").and_then(Value::as_str).unwrap_or("");
                let field = m.get("field").and_then(Value::as_str).unwrap_or("");
                if m.contains_key("value")
                    || matches!(op, "gte" | "gt" | "lte" | "lt" | "between")
                    || matches!(field, "amount" | "total")
                {
                    return Err(bad());
                }
            }
            let headers = p == ["columns", "headers"];
            for (k, x) in m {
                if has_digit(k) && !(headers && header_key_ok(k)) {
                    return Err(bad());
                }
                path.push(k.clone());
                let r = check_value(x, path, presets, tokens);
                path.pop();
                r?;
            }
            Ok(())
        }
        Value::Bool(_) | Value::Null => Ok(()),
    }
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn unresolved_of(u: &Value) -> Result<DraftUnresolved, ApiError> {
    let token = text(u, "ref").filter(|t| is_token(t)).ok_or_else(bad)?;
    let kind = u
        .get("kind")
        .and_then(Value::as_str)
        .and_then(DraftKind::of)
        .ok_or_else(bad)?;
    let reason = text(u, "reason").unwrap_or_default();
    // An ambiguous name is a books name ("Studio 54"), shown in the picker
    // and never part of the definition: it may hold a digit. Any other name
    // (the owner's words, nothing matched) never does, as the engine's rule.
    let name = match u.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if reason == "ambiguous" || !has_digit(s) => Some(s.clone()),
        Some(_) => return Err(bad()),
    };
    let sites = u
        .get("sites")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|s| {
            Ok(DraftSite {
                node_id: text(s, "nodeId").ok_or_else(bad)?,
                path: text(s, "path").unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let candidates = u
        .get("candidates")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(|c| {
            let id = text(c, "id").filter(|i| is_uuid(i)).ok_or_else(bad)?;
            Ok(DraftCandidate {
                id,
                label: text(c, "label").unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    if candidates.len() > MAX_CANDIDATES {
        return Err(bad());
    }
    Ok(DraftUnresolved {
        token,
        kind,
        name,
        reason,
        sites,
        candidates,
    })
}

/// Checks the engine's draft against what was sent.
///
/// # Errors
/// [`ApiError::ReportShape`] for a definition that isn't a builder schema
/// v1 object, a number or digit outside the structure, an amount filter, a
/// preset that wasn't sent, a placeholder not listed as unresolved (or
/// listed and absent), a candidate id that isn't a uuid, or `savable` that
/// disagrees with the unresolved list.
pub fn validate(req: &DraftRequest, v: &Value) -> Result<DraftReply, ApiError> {
    let def = v.get("definition").ok_or_else(bad)?;
    if def.get("kind").and_then(Value::as_str) != Some("builder")
        || def.get("schemaVersion").and_then(Value::as_u64) != Some(1)
    {
        return Err(bad());
    }
    if let Some(p) = def.pointer("/dates/preset").and_then(Value::as_str)
        && !req.date_presets.iter().any(|x| x == p)
    {
        return Err(bad());
    }
    let mut tokens = BTreeSet::new();
    check_value(def, &mut Vec::new(), &req.date_presets, &mut tokens)?;
    let unresolved = v
        .get("unresolved")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .map(unresolved_of)
        .collect::<Result<Vec<_>, ApiError>>()?;
    let listed: BTreeSet<String> = unresolved.iter().map(|u| u.token.clone()).collect();
    if listed != tokens || listed.len() != unresolved.len() {
        return Err(bad());
    }
    let savable = v.get("savable").and_then(Value::as_bool).ok_or_else(bad)?;
    if savable != unresolved.is_empty() {
        return Err(bad());
    }
    let notes = v
        .get("notes")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        // A code this app doesn't know yet is left out (a newer engine).
        .filter_map(|n| {
            Some(DraftNote {
                code: NoteCode::of(n.get("code")?.as_str()?)?,
                node_id: text(n, "nodeId"),
            })
        })
        .collect();
    Ok(DraftReply {
        definition: def.clone(),
        savable,
        unresolved,
        notes,
        reader: text(v, "reader"),
        provider: text(v, "provider"),
    })
}

impl Session {
    /// True when the engine drafts builder reports (`report-builder-ai-v1`).
    #[must_use]
    pub fn can_draft_reports(&self) -> bool {
        self.client()
            .capabilities()
            .has(Capabilities::REPORT_BUILDER_AI)
    }

    /// Asks the company's AI provider to draft a builder definition from
    /// `req.description`.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `report-builder-ai-v1`;
    /// `Local(InvalidValue)` for an empty or over-long description or too
    /// many presets (nothing sent); [`draft_refusal`] for the engine's;
    /// [`ApiError::ReportShape`] when the draft fails [`validate`]; any
    /// [`ApiError`].
    pub async fn draft_report(&self, req: &DraftRequest) -> Result<DraftReply, ApiError> {
        if !self.can_draft_reports() {
            return Err(ApiError::Local(LocalValidation::Unsupported));
        }
        if !description_ok(&req.description) || req.date_presets.len() > MAX_DATE_PRESETS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body = DraftRequest {
            description: req.description.trim().to_string(),
            date_presets: req.date_presets.clone(),
        };
        let body = serde_json::to_vec(&body)
            .map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let call = Call::at(Method::Post, REPORT_BUILDER_DRAFT.route()).json_body(body);
        let reply = self.send(&REPORT_BUILDER_DRAFT, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| bad())?;
        validate(req, &v)
    }
}

#[cfg(test)]
#[path = "draft_tests.rs"]
mod tests;
