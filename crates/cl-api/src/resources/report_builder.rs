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
//! Contract guesses (A07-desktop-status.md lists them): until PR-C's
//! samples are pinned, the wire shapes of `drill`, `fields`, `meta.notes`,
//! `meta.denied`, `meta.truncated`, a "—" cell and the `opening` row kind
//! follow the reading below; one function per shape so the samples replace
//! them in one place.

use std::str::FromStr;

use cl_core::CalendarDate;
use cl_core::ids::TransactionId;
use cl_core::report::{
    Adjusting, Basis, Cell, ColumnDrill, ColumnHead, ColumnKind, DateRange, Drill, ReportDates,
    ReportMeta, ReportRow, ReportTable, RowKind, RowPath, Truncated,
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
    /// Basis (`None`: the definition's, else the company's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basis: Option<Basis>,
    /// Adjusting lens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adjusting: Option<Adjusting>,
}

/// `POST /report-builder/run`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// Definition, dates and ranges.
    #[serde(flatten)]
    pub reference: RunRef,
    /// Open paths: `None` (`null`) on the first run uses the sections'
    /// `defaultExpanded`; afterwards the window's set (`[]`: all closed).
    pub open: Option<Vec<RowPath>>,
    /// Expand everything (bounded by the caps).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub open_all: bool,
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
    /// The engine path.
    pub path: RowPath,
    /// A period column's dates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<DateRange>,
    /// The page after this cursor (`None`: the first page).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// The run's fingerprint (409 `RUN_CHANGED {reason:'params'}`).
    pub fingerprint: String,
    /// The run's stamp (409 `RUN_CHANGED {reason:'books'}`).
    pub stamp: String,
}

/// A run's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuilderRun {
    /// The rows as a table.
    pub table: ReportTable,
    /// Hash of every parameter but the open paths and purpose.
    pub fingerprint: String,
    /// The ledger stamp the run read.
    pub stamp: String,
    /// Sections the role can't see (their node ids).
    pub denied: Vec<String>,
    /// The paths the engine expanded (`expanded: true`): the sections'
    /// defaults on a first run, everything on an `openAll` run. The window
    /// seeds its open set from them.
    pub expanded: Vec<RowPath>,
}

/// One page of detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetailPage {
    /// Up to 500 rows.
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
    /// The definition breaks a rule (400 `DEFINITION_INVALID {path, code}`).
    Invalid {
        /// Where (when sent).
        path: Option<String>,
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
            Some("DEFINITION_INVALID") => BuilderRefusal::Invalid {
                path: v
                    .issues()
                    .first()
                    .and_then(|i| i.path.as_ref())
                    .map(|p| p.as_str().to_string()),
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
        read_run(&v)
    }

    /// One page of a path's detail, read against the run's `columns`.
    ///
    /// # Errors
    /// As [`ReportBuilder::run`].
    pub async fn detail(
        &self,
        req: &DetailRequest,
        columns: &[ColumnHead],
    ) -> Result<DetailPage, ApiError> {
        self.require(Capabilities::REPORT_BUILDER)?;
        let body =
            serde_json::to_vec(req).map_err(|_| ApiError::Local(LocalValidation::InvalidValue))?;
        let call = Call::at(Method::Post, REPORT_BUILDER_DETAIL.route()).json_body(body);
        let reply = self.0.send(&REPORT_BUILDER_DETAIL, call, None).await?;
        let v: Value = serde_json::from_slice(&reply.body).map_err(|_| ApiError::Decode {
            what: "report detail",
        })?;
        read_detail(&v, &req.node_id, columns)
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

/// The request caps the engine also checks: refused before sending.
///
/// # Errors
/// `Local(InvalidValue)`.
pub fn check_run(req: &RunRequest) -> Result<(), ApiError> {
    let bad = || ApiError::Local(LocalValidation::InvalidValue);
    let r = &req.reference;
    if r.period_ranges.len() > MAX_PERIODS
        || r.compare_ranges.len() > 2
        || r.compare_ranges
            .iter()
            .any(|c| c.len() != r.period_ranges.len())
        || (r.definition.is_some() == r.definition_id.is_some())
    {
        return Err(bad());
    }
    if let Some(open) = &req.open
        && (open.len() > MAX_OPEN_PATHS || open.iter().any(|p| p.0.len() > MAX_PATH_SEGMENTS))
    {
        return Err(bad());
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

fn range_of(v: &Value) -> Result<Option<DateRange>, ApiError> {
    match v {
        Value::Null => Ok(None),
        Value::Object(_) => {
            let d = |k: &'static str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .and_then(|s| CalendarDate::parse_wire(s).ok())
                    .ok_or(shape("range"))
            };
            Ok(Some(DateRange {
                from: d("from")?,
                to: d("to")?,
            }))
        }
        _ => Err(shape("range")),
    }
}

fn column_kind(s: &str) -> Result<ColumnKind, ApiError> {
    Ok(match s {
        "amount" => ColumnKind::Amount,
        "percent" => ColumnKind::Percent,
        "quantity" => ColumnKind::Quantity,
        "count" => ColumnKind::Count,
        "text" => ColumnKind::Text,
        "date" => ColumnKind::Date,
        _ => return Err(shape("column kind")),
    })
}

fn columns(v: &Value) -> Result<Vec<ColumnHead>, ApiError> {
    let list = v
        .get("columns")
        .and_then(Value::as_array)
        .ok_or(shape("columns"))?;
    list.iter()
        .map(|c| {
            let kind = column_kind(c.get("kind").and_then(Value::as_str).unwrap_or(""))?;
            // Percent, compare and change columns have no lines behind
            // their cells (plan §4 drill rules).
            let no_drill =
                kind == ColumnKind::Percent || c.get("drillable") == Some(&Value::Bool(false));
            Ok(ColumnHead {
                key: c
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or(shape("column key"))?
                    .to_string(),
                caption: c
                    .get("caption")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                kind,
                range: range_of(c.get("range").unwrap_or(&Value::Null))?,
                drill: no_drill.then_some(ColumnDrill::None),
            })
        })
        .collect()
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

/// The "—" a cell shows when it depends on a section the role can't see.
const DASH: &str = "\u{2014}";

fn cell(col: &ColumnHead, raw: &Value, field: Option<&Value>) -> Result<Cell, ApiError> {
    let v = match (raw, field) {
        (Value::Null, Some(f)) => f,
        _ => raw,
    };
    let s = match v {
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
    if s == DASH && col.kind.is_numeric() {
        return Ok(Cell::Text(DASH.into()));
    }
    Ok(match col.kind {
        ColumnKind::Amount => Cell::Amount(exact(s, "amount cell")?),
        ColumnKind::Percent => Cell::Percent(Some(exact(s, "percent cell")?)),
        ColumnKind::Quantity => Cell::Quantity(exact(s, "quantity cell")?),
        ColumnKind::Count => Cell::Count(u64::from_str(s).map_err(|_| shape("count cell"))?),
        ColumnKind::Date => {
            Cell::Date(CalendarDate::parse_wire(s).map_err(|_| shape("date cell"))?)
        }
        ColumnKind::Text => Cell::Text(s.to_string()),
    })
}

fn path_of(v: &Value) -> Result<RowPath, ApiError> {
    let parts = v
        .get("path")
        .and_then(Value::as_array)
        .ok_or(shape("path"))?;
    parts
        .iter()
        .map(|p| p.as_str().map(str::to_string).ok_or(shape("path")))
        .collect::<Result<Vec<_>, _>>()
        .map(RowPath)
}

/// A row's drill (contract guess): `{kind:'transaction', transactionId,
/// txnType}`, `{kind:'detail'}` (the row's own node and path),
/// `{kind:'profit_loss', from, to}`, or nothing. A `more` row pages with
/// its `cursor`.
fn drill(
    r: &Value,
    kind: RowKind,
    node_id: &str,
    path: &RowPath,
    meta_basis: Option<Basis>,
) -> Result<Option<Drill>, ApiError> {
    if kind == RowKind::More {
        let cursor = r
            .get("cursor")
            .and_then(Value::as_str)
            .ok_or(shape("cursor"))?;
        return Ok(Some(Drill::BuilderPage {
            node_id: node_id.to_string(),
            path: path.clone(),
            cursor: cursor.to_string(),
        }));
    }
    let Some(d) = r.get("drill").filter(|d| !d.is_null()) else {
        return Ok(None);
    };
    Ok(Some(match d.get("kind").and_then(Value::as_str) {
        Some("transaction") => Drill::Transaction {
            id: d
                .get("transactionId")
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
            node_id: node_id.to_string(),
            path: path.clone(),
            range: None,
        },
        Some("profit_loss") => {
            let range = range_of(d)?.ok_or(shape("drill range"))?;
            // The section's adjusting lens and tags, when sent, so the
            // drill ties on a tagged or adjusting Balance Sheet.
            let adjusting = match d.get("adjusting").and_then(Value::as_str) {
                None => None,
                Some("include") => Some(Adjusting::Include),
                Some("exclude") => Some(Adjusting::Exclude),
                Some("only") => Some(Adjusting::Only),
                Some(_) => return Err(shape("drill adjusting")),
            };
            let tag_ids = match d.get("tagIds") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(a)) => a
                    .iter()
                    .map(|t| {
                        t.as_str()
                            .and_then(|s| cl_core::ids::TagId::parse(s).ok())
                            .ok_or(shape("drill tag"))
                    })
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(shape("drill tag")),
            };
            Drill::ProfitLoss {
                from: range.from,
                to: range.to,
                basis: meta_basis,
                adjusting,
                tag_ids,
            }
        }
        _ => return Err(shape("drill kind")),
    }))
}

fn row(r: &Value, cols: &[ColumnHead], basis: Option<Basis>) -> Result<ReportRow, ApiError> {
    let kind = row_kind(r.get("kind").and_then(Value::as_str).unwrap_or(""))?;
    let node_id = r
        .get("nodeId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let path = path_of(r)?;
    let level = r
        .get("level")
        .and_then(Value::as_u64)
        .and_then(|l| u8::try_from(l).ok())
        .ok_or(shape("level"))?;
    let raw = match r.get("cells") {
        Some(Value::Array(a)) => a.as_slice(),
        None | Some(Value::Null) => &[],
        _ => return Err(shape("cells")),
    };
    // Layout rows carry no cells; every other row carries one per column.
    if !raw.is_empty() && raw.len() != cols.len() {
        return Err(shape("cells"));
    }
    let fields = r.get("fields").and_then(Value::as_object);
    let cells = cols
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let field = fields.and_then(|f| f.get(&c.key));
            cell(c, raw.get(i).unwrap_or(&Value::Null), field)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let caption = match (kind, r.get("caption").and_then(Value::as_str)) {
        (_, Some(c)) if !c.is_empty() => c.to_string(),
        (RowKind::Denied, _) => "@reports.builder.denied".into(),
        (RowKind::More, _) => "@reports.builder.more".into(),
        _ => String::new(),
    };
    Ok(ReportRow {
        drill: drill(r, kind, &node_id, &path, basis)?,
        kind,
        level,
        path,
        caption,
        cells,
        expandable: r.get("expandable") == Some(&Value::Bool(true)),
    })
}

fn meta(v: &Value) -> Result<(ReportMeta, Vec<String>), ApiError> {
    let m = v.get("meta").ok_or(shape("meta"))?;
    let basis = match m.get("basis").and_then(Value::as_str) {
        Some("cash") => Some(Basis::Cash),
        Some("accrual") => Some(Basis::Accrual),
        _ => None,
    };
    let dates = match m.get("dates") {
        Some(d) if d.get("asOf").is_some() => Some(ReportDates::AsOf(
            d.get("asOf")
                .and_then(Value::as_str)
                .and_then(|s| CalendarDate::parse_wire(s).ok())
                .ok_or(shape("meta dates"))?,
        )),
        Some(d) if !d.is_null() => range_of(d)?.map(ReportDates::Range),
        _ => None,
    };
    let notes: Vec<&str> = m
        .get("notes")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let truncated = match m.get("truncated") {
        Some(t) if t.is_object() => Some(Truncated {
            shown: t.get("shown").and_then(Value::as_u64).unwrap_or(0),
            total: t.get("total").and_then(Value::as_u64).unwrap_or(0),
        }),
        _ => None,
    };
    let denied = m
        .get("denied")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok((
        ReportMeta {
            basis,
            dates,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
            note_drill: None,
            truncated,
            hidden_notice: m.get("hiddenNotice") == Some(&Value::Bool(true)),
        },
        denied,
    ))
}

/// Reads a `/run` answer.
///
/// # Errors
/// [`ApiError::ReportShape`] for anything not in the contract: an unknown
/// row kind, a non-exact amount, a cell count that doesn't match.
pub fn read_run(v: &Value) -> Result<BuilderRun, ApiError> {
    let cols = columns(v)?;
    let (meta, denied) = meta(v)?;
    let raw = v
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(shape("rows"))?;
    let rows = raw
        .iter()
        .map(|r| row(r, &cols, meta.basis))
        .collect::<Result<Vec<_>, _>>()?;
    let expanded = raw
        .iter()
        .zip(&rows)
        .filter(|(r, _)| r.get("expanded") == Some(&Value::Bool(true)))
        .map(|(_, x)| x.path.clone())
        .collect();
    let text = |k: &'static str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or(shape(k))
    };
    Ok(BuilderRun {
        fingerprint: text("fingerprint")?,
        stamp: text("stamp")?,
        table: ReportTable {
            columns: cols,
            rows,
            meta,
        },
        denied,
        expanded,
    })
}

/// Reads a `/run/detail` answer (`{rows, cursor, subtotal}`, plan §4) for
/// `node_id`; its cells line up with the run's `cols`.
///
/// # Errors
/// As [`read_run`].
pub fn read_detail(v: &Value, node_id: &str, cols: &[ColumnHead]) -> Result<DetailPage, ApiError> {
    let rows = v
        .get("rows")
        .and_then(Value::as_array)
        .ok_or(shape("rows"))?;
    if rows.len() > PAGE_ROWS {
        return Err(shape("page size"));
    }
    let rows = rows
        .iter()
        .map(|r| {
            let mut row = row(r, cols, None)?;
            // A detail page's `more`-less rows belong to the asked node.
            if let Some(Drill::BuilderPage { node_id: n, .. }) = &mut row.drill
                && n.is_empty()
            {
                node_id.clone_into(n);
            }
            Ok(row)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    let subtotal_raw = v
        .get("subtotal")
        .and_then(Value::as_array)
        .ok_or(shape("subtotal"))?;
    if subtotal_raw.len() != cols.len() {
        return Err(shape("subtotal"));
    }
    let subtotal = cols
        .iter()
        .zip(subtotal_raw)
        .map(|(c, s)| cell(c, s, None))
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
