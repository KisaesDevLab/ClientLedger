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
pub use validate::{Code, Issue, issues};

/// The schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The engine's cap on a builder definition (256 KB). Advisory here: the
/// engine's 413 decides.
pub const MAX_BYTES: usize = 256 * 1024;

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
    /// Not schema v1 JSON: an unknown or repeated field, a wrong type.
    #[error("this report definition can't be read: {0}")]
    Malformed(String),
    /// A rule is broken (`DEFINITION_INVALID {path, code}`).
    #[error("this report definition breaks a rule ({}) at {path}", code.as_str())]
    Invalid {
        /// Where.
        path: String,
        /// Which rule.
        code: Code,
    },
}

#[derive(serde::Deserialize)]
struct Peek {
    #[serde(rename = "schemaVersion")]
    schema_version: Option<u64>,
    kind: Option<String>,
}

/// Reads a definition from its JSON text: a newer `schemaVersion` is
/// refused first (before any strict check could call it "unknown field"),
/// then the strict parse (unknown and repeated fields refused at every
/// depth), then the rules.
///
/// # Errors
/// Any [`DefinitionError`].
pub fn parse(text: &str) -> Result<BuilderDefinition, DefinitionError> {
    if text.len() > MAX_BYTES {
        return Err(DefinitionError::TooLarge);
    }
    let peek: Peek =
        serde_json::from_str(text).map_err(|e| DefinitionError::Malformed(e.to_string()))?;
    if peek
        .schema_version
        .is_some_and(|v| v > u64::from(SCHEMA_VERSION))
    {
        return Err(DefinitionError::Newer);
    }
    if peek.kind.as_deref() != Some("builder") {
        return Err(DefinitionError::NotBuilder);
    }
    let def: BuilderDefinition =
        serde_json::from_str(text).map_err(|e| DefinitionError::Malformed(e.to_string()))?;
    if def.schema_version != SCHEMA_VERSION {
        return Err(DefinitionError::Malformed("schemaVersion".into()));
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
    serde_json::to_string(def).map_err(|e| DefinitionError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests;
