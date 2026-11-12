//! Report Builder runs (U-119, A07 plan §4): `POST /report-builder/run`,
//! `POST /report-builder/run/detail`, `GET /report-builder/fields` and
//! `GET /report-builder/starters`, and the adapter from the engine's rows
//! into [`cl_core::report::ReportTable`].
//!
//! Money: every amount cell is the engine's exact decimal string, read with
//! `Decimal::from_str_exact`; anything else is [`ApiError::ReportShape`],
//! never a fallback. Nothing here adds amounts up: a detail page's
//! `subtotal` is the engine's, and the window only checks it for equality.
//!
//! The definition is passed through as JSON (`cl-model` owns its shape and
//! cl-api may not depend on it).
//!
//! The shapes follow engine PR-C (bc41f48d) and its samples in
//! `tests/fixtures/report_builder/` (copied from the engine's
//! `shared/src/report-builder/fixtures/run`).

use std::str::FromStr;

use cl_core::CalendarDate;
use cl_core::ids::TransactionId;
use cl_core::report::{
    Adjusting, Basis, Cell, ColumnDrill, ColumnHead, ColumnKind, DateRange, Drill, ReportDates,
    ReportMeta, ReportRow, ReportTable, RowKind, RowPath, literal_caption,
};
use rust_decimal::Decimal;
use serde::Serialize;
use serde_json::Value;

use crate::client::{Call, Method};
use crate::errors::{ApiError, ConflictReason, LocalValidation};
use crate::resources::operations::{
    REPORT_BUILDER_DETAIL, REPORT_BUILDER_FIELDS, REPORT_BUILDER_RUN, REPORT_BUILDER_STARTERS,
};
use crate::session::Session;
use crate::wire::Capabilities;

/// Detail rows on one `/run/detail` page.
pub const PAGE_ROWS: usize = 500;
/// Detail rows per screen run before a `more` row (Q-311).
pub const RUN_DETAIL_ROWS: usize = 5_000;
/// Open paths a run may send.
pub const MAX_OPEN_PATHS: usize = 1_000;
/// Segments in one open path.
pub const MAX_PATH_SEGMENTS: usize = 4;
/// Period columns a run may ask for (checked before sending).
pub const MAX_PERIODS: usize = 36;
/// Fiscal year starts a run may send.
pub const MAX_FISCAL_YEAR_STARTS: usize = 60;

/// What the run is for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunPurpose {
    /// The window (5,000 detail rows).
    #[default]
    Screen,
    /// A file (100,000 detail rows, A09).
    Export,
}

/// The run's dates: a range, or an as-of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDates {
    /// First day (ranges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<CalendarDate>,
    /// Last day (ranges).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<CalendarDate>,
    /// As of (open items, balances).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_of: Option<CalendarDate>,
}

impl RunDates {
    /// A range.
    #[must_use]
    pub const fn range(r: DateRange) -> RunDates {
        RunDates {
            from: Some(r.from),
            to: Some(r.to),
            as_of: None,
        }
    }

    /// As of a date.
    #[must_use]
    pub const fn as_of(d: CalendarDate) -> RunDates {
        RunDates {
            from: None,
            to: None,
            as_of: Some(d),
        }
    }
}

/// Which definition, on which dates and ranges (shared by run and detail;
/// everything here is in the engine's `fingerprint`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRef {
    /// An inline definition (a starter, the editor's draft).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition: Option<Value>,
    /// A memorized definition's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_id: Option<String>,
    /// The version read (409 `STALE_VERSION` when it moved).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u64>,
    /// Dates.
    pub dates: RunDates,
    /// Period columns (≤ 36), resolved with `cl_core::fiscal`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub period_ranges: Vec<DateRange>,
    /// One array per compare set (≤ 2), each parallel to `period_ranges`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub compare_ranges: Vec<Vec<DateRange>>,
    /// The first day of each fiscal year the run touches.
    pub fiscal_year_starts: Vec<CalendarDate>,
    /// Basis (`None`: the definition's, else accrual — the engine's
    /// fallback, not the company's preference).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<Basis>,
    /// Adjusting lens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjusting: Option<Adjusting>,
}

/// `POST /report-builder/run`.
///
/// A row is expanded when `expand_all` is set, its path is in `open`, or its
/// section has `defaultExpanded` and the path is not in `collapsed` (the
/// engine's rule; `open` wins over `collapsed`, `expand_all` over both).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// Definition, dates and ranges.
    #[serde(flatten)]
    pub reference: RunRef,
    /// Paths opened by the owner (≤ 1,000).
    pub open: Vec<RowPath>,
    /// Paths closed under a `defaultExpanded` section (≤ 1,000).
    pub collapsed: Vec<RowPath>,
    /// Expand everything (bounded by the row caps; `collapsed` is ignored).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub expand_all: bool,
    /// Screen or export.
    pub purpose: RunPurpose,
}

/// `POST /report-builder/run/detail`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailRequest {
    /// The same definition, dates and ranges as the run.
    #[serde(flatten)]
    pub reference: RunRef,
    /// The section.
    pub node_id: String,
    /// The engine path (the innermost group-by line, or `n:<id>`).
    pub path: RowPath,
    /// One period column's dates: its lines only (U-119e). An as-of
    /// column's `from` is [`CalendarDate::min`], sent as `null`. `None`:
    /// every base column.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "wire_range")]
    pub range: Option<DateRange>,
    /// The page after this cursor (`None`: the first page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The run's fingerprint (409 `RUN_CHANGED {reason:'params'}`).
    pub fingerprint: String,
    /// The run's ledger stamp (409 `RUN_CHANGED {reason:'books'}`).
    pub stamp: u64,
}

/// Which of the engine's columns a table shows: the engine keeps a compare
/// amount that only feeds a change as a `hidden` column with null cells, in
/// place, so every row (and every detail page) carries one cell per wire
/// column. The table drops them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnLayout {
    /// Columns on the wire.
    pub wire: usize,
    /// The wire index of each shown column, in order.
    pub shown: Vec<usize>,
}

/// A run's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuilderRun {
    /// The rows as a table (hidden columns dropped). Value cells are
    /// [`Cell::Amount`]; a count or quantity section's unit is the
    /// definition's to apply (`cl_model::report_def::apply_units`).
    pub table: ReportTable,
    /// The wire columns behind the table's (detail pages use it).
    pub layout: ColumnLayout,
    /// Hash of every parameter but the open paths and purpose.
    pub fingerprint: String,
    /// The ledger stamp the run read.
    pub stamp: u64,
    /// Sections the role can't see (their node ids).
    pub denied: Vec<String>,
    /// The paths the engine shows expanded (`expanded: true`): the window's
    /// open set after every run.
    pub expanded: Vec<RowPath>,
    /// A path was capped (its `more` row pages the rest).
    pub truncated: bool,
}

/// One page of detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailPage {
    /// Up to 500 rows (an `opening` row first on a running balance). Their
    /// level is the engine's 0: the window places them under their line.
    pub rows: Vec<ReportRow>,
    /// The next page's cursor (`None`: the last page).
    pub cursor: Option<String>,
    /// The engine's subtotal for the whole path, echoed (equality only).
    pub subtotal: Vec<Cell>,
}

/// A starter report.
#[derive(Clone, Debug, PartialEq)]
pub struct Starter {
    /// Its key (`profit_loss`, `ar_aging_summary`...).
    pub key: String,
    /// Its title (engine text).
    pub title: String,
    /// The standard report it ties out to (`/reports/<tiesTo>`), when the
    /// engine says.
    pub ties_to: Option<String>,
    /// The definition (schema v1 JSON).
    pub definition: Value,
}

/// Why the engine refused a builder run or page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuilderRefusal {
    /// The books changed since the run (409 `RUN_CHANGED {reason:'books'}`).
    BooksChanged,
    /// The run's parameters changed (409 `RUN_CHANGED {reason:'params'}`).
    ParamsChanged,
    /// Over 30 s (503 `REPORT_TIMEOUT`).
    Timeout,
    /// Over 100,000 group rows (422 `REPORT_TOO_LARGE`).
    TooLarge,
    /// The definition breaks a rule (400 `DEFINITION_INVALID {path, code}`;
    /// the run adds `NOT_ON_CASH` and `NOT_IN_COMPANY`).
    Invalid {
        /// Where, from `definition` (when sent), scrubbed: an input key is
        /// `<key>` (S-L3).
        path: Option<String>,
        /// The rule's code (when sent).
        code: Option<String>,
    },
    /// The memorized definition changed (409 `STALE_VERSION`).
    StaleDefinition,
    /// The memorized definition was deleted (404).
    Deleted,
    /// The role can't see this section (403).
    Denied,
    /// The definition is over 256 KB (413).
    DefinitionTooLarge,
}

/// The builder refusal in `e`, if it is one.
#[must_use]
pub fn builder_refusal(e: &ApiError) -> Option<BuilderRefusal> {
    let code_is = |code: Option<&crate::ErrorCode>, c: &str| code.is_some_and(|x| x.as_str() == c);
    Some(match e {
        ApiError::Conflict { code, details, .. } if code_is(code.as_ref(), "RUN_CHANGED") => {
            if details.text("reason") == Some("books") {
                BuilderRefusal::BooksChanged
            } else {
                BuilderRefusal::ParamsChanged
            }
        }
        ApiError::Conflict {
            reason: ConflictReason::Stale,
            ..
        } => BuilderRefusal::StaleDefinition,
        ApiError::Conflict { code, .. } if code_is(code.as_ref(), "STALE_VERSION") => {
            BuilderRefusal::StaleDefinition
        }
        ApiError::Server { status: 503, code } if code_is(code.as_ref(), "REPORT_TIMEOUT") => {
            BuilderRefusal::Timeout
        }
        ApiError::Validation(v) => match v.code().map(crate::ErrorCode::as_str) {
            Some("REPORT_TOO_LARGE") => BuilderRefusal::TooLarge,
            // A cursor the engine can't read: run again, as for new
            // parameters.
            Some("CURSOR_INVALID") => BuilderRefusal::ParamsChanged,
            Some("DEFINITION_INVALID") => BuilderRefusal::Invalid {
                path: v
                    .details()
                    .text("path")
                    .map(cl_core::report::scrub_definition_path),
                code: v.details().text("code").map(str::to_string),
            },
            _ => return None,
        },
        ApiError::NotFound { .. } => BuilderRefusal::Deleted,
        ApiError::Forbidden { .. } => BuilderRefusal::Denied,
        ApiError::PayloadTooLarge => BuilderRefusal::DefinitionTooLarge,
        _ => return None,
    })
}

/// Report Builder runs of a session (`session.report_builder()`).
#[derive(Clone, Copy, Debug)]
pub struct ReportBuilder<'a>(&'a Session);

impl Session {
    /// Report Builder runs of the open company (A07).
    #[must_use]
    pub fn report_builder(&self) -> ReportBuilder<'_> {
        ReportBuilder(self)
    }
}

impl ReportBuilder<'_> {
    fn require(self, cap: &str) -> Result<(), ApiError> {
        if self.0.client().capabilities().has(cap) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// Runs a definition.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `report-builder-v1`; `Local(InvalidValue)`
    /// for a request over the caps (nothing sent); [`builder_refusal`] for
    /// the engine's refusals; [`ApiError::ReportShape`] for an answer that
    /// isn't exact; any [`ApiError`].
    pub async fn run(&self, req: &RunRequest) -> Result<BuilderRun, ApiError> {
        self.require(Capabilities::REPORT_BUILDER)?;
        check_run(req)?;
        let body =
            serde_json::to_vec(req).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let call = Call::at(Method::Post, REPORT_BUILDER_RUN.route()).json_body(body);
        let reply = self.0.send(&REPORT_BUILDER_RUN, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body)
            .map_err(|_| ApiError::Decode { what: "report run" })?;
        read_run(&v, req.reference.adjusting)
    }

    /// One page of a path's detail, read against the run's columns.
    ///
    /// # Errors
    /// As [`ReportBuilder::run`].
    pub async fn detail(
        &self,
        req: &DetailRequest,
        columns: &[ColumnHead],
        layout: &ColumnLayout,
    ) -> Result<DetailPage, ApiError> {
        self.require(Capabilities::REPORT_BUILDER)?;
        check_reference(&req.reference)?;
        if req.path.0.is_empty() || req.path.0.len() > MAX_PATH_SEGMENTS {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let body =
            serde_json::to_vec(req).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let call = Call::at(Method::Post, REPORT_BUILDER_DETAIL.route()).json_body(body);
        let reply = self.0.send(&REPORT_BUILDER_DETAIL, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "report detail",
        })?;
        read_detail(&v, columns, layout)
    }

    /// The engine's field catalogue, as sent (`cl-model` reads it).
    ///
    /// # Errors
    /// As [`ReportBuilder::run`].
    pub async fn fields(&self) -> Result<Value, ApiError> {
        self.require(Capabilities::REPORT_BUILDER)?;
        let call = Call::at(Method::Get, REPORT_BUILDER_FIELDS.route());
        let reply = self.0.send(&REPORT_BUILDER_FIELDS, call, None).await?;
        serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "report fields",
        })
    }

    /// The starters.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `report-builder-starters-v1`; any
    /// [`ApiError`].
    pub async fn starters(&self) -> Result<Vec<Starter>, ApiError> {
        self.require(Capabilities::REPORT_BUILDER_STARTERS)?;
        let call = Call::at(Method::Get, REPORT_BUILDER_STARTERS.route());
        let reply = self.0.send(&REPORT_BUILDER_STARTERS, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body)
            .map_err(|_| ApiError::Decode { what: "starters" })?;
        read_starters(&v)
    }
}

fn check_reference(r: &RunRef) -> Result<(), ApiError> {
    // Without periods a compare set is one range; with them, one per period.
    let compare_ok = r
        .compare_ranges
        .iter()
        .all(|c| c.len() == r.period_ranges.len().max(1));
    if r.period_ranges.len() > MAX_PERIODS
        || r.compare_ranges.len() > 2
        || !compare_ok
        || r.fiscal_year_starts.is_empty()
        || r.fiscal_year_starts.len() > MAX_FISCAL_YEAR_STARTS
        || (r.definition.is_some() == r.definition_id.is_some())
    {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(())
}

/// The request caps the engine also checks: refused before sending.
///
/// # Errors
/// `Local(InvalidValue)`.
pub fn check_run(req: &RunRequest) -> Result<(), ApiError> {
    check_reference(&req.reference)?;
    let paths_ok = |list: &[RowPath]| {
        list.len() <= MAX_OPEN_PATHS
            && list
                .iter()
                .all(|p| !p.0.is_empty() && p.0.len() <= MAX_PATH_SEGMENTS)
    };
    if !paths_ok(&req.open) || !paths_ok(&req.collapsed) {
        return Err(ApiError::Local(LocalValidation::InvalidValue));
    }
    Ok(())
}

// ------------------------------------------------------------ the adapter

const fn shape(what: &'static str) -> ApiError {
    ApiError::ReportShape { what }
}

/// An exact decimal, never rounded.
fn exact(s: &str, what: &'static str) -> Result<Decimal, ApiError> {
    Decimal::from_str_exact(s).map_err(|_| shape(what))
}

fn date(v: Option<&Value>, what: &'static str) -> Result<CalendarDate, ApiError> {
    v.and_then(Value::as_str)
        .and_then(|s| CalendarDate::parse_wire(s).ok())
        .ok_or(shape(what))
}

/// A column's `range`: `{from, to}`, or `{from: null, to}` for an as-of
/// column, read as from [`CalendarDate::min`] (a balance from the
/// beginning) and sent back as `from: null` (U-119e: `/run/detail` matches
/// an as-of column on `to`).
fn column_range(v: Option<&Value>) -> Result<Option<DateRange>, ApiError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(r @ Value::Object(_)) => {
            let to = date(r.get("to"), "range")?;
            let from = match r.get("from") {
                None | Some(Value::Null) => CalendarDate::min(),
                from => date(from, "range")?,
            };
            Ok(Some(DateRange { from, to }))
        }
        Some(_) => Err(shape("range")),
    }
}

/// A detail request's `range`: the column's own, `from: null` for an as-of
/// column.
#[allow(
    clippy::ref_option,
    reason = "serde's serialize_with hands the field by reference"
)]
fn wire_range<S: serde::Serializer>(r: &Option<DateRange>, s: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    struct Wire {
        from: Option<CalendarDate>,
        to: CalendarDate,
    }
    match r {
        Some(r) => Wire {
            from: (r.from != CalendarDate::min()).then_some(r.from),
            to: r.to,
        }
        .serialize(s),
        None => s.serialize_none(),
    }
}

/// What a detail field's cells hold.
fn field_kind(field: &str) -> Result<ColumnKind, ApiError> {
    Ok(match field {
        "date" | "due_date" | "service_date" => ColumnKind::Date,
        "debit" | "credit" | "amount" | "running_balance" | "open_balance" | "original_amount"
        | "unit_price" => ColumnKind::Amount,
        "quantity" => ColumnKind::Quantity,
        "days_past_due" => ColumnKind::Count,
        "txn_type" | "number" | "contact" | "memo" | "description" | "account" | "item" | "tag"
        | "cleared" | "aging_bucket" => ColumnKind::Text,
        _ => return Err(shape("detail field")),
    })
}

/// One wire column: `None` for a `hidden` one.
fn column(c: &Value) -> Result<Option<ColumnHead>, ApiError> {
    let mut range = column_range(c.get("range"))?;
    let (kind, drill) = match c.get("kind").and_then(Value::as_str).unwrap_or("") {
        "hidden" => return Ok(None),
        "field" => (
            field_kind(c.get("field").and_then(Value::as_str).unwrap_or(""))?,
            None,
        ),
        // The total's lines are every base column's: sent with no range
        // (aging columns refuse any range, U-119e).
        "total" | "total_debit" | "total_credit" => {
            range = None;
            (ColumnKind::Amount, None)
        }
        // A period column drills to its own lines (U-119e), as-of ones too.
        "period" | "period_debit" | "period_credit" if range.is_some() => {
            (ColumnKind::Amount, None)
        }
        // Compare and change cells don't drill (plan §4), nor do aging
        // buckets (the detail can't narrow to a bucket).
        "period" | "period_debit" | "period_credit" | "prior" | "prior_debit" | "prior_credit"
        | "change" | "bucket" => (ColumnKind::Amount, Some(ColumnDrill::None)),
        "pct_change" | "percent" => (ColumnKind::Percent, Some(ColumnDrill::None)),
        _ => return Err(shape("column kind")),
    };
    Ok(Some(ColumnHead {
        key: c
            .get("key")
            .and_then(Value::as_str)
            .ok_or(shape("column key"))?
            .to_string(),
        // Owner or engine text, never a key (S-L2).
        caption: literal_caption(c.get("caption").and_then(Value::as_str).unwrap_or_default()),
        kind,
        range,
        drill,
    }))
}

fn columns(v: &Value) -> Result<(Vec<ColumnHead>, ColumnLayout), ApiError> {
    let list = v
        .get("columns")
        .and_then(Value::as_array)
        .ok_or(shape("columns"))?;
    let mut heads = Vec::with_capacity(list.len());
    let mut shown = Vec::with_capacity(list.len());
    for (i, c) in list.iter().enumerate() {
        if let Some(h) = column(c)? {
            heads.push(h);
            shown.push(i);
        }
    }
    Ok((
        heads,
        ColumnLayout {
            wire: list.len(),
            shown,
        },
    ))
}

fn row_kind(s: &str) -> Result<RowKind, ApiError> {
    Ok(match s {
        "header" | "group" => RowKind::Header,
        "line" => RowKind::Group,
        "detail" => RowKind::Detail,
        "subtotal" => RowKind::Subtotal,
        "total" => RowKind::Total,
        "formula" => RowKind::Formula,
        "text" => RowKind::Text,
        "blank" => RowKind::Blank,
        "page_break" => RowKind::PageBreak,
        "more" => RowKind::More,
        "denied" => RowKind::Denied,
        "earnings" | "opening" => RowKind::Earnings,
        _ => return Err(shape("row kind")),
    })
}

fn cell(col: &ColumnHead, raw: &Value) -> Result<Cell, ApiError> {
    let s = match raw {
        Value::Null => {
            return Ok(match col.kind {
                ColumnKind::Percent => Cell::Percent(None),
                _ => Cell::Empty,
            });
        }
        Value::String(s) => s.as_str(),
        // A number is never read as money (U-14).
        _ => return Err(shape("cell")),
    };
    Ok(match col.kind {
        ColumnKind::Amount => Cell::Amount(exact(s, "amount cell")?),
        ColumnKind::Percent => Cell::Percent(Some(exact(s, "percent cell")?)),
        ColumnKind::Quantity => Cell::Quantity(exact(s, "quantity cell")?),
        ColumnKind::Count => Cell::Count(u64::from_str(s).map_err(|_| shape("count cell"))?),
        ColumnKind::Date => {
            Cell::Date(CalendarDate::parse_wire(s).map_err(|_| shape("date cell"))?)
        }
        // A memo or name, never a key (S-L2).
        ColumnKind::Text => Cell::Text(literal_caption(s)),
    })
}

fn path_in(v: Option<&Value>, what: &'static str) -> Result<RowPath, ApiError> {
    let parts = v.and_then(Value::as_array).ok_or(shape(what))?;
    if parts.is_empty() {
        return Err(shape(what));
    }
    parts
        .iter()
        .map(|p| p.as_str().map(str::to_string).ok_or(shape(what)))
        .collect::<Result<Vec<_>, _>>()
        .map(RowPath)
}

/// A `profit_loss` drill's first day: the engine starts prior years'
/// earnings at `0001-01-01`, before the first day the app keeps; that
/// means "from the beginning".
fn earliest(v: Option<&Value>) -> Result<CalendarDate, ApiError> {
    match v.and_then(Value::as_str) {
        Some(s) if s.len() == 10 && s < "1900-01-01" => Ok(CalendarDate::min()),
        _ => date(v, "drill range"),
    }
}

/// The run's basis and lens, which an earnings drill keeps.
#[derive(Clone, Copy)]
struct Lens {
    basis: Option<Basis>,
    adjusting: Option<Adjusting>,
}

/// A row's drill: `{type:'transaction', txnId, txnType}`, `{type:'detail',
/// nodeId, path}`, `{type:'profit_loss', from, to}`, or nothing. A `more`
/// row pages with its `cursor` (`null`: from the start).
fn drill(
    r: &Value,
    kind: RowKind,
    node_id: &str,
    path: &RowPath,
    lens: Lens,
) -> Result<Option<Drill>, ApiError> {
    if kind == RowKind::More {
        let cursor = match r.get("cursor") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => return Err(shape("cursor")),
        };
        return Ok(Some(Drill::BuilderPage {
            node_id: node_id.to_string(),
            path: path.clone(),
            cursor,
        }));
    }
    let Some(d) = r.get("drill").filter(|d| !d.is_null()) else {
        return Ok(None);
    };
    Ok(Some(match d.get("type").and_then(Value::as_str) {
        Some("transaction") => Drill::Transaction {
            id: d
                .get("txnId")
                .and_then(Value::as_str)
                .and_then(|s| TransactionId::parse(s).ok())
                .ok_or(shape("drill transaction"))?,
            txn_type: d
                .get("txnType")
                .and_then(Value::as_str)
                .ok_or(shape("drill type"))?
                .to_string(),
        },
        Some("detail") => Drill::BuilderDetail {
            node_id: d
                .get("nodeId")
                .and_then(Value::as_str)
                .ok_or(shape("drill node"))?
                .to_string(),
            path: path_in(d.get("path"), "drill path")?,
            range: None,
        },
        // The run's lens travels with it. A tag-filtered Balance Sheet
        // section's earnings drill carries no tags (Q-A07s).
        Some("profit_loss") => Drill::ProfitLoss {
            from: earliest(d.get("from"))?,
            to: date(d.get("to"), "drill range")?,
            basis: lens.basis,
            adjusting: lens.adjusting,
            tag_ids: Vec::new(),
        },
        _ => return Err(shape("drill kind")),
    }))
}

/// The row's caption: the fixed ones as i18n keys (the only `@` captions
/// this adapter makes); any other, owner or engine text, marked literal so
/// a caption typed as `@reports.builder.denied` shows as typed (S-L2).
fn caption_of(kind: RowKind, wire_kind: &str, caption: Option<&str>) -> String {
    match (kind, wire_kind) {
        (RowKind::Denied, _) => "@reports.builder.denied".into(),
        (RowKind::More, _) => "@reports.builder.more".into(),
        (_, "opening") => "@reports.builder.opening".into(),
        _ => literal_caption(caption.unwrap_or_default()),
    }
}

fn row(
    r: &Value,
    cols: &[ColumnHead],
    layout: &ColumnLayout,
    lens: Lens,
) -> Result<ReportRow, ApiError> {
    let wire_kind = r.get("kind").and_then(Value::as_str).unwrap_or("");
    let kind = row_kind(wire_kind)?;
    let node_id = r
        .get("nodeId")
        .and_then(Value::as_str)
        .ok_or(shape("node id"))?
        .to_string();
    // Every row carries its path (U-119e gave open-item detail rows theirs).
    let path = path_in(r.get("path"), "path")?;
    let level = r
        .get("level")
        .and_then(Value::as_u64)
        .and_then(|l| u8::try_from(l).ok())
        .ok_or(shape("level"))?;
    // One cell per wire column, every row.
    let raw = r
        .get("cells")
        .and_then(Value::as_array)
        .ok_or(shape("cells"))?;
    if raw.len() != layout.wire {
        return Err(shape("cells"));
    }
    let cells = cols
        .iter()
        .zip(&layout.shown)
        .map(|(c, &i)| cell(c, &raw[i]))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ReportRow {
        drill: drill(r, kind, &node_id, &path, lens)?,
        caption: caption_of(kind, wire_kind, r.get("caption").and_then(Value::as_str)),
        kind,
        level,
        path,
        cells,
        expandable: r.get("expandable") == Some(&Value::Bool(true)),
    })
}

/// The engine's notes the window words itself.
fn note_key(note: &str) -> String {
    match note {
        "No activity for these dates" => "@reports.builder.no_activity".into(),
        other => literal_caption(other),
    }
}

fn strings(v: Option<&Value>, what: &'static str) -> Result<Vec<String>, ApiError> {
    v.and_then(Value::as_array)
        .ok_or(shape(what))?
        .iter()
        .map(|n| n.as_str().map(str::to_string).ok_or(shape(what)))
        .collect()
}

fn meta(v: &Value) -> Result<(ReportMeta, Vec<String>, bool), ApiError> {
    let m = v.get("meta").ok_or(shape("meta"))?;
    let basis = match m.get("basis").and_then(Value::as_str) {
        Some("cash") => Some(Basis::Cash),
        Some("accrual") => Some(Basis::Accrual),
        _ => return Err(shape("meta basis")),
    };
    let dates = match m.get("dates") {
        Some(d) if d.get("asOf").is_some() => {
            Some(ReportDates::AsOf(date(d.get("asOf"), "meta dates")?))
        }
        Some(d @ Value::Object(_)) => Some(ReportDates::Range(DateRange {
            from: date(d.get("from"), "meta dates")?,
            to: date(d.get("to"), "meta dates")?,
        })),
        _ => None,
    };
    let notes: Vec<String> = strings(m.get("notes"), "meta notes")?
        .iter()
        .map(|n| note_key(n))
        .collect();
    let truncated = m
        .get("truncated")
        .and_then(Value::as_bool)
        .ok_or(shape("meta truncated"))?;
    let denied = strings(m.get("denied"), "meta denied")?;
    Ok((
        ReportMeta {
            basis,
            dates,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            note_drill: None,
            // A capped path has its own `more` row; the counts aren't sent.
            truncated: None,
            hidden_notice: m.get("hiddenNotice") == Some(&Value::Bool(true)),
        },
        denied,
        truncated,
    ))
}

/// Reads a `/run` answer; `adjusting` is the run's lens (earnings drills
/// keep it).
///
/// # Errors
/// [`ApiError::ReportShape`] for anything not in the contract: an unknown
/// row or column kind, a non-exact amount, a cell count that doesn't match.
pub fn read_run(v: &Value, adjusting: Option<Adjusting>) -> Result<BuilderRun, ApiError> {
    let (cols, layout) = columns(v)?;
    let (meta, denied, truncated) = meta(v)?;
    let raw = v
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(shape("rows"))?;
    let lens = Lens {
        basis: meta.basis,
        adjusting,
    };
    let mut rows = raw
        .iter()
        .map(|r| row(r, &cols, &layout, lens))
        .collect::<Result<Vec<_>, _>>()?;
    // A section the role can't see shows nothing, whatever was sent
    // (S-L5): its denied row is blanked here, and any other row of it with
    // a figure is off contract.
    for (r, x) in raw.iter().zip(rows.iter_mut()) {
        let of_denied = r
            .get("nodeId")
            .and_then(Value::as_str)
            .is_some_and(|n| denied.iter().any(|d| d == n));
        if x.kind == RowKind::Denied {
            for c in &mut x.cells {
                *c = match c {
                    Cell::Percent(_) => Cell::Percent(None),
                    _ => Cell::Empty,
                };
            }
            x.drill = None;
        } else if of_denied
            && x.cells
                .iter()
                .any(|c| !matches!(c, Cell::Empty | Cell::Percent(None)))
        {
            return Err(shape("denied section"));
        }
    }
    // Only a row that can close is open (#11): a detail or summary line
    // the engine marks `expanded` would make Collapse all the only choice.
    let expanded = raw
        .iter()
        .zip(&rows)
        .filter(|(r, x)| x.expandable && r.get("expanded") == Some(&Value::Bool(true)))
        .map(|(_, x)| x.path.clone())
        .collect();
    let fingerprint = v
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or(shape("fingerprint"))?
        .to_string();
    let stamp = v
        .get("stamp")
        .and_then(Value::as_u64)
        .ok_or(shape("stamp"))?;
    Ok(BuilderRun {
        table: ReportTable {
            columns: cols,
            rows,
            meta,
        },
        layout,
        fingerprint,
        stamp,
        denied,
        expanded,
        truncated,
    })
}

/// Reads a `/run/detail` answer (`{rows, cursor, subtotal}`): its cells
/// line up with the run's wire columns (`layout`).
///
/// # Errors
/// As [`read_run`].
pub fn read_detail(
    v: &Value,
    cols: &[ColumnHead],
    layout: &ColumnLayout,
) -> Result<DetailPage, ApiError> {
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(shape("rows"))?;
    if rows.len() > PAGE_ROWS {
        return Err(shape("page size"));
    }
    let lens = Lens {
        basis: None,
        adjusting: None,
    };
    let rows = rows
        .iter()
        .map(|r| row(r, cols, layout, lens))
        .collect::<Result<Vec<_>, ApiError>>()?;
    let subtotal_raw = v
        .get("subtotal")
        .and_then(Value::as_array)
        .ok_or(shape("subtotal"))?;
    if subtotal_raw.len() != layout.wire {
        return Err(shape("subtotal"));
    }
    let subtotal = cols
        .iter()
        .zip(&layout.shown)
        .map(|(c, &i)| cell(c, &subtotal_raw[i]))
        .collect::<Result<Vec<_>, _>>()?;
    let cursor = match v.get("cursor") {
        Some(Value::String(s)) => Some(s.clone()),
        None | Some(Value::Null) => None,
        _ => return Err(shape("cursor")),
    };
    Ok(DetailPage {
        rows,
        cursor,
        subtotal,
    })
}

fn read_starters(v: &Value) -> Result<Vec<Starter>, ApiError> {
    let list = match v {
        Value::Array(a) => a,
        Value::Object(m) => m
            .get("data")
            .or_else(|| m.get("starters"))
            .and_then(Value::as_array)
            .ok_or(ApiError::Decode { what: "starters" })?,
        _ => return Err(ApiError::Decode { what: "starters" }),
    };
    list.iter()
        .map(|s| {
            let text = |k: &str| s.get(k).and_then(Value::as_str).map(str::to_string);
            Ok(Starter {
                key: text("key").ok_or(ApiError::Decode {
                    what: "starter key",
                })?,
                title: text("title").unwrap_or_default(),
                ties_to: text("tiesTo"),
                definition: s
                    .get("definition")
                    .filter(|d| d.is_object())
                    .cloned()
                    .ok_or(ApiError::Decode {
                        what: "starter definition",
                    })?,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "report_builder_tests.rs"]
mod tests;
