//! The catch-up run (P15 lane T, U-63, `recurring-run-due-v1`):
//! `POST /recurring/run-due {asOf, runId, limit}` and the side-effect-free
//! `GET /recurring/attention`.
//!
//! The engine decides which dates are due and posts each occurrence exactly
//! once under its `(template, date)` key; the app sends a date and a
//! `runId`, never amounts. A lost reply is recovered by re-asking with the
//! same `runId` ([`Recurring::run_due`] keeps it in the request). Amounts
//! in the answer are display-only (strict `Money::parse_wire`), never
//! summed. The body never carries a closing-date password (the engine
//! refuses one): auto entries never post into a closed period.

use std::fmt;

use cl_core::ids::{RunId, TemplateId, TransactionId};
use cl_core::{CalendarDate, Money};
use serde_json::{Value, json};

use crate::client::{Call, Method};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::IdempotencyKey;
use crate::resources::operations::{RECURRING_ATTENTION, RECURRING_RUN_DUE};
use crate::resources::templates::TemplateKind;
use crate::session::{Session, json_body};
use crate::wire::Capabilities;

/// The most occurrences one run handles.
pub const RUN_LIMIT_MAX: u32 = 500;
/// The engine's default.
pub const RUN_LIMIT_DEFAULT: u32 = 200;

/// Why an occurrence could not be entered (`skipped[].reason`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// On or before the closing date.
    ClosedPeriod,
    /// An account is inactive or deleted.
    InvalidAccount,
    /// A name is inactive or deleted.
    InvalidContact,
    /// A tag is inactive or deleted.
    InvalidTag,
    /// The check number was taken.
    CheckNumber,
    /// The engine refused the draft.
    Validation,
    /// The posted transaction it was made from is gone.
    SourceMissing,
    /// A post was interrupted; check the register before entering it.
    NeedsCheck,
    /// The date was already handled.
    AlreadyPosted,
    /// The person who last saved the template lost permission to enter it
    /// automatically (recorded and advanced like any skip).
    Permission,
    /// More than 31 dates were due in one call: nothing was recorded for
    /// this template, the rest stays due and the next call continues (never
    /// stored; not skipped or dismissed by the owner).
    CatchUpCapped,
    /// A reason this version does not know.
    Other(String),
}

impl SkipReason {
    /// From the wire code.
    #[must_use]
    pub fn from_wire(s: &str) -> SkipReason {
        match s {
            "CLOSED_PERIOD" => SkipReason::ClosedPeriod,
            "INVALID_ACCOUNT" => SkipReason::InvalidAccount,
            "INVALID_CONTACT" => SkipReason::InvalidContact,
            "INVALID_TAG" => SkipReason::InvalidTag,
            "CHECK_NUMBER" => SkipReason::CheckNumber,
            "VALIDATION" => SkipReason::Validation,
            "TEMPLATE_SOURCE_MISSING" => SkipReason::SourceMissing,
            "NEEDS_CHECK" => SkipReason::NeedsCheck,
            "ALREADY_POSTED" => SkipReason::AlreadyPosted,
            "PERMISSION" => SkipReason::Permission,
            "CATCH_UP_CAPPED" => SkipReason::CatchUpCapped,
            other => SkipReason::Other(other.to_string()),
        }
    }

    /// The wire code.
    #[must_use]
    pub fn as_wire(&self) -> &str {
        match self {
            SkipReason::ClosedPeriod => "CLOSED_PERIOD",
            SkipReason::InvalidAccount => "INVALID_ACCOUNT",
            SkipReason::InvalidContact => "INVALID_CONTACT",
            SkipReason::InvalidTag => "INVALID_TAG",
            SkipReason::CheckNumber => "CHECK_NUMBER",
            SkipReason::Validation => "VALIDATION",
            SkipReason::SourceMissing => "TEMPLATE_SOURCE_MISSING",
            SkipReason::NeedsCheck => "NEEDS_CHECK",
            SkipReason::AlreadyPosted => "ALREADY_POSTED",
            SkipReason::Permission => "PERMISSION",
            SkipReason::CatchUpCapped => "CATCH_UP_CAPPED",
            SkipReason::Other(s) => s,
        }
    }

    /// A capped catch-up, not a stored skip: it can't be skipped or
    /// dismissed, and the run asks again (same run id) to continue.
    #[must_use]
    pub fn is_capped(&self) -> bool {
        *self == SkipReason::CatchUpCapped
    }
}

/// One occurrence the run posted.
#[derive(Clone, PartialEq, Eq)]
pub struct PostedOccurrence {
    /// The template.
    pub template: TemplateId,
    /// Its name (owner data: never logged).
    pub name: String,
    /// Its kind.
    pub kind: TemplateKind,
    /// The form it opens in.
    pub client_form: Option<String>,
    /// The date it posted on.
    pub date: CalendarDate,
    /// The new transaction.
    pub transaction: TransactionId,
    /// Its total as the engine reports it (display only).
    pub total: Option<Money>,
}

/// One due reminder (never posted or advanced by the run).
#[derive(Clone, PartialEq, Eq)]
pub struct DueReminder {
    /// The template.
    pub template: TemplateId,
    /// Its name.
    pub name: String,
    /// Its kind.
    pub kind: TemplateKind,
    /// The form it opens in.
    pub client_form: Option<String>,
    /// The due dates (at most 24).
    pub dates: Vec<CalendarDate>,
    /// How many are due in all.
    pub count: u32,
    /// Its total (display only).
    pub total: Option<Money>,
}

/// One occurrence that could not be entered and is still open.
#[derive(Clone, PartialEq, Eq)]
pub struct SkippedOccurrence {
    /// The template.
    pub template: TemplateId,
    /// Its name.
    pub name: String,
    /// The date.
    pub date: CalendarDate,
    /// Why.
    pub reason: SkipReason,
    /// Skipped by this run (not an older open one).
    pub new: bool,
}

/// A template the run could not check (retried by the next run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedTemplate {
    /// The template.
    pub template: TemplateId,
    /// The engine's code.
    pub reason: String,
}

macro_rules! ids_only {
    ($t:ty, $($f:ident),+) => {
        /// Ids and dates only (names and amounts are owner data).
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($t))
                    $(.field(stringify!($f), &self.$f))+
                    .finish_non_exhaustive()
            }
        }
    };
}
ids_only!(PostedOccurrence, template, date, transaction);
ids_only!(DueReminder, template, dates, count);
ids_only!(SkippedOccurrence, template, date, reason, new);

/// The answer of one catch-up run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunDue {
    /// The day it ran for.
    pub as_of: CalendarDate,
    /// The run (send it again to recover a lost reply).
    pub run_id: RunId,
    /// More due occurrences wait: call again with the same run id.
    pub more: bool,
    /// What this run (and earlier calls with its id) posted.
    pub posted: Vec<PostedOccurrence>,
    /// Due reminders.
    pub reminders: Vec<DueReminder>,
    /// Every open skipped occurrence of the company.
    pub skipped: Vec<SkippedOccurrence>,
    /// Templates that could not be checked.
    pub failed: Vec<FailedTemplate>,
}

/// Due reminders and open skipped occurrences (no side effects).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    /// Due reminders.
    pub reminders: Vec<DueReminder>,
    /// Open skipped occurrences.
    pub skipped: Vec<SkippedOccurrence>,
}

/// A fresh run id (keep it until the run's `more` loop ends).
#[must_use]
pub fn new_run_id() -> RunId {
    RunId::from_uuid(uuid::Uuid::new_v4())
}

fn bad() -> ApiError {
    ApiError::Decode {
        what: "catch-up run",
    }
}

fn s<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn date(v: &Value, key: &str) -> Result<CalendarDate, ApiError> {
    s(v, key)
        .and_then(|d| CalendarDate::parse_wire(d).ok())
        .ok_or_else(bad)
}

fn id<T: std::str::FromStr>(v: &Value, key: &str) -> Result<T, ApiError> {
    s(v, key).and_then(|x| x.parse().ok()).ok_or_else(bad)
}

/// A display amount: an exact string, or nothing (a number is refused).
fn total(v: &Value) -> Result<Option<Money>, ApiError> {
    match v.get("total") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(x)) => Money::parse_wire(x).map(Some).map_err(|_| bad()),
        Some(_) => Err(bad()),
    }
}

fn list<T>(
    v: &Value,
    key: &str,
    one: impl Fn(&Value) -> Result<T, ApiError>,
) -> Result<Vec<T>, ApiError> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(one).collect(),
        Some(_) => Err(bad()),
    }
}

fn kind(v: &Value) -> TemplateKind {
    TemplateKind::from_wire(s(v, "kind").unwrap_or("legacy"))
}

fn reminder(v: &Value) -> Result<DueReminder, ApiError> {
    let dates = list(v, "dueDates", |d| {
        d.as_str()
            .and_then(|d| CalendarDate::parse_wire(d).ok())
            .ok_or_else(bad)
    })?;
    Ok(DueReminder {
        template: id(v, "templateId")?,
        name: s(v, "templateName").unwrap_or_default().to_string(),
        kind: kind(v),
        client_form: s(v, "clientForm").map(str::to_string),
        count: v
            .get("dueCount")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(u32::try_from(dates.len()).unwrap_or(u32::MAX)),
        dates,
        total: total(v)?,
    })
}

fn skipped(v: &Value) -> Result<SkippedOccurrence, ApiError> {
    Ok(SkippedOccurrence {
        template: id(v, "templateId")?,
        name: s(v, "templateName").unwrap_or_default().to_string(),
        date: date(v, "occurrenceDate")?,
        reason: SkipReason::from_wire(s(v, "reason").unwrap_or_default()),
        new: v.get("new").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// Reads a run-due answer.
///
/// # Errors
/// `Decode` for an unknown shape or a JSON number amount.
pub fn run_due_from_json(body: &[u8]) -> Result<RunDue, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| bad())?;
    Ok(RunDue {
        as_of: date(&v, "asOf")?,
        run_id: id(&v, "runId")?,
        more: v.get("more").and_then(Value::as_bool).unwrap_or(false),
        posted: list(&v, "posted", |p| {
            Ok(PostedOccurrence {
                template: id(p, "templateId")?,
                name: s(p, "templateName").unwrap_or_default().to_string(),
                kind: kind(p),
                client_form: s(p, "clientForm").map(str::to_string),
                date: date(p, "occurrenceDate")?,
                transaction: id(p, "transactionId")?,
                total: total(p)?,
            })
        })?,
        reminders: list(&v, "reminders", reminder)?,
        skipped: list(&v, "skipped", skipped)?,
        failed: list(&v, "failed", |f| {
            Ok(FailedTemplate {
                template: id(f, "templateId")?,
                reason: s(f, "reason").unwrap_or("INTERNAL").to_string(),
            })
        })?,
    })
}

/// Reads an attention answer.
///
/// # Errors
/// `Decode` for an unknown shape.
pub fn attention_from_json(body: &[u8]) -> Result<Attention, ApiError> {
    let v: Value = serde_json::from_slice(body).map_err(|_| bad())?;
    Ok(Attention {
        reminders: list(&v, "reminders", reminder)?,
        skipped: list(&v, "skipped", skipped)?,
    })
}

/// The most template ids one run filter carries (the engine's limit).
pub const RUN_FILTER_MAX: usize = 500;

/// Which automatic templates one catch-up run handles (U-82,
/// `run-due-filter-v1`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RunDueFilter {
    /// Every due template (the plain run; no capability needed).
    #[default]
    All,
    /// Only these (`templateIds`, 1 to 500).
    Only(Vec<TemplateId>),
    /// All but these (`excludeTemplateIds`, up to 500; empty is `All`).
    Except(Vec<TemplateId>),
}

impl RunDueFilter {
    /// The body field and its ids, `None` for the plain run.
    fn wire(&self) -> Option<(&'static str, &[TemplateId])> {
        match self {
            RunDueFilter::All => None,
            RunDueFilter::Except(ids) if ids.is_empty() => None,
            RunDueFilter::Only(ids) => Some(("templateIds", ids)),
            RunDueFilter::Except(ids) => Some(("excludeTemplateIds", ids)),
        }
    }
}

/// Catch-up calls of a session (`session.recurring()`).
#[derive(Clone, Copy, Debug)]
pub struct Recurring<'a>(pub(crate) &'a Session);

impl Session {
    /// Recurring catch-up of the open company.
    #[must_use]
    pub fn recurring(&self) -> Recurring<'_> {
        Recurring(self)
    }
}

impl Recurring<'_> {
    fn gate(self) -> Result<(), ApiError> {
        if self
            .0
            .client()
            .capabilities()
            .has(Capabilities::RECURRING_RUN_DUE)
        {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::RunDueUnavailable))
        }
    }

    /// Posts the due automatic occurrences up to `as_of` (the local date),
    /// at most `limit` (1–500). Repeat with the same `run_id` while `more`,
    /// and after a lost reply.
    ///
    /// # Errors
    /// `Local(RunDueUnavailable)`, `Local(PageLimit)` (nothing sent);
    /// `Validation` (`RUN_DUE_FUTURE`); `Forbidden`; `Decode`; any
    /// [`ApiError`].
    pub async fn run_due(
        &self,
        as_of: CalendarDate,
        run_id: RunId,
        limit: u32,
    ) -> Result<RunDue, ApiError> {
        self.run_due_filtered(as_of, run_id, limit, &RunDueFilter::All)
            .await
    }

    /// [`Self::run_due`] for some templates only (U-82,
    /// `run-due-filter-v1`): the templates the filter leaves out are not
    /// touched and stay due. [`RunDueFilter::All`] is the plain run.
    ///
    /// # Errors
    /// As [`Self::run_due`], and `Local(RunDueFilterUnavailable)` (nothing
    /// sent) for a filter without `run-due-filter-v1`, an empty "only"
    /// list, or more than 500 ids.
    pub async fn run_due_filtered(
        &self,
        as_of: CalendarDate,
        run_id: RunId,
        limit: u32,
        filter: &RunDueFilter,
    ) -> Result<RunDue, ApiError> {
        self.gate()?;
        if !(1..=RUN_LIMIT_MAX).contains(&limit) {
            return Err(ApiError::Local(LocalValidation::PageLimit));
        }
        let mut body = json!({
            "asOf": as_of.to_wire(),
            "runId": run_id,
            "limit": limit,
        });
        if let Some((key, ids)) = filter.wire() {
            let filtered = self
                .0
                .client()
                .capabilities()
                .has(Capabilities::RUN_DUE_FILTER);
            if !filtered || ids.len() > RUN_FILTER_MAX || (key == "templateIds" && ids.is_empty()) {
                return Err(ApiError::Local(LocalValidation::RunDueFilterUnavailable));
            }
            body[key] = json!(ids);
        }
        let mut call =
            Call::at(Method::Post, RECURRING_RUN_DUE.route()).json_body(json_body(&body)?);
        // The run id is the real guard; the key lets the engine's
        // idempotency layer answer a replay at once.
        call.idempotency_key = Some(IdempotencyKey::new());
        let reply = self.0.send(&RECURRING_RUN_DUE, call, None).await?;
        run_due_from_json(&reply.body)
    }

    /// Due reminders and open skipped occurrences (no side effects).
    ///
    /// # Errors
    /// `Local(RunDueUnavailable)`; `Decode`; any [`ApiError`].
    pub async fn attention(&self) -> Result<Attention, ApiError> {
        self.gate()?;
        let call = Call::at(Method::Get, RECURRING_ATTENTION.route());
        let reply = self.0.send(&RECURRING_ATTENTION, call, None).await?;
        attention_from_json(&reply.body)
    }
}

#[cfg(test)]
#[path = "recurring_tests.rs"]
mod tests;
