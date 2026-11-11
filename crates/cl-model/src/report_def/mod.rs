//! Report Builder definitions (ADD-03 A07, plan §3): schema v1 as strict
//! serde, the field catalogue, the pre-order outline and the validation
//! rules, plus the pure run state of a builder report window.
//!
//! No egui, no I/O, no money arithmetic: the engine computes every figure.

pub mod fields;
pub mod outline;
pub mod run_state;
pub mod schema;
pub mod validate;

pub use schema::{
    BuilderDefinition, BuilderKind, Columns, CompareBase, CompareSet, DateMode, Dates, Display,
    Earnings, Filter, FilterOp, Formula, Group, GroupBy, Measure, Node, Periods, Polarity, Section,
    SourceKey, Spacer, Term, TextRow, Unit,
};
pub use validate::{Code, Issue, NodeUnit, issues, node_units};

/// The schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The engine's cap on a builder definition (256 KB). Advisory here: the
/// engine's 413 decides.
pub const MAX_BYTES: usize = 256 * 1024;

/// What made a definition unreadable, without its content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MalformedKind {
    /// A key appears twice in one object.
    DuplicateKey,
    /// A key schema v1 doesn't have, or a value outside an enumeration.
    UnknownKey,
    /// A required key is missing.
    MissingKey,
    /// Not JSON, or a value of the wrong type.
    Other,
}

fn malformed(e: &serde_json::Error) -> DefinitionError {
    // The message is only classified, never kept: it may quote owner text.
    let m = e.to_string();
    let kind = if m.starts_with("duplicate field") {
        MalformedKind::DuplicateKey
    } else if m.starts_with("unknown field") || m.starts_with("unknown variant") {
        MalformedKind::UnknownKey
    } else if m.starts_with("missing field") {
        MalformedKind::MissingKey
    } else {
        MalformedKind::Other
    };
    DefinitionError::Malformed {
        kind,
        line: e.line(),
        column: e.column(),
    }
}

/// Why a builder definition can't be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    /// Saved by a newer version of the app (the owner is asked to update the app).
    #[error("this report was saved by a newer version of the app")]
    Newer,
    /// Not a builder definition.
    #[error("this is not a Report Builder report")]
    NotBuilder,
    /// Over 256 KB.
    #[error("this report definition is too large")]
    TooLarge,
    /// Not schema v1 JSON (the engine's `SCHEMA`). Holds no text from the
    /// file: a caption is owner data.
    #[error("this report definition can't be read ({kind:?} at line {line}, column {column})")]
    Malformed {
        /// What kind of problem.
        kind: MalformedKind,
        /// Line in the text (1-based; 0 when not from text).
        line: usize,
        /// Column.
        column: usize,
    },
    /// A rule is broken (`DEFINITION_INVALID {path, code}`).
    #[error("this report definition breaks a rule ({}) at {path}", code.as_str())]
    Invalid {
        /// Where.
        path: String,
        /// Which rule.
        code: Code,
    },
}

/// The path of the first value nested deeper than any valid definition can
/// be (the engine's `deepestPath`: a node at the deepest group level sits at
/// JSON depth 16 and its filters add 4 more), or `None`.
fn too_deep(v: &serde_json::Value, path: &str, depth: usize) -> Option<String> {
    if depth > 2 * validate::limits::DEPTH + 8 {
        return Some(path.to_string());
    }
    match v {
        serde_json::Value::Array(a) => a
            .iter()
            .enumerate()
            .find_map(|(i, x)| too_deep(x, &format!("{path}.{i}"), depth + 1)),
        serde_json::Value::Object(m) => m
            .iter()
            .find_map(|(k, x)| too_deep(x, &format!("{path}.{k}"), depth + 1)),
        _ => None,
    }
}

/// Reads a definition from its JSON text, in the engine's order with one
/// change: a newer `schemaVersion` is refused first (the owner is asked to update the app,
/// before any strict check could call a new field unknown). Then the size
/// (UTF-8 bytes of the compact JSON, as the engine measures it), the
/// nesting bound, the strict parse (unknown and repeated keys refused at
/// every depth, which the engine's `JSON.parse` can't see) and the rules.
///
/// # Errors
/// Any [`DefinitionError`].
pub fn parse(text: &str) -> Result<BuilderDefinition, DefinitionError> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| malformed(&e))?;
    let version = value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64);
    if version.is_some_and(|v| v > u64::from(SCHEMA_VERSION)) {
        return Err(DefinitionError::Newer);
    }
    if value.get("kind").and_then(serde_json::Value::as_str) != Some("builder") {
        return Err(DefinitionError::NotBuilder);
    }
    let compact = serde_json::to_string(&value).map_err(|e| malformed(&e))?;
    if compact.len() > MAX_BYTES {
        return Err(DefinitionError::TooLarge);
    }
    if let Some(path) = too_deep(&value, "definition", 0) {
        return Err(DefinitionError::Invalid {
            path,
            code: Code::TooDeep,
        });
    }
    let def: BuilderDefinition = serde_json::from_str(text).map_err(|e| malformed(&e))?;
    if def.schema_version != SCHEMA_VERSION {
        return Err(DefinitionError::Invalid {
            path: "definition.schemaVersion".into(),
            code: Code::Schema,
        });
    }
    check(&def)?;
    Ok(def)
}

/// Checks the rules on a definition built in memory (A08's editor).
///
/// # Errors
/// [`DefinitionError::Invalid`] for the first broken rule;
/// [`DefinitionError::TooLarge`] over 256 KB.
pub fn check(def: &BuilderDefinition) -> Result<(), DefinitionError> {
    if let Some(i) = issues(def).into_iter().next() {
        return Err(DefinitionError::Invalid {
            path: i.path,
            code: i.code,
        });
    }
    let bytes = to_text(def)?.len();
    if bytes > MAX_BYTES {
        return Err(DefinitionError::TooLarge);
    }
    Ok(())
}

/// The definition's JSON text, as the engine stores it.
///
/// # Errors
/// [`DefinitionError::Malformed`] if it can't be written (never, for
/// these plain types).
pub fn to_text(def: &BuilderDefinition) -> Result<String, DefinitionError> {
    serde_json::to_string(def).map_err(|e| malformed(&e))
}

#[cfg(test)]
mod tests;
