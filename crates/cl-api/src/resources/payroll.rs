//! Payroll import (P30; engine U-242 to U-247, U-250, U-251).
//!
//! The engine does all of the accounting: it reads the file, sums every
//! amount exactly, builds one summary entry per pay date, splits expenses by
//! department, refuses an entry that does not balance, and posts every entry
//! of an import in one transaction. This module only carries its answers.
//!
//! - Nothing here parses an amount from the file or adds one up. Preview
//!   amounts and totals are the engine's exact decimal strings, read into
//!   [`Money`]; an unreadable one is a `Decode` error, never zero.
//! - Every post asks for summary entries (`aggregationMode: "summary"`) and
//!   for the employee rows and stored upload to be discarded once posted
//!   (`discardEmployeeDetail: true`, Q-150, U-251). [`PostRequest`] has no
//!   way to change either.
//! - Post and reverse take the screen's [`IdempotencyKey`] (`payroll-import-post-v1`
//!   replays it) and use the Import time class: one attempt of up to
//!   120 s. The screen never sends either again on its own; "Try again"
//!   after a lost answer is the owner's click with the same key.
//! - An upload always carries the pay period and check date, and a neutral
//!   file name (`payroll.<ext>`): the owner's file name never reaches the
//!   engine's history.
//! - `Debug` output names ids, dates, statuses and counts only: never
//!   amounts, column headers, sample cells, department names or memos.

use std::collections::BTreeMap;
use std::fmt;

use cl_core::ids::{AccountId, TagId, TransactionId, parse_uuid};
use cl_core::{CalendarDate, Money};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::customer_payments::{error_code, require};
use crate::resources::operations::{
    Operation, PAYROLL_ACCOUNT_MAPPINGS, PAYROLL_ACCOUNT_MAPPINGS_SAVE, PAYROLL_APPLY_MAPPING,
    PAYROLL_AUTO_MAP, PAYROLL_DESCRIPTION_MAP, PAYROLL_DESCRIPTION_MAP_SAVE, PAYROLL_GENERATE_JE,
    PAYROLL_POST, PAYROLL_REVERSE, PAYROLL_SESSION, PAYROLL_SESSION_DELETE, PAYROLL_SESSIONS,
    PAYROLL_TEMPLATE_CREATE, PAYROLL_TEMPLATE_DELETE, PAYROLL_TEMPLATE_UPDATE, PAYROLL_TEMPLATES,
    PAYROLL_UPLOAD, PAYROLL_VALIDATE,
};
use crate::resources::posting::{ClosedPeriodAuthorization, PostingOptions, posting_body};
use crate::session::Session;
use crate::wire::Capabilities;

const WHAT: &str = "payroll import";

/// The engine's upload limit (10 MB), checked before anything is sent.
pub const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
/// The file types the engine reads.
pub const FILE_EXTENSIONS: [&str; 5] = ["csv", "tsv", "txt", "xlsx", "xls"];
/// The most departments one import maps to tags (the engine's limit).
pub const MAX_DEPARTMENTS: usize = 200;
/// The longest department name the engine takes.
pub const MAX_DEPARTMENT_CHARS: usize = 100;
/// The longest reversal reason.
pub const MAX_REASON_CHARS: usize = 500;
/// History page size.
pub const HISTORY_PAGE: u16 = 50;
/// The most History pages read at once (1,000 imports).
pub const HISTORY_MAX_PAGES: usize = 20;

/// The capabilities the payroll screen needs, all three (Q-P30g).
pub const REQUIRED_CAPABILITIES: [&str; 3] = [
    Capabilities::PAYROLL_IMPORT_EXACT,
    Capabilities::PAYROLL_IMPORT_POST,
    Capabilities::PAYROLL_IMPORT_SENSITIVE,
];

/// True when the engine has everything the payroll screen needs.
#[must_use]
pub fn payroll_supported(caps: &Capabilities) -> bool {
    REQUIRED_CAPABILITIES.iter().all(|c| caps.has(c))
}

fn require_all(s: &Session) -> Result<(), ApiError> {
    for c in REQUIRED_CAPABILITIES {
        require(s, c)?;
    }
    Ok(())
}

fn decode_err() -> ApiError {
    ApiError::Decode { what: WHAT }
}

/// A payroll import (a session in the engine).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PayrollSessionId(uuid::Uuid);

impl PayrollSessionId {
    /// Parses an import id.
    ///
    /// # Errors
    /// `Decode` when it is not a UUID.
    pub fn parse(s: &str) -> Result<PayrollSessionId, ApiError> {
        parse_uuid(s)
            .map(PayrollSessionId)
            .map_err(|_| decode_err())
    }
}

impl std::str::FromStr for PayrollSessionId {
    type Err = ApiError;

    fn from_str(s: &str) -> Result<PayrollSessionId, ApiError> {
        PayrollSessionId::parse(s)
    }
}

impl fmt::Display for PayrollSessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// How a file carries payroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImportMode {
    /// One row per worker (Mode A): the engine maps columns and sums.
    EmployeeLevel,
    /// A ready-made journal export (Mode B): one row per account line.
    PrebuiltJe,
}

impl ImportMode {
    /// The wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            ImportMode::EmployeeLevel => "employee_level",
            ImportMode::PrebuiltJe => "prebuilt_je",
        }
    }

    fn parse(s: &str) -> Option<ImportMode> {
        match s {
            "employee_level" => Some(ImportMode::EmployeeLevel),
            "prebuilt_je" => Some(ImportMode::PrebuiltJe),
            _ => None,
        }
    }
}

/// Where an import stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SessionStatus {
    /// File read, not mapped yet.
    Uploaded,
    /// Columns mapped.
    Mapped,
    /// Checked; ready to post.
    Validated,
    /// Posted.
    Posted,
    /// The engine could not finish it.
    Failed,
    /// Reversed (or abandoned).
    Cancelled,
}

impl SessionStatus {
    /// The wire name.
    #[must_use]
    pub const fn as_wire(self) -> &'static str {
        match self {
            SessionStatus::Uploaded => "uploaded",
            SessionStatus::Mapped => "mapped",
            SessionStatus::Validated => "validated",
            SessionStatus::Posted => "posted",
            SessionStatus::Failed => "failed",
            SessionStatus::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Option<SessionStatus> {
        Some(match s {
            "uploaded" => SessionStatus::Uploaded,
            "mapped" => SessionStatus::Mapped,
            "validated" => SessionStatus::Validated,
            "posted" => SessionStatus::Posted,
            "failed" => SessionStatus::Failed,
            "cancelled" => SessionStatus::Cancelled,
            _ => return None,
        })
    }

    /// A draft the owner can still discard (not posted, not reversed).
    #[must_use]
    pub const fn is_unposted_draft(self) -> bool {
        matches!(
            self,
            SessionStatus::Uploaded | SessionStatus::Mapped | SessionStatus::Validated
        )
    }
}

/// A file to upload. Every field is required: the engine's duplicate and
/// overlap checks read the period and date from here.
#[derive(Clone, PartialEq, Eq)]
pub struct UploadRequest {
    /// How the file carries payroll.
    pub mode: ImportMode,
    /// The file's bytes, exactly as read.
    pub bytes: Vec<u8>,
    /// The owner's file name; only its extension is used.
    pub file_name: String,
    /// First day of the pay period.
    pub period_start: CalendarDate,
    /// Last day of the pay period.
    pub period_end: CalendarDate,
    /// The check (pay) date, used for rows without one.
    pub check_date: CalendarDate,
}

impl fmt::Debug for UploadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadRequest")
            .field("mode", &self.mode)
            .field("bytes", &self.bytes.len())
            .field("period_start", &self.period_start)
            .field("period_end", &self.period_end)
            .field("check_date", &self.check_date)
            .finish_non_exhaustive()
    }
}

/// The file's extension when the engine reads that type (lower-case).
#[must_use]
pub fn file_extension(file_name: &str) -> Option<String> {
    let ext = file_name.rsplit_once('.')?.1.to_ascii_lowercase();
    FILE_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

impl UploadRequest {
    /// Checks the request before anything is sent.
    ///
    /// # Errors
    /// `AttachmentTooLarge` over 10 MB; `AttachmentType` for another file
    /// type; `InvalidValue` for an empty file, a period that ends before it
    /// starts, or a check date before the period starts.
    pub fn check(&self) -> Result<String, LocalValidation> {
        if self.bytes.len() > MAX_UPLOAD_BYTES {
            return Err(LocalValidation::AttachmentTooLarge);
        }
        let ext = file_extension(&self.file_name).ok_or(LocalValidation::AttachmentType)?;
        if self.bytes.is_empty()
            || self.period_end < self.period_start
            || self.check_date < self.period_start
        {
            return Err(LocalValidation::InvalidValue);
        }
        Ok(ext)
    }

    fn multipart(&self, ext: &str) -> (String, Vec<u8>) {
        let boundary = format!("cl-payroll-{}", uuid::Uuid::new_v4().simple());
        let mut body = Vec::with_capacity(self.bytes.len() + 1024);
        for (name, value) in [
            ("importMode", self.mode.as_wire().to_string()),
            ("payPeriodStart", self.period_start.to_wire()),
            ("payPeriodEnd", self.period_end.to_wire()),
            ("checkDate", self.check_date.to_wire()),
        ] {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
                )
                .as_bytes(),
            );
        }
        let content_type = if ext.starts_with("xls") {
            "application/vnd.ms-excel"
        } else {
            "text/csv"
        };
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"payroll.{ext}\"\r\nContent-Type: {content_type}\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&self.bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (boundary, body)
    }
}

/// The first rows of a file as the engine read them. Owner data (a worker
/// column holds names): the screen shows a cell only through
/// `cl_model::payroll_import::SampleGuard`; `Debug` prints the shape only.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SampleRows(pub Vec<Vec<String>>);

impl fmt::Debug for SampleRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SampleRows({} rows)", self.0.len())
    }
}

/// What the engine made of an upload.
#[derive(Clone, PartialEq, Eq)]
pub struct UploadOutcome {
    /// The new import.
    pub session: PayrollSessionId,
    /// The header row's cells (column names; owner data).
    pub headers: Vec<String>,
    /// Up to 25 data rows (owner data).
    pub samples: SampleRows,
    /// The engine's provider guess (`gusto`, `adp_run_gli`, …).
    pub detected_provider: Option<String>,
    /// The mode the engine settled on.
    pub mode: Option<ImportMode>,
    /// Data rows in the file.
    pub row_count: u64,
    /// The same file was imported before.
    pub is_duplicate: bool,
    /// The same provider run was imported before.
    pub is_duplicate_by_key: bool,
}

impl fmt::Debug for UploadOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadOutcome")
            .field("session", &self.session)
            .field("columns", &self.headers.len())
            .field("detected_provider", &self.detected_provider)
            .field("mode", &self.mode)
            .field("row_count", &self.row_count)
            .field("is_duplicate", &self.is_duplicate)
            .field("is_duplicate_by_key", &self.is_duplicate_by_key)
            .finish_non_exhaustive()
    }
}

/// One mapped column (`{source, format?}`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnSource {
    /// The header in the file.
    pub source: String,
    /// A date format for this column (`MM/DD/YYYY`), when it differs.
    pub format: Option<String>,
}

/// A skip rule (`blank_field` or `value_match`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipRule {
    /// The engine field the rule reads.
    pub field: String,
    /// `None` = skip when blank; `Some` = skip when the cell is one of these.
    pub values: Option<Vec<String>>,
}

/// A Mode A column mapping in the engine's `apply-mapping` shape, plus the
/// desktop's own `clDepartmentTags` (kept in a saved template only).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ColumnMap {
    /// 0-based header row.
    pub header_row: u32,
    /// 0-based first data row.
    pub data_start_row: u32,
    /// Rows at the end to drop (total rows).
    pub skip_footer_rows: u32,
    /// The file's date format.
    pub date_format: Option<String>,
    /// Engine field → column.
    pub mappings: BTreeMap<String, ColumnSource>,
    /// Rows to skip.
    pub skip_rules: Vec<SkipRule>,
    /// Department (as in the file) → tag; `None` = no tag.
    pub department_tags: BTreeMap<String, Option<TagId>>,
    /// The file's columns the owner left out (`clLeftOutColumns`, kept in a
    /// saved template only); `None` for a template saved before it was
    /// kept (S4).
    pub left_out: Option<Vec<String>>,
}

impl ColumnMap {
    /// The file columns this layout maps (to find it again for a file).
    #[must_use]
    pub fn sources(&self) -> Vec<String> {
        self.mappings.values().map(|s| s.source.clone()).collect()
    }

    /// The `apply-mapping` body (without the department tags).
    #[must_use]
    pub fn apply_body(&self) -> Value {
        let mut mappings = Map::new();
        for (field, src) in &self.mappings {
            let mut m = Map::new();
            m.insert("source".into(), json!(src.source));
            if let Some(f) = &src.format {
                m.insert("format".into(), json!(f));
            }
            mappings.insert(field.clone(), Value::Object(m));
        }
        let rules: Vec<Value> = self
            .skip_rules
            .iter()
            .map(|r| match &r.values {
                None => json!({"type": "blank_field", "field": r.field}),
                Some(v) => json!({"type": "value_match", "field": r.field, "values": v}),
            })
            .collect();
        let mut body = Map::new();
        body.insert("header_row".into(), json!(self.header_row));
        body.insert("data_start_row".into(), json!(self.data_start_row));
        body.insert("skip_footer_rows".into(), json!(self.skip_footer_rows));
        if let Some(f) = &self.date_format {
            body.insert("date_format".into(), json!(f));
        }
        body.insert("mappings".into(), Value::Object(mappings));
        body.insert("skip_rules".into(), Value::Array(rules));
        body.insert("defaults".into(), json!({"is_contractor": false}));
        Value::Object(body)
    }

    /// The template's `columnMap` (the apply body plus `clDepartmentTags`).
    #[must_use]
    pub fn template_value(&self) -> Value {
        let mut v = self.apply_body();
        if let Value::Object(m) = &mut v {
            let tags: Map<String, Value> = self
                .department_tags
                .iter()
                .map(|(d, t)| (d.clone(), t.map_or(Value::Null, |t| json!(t.to_string()))))
                .collect();
            m.insert("clDepartmentTags".into(), Value::Object(tags));
            if let Some(l) = &self.left_out {
                m.insert("clLeftOutColumns".into(), json!(l));
            }
        }
        v
    }

    /// Reads a template's `columnMap`; `None` when it is not one.
    #[must_use]
    pub fn from_value(v: &Value) -> Option<ColumnMap> {
        let o = v.as_object()?;
        let n = |k: &str, d: u32| {
            o.get(k)
                .and_then(Value::as_u64)
                .and_then(|x| u32::try_from(x).ok())
                .unwrap_or(d)
        };
        let mut mappings = BTreeMap::new();
        for (field, src) in o.get("mappings")?.as_object()? {
            let source = src.get("source")?.as_str()?.to_string();
            let format = src
                .get("format")
                .and_then(Value::as_str)
                .map(str::to_string);
            mappings.insert(field.clone(), ColumnSource { source, format });
        }
        let skip_rules = o
            .get("skip_rules")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter_map(|r| {
                        let field = r.get("field")?.as_str()?.to_string();
                        let values = r.get("values").and_then(Value::as_array).map(|v| {
                            v.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect()
                        });
                        Some(SkipRule { field, values })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let department_tags = o
            .get("clDepartmentTags")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .map(|(d, t)| (d.clone(), t.as_str().and_then(|s| s.parse().ok())))
                    .collect()
            })
            .unwrap_or_default();
        Some(ColumnMap {
            header_row: n("header_row", 0),
            data_start_row: n("data_start_row", 1),
            skip_footer_rows: n("skip_footer_rows", 0),
            date_format: o
                .get("date_format")
                .and_then(Value::as_str)
                .map(str::to_string),
            mappings,
            skip_rules,
            department_tags,
            left_out: o
                .get("clLeftOutColumns")
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                }),
        })
    }
}

/// A saved column layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayrollTemplate {
    /// Its id.
    pub id: uuid::Uuid,
    /// The owner's name for it.
    pub name: String,
    /// The provider key it is for (`custom` for generic files).
    pub provider_key: String,
    /// Built into the engine (not the owner's).
    pub is_system: bool,
    /// The layout, when readable.
    pub column_map: Option<ColumnMap>,
    /// When it last changed (RFC 3339 text, for ordering only).
    pub updated_at: Option<String>,
}

/// A validation message, without the engine's text (it may quote cells).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationMessage {
    /// The data row, for row messages.
    pub row_number: Option<u64>,
    /// The engine field (`federal_income_tax`).
    pub field: String,
    /// The engine code (`AMOUNT_INVALID`, `MISSING_REQUIRED`, …).
    pub code: String,
    /// Blocks posting.
    pub is_error: bool,
}

/// The engine's check of an import. Debug shows counts only (department
/// names stay out of logs).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct ValidationSummary {
    /// Rows read.
    pub total_rows: u64,
    /// Rows with an error.
    pub error_rows: u64,
    /// Rows with only warnings.
    pub warning_rows: u64,
    /// Every message.
    pub messages: Vec<ValidationMessage>,
    /// Every distinct department in the file (`payroll-import-review-v1`,
    /// S3); `None` from an engine without it (the screen then reads the
    /// sample rows).
    pub departments: Option<Vec<String>>,
    /// The engine listed only the first departments (a very long list).
    pub departments_truncated: bool,
}

impl std::fmt::Debug for ValidationSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationSummary")
            .field("total_rows", &self.total_rows)
            .field("error_rows", &self.error_rows)
            .field("warning_rows", &self.warning_rows)
            .field("messages", &self.messages)
            .field("departments", &self.departments.as_ref().map(Vec::len))
            .field("departments_truncated", &self.departments_truncated)
            .finish()
    }
}

impl ValidationSummary {
    /// True when nothing blocks posting.
    #[must_use]
    pub fn can_continue(&self) -> bool {
        self.error_rows == 0 && self.messages.iter().all(|m| !m.is_error)
    }
}

/// One line of a preview entry.
#[derive(Clone, PartialEq, Eq)]
pub struct PreviewLine {
    /// The engine's line type (Mode A) or the file's description (Mode B).
    pub line_type: String,
    /// The engine's description of the line.
    pub description: String,
    /// The mapped account, when mapped.
    pub account: Option<AccountId>,
    /// The account's name.
    pub account_name: Option<String>,
    /// The account's number.
    pub account_number: Option<String>,
    /// The expense department this line is for (U-246), when split.
    pub department: Option<String>,
    /// Debit, exact.
    pub debit: Money,
    /// Credit, exact.
    pub credit: Money,
    /// The tag the engine will stamp (department map or Mode B mapping).
    pub tag: Option<TagId>,
}

impl fmt::Debug for PreviewLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewLine")
            .field("line_type", &self.line_type)
            .field("account", &self.account)
            .field("split", &self.department.is_some())
            .field("tag", &self.tag)
            .finish_non_exhaustive()
    }
}

/// One entry the import would post (one pay date).
#[derive(Clone, PartialEq, Eq)]
pub struct PreviewEntry {
    /// The pay date.
    pub date: CalendarDate,
    /// The engine's memo (no names, U-242).
    pub memo: String,
    /// The lines.
    pub lines: Vec<PreviewLine>,
    /// Debits, as the engine summed them.
    pub total_debits: Money,
    /// Credits, as the engine summed them.
    pub total_credits: Money,
    /// Debits minus credits, exact (U-250).
    pub difference: Money,
    /// Debits equal credits.
    pub is_balanced: bool,
}

impl fmt::Debug for PreviewEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewEntry")
            .field("date", &self.date)
            .field("lines", &self.lines.len())
            .field("is_balanced", &self.is_balanced)
            .finish_non_exhaustive()
    }
}

/// The entries an import would post, with the engine's token for them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preview {
    /// One per pay date, oldest first.
    pub entries: Vec<PreviewEntry>,
    /// The engine's hash of these lines (U-244); sent back with the post.
    pub token: Option<String>,
}

impl Preview {
    /// Every entry balances.
    #[must_use]
    pub fn is_balanced(&self) -> bool {
        !self.entries.is_empty() && self.entries.iter().all(|e| e.is_balanced)
    }
}

/// A tag for one preview line (`lineTags`): addressed by position and pinned
/// by the entry's date, so a changed preview is refused rather than tagged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineTag {
    /// The entry's position in the preview.
    pub entry: usize,
    /// The line's position in the entry.
    pub line: usize,
    /// The entry's date.
    pub date: CalendarDate,
    /// The tag, or `None` for no tag.
    pub tag: Option<TagId>,
}

/// What to post. The body always asks for summary entries and for the
/// employee detail to be discarded; nothing here changes that.
#[derive(Clone, Debug, Default)]
pub struct PostRequest {
    /// Post although earlier imports overlap (after the owner confirmed).
    pub force_post: bool,
    /// Lines whose tag differs from the preview's.
    pub line_tags: Vec<LineTag>,
    /// Department → tag (Mode A with a Department column).
    pub department_tags: BTreeMap<String, Option<TagId>>,
    /// The preview's token, so a changed preview is refused (409).
    pub preview_token: Option<String>,
    /// The closing-date password, when the owner gave it.
    pub closed_period: Option<ClosedPeriodAuthorization>,
}

impl PostRequest {
    fn body(&self) -> Result<Value, LocalValidation> {
        let mut m = Map::new();
        m.insert("aggregationMode".into(), json!("summary"));
        m.insert("discardEmployeeDetail".into(), json!(true));
        m.insert("forcePost".into(), json!(self.force_post));
        if !self.line_tags.is_empty() {
            let tags: Vec<Value> = self
                .line_tags
                .iter()
                .map(|t| {
                    json!({
                        "jeIndex": t.entry,
                        "lineIndex": t.line,
                        "date": t.date.to_wire(),
                        "tagId": t.tag.map(|x| x.to_string()),
                    })
                })
                .collect();
            m.insert("lineTags".into(), Value::Array(tags));
        }
        if !self.department_tags.is_empty() {
            m.insert(
                "departmentTagMap".into(),
                department_map(&self.department_tags)?,
            );
        }
        if let Some(t) = &self.preview_token {
            m.insert("previewToken".into(), json!(t));
        }
        Ok(Value::Object(m))
    }
}

fn department_map(map: &BTreeMap<String, Option<TagId>>) -> Result<Value, LocalValidation> {
    if map.len() > MAX_DEPARTMENTS || map.keys().any(|d| d.chars().count() > MAX_DEPARTMENT_CHARS) {
        return Err(LocalValidation::InvalidValue);
    }
    Ok(Value::Object(
        map.iter()
            .map(|(d, t)| (d.clone(), t.map_or(Value::Null, |t| json!(t.to_string()))))
            .collect(),
    ))
}

/// An earlier posted import that overlaps this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overlap {
    /// The earlier import.
    pub session: Option<PayrollSessionId>,
    /// Its pay period, when it had one.
    pub period: Option<(CalendarDate, CalendarDate)>,
    /// Its check date when it had no period.
    pub check_date: Option<CalendarDate>,
    /// This import's pay dates that land on its entries (±3 days).
    pub matching_dates: Vec<CalendarDate>,
    /// When it was posted, as the engine wrote it (display only).
    pub posted_on: Option<String>,
}

/// The answer to a post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostOutcome {
    /// Posted: the entries (one per pay date). `replayed` when the engine
    /// answered a repeated key with the first result.
    Posted {
        /// The posted entries.
        entries: Vec<TransactionId>,
        /// A replay of an earlier send with the same key.
        replayed: bool,
    },
    /// Earlier imports overlap; nothing was written. "Post anyway" sends
    /// again with `force_post` and a new key.
    NeedsConfirmation {
        /// The overlapping imports.
        overlaps: Vec<Overlap>,
    },
}

/// One import in History. The file's name is never read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    /// The import.
    pub id: PayrollSessionId,
    /// Where it stands.
    pub status: SessionStatus,
    /// Mode A or B.
    pub mode: Option<ImportMode>,
    /// The provider key the engine detected.
    pub provider: Option<String>,
    /// Pay period start.
    pub period_start: Option<CalendarDate>,
    /// Pay period end.
    pub period_end: Option<CalendarDate>,
    /// Check date.
    pub check_date: Option<CalendarDate>,
    /// Data rows.
    pub row_count: u64,
    /// Rows with errors.
    pub error_count: u64,
    /// Posted entries.
    pub entries: Vec<TransactionId>,
    /// The employee rows and upload were discarded (U-251).
    pub detail_discarded: bool,
}

/// One page of History.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionPage {
    /// The imports on this page, newest first.
    pub sessions: Vec<SessionSummary>,
    /// Imports in all.
    pub total: u64,
}

/// A line type's saved account (Mode A).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountMapping {
    /// The engine's line type.
    pub line_type: String,
    /// The mapped account.
    pub account: Option<AccountId>,
}

/// A Mode B description and its account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescriptionMapping {
    /// The description as in the file.
    pub description: String,
    /// `debit` or `credit` (from the file).
    pub side: Option<String>,
    /// The mapped account.
    pub account: Option<AccountId>,
    /// The mapped tag.
    pub tag: Option<TagId>,
    /// The account is the engine's own guess by name or number (status
    /// `suggested`), not one the company saved: the desktop checks it
    /// before pre-selecting it (P30 QA-1). Never sent.
    pub suggested: bool,
}

/// What a reverse did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseOutcome {
    /// Entries voided.
    pub reversed: u64,
    /// The voided entries.
    pub entries: Vec<TransactionId>,
}

/// A payroll refusal the screen explains (U-244, U-250, U-251).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayrollRefusal {
    /// 409 `PAYROLL_ALREADY_POSTED`: someone posted this import already.
    AlreadyPosted,
    /// 409 `PAYROLL_NOT_READY`: not checked yet (or changed back).
    NotReady,
    /// 409 `PAYROLL_DUPLICATE_FILE`: this file was posted before.
    DuplicateFile {
        /// The earlier import.
        prior: Option<PayrollSessionId>,
        /// When it was posted (engine text).
        posted_at: Option<String>,
    },
    /// 409 `PAYROLL_DUPLICATE_RUN`: this provider run was posted before.
    DuplicateRun {
        /// The earlier import.
        prior: Option<PayrollSessionId>,
        /// When it was posted (engine text).
        posted_at: Option<String>,
    },
    /// 409 `PAYROLL_PREVIEW_CHANGED`: the entries changed since the preview.
    PreviewChanged,
    /// 409 `PAYROLL_NOT_POSTED`: nothing to reverse.
    NotPosted,
    /// 422 `PAYROLL_UNBALANCED`: a pay date does not balance.
    Unbalanced {
        /// The pay date.
        date: Option<CalendarDate>,
        /// Debits minus credits.
        difference: Option<Money>,
    },
    /// 422 `PAYROLL_UNMAPPED`: line types with no account.
    Unmapped {
        /// The engine's line types.
        line_types: Vec<String>,
    },
    /// 422 `PAYROLL_DISCARD_UNSUPPORTED`: a check-register import.
    DiscardUnsupported,
    /// 422 `PAYROLL_AMOUNT_INVALID`: a ready-made file's amount isn't whole
    /// cents (rows named in details; never cell text).
    AmountInvalid,
    /// 422 `PAYROLL_DATE_INVALID`: a ready-made file has entry rows with no
    /// date the books can read (`payroll-import-review-v1`). Only the row
    /// numbers are kept; never cell text.
    DateInvalid {
        /// The file's row numbers, in order, without repeats.
        rows: Vec<u64>,
    },
}

/// Classifies `e` (`None` for anything else: closed periods, passwords,
/// hidden accounts, network, plain validation).
#[must_use]
pub fn payroll_refusal(e: &ApiError) -> Option<PayrollRefusal> {
    let code = error_code(e)?;
    let details = match e {
        ApiError::Conflict { details, .. } => Some(details),
        ApiError::Validation(v) => Some(v.details()),
        _ => None,
    };
    let text = |k: &str| details.and_then(|d| d.text(k)).map(str::to_string);
    let prior = || details.and_then(|d| d.id::<PayrollSessionId>("priorSessionId"));
    Some(match code {
        "PAYROLL_ALREADY_POSTED" => PayrollRefusal::AlreadyPosted,
        "PAYROLL_NOT_READY" => PayrollRefusal::NotReady,
        "PAYROLL_DUPLICATE_FILE" => PayrollRefusal::DuplicateFile {
            prior: prior(),
            posted_at: text("postedAt"),
        },
        "PAYROLL_DUPLICATE_RUN" => PayrollRefusal::DuplicateRun {
            prior: prior(),
            posted_at: text("postedAt"),
        },
        "PAYROLL_PREVIEW_CHANGED" => PayrollRefusal::PreviewChanged,
        "PAYROLL_NOT_POSTED" => PayrollRefusal::NotPosted,
        "PAYROLL_UNBALANCED" => PayrollRefusal::Unbalanced {
            date: text("date").and_then(|d| CalendarDate::parse_wire(&d).ok()),
            difference: details.and_then(|d| d.money("difference")),
        },
        "PAYROLL_UNMAPPED" => PayrollRefusal::Unmapped {
            line_types: details
                .and_then(|d| d.raw("lineTypes"))
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        },
        "PAYROLL_DISCARD_UNSUPPORTED" => PayrollRefusal::DiscardUnsupported,
        "PAYROLL_AMOUNT_INVALID" => PayrollRefusal::AmountInvalid,
        "PAYROLL_DATE_INVALID" => {
            let mut rows: Vec<u64> = details
                .and_then(|d| d.raw("rows"))
                .and_then(Value::as_array)
                .map(|l| {
                    l.iter()
                        .filter_map(|r| r.get("rowNumber").and_then(Value::as_u64))
                        .collect()
                })
                .unwrap_or_default();
            rows.sort_unstable();
            rows.dedup();
            PayrollRefusal::DateInvalid { rows }
        }
        _ => return None,
    })
}

// ── Wire readers ──

fn money(v: &Value) -> Result<Money, ApiError> {
    // Exact decimal strings only: a JSON number may have been rounded.
    let s = v.as_str().ok_or(ApiError::InexactMoney { what: WHAT })?;
    Money::parse_wire(s).map_err(|_| decode_err())
}

fn date(v: &Value) -> Option<CalendarDate> {
    let s = v.as_str()?;
    CalendarDate::parse_wire(s.get(..10).unwrap_or(s)).ok()
}

fn opt_id<T: std::str::FromStr>(v: &Value) -> Option<T> {
    v.as_str().and_then(|s| s.parse().ok())
}

fn count(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn parse_json(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| decode_err())
}

fn ids(v: &Value) -> Vec<TransactionId> {
    v.as_array()
        .map(|l| l.iter().filter_map(opt_id).collect())
        .unwrap_or_default()
}

pub(crate) fn upload_from(body: &[u8]) -> Result<UploadOutcome, ApiError> {
    let v = parse_json(body)?;
    let session = PayrollSessionId::parse(v["session"]["id"].as_str().ok_or_else(decode_err)?)?;
    let p = &v["preview"];
    let headers: Vec<String> = p["headers"]
        .as_array()
        .ok_or_else(decode_err)?
        .iter()
        .map(|h| h.as_str().map_or_else(String::new, str::to_string))
        .collect();
    let samples = p["sampleRows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| match r {
                    Value::Array(cells) => cells.iter().map(cell_text).collect(),
                    Value::Object(o) => headers
                        .iter()
                        .map(|h| o.get(h).map_or_else(String::new, cell_text))
                        .collect(),
                    _ => Vec::new(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(UploadOutcome {
        session,
        headers,
        samples: SampleRows(samples),
        detected_provider: p["detectedProvider"].as_str().map(str::to_string),
        mode: p["importMode"].as_str().and_then(ImportMode::parse),
        row_count: count(&p["rowCount"]),
        is_duplicate: p["isDuplicate"].as_bool().unwrap_or(false),
        is_duplicate_by_key: p["isDuplicateByKey"].as_bool().unwrap_or(false),
    })
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Reads the entries; `token_required` (the engine advertises
/// `payroll-import-post-v1`, which always sends `previewToken`) makes a
/// missing token a contract break (`Decode`, S8) rather than a post that
/// skips the engine's "entries changed" check.
pub(crate) fn preview_from(body: &[u8], token_required: bool) -> Result<Preview, ApiError> {
    let v = parse_json(body)?;
    let token = v["previewToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    if token_required && token.is_none() {
        return Err(decode_err());
    }
    let mut entries = Vec::new();
    for e in v["previews"].as_array().ok_or_else(decode_err)? {
        let mut lines = Vec::new();
        for l in e["lines"].as_array().ok_or_else(decode_err)? {
            lines.push(PreviewLine {
                line_type: l["lineType"].as_str().ok_or_else(decode_err)?.to_string(),
                description: l["description"].as_str().unwrap_or_default().to_string(),
                account: opt_id(&l["accountId"]),
                account_name: l["accountName"].as_str().map(str::to_string),
                account_number: l["accountNumber"].as_str().map(str::to_string),
                department: l["department"].as_str().map(str::to_string),
                debit: money(&l["debit"])?,
                credit: money(&l["credit"])?,
                tag: opt_id(&l["tagId"]),
            });
        }
        let total_debits = money(&e["totalDebits"])?;
        let total_credits = money(&e["totalCredits"])?;
        entries.push(PreviewEntry {
            date: date(&e["date"]).ok_or_else(decode_err)?,
            memo: e["memo"].as_str().unwrap_or_default().to_string(),
            lines,
            total_debits,
            total_credits,
            // `difference` is additive (U-250); its absence is a contract
            // break on an engine that advertises payroll-import-exact-v1.
            difference: money(&e["difference"])?,
            is_balanced: e["isBalanced"].as_bool().ok_or_else(decode_err)?,
        });
    }
    Ok(Preview { entries, token })
}

pub(crate) fn validation_from(body: &[u8]) -> Result<ValidationSummary, ApiError> {
    let v = parse_json(body)?;
    let messages = v["messages"]
        .as_array()
        .map(|l| {
            l.iter()
                .map(|m| ValidationMessage {
                    row_number: m["rowNumber"].as_u64(),
                    field: m["field"].as_str().unwrap_or_default().to_string(),
                    code: m["code"].as_str().unwrap_or_default().to_string(),
                    is_error: m["severity"].as_str() != Some("warning"),
                })
                .collect()
        })
        .unwrap_or_default();
    // `payroll-import-review-v1` (S3): every distinct department in a Mode A
    // file (names of departments, never of people).
    let departments = v["departments"].as_array().map(|l| {
        l.iter()
            .filter_map(Value::as_str)
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect()
    });
    Ok(ValidationSummary {
        total_rows: count(&v["totalRows"]),
        error_rows: count(&v["errorRows"]),
        warning_rows: count(&v["warningRows"]),
        messages,
        departments,
        departments_truncated: v["departmentsTruncated"].as_bool().unwrap_or(false),
    })
}

fn overlap_from(o: &Value) -> Overlap {
    let period = o["payPeriod"].as_str().and_then(|p| {
        let (a, b) = p.split_once(" to ")?;
        Some((
            CalendarDate::parse_wire(a).ok()?,
            CalendarDate::parse_wire(b).ok()?,
        ))
    });
    Overlap {
        session: opt_id(&o["sessionId"]),
        check_date: if period.is_none() {
            date(&o["payPeriod"])
        } else {
            None
        },
        period,
        matching_dates: o["matchingDates"]
            .as_array()
            .map(|l| l.iter().filter_map(date).collect())
            .unwrap_or_default(),
        // `filename` is never read (it can name people).
        posted_on: o["postedDate"].as_str().map(str::to_string),
    }
}

pub(crate) fn post_from(body: &[u8], replayed: bool) -> Result<PostOutcome, ApiError> {
    let v = parse_json(body)?;
    if v["requiresConfirmation"].as_bool() == Some(true) {
        let overlaps = v["overlaps"]
            .as_array()
            .map(|l| l.iter().map(overlap_from).collect())
            .unwrap_or_default();
        return Ok(PostOutcome::NeedsConfirmation { overlaps });
    }
    let entries = ids(&v["journalEntryIds"]);
    if entries.is_empty() {
        return Err(decode_err());
    }
    Ok(PostOutcome::Posted { entries, replayed })
}

fn session_from(s: &Value) -> Result<SessionSummary, ApiError> {
    let mut entries = ids(&s["journalEntryIds"]);
    if entries.is_empty()
        && let Some(id) = opt_id(&s["journalEntryId"])
    {
        entries.push(id);
    }
    Ok(SessionSummary {
        id: PayrollSessionId::parse(s["id"].as_str().ok_or_else(decode_err)?)?,
        status: s["status"]
            .as_str()
            .and_then(SessionStatus::parse)
            .ok_or_else(decode_err)?,
        mode: s["importMode"].as_str().and_then(ImportMode::parse),
        provider: s["detectedProvider"].as_str().map(str::to_string),
        period_start: date(&s["payPeriodStart"]),
        period_end: date(&s["payPeriodEnd"]),
        check_date: date(&s["checkDate"]),
        row_count: count(&s["rowCount"]),
        error_count: count(&s["errorCount"]),
        entries,
        detail_discarded: !s["detailDiscardedAt"].is_null(),
    })
}

pub(crate) fn sessions_from(body: &[u8]) -> Result<SessionPage, ApiError> {
    let v = parse_json(body)?;
    let sessions = v["data"]
        .as_array()
        .ok_or_else(decode_err)?
        .iter()
        .map(session_from)
        .collect::<Result<_, _>>()?;
    Ok(SessionPage {
        sessions,
        total: count(&v["total"]),
    })
}

fn template_from(t: &Value) -> Option<PayrollTemplate> {
    Some(PayrollTemplate {
        id: t["id"].as_str()?.parse().ok()?,
        name: t["name"].as_str()?.to_string(),
        provider_key: t["providerKey"].as_str().unwrap_or("custom").to_string(),
        is_system: t["isSystem"].as_bool().unwrap_or(false),
        column_map: ColumnMap::from_value(&t["columnMap"]),
        updated_at: t["updatedAt"].as_str().map(str::to_string),
    })
}

/// Payroll import operations of a session (`session.payroll()`).
#[derive(Clone, Copy, Debug)]
pub struct Payroll<'a>(pub(crate) &'a Session);

impl Session {
    /// Payroll imports in the open company.
    #[must_use]
    pub fn payroll(&self) -> Payroll<'_> {
        Payroll(self)
    }
}

fn at(op: &Operation, id: &str) -> Call {
    Call::new(op.method(), op.route(), op.route().replace("{id}", id))
}

fn company_path(s: &Session, op: &Operation) -> Result<Call, ApiError> {
    let company = match s.scope() {
        crate::session::Scope::Company { company_id, .. } => company_id,
        crate::session::Scope::Workspace => return Err(ApiError::NoCompanyOpen),
    };
    Ok(Call::new(
        op.method(),
        op.route(),
        op.route().replace("{companyId}", &company.to_string()),
    ))
}

impl Payroll<'_> {
    /// Uploads a file (checked first; nothing is sent when it is refused).
    ///
    /// # Errors
    /// `Local(Unsupported)` without the payroll capabilities;
    /// `Local(AttachmentTooLarge | AttachmentType | InvalidValue)` from
    /// [`UploadRequest::check`]; `Validation` when the engine can't read the
    /// file; any [`ApiError`].
    pub async fn upload(&self, req: &UploadRequest) -> Result<UploadOutcome, ApiError> {
        require_all(self.0)?;
        let ext = req.check().map_err(ApiError::Local)?;
        let (boundary, body) = req.multipart(&ext);
        let call = Call::at(Method::Post, PAYROLL_UPLOAD.route()).multipart_body(&boundary, body);
        let reply = self.0.send(&PAYROLL_UPLOAD, call, None).await?;
        upload_from(&reply.body)
    }

    /// One import.
    ///
    /// # Errors
    /// `NotFound`; any [`ApiError`].
    pub async fn session(&self, id: PayrollSessionId) -> Result<SessionSummary, ApiError> {
        require_all(self.0)?;
        let call = at(&PAYROLL_SESSION, &id.to_string());
        let reply = self.0.send(&PAYROLL_SESSION, call, None).await?;
        session_from(&parse_json(&reply.body)?["session"])
    }

    /// Discards an unposted import (the engine refuses a posted one).
    ///
    /// # Errors
    /// `Validation` for a posted import; any [`ApiError`].
    pub async fn discard(&self, id: PayrollSessionId) -> Result<(), ApiError> {
        require_all(self.0)?;
        let call = at(&PAYROLL_SESSION_DELETE, &id.to_string());
        self.0.send(&PAYROLL_SESSION_DELETE, call, None).await?;
        Ok(())
    }

    /// One page of History, newest first; `unposted_only` lists drafts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn sessions(
        &self,
        status: Option<SessionStatus>,
        offset: u64,
    ) -> Result<SessionPage, ApiError> {
        require_all(self.0)?;
        let mut call = Call::at(Method::Get, PAYROLL_SESSIONS.route());
        if let crate::session::Scope::Company { company_id, .. } = self.0.scope() {
            call.query
                .push(("companyId".into(), company_id.to_string()));
        }
        if let Some(s) = status {
            call.query.push(("status".into(), s.as_wire().into()));
        }
        call.query.push(("limit".into(), HISTORY_PAGE.to_string()));
        call.query.push(("offset".into(), offset.to_string()));
        let reply = self.0.send(&PAYROLL_SESSIONS, call, None).await?;
        sessions_from(&reply.body)
    }

    /// Every page of History for `status` (every import when `None`),
    /// newest first, up to [`HISTORY_MAX_PAGES`] pages (M1: drafts past the
    /// first page still show).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn sessions_all(
        &self,
        status: Option<SessionStatus>,
    ) -> Result<SessionPage, ApiError> {
        let mut all = SessionPage {
            total: 0,
            sessions: Vec::new(),
        };
        for _ in 0..HISTORY_MAX_PAGES {
            let offset = u64::try_from(all.sessions.len()).unwrap_or(u64::MAX);
            let page = self.sessions(status, offset).await?;
            all.total = page.total;
            let got = page.sessions.len();
            all.sessions.extend(page.sessions);
            if got == 0 || u64::try_from(all.sessions.len()).unwrap_or(u64::MAX) >= all.total {
                break;
            }
        }
        Ok(all)
    }

    /// Maps a Mode A file's columns.
    ///
    /// # Errors
    /// `Validation` when the engine refuses the mapping; any [`ApiError`].
    pub async fn apply_mapping(
        &self,
        id: PayrollSessionId,
        map: &ColumnMap,
    ) -> Result<(), ApiError> {
        require_all(self.0)?;
        let bytes = crate::session::json_body(&map.apply_body())?;
        let call = at(&PAYROLL_APPLY_MAPPING, &id.to_string()).json_body(bytes);
        self.0.send(&PAYROLL_APPLY_MAPPING, call, None).await?;
        Ok(())
    }

    /// A Mode B file's descriptions with their saved accounts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn description_map(
        &self,
        id: PayrollSessionId,
    ) -> Result<Vec<DescriptionMapping>, ApiError> {
        require_all(self.0)?;
        let call = at(&PAYROLL_DESCRIPTION_MAP, &id.to_string());
        let reply = self.0.send(&PAYROLL_DESCRIPTION_MAP, call, None).await?;
        let v = parse_json(&reply.body)?;
        Ok(v["mappings"]
            .as_array()
            .ok_or_else(decode_err)?
            .iter()
            .filter_map(|m| {
                Some(DescriptionMapping {
                    description: m["sourceDescription"].as_str()?.to_string(),
                    side: m["debitOrCredit"].as_str().map(str::to_string),
                    account: opt_id(&m["accountId"]),
                    tag: opt_id(&m["tagId"]),
                    suggested: m["status"].as_str() == Some("suggested"),
                })
            })
            .collect())
    }

    /// Saves a Mode B file's description → account (and tag) map; the engine
    /// keeps it per company and provider.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for an empty list or an unmapped description;
    /// any [`ApiError`].
    pub async fn save_description_map(
        &self,
        id: PayrollSessionId,
        provider_key: &str,
        mappings: &[DescriptionMapping],
    ) -> Result<(), ApiError> {
        require_all(self.0)?;
        if mappings.is_empty() || mappings.iter().any(|m| m.account.is_none()) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let list: Vec<Value> = mappings
            .iter()
            .map(|m| {
                json!({
                    "sourceDescription": m.description,
                    "accountId": m.account.map(|a| a.to_string()),
                    "tagId": m.tag.map(|t| t.to_string()),
                })
            })
            .collect();
        let bytes =
            crate::session::json_body(&json!({"providerKey": provider_key, "mappings": list}))?;
        let call = at(&PAYROLL_DESCRIPTION_MAP_SAVE, &id.to_string()).json_body(bytes);
        self.0
            .send(&PAYROLL_DESCRIPTION_MAP_SAVE, call, None)
            .await?;
        Ok(())
    }

    /// Checks the mapped rows.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn validate(&self, id: PayrollSessionId) -> Result<ValidationSummary, ApiError> {
        require_all(self.0)?;
        let call = at(&PAYROLL_VALIDATE, &id.to_string()).json_body(b"{}".to_vec());
        let reply = self.0.send(&PAYROLL_VALIDATE, call, None).await?;
        validation_from(&reply.body)
    }

    /// The entries the import would post (summary, one per pay date).
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a department map over the engine's limits;
    /// `InexactMoney` when an amount is not an exact string; any
    /// [`ApiError`].
    pub async fn preview(
        &self,
        id: PayrollSessionId,
        department_tags: &BTreeMap<String, Option<TagId>>,
    ) -> Result<Preview, ApiError> {
        require_all(self.0)?;
        let mut body = Map::new();
        body.insert("aggregationMode".into(), json!("summary"));
        if !department_tags.is_empty() {
            body.insert(
                "departmentTagMap".into(),
                department_map(department_tags).map_err(ApiError::Local)?,
            );
        }
        let bytes = crate::session::json_body(&Value::Object(body))?;
        let call = at(&PAYROLL_GENERATE_JE, &id.to_string()).json_body(bytes);
        let reply = self.0.send(&PAYROLL_GENERATE_JE, call, None).await?;
        preview_from(
            &reply.body,
            self.0
                .client()
                .capabilities()
                .has(Capabilities::PAYROLL_IMPORT_POST),
        )
    }

    /// Posts the import under `key` (made when the owner clicked Post; a
    /// changed body — confirmation, password, tags — gets a new key).
    ///
    /// # Errors
    /// The refusals [`payroll_refusal`] reads; `ClosedPeriod`,
    /// `ClosingPasswordRejected`, `ClosingPasswordRateLimited`;
    /// `Sensitive`; `OutcomeUnknown` when the answer was lost (check the
    /// import, or send again with the same key on the owner's click).
    pub async fn post(
        &self,
        id: PayrollSessionId,
        req: &PostRequest,
        key: IdempotencyKey,
    ) -> Result<PostOutcome, ApiError> {
        require_all(self.0)?;
        let body = req.body().map_err(ApiError::Local)?;
        let opts = PostingOptions {
            closed_period: req.closed_period.clone(),
        };
        let bytes = posting_body(self.0, body, &opts)?;
        let mut call = at(&PAYROLL_POST, &id.to_string()).json_body(bytes);
        call.idempotency_key = Some(key);
        call.once = true;
        let reply = self.0.send(&PAYROLL_POST, call, None).await?;
        post_from(&reply.body, reply.replayed)
    }

    /// Reverses a posted import: every entry is voided in one transaction.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or too long reason; 409
    /// `PAYROLL_NOT_POSTED`; the closed-period errors; any [`ApiError`].
    pub async fn reverse(
        &self,
        id: PayrollSessionId,
        reason: &str,
        closed_period: Option<ClosedPeriodAuthorization>,
        key: IdempotencyKey,
    ) -> Result<ReverseOutcome, ApiError> {
        require_all(self.0)?;
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let opts = PostingOptions { closed_period };
        let bytes = posting_body(self.0, json!({"reason": reason}), &opts)?;
        let mut call = at(&PAYROLL_REVERSE, &id.to_string()).json_body(bytes);
        call.idempotency_key = Some(key);
        call.once = true;
        let reply = self.0.send(&PAYROLL_REVERSE, call, None).await?;
        let v = parse_json(&reply.body)?;
        Ok(ReverseOutcome {
            reversed: count(&v["reversed"]),
            entries: ids(&v["journalEntryIds"]),
        })
    }

    /// Saved column layouts (system ones and the company's own).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn templates(&self) -> Result<Vec<PayrollTemplate>, ApiError> {
        require_all(self.0)?;
        let call = Call::at(Method::Get, PAYROLL_TEMPLATES.route());
        let reply = self.0.send(&PAYROLL_TEMPLATES, call, None).await?;
        let v = parse_json(&reply.body)?;
        Ok(v["templates"]
            .as_array()
            .ok_or_else(decode_err)?
            .iter()
            .filter_map(template_from)
            .collect())
    }

    /// Saves a new layout, or changes `existing`.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a blank or too long name; any [`ApiError`].
    pub async fn save_template(
        &self,
        existing: Option<uuid::Uuid>,
        name: &str,
        provider_key: &str,
        map: &ColumnMap,
    ) -> Result<PayrollTemplate, ApiError> {
        require_all(self.0)?;
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body =
            json!({"name": name, "providerKey": provider_key, "columnMap": map.template_value()});
        let bytes = crate::session::json_body(&body)?;
        let (op, call) = match existing {
            Some(id) => (
                &PAYROLL_TEMPLATE_UPDATE,
                at(&PAYROLL_TEMPLATE_UPDATE, &id.to_string()),
            ),
            None => (
                &PAYROLL_TEMPLATE_CREATE,
                Call::at(Method::Post, PAYROLL_TEMPLATE_CREATE.route()),
            ),
        };
        let reply = self.0.send(op, call.json_body(bytes), None).await?;
        template_from(&parse_json(&reply.body)?["template"]).ok_or_else(decode_err)
    }

    /// Deletes one of the company's layouts.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn delete_template(&self, id: uuid::Uuid) -> Result<(), ApiError> {
        require_all(self.0)?;
        let call = at(&PAYROLL_TEMPLATE_DELETE, &id.to_string());
        self.0.send(&PAYROLL_TEMPLATE_DELETE, call, None).await?;
        Ok(())
    }

    /// The company's line type → account map.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn account_mappings(&self) -> Result<Vec<AccountMapping>, ApiError> {
        require_all(self.0)?;
        let call = company_path(self.0, &PAYROLL_ACCOUNT_MAPPINGS)?;
        let reply = self.0.send(&PAYROLL_ACCOUNT_MAPPINGS, call, None).await?;
        let v = parse_json(&reply.body)?;
        Ok(v["mappings"]
            .as_array()
            .ok_or_else(decode_err)?
            .iter()
            .filter_map(|m| {
                Some(AccountMapping {
                    line_type: m["lineType"].as_str()?.to_string(),
                    account: opt_id(&m["accountId"]),
                })
            })
            .collect())
    }

    /// Saves the company's line type → account map.
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn save_account_mappings(
        &self,
        map: &BTreeMap<String, AccountId>,
    ) -> Result<(), ApiError> {
        require_all(self.0)?;
        let mappings: Map<String, Value> = map
            .iter()
            .map(|(k, a)| (k.clone(), json!(a.to_string())))
            .collect();
        let bytes = crate::session::json_body(&json!({ "mappings": mappings }))?;
        let call = company_path(self.0, &PAYROLL_ACCOUNT_MAPPINGS_SAVE)?.json_body(bytes);
        self.0
            .send(&PAYROLL_ACCOUNT_MAPPINGS_SAVE, call, None)
            .await?;
        Ok(())
    }

    /// The engine's suggested accounts by name (nothing is saved).
    ///
    /// # Errors
    /// Any [`ApiError`].
    pub async fn suggest_accounts(&self) -> Result<BTreeMap<String, AccountId>, ApiError> {
        #[derive(Deserialize)]
        struct W {
            suggestions: BTreeMap<String, String>,
        }
        require_all(self.0)?;
        let mut call = company_path(self.0, &PAYROLL_AUTO_MAP)?.json_body(b"{}".to_vec());
        call.no_key = true;
        let reply = self.0.send(&PAYROLL_AUTO_MAP, call, None).await?;
        let w: W = serde_json::from_slice(&reply.body).map_err(|_| decode_err())?;
        Ok(w.suggestions
            .into_iter()
            .filter_map(|(k, v)| Some((k, v.parse().ok()?)))
            .collect())
    }
}

#[cfg(test)]
#[path = "payroll_tests.rs"]
mod tests;
