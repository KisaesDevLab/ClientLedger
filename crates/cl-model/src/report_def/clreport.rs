//! `.clreport` report files (ADD-03 A09, Q-A09a, plan §3.3): memorized
//! reports carried from one company to another. UTF-8 JSON:
//!
//! ```json
//! { "format": "clientledger-report", "schemaVersion": 1,
//!   "app": "ClientLedger 0.9.0", "exportedAt": "2026-11-12T21:00:00Z",
//!   "reports": [ { "name": "…", "group": "For the bank", "kind": "builder",
//!                  "definition": { "…": "\"$ref:0\" where an id was" },
//!                  "references": [ { "kind": "account", "number": "4000",
//!                                    "name": "Sales", "accountType": "revenue" } ] } ] }
//! ```
//!
//! The engine (U-220) turns ids into `"$ref:N"` tokens on export and
//! matches them back on import; this module only reads and writes the
//! envelope, with the local checks that come before the engine is called:
//! - the file holds no ids, no company name, no author and no figures from
//!   a run (no `cells`, `rows`, `totals` or `balances`). The only decimal
//!   strings in it are the owner-typed thresholds of amount filters, kept
//!   exactly as typed (Q-A09d); nothing here does arithmetic;
//! - caps (Q-A09g): the file ≤ 2 MB, ≤ 200 reports, each definition at the
//!   engine's cap for its kind, JSON nesting ≤ 64;
//! - a file or definition `schemaVersion` newer than this build knows is
//!   refused before anything stricter (the owner is asked to update).
//!
//! No I/O: the caller reads and writes the bytes (temp file and rename).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `format` of every report file.
pub const FORMAT: &str = "clientledger-report";
/// The file schema this build reads and writes.
pub const SCHEMA_VERSION: u32 = 1;
/// The file name extension (without the dot).
pub const EXTENSION: &str = "clreport";
/// The largest file read or written (Q-A09g).
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
/// The most reports in one file (the engine's export cap).
pub const MAX_REPORTS: usize = 200;
/// The most references one report may carry (the engine's cap).
pub const MAX_REFERENCES: usize = 5000;
/// The deepest JSON nesting read.
pub const MAX_DEPTH: usize = 64;
/// The longest report or group name (the engine's cap).
pub const MAX_NAME_CHARS: usize = 200;

/// Why a report file can't be read or written. Holds no text from the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FileError {
    /// Not a `ClientLedger` report file (not JSON, a wrong `format`, a
    /// missing or unknown key, a bad name or kind).
    #[error("this isn't a ClientLedger report file")]
    NotReportFile,
    /// Over a cap: the file, a definition, or the number of reports.
    #[error("this report file is too large")]
    TooLarge,
    /// Saved by a newer version of the app (the file or a definition).
    #[error("this report file was saved by a newer version of the app")]
    Newer,
    /// The file has no reports.
    #[error("this report file has no reports")]
    Empty,
    /// Writing only: a value looks like an id, or a key holds run figures
    /// (a second check behind the engine's).
    #[error("this report file would hold company data")]
    CompanyData,
}

/// A report file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportFile {
    /// Always [`FORMAT`].
    pub format: String,
    /// The file schema ([`SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// The app that wrote it (`ClientLedger <version>`).
    pub app: String,
    /// When (UTC, RFC 3339).
    pub exported_at: String,
    /// The reports, in the order exported.
    pub reports: Vec<FileReport>,
}

/// One report in a file. `Debug` shows its kind only (names are owner
/// data).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileReport {
    /// Its memorized name.
    pub name: String,
    /// Its group's name, matched by name on import.
    #[serde(default)]
    pub group: Option<String>,
    /// `builder` or `standard`.
    pub kind: String,
    /// The definition, with `"$ref:N"` where an id was.
    pub definition: Value,
    /// What each `$ref:N` named in the exporting company.
    #[serde(default)]
    pub references: Vec<Value>,
}

impl std::fmt::Debug for FileReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileReport")
            .field("kind", &self.kind)
            .field("references", &self.references.len())
            .finish_non_exhaustive()
    }
}

impl ReportFile {
    /// A file of `reports` written by `app` at `exported_at`.
    #[must_use]
    pub fn new(app: &str, exported_at: &str, reports: Vec<FileReport>) -> ReportFile {
        ReportFile {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            app: app.into(),
            exported_at: exported_at.into(),
            reports,
        }
    }
}

/// The definition schema this build knows for `kind`.
fn known_version(kind: &str) -> Option<u64> {
    match kind {
        "builder" => Some(u64::from(super::SCHEMA_VERSION)),
        "standard" => Some(u64::from(crate::reports::definition::SCHEMA_VERSION)),
        _ => None,
    }
}

/// The engine's cap on a definition of `kind`.
fn definition_cap(kind: &str) -> usize {
    if kind == "builder" {
        super::MAX_BYTES
    } else {
        crate::reports::definition::MAX_BYTES
    }
}

/// True when `v` nests deeper than [`MAX_DEPTH`].
fn too_deep(v: &Value, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return true;
    }
    match v {
        Value::Array(a) => a.iter().any(|x| too_deep(x, depth + 1)),
        Value::Object(m) => m.values().any(|x| too_deep(x, depth + 1)),
        _ => false,
    }
}

/// A `schemaVersion` that is newer than `known`; a version that isn't a
/// positive whole number is left to the strict checks.
fn newer(v: Option<&Value>, known: u64) -> bool {
    v.and_then(Value::as_u64).is_some_and(|n| n > known)
}

/// Reads a report file from its bytes, in this order: the size (before
/// any parse), UTF-8 and JSON, the nesting bound, `format`, then a newer
/// file or definition `schemaVersion` (before any strict check could call
/// a new key unknown), then the strict envelope, the report count and each
/// report's kind, name and size.
///
/// # Errors
/// Any [`FileError`].
pub fn read(bytes: &[u8]) -> Result<ReportFile, FileError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| FileError::NotReportFile)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    // serde_json stops at 128 levels on its own; the tree is then bounded
    // at 64 here.
    let value: Value = serde_json::from_str(text).map_err(|_| FileError::NotReportFile)?;
    if too_deep(&value, 0) {
        return Err(FileError::NotReportFile);
    }
    if value.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(FileError::NotReportFile);
    }
    if newer(value.get("schemaVersion"), u64::from(SCHEMA_VERSION)) {
        return Err(FileError::Newer);
    }
    if let Some(reports) = value.get("reports").and_then(Value::as_array) {
        for r in reports {
            let kind = r
                .get("definition")
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str)
                .or_else(|| r.get("kind").and_then(Value::as_str))
                .unwrap_or_default();
            if let Some(known) = known_version(kind)
                && newer(
                    r.get("definition").and_then(|d| d.get("schemaVersion")),
                    known,
                )
            {
                return Err(FileError::Newer);
            }
        }
    }
    let file: ReportFile = serde_json::from_value(value).map_err(|_| FileError::NotReportFile)?;
    if file.schema_version != SCHEMA_VERSION {
        return Err(FileError::NotReportFile);
    }
    check(&file)?;
    Ok(file)
}

/// The checks both reading and writing make on every report.
fn check(file: &ReportFile) -> Result<(), FileError> {
    if file.reports.is_empty() {
        return Err(FileError::Empty);
    }
    if file.reports.len() > MAX_REPORTS {
        return Err(FileError::TooLarge);
    }
    for r in &file.reports {
        let Some(known) = known_version(&r.kind) else {
            return Err(FileError::NotReportFile);
        };
        if r.definition.get("kind").and_then(Value::as_str) != Some(r.kind.as_str()) {
            return Err(FileError::NotReportFile);
        }
        match r.definition.get("schemaVersion").and_then(Value::as_u64) {
            Some(v) if v > known => return Err(FileError::Newer),
            Some(v) if v >= 1 => {}
            _ => return Err(FileError::NotReportFile),
        }
        if !name_ok(&r.name) || r.group.as_deref().is_some_and(|g| !name_ok(g)) {
            return Err(FileError::NotReportFile);
        }
        if r.references.len() > MAX_REFERENCES || !r.references.iter().all(Value::is_object) {
            return Err(FileError::NotReportFile);
        }
        let size = serde_json::to_string(&r.definition)
            .map_err(|_| FileError::NotReportFile)?
            .len();
        if size > definition_cap(&r.kind) {
            return Err(FileError::TooLarge);
        }
    }
    Ok(())
}

/// A name the engine takes: not blank, at most [`MAX_NAME_CHARS`].
fn name_ok(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && n.chars().count() <= MAX_NAME_CHARS
}

/// Keys that hold run figures, never part of a definition.
const FIGURE_KEYS: [&str; 4] = ["cells", "rows", "totals", "balances"];

/// True when `v` holds an id-shaped string or a figures key anywhere.
fn holds_company_data(v: &Value) -> bool {
    match v {
        Value::String(s) => has_uuid(s),
        Value::Array(a) => a.iter().any(holds_company_data),
        Value::Object(m) => m.iter().any(|(k, x)| {
            FIGURE_KEYS.contains(&k.as_str()) || has_uuid(k) || holds_company_data(x)
        }),
        _ => false,
    }
}

/// True when `s` contains a uuid-shaped run (`8-4-4-4-12` hex digits).
#[must_use]
pub fn has_uuid(s: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let b = s.as_bytes();
    if b.len() < 36 {
        return false;
    }
    (0..=b.len() - 36).any(|start| {
        let mut i = start;
        GROUPS.iter().enumerate().all(|(n, &len)| {
            let ok = b[i..i + len].iter().all(u8::is_ascii_hexdigit);
            i += len;
            if n < GROUPS.len() - 1 {
                let dash = b.get(i) == Some(&b'-');
                i += 1;
                ok && dash
            } else {
                ok
            }
        })
    })
}

/// The file's bytes (pretty-printed UTF-8 JSON), after the same checks a
/// read makes, plus the no-company-data check: no id-shaped value and no
/// figures key anywhere.
///
/// # Errors
/// Any [`FileError`]; [`FileError::TooLarge`] when the file would be over
/// 2 MB (a reader would refuse it).
pub fn write(file: &ReportFile) -> Result<Vec<u8>, FileError> {
    if file.format != FORMAT || file.schema_version != SCHEMA_VERSION {
        return Err(FileError::NotReportFile);
    }
    check(file)?;
    for r in &file.reports {
        if holds_company_data(&r.definition) || r.references.iter().any(holds_company_data) {
            return Err(FileError::CompanyData);
        }
    }
    let bytes = serde_json::to_vec_pretty(file).map_err(|_| FileError::NotReportFile)?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(FileError::TooLarge);
    }
    Ok(bytes)
}

/// `name`, or `"<name> (2)"`, `"<name> (3)"`, … : the first that `taken`
/// says is free (the Rename choice on a name clash). Names compare as the
/// engine's unique index does, so `taken` should ignore case.
#[must_use]
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let base = name.trim();
    (2..=1000)
        .map(|n| format!("{base} ({n})"))
        .find(|c| !taken(c))
        .unwrap_or_else(|| format!("{base} (copy)"))
}

/// What a reference names, as the exporting company knew it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// An account (by number, else name).
    Account,
    /// A client or vendor.
    Contact,
    /// An item.
    Item,
    /// A tag (by its group and name).
    Tag,
    /// A tag group.
    TagGroup,
    /// A budget (by name and fiscal year).
    Budget,
}

/// A reference read for display. `Debug` shows its kind only.
#[derive(Clone, PartialEq, Eq)]
pub struct Reference {
    /// What it names.
    pub kind: RefKind,
    /// Its name (`None` when the exporting company had lost the row).
    pub name: Option<String>,
    /// An account's number.
    pub number: Option<String>,
    /// A contact's type (`customer`, `vendor`, `both`).
    pub contact_type: Option<String>,
    /// A tag's group name (`None`: no group).
    pub tag_group_name: Option<String>,
    /// A budget's fiscal year.
    pub fiscal_year: Option<i64>,
    /// The exporting company no longer had the row (`missing: true`).
    pub missing: bool,
}

impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reference")
            .field("kind", &self.kind)
            .field("missing", &self.missing)
            .finish_non_exhaustive()
    }
}

impl Reference {
    /// Reads a reference object; `None` for an unknown kind.
    #[must_use]
    pub fn of(v: &Value) -> Option<Reference> {
        let kind = match v.get("kind")?.as_str()? {
            "account" => RefKind::Account,
            "contact" => RefKind::Contact,
            "item" => RefKind::Item,
            "tag" => RefKind::Tag,
            "tagGroup" => RefKind::TagGroup,
            "budget" => RefKind::Budget,
            _ => return None,
        };
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Reference {
            kind,
            name: text("name"),
            number: text("number"),
            contact_type: text("contactType"),
            tag_group_name: text("tagGroupName"),
            fiscal_year: v.get("fiscalYear").and_then(Value::as_i64),
            missing: v.get("missing").and_then(Value::as_bool) == Some(true),
        })
    }

    /// What the file named, for a review card ("6100 · Shop supplies",
    /// "Region: West", "Plan (2026)"); `None` for a missing row.
    #[must_use]
    pub fn label(&self) -> Option<String> {
        let name = self.name.as_deref()?;
        Some(match self.kind {
            RefKind::Account => match self.number.as_deref() {
                Some(n) if !n.is_empty() => format!("{n} \u{b7} {name}"),
                _ => name.to_string(),
            },
            RefKind::Tag => match self.tag_group_name.as_deref() {
                Some(g) => format!("{g}: {name}"),
                None => name.to_string(),
            },
            RefKind::Budget => match self.fiscal_year {
                Some(y) => format!("{name} ({y})"),
                None => name.to_string(),
            },
            RefKind::Contact | RefKind::Item | RefKind::TagGroup => name.to_string(),
        })
    }
}

#[cfg(test)]
#[path = "clreport_tests.rs"]
mod tests;
